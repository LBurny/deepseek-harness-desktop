/**
 * Settings checks for the shared config store: defaults, normalization and
 * clamping, deep merge, and load/save round-trips against a temp file.
 *
 * Run from the repository root:
 *   "$RUNTIME/node.exe" tests/shared.test.mjs
 */
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mergeDeep, normalizeConfig, defaultConfig, loadConfig, saveConfig } from "../lib/config.js";

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

const dir = await mkdtemp(join(tmpdir(), "kit-settings-"));
const configPath = join(dir, "desktop-kit.json");
check((await loadConfig(configPath)).zoom.step === 0.02, "missing settings file yields defaults");
await writeFile(configPath, "{ not json", "utf8");
check((await loadConfig(configPath)).zoom.step === 0.02, "corrupt settings file yields defaults");
await saveConfig(configPath, normalizeConfig({ zoom: { level: 1.5 } }));
check(JSON.parse(await readFile(configPath, "utf8")).zoom.level === 1.5, "saveConfig round-trips");

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nSHARED TEST PASS (${results.length} checks)` : `\nSHARED TEST FAIL (${failed.length}/${results.length})`);
await rm(dir, { recursive: true, force: true });
if (failed.length > 0) process.exitCode = 1;
