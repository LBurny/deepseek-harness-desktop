# DSHDesktop 设计文档

> 面向后续开发者。读完本文应能回答：每个模块为什么存在、改动某处会影响谁、新增平台/功能该从哪里下手。
> 英文版：[design.md](design.md)。

## 1. 项目定位

DSHDesktop 是 [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（dsh，DeepSeek 的 agent harness CLI，npm 包 `@deepseek-ai/dsh`）的 **Windows 桌面壳应用**。

dsh 本身提供 `dsh web` 命令：在本机 127.0.0.1 上启动一个 Web UI 服务。但它要求用户先装 Node.js 22+、再全局装 dsh、再开终端敲命令——对普通用户门槛太高。DSHDesktop 把这一切打包成双击即用的桌面应用：

- 安装包内嵌 Node.js 便携运行时与 dsh（含全部生产依赖），**用户机器上零前置依赖**（仅需 Windows 10/11 x64，WebView2 缺失时安装程序自动联网安装）；
- 启动后自动拉起 `dsh web`，就绪即在应用窗口中打开官方 Web UI；
- 托盘常驻、关窗最小化到托盘、崩溃自动重启、事件转原生通知、标题栏主题跟随 dsh 设置。

**非目标**：不重写 dsh 的 UI（壳内直接加载官方 Web UI，dsh 升级即 UI 升级）；不修改 dsh 源码（只通过进程参数、环境变量、其暴露的 HTTP/WS 接口交互）；不做安装器之外的系统级集成（不写注册表自启以外的系统项）。

## 2. 关键设计决策

| 决策 | 理由 | 代价 |
| --- | --- | --- |
| 壳内嵌官方 Web UI，不自绘界面 | dsh 处于开发者预览期，UI 迭代快；壳跟随 npm 包版本即可 | 主窗口内容依赖 dsh 进程健康；需要进程监督 |
| 内嵌 Node + dsh 随安装包分发 | 零前置依赖，装完即用 | 安装包 45MB、安装后约 242MB |
| 运行时**原地运行**（安装目录可写时） | 省掉约 230MB 部署副本；NSIS 默认按用户安装即可写 | 需处理只读安装目录回退与旧副本清理 |
| 事件通知走 WebSocket `/api/events.mux` + `/api/events.host` | dsh 官方事件下行通道 | 上游接口不稳定，需适配层隔离 |
| 主题用 2s 轮询 settings.yaml | 文件极小、改动极少；比 inotify 简单且跨平台无差异 | 主题切换最多 2s 延迟 |
| 平台差异全部收口 `Platform` trait | 为 macOS/Linux 预留；`compile_error!` 强制新平台显式实现 | trait 方法需精心设计（见 §10） |

## 3. 总体架构

```
┌────────────────────────── 主窗口 (label: "main") ──────────────────────────┐
│  本地启动画面 (Svelte)  ──dsh 就绪──▶  navigate 到 http://127.0.0.1:<port> │
│  （dsh 官方 Web UI，壳不干预其内部）                                        │
└──────────────────────────────────┬───────────────────────────────────────┘
                                   │ Tauri IPC（invoke 命令 / emit 事件）
┌──────────────────────────────────┴───────────────────────────────────────┐
│ Rust 核心（src-tauri/src/）                                               │
│  lib.rs      组装：插件 → setup（代码创建主窗口）→ 事件桥（dsh 状态 → 前端事件/窗口导航）│
│  download.rs 主窗口下载处理：on_download 落系统下载目录 + 去重 + toast  │
│  presets.rs  启动期改写 shipped minimal 预设为 win32 pwsh 变体（签名门控）│
│  runtime.rs  运行时定位：原地运行 / 只读回退部署 / 旧副本清理               │
│  process.rs  DshProcess 监督循环：spawn、就绪探测、指数退避重启             │
│  locks.rs    陈旧锁自愈：spawn 前清 DSH_HOME 里持有者已退出的 *.lock        │
│  notify/     WS 订阅 dsh 事件(mux+host 双下行) → 分类/台账 → 原生通知      │
│  theme.rs    轮询 dsh 主题设置 → DWM 标题栏着色                            │
│  progress.rs 首启进度模型：阶段权重、百分比映射、结构化事件负载             │
│  tray.rs     托盘菜单；diagnostics.rs 状态/日志；commands.rs 基础命令     │
│  zoom.rs     UI 缩放：可配置步进（默认 2%）、快捷键钩子注入、持久化        │
│  settings.rs 壳设置：settings.json 模型/校验/持久化 + get/set/试听命令      │
│  skills.rs   技能管理：skills/ ↔ skills-disabled/ 移动开关 + 三源/ZIP 导入   │
│  mcp.rs      MCP 管理：cordis.patch.yml 条目读写/启停 + 三源导入           │
│  picker.rs   目录选择器钉 browse：幂等写 cordis.patch.yml（禁 auto 行       │
│              + insert browse 对），手机远程端才能选文件夹                  │
│  pickerpatch.rs browse 选择器包内文件原地补丁：host 加 "dsh:drives" 盘符    │
│              哨兵层级，client 隐藏条目默认显示+哨兵面包屑本地化/禁用打开     │
│  welcome.rs  内测声明豁免播种：预写 ui-onboarding.welcomeNoticeVersion      │
│  remote/     远程访问：token 门岗反向代理 + cloudflared 隧道监督            │
│  platform/   Platform trait（windows.rs 实现；macos/linux 为编译期占位）   │
└──────────────────────────────────┬───────────────────────────────────────┘
                                   │ spawn（CREATE_NO_WINDOW，DSH_HOME 隔离）
┌──────────────────────────────────┴───────────────────────────────────────┐
│ 内嵌运行时：<install>/runtime/windows-x64/                                 │
│   node.exe + dsh/node_modules/@deepseek-ai/dsh/lib/bin.js                 │
│ 运行方式：node bin.js web --port <空闲端口>（仅绑 127.0.0.1）              │
└──────────────────────────────────────────────────────────────────────────┘
```

数据目录：`%LOCALAPPDATA%\DSHDesktop\`

```
DSHDesktop\
  dsh-home\          dsh 的 DSH_HOME（settings.yaml、sessions、用户数据都在这里）
  events.log         进程事件调试日志（1MB 截断，诊断的最后手段）
  ui-zoom.txt        UI 缩放比例持久化（缺失/损坏回退 100%）
  settings.json      壳设置（缩放步进/快捷键/关窗行为；缺失/损坏回退默认）
  runtime\           仅"只读安装目录"回退模式下存在（部署副本，带 .version 标记）
```

## 4. 启动时序

1. **插件初始化**：`single_instance` 必须最先注册——第二次启动时聚焦已有主窗口，不重复拉起。`window-state` 记忆窗口几何：缩放/移动实时入内存缓存，退出（`RunEvent::Exit`）时落盘 `%APPDATA%/<identifier>/.window-state.json`，下次启动建窗时恢复；flags 只取 `SIZE | POSITION | MAXIMIZED`——不含 `VISIBLE`，否则托盘隐藏态下退出会把"隐藏"记住，下次启动主窗口不出来。注意 restore 在插件 `window_created`（建窗后经 `run_on_main_thread` 排队）里执行，**晚于首批可见帧**（实测默认尺寸会可见 ~370ms）——所以主窗口以 `visible(false)` 创建、`on_page_load(Finished)` 时再 `show()`（此刻 restore 早已完成，首个可见帧即记忆几何）；回归见 `scripts/verify-no-size-flash.ps1`。注意主窗口自引入下载处理后改为 **setup 里代码创建**（`WebviewWindowBuilder`，tauri.conf `windows` 为空）：`on_download` 只能挂在 builder 上，conf 声明的窗口无法附加；`visible(false)+center()` 及 restore 时序不变（代码创建窗口同样在 `window_created` 排队 restore——托盘按需窗口一直如此）。
2. **setup**：代码创建主窗口（挂 `on_download`，见 §9）→ 建托盘 → 注册 `BootstrapInfo`（启动错误兜底）→ 取 `resource_dir` 定位内嵌运行时。
3. **首启判定**：`dsh-home` 在 ensure_runtime 之前不存在即首启（前端据此决定显示进度条还是纯文字）。
4. **ensure_runtime**（§5）：失败不退出，错误写入 `BootstrapInfo` 并 emit `dsh-progress`（stage=error），窗口停在启动画面显示错误（前端会主动 `get_bootstrap_error` 查询，因为错误事件可能早于前端 listen 注册而丢失）。
5. **seed_theme_preference → spawn_theme_follower**：首启时 settings.yaml 不存在则按系统深浅色预写 `ui-theme.preference`（dsh 缺省渲染浅色，不播种会出现"深标题栏 + 浅 UI"）；然后立即按系统主题给标题栏着色一次，进入 2s 轮询。
6. **spawn_supervised**：经 `tauri::async_runtime::block_on` 调用（setup 里没有 tokio 上下文，内部 `tokio::spawn` 依赖它）。状态机：`Starting → Ready{port}`。
7. **事件桥 bridge_event**（lib.rs）：所有 `dsh-progress` 事件都是结构化负载 `{stage, message, percent}`（progress.rs），百分比由后端按阶段权重计算：
   - `runtime`：原地运行 → 0→15%；回退部署 → 按复制字节实时报 0→70%（节流：百分比变化才发）
   - `starting`：15% 或 70%（取决于是否部署过）
   - `ready` → 100% → 写入 port 的 watch 通道（通知 WS 订阅器换端口）→ emit `dsh-ready` → **主窗口 navigate 到 `http://127.0.0.1:<port>/`**
   - `error` → 主窗口 navigate 回本地启动画面
   - 每个事件同时追加到 `events.log`。
8. dsh 就绪后，WS 订阅器连上 `/api/events.mux` 与 `/api/events.host`，进入稳态。

## 5. 运行时管理

**背景**：安装包把运行时原样放在 `<install>\runtime\windows-x64\`（tauri.conf `resources` 映射：`"runtime" -> "runtime"`、`"resources/sounds" -> "sounds"`；列表形式会把 `resources/sounds` 原样放到 `<install>/resources/sounds/`，而壳在 exe 旁找 `sounds/*.wav`（`resolve_custom_sound`），探测不到自定义提示音就静默降级系统默认——0.1.16 实踩，`settings.rs` 有锚定测试）。早期设计是首次启动时把整个运行时复制到 `%LOCALAPPDATA%`（防只读安装目录），代价是安装后体积翻倍（约 +230MB）。

**现状：两级策略**

```
ensure_runtime(source_dir, app_version):
  1. strip_verbatim(source_dir)     # 剥 \\?\ 前缀（见下方坑）
  2. validate_source                # node.exe 与 dsh bin.js 必须存在，否则报 Incomplete
  3. home = %LOCALAPPDATA%\DSHDesktop\dsh-home（不存在则创建）
  4. 安装目录可写？（create_new 探测文件）
     ├─ 可写 → 清理旧版部署副本（%LOCALAPPDATA%\...\runtime 且带 .version 标记才删）
     │        → 原地运行：node_exe/bin.js 直接指向安装目录
     └─ 只读 → 部署到 %LOCALAPPDATA%\...\runtime：
              .version == app_version 则跳过，否则全量重复制
```

- **工作目录与运行时解耦**：dsh 的 cwd 永远是 `%LOCALAPPDATA%\DSHDesktop`（可写），即使原地运行模式也不污染安装目录。
- **旧副本清理只认 `.version` 标记**：那是我们自己写下的文件，避免误删用户数据。

**坑（已修复，勿回退）**：Tauri 的 `app.path().resource_dir()` 在 Windows 上返回带 `\\?\` 扩展前缀的路径（如 `\\?\F:\DSHDesktop\...`）。Node 的模块加载器不认这种路径——它把 `\\?\F:` 的首段当成盘符相对路径，入口解析直接 `EISDIR: illegal operation on a directory, lstat 'F:'` 崩溃。`strip_verbatim` 只剥"剩余部分是盘符绝对路径"的形态（`\\?\UNC\...` 保留），与 `dunce::simplified` 的保守策略一致。**任何给 Node 的路径都必须经过 ensure_runtime，不要在别处自己拼。**

## 6. 进程监督

`DshProcess::spawn_supervised` 拉起一个 tokio 监督循环，状态机：

```
Starting ──wait_ready 60s 内拿到 HTTP 响应──wait_token 静默预算内拿到 token──▶ Ready{port}
   │                                           │ child.wait() 返回（崩溃/退出）
   │ 超时/连续失败 5 次                          ▼
   ▼                                     指数退避 500ms×2（封顶 30s）→ 回到 Starting
Failed（不再自动重启，前端/托盘可手动 restart）
```

- **spawn 参数**：`node bin.js web --port <port>`，`env DSH_HOME=<home>`，PATH 前置内嵌 node 目录 + `<home>/profiles/web/node_modules/.bin`（node 目录在前：npx/npm/node 绑定运行时自带版本，dsh 派生的 MCP 命令常以 `npx` 配置，运行时不带 npx.cmd 会落到系统 PATH 任意 node 版本、引擎不兼容即崩——机器 B 全局 node v16 实测；.bin 在后：插件自带 CLI 不在用户 PATH 上，前置后会话终端/工具子进程才能按名解析——否则装完 modlens 这类插件在 dsh 终端敲不到它的命令），`cwd=%LOCALAPPDATA%\DSHDesktop`，stdout/stderr 管道泵入 `LogRing` + 事件流，`kill_on_drop(true)`，再经 `Platform::configure_child_command` 加 `CREATE_NO_WINDOW`（不弹控制台窗口）。
- **端口**：`free_port()` 让 OS 分配空闲端口——返回到使用之间存在竞态窗口，靠"就绪超时即杀、换端口重试"兜底；`wait_ready` 轮询 `http://127.0.0.1:<port>/` 直到拿到**任意** HTTP 响应（不要求 200）。
- **wait_token 是静默超时而非固定时长（0.5.6 起）**：等 token 就绪行期间，pump 每收到一行 stdout/stderr 都刷新活动时间，持续静默才计时（普通预算 60s）；见到 npm 冷装警告行（`npm warn exec … will be installed`，npx 解析未缓存包、MCP 条目联网安装的标志）切 install 长预算 10min（npm fetch 可能数分钟无输出），absolute 10min 封顶防"一直打印但永不就绪"挂死。旧固定 60s 在冷装场景把快装完的进程杀树、白等一轮再靠 npm 缓存余温重启（0.5.5 实踩：上游 MCP 包发版后的首次启动必现）。Ready 前落耗时分解行 `[dshdesktop] ready: port=N total=Xs http=Ys token=Zs`——诊断面板"上次启动"行的数据源（格式锚定 diagnostics::parse_boot_timing 与 tests/process.rs）。
- **stop/restart**：两个 `tokio::sync::Notify`。stop 置 shutdown 标志并通知，循环杀掉进程树（`taskkill /T /F`，dsh 可能派生 python 等子孙）后进入 `Stopped`；restart 在循环存活时通知其立即重来，循环已退出（Failed/Stopped）时重新 spawn 一个监督循环。
- **陈旧锁自愈（0.5.13 起，`locks.rs`）**：每次 spawn 前扫 DSH_HOME 里持有者已退出的 `*.lock` 并删除。dsh 的跨进程写锁是目标文件的兄弟 `<file>.lock`（`wx` 独占创建、内容 `${pid}\n`、只在 finally 里删，上游明确不做孤儿恢复），而 Windows 上壳只能用 `taskkill /F` 硬杀——恰好持锁时被杀就把锁永久留在盘上，之后每次启动都在 boot 阶段等锁超时（`.credentials.yaml` 的凭证写入预算 30s）、插件树加载失败、进程退出，壳只看到"就绪行没出现"，重试多少次都一样（机器 B：应用再也起不来）。判定保守：pid 可解析且进程已退出（或被无关进程复用 pid）才删，内容不是 pid 的要够老（5min）才删，持有者活着且镜像是 node 的一律保留；扫描限深 3 层、跳过 node_modules、不进符号链接/junction。每条判定进 events.log，误删可追溯。上游一旦自己做孤儿恢复，`tests/upstream_contract.rs` 的锁探针翻红提醒撤掉。
- **Job Object 防孤儿**：spawn 成功后立即 `Platform::register_child(pid)` 把子进程挂进全局 `KILL_ON_JOB_CLOSE` Job（`platform/windows.rs` 的 `job` 模块，句柄刻意永不关闭）。本进程以任何方式退出——包括被 NSIS 安装器/任务管理器强杀——内核都在最后句柄回收时连带终止全部成员及其子孙。0.1.8 之前没有这层保护：安装器只杀主程序，孤儿 node.exe/cloudflared.exe 锁住 runtime 目录导致重装中止（"Can't write: ...\cloudflared.exe"）。cloudflared 监督循环（remote/tunnel.rs）同样注册。
- **tokio 陷阱**：`Child::kill()` 返回 future，不 await 就不执行；泄漏的子进程若继承了 stdout 管道，外层等管道 EOF 会永远阻塞（集成测试曾因此假挂起）。所有子路径都必须 `kill_on_drop` + 显式 `child.wait().await` + 测试里 stdio 全 null。

## 7. 事件通知

```
dsh WS /api/events.mux ──▶ WsSource(mux) ──▶ handle_mux_frame ──┐
dsh WS /api/events.host ─▶ WsSource(host) ─▶ handle_host_frame ─▶ SessionBook
（两端点共用 WsSource：断线 5s 重连、端口经 watch 跟随重启换端口；   │（子代理集合
 host 重连先 clear_subagents——基线不可知，fail-open；              │ + 会话标题
 空闲 Ping + Pong 超时看门狗：回环半开连接假死时强制重连，          │ + 当前回合
 否则 stream.next() 永久挂起=从此再无通知）                         │   是否干过活）
                                                                ▼
                                              NotifySink：窗口聚焦态 + 壳设置四类规则 → toast
```

- dsh 的事件帧：`{"type":"server-request","method":<payload.type>,"payload":{...}}`，mux/host 两端点同构（仅 WS；GET 返回 426）。
- mux 流放行：
  - `approval/requested` / `question/requested`（待批准/待回答，regex 粗筛）→ **Approval** / **Question** 通知（分别按 `notify.approval` / `notify.question` 规则门控）。
  - `session/event` 且 `event.type=="turn/end"` 且 `data.reason.kind=="completed"` → 按"回合内是否干过活"（SessionBook 的 worked 痕迹：`turn/start` 清零、`tool/call` 置位）拆成 **TaskCompleted**（干过活，正文「标题」任务完成）与 **AnswerCompleted**（纯文字回答，正文「标题」回答完成），分别按 `notify.turn_done` / `notify.answer_done` 规则门控；aborted/error/blocked/max-tokens 一律忽略（任务出错暂不提醒）。
  - `session/event` 且 `event.type=="session/title"` → 记入 SessionBook，完成通知正文带「会话标题」（无标题回退"dsh 任务完成"/"dsh 回答完成"）。
- **两段式过滤**：先字符串 contains 粗筛、命中才 JSON 解析——流式期间每个 token chunk 都是一帧 `session/event`，不能逢帧解析。
- **子代理过滤**：mux 帧不含 origin；host 流的 `host/session-added`（`origin=="subagent"`）/ `host/session-removed` 维护子代理集合，命中的 turn/end 直接丢弃。子代理必然创建于 WS 连接之后（先创建再跑回合），时序天然安全；host 流不推基线，重连后集合清空（宁多弹一条，不漏弹）。
- dsh 的浏览器信任栅栏允许 loopback + 无 Origin 的请求，Rust 客户端天然满足。
- **适配层是有意为之**：`NotifySource` trait 隔离上游不稳定的接口，将来可加 `FileWatchSource`（解析 session jsonl）等替代实现。
- sink 弹通知前按类型查 `NotifyRule::allows(foreground)` 门控：**前台 = 本应用任一窗口（main/settings/diagnostics/skills/mcp/remote）处于聚焦态**（主窗口可见但失焦 = 用户已切走，算后台）；正在前台操作时不打扰，后台运行才弹（timing=always 的类型除外）。**被抑制的也写一行 `Notify suppressed: {kind} foreground={bool}` 到 events.log**（否则"没提醒"无从排查）；弹出的写 `Notify: {kind} {body}`；`builder.show()` 失败（WinRT 被系统策略拦截等）写 `toast show failed: {e}`——三个都是通知链路的现场诊断抓手。
- **通知提醒设置**（settings.json）：`notify.{approval,question,turn_done,answer_done}` 四条规则，各为 `{enabled, timing}`——timing ∈ `background`（默认，仅无聚焦窗口时提醒）/ `always`（前台也提醒），四类默认均开。旧版 `notify_on_completion` 布尔在 load 时迁移进 `notify.turn_done.enabled`（读后即弃，保存时不再写出）；0.4.x 及以前的配置文件没有 `answer_done` 键，serde default 补齐为默认开+仅后台。`completion_sound`（`silent`/`default` + 17 个壳内置音效 `bip-bop-01..10`/`staplebops-01..07`（音源 opencode，wav 落 resources/sounds/），默认 `staplebops-02`）作用于**全部四类**（dsh 卡住等用户输入时静音提醒等于没提醒，自本版起 Approval/Question 也带声）。`default` 透传 toast 音频预设（`ms-winsoundevent:Notification.Default`，系统内置、不受用户声音方案影响），其余 17 音由壳在专用线程上以自管 waveOut 播放内置 wav（toast 静音；每次播放 waveOutOpen 新开设备句柄、播完即关，失败 500ms 重试一次，打开/写入每步有真实 MMSYSERR 错误码，日志带实际播放设备名，结果经 SoundDiag 回调落 events.log——PlaySoundW 四轮翻车：SND_NOSTOP 忙时放弃、SND_ASYNC 缓冲悬垂、异步工作线程首播静默、SND_SYNC 缓存设备句柄失效致首响后续无声，均已弃用）。旧具名音（im/mail/reminder/sms/chime/drop/mellow，≤0.1.x）经 serde alias 迁移：前四→`default`，后三→`staplebops-02`——load() 对解析失败整份回退默认，alias 保住老用户其余设置。试听走 `preview_completion_sound` 命令，弹一条带所选音效的 toast（音效是 toast 的属性，只能连通知一起听）。所有壳 toast 由 notify/toast.rs 用 WinRT 直连弹出（tauri-plugin-notification 桌面后端在 Windows 无点击回调能力），点击 toast 走**协议激活**（`activationType="protocol" launch="dshdesktop://open"`）回到主界面：系统拉起已注册的 URL Protocol → 二次实例被 single-instance 插件拦截、参数递给运行中实例 → show+聚焦主窗口，应用未运行时则直接拉起应用。in-process Activated 回调在 Win10 未打包应用上实测不可靠（补注册表键也不触发），勿改回。启动时 ensure_activation_registered 幂等写 HKCU 的 dshdesktop:// 协议与 AUMID 显示名（仅安装形态；dev 跳过防覆盖已安装版指向），激活事件落 events.log。toast 图标只保留顶部行小图标（应用名左侧），由 HKCU AppUserModelId 键的 IconUri 指向随包 256px PNG（icons/128x128@2x.png，经 bundle.resources 映射；图标位单文件无法按 DPI 分套，256px 源系统自缩放，高缩放屏不糊）；toast XML 不放 `<image>`——appLogoOverride 会在正文区再渲染一个大图标，与顶部行叠出双图标且挤压文字排版（0.4.5 机器 B 实测）。漏 resources 映射则安装包不含图标文件、IconUri 静默失效，dev 下 resource_dir 指源码树恰好有文件会掩盖此坑（初版实踩），锚定测试 bundled_icon_is_shipped_hi_dpi 钉映射与源图像素，toast_xml 测试钉无 image 元素。激活后的"到顶"曾有更上游的根因：single-instance 回调内联 show+set_focus 三件套、根本没调 show_main，所有置顶加固都落在协议激活走不到的路径上（dev 验证假阳性：隐藏窗口 show 后自然出现在可见位置，无真实遮挡竞争）。现回调统一走 show_main → platform `bring_to_front`：后台线程两段择时（250ms/550ms，压住 Shell 在 toast 关闭动画后把前台归还点击前应用的动作）→ 每段 TOPMOST → AttachThreadInput 挂前台线程借权限 → BringWindowToTop/SetForegroundWindow/SetActiveWindow → detach → 落回 NOTOPMOST，不常驻置顶；另协议激活的二次实例在 main 开头（single-instance 拦截退出前）`AllowSetForegroundWindow(ASFW_ANY)` 把 Shell 授予的前台权限广播给主实例（Chromium/VSCode 单实例激活同款），两路权限并行。bring_to_front 每步落 events.log（每轮前台 pid、attach/sfg 返回值、2s 后最终归属），失败可直接读日志定位；attach 对 UWP 前台线程（操作中心）会被拒，属通知中心点条目的特例，真实 banner 点击时前台是桌面进程不受影响。

## 8. 主题与语言跟随

目标：dsh 设置里的 `ui-theme.preference`（light/dark/system，存于 `$DSH_HOME/settings.yaml`）变化时，应用所有窗口的**标题栏**跟着变；本地页面（splash/诊断/设置/技能/MCP）与托盘菜单也同步；界面语言跟随 dsh 的 `locale.preference`（zh/en，缺省按系统 UI 语言）。

- **2s 轮询**配置文件（文件极小，轮询比 inotify 简单且跨平台无差异）；`system` 经注册表 `AppsUseLightTheme` 解析。语言同理读 `locale.preference`，缺省按 `GetUserDefaultUILanguage` 主语言 ID 是否中文（dsh 侧缺省"跟随浏览器"，WebView2 的浏览器语言同样来自系统）。
- **首启播种（seed_theme_preference）**：dsh 在 settings.yaml 缺失/无 preference 时**缺省渲染浅色 UI**，而壳标题栏缺省跟随系统——系统为深色时首启出现"深标题栏 + 浅内容"。ensure_runtime 之后、spawn_supervised 之前，若 settings.yaml 不存在则按系统深浅色预写 `ui-theme.preference`（dark/light，无 BOM），dsh 首启即与壳一致。已存在的文件绝不动。
- **本地页面**：页面加载时 `invoke('get_shell_ui_state')` 取快照、订阅 `shell-ui-state` 事件（2s 轮询解析值变化才广播）。`ui.svelte.ts` 把快照写入 `<html data-theme="dark|light">` + `color-scheme`；`app.css` 以 CSS 变量承载全部颜色（`:root` 暗色默认 + `html[data-theme='light']` 浅色覆盖），五个 Svelte 页面只引用变量。JS 未跑的 splash 首帧按 `@media (prefers-color-scheme: light)` 兜底渲染（与首启播种的"系统色即主题"一致）。
- **托盘菜单**：Windows 托盘右键菜单不响应 DWM 属性，用 uxtheme 未文档化 API `SetPreferredAppMode(ForceDark=2 / ForceLight=3)` + `FlushMenuThemes()`（tao 同源做法；uxtheme 常驻进程，LoadLibraryA 无需 FreeLibrary），在 `apply` 时随解析主题刷新。
- **BOM 陷阱（已修复，勿回退）**：PowerShell 5.1 的 `Set-Content -Encoding utf8` 会写入 UTF-8 BOM，而 yaml-rust 不接受 BOM——解析失败会**静默回退 system 主题**，表象是"标题栏永远白色"。`read_preference` 先剥 BOM 再解析。教训：**不要用 PowerShell 改写 settings.yaml**。
- **Windows 双管齐下**：
  1. `window.set_theme()` 同步 tao 内部主题状态——不同步的话，tao 可能在窗口事件后用缓存的旧状态覆盖可视效果。隐藏窗口上调用可能报错甚至 panic，必须 `catch_unwind` 兜住；
  2. 直接对 HWND 调 `DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE=20)`——无缓存、幂等、对隐藏窗口同样生效，是标题栏颜色的权威来源。attr 20 失败（E_INVALIDARG）时回退旧值 19（Win10 20H1 之前）。
- **非客户区必须强制重绘（已修复，勿回退）**：`DwmSetWindowAttribute` 只改属性、不重绘标题栏——窗口会保持旧色直到下一次激活（用户"点一下才变色"）；tao `set_theme` 内部伪造 `WM_NCACTIVATE` 触发重绘，但该法在部分时序/焦点状态下不生效（winit/Electron 均因此弃用）。主题实际变化时（`LAST_APPLIED` 原子门控，轮询同值不重复强制，防每 2s 闪一次）对每个窗口 `SetWindowPos(SWP_FRAMECHANGED|SWP_NOACTIVATE|NOMOVE|NOSIZE|NOZORDER)` + `RedrawWindow(RDW_FRAME|RDW_INVALIDATE|RDW_UPDATENOW)` 强制非客户区重绘（Chromium/Windows Terminal 同款），并在 events.log 留痕。另外新建窗口的 DWM 属性来自系统主题（tao 建窗行为），"系统浅色 + dsh 深色"组合下会带错色出生；`on_page_load` 在 show 前经 `apply_before_show` 把属性落对（已可见的主窗口整页导航则同时强制重绘）。回归：`scripts/verify-titlebar-theme.ps1`（不点击窗口断言标题栏像素即时换色）。
- **语言**：`theme.rs` 的轮询把解析后的 locale 写入 `i18n.rs` 全局原子（`i18n::pick(zh, en)` 取当前语言，托盘菜单/窗口标题/进度/通知/命令错误文案全部经它取）；locale 变化时重建托盘菜单（`tray::apply_locale`，Windows 托盘菜单不能改文案只能重建）并刷新本地窗口标题；前端 `i18n.ts` 以中文原文为 key 的 en 字典 + 响应式 `t()`（未命中 fallback 中文，避免裸 key）。`ShellUiState` 启动时立即写入全局语言、关注循环首轮 force 同步——否则 locale=en 时托盘菜单要等设置变化才重建。
- **已知限制**：Win10 上深色标题栏**聚焦时纯黑、失焦时深灰**是系统行为；`DWMWA_CAPTION_COLOR`(35)/`DWMWA_TEXT_COLOR`(36) 仅 Win11 可用。要做到恒为 dsh 的深灰（#1B1B1C），需要无边框窗口 + `initialization_script` 注入自绘标题栏——暂缓。

## 9. 前端与窗口管理

壳的本地页面只有四个，用 **hash 路由**（`App.svelte` 监听 `hashchange`）：

- `#/`（默认）**Splash.svelte**：启动画面。onMount 先 `invoke('get_bootstrap_error')` 主动查引导错误、`invoke('is_first_launch')` 查首启标记，再 listen 结构化的 `dsh-progress`。**首启时**显示分阶段进度条（百分比数字 + 阶段清单 ✓/●/○）与"首次启动需要部署运行时，可能要花几分钟"提示（仅此分支渲染，后续启动不出现）：runtime/starting 阶段百分比由后端给下限，`starting` 期间前端向 95% 渐近缓动（dsh 无细分进度信号，缓动只是呈现层，永不触顶），`ready` 到 100%。**非首启**维持纯文字 + 不确定滚动条。dsh 就绪后由 **Rust 侧**把主窗口 navigate 到 dsh UI——前端不自己跳。
- `#/diagnostics` **Diagnostics.svelte**：诊断面板（状态/端口/PID/版本、诊断日志——回填 `%LOCALAPPDATA%\DSHDesktop\events.log` 尾部 500 行，壳侧诊断与 dsh 进程输出同流、跨会话持久（文件 1MB 自截断）+ `dsh-log` 事件实时流（含通知/声音/试听行）、重启按钮、开机自启开关）。
- `#/settings` **Settings.svelte**：其它设置（开机自启、关窗行为单选、四类通知提醒——任务确认/选项选择/任务完成/回答完成，各带启用勾选 + 仅后台时/总是时机下拉，提示音选择与试听独立成行、四类全关才禁用、缩放步进 1%–25%、放大/缩小快捷键录制器）。保存时前端先校验（至少一个修饰键、in/out 不冲突），再 `invoke('set_shell_settings', { next })` 由 Rust 端复验并落盘。
- `#/skills` **Skills.svelte**：技能管理。数据源是**壳注入给 dsh 的 DSH_HOME**（`<runtime_base>/dsh-home`，不是 `~/.dsh`）：`skills/` 为启用、旁路 `skills-disabled/` 为停用（dsh 的技能发现只认根目录直属条目、无原生禁用概念；移出根目录即停用，watcher 观察到变化后热刷新 catalog，无需重启）。导入从三个外部 agent 的用户级源复制目录：Codex `~/.codex/skills`、Claude Code `~/.claude/skills`、OpenCode `~/.config/opencode/skills`；同名冲突逐个选覆盖/跳过（覆盖会同时清掉禁用目录里的旧副本）。**独立 dsh 的默认目录 `~/.dsh/skills` 不作为导入源**——壳就是 dsh，启动时自动扫描它并补入新技能（`skills::seed_from_default_dsh_home`；`.skills-seeded` marker 记录已见名字，壳里删掉的不会复活）。删除只删 home 内副本，不动源目录。还可本地导入 ZIP 压缩包（`inspect_zip_skills`/`import_zip_skills`）：自动识别两种布局——包根直接含 SKILL.md（名字取 frontmatter name，缺失回退 zip 文件名）或顶层若干技能文件夹各含 SKILL.md；解包剥掉顶层前缀，条目路径经 enclosed_name 过滤防 zip-slip，另有 1 万条目/256MB 上限防 zip 炸弹；冲突语义与目录导入一致（跳过/覆盖，覆盖清两侧）。Rust 侧 `skills.rs` 的 frontmatter 解析只取单行键，行上操作均以目录名为准。
- `#/plugins` **Plugins.svelte**：插件管理（npm/cordis 插件的图形化装/卸/更新，详见 §18）。
- `#/mcp` **Mcp.svelte**：MCP server 管理（列表/启停/删除/新增/编辑 + 导入）。dsh 没有独立的 mcp.json——MCP server 是 Cordis 插件补丁，壳读写 `<dsh-home>/profiles/web/cordis.patch.yml` 中 `name == '@deepseek-ai/dsh-mcp-client'` 的 insert 条目（只动这些条目，其余 Value 级保留；tmp+rename 原子写；读前剥 BOM）。dsh 的 HMR（`watchUserPatches` + chokidar）监听该文件，改后自动 disconnect+reconnect，**无需重启**。启停 = entry 上加/去 `disabled: true`（cordis-plugin-loader 原生语义，disabled 的 entry 不起 fiber）。编辑以旧 config 为底、只覆盖表单字段，`toolCallTimeoutMs`/`reconnect.*` 等高级键保留；transport 只有 `stdio`（command/args/env/cwd）与 `streamable-http`（url/headers）两种，sse 不支持。启动时种子同步 `~/.dsh` 两层 patch 里的 MCP 条目（`mcp::seed_from_default_dsh_home`，`.mcp-seeded` marker 防复活；源里 disabled 的不同步也不记 marker，日后在 ~/.dsh 启用时仍能进来）。手动导入三源：Claude Code `~/.claude.json` 的 `mcpServers`（stdio/http 映射，sse 标记"不支持"跳过）、Codex `~/.codex/config.toml` 的 `[mcp_servers.*]`（`enabled=false` 不列出）、OpenCode `~/.config/opencode/opencode.json` 的 `mcp` 段（local/remote 映射）；冲突逐个覆盖/跳过。patch 文件解析失败（如含无法处理的语法）时页面降级为只读并提示手工编辑。
- `#/remote` **Remote.svelte**：远程访问。状态取 `get_remote_status` 快照并订阅 `remote-status` 事件；Up 态显示二维码（`get_remote_qr` 返回 SVG）与完整链接，链接变化（隧道重连换域名）自动重取二维码；开关按钮按当前 phase 调 `start_remote` / `stop_remote`。

窗口行为：

- 主窗口 `main`：**关窗行为可配置**（settings.json 的 `close_behavior`）：默认 `background` = 隐藏到托盘（`CloseRequested` 时 `prevent_close` + `hide`），`quit` = 走托盘"退出"同一流程直接退出程序；托盘"打开主界面"、**左键单击托盘图标**（`on_tray_icon_event` 的 Left/Up；`show_menu_on_left_click(false)`，菜单改走右键）或二次启动（单实例插件）时 `show` + `unminimize` + `set_focus`——窗口只是隐藏未销毁，位置保持隐藏前状态。
- 诊断窗口 `diagnostics`、设置窗口 `settings`、技能窗口 `skills`、插件窗口 `plugins`、MCP 窗口 `mcp`、远程访问窗口 `remote`：托盘菜单按需创建，**关窗 = 销毁**，下次再建。**创建即按壳当前解析主题铺底（tray.rs `theme_bootstrap`）**：`background_color` 给 WebView2 预绘制底色（深 `#0f1115`/浅 `#f5f6f8`，与 app.css 两主题的 `--bg` 一致），`initialization_script` 在首个绘制帧前写死 `data-theme` 与 `color-scheme`——否则 `visible(false)` + `on_page_load` 才显示也救不了"系统浅色 + dsh 深色"组合：app.css 首帧按系统色兜底（`@media prefers-color-scheme: light` 的浅色变量分支），页面 JS 置 `data-theme` 之前整页白几秒（插件管理页实踩）。主窗口不挂这个初始化脚本（它是远程 dsh UI，主题归 dsh 自己管）。
- 托盘"退出"：先 `stop()` 远程访问（杀 cloudflared 进程树 + 关停鉴权代理，链接即刻失效），再 `stop()` dsh，等 1.5s 让监督循环杀完进程树，最后 `exit(0)`。
- 导航到远程 URL 后窗口标题被 dsh 的 `document.title` 覆盖——**外部脚本不要按标题找窗口**（按 PID + 类名，见 `scripts/shot-window.ps1`）。
- **首次启动居中**：主窗口（setup 里 builder `.center()`，tauri.conf `windows` 已空）与托盘按需创建的五个窗口都以屏幕居中为默认位置；window-state 插件的 restore 在 window_created 时排队、早于首个可见帧执行，有记忆几何时覆盖居中默认值——首次启动居中、之后按上次位置，居中默认不会闪一帧再跳变（verify-no-size-flash.ps1 探针断言首个可见帧即记忆几何）。
- **下载处理（download.rs）**：WebView2 的下载不接管则静默消失——wry 默认 handler 放行但 `SetHandled(true)` 抑制了下载 UI，用户看不到文件去向（dsh "Session log" 导出即受此影响）。主窗口 builder 挂 `on_download`：Requested 时把目标改到系统下载目录（`dirs::download_dir`，已存在则追加 " (n)" 序号防覆盖），Finished 时按成败弹 toast 告知落盘路径；全程记 events.log（`Download: requested/finished`）。

IPC 命令：commands.rs 9 个——`get_shell_ui_state` / `get_status` / `restart_dsh` / `get_recent_logs` / `get_last_boot_timing` / `get_autostart` / `set_autostart` / `get_bootstrap_error` / `is_first_launch`；另有 zoom.rs 的 `zoom_ui`、settings.rs 的 `get_shell_settings` / `set_shell_settings` / `preview_completion_sound`、skills.rs 的 `list_skills` / `list_import_sources` / `import_skills` / `set_skill_enabled` / `delete_skill` / `inspect_zip_skills` / `import_zip_skills`、mcp.rs 的 `list_mcp_servers` / `upsert_mcp_server` / `set_mcp_enabled` / `delete_mcp_server` / `list_mcp_import_sources` / `import_mcp_servers`、plugins.rs 的 `get_plugin_status` / `list_plugins` / `search_plugins` / `install_plugin` / `uninstall_plugin` / `update_plugins`、remote/mod.rs 的 `start_remote` / `stop_remote` / `get_remote_status` / `copy_remote_link` / `get_remote_qr` / `reset_remote_link`、update.rs 的 `check_update` / `download_update` / `install_update` / `open_update_page`（共 42 个，见 build.rs AppManifest；tests/command_registration.rs 锚定 build.rs / capabilities / invoke_handler 三处一致）。

壳设置（settings.rs）：

- **模型**：`settings.json` 存 `zoom_step`（0.01–0.25，越界 clamp）、`zoom_in`/`zoom_out` 快捷键（`{ctrl, shift, alt, code, key}`）、`close_behavior`（`background`/`quit`）、`notify`（`{approval, question, turn_done, answer_done}` 四条 `{enabled, timing}` 规则，默认全开、仅后台时提醒（0.4.x 配置无 answer_done 键，serde default 补齐）；旧版 `notify_on_completion` 布尔读取时迁移进 `notify.turn_done.enabled`，保存时不再写出）、`completion_sound`（`silent`/`default` + 17 个内置音效 `bip-bop-01..10`/`staplebops-01..07`，默认 `staplebops-02`；旧具名音 im/mail/reminder/sms→`default`、chime/drop/mellow→`staplebops-02` 经 serde alias 迁移）。缺失/损坏 → 全默认；部分字段缺失 → 逐字段回退默认（serde default）；校验失败（无修饰键/in-out 冲突）→ 全默认，不带坏状态跑。
- **SettingsState**：托管内存值 + 持久化目录；`set` 先 clamp/校验再落盘再替换内存，校验或落盘失败则内存磁盘都保持旧值（落盘失败显式报"设置写入失败： …"——静默吞掉会让用户看到保存成功/无关报错而重启后回退，无法定位环境阻断）。
- **set_autostart 的幂等防御**：每次保存设置都会调 `set_autostart`，而 auto-launch 0.5 的 `disable()` 无条件 `RegDeleteValueW`——Run 值不存在时返回 `ERROR_FILE_NOT_FOUND`，从未开过自启动的用户每次保存都弹"系统找不到指定的文件。 (os error 2)"。命令先 `is_enabled()` 比对目标态，已达成即 Ok（顺带避免每次保存重写注册表）；commands.rs 有锚定测试钉住上游行为，上游改幂等后可简化。
- **保存即生效**：`set_shell_settings` 成功后对主窗口重注入缩放钩子（快捷键定义内嵌在脚本里必须重注入）；步进不写死在脚本里，`zoom_ui` 调用时从设置读，改步进本来就无需重注入。

UI 缩放（zoom.rs）：

- **快捷键**：默认 `Ctrl+Shift+=` 放大、`Ctrl+Shift+-` 缩小（可在设置窗口自定义），步进默认 ±2 个百分点（可配 1%–25%，clamp 到 25%–500%）。钩子脚本由 `hook_js(&ShellSettings)` 生成——快捷键定义内嵌为 JSON，匹配逻辑与 `Shortcut::matches` 对齐：`e.code` 物理键位为主，`e.key` 兜底（合成按键与 RDP 注入的 keydown `e.code` 为空，纯 code 匹配会整组失效），meta 永不命中。`on_page_load` 在每次整页加载完成后 eval 注入（**只注入 main 窗口**——设置窗口录制快捷键时不能被钩子抢先拦截；本地 splash 与远程 dsh UI 通用），capture 阶段拦截并 invoke `zoom_ui`（负载 `direction: "in"/"out"`），经 WebView2 原生 `SetZoomFactor` 生效——与浏览器 Ctrl++ 同一机制。监听器可热替换（`__dshZoomHookHandler` 存旧 handler，重注入先 `removeEventListener` 再挂新的，不叠加）。
- **持久化**：每次变更即写 `%LOCALAPPDATA%\DSHDesktop\ui-zoom.txt`；缺失/损坏回退 100%；每次页面加载时 `on_page_load` 统一重应用当前缩放（兼作 WebView2 重建后的兜底）。
- **远程 IPC**：dsh UI 是远程源，Tauri 对远程源的 IPC 一律走 ACL（无 app manifest 时远程调用全部拒绝）。因此 build.rs 用 `AppManifest::commands` 声明全部 42 个命令（生成 `permissions/autogenerated/allow-*.toml`），`capabilities/dsh-remote.json` 只对 `http://127.0.0.1:*` 开放 `allow-zoom-ui` 一个命令。**副作用**：本地页面的 app 命令也转为 ACL 管控，default.json 已逐个 allow——**新增命令必须同步三处**：build.rs 的 commands 列表、capabilities/default.json（本地）、按需 dsh-remote.json（远程）；tests/command_registration.rs 锚定三处一致。

## 10. 平台抽象

```rust
pub trait Platform: Send + Sync {
    fn node_exe_name(&self) -> &'static str;            // "node.exe" / "node"
    fn runtime_base_dir(&self) -> PathBuf;              // 应用数据根目录
    fn resource_runtime_dir(&self, resource_dir: &Path) -> PathBuf; // <res>/runtime/<triplet>
    fn runtime_triplet(&self) -> &'static str;          // "windows-x64" / "darwin-arm64" ...
    fn kill_process_tree(&self, pid: u32);              // taskkill /T /F（同样带 CREATE_NO_WINDOW，否则退出/重启闪 cmd 窗口）
    fn configure_child_command(&self, _cmd: &mut Command) {} // CREATE_NO_WINDOW 等
    fn system_dark_mode(&self) -> bool;                 // 注册表 AppsUseLightTheme
}
```

`mod.rs` 对 macOS/Linux 是 `compile_error!` 占位——新增平台时编译器会强制你实现 trait 并接线 `current()`。配套还要做：`scripts/fetch-runtime.ps1` 支持对应 triplet 的 Node 下载、tauri.conf `bundle.targets` 加 dmg/appimage、CI matrix 打开对应行（`.github/workflows/build.yml` 注释里有清单）。主题着色在 `theme.rs` 里按 `cfg(windows)` 分支，其他平台走 `set_theme` 即可。

## 11. 打包与分发

### 运行时管线

```
scripts/fetch-runtime.ps1
  1. 下载 Node v24.19.0 win-x64 zip，只取 node.exe
  2. npm install --prefix dsh --omit=dev @deepseek-ai/dsh@0.1.5-rc.2
  3. 冒烟：node bin.js --help
  4. 调 scripts/prune-runtime.ps1 精简
产物：src-tauri/runtime/windows-x64/（gitignore，不入库）
```

`prune-runtime.ps1` 的规则（支持 `-WhatIf` 预演）：

- 通用：删 `test/tests/__tests__/docs/example/examples/coverage/.github` 等目录；删 `*.d.ts/*.map/*.md/LICENSE*/CHANGELOG*` 等文件；
- node-pty：只保留 `prebuilds/win32-x64`（删 darwin-*/win32-arm64 约 30MB），另删 src/deps/third_party 等；
- `@img/sharp-wasm32`（9MB）：sharp 已有 win32-x64 原生包时用不到，删。

效果：staging 344→227.9MB；安装包 **45.2MB**；安装后 **241.8MB**（大头是 node.exe ~90MB 与 dsh 依赖树，运行时与 node-pty 终端必需的 native 模块不能再删）。

### 安装包

- `pnpm tauri build` → NSIS `src-tauri/target/release/bundle/nsis/DSHDesktop_<ver>_x64-setup.exe`。
- WebView2：缺失时安装程序联网下载安装（downloadBootstrapper 模式），因此安装包本体不含 WebView2。
- 静默安装：`setup.exe /S`（加 `/D=<dir>` 指定目录）。运行中的旧实例由安装器自动结束，无需手动卸载或杀进程。
- NSIS 钩子（`src-tauri/windows/nsis-hooks.nsh`，经 `bundle.windows.nsis.installerHooks` 接入，**路径相对 `src-tauri`**）：`NSIS_HOOK_PREINSTALL`/`NSIS_HOOK_PREUNINSTALL` 先 `taskkill /F /IM DSHDesktop.exe` 杀主程序（**绝不带 `/T`**：立即安装拉起的安装器与 `_?=` 原地运行的旧卸载器都是 DSHDesktop.exe 的后代，/T 会把它们一并杀掉——0.1.16 及以前"立即安装"装不上即此因；≥0.1.9 的子进程由 KILL_ON_JOB_CLOSE Job 随主程序死亡被内核连带回收，杀树本就多余），再用 PowerShell 按可执行路径清扫 `$INSTDIR` 下的所有残留进程（≤0.1.8 遗留的孤儿 node.exe/cloudflared.exe），然后轮询等进程退净（≤10s）等内核回收句柄，**再等三个 exe（主程序/runtime 的 node/cloudflared）文件锁释放**（0.5.10：进程死亡≠锁释放，Defender/PCA 对刚退出的进程映像持柄 1~3s，本机实测锁窗口 ~1.3s；模板的 `CheckIfAppIsRunning` 杀完主程序仅 500ms 就 Delete 主程序——快速连点+应用正在自行退出时 Delete 撞锁静默失败、退出码仍 0，模板 `FileExists` 复检弹 "Unable to uninstall!"。钩子以独占打开探针轮询（15s 封顶，超时照常继续不劣于旧行为），让模板检查时找不到活进程、Delete 落在锁释放后）。Tauri 模板自带的 `CheckIfAppIsRunning` 只杀主程序，杀不动子进程，单靠它必然复现 "Can't write" 失败。**清扫必须排除调用方自身**（取 PowerShell 父进程 PID）：覆盖安装/升级时模板在 `PageLeaveReinstall` 以 `_?=$INSTDIR` 原地运行旧卸载器，`$INSTDIR\uninstall.exe` 同样匹配 `$INSTDIR\*` 模式——0.1.9~0.1.12 没排除，卸载器把自己杀了，文件没删、退出码非零，新安装器弹 "Unable to uninstall!" 中止；独立卸载（设置/开始菜单）会自我复制到 %TEMP% 运行所以从不触发。`NSIS_HOOK_PREINSTALL` 还会 `RMDir /r "$INSTDIR\runtime"` 自清（0.5.10：/UPDATE 覆盖安装不经过旧卸载器，旧 runtime 树无人清理，防旧版独有文件跨版本混杂；非更新路径下旧卸载器已删净，重复清理是无害 no-op）。`NSIS_HOOK_POSTUNINSTALL` 再 `RMDir /r "$INSTDIR\runtime"` 兜底清单外残留（dsh 自更新新增的文件不在卸载清单里），模板的空目录 RMDir 才能收掉 `$INSTDIR`；常驻隧道与 `remote-session.json` 仅"真卸载"才清：`$UpdateMode = 1`（/UPDATE 覆盖安装）或**父进程是新安装器**（`DSHDesktop_*_x64-setup.exe`——模板 `_?=` 原地调用旧卸载器的手动升级流，0.5.9 实踩该路径 UpdateMode=0 会误杀隧道断链）都跳过清理；真卸载（设置/开始菜单，自我复制到 %TEMP%，父进程链已死或为 explorer）与 WMI 查询失败（fail-closed 保卸载卫生）照常清理。已知问题：≤0.1.12 升级 ≥0.1.13 会最后一次弹该对话框（旧卸载器无法被新安装器修复），先在系统设置里卸载再装即可；同理 0.5.9 手动双击 0.5.10 安装包选"卸载后再安装"仍由无等锁的旧卸载器执行、快速连点+应用刚退出时可能复现弹窗——选"不卸载"或退出应用半分钟后再装可避开（应用内更新走 /UPDATE 无此问题）。排障备忘：NSIS `_?=` 必须是卸载器命令行最后一个参数，其后的所有内容会被吞进 `$INSTDIR`（`_?=F:\DSHDesktop /S` → `$INSTDIR=F:\DSHDesktop /S`，Delete 全部打空、退出码仍 0）——Tauri 模板把 `_?=$4` 放最后是对的，手工/脚本复现放错顺序会得到假的"卸载失败"（0.5.10 排障实踩一轮无效复现）。
- 应用内更新的安装包启动参数（0.5.10）：`install_update` 恒传 `/UPDATE /P /R`（与 Tauri 官方 updater 插件对齐，其恒传 `/UPDATE`）。`/UPDATE` 让模板进入更新模式：`PageLeaveReinstall` 直接 `reinst_done`，**跳过 ExecWait `_?=` 旧卸载器 + FileExists 复检整段**——"Unable to uninstall!" 的两个触发条件（卸载器退出码非 0 / 主程序 exe 残留）都无从发生，旧卸载器不参与则隧道与远程会话文件也无人动。`/P` 被动模式只显进度条（裸 /UPDATE 的 GUI 模式仍会显示"已安装"页且单选钮被强制忽略，UX 误导）；`/R` 使被动/静默模式装完由 `.onInstSuccess` 自动拉起主程序，形成"点立即安装 → 进度条 → 新版自动起来"闭环。
- 国内构建机直连 GitHub 不稳时，NSIS 下载可用 ghproxy 预置 `%LOCALAPPDATA%\tauri\NSIS`（细节见 AGENTS.md）。

### CI 与发布

- `.github/workflows/build.yml`：push main 或手动触发 → windows-latest 上 fetch-runtime → `cargo test` → `tauri build` → 上传 artifact；兼作 rt 运行时缓存预热（缓存按 ref 隔离，细节见文件头注释）。
- `.github/workflows/release.yml`：**0.4.9 起降为 workflow_dispatch 手动备用**——tag 触发发布已弃用（私有仓库跑满 30~70 分钟太久）。正式发版：`scripts/release-local.ps1` 用本地 `pnpm tauri build` 产物直接建/更新 GitHub Release（exe + SHA256，格式同旧 CI，幂等）。
- 版本号三处同步：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`。

## 12. 测试策略

| 层 | 内容 | 命令 |
| --- | --- | --- |
| Rust 单元测试（164） | runtime 部署/回退/路径归一化/复制进度回调、progress 阶段权重与百分比映射、theme BOM 解析与首启播种、notify 帧分类/子代理台账/摘要、LogRing 淘汰、port 分配与就绪探测、platform 基础、zoom clamp/持久化/钩子脚本内嵌设置、settings 模型/校验/持久化/提示音枚举/通知规则门控与旧键迁移、skills frontmatter 解析/列表/启停/删除/导入冲突、mcp patch 解析/启停/删除/upsert 校验与高级键保留/种子 marker/三源解析与导入冲突、remote 隧道 URL 解析与 token 脱敏 | `cd src-tauri && cargo test` |
| 进程集成测试（2，tests/process.rs） | 用 `tests/fixtures/fake-dsh.cjs`（可脚本化崩溃的假 dsh）验证 就绪→HTTP 200→stop、崩溃→自动重启→二次 Ready | 同上 |
| 通知集成测试（2，tests/notify_ws.rs） | fixture 双 WS 端点发事件帧，验证 approval 过滤、turn/end 完成通知（含标题）、子代理过滤 | 同上 |
| 远程访问集成测试（16，tests/remote_{proxy,tunnel,manager,project}.rs） | 门岗 403/302/cookie/转发/浏览器标记头剥离（防 dsh 信任栅栏 403）/WS 桥接/503/停服释放端口、门岗中间件覆盖壳自有路由回归、fake-cloudflared URL 解析与崩溃重启、manager 全链路（缺文件 error、up→stop、start 幂等）、"项目"标签 resolve/list/file 全链路与路径逃逸 403/体积 413/下载头 | 同上 |
| 控制台窗口回归（3，tests/console_window.rs） | 正组：CREATE_NO_WINDOW 的子进程**无可见** ConsoleWindowClass 窗口；对照组：CREATE_NEW_CONSOLE 的子进程**有**（证明检测有效，屏幕上会短暂弹真实控制台窗口，属正常）；kill_process_tree 闪窗回归：无控制台父进程（CREATE_NO_WINDOW 重拉自身 + FreeConsole）里连杀 30 棵树，断言全程抓不到 taskkill 的可见控制台窗口 | 同上 |
| 端到端验收（scripts/acceptance.ps1） | 卸载旧版 → 静默安装 → 启动 → 等 dsh 就绪 → 单实例/无可见控制台/主题/截图 全项校验 | `powershell -File scripts/acceptance.ps1 -SetupExe <exe>` |

改进程/通知/主题逻辑后：`cargo test` + 重装走一遍 acceptance.ps1。

**调试手段优先级**：诊断面板（应用内） → `%LOCALAPPDATA%\DSHDesktop\events.log`（每个进程事件一行，1MB 截断；面板依赖应用内交互，卡启动时只有它能看） → `scripts/check-node.ps1` / `get-attr20.ps1` / `shot-window.ps1` 等外部脚本。

## 13. 已知限制与后续路线

- **Win10 深色标题栏聚焦纯黑**：系统行为，见 §8。路线：无边框 + 自绘标题栏（需处理 Win10 贴边分屏），暂缓。
- **通知覆盖**：approval/question + 回合正常完成拆两路（turn/end/completed，回合内有 tool/call→任务完成/turn_done，纯文字→回答完成/answer_done，全部带提示音）；任务出错（kind==error）暂不提醒。子代理过滤依赖 events.host 增量帧，host 重连窗口期内可能多弹一条（fail-open）；mux 重连后 dsh 会重推仍未决的 approval/question，极小概率同一条提醒弹两次。其余事件类型待 dsh 上游接口稳定后再扩。
- **dsh 版本固定**：随应用版本钉死（fetch-runtime 的 `-DshVersion`），dsh 升级 = 发新版应用（跟版流程见 §14）。将来可考虑应用内自选 dsh 通道。
- **UI 缩放只作用于主窗口**：诊断/设置窗口不注入钩子、不应用缩放值；快捷键与步进均可在"其它设置"中自定义。
- **fs-local 列目录遇 ACL 拒绝项即整列失败**：上游行为——列举目录时逐个子项解析，任一子项权限被拒（如 `C:\\` 根目录的 `DumpStack.log`、`C:\\Users` 下他人配置目录）整个列表报 `cannot list ...: permission denied`。Windows 上列系统盘根目录必现。壳侧不修它，缓解是让模型知道并待在自己的 workspace（极简模式的 persona 已补工作目录事实）。
- **仅 Windows x64**：平台抽象已就绪，见 §10 的扩展清单。

## 14. 更新策略（跟随 dsh 上游）

上游源仓库：[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（npm 包 `@deepseek-ai/dsh`）。壳不 fork、不打补丁、不改 dsh 源码（§1 非目标），只做跟版发版。**唯一例外**是 presets.rs 的极简模式 Windows 修复：上游 rc 的 minimal 预设无条件挂载 PTY 持久 bash，而终端检查器未实现 win32，且 composeProfile 会把 agent-presets 行的 roots 无条件重写为 shipped root、profile patch 层无法注入影子根——只能启动期原地改写 shipped 预设文件（配置组合而非代码）。该补丁签名门控（上游加了 win32 分支即自动停手）、幂等、随 dsh 自更新还原后重打；上游修复后应整体移除。dsh 的内部优化（启动速度、UI 迭代等）对壳透明，重打包即受益。

**版本钉死**：dsh 随应用版本钉死在安装包里（`fetch-runtime.ps1` 的 `-DshVersion`），用户机器上的 dsh 不会自动更新——**dsh 升级 = 我们发一版新应用**。

**跟版流程（常规升级为纯流程，零代码改动）**：

1. 关注上游 release 与 npm 版本流，对照 §15 事实清单评估是否触及接口契约。
2. `powershell -File scripts/follow-upstream.ps1 -DshVersion <新版> [-Bump patch|minor|major]`——一条命令完成：改钉版 → 清旧 dsh/（防 package-lock 锁旧 rc）→ 重抓（冒烟+prune）→ bump 三处版本号 → `cargo test`（契约套件守门）→ 文档基线同步（upstream.rs 头注释/README 双岸/§15 标题与 npm 行，每文件计数断言，不符跳过并警告）→ CHANGELOG 骨架。**契约核对表由 `tests/upstream_contract.rs` 自动执行**（真实运行时探测，无运行时自动 skip；CI 的 fetch-runtime 在 cargo test 之前，故 CI 一定真跑）：红了说明上游变了，按失败输出的指引改 `src-tauri/src/upstream.rs` 对应常量（全部上游事实的单一来源，每条注明出处与影响面），修复后**重跑同一条命令**即可续跑（每步幂等，已完成自动跳过）；必要时动对应消费模块。若上游新增依赖（尤其 native 模块、多平台 prebuilds），按需调整 `prune-runtime.ps1` 规则。
3. 填 CHANGELOG 的 TODO 摘要、review diff、commit；`pnpm tauri build` → `scripts/acceptance.ps1` 全项验收。
4. 打 tag `v*` 推送（不再触发 CI 发布），`powershell -File scripts/release-local.ps1` 用本地构建产物直接发布 GitHub Release。

**接口契约核对表（上游变了才动代码；代码化身：`src-tauri/src/upstream.rs` 常量 + `tests/upstream_contract.rs` 探测）**：

| 上游事实（当前值见 §15） | 变了要动哪里 |
| --- | --- |
| 入口 `lib/bin.js` 路径 | `upstream.rs` 的 `DSH_PKG_SEGMENTS`/`DSH_BIN_SEGMENTS`（`runtime.rs` 的 `paths_for`/`validate_source` 经助手自动跟随） |
| `web --port <N>` 命令形式 | `upstream.rs` 的 `DSH_WEB_SUBCOMMAND`/`DSH_PORT_FLAG`（`process.rs` spawn 引用） |
| 事件通道 `/api/events.mux` + `/api/events.host` 及帧格式 | `upstream.rs` 的 `EVENTS_*_PATH` 与帧常量（`notify/` 适配层引用；`NotifySource` trait 隔离来源实现） |
| `settings.yaml` 的 `ui-theme.preference` | `upstream.rs` 的 `SETTINGS_FILE`/`KEY_*`（`theme.rs` 解析与首启播种引用） |
| Node 版本要求 | `fetch-runtime.ps1` 的 `-NodeVersion` + `upstream.rs` 的 `DSH_NODE_MAJOR_FLOOR` |
| WS 信任栅栏（loopback / 无 Origin） | 契约套件直接探测（错 Origin→403）；行为变了重估 `remote/proxy.rs` 剥头策略 |
| 内测声明三元式 needle | `upstream.rs` 的 `WELCOME_NOTICE_NEEDLE`（`remote/proxy.rs` bundle 改写引用） |
| WS 信任栅栏（loopback / 无 Origin） | `notify/ws.rs` 握手 |

契约变化大多会在 `cargo test`（WS 通知集成、主题解析等单测）或 acceptance 全链路中暴露；现场问题先看 `events.log`。

**回滚**：新版 dsh 出严重问题、壳又要先发补丁时，`-DshVersion` 回退到上一可用版本重打包即可——用户数据全在 `dsh-home`，与 dsh 版本解耦。

## 15. 附录：dsh 上游事实清单（0.1.5-rc.2）

> 本表是文档形态；代码化身在 `src-tauri/src/upstream.rs`（单一事实源），
> 自动核对由 `tests/upstream_contract.rs` 执行。跟版改了 upstream.rs 就同步本表。

| 事实 | 值 |
| --- | --- |
| npm 包 | `@deepseek-ai/dsh@0.1.5-rc.2`（子包依赖为浮动区间，抓取时解析到最新 rc；npm latest 标签可能滞后于最新 rc，fetch 须显式 `-DshVersion`；dsh-web-app rc.8 起 openBrowser 默认 true） |
| Node 要求 | `^22.19 \|\| >=24`（上游仓库声明；发布 tarball 不含 engines 字段，契约套件实测确认。随包内嵌 v24.19.0） |
| 入口 | `node_modules/@deepseek-ai/dsh/lib/bin.js` |
| Web 命令 | `bin.js web --port <N> --no-open`，仅绑 127.0.0.1；`--no-open` 抑制系统浏览器弹出（dsh-web-app rc.8 起 openBrowser 默认 true） |
| 内测声明 | `dsh-client-ui-settings-models/lib/client.js` 的 welcome notice：`settings.yaml` 的 `ui-onboarding.welcomeNoticeVersion` ≠ 文案版本（如 `2026-08-13.1`，从 client.js 提取）时每次启动弹窗 → 壳 welcome.rs 启动期预写豁免；0.1.2 持久化三元式落 `dsh-client-ui-settings/lib/client.js`（接收者改 `ctx.remote.$host.isLoopback ? "host" : "memory"`，needle 须含接收者前缀否则改写出语法错误） |
| 鉴权（0.1.2 BrowserAuth） | 无关闭开关（回环也在门内）：每进程 launch token 经 stdout 就绪行 `dsh web: http://127.0.0.1:<port>/?token=<t>` 打印（**晚于 HTTP 绑定**，须持续 pump）；`GET /?token=<t>` → 303 + Set-Cookie `dsh-auth-<b64url(sha256(authority))>=v1.…`（HttpOnly/SameSite=Strict，30 天，**绑 authority——换端口即失效**）；静态资产无门、`/api/*` 与 WS 全在门内，无凭证 GET / → 401 `dsh web authentication required` → 壳 dsh_session.rs 统一凭证（token 解析 + 现换 cookie） |
| 事件通道 | 单 WS `/api/remote.mux`（0.1.2 起；旧 events.mux/events.host 已移除）。客户端帧 `{type:"open",streamId,endpoint,payload:{args}}` / `{type:"cancel"}`；服务端帧 `{type:"item"\|"end"\|"error",streamId,…}`（error 对象实测字段集 `["code","details","message"]`）；服务端 30s 心跳 Ping |
| $events 事件桥 | open 端点 `$events`（**args 必须为空**）：首条 item `{type:"ready",clientId,host}`；随后 `{type:"emit",event,args[]}`（api-session/added、api-session/removed、settings/document-updated…）与 `{type:"waterfall",event,eventId,request}`（approval/request、user-questions/request）。0.1.5 转发清单新增 `goal/activation-changed`（emit；壳对未订阅事件一律忽略，别据此加通知——目标暂停不是用户回合完成）。**严禁实现 `$events/result` 回包**——任一客户端回 result 即抢先替用户结算审批 |
| 会话跟随 | `session/follow` 端点，args 包 `{request:{address:{kind:"session",sessionId}}}`（typert wire 名 `request`，裸 address 被 gateway 拒 arguments-invalid）；下行 `{type:"snapshot"}`（历史重放）+ `{type:"event",event}`，event 形状同 0.1.1 的 session/event payload.event：完成判定 `turn/end`（`data.reason.kind`，实测 kind ∈ completed/error/aborted），回合结构 `turn/start`→(`tool/call` 每次工具调用一帧)→`turn/end`（壳据此拆任务完成/回答完成），标题 `session/title`；子代理标记用 `$events` 的 `api-session/added` `args[0].origin` |
| RPC 信封 | POST `/api/<method>`，`{type:"client-request",rpcId,method,payload:{args}}` → 恒 200 `{type:"server-response",rpcId,result:{ok,value}}`；**参数按 typert 描述符 wire 名传**（session/list 形参 `_request`、session/follow 形参 `request`，裸对象被拒 arguments-invalid） |
| 设置文件 | `$DSH_HOME/settings.yaml` → `ui-theme.preference: light\|dark\|system` |
| 信任栅栏 | Host fence（loopback/trustedHosts）+ sec-fetch-site cross-site → 403、Origin.host ≠ Host → 403（代理剥浏览器标记头的依据不变）；0.1.2 起鉴权 401 优先级在栅栏之前 |
| Agent 预设 | **0.1.2 起独立成包** `@deepseek-ai/dsh-agent-presets/presets/{minimal,…}`（node_modules 下）；rc.8 起全部自带 win32 平台分支（minimal 的 persistent-bash/persistent-pwsh 按 `process.platform` 互斥禁用，subprocess-local 新增 win32 终端检查器）→ 壳的原地改写补丁器已退役，presets.rs 仅存只读签名探测（契约套件断言 UpstreamHandled 当回归哨兵）。**0.1.5 起 minimal 只剩持久 shell**：整个 `filesystem` 组被删（`fs-local` 与 `str-replace-editor` 都不再挂），极简模式从「持久 shell + 文件编辑」降为单工具——签名哨兵仍绿（`dsh-tool-bash-persistent` + win32 门控都在），故另加一条「已无 str-replace-editor」探针守语义变更 |
| 目录选择器 browse | host `dsh-host-directory-picker-browse/lib/index.js`：`list()` 只认全限定路径、无盘符枚举入口；client `dsh-client-ui-directory-picker-browse/lib/client.js`：`showHidden` 默认 false 且开框重置、`displayCrumbs` 把 home 前缀折叠成"主页" → 壳 pickerpatch.rs 启动期原地补丁（`"dsh:drives"` 哨兵盘符层 + 默认显示隐藏 + 哨兵面包屑/禁用打开） |
| 图片附件 | 0.1.2：输入仅拖拽/剪贴板两条入口；host `dsh-attachment` 只认 png/jpeg/webp/gif（sharp 校验，3.5MB/图、20 图/条）→ 壳 mobile.js 注入附件按钮走合成 paste 复用该管线（文档类型上游不支持）。**0.1.5 起上游自带通用文件上传**（任意类型、与图片同区混排、进度/取消/切会话续显，模型按已保存路径读），手机端「回形针」按钮的存在价值需真机重估（保留则确认与原生上传共存不冲突）——见真机验收清单 |
| 预设根 | 旧版（0.1.1 线）时代 composeProfile 强制重写 roots 的行为上游已删（prep §一）；`$DSH_HOME/.agent-presets` 用户根可正常生效——如需预设补丁理论上可走 patch 影子覆盖（当前无需求，签名哨兵继续盯 win32 修复不回退） |
| WebView2 下载 | 宿主不处理 DownloadStarting 即静默取消；wry 默认放行且抑制下载 UI → 壳 download.rs 显式接管 |
| 会话格式（0.1.5） | `SESSION_FORMAT_VERSION` 0 → **3**：恢复旧会话时生成 V3 新日志、**保留原文件**，但升级后的会话不支持降级读取 → **用户数据单向**，发版后别回退 dsh 版本（壳不读写会话日志，只钉版本漂移） |
| 流式上传与新增路由（0.1.5） | `POST /api/session/uploadFileBinary`（`dsh-client-file-upload`，`requestBody:"streaming"`，dsh 侧不限体积；普通 buffered `/api` 路由上限 300MB）→ **代理必须开流式旁路**（`upstream::is_streaming_body_route` → `forward_streaming`：逐块直通、先换 cookie、不重放）。另新增**免鉴权**的 `/open-in-app/*`（`apps`/`icon`/`open`，`dsh-host-open-in-app`）——通用反代即透传，手机端「在应用中打开」入口无意义、评估隐藏 |
| 面板槽位（0.1.5） | `conversation`/`details`（单值槽）→ keyed `main`（保留 key `conversation`）+ `rightbar`，sidebar 内新增 `sidebar.panellist`，**`details` 槽删除**（原 Detail 面板移除）。影响：picker.rs 钉的 browse 表面挂在 `ui-workspace` 的 `directory-flow` 槽位，槽位重排后须真机点一次目录选择器；手机端 `_rightbarCol` 无面板打开时 0 宽（不侵入布局），打开文档/文件面板后的 ≤700px 形态待真机 |
| 出站代理与 Windows 子进程（0.1.5） | 新增 `@deepseek-ai/dsh-http-proxy`：dsh 出站请求遵循 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY/NO_PROXY`，**回环永不走代理**（显式豁免 `localhost`/`127.0.0.1`/`::1` 与 `127.0.0.0/8`、IPv4-mapped）；`dsh-subprocess-local` 的 spawn/taskkill 新增 `windowsHide: platform === "win32"`（与壳的 CREATE_NO_WINDOW 并行，互不依赖） |
| 许可证 | MIT（Copyright 2026 DeepSeek） |

## 16. 远程访问（Quick Tunnel + 内嵌鉴权代理）

托盘"远程访问"一键开启后，手机/异地浏览器凭带 token 的链接获得**完整 dsh Web UI**。零服务器、零账号、零配置：中继用 Cloudflare 免费 Quick Tunnel（`cloudflared.exe` 随 runtime 内嵌，匿名临时隧道，纯出站连接，无需公网 IP/端口映射/防火墙开口）。

**链路**：

```
手机浏览器 ─HTTPS→ Cloudflare 边缘 ─→ cloudflared(桌面，纯出站)
  → 127.0.0.1:<随机端口> remote::proxy(token 门岗)
  → 127.0.0.1:<dsh端口>  dsh web（HTTP + /api/remote.mux WS；0.1.2 起 dsh 全站 BrowserAuth，代理代持 dsh-auth cookie）
```

**模块**（`src-tauri/src/remote/`）：

- `mod.rs` — `RemoteManager`：生命周期（start/stop/status + 0.5.8 起会话持久化三方法 resume_or_start/suspend_for_exit/begin_resume）、token 生成（256-bit hex；每次 start 全新生成，**resume 与 resume 回退沿用持久化值**——只有手动"关闭远程访问"再开、或"重置链接"才换 token）、6 个 invoke 命令（start_remote/stop_remote/get_remote_status/copy_remote_link/get_remote_qr/reset_remote_link）。隧道事件回调里拼链接 `link = {url}/?token={token}`；状态变更广播 `remote-status` 事件 + 更新托盘子菜单 enabled + events.log；Up 事件把 SessionState 落状态文件（见 session.rs）。**启动复活**：`has_session()` 见状态文件 → `begin_resume()` 同步占位 Starting（与手动 start 竞态互斥）→ 异步 `resume_or_start()`：状态校验（常驻副本存在 + 按 PID 核进程映像路径防 PID 复用冒认）→ 通过则代理重绑持久化端口 + `TunnelProcess::adopt` 收养存活隧道（**链接字节级不变**，手机端收藏直接用）；校验不过回退全新开隧道（域名换、token 沿用），代理绑不上持久化端口则杀旧隧道后同样回退；回退路径状态文件重写。`suspend_for_exit`（托盘退出/覆盖更新走这条路）：代理随进程消亡、隧道留活保域名，只记日志不杀进程。`reset_link` 原地轮换 token 并掐断现有会话（链接泄露后的吊销手段），仅 Starting/Up 可用，隧道与域名不变，状态文件 token 同步改写。`stop`（手动关闭）额外杀隧道 + 删状态文件——语义即"会话作废"。`RemoteStatus.resumed` 区分 Up 来源（本次新开/复活），lib.rs 的 toast 文案随之分叉（复活="已自动恢复，链接未变"）。
- `proxy.rs` — axum 反向代理，只绑 127.0.0.1。**0.1.2 起 dsh 侧鉴权代持**：dsh 全站 BrowserAuth（launch token 换 dsh-auth cookie，无关闭开关），代理凭 creds watch（端口+token）经 `dsh_session::exchange_cookie` 现换 cookie 并缓存代持——HTTP 转发注入 Cookie 头（浏览器自带的同名头剥掉，代持值只能由代理注入）、响应 401 即清缓存重换并**重放一次**（只重放一次防环；dsh 重启换端口后旧 cookie 必失效，这条是手机端存活的命脉）、**WS 桥接 upgrade 用 http::Request 手动带 Cookie**（独立路径最易漏，漏了手机端表现为"页面开但全断"）。手机侧鉴权：有效 cookie `__dsh_remote` 直接转发；`?token=` 匹配（常数时间比较）→ 302 剥离 token + 种 HttpOnly 长效 cookie（`Max-Age=2592000`，0.5.7 起——会话 cookie 会被手机浏览器进程回收丢弃，而地址栏已被 302 剥掉 token，一丢再开页面即 403 假"失效"，同一次开启期间被迫反复回电脑扫码；长效化后手机端可收藏地址栏 URL 直接复用。吊销语义不变：重置链接轮换 token，旧 cookie 值即刻不匹配）；token 不匹配 → 固定 500ms 延迟后 403；无凭据 → 403 门页。HTTP 经 reqwest 流式转发（3xx 透传不跟随）；WS upgrade 在代理终结握手后与 dsh 另建连接逐帧双向桥接。dsh 端口走 `watch::Receiver` 动态读取，dsh 重启代理不断线。**token 存共享单元（RwLock），门岗逐请求读最新值**——重置后旧链接/旧 cookie 即刻失效；**WS 桥接同时挂 drain Notify**（`enable()` 提前挂号防 connect 窗口期漏掐），重置/停服 `notify_waiters` 掐断所有已建立连接，否则泄露场景下攻击者已开的页面仍能持续收事件流。**转发必须剥掉浏览器标记头**（`origin`/`referer`/`sec-fetch-*`）：dsh 的 /api 信任栅栏（dsh-client-connection `isTrustedApiRequest`）要求 Origin.host == Host 头且拒绝 `sec-fetch-site: cross-site`，隧道场景 Origin 是 trycloudflare 域名，不剥则页面所有 RPC 调用全 403；剥掉后请求在 dsh 眼里是无 Origin 的 loopback 客户端（WS 桥接侧 tungstenite 握手本就不带 Origin，天然满足）。**转发客户端必须 `.no_proxy()`**——用户系统代理（Clash 等）否则会把 127.0.0.1 转发劫持走。**插件 bundle 改写**：`/plugins/*/client.js` 响应被缓冲（≤4MB、仅 identity 编码）并把 `ctx.remote.$host.isLoopback ? "host" : "memory"`（0.1.2 形态，接收者前缀必须保留在 needle 里，否则替换残留 `ctx.remote.$host."host"` 语法错误）全量替换为 `"host"`——dsh 内测声明（WelcomeNoticeStore）对非回环源选 memory 持久化，隧道域名下确认记录不落 settings.yaml 导致每次连接都弹；改写后远程端与桌面端共用 host 持久化（桌面本就已确认，远程直接不弹）。改写路径转发时剥 `accept-encoding`（求 identity）与条件请求头（防 304），响应剥 `content-length`/`etag`；needle 失配（dsh 改版换写法）静默原样透传，声明照弹但不破坏页面。**HTML 文档注入移动端适配（mobile.css + mobile.js）**：文档导航请求（accept 含 text/html）且响应确为 `text/html` 时，缓冲后往 `</head>` 前注入 `mobile.css` 与 `mobile.js`（编译期 `include_str!` 内嵌，注入点带 `<!-- dshdesktop-mobile -->` 标记）——dsh Web UI 未做手机适配，实测三处破版：设置弹窗左侧 188px 固定导航列把内容区压到一字一行竖排（全屏化 + 导航改顶部横向 tab 条修复）、narrow 模式展开侧栏占 280px 固定网格轨把主区压到 110px（轨道归零 + 侧栏内容溢出成抽屉修复）、输入区 trailing 组 `flex:0 0 auto` 把 + 按钮压到与模型名重叠（模型选择器图标化：隐藏型号/推理档位文案、mask+currentColor 补火花图标，型号在点开的二级菜单里选；触发器改静态定位把弹出菜单的包含块上移到输入卡片，修 right:0 右对齐导致的左越界裁切；回合统计行 ~460px 宽单行必截断——mobile.js 随同注入（dsh 无 CSP）：在会话页“对话/轨迹”旁加“信息”标签，点开把统计行克隆进全屏面板逐行展示（MutationObserver 跟随同步；克隆而非搬家——React 对被移走的节点 removeChild 必崩），打 data-dshmobile-enhanced 后隐藏输入区下方的原行，且标记跟随 matchMedia 断点（旋屏/拉窗离开 700px 即摘除恢复原行，防宽屏下标签被 CSS 隐藏、统计无处可见）；JS 失效或节点未命中时 CSS 兜底：隐藏 | 分隔符改弹性换行两行居中，信息全保留）。**附件按钮**：上游输入只有拖拽/剪贴板两条图片入口（`onPaste → intakeImages → createDraftImages → base64 → session.prompt`），手机浏览器一条都没有——mobile.js 另在输入卡片工具行"+"旁注入回形针按钮（类名克隆 "+" 的 28px 圆形款，禁用态跟随它），点击调起系统文件选择器，选完构造 `DataTransfer` 合成 `ClipboardEvent('paste')` 喂回上游粘贴管线（类型/数量/体积校验与报错 toast 全复用上游；host `admitEncodedImages` 经 sharp 只认 png/jpeg/webp/gif 四种位图，文档类型上游不支持，故 accept 只开图片）；共享 file input 惰性创建放 body（脚本注入点在 `</head>` 前、执行时 body 可能未出来，且必须在 React 树外），按钮被 React 重渲染抹掉时观察器按 DOM 缺席重挂，离开 700px 断点摘除；**0.1.2 输入框从 textarea 改 contenteditable**——注入目标判定从 `card.querySelector('textarea')` 扩为 `textarea, [contenteditable="true"], [role="textbox"]`（旧判断下按钮静默消失，Playwright 390px 实测发现））。断点 700px（桌面壳窗口最小宽 900px 永不命中，注入只影响经代理的远程访问）；选择器只锚语义钩子（`[role="dialog"]`、`data-sidebar-collapsed` 展开时属性不存在）与 CSS Modules 本地名子串（`[class*="_nav"]` 等，哈希前缀随版本变化不影响），上游改名则对应规则静默失效回到未适配状态。找不到 `</head>` 或非 HTML 一律原文透传。**远程加载过渡页（splash.css + splash.js）**：dsh 服务端不做任何压缩，经隧道的远程首连要下载 ~5MB（SPA ~1.2MB + 插件 bundle ~3.7MB），弱网白屏几十秒、观感如卡死（0.5.5 手机实拍反馈）。HTML 改写时把 React 挂载点 `<div id="root"></div>`（Vite 模板原值，收 upstream.rs::SPA_ROOT_MOUNT_NEEDLE 契约常量）替换为 挂载点+splash 覆盖层（spinner+标题+12s 后才淡入的弱网提示，深浅色随 prefers-color-scheme，提示文案语言随 navigator.language——`<html lang>` 是 Vite 模板恒 en 不可靠）+卸载脚本（紧随标记注入、解析到即执行，MutationObserver 盯 #root 出现子节点即淡出 0.3s 后移除；React createRoot 只动 #root 内部不碰兄弟节点）；needle 漂移则 splash 连带样式整体不注入（回到白屏等待，功能不损），契约探针守门。**代理侧 gzip**：dsh 不压缩任何响应（0.1.2 全包 grep 无 gzip/deflate 落盘代码），远程首连 ~5MB 文本资产全走 identity——splash 管观感、gzip 管实际时长。对 ≥4KB 的文本资产（application/javascript、text/javascript、text/css、application/json、image/svg+xml、text/html，含插件 bundle 改写产物）在代理侧缓冲 gzip（flate2 随 zip 已在依赖树；spawn_blocking 防堵执行器，3.7MB 约一两百毫秒）——首连传输量降到约 1/3。门槛：GET 成功响应 + 客户端原始请求头宣告 accept-encoding: gzip（须在转发剥头之前捕获）+ 无 Range + 未编码 + 已知 content-length 在 4KB~16MB 内；变换响应剥 etag/content-length/accept-ranges（buffered_builder 统一）并标 content-encoding: gzip + vary: accept-encoding；压缩失败回退 identity 缓冲体，绝不发半包。
- `tunnel.rs` — `TunnelProcess` 监督（对齐 DshProcess 模式）：`cloudflared tunnel --url <代理地址> --no-autoupdate`，从 stdout 正则解析 `https://<rand>.trycloudflare.com`（60s 未出现视为失败），指数退避重启（隧道重连后**域名变、token 不变**），停止走 `kill_process_tree` + `kill_on_drop`。**0.5.8 起 persistent 常驻模式**（会话持久化的执行面）：隧道从数据目录副本 `%LOCALAPPDATA%\DSHDesktop\tunnel\cloudflared.exe` 运行且**刻意不挂 KILL_ON_JOB_CLOSE Job Object**——防孤儿原则的唯一例外，应用退出时内核不连带回收，隧道留活保域名；`adopt()` 按 PID 收养上个会话遗留的隧道（无 stdout 可泵、状态直接 Up），3s 轮询看门狗探活（`process_alive` = OpenProcess(QUERY_LIMITED_INFORMATION)+GetExitCodeProcess——别开 SYNCHRONIZE 走 WaitForSingleObject：无同步权的句柄恒 WAIT_FAILED 表现为"永远活着"，单元测试实踩），死后衔接监督循环退避重生换新域名；stop 走杀树 + 探死兜底。
- `session.rs` — 会话状态落盘（0.5.8）：`remote-session.json`（work_dir 下，含 token/域名/代理端口/隧道 PID/副本路径；**token 是敏感凭据，文件内容绝不落 events.log**），tmp+rename 原子写，缺失/损坏/缺字段读为 None 容错；`ensure_tunnel_copy` 把内嵌 cloudflared.exe 复制为常驻副本（已存在且尺寸一致不重复拷，尺寸不符才刷新——壳升级带新 cloudflared 时自动更新）；`image_path_matches` 大小写不敏感 + 剥 `\\?\` 前缀的路径比对（复活校验防 PID 复用冒认）。
- `project.rs` + `project.html` — 手机端"项目"标签（会话页"信息"前）：浏览当前会话工作区的文件树并预览图片/md/代码。mobile.js 注入的标签面板是 iframe，指向代理自有的 `GET /__dsh-desktop/project`（自包含单页，`include_str!` 内嵌，零外部请求）——树/预览 UI 完全脱离上游 React 页面，不锚 CSS Modules 类名、不怕重渲染抹节点，这是与"信息"标签（纯 DOM 克隆路线）的关键差异。页面同源，自读 `localStorage["dsh.sessions.current"]` 拿当前 sessionId（切会话靠 storage 事件 + 2s 轮询兜底；主题跟父页面 `<html style="color-scheme">`），再调三条只读 API：`resolve`（现读 `$DSH_HOME/storages/workspace.json`，sid 命中某工作区的 `sessionIds` 即得其 `{title, path}`）、`list`（懒加载目录，目录优先排序，2000 条截断；单条目元数据失败只跳过该条——fs-local 整列失败的教训）、`file`（按扩展名出 MIME：图片直接 `<img>`、md 自写迷你渲染器排版（相对路径图片改写回 file 端点）、代码 `<pre>`+行号+换行开关、pdf 交浏览器新标签页；`download=1` 加 RFC 5987 `filename*` 供"下载到手机"）。**安全边界**：客户端只能发 sid+相对路径（绝不接受绝对路径），rel 只允许 Normal 组件，拼接后 canonicalize 并做大小写不敏感、带分隔符边界的前缀禁锢（junction/符号链接逃逸被解析后检查兜住，越界一律 403）；体积闸门 文本 >8MB / 任意 >64MB → 413。两个上游事实（localStorage 键名、workspace.json 路径与 schema）收口 `upstream.rs` 由契约套件守门（needle 实测落盘 dsh-client-runtime/lib/client.js 与 dsh-workspace/lib/index.js）。桌面壳窗口直连 dsh 不经代理，天然没有此功能。已知取舍：GBK 编码文本预览花屏（agent 产物皆 UTF-8，需要时再引 encoding_rs）、代码无语法高亮、无 Range 分段、>64MB 文件拒服（零新依赖不引流式读）。**门岗形态变化**：token 门岗由 fallback handler 内部判断重构为 `from_fn_with_state` 中间件覆盖整个 Router——壳自有路由不经过 fallback，旧形态下它们会绕过鉴权直接暴露。

**安全模型**：链接即凭据（托盘/二维码页有"勿分享"提示）；token 只在手动"关闭远程访问"再开、或"重置链接"时轮换，0.5.8 起**持久化在 `%LOCALAPPDATA%\DSHDesktop\remote-session.json`**（本会话数据目录，纯本地文件，不同步不上传）——单设备泄露窗口相应从"本次开启期间"放宽到"直到手动重置/关闭"，这是会话持久化语义的自然代价（用户要的正是"链接不轻易失效"；吊销手段不变：重置链接一键掐断全部现有会话）；卸载（非覆盖更新）由 NSIS 钩子杀常驻隧道副本 + 删状态文件彻底清场。代理与 dsh 均不监听非 loopback。**token 不落日志**：events.log 只记 phase/url/error/proxy_port（不含链接），状态文件内容整体不记，cloudflared 输出里的 `?token=` 查询串经 `redact_token` 脱敏；开启成功的 toast 正文不带链接（系统通知中心会留痕），只提示去托盘复制；**应用重启后自动恢复（resumed，链接未变）不弹 toast**、只记 events.log——后台恢复属无打扰行为（0.5.8 用户反馈弹窗是噪音），首次开启/换域名重生与开启失败的 toast 不变。已知取舍：quick tunnel 无账号模式下域名随机是结构性限制——**Windows 重启/断电、或 Cloudflare 边缘掐长连接导致隧道进程死亡时，域名必换**（复活校验探死后回退全新开隧道，token 沿用）；要永久固定链接只能上命名隧道+自有域名。但应用退出/覆盖更新/崩溃重启都不再换链接（隧道留活 + 启动收养，0.5.8 起），叠加 0.5.7 的 30 天长效 cookie，手机端收藏地址栏 URL 即可长期复用、不再反复回电脑扫码。

**分发**：`fetch-runtime.ps1 -CloudflaredVersion`（默认见脚本）从 GitHub release 下载 `cloudflared-windows-amd64.exe`（ghproxy 兜底），落 `runtime/<triplet>/cloudflared.exe`，经既有 `resources: ["runtime"]` 打包与 `.version` 部署比对；缺失时 start 报 error 态（dev 的 fixture 运行时允许没有）。

**UI**：托盘子菜单（开启/关闭互斥 enabled、复制链接、显示二维码、重置远程链接——复制/二维码/重置仅 Up 可用）+ `#/remote` 本地窗口（二维码 SVG 由 `qrcode` crate 生成、复制、重置（confirm 确认）、开关按钮）+ 诊断面板"远程访问"状态行。locale 切换重建托盘菜单后 `TrayRemoteItems` 句柄替换并按当前 phase 重设 enabled。

**测试**：`tests/remote_proxy.rs`（门岗 403/302/种长效 cookie/转发/WS 桥接/503/shutdown 释放端口/插件 bundle 三元式改写/HTML 文档移动端样式与信息标签页脚本注入、无 </head> 透传、壳自有路由无凭据 403 的门岗覆盖回归）、`tests/remote_project.rs`（"项目"标签全链路：页面标记、resolve 命中/未命中、list 排序/逃逸 403/缺失 404、file 的 MIME/no-store/413/中文名 download 头、token 播种 302 在壳路由生效；mobile.js/css 注入钩子锚定）、`tests/remote_tunnel.rs`（fake-cloudflared.cjs：URL 解析、崩溃重启、stop、**adopt 收养活隧道→死后衔接监督循环重生、adopt 死 PID 直接重生**；fake-cloudflared.url 覆写打印域名供回退断言）、`tests/remote_manager.rs`（缺 cloudflared 报 error、fixture dsh + 假隧道全链路 up→stop、reset 轮换 token 域名不变、**会话持久化三件套**：session_file_lifecycle（start 落盘内容=status/reset 改写 token/stop 删除）、resume_adopts_surviving_tunnel（链接逐字节相同 + 门岗 302 实测可用）、resume_falls_back_when_tunnel_dead（死 PID→域名换 token 沿用、resumed=false、文件重写）——复活两条须用真 platform 实现（TestPlatform 桩会恒走回退），手工造孤儿隧道+手写状态文件布景；session.rs 模块内单元：状态文件往返/损坏容错、副本刷新规则、路径比对容错）。真隧道链路不进自动化（需外网），手动验收：托盘开启 → 手机扫码完整操作 dsh。

## 17. 应用更新检查（update.rs）

"其它设置 → 检查更新"卡片：手动更新 + "启动时自动检查更新"开关（settings.json `check_update_on_launch`，默认开——公开发布仓匿名可读后，老配置无此字段升级即获得；显式关过的不受影响）。

**链路**：GitHub `releases/latest` API（`LBurny/deepseek-harness-desktop-releases`，公开发布仓）→ 比较 tag 与 `CARGO_PKG_VERSION` → 有新版则流式下载 `*_x64-setup.exe` 资产到系统下载目录（`.part` 写毕 rename，防半成品被当完整包）。版本比较自实现（去 `v` 前缀、`-rc`/`+build` 后缀忽略、逐段数值、短序列补 0；任一侧解析失败按"非新版"处理，宁漏报不误报），未引 semver crate。

**要点**：

- reqwest **走系统代理**（访问外网 GitHub，代理是通路必要条件；与 remote/proxy.rs 回环必须 `.no_proxy()` 正好相反）；GitHub API 必须带 User-Agent 否则 403
- 检查 15s 超时；下载只设 connect 超时不设总超时（60MB 慢网）
- 进度事件 `update-download-progress {downloaded,total}` 按百分比变化节流（同 lib.rs copy_cb），收尾强制 100%（content-length 与实际字节数可能不一致）
- 4 个命令（check_update/download_update/install_update/open_update_page）走既有 ACL 三处同步（build.rs/capabilities/default.json；dsh-remote 不开）；未引入 opener 插件——`open_update_page` 用 rundll32 `FileProtocolHandler`（GUI 子系统不闪控制台），`install_update` 校验路径以 `_x64-setup.exe` 结尾后 spawn，随后走 `quit_app` 让本进程先行退出（安装器是本进程子进程，旧版钩子的 `taskkill /T` 会连它一起杀；本进程先死，钩子杀树即成空操作），用户在向导里完成覆盖安装
- 启动时检查（开关开启时）在 setup 末尾 spawn：有新版弹 toast 指向其它设置页，失败只记 events.log
- 单元测试只覆盖纯函数（版本解析/比较、资产选择、响应反序列化容错）；真实网络链路不进自动化，手动验收：其它设置 → 手动更新 → 进度条 → 立即安装
## 18. 插件管理（plugins.rs）

dsh 的"插件"= 声明了 `dsh.bundle` 的 npm 包（cordis bundle，装进 profile 后作为层加载，UI 插件出现在 `/plugins/<id>/client.js`）。装/卸/更新**全部走 dsh 官方 `plugin` 子命令**（`node bin.js plugin --profile web <pnpm args>`）：profile 首次使用时由上游初始化，`pnpm add/remove/update` 在 `<dsh-home>/profiles/web/` 里跑完，上游按**安装态**对账 `dsh.profile.bundles` 层列表（解析到声明 `dsh.bundle` 的包就入层栈，被移除或新版丢声明的就踢出）。壳**不自己写 bundles**——对账逻辑归上游，跟版只动 upstream.rs 常量 + 契约测试。

**pnpm 壳内置**（`dsh plugin` 内部是 `spawnSync("pnpm", ...)`，Windows 上带 `shell: true` 走 cmd.exe 解析）：fetch-runtime.ps1 从 npm registry 下载 pnpm tarball，整包保留为 `<runtime>/pnpm/`（`bin/pnpm.cjs` → `./pnpm.mjs` → `../dist/pnpm.mjs`，dist 是 14MB standalone 全量 bundle，**不能摊平**），同时生成 `pnpm.cmd` 包装（调同目录 node.exe 跑 `pnpm\bin\pnpm.cjs`）——Windows 按 PATHEXT 只认 .exe/.cmd/.bat，没有 .cmd 包装 dsh 解析不到 pnpm。壳侧 spawn 时把 runtime 目录**前置到 PATH**（`.env("PATH", ...)` 全量保留原 PATH），dsh 内部即可解析到内置 pnpm，不污染用户环境。⚠️ 开发机测试时别用 Git Bash 手工验证 spawnSync 解析——msys 会把 `H:\...` 路径改写成 `H;C:\...` 伪失败；集成测试（cargo test，原生进程环境）是权威验证。

**命令与数据流**（6 个，PluginsHome 状态托管 node_exe/dsh_bin/home/pnpm_dir，与 DshProcess 同源路径）：

- `get_plugin_status`：pnpm 就绪态（`pnpm.cmd` + `pnpm\bin\pnpm.cjs` 都存在）+ `node pnpm.cjs --version` 实测版本 + profile 是否已初始化（`profiles/web/package.json` 存在）
- `list_plugins`：读清单 `dependencies`（版本）与 `dsh.profile.bundles`（标"插件"徽章，其余标"依赖"）；文件缺失 → 空列表；BOM 容忍；解析失败显式报错
- `search_plugins(q)`：npm registry search API（`registry.npmjs.org/-/v1/search`，reqwest 带 UA；外网走系统代理——与回环的 no_proxy 相反，同 update.rs 约定）；结果与已装列表交叉标"已安装"；查询 <2 字符直接返回空
- `install_plugin(spec)` / `uninstall_plugin(name)` / `update_plugins()`：`run_plugin_op` 统一执行——`node bin.js plugin --profile web <args>`，DSH_HOME 注入、PATH 前置、无 shell（参数直接走 argv，杜绝注入）、`configure_child_command`（CREATE_NO_WINDOW 防闪控制台）、spawn 后 `register_child` 挂全局 Job Object（壳被杀连带回收，防孤儿）；**stdout/stderr 必须显式 pipe**——tokio 的 spawn 默认继承父进程 stdio，`wait_with_output` 只读管道句柄，不接管道则 output 恒为空（前端"看下方输出"永远没内容，0.2.0 实踩）；输出 stdout+stderr 合并截断 200KB 返回。`validate_spec` 拦截空/超长/`-` 开头（防参数注入）。`busy` Mutex 串行锁：同一时刻只允许一个操作（try_lock 失败报"进行中"），防并发写 profile。

**IPC 契约**：本模块返回前端的结构体（`PluginOpResult`/`PluginStatus`/`PluginRow`）一律 `#[serde(rename_all = "camelCase")]`——前端按 camelCase 读键，漏了 rename 时多词字段（`exit_code`/`pnpm_ready`/`is_bundle`）在前端恒为 `undefined`：`exitCode === 0` 永不成立 → 成功被误报"失败"、pnpm 状态恒显示"缺失"、bundle 徽章恒显示"依赖"（0.2.0 全中）。`ipc_payloads_serialize_camel_case` 锚定测试守门。

**生效方式**：装/卸/更新**没有 MCP 那种 HMR**——新层经 profile manifest 在 web 启动时加载，完成后面板提示"重启 dsh 后生效"，内置"重启 dsh"按钮复用 `restart_dsh` 命令（装多个插件只需最后重启一次）。运行中安装不冲突（Node 模块文件句柄带共享删除标志，pnpm 增删无碍）；若 pnpm 报错引导先重启再重试。

**已知取舍**：registry 搜索请求本身不进自动化测试（同 update.rs 策略，只测解析）；集成测试（`tests/plugins_integration.rs`，无运行时自动 skip）用真实 dsh bin.js + 假 pnpm.cmd 断言 profile 初始化、参数透传、cwd=profile 目录、退出码透传、PATH 注入生效；真实安装链路手动验收（托盘 → 插件管理 → 搜索 → 安装）。契约测试 `probe_plugins_cli` 探测 bin.js 的 `command("plugin")` 与 `requiredOption("--profile <name>")`——上游改版即红。

## 19. 目录选择器钉 browse（picker.rs）

dsh 新建工作区要选文件夹，选择器有两套交互，启动时由 `directory-picker-auto` 一次性决议：绑 127.0.0.1 + win32 ⇒ **native**（koffi 驱动 Win32 系统对话框，弹在电脑屏幕上）；非回环/SSH ⇒ **browse**（网页内嵌对话框）。壳的远程代理对 dsh 透明，dsh 永远决议 native——**手机远程端点"添加工作区"，系统对话框弹在电脑屏幕上，手机上什么都看不到，无法选择**。

修复走上游官方 pin 方式（`apps/web/tests/pin-browse-picker.overlay.yml` 与 shipped bundle patch 行注释明示）：`picker::ensure_browse_picker` 在 spawn dsh 前往 `<dsh-home>/profiles/web/cordis.patch.yml` 幂等确保两条补丁——`{id: directory-picker, disabled: true}` 禁用 auto 行，insert `@deepseek-ai/dsh-host-directory-picker-browse`（host 列目录/建目录）+ `@deepseek-ai/dsh-client-ui-directory-picker-browse`（网页表面，占 ui-workspace 的 directory-flow 槽位）。与 mcp.rs 管理同一文件：mcp 只认 `name=='@deepseek-ai/dsh-mcp-client'` 的 insert 条目，picker 的三条互不命中，Value 级共存（read_patch/write_patch 复用 mcp.rs，BOM 容忍 + tmp+rename 原子写）；缺行补行、用户手加的 disabled 摘掉，只在有变化时写盘（无谓写会触发 HMR 重载）；失败只记 events.log 不阻断启动。

**桌面端同步变为网页版对话框**（选择器是 dsh 启动期全局决议，无法桌面 native/手机 browse 并存）——功能不减：浏览全盘、面包屑、手输路径（前辍过滤）、新建文件夹。对话框本体是上游 figma 设计（680×500 viewport-clamped，Miller 双栏窄屏横滚 + JS 自动钉右），mobile.css 只补布局：≤700px 时高度放宽到 `calc(100dvh - 48px)`（500px 上限在手机上列表仅 ~9 行），footer 三控件一行均分且移除"显示隐藏文件"开关（隐藏条目已由 pickerpatch 改为默认显示，开关在手机一行布局里挤占"新建文件夹"），锚点 `_millerRow` 是该包独有 CSS Modules 本地名（`:has` 限定不误伤设置弹窗）。

**pickerpatch.rs 运行时补丁**（presets.rs 同款签名门控 + marker 幂等原地改写，dsh 自更新还原后下次启动重打；needle 收口 upstream.rs、`probe_pickerpatch` 守门）：
- *盘符层级*：host `list()` 特判哨兵路径 `"dsh:drives"` 返回 A-Z 可用盘符根（不可读/未就绪的盘 stat 跳过），`ancestryCrumbs` 对盘符根路径前插"此电脑" crumb——没有这一层，面包屑在 home 子树内被客户端 `displayCrumbs` 折叠成单个"主页"，想到其它盘只能手输路径（手机端实踩痛点）。客户端配套：`displayCrumbs` 折叠时保留哨兵 crumb 居首（任意位置一键回盘符层）、哨兵 crumb 走 locale 文案（`browser.drives` 此电脑/This PC，host 不知道客户端语言）、哨兵层级禁用"打开/新建文件夹"（防把 `"dsh:drives"` 选成工作区/当父目录）。
- *隐藏条目默认显示*：client 的 `showHidden` 初值与每次开框重置都改 `true`。
- *耦合规则*：客户端是哨兵功能的安全前提（本地化 crumb + 禁用"打开"），其签名漂移/文件缺失时整组停手——只改 host 会放出能把哨兵选成工作区的半成品。

**跟版门禁**：`probe_picker` 契约探测——shipped bundle patch 仍含 `id: directory-picker` 的 auto 行（disable 目标）、两个 browse 包仍在依赖闭包（insert 行能被 Loader 解析）；`probe_pickerpatch` 核对两包内文件的全部补丁 needle（host 2 处 + client 8 处），上游改版即红；上游若默认 browse 即可删 picker.rs。`remote_proxy.rs` 的注入测试断言 `_millerRow` 规则随 mobile.css 注入。
