/**
 * Notification bus: host-side event observation fanned out to the browser over
 * server-sent events.
 *
 * Doing the observation here rather than in the page is deliberate. The page
 * could subscribe to dsh's own mux (`/api/remote.mux`) and replay the desktop
 * shell's frame protocol, but that protocol is internal and would have to be
 * reimplemented in the browser (stream ids, `$events` waterfall replies, per
 * session `session/follow` streams). Subscribing on the host with plain
 * `ctx.on(...)` and forwarding a small typed record is both simpler and far less
 * likely to break on a dsh upgrade.
 *
 * SSE (rather than a websocket) also needs no upgrade route, no extra protocol,
 * and reconnects on its own via `EventSource`.
 *
 * @module dsh-desktop-kit/notify/bus
 */

/** Keep-alive comment interval; beats intermediate proxy idle timeouts. */
const HEARTBEAT_MS = 30_000;
/** Recent notifications retained for the panel's list view. */
const RECENT_LIMIT = 50;

/**
 * Create the bus.
 * @returns publish/subscribe handle used by the host wiring and the HTTP routes.
 */
export function createNotifyBus() {
	const subscribers = new Set();
	const recent = [];
	let counter = 0;

	/** Broadcast one record to every open SSE stream. */
	function publish(record) {
		counter += 1;
		const item = { id: counter, at: Date.now(), ...record };
		recent.push(item);
		if (recent.length > RECENT_LIMIT) recent.splice(0, recent.length - RECENT_LIMIT);
		const payload = `data: ${JSON.stringify(item)}\n\n`;
		for (const res of subscribers) {
			try {
				res.write(payload);
			} catch {
				subscribers.delete(res);
			}
		}
		return item;
	}

	return {
		publish,
		/** Record already delivered, newest last — used to prime the panel. */
		recent: () => [...recent],
		/** Number of open streams, for diagnostics. */
		size: () => subscribers.size,
		/**
		 * Open an SSE stream on a response object. The caller keeps response
		 * ownership; this returns the teardown to run when the request closes.
		 * @param res - the HTTP response to stream on.
		 * @returns a function that detaches the stream.
		 */
		attach(res) {
			res.writeHead(200, {
				"content-type": "text/event-stream; charset=utf-8",
				"cache-control": "no-cache, no-transform",
				connection: "keep-alive",
				"x-accel-buffering": "no"
			});
			res.write("retry: 3000\n\n");
			res.write(`event: ready\ndata: ${JSON.stringify({ recent: recent.length })}\n\n`);
			subscribers.add(res);
			const heartbeat = setInterval(() => {
				try {
					res.write(": keepalive\n\n");
				} catch {
					cleanup();
				}
			}, HEARTBEAT_MS);
			// Do not hold the process open for a keep-alive timer.
			heartbeat.unref?.();
			let done = false;
			function cleanup() {
				if (done) return;
				done = true;
				clearInterval(heartbeat);
				subscribers.delete(res);
			}
			res.on("close", cleanup);
			res.on("error", cleanup);
			return cleanup;
		}
	};
}
