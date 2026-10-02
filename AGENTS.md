# DSHDesktop

[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（dsh，DeepSeek agent harness CLI）的 Windows 桌面壳：Tauri 2 窗口内嵌 dsh 官方 Web UI，Node.js 与 dsh 随安装包分发、装完即用。

## 技术栈与形态

- **Tauri 2 + Rust**（`src-tauri/`）：进程监督、运行时部署、托盘、通知、主题跟随、诊断命令
- **Svelte 5 + TypeScript**（`src/`）：六个本地页面（splash/diagnostics/skills/mcp/remote/settings）；主界面是导航到的 dsh Web UI（`http://127.0.0.1:<port>`）。壳的插件管理面板已随 0.5.21 移除（dsh 自带）
- 安装包：NSIS（`pnpm tauri build`），单实例、托盘常驻、关窗默认隐藏到托盘（可在"其它设置"改为直接退出）

## 目录结构（逐模块细节见 docs/design.zh-CN.md）

```
src-tauri/src/
  main.rs           应用入口（薄）：windows_subsystem + lib::run
  lib.rs            Builder 组装 + setup 编排：建主窗口（visible(false)+几何记忆排队，防白闪/
                    托盘隐藏态被记住）、事件桥/通知汇接线；single_instance 插件必须最先注册；
                    run() 顶部 debug-cdp marker → WebView2 CDP :9222 诊断开关
  logging.rs        events.log 统一追加层：append_debug_line 补时间戳前缀、1MB 截断
  redact.rs         token 脱敏：任何落日志/事件的文字先过它（链接即凭据）
  patchstore.rs     cordis.patch.yml 通用条目读写（Value 级保留、tmp+rename 原子写）；
                    设置存储（ui-theme/locale/onboarding/overlay）与 MCP 共用
  eventbridge.rs    进程事件桥：dsh-ready→带 token 导航 + 431 cookie 防护 + 30s 心跳看门狗
                    自愈一次 + 凭据下发；与 pagebridge（页面→壳）成对
  i18n.rs           壳界面语言跟随 dsh locale.preference；文案 pick(zh,en) 二选一
  dsh/              dsh 运行时与进程事实
    process.rs      DshProcess 监督循环：spawn `node --max-http-header-size=65536 bin.js
                    web --port N --no-open`；stdout 就绪行是 launch token 唯一来源；npm 冷装
                    切 10min 长预算；指数退避、stop/restart；**端口优先复用记忆值**
    dsh_session.rs  BrowserAuth 凭证：token 解析+换 cookie（绑 authority，换端口即失效）；
                    凭证只在内存、日志脱敏
    upstream.rs     dsh 上游内部事实单一来源（入口/命令形/WS 帧/needle/钩子）；跟版红了
                    只改这个文件；tests/upstream_contract.rs 逐项探测（无运行时自动 skip）
    presets.rs      minimal 预设签名只读哨兵
    runtime.rs      ensure_runtime：可写原地跑、只读回退部署副本；`\\?\` 经 strip_verbatim
                    （别绕过它自己拼）
    port.rs         选端口：复用记忆端口（dsh-port.txt，Web 源站跨启动稳定）+ free_port
                    兜底 + wait_ready
  platform/         Platform trait（多平台预留）；windows.rs 含全局 Job Object（父被强杀
                    内核连带回收子进程树）
  bootstrap/        启动期自愈总入口 run_all：补丁 → pnpm store 自愈 → preseed 播种
    plugins.rs      插件操作执行层（只服务 preseed；装/卸走官方 dsh plugin 子命令）；spawn
                    前 pnpm store 迁移自愈 heal_profile_store
    preseed.rs      预安装插件播种（marker 语义；dev 下静默无操作）
    progress.rs     首启进度模型
    patches/        run_all（顺序即契约，签名门控+marker 幂等，失败只记日志）：welcome/
                    picker/pickerpatch/mcpgate/oiacache——补丁随跟版增删只动本组
  ui/               壳窗口与 UI 设施
    state.rs        UiSnapshot/ShellUiState/主题语言解析 + get_shell_ui_state 命令
    theme.rs        标题栏主题跟随 ui-theme 条目（settings.yaml 是遗留导入通道，壳不读写）
    tray.rs         托盘菜单 + 五个按需窗口（theme_bootstrap 防白底闪）
    zoom.rs         UI 缩放：hook_js 只注入 main；ui-zoom.txt 持久化
    pagebridge.rs   主窗口观测桥：错误→events.log（限流+截断+脱敏）；#root 心跳驱动看门狗
    download.rs     主窗口下载：系统下载目录+去重+toast；缺它文件无声消失
  features/         本地页面后端
    diagnostics.rs  诊断命令（原 commands.rs：get_status/restart/日志尾部/自启动等八条）
    settings.rs     壳设置 settings.json（校验；落盘失败显式报错不静默吞）
    skills.rs       skills/ ↔ skills-disabled/ 移动即开关；三源+ZIP 导入（防穿越+条目上限）
    mcp.rs          cordis.patch.yml 的 dsh-mcp-client 条目读写（通用层已拆 patchstore.rs）
    update.rs       检查更新（发布仓 releases/latest）；install_update 必传 /UPDATE /P /R
  notify/           mod=单 WS /api/remote.mux 事件桥（**严禁回包 $events/result**）；
                    mux=传输层（帧形收 upstream.rs）；sink=通知门控+提示音；toast=WinRT
                    直连（点击走协议激活）
  remote/           mod=RemoteManager（收养存活隧道+链接字节级不变；退出只死代理留隧道）；
                    session.rs=remote-session.json（含 token，绝不落 events.log）；
                    proxy.rs=token 门岗反向代理（401 重放整读体 64MiB 上限、流式上传旁路、
                    HTML 注入 mobile/splash、≥4KB 文本资产 gzip、WS/SSE 静默 30s 触发式
                    保活对策 Cloudflare ~100s 空闲斩杀）；project.rs=手机端"项目"只读路由；
                    tunnel.rs=cloudflared 监督（**刻意不挂 Job Object**；5 次快败降级 5min
                    慢速无限重试；spawn 钉 --protocol http2）
src/                七个本地 Svelte 页面 + App.svelte(hash 路由) + i18n.ts
src-tauri/windows/  nsis-hooks.nsh 安装/卸载钩子（杀树/等锁/清扫，细节见坑区）
scripts/            follow-upstream.ps1(一键跟版) fetch-runtime.ps1(抓运行时) acceptance.ps1
                    (真机验收) release-local.ps1(双仓上传) release.ps1(pnpm release 流水线)
                    use-fixture-runtime.ps1 verify-*.ps1 等
docs/design.zh-CN.md 设计文档（架构/模块/打包/测试/已知限制）
.trash/             本机垃圾暂存区（规则见「垃圾文件与清理」节）
```

## 垃圾文件与清理

- **仓内垃圾统一落点 = `.trash/`**：被 `.gitignore` 挡住（git status 看不见、git add -A 不收、release 工作区干净判定与镜像/CI 都不受影响）——"可删但先别删"的一律移进去，别直接删。其 README.md 维护内容清单与"删掉的代价"；清空 = `rm -rf .trash/*`。只读/被占用文件 MSYS rm 会 Permission denied：用 PowerShell 先清 ReadOnly 再 `Remove-Item -Recurse -Force`。
- **`src-tauri/target` 会无上限长大**（2026-09-21 实测 182GB，已清）：可删 `debug/` 全部 + release 下 `deps`/`build`/`incremental`/`.fingerprint`/`_up_`/陈旧 `*.lib`/`*.rlib` + 打包 staging（`runtime`/`resources`/`sounds`/`icons`/`nsis` 工作目录/`preseed-plugins`，全是 bundler 从 src-tauri 拷出的副本）。**必须保留 `target/release/bundle/nsis/`**——历版安装包原件档案库（远端缺资产靠它回填；发布门禁缓存要求最新一版 exe 在场）。**跑 `pnpm tauri build` 前先把最新一版 exe 备份出仓**：同版本重编会就地覆盖同名 exe，且 NSIS 打包不可字节复现（0.5.17 实测重编件与发布原件字节不同），覆盖即丢原件、随件 sha256 失真；核对线上真值读 `api.github.com/.../releases/latest` 资产的 `digest` 字段（比 sidecar 可靠）。清理落点统一 `.trash/`；删后下次 cargo test/build 全量重编一次属正常。
- **`.playwright-mcp/`**：MCP 浏览器的快照/console 残留（gitignore 内），随时可整目录删。
- **临时探针现场**（手机端七步法等的运行时副本+home 拷贝，700MB 级）：用完即删；别放 C 盘；含会话数据的 home 拷贝**绝不外传**。
- events.log（壳数据目录 `%LOCALAPPDATA%\DSHDesktop\`）1MB 自截断，无需清理。

## 常用命令

```bash
# 开发（需要 fixture 运行时：先跑 scripts/use-fixture-runtime.ps1，再设 DSHDESKTOP_RUNTIME_DIR）
cd src-tauri && cargo test            # 全部测试（单元+进程集成+WS通知+控制台窗口+远程访问+上游契约）
pnpm tauri build                      # 产出 src-tauri/target/release/bundle/nsis/DSHDesktop_*_x64-setup.exe
powershell -File scripts/fetch-runtime.ps1   # 抓取真实运行时到 src-tauri/runtime/windows-x64/
powershell -File scripts/follow-upstream.ps1 -DshVersion <新版> [-Bump patch]   # 一键跟版
powershell -File scripts/acceptance.ps1 -SetupExe <setup.exe>   # 卸载旧版→安装→启动→全项校验→截图
pnpm install --frozen-lockfile        # 依赖（pnpm store 在 F:\Code\pnpm-store；重装删残 node_modules 的坑见坑区）
pnpm release                # 一条命令发版（-DryRun 演练、-SelfTest 自检；跑前工作区必须干净）
```

## 版本与发布

- **版本号规则（固定）**：每发一版 patch +1，patch 到 9 归零、minor +1——`0.4.0 → … → 0.4.9 → 0.5.0`，不按 semver 语义跳版（0.x 阶段只数发版次数）。当前 0.5.25，下一版 0.5.26。
- **发版**：`pnpm release` 一条命令跑完整链路（参数 -Version / -CommitMsg / -SkipAcceptance / -DryRun / -SelfTest；跑前工作区必须干净——本次改动先单独 commit，docs/release-notes/、CHANGELOG、三处版本文件、AGENTS.md 允许脏并会被收编；说明文件缺失会脚手架后退出，填完重跑）。机械步骤：bump 三处 → CHANGELOG 收编（补回空 Unreleased）→ 说明脚手架+格式 lint → 版本指针 → cargo test → build → acceptance 真机验收 → commit/tag/push → 镜像说明 → release-local 双仓上传 → 匿名 API 终验（读回线上正文须含说明文件首行原样）。**门禁缓存**：`%LOCALAPPDATA%\DSHDesktop\release-cache\v<ver>.json`（head 未变或 diff 只含白名单文件即秒级跳过）。
- **双语说明（0.5.11 起）**：`docs/release-notes/` 每版两文件（`v<ver>.md` 英文正文传 -NotesPath，`v<ver>.zh.md` 中文）；正文首行 `English | [中文说明](<发布仓 blob 链接>#中文说明)`，中文不内联、点击跳发布仓（该目录随镜像进发布仓，**先推镜像再 PATCH** 外链才不 404）；只改正文用 `-NotesOnly`（不碰资产）。0.5.11 前只有一条 Full Changelog 链接，别再犯。
- **弃用 CI 发布（0.4.9 起）**：私有仓 tag 触发跑满 30~70min（Actions 缓存按 ref 隔离），本地 3~5min。release.yml 降为 workflow_dispatch 备用；CI 只剩 build.yml（push main 触发，兼作 rt 运行时缓存预热）。
- **断点重跑一律显式带 `-Version <ver>`**（2026-09-21 实踩）：`pnpm release` 不带参按进位规则算的是**下一版**——"重跑一遍确认"会直接开新版本：bump 三处 → 把空 `[Unreleased]` 收编成 `## [<新版>]` → 脚手架出两份含 TODO 的说明文件 → 停在 [3/9] 退出码 1（**此时尚未 commit/tag/push，远端无污染**）。回退 = `git restore package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml CHANGELOG.md` + 删掉脚手架出的两份说明文件；续跑同一版用 `pnpm release -- -Version <ver>`。
- **[9/9] 上传 exe 撞 TLS 抖动不必重跑整链**（同轮实踩：`Invoke-RestMethod : The underlying connection was closed`，发布仓那半已完成）：按幂等语义重跑 `powershell -File scripts/release-local.ps1 -Version <ver> -NotesPath <绝对路径/正斜杠>`（Release 已存在则复用、删除同名旧资产再传），再手工补终验三件套——匿名 GET 两仓 `releases/latest`（tag_name、资产恰为 exe+sha256、正文首行与说明文件首行逐字相同、无 `????`）+ 镜像仓中文说明 `api/.../contents/` 可达。

## GitHub 访问

- **源码仓** `LBurny/deepseek-harness-desktop`（origin，2026-09-09 起公开）：源码 + tag + Release（exe+sha256 存档）
- **发布仓** `LBurny/deepseek-harness-desktop-releases`（public，匿名可读）：**只发 exe+sha256**（分发门面红线，release-local.ps1 守卫拒发其它文件）；应用的检查更新全部指向它（update.rs 常量有锚定测试）；本地镜像 `H:\My_Software\deepseek-harness-desktop-releases`。Release 页的 "Source code (zip)/(tar.gz)" 是 GitHub 按 tag 自动生成的仓内归档——删不掉也别删 tag：**删 tag 会把 Release 转成草稿**、匿名 releases/latest 即断（草稿 PATCH draft:false 会把 tag 复活回来）
- 本机没装 gh CLI；GitHub API 用环境变量 **GH_TOKEN**（对双仓 200）；api.github.com 直连正常
- github.com 直连可能被 TLS 拦（间歇性）：push 失败先试直连，再一次性 `git -c http.proxy=http://127.0.0.1:7890 -c http.sslBackend=schannel push`（不写配置；先 curl 确认代理在线）

## 关键约定与坑（细节与踩坑史见 docs/design.zh-CN.md 与 CHANGELOG.md）

- **set_autostart 先查再关**：auto-launch 0.5 的 disable() 对不存在的 Run 值直接 RegDeleteValueW → "os error 2"，从未开过自启动的用户每次保存设置都弹——先 is_enabled() 比目标态（features/diagnostics.rs 锚定测试）。
- **dsh 事实基线（upstream.rs 为单一来源）**：Node `^22.19 || >=24`；入口 `lib/bin.js`；`dsh web` 只绑 127.0.0.1 且 spawn 必带 `--no-open`。BrowserAuth：launch token 走 stdout 就绪行（**晚于 HTTP 绑定**，必须持续 pump），`/?token=` 303 换 `dsh-auth-<hash>` cookie（HttpOnly/Strict，**绑 authority，换端口即失效**）；静态资产无门、`/api/*` 与 WS 在门内。事件走单 WS `/api/remote.mux`（帧形收 upstream.rs），完成判定看 follow 流 `turn/end`（reason.kind=="completed"），子代理看 `api-session/added` 的 origin；**严禁回包 `$events/result`**。预设独立成包；主题/语言键在 profile patch 的 `ui-theme`/`locale` 条目 config.preference（0.2.0 起 settings.yaml 被上游废除为一次性遗留导入通道，壳写它会每次启动触发导入、把旧值灌回覆盖用户现选——0.5.19"深色模式启动后 UI 未跟着变深色"根因；壳偏好读写统一走 `profiles/web/cordis.patch.yml` 顶层直排条目，**别读 cordis.yml**——它每次启动被重写为空序列）；npm 依赖浮动区间，靠契约套件守门。
- **0.1.5 四条新事实**：①会话格式 V3 起**升级后不可降级读取（用户数据单向）**，发版说明必须明示、验收必须覆盖"保留 DSH_HOME 升级首启"；②流式上传路由 `POST /api/session/uploadFileBinary`——代理必须开流式旁路；③面板槽位 keyed `main`+`rightbar`+`sidebar.panellist`；④minimal 预设只剩持久 shell。另：出站遵循 HTTP(S)_PROXY/ALL_PROXY/NO_PROXY（回环豁免）、子进程 windowsHide、免鉴权 `/open-in-app/*` 路由族。
- **流式上传必须旁路**：proxy 的 forward() 为 401 重放会整读请求体（64MiB 上限），命中 `is_streaming_body_route` 即转逐块直通（上传前强制换 cookie、不做 401 重放），换不到 cookie 时 401 原样透传不误报 502。回归 `upload_route_streams_without_buffering`——hyper 对 wrap_stream 体不主动 flush，测试客户端必须裸 TCP 手写 chunked。
- **跟版脚本文档锚点是计数断言式**（upstream.rs=1 / design=3 / README×2=1）：改 upstream.rs/design 不得引入钉版全串以外的版本字面量，历史对照写成不带 `-rc` 的形态；计数不符整步报错。
- **运行时布局**：tauri.conf resources 用映射形式 `{ "runtime": "runtime", "resources/sounds": "sounds" }`（列表形式会错落致提示音探测不到；`..` 会变 `_up_`）。
- **子进程控制台**：configure_child_command 设 CREATE_NO_WINDOW；taskkill 同样必须带（否则退出/重启闪 cmd）。复现"无控制台父进程"须 FreeConsole()；验收判据是可见 ConsoleWindowClass 窗口。
- **PowerShell 5.1 与 7 行为不同，发布流水线恒跑 5.1**（package.json 硬编码 powershell）：①文件内容进 JSON 一律 `[System.IO.File]::ReadAllText`——`Get-Content -Raw` 挂 NoteProperty 会被 ConvertTo-Json 按对象序列化（422）；②JSON 正文一律经 `Get-JsonBodyBytes` 发 UTF-8 字节体 + charset=utf-8——string 体在 ContentType 无 charset 时按 ISO-8859-1 编码、非 ASCII 变 `?`（0.5.12 线上正文 `????`）；③含中文的 .ps1 必须 UTF-8 **带 BOM**（5.1 按 ANSI 代码页读，本机 936；Edit 工具改完会丢 BOM 须补回）。防线四道：release-local -SelfTest（5.1/7 双跑）、[0/9] 自检、[3/9] 形状预检、[9/9] 终验读回线上正文。
- **脚本别用 Process.MainWindowHandle**（debug exe 句柄指错）：按 class "Tauri Window" 枚举（verify-*.ps1 模式）。
- **Tauri setup 无 tokio 上下文**：spawn_supervised 经 `tauri::async_runtime::block_on`。
- **resource_dir() 返回 `\\?\` 扩展路径**：Node 加载器不认，`runtime::strip_verbatim` 已处理，别绕过 ensure_runtime 自己拼。
- **events.log 是壳+dsh 事件的统一持久层**（1MB 截断，诊断面板回填读尾部）：写入统一走 append_debug_line，无时间戳的行自动补本地前缀（排查启动时序读行首）。
- **fixture 用 .cjs**（根 package.json 是 type:module，use-fixture-runtime.ps1 会铺 CJS 桩）；`#[tokio::test]` 带 sleep 须 `flavor="multi_thread"`。
- **dev 模式不拷贝 bundle.resources**：内置音效手动复制到 target/debug/sounds/；真实运行时别放 src-tauri/runtime 下跑 dev（dsh 自更新会触发 watcher 重建循环）——复制到 src-tauri 外用 DSHDESKTOP_RUNTIME_DIR 指向。dev 与安装版共用壳数据目录（含端口记忆 dsh-port.txt），同时运行会互相顶掉各自记住的端口（都在跑、各用各的仍正常；验端口复用别开着安装版）。
- **NSIS 离线**：直连不稳时用 ghproxy.net 预置 `%LOCALAPPDATA%\tauri\NSIS`（含 nsis_tauri_utils.dll，SHA1 须匹配）。
- **托盘 quit 顺序**：远程开着时**不杀隧道**（suspend_for_exit 保链接），只 stop dsh 等 1.5s 再 exit；杀子进程树用 `taskkill /T /F`。
- **安装器只杀主程序**：子进程全靠 Job Object 随父死亡回收 + nsis-hooks 杀树/清扫；缺层则运行中重装必现 "Can't write: …\cloudflared.exe"。
- **NSIS 按路径清扫必须排除调用方自身**：`$INSTDIR\*` 匹配会把 `_?=` 原地运行的卸载器自己杀掉 → "Unable to uninstall!"（排除用 PowerShell 父进程 PID）；覆盖安装回归只能靠带 `_?=` 的原地调用测。
- **进程死亡≠exe 文件锁释放**：Defender/PCA 对刚退出的映像持柄 1~3s——钩子等净进程后再等三个 exe 可独占打开（探针 15s 封顶）；"Unable to uninstall!" 的触发条件是"卸载器退出码非 0 **或**主程序 exe 仍存在"，两路都要想到。
- **NSIS `_?=` 必须是卸载器命令行最后一个参数**：之后的内容全被吞进 `$INSTDIR`（放错会得到假"卸载失败"）。
- **install_update 必须传 /UPDATE /P /R**：模板仅在 /UPDATE 跳过"先卸载旧版"直接覆盖安装（旧卸载器不参与 = 文件锁竞态与隧道清理都无从发生）；裸跑会断链。手动升级流由 POSTUNINSTALL 的父进程检测兜底；已知残留：从旧版手动双击新包选"卸载后再安装"可能复现弹窗——选"不卸载"或退出应用半分钟后再装。
- **远程 IPC 放行三处同步**：build.rs `AppManifest::commands` 生成 permissions、capabilities/default.json、按需 dsh-remote.json（只对 127.0.0.1 开放 zoom_ui + pagebridge 两条写日志命令）。新增命令三处同步（tests/command_registration.rs + remote_capability_only_exposes_zoom_and_pagebridge 锚定）。
- **缩放快捷键匹配**：主匹配 `e.code`、`e.key` 兜底（合成/RDP 注入的 code 为空）；改快捷键须重注入钩子（set_shell_settings 已做）。
- **reqwest 在系统代理下会劫持 127.0.0.1**：凡访问回环必须 `.no_proxy()`，否则被代理接管表现为假 502/挂起。
- **手机端 UI 检查七步法（临时实例 + MCP 浏览器）**：mobile.css/js 只由代理注入，直连页面≠手机所见；真走代理要从托盘读链接（用户用机期间别动 CUA）。日常验证 = 临时裸 dsh 实例（**副本运行时**+临时 DSH_HOME+stdout 抓 token，别原地跑防自更新动钉版树）→ 拷本机 `storages/workspace.json` 播种（别手搓 schema）→ MCP 浏览器 390px → 静态服务跨源 fetch 注入改后的 css/js → `browser_evaluate` 量 computed style/祖先链，断点行为 resize 扫 500/700/720；空 home 无会话时 mobile.js 不打 `data-dshmobile-enhanced` 门（找到会话根才打），验"增强态"须手动补 `setAttribute`。输入框是 **contenteditable 不是 textarea**；无 API key 可真跑一轮（匿名通道）。现场用完即删（见「垃圾文件与清理」）。
- **0.1.2 token/cookie 时序四坑**：①"端口通了"≠能登录（就绪行晚于绑定，wait_token 门控）；②换端口 cookie 全失效——代理 401 清缓存重换重放一次；③WS upgrade 是独立桥接路径，cookie 注入最易漏；④**token 按进程轮换，spawn 前必须清缓存**。假 dsh（tests/support）全仿真。
- **Cloudflare 对静默长连接有约 100s 空闲斩杀窗口（0.5.22 修）**：对策在代理层、**静默触发式**（通道活跃时一个保活帧都不发）：WS 桥接 dsh→浏览器静默 30s 代理自发 Ping（浏览器协议层自动回 Pong）；任意 `text/event-stream` 上游静默 30s 注入 `: keepalive\n\n` 注释行（SSE 规范内，EventSource 忽略，dsh 零改动）。保活间隔经 `Keepalive`/`spawn_proxy_with_keepalive` 注入（生产 30s，测试短间隔）。**传输层另有独立死因（0.5.24 修）**：QUIC/UDP 被运营商空闲回收整连接打死，应用层保活救不了——spawn 钉死 `--protocol http2`（`tunnel.rs::tunnel_args`，锚定测试防回退）；quick tunnel 锁 `ha-connections:1` 单连接无冗余，HA 只有命名隧道有。排障：events.log grep `stream canceled`（cloudflared 原话带流号 `stream <N> canceled by remote`，别漏中间数字）。
- **dsh-auth cookie 按进程累积 → 431（0.5.11 修）**：每进程新名 cookie（30 天）不分端口，WebView2 罐子只进不出；攒 ~66 个 + 插件组合 URL 超 Node 16KB 头上限 → 431 → "Failed to load plugins"，重启不消。对策：Ready 导航前 `prune_stale_dsh_cookies`（只删 dsh-auth-*；必须跑在 async 上下文——Windows 同步回调里调 cookies() 会死锁）。排障：debug-cdp marker → CDP :9222 看 Network.loadingFailed；cookie 加密绑 WebView2 应用（拷给 Edge 解不开=假阴性）；dsh 静态资产确实无门，别往"cookie 校验"查。
- **主窗口观测桥三件套（0.5.11）**：pagebridge 错误捕获（document-start、绕 CSP、限流/截断/脱敏）→ events.log；#root 心跳 → Ready 导航 30s 无心跳自愈一次（清 cookie+带 token 重导航，one-shot 不自旋）；debug-cdp 开关。页面桥命令从远程源调用只写日志，再放行新命令先过安全审查再改锚定测试。
- **dsh 就绪行会被 MCP 插件加载阻塞**：dsh-mcp-client apply() 无条件 await connection.ready，`npx 裸名/@latest` 首次冷装可拖分钟级。壳对策：wait_token 见 npm `will be installed` 切长预算、splash 换文案、Ready 落耗时分解。MCP 装什么是用户自由，壳只做"不杀错、可感知、可诊断"。
- **reqwest 错误 Display 自带完整 URL**：带 token 的请求失败若直接 `{e}` 会明文落 events.log——一律 `e.without_url()` 后再格式化（dsh_session.rs 钉死）。
- **插件 bundle 合并加载（0.1.2 起）**：`/plugins/??<a>/client.js,…&rev=N`（rev 是内容哈希、响应 immutable）；壳改写产物变了须 302 到带 `dshv=<壳版本>` 的 URL 击穿；真 dsh 对组合 URL query 逐字校验、多余参数 404——buster 转发前 strip_cache_bust 剥掉；`send_forwarded` 别就地按 url 重算改写判定（带 scheme 恒 false）。
- **iOS WKWebView 两个键盘坑（独立症状别混修）**：①聚焦退出后视口平移残留——mobile.js 失焦/vv resize 时复位文档滚动 0；②聚焦 <16px 输入框自动缩放整页不复原——proxy 把 viewport meta 改写为 `maximum-scale=1, user-scalable=no`（needle 收 upstream.rs）+ mobile.css composer 字号抬 16px，双防线。
- **主窗口由 setup 代码创建**（tauri.conf windows 为空）：on_download/on_new_window 只能挂 WebviewWindowBuilder；建窗参数与原 conf 一致；回归 verify-no-size-flash / verify-window-state。
- **dsh 预设已独立成包（0.1.2）**：PRESET_DIR_SEGMENTS 随版；签名哨兵盯 win32 修复不回退。
- **fs-local 列目录遇 ACL 拒绝项即整列失败**（C:\ 根必现）：上游行为，壳侧让模型待在 workspace，别在壳里修。
- **提示音禁用 PlaySoundW，自管 waveOut**：专用线程每次 waveOutOpen 新开设备句柄、播完即关、每步真实错误码、打断自管——四轮翻车史见 CHANGELOG，改回等于把盲区请回来。
- **toast 点击激活只能走协议激活**：in-process Activated 回调 Win10 不可靠；XML 带 `activationType="protocol" launch="dshdesktop://open"` → single-instance 拦截 → show_main；依赖启动时 ensure_activation_registered（仅安装形态写，dev 跳过）。
- **协议激活到顶统一走 show_main → platform bring_to_front**：两段择时（250/550ms）→ TOPMOST → AttachThreadInput 借权限 → SetForegroundWindow → NOTOPMOST；二次实例在 main 开头 `AllowSetForegroundWindow(ASFW_ANY)` 广播给主实例；每步落 events.log。教训：**先验证调用链再加固机制**。
- **toast 图标只有顶部行小图标**（HKCU AppUserModelId 的 IconUri → 随包 256px `icons/128x128@2x.png`），XML 不含 `<image>`（appLogoOverride 叠双图标）；锚定测试：bundle.resources 漏映射则 IconUri 静默失效、断言 XML 无 `<image>`。
- **dsh 写锁死锁（0.5.13 壳侧自愈；0.2.0 起上游原生接管，locks.rs 已退役）**：锁是 `<file>.lock` 兄弟文件；上游 dsh-atomic-write 对持有者已死的锁经 PID 探活自动接管（记录不完整的锁只等待不接管，理论边缘）。旧急救脚本 `scripts/check-dsh-locks.ps1` 保留（-Remove 只清死锁）；契约 probe_locks 转为哨兵盯上游接管机制不回退。
- **模型触发器的图标只有一枚（0.1.5 起）**：上游自带 triggerIcon（默认 display:none，容器 ≤360px 才亮），壳不得自造第二枚——mobile.css 在 700px 断点点亮上游那枚（361~700px 区间否则一枚不剩）。回归 model_trigger_iconified_rule + 契约探针 MODEL_TRIGGER_ICON_NEEDLE。
- **回合统计行搬移锚点是 `data-composer-stats`（0.1.5 起）**：StatsPills 行的分隔点"·"在药丸 label 内部，结构锚必须认该属性（旧的子代 `_sep` 计数锚恒失配）；面板只藏行级分隔符（药丸内部的 · 保留，否则计数与速率文本粘连）。契约探针 COMPOSER_STATS_ROW_HOOK。
- **「在文件资源管理器中显示」点了没反应（0.2.0 起上游原生修复，壳补丁已退役）**：根因 = libuv 的 windowsHide 让 Explorer 文件夹窗口继承 SW_HIDE——窗口建出来了（路径/选中都对、HTTP 204 照常回）但 `IsWindowVisible=false`。0.2.0 上游把 runner 参数化（explorer 显式 "visible"），revealshow.rs 按探针指引删除（墓碑段注留回退路径）。验证：真机点一次卡片菜单应弹出带选中文件的资源管理器窗口。
- **手机端会话头部在任务运行场景挤乱（0.5.23 定稿）**：根因全在上游——子智能体药丸计数 label 是无类名裸 span（引用 `.count` 类但 CSS module key 表没有该键，**上游死引用**）被压时 CJK 逐字竖排；jobs/计划 root 无 `min-width:0` 整行溢出把药丸顶出屏；0.5.18 旧壳规则打穿上游 `@container` 窄屏隐藏（该旧规则已删）。**定稿对策 = 药丸区改横向滑动条**（用户裁定"写死不是办法"）：`headerActions` 容器 `overflow-x:auto` 收纳溢出，槽位贡献外层是 `display:contents` 包装，**结构锚 `> * > *` 穿透包装钉 `flex:none`**——药丸保持天然宽度永不压缩，上游未来新增药丸自动进条、壳零适配；标题 crumbs `flex:1 1 auto` + 下限 `min(76px,20vw)` 钉在条外恒可见；滚动条双隐藏 + `overscroll-behavior:contain`。耐久事实：**所有 `_headerActions` 选择器必须限定在 `header[class*="_header"]` 之下**（裸匹配会命中侧栏 `bhn1Oq_headerActions`）；属性要写到 display:contents 包装层下的直接子代（写在包装层是空操作）。契约退役 `aria-haspopup=tree` 探针，保留 `.count` 死引用哨兵盯上游修复。回归 `session_header_squeeze_rules` + `probe_remote_needles`。
- **dsh 自带插件管理页手机端头部挤乱（0.5.22 修）**：该页 `pageHead` 包内零窄屏适配——对策在 `mobile.css` ≤700px 五条：addButton `nowrap+flex:none`、toolbar `flex:none`、标题组 `min-width:0`、intro 单行+文案段省略号（完整说明在 ⓘ tooltip 里，截断不损功能）。锚 `data-plugin-panel`（upstream::PLUGIN_PANEL_HOOK 契约守门）；`_toolbar`/`_addButton`/`_pageIntro` 都不唯一，**必须带 `[data-plugin-panel] header[class*="_pageHead"]` 前缀限定**（回归 `plugin_panel_header_rules` 有撞名护栏）。
- **桌面端外链点击"没反应"= wry 默认吞新窗口请求（0.5.23 修）**：dsh 的 `openExternalLink` 终点是 `window.open(url,"_blank")` → WebView2 `NewWindowRequested` → 壳不挂 handler 时 wry 兜底 `SetHandled(true)` 静默吞掉。对策：主窗口 builder `.on_new_window`——`http/https` 经 `update::external_open_allowed`（javascript:/data:/about:/file:/自定义协议拒）+ `update::open_url`（rundll32）交系统浏览器，WebView2 子窗恒 `NewWindowResponse::Deny`；日志只记 host（URL 可能带 token 恒不落全文）。五个按需设置窗无外链不挂；手机远程走真浏览器不经此路径。
- **dsh 端口必须跨启动稳定，否则客户端偏好全丢**：上游把若干 UI 偏好只存浏览器 localStorage（`dsh-client-store` 的 `persist`，无服务端副本）——会话头部"打开方式"选择、会话宽度、当前会话、轨迹时长；localStorage 按 **origin（含端口）** 隔离，随机端口 = 每次全新源站 → 用户选的 VS Code 重启就变回文件资源管理器（0.5.14 修，实锤：leveldb 里同库并存 7 个源站）。对策 = port.rs 记忆端口：Ready 时写 `dsh-port.txt`，下次探活通过即复用（被占则换新端口并在 Ready 后改写记忆，一轮收敛；同一轮重试回避它）。
- **pnpm store 在 `F:\Code\pnpm-store`**（2026-09-21 从 `H:\.pnpm-store` 迁出；配置在 user 级 `%LOCALAPPDATA%\pnpm\config\config.yaml` 的 storeDir，`pnpm store path` 应回 `F:\Code\pnpm-store\v11`）。三条连带坑：①**跨盘 = copy 模式**——store 与项目不同盘无法硬链接，H: 项目的 node_modules 是真实副本，装得更慢更占空间；②**重装会删残 node_modules**——先删 `.pnpm` 再删顶层 symlink，此时链接已断删不动 → `ERR_PNPM_PACKAGE_MANAGER_REMOVE_MODULES_DIR … os error 5`；修法：MSYS `rm -rf node_modules/*`（走 lstat 能删断链）后 `pnpm install --frozen-lockfile`；③**别用 robocopy 搬 store**——不认硬链接，把共享块实体化成独立副本（实测 7GB 唯一数据 → 70GB）。
- **pnpm store 迁移会永久卡死插件安装（0.5.22 修）**：profile 的 node_modules 链接在**创建时**的 store（`.modules.yaml` 记录 storeDir），内置 pnpm 每次按**用户全局配置**解析 store——一经漂移，装/卸/更新全被 `ERR_PNPM_UNEXPECTED_STORE` 拒绝且重启无效。自愈 = `plugins.rs::heal_profile_store` 每次 spawn 前跑：storeDir 不一致则 rename node_modules 备用 → 内置 pnpm install 重链 → 成功删备份；失败/超时（5 分钟预算）回滚，现状不劣化。结果只落 events.log（`pnpm store heal: ...`）。上游若哪天自己处理 store 迁移则按惯例退役。

## 测试基线

`cargo test` 应全绿（当前 293 个，其中 3 条 ignored（补丁模块的 `apply_to_real_runtime` 开发辅助等），含 `tests/upstream_contract.rs` 对真实运行时的上游契约探测——跟版门禁：fetch 新版 dsh 后它红了就按输出改 `src/dsh/upstream.rs`）。`tests/console_window.rs` 的对照组会短暂弹出真实控制台窗口，属正常。改主题/进程/通知逻辑后，跑 `cargo test` + 重装走一遍 `acceptance.ps1`。

## 多平台预留

平台差异收口在 `platform/mod.rs` 的 Platform trait（可执行名、运行时目录、triplet、杀进程树、子进程配置、进程存活/镜像、系统深浅色）。CI matrix 的 macos/linux 行已注释，启用前需实现对应 platform 文件并让 fetch-runtime 支持对应 triplet；注意 process_alive 在桩平台恒 false。

## 已知限制

- Win10 深色标题栏聚焦时纯黑（系统行为，`DWMWA_CAPTION_COLOR` 仅 Win11）；恒为 dsh 深灰需无边框自绘标题栏——方案要点见 docs/design.zh-CN.md §8，暂缓。
- ~~检查更新在仓库私有期间必 404~~（0.5.3 已修：发布仓分担匿名可读）。
