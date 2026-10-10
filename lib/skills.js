/**
 * Skill management: list / enable / disable / delete / import.
 *
 * dsh's filesystem skill provider has no native disable concept — it scans a
 * fixed set of roots, and `<DSH_HOME>/skills` is the user-owned one. So a
 * disabled skill is simply a directory parked in the sibling
 * `<DSH_HOME>/skills-disabled`, which dsh never scans. Moving it back re-enables
 * it, and dsh's chokidar watcher hot-refreshes the catalog either way, so no
 * restart is involved. This mirrors the desktop shell's behavior exactly.
 *
 * @module dsh-desktop-kit/skills/skills
 */
import { cp, mkdir, readdir, readFile, rename, rm, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import { discoverZipSkills, extractZipPrefix } from "./zip.js";

/** A directory is a skill only if it directly contains this marker file. */
const SKILL_MARKER = "SKILL.md";

/** Skill names are used as path segments, so they stay deliberately narrow. */
const NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]*$/u;

/**
 * Directories the kit can import skills from, i.e. the same ecosystems the
 * desktop shell offers. Missing directories are reported so the UI greys them out.
 */
export function importSources() {
	const home = homedir();
	return [
		{ id: "codex", label: "Codex (~/.codex/skills)", dir: join(home, ".codex", "skills") },
		{ id: "claude", label: "Claude Code (~/.claude/skills)", dir: join(home, ".claude", "skills") },
		{ id: "opencode", label: "OpenCode (~/.config/opencode/skills)", dir: join(home, ".config", "opencode", "skills") }
	];
}

/** Reject a name that would escape the skill roots. */
function assertName(name) {
	if (typeof name !== "string" || !NAME_PATTERN.test(name)) throw new Error(`invalid skill name: ${JSON.stringify(name)}`);
	return name;
}

/**
 * Pull `name` and `description` out of the leading `---` frontmatter block.
 * Deliberately a shallow line scan: multi-line YAML is not worth a dependency
 * for two scalar fields, and the provider itself only needs them as strings.
 */
function parseFrontmatter(text) {
	const match = /^---\r?\n([\s\S]*?)\r?\n---/u.exec(text);
	if (match === null) return {};
	const fields = {};
	for (const line of match[1].split(/\r?\n/u)) {
		const pair = /^([A-Za-z0-9_-]+):\s*(.*)$/u.exec(line);
		if (pair === null) continue;
		let value = pair[2].trim();
		if ((value.startsWith("\"") && value.endsWith("\"")) || (value.startsWith("'") && value.endsWith("'"))) {
			value = value.slice(1, -1);
		}
		fields[pair[1]] = value;
	}
	return fields;
}

/** Whether a directory entry is a skill directory (contains SKILL.md). */
async function isSkillDir(path) {
	try {
		return (await stat(join(path, SKILL_MARKER))).isFile();
	} catch {
		return false;
	}
}

/** Read one skill directory into its API summary. */
async function describeSkill(dir, enabled) {
	const name = basename(dir);
	let description = "";
	try {
		description = parseFrontmatter(await readFile(join(dir, SKILL_MARKER), "utf8")).description ?? "";
	} catch {
		/* unreadable SKILL.md: still list it, just without a description */
	}
	return { name, description, enabled, path: dir };
}

/**
 * List skills from both roots. Enabled skills sort first, then by name, so the
 * UI's default order is stable.
 */
export async function listSkills(layout) {
	const collect = async (root, enabled) => {
		let entries;
		try {
			entries = await readdir(root, { withFileTypes: true });
		} catch {
			return [];
		}
		const skills = [];
		for (const entry of entries) {
			if (!entry.isDirectory() && !entry.isSymbolicLink()) continue;
			const dir = join(root, entry.name);
			if (!(await isSkillDir(dir))) continue;
			skills.push(await describeSkill(dir, enabled));
		}
		return skills;
	};
	const [enabled, disabled] = await Promise.all([
		collect(layout.skillsDir, true),
		collect(layout.skillsDisabledDir, false)
	]);
	return [...enabled, ...disabled].sort((a, b) => (a.enabled === b.enabled ? a.name.localeCompare(b.name) : a.enabled ? -1 : 1));
}

/** Locate a skill directory in either root. */
async function findSkill(layout, name) {
	assertName(name);
	for (const [root, enabled] of [[layout.skillsDir, true], [layout.skillsDisabledDir, false]]) {
		const dir = join(root, name);
		if (await isSkillDir(dir)) return { dir, enabled, root };
	}
	return undefined;
}

/**
 * Enable or disable a skill by moving its directory between the two roots.
 * @returns the skill's new state.
 */
export async function setSkillEnabled(layout, name, enabled) {
	const found = await findSkill(layout, name);
	if (found === undefined) throw new Error(`skill not found: ${name}`);
	if (found.enabled === enabled) return { name, enabled };
	const targetRoot = enabled ? layout.skillsDir : layout.skillsDisabledDir;
	await mkdir(targetRoot, { recursive: true });
	const target = join(targetRoot, name);
	if (await isSkillDir(target)) throw new Error(`a skill named "${name}" already exists in the other root`);
	await rename(found.dir, target);
	return { name, enabled };
}

/** Delete a skill's copy from whichever root holds it. Import sources are untouched. */
export async function deleteSkill(layout, name) {
	const found = await findSkill(layout, name);
	if (found === undefined) throw new Error(`skill not found: ${name}`);
	await rm(found.dir, { recursive: true, force: true });
	return { name, deleted: true };
}

/** List what each import source would contribute, marking conflicts. */
export async function listImportable(layout) {
	const existing = new Set((await listSkills(layout)).map((skill) => skill.name));
	const sources = [];
	for (const source of importSources()) {
		let entries;
		try {
			entries = await readdir(source.dir, { withFileTypes: true });
		} catch {
			sources.push({ ...source, available: false, skills: [] });
			continue;
		}
		const skills = [];
		for (const entry of entries) {
			if (!entry.isDirectory()) continue;
			const dir = join(source.dir, entry.name);
			if (!(await isSkillDir(dir))) continue;
			const described = await describeSkill(dir, true);
			skills.push({ ...described, conflict: existing.has(described.name) });
		}
		sources.push({ ...source, available: true, skills: skills.sort((a, b) => a.name.localeCompare(b.name)) });
	}
	return sources;
}

/**
 * Copy selected skills from an import source into the enabled root.
 * @param layout - resolved paths.
 * @param sourceId - one of {@link importSources} ids.
 * @param names - skill directory names to copy.
 * @param onConflict - `"skip"` (default) or `"overwrite"`.
 */
export async function importSkills(layout, sourceId, names, onConflict = "skip") {
	const source = importSources().find((entry) => entry.id === sourceId);
	if (source === undefined) throw new Error(`unknown import source: ${sourceId}`);
	await mkdir(layout.skillsDir, { recursive: true });
	const imported = [];
	const skipped = [];
	for (const rawName of Array.isArray(names) ? names : []) {
		const name = assertName(rawName);
		const from = join(source.dir, name);
		if (!(await isSkillDir(from))) {
			skipped.push({ name, reason: "not a skill directory in the source" });
			continue;
		}
		const conflict = (await findSkill(layout, name)) !== undefined;
		if (conflict && onConflict !== "overwrite") {
			skipped.push({ name, reason: "already exists" });
			continue;
		}
		if (conflict) {
			for (const root of [layout.skillsDir, layout.skillsDisabledDir]) {
				await rm(join(root, name), { recursive: true, force: true });
			}
		}
		await cp(from, join(layout.skillsDir, name), { recursive: true });
		imported.push(name);
	}
	return { imported, skipped };
}

/** Counts for the diagnostics panel. */
export async function skillStats(layout) {
	const skills = await listSkills(layout);
	return { total: skills.length, enabled: skills.filter((skill) => skill.enabled).length };
}

/* ------------------------------------------------------- ZIP local import */

/**
 * What an uploaded ZIP would contribute: discovered skills with the conflict
 * flag set against both roots. Mirrors the desktop shell's ZIP import
 * (`src-tauri/src/features/skills.rs`).
 * @param layout - resolved paths.
 * @param buffer - the whole archive.
 * @param archiveName - the uploaded file name (fallback skill naming).
 */
export async function inspectZipSkills(layout, buffer, archiveName) {
	const discovered = discoverZipSkills(buffer, archiveName);
	const existing = new Set((await listSkills(layout)).map((skill) => skill.name));
	return discovered.map((entry) => ({ name: entry.name, description: entry.description, conflict: existing.has(entry.name), error: entry.error }));
}

/**
 * Extract the selected skills out of an uploaded ZIP into the enabled root.
 * @param layout - resolved paths.
 * @param buffer - the whole archive.
 * @param archiveName - the uploaded file name.
 * @param names - skills to import (must come from {@link inspectZipSkills}).
 * @param onConflict - `"skip"` (default) or `"overwrite"`.
 * @returns per-name outcome lists.
 */
export async function importZipSkills(layout, buffer, archiveName, names, onConflict = "skip") {
	const discovered = discoverZipSkills(buffer, archiveName);
	const imported = [];
	const skipped = [];
	const errors = [];
	for (const rawName of Array.isArray(names) ? names : []) {
		let name;
		try {
			name = assertName(rawName);
		} catch (error) {
			// One bad name must not abort the whole selection: report it and move on.
			errors.push({ name: String(rawName), reason: String(error?.message ?? error) });
			continue;
		}
		const entry = discovered.find((item) => item.name === name);
		if (entry === undefined) {
			errors.push({ name, reason: "not found in the archive" });
			continue;
		}
		const conflict = (await findSkill(layout, name)) !== undefined;
		if (conflict && onConflict !== "overwrite") {
			skipped.push({ name, reason: "already exists" });
			continue;
		}
		if (conflict) {
			for (const root of [layout.skillsDir, layout.skillsDisabledDir]) {
				await rm(join(root, name), { recursive: true, force: true });
			}
		}
		const destination = join(layout.skillsDir, name);
		await mkdir(destination, { recursive: true });
		try {
			await extractZipPrefix(buffer, entry.prefix, destination);
			imported.push(name);
		} catch (error) {
			// A failed extraction must not leave a half-written skill behind.
			await rm(destination, { recursive: true, force: true });
			errors.push({ name, reason: String(error?.message ?? error) });
		}
	}
	return { imported, skipped, errors };
}
