/**
 * Notification bus checks: SSE attach/detach, publish fan-out, and the recent
 * buffer. No filesystem and no running harness needed.
 *
 * Run from the repository root:
 *   "$RUNTIME/node.exe" tests/bus.test.mjs
 */
import { createNotifyBus } from "../lib/bus.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

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

// A reminder record carries its session id end to end, which is what a
// notification click needs to jump back to the conversation.
const tagged = bus.publish({ kind: "turn_done", sessionId: "s-42", title: "t", text: "done" });
check(tagged.sessionId === "s-42", "publish forwards the session id", String(tagged.sessionId));
check(stream.chunks.join("").includes(`"sessionId":"s-42"`), "the session id reaches the stream");
check(bus.recent().at(-1).sessionId === "s-42", "recent keeps the session id");

detach();
check(bus.size() === 0, "detach removes the stream");
bus.publish({ kind: "question", text: "after detach" });
check(!stream.chunks.join("").includes("after detach"), "detached stream receives nothing");
check(bus.recent().length === 3, "recent still records after detach");

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nBUS TEST PASS (${results.length} checks)` : `\nBUS TEST FAIL (${failed.length}/${results.length})`);
if (failed.length > 0) process.exitCode = 1;
