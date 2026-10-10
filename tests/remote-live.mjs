/**
 * Live checks for the remote feature. Needs a running dsh with the kit installed:
 *   node test/remote-live.mjs <kitBaseUrl> <proxyPort> <token>
 * The three arguments come from `GET /dshdesk/api/remote` (proxyPort, and the
 * token inside `remote.link`). Excluded from the offline suite because it needs
 * a listening proxy; run it against a sandbox instance, never the real one.
 */

import { connect } from "node:net";
import { deflateRawSync } from "node:zlib";

const [base, proxyPortRaw, token] = process.argv.slice(2);
const proxyPort = Number(proxyPortRaw);
const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });
const note = (text) => results.push({ ok: true, label: text, detail: "" });

const proxy = `http://127.0.0.1:${String(proxyPort)}`;

/** Fetch, never following redirects so the gate's answer is visible. */
const raw = (url, headers = {}) => fetch(url, { redirect: "manual", headers });

/**
 * Build a minimal ZIP archive in memory (local headers + central directory +
 * EOCD, CRC zeroed — the reader does not verify it). Same helper as in
 * zip.test.mjs, duplicated so this script stays standalone.
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
		offset += local.length + nameBytes.length + data.length;
	}
	const centralBlock = Buffer.concat(centrals);
	const eocd = Buffer.alloc(22);
	eocd.writeUInt32LE(0x06054b50, 0);
	eocd.writeUInt16LE(entries.length, 8);
	eocd.writeUInt16LE(entries.length, 10);
	eocd.writeUInt32LE(centralBlock.length, 12);
	eocd.writeUInt32LE(offset, 16);
	return Buffer.concat([...locals, centralBlock, eocd]);
}

// --- gate: no credential --------------------------------------------------
const bare = await raw(`${proxy}/`);
check(bare.status === 403, "no credential is refused with 403", String(bare.status));

// --- gate: wrong token ----------------------------------------------------
const started = Date.now();
const wrong = await raw(`${proxy}/?token=${"0".repeat(64)}`);
const elapsed = Date.now() - started;
check(wrong.status === 403, "wrong token is refused with 403", String(wrong.status));
check(elapsed >= 450, "wrong token is delayed (anti-probe)", `${String(elapsed)}ms`);

// --- gate: right token mints the cookie ----------------------------------
const entry = await raw(`${proxy}/?token=${token}`);
check(entry.status === 303, "correct token redirects (303)", String(entry.status));
check(entry.headers.get("location") === "/", "redirect strips the token from the address", String(entry.headers.get("location")));
const setCookie = entry.headers.getSetCookie?.() ?? [];
const gate = setCookie[0]?.split(";", 1)[0] ?? "";
check(gate.startsWith("__dsh_remote="), "a gate cookie is issued", gate.slice(0, 24));
check(/HttpOnly/i.test(setCookie[0] ?? ""), "the gate cookie is HttpOnly");

// --- authenticated passthrough -------------------------------------------
const cookie = { cookie: gate };
// A document navigation, the way a phone browser asks for it — the proxy's
// HTML rewrite only fires for those, never for asset fetches.
const documentAccept = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
const index = await raw(`${proxy}/`, { ...cookie, accept: documentAccept });
const html = await index.text();
check(index.status === 200, "cookie authenticates the index", String(index.status));
check(html.includes("dshdesk/client.js"), "the harness index is served through the proxy (with our injection)");

const api = await raw(`${proxy}/dshdesk/api/state`, cookie);
const apiJson = await api.json().catch(() => ({}));
check(api.status === 200 && apiJson.ok === true, "kit API passes through the proxy authenticated", `${String(api.status)}`);

// A path that lives on the harness itself, proving dsh's own auth accepted us.
const harness = await raw(`${proxy}/api/remote.mux`, cookie);
check(harness.status !== 401 && harness.status !== 403, "harness API is not rejected by dsh's own auth", String(harness.status));
await harness.body?.cancel?.();

// --- response enhancements: HTML rewrite ----------------------------------
check(html.includes("<!-- dshdesk-mobile -->"), "the proxied document carries the mobile injection marker");
check(html.includes("maximum-scale=1, user-scalable=no"), "the viewport is rewritten to the no-zoom form");
check(html.includes("dsh-splash"), "the splash overlay is injected");

// --- response enhancements: gzip ------------------------------------------
// A document fetch answers identity while the same asset fetched by a
// gzip-capable client comes back compressed — only the proxy's transform
// produces that pair (upstream dsh does not compress).
const assetPath = html.match(/src="(\/[^"]+\.js)"/u)?.[1];
if (assetPath === undefined) {
	check(false, "a script asset is found in the document");
} else {
	const plain = await raw(`${proxy}${assetPath}`, { ...cookie, "accept-encoding": "identity" });
	const gzipped = await raw(`${proxy}${assetPath}`, { ...cookie, "accept-encoding": "gzip" });
	await plain.arrayBuffer();
	await gzipped.arrayBuffer();
	check(plain.headers.get("content-encoding") === null, "identity fetch is not compressed", String(plain.headers.get("content-encoding")));
	check(gzipped.headers.get("content-encoding") === "gzip", "gzip fetch is compressed by the proxy", `${String(gzipped.status)} ${assetPath}`);
}

// --- project routes --------------------------------------------------------
const projectPage = await raw(`${proxy}/dshdesk/remote/project`, cookie);
const projectHtml = projectPage.status === 200 ? await projectPage.text() : "";
check(projectPage.status === 200 && projectHtml.includes("dshdesk-project"), "the project page is served through the proxy", String(projectPage.status));
const resolveMiss = await raw(`${proxy}/dshdesk/remote/api/resolve?sid=${"f".repeat(32)}`, cookie);
check(resolveMiss.status === 404, "an unknown session 404s on resolve", String(resolveMiss.status));
await resolveMiss.body?.cancel?.();

// --- ZIP import round trip --------------------------------------------------
const archive = buildZip([
	{ name: "live-probe-skill/SKILL.md", data: Buffer.from('---\nname: live-probe-skill\ndescription: "live probe"\n---\n\nBody\n', "utf8"), method: 8 },
	{ name: "live-probe-skill/notes.txt", data: Buffer.from("probe\n", "utf8"), method: 0 }
]);
const upload = await fetch(`${base}/api/skills/zip?name=live-probe.zip`, { method: "POST", body: archive });
const uploadJson = await upload.json().catch(() => ({}));
check(uploadJson.ok === true && uploadJson.skills?.[0]?.name === "live-probe-skill", "zip upload inspects the archive", JSON.stringify(uploadJson).slice(0, 140));
if (uploadJson.ok === true) {
	const importRes = await fetch(`${base}/api/skills`, {
		method: "POST",
		headers: { "content-type": "application/json" },
		body: JSON.stringify({ action: "import-zip", uploadId: uploadJson.uploadId, names: ["live-probe-skill"], onConflict: "overwrite" })
	});
	const importJson = await importRes.json().catch(() => ({}));
	check(importJson.ok === true && importJson.imported?.includes("live-probe-skill"), "zip import lands the skill", JSON.stringify(importJson).slice(0, 140));
	const listed = await (await fetch(`${base}/api/skills`)).json().catch(() => ({}));
	check(listed.skills?.some((skill) => skill.name === "live-probe-skill"), "the imported skill is listed");
	// Clean up so the probe is rerunnable.
	await fetch(`${base}/api/skills`, {
		method: "POST",
		headers: { "content-type": "application/json" },
		body: JSON.stringify({ action: "delete", name: "live-probe-skill" })
	});
}

// --- WebSocket upgrade passthrough ---------------------------------------
function upgrade(path, headers) {
	return new Promise((resolve) => {
		const socket = connect(proxyPort, "127.0.0.1", () => {
			socket.write(
				`GET ${path} HTTP/1.1\r\nHost: 127.0.0.1:${String(proxyPort)}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n${headers}\r\n`
			);
		});
		let data = "";
		socket.on("data", (chunk) => {
			data += String(chunk);
			if (data.includes("\r\n\r\n")) {
				socket.destroy();
				resolve(data.split("\r\n", 1)[0]);
			}
		});
		socket.on("error", () => resolve("socket error"));
		socket.on("close", () => resolve(data.split("\r\n", 1)[0] ?? "closed"));
		setTimeout(() => {
			socket.destroy();
			resolve(data.split("\r\n", 1)[0] ?? "timeout");
		}, 5000);
	});
}

const deniedUpgrade = await upgrade("/api/remote.mux", "Cookie: __dsh_remote=nope\r\n");
check(deniedUpgrade !== "HTTP/1.1 101 Switching Protocols", "upgrade without a valid cookie is refused", deniedUpgrade);

const grantedUpgrade = await upgrade("/api/remote.mux", `Cookie: ${gate}\r\n`);
check(grantedUpgrade === "HTTP/1.1 101 Switching Protocols", "upgrade with the gate cookie is proxied to dsh", grantedUpgrade);

// --- QR ------------------------------------------------------------------
const qr = await raw(`${base}/remote/qr.svg`);
const qrBody = qr.status === 200 ? await qr.text() : "";
check(qr.status === 200 && qrBody.includes("<svg"), "QR route returns an SVG", `${String(qr.status)} / ${String(qrBody.length)}B`);

note(`proxy port ${String(proxyPort)}`);
const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nREMOTE LIVE PASS (${String(results.length)})` : `\nREMOTE LIVE FAIL (${String(failed.length)})`);
if (failed.length > 0) process.exitCode = 1;
