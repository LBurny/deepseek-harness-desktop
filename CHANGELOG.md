# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.15] - 2026-09-13

### Fixed

- **配置 MCP 服务后启动长时间卡在「正在准备运行时…」**（用户反馈：装了两个 MCP 后启动要等半分多钟）。根因链实锤：dsh-web-app 的 `announceReady()` 有意等 `loader.await()`（全部 cordis 插件 settle）才打 stdout 就绪行，而 dsh-mcp-client 的 `apply()` 无条件 `await connection.ready`；用户侧的 MCP 都用 npx 浮动 spec（未钉版本），每次启动要做 registry 往返解析（冷装 >2-3min），于是每个 MCP 服务的连接耗时全额叠进启动等待（用户实机 36.6s）。关键源码事实：`failOnStartupError=false`（默认）时这个 await 的结果只服务 throw 分支——纯拖延；工具注册走 startConnection 内部 generation 链、dispose() 独立 await settling+syncChain，不 await 对功能性零影响。修复：新增 `mcpgate.rs` 启动期原地补丁（与 pickerpatch 同款签名门控 + marker 幂等 + tmp+rename 原子写 + 漂移整组停手）：`failOnStartupError=true` 保留上游语义原样抛错；`false`（默认）改为后台观察，连接失败只记 ctx.logger.error 不再卡就绪行。确定性实测（sleeper MCP 复现）：84s → 5.1s。代价已知并文档化：界面打开后最初几秒 MCP 工具可能尚未注册（连上自动补注册）。回归：`mcpgate` 单元测试 4 条 + 契约套件 `probe_mcpgate`（marker 在即通过、否则钉上游原文 needle）
- **会话头部「打开方式」按钮比其它按钮晚两三秒出现**（用户反馈）。根因：apps store 初值 `null`，组件在 `GET /open-in-app/apps` 应答前返回 null 不渲染；宿主机探测是每进程一次的懒加载（`resolutions ??=`），实测冷 2861ms，图标另要 271ms。修复：新增 `oiacache.rs` 同款签名门控补丁，给 apps store 加 `persist: { name: "dsh.open-in-app.apps" }`——`createSnapshotStore` 的 persist 走 `JSON.stringify/parse`，数组透明持久化（同文件 choice store 是既有先例）；0.5.14 的端口记忆保住源站跨启动稳定后，localStorage 缓存才真正跨启动有效。按钮第一帧按缓存渲染，真实探测落地后静默校正。验证方法学：新 Playwright profile 的页面管道本身要 10-21s 会掩盖按钮门禁，改用 route-delay A/B（`page.route` 把 `/open-in-app/apps` 延迟 20s 再 fulfill）——有缓存：按钮与头部同帧（delta=0），fetch 仍在飞；清缓存：按钮 2561ms 后才出现且钉在 fetch resolve 上。边界已知：首次启动无缓存走原节奏；已卸载的应用在缓存期可能残留，探测落地即校正。回归：`oiacache` 单元测试 4 条 + 契约套件 `probe_oiacache`

## [0.5.14] - 2026-09-12

### Fixed

- **会话头部"打开方式"的选择（VS Code / 文件资源管理器）重启后回默认值**：用户选了 VS Code，重启应用又变回文件资源管理器。根因不在选择本身——这份选择由上游插件 `dsh-client-ui-open-in-app` 存在浏览器 localStorage（`dsh-client-store` 的 `persist: { name: "dsh.open-in-app.choice" }`，无服务端副本，代码注释也写明"shared across sessions and browser restarts"），而 localStorage 按 **origin（含端口）** 隔离；壳此前每次 spawn 都 `free_port()` 让 OS 随机分配端口，等于每次启动都是全新源站，于是每次都读不到上次写的值、回退到探测到的第一个应用（Windows 上即文件资源管理器）。实锤：WebView2 的 `Local Storage\leveldb` 里同一个库并存 7 个 `http://127.0.0.1:<port>` 源站，每个源站各存各的 `open-in-app.choice`，最新一条确实写着 `"vscode"`——写在当次启动的端口下，下次启动读不到。同一条船上还有另外三个走 localStorage 的偏好（`dsh.conversation.contentWidth` 会话内容宽度、`dsh.sessions.current` 当前会话、`dsh.trajectory.duration` 轨迹时长），所以修法选在源站层而不是逐个补偏好：`port.rs` 新增**记忆端口**——Ready 时把实际绑上的端口写进壳数据目录 `dsh-port.txt`，下次启动探活通过就复用它（2s 重试预算覆盖上个进程刚退、监听套接字尚未回收的瞬间），端口确实被占则换新端口并在 Ready 后改写记忆，一轮收敛；同一轮内的重试回避记忆端口，保住"启动失败即换端口重试"的原有兜底（否则会在旧端口上反复撞、白等满 5 轮才 Failed）。首次升级后需重新选一次打开方式——旧选择留在已废弃的随机源站下，读不回来（浏览器存储按源站隔离，无跨源迁移途径）。回归：`port.rs` 单元测试 6 条（记忆读写往返/损坏与低端口拒绝/空闲复用/被占回退/短暂占用等待/低端口不复用）+ `tests/process.rs::port_is_reused_across_launches`（首个进程 Ready 后落盘、第二个进程读回同一端口并落 `reusing remembered port` 日志）

- **手机端输入框底栏排版不等距**（用户手机实拍反馈）：工具行的 flex gap 虽恒为 12px，但各件盒子宽度不一（"+"/附件 28px 实心圆、权限/模型触发器 44~46px 且 padding 左 8 右 4 不对称、发送键 34px），触发器尾部还各拖一枚 chevron——视觉间距实测 12/20/32/22/18px 全不一样。现按手机端"栏内只留图标"惯例（与藏触发器文案同一逻辑）：行内两枚触发器 chevron 一并隐藏（本地名 `_chevron` 实测落盘两包——dsh-client-ui-model-selection 的 svg 与 dsh-client-ui-permission-presets 的 span，新增 `upstream::COMPOSER_TRIGGER_CHEVRON_NEEDLE` 与两条按包定向的契约探针；该词在 14 个包里都有，全树探测恒绿无意义），行内全部按钮统一 32×32 盒、padding 归零、内容居中（发送键 34→32 一并拉齐，主操作靠蓝色填充区分而非尺寸），组内间距恒 12px、44px 均匀节拍，组间仍由 space-between 留白分隔。选择器 scoped 到 `[class*="_row"]:has(> [class*="_tools"])`——`_tools` 全 node_modules 仅会话包独有，而 `_trailing` 与 dsh-client-ui-input-trigger 弹窗组件撞名（其 `_trailing` 是行尾 inline-flex 段），裸匹配会误伤弹窗。真浏览器实测（临时裸 dsh 实例 + Playwright 注入适配层量 DOM）：360/390/500/700px 组内间距恒 12px 无溢出，720px 断点外原生全名药丸恢复，暗色主题与首页新建会话态（无用量环）同验。回归：`tests/remote_project.rs::composer_toolbar_even_spacing_rule`（两条规则的锚点/关键声明/断点位置 + `_trailing` 裸匹配护栏）

## [0.5.13] - 2026-09-12

### Added

- **陈旧锁自愈（`locks.rs`）**：每次启动 dsh 前清扫 `DSH_HOME` 里持有者已退出的 `*.lock`。dsh 的跨进程写锁是目标文件的兄弟 `<file>.lock`（`wx` 独占创建、内容为 pid、只在 `finally` 里删），上游明确不做孤儿恢复；而壳在 Windows 上只能 `taskkill /T /F` 硬杀（dsh 的 SIGTERM 优雅退场在 TerminateProcess 下拿不到信号），恰好持锁时被杀就把锁永久留在盘上——此后每次启动都在 boot 阶段等锁超时（凭证写入预算 30s）、插件树加载失败、进程退出，壳只看到"就绪行没出现"，重试多少次都一样，用户视角是**应用再也起不来**（机器 B 实踩：`.credentials.yaml.lock`）。判定保守：pid 可解析且进程已退出（或被无关进程复用了 pid）才删，内容不是 pid 的要够老（5 分钟）才删，持有者活着且镜像是 node 的一律保留；扫描限深 3 层、跳过 `node_modules`、不进符号链接/junction；每条判定落 events.log。真机复现链已验：干净 home 能起（且锁文件每次 boot 都会建又删，故硬杀窗口每次启动都存在）→ 塞入死 pid 的锁后 dsh 卡 `timed out waiting for the writer lock` → 删锁即恢复。回归：`locks` 单元测试 10 条（含真平台接线）+ `tests/process.rs::spawn_heals_stale_dsh_lock_files` 钉住"确实接在 spawn 路径上"
- 契约套件新增 2 条锁探针：`dsh-atomic-write` 的锁形状（`wx` 建 `.lock` 兄弟 + 内容 pid）与"上游仍不自愈孤儿锁"（`never removes an existing lock`）——上游一旦自己做陈旧锁回收，自愈就该撤掉，探针翻红提醒

### Fixed

- **v0.5.12 的 Release 正文首行中文被写成 `????`**（线上已就地修正，双仓同文）：正文首行 `English | [中文说明](…#中文说明)` 上线后成了 `English | [????](…#????)`，链接文本与跳转锚点一起烂掉（页面照常打开、资产与标签全对，不点进去看不出）。根因是同一条多解释器差异的**另一半**：`Invoke-RestMethod` 收到 `string` 体而 `-ContentType` 不带 `charset` 时按 **ISO-8859-1** 编码，非 ASCII 一律变 `?`（`powershell` 5.1；PS 7 按 UTF-8 故 0.5.11 侥幸正常）。现所有 JSON 正文经新的 `Get-JsonBodyBytes` 发 UTF-8 **字节**体（`-ContentType 'application/json; charset=utf-8'`），并纠正旧注释的错误说法（5.1 的 `ConvertTo-Json` **不会**把中文转 `\uXXXX`，中文原样进 JSON，编码责任全在发送这一步）。防线补到四道：`release-local.ps1 -SelfTest` 增加"上行正文按 UTF-8 字节发送"与"所有 JSON 调用点都经字节助手"两条断言、[0/9] 前置自检、[3/9] 形状预检、[9/9] 终验读回线上正文断言含说明文件首行原样。本机实测矩阵（本地收包器抓原始上行字节）：5.1 + string → `?`；5.1 + string + `charset=utf-8` → 正常；5.1 + 字节体 → 正常；7 三种都正常
- `release-local.ps1` 支持 `-NotesOnly`：只 PATCH 两仓已发布 Release 的正文，不先删后传 exe/sha256（改错别字/坏链接不必为几行字动公开安装包）；`-NotesOnly` 找不到 Release 时直接报错，不会建出无资产的空发布页
- **手机端回合统计行不再贴在输入区下方**（0.1.5 适配回退）：底部那行"N 轮 N 步 / token 用量"应整行搬进顶部「信息」页，但 0.1.5 的 StatsPills 行把分隔点"·"挪进了药丸 label **内部**，旧适配（mobile.js 找行要求"直接子代含 ≥2 个 _sep"、mobile.css 隐藏规则要求 `:has(> _sep)`）全部失配——统计行留在输入区下方贴着屏幕下缘，信息页恒为空态。现双双改锚到上游新挂的稳定属性 `data-composer-stats`（`upstream::COMPOSER_STATS_ROW_HOOK`，契约探针守门），统计照旧克隆进「信息」页；面板只藏**行级**分隔符，药丸内部的"·"保留（否则"N 轮 N 步"和"38 tok/s"会粘在一起）。真浏览器实测（真实回合数据：1 轮 1 步 · 221 tok/s · 25.8K tok）：增强生效后原行 `display:none`、信息页两行统计齐全。回归：`tests/remote_project.rs::composer_stats_row_anchors`（新钩子接线 + 旧锚点不得复活 + 面板分隔符限定行级）
- **手机端模型选择器上的重复图标**（0.1.5 适配回退）：窄屏上触发器里同时出现"火花 + 数据"两枚图标。dsh 0.1.5 的模型触发器自带 `triggerIcon`（`IconDataOutline16`），而我们从 0.4.x 起用 `::before` + mask SVG 补过一枚火花图标（当时上游只有文案 + chevron）——上游补上图标后两枚就并排了。现删掉自造图标、`mobile.css` 改为在 700px 断点内点亮上游那一枚（上游只在容器 ≤360px 时显示它，361~700px 区间不点亮就一个图标都不剩）。真浏览器实测（390/500/700/720px 四档量 DOM）：≤700px 稳定为「原生图标 + chevron」，>700px 回到「模型名 · 推理等级 + chevron」文本态；把退役前的规则临时注入即可复现旧的"两枚"。回归：`tests/remote_project.rs::model_trigger_iconified_rule` 加三条断言（原生图标须点亮、不得再出现 `::before` 自造图标、不得再出现 mask SVG），契约探针加 `MODEL_TRIGGER_ICON_NEEDLE`

## [0.5.12] - 2026-09-12

### Changed

- **dsh 运行时 0.1.2-rc.1 → 0.1.5-rc.2**（fetch-runtime.ps1 钉版；跟版预研与影响面见 `docs/upstream-0.1.5-prep.zh-CN.md`，执行计划见 `docs/superpowers/plans/` 下 2026-09-12 的执行计划）。上游要点：**会话格式升 V3**——恢复旧会话时生成 V3 新日志并保留原文件，但**升级后的会话不可降级读取（用户数据单向）**；Web 新增流式文件上传（任意类型，进度/取消/切会话续显）与右侧栏（多标签/分栏/全屏，Markdown/代码/HTML/PDF/图片预览，包括子代理与未激活会话的文件，**原 Detail 面板移除**）；Web minimal 预设只剩持久 shell（`str_replace_editor` 与 `fs-local` 整组移除，极简模式从双工具降为单工具）；出站请求开始遵循 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY/NO_PROXY`（回环显式豁免 `127.0.0.0/8`）；Windows 本地子进程新增 `windowsHide`；MCP 工具列表遇重复分页游标不再挂住启动；插件面板槽位重排（`conversation`/`details` → keyed `main` + `rightbar` + `sidebar.panellist`）。**逆向面实测几乎不动**：契约套件 37 条探针仅 1 条漂移（见下）
- 会话头部「Session log 下载按钮」在 0.1.5 改为「更多操作」图标按钮（CSS Modules 本地名 `sessionLogButton` → `moreButton`，下载动作收进它弹出的菜单）；`upstream.rs` 常量随之改 `SESSION_HEADER_MORE_BUTTON_NEEDLE`，`mobile.css` 的手机端隐藏规则同步改锚（锚点全 node_modules 唯一命中，无同名歧义）

### Added

- **远程代理流式上传旁路**（`remote/proxy.rs`）：`forward()` 为支持 401 换 cookie 后重放会整读请求体（上限 64MiB），而 0.1.5 的 `POST /api/session/uploadFileBinary` 是流式、服务端不限体积——命中 `is_streaming_body_route` 即转 `forward_streaming` 逐块直通（上传前强制换一次 cookie、不做 401 重放；换不到 cookie 时 dsh 的 401 原样透传，绝不误报 502）。修复手机端经隧道传大文件撞壳侧上限报 502「读取请求体失败」与壳进程短暂驻留 64MiB 两个问题。回归 `upload_route_streams_without_buffering` 用裸 TCP 分块 + 假 dsh 进度探针钉死「客户端发完之前代理已把首块转给 dsh」，并对旧缓冲实现验过红（注意 hyper 客户端对 `wrap_stream` 请求体在下一帧前不 flush，测试客户端必须用裸 TCP 才能观察到首块上线）
- 契约套件新增 6 条 0.1.5 面探针：流式上传路由存在 + `requestBody: streaming` 声明、`/open-in-app/*` 三条免鉴权路由、面板槽位（`main`/`rightbar`/`sidebar.panellist`）、会话格式版本 V3、命令服务 `attachments` 旗标（预装 `/init` 插件兼容哨兵）、minimal 预设已无文件编辑工具（语义变更哨兵，与既有 win32 修复哨兵并存）

### Fixed

- **跟版脚本的文档基线同步复活**：`follow-upstream.ps1` 的文档同步靠计数断言（`upstream.rs=1 / design.zh-CN.md=3 / README×2=1`）定位版本串，0.1.2 跟版时因 `upstream.rs` 里两处历史对照注释含旧版全串（整文件计数 3≠1）导致**四个文件全部被静默 skip**、头注烂了两个版本。现把历史对照改成不带 `-rc` 的写法、计数不符时**整步显式报错**（不再静默跳过），SelfTest 增加「四文件钉版引用计数 == 期望」断言（13/13 绿）。本次跟版四个文件全部自动翻转成功
- fetch-runtime 冒烟的就绪行轮询预算 30s → 120s，且失败路径打印 dsh 原始输出：首跑曾在 `npm install`（500+ 包）与 `prune`（删约 2 万文件）之后把环境抖动误报成「就绪行契约漂移」（同一棵树随后手动起 5s 就绪，就绪行字符串与 0.1.2 逐字相同）

## [0.5.11] - 2026-09-09

### Fixed

- **主窗口 "Failed to load plugins"（bundle 加载失败）根因修复**（用户反馈：配置 MCP 本地插件后主窗口整页报错，重启 dsh 多次不消）。根因实锤（挂 WebView2 CDP 抓网络层）：dsh 每个进程 Set-Cookie 一个**新名的** `dsh-auth-<hash>`（30 天 Max-Age），cookie 不分端口，主窗口 WebView2 的 cookie 罐只进不出——实机抓到 **66 个**累积 cookie（≈15KB）；dsh 的插件 bundle 是 45 个 client.js 的组合 URL（≈2.2KB），两者相加超过 Node 默认 16KB 请求头上限，dsh 回 **431 Request Header Fields Too Large**，`<script>` error 事件 → "bundle script failed to load"。阈值特性解释全部现象：短 URL 的 HTML 与小 bundle（client-modules）恒 200，只有最长的 bundle 触发；干净 profile 的 Playwright/Edge 复刻全部正常；手机走代理不受影响（代理代持单个 cookie）。修复：壳在每次 Ready 导航主窗口前用 Tauri cookie API（`cookies()` 含 HttpOnly + `delete_cookie`，跑在 async 任务避 Windows 同步死锁 wry#583）清光罐里的 `dsh-auth-*` 再带 token 导航——`?token=` 立即补发新 cookie，罐子此后恒 ≤1 个；远程代理门岗 cookie `__dsh_remote` 与其它站点 cookie 不动（锚定测试防误删）。当场处置：CDP 清 cookie + 带 token 重导航，UI 恢复（DOM+截图验证）。**注意**：已装的 0.5.10 不含此修复，清干净的罐需再积累数周才复发，发 0.5.11 即根治；用户若用 Chrome 直连过 dsh，Chrome 自己的 cookie 罐同样会中招，清一次 127.0.0.1 的 cookie 即可

### Added

- 主窗口页面观测桥：document-start 注入脚本（每次导航都跑、先于页面脚本、绕页面 CSP）把 dsh UI 的脚本错误/资源加载失败/unhandledrejection/console.error 限流落 events.log（`[page:<kind>]` 前缀行，60 行/分钟限流 + 800 字符截断 + token 脱敏）——"进程 Ready 但 UI 死"类故障（如本次 431）第一时间在日志可见
- UI 启动心跳与自愈看门狗：dsh UI `#root` 挂载成功上报落 `dsh UI booted (Xs)`；Ready 导航后 30s 无心跳自动清 dsh-auth cookie 并带 token 重导航一次（one-shot 不自旋，动作落日志；期间 dsh 停止则不导航到死端口）
- 诊断 CDP 开关：`%LOCALAPPDATA%\DSHDesktop\debug-cdp` 空文件存在时 WebView2 带 `--remote-debugging-port=9222` 启动，免改代码挂 DevTools 协议排障（端口对本机全进程开放页面调试，仅排障期间放置 marker）

### Changed

- dsh 子进程 Node 请求头上限 16KB→64KB（`--max-http-header-size=65536`）：cookie 剪枝之外的纵深防御，兼护用户自带浏览器直连 dsh 端口攒了 cookie 的场景

## [0.5.10] - 2026-09-05

### Fixed

- **覆盖安装/应用内更新不再弹 "Unable to uninstall!"**（用户反馈：更新下载后装不上，老问题复发）。模板该弹窗有两个触发条件：`_?=` 原地运行的旧卸载器退出码非 0，或卸载后主程序 exe 仍存在（`PageLeaveReinstall` 复检）。排障实测定位到两类引爆点：①`install_update` 裸启动安装包不传 `/UPDATE`（Tauri 官方 updater 插件恒传），升级必经"先卸载旧版"步骤，整个失败类才有机会发生；②卸载时序窗口：用户快速连点时，旧卸载器的 `CheckIfAppIsRunning` 会撞上正在自行退出（quit_app 停 dsh 等 1.5s）的主程序，杀进程后仅 500ms 就 Delete 主程序 exe，而 Windows 系统组件（Defender/PCA）对刚退出的进程映像持柄 1~3s（本机实测锁窗口 ~1.3s，19MB exe + 机械盘 + 杀软扫描），Delete 静默失败、退出码仍 0 → 触发条件②。修复：应用内更新改传 `/UPDATE /P /R`——更新模式下模板跳过卸载步骤直接覆盖安装（旧卸载器不参与，整个失败类无从发生）、只显进度条免逐页点击、装完自动拉起新版形成闭环；NSIS 钩子在等净进程后再等主程序与 runtime 两件 exe 可独占打开（15s 封顶，超时照常继续不劣于旧行为），把手动流残余竞态窗口压到最小。**已知残留**：从 0.5.9 手动双击 0.5.10 安装包并选"卸载后再安装"仍由旧版（无等锁）卸载器执行，快速连点+应用刚退出时可能复现——改选"不卸载"直接覆盖，或退出应用半分钟后再装即可避开；应用内更新无此问题
- **远程链接跨更新保持真正落地**（0.5.8 语义此前在真实更新路径被破坏，每次更新必断链）：`install_update` 不传 `/UPDATE` 时模板调旧卸载器也不带 `/UPDATE`，旧卸载器 POSTUNINSTALL 按"真卸载"杀常驻隧道、删 `remote-session.json`（0.5.8→0.5.9 实锤：装完启动无 auto-resume，只能手动重开换新链接）。修复双保险：①应用内更新走 /UPDATE 模式，旧卸载器根本不运行；②POSTUNINSTALL 清理前检测父进程——父进程是新安装器（`DSHDesktop_*_x64-setup.exe`，模板 `_?=` 原地调用的升级卸载场景）则跳过隧道清理，手动双击安装包的升级流同样保链；真卸载（设置/开始菜单，自我复制到 %TEMP%，父进程链不匹配）与 WMI 查询失败时照常清理（fail-closed 保卸载卫生）
- **/UPDATE 覆盖安装的运行时卫生**：更新模式不经过旧卸载器，旧版 runtime 树无人清理——PREINSTALL 钩子装前自清 `$INSTDIR\runtime`（`RMDir /r /REBOOTOK`），防旧版独有文件（含 dsh 自更新残留）跨版本混杂；非更新路径旧卸载器已删净，重复清理是无害 no-op

### Known Issues（排障备忘）

- NSIS `_?=` 开关必须是卸载器命令行的**最后一个**参数：它之后的所有内容会被吞进 `$INSTDIR`（如 `_?=F:\DSHDesktop /S` 会让 `$INSTDIR` 变成 `F:\DSHDesktop /S`，Delete 全部打空、退出码仍 0）——Tauri 模板自身把 `_?=$4` 放最后是对的，但任何手工复现/脚本化测试放错顺序会得到假的"卸载失败"现象（本次排障实踩一轮）

## [0.5.9] - 2026-09-05

### Changed

- **远程访问自动恢复不再弹 Windows 通知**（用户反馈：重启应用自动连回上次链接是后台行为，弹窗是噪音）：复活成功只写 events.log（phase=up 行 + 一条 `auto-resumed, toast skipped` 语义行），托盘远程子菜单状态照常更新；首次开启/换域名重生的 toast 与开启失败的 error toast 均不变

## [0.5.8] - 2026-09-05

### Added

- **远程访问会话跨应用重启保持，链接不再因退出/更新而失效**（用户反馈："关闭后又要重新取一次链接，很麻烦。除非主动重置，不然链接不该失效"）：开启远程后 cloudflared 改从数据目录常驻副本（`%LOCALAPPDATA%\DSHDesktop\tunnel\`）运行且**刻意不挂 KILL_ON_JOB_CLOSE Job Object**（防孤儿原则唯一例外——应用退出时内核不连带回收，隧道留活保域名）；会话状态（token/域名/代理端口/隧道 PID/副本路径）落盘 `remote-session.json`。托盘退出/覆盖更新只死代理、隧道留活；下次启动见状态文件即自动复活：核进程映像路径防 PID 复用冒认 → 收养存活隧道 + 同端口重起代理——**链接字节级不变，手机端收藏直接用**，复活 toast 单独文案（"远程访问已自动恢复，链接未变"）。复活校验不过（隧道已死/副本缺失/持久化端口被占）回退全新开隧道：域名换、token 沿用，状态文件重写。注意 token 随状态文件落盘（纯本地、不同步、不落日志），单设备泄露窗口相应从"本次开启期间"放宽到"直到手动重置/关闭"——这是"链接不轻易失效"语义的自然代价，吊销手段不变（重置链接一键掐断现有会话）
- **链接从此只有三个失效时刻**：①手动"关闭远程访问"再开（全新会话，token+域名都换）；②手动"重置链接"（token 轮换，域名不变）；③Windows 重启/断电或 Cloudflare 掐断长连接致隧道进程死亡（域名必换、token 沿用——quick tunnel 无账号模式下域名随机是结构性限制，要永久固定链接只能上命名隧道+自有域名）。应用退出、崩溃重启、覆盖更新都不再换链接

### Changed

- **托盘退出不再杀远程隧道**：远程开启期间退出应用走 suspend（代理随进程消亡、隧道留活保域名），此前退出即整体作废链接；手动"关闭远程访问"语义不变（杀隧道 + 删状态文件 = 会话作废）
- **卸载清场补齐远程残留**：NSIS postuninstall 在 `$UpdateMode <> 1`（非覆盖更新）时杀数据目录常驻隧道副本进程并删除 `tunnel\` 目录与 `remote-session.json`；覆盖安装不动（否则更新后链接失效，违背持久化语义）

## [0.5.7] - 2026-09-05

### Changed

- **远程访问门岗 cookie 改为 30 天长效，手机端不再反复"链接无效或已过期"**（用户反馈）：此前 `__dsh_remote` 是会话 cookie（无 Max-Age），手机浏览器（iOS Safari / 微信内置浏览器）进程一回收就把它丢了——而地址栏已被 302 剥掉 token，cookie 一丢再开页面即 403 失效页，同一次开启期间被迫反复回电脑扫码。现加 `Max-Age=2592000`：同一次开启期间手机端可收藏地址栏 URL 直接复用，杀浏览器/关机重开都不再掉登录。吊销语义不变（"重置链接"轮换 token，旧 cookie 值即刻不匹配）；链接整体仍随每次开启换新（quick tunnel 域名随机是结构性限制，永久固定链接需命名隧道+自有域名）
- **失效门页文案改为分场景指引**：电脑端远程仍开着 → 改点最初那条带 `?token=` 的完整链接即可重新进入（多数"失效"其实只是手机端 cookie 丢了）；重开过远程访问 → 旧链接整体作废，托盘菜单复制新链接

## [0.5.6] - 2026-09-04

### Added

- **远程访问加载过渡页（splash）**（0.5.5 手机实拍反馈：打开远程页面白屏几十秒才出内容，观感如卡死）：dsh 服务端不做任何压缩，经 cloudflared 隧道的远程首连要下载 ~5MB（SPA ~1.2MB + 插件 bundle ~3.7MB，后者因壳侧改写还被迫走 identity）。现代理改写 SPA 入口文档时在 React 挂载点后注入加载过渡页：spinner + "DeepSeek Harness" 标题随 HTML 解析立即可见，深浅色随 `prefers-color-scheme`，提示文案语言随 `navigator.language`（`<html lang>` 是 Vite 模板恒 en 不可靠）；加载超 12s 淡入一行弱网说明；React 挂载完成（#root 出现子节点）即淡出移除。挂载点 needle 收 upstream.rs::SPA_ROOT_MOUNT_NEEDLE 契约常量——上游改版则契约探针翻红、splash 静默不注入（回到白屏，功能不损）
- **代理侧 gzip 压缩**：dsh 不压缩任何响应，远程首连 ~5MB 文本资产全走 identity。现对 ≥4KB 的文本资产（js/css/json/svg/html，含插件 bundle 改写产物）在代理侧缓冲 gzip（spawn_blocking + flate2，zip 传递依赖同树不新增编译单元）——首连传输量降到约 1/3。仅 GET 成功响应 + 客户端宣告 `accept-encoding: gzip` + 无 Range + 未编码 + 已知长度在 4KB~16MB 内才做；变换响应剥 etag/content-length/accept-ranges 并标 `content-encoding: gzip` + `vary: accept-encoding`；压缩失败回退 identity，绝不发半包
- **诊断面板"上次启动"耗时分解**：dsh 每次 Ready 前落 `[dshdesktop] ready: port=N total=Xs http=Ys token=Zs` 分解行（进入 Starting → HTTP 绑定 → token 就绪行三段耗时），诊断面板信息卡新增"上次启动"行（跨会话读 events.log 尾部取最近一次；面板开着期间状态翻转到 Ready 自动刷新）。"启动慢"类反馈从此有分解数据可读。新增命令 `get_last_boot_timing`（build.rs / capabilities / invoke_handler 三处注册，新增 tests/command_registration.rs 锚定三处一致 + 远程 capability 只放行 zoom_ui）
- **events.log 统一时间戳**：append_debug_line 对无时间戳的行统一补 `[HH:MM:SS.mmm]` 本地前缀（壳侧自带戳的 bring_to_front/播放行、cloudflared 的 RFC3339 UTC 行不重复盖）。此前生命周期关键行（Starting/spawn/Ready）全无时间戳，启动时序只能靠外部文件 mtime 反推

### Changed

- **启动时自动检查更新改为默认开**：公开发布仓匿名可读后，settings.json 无此字段的老配置升级即获得启动检查；显式关过的用户不受影响（字段已落盘，serde default 只兜缺省）
- **手机端隐藏 Session 日志下载按钮**（0.5.5 手机实拍反馈）：没有人在手机上翻 session 日志（要看也是在 PC 端看），且该药丸悬浮盖住会话头部"N 个后台任务运行中"文案。mobile.css ≤700px 下 `_sessionLogButton` 规则由缩小改为 `display: none`（桌面与宽屏远程保持原生显示；上游类名改名则按钮复原显示、契约探针守门，功能不损）

### Fixed

- **MCP 组件联网安装导致启动卡在"正在启动 dsh"数分钟，甚至超时被杀树白等一轮**（用户反馈，0.5.5 实踩定位）：dsh 的 `dsh web:` 就绪行要等 cordis loader 全部 settle，而 dsh-mcp-client 无条件等 MCP server 连接+首次工具同步完成——MCP 条目配成 `npx 裸名/@latest` 时每次启动都联网解析版本，上游发版（如 context7-mcp 4.0.5 发布当天）后的首次启动全量冷装，慢网下实测 >2-3min。两道工序：①wait_token 从固定 60s 改为**静默预算**——pump 有输出就继续等，见到 npm `will be installed` 警告行切 10min 冷装长预算（旧逻辑把快装完的进程杀树，60s 白等后靠 npm 缓存余温重试才成功）；②splash 检测到冷装信号即从"正在启动 dsh 服务…"切换为"正在下载 MCP 组件（首次联网安装，可能较慢）…"。回归：fake-dsh 新增 npm-install-warn 模拟，冷装长预算不被杀 + 纯静默卡死仍按预算杀树两条集成测试钉死

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
