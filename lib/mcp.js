/**
 * MCP server management: list / add / edit / enable / disable / delete / import.
 *
 * dsh has no MCP service — every server is its own loader row in the active
 * profile's patch document (`cordis.patch.yml`), a `dsh-mcp-client` entry whose
 * `config` carries the transport settings. So managing servers means editing
 * that YAML document, and the document is shared: it holds every other profile
 * override, `!!js` expression tags, and user comments. Re-serializing a parsed
 * object would destroy the tags and comments, so edits go through the `yaml`
 * package's document API, which preserves both, and are written atomically
 * under the same `package.json` lock dsh's own config editor takes.
 *
 * Row ids follow the desktop shell's convention (`mcp-<serverName>`) so both
 * tools address the same rows.
 *
 * @module dsh-desktop-kit/mcp/mcp
 */
import { readFile, readdir, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { withFileLock, writeFileAtomic } from "@deepseek-ai/dsh-atomic-write";
import { isMap, isSeq, parseDocument } from "yaml";

/** Loader row name that owns an MCP server instance. */
const MCP_PLUGIN_NAME = "@deepseek-ai/dsh-mcp-client";
/** Row id prefix, matching the desktop shell. */
const ROW_ID_PREFIX = "mcp-";
/** Transport values the plugin accepts (the schema rejects `sse`). */
export const TRANSPORTS = ["stdio", "streamable-http"];
const SERVER_NAME_PATTERN = /^[A-Za-z0-9_-]{1,32}$/u;
/** Keys copied from a submitted form; everything else on an existing row is preserved. */
const STDIO_KEYS = ["command", "args", "env", "cwd"];
const HTTP_KEYS = ["url", "headers"];

/** Validate a server name, which becomes both the CLI-visible tool prefix and the row id. */
function assertServerName(name) {
	if (typeof name !== "string" || !SERVER_NAME_PATTERN.test(name)) {
		throw new Error("serverName must be 1-32 characters of A-Z, a-z, 0-9, _ or -");
	}
	return name;
}

/** Collect the row nodes owned by an MCP server, wherever they sit in the op list. */
function mcpRowNodes(doc) {
	const rows = [];
	const root = doc.contents;
	if (!isSeq(root)) return rows;
	for (const op of root.items) {
		if (!isMap(op)) continue;
		const insert = op.get("insert", true);
		if (isSeq(insert)) {
			for (const row of insert.items) if (isMap(row) && row.get("name") === MCP_PLUGIN_NAME) rows.push({ row, container: insert });
			continue;
		}
		// A row may also be written directly at the op level.
		if (op.get("name") === MCP_PLUGIN_NAME) rows.push({ row: op, container: undefined });
	}
	return rows;
}

/** Serialize one row's config to plain JS, preserving unknown tags as raw strings. */
function configToJs(row) {
	const config = row.get("config", true);
	if (!isMap(config)) return {};
	const out = {};
	for (const pair of config.items) {
		const name = typeof pair.key?.value === "string" ? pair.key.value : String(pair.key?.value ?? "");
		if (name === "") continue;
		const value = pair.value;
		out[name] = value !== null && typeof value?.toJSON === "function" ? value.toJSON() : value?.value;
	}
	return out;
}

/** Summarize a config into the one-line description the UI shows. */
function summarize(transport, config) {
	if (transport === "streamable-http") return String(config.url ?? "");
	const args = Array.isArray(config.args) ? config.args.map(String) : [];
	return [config.command, ...args].filter((part) => part.length > 0).join(" ");
}

/** Read the patch document text, treating a missing file as an empty list. */
async function readDocument(text) {
	const doc = parseDocument(text === "" ? "[]\n" : text);
	if (doc.errors.length > 0) throw new Error(`cordis.patch.yml is not valid YAML: ${doc.errors[0].message}`);
	if (doc.contents === null) doc.contents = doc.createNode([]);
	if (!isSeq(doc.contents)) throw new Error("cordis.patch.yml must be a YAML sequence");
	// Block style for the row list, matching dsh's own writer (see dsh-config-editor):
	// a profile patch is read by humans far more often than by machines.
	doc.contents.flow = false;
	return doc;
}

/** Load, mutate and atomically persist the profile patch under the loader's lock. */
async function editDocument(layout, mutate) {
	return withFileLock(join(layout.profileDir, "package.json"), async () => {
		let before;
		try {
			before = await readFile(layout.patchPath, "utf8");
		} catch (error) {
			if (error.code !== "ENOENT") throw error;
			before = "[]\n";
		}
		const doc = await readDocument(before);
		const result = mutate(doc);
		const text = String(doc);
		await writeFileAtomic(layout.patchPath, text.endsWith("\n") ? text : `${text}\n`, { mode: 384 });
		return result;
	});
}

/** List every MCP row in the active profile's patch layer. */
export async function listMcpServers(layout) {
	let text;
	try {
		text = await readFile(layout.patchPath, "utf8");
	} catch {
		return [];
	}
	const doc = await readDocument(text);
	return mcpRowNodes(doc).map(({ row }) => {
		const config = configToJs(row);
		const id = typeof row.get("id") === "string" ? row.get("id") : `${ROW_ID_PREFIX}${String(config.serverName ?? "")}`;
		const transport = TRANSPORTS.includes(config.transport) ? config.transport : "stdio";
		return {
			id,
			serverName: String(config.serverName ?? ""),
			transport,
			summary: summarize(transport, config),
			enabled: row.get("disabled") !== true,
			config
		};
	});
}

/** Build the config node for a submitted server form. */
function buildConfig(input) {
	const transport = TRANSPORTS.includes(input?.transport) ? input.transport : "stdio";
	if (transport === "stdio") {
		const command = String(input?.command ?? "").trim();
		if (command === "") throw new Error("stdio transport requires a command");
		const config = { serverName: assertServerName(input?.serverName), transport, command };
		const args = Array.isArray(input?.args) ? input.args.map((value) => String(value)).filter((value) => value !== "") : [];
		if (args.length > 0) config.args = args;
		if (isPlainObject(input?.env) && Object.keys(input.env).length > 0) config.env = stringMap(input.env);
		const cwd = String(input?.cwd ?? "").trim();
		if (cwd !== "") config.cwd = cwd;
		return config;
	}
	const url = String(input?.url ?? "").trim();
	if (url === "") throw new Error("streamable-http transport requires a url");
	const config = { serverName: assertServerName(input?.serverName), transport, url };
	if (isPlainObject(input?.headers) && Object.keys(input.headers).length > 0) config.headers = stringMap(input.headers);
	return config;
}

const isPlainObject = (value) => typeof value === "object" && value !== null && !Array.isArray(value);

/** Coerce an object of arbitrary values into a map of strings. */
function stringMap(source) {
	const out = {};
	for (const [key, value] of Object.entries(source)) out[String(key)] = String(value);
	return out;
}

/**
 * Add a server, or update the one with the same `serverName`.
 *
 * On update the row keeps every key the form does not own (timeouts, reconnect
 * policy, …): only the transport-relevant keys are replaced, and switching
 * transport drops the other branch's keys rather than leaving a mixed config.
 * @param layout - resolved paths.
 * @param input - the submitted server form.
 * @param currentId - optional existing row id when renaming.
 */
export async function upsertMcpServer(layout, input, currentId) {
	const config = buildConfig(input);
	// The canonical id always follows the server name; `currentId` only locates
	// the row the panel is editing (which may still carry the pre-rename id).
	const wantedId = `${ROW_ID_PREFIX}${config.serverName}`;
	const rowId = currentId ?? wantedId;
	return editDocument(layout, (doc) => {
		const existing = mcpRowNodes(doc).find(({ row }) => row.get("id") === rowId);
		if (existing === undefined) {
			const node = doc.createNode({ id: rowId, name: MCP_PLUGIN_NAME, config });
			const host = firstInsertContainer(doc);
			if (host !== undefined) host.add(node);
			else doc.add(doc.createNode({ insert: [node] }));
			return { id: rowId, created: true };
		}
		// A rename has to move the row id too, or the stale `mcp-<oldName>` would
		// leave a second row behind the next time the new name is added.
		if (rowId !== wantedId) existing.row.set("id", wantedId);
		const configNode = existing.row.get("config", true);
		if (!isMap(configNode)) existing.row.set("config", doc.createNode(config));
		else {
			const keep = config.transport === "stdio" ? HTTP_KEYS : STDIO_KEYS;
			for (const key of keep) configNode.delete(key);
			for (const [key, value] of Object.entries(config)) configNode.set(key, doc.createNode(value));
		}
		return { id: wantedId, created: false };
	});
}

/** The `insert` sequence new rows should be appended to, if the document has one. */
function firstInsertContainer(doc) {
	if (!isSeq(doc.contents)) return undefined;
	for (const op of doc.contents.items) {
		if (isMap(op) && isSeq(op.get("insert", true))) return op.get("insert", true);
	}
	return undefined;
}

/** Enable or disable a row via the loader's native `disabled` key. */
export async function setMcpEnabled(layout, rowId, enabled) {
	return editDocument(layout, (doc) => {
		const found = mcpRowNodes(doc).find(({ row }) => row.get("id") === rowId);
		if (found === undefined) throw new Error(`no MCP server row "${rowId}"`);
		if (enabled) found.row.delete("disabled");
		else found.row.set("disabled", true);
		return { id: rowId, enabled };
	});
}

/** Remove a row; an `insert` op left empty is removed with it. */
export async function deleteMcpServer(layout, rowId) {
	return editDocument(layout, (doc) => {
		let removed = false;
		for (const op of [...(isSeq(doc.contents) ? doc.contents.items : [])]) {
			if (!isMap(op)) continue;
			const insert = op.get("insert", true);
			if (!isSeq(insert)) continue;
			for (let index = insert.items.length - 1; index >= 0; index--) {
				const row = insert.items[index];
				if (isMap(row) && row.get("id") === rowId && row.get("name") === MCP_PLUGIN_NAME) {
					insert.items.splice(index, 1);
					removed = true;
				}
			}
			if (removed && insert.items.length === 0) {
				const opIndex = doc.contents.items.indexOf(op);
				if (opIndex !== -1) doc.contents.items.splice(opIndex, 1);
			}
		}
		if (!removed) throw new Error(`no MCP server row "${rowId}"`);
		return { id: rowId, deleted: true };
	});
}

/**
 * Config files the kit can import MCP servers from. Each entry maps that
 * ecosystem's shape onto the kit's stdio/streamable-http model.
 */
export function mcpImportSources() {
	const home = homedir();
	return [
		{ id: "claude", label: "Claude Code (~/.claude.json)", path: join(home, ".claude.json") },
		{ id: "codex", label: "Codex (~/.codex/config.toml)", path: join(home, ".codex", "config.toml") },
		{ id: "opencode", label: "OpenCode (~/.config/opencode/opencode.json)", path: join(home, ".config", "opencode", "opencode.json") }
	];
}

/** Extract MCP servers from a Claude Code `~/.claude.json`. */
function fromClaude(text) {
	const parsed = JSON.parse(text);
	const servers = isPlainObject(parsed?.mcpServers) ? parsed.mcpServers : {};
	return Object.entries(servers).map(([serverName, value]) => {
		const entry = isPlainObject(value) ? value : {};
		if (entry.type === "http" || typeof entry.url === "string") {
			return { serverName, transport: "streamable-http", url: String(entry.url ?? ""), headers: entry.headers, unsupported: entry.type !== undefined && entry.type !== "http" };
		}
		return { serverName, transport: "stdio", command: String(entry.command ?? ""), args: entry.args, env: entry.env };
	});
}

/** Extract stdio MCP servers from a Codex `config.toml` (`[mcp_servers.<name>]` blocks). */
function fromCodex(text) {
	const out = [];
	let current = null;
	for (const rawLine of text.split(/\r?\n/u)) {
		const line = rawLine.trim();
		const header = /^\[mcp_servers\.(?:"([^"]+)"|([^\]]+))\]$/u.exec(line);
		if (header !== null) {
			current = { serverName: (header[1] ?? header[2]).trim(), transport: "stdio", command: "", args: [], env: {} };
			out.push(current);
			continue;
		}
		if (current === null) continue;
		if (line.startsWith("[")) {
			current = null;
			continue;
		}
		const pair = /^([A-Za-z0-9_-]+)\s*=\s*(.*)$/u.exec(line);
		if (pair === null) continue;
		const [, key, raw] = pair;
		if (key === "command") current.command = raw.replace(/^["']|["']$/gu, "");
		else if (key === "args") current.args = tomlStringArray(raw);
		else if (key === "enabled" && raw.trim() === "false") current.unsupported = true;
	}
	return out.filter((entry) => entry.unsupported !== true);
}

/** Parse the simple `["a", "b"]` array form Codex uses for `args`. */
function tomlStringArray(raw) {
	const inner = /^\[(.*)\]$/u.exec(raw.trim());
	if (inner === null) return [];
	return inner[1]
		.split(",")
		.map((part) => part.trim().replace(/^["']|["']$/gu, ""))
		.filter((part) => part !== "");
}

/** Extract MCP servers from an OpenCode `opencode.json`. */
function fromOpencode(text) {
	const parsed = JSON.parse(text);
	const servers = isPlainObject(parsed?.mcp) ? parsed.mcp : {};
	const out = [];
	for (const [serverName, value] of Object.entries(servers)) {
		const entry = isPlainObject(value) ? value : {};
		if (entry.enabled === false) continue;
		if (entry.type === "remote") {
			out.push({ serverName, transport: "streamable-http", url: String(entry.url ?? ""), headers: entry.headers });
			continue;
		}
		const command = Array.isArray(entry.command) ? entry.command.map(String) : [];
		out.push({ serverName, transport: "stdio", command: command[0] ?? "", args: command.slice(1), env: entry.environment });
	}
	return out;
}

/** List importable servers per source, marking conflicts with existing rows. */
export async function listMcpImportable(layout) {
	const existing = new Set((await listMcpServers(layout)).map((server) => server.serverName));
	const sources = [];
	for (const source of mcpImportSources()) {
		let text;
		try {
			text = await readFile(source.path, "utf8");
		} catch {
			sources.push({ ...source, available: false, servers: [] });
			continue;
		}
		let servers = [];
		try {
			if (source.id === "claude") servers = fromClaude(text);
			else if (source.id === "codex") servers = fromCodex(text);
			else servers = fromOpencode(text);
		} catch (error) {
			sources.push({ ...source, available: true, error: String(error?.message ?? error), servers: [] });
			continue;
		}
		sources.push({
			...source,
			available: true,
			servers: servers.map((server) => ({ ...server, conflict: existing.has(server.serverName), unsupported: server.unsupported === true }))
		});
	}
	return sources;
}

/** Import the named servers from one source. */
export async function importMcpServers(layout, sourceId, servers, onConflict = "skip") {
	const catalog = await listMcpImportable(layout);
	const source = catalog.find((entry) => entry.id === sourceId);
	if (source === undefined || source.available !== true) throw new Error(`import source unavailable: ${sourceId}`);
	const existing = new Set((await listMcpServers(layout)).map((server) => server.serverName));
	const selected = Array.isArray(servers) ? servers.map(String) : [];
	const imported = [];
	const skipped = [];
	for (const candidate of source.servers) {
		if (!selected.includes(candidate.serverName)) continue;
		if (candidate.unsupported === true) {
			skipped.push({ serverName: candidate.serverName, reason: "unsupported transport in the source" });
			continue;
		}
		if (existing.has(candidate.serverName) && onConflict !== "overwrite") {
			skipped.push({ serverName: candidate.serverName, reason: "already exists" });
			continue;
		}
		if (existing.has(candidate.serverName)) await deleteMcpServer(layout, `${ROW_ID_PREFIX}${candidate.serverName}`);
		await upsertMcpServer(layout, candidate);
		imported.push(candidate.serverName);
	}
	return { imported, skipped };
}

/** Counts for the diagnostics panel. */
export async function mcpStats(layout) {
	const servers = await listMcpServers(layout);
	return { total: servers.length, enabled: servers.filter((server) => server.enabled).length };
}

/** Exposed for the "does the file exist" hint in the import dialog. */
export async function pathExists(path) {
	try {
		await stat(path);
		return true;
	} catch {
		return false;
	}
}

