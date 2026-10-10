/**
 * Notification records: the host half's classification of harness events into
 * the small, session-tagged records the browser half receives over SSE.
 *
 * Every record carries the session id (and the title when one is known), so the
 * page can jump back to the exact conversation a reminder came from. Composing
 * the record here rather than inline in the event bridge keeps the four-kind
 * rules in one place and makes them testable without a running harness.
 *
 * The record is deliberately language-neutral: it names a kind and carries the
 * raw title, and the browser half composes the visible sentence through its own
 * `T(...)` strings. That keeps a reminder in step with the panel language, which
 * follows dsh's `locale` setting (and can change while the page lives).
 *
 * @module dsh-desktop-kit/notify
 */

/** Kinds, matching `config.NOTIFY_KINDS` and the browser half's labels. */
export const TURN_DONE = "turn_done";
export const ANSWER_DONE = "answer_done";
export const APPROVAL = "approval";
export const QUESTION = "question";

/**
 * Record for a finished turn, or `undefined` when the turn must stay silent.
 * A subagent turn is noise; a turn that did not complete is not a reminder.
 * @param options - the turn end facts.
 * @param options.sessionId - the session the turn belongs to.
 * @param options.title - the session title, when the harness reported one.
 * @param options.worked - whether the turn called any tool.
 * @param options.reason - the turn's end reason kind.
 * @param options.origin - the session's origin (`subagent` silences it).
 * @returns the bus record, or `undefined` when nothing should be raised.
 */
export function turnRecord({ sessionId, title, worked, reason, origin }) {
	if (origin === "subagent") return undefined;
	if (reason !== "completed") return undefined;
	return { kind: worked ? TURN_DONE : ANSWER_DONE, sessionId, title };
}

/**
 * Record for an approval or question waterfall request. The request payload is
 * not echoed into the record; the reminder only says something is waiting.
 * @param options - the attention request facts.
 * @param options.kind - `approval` or `question`.
 * @param options.sessionId - the session waiting on the user.
 * @param options.title - the session title, when known.
 * @returns the bus record.
 */
export function attentionRecord({ kind, sessionId, title }) {
	return { kind, sessionId, title };
}
