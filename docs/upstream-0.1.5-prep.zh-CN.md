# dsh 0.1.5 跟版预研（目标 `0.1.5-rc.2`，2026-09-12）

> 本文是 **0.1.2-rc.1 → 0.1.5-rc.2** 跟版的契约与影响面预研，对照
> [upstream-0.1.2-alpha.1-prep.zh-CN.md](upstream-0.1.2-alpha.1-prep.zh-CN.md)
> 的写法。实施步骤见 [docs/superpowers/plans/2026-09-12-dsh-0.1.5-rc.2-upgrade.md](superpowers/plans/2026-09-12-dsh-0.1.5-rc.2-upgrade.md)。

> **证据基线**：本文所有判定来自 npm tarball 实测核对（`0.1.2-rc.1` 与
> `0.1.5-rc.1` / `0.1.5-rc.2` 双版本对照，子包按 lockstep 版本取）。引用形如
> `包名@版本:包内路径`。凡标「未定」的项必须等 `fetch-runtime.ps1` 产出真实
> 运行时树后在**运行中**复验，别照本文猜测直接改代码。

## 一、结论速览

**目标版本**：`@deepseek-ai/dsh@0.1.5-rc.2`（`next` 标签；`latest` = `0.1.5-rc.1`）。
仓内既有规则是「钉最新 rc，不依赖 dist-tag」（0.1.2 跟版时 `latest` 仍指
0.1.1-rc.2、钉的 0.1.2-rc.1 走 `next`），故取 rc.2。rc.2 相对 rc.1 只有两条
客户端外观改动（反馈弹窗确认、交付文件卡片排版/图标），契约面无差异；若 rc.2
真机出问题，回落 rc.1 只需改钉版 + 重抓。

**总体判断：这是继 0.1.2 之后最"轻"的一次跟版——逆向面几乎不动。**

40 余条契约探针已逐条对 0.1.5 真实包预核（§三），**只有 1 条确定翻红**：

- `SESSION_LOG_BUTTON_NEEDLE`（`sessionLogButton`）——该 CSS Modules 本地名在
  `dsh-session-log-export/lib/client.js` 中已消失（同时出现 `moreButton`，语义
  从"下载按钮"变成"更多菜单"，见 §三.15）。**不是机械改名**：手机端隐藏规则的
  理由（药丸盖住"N 个后台任务运行中"文案）要重新实测，再决定隐藏/改锚。

**真正的四项工作量**（都不是"改个常量"）：

1. **会话日志按钮锚点丢失**（探针 + `mobile.css` 规则）——见上，需运行时复验后重定。
2. **新流式上传路由 vs 壳代理全量缓冲**：0.1.5 新增
   `POST /api/session/uploadFileBinary`（`requestBody:"streaming"`、不受 300MB
   buffered 上限约束），而 `remote/proxy.rs` 对**所有**请求
   `axum::body::to_bytes(req.into_body(), REPLAY_BODY_LIMIT=64MB)` 整读（为 401
   重放）——手机端经远程隧道上传大文件会 502（"读取请求体失败"）并在壳进程里
   短暂驻留 64MB。必须给流式路由开旁路（先换 cookie，再流式转发，不做重放）。
3. **新右栏（`rightbar`）+ 文档预览 + 交付文件卡片**：`details` 槽位删除、
   面板体系改为 keyed `main` + `rightbar` + `sidebar.panellist`。桌面无感，
   **手机端 ≤700px 是新增元素**（可能是抽屉/多标签），需新写规则而非"复验零改动"。
4. **会话格式 V3（0 → 3）**：恢复旧会话时生成新格式日志、**保留原文件**，
   但"升级后的会话不支持降级读取"。对壳意味着**发版即单向**——装了 0.5.12
   的用户把会话滚到 V3 后，回滚 dsh 版本会读不回新日志（原文件还在，可作部分
   兜底）。发版说明必须明示；验收必须覆盖"保留 DSH_HOME 的升级首启"。

**另有一项与上游无关但必须先行**：跟版脚本的文档基线同步**当前是坏的**
（`follow-upstream.ps1 -SelfTest` 实测 11/12，红项
`钉版与 upstream.rs 文档基线一致：期望 '0.1.2-rc.1'，实得 '0.1.1-rc.2'`）。
四文件计数断言全部对不上 → 文档同步会**静默 skip 全部四个文件**。跟版前必须先修
（§五），否则这次跟版照样不会翻转 `upstream.rs` 头注与 design 文档。

## 一之补、rc.2 实测结论（2026-09-12 跟版执行中回填）

首跑 `follow-upstream.ps1 -DshVersion 0.1.5-rc.2` + 契约套件（真实运行时）结果：

- **37 条探针全绿，仅 1 条 red**：`sessionLogButton`——与 §一 预测完全一致，无预期外红项。
  就绪行/BrowserAuth cookie/mux/RPC 信封/follow 错误帧字段集/picker 10 针/预设签名/
  内测声明 needle 与版本针/workspace schema/`dsh.sessions.current`/viewport/`#root`
  全部实测成立。`dsh.sessions.current` 实测落 `dsh-api-session-controller/lib/client.js`
  （§三.26 的换包判断成立；探针是全树 tree_find，故仍绿）。
- **新锚点实测**：0.1.5 的 `dsh-session-log-export/lib/client.js` 渲染的是
  `moreButton`——28px 圆角图标按钮、`aria-label=header.more`，下载动作收进它弹出的
  菜单，整条注册进槽位 `conversation.session.header.utilities`。`moreButton` 在
  全 node_modules **唯一命中**（无同名歧义，可安全当锚）。落地改动：
  `upstream.rs` 的 `SESSION_LOG_BUTTON_NEEDLE` → `SESSION_HEADER_MORE_BUTTON_NEEDLE`，
  `mobile.css` 选择器随之改 `_moreButton`（含注释说明"按钮→菜单"的语义变更）。
- **新增 6 条探针**（流式上传路由 + `requestBody: streaming`、open-in-app 三路由、
  `rightbar`/`main`/`sidebar.panellist` 槽位、会话格式 V3、命令服务 `attachments`、
  minimal 预设已无文件编辑工具）**首次即绿**——它们守的是本次没有改代码但语义变了的
  那几处。
- **冒烟预算修正**（跟版过程中实踩）：`fetch-runtime.ps1` 的就绪行轮询 30s 曾在
  首跑报"stdout 未出现就绪行（token）——上游就绪行契约漂移？"，但同一棵树手动起
  5s 就绪、就绪行字符串与 0.1.2 逐字相同——是 `npm install`（500+ 包）+ `prune`
  （删约 2 万文件）之后的环境抖动被误判成契约漂移。已把预算提到 120s，并让失败路径
  **打印 dsh 原始输出**（有报错=启动失败，空输出=真漂移），失败不再需要人工复现。
- **跟版脚本文档同步复活**（§五）：修数据 + 把计数不符从"警告跳过"改成"整步显式报错"，
  SelfTest 加"四文件钉版引用计数 == 期望"断言（`upstream.rs=1 / design=3 / README×2=1`），
  现 13/13 绿。
- **会话内视觉项已补验**（2026-09-12，用真实数据副本 + Playwright；原计划留真机，实际用
  "拷贝 `%LOCALAPPDATA%\DSHDesktop\dsh-home` → 独立 dsh 0.1.5 起在副本上"的办法在本地复刻了
  真实会话，两件事一次验完）：
  ①**老会话完整可读**：工作区列表（6 组）与 48 条会话索引正常，打开 0.1.2 时代的会话
  （`/picad-modeling 帮我设计一把梳子`）**全文重放无损**（工具调用/思考块/任务队列/用量统计
  都在）；磁盘实证迁移语义——原 `session.jsonl.zstd`（385,117 B）**原样保留**，新生成
  `session.v3.jsonl.zstd`（149,297 B）；
  ②**移动端（390×844）**：`_moreButton` 注入前可见、注入 mobile.css 后**被隐藏**（新锚点规则
  在真 0.1.5 UI 上生效）；`项目/信息` 标签照常注入（原生 `对话/轨迹` 旁）；`_rightbarCol`
  仍 0 宽；**零 console/page 报错**。
- **移动端两处适配修正（0.1.5 实证驱动）**：①上游自带附件按钮（`aria-label="添加附件"`，
  任意文件类型）与我们注入的图片版（"添加图片附件"）在输入行**并排成了两个回形针**——
  `mobile.js` 改为先探测原生入口，存在即不注入并摘掉残留（上游再撤掉该入口会自动回退到注入）；
  ②会话用量统计 0.1.5 改成**两个紧凑药丸**（`aria-label="1 轮 16 步 · 84 tok/s"` /
  `"778K tok · 缓存命中 0%"`，在输入卡片内，窄屏不溢出）——旧「隐藏原统计行 + 克隆进信息面板」
  的锚点已不再是新元素，规则自然失效（功能不损、最多在信息面板里重复展示一次，纯外观项）。
- **会话内视觉项本地无法验证**（原始记录，现已由上面的副本法补验）：会话要先发消息才生成
  （需模型凭据），且本机回环下 dsh 的目录选择器决议为**原生 Win32 对话框**（Playwright 不可
  驱动）——用临时 DSH_HOME + 种工作区只能走到空态 composer。

## 二、上游 0.1.5 变更清单（发布说明，供 CHANGELOG/Release 摘用）

`0.1.5-rc.1` 是 0.1.5 系列首候选，汇总了自 `0.1.2-rc.1` 以来的变更
（GitHub release `dsh-v0.1.5-rc.1` / `dsh-v0.1.5-rc.2`）。与壳相关的：

- **Web 通用文件上传**（任意类型，与图片同区混排，进度/取消/切会话续显）；
  **`/feedback` 独立提交**；**Skill 选择器模糊搜索**；**Web 顶栏"在应用中打开"**。
- **右侧 Sidebar**（多标签/分栏/全屏，Markdown/代码/HTML/PDF/图片预览，子代理
  与未激活会话的文件；**原 Detail 面板移除**）；模型可显式交付文件。
- **出站请求遵循 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY`**。
- **Windows 本地非终端子进程不再弹控制台窗口**。
- **MCP 工具发现拒绝重复分页游标**（不再挂住启动/同步）——顺带覆盖壳的 npx 冷装卡顿场景。
- **会话格式 V3**；会话持久化 API 归 `SessionHandle`，新增会话锁（同一会话至多一个进程持有）。
- **默认工具调整**：Web `minimal` 只给持久 shell，`str_replace_editor` 需显式开启
  （SDK/Headless/ACP 默认用 read/write/edit）。
- **插件 API 变动**：移除 `ctx.agent`；`Inbox` 改为类型接口（`hasPending`/`claim`
  退出公共 API）；**Web 插件面板 API：`sidebar.panellist` + `main`，原 `conversation`
  槽位迁为 `main` 的 `conversation` key**。
- 其他：persona 配置拆 prefix/suffix（`existing configurations and related constants
  need to be updated`）；pi-ai 0.85.1；可选子代理插件升 Codex 0.153.4 / Claude Code 2.1.263；
  实验性 Agent Teams 包可从 npm 装（不默认启用）；Windows 普通子进程清理改进
  （普通子进程 handle 不再暴露 pid，终端 handle 不变）。

## 三、契约逐项核对（`0.1.2-rc.1` → `0.1.5-rc.2`）

判定图例：**保持** = 我方零改动；**改针** = 只需改 `upstream.rs` 常量；**改码** = 动消费模块。

| # | 契约面 | 0.1.2-rc.1 | 0.1.5-rc.2 | 判定 |
| --- | --- | --- | --- | --- |
| 1 | Node 要求 | tarball 无 `engines` 字段（上游仓库口径 `^22.19 \|\| >=24`） | 同 | 保持（随包 Node 24.19.0 不动） |
| 2 | 入口 | `lib/bin.js`（`bin.dsh`） | 同 | 保持 |
| 3 | `dsh web` 旗标 | `startup.js`：`--port <port>`、`--no-open`、`--host`（拒 0.0.0.0）、`--trusted-host` | **逐字节相同** | 保持（`--no-open` 仍必须） |
| 4 | 启动器 `bin.js` | 无 `--from-default-profile` | 新增 `--from-default-profile <name>` + 拒 `profile "desktop"`（Electron 门） | 保持（壳走 `web` profile，不相关；仅文档记录） |
| 5 | stdout 就绪行 | `` dsh web: ${authenticatedUrl} ``（`/?token=`） | 字符串同（仅 `launchedThroughSsh` 改从 `@deepseek-ai/dsh-launch-environment` 导入） | 保持（`READY_URL_PREFIX` 不变；fetch 冒烟同款解析） |
| 6 | BrowserAuth cookie | 前缀 `dsh-auth-`、`v1.<body>.<sig>`、`Max-Age=2592000`、`HttpOnly; SameSite=Strict`、绑 authority | 全同（`dsh-client-connection`：前缀/cookie 构造/303 流/`cookieMaxAgeDays` 默认 30） | 保持（`dsh_session.rs` 零改动） |
| 7 | mux 传输 | `/api/remote.mux`；`{type:"open",streamId,endpoint,payload}` / `cancel`；下行 `item`/`end`/`error{code,message,details}` | `stream-protocol.js` **逐字节相同** | 保持（`notify/mux.rs` 零改动） |
| 8 | HTTP RPC 信封 | `POST /api/<method>`，`client-request` → `server-response{result:{ok,value}}` | 同 | 保持 |
| 9 | `$events` 桥 | `open` 空 args → ready；`emit`/`waterfall` 帧形 | 帧形同；转发清单**新增** `{event:'goal/activation-changed', mode:'emit'}` | 保持（壳忽略未知事件；可选：加进 `upstream.rs` 词表做记录） |
| 10 | `session/follow` | args 包 `{request:{address:{kind:"session",sessionId}}}`；`turn/start`/`tool/call`/`turn/end`+`reason.kind=completed`；`api-session/added` 的 `origin` | 事件类型集 **相同**；请求新增可选 `assistantStream?: true`；wire header `seedLength` 移除、加必需 `isSeeded`、surfaceOp `start/end`→`startSeq/endSeq`、`chunk-rows` 删除 | 保持（壳只读 `event` 帧、**忽略 snapshot 帧**、不读上述字段；已核 `upstream.rs`/`notify` 无引用） |
| 11 | 插件 bundle 路由 | `/plugins/??<id>/client.js,…&rev=N`；`cache-control: public, max-age=31536000, immutable` | `comboUrl`/`IMMUTABLE_CACHE`/`MAX_COMBO_URL_BYTES=3*1024` 全同 | 保持（`proxy.rs` 缓存击穿逻辑零改动） |
| 12 | 面板槽位 | `sidebar` / `conversation` / `details` / `shell.overlay` | `sidebar` / **`main`（keyed，保留 key `conversation`）** / **`rightbar`** / `shell.overlay`；sidebar 内新增 **`sidebar.panellist`**；**`details` 删除** | **改码风险**（`picker.rs` 钉的 browse 表面占 `ui-workspace` 的 `directory-flow` 槽位 → 运行时复验是否仍解析；`mobile.js` 标签注入复核） |
| 13 | 预设目录 | `node_modules/@deepseek-ai/dsh-agent-presets/presets/minimal` | **同路径** | 保持（`PRESET_DIR_SEGMENTS` 不动） |
| 14 | 预设签名哨兵 | `dsh-tool-bash-persistent` 仍在 + win32 门控 | **仍在**（7 处 `win32`；只是删掉了 `filesystem` 组） | 保持（`presets.rs` 判 `UpstreamHandled`；但 minimal 语义变了，见 §四.6） |
| 15 | 会话日志按钮 | `dsh-session-log-export/lib/client.js` 含 CSS 本地名 `sessionLogButton` | **`sessionLogButton` 消失**（该包只剩 `sessionLogDownload` 服务名 + `moreButton`） | **红**：`SESSION_LOG_BUTTON_NEEDLE` 探针翻红 + `mobile.css` 隐藏规则静默失效（§四.4） |
| 16 | settings.yaml 键 | `ui-theme.preference` / `locale.preference` | 命名空间与字段名全同（`dsh-client-ui-theme`、`dsh-client-locale`） | 保持 |
| 17 | 内测声明 | `ui-onboarding.welcomeNoticeVersion`；文案版本 `WELCOME_NOTICE_VERSION = "2026-08-13.1"`；三元式 needle（接收者前缀 `ctx.remote.$host.` 形态） | 命名空间/字段/版本值全同；`WELCOME_NOTICE_NEEDLE` 仍命中 `dsh-client-ui-settings/lib/client.js` | 保持（`welcome.rs` + `proxy.rs` 改写零改动） |
| 18 | MCP 补丁条目 | `@deepseek-ai/dsh-mcp-client`；stdio/http 两种 transport（无 sse）；`disabled` 为 loader 行属性 | 配置联合体同；仅新增重复 `tools/list` 游标检测 | 保持（`mcp.rs` 零改动） |
| 19 | `dsh plugin` CLI | `bin.js` 有 `command("plugin")` + `requiredOption("--profile <name>")` | **两串都仍在** | 保持（`plugins.rs` 零改动） |
| 20 | 目录选择器 | web-app patch 有 `id: directory-picker` 的 auto 行；browse host/client 包存在 | 行与两个包**都仍在**（`^0.1.5-rc.1`）；host 2 针 + client 8 针**全命中** | 保持（`picker.rs`/`pickerpatch.rs` 补丁锚点不动；运行时复验钉入是否解析） |
| 21 | 移动端锚点 | `_tabs`/`_tabActive`/`_composerStack`/`_trailing`/`_content`（conversation）、`_sidebarCol`/`_handle`/`_sidebar`（layout）、`_panel`（chat）、`_nav*`（settings-general）、`_frame`、`_card`、`_root` | **全部仍命中**（逐包核对） | 保持（仍需 Playwright 截图复验，外观可能位移） |
| 22 | 模型选择器 | `conversation.input.model` 槽位；`triggerLabel`/`triggerEffort` | **全命中** | 保持 |
| 23 | `/init` 折叠行 | `source.kind !== "user"` 分支、`contextInjection` locale 键、`noticeSummary` | **全命中**（`dsh-client-ui-chat`） | 保持（预装插件 dsh-command-init 无需改） |
| 24 | 命令服务 API | `ctx.commands.register({name,description,input,handler})`；`input.images` 布尔旗标 | `register(definition)` 同；**`images` 改名为 `attachments`**；新增 file-receipt resolver | 保持（我方插件不含 `images`/`attachments` 旗标，已核） |
| 25 | 会话格式版本 | `SESSION_FORMAT_VERSION = 0` | **`3`** | **用户数据单向**（§四.2） |
| 26 | 当前会话 localStorage 键 | `dsh.sessions.current`（`@deepseek-ai/dsh-client-runtime`，该包 0.1.2 后停发） | 值仍在，**归属包变为 `dsh-api-session-controller/lib/client.js`** | 保持（`probe_project` 是 `tree_find` 全树搜 → 仍绿；仅 `upstream.rs` 注释里的出处要改） |
| 27 | workspace 注册表 | `$DSH_HOME/storages/workspace.json`，`sessionIds` 锚点 | **仍命中** | 保持（`project.rs` 零改动） |
| 28 | 入口文档注入点 | viewport meta（Vite 原值）+ `<div id="root"></div>` | **两针都命中**（`dsh-web-frontend/dist/index.html`） | 保持（`proxy.rs` viewport 改写 + splash 注入不动） |
| 29 | 静态资产路由 | `dsh-host-frontend-static`（index 鉴权、其余公开） | **逐字节相同**；dist 目录结构不变（仅哈希文件名变） | 保持 |
| 30 | 新静态路由 | 无 | **新增 `dsh-host-open-in-app`**：`/open-in-app/apps`（GET）、`/open-in-app/icon`（前缀 GET，`max-age=3600`）、`/open-in-app/open`（POST），均**在鉴权门外** | **改码**：代理必须原样透传（当前是通用反代，预期自带；需真机点一次"在应用中打开"验证；手机端评估隐藏入口） |
| 31 | 上传路由 | 无（仅 buffered 请求体，`DEFAULT_MAX_REQUEST_BODY_BYTES=300MB`） | **新增 `POST /api/session/uploadFileBinary`，`requestBody:"streaming"`，不受 300MB 上限** | **改码**：壳代理流式旁路（§四.1） |
| 32 | HTTP 请求头上限 | `createServer` 无 options（Node 默认 16KB） | 同（无 `maxHeaderSize`） | 保持（壳 `--max-http-header-size=65536` 仍是唯一防线） |
| 33 | Windows 控制台 | `subprocess-local` 无 `windowsHide` | 新增 `windowsHide: platform === "win32"`（含 taskkill/spawnSync）；新包 `dsh-win32-process` | 保持（上游自修，与壳 `CREATE_NO_WINDOW` 并行；`console_window.rs` 验收应仍绿） |
| 34 | 出站代理 | 无（无 `dsh-http-proxy` 包） | **新增 `@deepseek-ai/dsh-http-proxy`**：读 `http_proxy/HTTP_PROXY`、`https_proxy/HTTPS_PROXY`、`no_proxy/NO_PROXY`、`ALL_PROXY`，装 undici 全局 dispatcher；**回环永不走代理**（`LOOPBACK_NO_PROXY` + `isLoopbackHost` 覆盖 `127.0.0.0/8` 与 IPv4-mapped） | 保持（利好：Clash 用户的 LLM/MCP 流量终于走代理；回环安全。壳的 reqwest 侧 `.no_proxy()` 不受影响） |
| 35 | npm 依赖面 | 70 个依赖 | **72**：新增 `dsh-http-proxy`、`dsh-tool-present`；web-app 侧新增 `dsh-host-open-in-app`、`dsh-client-ui-open-in-app`、`dsh-api-workspace-files`、`dsh-client-file-upload`、`dsh-client-resources`、`dsh-client-ui-sidebar-right`、`dsh-client-ui-sidebar-documentpreview`、`dsh-client-ui-sidebar-files` | **核查**：`prune-runtime.ps1` 规则 + 体积对比（§四.7） |

**已核销的"伪变更"**（发布说明里有但 tarball 中无对应物，别去追）：

- "修复 npm 安装需要依赖 `fs-ext` 本地编译"——`fs-ext`/`fs_ext` 在 0.1.2 与 0.1.5
  的 tarball、壳仓库、本机运行时树里**都不存在**；原生文件层走 `koffi` FFI。
  该条属上游内部事务（可能指 Python SDK 面），对壳无动作。
- "persona 拆 prefix/suffix"——壳不写 persona 配置（`grep` 全 `src-tauri` 无命中），
  仅影响在 `$DSH_HOME` 里手写过自定义 persona 的**用户**，进发版说明即可。

## 四、壳侧影响清单（按模块，含"改哪"）

1. **`remote/proxy.rs` 流式上传旁路（改码，本次唯一功能级改造）**
   现状：`forward()` 对每个请求 `axum::body::to_bytes(req.into_body(), 64MB)`。
   0.1.5 的流式路由（`/api/session/uploadFileBinary`）在 dsh 侧无上限，经手机隧道
   传大文件必撞壳侧 64MB 上限 → 502；且 64MB 常驻壳进程不合理。
   设计：把"流式路由"清单收进 `upstream.rs`；命中则 ①先 `ensure_cookie(force)`
   主动换 cookie（**不做 401 重放**——消费过的流无法重放）②`Body::from_stream`
   直通 `reqwest`。其余路由保持"整读 + 401 重放一次"的既有语义。
   测试：假 dsh 增一条流式路由 + 一个超大 body 用例，断言不整读、不被 64MB 限制
   截断（用可配阈值，别真造 64MB+）。

2. **会话格式 V3（无代码改动，验收 + 说明义务）**
   dsh 在恢复旧会话时生成 V3 新日志并保留原文件；**升级后不支持降级读取**。
   壳侧动作：`acceptance.ps1` 的"卸旧不删 DSH_HOME"路径必须验证"装 0.5.12 后
   恢复 0.1.2 时代会话，标题/内容完整、无报错"。发版说明明示不可回滚 dsh。
   `project.rs`（workspace.json / `dsh.sessions.current`）另跑一次真机确认。

3. **`remote/mobile.css` / `mobile.js`（改码，新增规则）**
   - 新右栏 `rightbar`（多标签/分栏/全屏 + 文档预览）与交付文件卡片：≤700px 下
     是否遮挡输入区/标签栏，冲突则隐藏或抽屉化——**新增规则，不是复验**。
   - "在应用中打开"入口在手机上无意义（无桌面应用可拉起），评估隐藏。
   - `sessionLogButton` 锚点丢失（探针红）；先在真实运行时树里找导出入口的新
     形态（同包新名候选 `moreButton`，语义已从"按钮"变"更多菜单"），再决定
     隐藏规则——**别盲改**。
   - 保持项复验：标签条（项目/信息）、StatsLine 隐藏、模型选择器图标化
     （`triggerLabel`/`triggerEffort` 两段，含 "Default" 态）、`/init` 折叠行、
     附件按钮（dsh 现在原生支持通用文件上传，评估我方"回形针"按钮是否仍需要）。

4. **探针/常量（改针）**
   `SESSION_LOG_BUTTON_NEEDLE` 重定锚；新增常量：上传流式路由、`/open-in-app/*`
   路由族、面板槽位改名、`goal/activation-changed`、会话格式版本、命令服务
   `images`→`attachments`、minimal 预设单工具事实；`upstream.rs` 头部基线行
   **留给跟版脚本翻转**，改针时不得引入额外旧版字面量（§五）。

5. **`picker.rs` / `pickerpatch.rs`（复验，预期零改动）**
   10 条补丁锚点 + auto 行 id 全命中，包也都在。但 `picker.rs` 钉的 browse 表面
   挂在 `ui-workspace` 的 `directory-flow` 槽位，而槽位体系动了（`conversation`→
   `main`、新增 `rightbar`/`panellist`）——**必须在真机点一次目录选择器**确认
   browse 对话框仍然起来（起不来 = 远程手机端彻底选不了目录）。

6. **`presets.rs`（保持，但要改文档口径）**
   minimal 预设仍含 `dsh-tool-bash-persistent` + win32 门控 → 哨兵仍 `UpstreamHandled`。
   但语义变了：**极简模式从"持久 shell + str_replace_editor"降为"只有持久 shell"**，
   `filesystem` 组整块删除（`fs-local`、`str-replace-editor` 都不再挂）。
   用户可感（极简模式下模型不再有文件编辑工具），进发版说明。

7. **`scripts/prune-runtime.ps1` + 体积（核查）**
   新增 8 个包与 `dsh-win32-process`/`koffi` 相关原生面，需对比 0.1.2 运行时体积、
   确认没有新的测试/文档/多平台二进制残留；必要时加规则。

8. **`notify/`（保持）**：事件类型集与帧形未变，新增的 `goal/activation-changed`
   自动被忽略；`notify/mod.rs` 本就忽略 `snapshot` 帧，wire header 变动不影响。

## 五、跟版脚本缺陷：文档基线同步现在是坏的（必须先修）

`powershell -File scripts/follow-upstream.ps1 -SelfTest` 实测 **11/12 通过**，红项：

```
[失败] 钉版与 upstream.rs 文档基线一致：期望 '0.1.2-rc.1'，实得 '0.1.1-rc.2'
```

机理（已定位）：`Get-DocBaseline` 从 `upstream.rs` 头注「事实基线」行取旧版本串，
`Set-VersionString` 要求**整文件字面量计数精确等于 Expected**，四文件期望为
`upstream.rs=1 / design.zh-CN.md=3 / README.md=1 / README.zh-CN.md=1`。当前实测：

| 文件 | 期望 | 实际（旧串 `0.1.1-rc.2`） | 结果 |
| --- | --- | --- | --- |
| `src-tauri/src/upstream.rs` | 1 | **3**（头注 + 两处历史对照注释） | mismatch → **跳过** |
| `docs/design.zh-CN.md` | 3 | **1**（一行历史注释） | mismatch → **跳过** |
| `README.md` / `README.zh-CN.md` | 1 | 0 | mismatch → **跳过** |

即 0.1.2 跟版时 `upstream.rs` 因为**历史对照注释里的旧版本串**导致整文件计数
不符，同步被静默 skip —— 头注至今留在 `0.1.1-rc.2`，而 design/README 是手工改的
（design 里新旧两串并存正是这个过程的残留）。**后果**：这次跟版若不先修，脚本
仍会 skip 全部四文件，文档基线继续烂下去。

修法见计划 Task 0：立即修数据（头注置真值 + 把历史对照注释里的版本串改写为
不带 `-rc` 的形态）+ 加固机制（文档同步改为**按头注行单命中替换**，不再整文件
计数；SelfTest 增加"四文件计数 == 期望"断言，防止再次静默腐烂）。

## 六、坑与风险

1. **会话数据单向**（最高优先）：V3 升级不可逆。若 0.5.12 发布后需要回滚，
   用户会话的新日志旧版读不回（原文件保留可作部分兜底）。→ 发版说明明示；
   验收覆盖升级首启；发版前把 0.1.5 的契约面再全量过一遍，别把回滚当预案。
2. **流式上传的 401 语义**：旁路不做重放，必须**先**换 cookie。若 dsh 恰好在这
   一瞬间重启（token/cookie 全换），单次失败可接受（用户重传），但别做成静默丢文件。
3. **手机端右栏是新增规则**：别按"复验零改动"估工。rc.2 还改了交付文件卡片排版
   与图标，截图对比要以 rc.2 为准。
4. **"在应用中打开"**：新路由在鉴权门外、会调起桌面应用——手机端点到会怎样
   （隧道对端是 PC，可能真的拉起 PC 上的编辑器）需实测；若会拉起 PC 应用，
   手机上应隐藏该入口。
5. **代理 env**：dsh 现在读 `*_PROXY`。壳 spawn dsh 时**不要**为回环注入代理变量；
   dsh 自身已豁免回环。用户报"LLM 连不上"时先看是不是被自家 Clash 规则拦了。
6. **会话锁**：同一会话至多一个进程持有——用户在终端另起一个 dsh 指同一
   `DSH_HOME` 可能与壳内实例互斥，值得写进排障说明（不是壳能修的）。
7. **`upstream.rs` 字面量纪律**：改针时不得引入除头注行以外的旧版本串，
   否则 Task 8 的自动翻转又会 skip（Task 0 加固后应改为显式报错，不再静默）。
8. **未定项（跟版执行中更新）**：会话日志入口的新锚点**已定** = `moreButton`（§一之补）；
   会话内视觉项**已用真实数据副本补验**（§一之补）；`rightbar` 已知"无面板打开时是 0 宽列
   （`_rightbarCol`），不侵入布局"，**打开文档/文件面板后的窄屏形态仍未定**；browse 表面在
   新槽位体系下是否仍解析**仍未定**——这两项要真机点一次（真机核：目录选择器、右栏打开态、
   手机端大文件上传、通知去重；本地副本法已覆盖老会话读取与移动端主要适配）。

## 七、证据索引

均为 npm tarball 实测（子包按 lockstep 版本取；路径相对包根）：

- **鉴权/就绪行**：`@deepseek-ai/dsh-web-app@0.1.5-rc.1:lib/index.js`（就绪行字符串与
  0.1.2 逐字相同；`startup.js` 逐字节相同）；`@deepseek-ai/dsh-client-connection@0.1.5-rc.1:lib/index.js`
  （`dsh-auth-` 前缀、cookie 构造、303 交换、`cookieMaxAgeDays` 默认 30）。
- **传输**：`@deepseek-ai/dsh-api-gateway@0.1.5-rc.1:lib/types/stream-protocol.js`
  （逐字节相同：mux 路径、`$events`/`$events/result`、帧校验）；
  `@deepseek-ai/dsh-api-remotes@0.1.5-rc.1:lib/types/remote-events.js`（新增
  `goal/activation-changed`）。
- **槽位**：`@deepseek-ai/dsh-client-ui-layout@0.1.5-rc.1:lib/types/client/index.d.ts`
  （`main` keyed + `rightbar`，无 `details`）；`.../lib/client.js`（`renderSlot("main")`/
  `renderSlot("rightbar")`）；`@deepseek-ai/dsh-client-ui-sidebar@0.1.5-rc.1:contract/slots.d.ts`
  （`sidebar.panellist`）。
- **预设**：`@deepseek-ai/dsh-agent-presets@0.1.5-rc.1:presets/minimal/agent.cordis.yml`
  （`filesystem` 组删除；`dsh-tool-bash-persistent` + `process.platform === 'win32'`
  门控仍在）。
- **会话格式**：`@deepseek-ai/dsh-session@0.1.5-rc.1:lib/index.js`（`SESSION_FORMAT_VERSION = 3`）。
- **上传/新路由**：`@deepseek-ai/dsh-client-file-upload@0.1.5-rc.1:lib/index.js`
  （`/api/session/uploadFileBinary`、`requestBody:"streaming"`）；
  `@deepseek-ai/dsh-client-connection@0.1.5-rc.1:lib/index.js`（streaming body 走
  `Readable.toWeb`，buffered 仍 `DEFAULT_MAX_REQUEST_BODY_BYTES=300MB`）；
  `@deepseek-ai/dsh-host-open-in-app@0.1.5-rc.1:lib/index.js`（三条 `/open-in-app/*`）。
- **代理 env**：`@deepseek-ai/dsh-http-proxy@0.1.5-rc.1:lib/index.js`（`LOOPBACK_NO_PROXY`、
  `withLoopback`、`isLoopbackHost` 覆盖 `127.0.0.0/8`）。
- **Windows 子进程**：`@deepseek-ai/dsh-subprocess-local@0.1.5-rc.1:lib/runner-launch-*.js`
  （`windowsHide: platform === "win32"`）。
- **命令服务**：`@deepseek-ai/dsh-commands@0.1.5-rc.2:lib/index.js`（`images` → `attachments`；
  `register(definition)` 签名不变）。
- **逐针命中记录（本次核对，`0.1.5-rc.2` 包）**：`sessionLogButton` 消失（该包仅剩
  `sessionLogDownload` 服务名 + `moreButton`）；`dsh.sessions.current` 落
  `@deepseek-ai/dsh-api-session-controller`；`viewports`/`#root` 挂载点、
  `conversation.input.model`、`triggerLabel`/`triggerEffort`、`contextInjection`/
  `noticeSummary`/`source.kind !== "user"`、picker host 2 针 + client 8 针、
  `directory-picker` auto 行、`command("plugin")`/`requiredOption("--profile <name>")`
  全部命中。

## 八、跟版 runbook（战略视图；执行按计划文档）

1. **先修跟版脚本锚点**（§五），`-SelfTest` 回到全绿。
2. `powershell -File scripts/follow-upstream.ps1 -DshVersion 0.1.5-rc.2`（**不带
   `-Bump`**：版本进位交给 `pnpm release`）；预期红项只有会话日志按钮一条，
   其余按 §三 全绿（红了先按输出改 `upstream.rs`）。
3. 按 §四 逐项：改针 → 代理流式旁路 → 移动端新规则 → 升级路径验收 → 远程真机 →
   精简/体积 → 文档。
4. 发版走 `pnpm release`（0.5.11 → 0.5.12，机械步骤全自动 + 双仓上传 + 匿名终验）；
   发版说明双语成文放 `docs/release-notes/v0.5.12.md` / `.zh.md`，**必须写明会话
   格式 V3 单向、极简模式工具收缩、手机端文件上传**三件用户可感变化。
