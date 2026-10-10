/**
 * dsh-desktop-kit — the desktop shell's convenience features, delivered as a
 * dsh plugin instead of a native window.
 *
 * Ported from DSHDesktop (github.com/LBurny/deepseek-harness-desktop):
 *   1. skill management        4. page zoom
 *   2. MCP management          5. task reminders (popup + sound)
 *   3. diagnostics             6. shortcut rebinding
 *   7. /init                   7. (already shipped as `dsh-command-init`; kept as a guarded fallback)
 *
 * Shape of the port. The desktop shell got these features from Tauri, a tray
 * icon, a native WebView2 zoom call, WinRT toasts and a reverse proxy. None of
 * that exists inside dsh, so the split is:
 *
 *   host half (index.js + the feature folders)  — owns the filesystem and the harness:
 *     skills/MCP/zoom/notification settings persisted under `$DSH_HOME`,
 *     slash commands, and a small same-origin HTTP API.
 *   browser half (ui/kit.js)  — owns the page:
 *     a launcher + panel UI, CSS page zoom, a keydown recorder for the zoom
 *     shortcuts, and the Notification API + HTMLAudioElement for reminders.
 *
 * The two halves meet at `/dshdesk/*`, and the page loads the browser half
 * through the web server's structured index injection (`webserver/index-inject`)
 * rather than a client-UI bundle: client bundles have no public out-of-tree
 * build path and bind to internal slot names, while an injected script depends
 * only on the DOM and on this plugin's own routes.
 *
 * @module dsh-desktop-kit
 */
import { readFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createUserMessage } from "@deepseek-ai/dsh-llm";

import { createNotifyBus } from "./bus.js";
import { loadConfig, mergeDeep, normalizeConfig, saveConfig } from "./config.js";
import { collectDiagnostics, createLogRing, readLogs } from "./diagnostics.js";
import { guard, readJson, readRaw, sendBuffer, sendJson, sendText } from "./http.js";
import {
	deleteMcpServer,
	importMcpServers,
	listMcpImportable,
	listMcpServers,
	setMcpEnabled,
	upsertMcpServer
} from "./mcp.js";
import { resolveLayout } from "./paths.js";
import { readUiPreferences } from "./preference.js";
import { createQrRenderer } from "./qr.js";
import { createRemoteManager } from "./remote.js";
import {
	deleteSkill,
	importSkills,
	importZipSkills,
	inspectZipSkills,
	listImportable,
	listSkills,
	setSkillEnabled,
	skillStats
} from "./skills.js";
import { SILENT, SYSTEM_DEFAULT, listSounds, readSound } from "./sounds.js";
import { mcpStats } from "./mcp.js";

const name = "desktop-kit";
const inject = ["commands"];
/** Shown in the panel and /api/state; bump alongside package.json. */
const VERSION = "0.2.0";

/** Every route this plugin owns hangs off this prefix. */
const ROUTE_PREFIX = "/dshdesk";
const SCRIPT_PATH = `${ROUTE_PREFIX}/client.js`;
const STYLE_PATH = `${ROUTE_PREFIX}/client.css`;
const SOUND_PATH = `${ROUTE_PREFIX}/sounds/`;
const QR_PATH = `${ROUTE_PREFIX}/remote/qr.svg`;

/** Bundled assets served to the page. */
const ASSETS_DIR = dirname(fileURLToPath(import.meta.url));

/** Read one bundled asset as UTF-8. */
async function readAsset(file) {
	return readFile(join(ASSETS_DIR, file), "utf8");
}

/** Render a list of skills or servers as the one-line-per-item text a command returns. */
function renderRows(rows, empty) {
	if (rows.length === 0) return empty;
	return rows.map((row) => `${row.enabled ? "[x]" : "[ ]"} ${row.name ?? row.serverName}${row.description ? ` — ${row.description}` : ""}${row.summary ? ` — ${row.summary}` : ""}`).join("\n");
}

/**
 * Wrap a command handler so a failure (most often a service this profile does
 * not mount) becomes an error result instead of an exception. A management
 * command should report why it could not run, not break the invocation.
 * @param fn - the real handler body.
 * @returns a handler returning `{kind:"error"}` on failure.
 */
function command(fn) {
	return async (invocation) => {
		try {
			return await fn(invocation);
		} catch (error) {
			return { kind: "error", text: `desktop-kit: ${String(error?.message ?? error)}` };
		}
	};
}

/**
 * Register the slash commands. Each registration is individually guarded: a
 * name clash with another plugin would otherwise throw during `apply` and take
 * the whole plugin tree (i.e. dsh itself) down, so a clash degrades to a
 * warning instead.
 * @param ctx - plugin context carrying the command registry.
 * @param kit - the shared kit state.
 */
function registerCommands(ctx, kit) {
	const add = (definition) => {
		try {
			ctx.commands.register(definition);
		} catch (error) {
			// Command names are first-come, first-served, so losing a name to
			// another plugin is survivable — but it must be visible, both in dsh's
			// log and in the kit's own ring (which the diagnostics panel reads).
			const reason = String(error?.message ?? error);
			ctx.logger.warn(`[desktop-kit] command /${definition.name} not registered: ${reason}`);
			kit.log(`command /${definition.name} not registered: ${reason}`);
		}
	};

	add({
		name: "skills",
		description: "List skills and whether each is enabled (manage them in the desktop-kit panel)",
		handler: command(async () => {
			const layout = kit.requireLayout();
			const skills = await listSkills(layout);
			return { kind: "success", text: `${skills.length} skill(s), ${skills.filter((skill) => skill.enabled).length} enabled\n${renderRows(skills, "(no skills found)")}` };
		})
	});

	add({
		name: "mcp",
		description: "List MCP servers from the active profile's patch layer",
		handler: command(async () => {
			const layout = kit.requireLayout();
			const servers = await listMcpServers(layout);
			return { kind: "success", text: `${servers.length} MCP server(s), ${servers.filter((server) => server.enabled).length} enabled\n${renderRows(servers, "(no MCP servers configured)")}` };
		})
	});

	add({
		name: "doctor",
		description: "Show harness diagnostics: version, port, pid, profile and log tail",
		handler: command(async () => {
			const layout = kit.requireLayout();
			const [skills, mcp] = await Promise.all([skillStats(layout), mcpStats(layout)]);
			const status = await collectDiagnostics({ webServer: kit.webServer(), sessions: kit.sessions(), layout, ring: kit.ring, skills, mcp });
			const lines = [
				`dsh ${status.version} on node ${status.node}`,
				`state=${status.state} port=${status.port} pid=${status.pid} uptime=${status.uptimeSeconds}s rss=${status.memoryMb}MB`,
				`profile=${status.profile} sessions=${status.sessions ?? "?"}`,
				`skills=${skills.enabled}/${skills.total} enabled, mcp=${mcp.enabled}/${mcp.total} enabled`,
				`dsh home: ${status.dshHome}`,
				`patch: ${status.patchPath}`
			];
			const logs = await readLogs(layout, kit.ring);
			const tail = logs.lines.slice(-10);
			if (tail.length > 0) lines.push("", `recent log (${logs.source}):`, ...tail);
			return { kind: "success", text: lines.join("\n") };
		})
	});

	add({
		name: "kit",
		description: "Show the desktop-kit panel endpoint and current settings",
		handler: command(async () => {
			const layout = kit.requireLayout();
			const config = await loadConfig(layout.kitConfigPath);
			return {
				kind: "success",
				text: [
					"The panel is injected into the dsh web UI — open it from the launcher in the corner.",
					`API base: ${ROUTE_PREFIX}/api  (config: ${layout.kitConfigPath})`,
					`zoom=${config.zoom.level} step=${config.zoom.step} sound=${config.notify.sound}`,
					"/init is provided by the dsh-command-init plugin, not by the kit."
				].join("\n")
			};
		})
	});
}

/**
 * The `/init` command body — the same AGENTS.md authoring prompt the
 * `dsh-command-init` plugin ships.
 *
 * It exists here for the case where that plugin is absent, and is registered
 * only when the row config asks for it (`provideInit: true`). Registering it
 * unconditionally would be worse than useless: command names are first-come,
 * first-served, so a kit that claimed `/init` first would deactivate the
 * official plugin (observed as `1 entry did not activate` in the startup log)
 * and the command would silently come from here instead. Deferring to the
 * plugin that owns the name is the correct default.
 */
function initCommandDefinition() {
	return {
		name: "init",
		description: "Create or update this workspace's AGENTS.md instruction file",
		input: { hint: "additional instructions (optional)" },
		handler: (invocation) => {
			const cwd = invocation.agent?.session?.header?.cwd;
			const workspace = typeof cwd === "string" && cwd.length > 0 ? cwd : "<the workspace the agent is running in>";
			const extra = invocation.rawInput.trim();
			const text = `Your task is to create or update a concise workspace instruction file for future AI coding agents.

Target:
- Workspace directory: ${workspace}
- Instruction file: ${workspace}/AGENTS.md
- Use exactly this file name; never write outside the workspace.

Process:
1. Check which instruction files already exist (AGENTS.md, CLAUDE.md, *.local.md).
2. If AGENTS.md exists, update it with targeted edits rather than replacing it wholesale.
3. Inspect the repository with read-only tools before writing.
4. Keep it short: it is injected into every future session in this workspace.
5. Include only project-specific facts — build/test commands, architecture boundaries, and gotchas a newcomer would trip over.
6. Summarize the sections you wrote and mention the file path.

Additional user instructions supplied with /init:
\`\`\`text
${extra.length > 0 ? extra : "(none supplied)"}
\`\`\``;
			invocation.agent.followup(createUserMessage({
				content: [{ type: "text", text }],
				source: { kind: "plugin", plugin: "dsh-desktop-kit", form: "notice", summary: extra.length > 0 ? "Create/update AGENTS.md workspace instructions (+ additional instructions)" : "Create/update AGENTS.md workspace instructions" }
			}));
			return { kind: "success", text: "Prompt submitted to prepare for generating AGENTS.md" };
		}
	};
}

/** ZIP uploads may be at most this large (skill packs are a few MB at most). */
const ZIP_UPLOAD_LIMIT = 64 * 1024 * 1024;
/** Uploaded archives are held in memory for this long between inspect and import. */
const ZIP_STASH_TTL_MS = 10 * 60 * 1000;
/** At most this many uploaded archives are stashed at once. */
const ZIP_STASH_MAX = 4;
/** Page-error reports accepted within one window (the page throttles too). */
const CLIENT_LOG_WINDOW_MS = 10_000;
const CLIENT_LOG_MAX_PER_WINDOW = 5;
/** One client error line may carry at most this much text. */
const CLIENT_LOG_TEXT_LIMIT = 500;

/** Strip credentials from a page-error line before it touches any log. */
function redactSecrets(text) {
	return text.replace(/([?&]token=)[^&\s]+/giu, "$1…").replace(/\b[0-9a-f]{48,}\b/giu, "<hex>");
}

/**
 * Create the shared kit state: layout resolution, settings cache, log ring and
 * notification bus. Layout is resolved lazily because the config-editor service
 * may not be mounted yet when `apply` runs.
 * @param ctx - plugin context.
 * @returns the state object the routes and events read from.
 */
function createKit(ctx) {
	const ring = createLogRing();
	const bus = createNotifyBus();
	/** Errors reported by the browser half, newest last. */
	const clientErrors = createLogRing(100);
	/** Uploaded ZIPs awaiting an import decision: id → {buffer, name, at}. */
	const zipStash = new Map();
	let clientLogCount = 0;
	let clientLogWindowStart = 0;

	/** Cached config, invalidated by every write. */
	let configCache;
	const layout = () => {
		const configEditor = ctx.get("configEditor");
		if (configEditor === undefined) throw new Error("config-editor service is not available in this profile");
		return resolveLayout(configEditor);
	};

	const kit = {
		ring,
		bus,
		clientErrors,
		layout,
		requireLayout: layout,
		webServer: () => ctx.get("webServer"),
		sessions: () => ctx.get("sessions"),
		/** Load settings, memoized until the next save. */
		async config() {
			if (configCache === undefined) configCache = await loadConfig(layout().kitConfigPath);
			return configCache;
		},
		/** Persist a settings patch and refresh the memo. Deep-merged: a patch
		 * carrying one zoom field must not drop the others. */
		async save(patch) {
			const current = await this.config();
			const next = normalizeConfig(patch === undefined ? current : mergeDeep(current, patch));
			configCache = await saveConfig(layout().kitConfigPath, next);
			return configCache;
		},
		log(message) {
			ring.add(message);
			ctx.logger.info(`[desktop-kit] ${message}`);
		},
		/** Remember an uploaded archive for the subsequent import call. */
		stashZip(buffer, name) {
			const now = Date.now();
			for (const [id, entry] of zipStash) {
				if (now - entry.at > ZIP_STASH_TTL_MS) zipStash.delete(id);
			}
			while (zipStash.size >= ZIP_STASH_MAX) zipStash.delete(zipStash.keys().next().value);
			const id = randomBytes(8).toString("hex");
			zipStash.set(id, { buffer, name, at: now });
			return id;
		},
		/** Take a stashed archive out (one-shot: it is consumed by the import). */
		takeZip(id) {
			const entry = zipStash.get(id);
			if (entry === undefined) return undefined;
			zipStash.delete(id);
			return entry;
		},
		/**
		 * Record a page-side error. Rate-limited, truncated and redacted — the
		 * same rules the desktop shell's page bridge applies before a line may
		 * reach a log.
		 */
		clientLog(fields) {
			const now = Date.now();
			if (now - clientLogWindowStart > CLIENT_LOG_WINDOW_MS) {
				clientLogWindowStart = now;
				clientLogCount = 0;
			}
			clientLogCount += 1;
			if (clientLogCount > CLIENT_LOG_MAX_PER_WINDOW) return false;
			const message = redactSecrets(String(fields.message ?? "")).slice(0, CLIENT_LOG_TEXT_LIMIT);
			if (message === "") return false;
			const where = [fields.source, fields.line].filter((part) => part !== undefined && part !== "").join(":");
			clientErrors.add(`page error: ${message}${where === "" ? "" : ` (${redactSecrets(String(where)).slice(0, 200)})`}`);
			return true;
		}
	};
	// The remote manager resolves webServer/connection lazily, so it can be built
	// here and only fails (loudly, per request) if this profile lacks them.
	kit.remote = createRemoteManager({
		ctx,
		layout,
		config: () => kit.config(),
		save: (patch) => kit.save(patch),
		log: (message) => kit.log(message)
	});
	kit.qr = createQrRenderer(layout);
	return kit;
}

/**
 * Bring remote access back up after a restart, when it was enabled before.
 * The token is persisted, so the link a phone already saved keeps working.
 * @param kit - shared kit state.
 */
async function resumeRemote(kit) {
	const config = await kit.config();
	if (config.remote.enabled !== true) return;
	try {
		const status = await kit.remote.enable(config.remote.mode);
		kit.log(`remote access resumed (${config.remote.mode}, state ${status.state})`);
	} catch (error) {
		kit.log(`remote resume failed: ${String(error?.message ?? error)}`);
	}
}

/**
 * Mount the HTTP surface and the index injection.
 * @param ready - a context where `webServer` and `configEditor` are both available.
 * @param kit - shared kit state.
 */
function mountWebSurface(ready, kit) {
	const { webServer } = ready;

	/** Read a bundled asset, reporting a missing build artifact clearly. */
	const asset = async (res, file, type) => {
		try {
			sendText(res, 200, await readAsset(file), type);
		} catch (error) {
			sendText(res, 500, `desktop-kit: asset ${file} unavailable — ${String(error?.message ?? error)}`);
		}
	};

	/** Dispatch one kit API call. */
	const handleApi = async (req, res, path) => {
		const method = req.method ?? "GET";
		if (path === "/api/state" && method === "GET") {
			const config = await kit.config();
			const layout = kit.layout();
			const [sounds, skills, mcp, preferences] = await Promise.all([listSounds(), skillStats(layout), mcpStats(layout), readUiPreferences(layout)]);
			sendJson(res, 200, {
				ok: true,
				config,
				sounds: [SILENT, SYSTEM_DEFAULT, ...sounds],
				counts: { skills, mcp },
				preferences,
				routePrefix: ROUTE_PREFIX,
				plugin: { name, version: VERSION }
			});
			return;
		}
		if (path === "/api/config" && method === "POST") {
			const body = await readJson(req);
			if (body === undefined) return sendJson(res, 400, { ok: false, error: "expected a JSON body" });
			const config = await kit.save(body);
			kit.log("settings updated");
			return sendJson(res, 200, { ok: true, config });
		}
		if (path === "/api/events" && method === "GET") {
			kit.bus.attach(res);
			return;
		}
		if (path === "/api/notifications" && method === "GET") {
			return sendJson(res, 200, { ok: true, items: kit.bus.recent() });
		}
		if (path === "/api/skills" && method === "GET") {
			const layout = kit.layout();
			const [skills, sources] = await Promise.all([listSkills(layout), listImportable(layout)]);
			return sendJson(res, 200, { ok: true, skills, sources });
		}
		if (path === "/api/skills" && method === "POST") {
			return sendJson(res, 200, await handleSkillAction(kit, await readJson(req)));
		}
		if (path === "/api/skills/zip" && method === "POST") {
			// Step one of the ZIP import: the page uploads the archive, gets back
			// the discovered skills; step two is the "import-zip" action.
			const buffer = await readRaw(req, ZIP_UPLOAD_LIMIT);
			if (buffer === undefined) return sendJson(res, 413, { ok: false, error: `expected a ZIP body of at most ${String(ZIP_UPLOAD_LIMIT >> 20)} MiB` });
			const archiveName = new URL(req.url ?? "/", "http://x").searchParams.get("name") ?? "skills.zip";
			try {
				const skills = await inspectZipSkills(kit.layout(), buffer, archiveName);
				return sendJson(res, 200, { ok: true, uploadId: kit.stashZip(buffer, archiveName), skills });
			} catch (error) {
				return sendJson(res, 200, { ok: false, error: String(error?.message ?? error) });
			}
		}
		if (path === "/api/client-log" && method === "POST") {
			const body = await readJson(req);
			if (body === undefined || typeof body.message !== "string") return sendJson(res, 400, { ok: false, error: "expected {message}" });
			const accepted = kit.clientLog(body);
			return sendJson(res, accepted ? 200 : 429, { ok: accepted });
		}
		if (path === "/api/mcp" && method === "GET") {
			const layout = kit.layout();
			const [servers, sources] = await Promise.all([listMcpServers(layout), listMcpImportable(layout)]);
			return sendJson(res, 200, { ok: true, servers, sources, patchPath: layout.patchPath });
		}
		if (path === "/api/mcp" && method === "POST") {
			return sendJson(res, 200, await handleMcpAction(kit, await readJson(req)));
		}
		if (path === "/api/remote" && method === "GET") {
			await kit.remote.sync();
			return sendJson(res, 200, { ok: true, remote: kit.remote.snapshot(), qrAvailable: kit.qr.available(), qrPath: QR_PATH });
		}
		if (path === "/api/remote" && method === "POST") {
			return sendJson(res, 200, await handleRemoteAction(kit, await readJson(req)));
		}
		if (path === "/api/diagnostics" && method === "GET") {
			const layout = kit.layout();
			const status = await collectDiagnostics({
				webServer: kit.webServer(),
				sessions: kit.sessions(),
				layout,
				ring: kit.ring,
				skills: await skillStats(layout),
				mcp: await mcpStats(layout)
			});
			status.notificationStreams = kit.bus.size();
			// Summarized, not dumped: the snapshot carries the link (i.e. the credential).
			const remote = kit.remote.snapshot();
			status.remote = { state: remote.state, enabled: remote.enabled, mode: remote.mode, proxyPort: remote.proxyPort, tunnel: remote.tunnel?.state, error: remote.error };
			return sendJson(res, 200, { ok: true, status });
		}
		if (path === "/api/logs" && method === "GET") {
			const logs = await readLogs(kit.layout(), kit.ring);
			return sendJson(res, 200, { ok: true, ...logs, clientErrors: kit.clientErrors.snapshot().slice(-50) });
		}
		return sendJson(res, 404, { ok: false, error: `no kit route for ${method} ${path}` });
	};

	ready.effect(() => webServer.register({
		kind: "prefix",
		path: ROUTE_PREFIX,
		handler: guard(async (req, res) => {
			const path = new URL(req.url ?? "/", "http://x").pathname;
			if (path === SCRIPT_PATH) return asset(res, "client.js", "text/javascript; charset=utf-8");
			if (path === STYLE_PATH) return asset(res, "client.css", "text/css; charset=utf-8");
			if (path.startsWith(SOUND_PATH)) {
				const id = decodeURIComponent(path.slice(SOUND_PATH.length)).replace(/\.wav$/u, "");
				const buffer = await readSound(id);
				if (buffer === undefined) return sendJson(res, 404, { ok: false, error: `unknown sound "${id}"` });
				return sendBuffer(res, 200, buffer, "audio/wav");
			}
			if (path === QR_PATH) {
				const snapshot = kit.remote.snapshot();
				const payload = snapshot.link ?? snapshot.lanUrls?.[0];
				if (payload === undefined) return sendJson(res, 409, { ok: false, error: "remote access is not enabled yet" });
				const svg = await kit.qr.svg(payload);
				if (svg === undefined) return sendJson(res, 501, { ok: false, error: "the qrcode package is not installed; run: dsh plugin --profile web add qrcode" });
				return sendText(res, 200, svg, "image/svg+xml; charset=utf-8");
			}
			if (path.startsWith(`${ROUTE_PREFIX}/api/`)) return handleApi(req, res, path.slice(ROUTE_PREFIX.length));
			return sendJson(res, 404, { ok: false, error: `no kit route for ${path}` });
		})
	}), "desktop-kit http routes");

	ready.effect(() => ready.on("webserver/index-inject", (table) => {
		table.push({ kind: "html", html: `<link rel="stylesheet" href="${STYLE_PATH}">`, placement: "head" });
		table.push({ kind: "script-src", src: SCRIPT_PATH, placement: "body" });
	}), "desktop-kit index injection");

	kit.log(`panel served at ${SCRIPT_PATH}`);
}

/** Apply a remote-access action coming from the panel. */
async function handleRemoteAction(kit, body) {
	if (body === undefined) throw new Error("expected a JSON body");
	switch (body.action) {
		case "enable": {
			const status = await kit.remote.enable(body.mode === "lan" ? "lan" : "tunnel");
			kit.log(`remote access enabled (${status.mode})`);
			return { ok: true, remote: status };
		}
		case "disable": {
			const status = await kit.remote.disable();
			return { ok: true, remote: status };
		}
		case "mode": {
			const status = await kit.remote.setMode(body.mode);
			kit.log(`remote mode set to ${status.mode}`);
			return { ok: true, remote: status };
		}
		case "reset-token": {
			const status = await kit.remote.resetToken();
			return { ok: true, remote: status };
		}
		default:
			throw new Error(`unknown remote action: ${JSON.stringify(body.action)}`);
	}
}

/** Apply a skill management action coming from the panel. */
async function handleSkillAction(kit, body) {
	if (body === undefined) throw new Error("expected a JSON body");
	const layout = kit.layout();
	switch (body.action) {
		case "toggle": {
			const result = await setSkillEnabled(layout, body.name, body.enabled === true);
			kit.log(`skill ${result.name} ${result.enabled ? "enabled" : "disabled"}`);
			return { ok: true, ...result };
		}
		case "delete": {
			const result = await deleteSkill(layout, body.name);
			kit.log(`skill ${result.name} deleted`);
			return { ok: true, ...result };
		}
		case "import": {
			const result = await importSkills(layout, body.source, body.names, body.onConflict);
			kit.log(`skills imported from ${body.source}: ${result.imported.length}`);
			return { ok: true, ...result };
		}
		case "import-zip": {
			const stash = kit.takeZip(body.uploadId);
			if (stash === undefined) throw new Error("the uploaded archive expired — pick the file again");
			const result = await importZipSkills(layout, stash.buffer, stash.name, body.names, body.onConflict);
			kit.log(`skills imported from zip ${stash.name}: ${result.imported.length}`);
			return { ok: true, ...result };
		}
		default:
			throw new Error(`unknown skills action: ${JSON.stringify(body.action)}`);
	}
}

/** Apply an MCP management action coming from the panel. */
async function handleMcpAction(kit, body) {
	if (body === undefined) throw new Error("expected a JSON body");
	const layout = kit.layout();
	switch (body.action) {
		case "upsert": {
			const result = await upsertMcpServer(layout, body.server, body.currentId);
			kit.log(`mcp ${result.id} ${result.created ? "added" : "updated"}`);
			return { ok: true, ...result };
		}
		case "toggle": {
			const result = await setMcpEnabled(layout, body.id, body.enabled === true);
			kit.log(`mcp ${result.id} ${result.enabled ? "enabled" : "disabled"}`);
			return { ok: true, ...result };
		}
		case "delete": {
			const result = await deleteMcpServer(layout, body.id);
			kit.log(`mcp ${result.id} deleted`);
			return { ok: true, ...result };
		}
		case "import": {
			const result = await importMcpServers(layout, body.source, body.names, body.onConflict);
			kit.log(`mcp servers imported from ${body.source}: ${result.imported.length}`);
			return { ok: true, ...result };
		}
		default:
			throw new Error(`unknown mcp action: ${JSON.stringify(body.action)}`);
	}
}

/**
 * Subscribe to the host events that drive reminders, and forward them to the
 * page. The four kinds mirror the desktop shell's classification:
 * an approval or question waiting on the user, and a finished turn split into
 * "did work" (task done) versus "answered only" (answer done).
 * @param ready - context whose lifetime owns these subscriptions.
 * @param kit - shared kit state.
 */
function mountEventBridge(ready, kit) {
	/** Per-session turn state: whether the running turn used any tool. */
	const turns = new Map();
	/** Session titles, learned from the session's own title events. */
	const titles = new Map();

	const titleOf = (sessionId, session) => titles.get(sessionId) ?? session?.header?.title ?? undefined;

	ready.on("session/event", (session, event) => {
		try {
			const sessionId = session?.id;
			if (typeof sessionId !== "string") return;
			switch (event?.type) {
				case "session/title":
					if (typeof event.data?.title === "string") titles.set(sessionId, event.data.title);
					break;
				case "turn/start":
					turns.set(sessionId, { worked: false });
					break;
				case "tool/call": {
					const state = turns.get(sessionId) ?? { worked: false };
					state.worked = true;
					turns.set(sessionId, state);
					break;
				}
				case "turn/end": {
					const reason = event.data?.reason?.kind;
					const worked = turns.get(sessionId)?.worked === true;
					turns.delete(sessionId);
					// Subagent turns are noise; the shell suppresses them too.
					if (session?.header?.origin === "subagent") break;
					if (reason !== "completed") break;
					const title = titleOf(sessionId, session);
					kit.bus.publish({
						kind: worked ? "turn_done" : "answer_done",
						sessionId,
						title,
						text: title === undefined ? (worked ? "dsh task completed" : "dsh answer ready") : `${worked ? "Task" : "Answer"} ready — ${title}`
					});
					break;
				}
				default:
					break;
			}
		} catch (error) {
			kit.log(`session event handling failed: ${String(error?.message ?? error)}`);
		}
	});

	// Waterfall events: publish, then hand control on. Not calling `next()` would
	// stall the approval/question the record is about.
	ready.on("approval/request", (request, next) => {
		try {
			if (request?.agent?.session?.header?.origin !== "subagent") {
				kit.bus.publish({
					kind: "approval",
					sessionId: request?.agent?.id,
					title: titleOf(request?.agent?.id, request?.agent?.session),
					text: "dsh has an operation waiting for your approval"
				});
			}
		} catch (error) {
			kit.log(`approval observation failed: ${String(error?.message ?? error)}`);
		}
		return next();
	});

	ready.on("user-questions/request", (request, next) => {
		try {
			if (request?.agent?.session?.header?.origin !== "subagent") {
				kit.bus.publish({
					kind: "question",
					sessionId: request?.agent?.id,
					title: titleOf(request?.agent?.id, request?.agent?.session),
					text: "dsh has a question waiting for your answer"
				});
			}
		} catch (error) {
			kit.log(`question observation failed: ${String(error?.message ?? error)}`);
		}
		return next();
	});

	ready.on("session/disposed", (session) => {
		if (typeof session?.id === "string") {
			turns.delete(session.id);
			titles.delete(session.id);
		}
	});
}

/**
 * Plugin entry point.
 * @param ctx - plugin context (declares `commands`).
 * @param config - the row config from `cordis.patch.yml`; `provideInit: true`
 *   registers the fallback `/init` command (see {@link initCommandDefinition}).
 */
function apply(ctx, config) {
	const kit = createKit(ctx);
	registerCommands(ctx, kit);
	if (config?.provideInit === true) {
		try {
			const definition = initCommandDefinition();
			definition.handler = command(definition.handler);
			ctx.commands.register(definition);
			kit.log("fallback /init registered (provideInit: true)");
		} catch (error) {
			const reason = String(error?.message ?? error);
			ctx.logger.warn(`[desktop-kit] fallback /init not registered: ${reason}`);
			kit.log(`fallback /init not registered: ${reason}`);
		}
	}
	ctx.inject(["webServer", "configEditor"], (ready) => {
		mountWebSurface(ready, kit);
		mountEventBridge(ready, kit);
		kit.log("host half wired (web surface + event bridge)");
	});
	// Resume separately, and only once `connection` is mounted: at first boot it
	// is not there yet, and a missing remote must never take the rest of the
	// plugin down with it.
	ctx.inject(["connection", "webServer"], () => {
		void resumeRemote(kit).catch((error) => kit.log(`remote resume error: ${String(error?.message ?? error)}`));
	});
}

export { apply, inject, name };
