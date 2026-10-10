/**
 * Unit checks for the remote proxy's enhancement pieces: the WebSocket frame
 * bridge (lib/wsbridge.js), the response transforms (lib/enhance.js), the
 * phone project backend (lib/project.js) and the patch-preference reader
 * (lib/preference.js).
 *
 * Run from a plugin location where dependencies resolve (the sandbox or the
 * deployed profile copy):
 *   "$RUNTIME/node.exe" tests/proxy.test.mjs
 */
import { createServer, connect } from "node:net";
import { PassThrough, Readable } from "node:stream";
import { finished } from "node:stream/promises";
import { gunzipSync } from "node:zlib";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FrameSplitter, bridgeSockets } from "../lib/wsbridge.js";
import {
	clientAcceptsGzip,
	gzipEligibleType,
	isEventStream,
	isHtmlDocument,
	pipeSseWithKeepalive,
	respond,
	rewriteHtmlDocument,
	wantsHtmlDocument
} from "../lib/enhance.js";
import { contained, contentDisposition, handleProjectRoute, listDir, mimeFor, readProjectFile, resolveWorkspace } from "../lib/project.js";
import { isZhLocale, readPreference, readUiPreferences } from "../lib/preference.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

/** Wait until the predicate holds, polling briefly. */
async function waitFor(predicate, timeoutMs = 3000) {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		if (predicate()) return true;
		await new Promise((resolve) => setTimeout(resolve, 15));
	}
	return predicate();
}

/** A fake remote response that records headers and collects the body. */
function fakeRes() {
	const res = new PassThrough();
	const chunks = [];
	res.on("data", (chunk) => chunks.push(chunk));
	res.writeHead = (status, headers) => {
		res.status = status;
		res.headers = headers;
	};
	res.collected = async () => {
		await finished(res);
		return Buffer.concat(chunks);
	};
	return res;
}

/** A fake upstream response: a one-shot readable with header fields attached. */
function fakeUpstream(body, headers, statusCode = 200) {
	const upstream = Readable.from([body]);
	upstream.headers = headers;
	upstream.statusCode = statusCode;
	return upstream;
}

/* ---------------------------------------------------------- frame splitter */

/** An unmasked server frame. */
const serverFrame = (payload, first = 0x81) => Buffer.concat([Buffer.from([first, payload.length]), payload]);
/** A masked client frame. */
function clientFrame(payload) {
	const mask = Buffer.from([1, 2, 3, 4]);
	const masked = Buffer.from(payload);
	for (let index = 0; index < masked.length; index++) masked[index] ^= mask[index % 4];
	return Buffer.concat([Buffer.from([0x81, 0x80 | payload.length]), mask, masked]);
}

const hello = serverFrame(Buffer.from("hello"));
const splitter = new FrameSplitter();
check(splitter.push(hello).length === 1, "a whole frame parses at once");
check(splitter.push(hello)[0].equals(hello), "the frame forwards its raw bytes verbatim");

const bytewise = new FrameSplitter();
let frames = [];
for (const byte of hello) frames = frames.concat(bytewise.push(Buffer.from([byte])));
check(frames.length === 1 && frames[0].equals(hello), "a frame split into single bytes still reassembles");

const two = new FrameSplitter();
const frag1 = serverFrame(Buffer.from("Hel"), 0x01); // FIN=0, text
const frag2 = serverFrame(Buffer.from("lo"), 0x80); // FIN=1, continuation
const both = two.push(Buffer.concat([frag1, frag2]));
check(both.length === 2 && both[0][0] === 0x01 && both[1][0] === 0x80, "fragmented messages forward as their own frames");

const maskedSplitter = new FrameSplitter();
const maskedOut = maskedSplitter.push(clientFrame(Buffer.from("ping")));
check(maskedOut.length === 1 && maskedOut[0].equals(clientFrame(Buffer.from("ping"))), "masked client frames forward mask-and-all");

const ext = new FrameSplitter();
const big = Buffer.alloc(300, 0x61);
const extFrame = Buffer.concat([Buffer.from([0x82, 126, 0x01, 0x2c]), big]);
check(ext.push(extFrame)[0]?.equals(extFrame), "16-bit extended length frames parse", String(extFrame.length));

const wide = new FrameSplitter();
const widePayload = Buffer.alloc(70000, 0x62);
const wideHeader = Buffer.alloc(10);
wideHeader[0] = 0x82;
wideHeader[1] = 127;
wideHeader.writeBigUInt64BE(BigInt(70000), 2);
const wideFrame = Buffer.concat([wideHeader, widePayload]);
let wideFrames = [];
for (let at = 0; at < wideFrame.length; at += 9999) wideFrames = wideFrames.concat(wide.push(wideFrame.subarray(at, at + 9999)));
check(wideFrames.length === 1 && wideFrames[0].equals(wideFrame), "64-bit length frames parse across chunk boundaries");

const partial = new FrameSplitter();
check(partial.push(hello.subarray(0, 3)).length === 0 && partial.pendingBytes === 3, "an incomplete frame is held back");
check(partial.push(hello.subarray(3))[0]?.equals(hello), "the remainder completes the held frame");

let oversized = false;
try {
	const monster = Buffer.alloc(10);
	monster[0] = 0x82;
	monster[1] = 127;
	monster.writeBigUInt64BE(BigInt("0x100000000"), 2);
	new FrameSplitter().push(monster);
} catch {
	oversized = true;
}
check(oversized, "an absurd frame size drops the bridge instead of buffering forever");

/* ------------------------------------------------------------------ bridge */

// A minimal "dsh": answer the upgrade, send one frame, then stay silent so
// the bridge's idle Ping can fire.
const dsh = createServer((socket) => {
	socket.once("data", () => {
		socket.write("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: x\r\n\r\n");
		setTimeout(() => socket.write(serverFrame(Buffer.from("hello"))), 30);
	});
});
await new Promise((resolve) => dsh.listen(0, "127.0.0.1", resolve));
const dshPort = dsh.address().port;
const dshReceived = [];
dsh.on("connection", (socket) => socket.on("data", (chunk) => dshReceived.push(Buffer.from(chunk))));

const browserSide = createServer((proxySocket) => {
	const upstream = connect(dshPort, "127.0.0.1");
	upstream.once("connect", () => {
		upstream.write("GET /api/remote.mux HTTP/1.1\r\nhost: 127.0.0.1\r\nupgrade: websocket\r\n\r\n");
		bridgeSockets(proxySocket, upstream, { idlePingMs: 80 });
	});
});
await new Promise((resolve) => browserSide.listen(0, "127.0.0.1", resolve));

const browser = connect(browserSide.address().port, "127.0.0.1");
const browserChunks = [];
browser.on("data", (chunk) => browserChunks.push(Buffer.from(chunk)));
await new Promise((resolve) => browser.on("connect", resolve));
// The "phone" speaks: a masked frame must reach dsh byte-for-byte.
browser.write(clientFrame(Buffer.from("from-phone")));

check(await waitFor(() => Buffer.concat(browserChunks).toString("latin1").includes("101 Switching Protocols")), "the 101 handshake reaches the browser");
check(await waitFor(() => Buffer.concat(browserChunks).includes(Buffer.from("hello"))), "a downstream frame reaches the browser");
check(await waitFor(() => Buffer.concat(dshReceived).includes(clientFrame(Buffer.from("from-phone")))), "the masked client frame reaches dsh verbatim");
// After the silence window, the proxy must have emitted its own Ping frame.
const pingSeen = await waitFor(() => Buffer.concat(browserChunks).includes(Buffer.from([0x89, 0x00])), 2000);
check(pingSeen, "an idle downstream triggers the proxy Ping");

browser.destroy();
dsh.close();
browserSide.close();

/* ------------------------------------------------------------ enhance pure */

check(clientAcceptsGzip({ "accept-encoding": "gzip, br" }), "gzip acceptance parses");
check(clientAcceptsGzip({ "accept-encoding": "br, gzip" }), "gzip acceptance order-free");
check(!clientAcceptsGzip({ "accept-encoding": "xgzip, br" }), '"xgzip" is not gzip');
check(!clientAcceptsGzip({}), "missing accept-encoding means no gzip");
check(gzipEligibleType("text/css; charset=utf-8") && gzipEligibleType("application/javascript"), "compressible types are eligible");
check(!gzipEligibleType("image/png") && !gzipEligibleType(undefined), "binary/missing types are not eligible");
check(isEventStream("text/event-stream; charset=utf-8") && !isEventStream("text/html"), "event-stream detection");
check(isHtmlDocument("text/html; charset=utf-8") && !isHtmlDocument("application/json"), "html detection");
check(wantsHtmlDocument({ accept: "text/html,application/xhtml+xml" }), "document navigation detected");
check(!wantsHtmlDocument({ accept: "*/*" }), "asset fetches are not document navigations");

/* --------------------------------------------------------- html rewriting */

const assets = { mobileCss: "MCSS", mobileJs: "MJS", splashCss: "SCSS", splashJs: "SJS" };
const doc = Buffer.from(
	'<!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1"><title>x</title></head><body><div id="root"></div></body></html>',
	"utf8"
);
const rewritten = rewriteHtmlDocument(doc, assets).toString("utf8");
check(rewritten.includes("maximum-scale=1, user-scalable=no"), "the viewport gains the no-zoom form");
check(rewritten.includes("<!-- dshdesk-mobile -->"), "the inject marker is present");
check(rewritten.includes("MCSS") && rewritten.includes("MJS"), "mobile assets are injected");
check(rewritten.includes("SCSS") && rewritten.includes("SJS") && rewritten.includes("dsh-splash"), "the splash overlay is injected");
check(rewritten.includes('<div id="root"></div>'), "the mount point itself is preserved");

const oddCase = rewriteHtmlDocument(Buffer.from('<head></HEAD><div id="root"></div>', "utf8"), assets).toString("utf8");
check(oddCase.includes("<!-- dshdesk-mobile -->"), "</HEAD> in any case still anchors the injection");

// A document missing only some needles gets only the injections whose anchor survived.
const partialDoc = rewriteHtmlDocument(Buffer.from("<html><head></head><body>no viewport, no root</body></html>", "utf8"), assets).toString("utf8");
check(partialDoc.includes("<!-- dshdesk-mobile -->") && !partialDoc.includes("dsh-splash"), "a missing splash needle skips the splash but keeps the head injection");

const drifted = Buffer.from("<html><body>no needles at all</body></html>", "utf8");
const untouched = rewriteHtmlDocument(drifted, assets).toString("utf8");
check(!untouched.includes("dshdesk-mobile") && !untouched.includes("dsh-splash"), "a drifted document passes through without injections");

/* ----------------------------------------------------------------- respond */

// HTML document navigation → rewritten document.
{
	const res = fakeRes();
	const upstream = fakeUpstream(doc, { "content-type": "text/html; charset=utf-8", "content-length": String(doc.length) });
	await respond({ req: { method: "GET", headers: { accept: "text/html" } }, res, upstreamRes: upstream, assets });
	const body = (await res.collected()).toString("utf8");
	check(body.includes("<!-- dshdesk-mobile -->"), "respond rewrites the SPA document");
	check(res.headers["set-cookie"] === undefined, "upstream cookies never reach the remote side");
}

// Compressible asset + gzip client → gzipped body.
{
	const bigCss = Buffer.from("body { color: red; }\n".repeat(600), "utf8");
	const res = fakeRes();
	const upstream = fakeUpstream(bigCss, { "content-type": "text/css", "content-length": String(bigCss.length), "set-cookie": "dsh-auth-x" });
	await respond({ req: { method: "GET", headers: { "accept-encoding": "gzip, br", accept: "*/*" } }, res, upstreamRes: upstream, assets });
	const body = await res.collected();
	check(res.headers["content-encoding"] === "gzip", "respond gzips an eligible asset");
	check(body[0] === 0x1f && body[1] === 0x8b, "the body really is gzip");
	check(gunzipSync(body).equals(bigCss), "the gzip round-trips");
	check(res.headers["set-cookie"] === undefined, "gzip path also strips upstream cookies");
}

// Ineligible type → identity passthrough.
{
	const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 1, 2, 3]);
	const res = fakeRes();
	const upstream = fakeUpstream(png, { "content-type": "image/png", "content-length": String(png.length) });
	await respond({ req: { method: "GET", headers: { "accept-encoding": "gzip", accept: "*/*" } }, res, upstreamRes: upstream, assets });
	check((await res.collected()).equals(png) && res.headers["content-encoding"] === undefined, "binary assets pass through identity");
}

// Over the rewrite limit → everything streams through byte-for-byte (no truncation).
{
	const head = Buffer.alloc(8 * 1024 * 1024, 0x62);
	const middle = Buffer.alloc(8 * 1024 * 1024, 0x63);
	const over = Buffer.alloc(1 * 1024 * 1024, 0x64);
	const tail = Buffer.from("<!-- TAILMARK -->", "utf8");
	const res = fakeRes();
	const upstream = Readable.from([head, middle, over, tail]);
	upstream.headers = { "content-type": "text/html", "content-length": String(head.length + middle.length + over.length + tail.length) };
	upstream.statusCode = 200;
	await respond({ req: { method: "GET", headers: { accept: "text/html" } }, res, upstreamRes: upstream, assets });
	const body = await res.collected();
	check(body.equals(Buffer.concat([head, middle, over, tail])), "an over-limit body streams through byte-identical", `${body.length} vs ${head.length + middle.length + over.length + tail.length}`);
	check(body.subarray(-tail.length).equals(tail), "the bytes after the overflow chunk are not dropped");
	check(res.status === 200 && res.headers["set-cookie"] === undefined, "the overflow passthrough keeps status and strips cookies");
}

// A single chunk larger than the limit must not vanish.
{
	const body = Buffer.alloc(20 * 1024 * 1024, 0x61);
	const res = fakeRes();
	const upstream = Readable.from([body]);
	upstream.headers = { "content-type": "text/html" };
	upstream.statusCode = 200;
	await respond({ req: { method: "GET", headers: { accept: "text/html" } }, res, upstreamRes: upstream, assets });
	const got = await res.collected();
	check(got.equals(body), "a single chunk over the limit passes through whole", `${got.length}/${body.length}`);
}

// SSE → keepalive injection on silence.
{
	const res = fakeRes();
	const upstream = new PassThrough();
	upstream.headers = { "content-type": "text/event-stream" };
	upstream.statusCode = 200;
	const produced = [];
	res.on("data", (chunk) => produced.push(String(chunk)));
	res.writeHead(200, {});
	pipeSseWithKeepalive(upstream, res, 60);
	upstream.write("data: one\n\n");
	await new Promise((resolve) => setTimeout(resolve, 150));
	upstream.write("data: two\n\n");
	upstream.end();
	await finished(res);
	const wire = produced.join("");
	check(wire.includes(": keepalive\n\n"), "an idle event stream gets a keepalive comment", wire.replaceAll("\n", "\\n"));
	check(wire.includes("data: one") && wire.includes("data: two"), "real events flow through unchanged");
}

/* ---------------------------------------------------------------- project */

const dir = await mkdtemp(join(tmpdir(), "kit-project-"));
const ws = join(dir, "ws-a");
await mkdir(join(ws, "src"), { recursive: true });
await writeFile(join(ws, "README.md"), "# 标题\n", "utf8");
await writeFile(join(ws, "src", "main.rs"), "fn main() {}\n", "utf8");
await mkdir(join(dir, "storages"), { recursive: true });
const storePath = join(dir, "storages", "workspace.json");
await writeFile(storePath, JSON.stringify({ tables: { workspaces: { w1: { path: ws, title: "A 项目", sessionIds: ["session-aaa"] } } } }), "utf8");

const resolved = await resolveWorkspace(storePath, "session-aaa");
check(resolved?.title === "A 项目" && resolved.root === ws, "workspace resolves by session id");
check((await resolveWorkspace(storePath, "session-zzz")) === undefined, "an unknown session resolves to nothing");
await writeFile(storePath, `﻿${JSON.stringify({ tables: { workspaces: { w1: { path: ws, sessionIds: ["session-aaa"] } } } })}`, "utf8");
check((await resolveWorkspace(storePath, "session-aaa"))?.title === "ws-a", "a BOM is tolerated and the title falls back to the directory name");
await writeFile(storePath, "not json", "utf8");
check((await resolveWorkspace(storePath, "session-aaa")) === undefined, "a corrupt store resolves to nothing");
await writeFile(storePath, JSON.stringify({ tables: { workspaces: { w1: { path: ws, title: "A 项目", sessionIds: ["session-aaa"] } } } }), "utf8");

check((await contained(ws, "")).length > 0, "the workspace root itself is contained");
check((await contained(ws, "src")).length > 0, "a plain subdirectory is contained");
for (const bad of ["..", "src/../../x", "/etc/passwd", "C:/Windows", "./src", "src\\x"]) {
	let forbiddenHit = false;
	try {
		await contained(ws, bad);
	} catch (error) {
		forbiddenHit = error?.code === "forbidden";
	}
	check(forbiddenHit, `traversal ${JSON.stringify(bad)} is refused`);
}
let missingHit = false;
try {
	await contained(ws, "nope.txt");
} catch (error) {
	missingHit = error?.code === "not_found";
}
check(missingHit, "a missing path reports not_found");

const listing = await listDir(ws, "");
check(listing.entries[0]?.kind === "dir" && listing.entries[0].name === "src", "listings sort directories first", JSON.stringify(listing.entries));
check(listing.truncated === false, "small listings are not truncated");
check((await listDir(ws, "", 1)).truncated === true, "the cap truncates and reports");

check(mimeFor("a.PNG") === "image/png", "mime by extension is case-insensitive");
check(mimeFor("b.md") === "text/markdown; charset=utf-8", "markdown mime");
check(mimeFor("Makefile") === "application/octet-stream", "extensionless files are octet-stream");
check(contentDisposition("走势图 01.png") === "attachment; filename*=UTF-8''%E8%B5%B0%E5%8A%BF%E5%9B%BE%2001.png", "RFC 5987 disposition for non-ASCII names");
check(contentDisposition("a-b_c.txt") === "attachment; filename*=UTF-8''a-b_c.txt", "ASCII names pass through");

const readBack = await readProjectFile(ws, "README.md", false);
check(readBack.mime.startsWith("text/markdown") && readBack.buffer.toString("utf8").includes("标题"), "a text file reads with its mime");
const bigPath = join(ws, "big.log");
await writeFile(bigPath, Buffer.alloc(8 * 1024 * 1024 + 16, 0x61), "utf8");
let tooLargeHit = false;
try {
	await readProjectFile(ws, "big.log", false);
} catch (error) {
	tooLargeHit = error?.code === "too_large";
}
check(tooLargeHit, "text preview beyond the cap is refused");
check((await readProjectFile(ws, "big.log", true)).buffer.length > 8 * 1024 * 1024, "downloads are exempt from the text cap");

// The routed surface: page, resolve/list/file, and gate-adjacent errors.
{
	const call = async (path, method = "GET") => {
		const res = fakeRes();
		res.end = ((original) => (body) => original.call(res, body))(res.end);
		const handled = await handleProjectRoute({ method }, res, new URL(path, "http://x"), { workspaceStorePath: storePath, page: "PAGE" });
		if (!handled) {
			res.end();
			return { handled, res, body: Buffer.alloc(0) };
		}
		return { handled, res, body: await res.collected() };
	};
	const page = await call("/dshdesk/remote/project");
	check(page.handled && page.res.status === 200 && page.body.toString("utf8") === "PAGE", "the project page is served");
	const resolvedApi = await call("/dshdesk/remote/api/resolve?sid=session-aaa");
	check(JSON.parse(resolvedApi.body.toString("utf8")).title === "A 项目", "resolve answers the workspace");
	const unknown = await call("/dshdesk/remote/api/resolve?sid=session-zzz");
	check(unknown.res.status === 404 && JSON.parse(unknown.body.toString("utf8")).error === "not_found", "an unknown session 404s");
	const list = await call("/dshdesk/remote/api/list?sid=session-aaa&rel=");
	check(JSON.parse(list.body.toString("utf8")).entries.length === 3, "list answers the root entries");
	const escape = await call("/dshdesk/remote/api/list?sid=session-aaa&rel=..");
	check(escape.res.status === 403, "a traversal rel is 403");
	const file = await call("/dshdesk/remote/api/file?sid=session-aaa&rel=README.md");
	check(file.res.headers["content-type"].startsWith("text/markdown"), "file answers with its mime");
	const download = await call("/dshdesk/remote/api/file?sid=session-aaa&rel=README.md&download=1");
	check(String(download.res.headers["content-disposition"]).startsWith("attachment;"), "download sets content-disposition");
	const outside = await call("/dshdesk/api/state");
	check(outside.handled === false, "non-project paths fall through to the proxy");
}

/* ------------------------------------------------------------- preference */

const patchPath = join(dir, "cordis.patch.yml");
const patchLayout = { patchPath };
await writeFile(patchPath, "- id: locale\n  name: '@deepseek-ai/dsh-client-locale'\n  config:\n    preference: zh\n- id: ui-theme\n  name: '@deepseek-ai/dsh-client-ui-theme'\n  config:\n    preference: dark\n", "utf8");
check((await readPreference(patchLayout, "locale")) === "zh", "the locale row's preference is read");
check((await readPreference(patchLayout, "ui-theme")) === "dark", "the ui-theme row's preference is read");
const prefs = await readUiPreferences(patchLayout);
check(prefs.locale === "zh" && prefs.theme === "dark", "both preferences come back together");
check((await readPreference(patchLayout, "missing")) === undefined, "an absent row yields undefined");
await writeFile(patchPath, "- insert:\n    - id: mcp-x\n      name: y\n", "utf8");
check((await readPreference(patchLayout, "locale")) === undefined, "insert ops are not preference rows");
await writeFile(patchPath, "{ not: yaml seq", "utf8");
check((await readPreference(patchLayout, "locale")) === undefined, "a corrupt patch yields undefined");
check((await readPreference({ patchPath: join(dir, "missing.yml") }, "locale")) === undefined, "a missing patch yields undefined");

/* ------------------------------------------- language rule (isZhLocale) */

// zh* is Chinese; every other tag, and anything that is not a usable tag, is
// English. The panel falls back to navigator.language only when the tag is
// absent or empty, which is why "" answers false here rather than throwing.
check(isZhLocale("zh"), "zh is Chinese");
check(isZhLocale("zh-CN"), "zh-CN is Chinese");
check(isZhLocale("zh-TW"), "zh-TW is Chinese");
check(isZhLocale("zh-Hans"), "zh-Hans is Chinese");
check(isZhLocale("ZH-cn"), "the tag is matched case-insensitively");
check(isZhLocale("  zh  "), "surrounding whitespace is ignored");
check(isZhLocale("en") === false, "en is English");
check(isZhLocale("en-US") === false, "en-US is English");
check(isZhLocale("fr") === false, "fr falls back to English");
check(isZhLocale("ja") === false, "ja falls back to English");
check(isZhLocale("de") === false, "de falls back to English");
check(isZhLocale("") === false, "an empty tag is not Chinese");
check(isZhLocale(undefined) === false, "an absent tag is not Chinese");
check(isZhLocale(null) === false, "a null tag is not Chinese");
check(isZhLocale(42) === false, "a non-string tag is not Chinese");

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nPROXY TEST PASS (${results.length} checks)` : `\nPROXY TEST FAIL (${failed.length}/${results.length})`);
await rm(dir, { recursive: true, force: true });
if (failed.length > 0) process.exitCode = 1;
