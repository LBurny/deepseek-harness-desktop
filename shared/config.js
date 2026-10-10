/**
 * Kit settings: load / merge / save `<DSH_HOME>/desktop-kit.json`.
 *
 * Deliberately a plain JSON file rather than a dsh settings namespace: the
 * browser half reads and writes it through the kit's own HTTP routes, and a
 * patch-entry config would need a dsh restart (or an HMR round trip) for every
 * zoom-step change. `writeFileAtomic` keeps concurrent saves from truncating.
 *
 * @module dsh-desktop-kit/shared/config
 */
import { readFile } from "node:fs/promises";
import { writeFileAtomic } from "@deepseek-ai/dsh-atomic-write";

/** Default zoom shortcut: Ctrl+Shift+= (mirrors the desktop shell). */
const DEFAULT_ZOOM_IN = { ctrl: true, shift: true, alt: false, code: "Equal", key: "+" };
/** Default zoom-out shortcut: Ctrl+Shift+- (mirrors the desktop shell). */
const DEFAULT_ZOOM_OUT = { ctrl: true, shift: true, alt: false, code: "Minus", key: "_" };

/** Notification kinds the kit can raise, matching the desktop shell's four rules. */
export const NOTIFY_KINDS = ["approval", "question", "turn_done", "answer_done"];

/** Bundled completion sounds; the desktop shell's default is `staplebops-02`. */
export const DEFAULT_SOUND = "staplebops-02";

/** Fresh-install settings. */
export function defaultConfig() {
	return {
		zoom: {
			level: 1,
			step: 0.02,
			min: 0.25,
			max: 5,
			in: { ...DEFAULT_ZOOM_IN },
			out: { ...DEFAULT_ZOOM_OUT }
		},
		notify: {
			approval: { enabled: true, timing: "background" },
			question: { enabled: true, timing: "background" },
			turn_done: { enabled: true, timing: "background" },
			answer_done: { enabled: true, timing: "background" },
			sound: DEFAULT_SOUND,
			volume: 0.7,
			toast: true,
			browser: true
		},
		ui: { launcher: true },
		remote: {
			// Off by default: enabling publishes the harness (LAN, or the internet in
			// tunnel mode), and the link it hands out is the only credential.
			enabled: false,
			mode: "tunnel",
			port: 0,
			token: "",
			cloudflaredPath: ""
		}
	};
}

const isObject = (value) => typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * Deep-merge a patch over a base, returning a new value. Arrays and scalars
 * replace wholesale; plain objects merge key by key.
 */
export function mergeDeep(base, patch) {
	if (!isObject(base) || !isObject(patch)) return patch === undefined ? base : patch;
	const out = { ...base };
	for (const [key, value] of Object.entries(patch)) {
		out[key] = isObject(value) && isObject(base[key]) ? mergeDeep(base[key], value) : value;
	}
	return out;
}

/** Clamp a numeric setting, falling back when the value is not usable. */
function clampNumber(value, min, max, fallback) {
	const num = typeof value === "number" && Number.isFinite(value) ? value : fallback;
	return Math.min(max, Math.max(min, num));
}

/**
 * Coerce untrusted input (a settings file, or a browser POST) into the shape
 * the kit expects. Anything malformed falls back to the default rather than
 * failing the whole save.
 * @param raw - candidate config.
 * @returns a fully populated, range-checked config.
 */
export function normalizeConfig(raw) {
	const defaults = defaultConfig();
	const merged = mergeDeep(defaults, isObject(raw) ? raw : {});
	const zoom = merged.zoom;
	zoom.min = clampNumber(zoom.min, 0.1, 5, defaults.zoom.min);
	zoom.max = clampNumber(zoom.max, zoom.min, 8, defaults.zoom.max);
	zoom.step = clampNumber(zoom.step, 0.01, 0.25, defaults.zoom.step);
	zoom.level = clampNumber(zoom.level, zoom.min, zoom.max, 1);
	for (const side of ["in", "out"]) {
		const shortcut = zoom[side];
		if (!isObject(shortcut)) {
			zoom[side] = { ...defaults.zoom[side] };
			continue;
		}
		zoom[side] = {
			ctrl: shortcut.ctrl === true,
			shift: shortcut.shift === true,
			alt: shortcut.alt === true,
			code: typeof shortcut.code === "string" && shortcut.code.length > 0 ? shortcut.code : defaults.zoom[side].code,
			key: typeof shortcut.key === "string" ? shortcut.key : defaults.zoom[side].key
		};
	}
	const notify = merged.notify;
	for (const kind of NOTIFY_KINDS) {
		const rule = notify[kind];
		notify[kind] = isObject(rule)
			? { enabled: rule.enabled !== false, timing: rule.timing === "always" ? "always" : "background" }
			: { ...defaults.notify[kind] };
	}
	notify.volume = clampNumber(notify.volume, 0, 1, defaults.notify.volume);
	notify.sound = typeof notify.sound === "string" && notify.sound.length > 0 ? notify.sound : DEFAULT_SOUND;
	notify.toast = notify.toast !== false;
	notify.browser = notify.browser !== false;
	merged.ui = { launcher: !isObject(merged.ui) || merged.ui.launcher !== false };
	const remote = merged.remote;
	remote.mode = remote.mode === "lan" ? "lan" : "tunnel";
	remote.enabled = remote.enabled === true;
	// An empty or malformed token means "mint a new one when access is enabled".
	remote.token = typeof remote.token === "string" && /^[0-9a-f]{32,}$/u.test(remote.token) ? remote.token : "";
	remote.port = Number.isInteger(remote.port) && remote.port >= 0 && remote.port <= 65535 ? remote.port : 0;
	remote.cloudflaredPath = typeof remote.cloudflaredPath === "string" ? remote.cloudflaredPath : "";
	return merged;
}

/** Read the kit settings file, tolerating a missing or corrupt file. */
export async function loadConfig(path) {
	try {
		return normalizeConfig(JSON.parse(await readFile(path, "utf8")));
	} catch {
		return normalizeConfig(undefined);
	}
}

/** Atomically persist kit settings. */
export async function saveConfig(path, config) {
	await writeFileAtomic(path, `${JSON.stringify(config, null, "\t")}\n`, { mode: 384 });
	return config;
}
