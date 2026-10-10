/**
 * Regression check for the remote proxy's 401 re-authentication replay.
 *
 * When dsh answered a proxied request with 401 (its `dsh-auth` session cookie
 * had gone stale), the proxy re-exchanged the launch token for a fresh cookie
 * and replayed the buffered request. The replay used to re-enter `forward`,
 * which re-registered `req`'s `data`/`end` listeners on a request that had
 * already emitted `end`: EventEmitter never re-emits `end`, so the replay was
 * never sent and the phone's request hung until timeout — and every later
 * remote request hung the same way until the plugin was restarted.
 *
 * This drives the real manager against a fake upstream. The fake mints a
 * rotated cookie on every exchange and only accepts the newest one, so a
 * replay that (wrongly) kept the pre-401 cookie cannot pass either.
 *
 * Run from a plugin location where dependencies resolve:
 *   "$RUNTIME/node.exe" tests/remote-replay.test.mjs
 */
import { createServer } from "node:http";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRemoteManager } from "../lib/remote.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

const dir = await mkdtemp(join(tmpdir(), "kit-replay-"));
let exchanges = 0;
let business = 0;

/**
 * A fake dsh: the launch-token exchange mints a fresh, rotating cookie; the
 * first business request answers 401 (the stale-cookie case), and later ones
 * answer 200 only when they present the cookie from the latest exchange.
 */
const upstream = createServer((req, res) => {
	const url = new URL(req.url ?? "/", "http://x");
	if (url.searchParams.get("token") === "LAUNCH") {
		exchanges += 1;
		res.writeHead(200, { "set-cookie": `dsh-auth=v${String(exchanges)}; Path=/; HttpOnly` });
		res.end("ok");
		return;
	}
	business += 1;
	const fresh = String(req.headers.cookie ?? "").includes(`dsh-auth=v${String(exchanges)}`);
	if (business === 1 || !fresh) {
		res.writeHead(401);
		res.end("stale");
		return;
	}
	res.writeHead(200, { "content-type": "text/plain" });
	res.end("HELLO");
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));

const remote = { enabled: false, mode: "lan", port: 0, token: "", cloudflaredPath: "" };
const ctx = {
	get(name) {
		if (name === "webServer") return { port: upstream.address().port };
		if (name === "connection") return { authenticatedUrl: (base) => `${base}/?token=LAUNCH`, browserAuth: { launchToken: "LAUNCH" } };
		return undefined;
	}
};
const manager = createRemoteManager({
	ctx,
	layout: () => ({ dshHome: dir, profileDir: dir, workspaceStorePath: join(dir, "workspace.json"), patchPath: join(dir, "cordis.patch.yml") }),
	config: async () => ({ remote }),
	save: async (patch) => {
		Object.assign(remote, patch.remote);
		return { remote };
	},
	log: () => {}
});

/** One gated request; the timeout turns a hung replay into a failure, not a hang. */
async function ask(port) {
	try {
		const response = await fetch(`http://127.0.0.1:${String(port)}/`, {
			headers: { cookie: `__dsh_remote=${remote.token}` },
			signal: AbortSignal.timeout(4000)
		});
		return { status: response.status, body: await response.text() };
	} catch (cause) {
		return { error: String(cause?.name ?? cause) };
	}
}

const snapshot = await manager.enable("lan");
check(exchanges === 1, "enable exchanges the launch token once up front", String(exchanges));

const first = await ask(snapshot.proxyPort);
check(first.status === 200 && first.body === "HELLO", "a stale cookie is re-exchanged and the request replayed", first.error ?? `${String(first.status)} ${JSON.stringify(first.body)}`);
check(business === 2, "the replayed request reaches upstream a second time", String(business));
check(exchanges === 2, "the replay follows a fresh launch-token exchange", String(exchanges));

const after = await ask(snapshot.proxyPort);
check(after.status === 200 && after.body === "HELLO", "requests after a replay are still served", after.error ?? `${String(after.status)} ${JSON.stringify(after.body)}`);

await manager.disable();
await new Promise((resolve) => upstream.close(resolve));
await rm(dir, { recursive: true, force: true });

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nREMOTE REPLAY TEST PASS (${results.length} checks)` : `\nREMOTE REPLAY TEST FAIL (${failed.length}/${results.length})`);
if (failed.length > 0) process.exitCode = 1;
