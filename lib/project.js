/**
 * Phone-side "Project" tab backend: resolve the current session's workspace,
 * list directories and read files — strictly read-only.
 *
 * Port of the desktop shell's `src-tauri/src/remote/project.rs`. The routes
 * hang off the kit's remote proxy (behind the same token gate as everything
 * else), and the single-page file browser is served from a vendored copy of
 * the shell's self-contained page.
 *
 * The session → workspace mapping comes from dsh's own
 * `storages/workspace.json` (`tables.workspaces[*] = {path, title,
 * sessionIds[]}`), re-read on every call so workspace edits apply instantly.
 *
 * Path safety mirrors the shell: a `rel` may only contain ordinary segments
 * (no `.`/`..`, no separators-as-segments, no drive letters), and the joined
 * path must still sit inside the canonical workspace root after `realpath` —
 * junction/symlink escapes are caught by the resolved-prefix check.
 *
 * @module dsh-desktop-kit/remote/project
 */
import { readdir, readFile, realpath, stat } from "node:fs/promises";
import { join } from "node:path";

/** Route prefix on the kit's remote proxy (page + API). */
export const PROJECT_PREFIX = "/dshdesk/remote/project";
export const PROJECT_API_PREFIX = "/dshdesk/remote/api";

/** Per-directory entry cap; the client reports truncation past it. */
export const LIST_CAP = 2000;
/** Text preview size cap (downloads exempt): phone DOM dies on huge renders. */
export const TEXT_PREVIEW_CAP = 8 * 1024 * 1024;
/** Hard cap for any whole-file read, downloads included. */
export const FILE_CAP = 64 * 1024 * 1024;

/** Error with a stable wire code; mapped to an HTTP status by the handler. */
class ProjectError extends Error {
	constructor(code, status, extra) {
		super(code);
		this.code = code;
		this.status = status;
		this.extra = extra;
	}
}

const notFound = () => new ProjectError("not_found", 404);
const forbidden = () => new ProjectError("forbidden", 403);
const permission = () => new ProjectError("permission", 403);
const tooLarge = (size) => new ProjectError("too_large", 413, size);
const io = () => new ProjectError("io", 500);

const mapFs = (error) => (error?.code === "ENOENT" ? notFound() : error?.code === "EPERM" || error?.code === "EACCES" ? permission() : io());

/**
 * Resolve a session id to its workspace.
 * @param workspaceStorePath - `<DSH_HOME>/storages/workspace.json`.
 * @param sid - session id the page remembered in localStorage.
 * @returns `{ title, root }` or `undefined` (missing file, bad JSON, no hit).
 */
export async function resolveWorkspace(workspaceStorePath, sid) {
	if (typeof sid !== "string" || sid === "") return undefined;
	let store;
	try {
		const text = await readFile(workspaceStorePath, "utf8");
		store = JSON.parse(text.replace(/^﻿/u, ""));
	} catch {
		return undefined;
	}
	const workspaces = store?.tables?.workspaces;
	if (typeof workspaces !== "object" || workspaces === null) return undefined;
	for (const ws of Object.values(workspaces)) {
		if (typeof ws?.path !== "string" || !Array.isArray(ws?.sessionIds)) continue;
		if (!ws.sessionIds.includes(sid)) continue;
		const root = ws.path;
		const tail = root.replaceAll("\\", "/").split("/").filter(Boolean).at(-1);
		const title = typeof ws.title === "string" && ws.title !== "" ? ws.title : (tail ?? root);
		return { title, root };
	}
	return undefined;
}

/**
 * Join `rel` onto `root`, rejecting escapes. Only ordinary segments pass;
 * the canonical result must stay inside the canonical root.
 * @returns the canonical absolute path.
 */
export async function contained(root, rel) {
	const text = String(rel ?? "");
	if (/^[A-Za-z]:/u.test(text) || text.includes("\\") || text.startsWith("/")) throw forbidden();
	const segments = text === "" ? [] : text.split("/");
	for (const segment of segments) {
		if (segment === "" || segment === "." || segment === "..") throw forbidden();
	}
	let rootReal;
	let candidateReal;
	try {
		rootReal = await realpath(root);
	} catch (error) {
		throw mapFs(error);
	}
	try {
		candidateReal = await realpath(join(rootReal, ...segments));
	} catch (error) {
		throw mapFs(error);
	}
	// Windows is case-insensitive; strip \\?\ verbatim prefixes on both sides.
	const norm = (p) => p.replace(/^\\\\\?\\/u, "").toLowerCase();
	const [r, c] = [norm(rootReal), norm(candidateReal)];
	if (c === r || c.startsWith(`${r}\\`) || c.startsWith(`${r}/`)) return candidateReal;
	throw forbidden();
}

/**
 * List one directory: directories first, then case-insensitive name order.
 * A single entry failing stat is skipped (ACL-denied entries must not sink
 * the whole listing — the same lesson the shell learned from fs-local).
 */
export async function listDir(root, rel, cap = LIST_CAP) {
	const dir = await contained(root, rel);
	let info;
	try {
		info = await stat(dir);
	} catch (error) {
		throw mapFs(error);
	}
	if (!info.isDirectory()) throw notFound();
	let dirents;
	try {
		dirents = await readdir(dir, { withFileTypes: true });
	} catch (error) {
		throw mapFs(error);
	}
	const entries = [];
	let truncated = false;
	for (const dirent of dirents) {
		if (entries.length >= cap) {
			truncated = true;
			break;
		}
		let meta;
		try {
			meta = await stat(join(dir, dirent.name));
		} catch {
			continue;
		}
		entries.push({
			name: dirent.name,
			kind: meta.isDirectory() ? "dir" : "file",
			size: meta.size,
			mtime: Math.round(meta.mtimeMs / 1000)
		});
	}
	entries.sort((a, b) => (a.kind === b.kind ? a.name.toLowerCase().localeCompare(b.name.toLowerCase()) || a.name.localeCompare(b.name) : a.kind === "dir" ? -1 : 1));
	return { entries, truncated };
}

/** Preview MIME by extension; the vendored page keeps a mirror of this table. */
export function mimeFor(name) {
	const at = String(name).lastIndexOf(".");
	const ext = at > 0 ? String(name).slice(at + 1).toLowerCase() : "";
	switch (ext) {
		case "png": return "image/png";
		case "jpg": case "jpeg": return "image/jpeg";
		case "gif": return "image/gif";
		case "webp": return "image/webp";
		case "bmp": return "image/bmp";
		case "ico": return "image/x-icon";
		case "svg": return "image/svg+xml";
		case "avif": return "image/avif";
		case "pdf": return "application/pdf";
		case "md": case "markdown": return "text/markdown; charset=utf-8";
		case "txt": case "log": case "json": case "jsonc": case "csv": case "tsv":
		case "yaml": case "yml": case "toml": case "xml": case "html": case "htm":
		case "css": case "js": case "jsx": case "ts": case "tsx": case "mjs": case "cjs":
		case "py": case "rs": case "go": case "java": case "c": case "h": case "cpp":
		case "hpp": case "cs": case "sh": case "ps1": case "bat": case "cmd": case "ini":
		case "cfg": case "conf": case "env": case "sql": case "vue": case "svelte":
		case "lock": case "editorconfig": case "gitignore": case "dockerfile":
			return "text/plain; charset=utf-8";
		default:
			return "application/octet-stream";
	}
}

/** RFC 5987 `filename*`: ASCII whitelist passes, everything else percent-encodes. */
export function contentDisposition(name) {
	let encoded = "";
	for (const byte of Buffer.from(String(name), "utf8")) {
		const ch = String.fromCharCode(byte);
		encoded += /[A-Za-z0-9._~-]/u.test(ch) ? ch : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
	}
	return `attachment; filename*=UTF-8''${encoded}`;
}

/**
 * Read one file for preview or download, under the size gates.
 * @returns `{ buffer, mime, name, size }`.
 */
export async function readProjectFile(root, rel, download) {
	const path = await contained(root, rel);
	let info;
	try {
		info = await stat(path);
	} catch (error) {
		throw mapFs(error);
	}
	if (!info.isFile()) throw notFound();
	const name = path.replaceAll("\\", "/").split("/").at(-1) ?? "";
	const mime = mimeFor(name);
	if (info.size > FILE_CAP || (download !== true && mime.startsWith("text/") && info.size > TEXT_PREVIEW_CAP)) {
		throw tooLarge(info.size);
	}
	let buffer;
	try {
		buffer = await readFile(path);
	} catch (error) {
		throw mapFs(error);
	}
	return { buffer, mime, name, size: info.size };
}

/** Send a JSON error shaped like the shell's (`{"error": code}`). */
function sendProjectError(res, error) {
	const failure = error instanceof ProjectError ? error : io();
	const body = JSON.stringify(failure.extra === undefined ? { error: failure.code } : { error: failure.code, size: failure.extra });
	res.writeHead(failure.status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
	res.end(body);
}

/**
 * Dispatch one project route. The caller (the remote proxy) has already
 * authenticated the request. `assets.page` is the vendored browser page.
 * @param req - the remote request.
 * @param res - the remote response.
 * @param url - parsed request URL.
 * @param options - `{ workspaceStorePath, page }`.
 * @returns true when the path was a project route.
 */
export async function handleProjectRoute(req, res, url, { workspaceStorePath, page }) {
	const isPage = url.pathname === PROJECT_PREFIX;
	const isApi = url.pathname.startsWith(`${PROJECT_API_PREFIX}/`);
	if (!isPage && !isApi) return false;
	if ((req.method ?? "GET") !== "GET") {
		res.writeHead(405, { "content-type": "text/plain; charset=utf-8" }).end("method not allowed");
		return true;
	}
	try {
		if (isPage) {
			res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
			res.end(page);
			return true;
		}
		const kind = url.pathname.slice(PROJECT_API_PREFIX.length + 1);
		const sid = url.searchParams.get("sid") ?? "";
		const rel = url.searchParams.get("rel") ?? "";
		const workspace = await resolveWorkspace(workspaceStorePath, sid);
		if (workspace === undefined) throw notFound();
		if (kind === "resolve") {
			const body = JSON.stringify({ title: workspace.title, root: workspace.root });
			res.writeHead(200, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
			res.end(body);
			return true;
		}
		if (kind === "list") {
			const listing = await listDir(workspace.root, rel);
			res.writeHead(200, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
			res.end(JSON.stringify(listing));
			return true;
		}
		if (kind === "file") {
			const download = url.searchParams.has("download");
			const file = await readProjectFile(workspace.root, rel, download);
			const headers = { "content-type": file.mime, "cache-control": "no-store", "content-length": file.buffer.length };
			if (download) headers["content-disposition"] = contentDisposition(file.name);
			res.writeHead(200, headers);
			res.end(file.buffer);
			return true;
		}
		throw notFound();
	} catch (error) {
		sendProjectError(res, error);
		return true;
	}
}
