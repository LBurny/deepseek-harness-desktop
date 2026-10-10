/**
 * Notification record checks: the host half's classification of harness events
 * into session-tagged bus records (lib/notify.js). No harness needed.
 *
 * Run from the repository root:
 *   "$RUNTIME/node.exe" tests/notify.test.mjs
 */
import { ANSWER_DONE, APPROVAL, QUESTION, TURN_DONE, attentionRecord, turnRecord } from "../lib/notify.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

/* ----------------------------------------------------------- turn records */

const worked = turnRecord({ sessionId: "s1", title: "fix the bug", worked: true, reason: "completed", origin: undefined });
check(worked.kind === TURN_DONE, "a worked turn is turn_done", worked.kind);
check(worked.sessionId === "s1", "turn record carries the session id");
check(worked.title === "fix the bug", "turn record carries the title");
check(worked.text === undefined, "the record is language-neutral (no baked sentence)", String(worked.text));

const answered = turnRecord({ sessionId: "s2", title: undefined, worked: false, reason: "completed", origin: undefined });
check(answered.kind === ANSWER_DONE, "a pure reply is answer_done", answered.kind);
check(answered.title === undefined, "a titleless turn keeps title undefined");
check(answered.text === undefined, "a titleless reply also carries no prose", String(answered.text));

const subagent = turnRecord({ sessionId: "s3", title: "sub", worked: true, reason: "completed", origin: "subagent" });
check(subagent === undefined, "a subagent turn is suppressed");

for (const reason of ["aborted", "error", "blocked", "max-tokens"]) {
	const dropped = turnRecord({ sessionId: "s4", title: "x", worked: true, reason, origin: undefined });
	check(dropped === undefined, `a non-completed turn (${reason}) is suppressed`);
}

// The jump target must survive even when no title was ever reported.
const titleless = turnRecord({ sessionId: "s5", title: undefined, worked: true, reason: "completed", origin: undefined });
check(titleless.sessionId === "s5", "a titleless turn still tags its session");

/* -------------------------------------------------------- attention records */

const approval = attentionRecord({ kind: APPROVAL, sessionId: "s6", title: "deploy" });
check(approval.kind === APPROVAL, "approval keeps its kind");
check(approval.sessionId === "s6", "approval carries the session id", approval.sessionId);
check(approval.title === "deploy", "approval carries the title");
check(approval.text === undefined, "approval carries no baked sentence", String(approval.text));

const question = attentionRecord({ kind: QUESTION, sessionId: "s7", title: undefined });
check(question.kind === QUESTION, "question keeps its kind");
check(question.sessionId === "s7", "question carries the session id");
check(question.title === undefined, "a titleless question keeps title undefined");
check(question.text === undefined, "question carries no baked sentence", String(question.text));

/* -------------------------------------------------------------- reporting */

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? `\nNOTIFY TEST PASS (${results.length} checks)` : `\nNOTIFY TEST FAIL (${failed.length}/${results.length})`);
if (failed.length > 0) process.exitCode = 1;
