# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `scripts/follow-upstream.ps1`: one-command dsh version-follow tooling — pin bump, stale-runtime cleanup (the old `dsh/` dir is removed first so floating sub-packages are not held at the previous rc by its package-lock), runtime re-fetch, three-carrier app version bump (package.json is now synced too; it had drifted to 0.1.12), the contract suite as gate, doc baseline sync with per-file occurrence guards (upstream.rs header / design §15 / both READMEs) and a CHANGELOG skeleton entry. Every step is idempotent: when the contract suite goes red, fix upstream.rs and re-run the same command to resume. `-SelfTest` runs 12 zero-network pure-function assertions

## [0.3.0] - 2026-08-21

### Added

- Remote access: new "项目" (Project) tab on the session page, between 轨迹 (Trace) and 信息 (Info) — browse the current session's workspace file tree from the phone and preview images (png/jpeg/webp/gif), rendered Markdown (headings/tables/task lists, relative-path images resolved through the file endpoint) and line-numbered code/text with a wrap toggle; unsupported types and oversized previews offer a download button instead. Implemented as shell-owned routes under `/__dsh-desktop/` (self-contained page + resolve/list/file APIs) served by the token-gated reverse proxy, with the page embedded via an injected iframe tab (mobile.js) — zero modification of dsh files. Session→workspace resolution reads `$DSH_HOME/storages/workspace.json` read-only (schema pinned by contract probes); requested paths are restricted to normal components, canonicalized and prefix-checked (escape/junction → 403); text previews cap at 8MB, all files at 64MB, download exempt from the preview cap. The desktop app is unaffected: the tab only exists on responses passing through the remote proxy. As part of this the token gate moved from fallback-handler-internal logic to a Router-wide axum middleware — shell-owned routes mounted ahead of the fallback would otherwise have bypassed authentication (regression test `gate_covers_shell_routes`)

### Changed

- Mobile session header: the upstream "Session log" pill is shrunk (32px height / 111px min-width → 26px / content-width) inside the 700px breakpoint so it no longer crowds the tab bar. The doubled `[class*="_sessionLogButton"]` selector (0-2-0 specificity) is required because upstream's stylesheet is JS-runtime-injected after ours; a new contract probe (`SESSION_LOG_BUTTON_NEEDLE`) goes red if upstream renames the CSS Modules local name, instead of the rule silently dying
- dsh runtime 0.1.0-rc.8 → 0.1.1-rc.2 (fetch-runtime.ps1 pin): upstream added image upload via Files API, the DeepSeek-V4-Flash-Vision-Exp model, responsive Markdown tables and a Bubblewrap /proc escape fix — no drift in any probed fact (entry/command shape, WS frames, settings keys, workspace schema, localStorage key, picker needles, plugin CLI)

Tests: 188 → 198 (197 passed + 1 ignored)

## [0.2.2] - 2026-08-21

### Changed

- The dsh subprocess now spawns with `<DSH_HOME>/profiles/web/node_modules/.bin` prepended to PATH (new `dsh_child_path` in process.rs — the same pattern plugins.rs already used for the bundled pnpm). Plugin-shipped CLIs installed into the profile by the plugin manager are not on the user's PATH, so session terminals and tool subprocesses could not resolve them by name — diagnosed from a real modlens 3.22.0 install where the tool itself loaded fine but `modlens ...` in the dsh terminal failed with "The term 'modlens' is not recognized", forcing users to dig out the absolute path under `%LOCALAPPDATA%` to run the plugin's own setup commands. Base PATH entries are preserved in order; with no parent PATH the result is just the `.bin` entry. The upstream fact (pnpm's standard `node_modules/.bin` layout inside a profile) is pinned in upstream.rs as `PROFILE_BIN_DIR_SEGMENTS`; layout drift degrades silently to the old behavior, never a crash. Scope note: this only fixes command resolution — plugins writing config outside the session workspace (e.g. `~/.modlens/config.json`) remain subject to dsh's own sandbox policy, by upstream design

Tests: 186 → 188 (187 passed + 1 ignored)

## [0.2.1] - 2026-08-21

### Fixed

- Plugin manager no longer reports a successful install/uninstall/update as "安装失败" (failed). The IPC structs in plugins.rs (`PluginOpResult`/`PluginStatus`/`PluginRow`) were missing `#[serde(rename_all = "camelCase")]` — the convention every mcp.rs struct already follows — so Tauri serialized `exit_code`/`pnpm_ready`/`is_bundle` in snake_case while Plugins.svelte reads camelCase. `r.exitCode` was therefore always `undefined`, `undefined === 0` never holds, and every operation took the failure branch with a blank exit code (Svelte renders `undefined` as nothing). The same mismatch made the status line permanently show "pnpm 缺失" even though the bundled pnpm worked, and forced the bundle badge to always read "依赖". Anchor test `ipc_payloads_serialize_camel_case` pins the wire shape
- The collapsible "操作输出" (operation output) panel now actually contains output: `run_plugin_op` never piped the child's stdout/stderr — tokio's spawn inherits the parent stdio by default and `wait_with_output` only reads piped handles — so the panel was permanently empty and genuine failures carried zero diagnostics. The child now gets explicit `stdout/stderr(Stdio::piped())`; anchor test `run_captures_child_output` pins it. Verified end-to-end against the real runtime and the real dsh-home: installing `@liustack/modlens` exits 0 with `{"exitCode":0,"output":"…pnpm log…"}`, and installing a nonexistent package exits 1 with pnpm's full 404 error captured
- The Chinese failure notice now includes the exit code number: the i18n key `失败，退出码` itself had no `{code}` placeholder (only the English translation did), so zh never rendered the code even with the serialization fixed. The key is now `失败，退出码 {code}`

Tests: 184 → 186 (185 passed + 1 ignored)

## [0.2.0] - 2026-08-21

### Changed

- Track dsh 0.1.0-rc.8 (subpackages resolve to rc.8 across the board; npm's `latest` tag lags, so fetch-runtime.ps1 pins `-DshVersion` explicitly). The WebSocket event channels (`/api/events.mux` + `/api/events.host`), the `--no-open` flag, the browse-picker pin rows, all pickerpatch/welcome needles, and the plugin command surface are unchanged — verified item by item by the upstream contract suite against the real runtime
- The minimal preset (极简模式) patcher is retired: upstream rc.8 fixes the win32 gap itself — the shipped preset now gates `persistent-bash`/`persistent-pwsh` by `process.platform`, and subprocess-local gained a koffi-based Windows terminal inspector. presets.rs keeps only the read-only signature probe; the contract suite asserts `UpstreamHandled` as a regression sentinel, so an upstream revert turns the suite red. Runtime files patched by ≤0.1.21 are still classified correctly via the old marker

Tests: 189 → 184 (183 passed + 1 ignored)

## [0.1.13] - 2026-08-18

### Fixed

- Installing over an existing copy (upgrade or reinstall) no longer aborts with "Unable to uninstall!". The cleanup sweep added in 0.1.9 kills every process whose executable lives under the install directory — and the Tauri template runs the old uninstaller **in place** via `_?=$INSTDIR`, so `uninstall.exe` matched the sweep pattern and the uninstaller killed itself mid-hook, before deleting a single file; the new installer then saw a non-zero exit code and aborted. The sweep now excludes the caller's own process (its PowerShell parent PID, so it is name-agnostic). Standalone uninstalls were never affected because the uninstaller self-copies to %TEMP% unless `_?=` is passed, which is why the bug only surfaced on in-place upgrades. Known issue: upgrading **from ≤0.1.12** still hits the dialog one final time (the old uninstaller cannot be patched by the new installer) — uninstall from Windows Settings first, then run the new setup
- Leftover runtime files after uninstall: the uninstaller only deletes files recorded in its install manifest, so files added later by dsh self-updates (e.g. new node_modules packages) survived and kept `$INSTDIR` non-empty. A POSTUNINSTALL hook now force-removes the runtime tree after the manifest deletions
- The pre-install/pre-uninstall cleanup now polls until the killed processes are actually gone (up to 10s) instead of a fixed 1.5s sleep, so slow-to-die node.exe/cloudflared.exe can't still hold runtime files when deletion starts

## [0.1.12] - 2026-08-17

### Fixed

- Saving in Other Settings no longer fails with "系统找不到指定的文件。 (os error 2)" for users who never enabled launch-at-login. Every save calls set_autostart, and auto-launch 0.5's disable() unconditionally deletes the registry Run value — when the value doesn't exist, RegDeleteValueW returns ERROR_FILE_NOT_FOUND and the whole save reported failure. The command now compares the current state first and treats an already-reached target state as success (which also avoids rewriting the registry on every save). A regression test pins the upstream behavior so a future idempotent auto-launch release flags the workaround as removable
- Settings write failures are now reported instead of silently swallowed: previously a blocked settings.json write (e.g. by antivirus folder protection) looked like a successful save but reverted on restart. set_shell_settings now surfaces "设置写入失败: …" and keeps the in-memory value consistent with disk

## [0.1.11] - 2026-08-17

### Added

- Mobile UI adaptation for remote access: the token-gate proxy now injects `mobile.css` + `mobile.js` into every HTML document it forwards (breakpoint 700px, so the desktop shell window at min-width 900px never matches). Three verified breakages are fixed: the settings dialog goes full-screen with its fixed 188px nav column turned into a horizontal tab strip (content was squeezed to one character per line), the expanded sidebar becomes an overlay drawer instead of a fixed 280px grid track that crushed the main area to 110px, and the composer model selector is iconified (sparkle mask icon following the theme text color; the model itself is picked from the opened second-level menu) with the trigger menu's containing block moved up to the composer card so the popup is no longer clipped off-screen
- New "信息" (Info) tab next to "对话/轨迹" in the conversation header: tapping it opens a full panel listing per-turn stats (turns/steps, LLM time, first-token latency and speed, cache hit rate, input/output tokens) one per row, live-synced via MutationObserver — the stats node is cloned rather than moved because React crashes on removeChild of moved nodes. The enhancement marker follows the matchMedia breakpoint so rotating to landscape restores the native stats row, and if the script can't find its anchors (upstream renames) a CSS fallback keeps the stats readable as a centered two-row wrap. All selectors anchor on semantic hooks (`role`, `data-sidebar-collapsed`) and CSS Modules local-name substrings, so upstream hash changes degrade silently to the un-adapted page

### Changed

- Other Settings: removed the explanatory line under the notification rules ("后台 = 本应用窗口均未聚焦…") for visual consistency

## [0.1.10] - 2026-08-17

### Fixed

- "极简模式" (minimal preset) sessions on Windows can actually run shell commands now. Upstream dsh rc.6 mounts a PTY-backed persistent bash for that preset, but `dsh-subprocess-local`'s terminal inspector only implements linux/darwin, so every call failed with "terminal inspection is unsupported on platform win32". At startup the shell rewrites the shipped preset into a PowerShell variant (`tool-pwsh` over the host-plane `pwsh-sandbox` executor, which uses plain pipe spawns) and makes the persona state the working directory explicitly — previously the fixed persona hid all runtime context, so with bash dead the model resorted to guessing `/` / `C:\` and hit Windows ACL denials. The patch is signature-gated (stops applying once upstream adds a `win32` branch) and idempotent, re-applying after dsh self-updates
- Session log export ("Session log" button) no longer vanishes: WebView2 cancels downloads unless the host handles `DownloadStarting`, and wry's default handler allows them silently with the download UI suppressed, so files either never appeared or landed without any trace. The main window is now created in code (tauri.conf `windows` is empty) so an `on_download` handler can be attached: exports land in the system Downloads folder with ` (n)` dedup, and a toast reports the saved path or the failure. Window geometry memory and first-frame behavior are unchanged (verify-window-state / verify-no-size-flash regressions pass)

- Remote sessions no longer show the internal-testing notice （内测声明） on every visit. dsh's web UI picks `memory` persistence for the acknowledgement when the page origin is not loopback, so through the Cloudflare tunnel domain the confirmation never reached `settings.yaml`. The remote-access proxy now buffers plugin bundles (`/plugins/*/client.js`, ≤4MB, identity encoding only) and rewrites the `connection.isLoopback ? "host" : "memory"` ternary to `"host"`, making remote clients share the host-persisted acknowledgement with the desktop. The rewrite fails soft — if dsh changes the wording upstream, the bundle passes through untouched and only the notice reappears

## [0.1.9] - 2026-08-17

### Added

- Reset remote link: one click on the `#/remote` page or in the tray submenu rotates the access token in place and drops every established session — the old link, old cookies, and live WebSocket bridges die instantly while the tunnel and domain stay up. This is the revocation path when a link leaks; previously the only option was the non-obvious stop-then-start (which also rebuilds the tunnel and changes the domain). The proxy gate now reads the token from a shared cell per request, and WS bridges select on a drain notify so reset/shutdown cuts them immediately

### Changed

- Settings copy tightened: "保持后台运行（最小化到托盘）" → "最小化到托盘"; notification rules "任务确认（待批准）/选项选择（待回答）/回答完毕（任务完成）" → "任务确认/选项选择/任务完成"

### Fixed

- Reinstalling or uninstalling while the app is running no longer aborts with "Can't write: ...\cloudflared.exe". The stock NSIS flow kills only the main binary, which orphaned the bundled node.exe/cloudflared.exe holding the runtime directory open. Two layers now prevent this: all supervised children are registered in a `KILL_ON_JOB_CLOSE` job object so the kernel reaps them whenever the shell exits for any reason (`Platform::register_child`), and new NSIS pre-install/pre-uninstall hooks (`src-tauri/windows/nsis-hooks.nsh`) taskkill the process tree plus sweep any legacy ≤0.1.8 orphans whose executable lives under the install directory

## [0.1.8] - 2026-08-17

### Added

- First-launch window centering: the main window and all on-demand tray windows (diagnostics / settings / skills / MCP / remote) open at screen center when no geometry is remembered; the window-state plugin's restore still overrides the default before the first visible frame, so later launches keep the previous position with no flash
- Configurable notification rules: three notification types — task confirmation (approval requested), choice pending (question requested), reply finished (turn completed) — each with its own enable toggle and timing choice (only in background / always); background means no app window is focused, so toasts never interrupt while you are working in the app

### Changed

- The completion-notification toggle migrated into `notify.turn_done` (existing `notify_on_completion` values carry over automatically); completion sound and preview now belong to the reply-finished rule

## [0.1.7] - 2026-08-17

### Added

- Remote access: one tray click brings the full dsh Web UI to a phone or remote browser via a token-bearing link and QR code. Relay is Cloudflare Quick Tunnel (`cloudflared.exe` bundled in the runtime); zero server, account, or configuration
- Embedded token-gate reverse proxy (`remote/proxy.rs`): `?token=` → 302 + HttpOnly cookie, constant-time compare, fixed 500ms delay on wrong tokens, HTTP streaming forward and WebSocket frame bridging to dsh; browser-marker headers (`origin`/`referer`/`sec-fetch-*`) are stripped so dsh's /api trust fence accepts tunneled requests
- cloudflared supervision with stdout URL parsing, exponential-backoff restarts (new domain on reconnect, token unchanged), and process-tree kill on stop
- Tray submenu (start/stop with mutually exclusive enabled states, copy link, show QR), `#/remote` window with QR SVG, and a remote-access row in the diagnostics panel
- Token hygiene: the token never lands in `events.log` (status lines omit the link, tunnel output is redacted) or in toast bodies

## [0.1.6] - 2026-08-17

### Added

- Theme following extended to the shell's own pages: local pages (diagnostics, skills, MCP, settings, splash) converge on CSS variables and switch with dsh's `ui-theme.preference`; tray menu follows via uxtheme `SetPreferredAppMode`
- Language following: new i18n module reads dsh `locale.preference` (zh/en, default from system UI language); local pages, tray menu, window titles, startup progress, notifications, and command errors are fully bilingual

## [0.1.5] - 2026-08-16

### Added

- Built-in custom sounds for the completion notification (silent/default/im/mail/reminder/sms), with a preview command; bundled via `resources/sounds/*.wav`

## [0.1.4] - 2026-08-16

### Added

- Skills management: enable/disable skills by moving them between `skills/` and `skills-disabled/` (hot-reloaded by dsh's watcher), first-launch seeding, import from codex/claude/opencode with conflict handling, and deletion
- MCP management: read/write dsh's `cordis.patch.yml` MCP client entries (atomic writes, BOM-tolerant), toggle via native `disabled` flag with HMR taking effect without restart, import from claude/codex/opencode configs

## [0.1.2] - 2026-08-16

### Added

- Settings window: customizable zoom step (1-25%) and shortcuts, close-window behavior (hide to tray or quit), completion-notification toggle, first-launch theme seeding
- Splash shows a "takes a few minutes" hint only on first launch

### Fixed

- Remember window size/position across restarts via the window-state plugin

## [0.1.1] - 2026-08-16

### Added

- UI zoom in/out (Ctrl+Shift+= / Ctrl+Shift+-, 2% step), persisted per user

## [0.1.0] - 2026-08-15

First public release.

### Added

- Windows x64 desktop shell for `@deepseek-ai/dsh` 0.1.0-rc.6: spawns `dsh web` on a free loopback port and opens the official Web UI when ready
- Bundled portable Node.js 24.19.0 + dsh runtime (zero prerequisites; WebView2 auto-installed by the NSIS installer if missing)
- In-place runtime execution when the install directory is writable, with automatic cleanup of legacy deployed copies; read-only install dirs fall back to a versioned deployed copy under `%LOCALAPPDATA%`
- Process supervision with exponential-backoff restart (up to 5 consecutive failures), one-click restart from tray and diagnostics panel
- Tray residence: close-to-tray, single-instance focus, tray menu (open / diagnostics / restart / quit)
- Native Windows notifications for dsh `approval/requested` and `question/requested` events (via WebSocket `/api/events.mux`), shown only while the window is hidden
- Title-bar theme following of dsh's `ui-theme.preference` (light/dark/system), applied via tao `set_theme` + DWM `DWMWA_USE_IMMERSIVE_DARK_MODE`
- Diagnostics panel: service state, port, PID, 500-line live log ring, autostart toggle
- First-launch progress UI on the splash page: stage-based progress bar with percent and step checklist (shown only when `dsh-home` does not exist yet); real byte-level percent during fallback runtime deployment; ease-toward-95% while waiting for dsh readiness, 100% only on actual ready
- `events.log` debug log at `%LOCALAPPDATA%\DSHDesktop\events.log` (1MB truncation)
- Runtime slimming pipeline (`scripts/prune-runtime.ps1`): installer 45.2MB, installed size 241.8MB
- End-to-end acceptance script (`scripts/acceptance.ps1`) and console-window / process / notification regression tests (24 tests total)

### Known limitations

- On Windows 10 the dark title bar is pure black while focused (system behavior; `DWMWA_CAPTION_COLOR` is Windows 11 only)
- Windows x64 only; macOS/Linux platform hooks are reserved behind the `Platform` trait
