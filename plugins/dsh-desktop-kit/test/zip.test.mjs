/**
 * Unit checks for the ZIP reader (lib/zip.js) and the skill store's ZIP
 * import path (lib/skills.js): archive parsing, skill discovery layouts,
 * traversal protection and extraction, driven by synthetic archives built
 * here in memory.
 *
 * Run from a plugin location where dependencies resolve:
 *   "$RUNTIME/node.exe" test/zip.test.mjs
 */
import { deflateRawSync } from "node:zlib";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { discoverZipSkills, extractZipPrefix, listEntries, normalizeEntryName, readEntry, ZipError } from "../lib/zip.js";
import { importZipSkills, inspectZipSkills } from "../lib/skills.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

/** Whether a path exists. */
async function exists(path) {
	try {
		await stat(path);
		return true;
	} catch {
		return false;
	}
}

/**
 * Build a minimal ZIP archive in memory: local headers + central directory +
 * EOCD. CRC fields are left zero (the reader does not verify them).
 * @param entries - `[{ name, data: Buffer, method: 0|8 }]`.
 */
function buildZip(entries) {
	const locals = [];
	const centrals = [];
	let offset = 0;
	for (const entry of entries) {
		const nameBytes = Buffer.from(entry.name, "utf8");
		const data = entry.method === 8 ? deflateRawSync(entry.data) : entry.data;
		const local = Buffer.alloc(30);
		local.writeUInt32LE(0x04034b50, 0);
		local.writeUInt16LE(20, 4);
		local.writeUInt16LE(0, 6);
		local.writeUInt16LE(entry.method, 8);
		local.writeUInt32LE(data.length, 18);
		local.writeUInt32LE(entry.data.length, 22);
		local.writeUInt16LE(nameBytes.length, 26);
		locals.push(local, nameBytes, data);
		const central = Buffer.alloc(46);
		central.writeUInt32LE(0x02014b50, 0);
		central.writeUInt16LE(20, 4);
		central.writeUInt16LE(20, 6);
		central.writeUInt16LE(0, 8);
		central.writeUInt16LE(entry.method, 10);
		central.writeUInt32LE(data.length, 20);
		central.writeUInt32LE(entry.data.length, 24);
		central.writeUInt16LE(nameBytes.length, 28);
		central.writeUInt32LE(offset, 42);
		centrals.push(central, nameBytes);
		offset += 30 + nameBytes.length + data.length;
	}
	const cdStart = offset;
	const directory = Buffer.concat(centrals);
	const eocd = Buffer.alloc(22);
	eocd.writeUInt32LE(0x06054b50, 0);
	eocd.writeUInt16LE(entries.length, 8);
	eocd.writeUInt16LE(entries.length, 10);
	eocd.writeUInt32LE(directory.length, 12);
	eocd.writeUInt32LE(cdStart, 16);
	return Buffer.concat([...locals, directory, eocd]);
}

/** Expect a ZipError from a sync/async body. */
async function expectZipError(fn, label) {
	try {
		await fn();
	} catch (error) {
		check(error instanceof ZipError, label, String(error?.message ?? error));
		return;
	}
	check(false, label, "no error was thrown");
}

/* ------------------------------------------------------------- name rules */

check(normalizeEntryName("dir/file.md") === "dir/file.md", "plain relative name passes");
check(normalizeEntryName("dir\\file.md") === "dir/file.md", "backslashes are unified");
check(normalizeEntryName("dir/") === "dir", "a directory entry normalizes without the slash");
check(normalizeEntryName("../escape") === undefined, "dot-dot is rejected");
check(normalizeEntryName("a/./b") === undefined, "dot segments are rejected");
check(normalizeEntryName("/abs/path") === undefined, "absolute paths are rejected");
check(normalizeEntryName("C:/Windows/x") === undefined, "drive-qualified paths are rejected");
check(normalizeEntryName("a//b") === undefined, "empty segments are rejected");

/* -------------------------------------------------------- reader round trip */

const archive = buildZip([
	{ name: "alpha/SKILL.md", data: Buffer.from("---\nname: alpha\ndescription: \"Alpha skill\"\n---\n\nBody\n", "utf8"), method: 8 },
	{ name: "alpha/scripts/run.sh", data: Buffer.from("#!/bin/sh\necho hi\n", "utf8"), method: 0 },
	{ name: "beta/SKILL.md", data: Buffer.from("---\nname: beta\n---\n", "utf8"), method: 0 },
	{ name: "beta/", data: Buffer.alloc(0), method: 0 },
	{ name: "../evil.txt", data: Buffer.from("nope", "utf8"), method: 0 }
]);

const entries = listEntries(archive);
check(entries.length === 5, "central directory lists every entry", String(entries.length));
check(entries[0].method === 8 && entries[1].method === 0, "methods are parsed");
const inflated = readEntry(archive, entries[0]);
check(inflated.toString("utf8").includes("name: alpha"), "deflated entry inflates", inflated.length);
const stored = readEntry(archive, entries[1]);
check(stored.toString("utf8").startsWith("#!/bin/sh"), "stored entry reads verbatim");

await expectZipError(() => listEntries(Buffer.from("not a zip at all", "utf8")), "garbage is not a ZIP");
await expectZipError(() => listEntries(Buffer.alloc(10)), "a truncated buffer is not a ZIP");

/* ---------------------------------------------------------------- discovery */

const discovered = discoverZipSkills(archive, "pack.zip");
check(discovered.length === 2, "two top-level skill directories are discovered", JSON.stringify(discovered.map((s) => s.name)));
check(discovered[0].name === "alpha" && discovered[1].name === "beta", "skills sort by name");
check(discovered[0].description === "Alpha skill", "frontmatter description is read");
check(discovered.every((s) => s.prefix.endsWith("/")), "directory skills carry a prefix");

// Root-level SKILL.md: single skill, named by frontmatter, then by file name.
const single = buildZip([{ name: "SKILL.md", data: Buffer.from("---\nname: solo\ndescription: one\n---\n", "utf8"), method: 8 }]);
check(discoverZipSkills(single, "pack.zip")[0].name === "solo", "root SKILL.md names the skill from frontmatter");
const unnamed = buildZip([{ name: "SKILL.md", data: Buffer.from("no frontmatter\n", "utf8"), method: 0 }]);
check(discoverZipSkills(unnamed, "fallback-name.zip")[0].name === "fallback-name", "missing frontmatter falls back to the archive name");

await expectZipError(() => discoverZipSkills(buildZip([{ name: "readme.txt", data: Buffer.from("x"), method: 0 }]), "x.zip"), "an archive without SKILL.md is rejected");

/* --------------------------------------------------------------- extraction */

const dir = await mkdtemp(join(tmpdir(), "kit-zip-"));
const out = join(dir, "out");
const extracted = await extractZipPrefix(archive, "alpha/", out);
check(extracted.files === 2, "extraction counts the prefix's files", String(extracted.files));
check((await readFile(join(out, "SKILL.md"), "utf8")).includes("name: alpha"), "SKILL.md extracted with the prefix stripped");
check((await readFile(join(out, "scripts", "run.sh"), "utf8")).startsWith("#!/bin/sh"), "nested files extract");
check(!(await exists(join(dir, "evil.txt"))), "traversal entries cannot escape the target");

/* --------------------------------------------- skills store ZIP integration */

const skillsDir = join(dir, "skills");
const layout = { skillsDir, skillsDisabledDir: join(dir, "skills-disabled") };
const inspected = await inspectZipSkills(layout, archive, "pack.zip");
check(inspected.length === 2 && inspected.every((s) => s.conflict === false), "inspection marks no conflicts on an empty store");

const first = await importZipSkills(layout, archive, "pack.zip", ["alpha"], "skip");
check(first.imported.length === 1 && first.imported[0] === "alpha", "import writes the selected skill");
check((await exists(join(skillsDir, "alpha", "SKILL.md"))), "the imported skill is in the enabled root");

const again = await inspectZipSkills(layout, archive, "pack.zip");
check(again.find((s) => s.name === "alpha")?.conflict === true, "re-inspection marks the conflict");
check(again.find((s) => s.name === "beta")?.conflict === false, "the unimported skill stays conflict-free");

const second = await importZipSkills(layout, archive, "pack.zip", ["alpha", "beta"], "skip");
check(second.imported.length === 1 && second.imported[0] === "beta", "skip policy imports only the new skill");
check(second.skipped.length === 1 && second.skipped[0].name === "alpha", "the conflicting skill is skipped");

await writeFile(join(skillsDir, "alpha", "extra.txt"), "stale", "utf8");
const third = await importZipSkills(layout, archive, "pack.zip", ["alpha"], "overwrite");
check(third.imported.length === 1, "overwrite re-imports");
check(!(await exists(join(skillsDir, "alpha", "extra.txt"))), "overwrite clears the previous copy first");

const bogus = await importZipSkills(layout, archive, "pack.zip", ["gamma"], "skip");
check(bogus.errors.length === 1, "a name missing from the archive reports an error");

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nZIP TEST PASS (${results.length} checks)` : `\nZIP TEST FAIL (${failed.length}/${results.length})`);
await rm(dir, { recursive: true, force: true });
if (failed.length > 0) process.exitCode = 1;
