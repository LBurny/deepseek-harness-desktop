/**
 * Remote-access pure-function checks: token generation, tunnel URL parsing,
 * LAN address filtering and cloudflared discovery. No running harness needed.
 *
 * Run from the repository root:
 *   "$RUNTIME/node.exe" tests/remote.test.mjs
 */
import { tmpdir } from "node:os";
import { join } from "node:path";
import { findCloudflared, generateToken, lanAddresses, parseTunnelUrl } from "../lib/remote.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

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
console.log(failed.length === 0 ? `\nREMOTE TEST PASS (${results.length} checks)` : `\nREMOTE TEST FAIL (${failed.length}/${results.length})`);
if (failed.length > 0) process.exitCode = 1;
