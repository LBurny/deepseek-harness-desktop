/**
 * Completion sounds bundled with the kit.
 *
 * The desktop shell plays its own WAVs through `waveOut` on a native thread;
 * a page has neither, so the kit serves the same files over its web route and
 * the browser half plays them with `HTMLAudioElement`. The file set and the
 * default (`staplebops-02`) are copied from the shell so behavior matches.
 *
 * @module dsh-desktop-kit/lib/sounds
 */
import { readFile, readdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/** Bundled sound directory, resolved relative to this module. */
const SOUNDS_DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "sounds");

/** Names matching this pattern are the only ones the route will serve. */
const SOUND_NAME_PATTERN = /^[a-z0-9][a-z0-9._-]*\.wav$/u;

/** The `silent` pseudo-choice: no audio at all. */
export const SILENT = "silent";
/** The `default` pseudo-choice: let the notification use the system sound. */
export const SYSTEM_DEFAULT = "default";

/** List the bundled sounds, sorted, as ids the settings UI can offer. */
export async function listSounds() {
	let files;
	try {
		files = await readdir(SOUNDS_DIR);
	} catch {
		return [];
	}
	return files
		.filter((name) => SOUND_NAME_PATTERN.test(name))
		.map((name) => name.replace(/\.wav$/u, ""))
		.sort();
}

/**
 * Read one bundled sound.
 * @param id - sound id without the extension.
 * @returns the WAV bytes, or `undefined` when the id is unknown.
 */
export async function readSound(id) {
	if (typeof id !== "string" || !SOUND_NAME_PATTERN.test(`${id}.wav`)) return undefined;
	try {
		return await readFile(join(SOUNDS_DIR, `${id}.wav`));
	} catch {
		return undefined;
	}
}
