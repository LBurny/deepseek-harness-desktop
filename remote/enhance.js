/**
 * Response-side enhancements for the remote-access proxy.
 *
 * dsh serves everything identity-encoded and desktop-shaped. For a phone on a
 * tunnel that meant three concrete problems the desktop shell already solved
 * (see `src-tauri/src/remote/proxy.rs`); this module ports the same three
 * transforms, applied only to traffic passing through the proxy:
 *
 *  1. HTML rewrite — the SPA entry document gets a no-zoom viewport meta
 *     (iOS WKWebView otherwise auto-zooms on the <16px composer and never
 *     restores), the mobile adaptation stylesheet/script, and a splash
 *     overlay so the multi-megabyte first load shows progress instead of a
 *     white screen. Every rewrite is needle-anchored: if a dsh update changes
 *     the document shape, the transform silently skips and the page falls
 *     back to the un-adapted — but fully working — original.
 *  2. gzip — responses of compressible types at ≥4 KiB are buffered and
 *     gzipped when the client accepts it. dsh compresses nothing, so the
 *     tunnel first load otherwise downloads ~5 MB identity.
 *  3. SSE keepalive — an idle `text/event-stream` gets a `: keepalive`
 *     comment line every 30 s of silence, inside the SSE spec (EventSource
 *     ignores comments), so the tunnel's ~100 s idle kill never trips.
 *
 * @module dsh-desktop-kit/remote/enhance
 */
import { gzipSync } from "node:zlib";

/** Buffering bound for any transform; larger bodies pass through untouched. */
export const REWRITE_BUFFER_LIMIT = 16 * 1024 * 1024;
/** Below this, gzip costs more in CPU and first-byte latency than it saves. */
export const GZIP_MIN_SIZE = 4 * 1024;
/** Idle silence before an SSE stream gets a keepalive comment. */
export const SSE_IDLE_HEARTBEAT_MS = 30_000;

/** iOS zoom fix: viewport needle → no-zoom replacement (from the shell's upstream.rs). */
const VIEWPORT_META_NEEDLE = Buffer.from('content="width=device-width, initial-scale=1"', "utf8");
const VIEWPORT_META_REPLACEMENT = Buffer.from('content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no"', "utf8");
/** React mount point of the SPA entry document (Vite template, verbatim). */
const SPA_ROOT_MOUNT_NEEDLE = Buffer.from('<div id="root"></div>', "utf8");
/** Marker comment so tests and debugging can tell an enhanced document. */
const INJECT_MARKER = "<!-- dshdesk-mobile -->";

/** Splash overlay markup; the unloader script follows immediately after it. */
const SPLASH_HTML =
	'<div id="dsh-splash" role="status"><div class="dsh-splash-spinner"></div>' +
	'<p class="dsh-splash-title">DeepSeek Harness</p><p class="dsh-splash-hint"></p></div>';

/** Content types worth compressing (already-compressed formats are excluded). */
const GZIP_TYPES = new Set(["application/javascript", "text/javascript", "text/css", "application/json", "image/svg+xml", "text/html"]);

/** Strip parameters and compare case-insensitively. */
const baseType = (contentType) => (typeof contentType === "string" ? contentType.split(";", 1)[0].trim().toLowerCase() : "");

/** Whether the response declares an event stream. */
export function isEventStream(contentType) {
	return baseType(contentType) === "text/event-stream";
}

/** Whether the response is an HTML document. */
export function isHtmlDocument(contentType) {
	return baseType(contentType) === "text/html";
}

/** Whether the type is worth proxy-side compression. */
export function gzipEligibleType(contentType) {
	return GZIP_TYPES.has(baseType(contentType));
}

/**
 * Whether the remote browser accepts gzip — read from the ORIGINAL request
 * headers, before the forward path strips `accept-encoding`.
 * @param requestHeaders - `req.headers` of the remote request.
 */
export function clientAcceptsGzip(requestHeaders) {
	const value = requestHeaders["accept-encoding"];
	if (typeof value !== "string") return false;
	return value.split(",").some((token) => token.trim().startsWith("gzip"));
}

/** A document navigation wants HTML rewrites; asset fetches do not. */
export function wantsHtmlDocument(requestHeaders) {
	return typeof requestHeaders.accept === "string" && requestHeaders.accept.includes("text/html");
}

/** Byte-level replace-all; returns `undefined` when the needle never matched. */
function replaceAll(haystack, needle, replacement) {
	let at = haystack.indexOf(needle);
	if (at === -1) return undefined;
	const out = [];
	let from = 0;
	while (at !== -1) {
		out.push(haystack.subarray(from, at), replacement);
		from = at + needle.length;
		at = haystack.indexOf(needle, from);
	}
	out.push(haystack.subarray(from));
	return Buffer.concat(out);
}

/** ASCII case-insensitive indexOf (`</head>` may arrive in any case). */
function indexOfCI(haystack, needle) {
	const lower = haystack.toString("latin1").toLowerCase();
	return lower.indexOf(needle.toLowerCase());
}

/**
 * Apply the HTML transforms to a buffered document. Every step is optional:
 * a needle that misses leaves the document unchanged in that respect, never
 * broken. `assets` carries the vendored mobile/splash files as strings.
 * @param body - the original document.
 * @param assets - `{ mobileCss, mobileJs, splashCss, splashJs }`.
 */
export function rewriteHtmlDocument(body, assets) {
	let doc = replaceAll(body, VIEWPORT_META_NEEDLE, VIEWPORT_META_REPLACEMENT) ?? body;
	const splashReplacement = Buffer.concat([
		SPA_ROOT_MOUNT_NEEDLE,
		Buffer.from(`${SPLASH_HTML}<script>${assets.splashJs}</script>`, "utf8")
	]);
	const withSplash = replaceAll(doc, SPA_ROOT_MOUNT_NEEDLE, splashReplacement);
	const splash = withSplash !== undefined;
	if (splash) doc = withSplash;
	const headEnd = indexOfCI(doc, "</head>");
	if (headEnd === -1) return doc;
	const style = splash ? `${assets.mobileCss}\n${assets.splashCss}` : assets.mobileCss;
	const injection = Buffer.from(`${INJECT_MARKER}<style>${style}</style><script>${assets.mobileJs}</script>`, "utf8");
	return Buffer.concat([doc.subarray(0, headEnd), injection, doc.subarray(headEnd)]);
}

/**
 * Buffer an upstream response up to the transform limit.
 * @returns `{ body }` when complete, or `{ overflow: true, chunks }` when the
 *   body exceeded the limit — the caller then passes the chunks through.
 */
export function bufferUpTo(upstreamRes, limit = REWRITE_BUFFER_LIMIT) {
	return new Promise((resolve, reject) => {
		const chunks = [];
		let size = 0;
		let overflow = false;
		upstreamRes.on("data", (chunk) => {
			if (overflow) return;
			size += chunk.length;
			if (size > limit) {
				overflow = true;
				return;
			}
			chunks.push(chunk);
		});
		upstreamRes.on("end", () => resolve(overflow ? { overflow: true, chunks } : { body: Buffer.concat(chunks) }));
		upstreamRes.on("error", reject);
	});
}

/**
 * Headers for a buffered/transformed response: drop hop-by-hop and every
 * header the transform invalidates (length, digest, ranges) plus dsh's own
 * cookie, which is bound to the loopback authority and must never leave it.
 */
export function transformedHeaders(headers) {
	const out = {};
	for (const [key, value] of Object.entries(headers)) {
		const lower = key.toLowerCase();
		if (["connection", "transfer-encoding", "keep-alive", "upgrade", "content-length", "etag", "accept-ranges", "set-cookie"].includes(lower)) continue;
		out[key] = value;
	}
	return out;
}

/**
 * Forward a response body with SSE keepalive injection. Any upstream chunk
 * resets the timer; after `idleMs` of silence a comment line is emitted.
 * @param upstreamRes - the dsh response (already header-forwarded).
 * @param res - the remote response (headers already written).
 */
export function pipeSseWithKeepalive(upstreamRes, res, idleMs = SSE_IDLE_HEARTBEAT_MS) {
	let done = false;
	const cleanup = () => {
		if (done) return;
		done = true;
		clearInterval(timer);
	};
	const timer = setInterval(() => {
		try {
			res.write(": keepalive\n\n");
		} catch {
			cleanup();
		}
	}, idleMs);
	timer.unref?.();
	upstreamRes.on("data", (chunk) => {
		if (!res.writable) {
			cleanup();
			upstreamRes.destroy();
			return;
		}
		// Silence-triggered: any real event postpones the next keepalive line.
		timer.refresh();
		res.write(chunk);
	});
	upstreamRes.on("end", () => {
		cleanup();
		res.end();
	});
	upstreamRes.on("error", () => {
		cleanup();
		res.end();
	});
	res.on("close", cleanup);
	res.on("error", cleanup);
}

/**
 * The response-side pipeline, one call per proxied request. Owns `res` from
 * here on: picks HTML rewrite / gzip / SSE-wrapped passthrough / plain
 * passthrough, always exactly one terminal write sequence.
 *
 * Decisions match the desktop shell: HTML rewrites apply to 2xx document
 * navigations; gzip needs a GET, a 200, no Range, an eligible type, a known
 * length inside the window, and a client that accepts it. When the assets
 * failed to load (`assets === undefined`) the HTML rewrite silently drops
 * out — passthrough is the correct degradation, never a 502.
 *
 * @param options - `{ req, res, upstreamRes, assets, log }`.
 */
export async function respond({ req, res, upstreamRes, assets, log = () => {} }) {
	const headers = upstreamRes.headers;
	const contentType = headers["content-type"];
	const status = upstreamRes.statusCode ?? 502;
	const method = req.method ?? "GET";
	const encoded = headers["content-encoding"] !== undefined;
	const success = status >= 200 && status < 300;
	const contentLength = Number(headers["content-length"] ?? Number.NaN);

	/** Pass the buffered overflow through without a transform. */
	const passthroughBuffered = (chunks) => {
		res.writeHead(status, passthroughHeaders(headers));
		for (const chunk of chunks) res.write(chunk);
		upstreamRes.pipe(res);
	};

	// HTML document: buffer within the limit and inject the mobile adaptation.
	if (assets !== undefined && success && !encoded && isHtmlDocument(contentType) && wantsHtmlDocument(req.headers)) {
		const buffered = await bufferUpTo(upstreamRes);
		if (buffered.overflow === true) return passthroughBuffered(buffered.chunks);
		const body = rewriteHtmlDocument(buffered.body, assets);
		res.writeHead(status, transformedHeaders(headers));
		res.end(body);
		return;
	}

	// Compressible text assets: buffer and gzip at the proxy.
	if (
		clientAcceptsGzip(req.headers) &&
		method === "GET" &&
		req.headers.range === undefined &&
		status === 200 &&
		!encoded &&
		gzipEligibleType(contentType) &&
		Number.isFinite(contentLength) &&
		contentLength >= GZIP_MIN_SIZE &&
		contentLength <= REWRITE_BUFFER_LIMIT
	) {
		const buffered = await bufferUpTo(upstreamRes);
		if (buffered.overflow === true) return passthroughBuffered(buffered.chunks);
		try {
			const compressed = gzipSync(buffered.body);
			res.writeHead(status, { ...transformedHeaders(headers), "content-encoding": "gzip", vary: "accept-encoding" });
			res.end(compressed);
		} catch (error) {
			// A compression failure must not lose the body: identity fallback.
			log(`gzip failed, serving identity: ${String(error?.message ?? error)}`);
			res.writeHead(status, passthroughHeaders(headers));
			res.end(buffered.body);
		}
		return;
	}

	// Plain passthrough; event streams get the idle keepalive wrapper.
	res.writeHead(status, passthroughHeaders(headers));
	if (isEventStream(contentType)) pipeSseWithKeepalive(upstreamRes, res);
	else upstreamRes.pipe(res);
}

/** Response headers for passthrough: everything except dsh's own cookie. */
function passthroughHeaders(headers) {
	const out = {};
	for (const [key, value] of Object.entries(headers)) {
		if (key.toLowerCase() === "set-cookie") continue;
		out[key] = value;
	}
	return out;
}
