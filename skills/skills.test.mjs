/**
 * Skill store checks: listing, enable/disable moves between the two roots,
 * delete, conflict handling and path-traversal rejection, all against a temp
 * directory. No running harness needed.
 *
 * Run from the repository root:
 *   "$RUNTIME/node.exe" skills/skills.test.mjs
 *
 * zip.test.mjs in the same folder covers the ZIP import machinery separately.
 */
import { mkdir, mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deleteSkill, listSkills, setSkillEnabled } from "./skills.js";

/** Whether a path exists. */
async function exists(path) {
	try {
		await stat(path);
		return true;
	} catch {
		return false;
	}
}

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

/* --------------------------------------------------------------- skills */

const dir = await mkdtemp(join(tmpdir(), "kit-skills-"));
const skillsDir = join(dir, "skills");
const disabledDir = join(dir, "skills-disabled");
const layout = { skillsDir, skillsDisabledDir: disabledDir };
await mkdir(join(skillsDir, "alpha"), { recursive: true });
await writeFile(join(skillsDir, "alpha", "SKILL.md"), '---\nname: alpha\ndescription: "Alpha skill"\n---\n\nBody\n', "utf8");
await mkdir(join(skillsDir, "not-a-skill"), { recursive: true });
await writeFile(join(skillsDir, "not-a-skill", "readme.md"), "no SKILL.md here\n", "utf8");

let skills = await listSkills(layout);
check(skills.length === 1, "listSkills ignores directories without SKILL.md", JSON.stringify(skills.map((skill) => skill.name)));
check(skills[0].name === "alpha", "skill name comes from the directory");
check(skills[0].description === "Alpha skill", "frontmatter description is unquoted", skills[0].description);
check(skills[0].enabled === true, "skill in the active root reads as enabled");

await setSkillEnabled(layout, "alpha", false);
skills = await listSkills(layout);
check(skills[0].enabled === false, "disable is reflected in the listing");
check(await exists(join(disabledDir, "alpha", "SKILL.md")), "disabled skill's directory moved to the bypass root");
check(!(await exists(join(skillsDir, "alpha"))), "the enabled root no longer holds it");

// Occupying the destination must fail loudly rather than overwrite a skill.
await setSkillEnabled(layout, "alpha", true);
await mkdir(join(disabledDir, "alpha"), { recursive: true });
await writeFile(join(disabledDir, "alpha", "SKILL.md"), "---\nname: alpha\ndescription: a\n---\n", "utf8");
let conflictReported = false;
try {
	await setSkillEnabled(layout, "alpha", false);
} catch (error) {
	conflictReported = String(error?.message ?? error).includes("already exists");
}
check(conflictReported, "toggling into an occupied root reports a conflict instead of clobbering");
check((await listSkills(layout)).filter((skill) => skill.enabled).length === 1, "the conflicting toggle changed nothing");

await deleteSkill(layout, "alpha");
check((await listSkills(layout)).filter((skill) => skill.enabled).length === 0, "delete removes the skill");

let rejected = false;
try {
	await deleteSkill(layout, "../escape");
} catch {
	rejected = true;
}
check(rejected, "path traversal in a skill name is rejected");

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nSKILLS TEST PASS (${results.length} checks)` : `\nSKILLS TEST FAIL (${failed.length}/${results.length})`);
await rm(dir, { recursive: true, force: true });
if (failed.length > 0) process.exitCode = 1;
