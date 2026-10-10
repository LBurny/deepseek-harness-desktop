/**
 * The guarded `/init` fallback: it must stay opt-in, and the message it injects
 * must carry a producer-owned source kind.
 *
 * Run from the repository root:
 *   "$RUNTIME/node.exe" tests/init.test.mjs
 *
 * The regression this pins: dsh 0.2.0's session format V4 retired the generic
 * `kind: "plugin"` wrapper, so a message written with it throws
 * `format v4 message requires a producer-owned source kind` at append time and
 * the whole `/init` turn dies. The command still registers and still returns
 * success, which is exactly why only a source-shape check catches it.
 */
import { apply, inject, name } from "../lib/index.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

/** Build a plugin context that records the registered commands and nothing else. */
function fakeContext() {
	const commands = new Map();
	const context = {
		commands: {
			register(definition) {
				if (commands.has(definition.name)) throw new Error(`command "${definition.name}" is already registered`);
				commands.set(definition.name, definition);
			}
		},
		logger: { warn() {}, info() {}, error() {} },
		get() { return undefined; },
		inject() {},
		on() {},
		effect() {}
	};
	return { context, commands };
}

/** Run `apply` and hand back the registered command table. */
function mount(config) {
	const { context, commands } = fakeContext();
	apply(context, config);
	return commands;
}

/* ------------------------------------------------- shape of the plugin row */

check(name === "desktop-kit", "module name stays the row's short name", name);
check(inject.includes("commands"), "the row still declares the commands service");

/* -------------------------------------------------- opt-in: default is off */

const off = mount(undefined);
check(!off.has("init"), "provideInit defaults to off: /init is not registered");
for (const command of ["skills", "mcp", "doctor", "kit"]) check(off.has(command), `/${command} is registered regardless`);

const offExplicit = mount({ provideInit: false });
check(!offExplicit.has("init"), "provideInit: false still leaves /init to dsh-command-init");

/* ------------------------------------------------------ opt-in: turns on */

const on = mount({ provideInit: true });
check(on.has("init"), "provideInit: true registers the /init fallback");

const definition = on.get("init");
check(definition.description.length > 0, "the fallback has a non-empty description");
check(typeof definition.input?.hint === "string" && definition.input.hint.length > 0, "the fallback has a non-empty input hint");

/* --------------------------------- the injected message and its source */

let message;
const invocation = {
	rawInput: "  keep it short  ",
	agent: {
		session: { header: { cwd: "H:/ws" } },
		followup(value) { message = value; }
	}
};
const result = await definition.handler(invocation);

check(result?.kind === "success", "the handler reports success", String(result?.kind));
check(message !== undefined, "the handler submits a message to the agent");
check(message.source !== undefined, "the submitted message carries a source");
check(
	message.source.kind !== "plugin",
	"the source kind is producer-owned, not the retired \"plugin\" wrapper",
	String(message.source.kind)
);
check(
	message.source.kind === "plugin:dsh-desktop-kit",
	"the source kind is the canonical plugin:<package> identity",
	String(message.source.kind)
);
check(message.source.form === "notice", "the source keeps the collapsible notice form", String(message.source.form));
check(
	typeof message.source.summary === "string" && message.source.summary.length > 0 && message.source.summary.length <= 120,
	"the source summary is a non-empty, short one-liner",
	String(message.source.summary)
);
check(
	Array.isArray(message.content) && message.content.some((block) => typeof block.text === "string" && block.text.includes("H:/ws")),
	"the prompt names the workspace it was invoked in"
);
check(
	message.content.some((block) => block.text.includes("keep it short")),
	"the prompt carries the extra arguments"
);

/* -------------------------------- the runtime's own admission, when present */

let admission;
try {
	const { releasedV4SessionFormatCodec } = await import("@deepseek-ai/dsh-session-format-v3-to-v4");
	// A fixture first: without it a passing probe proves nothing, since a missing
	// package would also "pass".
	let fixtureRejected = false;
	try {
		releasedV4SessionFormatCodec.encodeEvent({
			type: "user/message",
			seq: 1,
			data: { ...message, source: { kind: "plugin", plugin: "dsh-desktop-kit", form: "notice", summary: "x" } }
		});
	} catch {
		fixtureRejected = true;
	}
	check(fixtureRejected, "fixture: this runtime rejects the retired kind:plugin source");
	try {
		releasedV4SessionFormatCodec.encodeEvent({ type: "user/message", seq: 1, data: message });
		admission = "accepted";
	} catch (error) {
		admission = String(error?.message ?? error);
	}
} catch (error) {
	admission = "skipped";
	// A bare clone without the @deepseek-ai junction cannot resolve the runtime's
	// session-format package. The shape checks above still pin the contract.
	check(true, "session-format admission probe skipped (package not resolvable)", String(error?.message ?? error));
}
if (admission !== "skipped") check(admission === "accepted", "the message passes dsh's V4 source admission", admission);

/* -------------------------------------------------------------- reporting */

const failed = results.filter((item) => !item.ok);
for (const item of results) console.log(`${item.ok ? "ok  " : "FAIL"} ${item.label}${item.detail ? ` — ${item.detail}` : ""}`);
console.log(failed.length === 0 ? `\nINIT TEST PASS (${results.length} checks)` : `\nINIT TEST FAIL (${failed.length}/${results.length})`);
if (failed.length > 0) process.exitCode = 1;
