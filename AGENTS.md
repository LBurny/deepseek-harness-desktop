# DSHDesktop

[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（dsh，DeepSeek agent harness CLI）的 Windows 桌面壳应用：Tauri 2 窗口内嵌 dsh 官方 Web UI，Node.js 与 dsh 随安装包分发、装完即用。

## 技术栈与形态

- **Tauri 2 + Rust**（`src-tauri/`）：进程监督、运行时部署、托盘、通知、主题跟随、诊断命令
- **Svelte 5 + TypeScript**（`src/`）：启动画面（splash）、诊断面板、其它设置三个本地页面；主界面是导航到的远程 dsh Web UI（`http://127.0.0.1:<port>`）
- 安装包：NSIS（`pnpm tauri build`），单实例、托盘常驻、关窗默认隐藏到托盘（可在"其它设置"改为直接退出）

## 目录结构

```
src-tauri/src/
  lib.rs            Builder 组装：插件(single_instance 必须最先；window-state 记忆窗口几何，
                    flags 不含 VISIBLE 防托盘隐藏态被记住；restore 在 window_created 排队、
                    早于首个可见帧落地，故主窗口 visible(false)+center() 创建
                    （首启居中，有记忆几何则被覆盖）、on_page_load(Finished) 再 show 不闪变)
                    → setup(代码创建主窗口，挂 on_download) → 事件桥(dsh-ready→导航)
  download.rs       主窗口下载处理：on_download 只能挂 builder（conf 窗口无法附加），
                    目标统一改到系统下载目录并 " (n)" 去重，完成/失败弹 toast + 记 events.log；
                    缺它时 wry 默认 handler 静默放行且抑制 WebView2 下载 UI，文件无声消失
  presets.rs        minimal(极简模式)预设的上游签名探测（只读）：rc ≤rc.7 它无条件
                    挂载 PTY 持久 bash 而终端检查器未实现 win32，本模块曾是启动期
                    原地改写补丁（tool-pwsh 变体、签名门控+marker 幂等）；rc.8 上游
                    自修（bash/pwsh 按 process.platform 互斥禁用 + koffi 版 win32
                    检查器），补丁器退役，只读判定 preset_signature_state 留给
                    契约套件当回归哨兵（上游回退修复→NeedsPatch→契约翻红）
  upstream.rs       dsh 上游内部事实单一来源（入口/命令形/WS 端点与帧/设置键/
                    cordis patch/预设签名/改写 needle，每条注明出处与影响面）；
                    跟版红了只改这一个文件；tests/upstream_contract.rs 对真实运行时
                    逐项探测（无运行时自动 skip，CI 里 fetch-runtime 在 cargo test
                    之前故必真跑），DRIFT 输出直接指出改哪条常量、影响哪个模块
  platform/         平台抽象 trait（多平台预留）；windows.rs 实现（含 job 模块：
                    全局 KILL_ON_JOB_CLOSE Job Object，register_child 把每个子进程挂进去，
                    父进程被强杀时内核连带回收整树，防孤儿锁 runtime）；macos/linux 待实现
  process.rs        DshProcess 监督循环：spawn node bin.js web --port N --no-open
                    （dsh-web-app rc.8 起 openBrowser 默认 true，不带则每次启动
                    额外弹系统浏览器）；PATH 前置 profile 的 node_modules/.bin
                    （插件自带 CLI 在会话终端/工具子进程按名可解析，否则装完
                    插件敲不到它的命令）、指数退避、stop/restart
  runtime.rs        ensure_runtime：安装目录可写则原地运行内嵌运行时；只读则回退部署副本
                    （.version 比对）；原地模式会清理旧版留下的 %LOCALAPPDATA% 部署副本
  notify/           WS 事件源（ws.rs 泛化 {path, handler, on_connect}，连 events.mux +
                    events.host 双下行）+ 帧分类（approval/question、turn/end 完成、
                    session/title 入 SessionBook 台账；子代理经 host 流 origin 过滤）；
                    sink 在 lib.rs：前台=任一窗口聚焦，按 settings.notify 三类规则门控
  theme.rs          标题栏主题跟随：轮询 dsh-home/settings.yaml 的 ui-theme.preference；
                    首启播种（settings.yaml 缺失时按系统深浅色预写 preference，dsh 缺省是浅色）；
                    主题变化时 SWP_FRAMECHANGED+RedrawWindow 强制非客户区重绘——
                    DwmSetWindowAttribute 只改属性不重绘，tao 的 WM_NCACTIVATE 伪造不可靠，
                    不强制重绘则标题栏要等用户点击（激活）才换色；轮询同值不重复强制（防闪）；
                    apply_before_show 在 on_page_load show 前给新建窗口落 DWM 属性
                    （tao 建窗按系统主题着色，系统浅色+dsh 深色时新窗口标题栏会带错色出生）
  progress.rs       首启进度模型：阶段权重、百分比映射、结构化 dsh-progress 负载
  tray.rs           系统托盘菜单（打开/诊断/插件/技能/MCP 管理/重启/其它设置/退出）；
                    左键单击托盘图标=打开主界面（show_menu_on_left_click(false)，
                    on_tray_icon_event Left/Up → show_main；窗口隐藏未销毁，位置不变），
                    菜单走右键；按需窗口 builder .center()（首启居中，记忆几何由 window-state 覆盖）；
                    六个按需窗口带 theme_bootstrap（background_color 按壳解析主题铺底
                    深#0f1115/浅#f5f6f8 + initialization_script 首帧前写死
                    data-theme/color-scheme——否则"系统浅色+dsh 深色"组合下
                    WebView2 白底与 app.css 浅色兜底分支会让页面先白几秒，
                    插件管理页实踩）；diagnostics.rs 状态/日志环形缓冲；
                    commands.rs 8 个 invoke 命令
  zoom.rs           UI 缩放：钩子脚本 hook_js(settings) 动态内嵌快捷键(on_page_load eval，
                    只注入 main 窗口)、direction 命令按设置读步进、ui-zoom.txt 持久化
  settings.rs       壳设置：settings.json 模型（步进 1-25%/快捷键/关窗行为/
                    notify 三类通知规则 {enabled,timing:background|always}（旧
                    notify_on_completion 读取时迁移进 turn_done）/提示音
                    silent|default|im|mail|reminder|sms|chime|drop|mellow/
                    check_update_on_launch 启动时检查更新（默认关））、校验、
                    落盘失败显式报错（不静默吞，防"保存成功但重启回退"）、
                    SettingsState、get/set_shell_settings + preview_completion_sound 命令
  skills.rs         技能管理：DSH_HOME(壳注入，非~/.dsh)的 skills/(启用) ↔
                    skills-disabled/(停用) 目录移动即开关（dsh watcher 热刷新）；
                    启动自动种子 ~/.dsh/skills(.skills-seeded marker 防复活)；
                    三源导入(codex/claude/opencode)+ZIP 本地导入
                    (inspect_zip_skills 识别包根 SKILL.md/顶层技能文件夹两种布局,
                    解包剥前缀+enclosed_name 防穿越+1万条目/256MB 上限;
                    前端经 tauri-plugin-dialog 选文件)、冲突覆盖/跳过、删除；
                    7 个命令 + SkillsHome 状态
  mcp.rs            MCP 管理：读写 dsh-home/profiles/web/cordis.patch.yml 中
                    name=='@deepseek-ai/dsh-mcp-client' 的 insert 条目（其余条目
                    Value 级保留，tmp+rename 原子写，BOM 容忍）；启停=entry 上
                    disabled:true（cordis loader 原生，HMR 热生效无需重启）；
                    编辑保留 toolCallTimeoutMs/reconnect 等高级键；启动种子
                    ~/.dsh 两层 patch（.mcp-seeded marker 防复活，源里 disabled
                    的不同步）；导入 claude(.claude.json)/codex(config.toml)/
                    opencode(opencode.json)，sse 不支持标记跳过；
                    6 个命令 + McpHome 状态
  plugins.rs        插件管理：装/卸/更新走 dsh 官方 plugin 子命令（node bin.js
                    plugin --profile web <pnpm args>，profile 初始化与
                    dsh.profile.bundles 对账归上游，壳不自己写）；pnpm 壳内置
                    （fetch-runtime 下 tarball 整包留 <runtime>/pnpm/，
                    bin/pnpm.cjs→./pnpm.mjs→../dist/pnpm.mjs 不能摊平，~14MB；
                    pnpm.cmd 包装——Windows PATHEXT 只认 .cmd，dsh 内部
                    spawnSync("pnpm") 带 shell:true 经 cmd 解析）；spawn 时
                    PATH 前置 runtime 目录；无 shell + CREATE_NO_WINDOW +
                    register_child 挂 Job Object；stdout/stderr 必须显式
                    pipe（tokio spawn 默认继承父进程 stdio，wait_with_output
                    只读管道句柄，不接则 output 恒空、"看下方输出"永远没
                    内容）；IPC 返回结构体全部 serde camelCase 重命名
                    （前端按 camelCase 读键，漏 rename_all 则 exitCode 恒
                    undefined、成功被误报失败；锚定测试守门）；清单读
                    profiles/web/package.json（dependencies + dsh.profile.bundles，BOM
                    容忍）；npm 搜索走 registry API（外网走系统代理）；
                    串行锁防并发写 profile；装完需重启 dsh 生效（无 HMR，
                    面板一键重启复用 restart_dsh）；6 命令 + PluginsHome 状态
  picker.rs         目录选择器钉 browse：dsh 的 directory-picker-auto 在
                    win32+回环决议为 native（Win32 系统对话框弹在电脑屏幕上，
                    手机远程端不可见，新建项目选不了文件夹）；启动时幂等写
                    profiles/web/cordis.patch.yml（禁用 directory-picker
                    auto 行 + insert browse 对，上游官方 overlay 形态），
                    与 mcp.rs 同文件 Value 级共存（条目互不命中），spawn 前
                    完成，失败只记 events.log；桌面端同步变网页版对话框
                    （功能不减：浏览全盘/新建文件夹/手输路径）
  pickerpatch.rs    browse 选择器运行时补丁（签名门控+marker 幂等原地改写，
                    dsh 自更新还原后下次启动重打；presets.rs 补丁器退役后
                    本模块是该模式的唯一使用者）：
                    host index.js 加 "dsh:drives" 哨兵层级（list 特判返回
                    A-Z 可用盘符根，ancestryCrumbs 对盘符根前插"此电脑"
                    crumb——否则面包屑在 home 子树内被折叠成"主页"，手机端
                    爬不到其它盘）；client.js 隐藏条目默认显示（含开框重置）、
                    displayCrumbs 折叠时保留哨兵 crumb 居首、哨兵 crumb 走
                    locale 文案（browser.drives 此电脑/This PC）、哨兵层级
                    禁用"打开/新建文件夹"（防把哨兵选成工作区）；客户端是
                    安全前提，其签名漂移/缺失时整组停手（只改 host 会放出
                    半成品）；needle 与文件路径收口 upstream.rs，契约套件守门
  welcome.rs        内测声明豁免播种：dsh 的 welcome notice（"内测声明"对话框）
                    在 settings.yaml 的 ui-onboarding.welcomeNoticeVersion ≠ 当前
                    文案版本时每次启动弹窗；启动时从运行时 client.js 提取文案版本
                    预写进 settings.yaml（Value 级改写其余键不动，BOM 容忍，
                    tmp+rename 原子写，须在主题播种之后——它只在文件缺失时写），
                    桌面用户永不见对话框；needle 由契约套件守门，失败只记
                    events.log 不阻断启动（回退为 dsh 原生弹一次）
  update.rs         检查更新：GitHub releases/latest API（必带 UA；走系统代理——
                    外网场景，与回环必须 no_proxy 相反）、版本比较（自实现，
                    解析失败按非新版）、下载 *_x64-setup.exe 到系统下载目录
                    （.part→rename，百分比节流 emit update-download-progress）、
                    install_update 起 NSIS 后走 quit_app 自行退出——安装器是
                    本进程子进程，旧版钩子的 taskkill /T 会连它一起杀（立即安装
                    曾因此装不上）；本进程先死，钩子杀树即成空操作）、
                    open_update_page 用 rundll32 开 releases 页（不引 opener 插件）；
                    启动时检查默认关，有新版弹 toast，失败只记 events.log；4 命令
  remote/           远程访问：mod.rs=RemoteManager(生命周期/token/6 命令；reset_link
                    原地轮换 token 吊销泄露链接，域名不变) +
                    proxy.rs(axum token 门岗反向代理，cookie 种发，HTTP 流式转发
                    + WS 帧桥接；token 存 RwLock 共享单元门岗逐请求读最新值，
                    桥接挂 drain Notify，重置/停服 notify_waiters 掐断所有已建立连接；
                    转发必须剥 origin/referer/sec-fetch-* 浏览器标记头
                    （dsh /api 信任栅栏：Origin.host≠Host 头或 cross-site → 403）；
                    转发客户端必须 .no_proxy() 防系统代理劫持 127.0.0.1；
                    HTML 文档注入移动端适配（accept 含 text/html 且 identity ≤4MB）：
                    mobile.css 700px 断点，设置弹窗全屏+横向 tab、侧栏抽屉化、
                    模型选择器图标化、触发器菜单包含块修复（position:static 上移到
                    输入卡片）、目录浏览对话框近全屏且 footer 三控件一行均分
                    （"显示隐藏文件"开关随 pickerpatch 默认显示而移除）；
                    mobile.js 在"对话/轨迹"旁加"信息"标签页——统计行
                    克隆进面板（MutationObserver 同步，克隆而非搬家：React 对被移
                    节点 removeChild 必崩），enhanced 标记隐藏原行且跟随
                    matchMedia 断点（离开 700px 摘除，防宽屏统计无处可见）；
                    另在输入卡片工具行"+"旁注入回形针附件按钮（类名克隆同款
                    28px 圆形）：点击调起系统文件选择器（仅 png/jpeg/webp/gif——
                    上游 host sharp 校验只认四种位图，文档不支持），选完构造
                    DataTransfer 合成 paste 事件喂回上游粘贴管线（intakeImages
                    校验/报错 toast 全复用）；file input 惰性创建放 body
                    （注入点在 </head> 前，执行时 body 可能未出来，且须在
                    React 树外），按钮被重渲染抹掉时观察器按 DOM 缺席重挂；
                    JS 失效时 CSS 两行换行兜底。选择器锚 role/data-* 语义钩子
                    +CSS Modules 本地名子串（[class*="_nav"]），上游改名静默失效；
                    /plugins/*/client.js 响应缓冲改写（≤4MB 仅 identity，剥
                    accept-encoding 与条件请求头）：isLoopback 三元式→"host"，
                    修远程每次弹内测声明（非回环源 memory 持久化不落盘）) +
                    tunnel.rs(cloudflared quick tunnel 监督，stdout 解析
                    trycloudflare URL，退避重启后域名变 token 不变)；
                    托盘子菜单开关/复制/二维码/重置(#/remote 窗口)
src/                splash/Splash.svelte、diagnostics/Diagnostics.svelte、
                    settings/Settings.svelte、skills/Skills.svelte、
                    mcp/Mcp.svelte、remote/Remote.svelte、App.svelte(hash 路由)
src-tauri/windows/  nsis-hooks.nsh：NSIS 安装/卸载钩子（bundle.windows.nsis.installerHooks
                    接入，路径相对 src-tauri）；preinstall/preuninstall 先
                    taskkill /F /IM 杀主程序（绝不带 /T：安装器/旧卸载器可能
                    在树上被误杀；子进程回收靠 Job Object），再按可执行路径清扫
                    $INSTDIR 下残留进程（≤0.1.8 遗留孤儿）——清扫必须排除调用方自身
                    （父进程 PID）：覆盖安装时模板以 `_?=$INSTDIR` 原地运行旧卸载器，
                    其路径同样匹配 $INSTDIR\*，0.1.9~0.1.12 会误杀卸载器自身致
                    "Unable to uninstall!"（开始菜单/设置卸载走 %TEMP% 副本，不受影响）；
                    杀后轮询等退净（≤10s）；postuninstall 再 RMDir /r runtime 兜底
                    清单外残留（dsh 自更新新增的文件）
scripts/            fetch-runtime.ps1(下载 Node+dsh+cloudflared+精简)、prune-runtime.ps1(精简运行时)、
                    acceptance.ps1(端到端验收)、shot-window.ps1(窗口截图)、
                    hide-show-theme.ps1(托盘隐藏回归)、get-attr20.ps1(读 DWM 深色属性)、
                    simulate-first-launch.ps1(模拟首启并截图)、verify-zoom.ps1(UI 缩放目验，需先 pnpm dev)、
                    verify-window-state.ps1(窗口几何记忆回归：调尺寸→退出→重启→断言恢复)、
                    verify-no-size-flash.ps1(尺寸闪变回归：预写状态文件→断言首个可见帧即记忆几何)、
                    verify-completion-notify.ps1(完成通知回归：fixture 运行时+隐藏窗口→断言
                    events.log 出现 Notify: TurnCompleted)、use-fixture-runtime.ps1、
                    verify-titlebar-theme.ps1(标题栏主题即时重绘回归：写 settings.yaml 切主题→
                    不点击窗口断言标题栏像素即时换色，需先 pnpm dev)、gen-icon.mjs
docs/design.zh-CN.md / design.md                  设计文档（架构/模块/打包/测试/已知限制，先读它）
```

## 常用命令

```bash
# 开发（需要 fixture 运行时：先跑 scripts/use-fixture-runtime.ps1，再设 DSHDESKTOP_RUNTIME_DIR）
cd src-tauri && cargo test            # 全部测试（188 个：单元+进程集成+WS通知+控制台窗口+远程访问+上游契约）
pnpm tauri build                      # 产出 src-tauri/target/release/bundle/nsis/DSHDesktop_*_x64-setup.exe
powershell -File scripts/fetch-runtime.ps1   # 抓取真实运行时到 src-tauri/runtime/windows-x64/
powershell -File scripts/acceptance.ps1 -SetupExe <setup.exe>   # 卸载旧版→安装→启动→全项校验→截图
```

## 关键约定与坑（细节见 docs/design.zh-CN.md）

- **set_autostart 不能无条件透传 disable()**：auto-launch 0.5 的 disable() 直接
  RegDeleteValueW，Run 值不存在时返回 ERROR_FILE_NOT_FOUND——从未开过自启动的用户
  每次保存设置都弹"系统找不到指定的文件 (os error 2)"。先 is_enabled() 比目标态，
  已达成即 Ok（commands.rs 有锚定测试）

- **dsh 事实**：Node `^22.19 || >=24`；入口 `lib/bin.js`；`dsh web` 只许绑 127.0.0.1 且 **dsh-web-app rc.8 起默认把 UI 弹给系统默认浏览器**（spawn 必带 `--no-open`，壳内嵌 WebView 就是浏览器）；事件走 **WebSocket** `/api/events.mux` + `/api/events.host`（GET 返回 426），帧格式 `{"type":"server-request","method":<payload.type>,"payload":{...}}`；完成判定看 `session/event` 里的 `turn/end`（`data.reason.kind=="completed"`），子代理标记看 `host/session-added` 的 `origin`；设置在 `$DSH_HOME/settings.yaml` 的 `ui-theme.preference`（light/dark/system）；**npm 依赖是浮动区间**（dsh 根包 rc 固定、子包解析到最新 rc），跟版靠契约套件守门
- **运行时布局**：暂存 `src-tauri/runtime/<triplet>/`，tauri.conf `resources` 用映射形式
  `{ "runtime": "runtime", "resources/sounds": "sounds" }`，安装后 `<install>/runtime/<triplet>/`
  与 `<install>/sounds/*.wav`（自定义提示音在 exe 旁/资源根找 `sounds/`，映射让它们落对位——
  列表形式会把 `resources/sounds` 原样放 `<install>/resources/sounds/`，探测不到即降级系统默认，
  0.1.16 实踩；`settings.rs` 有锚定测试）；`bundle.resources` 相对路径映射（`..` 会变 `_up_`，别用）
- **子进程控制台**：`Platform::configure_child_command` 设 CREATE_NO_WINDOW；`kill_process_tree` 的 taskkill 同样必须带（它是控制台子系统程序，GUI 主进程没有控制台，不带标志系统会为它新分配可见控制台窗口——退出/重启时闪 cmd）。复现"无控制台父进程"不能用 CREATE_NO_WINDOW 拉中间进程（那只是**隐藏**控制台，子孙会静默继承、不产生新窗口），须在中间进程里 FreeConsole()。验收判据是**可见 ConsoleWindowClass 窗口**（conhost 进程存在≠窗口可见）
- **PowerShell 5.1**：含中文的 .ps1 必须 UTF-8 **带 BOM**（注意 ZCode Edit 工具改完会丢 BOM，须补回）；别用 PS 改写 `settings.yaml`（会引入 BOM 导致 yaml-rust 解析失败，主题静默回退）
- **脚本里别用 Process.MainWindowHandle**：debug exe 还持有可见控制台与 Tao/托盘辅助窗口，句柄会指错；按 class "Tauri Window" 枚举进程顶层窗口（verify-no-size-flash.ps1 / verify-window-state.ps1 的 FindByClass 模式）
- **Tauri setup 无 tokio 上下文**：spawn_supervised 必须经 `tauri::async_runtime::block_on`
- **Tauri `resource_dir()` 返回 `\\?\` 扩展路径**：Node 加载器不认（EISDIR 崩溃），`runtime::strip_verbatim` 已处理，别绕过 ensure_runtime 自己拼路径
- **外部诊断手段**：`%LOCALAPPDATA%\DSHDesktop\events.log` 记录每个进程事件（1MB 截断），应用卡启动时先看它
- **fixture 用 .cjs**（根 package.json 是 type:module）；`#[tokio::test]` 涉及 std::thread::sleep 时须 `flavor="multi_thread"`。use-fixture-runtime.ps1 会在 @deepseek-ai/dsh 下铺 CJS 桩 package.json——fetch-runtime 抓过的树带真实 `"type":"module"`，不铺桩 mock bin.js 会按 ESM 加载崩溃
- **dev 模式 tauri 不拷贝 bundle.resources**：内置音效（resources/sounds/*.wav）在 dev 下要手动复制到 `src-tauri/target/debug/sounds/`，否则自定义提示音静默降级为系统默认；生产包由 bundle.resources 正常打进安装目录。另外真实运行时放 src-tauri/runtime 下跑 dev 会被 dsh 自更新（package-lock/node_modules 变动）触发 watcher 重建循环——复制到 src-tauri 外用 DSHDESKTOP_RUNTIME_DIR 指向
- **NSIS 离线**：github 直连不稳时用 ghproxy.net 预置 `%LOCALAPPDATA%\tauri\NSIS`（含 nsis_tauri_utils.dll，SHA1 须匹配 bundler 常量）
- **托盘 quit 顺序**：先 stop dsh 等 1.5s 再 exit；杀子进程树用 `taskkill /T /F`
- **安装器只杀主程序**：Tauri NSIS 模板的 CheckIfAppIsRunning 仅 TerminateProcess 主 exe，
  关窗默认隐藏到托盘也挡不住强杀——子进程全靠 Job Object（register_child）随父死亡被内核回收，
  外加 nsis-hooks.nsh 在安装/卸载前杀树+按路径清扫旧版孤儿；缺了这两层，运行中重装必现
  "Can't write: ...\cloudflared.exe"
- **按路径清扫必须排除调用方自身**：NSIS 钩子里 `$INSTDIR\*` 的路径匹配会把
  `_?=$INSTDIR` 原地运行的卸载器（$INSTDIR\uninstall.exe）自己也杀掉——卸载中途死透、
  文件没删，新安装器 ExecWait 拿到非零退出码弹 "Unable to uninstall!" 并中止
  （0.1.9~0.1.12 实踩）。排除用 PowerShell 父进程 PID（nsExec 直接 CreateProcess），
  别用进程名硬编码。独立卸载（设置/开始菜单）自我复制到 %TEMP% 运行所以从不触发；
  acceptance.ps1 的卸载步骤也是这条路——**覆盖安装回归只能靠带 `_?=` 的原地调用测**
- **≤0.1.12 升级到 ≥0.1.13 会最后一次弹 "Unable to uninstall!"**：旧卸载器的自杀钩子
  无法由新安装器修复（模板在 PageLeaveReinstall 前没有任何钩子点），用户点确定后到
  系统设置里卸载 DSHDesktop（独立卸载走 %TEMP% 副本，正常），再重跑新安装包
- **远程 IPC 放行**：dsh UI 是远程源，远程 IPC 一律走 ACL。build.rs 用 `AppManifest::commands` 声明全部 41 个命令（生成 `permissions/autogenerated/allow-*.toml`），`capabilities/dsh-remote.json` 只对 `http://127.0.0.1:*` 开放 `allow-zoom-ui`；副作用是本地命令也全部 ACL 化——**新增命令要同步三处**：build.rs、capabilities/default.json、按需 dsh-remote.json
- **缩放快捷键匹配**：主匹配 `e.code`，`e.key` 兜底（合成按键/RDP 注入 keydown 的 `e.code` 为空）；zoom_ui 负载是 `direction:"in"/"out"`，步进由命令读设置（不写死在脚本里）；改快捷键须重注入钩子（set_shell_settings 已做，热替换不叠加）
- **reqwest 在系统代理下会劫持 127.0.0.1**：用户开 Clash 等系统代理时 reqwest 默认走代理且不认 bypass 列表——凡访问本机回环（remote/proxy.rs 转发客户端、测试里访问 fixture/代理端口的客户端）必须 `.no_proxy()`，否则请求被代理软件接管表现为假 502/挂起
- **主窗口由 setup 代码创建（tauri.conf windows 为空）**：on_download 只能挂 WebviewWindowBuilder，conf 声明的窗口无法附加。建窗参数须与原 conf 一致（visible(false)+center()+min 900x600），window-state 对代码创建窗口同样在创建事件排队 restore（托盘按需窗口同款），回归靠 verify-no-size-flash/verify-window-state 两脚本
- **dsh 预设不能经 profile patch 影子覆盖**：composeProfile 会把 agent-presets 行的 roots 无条件重写为 shipped root（用户层的 roots 被丢弃），且 shipped root 先于 $DSH_HOME/.agent-presets（同名 id shipped 优先）——这是当年 presets.rs 只能原地改写 shipped 预设文件的原因；rc.8 上游自修 win32 后补丁器已退役，但该上游事实仍成立（若需重引入补丁，仍旧不能走 patch 影子覆盖）
- **fs-local 列目录遇 ACL 拒绝项即整列失败**（如 C:\ 根目录撞上 DumpStack.log）：上游 dsh 行为，Windows 上列举系统盘根目录必现；壳侧缓解是让模型知道 cwd 并待在 workspace（rc.8 起极简模式由上游 persistent-pwsh 的工具描述承担平台指引），别试图在壳里修列目录

## 测试基线

`cargo test` 应全绿（当前 188 个，含 `tests/upstream_contract.rs` 对真实运行时的上游契约探测——跟版门禁：fetch 新版 dsh 后它红了就按输出改 `src/upstream.rs`）。`tests/console_window.rs` 的对照组会在屏幕上短暂弹出真实控制台窗口，属正常。改主题/进程/通知逻辑后，跑 `cargo test` + 重装走一遍 `acceptance.ps1`。

## 多平台预留

平台差异都收口在 `platform/mod.rs` 的 `Platform` trait（节点可执行名、运行时目录、triplet、杀进程树、子进程配置、系统深浅色）。CI matrix 里 macos/linux 行已注释，启用前需实现对应 `platform/{macos,linux}.rs` 并在 fetch-runtime 支持对应 triplet。

## 已知限制

- Win10 深色标题栏聚焦时纯黑（系统行为，`DWMWA_CAPTION_COLOR` 仅 Win11）；要做成恒为 dsh 深灰需无边框自绘标题栏——方案要点见 docs/design.zh-CN.md §8，暂缓。
