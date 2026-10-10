/**
 * Diagnostics: service status, environment facts, and a log tail.
 *
 * Two log sources, best-effort in this order:
 *  1. The desktop shell's unified `events.log`, present only when this dsh runs
 *     under DSHDesktop (`<DSH_HOME>/../events.log`) — that file already holds both
 *     the shell's lines and the child dsh's stdout/stderr.
 *  2. The kit's own ring buffer, which always exists and records what the kit
 *     observed (plugin lifecycle, notifications raised, management actions).
 * A bare `~/.dsh` setup therefore still gets a useful panel.
 *
 * @module dsh-desktop-kit/doctor/diagnostics
 */
import { open, stat } from "node:fs/promises";
import { getDshRuntimeVersion } from "@deepseek-ai/dsh-app-boot";

/** Log lines kept in the kit's in-memory ring. */
const RING_LIMIT = 500;
/** Bytes read from the tail of the shell log; matches the desktop shell's own cap. */
const SHELL_LOG_MAX_BYTES = 512 * 1024;
/** Lines returned from the shell log tail. */
const SHELL_LOG_MAX_LINES = 500;

/**
 * A bounded in-memory log. Timestamped like the shell's own lines so a
 * kit-supplied tail reads the same as a shell-supplied one.
 */
export function createLogRing(limit = RING_LIMIT) {
	const lines = [];
	return {
		/** Append one line, prefixed with the wall clock. */
		add(message) {
			const now = new Date();
			const stamp = `${String(now.getHours()).padStart(2, "0")}:${String(now.getMinutes()).padStart(2, "0")}:${String(now.getSeconds()).padStart(2, "0")}.${String(now.getMilliseconds()).padStart(3, "0")}`;
			lines.push(`[${stamp}] [desktop-kit] ${message}`);
			if (lines.length > limit) lines.splice(0, lines.length - limit);
		},
		/** Snapshot the ring, oldest first. */
		snapshot() {
			return [...lines];
		}
	};
}

/**
 * Read the tail of a file without loading it whole.
 * @param path - file to read.
 * @param maxBytes - how much of the end to read.
 * @returns the trailing lines, or `undefined` when the file is absent.
 */
async function readTail(path, maxBytes) {
	let info;
	try {
		info = await stat(path);
	} catch {
		return undefined;
	}
	const start = Math.max(0, info.size - maxBytes);
	const length = info.size - start;
	const handle = await open(path, "r");
	try {
		const buffer = Buffer.alloc(length);
		await handle.read(buffer, 0, length, start);
		const lines = buffer.toString("utf8").split(/\r?\n/u);
		// A byte-window read usually lands mid-line; drop that fragment.
		if (start > 0) lines.shift();
		return lines.filter((line) => line.length > 0);
	} finally {
		await handle.close();
	}
}

/**
 * Read the shell log tail plus the kit ring.
 * @param layout - resolved paths.
 * @param ring - the kit's log ring.
 * @returns `{ source, lines }` where source names what was read.
 */
export async function readLogs(layout, ring) {
	const tail = await readTail(layout.shellLogPath, SHELL_LOG_MAX_BYTES);
	if (tail !== undefined) {
		return { source: "shell", path: layout.shellLogPath, lines: tail.slice(-SHELL_LOG_MAX_LINES) };
	}
	return { source: "kit", path: undefined, lines: ring.snapshot() };
}

/** Count sessions, tolerating a service that is not mounted. */
function sessionCount(sessions) {
	try {
		const list = sessions?.list?.();
		return Array.isArray(list) ? list.length : undefined;
	} catch {
		return undefined;
	}
}

/**
 * Assemble the diagnostics payload.
 * @param options - web server, session registry, layout, ring, extra counters.
 * @returns a JSON-serializable status document.
 */
export async function collectDiagnostics({ webServer, sessions, layout, ring, skills, mcp }) {
	const memory = process.memoryUsage();
	return {
		state: "ready",
		version: getDshRuntimeVersion(),
		node: process.version,
		pid: process.pid,
		uptimeSeconds: Math.round(process.uptime()),
		memoryMb: Math.round(memory.rss / (1024 * 1024)),
		port: webServer?.port,
		host: webServer?.host,
		profile: layout.profileName,
		profileDir: layout.profileDir,
		dshHome: layout.dshHome,
		patchPath: layout.patchPath,
		sessions: sessionCount(sessions),
		skills,
		mcp,
		platform: process.platform,
		arch: process.arch
	};
}
