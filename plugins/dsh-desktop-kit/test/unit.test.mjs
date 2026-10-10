/**
 * Unit checks for the pieces that do not need a running harness: settings
 * normalization/merge, the notification bus, and the skill store's filesystem
 * operations.
 *
 * Run from a plugin location where `@deepseek-ai/dsh-atomic-write` resolves:
 *   "$RUNTIME/node.exe" test/unit.test.mjs
 *
 * mcp.test.mjs covers the patch-document editor separately (it needs `yaml`).
 */
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createNotifyBus } from "../lib/bus.js";
import { mergeDeep, normalizeConfig, defaultConfig, loadConfig, saveConfig } from "../lib/config.js";
import { deleteSkill, listSkills, setSkillEnabled } from "../lib/skills.js";
import { findCloudflared, generateToken, lanAddresses, parseTunnelUrl } from "../lib/remote.js";

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

/* ------------------------------------------------------------- settings */

check(defaultConfig().zoom.step === 0.02, "default zoom step is 0.02");
check(defaultConfig().notify.sound === "staplebops-02", "default sound matches the desktop shell");
check(defaultConfig().notify.browser === true, "browser notifications are on by default");
check(normalizeConfig({ notify: { browser: false } }).notify.browser === false, "browser notifications can be turned off");
check(normalizeConfig({ notify: { browser: "yes" } }).notify.browser === true, "a non-false browser value means on");

// The bug this guards: saving one zoom field must not reset the others.
const merged = mergeDeep(defaultConfig(), { zoom: { in: { ctrl: true, shift: false, alt: false, code: "KeyZ", key: "z" } } });
check(merged.zoom.step === 0.02, "mergeDeep keeps zoom.step when only zoom.in is patched", String(merged.zoom.step));
check(merged.zoom.min === 0.25 && merged.zoom.max === 5, "mergeDeep keeps zoom bounds");
check(merged.zoom.in.code === "KeyZ", "mergeDeep applies the patched field");
check(merged.notify.sound === "staplebops-02", "mergeDeep leaves unrelated sections alone");

const clamped = normalizeConfig({ zoom: { level: 99, step: 5, min: 0.2, max: -1 }, notify: { approval: { timing: "nonsense" }, volume: 3 } });
check(clamped.zoom.level === clamped.zoom.max, "level clamps to max", String(clamped.zoom.level));
check(clamped.zoom.step === 0.25, "step clamps into 0.01..0.25", String(clamped.zoom.step));
check(clamped.zoom.max >= clamped.zoom.min, "max is raised to at least min");
check(clamped.notify.approval.timing === "background", "unknown timing falls back to background", clamped.notify.approval.timing);
check(clamped.notify.volume === 1, "volume clamps into 0..1", String(clamped.notify.volume));

const dir = await mkdtemp(join(tmpdir(), "kit-unit-"));
const configPath = join(dir, "desktop-kit.json");
check((await loadConfig(configPath)).zoom.step === 0.02, "missing settings file yields defaults");
await writeFile(configPath, "{ not json", "utf8");
check((await loadConfig(configPath)).zoom.step === 0.02, "corrupt settings file yields defaults");
await saveConfig(configPath, normalizeConfig({ zoom: { level: 1.5 } }));
check(JSON.parse(await readFile(configPath, "utf8")).zoom.level === 1.5, "saveConfig round-trips");

/* ------------------------------------------------------ notification bus */

const bus = createNotifyBus();
check(bus.recent().length === 0, "bus starts empty");

/** Minimal stand-in for an HTTP response the bus can stream on. */
function fakeResponse() {
	const chunks = [];
	return {
		chunks,
		headers: undefined,
		listeners: {},
		writeHead(status, headers) {
			this.status = status;
			this.headers = headers;
		},
		write(chunk) {
			chunks.push(String(chunk));
			return true;
		},
		on(event, handler) {
			this.listeners[event] = handler;
		}
	};
}

const stream = fakeResponse();
const detach = bus.attach(stream);
check(stream.status === 200, "SSE stream answers 200", String(stream.status));
check(stream.headers["content-type"].startsWith("text/event-stream"), "SSE content type");
check(stream.chunks.join("").includes("event: ready"), "SSE sends a ready frame");
check(bus.size() === 1, "bus counts the open stream");

const published = bus.publish({ kind: "turn_done", text: "done" });
check(typeof published.id === "number", "publish assigns an id");
const wire = stream.chunks.join("");
check(wire.includes(`"kind":"turn_done"`), "publish reaches the stream", wire.slice(-120));
check(bus.recent().length === 1, "recent keeps published items");
check(bus.recent()[0].at !== undefined, "recent items are timestamped");

detach();
check(bus.size() === 0, "detach removes the stream");
bus.publish({ kind: "question", text: "after detach" });
check(!stream.chunks.join("").includes("after detach"), "detached stream receives nothing");
check(bus.recent().length === 2, "recent still records after detach");

/* --------------------------------------------------------------- skills */

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

/* --------------------------------------------------------------- remote */

const token = generateToken();
check(/^[0-9a-f]{64}$/u.test(token), "remote token is 256 bits of lowercase hex", String(token.length));
check(generateToken() !== token, "each remote token is unique");

// cloudflared prints the URL inside a boxed banner, so parsing must ignore the frame.
const BOXED = [
	"2026-10-10T00:00:00Z INF Requesting new quick Tunnel on trycloudflare.com...",
	"+--------------------------------------------------------------------------------------------+",
	"|  https://plain-craft-probe-words.trycloudflare.com                                        |",
	"+--------------------------------------------------------------------------------------------+"
].join("\n");
check(parseTunnelUrl(BOXED) === "https://plain-craft-probe-words.trycloudflare.com", "tunnel URL is found inside the boxed banner", String(parseTunnelUrl(BOXED)));
check(parseTunnelUrl("your quick Tunnel has been created! Visit it at https://abc-123.trycloudflare.com") === "https://abc-123.trycloudflare.com", "tunnel URL is found in a plain log line");
check(parseTunnelUrl("2026-10-10T00:00:01Z ERR failed to connect") === undefined, "a chunk without a URL yields nothing");
check(parseTunnelUrl("https://example.com/") === undefined, "an unrelated https URL is not mistaken for a tunnel");

const addresses = lanAddresses();
check(Array.isArray(addresses), "lanAddresses returns a list");
// APIPA placeholders are unroutable, so offering one as a phone link is always wrong.
check(addresses.every((address) => !address.startsWith("169.254.")), "link-local addresses are excluded", JSON.stringify(addresses));
check(addresses.every((address) => /^\d+\.\d+\.\d+\.\d+$/u.test(address)), "only IPv4 literals are returned", JSON.stringify(addresses));

// A configured-but-missing path must never be handed back as if it were usable.
// (Whether *some* other candidate exists is environment-dependent: the installer
// ships cloudflared next to node, a sandbox does not.)
const missing = join(tmpdir(), "definitely-not-cloudflared");
check(findCloudflared(missing, join(tmpdir(), "no-such-home")) !== missing, "a missing configured cloudflared is not accepted as-is");
check(findCloudflared(process.execPath, undefined) === process.execPath, "an explicit cloudflared path wins when it exists");

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nUNIT TEST PASS (${results.length} checks)` : `\nUNIT TEST FAIL (${failed.length}/${results.length})`);
await rm(dir, { recursive: true, force: true });
if (failed.length > 0) process.exitCode = 1;
