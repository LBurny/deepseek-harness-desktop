/**
 * Route-guard checks for the kit's HTTP surface (lib/http.js): a RequestError is
 * the caller's mistake and must answer 400, while any other failure stays a 500
 * and a response already being streamed is only ended.
 *
 * Run from the repository root:
 *   "$RUNTIME/node.exe" tests/http.test.mjs
 */
import { RequestError, guard } from "../lib/http.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

/** Minimal response double recording the status and JSON body. */
function fakeResponse() {
	return {
		headersSent: false,
		status: undefined,
		body: undefined,
		writeHead(status) {
			this.status = status;
			this.headersSent = true;
		},
		end(text) {
			this.body = text === undefined ? "" : String(text);
		}
	};
}

/** Run a guarded handler against a fresh response double. */
async function call(handler) {
	const res = fakeResponse();
	await handler({}, res);
	return res;
}

const badRequest = await call(guard(async () => {
	throw new RequestError('unknown skills action: "nope"');
}));
check(badRequest.status === 400, "a RequestError answers 400", String(badRequest.status));
check(JSON.parse(badRequest.body).error.startsWith("unknown skills action"), "the message is forwarded", badRequest.body);

const serverFault = await call(guard(async () => {
	throw new Error("disk on fire");
}));
check(serverFault.status === 500, "any other error still answers 500", String(serverFault.status));

const ok = await call(guard(async (req, res) => {
	res.writeHead(200);
	res.end("{}");
}));
check(ok.status === 200, "a successful handler is untouched", String(ok.status));

// A response already being streamed must only be ended, never re-headed.
const streamed = fakeResponse();
streamed.headersSent = true;
await guard(async () => {
	throw new RequestError("too late");
})({}, streamed);
check(streamed.status === undefined && streamed.body === "", "a sent response is only ended", String(streamed.body));

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nHTTP TEST PASS (${results.length} checks)` : `\nHTTP TEST FAIL (${failed.length}/${results.length})`);
if (failed.length > 0) process.exitCode = 1;
