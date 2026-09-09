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
                    → 事件桥（dsh-ready→导航：导航前清陈年 dsh-auth cookie 防 431、
                    导航后 30s UI 心跳看门狗自愈一次，均见关键约定坑）；run() 最顶部
                    debug-cdp marker → WebView2 CDP :9222 诊断开关
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
  process.rs        DshProcess 监督循环：spawn node --max-http-header-size=65536
                    bin.js web --port N --no-open
                    （rc.8 起 openBrowser 默认 true，不带则额外弹系统浏览器；
                    请求头上限 16KB→64KB——431 纵深防御，兼护用户浏览器直连
                    dsh 端口攒 cookie 的场景）；
                    0.1.2 起 stdout 就绪行是 launch token 唯一来源（pump 先捕获
                    再脱敏转发，Ready 门控保证 token 必在）；wait_token 是静默
                    超时（pump 每行刷新活动时间，npm 冷装警告行切 10min 长预算，
                    absolute 10min 封顶——固定 60s 会把快装完的进程杀树白等一轮）；
                    Ready 前落耗时分解行 `[dshdesktop] ready: port=N total=Xs
                    http=Ys token=Zs`（诊断面板"上次启动"数据源）；
                    子进程 PATH 前置内嵌 node 目录（npx/npm/node 绑定运行时版本，
                    MCP `npx` 命令不落系统旧 node——机器 B 实踩）+ profile 的
                    node_modules/.bin（插件自带 CLI 按名可解析）；指数退避、stop/restart
                    子进程 PATH 前置内嵌 node 目录（npx/npm/node 绑定运行时版本，
                    MCP `npx` 命令不落系统旧 node——机器 B 实踩）+ profile 的
                    node_modules/.bin（插件自带 CLI 按名可解析）；指数退避、stop/restart
  dsh_session.rs    0.1.2 BrowserAuth 凭证模块：launch token 解析（parse_ready_line）+
                    token 换 cookie（exchange_cookie，303 + Set-Cookie dsh-auth-*，
                    绑 127.0.0.1:<port> authority——换端口即失效要重换）；凭证只在内存、
                    日志脱敏（token 经 remote::redact_token，cookie 不记值）
  runtime.rs        ensure_runtime：安装目录可写则原地运行内嵌运行时，只读则回退部署
                    副本（.version 比对）；原地模式清理旧版 %LOCALAPPDATA% 部署副本
  port.rs           free_port（OS 分配空闲端口，有竞态窗口需重试）+ wait_ready
                    （轮询 HTTP 直到任意响应或超时）
  i18n.rs           壳界面语言跟随 dsh locale.preference（zh/en，缺省按系统 UI 语言）；
                    文案经 pick(zh,en) 二选一；theme 关注循环写全局原子值，
                    深层辅助函数免层层透传 locale
  pagebridge.rs     主窗口页面观测桥（0.5.11）：INIT_SCRIPT 经 initialization_script
                    document-start 注入（每次导航都跑、先于页面脚本、绕 CSP）——error
                    捕获相/unhandledrejection/console.error → report_page_error 落
                    events.log [page:<kind>] 行（60 行/分钟限流+800 字符截断+
                    redact_token 脱敏）；#root 挂载心跳（仅 127.0.0.1 源）→ ui_boot_ok，
                    驱动 lib.rs 自愈看门狗；两条命令只写日志（不读数据不动作），是
                    dsh-remote.json 放行它们的安全前提
  notify/           事件源 mux.rs（0.1.2 单 WS /api/remote.mux 承载 $events 事件桥 +
                    N 条 session/follow；cookie 鉴权、每次重连现换；空闲 Ping+Pong
                    超时看门狗防半开连接假死；follow 集重连重播种 fail-open）
                    + 帧分类（$events: api-session/added 驱动逐会话 follow、
                    api-session/removed 摘除、approval/request 与 user-questions/
                    request waterfall 只弹通知**严禁回包 $events/result**；
                    follow 流: turn/start 清痕、tool/call 置痕、turn/end 按"回合内
                    是否干过活"拆任务完成/回答完成、session/title 台账、子代理经
                    origin 过滤）；sink 在 lib.rs：前台=任一窗口聚焦，
                    按 settings.notify 四类规则门控，全部通知统一挂提示音，
                    被抑制/弹失败都写 events.log；toast.rs=WinRT 直连 toast（顶部行
                    小图标经 AUMID IconUri 指向随包 256px PNG，XML 不含 image），
                    点击走协议激活回主窗口（启动时注册 dshdesktop:// + AUMID 显示名
                    /IconUri；single-instance 回调统一走 show_main→platform
                    bring_to_front 两段择时+AttachThreadInput+ASFW_ANY 授权）
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
                    check_update_on_launch 默认开）、校验、落盘失败显式报错（不静默吞）
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
  preseed.rs        预安装插件播种（resources/preseed-plugins/ 随包分发，安装后落
                    <install>/preseed-plugins/）：首启同步文件到 $DSH_HOME/profiles/plugins/
                    <name> 再走官方 dsh plugin add；插件必须是 bundle 形态（package.json
                    声明 dsh.bundle.patch 自我挂载 insert）——reconcile 才会自动挂层/摘层，
                    用户在插件面板删除后无残留；marker .plugins-preseeded 记录种过的名字，
                    依赖消失而 marker 在 = 用户删除 → 不复活（语义同 skills 种子）；
                    壳升级时文件有变化则覆盖同步；dev 下 resources 不拷贝 = 静默无操作
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
  remote/           mod.rs=RemoteManager（会话持久化生命周期/token/6 命令：start=
                    全新会话新 token；resume_or_start=启动复活——收养存活隧道+同端口
                    重起代理，链接字节级不变，校验不过回退全新开（域名换、token 沿用，
                    只有手动关闭重开/重置才换 token）；suspend_for_exit=应用退出只
                    死代理、隧道留活；reset_link 原地轮换 token 同步状态文件，域名
                    不变）；session.rs=remote-session.json 状态落盘（token/域名/端口/
                    PID/副本路径；含 token 敏感凭据，绝不落 events.log）+ 常驻副本
                    ensure_tunnel_copy（尺寸不符才刷新）；proxy.rs=axum token 门岗反向代理（覆盖全 Router
                    的中间件；HTTP 流式转发+WS 帧桥接；转发剥 origin/referer/sec-fetch-*
                    否则 dsh 403；.no_proxy() 防系统代理劫持回环；门岗 cookie 30 天
                    长效（Max-Age=2592000——会话 cookie 会被手机浏览器进程回收丢弃，
                    地址栏已被 302 剥掉 token，一丢即 403 假"失效"，0.5.7 起长效化）；
                    HTML 注入移动端适配：
                    mobile.css 700px 断点 + mobile.js "项目/信息"标签 + 回形针附件按钮；
                    HTML 挂载点后注入 splash.css/splash.js 加载过渡页（#root 出现子
                    节点即淡出，挂载点 needle 收 upstream.rs，miss 则整体不注入）；
                    ≥4KB 文本资产（js/css/json/svg/html，含 bundle 改写产物）代理侧
                    缓冲 gzip——dsh 不压缩任何响应，隧道首连 ~5MB→~1/3；
                    /plugins/*/client.js 缓冲改写修远程内测声明重复弹）；project.rs=
                    手机端"项目"标签后端（自包含 project.html + resolve/list/file 四条
                    只读路由，canonicalize 前缀禁锢防逃逸/junction）；tunnel.rs=
                    cloudflared quick tunnel 监督（退避重启后域名变 token 不变；
                    persistent 常驻模式：从数据目录副本 tunnel/cloudflared.exe 运行、
                    **刻意不挂 Job Object**——防孤儿原则唯一例外，死了内核不连带回收；
                    adopt() 按 PID 收养上个会话遗留隧道+3s 轮询看门狗，死后衔接
                    监督循环退避重生换新域名）
src/                splash/diagnostics/settings/plugins/skills/mcp/remote 七个本地页面
                    + App.svelte(hash 路由) + i18n.ts
src-tauri/windows/  nsis-hooks.nsh：安装/卸载钩子；preinstall/preuninstall 先
                    taskkill /F /IM 杀主程序（绝不带 /T），再按路径清扫 $INSTDIR
                    残留进程（必须排除调用方自身父 PID——否则覆盖安装误杀原地运行的
                    旧卸载器，弹 "Unable to uninstall!"）；杀后轮询等退净 + **等三个
                    exe（主程序/runtime 的 node/cloudflared）文件锁释放**（0.5.10，
                    进程死亡≠锁释放，Defender/PCA 持柄 1~3s，Delete/File 撞锁静默
                    失败或 "Can't write"）；preinstall 再 RMDir /r runtime（/UPDATE
                    覆盖安装不经过旧卸载器，旧 runtime 树没人清）；postuninstall
                    RMDir /r runtime 兜底清单外残留 + **$UpdateMode<>1 且父进程不是
                    新安装器（DSHDesktop_*_x64-setup.exe）才清扫常驻隧道副本与
                    remote-session.json**（/UPDATE=覆盖安装、_?= 原地卸载=手动升级，
                    杀了则更新后链接失效，违背会话持久化语义；真卸载父进程链已死或
                    explorer，照常清理）
scripts/            follow-upstream.ps1(一键跟版)、fetch-runtime.ps1(下载
                    Node+dsh+cloudflared+精简)、prune-runtime.ps1、
                    acceptance.ps1(端到端验收)、release-local.ps1(本地发版直接
                    上传 GitHub Release)、use-fixture-runtime.ps1、
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
  不按 semver 的 feature/breaking 语义跳版（0.x 阶段只数发版次数）。当前 0.5.11，下一版 0.5.12。
- **发版步骤（0.4.9 起本地发布，弃用 CI release；0.5.3 起公开仓分发，2026-09-04 起 release-local.ps1 双仓上传）**：
  bump 三处版本号（`package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml`）→
  CHANGELOG 把 Unreleased 收编进新版节 → 本地 `cargo test` + `pnpm tauri build` + `acceptance.ps1` 全过 →
  commit → `git tag v0.y.z` 推送（tag 不再触发发布）→ `powershell -File scripts/release-local.ps1`
  用本地安装包在**公开仓**（`deepseek-harness-desktop-releases`，对外分发）与**私有仓**
  （`deepseek-harness-desktop`，exe 存档）各建/更新 Release
  （需 GH_TOKEN；资产 exe+sha256，格式与旧 CI 完全一致，幂等可重跑；公开仓资产红线见 §GitHub 访问）。
  github 直连被拦时 push 走 §GitHub 访问的代理（openssl 被掐就加 `-c http.sslBackend=schannel`）。
  **Release 说明必须双语成文（0.5.11 起，弃用裸 generate_release_notes）**：说明文件
  存主仓 `docs/release-notes/`（每版两个：`v<ver>.md` = Release 正文（英文），
  `v<ver>.zh.md` = 中文说明），正文文件传 `release-local.ps1 -NotesPath <file>`
  （两仓同文；幂等重跑会重新 PATCH，改完重跑即生效）。正文首行
  `English | [中文说明](<公开仓 blob 链接>#中文说明)` 切换外链，英文正文平话编号
  小节（对齐 notion-desktop 的 Release 样式，少堆内部黑话）；中文说明**不内联**
  在 Release 页，点击跳转公开发布仓的 `docs/release-notes/v<ver>.zh.md`——该目录
  随镜像进公开仓（H:\My_Software\deepseek-harness-desktop-releases，先推它再
  PATCH，外链才不 404），主仓同目录存档。0.5.11 之前各版只有一条 Full Changelog
  链接，太模糊，别再犯。
- **弃用 CI 发布的原因（0.4.9 实踩）**：私有仓库 tag 触发的 release run 跑满 30~70 分钟
  （Actions 缓存按 ref 隔离，tag run 读不到自己存的缓存、只能等 main 预热），而本地构建
  3~5 分钟 + release-local.ps1 上传总共几分钟。release.yml 已降为 workflow_dispatch 手动备用。
  CI 只剩 build.yml（push main 触发）：跑测试+构建 artifact，兼作 rt 运行时缓存预热
  （缓存按 dsh 版本+脚本哈希为 key；ref 隔离机制细节见 build.yml 头注释）。

## GitHub 访问

- 仓库 **已转私有**：`LBurny/deepseek-harness-desktop`（origin 指向它）——存**源码 + tag + Release**
  （exe+sha256 双份存档）：v0.1.0~v0.5.2 是旧 CI 时代资产，0.5.3~0.5.6 缺口已于 2026-09-04
  用本地原件补齐（回拉哈希逐一核对过），此后 release-local.ps1 每版双仓上传
- **公开发布仓（0.5.3 起）**：`LBurny/deepseek-harness-desktop-releases`（public，匿名可读）——
  **只发 exe+sha256**，仓库内仅 README/LICENSE/界面截图作门面。应用的检查更新/手动更新/
  "GitHub 下载"全部指向它（update.rs 常量，有锚定测试防指回私有仓）。
  本地镜像在 `H:\My_Software\deepseek-harness-desktop-releases`（repo-local 身份同主仓 DSHDesktop）
- **红线（重大事故级，2026-09-04）**：公开仓**严禁上传源码或任何非 `*_x64-setup.exe(+.sha256)` 资产**
  （release-local.ps1 有守卫，违规直接拒发）。公开仓 Release 页的 "Source code (zip)/(tar.gz)"
  是 GitHub 按 tag 自动生成的**仓内文件归档**（内容只有 README/LICENSE/截图，无应用源码）——
  删不掉也别去删 tag：删 tag 会把已发布 Release 转成草稿（2026-09-04 API 实测：删 ref →
  draft:true，匿名 `releases/latest` 即断、应用内检查更新死；草稿 PATCH draft:false 重发布会
  把 tag 复活回来。试验用临时 Release 已清理）
- 本机**没装 gh CLI**；访问 GitHub API 用环境变量 **`GH_TOKEN`**（属主 LBurny，已验证对私有仓库返回 200）：
  `curl -H "Authorization: Bearer $GH_TOKEN" -H "Accept: application/vnd.github+json" https://api.github.com/repos/LBurny/deepseek-harness-desktop`
- git fetch/push 走凭据管理器 `manager-core`，不受私有化影响
- **github.com 直连可能被 TLS 拦截**（0.3.0 后实踩）：push 报 `self signed certificate in
  certificate chain`（openssl）或 `SEC_E_UNTRUSTED_ROOT`（schannel），而 api.github.com
  正常——是网络层拦截不是配置问题，别改 sslBackend/别关 sslVerify。系统 Clash 代理
  127.0.0.1:7890 走 github.com 通畅，一次性绕法：
  `git -c http.proxy=http://127.0.0.1:7890 push`（**不写入 git 配置**，拦截消失后直连仍可用；
  先 `curl -sI -x http://127.0.0.1:7890 https://github.com` 确认代理在线）。0.4.9 实踩补充：
  同一代理下 curl 稳定 200 但 git（openssl）握手被掐 SSL_ERROR_SYSCALL 时，加
  `-c http.sslBackend=schannel` 一次性即通（只换 TLS 栈，sslVerify 不动、不写配置）

## 关键约定与坑（细节见 docs/design.zh-CN.md）

- **set_autostart 不能无条件透传 disable()**：auto-launch 0.5 的 disable() 直接
  RegDeleteValueW，Run 值不存在时返回 ERROR_FILE_NOT_FOUND——从未开过自启动的用户
  每次保存设置都弹"系统找不到指定的文件 (os error 2)"。先 is_enabled() 比目标态，
  已达成即 Ok（commands.rs 有锚定测试）
- **dsh 事实（0.1.2）**：Node `^22.19 || >=24`；入口 `lib/bin.js`；`dsh web` 只许绑 127.0.0.1 且 spawn 必带 `--no-open`；**0.1.2 起 BrowserAuth 鉴权无关闭开关（回环也在门内）**：每进程 launch token 经 stdout 就绪行 `dsh web: http://127.0.0.1:<port>/?token=<t>` 打印（就绪行晚于 HTTP 绑定，必须持续 pump），`GET /?token=<t>` → 303 + Set-Cookie `dsh-auth-<hash>=v1.…`（HttpOnly/SameSite=Strict，**绑 authority——换端口即失效**），静态资产无门、`/api/*` 与 WS 全在门内；事件走**单 WS `/api/remote.mux`**：客户端发 `{type:"open",streamId,endpoint,payload:{args}}`，服务端回 `{type:"item"|"end"|"error",streamId,…}`，`$events` 端点（open 空 args）首条 item 是 ready，随后 `{type:"emit"|"waterfall",event,args|request}`，会话事件经 `session/follow`（args 包 `{request:{address:{kind:"session",sessionId}}}`——typert wire 名，裸 address 被拒）；完成判定看 follow 流 `event.type=="turn/end"`（`data.reason.kind=="completed"`），子代理标记看 `$events` 的 `api-session/added` `args[0].origin`；**严禁实现 `$events/result` 回包**（任一客户端回 result 即抢先替用户结算审批）；agent 预设独立成包 `@deepseek-ai/dsh-agent-presets`；设置在 `$DSH_HOME/settings.yaml` 的 `ui-theme.preference`（light/dark/system）；**npm 依赖是浮动区间**，跟版靠契约套件守门
- **运行时布局**：暂存 `src-tauri/runtime/<triplet>/`，tauri.conf `resources` 用映射形式
  `{ "runtime": "runtime", "resources/sounds": "sounds" }`，安装后落 `<install>/runtime/<triplet>/`
  与 `<install>/sounds/*.wav`（列表形式会错落到 `<install>/resources/sounds/` 致提示音探测不到，
  0.1.16 实踩；settings.rs 有锚定测试）；`bundle.resources` 相对路径映射（`..` 会变 `_up_`，别用）
- **子进程控制台**：`Platform::configure_child_command` 设 CREATE_NO_WINDOW；`kill_process_tree` 的 taskkill 同样必须带（GUI 主进程没有控制台，不带标志系统会为它新分配可见控制台窗口——退出/重启时闪 cmd）。复现"无控制台父进程"不能用 CREATE_NO_WINDOW 拉中间进程（那只是隐藏控制台，子孙会静默继承），须在中间进程里 FreeConsole()。验收判据是**可见 ConsoleWindowClass 窗口**（conhost 进程存在≠窗口可见）
- **PowerShell 5.1**：含中文的 .ps1 必须 UTF-8 **带 BOM**（注意 ZCode Edit 工具改完会丢 BOM，须补回）；别用 PS 改写 `settings.yaml`（会引入 BOM 导致 yaml-rust 解析失败，主题静默回退）
- **脚本里别用 Process.MainWindowHandle**：debug exe 还持有可见控制台与 Tao/托盘辅助窗口，句柄会指错；按 class "Tauri Window" 枚举进程顶层窗口（verify-no-size-flash.ps1 / verify-window-state.ps1 的 FindByClass 模式）
- **Tauri setup 无 tokio 上下文**：spawn_supervised 必须经 `tauri::async_runtime::block_on`
- **Tauri `resource_dir()` 返回 `\\?\` 扩展路径**：Node 加载器不认（EISDIR 崩溃），`runtime::strip_verbatim` 已处理，别绕过 ensure_runtime 自己拼路径
- **外部诊断手段**：`%LOCALAPPDATA%\DSHDesktop\events.log` 是壳侧诊断 + dsh 进程事件的统一持久层（1MB 截断），诊断面板回填读它的尾部（跨会话），应用卡启动时先看它。写入统一经 append_debug_line：无时间戳的行自动补 `[HH:MM:SS.mmm]` 本地前缀（壳侧自带戳的行与 cloudflared RFC3339 UTC 行不重复盖）——排查启动时序直接读行首时间戳
- **fixture 用 .cjs**（根 package.json 是 type:module）；`#[tokio::test]` 涉及 std::thread::sleep 时须 `flavor="multi_thread"`。use-fixture-runtime.ps1 会在 @deepseek-ai/dsh 下铺 CJS 桩 package.json——fetch-runtime 抓过的树带真实 `"type":"module"`，不铺桩 mock bin.js 会按 ESM 加载崩溃
- **dev 模式 tauri 不拷贝 bundle.resources**：内置音效在 dev 下要手动复制到 `src-tauri/target/debug/sounds/`，否则静默降级系统默认；另外真实运行时放 src-tauri/runtime 下跑 dev 会被 dsh 自更新触发 watcher 重建循环——复制到 src-tauri 外用 DSHDESKTOP_RUNTIME_DIR 指向
- **NSIS 离线**：github 直连不稳时用 ghproxy.net 预置 `%LOCALAPPDATA%\tauri\NSIS`（含 nsis_tauri_utils.dll，SHA1 须匹配 bundler 常量）
- **托盘 quit 顺序**：远程开着时**不杀隧道**（suspend_for_exit：代理随进程消亡，
  常驻隧道留活保域名，下次启动 resume 复活链接不变；0.5.8 起），只 stop dsh
  等 1.5s 再 exit；杀子进程树用 `taskkill /T /F`
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
- **进程死亡≠exe 文件锁释放（0.5.10 实踩）**：Windows 系统组件（Defender/PCA）对刚
  退出的进程映像持柄 1~3s（本机 19MB exe+机械盘实测锁窗口 ~1.3s）。模板
  `CheckIfAppIsRunning` 杀完主程序仅 500ms 就 Delete——快速连点 + 应用正在自行退出
  时 Delete 撞锁静默失败、退出码仍 0，模板 `PageLeaveReinstall` 的
  `OrIf FileExists $INSTDIR\主程序.exe` 复检弹 "Unable to uninstall!"。钩子在等净
  进程后再等三个 exe 可独占打开（`[System.IO.File]::Open(...,ReadWrite,None)` 探针，
  15s 封顶），让 CheckIfAppIsRunning 找不到活进程、Delete 落在锁释放后。
  **模板 "Unable to uninstall!" 的触发条件是卸载器退出码非 0 或主程序 exe 仍存在，
  两路都要想到**
- **NSIS `_?=` 必须是卸载器命令行最后一个参数**：它之后的所有内容会被吞进
  `$INSTDIR`（`_?=F:\DSHDesktop /S` → `$INSTDIR` 变成 `F:\DSHDesktop /S`，
  Delete 全部打空、退出码仍 0）。Tauri 模板把 `_?=$4` 放最后是对的；手工/脚本
  复现放错顺序会得到假的"卸载失败"（0.5.10 排障实踩一轮无效复现）
- **install_update 必须传 /UPDATE /P /R（0.5.10）**：Tauri NSIS 模板仅在 /UPDATE
  更新模式下跳过"先卸载旧版"（ExecWait `_?=` 旧卸载器 + FileExists 复检整段不执行），
  直接覆盖安装——旧卸载器不参与 = 上面的文件锁竞态无从发生，常驻隧道/remote-session.json
  也无人动（0.5.8 链接跨更新保持此前被裸参数破坏：模板只在自身带 /UPDATE 时才给旧卸载器
  追加 /UPDATE，裸跑则旧卸载器 POSTUNINSTALL 按真卸载杀隧道删状态文件，0.5.8→0.5.9
  实锤断链）。/P 被动只显进度条（GUI 模式"已安装"页仍显示且单选钮被强制忽略，UX 误导）；
  /R 被动/静默装完自动拉起主程序（.onInstSuccess）。Tauri 官方 updater 插件恒传 /UPDATE
  （plugins-workspace updater.rs updater_parameters）。**已知残留**：从 0.5.9 手动双击
  0.5.10 安装包选"卸载后再安装"仍由旧版（无等锁）卸载器执行可能复现弹窗——选"不卸载"
  或退出应用半分钟后再装可避开；POSTUNINSTALL 的父进程检测（父进程是新安装器则跳过
  隧道清理）让手动升级流也保链，真卸载（自我复制到 %TEMP%，父链已死/explorer）不受影响
- **远程 IPC 放行**：dsh UI 是远程源，远程 IPC 一律走 ACL。build.rs 用 `AppManifest::commands` 声明全部 44 个命令（生成 `permissions/autogenerated/allow-*.toml`），`capabilities/dsh-remote.json` 只对 `http://127.0.0.1:*` 开放 `allow-zoom-ui` + `allow-report-page-error` + `allow-ui-boot-ok`（页面桥两条，写日志专用）；副作用是本地命令也全部 ACL 化——**新增命令要同步三处**：build.rs、capabilities/default.json、按需 dsh-remote.json（tests/command_registration.rs 锚定三处一致 + 远程面锚定测试 remote_capability_only_exposes_zoom_and_pagebridge）
- **缩放快捷键匹配**：主匹配 `e.code`，`e.key` 兜底（合成按键/RDP 注入 keydown 的 `e.code` 为空）；zoom_ui 负载是 `direction:"in"/"out"`，步进由命令读设置（不写死在脚本里）；改快捷键须重注入钩子（set_shell_settings 已做，热替换不叠加）
- **reqwest 在系统代理下会劫持 127.0.0.1**：用户开 Clash 等系统代理时 reqwest 默认走代理且不认 bypass 列表——凡访问本机回环（remote/proxy.rs 转发客户端、测试里访问 fixture/代理端口的客户端）必须 `.no_proxy()`，否则请求被代理软件接管表现为假 502/挂起
- **手机端适配验证不能直连 dsh 端口**：mobile.css/mobile.js 只由代理注入，直连 `127.0.0.1:<dsh端口>` 的页面与手机看到的不是一回事（没有 项目/信息 标签、没有任何适配样式）——0.4.6 前在直连环境"验证"图标化白忙一轮。复刻手机环境=Playwright 390px 视口 + `addStyleTag/addScriptTag({path})` 注入同目录 mobile.css/mobile.js（mobile.js 挂观察器后有初始 `ensure()` 全量扫描，晚注入等价于 head 注入）；token 仅内存且日志脱敏，真走代理只能从托盘远程页读链接（用户用机期间别动 CUA）
- **0.1.2 token/cookie 时序四坑**：①就绪行晚于 HTTP 绑定——端口可探通但 401，
  token 必须从 stdout 持续 pump 捕获，"端口通了"不等于"能登录"；process.rs 的
  Ready 门控（wait_token）就是为此，超时按未就绪杀树重试而非白屏；②cookie 绑
  `127.0.0.1:<port>` authority——dsh 重启换端口后旧 cookie 全失效，代理转发遇
  401 要清缓存重换并重放一次（只重放一次防环），MuxSource/代理每次（重）连
  现换不缓存跨端口值；③WS upgrade 是独立桥接路径，cookie 注入最易漏（proxy
  bridge 用 http::Request 手动带 Cookie 头）——漏了手机端表现为"页面开但全断"；
  ④**token 按进程轮换，spawn 前必须清缓存**（0.5.1 实踩）：token/watch 只写不清，
  重启后 wait_token 拿旧 token 秒过、Ready 抢跑——主窗口带旧 token 导航落 401
  页、mux/代理 {新端口,旧 token} 换 cookie 401 死循环；现 supervise 循环每次
  spawn 前清空 token 缓存与广播端，回归测试靠 fixture 的 fake-dsh.token（按次换
  token）+ fake-dsh.token-delay（拉开 HTTP 就绪与 token 打印窗口）钉死；
  假 dsh（tests/support）把①②③全仿真，契约漂移先红在测试里
- **dsh-auth cookie 按进程累积 → 431 打死主窗口（0.5.11 实锤，2026-09-09）**：
  dsh 每个进程 Set-Cookie 一个**新名** `dsh-auth-<hash>`（30 天 Max-Age），cookie
  不分端口——WebView2/浏览器的罐子只进不出，dsh 重启多少次攒多少个。攒到 ~66 个
  （≈15KB）时，Cookie 头 + 插件 bundle 组合 URL（45 个 client.js ≈2.2KB）超过
  Node 默认 **16KB 请求头上限** → dsh 回 **431 Request Header Fields Too Large**
  → `<script>` error 事件 → 主窗口 "Failed to load plugins"，重启 dsh 不消（罐子
  只会更胖）。**阈值特性是排查陷阱**：短 URL 的 HTML 与小 bundle（client-modules
  单条）恒 200，只有最长的组合 bundle 触发；干净 profile 的 Playwright/Edge 全
  正常；手机走代理不受影响（代理只带自己那一个 cookie）。壳修复=0.5.11 每次
  Ready 导航主窗口前 `prune_stale_dsh_cookies`（tauri cookies()+delete_cookie，
  async 任务里跑避 Windows 同步死锁 wry#583；只删 `dsh-auth-*` 前缀，代理门岗
  `__dsh_remote` 不动，锚定测试钉死）。**排障手段（复用价值高）**：装包运行时
  无法看主窗口网络层——`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-
  debugging-port=9222` 重启应用即挂 CDP，`/json/list` 找主窗口 target 后
  Runtime/Network/Page 域全开（Node 22 内建 WebSocket 可直连），Network.
  loadingFailed 一次看真因；同法可清 cookie（Network.deleteCookies）+ Page.navigate
  带 token 免重启恢复现场。**注意**：cookie 加密是 WebView2 应用绑定的，把
  EBWebView profile 拷给 Edge 会解不开 cookie（表现为"干净罐"假阴性）；dsh 的
  静态资产（/plugins/*）确实无门（假 cookie 也 200），别再往"cookie 校验"方向查
- **主窗口观测桥 + 心跳自愈 + CDP 诊断开关（0.5.11）**：431 定位成本大头是"进程
  Ready 但窗口内黑盒"，三件套补齐。①错误桥（pagebridge.rs INIT_SCRIPT，挂
  initialization_script，document-start 先于页面脚本、绕 CSP）：error 捕获相
  （资源加载失败不冒泡，必须捕获相才接得到）/unhandledrejection/console.error →
  report_page_error 落 events.log [page:<kind>] 行——60 行/分钟限流（MinuteBucket
  纯逻辑可测）、800 字符 char 截断、\r\n 压平防拆行注入、redact_token 脱敏
  （页面 URL 的 ?token= 会经 e.filename/资源 src 原样带出）；invoke 句柄惰性解析
  （document-start 时 __TAURI__ 未必就绪，zoom.rs 同款回退）。②心跳仅
  127.0.0.1 源探 #root 子节点（本地 splash 也有 #root，不隔离秒报假心跳），90s
  封顶；Ready 导航后 30s 无心跳 → 看门狗自愈一次（清 dsh-auth cookie+带 token
  重导航，one-shot 不自旋；期间 dsh Failed/Stopped 牵回 splash 则放弃，不导航到
  死端口）。③诊断开关：runtime_base_dir/debug-cdp 空文件 → run() 最顶部（任何
  webview 创建前）set_var WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=
  --remote-debugging-port=9222（431 实锤靠挂 CDP 看 Network.loadingFailed）；
  9222 对本机全进程开放页面调试（可读 token），排完删 marker。页面桥两条命令
  从 dsh 远程源调用，dsh-remote.json 放行集扩为 {zoom_ui, report_page_error,
  ui_boot_ok}——只写日志、限流、脱敏；再往远程放命令先过安全审查再改锚定测试
- **dsh 就绪行被 MCP 插件加载整体阻塞（0.5.6 实踩定位）**：dsh-mcp-client 的
  apply() 无条件 `await connection.ready`（连接+首次工具同步才 settle，
  failOnStartupError 只控是否抛错、不控等不等），web-app 的 `dsh web:` 就绪行
  又等 cordis loader.await()——MCP 条目配成 `npx 裸名/@latest` 时**每次启动**
  联网解析版本，上游发版后首次启动全量冷装（npm 直连慢网实测 >2-3min）直接
  卡在 token 等待上。壳对策三件套（process.rs / lib.rs）：wait_token 见 npm
  `will be installed` 警告行切 install 长预算（固定 60s 会把快装完的进程杀树、
  白等一轮靠缓存余温重启）；splash 同一信号换"正在下载 MCP 组件"文案；Ready 前
  落耗时分解行供诊断面板"上次启动"。fixture 有 fake-dsh.npm-install-warn 模拟。
  注意：MCP 装什么是用户自由，壳不代做本地化/钉版，只做"不杀错、可感知、可诊断"
- **reqwest 错误 Display 自带完整 URL**：`error sending request for url (…/?token=…)`
  ——凭据相关请求的失败日志若直接 `{e}` 输出，token 明文落 events.log（0.5.1
  实踩）。凡带 token 的 URL 请求，错误一律 `e.without_url()` 剥尾巴后再格式化；
  dsh_session.rs 有单元测试钉死
- **0.1.2 插件客户端 bundle 合并加载，壳侧改写必须双形态+缓存击穿（0.5.3 实踩）**：
  bundle 从单插件 `/plugins/<id>/client.js?rev=N` 改为 `/plugins/??<a>/client.js,
  <b>/client.js,...&rev=N`（path 只剩 `/plugins/`，清单在 query，单条 3.7MB）——
  proxy 的 `is_plugin_client_bundle` 用 `ends_with("/client.js")` 判定静默失配，
  内测声明改写失效、远程端每次连接都弹。要点：①matcher 认 `starts_with("/plugins/??")`；
  ②bundle 响应带 `cache-control: immutable, max-age=1y` 且 **rev 跨 dsh 重启稳定**
  （内容哈希）——壳侧改写产物变了手机端也不会重取，须 302 到带 `dshv=<壳版本>`
  的同 URL 击穿（重定向 no-store）；③**真 dsh 对组合 URL query 逐字校验，多余
  参数 404**（追加 `&dshv=` 实测 404）——buster 只活在代理与浏览器之间，转发前
  由 `strip_cache_bust` 剥掉；④`send_forwarded` 别就地按 url 重算改写判定（带
  scheme 恒 false → accept-encoding 不剥，真 dsh 压缩响应即改写失效），由
  forward() 按 path_and_query 判定后传参。假 dsh 有 `/plugins/` 合并形态路由+
  命中记录（tests/support），回归在 tests/remote_proxy.rs
- **iOS WKWebView 键盘收起后视口平移残留（0.5.3 手机实拍）**：底部输入框聚焦→
  退出后，页面停在无法手势复位的平移偏移上——头部与 对话/轨迹/项目/信息 标签栏
  停在视口外，观感如"全屏"，滑动失灵。dsh 自身无键盘视口处理（bundle 仅
  react-dom 引用 visualViewport），桌面 Chromium/模拟键盘均不可复现，纯 iOS
  WebKit 行为——mobile.js 视口复位：输入框失焦与 vv resize 时把文档滚动复位 0
  并强制重排（聚焦中的合法平移不干预；健康态文档滚动恒 0，复位为无操作）
- **iOS WKWebView 聚焦 <16px 输入框自动缩放整页且不复原（0.5.4 手机实拍实踩）**：
  输入框聚焦→返回后整页放大，标签栏/输入框被推出可视区，手势缩不回（微信内置
  浏览器同样中招）。触发条件：viewport meta 未禁缩放（dsh 入口文档是 Vite 模板
  原值 `width=device-width, initial-scale=1`）+ 聚焦元素 font-size<16px
  （composer 实测 `var(--dsh-content-font-size,14px)`）。双防线：①proxy.rs 注入
  HTML 时把 viewport meta 改写为 `maximum-scale=1, user-scalable=no`（needle/
  replacement 收 upstream.rs::VIEWPORT_META_*，上游改模板值契约探针翻红）；
  ②mobile.css ≤700px 下 `[class*="_composerStack"] textarea{font-size:16px}`——
  从触发条件上消灭缩放，改写 miss 时兜底。注意 dsh 包内无任何 `textarea{}` 字号
  规则，composer 字号是继承卡片来的，直接子代规则稳赢；平移残留（上一条）与
  缩放是两种独立症状，视口复位对缩放无效，别混修
- **主窗口由 setup 代码创建（tauri.conf windows 为空）**：on_download 只能挂 WebviewWindowBuilder，conf 声明的窗口无法附加。建窗参数须与原 conf 一致（visible(false)+center()+min 900x600），window-state 对代码创建窗口同样在创建事件排队 restore（托盘按需窗口同款），回归靠 verify-no-size-flash/verify-window-state 两脚本
- **dsh 预设已独立成包（0.1.2）**：minimal 预设从 dsh 包内 `config/agent-presets/` 迁到 node_modules 的 `@deepseek-ai/dsh-agent-presets/presets/minimal`（PRESET_DIR_SEGMENTS 已随版）；0.1.1-rc.2 时代 composeProfile 重写 roots 的行为上游已删（prep §一），预设如需补丁理论上可走 patch 影子覆盖，但当前无需求——签名哨兵（presets.rs + 契约探针）继续盯着 win32 修复不回退
- **fs-local 列目录遇 ACL 拒绝项即整列失败**（如 C:\ 根目录撞上 DumpStack.log）：上游 dsh 行为，Windows 上列举系统盘根目录必现；壳侧缓解是让模型知道 cwd 并待在 workspace，别试图在壳里修列目录
- **提示音播放禁用 PlaySoundW，改自管 waveOut**：PlaySoundW 四轮翻车史——SND_NOSTOP 忙时放弃（0.3.x）、SND_ASYNC 缓冲悬垂（0.4.2）、SND_ASYNC 工作线程首播静默吞错（0.4.2 修后仍复现）、SND_SYNC 下 winmm 缓存设备句柄失效（0.4.5 机器 B：首次有声后续全静默，日志全 ok 播满时长）。现 `play_sound_file` 在专用线程自管 waveOut：每次播放 waveOutOpen 新开设备句柄（WAVE_MAPPER 取当前默认）、播完即关，打开/写入/收尾每步都有真实 MMSYSERR 错误码，日志带实际设备名；打断语义自管（新播放 reset 旧会话）。改回 PlaySound 等于把盲区请回来
- **toast 点击激活只能走协议激活**：未打包 Win32 应用的 in-process `ToastNotification.Activated` 回调在 Win10 不可靠——AUMID 无注册、补 HKCU AppUserModelId 键两种条件下点击均不触发（机器 A 实测，toast 被点掉但不回调）；现 toast XML 带 `activationType="protocol" launch="dshdesktop://open"`，点击由系统拉起协议 → 二次实例被 single-instance 拦截 → 回调 show 主窗口。链路依赖启动时的 `ensure_activation_registered`（HKCU 写协议+AUMID，仅安装形态写入，dev 跳过防覆盖已安装版指向）
- **协议激活到顶的教训：先验证调用链，再加固机制**：机器 B"窗口在浏览器下面"的真根因是 single-instance 回调里内联的 `show+unminimize+set_focus` 三件套**根本没调 `tray::show_main`**——此前对 show_main 做的所有置顶加固都落在协议激活走不到的路径上（机器 A"验证通过"是假阳性：隐藏窗口 show 后自然出现在可见位置，没有真实遮挡竞争）。现回调统一走 show_main → platform `bring_to_front`：后台线程两段择时（250/550ms，压住 Shell 在 toast 关闭动画后把前台归还点击前应用的动作）→ TOPMOST → AttachThreadInput 挂前台线程借权限 → BringWindowToTop/SetForegroundWindow/SetActiveWindow → detach → NOTOPMOST（不常驻置顶）；另有第二路权限——协议激活拉起的二次实例在 main 开头（single-instance 拦截前）`AllowSetForegroundWindow(ASFW_ANY)` 把 Shell 授予的前台权限广播给主实例（Chromium/VSCode 单实例激活同款）。bring_to_front 每步落 events.log（每轮前台 pid/attach/sfg 结果 + 2s 后最终归属），失败可直接读日志定位。AttachThreadInput 在 windows-sys `Win32::System::Threading`，SetActiveWindow 在 `Win32::UI::Input::KeyboardAndMouse`；attach 对 UWP 前台线程（如操作中心）会被拒——那是通知中心点条目的特例，真实 banner 点击时前台是桌面进程不受影响
- **toast 图标只有顶部行小图标，不放 appLogoOverride**：顶部行（应用名左侧）小图标由 HKCU AppUserModelId 键的 IconUri 提供，指向随包 256px `icons/128x128@2x.png`（图标位单文件无法按 DPI 分套，256px 源系统自缩放，128px 在 200%+ 缩放发糊）；toast XML **不含** `<image>`——appLogoOverride 会在正文区再渲染一个大图标，与顶部行叠出双图标且挤压文字排版（机器 B 实测）。两道坑都有锚定测试：①bundle.resources 漏映射 `icons/128x128@2x.png` 则安装包不含图标、IconUri 静默失效，dev 下 resource_dir 指源码树恰好有文件会掩盖（0.4.5 初版实踩）；②toast XML 断言无 `<image>` 元素

## 测试基线

`cargo test` 应全绿（当前 250 个，含 `tests/upstream_contract.rs` 对真实运行时的上游契约探测——跟版门禁：fetch 新版 dsh 后它红了就按输出改 `src/upstream.rs`）。`tests/console_window.rs` 的对照组会在屏幕上短暂弹出真实控制台窗口，属正常。改主题/进程/通知逻辑后，跑 `cargo test` + 重装走一遍 `acceptance.ps1`。

## 多平台预留

平台差异都收口在 `platform/mod.rs` 的 `Platform` trait（节点可执行名、运行时目录、triplet、杀进程树、子进程配置、系统深浅色）。CI matrix 里 macos/linux 行已注释，启用前需实现对应 `platform/{macos,linux}.rs` 并在 fetch-runtime 支持对应 triplet。

## 已知限制

- Win10 深色标题栏聚焦时纯黑（系统行为，`DWMWA_CAPTION_COLOR` 仅 Win11）；要做成恒为 dsh 深灰需无边框自绘标题栏——方案要点见 docs/design.zh-CN.md §8，暂缓。
- ~~检查更新在仓库私有期间必 404~~ **（0.5.3 已修，走候选方案②）**：另建公开发布仓
  `deepseek-harness-desktop-releases` 专发 Release，update.rs 两个常量与 release-local.ps1
  指向它——公开仓匿名 API 可读，启动检查与"其它设置"手动检查恢复。历史背景：0.4.x~0.5.2
  匿名查私有仓 `releases/latest` 一律 404，只在 events.log 落 `Update: check on launch failed`
