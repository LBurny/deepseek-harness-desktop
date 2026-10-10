/**
 * Read the harness's own UI preferences out of the profile patch document.
 *
 * The desktop shell follows dsh's `ui-theme` / `locale` rows
 * (`config.preference`) for its title bar and tray language
 * (`src-tauri/src/ui/state.rs::home_preference`). The kit's browser half wants
 * the same values for its panel language and theme, so it does not drift from
 * the UI it is injected into. The rows are top-level entries in the profile's
 * `cordis.patch.yml` (siblings of the `insert` ops); a missing row or field
 * simply yields `undefined` and the caller falls back (navigator language /
 * system color scheme).
 *
 * @module dsh-desktop-kit/shared/preference
 */
import { readFile } from "node:fs/promises";
import { isMap, isSeq, parseDocument } from "yaml";

/**
 * Read `config.preference` of the top-level patch row with the given id.
 * @param layout - resolved paths (only `patchPath` is used).
 * @param entryId - the row id, e.g. `"locale"` or `"ui-theme"`.
 * @returns the preference string, or `undefined` when absent/unreadable.
 */
export async function readPreference(layout, entryId) {
	let text;
	try {
		text = await readFile(layout.patchPath, "utf8");
	} catch {
		return undefined;
	}
	let doc;
	try {
		doc = parseDocument(text === "" ? "[]\n" : text);
	} catch {
		return undefined;
	}
	if (doc.errors.length > 0 || !isSeq(doc.contents)) return undefined;
	for (const op of doc.contents.items) {
		if (!isMap(op)) continue;
		if (op.get("id") !== entryId) continue;
		const config = op.get("config", true);
		if (!isMap(config)) return undefined;
		const value = config.get("preference", true);
		const scalar = typeof value === "object" && value !== null ? value.value : value;
		return typeof scalar === "string" && scalar !== "" ? scalar : undefined;
	}
	return undefined;
}

/**
 * The panel's two preferences in one shot.
 * @returns `{ locale?: string, theme?: string }` — e.g. `{locale:"zh", theme:"dark"}`.
 */
export async function readUiPreferences(layout) {
	const [locale, theme] = await Promise.all([readPreference(layout, "locale"), readPreference(layout, "ui-theme")]);
	return { locale, theme };
}
