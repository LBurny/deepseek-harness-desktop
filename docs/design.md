# DSHDesktop Design Document

> Written for future maintainers. After reading this you should be able to answer: why each module exists, what a change in one place affects, and where to start when adding a platform or feature.
> 中文版：[design.zh-CN.md](design.zh-CN.md).

## 1. What this is

DSHDesktop is a **Windows desktop shell** for [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) (dsh, DeepSeek's agent harness CLI, npm package `@deepseek-ai/dsh`).

dsh itself ships a `dsh web` command that serves a Web UI on 127.0.0.1. But using it requires installing Node.js 22+, installing dsh globally, and keeping a terminal around. That is too much friction for end users. DSHDesktop packages all of that into a double-clickable desktop app:

- The installer bundles a portable Node.js runtime and dsh (with all production dependencies). The result is **zero prerequisites on the user's machine**: Windows 10/11 x64, with WebView2 downloaded automatically by the installer if missing;
- On launch it spawns `dsh web`, and once ready, opens the official Web UI inside the app window;
- Tray-resident, close-to-tray, automatic crash restart, native notifications for dsh events, and a title bar that follows dsh's theme setting.

**Non-goals**: reimplementing dsh's UI (the shell loads the official Web UI as-is, so a dsh upgrade is a UI upgrade); patching dsh itself (interaction happens only via process args, env vars, and its HTTP/WS interfaces); OS integration beyond what the installer and autostart provide.

## 2. Key design decisions

| Decision | Rationale | Cost |
| --- | --- | --- |
| Embed the official Web UI instead of building our own | dsh is in developer preview and iterates fast; the shell just follows the npm package version | Main window content depends on dsh process health; requires process supervision |
| Bundle Node + dsh in the installer | Zero prerequisites, works out of the box | 45MB installer, ~242MB installed |
| Run the runtime **in place** when the install dir is writable | Saves a ~230MB deployed copy; NSIS per-user installs are writable by default | Must handle the read-only fallback and legacy-copy cleanup |
| Event notifications via WebSocket `/api/events.mux` + `/api/events.host` | dsh's official event downlinks | Upstream API is unstable; isolated behind an adapter |
| Theme follow via 2s polling of settings.yaml | The file is tiny and changes rarely; polling is simpler than inotify and behaves identically cross-platform | Up to 2s of theme-switch latency |
| All platform differences behind a `Platform` trait | Reserves macOS/Linux; `compile_error!` forces an explicit implementation per platform | Trait surface must be designed carefully (§10) |

## 3. Architecture

```
┌──────────────────────── main window (label: "main") ───────────────────────┐
│  local splash page (Svelte)  ──dsh ready──▶  navigate to                   │
│  http://127.0.0.1:<port>  (official dsh Web UI; the shell doesn't touch it)│
└──────────────────────────────────┬─────────────────────────────────────────┘
                                   │ Tauri IPC (invoke commands / emit events)
┌──────────────────────────────────┴─────────────────────────────────────────┐
│ Rust core (src-tauri/src/)                                                 │
│  lib.rs      wiring: plugins → setup → event bridge (dsh state → frontend  │
│              events / window navigation)                                   │
│  download.rs main-window downloads: on_download → system Downloads dir,  │
│              dedup, toast; without it wry silently swallows downloads    │
│  presets.rs  startup rewrite of the shipped minimal preset into a win32  │
│              pwsh variant (signature-gated, idempotent)                  │
│  runtime.rs  runtime location: in-place / read-only fallback deploy /      │
│              legacy copy cleanup                                           │
│  process.rs  DshProcess supervision loop: spawn, readiness probe,          │
│              exponential-backoff restart                                   │
│  notify/     WS subscription to dsh events (mux+host) → classify/book → toast │
│              (only while the window is hidden)                             │
│  theme.rs    polls dsh theme setting → DWM title-bar coloring              │
│  progress.rs first-launch progress model: stage weights, percent mapping,  │
│              structured event payload                                      │
│  tray.rs     tray menu; diagnostics.rs state/logs; commands.rs base cmds  │
│  zoom.rs     UI zoom: configurable step (2% default), hook injection,      │
│              persistence; settings.rs shell settings model + get/set cmds  │
│              persistence                                                   │
│  skills.rs   skills toggle via skills/ ↔ skills-disabled/ + 3-source import│
│  mcp.rs      MCP management: cordis.patch.yml entry r/w, toggle, import    │
│  remote/     remote access: token-gate reverse proxy + cloudflared tunnel  │
│              supervision                                                   │
│  platform/   Platform trait (windows.rs implemented; macos/linux are       │
│              compile-time placeholders)                                    │
└──────────────────────────────────┬─────────────────────────────────────────┘
                                   │ spawn (CREATE_NO_WINDOW, isolated DSH_HOME)
┌──────────────────────────────────┴─────────────────────────────────────────┐
│ Bundled runtime: <install>/runtime/windows-x64/                            │
│   node.exe + dsh/node_modules/@deepseek-ai/dsh/lib/bin.js                  │
│ Runs as: node bin.js web --port <free port> (binds 127.0.0.1 only)         │
└────────────────────────────────────────────────────────────────────────────┘
```

Data directory: `%LOCALAPPDATA%\DSHDesktop\`

```
DSHDesktop\
  dsh-home\          dsh's DSH_HOME (settings.yaml, sessions, user data)
  events.log         process-event debug log (truncated at 1MB; last-resort diagnostics)
  ui-zoom.txt        UI zoom factor persistence (missing/corrupt falls back to 100%)
  settings.json      shell settings (zoom step/shortcuts/close behavior; defaults on missing/corrupt)
  runtime\           exists only in read-only-install fallback mode
                     (deployed copy, marked with a .version file)
```

## 4. Startup sequence

1. **Plugin init**: `single_instance` must be registered first, so that a second launch focuses the existing main window instead of spawning again. `window-state` remembers window geometry: resize/move events update an in-memory cache, the state is written to `%APPDATA%/<identifier>/.window-state.json` on `RunEvent::Exit`, and restored when each window is created on the next launch. Flags are limited to `SIZE | POSITION | MAXIMIZED` — `VISIBLE` is excluded, otherwise quitting while hidden to tray would persist "hidden" and the main window would not show on the next launch. Note the restore runs in the plugin's `window_created` hook (queued via `run_on_main_thread` after creation), i.e. **after the first visible frames** — a probe showed the default size on screen for ~370ms. So the main window is created with `visible(false)` and only `show()`n on `on_page_load(Finished)`, when the restored geometry is already applied (the first visible frame has the remembered geometry; regression: `scripts/verify-no-size-flash.ps1`). Since the download handling landed, the main window is **created in code inside setup** (`WebviewWindowBuilder`, tauri.conf `windows` is empty): `on_download` is a builder-only hook that conf-declared windows cannot take; `visible(false)+center()` and the restore timing are unchanged (code-created windows get the same queued restore on `window_created` — the on-demand tray windows have always worked this way).
2. **setup**: create the main window in code (with `on_download`, see §9) → build the tray → register `BootstrapInfo` (bootstrap-error fallback) → locate the bundled runtime via `resource_dir`.
3. **First-launch detection**: if `dsh-home` doesn't exist before ensure_runtime, this is the first launch (the splash uses this to choose between the progress bar and plain text).
4. **ensure_runtime** (§5): on failure the app does **not** exit. Instead, the error is stored in `BootstrapInfo` and emitted as `dsh-progress` (stage=error), leaving the window on the splash page showing the error. (The frontend actively queries `get_bootstrap_error` because the error event may be emitted before the frontend's listener registers.)
5. **seed_theme_preference → spawn_theme_follower**: on first launch, if settings.yaml doesn't exist, pre-write `ui-theme.preference` from the system dark/light mode (dsh renders light by default, so without seeding a dark system would get "dark title bar + light UI"); then color the title bar once from the system theme and poll every 2s.
6. **spawn_supervised**: invoked via `tauri::async_runtime::block_on` (there is no tokio context inside `setup`, and the supervision loop's `tokio::spawn` needs one). State machine: `Starting → Ready{port}`.
7. **Event bridge** (`bridge_event` in lib.rs): every `dsh-progress` event carries a structured payload `{stage, message, percent}` (progress.rs), with percents computed backend-side from stage weights:
   - `runtime`: in-place → 0→15%; fallback deploy → real byte progress 0→70% (throttled: only emitted when the percent changes)
   - `starting`: 15% or 70% (depending on whether a deploy happened)
   - `ready` → 100% → pushes the port into the watch channel (WS subscriber follows) → emits `dsh-ready` → **navigates the main window to `http://127.0.0.1:<port>/`**
   - `error` → navigates the main window back to the local splash page
   - every event is also appended to `events.log`.
8. Once dsh is ready, the WS subscribers connect to `/api/events.mux` and `/api/events.host`, and the app reaches steady state.

## 5. Runtime management

**Background**: the installer places the runtime verbatim at `<install>\runtime\windows-x64\` (via the tauri.conf `resources` map: `"runtime" -> "runtime"` and `"resources/sounds" -> "sounds"`; a list entry would place `resources/sounds` verbatim at `<install>/resources/sounds/`, while the shell probes for `sounds/*.wav` next to the exe (`resolve_custom_sound`) — a missed probe silently falls back to the system default, hit in 0.1.16, guarded by an anchor test in settings.rs). The original design copied the whole runtime to `%LOCALAPPDATA%` on first launch (to survive read-only install dirs), at the cost of doubling the installed size (~+230MB).

**Current two-tier strategy**:

```
ensure_runtime(source_dir, app_version):
  1. strip_verbatim(source_dir)     # remove the \\?\ prefix (see pitfall below)
  2. validate_source                # node.exe and dsh bin.js must exist, else Incomplete
  3. home = %LOCALAPPDATA%\DSHDesktop\dsh-home (created if missing)
  4. install dir writable? (create_new probe file)
     ├─ writable → delete legacy deployed copy (%LOCALAPPDATA%\...\runtime, only if
     │             it carries our .version marker) → run in place: node_exe/bin.js
     │             point straight into the install dir
     └─ read-only → deploy to %LOCALAPPDATA%\...\runtime:
                    skip if .version == app_version, else full recopy
```

- **Working directory decoupled from the runtime**: dsh's cwd is always `%LOCALAPPDATA%\DSHDesktop` (writable), so even in-place mode never pollutes the install dir.
- **Legacy cleanup only trusts the `.version` marker**, a file we wrote ourselves, so user data is never deleted by mistake.

**Pitfall (fixed; do not regress)**: Tauri's `app.path().resource_dir()` returns a `\\?\`-prefixed verbatim path on Windows (e.g. `\\?\F:\DSHDesktop\...`). Node's module loader does not understand these: it treats the leading `\\?\F:` segment as a drive-relative path and crashes resolving the entry point with `EISDIR: illegal operation on a directory, lstat 'F:'`. `strip_verbatim` strips the prefix only when the remainder is a drive-letter absolute path (`\\?\UNC\...` is left alone), matching `dunce::simplified`'s conservative policy. **Any path handed to Node must come from ensure_runtime. Never assemble one yourself elsewhere.**

## 6. Process supervision

`DshProcess::spawn_supervised` runs a tokio supervision loop with this state machine:

```
Starting ──wait_ready gets any HTTP response within 60s──▶ Ready{port}
   │                                                        │ child.wait() returns (crash/exit)
   │ timeout / 5 consecutive failures                        ▼
   ▼                                            exponential backoff 500ms×2 (capped at 30s)
Failed (no more auto-restarts; manual restart               → back to Starting
        from frontend/tray)
```

- **Spawn**: `node bin.js web --port <port>`, env `DSH_HOME=<home>`, PATH prepended with `<home>/profiles/web/node_modules/.bin` (plugin-shipped CLIs are not on the user's PATH; prepending lets session terminals and tool subprocesses resolve them by name — otherwise an installed plugin's command, e.g. modlens, is unreachable in the dsh terminal), cwd `%LOCALAPPDATA%\DSHDesktop`, stdout/stderr piped into `LogRing` + the event stream, `kill_on_drop(true)`, plus `CREATE_NO_WINDOW` via `Platform::configure_child_command` (no console window pops up).
- **Ports**: `free_port()` asks the OS for a free port. There is a race window between allocation and use, covered by "kill on readiness timeout, retry with another port"; `wait_ready` polls `http://127.0.0.1:<port>/` until it gets **any** HTTP response (200 not required).
- **stop/restart**: two `tokio::sync::Notify`. stop sets the shutdown flag and notifies; the loop kills the process tree (`taskkill /T /F`, since dsh may spawn grandchildren like python) and lands in `Stopped`. restart notifies the live loop to cycle immediately, or re-spawns a supervision loop if the old one already exited (Failed/Stopped).
- **Job object against orphans**: right after spawn, `Platform::register_child(pid)` assigns the child to a global `KILL_ON_JOB_CLOSE` job (the `job` module in `platform/windows.rs`, whose handle is deliberately never closed). Whenever the shell exits by any means — including being force-killed by the NSIS installer or Task Manager — the kernel terminates all members and their descendants with it. Before 0.1.9 there was no such protection: the installer killed only the main binary, and orphaned node.exe/cloudflared.exe kept the runtime directory locked, aborting reinstalls ("Can't write: ...\cloudflared.exe"). The cloudflared supervision loop (remote/tunnel.rs) registers the same way.
- **tokio traps**: `Child::kill()` returns a future that does nothing unless awaited; a leaked child that inherited the stdout pipe keeps the pipe open and any outer task waiting for EOF hangs forever (an integration test once appeared to "hang" for exactly this reason). All child paths must use `kill_on_drop` + explicit `child.wait().await` + fully-nulled stdio in tests.

## 7. Event notifications

```
dsh WS /api/events.mux ──▶ WsSource(mux) ──▶ handle_mux_frame ──┐
dsh WS /api/events.host ─▶ WsSource(host) ─▶ handle_host_frame ─▶ SessionBook
(one WsSource impl for both endpoints: 5s reconnect, port follows  │ (subagent set
 restarts via the watch channel; host reconnect clears the        │ + session titles)
 subagent set first — baseline unknown, fail-open)                 ▼
                                              NotifySink: window focus state +
                                              shell settings' per-type rules → toast
```

- dsh event frames: `{"type":"server-request","method":<payload.type>,"payload":{...}}`, identical on both endpoints (a plain GET returns 426 — WS only).
- Three frame kinds pass on the mux stream:
  - `approval/requested` / `question/requested` (awaiting approval/answer, regex fast path) → **Approval** / **Question** notifications (silent toasts, gated by the `notify.approval` / `notify.question` rules respectively).
  - `session/event` with `event.type=="turn/end"` and `data.reason.kind=="completed"` → **TurnCompleted** notification (optional sound); `aborted`/`error`/`blocked`/`max-tokens` are ignored.
  - `session/event` with `event.type=="session/title"` → recorded in the SessionBook so the completion body carries the session title (「title」回答完成; falls back to "dsh 回答完成").
- **Two-stage filtering**: a plain substring check first, JSON parsing only on hits — during streaming every token chunk is a `session/event` frame, so per-frame parsing is off the table.
- **Subagent filtering**: mux frames carry no origin; the host stream's `host/session-added` (`origin=="subagent"`) / `host/session-removed` maintain the subagent set, and a matching turn/end is dropped. A subagent is always created after the WS connects (created first, runs turns later), so ordering is naturally safe; the host stream replays no baseline, so the set is cleared on reconnect (rather one toast too many than one too few).
- dsh's browser trust fence allows loopback + Origin-less requests, which a Rust client satisfies naturally.
- **The adapter is intentional**: the `NotifySource` trait isolates the unstable upstream API; alternatives (e.g. a `FileWatchSource` parsing session jsonl) can be added later.
- Before showing a toast the sink consults `NotifyRule::allows(foreground)` for the notification's type: **foreground = any app window (main/settings/diagnostics/skills/mcp/remote) is focused** (a visible but unfocused main window means the user has switched away — that counts as background). The user is not interrupted while actively working in the app; toasts appear while it runs in the background (except types set to timing=always). Before showing it writes one `Notify: {kind} {body}` line to events.log (the field-diagnosis hook for the notification path).
- **Notification rules** (settings.json): `notify.{approval,question,turn_done}`, each a `{enabled, timing}` rule — timing ∈ `background` (default: only notify when no app window is focused) / `always` (notify even in the foreground); all three types default to on. The legacy `notify_on_completion` boolean migrates into `notify.turn_done.enabled` at load time (dropped on read, never written back). `completion_sound` (silent/default/im/mail/reminder/sms/chime/drop/mellow, default `default`) applies only to TurnCompleted. The sound maps straight onto toast audio presets (`ms-winsoundevent:Notification.*`, built into Windows regardless of the user's sound scheme; no sound value → silent toast, which is what Approval/Question get). Preview goes through the `preview_completion_sound` command, showing a toast with the chosen sound (the sound is a property of the toast, so it can only be auditioned with one).

## 8. Theme following

Goal: when dsh's `ui-theme.preference` (light/dark/system, stored in `$DSH_HOME/settings.yaml`) changes, every app window's title bar follows.

- **2s polling** of the settings file (tiny file, rare changes; polling is simpler than inotify and identical cross-platform); `system` resolves via the registry value `AppsUseLightTheme`.
- **First-launch seeding (seed_theme_preference)**: when settings.yaml is missing or has no preference, dsh **renders its UI light by default**, while the shell's title bar defaults to the system theme — on a dark system the first launch showed a "dark title bar + light content" mismatch. Between ensure_runtime and spawn_supervised, if settings.yaml doesn't exist, pre-write `ui-theme.preference` (dark/light, no BOM) from the system mode so dsh's first render matches the shell. An existing file is never touched.
- **Splash light-mode adaptation**: the splash page switches between dark and light palettes via `prefers-color-scheme`, staying consistent with the light title bar on light systems.
- **BOM trap (fixed; do not regress)**: PowerShell 5.1's `Set-Content -Encoding utf8` writes a UTF-8 BOM, and yaml-rust rejects BOMs, so the parse failure **silently falls back to the system theme**, which presented as "the title bar is stuck white". `read_theme_preference` strips the BOM before parsing. Lesson: **never rewrite settings.yaml with PowerShell**.
- **Windows two-pronged application**:
  1. `window.set_theme()` syncs tao's internal theme state. Otherwise tao may overwrite the visual effect with its cached stale state on the next window event. Calls on hidden windows can error or even panic, so they're wrapped in `catch_unwind`;
  2. `DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE=20)` directly on the HWND: cache-free, idempotent, and effective on hidden windows; this is the authoritative source of title-bar color. If attribute 20 fails (E_INVALIDARG), fall back to the legacy value 19 (pre-20H1 Windows 10).
- **Non-client area must be force-redrawn (fixed; do not regress)**: `DwmSetWindowAttribute` changes the attribute without repainting the title bar — the caption keeps the old color until the next activation ("it only switches when I click"). tao's `set_theme` fakes `WM_NCACTIVATE` to trigger a repaint, but that trick fails under some timing/focus states (winit/Electron abandoned it for the same reason). When the resolved theme actually changes (gated by the `LAST_APPLIED` atomic; unchanged polls don't force-redraw, so visible captions don't flicker every 2s), every window gets `SetWindowPos(SWP_FRAMECHANGED|SWP_NOACTIVATE|NOMOVE|NOSIZE|NOZORDER)` + `RedrawWindow(RDW_FRAME|RDW_INVALIDATE|RDW_UPDATENOW)` (the Chromium/Windows Terminal approach), with a trace line in events.log. Separately, a newly created window's DWM attribute comes from the *system* theme (tao creation behavior), so under "system light + dsh dark" it is born with the wrong caption color; `on_page_load` calls `apply_before_show` to set the attribute before the first visible frame (and force-redraws when the window is already visible, i.e. main-window full-page navigations). Regression: `scripts/verify-titlebar-theme.ps1` (asserts caption pixels switch without any click).
- **Known limitation**: on Windows 10 a dark title bar is **pure black when focused, dark gray when unfocused**. That is system behavior; `DWMWA_CAPTION_COLOR`(35)/`DWMWA_TEXT_COLOR`(36) only exist on Windows 11. A constant dsh-gray (#1B1B1C) bar would require a frameless window plus a custom title bar injected via `initialization_script`. Deferred.

## 9. Frontend and window management

The shell has exactly four local pages, routed by **hash** (`App.svelte` listens to `hashchange`):

- `#/` (default) **Splash.svelte**: the startup page. On mount it first `invoke('get_bootstrap_error')` for any bootstrap error and `invoke('is_first_launch')` for the first-launch flag, then listens to the structured `dsh-progress`. **On first launch** it shows a stage-based progress bar (percent number + a ✓/●/○ step checklist) plus a "first launch deploys the runtime and may take a few minutes" hint (rendered only in this branch, never on later launches): the backend supplies percent floors for the runtime/starting stages; during `starting` the frontend eases asymptotically toward 95% (dsh exposes no finer progress; the easing is presentation-only and never tops out), and `ready` pins it to 100%. **On subsequent launches** it keeps plain text plus an indeterminate bar. Once dsh is ready, **the Rust side** navigates the main window to the dsh UI. The frontend never navigates itself.
- `#/diagnostics` **Diagnostics.svelte**: the diagnostics panel (state/port/PID/version, a 500-line live log backfilled from the ring plus the `dsh-log` event stream, a restart button, an autostart toggle).
- `#/settings` **Settings.svelte**: shell settings (launch at login, close-behavior radio, three notification rules — task confirmation / choice pending / reply finished, each with an enable checkbox plus an only-in-background/always timing dropdown, the reply-finished row tied to the completion sound + preview, zoom step 1%–25%, zoom in/out shortcut recorders). On save the frontend validates first (at least one modifier key, no in/out conflict), then calls `invoke('set_shell_settings', { next })` for Rust-side revalidation and persistence.
- `#/skills` **Skills.svelte**: skill management. The data root is the **DSH_HOME the shell injects into dsh** (`<runtime_base>/dsh-home`, not `~/.dsh`): `skills/` holds enabled skills, the sibling `skills-disabled/` holds disabled ones (dsh's skill discovery only scans direct entries of known roots and has no native disable flag; moving a directory out of the root disables it, and dsh's watcher hot-refreshes its catalog — no restart needed). Import copies skill directories from three external-agent sources: Codex `~/.codex/skills`, Claude Code `~/.claude/skills`, OpenCode `~/.config/opencode/skills`; same-name conflicts are resolved per item (overwrite also clears any stale copy in the disabled dir). **Standalone dsh's default `~/.dsh/skills` is not an import source** — the shell *is* dsh, so at every launch it auto-imports new skills from there (`skills::seed_from_default_dsh_home`; the `.skills-seeded` marker remembers seen names so deleted skills never resurrect). Delete removes only the copy inside the home, never the source. The Rust side (`skills.rs`) parses only single-line `description` from SKILL.md frontmatter and keys all row operations by directory name.
- `#/mcp` **Mcp.svelte**: MCP server management (list / enable-disable / delete / add / edit / import). dsh has no standalone mcp.json — MCP servers are Cordis plugin patches, so the shell reads and writes the `name == '@deepseek-ai/dsh-mcp-client'` insert entries in `<dsh-home>/profiles/web/cordis.patch.yml` (other entries preserved at the YAML-value level; atomic tmp+rename writes; BOM stripped on read). dsh's HMR (`watchUserPatches` + chokidar) watches this file and hot-swaps MCP clients on change — **no restart needed**. Enable/disable toggles the entry-level `disabled: true` flag (native cordis-plugin-loader semantics: a disabled entry starts no fiber). Editing overlays form fields onto the existing config, preserving advanced keys like `toolCallTimeoutMs` / `reconnect.*`; only two transports exist — `stdio` (command/args/env/cwd) and `streamable-http` (url/headers); sse is unsupported. At launch, MCP entries are seeded from both `~/.dsh` patch layers (`mcp::seed_from_default_dsh_home`, `.mcp-seeded` marker prevents resurrection; entries disabled at the source are neither imported nor marked, so enabling them in ~/.dsh later still syncs). Manual import covers three sources: Claude Code `~/.claude.json` `mcpServers` (stdio/http mapped, sse flagged unsupported), Codex `~/.codex/config.toml` `[mcp_servers.*]` (`enabled=false` entries not listed), OpenCode `~/.config/opencode/opencode.json` `mcp` (local/remote mapped); conflicts are per-item overwrite/skip. If the patch file fails to parse, the page degrades to read-only and asks the user to edit the file by hand.
- `#/remote` **Remote.svelte**: remote access. State comes from a `get_remote_status` snapshot plus the `remote-status` event stream; while Up it shows the QR code (SVG from `get_remote_qr`) and the full link, re-fetching the QR whenever the link changes (tunnel reconnects rotate the domain); the toggle button calls `start_remote` / `stop_remote` by phase.

Window behavior:

- Main window `main`: **close behavior is configurable** (`close_behavior` in settings.json): the default `background` hides to tray (`CloseRequested` → `prevent_close` + `hide`); `quit` runs the same exit path as tray "Quit". Shown again via the tray menu or a second launch (single-instance plugin) with `show` + `unminimize` + `set_focus`.
- Diagnostics window `diagnostics`, settings window `settings`, skills window `skills`, MCP window `mcp`, remote window `remote`: created on demand from the tray menu, **close = destroy**, recreated next time.
- Tray "Quit": `stop()` remote access first (kill the cloudflared process tree + shut down the auth proxy, invalidating the link immediately), then `stop()` dsh, wait 1.5s for the supervision loop to kill the process tree, then `exit(0)`.
- After navigating to the remote URL, the window title is overwritten by dsh's `document.title`, so **external scripts must not locate the window by title** (match by PID + class name, see `scripts/shot-window.ps1`).
- **Centered on first launch**: the main window (builder `.center()` in setup; tauri.conf `windows` is now empty) and the five on-demand tray windows all default to screen center; the window-state plugin's restore is queued at window_created and runs before the first visible frame, so a remembered geometry overrides the centered default — first launch centered, later launches at the previous position, with no visible flash of the default (verify-no-size-flash.ps1 asserts the first visible frame already has the remembered geometry).
- **Downloads (download.rs)**: a WebView2 download the host never handles dies silently — wry's default handler allows it but suppresses the download UI via `SetHandled(true)`, so the user never learns where the file went (this is what swallowed dsh's "Session log" export). The main-window builder carries `on_download`: on Requested the destination is moved to the system Downloads folder (`dirs::download_dir`, with " (n)" dedup), on Finished a toast reports the saved path or the failure; both are logged to events.log (`Download: requested/finished`).

IPC commands: commands.rs carries 8 — `get_shell_ui_state` / `get_status` / `restart_dsh` / `get_recent_logs` / `get_autostart` / `set_autostart` / `get_bootstrap_error` / `is_first_launch` — plus `zoom_ui` in zoom.rs, `get_shell_settings` / `set_shell_settings` / `preview_completion_sound` in settings.rs, and `list_skills` / `list_import_sources` / `import_skills` / `set_skill_enabled` / `delete_skill` in skills.rs, plus `list_mcp_servers` / `upsert_mcp_server` / `set_mcp_enabled` / `delete_mcp_server` / `list_mcp_import_sources` / `import_mcp_servers` in mcp.rs, plus `start_remote` / `stop_remote` / `get_remote_status` / `copy_remote_link` / `get_remote_qr` in remote/mod.rs (28 total).

Shell settings (settings.rs):

- **Model**: `settings.json` stores `zoom_step` (0.01–0.25, clamped), the `zoom_in`/`zoom_out` shortcuts (`{ctrl, shift, alt, code, key}`), `close_behavior` (`background`/`quit`), `notify` (`{approval, question, turn_done}` rules of `{enabled, timing}`, default all on with background-only timing; the legacy `notify_on_completion` boolean migrates into `notify.turn_done.enabled` at load and is never written back), and `completion_sound` (`silent`/`default`/`im`/`mail`/`reminder`/`sms`/`chime`/`drop`/`mellow`, default `default`). Missing/corrupt file → all defaults; partially missing fields → per-field defaults (serde default); failed validation (modifier-less shortcut, in/out conflict) → all defaults rather than running with a broken state.
- **SettingsState**: managed state holding the in-memory value plus the persistence directory; `set` clamps/validates first, then writes, then swaps memory — on validation failure both memory and disk keep the old value.
- **Effective on save**: after `set_shell_settings` succeeds, the main window's zoom hook is re-injected (the shortcut definitions are embedded in the script, so a re-inject is required). The step is not baked into the script — `zoom_ui` reads it from settings at call time — so step changes need no re-injection.

UI zoom (zoom.rs):

- **Shortcuts**: `Ctrl+Shift+=` zooms in and `Ctrl+Shift+-` zooms out by default (customizable in the settings window); the additive step defaults to ±2 percentage points (configurable 1%–25%, factor clamped to 25%–500%). The hook script is generated by `hook_js(&ShellSettings)` — the shortcut definitions are embedded as JSON, and matching mirrors `Shortcut::matches`: physical `e.code` first, `e.key` as fallback (synthesized keystrokes and RDP-injected keydowns arrive with an empty `e.code`, so a code-only match silently breaks there), meta never matches. `on_page_load` eval-injects it after every full page load (**main window only** — otherwise the settings window's shortcut recorder would be pre-empted by the hook; covers both the local splash and the remote dsh UI), intercepts at capture phase, and invokes `zoom_ui` (payload `direction: "in"/"out"`), which applies the factor through WebView2's native `SetZoomFactor` — the same mechanism as browser Ctrl++. The listener is hot-replaceable (`__dshZoomHookHandler` holds the previous handler; re-injection `removeEventListener`s it before adding the new one, so handlers never stack).
- **Persistence**: every change is written to `%LOCALAPPDATA%\DSHDesktop\ui-zoom.txt`; a missing/corrupt file falls back to 100%; `on_page_load` re-applies the current zoom on every page load (also covers WebView2 recreations).
- **Remote IPC**: the dsh UI is a remote origin, and Tauri routes all remote-origin IPC through the ACL (without an app manifest, remote calls are rejected outright). So build.rs declares all 28 commands via `AppManifest::commands` (generating `permissions/autogenerated/allow-*.toml`), and `capabilities/dsh-remote.json` grants only `allow-zoom-ui` to `http://127.0.0.1:*`. **Side effect**: app commands from local pages also become ACL-gated; default.json allows them one by one — **adding a command means touching three places**: the build.rs commands list, capabilities/default.json (local), and dsh-remote.json (remote, if needed).

## 10. Platform abstraction

```rust
pub trait Platform: Send + Sync {
    fn node_exe_name(&self) -> &'static str;            // "node.exe" / "node"
    fn runtime_base_dir(&self) -> PathBuf;              // app data root
    fn resource_runtime_dir(&self, resource_dir: &Path) -> PathBuf; // <res>/runtime/<triplet>
    fn runtime_triplet(&self) -> &'static str;          // "windows-x64" / "darwin-arm64" ...
    fn kill_process_tree(&self, pid: u32);              // taskkill /T /F
    fn configure_child_command(&self, _cmd: &mut Command) {} // CREATE_NO_WINDOW etc.
    fn system_dark_mode(&self) -> bool;                 // registry AppsUseLightTheme
}
```

`mod.rs` carries `compile_error!` placeholders for macOS/Linux, so adding a platform means the compiler forces you to implement the trait and wire up `current()`. The accompanying work: teach `scripts/fetch-runtime.ps1` to download Node for the new triplet, add dmg/appimage to tauri.conf `bundle.targets`, and uncomment the matrix rows in `.github/workflows/build.yml` (a checklist lives in the comments there). Title-bar coloring is `cfg(windows)`-branched in `theme.rs`; other platforms just use `set_theme`.

## 11. Packaging and distribution

### Runtime pipeline

```
scripts/fetch-runtime.ps1
  1. download Node v24.19.0 win-x64 zip, keep only node.exe
  2. npm install --prefix dsh --omit=dev @deepseek-ai/dsh@0.1.0-rc.8
  3. smoke test: node bin.js --help
  4. run scripts/prune-runtime.ps1 to slim it down
output: src-tauri/runtime/windows-x64/ (gitignored, never committed)
```

`prune-runtime.ps1` rules (supports `-WhatIf` dry run):

- generic: delete `test/tests/__tests__/docs/example/examples/coverage/.github`-style directories; delete `*.d.ts/*.map/*.md/LICENSE*/CHANGELOG*`-style files;
- node-pty: keep only `prebuilds/win32-x64` (dropping darwin-*/win32-arm64 saves ~30MB), plus src/deps/third_party etc.;
- `@img/sharp-wasm32` (9MB): unused when sharp's win32-x64 native package exists, so it is deleted.

Result: staging 344 to 227.9 MB; installer **45.2 MB**; installed **241.8 MB** (the bulk is node.exe at ~90 MB plus dsh's dependency tree and node-pty's native terminal module; nothing left is safe to cut).

### Installer

- `pnpm tauri build` → NSIS `src-tauri/target/release/bundle/nsis/DSHDesktop_<ver>_x64-setup.exe`.
- WebView2: if missing, the installer downloads and installs it (downloadBootstrapper mode), so the installer itself doesn't carry WebView2.
- Silent install: `setup.exe /S` (add `/D=<dir>` for a custom directory). A running old instance is terminated by the installer itself; no manual uninstall or kill needed.
- NSIS hooks (`src-tauri/windows/nsis-hooks.nsh`, wired via `bundle.windows.nsis.installerHooks`, **path relative to `src-tauri`**): `NSIS_HOOK_PREINSTALL`/`NSIS_HOOK_PREUNINSTALL` first `taskkill /F /IM DSHDesktop.exe` to kill the main process (**never `/T`**: the installer spawned by install_update and the in-place `_?=` old uninstaller are both descendants of DSHDesktop.exe, so a tree kill terminates them too — this is why "Install now" silently failed up to 0.1.16; since 0.1.9 the KILL_ON_JOB_CLOSE Job already reaps all children when the main process dies, making the tree kill redundant), then use PowerShell to sweep any remaining process whose executable lives under `$INSTDIR` (orphaned node.exe/cloudflared.exe left by ≤0.1.8), then poll until they are gone (≤10s) so the kernel releases file handles. The template's built-in `CheckIfAppIsRunning` kills only the main binary and cannot reach child processes — relying on it alone reproduces the "Can't write" failure. **The path sweep must exclude the caller itself** (done via the PowerShell parent PID): on reinstall/upgrade the template runs the old uninstaller in place via `_?=$INSTDIR` in `PageLeaveReinstall`, and `$INSTDIR\uninstall.exe` matches `$INSTDIR\*` too — 0.1.9~0.1.12 did not exclude it, so the uninstaller killed itself mid-hook, deleted nothing, and exited non-zero, making the new installer abort with "Unable to uninstall!". Standalone uninstalls never hit this because the uninstaller self-copies to %TEMP% first. `NSIS_HOOK_POSTUNINSTALL` additionally `RMDir /r "$INSTDIR\runtime"` to remove off-manifest leftovers (files added by dsh self-updates) so the template's empty-dir RMDir can remove `$INSTDIR`. Known issue: upgrading from ≤0.1.12 to ≥0.1.13 shows that dialog one last time (the old uninstaller cannot be fixed by the new installer); uninstall via Windows Settings first, then install.
- If GitHub is unreachable from the build machine, the NSIS download can be pre-seeded via a mirror into `%LOCALAPPDATA%\tauri\NSIS` (details in AGENTS.md).

### CI and release

- `.github/workflows/build.yml`: on tag `v*` or manual dispatch → windows-latest: fetch-runtime → `cargo test` → `tauri build` → upload artifact.
- `.github/workflows/release.yml`: on tag `v*`, builds and publishes the setup.exe plus its SHA256 straight to a GitHub Release.
- Version numbers live in three places and must be bumped together: `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`.

## 12. Test strategy

| Layer | Coverage | Command |
| --- | --- | --- |
| Rust unit tests (99) | runtime deploy/fallback/path normalization/copy-progress callback, progress stage weights & percent mapping, theme BOM parsing & first-launch seeding, notify frame classification/subagent book/summary, LogRing eviction, port allocation & readiness, platform basics, zoom clamp/persistence/hook embeds settings, settings model/validation/persistence/sound enum/notification-rule gating & legacy-key migration, skills frontmatter parsing/list/toggle/delete/import conflicts, mcp patch parsing/toggle/delete/upsert validation & advanced-key preservation/seed marker/three-source parsing & import conflicts, remote tunnel-URL parsing & token redaction | `cd src-tauri && cargo test` |
| Process integration (2, tests/process.rs) | with `tests/fixtures/fake-dsh.cjs` (a scriptable-crash fake dsh): ready→HTTP 200→stop, and crash→auto-restart→second Ready | same |
| Notification integration (2, tests/notify_ws.rs) | the fixture serves both WS endpoints; verifies approval filtering, turn/end completion notifications (with title), and subagent filtering | same |
| Remote access integration (12, tests/remote_{proxy,tunnel,manager}.rs) | gate 403/302/cookie/forward/browser-marker-header stripping (avoids dsh trust-fence 403)/WS-bridge/503/shutdown-releases-port, fake-cloudflared URL parsing & crash-restart, manager full chain (missing-exe error, up→stop, idempotent start) | same |
| Console-window regression (2, tests/console_window.rs) | positive: a CREATE_NO_WINDOW child has **no visible** ConsoleWindowClass window; control: a CREATE_NEW_CONSOLE child **has one** (proves the detector works; a real console window briefly flashes on screen, which is expected) | same |
| End-to-end acceptance (scripts/acceptance.ps1) | uninstall old → silent install → launch → wait for dsh ready → single-instance / no visible console / theme / screenshots | `powershell -File scripts/acceptance.ps1 -SetupExe <exe>` |

After touching process/notification/theme logic: run `cargo test` and one full acceptance pass on a fresh install.

**Debugging aids, in order of preference**: the diagnostics panel (in-app) → `%LOCALAPPDATA%\DSHDesktop\events.log` (one line per process event, truncated at 1MB; the panel needs in-app interaction, so this file is the only option when startup hangs) → external scripts: `scripts/check-node.ps1` / `get-attr20.ps1` / `shot-window.ps1`.

## 13. Known limitations and roadmap

- **Windows 10 dark title bar is pure black when focused**: system behavior, see §8. Path forward: frameless window + custom title bar (needs extra work for Win10 edge-snapping); deferred.
- **Notification coverage**: approval/question + turn completion (turn/end/completed, optional sound); failed turns (kind==error) don't notify yet. Subagent filtering relies on incremental events.host frames — during a host reconnect window one extra toast may slip through (fail-open). Further event kinds wait on upstream API stabilization.
- **Pinned dsh version**: locked per app release (`-DshVersion` in fetch-runtime), so upgrading dsh means releasing a new app version (tracking flow in §14). An in-app dsh channel selector is a possible future feature.
- **UI zoom applies to the main window only**: the diagnostics/settings windows get neither the hook nor the zoom factor; shortcuts and step size are customizable under "Settings".
- **fs-local directory listings fail wholesale on the first ACL-denied child**: upstream behavior — each listed child is resolved, and any denied entry (e.g. `DumpStack.log` at `C:\`, other users' profile dirs under `C:\Users`) turns the whole listing into `cannot list ...: permission denied`. Listing a system drive root on Windows always trips this. The shell does not fix the listing; mitigation is keeping the model inside its workspace by telling it the working directory (the minimal preset's persona now states it).
- **Windows x64 only**: the platform abstraction is ready; see the checklist in §10.

## 14. Update strategy (tracking upstream dsh)

Upstream source: [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) (npm package `@deepseek-ai/dsh`). The shell does not fork, patch, or modify dsh sources (§1 non-goals) — it only tracks releases. **The sole exception** is presets.rs's Windows fix for the minimal preset: upstream rc mounts a PTY-backed persistent bash there whose terminal inspector is unimplemented on win32, and composeProfile unconditionally rewrites the agent-presets row's roots to the shipped root, so no profile-patch layer can inject a shadow root — the only lever is rewriting the shipped preset file in place at startup (a composition, not code). The patch is signature-gated (stops applying once upstream adds a win32 branch), idempotent, and re-applied after dsh self-updates; remove it wholesale once fixed upstream. dsh's internal improvements (startup speed, UI iterations) are transparent to the shell: rebundling inherits them for free.

**Pinned version**: dsh is locked per app release (`-DshVersion` in `fetch-runtime.ps1`) and bundled into the installer; the dsh on users' machines never self-updates — **upgrading dsh means shipping a new app release**.

**Tracking flow (a routine upgrade is process-only, no code changes)**:

1. Watch upstream releases and the npm version stream; check the changelog against the §15 facts table for contract changes.
2. Bump the `-DshVersion` default in `scripts/fetch-runtime.ps1` and re-fetch the runtime (smoke test + pruning included). If upstream adds dependencies (native modules, multi-platform prebuilds), adjust `prune-runtime.ps1` rules as needed.
3. `cd src-tauri && cargo test` all green → `pnpm tauri build` → full pass of `scripts/acceptance.ps1`. **The contract checklist below runs automatically via `tests/upstream_contract.rs`** (probes the real bundled runtime; auto-skips when no runtime is present, and CI's fetch-runtime runs before cargo test so CI always exercises it): a red suite means upstream moved something — follow the failure output to the matching constant in `src-tauri/src/upstream.rs` (the single source of truth for every upstream fact, each annotated with its provenance and blast radius), and touch the consuming module only if needed.
4. Sync the version number in all three places (§11), tag `v*`; CI builds and publishes the GitHub Release.

**Interface contract checklist (code changes only when upstream moves these; code form: `src-tauri/src/upstream.rs` constants + `tests/upstream_contract.rs` probes)**:

| Upstream fact (current values in §15) | Where to change |
| --- | --- |
| Entry point `lib/bin.js` path | `DSH_PKG_SEGMENTS`/`DSH_BIN_SEGMENTS` in `upstream.rs` (`paths_for`/`validate_source` in `runtime.rs` follow via the helper) |
| `web --port <N>` command shape | `DSH_WEB_SUBCOMMAND`/`DSH_PORT_FLAG` in `upstream.rs` (referenced by the spawn in `process.rs`) |
| Event channels `/api/events.mux` + `/api/events.host` and frame format | `EVENTS_*_PATH` and the frame constants in `upstream.rs` (referenced by the `notify/` adapter; the `NotifySource` trait isolates the source implementation) |
| `ui-theme.preference` in `settings.yaml` | `SETTINGS_FILE`/`KEY_*` in `upstream.rs` (parsing + first-launch seeding in `theme.rs`) |
| Node version requirement | `-NodeVersion` in `fetch-runtime.ps1` + `DSH_NODE_MAJOR_FLOOR` in `upstream.rs` |
| WS trust fence (loopback / no Origin) | probed directly by the contract suite (wrong Origin → 403); if the fence behavior changes, re-evaluate the header-stripping in `remote/proxy.rs` |
| WelcomeNotice ternary needle | `WELCOME_NOTICE_NEEDLE` in `upstream.rs` (bundle rewrite in `remote/proxy.rs`) |

Contract breaks mostly surface in `cargo test` (WS notification integration, theme parsing unit tests) or the end-to-end acceptance run; in the field, check `events.log` first.

**Rollback**: if a new dsh turns out broken while the shell needs to ship a fix, revert `-DshVersion` to the last known-good version and rebundle — user data lives entirely in `dsh-home`, decoupled from the dsh version.

## 15. Appendix: upstream dsh facts (0.1.0-rc.8)

> This table is the documentary form; its code form lives in `src-tauri/src/upstream.rs` (single source of truth), and `tests/upstream_contract.rs` verifies it automatically. When a tracking bump changes `upstream.rs`, sync this table.

| Fact | Value |
| --- | --- |
| npm package | `@deepseek-ai/dsh@0.1.0-rc.8` (subpackages use floating ranges and resolve to the newest rc at fetch time; the npm `latest` tag may lag the newest rc, so fetch must pass `-DshVersion` explicitly; dsh-web-app defaults `openBrowser` to true since rc.8) |
| Node requirement | `^22.19 \|\| >=24` (declared in the upstream repo; the published tarball carries no engines field — confirmed by the contract suite. Bundled: v24.19.0) |
| Entry point | `node_modules/@deepseek-ai/dsh/lib/bin.js` |
| Web command | `bin.js web --port <N> --no-open`, binds 127.0.0.1 only; `--no-open` suppresses the system-browser popup (dsh-web-app defaults `openBrowser` to true since rc.8) |
| Event channels | WebSocket `/api/events.mux` + `/api/events.host` (GET → 426, WS only) |
| Event frame | `{"type":"server-request","method":<payload.type>,"payload":{...}}`; completion detection uses `turn/end` (`data.reason.kind`) inside `session/event`, subagent marking uses `origin` on `host/session-added` |
| Settings file | `$DSH_HOME/settings.yaml` → `ui-theme.preference: light\|dark\|system` |
| Trust fence | allows loopback + Origin-less WS connections |
| Agent presets | `config/agent-presets/{minimal,standard,code,cordis}`; since rc.8 all ship win32 platform branches (minimal gates persistent-bash/persistent-pwsh by `process.platform`, and subprocess-local gained a win32 terminal inspector) → the shell's in-place rewrite patcher is retired; presets.rs now only holds the read-only signature probe (the contract suite asserts `UpstreamHandled` as a regression sentinel) |
| Preset roots | composeProfile force-rewrites the agent-presets row's roots to the shipped root (`config/agent-presets/`); the `$DSH_HOME/.agent-presets` user root is appended last and shipped wins duplicate ids |
| WebView2 downloads | silently canceled unless the host handles DownloadStarting; wry's default allows them with the download UI suppressed → the shell's download.rs takes over explicitly |
| License | MIT (Copyright 2026 DeepSeek) |

## 16. Remote access (Quick Tunnel + embedded auth proxy)

One click on the tray "Remote access" item gives a phone/remote browser the **full dsh Web UI** via a token-bearing link. No server, no account, no configuration: the relay is Cloudflare's free Quick Tunnel (`cloudflared.exe` bundled in the runtime; anonymous ephemeral tunnel, outbound-only — no public IP, port mapping, or firewall pinhole).

**Chain**:

```
phone browser —HTTPS→ Cloudflare edge → cloudflared (desktop, outbound-only)
  → 127.0.0.1:<random port> remote::proxy (token gate)
  → 127.0.0.1:<dsh port>  dsh web (HTTP + /api/events.* WS)
```

**Modules** (`src-tauri/src/remote/`):

- `mod.rs` — `RemoteManager`: lifecycle (start/stop/status), token generation (fresh 256-bit hex on every start), 6 invoke commands (start_remote/stop_remote/get_remote_status/copy_remote_link/get_remote_qr/reset_remote_link). Tunnel events compose `link = {url}/?token={token}`; status changes broadcast `remote-status`, update tray submenu enabled states, and append to events.log. `reset_link` rotates the token in place and drops all live sessions (the revocation path for a leaked link); only available while Starting/Up, tunnel and domain unchanged.
- `proxy.rs` — axum reverse proxy bound to 127.0.0.1 only. Auth: a valid `__dsh_remote` cookie forwards directly; a matching `?token=` (constant-time compare) → 302 that strips the token + sets an HttpOnly cookie; a mismatched token → fixed 500ms delay then 403; no credentials → 403 gate page. HTTP forwards via reqwest streaming (3xx passed through, never followed); WS upgrades are terminated at the proxy then re-established to dsh with frame-level bidirectional bridging. The dsh port is read from a `watch::Receiver`, so dsh restarts don't break the proxy. **The token lives in a shared cell (RwLock) and the gate reads the latest value per request** — after a reset the old link/cookie die instantly; **WS bridges also select on a drain Notify** (`enable()` registers interest up front so a reset during connect can't slip through), and reset/shutdown `notify_waiters()` drops every established connection — otherwise an attacker's already-open page keeps receiving the event stream after revocation. **Browser-marker headers must be stripped when forwarding** (`origin`/`referer`/`sec-fetch-*`): dsh's /api trust fence (dsh-client-connection `isTrustedApiRequest`) requires Origin.host == Host header and rejects `sec-fetch-site: cross-site`; through the tunnel the Origin is the trycloudflare domain, so every RPC call from the page would 403. Stripped, the request looks like an Origin-less loopback client to dsh (the WS bridge side already sends no Origin in the tungstenite handshake, so it passes natively). **The forwarding client must use `.no_proxy()`** — otherwise a user-level system proxy (e.g. Clash) hijacks the 127.0.0.1 forwarding. **Plugin-bundle rewrite**: `/plugins/*/client.js` responses are buffered (≤4MB, identity encoding only) and every `connection.isLoopback ? "host" : "memory"` is replaced with `"host"` — dsh's internal-testing notice (WelcomeNoticeStore) picks memory persistence for non-loopback origins, so through the tunnel domain the acknowledgement never reaches settings.yaml and the modal pops on every visit; rewritten, remote clients share the host-persisted acknowledgement with the desktop (which has already confirmed, so remote never pops). The rewrite path strips `accept-encoding` (forcing identity) and conditional request headers (no 304) from the forwarded request, and drops `content-length`/`etag` from the response; if the needle is absent (dsh changed the wording upstream) the body passes through untouched — the notice still pops, but nothing breaks.
- `tunnel.rs` — `TunnelProcess` supervision (mirrors DshProcess): `cloudflared tunnel --url <proxy addr> --no-autoupdate`; parses `https://<rand>.trycloudflare.com` from stdout with a regex (failure if unseen within 60s), exponential-backoff restarts (after a reconnect the **domain changes, token stays**), stop via `kill_process_tree` + `kill_on_drop`.

**Security model**: the link is the credential (tray/QR page warn "do not share"); the token regenerates on every start, and stopping remote access or quitting the app invalidates it immediately (quit order: stop remote first, then dsh). On a leak, "Reset link" revokes with one click (token rotation + dropping live sessions, domain unchanged). Neither the proxy nor dsh listens on non-loopback. **The token never touches logs**: events.log records only phase/url/error/proxy_port (never the link), `?token=` query strings in cloudflared output are redacted via `redact_token`, and the up-toast body omits the link (system notification center keeps history) — it just points to the tray menu. Known trade-off: links are not stable — phones cannot bookmark them (each start requires a fresh scan), in exchange for the shortest possible leak window.

**Distribution**: `fetch-runtime.ps1 -CloudflaredVersion` downloads `cloudflared-windows-amd64.exe` from GitHub releases (ghproxy fallback) into `runtime/<triplet>/cloudflared.exe`, riding the existing `resources: ["runtime"]` bundling and `.version` deploy comparison; a missing binary makes start report the error state (the dev fixture runtime is allowed to lack it).

**UI**: tray submenu (start/stop mutually exclusive enabled, copy link, show QR, reset remote link — copy/QR/reset only available when Up) + the `#/remote` local window (QR SVG generated by the `qrcode` crate, copy button, reset button with confirm, toggle) + a "remote access" status row in the diagnostics panel. After a locale-switch tray-menu rebuild, the `TrayRemoteItems` handles are replaced and enabled states re-applied from the current phase.

**Tests**: `tests/remote_proxy.rs` (gate 403/302/cookie/forward/WS bridge/503/shutdown releases the port/plugin-bundle ternary rewrite), `tests/remote_tunnel.rs` (fake-cloudflared.cjs: URL parsing, crash restart, stop), `tests/remote_manager.rs` (missing cloudflared → error; fixture dsh + fake tunnel full chain up→stop). The real-tunnel path is not automated (needs outbound internet); manual acceptance: start from tray → scan on phone → drive dsh end-to-end.
