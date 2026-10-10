/**
 * Remote access: reach the harness from a phone.
 *
 * Three parts, all inside the dsh process — no native shell involved:
 *
 *  1. An authenticating reverse proxy on its own port, forwarding to dsh's own
 *     web server. The phone authenticates against the proxy with the token
 *     embedded in the link; the proxy then authenticates *itself* to dsh by
 *     exchanging this process's launch token for the authority-bound `dsh-auth`
 *     cookie (the same exchange the desktop shell performs).
 *  2. Two transports. `lan` binds the proxy to all interfaces, so a phone on the
 *     same network connects directly and nothing leaves the machine. `tunnel`
 *     additionally spawns the bundled cloudflared quick tunnel, which makes the
 *     link work from anywhere.
 *  3. Link/QR plumbing: one random 32-byte token is the whole credential, and it
 *     is persisted so a link the phone already saved keeps working across a
 *     restart.
 *
 * Why the proxy has to exist: dsh binds a loopback address and its browser-auth
 * cookie is bound to the authority that minted it, so a remote authority (a
 * trycloudflare hostname, a LAN IP) can never present a valid cookie directly.
 * The proxy terminates the remote authority and re-issues the request to dsh
 * under the loopback authority it holds a cookie for.
 *
 * Security model, identical to the desktop shell's: the link *is* the credential
 * — a 256-bit random token, compared in constant time, exchanged on first use
 * for an HttpOnly cookie. The token is never logged, and disabling revokes it.
 *
 * @module dsh-desktop-kit/remote/remote
 */
import { spawn } from "node:child_process";
import { randomBytes, timingSafeEqual } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createServer, request as httpRequest } from "node:http";
import { connect as netConnect } from "node:net";
import { networkInterfaces } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { respond as respondEnhanced } from "./enhance.js";
import { handleProjectRoute, PROJECT_API_PREFIX, PROJECT_PREFIX } from "./project.js";
import { bridgeSockets } from "./wsbridge.js";

/** Cookie the proxy issues to a remote browser. */
const GATE_COOKIE = "__dsh_remote";
/** Cookie lifetime, matching the desktop shell's 30-day session. */
const GATE_MAX_AGE_SECONDS = 30 * 24 * 60 * 60;
/** Bodies up to this size are buffered so a stale upstream cookie can be retried. */
const BUFFER_LIMIT_BYTES = 4 * 1024 * 1024;
/** Paths that are always streamed, because they carry large uploads. */
const STREAM_PATH_HINT = "uploadFileBinary";
/** How long cloudflared may take to report a tunnel URL. */
const TUNNEL_URL_TIMEOUT_MS = 60_000;
/** Restart backoff for a crashed tunnel. */
const TUNNEL_RETRY_MIN_MS = 500;
const TUNNEL_RETRY_MAX_MS = 30_000;
/** Hop-by-hop headers that must not be forwarded. */
const HOP_BY_HOP = new Set(["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"]);
/** Browser markers dsh's fence reads; from dsh's point of view the proxy is the origin. */
const FENCE_MARKERS = ["origin", "referer", "sec-fetch-site", "sec-fetch-mode", "sec-fetch-dest", "sec-fetch-user"];
/** Vendored mobile/splash/project assets served by the response-side transforms. */
const ASSETS_DIR = join(dirname(fileURLToPath(import.meta.url)), "assets");

/**
 * Load the vendored enhancement assets once. A packaging mistake (missing
 * file) degrades the proxy to plain passthrough — it must never 502.
 * @returns `{ mobileCss, mobileJs, splashCss, splashJs, projectPage } | undefined`.
 */
function createAssetLoader(log) {
	let cache;
	return () => {
		cache ??= (async () => {
			try {
				const [mobileCss, mobileJs, splashCss, splashJs, projectPage] = await Promise.all([
					readFile(join(ASSETS_DIR, "mobile.css"), "utf8"),
					readFile(join(ASSETS_DIR, "mobile.js"), "utf8"),
					readFile(join(ASSETS_DIR, "splash.css"), "utf8"),
					readFile(join(ASSETS_DIR, "splash.js"), "utf8"),
					readFile(join(ASSETS_DIR, "project.html"), "utf8")
				]);
				return { mobileCss, mobileJs, splashCss, splashJs, projectPage };
			} catch (cause) {
				log(`remote assets unavailable, running without enhancements: ${String(cause?.message ?? cause)}`);
				return undefined;
			}
		})();
		return cache;
	};
}

/** A fresh 256-bit link credential, lowercase hex. */
export function generateToken() {
	return randomBytes(32).toString("hex");
}

/** Constant-time token comparison. */
function tokenEquals(actual, expected) {
	if (typeof actual !== "string" || typeof expected !== "string" || actual.length !== expected.length || actual.length === 0) return false;
	return timingSafeEqual(Buffer.from(actual, "utf8"), Buffer.from(expected, "utf8"));
}

/**
 * Interface names that are almost never the address a phone should use:
 * hypervisor switches, container/WSL bridges, VPN overlays, Bluetooth PAN and
 * capture drivers. They all report plausible-looking IPv4 addresses, so a raw
 * address list puts a Docker bridge first and the real Wi-Fi address last.
 */
const VIRTUAL_INTERFACE = /vEthernet|Hyper-V|VMware|VirtualBox|WSL|Docker|Loopback|Tailscale|ZeroTier|Wintun|WireGuard|Bluetooth|Npcap|TAP-Windows/iu;

/** Rank a candidate address: home/office LANs first, container bridges last. */
function lanRank(address) {
	const octets = address.split(".").map(Number);
	if (octets[0] === 192 && octets[1] === 168) return 0;
	if (octets[0] === 10) return 1;
	if (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31) return 3;
	return 2;
}

/**
 * IPv4 addresses a phone on the same network could reach, best candidate first.
 * Link-local (169.254.x) addresses are dropped: they are APIPA placeholders that
 * nothing routes to.
 * @returns ordered addresses.
 */
export function lanAddresses() {
	const physical = [];
	const virtual = [];
	for (const [name, list] of Object.entries(networkInterfaces())) {
		for (const iface of list ?? []) {
			if (iface.family !== "IPv4" || iface.internal || iface.address.startsWith("169.254.")) continue;
			(VIRTUAL_INTERFACE.test(name) ? virtual : physical).push(iface.address);
		}
	}
	const byRank = (a, b) => lanRank(a) - lanRank(b);
	physical.sort(byRank);
	virtual.sort(byRank);
	return [...physical, ...virtual];
}

/**
 * Locate the cloudflared binary the installer ships next to the bundled node.
 * @param override - explicit path from settings, if any.
 * @param dshHome - active DSH home, used to find the shell's persistent copy.
 * @returns the first candidate that exists.
 */
export function cloudflaredCandidates(override, dshHome, env = process.env, execPath = process.execPath, platform = process.platform) {
	const name = platform === "win32" ? "cloudflared.exe" : "cloudflared";
	const candidates = [];
	if (typeof override === "string" && override.length > 0) candidates.push(override);
	// `process.execPath` is the runtime's node.exe under both the shell and the installer.
	candidates.push(join(dirname(execPath), name));
	// The shell keeps a copy outside its install dir so a tunnel outlives the app.
	if (typeof dshHome === "string" && dshHome.length > 0) candidates.push(join(dirname(dshHome), "tunnel", name));
	// A hand-installed cloudflared is on PATH, which is where a user can reasonably be
	// expected to have put it: the shell's own bundled copy is not the only answer.
	for (const dir of String(env.PATH ?? "").split(delimiter)) {
		if (dir.length > 0) candidates.push(join(dir, name));
	}
	if (platform === "win32") {
		if (typeof env.LOCALAPPDATA === "string" && env.LOCALAPPDATA.length > 0) {
			candidates.push(join(env.LOCALAPPDATA, "Microsoft", "WinGet", "Links", name));
		}
		for (const root of [env.ProgramFiles, env["ProgramFiles(x86)"]]) {
			if (typeof root === "string" && root.length > 0) candidates.push(join(root, "cloudflared", name));
		}
	}
	return candidates;
}

/** Locate the cloudflared binary, preferring the setting and then the real installs. */
export function findCloudflared(override, dshHome, exists = existsSync) {
	return cloudflaredCandidates(override, dshHome).find((candidate) => exists(candidate));
}

/**
 * Pull the quick-tunnel URL out of a cloudflared log chunk. cloudflared prints it
 * inside a boxed banner, so the match is anchored on the hostname shape rather
 * than on line structure.
 * @param text - a chunk of cloudflared's stdout or stderr.
 * @returns the tunnel URL, or undefined when this chunk does not carry one.
 */
export function parseTunnelUrl(text) {
	return /https:\/\/[a-z0-9-]+\.trycloudflare\.com/u.exec(String(text))?.[0];
}

/**
 * Supervise a cloudflared quick tunnel: spawn it, read the URL off its log, and
 * restart with backoff while the intent to run is still set.
 * @param options - binary path, local target and a logger.
 * @returns start/stop handle plus a status snapshot.
 */
function createTunnel({ exe, target, log }) {
	let child;
	let url = "";
	let state = "off";
	let error;
	let failures = 0;
	let retryTimer;
	let stopping = true;
	const listeners = new Set();

	const status = () => ({ state, url, error });
	const notify = () => {
		for (const listener of listeners) listener(status());
	};

	const scheduleRetry = () => {
		if (stopping) return;
		failures += 1;
		const delay = Math.min(TUNNEL_RETRY_MAX_MS, TUNNEL_RETRY_MIN_MS * 2 ** (failures - 1));
		log(`tunnel restarting in ${String(delay)}ms (failure ${String(failures)})`);
		retryTimer = setTimeout(start, delay);
		retryTimer.unref?.();
	};

	function start() {
		if (stopping || child !== undefined) return;
		state = "starting";
		error = undefined;
		notify();
		// http2 is pinned on purpose: the desktop shell found QUIC/UDP gets killed by
		// some carriers, which shows up as a tunnel that dies mid-session.
		child = spawn(exe, ["tunnel", "--url", target, "--no-autoupdate", "--protocol", "http2"], {
			windowsHide: true,
			stdio: ["ignore", "pipe", "pipe"]
		});
		const deadline = setTimeout(() => {
			if (state === "starting") {
				error = "cloudflared did not report a tunnel url";
				log(error);
				stopChild();
				scheduleRetry();
			}
		}, TUNNEL_URL_TIMEOUT_MS);
		deadline.unref?.();
		const readChunk = (chunk) => {
			const found = parseTunnelUrl(chunk);
			if (found === undefined || url !== "") return;
			url = found;
			state = "up";
			error = undefined;
			failures = 0;
			clearTimeout(deadline);
			log("tunnel up");
			notify();
		};
		child.stdout.on("data", readChunk);
		child.stderr.on("data", readChunk);
		child.on("exit", (code) => {
			clearTimeout(deadline);
			child = undefined;
			if (stopping) return;
			state = "error";
			error = `cloudflared exited (${String(code)})`;
			notify();
			scheduleRetry();
		});
	}

	/** Kill the current child without touching the run intent. */
	function stopChild() {
		if (child === undefined) return;
		const victim = child;
		child = undefined;
		try {
			victim.kill();
		} catch {
			/* already gone */
		}
	}

	function stop() {
		stopping = true;
		clearTimeout(retryTimer);
		stopChild();
		state = "off";
		url = "";
		error = undefined;
		notify();
	}

	return {
		start() {
			stopping = false;
			failures = 0;
			start();
		},
		stop,
		status,
		onChange(listener) {
			listeners.add(listener);
			return () => listeners.delete(listener);
		}
	};
}

/**
 * The remote-access manager: owns the proxy, the tunnel and the token.
 * @param options - context, path layout, settings accessor/saver and a logger.
 */
export function createRemoteManager({ ctx, layout, config, save, log }) {
	let server;
	let tunnel;
	let authCookie;
	let state = "off";
	let error;
	let link;
	const listeners = new Set();
	const loadAssets = createAssetLoader(log);

	/**
	 * Settings snapshot. The gate runs on every request and on every WebSocket
	 * upgrade, so reads must be synchronous; the manager refreshes this cache
	 * whenever it loads or writes settings instead.
	 */
	let cachedSettings;
	const settings = () => cachedSettings ?? { enabled: false, mode: "tunnel", port: 0, token: "", cloudflaredPath: "" };
	/** Reload the snapshot from the kit's settings store. */
	async function refreshSettings() {
		cachedSettings = (await config()).remote;
		return cachedSettings;
	}
	/** Persist a patch and refresh the snapshot. */
	async function persist(patch) {
		await save({ remote: patch });
		return refreshSettings();
	}

	/** Proxy port, or undefined while stopped. */
	const proxyPort = () => server?.address()?.port;

	/** LAN links for the current proxy port, when it is reachable that way. */
	function lanUrls() {
		const port = proxyPort();
		const token = settings().token;
		if (port === undefined || typeof token !== "string" || token.length === 0) return [];
		return lanAddresses().map((address) => `http://${address}:${String(port)}/?token=${token}`);
	}

	const snapshot = () => ({
		state,
		enabled: server !== undefined,
		mode: settings().mode,
		link,
		proxyPort: proxyPort(),
		lanUrls: lanUrls(),
		tunnel: tunnel?.status(),
		// null rather than undefined so the field always survives JSON and the panel
		// can tell "not found" from "not reported".
		cloudflared: findCloudflared(settings().cloudflaredPath, layout().dshHome) ?? null,
		error
	});

	const publish = () => {
		for (const listener of listeners) listener(snapshot());
	};

	/** This dsh web server's loopback port and authority. */
	function upstream() {
		const port = ctx.get("webServer")?.port;
		if (port === undefined) throw new Error("webServer service is not available");
		return { port, authority: `127.0.0.1:${String(port)}` };
	}

	/**
	 * Exchange this process's launch token for the authority-bound cookie.
	 * `authenticatedUrl` is the documented way to read the token back out; the
	 * `browserAuth.launchToken` field is the fallback.
	 */
	async function refreshAuthCookie() {
		const { port, authority } = upstream();
		const connection = ctx.get("connection");
		if (connection === undefined) throw new Error("connection service is not available");
		let token;
		try {
			token = new URL(connection.authenticatedUrl(`http://${authority}`)).searchParams.get("token");
		} catch {
			token = undefined;
		}
		token ??= connection.browserAuth?.launchToken;
		if (typeof token !== "string" || token.length === 0) throw new Error("no launch token is available for this harness process");
		const response = await fetch(`http://127.0.0.1:${String(port)}/?token=${encodeURIComponent(token)}`, { redirect: "manual" });
		const cookie = (response.headers.getSetCookie?.() ?? [])[0];
		if (typeof cookie !== "string" || cookie.length === 0) throw new Error(`launch-token exchange failed (status ${String(response.status)})`);
		authCookie = cookie.split(";", 1)[0];
		return authCookie;
	}

	/**
	 * Headers for an upstream request: loopback authority, our cookie, and no
	 * browser markers. Compression is dropped so responses stay identity-encoded
	 * and the proxy never has to reason about content encoding.
	 * @param req - the remote request.
	 * @param options - `upgrade` keeps `connection`/`upgrade`, which are hop-by-hop
	 *   for an ordinary request but are exactly what makes an upgrade an upgrade.
	 */
	function upstreamHeaders(req, options = {}) {
		const headers = {};
		for (const [key, value] of Object.entries(req.headers)) {
			const lower = key.toLowerCase();
			if (lower === "accept-encoding" || lower === "cookie" || lower === "host") continue;
			if (FENCE_MARKERS.includes(lower)) continue;
			if (HOP_BY_HOP.has(lower) && !(options.upgrade === true && (lower === "connection" || lower === "upgrade"))) continue;
			headers[key] = value;
		}
		headers.host = upstream().authority;
		headers.cookie = authCookie;
		return headers;
	}

	/** Read one cookie value out of a Cookie header. */
	function gateCookie(header) {
		if (typeof header !== "string") return undefined;
		for (const part of header.split(";")) {
			const at = part.indexOf("=");
			if (at !== -1 && part.slice(0, at).trim() === GATE_COOKIE) return part.slice(at + 1).trim();
		}
		return undefined;
	}

	/** Whether a remote request carries the link token or the gate cookie. */
	function admitted(req) {
		const expected = settings().token;
		const url = new URL(req.url ?? "/", "http://remote.invalid");
		const presented = url.searchParams.get("token");
		if (presented !== null) return { ok: tokenEquals(presented, expected), presented: true, url };
		return { ok: tokenEquals(gateCookie(req.headers.cookie), expected), presented: false, url };
	}

	/** Refuse a request that presented no usable credential. */
	async function refuse(res, slow) {
		// A wrong token waits, so the gate cannot be probed for free.
		if (slow) await new Promise((resolve) => setTimeout(resolve, 500));
		res.writeHead(403, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
		res.end("<!doctype html><meta charset=utf-8><title>dsh remote</title><p><b>403</b> — open the full link, including its <code>?token=</code>, as shown in the Desktop Kit panel.</p>");
	}

	/**
	 * Forward one HTTP request. The response side runs through the enhancement
	 * pipeline (HTML rewrite / gzip / SSE keepalive), which owns `res`.
	 * @param req - the remote request.
	 * @param res - the remote response.
	 * @param replay - buffered body to resend after a re-authentication retry.
	 */
	function forward(req, res, replay) {
		const { port } = upstream();
		const streaming = replay === undefined && (Number(req.headers["content-length"] ?? "0") > BUFFER_LIMIT_BYTES || String(req.url ?? "").includes(STREAM_PATH_HINT));
		const headers = upstreamHeaders(req);

		/** Hand the upstream response to the enhancement pipeline. */
		const answer = (upstreamRes) => {
			loadAssets()
				.then((assets) => respondEnhanced({ req, res, upstreamRes, assets, log }))
				.catch((cause) => fail(res, cause));
		};

		if (streaming) {
			const upstreamReq = httpRequest({ host: "127.0.0.1", port, method: req.method, path: req.url, headers }, answer);
			upstreamReq.on("error", (cause) => fail(res, cause));
			req.pipe(upstreamReq);
			return;
		}

		const chunks = [];
		let size = 0;
		req.on("data", (chunk) => {
			chunks.push(chunk);
			size += chunk.length;
			// A declared length smaller than the stream means we cannot trust the buffer.
			if (size > BUFFER_LIMIT_BYTES && replay === undefined) req.destroy();
		});
		req.on("error", (cause) => fail(res, cause));
		req.on("end", () => {
			const body = replay ?? Buffer.concat(chunks);
			const requestHeaders = { ...headers, "content-length": String(body.length) };
			const upstreamReq = httpRequest({ host: "127.0.0.1", port, method: req.method, path: req.url, headers: requestHeaders }, (upstreamRes) => {
				if (upstreamRes.statusCode === 401 && replay === undefined) {
					upstreamRes.resume();
					log("upstream rejected the session cookie; re-exchanging and replaying once");
					refreshAuthCookie()
						.then(() => forward(req, res, body))
						.catch((cause) => fail(res, new Error(`re-authentication failed: ${String(cause?.message ?? cause)}`)));
					return;
				}
				answer(upstreamRes);
			});
			upstreamReq.on("error", (cause) => fail(res, cause));
			if (body.length > 0) upstreamReq.write(body);
			upstreamReq.end();
		});
	}

	/** Report an upstream failure on a response that has not been written yet. */
	function fail(res, cause) {
		const message = `dsh remote: upstream error: ${String(cause?.message ?? cause)}`;
		if (!res.headersSent) res.writeHead(502, { "content-type": "text/plain; charset=utf-8" }).end(message);
		else res.end();
	}

	/**
	 * Forward a WebSocket upgrade. The request line and headers are re-emitted
	 * against the loopback authority with our cookie; once connected, the two
	 * sockets are bridged at frame level so the proxy can inject an idle Ping
	 * without ever splitting a frame.
	 */
	function forwardUpgrade(req, socket, head) {
		const { port } = upstream();
		const headers = upstreamHeaders(req, { upgrade: true });
		const lines = [`${req.method ?? "GET"} ${req.url ?? "/"} HTTP/1.1`];
		for (const [key, value] of Object.entries(headers)) {
			if (value === undefined) continue;
			lines.push(`${key}: ${Array.isArray(value) ? value.join(", ") : String(value)}`);
		}
		const upstreamSocket = netConnect(port, "127.0.0.1", () => {
			upstreamSocket.write(`${lines.join("\r\n")}\r\n\r\n`);
			if (head !== undefined && head.length > 0) upstreamSocket.write(head);
			bridgeSockets(socket, upstreamSocket, { log });
		});
		const close = () => {
			socket.destroy();
			upstreamSocket.destroy();
		};
		socket.on("error", close);
		upstreamSocket.on("error", close);
		socket.on("close", close);
		upstreamSocket.on("close", close);
	}

	/** Route one proxied request. */
	async function handle(req, res) {
		const verdict = admitted(req);
		if (!verdict.ok) {
			await refuse(res, verdict.presented);
			return;
		}
		if (verdict.presented) {
			// Strip the token from the address bar and remember it in a cookie, so the
			// phone's later requests and its WebSocket upgrades are authenticated.
			const rest = new URLSearchParams(verdict.url.search);
			rest.delete("token");
			const query = rest.toString();
			res.writeHead(303, {
				location: query === "" ? verdict.url.pathname : `${verdict.url.pathname}?${query}`,
				"cache-control": "no-store",
				"referrer-policy": "no-referrer",
				"set-cookie": `${GATE_COOKIE}=${settings().token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${String(GATE_MAX_AGE_SECONDS)}`
			});
			res.end();
			return;
		}
		// The kit's own read-only routes (phone project browser) sit behind the
		// same gate; they answer locally instead of being forwarded to dsh.
		const path = verdict.url.pathname;
		if ((req.method ?? "GET") === "GET" && (path === PROJECT_PREFIX || path.startsWith(`${PROJECT_API_PREFIX}/`))) {
			const assets = await loadAssets();
			if (assets?.projectPage !== undefined) {
				await handleProjectRoute(req, res, verdict.url, { workspaceStorePath: layout().workspaceStorePath, page: assets.projectPage });
				return;
			}
		}
		forward(req, res);
	}

	/** Start the proxy, and in tunnel mode bring up cloudflared. */
	async function enable(mode) {
		const wanted = mode === "lan" ? "lan" : "tunnel";
		if (server !== undefined) {
			// Already running: an enable that asks for the other transport means switch,
			// not "keep what is there".
			if (settings().mode === wanted) return snapshot();
			return setMode(wanted);
		}
		const current = await refreshSettings();
		const token = typeof current.token === "string" && current.token.length > 0 ? current.token : generateToken();
		await persist({ token, enabled: true, mode: wanted });
		// Must happen before the proxy accepts a request, or the first proxied
		// request would go upstream without a session cookie.
		await refreshAuthCookie();

		const listening = createServer((req, res) => {
			handle(req, res).catch((cause) => fail(res, cause));
		});
		listening.on("upgrade", (req, socket, head) => {
			if (!admitted(req).ok) {
				socket.destroy();
				return;
			}
			forwardUpgrade(req, socket, head);
		});
		const bind = wanted === "lan" ? "0.0.0.0" : "127.0.0.1";
		/**
		 * Bind, reusing the remembered port so a shared link keeps working after a
		 * restart. A port taken by something else falls back to an ephemeral one
		 * rather than refusing to start.
		 */
		const listen = (port) =>
			new Promise((resolve, reject) => {
				const onError = (error) => {
					if (port !== 0 && error.code === "EADDRINUSE") {
						listening.removeListener("error", onError);
						log(`remote port ${String(port)} is taken; falling back to an ephemeral port`);
						listen(0).then(resolve, reject);
						return;
					}
					reject(error);
				};
				listening.once("error", onError);
				listening.listen(port, bind, () => {
					listening.removeListener("error", onError);
					resolve();
				});
			});
		await listen(current.port ?? 0);
		server = listening;
		const port = listening.address().port;
		state = "up";
		error = undefined;
		link = wanted === "lan" ? (lanUrls()[0] ?? undefined) : undefined;
		if (port !== current.port) await persist({ port });
		log(`remote proxy listening on ${bind}:${String(port)}`);
		publish();

		if (wanted === "tunnel") {
			const exe = findCloudflared(current.cloudflaredPath, layout().dshHome);
			if (exe === undefined) {
				state = "error";
				error = "cloudflared binary not found — set remote.cloudflaredPath, or switch to LAN mode";
				log(error);
				publish();
			} else {
				tunnel = createTunnel({ exe, target: `http://127.0.0.1:${String(port)}`, log });
				tunnel.onChange((tunnelStatus) => {
					state = tunnelStatus.state === "up" ? "up" : tunnelStatus.state === "off" ? "off" : "starting";
					error = tunnelStatus.error;
					if (tunnelStatus.url !== "") link = `${tunnelStatus.url}/?token=${settings().token}`;
					publish();
				});
				tunnel.start();
			}
		}
		return snapshot();
	}

	/** Stop everything and revoke the credential. */
	async function disable() {
		tunnel?.stop();
		tunnel = undefined;
		if (server !== undefined) {
			const victim = server;
			server = undefined;
			await new Promise((resolve) => victim.close(() => resolve()));
		}
		authCookie = undefined;
		state = "off";
		error = undefined;
		link = undefined;
		await persist({ enabled: false });
		log("remote access disabled");
		publish();
		return snapshot();
	}

	/** Switch transport, keeping the token so a saved link keeps working. */
	async function setMode(mode) {
		const next = mode === "lan" ? "lan" : "tunnel";
		const wasEnabled = server !== undefined;
		if (wasEnabled) await disable();
		await persist({ mode: next });
		if (wasEnabled) return enable(next);
		state = "off";
		publish();
		return snapshot();
	}

	return {
		snapshot,
		/** Reload the synchronous settings snapshot from the settings store. */
		sync: refreshSettings,
		onStatus(listener) {
			listeners.add(listener);
			return () => listeners.delete(listener);
		},
		enable,
		disable,
		setMode,
		/** Issue a fresh credential; every link shared before this stops working. */
		async resetToken() {
			const wasEnabled = server !== undefined;
			const mode = settings().mode;
			if (wasEnabled) await disable();
			await persist({ token: generateToken(), enabled: false });
			log("remote token reset");
			if (wasEnabled) return enable(mode);
			publish();
			return snapshot();
		},
		refresh: refreshAuthCookie,
		cloudflared: () => findCloudflared(settings().cloudflaredPath, layout().dshHome)
	};
}
