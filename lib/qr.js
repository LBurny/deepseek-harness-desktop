/**
 * QR rendering for the remote link.
 *
 * Deliberately built on a real library rather than a hand-rolled encoder: a QR
 * symbol that is subtly wrong fails silently at the one moment it matters (the
 * user pointing a phone at it), and an encoder cannot be verified offline
 * without a scanner. `qrcode` is a mature, pure-JS package, so the kit uses it
 * when it is installed and degrades to "show the link instead" when it is not.
 *
 * Two resolution strategies, because a dsh plugin's dependencies can live in two
 * places and neither covers the other:
 *  - alongside the plugin (`profiles/plugins/<pkg>/node_modules`) — what a plain
 *    `npm install` in the plugin directory produces;
 *  - as a profile dependency (`profiles/<name>/node_modules`) — what
 *    `dsh plugin --profile web add qrcode` produces, which Node's own lookup
 *    from the plugin directory can never reach.
 *
 * @module dsh-desktop-kit/remote/qr
 */
import { createRequire } from "node:module";
import { join } from "node:path";

/** Resolution results do not change while dsh runs, so cache the outcome. */
let cached;

/** Try one resolution anchor. */
function tryRequire(anchor) {
	try {
		return createRequire(anchor)("qrcode");
	} catch {
		return undefined;
	}
}

/**
 * Resolve the QR library.
 * @param profileDir - the profile directory (`configEditor.documentPath`'s parent).
 * @returns the module, or undefined when it is not installed.
 */
function loadLibrary(profileDir) {
	if (cached !== undefined) return cached;
	cached = tryRequire(import.meta.url) ?? tryRequire(join(profileDir, "package.json"));
	return cached;
}

/**
 * Create a renderer bound to the active profile.
 * @param layout - resolved path layout.
 * @returns `available` plus an SVG renderer.
 */
export function createQrRenderer(layout) {
	return {
		/** Whether QR rendering is usable (the `qrcode` package is installed). */
		available: () => loadLibrary(layout().profileDir) !== undefined,
		/**
		 * Render text as an SVG QR symbol.
		 * @param text - the payload, normally the remote link.
		 * @returns the SVG markup, or undefined when the library is missing.
		 */
		async svg(text) {
			const library = loadLibrary(layout().profileDir);
			if (library === undefined) return undefined;
			return library.toString(text, {
				type: "svg",
				margin: 1,
				width: 320,
				errorCorrectionLevel: "M",
				color: { dark: "#000000ff", light: "#ffffffff" }
			});
		}
	};
}
