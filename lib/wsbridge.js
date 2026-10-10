/**
 * WebSocket frame bridging for the remote-access proxy.
 *
 * The proxy terminates the phone's upgrade and opens a second connection to
 * dsh, then has to move frames both ways. It cannot simply pipe the sockets:
 * Cloudflare's quick tunnel kills connections that stay idle for roughly a
 * hundred seconds, and the countermeasure — a Ping from the proxy when the
 * downstream has been quiet — must land *between* frames. Writing bytes into
 * a raw piped stream could split a frame that spans two TCP chunks and
 * corrupt the whole connection, so the downstream goes through a frame
 * splitter that only ever forwards complete frames, and the Ping is written
 * only when no partial frame is buffered.
 *
 * The splitter does not interpret payloads: frames are forwarded as their
 * original raw bytes (masking preserved, fragmentation preserved), so there
 * is no re-framing and no codec drift. Control frames interleaved inside a
 * fragmented message are just frames to it — valid per RFC 6455.
 *
 * Mirrors the desktop shell's `bridge` in `src-tauri/src/remote/proxy.rs`:
 * strictly silence-triggered (an active channel gets zero keepalive frames),
 * and the browser answers the Ping at the protocol level, which keeps the
 * tunnel warm in both directions.
 *
 * @module dsh-desktop-kit/remote/wsbridge
 */

/** Downstream silence before the proxy emits a Ping of its own. */
export const WS_IDLE_PING_MS = 30_000;
/** A complete, zero-payload Ping frame (FIN set, opcode 9, unmasked). */
const PING_FRAME = Buffer.from([0x89, 0x00]);
/** Frames larger than this are almost certainly not dsh traffic; drop the bridge. */
const MAX_FRAME_BYTES = 256 * 1024 * 1024;

/**
 * Incremental splitter that yields complete WebSocket frames as raw slices,
 * header included. Feed upstream bytes in, forward what comes out.
 */
export class FrameSplitter {
	#pending = Buffer.alloc(0);

	/** Bytes of an incomplete frame currently held back. */
	get pendingBytes() {
		return this.#pending.length;
	}

	/**
	 * Consume one chunk and return every frame it completes.
	 * @param chunk - bytes read from the socket.
	 * @returns complete frames, in wire order.
	 */
	push(chunk) {
		this.#pending = this.#pending.length === 0 ? chunk : Buffer.concat([this.#pending, chunk]);
		const frames = [];
		let at = 0;
		const buffer = this.#pending;
		while (at + 2 <= buffer.length) {
			const second = buffer[at + 1];
			const masked = (second & 0x80) !== 0;
			let length = second & 0x7f;
			let header = 2;
			if (length === 126) {
				if (at + 4 > buffer.length) break;
				length = buffer.readUInt16BE(at + 2);
				header = 4;
			} else if (length === 127) {
				if (at + 10 > buffer.length) break;
				const big = buffer.readBigUInt64BE(at + 2);
				if (big > BigInt(MAX_FRAME_BYTES)) throw new Error("ws frame exceeds the 256 MiB bridge limit");
				length = Number(big);
				header = 10;
			}
			if (masked) header += 4;
			const total = header + length;
			if (total > MAX_FRAME_BYTES + 14) throw new Error("ws frame exceeds the 256 MiB bridge limit");
			if (at + total > buffer.length) break;
			frames.push(buffer.subarray(at, at + total));
			at += total;
		}
		this.#pending = buffer.subarray(at);
		return frames;
	}
}

/**
 * Bridge two sockets at frame level.
 *
 * The upstream side begins in handshake mode: dsh's `101 Switching Protocols`
 * response (or an error response, forwarded verbatim) is HTTP, not frames, so
 * bytes pass through until the header terminator arrives. After that the
 * downstream is frame-split; the client-to-upstream direction stays a raw
 * forward — the proxy never injects into it, so it needs no framing.
 *
 * @param client - socket to the remote browser (already upgraded).
 * @param upstream - socket to dsh (request already written).
 * @param options - `idlePingMs` (tests shorten it), `log`.
 * @returns a `destroy()` that tears both sides down.
 */
export function bridgeSockets(client, upstream, { idlePingMs = WS_IDLE_PING_MS, log = () => {} } = {}) {
	const splitter = new FrameSplitter();
	let handshaken = false;
	let handshake = Buffer.alloc(0);
	let lastDownstream = Date.now();
	let done = false;

	const destroy = () => {
		if (done) return;
		done = true;
		clearInterval(timer);
		client.destroy();
		upstream.destroy();
	};

	// Silence-triggered Ping: fires only when nothing came downstream for the
	// whole interval AND no partial frame is being held (injecting between two
	// chunks of one frame would corrupt it).
	const timer = setInterval(() => {
		if (done || !handshaken) return;
		if (Date.now() - lastDownstream < idlePingMs) return;
		if (splitter.pendingBytes !== 0) return;
		if (client.writable) {
			lastDownstream = Date.now();
			client.write(PING_FRAME, () => {});
		}
	}, idlePingMs);
	timer.unref?.();

	upstream.on("data", (chunk) => {
		lastDownstream = Date.now();
		try {
			if (!handshaken) {
				// Hold handshake bytes until the header terminator arrives: the
				// terminator may span two chunks, so partial headers are never
				// forwarded on their own.
				handshake = handshake.length === 0 ? chunk : Buffer.concat([handshake, chunk]);
				const end = handshake.indexOf("\r\n\r\n");
				if (end === -1) {
					if (handshake.length > 64 * 1024) destroy();
					return;
				}
				handshaken = true;
				const rest = handshake.subarray(end + 4);
				client.write(handshake.subarray(0, end + 4), () => {});
				handshake = Buffer.alloc(0);
				if (rest.length === 0) return;
				for (const frame of splitter.push(rest)) client.write(frame, () => {});
				return;
			}
			for (const frame of splitter.push(chunk)) client.write(frame, () => {});
		} catch (error) {
			log(`ws bridge downstream error: ${String(error?.message ?? error)}`);
			destroy();
		}
	});

	// Client → upstream is a pure forward: the proxy injects nothing into it.
	client.on("data", (chunk) => {
		if (upstream.writable) upstream.write(chunk, () => {});
	});

	client.on("error", destroy);
	upstream.on("error", destroy);
	client.on("close", destroy);
	upstream.on("close", destroy);

	return { destroy };
}
