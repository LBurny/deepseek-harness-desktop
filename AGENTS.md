# DSHDesktop

[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（dsh，DeepSeek agent harness CLI）的 Windows 桌面壳应用：Tauri 2 窗口内嵌 dsh 官方 Web UI，Node.js 与 dsh 随安装包分发、装完即用。

## 技术栈与形态

- **Tauri 2 + Rust**（`src-tauri/`）：进程监督、运行时部署、托盘、通知、主题跟随、诊断命令
- **Svelte 5 + TypeScript**（`src/`）：splash/diagnostics/plugins/skills/mcp/remote/settings 七个本地页面；主界面是导航到的远程 dsh Web UI（`http://127.0.0.1:<port>`）
- 安装包：NSIS（`pnpm tauri build`），单实例、托盘常驻、关窗默认隐藏到托盘（可在"其它设置"改为直接退出）

## 目录结构

```
src-tauri/src/
  lib.rs            Builder 组装（single_instance 插件必须最先）→ setup 代码创建主窗口
                    （visible(false)+center()，window-state 恢复记忆几何，flags 不含
                    VISIBLE 防托盘隐藏态被记住；on_page_load(Finished) 再 show 不闪变）
                    → 事件桥（dsh-ready→导航）
  download.rs       主窗口下载：目标改到系统下载目录+" (n)" 去重，toast+events.log；
                    缺它 wry 默认 handler 静默放行且抑制下载 UI，文件无声消失
  presets.rs        minimal 预设上游签名探测（只读）：rc ≤rc.7 曾需原地改写补丁，
                    rc.8 上游自修后补丁器退役，只读判定留作契约套件回归哨兵
  upstream.rs       dsh 上游内部事实单一来源（入口/命令形/WS 端点与帧/设置键/cordis
                    patch/预设签名/改写 needle，每条注明出处与影响面）；跟版红了只改
                    这一个文件；tests/upstream_contract.rs 对真实运行时逐项探测
                    （无运行时自动 skip，CI 里 fetch-runtime 在 cargo test 之前故必真跑），
                    DRIFT 输出直接指出改哪条常量、影响哪个模块
  platform/         平台抽象 trait（多平台预留）；windows.rs 实现（含 job 模块：全局
                    KILL_ON_JOB_CLOSE Job Object，register_child 挂每个子进程，
                    父进程被强杀时内核连带回收整树，防孤儿锁 runtime）
  process.rs        DshProcess 监督循环：spawn node bin.js web --port N --no-open
                    （rc.8 起 openBrowser 默认 true，不带则额外弹系统浏览器）；
                    子进程 PATH 前置内嵌 node 目录（npx/npm/node 绑定运行时版本，
                    MCP `npx` 命令不落系统旧 node——机器 B 实踩）+ profile 的
                    node_modules/.bin（插件自带 CLI 按名可解析）；指数退避、stop/restart
  runtime.rs        ensure_runtime：安装目录可写则原地运行内嵌运行时，只读则回退部署
                    副本（.version 比对）；原地模式清理旧版 %LOCALAPPDATA% 部署副本
  port.rs           free_port（OS 分配空闲端口，有竞态窗口需重试）+ wait_ready
                    （轮询 HTTP 直到任意响应或超时）
  i18n.rs           壳界面语言跟随 dsh locale.preference（zh/en，缺省按系统 UI 语言）；
                    文案经 pick(zh,en) 二选一；theme 关注循环写全局原子值，
                    深层辅助函数免层层透传 locale
  notify/           WS 事件源（ws.rs 泛化 {path, handler, on_connect}，连 events.mux +
                    events.host 双下行；空闲 Ping+Pong 超时看门狗防半开连接假死）
                    + 帧分类（approval/question、turn/start 清痕、tool/call 置痕、
                    turn/end 按"回合内是否干过活"拆任务完成/回答完成、session/title
                    台账、子代理经 origin 过滤）；sink 在 lib.rs：前台=任一窗口聚焦，
                    按 settings.notify 四类规则门控，全部通知统一挂提示音，
                    被抑制/弹失败都写 events.log
  theme.rs          标题栏主题跟随 settings.yaml 的 ui-theme.preference；首启播种；
                    主题变化时 SWP_FRAMECHANGED+RedrawWindow 强制非客户区重绘
                    （DwmSetWindowAttribute 只改属性不重绘，否则标题栏要等激活才换色）；
                    apply_before_show 在 show 前给新建窗口落 DWM 属性
  progress.rs       首启进度模型：阶段权重、百分比映射、结构化 dsh-progress 负载
  tray.rs           托盘菜单（打开/诊断/插件/技能/MCP/远程子菜单/重启/其它设置/退出）；
                    左键单击=打开主界面（show_menu_on_left_click(false)），菜单走右键；
                    六个按需窗口带 theme_bootstrap（按壳解析主题铺底 + 首帧前写死
                    data-theme/color-scheme，防"系统浅色+dsh 深色"下白底闪几秒）；
                    diagnostics.rs 状态/日志环形缓冲；commands.rs 8 个 invoke 命令
  zoom.rs           UI 缩放：hook_js 动态内嵌快捷键（on_page_load eval，只注入 main）、
                    direction 命令按设置读步进、ui-zoom.txt 持久化
  settings.rs       壳设置 settings.json 模型（步进 1-25%/快捷键/关窗行为/notify 四类
                    规则 {enabled,timing}（旧 notify_on_completion 读取时迁移）/提示音/
                    check_update_on_launch 默认关）、校验、落盘失败显式报错（不静默吞）
  skills.rs         skills/(启用) ↔ skills-disabled/(停用) 目录移动即开关（dsh watcher
                    热刷新）；启动自动种子 ~/.dsh/skills；三源导入(codex/claude/opencode)
                    +ZIP 导入（两种布局识别，剥前缀+enclosed_name 防穿越+条目/大小上限）
  mcp.rs            读写 cordis.patch.yml 中 dsh-mcp-client 的 insert 条目（其余条目
                    Value 级保留，tmp+rename 原子写，BOM 容忍）；启停=entry 上
                    disabled:true（cordis loader 原生，HMR 热生效）；启动种子
                    ~/.dsh 两层 patch；导入 claude/codex/opencode（sse 不支持跳过）
  plugins.rs        装/卸/更新走 dsh 官方 plugin 子命令（壳不自己写 profile）；
                    pnpm 壳内置（不能摊平；pnpm.cmd 包装——PATHEXT 只认 .cmd）；
                    无 shell + CREATE_NO_WINDOW + register_child；stdout/stderr 必须
                    显式 pipe（否则 output 恒空）；IPC 返回全部 serde camelCase 重命名
                    （漏 rename_all 则 exitCode 恒 undefined、成功被误报失败）；
                    清单读 profiles/web/package.json；串行锁防并发写；装完需重启生效
  picker.rs         目录选择器钉 browse：win32+回环时 dsh 决议为 native（系统对话框弹在
                    电脑屏幕，手机远程端不可见）；启动幂等写 cordis.patch.yml 官方
                    overlay，与 mcp.rs 同文件 Value 级共存，失败只记 events.log
  pickerpatch.rs    browse 选择器运行时补丁（签名门控+marker 幂等原地改写，dsh 自更新
                    还原后下次启动重打）：host 加 "dsh:drives" 哨兵层级（盘符根+"此电脑"
                    crumb）；client 隐藏条目默认显示、哨兵 crumb 居首/走 locale 文案/
                    禁"打开/新建文件夹"；客户端是安全前提，其签名漂移整组停手
  welcome.rs        内测声明豁免播种：从运行时 client.js 提取文案版本预写 settings.yaml
                    （Value 级改写，须在主题播种之后），桌面用户永不见对话框；
                    失败只记 events.log（回退为 dsh 原生弹一次）
  update.rs         检查更新：GitHub releases/latest API（必带 UA，走系统代理）、版本
                    比较、下载 *_x64-setup.exe（.part→rename，节流 emit 进度）；
                    install_update 起 NSIS 后 quit_app 自行退出（本进程先死，旧钩子
                    taskkill /T 杀树成空操作——否则安装器被连杀装不上）
  remote/           mod.rs=RemoteManager（生命周期/token/6 命令；reset_link 原地轮换
                    token，域名不变）；proxy.rs=axum token 门岗反向代理（覆盖全 Router
                    的中间件；HTTP 流式转发+WS 帧桥接；转发剥 origin/referer/sec-fetch-*
                    否则 dsh 403；.no_proxy() 防系统代理劫持回环；HTML 注入移动端适配：
                    mobile.css 700px 断点 + mobile.js "项目/信息"标签 + 回形针附件按钮；
                    /plugins/*/client.js 缓冲改写修远程内测声明重复弹）；project.rs=
                    手机端"项目"标签后端（自包含 project.html + resolve/list/file 四条
                    只读路由，canonicalize 前缀禁锢防逃逸/junction）；tunnel.rs=
                    cloudflared quick tunnel 监督（退避重启后域名变 token 不变）
src/                splash/diagnostics/settings/plugins/skills/mcp/remote 七个本地页面
                    + App.svelte(hash 路由) + i18n.ts
src-tauri/windows/  nsis-hooks.nsh：安装/卸载钩子；preinstall/preuninstall 先
                    taskkill /F /IM 杀主程序（绝不带 /T），再按路径清扫 $INSTDIR
                    残留进程（必须排除调用方自身父 PID——否则覆盖安装误杀原地运行的
                    旧卸载器，弹 "Unable to uninstall!"）；杀后轮询等退净；
                    postuninstall RMDir /r runtime 兜底清单外残留
scripts/            follow-upstream.ps1(一键跟版)、fetch-runtime.ps1(下载
                    Node+dsh+cloudflared+精简)、prune-runtime.ps1、
                    acceptance.ps1(端到端验收)、use-fixture-runtime.ps1、
                    check-node.ps1(查 dsh 进程/运行时目录)、gen-icon.mjs、
                    shot-window.ps1、
                    simulate-first-launch.ps1、hide-show-theme.ps1、get-attr20.ps1、
                    verify-*.ps1(zoom/window-state/no-size-flash/completion-notify/
                    titlebar-theme 回归)
docs/design.zh-CN.md                            设计文档（架构/模块/打包/测试/已知限制，先读它）
```

## 常用命令

```bash
# 开发（需要 fixture 运行时：先跑 scripts/use-fixture-runtime.ps1，再设 DSHDESKTOP_RUNTIME_DIR）
cd src-tauri && cargo test            # 全部测试（单元+进程集成+WS通知+控制台窗口+远程访问+上游契约）
pnpm tauri build                      # 产出 src-tauri/target/release/bundle/nsis/DSHDesktop_*_x64-setup.exe
powershell -File scripts/fetch-runtime.ps1   # 抓取真实运行时到 src-tauri/runtime/windows-x64/
powershell -File scripts/follow-upstream.ps1 -DshVersion <新版> [-Bump patch]   # 一键跟版：钉版→清旧→重抓→bump→cargo test→文档/CHANGELOG
powershell -File scripts/acceptance.ps1 -SetupExe <setup.exe>   # 卸载旧版→安装→启动→全项校验→截图
```

## 版本与发布

- **版本号进位规则（固定）**：每发一版 patch +1，patch 到 9 归零、minor +1——
  `0.4.0 → 0.4.1 → … → 0.4.9 → 0.5.0 → 0.5.1 → …`。每 10 个小版本进一位"大版本"，
  不按 semver 的 feature/breaking 语义跳版（0.x 阶段只数发版次数）。当前 0.4.0，下一版 0.4.1。
- **发版步骤**：bump 三处版本号（`package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml`）
  → CHANGELOG 把 Unreleased 收编进新版节 → 本地 `cargo test` + `pnpm tauri build` +
  `acceptance.ps1` 全过 → commit → `git tag v0.y.z` 推送 → `release.yml`（CI 跑测试门禁→
  构建→发 Release，资产 *_x64-setup.exe + sha256）。github 直连被拦时 push 走 §GitHub 访问的代理。
- CI 的 release 链路有**运行时缓存**（key=`rt-<dsh版本>-<fetch/prune脚本哈希>`）：dsh 版本
  不变则跳过 ~20 分钟的 npm install；想强制重拉就换 dsh 版本或改脚本（key 自动失效）。
  **缓存必须由 main 分支的 build.yml 预热**（Actions 缓存按 ref 隔离，tag run 存的缓存
  下一个 tag run 读不到——0.4.0→0.4.1 实测同 key 仍 miss、release 跑满 ~70 分钟）：
  tag 触发的 release run 只能读「当前 tag / 默认分支」的缓存，main 上的 build.yml 把
  回填存进默认分支作用域。所以发版注意：dsh 版本变了（key 变）时**先推 main 等
  build.yml 回填缓存，再打 tag**；dsh 版本没变的常规发版，main 的缓存是热的，一次
  push main+tag 即可命中。

## GitHub 访问

- 仓库 **已转私有**：`LBurny/deepseek-harness-desktop`（origin 指向它）
- 本机**没装 gh CLI**；访问 GitHub API 用环境变量 **`GH_TOKEN`**（属主 LBurny，已验证对私有仓库返回 200）：
  `curl -H "Authorization: Bearer $GH_TOKEN" -H "Accept: application/vnd.github+json" https://api.github.com/repos/LBurny/deepseek-harness-desktop`
- git fetch/push 走凭据管理器 `manager-core`，不受私有化影响
- **github.com 直连可能被 TLS 拦截**（0.3.0 后实踩）：push 报 `self signed certificate in
  certificate chain`（openssl）或 `SEC_E_UNTRUSTED_ROOT`（schannel），而 api.github.com
  正常——是网络层拦截不是配置问题，别改 sslBackend/别关 sslVerify。系统 Clash 代理
  127.0.0.1:7890 走 github.com 通畅，一次性绕法：
  `git -c http.proxy=http://127.0.0.1:7890 push`（**不写入 git 配置**，拦截消失后直连仍可用；
  先 `curl -sI -x http://127.0.0.1:7890 https://github.com` 确认代理在线）

## 关键约定与坑（细节见 docs/design.zh-CN.md）

- **set_autostart 不能无条件透传 disable()**：auto-launch 0.5 的 disable() 直接
  RegDeleteValueW，Run 值不存在时返回 ERROR_FILE_NOT_FOUND——从未开过自启动的用户
  每次保存设置都弹"系统找不到指定的文件 (os error 2)"。先 is_enabled() 比目标态，
  已达成即 Ok（commands.rs 有锚定测试）
- **dsh 事实**：Node `^22.19 || >=24`；入口 `lib/bin.js`；`dsh web` 只许绑 127.0.0.1 且 **rc.8 起默认把 UI 弹给系统默认浏览器**（spawn 必带 `--no-open`）；事件走 **WebSocket** `/api/events.mux` + `/api/events.host`（GET 返回 426），帧格式 `{"type":"server-request","method":<payload.type>,"payload":{...}}`；完成判定看 `session/event` 里的 `turn/end`（`data.reason.kind=="completed"`），子代理标记看 `host/session-added` 的 `origin`；设置在 `$DSH_HOME/settings.yaml` 的 `ui-theme.preference`（light/dark/system）；**npm 依赖是浮动区间**，跟版靠契约套件守门
- **运行时布局**：暂存 `src-tauri/runtime/<triplet>/`，tauri.conf `resources` 用映射形式
  `{ "runtime": "runtime", "resources/sounds": "sounds" }`，安装后落 `<install>/runtime/<triplet>/`
  与 `<install>/sounds/*.wav`（列表形式会错落到 `<install>/resources/sounds/` 致提示音探测不到，
  0.1.16 实踩；settings.rs 有锚定测试）；`bundle.resources` 相对路径映射（`..` 会变 `_up_`，别用）
- **子进程控制台**：`Platform::configure_child_command` 设 CREATE_NO_WINDOW；`kill_process_tree` 的 taskkill 同样必须带（GUI 主进程没有控制台，不带标志系统会为它新分配可见控制台窗口——退出/重启时闪 cmd）。复现"无控制台父进程"不能用 CREATE_NO_WINDOW 拉中间进程（那只是隐藏控制台，子孙会静默继承），须在中间进程里 FreeConsole()。验收判据是**可见 ConsoleWindowClass 窗口**（conhost 进程存在≠窗口可见）
- **PowerShell 5.1**：含中文的 .ps1 必须 UTF-8 **带 BOM**（注意 ZCode Edit 工具改完会丢 BOM，须补回）；别用 PS 改写 `settings.yaml`（会引入 BOM 导致 yaml-rust 解析失败，主题静默回退）
- **脚本里别用 Process.MainWindowHandle**：debug exe 还持有可见控制台与 Tao/托盘辅助窗口，句柄会指错；按 class "Tauri Window" 枚举进程顶层窗口（verify-no-size-flash.ps1 / verify-window-state.ps1 的 FindByClass 模式）
- **Tauri setup 无 tokio 上下文**：spawn_supervised 必须经 `tauri::async_runtime::block_on`
- **Tauri `resource_dir()` 返回 `\\?\` 扩展路径**：Node 加载器不认（EISDIR 崩溃），`runtime::strip_verbatim` 已处理，别绕过 ensure_runtime 自己拼路径
- **外部诊断手段**：`%LOCALAPPDATA%\DSHDesktop\events.log` 记录每个进程事件（1MB 截断），应用卡启动时先看它
- **fixture 用 .cjs**（根 package.json 是 type:module）；`#[tokio::test]` 涉及 std::thread::sleep 时须 `flavor="multi_thread"`。use-fixture-runtime.ps1 会在 @deepseek-ai/dsh 下铺 CJS 桩 package.json——fetch-runtime 抓过的树带真实 `"type":"module"`，不铺桩 mock bin.js 会按 ESM 加载崩溃
- **dev 模式 tauri 不拷贝 bundle.resources**：内置音效在 dev 下要手动复制到 `src-tauri/target/debug/sounds/`，否则静默降级系统默认；另外真实运行时放 src-tauri/runtime 下跑 dev 会被 dsh 自更新触发 watcher 重建循环——复制到 src-tauri 外用 DSHDESKTOP_RUNTIME_DIR 指向
- **NSIS 离线**：github 直连不稳时用 ghproxy.net 预置 `%LOCALAPPDATA%\tauri\NSIS`（含 nsis_tauri_utils.dll，SHA1 须匹配 bundler 常量）
- **托盘 quit 顺序**：先 stop dsh 等 1.5s 再 exit；杀子进程树用 `taskkill /T /F`
- **安装器只杀主程序**：Tauri NSIS 模板的 CheckIfAppIsRunning 仅 TerminateProcess 主 exe，
  关窗默认隐藏到托盘也挡不住强杀——子进程全靠 Job Object 随父死亡被内核回收，
  外加 nsis-hooks.nsh 安装/卸载前杀树+按路径清扫旧版孤儿；缺了这两层，运行中重装必现
  "Can't write: ...\cloudflared.exe"
- **按路径清扫必须排除调用方自身**：NSIS 钩子里 `$INSTDIR\*` 的路径匹配会把
  `_?=$INSTDIR` 原地运行的卸载器（$INSTDIR\uninstall.exe）自己也杀掉——卸载中途死透，
  新安装器 ExecWait 拿到非零退出码弹 "Unable to uninstall!" 并中止（0.1.9~0.1.12 实踩）。
  排除用 PowerShell 父进程 PID（nsExec 直接 CreateProcess），别用进程名硬编码。
  独立卸载（设置/开始菜单）自我复制到 %TEMP% 运行所以从不触发；**覆盖安装回归只能靠
  带 `_?=` 的原地调用测**
- **远程 IPC 放行**：dsh UI 是远程源，远程 IPC 一律走 ACL。build.rs 用 `AppManifest::commands` 声明全部 41 个命令（生成 `permissions/autogenerated/allow-*.toml`），`capabilities/dsh-remote.json` 只对 `http://127.0.0.1:*` 开放 `allow-zoom-ui`；副作用是本地命令也全部 ACL 化——**新增命令要同步三处**：build.rs、capabilities/default.json、按需 dsh-remote.json
- **缩放快捷键匹配**：主匹配 `e.code`，`e.key` 兜底（合成按键/RDP 注入 keydown 的 `e.code` 为空）；zoom_ui 负载是 `direction:"in"/"out"`，步进由命令读设置（不写死在脚本里）；改快捷键须重注入钩子（set_shell_settings 已做，热替换不叠加）
- **reqwest 在系统代理下会劫持 127.0.0.1**：用户开 Clash 等系统代理时 reqwest 默认走代理且不认 bypass 列表——凡访问本机回环（remote/proxy.rs 转发客户端、测试里访问 fixture/代理端口的客户端）必须 `.no_proxy()`，否则请求被代理软件接管表现为假 502/挂起
- **主窗口由 setup 代码创建（tauri.conf windows 为空）**：on_download 只能挂 WebviewWindowBuilder，conf 声明的窗口无法附加。建窗参数须与原 conf 一致（visible(false)+center()+min 900x600），window-state 对代码创建窗口同样在创建事件排队 restore（托盘按需窗口同款），回归靠 verify-no-size-flash/verify-window-state 两脚本
- **dsh 预设不能经 profile patch 影子覆盖**：composeProfile 会把 agent-presets 行的 roots 无条件重写为 shipped root（用户层 roots 被丢弃），且 shipped root 先于 $DSH_HOME/.agent-presets（同名 id shipped 优先）——若需重引入补丁，仍旧不能走 patch 影子覆盖，只能原地改写 shipped 预设文件
- **fs-local 列目录遇 ACL 拒绝项即整列失败**（如 C:\ 根目录撞上 DumpStack.log）：上游 dsh 行为，Windows 上列举系统盘根目录必现；壳侧缓解是让模型知道 cwd 并待在 workspace，别试图在壳里修列目录

## 测试基线

`cargo test` 应全绿（当前 201 个，含 `tests/upstream_contract.rs` 对真实运行时的上游契约探测——跟版门禁：fetch 新版 dsh 后它红了就按输出改 `src/upstream.rs`）。`tests/console_window.rs` 的对照组会在屏幕上短暂弹出真实控制台窗口，属正常。改主题/进程/通知逻辑后，跑 `cargo test` + 重装走一遍 `acceptance.ps1`。

## 多平台预留

平台差异都收口在 `platform/mod.rs` 的 `Platform` trait（节点可执行名、运行时目录、triplet、杀进程树、子进程配置、系统深浅色）。CI matrix 里 macos/linux 行已注释，启用前需实现对应 `platform/{macos,linux}.rs` 并在 fetch-runtime 支持对应 triplet。

## 已知限制

- Win10 深色标题栏聚焦时纯黑（系统行为，`DWMWA_CAPTION_COLOR` 仅 Win11）；要做成恒为 dsh 深灰需无边框自绘标题栏——方案要点见 docs/design.zh-CN.md §8，暂缓。
