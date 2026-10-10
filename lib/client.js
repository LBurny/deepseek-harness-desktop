/**
 * dsh-desktop-kit — browser half.
 *
 * Loaded into dsh's own web UI by the host half through the web server's index
 * injection (`<script src="/dshdesk/kit.js">`), so it runs same-origin with the
 * harness and can call the kit's `/dshdesk/api/*` routes directly. It owns the
 * four things that can only live in the page:
 *
 *   zoom        — CSS `zoom` on the document element (a page equivalent of the
 *                 desktop shell's WebView2 `set_zoom`), plus the keydown handler
 *   shortcuts   — a capture-phase matcher for the zoom keys, with a recorder
 *   reminders   — an `EventSource` on the kit's SSE stream driving the
 *                 Notification API, an in-page toast and an audio element
 *   panel       — skills / MCP / settings / diagnostics UI over the same routes
 *
 * Plain DOM, no build step and no framework: the injected script must not depend
 * on dsh's internal client bundle format or slot names, both of which are
 * undocumented outside the tree and change between releases.
 */
(() => {
	"use strict";

	// The loader may inject this script more than once (e.g. after a client HMR);
	// a second copy would double every listener and toast.
	if (window.__dshDesktopKitLoaded === true) return;
	window.__dshDesktopKitLoaded = true;

	const BASE = "/dshdesk";
	const API = `${BASE}/api`;

	/* ------------------------------------------------------- error reporting */

	// Page-error capture, mirroring the desktop shell's page bridge (errors →
	// its events.log). The host side rate-limits, truncates and redacts; this
	// side throttles to one report per 2 s and caps a page life at 20, so a
	// crashing render loop cannot spam the ring.
	(() => {
		let sent = 0;
		let lastSent = 0;
		const report = (fields) => {
			if (sent >= 20) return;
			const now = Date.now();
			if (now - lastSent < 2000) return;
			lastSent = now;
			sent += 1;
			void fetch(`${API}/client-log`, {
				method: "POST",
				credentials: "same-origin",
				headers: { "content-type": "application/json" },
				body: JSON.stringify(fields)
			}).catch(() => {
				/* the panel being unreachable is not itself reportable */
			});
		};
		window.addEventListener("error", (event) => {
			report({ message: String(event.message ?? "unknown error"), source: event.filename, line: event.lineno });
		});
		window.addEventListener("unhandledrejection", (event) => {
			const reason = event.reason;
			report({ message: `unhandled rejection: ${String(reason?.message ?? reason)}` });
		});
	})();

	/* ---------------------------------------------------------------- i18n */

	// Follows dsh's own `locale` row when /api/state reports one; until then the
	// navigator language is the guess. Mutable because boot() refines it.
	let ZH = navigator.language.toLowerCase().startsWith("zh");
	const STRINGS = {
		title: ["DSH Desktop Kit", "DSH 桌面套件"],
		skills: ["Skills", "技能"],
		mcp: ["MCP", "MCP"],
		notify: ["Reminders", "提醒"],
		view: ["View & keys", "缩放与快捷键"],
		diag: ["Diagnostics", "诊断"],
		about: ["About", "关于"],
		remote: ["Remote", "远程"],
		remoteHint: [
			"Reach this harness from a phone. The link below is the credential — anyone who has it can drive the agent, so treat it like a password. LAN mode keeps everything on your network; Tunnel mode publishes it through a Cloudflare quick tunnel so it works from anywhere.",
			"用手机访问这个 harness。下面的链接本身就是凭证——拿到它的人就能操作 agent，请当作密码对待。局域网模式不出本机网络；隧道模式会经 Cloudflare 快速隧道发布到公网，因此任何网络下都能用。"
		],
		remoteEnable: ["Enable", "开启"],
		remoteDisable: ["Turn off", "关闭"],
		remoteMode: ["Transport", "通道"],
		modeTunnel: ["Tunnel (works anywhere)", "隧道（可在任何网络使用）"],
		modeLan: ["LAN only (no external service)", "仅局域网（不经外部服务）"],
		remoteLink: ["Link", "链接"],
		remoteLanUrls: ["LAN addresses", "局域网地址"],
		copy: ["Copy", "复制"],
		copied: ["Copied", "已复制"],
		resetLink: ["Reset link", "重置链接"],
		resetLinkHint: ["Issues a new token; every previously shared link stops working immediately.", "会换发新 token，之前分享出去的链接立即失效。"],
		qr: ["QR code", "二维码"],
		qrUnavailable: ["Install the qrcode package to show a scannable code:", "安装 qrcode 包即可显示可扫二维码："],
		linkIsCredential: ["The link is the credential — do not paste it anywhere public.", "链接即凭证，请勿发到任何公开地方。"],
		cloudflaredMissing: ["cloudflared was not found, so Tunnel mode cannot start. Switch to LAN mode, or set remote.cloudflaredPath.", "未找到 cloudflared，隧道模式无法启动。请改用局域网模式，或设置 remote.cloudflaredPath。"],
		stateOff: ["Off", "未开启"],
		stateStarting: ["Starting…", "启动中…"],
		stateUp: ["Up", "已就绪"],
		stateError: ["Error", "出错"],
		enabled: ["Enabled", "已启用"],
		disabled: ["Disabled", "已停用"],
		remove: ["Delete", "删除"],
		edit: ["Edit", "编辑"],
		add: ["Add", "新增"],
		save: ["Save", "保存"],
		cancel: ["Cancel", "取消"],
		import: ["Import", "导入"],
		close: ["Close", "关闭"],
		refresh: ["Refresh", "刷新"],
		openPanel: ["Open the DSH Desktop Kit panel", "打开 DSH 桌面套件面板"],
		noSkills: ["No skills found. Put a SKILL.md in a directory under $DSH_HOME/skills.", "未找到技能。把 SKILL.md 放到 $DSH_HOME/skills 下的目录里。"],
		noServers: ["No MCP servers in this profile's patch layer.", "本 profile 的 patch 层里没有 MCP 服务器。"],
		importSkills: ["Import skills from another agent", "从其它 agent 导入技能"],
		importZip: ["Import ZIP", "导入 ZIP"],
		zipHint: [
			"A ZIP with SKILL.md at its root is one skill; a ZIP with SKILL.md inside top-level directories carries several.",
			"ZIP 包根有 SKILL.md 即为单个技能；SKILL.md 在各顶层目录下则为多个技能。"
		],
		importServers: ["Import MCP servers from another agent", "从其它 agent 导入 MCP 服务器"],
		conflictSkip: ["Skip existing", "已存在则跳过"],
		conflictOverwrite: ["Overwrite existing", "已存在则覆盖"],
		onConflict: ["On conflict", "冲突时"],
		notAvailable: ["not found", "未找到"],
		source: ["Source", "来源"],
		zoom: ["Page zoom", "页面缩放"],
		zoomStep: ["Zoom step", "缩放步进"],
		zoomIn: ["Zoom in", "放大"],
		zoomOut: ["Zoom out", "缩小"],
		zoomCeiling: ["Zoom is capped by the window height", "缩放已按窗口高度封顶"],
		record: ["Record", "录制"],
		pressKeys: ["Press keys…", "请按组合键…"],
		shortcutHint: ["Click Record, then press the combination. Esc cancels. At least one of Ctrl/Shift/Alt is required.", "点击「录制」后按下组合键，Esc 取消。至少需要 Ctrl/Shift/Alt 之一。"],
		reminders: ["Task reminders", "任务提醒"],
		remindersHint: ["Notifications fire when a turn finishes, or when dsh waits for your approval or an answer. \"Background only\" keeps them quiet while this tab is focused.", "任务完成、或 dsh 等待你批准/回答时会提醒。「仅后台」表示本标签页在前台时不打扰。"],
		approval: ["Approval waiting", "等待批准"],
		question: ["Question waiting", "等待回答"],
		turnDone: ["Task completed", "任务完成"],
		answerDone: ["Answer ready", "回答完成"],
		timingBackground: ["Background only", "仅后台"],
		timingAlways: ["Always", "总是"],
		sound: ["Sound", "提示音"],
		volume: ["Volume", "音量"],
		silent: ["Silent", "静音"],
		systemDefault: ["System default", "系统默认"],
		enableBrowser: ["Enable browser notifications", "启用浏览器通知"],
		browserNotify: ["System notifications (browser)", "系统通知（浏览器）"],
		inPageToast: ["In-page toasts", "页面内提示条"],
		browserGranted: ["Browser notifications are enabled.", "浏览器通知已启用。"],
		browserDenied: ["Browser notifications are blocked by the browser.", "浏览器通知已被浏览器阻止。"],
		recent: ["Recent", "最近"],
		noRecent: ["Nothing yet.", "暂无。"],
		test: ["Test", "测试"],
		serverName: ["Server name", "服务器名"],
		transport: ["Transport", "传输方式"],
		command: ["Command", "命令"],
		args: ["Arguments (one per line)", "参数（每行一个）"],
		env: ["Environment KEY=VALUE (one per line)", "环境变量 KEY=VALUE（每行一个）"],
		cwd: ["Working directory (optional)", "工作目录（可选）"],
		url: ["URL", "URL"],
		headers: ["Headers KEY=VALUE (one per line)", "请求头 KEY=VALUE（每行一个）"],
		savedReload: ["Saved. dsh hot-reloads the profile patch, so the change applies without a restart.", "已保存。dsh 会热重载 profile patch，无需重启即生效。"],
		state: ["State", "状态"],
		version: ["dsh version", "dsh 版本"],
		node: ["Node", "Node"],
		pid: ["PID", "PID"],
		uptime: ["Uptime", "运行时长"],
		rss: ["Memory", "内存"],
		port: ["Port", "端口"],
		profile: ["Profile", "Profile"],
		home: ["DSH home", "DSH home"],
		patch: ["Patch file", "Patch 文件"],
		sessions: ["Sessions", "会话"],
		logs: ["Logs", "日志"],
		pageErrors: ["Page errors", "页面错误"],
		aboutText: [
			"Features ported from DSHDesktop (github.com/LBurny/deepseek-harness-desktop) into a plain dsh plugin: skill management (including ZIP import), MCP management, diagnostics, page zoom, task reminders, shortcut rebinding, and phone remote access with a mobile-adapted UI and a read-only project browser. /init is provided by dsh-command-init and kept here only as a fallback.",
			"本插件把 DSHDesktop 的便利功能移植进 dsh 本体：技能管理（含 ZIP 导入）、MCP 管理、故障诊断、页面缩放、任务提醒、快捷键设置，以及带移动端适配界面与只读项目浏览的手机远程访问。/init 由 dsh-command-init 提供，这里仅作兜底。"
		],
		failed: ["Failed", "失败"]
	};
	const T = (key) => {
		const pair = STRINGS[key];
		return pair === undefined ? key : (ZH ? pair[1] : pair[0]);
	};

	/* ------------------------------------------------------------- helpers */

	/** Create an element with attributes and children. */
	function el(tag, attrs, ...children) {
		const node = document.createElement(tag);
		for (const [key, value] of Object.entries(attrs ?? {})) {
			if (value === undefined || value === null || value === false) continue;
			if (key === "class") node.className = value;
			else if (key === "text") node.textContent = value;
			else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
			else if (key === "dataset") Object.assign(node.dataset, value);
			else node.setAttribute(key, value === true ? "" : String(value));
		}
		for (const child of children.flat()) {
			if (child === undefined || child === null || child === false) continue;
			node.append(child instanceof Node ? child : document.createTextNode(String(child)));
		}
		return node;
	}

	/** GET JSON, surfacing the server's error message. */
	async function getJson(path) {
		const response = await fetch(path, { credentials: "same-origin" });
		return response.json();
	}

	/** POST JSON. */
	async function postJson(path, body) {
		const response = await fetch(path, {
			method: "POST",
			credentials: "same-origin",
			headers: { "content-type": "application/json" },
			body: JSON.stringify(body)
		});
		return response.json();
	}

	/** Parse a `KEY=VALUE` textarea into an object, ignoring blank lines. */
	function parsePairs(text) {
		const out = {};
		for (const line of String(text ?? "").split(/\r?\n/u)) {
			const trimmed = line.trim();
			if (trimmed === "") continue;
			const at = trimmed.indexOf("=");
			if (at <= 0) continue;
			out[trimmed.slice(0, at).trim()] = trimmed.slice(at + 1).trim();
		}
		return out;
	}

	/** Serialize an object back into `KEY=VALUE` lines. */
	const pairsToText = (value) =>
		Object.entries(value ?? {})
			.map(([key, item]) => `${key}=${item}`)
			.join("\n");

	/* ---------------------------------------------------------------- state */

	const state = {
		config: undefined,
		sounds: [],
		counts: { skills: {}, mcp: {} },
		tab: "skills",
		open: false,
		recordTarget: undefined,
		remote: undefined
	};

	/* ----------------------------------------------------------------- zoom */

	const zoom = {
		/**
		 * What limits zoom is the harness's own chrome, not the page content: `zoom`
		 * on `<html>` divides the layout viewport by the factor, so past
		 * `window height / this` the sidebar's stack no longer fits. The shell clips
		 * it (`overflow: hidden`) and the document never scrolls, so its bottom block
		 * — 设置, 意见反馈, 退出登录 — and the avatar menu hanging off it become
		 * unreachable.
		 *
		 * The requirement is measured from the page rather than assumed: the sidebar
		 * column is the shell's only non-shrinkable stack (each child is `flex: none`
		 * except the session region, which collapses to zero), so forcing a
		 * deliberately oversized zoom makes its `scrollHeight` report exactly what the
		 * column needs. That reading is in layout px — which the factor does not
		 * scale — so it holds at any zoom and for any sidebar contents: extra plugins
		 * add nav rows, a signed-in profile adds footer rows, and both raise the floor.
		 */
		SIDEBAR_COLUMN: '[class*="_sidebarCol"]',
		/** Used until the shell's sidebar exists, and in a host that has none. */
		FALLBACK_CHROME_PX: 360,
		/** Big enough to overflow any real window, so `scrollHeight` reports the need. */
		SAMPLE_ZOOM: 6,
		/** Keeps the cap strictly below the physical limit, so rounding cannot clip. */
		SLACK: 0.98,
		/**
		 * How tall the harness's sidebar needs to be, in layout px. The zoom used to
		 * measure is restored in the same task: no frame is painted oversized, and the
		 * shell's own width watcher — which reads a viewport-space rect on the next
		 * animation frame — never sees it either.
		 */
		required() {
			const column = document.querySelector(zoom.SIDEBAR_COLUMN);
			if (column === null) return zoom.FALLBACK_CHROME_PX;
			const html = document.documentElement;
			const applied = html.style.zoom;
			let required;
			try {
				html.style.zoom = String(zoom.SAMPLE_ZOOM);
				required = column.scrollHeight;
			} finally {
				html.style.zoom = applied ?? "";
			}
			return Number.isFinite(required) && required > 0 ? required : zoom.FALLBACK_CHROME_PX;
		},
		/**
		 * The largest level this window can render with the harness still usable. A
		 * physical bound, not a policy one, so it is derived per apply: `max` stays
		 * the user's preference.
		 */
		ceiling() {
			const { min, max } = state.config.zoom;
			const height = window.innerHeight;
			// A hidden or not-yet-laid-out window reports 0: that is not a small
			// window, it is no answer, so keep the user's own maximum.
			if (!Number.isFinite(height) || height <= 0) return max;
			// Never shrink the page on the user's behalf: 100% is the floor of what a
			// cap may impose, so a short window cannot be talked into a smaller level.
			const bounded = Math.max(1, (height * zoom.SLACK) / zoom.required());
			return Math.floor(Math.min(max, Math.max(min, bounded)) * 100) / 100;
		},
		/** Apply a level and remember it in the config (write is debounced). */
		apply(level, persist) {
			const { min } = state.config.zoom;
			const next = Math.min(zoom.ceiling(), Math.max(min, level));
			const html = document.documentElement;
			html.style.zoom = String(next);
			// The factor, published for the `data-menu-material` rules in the
			// stylesheet: dsh's portaled menus have to cancel it on themselves and
			// take it back on their contents, or the zoom lands on their inline
			// coordinates a second time. The names are shared with the standalone
			// dsh-zoom package on purpose — both may be installed, and two rules on
			// the same selector have to agree on one variable.
			html.style.setProperty("--dsh-zoom", String(next));
			html.style.setProperty("--dsh-zoom-inverse", String(1 / next));
			state.config.zoom.level = next;
			if (persist) zoom.persist();
			return next;
		},
		persist: debounce(() => {
			void postJson(`${API}/config`, { zoom: { level: state.config.zoom.level } });
		}, 400),
		step(direction) {
			const { level, step } = state.config.zoom;
			const next = zoom.apply(direction === "in" ? level + step : level - step, true);
			toast(T("zoom"), `${Math.round(next * 100)}%`);
		},
		reset() {
			zoom.apply(1, true);
		}
	};

	/** Match a keyboard event against a stored shortcut descriptor. */
	function matchesShortcut(event, shortcut) {
		if (event.metaKey) return false;
		if (event.ctrlKey !== shortcut.ctrl) return false;
		if (event.shiftKey !== shortcut.shift) return false;
		if (event.altKey !== shortcut.alt) return false;
		// `code` is authoritative; `key` covers synthesized/RDP events.
		return (event.code && event.code === shortcut.code) || (event.key && event.key === shortcut.key);
	}

	/**
	 * Capture-phase key handling for the zoom shortcuts. Registered once; the
	 * handler reads the live config, so rebinding needs no re-registration.
	 */
	function installShortcutMatcher() {
		window.addEventListener("keydown", (event) => {
			if (state.recordTarget !== undefined) return;
			const config = state.config;
			if (config === undefined) return;
			for (const [side, action] of [["in", "in"], ["out", "out"]]) {
				if (!matchesShortcut(event, config.zoom[side])) continue;
				event.preventDefault();
				event.stopImmediatePropagation();
				zoom.step(action);
				return;
			}
		}, true);
	}

	/* ------------------------------------------------------- reminders side */

	const notifier = {
		source: undefined,
		audio: undefined,
		recent: [],
		/** Open the SSE stream; the browser reconnects on its own. */
		start() {
			if (state.config === undefined) return;
			if (notifier.source !== undefined) notifier.source.close();
			const source = new EventSource(`${API}/events`);
			source.onmessage = (event) => {
				try {
					notifier.deliver(JSON.parse(event.data));
				} catch {
					/* a malformed frame is not worth surfacing */
				}
			};
			notifier.source = source;
		},
		/** Whether a kind should be raised right now, given its rule and focus. */
		allows(kind) {
			const rule = state.config.notify[kind];
			if (rule === undefined || rule.enabled !== true) return false;
			if (rule.timing === "always") return true;
			return document.hidden || !document.hasFocus();
		},
		deliver(item) {
			notifier.recent.push(item);
			if (notifier.recent.length > 50) notifier.recent.shift();
			if (state.open && state.tab === "notify") render();
			if (item.kind === undefined) return;
			if (!notifier.allows(item.kind)) return;
			notifier.play(item.kind);
			if (state.config.notify.toast) {
				toast(labelForKind(item.kind), item.text ?? "", () => focusPanel("notify"));
			}
			if (state.config.notify.browser && typeof Notification !== "undefined" && Notification.permission === "granted") {
				try {
					const notice = new Notification(item.text ?? labelForKind(item.kind), { tag: `dshkit-${item.kind}` });
					// The shell's toast click raises the main window; the browser
					// equivalent is focusing this tab.
					notice.onclick = () => window.focus();
				} catch {
					/* some engines refuse notifications outside a user gesture */
				}
			}
			pulseLauncher();
		},
		play(kind) {
			const choice = state.config.notify.sound;
			if (choice === "silent") return;
			// "default" means: leave the sound to the OS notification, play nothing here.
			if (choice === "default") return;
			const audio = notifier.audio ?? new Audio();
			notifier.audio = audio;
			audio.src = `${BASE}/sounds/${encodeURIComponent(choice)}.wav`;
			audio.volume = state.config.notify.volume;
			audio.play().catch(() => {
				/* autoplay policy: harmless, the toast still shows */
			});
		}
	};

	/** Human label for a notification kind. */
	function labelForKind(kind) {
		switch (kind) {
			case "approval": return T("approval");
			case "question": return T("question");
			case "turn_done": return T("turnDone");
			case "answer_done": return T("answerDone");
			default: return kind;
		}
	}

	/* ------------------------------------------------------------- toast/dom */

	let root;
	let launcher;
	let panel;
	let toasts;
	let body;

	/** In-page toast, used for both notifications and local feedback. */
	function toast(title, message, onClick) {
		if (toasts === undefined) return;
		const node = el("div", { class: "dshkit-toast", onclick: () => { node.remove(); onClick?.(); } },
			el("div", { class: "dshkit-toast-title", text: title }),
			message ? el("div", { class: "dshkit-row-sub", text: message }) : undefined);
		toasts.append(node);
		setTimeout(() => node.remove(), 8000);
	}

	/** Nudge the launcher when a reminder arrives while the panel is closed. */
	function pulseLauncher() {
		if (state.open || launcher === undefined) return;
		launcher.classList.add("attention");
		setTimeout(() => launcher.classList.remove("attention"), 4000);
	}

	/** Debounce helper for settings writes. */
	function debounce(fn, delay) {
		let handle;
		return (...args) => {
			clearTimeout(handle);
			handle = setTimeout(() => fn(...args), delay);
		};
	}

	/** A labelled switch bound to a getter/setter. */
	function switchControl(value, onToggle) {
		const node = el("div", { class: "dshkit-switch", dataset: { on: String(value === true) }, role: "switch" });
		node.addEventListener("click", () => {
			const next = node.dataset.on !== "true";
			node.dataset.on = String(next);
			onToggle(next);
		});
		return node;
	}

	/** Save a config patch and surface failures. */
	async function patchConfig(patch) {
		const result = await postJson(`${API}/config`, patch);
		if (result.ok !== true) {
			toast(T("failed"), result.error ?? "config save failed");
			return;
		}
		state.config = result.config;
		if (patch.zoom !== undefined && patch.zoom.level !== undefined) zoom.apply(result.config.zoom.level, false);
	}

	/* ---------------------------------------------------------------- panel */

	function focusPanel(tab) {
		state.open = true;
		if (tab !== undefined) state.tab = tab;
		panel.hidden = false;
		launcher.textContent = "▾";
		void render();
	}

	function togglePanel() {
		state.open = !state.open;
		panel.hidden = !state.open;
		launcher.textContent = state.open ? "▾" : "⚙";
		if (state.open) void render();
	}

	/** Build the shell once; `render()` refills the body per tab. */
	function buildChrome() {
		root = el("div", { id: "dshkit-root" });
		launcher = el("div", { id: "dshkit-launcher", title: T("openPanel"), text: "⚙", onclick: togglePanel });
		toasts = el("div", { class: "dshkit-toasts" });
		body = el("div", { class: "dshkit-body" });
		const tabs = el("div", { class: "dshkit-tabs" });
		for (const [id, label] of [["skills", T("skills")], ["mcp", T("mcp")], ["remote", T("remote")], ["notify", T("notify")], ["view", T("view")], ["diag", T("diag")], ["about", T("about")]]) {
			tabs.append(el("button", {
				text: label,
				class: id === state.tab ? "active" : "",
				onclick: () => {
					state.tab = id;
					void render();
				}
			}));
		}
		panel = el("div", { id: "dshkit-panel", hidden: true },
			el("div", { class: "dshkit-head" },
				el("div", { class: "dshkit-title", text: T("title") }),
				el("button", { text: T("close"), onclick: togglePanel })),
			tabs,
			body);
		root.append(launcher, toasts, panel);
		document.body.append(root);
		// Remember which tab is active by rebuilding only its button classes.
		panel.__tabs = tabs;
	}

	/** Re-render the active tab and refresh the tab strip's active state. */
	async function render() {
		for (const node of [...panel.__tabs.children]) {
			node.className = node.textContent === labelOfTab(state.tab) ? "active" : "";
		}
		body.replaceChildren(el("div", { class: "dshkit-muted", text: "…" }));
		try {
			if (state.tab === "skills") body.replaceChildren(await renderSkills());
			else if (state.tab === "mcp") body.replaceChildren(await renderMcp());
			else if (state.tab === "remote") body.replaceChildren(await renderRemote());
			else if (state.tab === "notify") body.replaceChildren(renderNotify());
			else if (state.tab === "view") body.replaceChildren(renderView());
			else if (state.tab === "diag") body.replaceChildren(await renderDiagnostics());
			else body.replaceChildren(renderAbout());
		} catch (error) {
			body.replaceChildren(el("div", { class: "dshkit-row-sub", text: `${T("failed")}: ${String(error?.message ?? error)}` }));
		}
	}

	const labelOfTab = (id) => ({ skills: T("skills"), mcp: T("mcp"), remote: T("remote"), notify: T("notify"), view: T("view"), diag: T("diag"), about: T("about") })[id];

	/** One list row: name, subtitle, switch and actions. */
	function row(name, subtitle, enabled, toggle, actions) {
		return el("div", { class: "dshkit-row" },
			el("div", { class: "dshkit-row-main" },
				el("div", { class: "dshkit-row-name", text: name }),
				subtitle ? el("div", { class: "dshkit-row-sub", text: subtitle }) : undefined),
			el("div", { class: "dshkit-row-actions" },
				...(actions ?? []),
				switchControl(enabled, async (next) => {
					try {
						await toggle(next);
					} catch (error) {
						toast(T("failed"), String(error?.message ?? error));
					}
					void render();
				})));
	}

	/* --------------------------------------------------------------- skills */

	async function renderSkills() {
		const data = await getJson(`${API}/skills`);
		if (data.ok !== true) throw new Error(data.error ?? "failed to list skills");
		const fragment = document.createDocumentFragment();
		fragment.append(el("div", { class: "dshkit-actions" },
			el("button", { text: T("importSkills"), onclick: () => showSkillImport(data.sources) }),
			el("button", { text: T("importZip"), onclick: () => showZipImport() }),
			el("button", { text: T("refresh"), onclick: () => void render() })));
		if (data.skills.length === 0) fragment.append(el("div", { class: "dshkit-note", text: T("noSkills") }));
		for (const skill of data.skills) {
			fragment.append(row(skill.name, skill.description || (skill.enabled ? T("enabled") : T("disabled")), skill.enabled,
				(enabled) => postJson(`${API}/skills`, { action: "toggle", name: skill.name, enabled }).then(assertOk),
				[el("button", {
					class: "danger",
					text: T("remove"),
					onclick: async () => {
						await postJson(`${API}/skills`, { action: "delete", name: skill.name }).then(assertOk);
						void render();
					}
				})]));
		}
		return fragment;
	}

	/** Import dialog: pick a source, tick skills, choose conflict policy. */
	function showSkillImport(sources) {
		const selected = new Set();
		const policy = el("select", {},
			el("option", { value: "skip", text: T("conflictSkip") }),
			el("option", { value: "overwrite", text: T("conflictOverwrite") }));
		const list = el("div", {});
		const sourceSelect = el("select", {});
		for (const source of sources) {
			sourceSelect.append(el("option", { value: source.id, text: `${source.label}${source.available ? "" : ` (${T("notAvailable")})`}`, disabled: !source.available }));
		}
		const paint = () => {
			const source = sources.find((entry) => entry.id === sourceSelect.value);
			selected.clear();
			list.replaceChildren();
			if (source === undefined || !source.available || source.skills.length === 0) {
				list.append(el("div", { class: "dshkit-note", text: T("noSkills") }));
				return;
			}
			for (const skill of source.skills) {
				const box = el("input", { type: "checkbox" });
				box.addEventListener("change", () => (box.checked ? selected.add(skill.name) : selected.delete(skill.name)));
				list.append(el("label", { class: "dshkit-row" }, box,
					el("div", { class: "dshkit-row-main" },
						el("div", { class: "dshkit-row-name", text: skill.name }),
						el("div", { class: "dshkit-row-sub", text: [skill.description, skill.conflict ? "⚠ already installed" : ""].filter(Boolean).join(" · ") }))));
			}
		};
		sourceSelect.addEventListener("change", paint);
		paint();
		const close = () => showView(renderSkills);
		showView(async () => el("div", {},
			el("div", { class: "dshkit-field" }, el("label", { text: T("source") }), sourceSelect),
			list,
			el("div", { class: "dshkit-field" }, el("label", { text: T("onConflict") }), policy),
			el("div", { class: "dshkit-actions" },
				el("button", {
					class: "primary",
					text: T("import"),
					onclick: async () => {
						const result = await postJson(`${API}/skills`, { action: "import", source: sourceSelect.value, names: [...selected], onConflict: policy.value });
						if (result.ok !== true) return toast(T("failed"), result.error);
						toast(T("import"), `${result.imported.length} → ${result.skipped.length} skipped`);
						close();
					}
				}),
					el("button", { text: T("cancel"), onclick: close }))));
	}

	/** ZIP import, step one: pick the file and upload it for inspection. */
	function showZipImport() {
		const input = el("input", { type: "file", accept: ".zip,application/zip", style: "display:none" });
		document.body.append(input);
		input.addEventListener("change", async () => {
			const file = input.files?.[0];
			input.remove();
			if (file === undefined) return;
			let result;
			try {
				const response = await fetch(`${API}/skills/zip?name=${encodeURIComponent(file.name)}`, {
					method: "POST",
					credentials: "same-origin",
					headers: { "content-type": "application/zip" },
					body: file
				});
				result = await response.json();
			} catch (error) {
				return toast(T("failed"), String(error?.message ?? error));
			}
			if (result?.ok !== true) return toast(T("failed"), result?.error ?? "not a valid ZIP file");
			showZipPicker(result.uploadId, result.skills ?? [], file.name);
		});
		input.click();
	}

	/** ZIP import, step two: tick the discovered skills and confirm. */
	function showZipPicker(uploadId, skills, archiveName) {
		// Non-conflicting skills start ticked: the common case is "import all".
		const selected = new Set(skills.filter((skill) => skill.conflict !== true).map((skill) => skill.name));
		const policy = el("select", {},
			el("option", { value: "skip", text: T("conflictSkip") }),
			el("option", { value: "overwrite", text: T("conflictOverwrite") }));
		const list = el("div", {});
		for (const skill of skills) {
			const box = el("input", { type: "checkbox" });
			box.checked = selected.has(skill.name);
			box.addEventListener("change", () => (box.checked ? selected.add(skill.name) : selected.delete(skill.name)));
			list.append(el("label", { class: "dshkit-row" }, box,
				el("div", { class: "dshkit-row-main" },
					el("div", { class: "dshkit-row-name", text: skill.name }),
					el("div", { class: "dshkit-row-sub", text: [skill.description, skill.conflict ? "⚠ already installed" : ""].filter(Boolean).join(" · ") }))));
		}
		const close = () => showView(renderSkills);
		showView(async () => el("div", {},
			el("div", { class: "dshkit-note", text: `${archiveName} — ${T("zipHint")}` }),
			list,
			el("div", { class: "dshkit-field" }, el("label", { text: T("onConflict") }), policy),
			el("div", { class: "dshkit-actions" },
				el("button", {
					class: "primary",
					text: T("import"),
					onclick: async () => {
						const result = await postJson(`${API}/skills`, { action: "import-zip", uploadId, names: [...selected], onConflict: policy.value });
						if (result.ok !== true) return toast(T("failed"), result.error);
						const failed = (result.errors ?? []).length;
						toast(T("import"), `${result.imported.length} → ${result.skipped.length} skipped${failed > 0 ? ` · ${failed} ${T("failed")}` : ""}`);
						close();
					}
				}),
				el("button", { text: T("cancel"), onclick: close }))));
	}

	/* ------------------------------------------------------------------ mcp */
	async function renderMcp() {
		const data = await getJson(`${API}/mcp`);
		if (data.ok !== true) throw new Error(data.error ?? "failed to list MCP servers");
		const fragment = document.createDocumentFragment();
		fragment.append(el("div", { class: "dshkit-actions" },
			el("button", { text: T("add"), onclick: () => showServerForm(undefined) }),
			el("button", { text: T("importServers"), onclick: () => showServerImport(data.sources) }),
			el("button", { text: T("refresh"), onclick: () => void render() })));
		fragment.append(el("div", { class: "dshkit-note", text: `${T("patch")}: ${data.patchPath} — ${T("savedReload")}` }));
		if (data.servers.length === 0) fragment.append(el("div", { class: "dshkit-note", text: T("noServers") }));
		for (const server of data.servers) {
			fragment.append(row(server.serverName, `${server.transport} · ${server.summary}`, server.enabled,
				(enabled) => postJson(`${API}/mcp`, { action: "toggle", id: server.id, enabled }).then(assertOk),
				[
					el("button", { text: T("edit"), onclick: () => showServerForm(server) }),
					el("button", {
						class: "danger",
						text: T("remove"),
						onclick: async () => {
							await postJson(`${API}/mcp`, { action: "delete", id: server.id }).then(assertOk);
							void render();
						}
					})
				]));
		}
		return fragment;
	}

	/** Add/edit form for one MCP server. */
	function showServerForm(server) {
		// Keep advanced keys the form does not own (timeouts, reconnect) by starting
		// from the existing config and only overwriting the fields below.
		const current = server?.config ?? {};
		const name = el("input", { value: current.serverName ?? "" });
		const transport = el("select", {},
			el("option", { value: "stdio", text: "stdio" }),
			el("option", { value: "streamable-http", text: "streamable-http" }));
		transport.value = current.transport === "streamable-http" ? "streamable-http" : "stdio";
		const command = el("input", { value: current.command ?? "" });
		const args = el("textarea", { rows: 3, text: (current.args ?? []).join("\n") });
		const env = el("textarea", { rows: 3, text: pairsToText(current.env) });
		const cwd = el("input", { value: current.cwd ?? "" });
		const url = el("input", { value: current.url ?? "" });
		const headers = el("textarea", { rows: 3, text: pairsToText(current.headers) });
		const stdioFields = el("div", {},
			el("div", { class: "dshkit-field" }, el("label", { text: T("command") }), command),
			el("div", { class: "dshkit-field" }, el("label", { text: T("args") }), args),
			el("div", { class: "dshkit-field" }, el("label", { text: T("env") }), env),
			el("div", { class: "dshkit-field" }, el("label", { text: T("cwd") }), cwd));
		const httpFields = el("div", {},
			el("div", { class: "dshkit-field" }, el("label", { text: T("url") }), url),
			el("div", { class: "dshkit-field" }, el("label", { text: T("headers") }), headers));
		const sync = () => {
			stdioFields.hidden = transport.value !== "stdio";
			httpFields.hidden = transport.value !== "streamable-http";
		};
		transport.addEventListener("change", sync);
		sync();
		const close = () => showView(renderMcp);
		showView(async () => el("div", {},
			el("div", { class: "dshkit-field" }, el("label", { text: T("serverName") }), name),
			el("div", { class: "dshkit-field" }, el("label", { text: T("transport") }), transport),
			stdioFields,
			httpFields,
			el("div", { class: "dshkit-actions" },
				el("button", {
					class: "primary",
					text: T("save"),
					onclick: async () => {
						const payload = {
							serverName: name.value.trim(),
							transport: transport.value,
							command: command.value,
							args: args.value.split(/\r?\n/u).map((line) => line.trim()).filter((line) => line !== ""),
							env: parsePairs(env.value),
							cwd: cwd.value,
							url: url.value,
							headers: parsePairs(headers.value)
						};
						const result = await postJson(`${API}/mcp`, { action: "upsert", server: payload, currentId: server?.id });
						if (result.ok !== true) return toast(T("failed"), result.error);
						toast(T("mcp"), `${result.id} — ${T("savedReload")}`);
						close();
					}
				}),
				el("button", { text: T("cancel"), onclick: close }))));
	}

	/** Import dialog for MCP servers. */
	function showServerImport(sources) {
		const selected = new Set();
		const policy = el("select", {}, el("option", { value: "skip", text: T("conflictSkip") }), el("option", { value: "overwrite", text: T("conflictOverwrite") }));
		const sourceSelect = el("select", {});
		for (const source of sources) sourceSelect.append(el("option", { value: source.id, text: `${source.label}${source.available ? "" : ` (${T("notAvailable")})`}` }));
		const list = el("div", {});
		const paint = () => {
			const source = sources.find((entry) => entry.id === sourceSelect.value);
			selected.clear();
			list.replaceChildren();
			if (source === undefined || !source.available || source.servers.length === 0) {
				list.append(el("div", { class: "dshkit-note", text: T("noServers") }));
				return;
			}
			for (const server of source.servers) {
				const box = el("input", { type: "checkbox", disabled: server.unsupported });
				box.addEventListener("change", () => (box.checked ? selected.add(server.serverName) : selected.delete(server.serverName)));
				list.append(el("label", { class: "dshkit-row" }, box,
					el("div", { class: "dshkit-row-main" },
						el("div", { class: "dshkit-row-name", text: server.serverName }),
						el("div", { class: "dshkit-row-sub", text: [server.transport, server.command ?? server.url, server.conflict ? "⚠ already installed" : "", server.unsupported ? "unsupported transport" : ""].filter(Boolean).join(" · ") }))));
			}
		};
		sourceSelect.addEventListener("change", paint);
		paint();
		const close = () => showView(renderMcp);
		showView(async () => el("div", {},
			el("div", { class: "dshkit-field" }, el("label", { text: T("source") }), sourceSelect),
			list,
			el("div", { class: "dshkit-field" }, el("label", { text: T("import") }), policy),
			el("div", { class: "dshkit-actions" },
				el("button", {
					class: "primary",
					text: T("import"),
					onclick: async () => {
						const result = await postJson(`${API}/mcp`, { action: "import", source: sourceSelect.value, names: [...selected], onConflict: policy.value });
						if (result.ok !== true) return toast(T("failed"), result.error);
						toast(T("import"), `${result.imported.length} → ${result.skipped.length} skipped`);
						close();
					}
				}),
				el("button", { text: T("cancel"), onclick: close }))));
	}

	/* ------------------------------------------------------------- reminders */

	function renderNotify() {
		const notify = state.config.notify;
		const fragment = document.createDocumentFragment();
		fragment.append(el("div", { class: "dshkit-note", text: T("remindersHint") }));
		const kinds = [["approval", T("approval")], ["question", T("question")], ["turn_done", T("turnDone")], ["answer_done", T("answerDone")]];
		for (const [kind, label] of kinds) {
			const timing = el("select", {},
				el("option", { value: "background", text: T("timingBackground") }),
				el("option", { value: "always", text: T("timingAlways") }));
			timing.value = notify[kind].timing;
			timing.addEventListener("change", () => void patchConfig({ notify: { [kind]: { timing: timing.value } } }));
			fragment.append(row(label, undefined, notify[kind].enabled,
				(enabled) => patchConfig({ notify: { [kind]: { enabled } } }),
				[timing]));
		}
		const sound = el("select", {});
		for (const id of state.sounds) {
			const text = id === "silent" ? T("silent") : id === "default" ? T("systemDefault") : id;
			sound.append(el("option", { value: id, text }));
		}
		sound.value = notify.sound;
		sound.addEventListener("change", () => void patchConfig({ notify: { sound: sound.value } }));
		const volume = el("input", { type: "range", min: "0", max: "1", step: "0.05", value: String(notify.volume) });
		volume.addEventListener("input", () => {
			notifier.audio ??= new Audio();
			notifier.audio.volume = Number(volume.value);
		});
		volume.addEventListener("change", () => void patchConfig({ notify: { volume: Number(volume.value) } }));
		fragment.append(el("div", { class: "dshkit-field" }, el("label", { text: T("sound") }), sound));
		fragment.append(el("div", { class: "dshkit-field" }, el("label", { text: T("volume") }), volume));
		fragment.append(row(T("browserNotify"), undefined, notify.browser === true, (enabled) => patchConfig({ notify: { browser: enabled } })));
		fragment.append(row(T("inPageToast"), undefined, notify.toast === true, (enabled) => patchConfig({ notify: { toast: enabled } })));
		fragment.append(el("div", { class: "dshkit-actions" },
			el("button", {
				text: T("test"),
				onclick: () => {
					notifier.play("turn_done");
					toast(T("reminders"), T("turnDone"));
				}
			}),
			el("button", {
				text: T("enableBrowser"),
				onclick: async () => {
					if (typeof Notification === "undefined") return toast(T("failed"), "Notification API unavailable");
					const permission = await Notification.requestPermission();
					toast(T("reminders"), permission === "granted" ? T("browserGranted") : T("browserDenied"));
				}
			})));
		fragment.append(el("div", { class: "dshkit-note", text: `${T("recent")}: ${notifier.recent.length}` }));
		for (const item of notifier.recent.slice(-10).reverse()) {
			fragment.append(el("div", { class: "dshkit-row" },
				el("div", { class: "dshkit-row-main" },
					el("div", { class: "dshkit-row-name", text: labelForKind(item.kind) }),
					el("div", { class: "dshkit-row-sub", text: item.text ?? "" }))));
		}
		return fragment;
	}

	/* --------------------------------------------------------------- remote */

	/** Copy to the clipboard, reporting either outcome. */
	function copy(text) {
		const clipboard = navigator.clipboard;
		if (clipboard === undefined) {
			toast(T("failed"), "clipboard unavailable");
			return;
		}
		clipboard.writeText(text).then(() => toast(T("remote"), T("copied")), () => toast(T("failed"), "clipboard blocked by the browser"));
	}

	/** Re-render the remote tab, polling while the tunnel is still coming up. */
	function refreshRemote() {
		if (state.tab !== "remote") return;
		void render();
		if (state.remote?.state === "starting") setTimeout(() => refreshRemote(), 2000);
	}

	async function renderRemote() {
		const data = await getJson(`${API}/remote`);
		if (data.ok !== true) throw new Error(data.error ?? "failed to read remote state");
		const remote = data.remote;
		state.remote = remote;
		const fragment = document.createDocumentFragment();
		fragment.append(el("div", { class: "dshkit-note", text: T("remoteHint") }));

		const stateLabel = { off: T("stateOff"), starting: T("stateStarting"), up: T("stateUp"), error: T("stateError") }[remote.state] ?? remote.state;
		fragment.append(el("div", { class: "dshkit-row" },
			el("div", { class: "dshkit-row-main" },
				el("div", { class: "dshkit-row-name", text: `${T("remote")}: ${stateLabel}` }),
				el("div", { class: "dshkit-row-sub", text: [remote.error, remote.tunnel === undefined ? "" : `tunnel: ${remote.tunnel}`, remote.proxyPort === undefined ? "" : `port ${remote.proxyPort}`].filter(Boolean).join(" · ") }))));

		const mode = el("select", {},
			el("option", { value: "tunnel", text: T("modeTunnel") }),
			el("option", { value: "lan", text: T("modeLan") }));
		mode.value = remote.mode;
		mode.addEventListener("change", async () => {
			const result = await postJson(`${API}/remote`, { action: "mode", mode: mode.value });
			if (result.ok !== true) return toast(T("failed"), result.error);
			refreshRemote();
		});
		fragment.append(el("div", { class: "dshkit-field" }, el("label", { text: T("remoteMode") }), mode));

		const actions = el("div", { class: "dshkit-actions" });
		if (remote.enabled) {
			actions.append(el("button", {
				text: T("remoteDisable"),
				onclick: async () => {
					const result = await postJson(`${API}/remote`, { action: "disable" });
					if (result.ok !== true) return toast(T("failed"), result.error);
					refreshRemote();
				}
			}));
			actions.append(el("button", {
				class: "danger",
				text: T("resetLink"),
				title: T("resetLinkHint"),
				onclick: async () => {
					const result = await postJson(`${API}/remote`, { action: "reset-token" });
					if (result.ok !== true) return toast(T("failed"), result.error);
					refreshRemote();
				}
			}));
		} else {
			actions.append(el("button", {
				class: "primary",
				text: T("remoteEnable"),
				onclick: async () => {
					const result = await postJson(`${API}/remote`, { action: "enable", mode: mode.value });
					if (result.ok !== true) return toast(T("failed"), result.error);
					refreshRemote();
				}
			}));
		}
		actions.append(el("button", { text: T("refresh"), onclick: () => void refreshRemote() }));
		fragment.append(actions);

		if (!remote.cloudflared && remote.mode === "tunnel") {
			fragment.append(el("div", { class: "dshkit-note", text: T("cloudflaredMissing") }));
		}

		const link = remote.link ?? remote.lanUrls?.[0];
		if (link !== undefined) {
			fragment.append(el("div", { class: "dshkit-field" }, el("label", { text: T("remoteLink") }),
				el("div", { class: "dshkit-row" },
					el("div", { class: "dshkit-row-main" }, el("div", { class: "dshkit-row-sub", text: link })),
					el("div", { class: "dshkit-row-actions" }, el("button", { text: T("copy"), onclick: () => copy(link) })))));
			fragment.append(el("div", { class: "dshkit-note", text: T("linkIsCredential") }));
			if (data.qrAvailable === true) {
				fragment.append(el("div", { class: "dshkit-field" }, el("label", { text: T("qr") }),
					el("img", { src: `${data.qrPath}?v=${String(Date.now())}`, alt: "remote link QR code", style: "width:220px;height:220px;background:#fff;border-radius:8px;padding:6px" })));
			} else {
				fragment.append(el("div", { class: "dshkit-note", text: `${T("qrUnavailable")} dsh plugin --profile web add qrcode` }));
			}
		}

		// The LAN addresses stay useful as a fallback while a tunnel is up.
		if (remote.enabled && remote.mode === "tunnel" && (remote.lanUrls?.length ?? 0) > 0) {
			fragment.append(el("div", { class: "dshkit-field" }, el("label", { text: T("remoteLanUrls") }),
				...remote.lanUrls.map((url) => el("div", { class: "dshkit-row" },
					el("div", { class: "dshkit-row-main" }, el("div", { class: "dshkit-row-sub", text: url })),
					el("div", { class: "dshkit-row-actions" }, el("button", { text: T("copy"), onclick: () => copy(url) }))))));
		}
		return fragment;
	}

	// A shorter window lowers the ceiling; re-apply through the clamp so resizing
	// after zooming in cannot strand the harness's own bottom block out of reach.
	window.addEventListener("resize", () => {
		if (state.config !== undefined) zoom.apply(state.config.zoom.level, false);
	});

	/**
	 * This script is injected with the page, but the shell mounts its sidebar
	 * afterwards, and the cap can only be measured once it is there. Until then
	 * `ceiling` answers with the fallback, so re-apply through the clamp as soon as
	 * the column appears — otherwise a saved level survives a reload uncapped.
	 */
	function watchForSidebar() {
		if (document.querySelector(zoom.SIDEBAR_COLUMN) !== null) return;
		if (typeof MutationObserver === "undefined") return;
		const observer = new MutationObserver(() => {
			if (document.querySelector(zoom.SIDEBAR_COLUMN) === null) return;
			observer.disconnect();
			if (state.config !== undefined) zoom.apply(state.config.zoom.level, false);
		});
		observer.observe(document.documentElement, { childList: true, subtree: true });
	}

	/* ---------------------------------------------------------- view & keys */

	function renderView() {
		const config = state.config;
		const fragment = document.createDocumentFragment();
		fragment.append(el("div", { class: "dshkit-note", text: T("shortcutHint") }));
		if (zoom.ceiling() < config.zoom.max) fragment.append(el("div", { class: "dshkit-note", text: T("zoomCeiling") }));
		fragment.append(el("div", { class: "dshkit-actions" },
			el("button", { text: T("zoomOut"), onclick: () => zoom.step("out") }),
			el("button", { text: `${Math.round(config.zoom.level * 100)}%`, onclick: () => zoom.reset() }),
			el("button", { text: T("zoomIn"), onclick: () => zoom.step("in") })));
		const step = el("input", { type: "range", min: "0.01", max: "0.25", step: "0.01", value: String(config.zoom.step) });
		step.addEventListener("input", () => {
			config.zoom.step = Number(step.value);
		});
		step.addEventListener("change", () => void patchConfig({ zoom: { step: Number(step.value) } }));
		fragment.append(el("div", { class: "dshkit-field" }, el("label", { text: `${T("zoomStep")}: ${Math.round(config.zoom.step * 100)}%` }), step));
		for (const [side, label] of [["in", T("zoomIn")], ["out", T("zoomOut")]]) {
			const cap = el("span", { class: "dshkit-keycap", text: describeShortcut(config.zoom[side]) });
			fragment.append(el("div", { class: "dshkit-row" },
				el("div", { class: "dshkit-row-main" },
					el("div", { class: "dshkit-row-name", text: label }),
					cap),
				el("div", { class: "dshkit-row-actions" },
					el("button", {
						text: T("record"),
						onclick: () => recordShortcut(side, cap)
					}))));
		}
		return fragment;
	}

	/** Render a shortcut descriptor as `Ctrl+Shift+=`. */
	function describeShortcut(shortcut) {
		const parts = [];
		if (shortcut.ctrl) parts.push("Ctrl");
		if (shortcut.shift) parts.push("Shift");
		if (shortcut.alt) parts.push("Alt");
		parts.push(prettyCode(shortcut.code, shortcut.key));
		return parts.join("+");
	}

	/** Turn a KeyboardEvent code into the label the shell used. */
	function prettyCode(code, key) {
		if (typeof code === "string" && code.startsWith("Key")) return code.slice(3);
		if (typeof code === "string" && code.startsWith("Digit")) return code.slice(5);
		const named = { Equal: "+", Minus: "-", NumpadAdd: "Numpad+", NumpadSubtract: "Numpad-" };
		if (code !== undefined && named[code] !== undefined) return named[code];
		return key || code || "?";
	}

	/** Capture the next chord and store it as a zoom shortcut. */
	function recordShortcut(side, cap) {
		state.recordTarget = side;
		cap.textContent = T("pressKeys");
		const finish = (shortcut) => {
			window.removeEventListener("keydown", handler, true);
			state.recordTarget = undefined;
			if (shortcut === undefined) {
				cap.textContent = describeShortcut(state.config.zoom[side]);
				void render();
				return;
			}
			// Require a modifier and keep the two directions distinct, like the shell did.
			const other = state.config.zoom[side === "in" ? "out" : "in"];
			const same = other.ctrl === shortcut.ctrl && other.shift === shortcut.shift && other.alt === shortcut.alt && other.code === shortcut.code && other.key === shortcut.key;
			if (!shortcut.ctrl && !shortcut.shift && !shortcut.alt) {
				toast(T("failed"), "at least one of Ctrl/Shift/Alt is required");
				cap.textContent = describeShortcut(state.config.zoom[side]);
				void render();
				return;
			}
			if (same) {
				toast(T("failed"), "zoom in and zoom out must differ");
				cap.textContent = describeShortcut(state.config.zoom[side]);
				void render();
				return;
			}
			void patchConfig({ zoom: { [side]: shortcut } }).then(() => void render());
		};
		const handler = (event) => {
			event.preventDefault();
			event.stopImmediatePropagation();
			if (event.key === "Escape") return finish(undefined);
			if (["Control", "Shift", "Alt", "Meta"].includes(event.key)) return;
			finish({ ctrl: event.ctrlKey, shift: event.shiftKey, alt: event.altKey, code: event.code, key: event.key });
		};
		window.addEventListener("keydown", handler, true);
	}

	/* ---------------------------------------------------------- diagnostics */

	async function renderDiagnostics() {
		const [status, logs] = await Promise.all([getJson(`${API}/diagnostics`), getJson(`${API}/logs`)]);
		if (status.ok !== true) throw new Error(status.error ?? "failed to read diagnostics");
		const info = status.status;
		const fragment = document.createDocumentFragment();
		fragment.append(el("div", { class: "dshkit-actions" }, el("button", { text: T("refresh"), onclick: () => void render() })));
		const grid = el("dl", { class: "dshkit-grid" });
		const entries = [
			[T("state"), info.state],
			[T("version"), info.version],
			[T("node"), info.node],
			[T("pid"), `${info.pid} (${info.platform}/${info.arch})`],
			[T("uptime"), `${info.uptimeSeconds}s`],
			[T("rss"), `${info.memoryMb} MB`],
			[T("port"), `${info.host}:${info.port}`],
			[T("profile"), info.profile],
			[T("home"), info.dshHome],
			[T("patch"), info.patchPath],
			[T("sessions"), info.sessions ?? "?"],
			[T("skills"), `${info.skills.enabled}/${info.skills.total} ${T("enabled")}`],
			[T("mcp"), `${info.mcp.enabled}/${info.mcp.total} ${T("enabled")}`],
			[T("notify"), `${T("recent")}: ${info.notificationStreams ?? 0} stream(s)`]
		];
		for (const [key, value] of entries) grid.append(el("dt", { text: key }), el("dd", { text: String(value ?? "-") }));
		fragment.append(grid);
		fragment.append(el("div", { class: "dshkit-field" }, el("label", { text: `${T("logs")} (${logs.source})` }), el("pre", { class: "dshkit-logs", text: (logs.lines ?? []).slice(-120).join("\n") })));
		const pageErrors = logs.clientErrors ?? [];
		if (pageErrors.length > 0) {
			fragment.append(el("div", { class: "dshkit-field" },
				el("label", { text: `${T("pageErrors")} (${pageErrors.length})` }),
				el("pre", { class: "dshkit-logs", text: pageErrors.slice(-30).join("\n") })));
		}
		return fragment;
	}

	/* ---------------------------------------------------------------- about */

	function renderAbout() {
		return el("div", {},
			el("div", { class: "dshkit-note", text: T("aboutText") }),
			el("div", { class: "dshkit-grid" },
				el("dt", { text: "API" }), el("dd", { text: API }),
				el("dt", { text: "assets" }), el("dd", { text: `${BASE}/kit.js, ${BASE}/kit.css` }),
				el("dt", { text: "commands" }), el("dd", { text: "/skills /mcp /doctor /kit" }),
				el("dt", { text: T("remote") }), el("dd", { text: `${API}/remote` })));
	}

	/** Swap the panel body for a sub-view (form/dialog); the callback supplies it. */
	function showView(factory) {
		void (async () => {
			try {
				const node = await factory();
				body.replaceChildren(node);
			} catch (error) {
				body.replaceChildren(el("div", { class: "dshkit-row-sub", text: `${T("failed")}: ${String(error?.message ?? error)}` }));
			}
		})();
	}

	/** Throw when an API result reports failure. */
	function assertOk(result) {
		if (result?.ok !== true) throw new Error(result?.error ?? "request failed");
		return result;
	}

	/* ------------------------------------------------------------- bootstrap */

	async function boot() {
		let snapshot;
		try {
			snapshot = await getJson(`${API}/state`);
		} catch {
			return;
		}
		if (snapshot.ok !== true) return;
		// Follow the harness's own UI preferences (the `locale` / `ui-theme`
		// patch rows) before painting anything, so the panel speaks and shades
		// like the UI it is injected into. Absent rows fall back to navigator
		// language / system color scheme.
		const preferences = snapshot.preferences ?? {};
		if (typeof preferences.locale === "string") ZH = preferences.locale.toLowerCase().startsWith("zh");
		state.config = snapshot.config;
		state.sounds = snapshot.sounds;
		state.counts = snapshot.counts;
		state.preferences = preferences;
		buildChrome();
		if (preferences.theme === "dark" || preferences.theme === "light") root.dataset.dshkitTheme = preferences.theme;
		if (state.config.ui.launcher !== false) {
			zoom.apply(state.config.zoom.level, false);
			installShortcutMatcher();
			watchForSidebar();
			notifier.start();
		}
		window.__dshDesktopKit = { state, notifier, zoom, patchConfig, render, togglePanel: focusPanel };
	}

	// The injected script lands at the end of <body>; defer only if that is not
	// true, so the panel appears with the rest of the UI.
	if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", () => void boot(), { once: true });
	else void boot();
})();
