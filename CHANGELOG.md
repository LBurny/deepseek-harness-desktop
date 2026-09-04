# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.5] - 2026-09-04

### Fixed

- **手机端输入框聚焦→返回后整页放大、标签栏与输入框都看不到**（0.5.4 手机实拍反馈，0.5.4 视口复位修的是平移残留、对缩放无效）：根因是 iOS WKWebView（含微信内置浏览器）聚焦 font-size<16px 的输入框时**自动缩放整页**、收起键盘后缩放不复原——dsh composer 实测字号 `var(--dsh-content-font-size,14px)`，命中触发条件；dsh 入口文档的 viewport meta 是 Vite 模板原值 `width=device-width, initial-scale=1`，未禁缩放。两道防线：①代理注入 HTML 时把 viewport meta 改写为 `initial-scale=1, maximum-scale=1, user-scalable=no`（needle 即上游模板原值，收 upstream.rs::VIEWPORT_META_NEEDLE/REPLACEMENT 契约常量；上游改值则契约探针翻红）；②mobile.css ≤700px 下 composer textarea 字号抬到 16px，从触发条件上消灭缩放（改写 miss 时仍生效）。Playwright 实测改写后 meta 生效、390px 视口 composer 字号 16px、1200px 桌面宽度规则不生效

## [0.5.4] - 2026-09-04

### Fixed

- **远程端每次进入/新建会话都弹内测声明**（0.5.1 起 dsh 0.1.2 实踩，手机远程实拍反馈）：dsh 0.1.2 把插件客户端 bundle 从单插件 `/plugins/<id>/client.js?rev=N` 改为**合并加载** `/plugins/??<a>/client.js,<b>/client.js,...&rev=N`（path 部分只剩 `/plugins/`，组合清单整体在 query 里，单条 3.7MB），代理的改写判定 `ends_with("/client.js")` 静默失配——内测声明的持久化三元式没被改写成 `"host"`，远程端确认记录不落 settings.yaml、每次连接都弹。现 matcher 双形态都认；顺带修掉 `send_forwarded` 就地重算改写判定的潜伏 bug（拿带 scheme 的完整 URL 判定恒 false → accept-encoding 从未被剥过，真 dsh 一旦压缩响应改写路径即整体失效）。真机 combo bundle 改写产物已过 `node --check` 语法验证
- **改写产物对已缓存手机端不生效**：bundle 响应被 dsh 标为 `cache-control: immutable, max-age=1y`，而 rev 跨 dsh 重启稳定（内容哈希）——修好改写后，手机端一年内仍会命中缓存里的未改写副本。现未带 `dshv=<壳版本>` 的 bundle 请求由代理 302 到带参同 URL 强制重取（重定向 no-store）；带参请求转发前由代理剥掉该参数——真 dsh 对组合 URL 的 query 逐字校验、多余参数直接 404（真机实测），buster 只活在代理与浏览器之间。改写缓冲上限 4MB→16MB（combo 实测 3.7MB，留增长余量）。回归测试：combo 改写 + 302 击穿 + 转发剥参 + 非 bundle 资源不重定向（tests/remote_proxy.rs，假 dsh 增设 `/plugins/` 合并形态路由与命中记录）
- **手机端输入框聚焦→退出后，对话/轨迹/项目/信息标签栏消失、页面难以滑动**（微信内置浏览器实拍反馈）：iOS WKWebView 键盘收起后页面停在无法用手势复位的平移残留上——头部（会话标题+标签栏）停在视口外，观感如"进入全屏"。dsh 自身无键盘视口处理（bundle 仅 react-dom 引用 visualViewport；桌面 Chromium 与模拟键盘均不可复现，纯 iOS WebKit 行为）。现 mobile.js 增加视口复位：输入框失焦与 visualViewport resize 时把文档滚动复位到 0 并强制重排（聚焦中的合法平移不干预；健康态文档滚动恒 0，复位为无操作）。Playwright 已验证复位路径生效

## [0.5.3] - 2026-09-04

### Changed

- 检查更新链路指向新建的**公开发布仓库** [deepseek-harness-desktop-releases](https://github.com/LBurny/deepseek-harness-desktop-releases)（源码仓库保持私有，发布渠道分离）——其它设置页的"GitHub 下载"、"手动更新"与启动时自动检查全部改读该仓库的 releases。由此修掉持续已久的已知限制：源码仓库转私有后匿名检查更新必 404（0.4.x~0.5.2）；公开仓库匿名可读，检查更新恢复正常。release-local.ps1 发版目标同步切换到发布仓库；update.rs 新增锚定测试防指回私有仓

## [0.5.2] - 2026-09-04

### Fixed

- **运行中重启 dsh 后主窗口落 401 页、通知/远程凭据 401 死循环**（0.5.1 实踩）：launch token 按 dsh 进程轮换，而壳侧 token 缓存只写不清——重启后 `wait_token` 读到上一进程的旧 token 秒过、Ready 抢跑，主窗口带旧 token 导航被 BrowserAuth 拒（"dsh web authentication required" 页），mux/远程代理拿 {新端口, 旧 token} 换 cookie 无限 401。现 supervise 循环每次 spawn 前清空 token 缓存与广播端，Ready 只认当前进程打出的新 token；回归测试用"第二进程换 token + 就绪行延迟 3s"拉开竞态窗口钉死（tests/process.rs::restart_replaces_stale_token，fixture 新增 fake-dsh.token / fake-dsh.token-delay 文件开关）
- **launch token 明文落 events.log**（0.5.1 实踩）：reqwest 网络错误的 Display 自带 `for url (…/?token=…)` 尾巴，cookie 交换失败日志把完整带 token URL 打了出来。现 `exchange_cookie` 用 `without_url()` 剥掉 URL，mux 失败日志再过一道 `redact_token` 纵深防御；新增单元测试钉死错误串不含 token/URL

## [0.5.1] - 2026-09-04

### Changed

- dsh 运行时 0.1.1-rc.2 → 0.1.2-rc.1（rewrite 级跟版）。上游要点：Web UI 引入 BrowserAuth 鉴权（每进程 launch token 换会话 cookie，无关闭开关、回环也在门内）；事件传输重写（`/api/events.mux`+`/api/events.host` 双 WS → 单 `/api/remote.mux` + `$events` 事件桥 + 逐会话 `session/follow`）；agent 预设独立成包 `@deepseek-ai/dsh-agent-presets`；修复 0.1.1-rc.2 升级后启动失败与会话标题丢失（alpha.5，直接覆盖本机用户的升级路径）；RPC 信封改 typert wire 名传参
- 壳侧三条链路随版改造：①`dsh_session.rs` 新模块统一凭证（stdout 就绪行捕获 launch token，Ready 门控保证导航必带 `?token=`，日志全链路脱敏）；②通知传输换 `notify/mux.rs` MuxSource（单 WS 承载 $events + N 条 follow，`api-session/added` 驱动逐会话跟随，子代理过滤与任务/回答完成拆分语义平移；严禁回包 `$events/result` 防抢先结算审批）；③远程代理 cookie 代持（launch token 现换 dsh-auth cookie、401 失效清缓存重换并重放一次、WS 桥接注入 cookie——漏注入手机端表现为页面开但全断）
- 契约套件随 0.1.2 重写（BrowserAuth 门/303 交换/RPC 信封/`$events` ready 帧/follow 错误帧字段集逐项探真）；测试用假 dsh 服务器（tests/support，axum 进程内）替代进程外 fake-dsh.cjs（后者仅剩进程监督与 RemoteManager 套件在用）
- mobile.js 适配 0.1.2 输入框 contenteditable 化：回形针附件按钮的注入目标从 textarea 扩为 contenteditable（旧判断下按钮静默消失，Playwright 390px 实测发现）
- `/init` 命令结果行文案改为 "Prompt submitted to prepare for generating the AGENTS.md file"（原 "Submitted the AGENTS.md init prompt as a collapsed context injection."——用户反馈原句偏实现细节）。dsh-command-init 插件 0.1.1 → 0.1.2，已安装实例下次启动经 preseed 文件同步自动更新

## [0.5.0] - 2026-09-03

### Changed

- 内置 `/init` 命令注入的提示词不再以完整用户消息气泡显示：消息源从 `kind:"user"` 改为 `kind:"plugin"` + `form:"notice"`（一行摘要），UI 渲染成一行可折叠的「上下文注入 · dsh-command-init · 摘要」行（点击展开全文），模型仍收到完整提示词。与官方 dsh-plan-mode /goal 的注入方式一致；用户在插件面板删除行为不受影响。dsh-command-init 插件 0.1.0 → 0.1.1，已安装实例经 preseed 文件同步在下次启动自动更新

### Added

- 上游契约套件新增三条探针守护上述折叠行为：会话 UI 的 `source.kind !== "user"` 折叠分支、`contextInjection` locale 键、`noticeSummary` 摘要读取（upstream.rs 新增 CONTEXT_INJECTION_BRANCH_NEEDLE / CONTEXT_INJECTION_TITLE_NEEDLE / NOTICE_SUMMARY_NEEDLE 常量，上游改版即红）；preseed.rs 加锚定测试钉死随包插件 index.js 不走 `kind:"user"`

## [0.4.9] - 2026-09-02

### Added

- 预安装插件机制 + 内置 `/init` 斜杠命令：随安装包分发 `resources/preseed-plugins/dsh-command-init`（bundle 形态，自带 `dsh.bundle.patch` 自我挂载），首启时播种到 `$DSH_HOME/profiles/plugins/` 并走官方 `dsh plugin add` 挂进 web profile 层列表（preseed.rs）；`/init` 把"创建/更新工作区 AGENTS.md"的提示语按当前工作区路径渲染后以用户消息提交执行。用户可在插件管理面板正常删除（dsh plugin remove 自动摘层），marker 文件 `.plugins-preseeded` 保证删除后不被重新播种；壳升级时插件文件有变化则覆盖同步

## [0.4.8] - 2026-08-28

### Fixed

- 手机远程界面：未配置 API key 的机器上模型选择器显示 "✦ Default ⌄" 裸文本药丸，把输入区底栏的 trailing 组挤换行（模型药丸与发送键掉到第二行，实机截图）。根因：0.4.6 的图标化只隐藏了模型名段（`_triggerLabel`），触发器还有第二段"推理等级"文案（`_triggerEffort`）——无 key 时该段显示 providerDefault 文案 "Default"，有 key 时显示 "High" 等等级名，有 key 机器若当前模型无推理等级元数据则恰好整段不渲染，解释了"同版本两台机器表现不同"。≤700px 下两段文案一并隐藏，药丸回归 45px 纯图标单行布局（无 key 复刻环境实测：临时空 DSH_HOME + 摘除 DEEPSEEK_API_KEY 环境变量起 dsh，Playwright 390px 注入验证）
- 上游契约套件新增三条探针：模型槽位钩子 `conversation.input.model`、触发器两段文案的 CSS Modules 本地名 `triggerLabel`/`triggerEffort`（upstream.rs 新增 MODEL_SLOT_HOOK / MODEL_TRIGGER_LABEL_NEEDLE / MODEL_TRIGGER_EFFORT_NEEDLE 常量，上游改名即红）；另加锚定测试 `model_trigger_iconified_rule` 钉死 mobile.css 两段隐藏规则都在 700px 断点内

## [0.4.7] - 2026-08-28

### Fixed

- 手机远程界面：消息页脚的统计串（时间 · 用时 · 首 token · tok/s，单行 nowrap 约 367px）在窄屏溢出窗口右缘被裁；允许换行的备选方案实测第二行会压进输入卡片被遮挡。按"信息"子页即统计归处的定位，≤700px 下页脚统计整体隐藏，只留操作图标（原生单行 28px 布局不动）；统计信息仍可从「信息」标签页查看

## [0.4.6] - 2026-08-28

### Fixed

- 手机远程界面：模型选择药丸退回全名显示、窄屏溢出截断（"glm-5.3-flash …"）——上游 dsh 删除了图标化规则锚定的 `_triggerEffort` 段，按 mobile.css 的锚定哲学规则静默失配、页面回到未适配态。图标化改锚语义钩子 `data-slot="conversation.input.model"`（隐藏文案，mask+currentColor 火花图标跟随主题文字色）
- 手机远程界面：模型选择菜单左缘越出屏幕被裁（模型列表显示 "-V4-Flash"、"DeepSeek" 前缀丢失）。菜单以 45px 药丸为包含块 `right:0` 绝对定位；≤700px 下药丸根改静态定位使包含块上移到输入卡片，显式 `bottom: calc(100% + 8px)` 让菜单从卡片上方弹出（上游的 `bottom:36px` 锚相对卡片会叠进输入行），`max-width: calc(100vw - 24px)` + 内部滚动兜底超长模型名
- 手机远程界面：产物块的「在文件夹中显示」按钮在 ≤700px 下隐藏——它唤起的是 PC 端资源管理器，手机远程端看不见也点不到，还白占一行；「产物」标签与文件药丸保留

## [0.4.5] - 2026-08-28

### Added

- Shell toasts now carry the app icon in the header row (the small icon left of the app name), provided by the AUMID registry key's IconUri pointing at the shipped 256px `icons/128x128@2x.png` — a toast icon slot accepts a single file with no per-DPI variant set, so the hi-res source keeps it crisp on 200%+ display scaling. The toast XML itself carries **no** `<image>` element: `appLogoOverride` renders a second, large icon in the body area next to the header icon and squeezes the text layout (observed on machine B as "icon appears twice") — the body stays text-only. An anchoring test (`bundled_icon_is_shipped_hi_dpi`) pins both the resources mapping and the source image's real pixels — a missing mapping silently ships an installer without the icon file, which dev builds mask because their resource_dir points at the source tree (the initial 0.4.5 package shipped exactly this way: fine on the dev machine, no icon on any installed machine)
- Clicking a toast opened the main window but buried it under the current foreground app (observed on machine B: "dsh UI stays below the browser"). The root cause turned out to be one level up from the foreground mechanics: the single-instance callback handled protocol activation with an inline `show()+set_focus()` trio and **never called `tray::show_main`** — so every raise reinforcement landed on a code path the toast click never took (an earlier dev-machine "verification" was a false positive: showing a *hidden* window places it visibly on top even without any raise). The callback now goes through `tray::show_main` → platform `bring_to_front`: a background thread runs two timed attempts (250 ms / 550 ms, straddling the Shell's habit of returning the foreground to the previously active app after the toast dismiss animation), each raising topmost, attaching its input queue to the foreground thread (AttachThreadInput) to borrow foreground rights, then BringWindowToTop/SetForegroundWindow/SetActiveWindow, detaching and dropping back to non-topmost. Additionally, a protocol-launched second instance now broadcasts its Shell-granted foreground right via `AllowSetForegroundWindow(ASFW_ANY)` before the single-instance plugin discards it (the same hand-off Chromium/VS Code use for single-instance activation), making the running instance's SetForegroundWindow a legal call rather than a smuggled one. `bring_to_front` logs every step to events.log (per-attempt foreground pid, attach/SetForegroundWindow results, and a settled-state recheck 2 s later), so any remaining failure says exactly which step and when
- Clicking a shell toast notification now returns you to the main dsh window (same effect as left-clicking the tray icon), so a background/remote user can jump back into the UI straight from the notification. All shell toasts go through a new WinRT-direct toast module (`notify/toast.rs`): sound and AUMID mapping replicate tauri-plugin-notification's Windows backend exactly (silent toast for the 17 built-in wavs which the shell plays itself via its own waveOut player, system-default toast audio for `default`, app identifier as AUMID for installed builds with the PowerShell AUMID fallback in dev), so appearance and sound are unchanged — only the click behavior is new. Click activation uses a **protocol activation** toast (`activationType="protocol" launch="dshdesktop://open"`): the OS opens the registered URL protocol on click, the second instance is intercepted by the single-instance plugin, and the running instance shows+focuses the main window (the same handler as double-launch). In-process `ToastNotification.Activated` handlers were tested and abandoned: for unpackaged Win32 apps on Win10 they silently never fire (verified with and without an AUMID registry key on a real machine) — protocol activation is the reliable route and additionally launches the app when it isn't running. Setup registers the `dshdesktop://` URL protocol and the AUMID display-name key under HKCU on installed builds (idempotent, self-healing across install paths; skipped for dev builds so they don't hijack the installed app's registration); a protocol-activated toast logs `toast activated (protocol) -> show main` to events.log. An `examples/toast_click.rs` manual harness remains for verifying the raw mechanism

### Fixed

- 诊断面板的「打开日志」在 events.log 尚未生成时改为创建空文件再打开，不再报「日志文件尚未生成」——按钮永远可用
- Sound played silently on some machines — the fourth recurrence of the "notification/preview has no sound" family (0.3.x: `SND_NOSTOP` giving up while busy; 0.4.2: `SND_ASYNC` filename-buffer dangle; 0.4.5 interim: `SND_SYNC` — machine B then flipped to "first notification audible, every later one silent" while events.log kept showing `ok` with full playback durations). `PlaySoundW` is a black box: it accepts the request, reports success, and its hidden winmm state machine (worker thread, process-wide cached device handle) fails internally with no error surfaced anywhere. The playback engine is replaced wholesale: `play_sound_file` now drives **waveOut directly** on a dedicated thread (dispatch returns instantly; preview/notify paths never block). Every play opens a **fresh device handle** via `WAVE_MAPPER` (picking the current default endpoint) and closes it after playback — the winmm cache that went stale on machine B is bypassed entirely. Each step (open/prepare/write/unprepare) returns a real `MMSYSERR` code that lands in events.log on failure, and the log line now includes which device the sound was played to (`device=…`) plus actual elapsed ms. Interrupt semantics are preserved (a new play waveOutResets the previous one); a failed open is retried once after 500 ms to self-heal a cold device; a missing file still returns Err synchronously so callers keep their toast-default fallback. The 0.4.2 path table is gone, and new anchoring tests pin the "dispatch returns immediately" and WAV-parsing contracts

## [0.4.4] - 2026-08-28

### Changed

- Diagnostics panel layout now matches the plugins panel: the header is split left/right with the status badge staying beside the title and the action buttons moved to the right — 重启服务 becomes the primary (solid) button in the 更新全部 position, and a new 打开日志 ghost button sits to its left in the 重启 dsh position. 打开日志 opens `%LOCALAPPDATA%\DSHDesktop\events.log` with the system default program (new `open_log_file` command via the shell's rundll32 path — no console flash; registered in the ACL manifest/capabilities but not exposed to the remote source), so a full log can be copied for reporting without digging through Explorer. The log area itself is card-ified like the 已安装 list: the 诊断日志 heading lives inside the card and the log fills the remaining height in a bordered, rounded box

## [0.4.3] - 2026-08-28

### Added

- Diagnostics panel (托盘 → 诊断) now shows the unified log stream: shell-side diagnostics (notify dispatch/suppression, sound playback with path+elapsed, theme/tray/remote lifecycle) and dsh process output used to live only in `%LOCALAPPDATA%\DSHDesktop\events.log` — the panel backfill now reads the tail of that file directly (last 500 lines, crossing session restarts), so "which notification had no sound" is visible in-app instead of requiring a manual log export. Live lines keep streaming through the `dsh-log` event (notify/play/preview lines included). The in-memory LogRing is removed — events.log is the single persistent store (1MB self-truncating), the section is renamed 服务日志 → 诊断日志

### Fixed

- MCP servers configured as `npx ...` crashed on machines whose system node is old (observed on a second machine: global node v16.14.2 resolved `npx`, engine-incompatible packages died with "Class extends value undefined"). The bundled runtime shipped only `node.exe` — fetch-runtime.ps1 now also copies npm/npx from the node distribution, and the dsh child PATH prepends the runtime's node directory ahead of the profile `.bin` (both npx/npm/node then bind to the bundled node 24, independent of what the machine has on PATH); a guard test pins the runtime's npx presence
- Sound played silently on the first attempt (preview or notification): `play_sound_file` kept the PlaySoundW filename buffer in a local variable and dropped it right after the call returned, but `SND_ASYNC` means winmm's internal playback thread opens the file by name slightly LATER — when it loses that race (cold thread, slower machine), the sound silently never plays while the toast still shows. Observed on a second machine as "first preview click silent, subsequent clicks audible". The filename buffer now lives in a process-lifetime table (candidates are a fixed ~19-entry enum, so the table stays under 4KB), removing the race entirely
- Sound-link diagnostics: every sound play attempt (preview clicks and real notifications) now logs a timestamped line to events.log with the resolved wav path, PlaySound result and elapsed ms, so "which click had no sound" can be reconciled against the log. A failed PlaySound additionally falls back to the toast's system-default sound instead of leaving the toast silent (machines with broken winmm, e.g. Windows N editions)

### Added

- CI: the bundled-runtime cache (`rt-<dsh version>-<script hash>`) never actually hit across releases — Actions caches are ref-scoped, so a cache saved by a tag-triggered run is invisible to the next tag run (observed on 0.4.0→0.4.1: same key still missed, the ~20-minute runtime fetch re-ran and the release pipeline took ~70 min). The cache steps already existed in build.yml, but its trigger was manual-only. build.yml now also runs on main pushes (docs-only changes ignored), warming the cache into the default-branch scope that tag runs can read; from now on, a release whose dsh version is unchanged hits the cache, and the first release after a dsh bump should push main first, then tag

## [0.4.1] - 2026-08-27

### Added

- Fourth notification rule "回答完成" (Reply completed): turn completions now split by whether the turn did real work — a turn with tool calls (`tool/call` between `turn/start` and `turn/end`) reports 任务完成 under the existing `notify.turn_done` rule (toast body 「title」任务完成), while a pure text-only reply reports 回答完成 under the new `notify.answer_done` rule (toast body 「title」回答完成). Each rule has its own enabled/timing, so e.g. per-reply reminders can be set to "always" while work-task completions stay background-only. Existing settings files without the `answer_done` key fall back to the default (on + background) via serde default; the `turn_done` key name is unchanged so user tweaks survive

### Fixed

- Notifications were often soundless or missing entirely. Soundless: approval/question toasts were deliberately silent by design, yet those are exactly the moments dsh sits blocked waiting for the user — all four notification kinds now play the configured sound (the sound row's disable condition is now "all four rules off" instead of tied to 任务完成). Missing toasts had three stacked causes, all addressed: (1) suppressed notifications and `builder.show()` failures were swallowed silently — both now leave a line in events.log (`Notify suppressed: … foreground=…` / `toast show failed: …`); (2) a half-open loopback WebSocket left `stream.next()` hanging forever with no reconnect — WsSource now sends an idle Ping after 60s of silence and force-reconnects if nothing (including Pong) arrives within 30s; (3) the foreground gating itself is unchanged (a focused shell window still suppresses background-timed notifications — set the rule's timing to 总是提醒 to override)
- WS source previously treated any inbound non-text frame (server Ping, Pong) as a dead connection and needlessly reconnected; such frames now just reset the idle watchdog

## [0.4.0] - 2026-08-27

### Added

- Completion sounds overhaul: `completion_sound` now offers 17 built-in sounds (`bip-bop-01..10` / `staplebops-01..07`, sourced from opencode, shipped as `resources/sounds/*.wav`) alongside `silent` / `default`; the default is now `staplebops-02`. Legacy named sounds (`im`/`mail`/`reminder`/`sms`/`chime`/`drop`/`mellow`, ≤0.1.x) migrate through serde aliases — the first four map to `default`, the last three to `staplebops-02` — so a load() of an old settings file keeps the user's remaining settings intact. `default` still passes the toast's system audio preset; the 17 custom sounds play via PlaySoundW while the toast itself stays silent
- `scripts/follow-upstream.ps1`: one-command dsh version-follow tooling — pin bump, stale-runtime cleanup (the old `dsh/` dir is removed first so floating sub-packages are not held at the previous rc by its package-lock), runtime re-fetch, three-carrier app version bump (package.json is now synced too; it had drifted to 0.1.12), the contract suite as gate, doc baseline sync with per-file occurrence guards (upstream.rs header / design §15 / both READMEs) and a CHANGELOG skeleton entry. Every step is idempotent: when the contract suite goes red, fix upstream.rs and re-run the same command to resume. `-SelfTest` runs 12 zero-network pure-function assertions

### Changed

- Settings page dropdowns: the completion-sound picker and the three notify-timing dropdowns now share a new `PopupSelect` component. The popup is width-matched to the trigger, height-capped (~7 items visible) and scrollable — the native `<select>` popup renders all options in one OS-drawn sheet whose corners/highlight cannot be styled. The trigger mirrors the native select look (border, input background, 32px height, thin chevron), hover/selected use a native-popup-style light highlight (theme-aware `--sel-bg`/`--sel-fg` tokens) with square corners, and the popup itself carries the page's rounded corners and a visible scrollbar; the current selection is scrolled into view on open. Outside-click/Escape close, disabled state follows the corresponding notify rule toggle
- PlaySoundW no longer passes `SND_NOSTOP`: that flag means "give up if the previous sound is still playing" (no queueing, no mixing), so rapid previews or back-to-back notifications were silently dropped. The default interrupt-previous behavior is the intended one
- dsh runtime pinned at 0.1.1-rc.2 (no drift in probed facts); test suite 198 → 200 (custom-sound resource presence probes)

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
