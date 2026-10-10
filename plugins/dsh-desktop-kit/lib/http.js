/**
 * Tiny helpers for the kit's own HTTP surface. The dsh web server hands route
 * handlers full response ownership, so these keep the ownership rules in one
 * place: every helper ends the response exactly once.
 *
 * @module dsh-desktop-kit/lib/http
 */

/** Body cap for kit API requests; the payloads are small settings documents. */
const MAX_BODY_BYTES = 1 << 20;

/** Send a JSON response. */
export function sendJson(res, status, payload) {
	const body = JSON.stringify(payload);
	res.writeHead(status, {
		"content-type": "application/json; charset=utf-8",
		"content-length": Buffer.byteLength(body),
		"cache-control": "no-store"
	});
	res.end(body);
}

/** Send a UTF-8 text response. */
export function sendText(res, status, text, type = "text/plain; charset=utf-8") {
	res.writeHead(status, {
		"content-type": type,
		"content-length": Buffer.byteLength(text),
		"cache-control": "no-store"
	});
	res.end(text);
}

/** Send a binary response with a caller-supplied content type. */
export function sendBuffer(res, status, buffer, type) {
	res.writeHead(status, {
		"content-type": type,
		"content-length": buffer.length,
		"cache-control": "public, max-age=300"
	});
	res.end(buffer);
}

/**
 * Read and parse a JSON request body.
 * @param req - the incoming request.
 * @returns the parsed value, or `undefined` when the body is absent/oversized/invalid.
 */
export async function readJson(req) {
	const chunks = [];
	let size = 0;
	for await (const chunk of req) {
		size += chunk.length;
		if (size > MAX_BODY_BYTES) return undefined;
		chunks.push(chunk);
	}
	if (size === 0) return undefined;
	try {
		return JSON.parse(Buffer.concat(chunks).toString("utf8"));
	} catch {
		return undefined;
	}
}

/**
 * Read a raw binary request body (ZIP uploads).
 * @param req - the incoming request.
 * @param limit - byte cap; an oversized body aborts the read.
 * @returns the body, or `undefined` when absent/oversized.
 */
export async function readRaw(req, limit) {
	const chunks = [];
	let size = 0;
	try {
		for await (const chunk of req) {
			size += chunk.length;
			if (size > limit) return undefined;
			chunks.push(chunk);
		}
	} catch {
		return undefined;
	}
	if (size === 0) return undefined;
	return Buffer.concat(chunks);
}

/**
 * Wrap a handler so a thrown error becomes a JSON 500 instead of an unhandled
 * rejection inside the web server's request path.
 * @param handler - the real handler.
 * @returns a handler that reports failures as JSON.
 */
export function guard(handler) {
	return async (req, res) => {
		try {
			await handler(req, res);
		} catch (error) {
			const message = String(error?.message ?? error);
			if (!res.headersSent) sendJson(res, 500, { ok: false, error: message });
			else res.end();
		}
	};
}
