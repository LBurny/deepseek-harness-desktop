# dsh 0.1.2-alpha.1 跟版预研（npm 未发布，2026-08-28）

上游已打 tag `dsh-v0.1.2-alpha.1`（prerelease，2026-08-27 发布），**npm latest 仍为 0.1.1-rc.2**，
`fetch-runtime.ps1` 暂时抓不到。本文基于 GitHub tag 源码树（commit cd5ef81）对照现基线
0.1.1-rc.2（b150a55）逐项核对壳侧事实，并把新版**全部相关线上接口**（HTTP/WS/RPC/事件/
CLI/配置）整理成契约篇——正式版发布后照 §五 runbook 行动，实现直接对 §二~§四 契约写代码。

核对方法：解包两版 tag tarball 逐包 diff + 精读传输/鉴权/事件源文件，全部结论有 file:line
证据（§六）。**等正式版发布，我们再行动**；prerelease 阶段（alpha.1，后续可能 alpha.2…）
不动代码，只消化本文。

## 一、结论速览

🔴 **大改——壳三条链路必须动**：

1. **Web 鉴权（全新机制，影响最大）**：一次性 launch token + 签名 cookie（`BrowserAuth`），
   **无关闭开关，回环也在门内**。壳的 WebView 导航、通知 WS、远程反向代理三条路径全部
   受影响（§二）。
2. **事件传输重写**：`/api/events.mux` + `/api/events.host` 移除 → 单一 WS mux
   `/api/remote.mux` + HTTP POST `/api/<endpoint>` RPC；turn 级会话事件从全局广播改为
   per-session `session/follow` 流（§三、§四）。
3. **shipped 预设搬家**：`apps/cli` 的 `files` 收缩为 `lib/*.js`，预设移入
   `@deepseek-ai/dsh-agent-presets` 包（`files: lib + presets`）。upstream.rs
   `PRESET_DIR_SEGMENTS`（现指 dsh 包内 `config/agent-presets/minimal`）必漂移，presets.rs
   探测与契约套件同红。预设**内容**零变化（presets 目录 diff 为空）。

🟡 **中改——复验/适配**：

4. **移动端适配**：聊天渲染从 `ui-conversation` 拆入新包 `@deepseek-ai/dsh-client-ui-chat`
   （CSS Modules 哈希全变，本地名子串匹配的规则大多存活但须 Playwright 复验）；新增回合
   导航右轨 / token 用量展开 / 宽度拖拽手柄 / 字号调节四个新块需评估 700px 断点（§四·6）。
5. **WS 心跳**：网关每 30s 发协议层 Ping 帧（0.1.2 新增）。tungstenite 自动回 Pong，
   ws.rs 现有 PING_IDLE(60s)/PONG_TIMEOUT(30s) 看门狗兼容（服务端 Ping 属"任意下行帧"，
   会复位探活计时），无需改。
6. **isLoopback 语义放宽**：新计算式并入 `transport?.ownsHost === true`；
   `connection.isLoopback ? 'host' : 'memory'` 三元在源码仍原样
   （ui-settings/settings-scope.ts:291、client/index.ts:60），welcome notice 改写
   needle 待真实产物验证。

🟢 **不变——契约套件预期全绿项**：

- 命令形：`web` alias、`--port`、`--no-open`（web-app/startup.ts:52-53）、入口
  `lib/bin.js`；engines 仓库根 `^22.19.0 || >=24.0.0` 未变（tarball 本就不带，契约测试已按
  rc.6 实测注释处理）
- 设置：`ui-theme.preference` / `locale.preference` 键、settings-file 落盘机制
  （withFileLock + writeFileAtomic）原样——壳的文件轮询跟主题可平移
- picker：shipped `cordis.patch.yml` 的 `- id: directory-picker` 行原样
  （bundle/web-app/cordis.patch.yml:78-79）；browse host 两个锚点行原样（index.ts:222
  fullyQualified 校验、:35 parent===current，该文件 diff 仅注释）；browse client 全部锚点
  原样（showHidden useState(false):292、setShowHidden(false):582、homeIndex===-1:118、
  crumb.name:834、两个 disabled 表达式 :968/:998、locale browser.showHidden zh/en 位于
  client/index.ts:51/:66）。本次「开」字截断修复落在 **native** 对话框包
  （directory-picker-native/src/win32-dialog-bindings.ts），与壳补丁的 browse 包无关
- plugin 子命令：thin pnpm forwarder 逐行未变（plugin.ts diff 仅 initProfile 模板形）；
  profile 布局 `$DSH_HOME/profiles/<name>/`（package.json + cordis.patch.yml + pnpm
  node_modules/.bin）原样。**注意**：profile-boot.ts 删除了「agent-presets 行 roots 无条件
  重写为 shipped root」的补丁（即发布说明修复项"Profile 配置的 Agent Preset 目录在启动时
  丢失"），AGENTS.md 里「预设不能经 profile patch 影子覆盖」的旧结论届时需重新验证
- workspace 存储域 schema（storages/workspace.json、tables.workspaces、sessionIds）逐字
  原样；`dsh.sessions.current` 键保持（实现从 client-runtime 迁到 api/session-controller，
  值对象多可选 `subagentAddress` 字段，读 sessionId 兼容）
- locale 字典扁平 `Record<string,string>` 形态与 register API 原样——browse picker 补丁
  注入的 `browser.drives` 自定义键兼容
- `sessionLogButton` 类名与 32px/111px 尺寸原样（仅文本改走 locale 键）；welcome notice
  键（ui-onboarding / welcomeNoticeVersion / WELCOME_NOTICE_VERSION 值仍 `2026-08-13.1`）
  原样；模型槽位 `data-slot="conversation.input.model"` 与 `triggerLabel`/`triggerEffort`
  类名原样

## 二、接口契约：Web 鉴权（BrowserAuth）

### 2.1 机制与存储

| 要素 | 事实 |
|---|---|
| launch token | 每 dsh 进程一枚，`randomBytes(32)` base64url（43 字符），进程内存持有（browser-auth.ts:50-57）；每次 dsh 重启换新 |
| 签名 secret | 32 字节随机，首次生成后持久化在 DSH_HOME credentials（记录键 `client-connection/browser-session`，browser-auth.ts:14,190-213）——**跨重启有效**，cookie 因此跨重启可用 |
| cookie 名 | `dsh-auth-` + base64url(sha256(authority))，authority = 请求 Host 头（`127.0.0.1:<port>`）——**换端口即新 cookie 名**（:16,:106-108） |
| cookie 值 | `v1.<base64url(JSON 载荷)>.<base64url(HMAC-SHA256)>`，载荷 `{version:1, authority, issuedAt, expiresAt}`（:110-142） |
| cookie 属性 | `Max-Age=<30天秒>; Path=/; Expires=…; HttpOnly; SameSite=Strict`（:145-149） |
| 有效期 | `cookieMaxAgeDays` 配置，默认 30 天（connection index.ts:88,102） |

### 2.2 token 交换流（壳要实现的唯一一步）

```
GET http://127.0.0.1:<port>/?token=<launchToken>
  Host: 127.0.0.1:<port>
→ 303 { Location: /, Set-Cookie: dsh-auth-…=…, Cache-Control: no-store, Referrer-Policy: no-referrer }
→ 之后 GET / 无参数（带 cookie）→ 200 index.html
```

判定细节（browser-auth.ts:240-287）：

- 只认 `GET` + pathname 精确 `/` + **单个** token 参数 + Host 头可解析；token 常量时间比较
- 带有效 cookie 又带 token 访问 `/` → 303 去 `/`（清洁化）；token 错/多个/非 GET → 401
- 401 响应体：`dsh web authentication required; reopen the URL printed by dsh web.\n`
  （text/plain；HEAD 无体）

### 2.3 鉴权判定矩阵（什么在门内）

| 请求 | 门 | 无凭证结果 |
|---|---|---|
| `GET /`（HTML 入口，经 static fallback 的 index） | authorizeIndex | 401 |
| 其余静态资源（`/assets/*` 等，非 index 目标） | **无门**（frontend-static:88-95 只对 index 调 authorizeIndex） | 200（公开） |
| `POST /api/<endpoint>`（全部 RPC） | requestRejection | 401（无/坏 cookie）或 403（Host 栅栏） |
| `GET /api/remote.mux` WS upgrade | requestRejection | 401/403（原始 HTTP 响应 + `Connection: close`，stream-server.ts:197-208） |
| 静态目录穿越 | 路径校验 | 403 |

Host 栅栏（api-request-trust.ts:91-118，403 优先于 401）：

- Host 头必须存在且可解析；hostname 为回环（`localhost`、`[::1]`、`127.x.x.x`）或命中
  `trustedHosts` 条目（精确 `host:port`，或不带端口的 `host` = 任意端口）
- `sec-fetch-site: cross-site` → 403；Origin 存在时必须与 Host 同源（`"null"` 也拒；
  无 Origin 合法——Rust 客户端天然满足）

### 2.4 CLI / 配置面

- 旗标（web-app/startup.ts:50-54）：`--host <host>`（绑 host；**`--host 0.0.0.0` 被 CLI
  显式拒绝**，"intentionally not supported yet for safety"）、`--no-open`、
  `--port <port>`（数字，`0`=OS 分配）、`--trusted-host <authority…>`（可重复）
- webserver 配置：`host: '127.0.0.1'|'0.0.0.0'`、`port`（required，0=OS）——壳总显式传
  `--port N`，不受默认影响
- ConnectionConfig：`trustedHosts`（默认 `[]`）、`cookieMaxAgeDays`（默认 30）、
  `maxRequestBodyBytes`（默认 300MiB）
- **没有关闭鉴权的开关**——BrowserAuth 无条件装配，回环也必须走 token 交换

### 2.5 stdout 就绪行（token 来源）

```
dsh web: http://127.0.0.1:<port>/?token=<43字符base64url>
```

- printUrl 默认开（bundle/web-app/src/index.ts:45-56,281）；绑回环时无 LAN 后缀
  （LAN URL 仅 0.0.0.0 绑定时出现：`… (LAN: http://<ip>:<port>/?token=…)`）
- **时序竞态**：该行在 Loader 树装配完成后才打印（announceReady 等 `loader.await()`，
  :264-275），而 HTTP server 绑定早于此——`wait_ready` 可能先探测到 401 响应而 stdout
  还没出行。壳的 token 捕获必须**持续 pump stdout 直到拿到 token 或超时**（READY_TIMEOUT
  60s 内正常都会出行），不能假设 ready 时 token 已在
- `--no-open` 照旧必须带（openBrowser 默认 true；带的后果只是多打印一行
  `dsh web: opening the default browser…`）

### 2.6 壳侧改造设计（三条链路）

1. **主窗口导航**（process.rs + lib.rs）：
   - stdout pump 现在只落日志——加一条解析：正则 `dsh web: (http://127\.0\.0\.1:\d+/\?token=[A-Za-z0-9_-]+)`
     提取 token，存进 DshProcess 状态（内存，不落盘，与远程 token 同约定）
   - `dsh-ready` 事件负载从 `{port}` 扩为 `{port, token}`；lib.rs 导航改
     `http://127.0.0.1:{port}/?token={token}`（WebView 收 303 自动种 cookie，此后免 token；
     cookie 30 天持久，但每次 dsh 重启换新 token，重导航即再种）
   - 通知/远程子系统从同一状态读 token
2. **wait_ready**：不改——401 也是"任意响应"
3. **远程代理**（remote/proxy.rs）：
   - dsh 每次 spawn 后（拿到 token 时）向 `http://127.0.0.1:<port>/?token=` 发一次 GET
     （reqwest `.no_proxy()`，Host 即目标回环 authority），从响应抓 `Set-Cookie`
   - 把该 cookie 注入后续所有转发请求（HTTP 与 WS upgrade 都要）；cookie 绑定
     `127.0.0.1:<port>` authority，与转发请求的 Host（reqwest 按目标 URL 重算）一致——
     现有剥头清单（host/origin/referer/sec-fetch-*，proxy.rs:239-262）**保持不变**，
     Host 栅栏已被满足
   - 换端口/重启 dsh → 重换 cookie；401 响应可作再交换触发器（兜底）
   - 手机端首次打开无需感知 token：cookie 由代理代持，代理自身的 token 门（现有
     cookie_authed）不动
4. **welcome notice 改写**（proxy.rs:319）：机制不变，needle 待真实产物复验（三元式
   源码原样在 ui-settings 两处）

## 三、接口契约：传输层

### 3.1 HTTP RPC 信封（`POST /api/<endpoint>`）

请求（Content-Type 必须 `application/json`，否则 415）：

```json
{ "type": "client-request", "rpcId": "<相关 id>", "method": "<endpoint>", "payload": { "args": { …命名参数… } } }
```

- `method` 必须等于 URL 里的 endpoint（否则按 bad-request 信封回 200）
- body 非 JSON → 400；非 POST → 404 `not found`；endpoint 段字符集
  `[A-Za-z0-9_$.-]`（rpc-host.ts:283-290）
- 鉴权失败在进 handler 前：401 `unauthorized` / 403 `forbidden`（text/plain）

响应（HTTP 恒 200，业务成败看 result）：

```json
{ "type": "server-response", "rpcId": "<回显>", "result": { "ok": true, "value": … } }
{ "type": "server-response", "rpcId": "<回显>", "result": { "ok": false, "error": { "code": "…", "message": "…", "details": {} } } }
```

- 端点命名：`<namespace>/<method>`（typert 规范，gateway index.ts:130-131）——如
  `session/prompt`、`session/follow`；gateway 内部例外用 `$` 前缀（`$events`、`$events/result`）
- 请求体上限默认 300MiB（base64 图片预算）

### 3.2 WS mux（`/api/remote.mux`）

- 连接：`ws://127.0.0.1:<port>/api/remote.mux`，upgrade 需过 requestRejection
  （cookie + Host 栅栏）
- 客户端 → 服务端（stream-protocol + client/stream-client.ts:97,116）：

```json
{ "type": "open",   "streamId": "<uuid>", "endpoint": "$events",              "payload": { "args": {} } }
{ "type": "open",   "streamId": "<uuid>", "endpoint": "session/follow",       "payload": { "args": { "address": { "kind": "session", "sessionId": "…" } } } }
{ "type": "cancel", "streamId": "<uuid>" }
```

- 服务端 → 客户端（stream-server.ts:148-150）：

```json
{ "type": "item",  "streamId": "<uuid>", "value": <逻辑流条目，见 §四/§五> }
{ "type": "end",   "streamId": "<uuid>" }
{ "type": "error", "streamId": "<uuid>", "error": { "name": "…", "message": "…", "code": "…?", "details": …? } }
```

- 心跳：服务端每 `websocketHeartbeatIntervalMs`（默认 30_000，min 1，max 2^31-1）对所有
  OPEN socket 发 **WS 协议层 Ping**（stream-server.ts:90-96，unref 定时器）；客户端自动
  Pong。壳 ws.rs 的看门狗把 Ping/Pong 计入"任意下行帧"即兼容，无需改造
- 一条 WS 连接可多路复用多条逻辑流（streamId 区分）；壳可复用一条连接开 `$events` +
  N 条 `session/follow`

## 四、接口契约：事件流 `$events`（通知管道的主数据源）

### 4.1 流的开启与帧

open payload 必须是**空 args 对象** `{args:{}}`（非空 → signature-invalid，
gateway index.ts:398-405）。下行条目（value）：

```json
{ "type": "ready", "clientId": "<本次连接身份>", "host": { "home": "<DSH home 路径>" } }
{ "type": "emit",      "event": "<cordis 事件名>", "args": [ <位置参数…> ] }
{ "type": "waterfall", "event": "<cordis 事件名>", "eventId": "<id>", "agentId": "<id>", "request": { … } }
{ "type": "cancel",    "eventId": "<id>" }
```

`ready` 必为首条；`args` 是**位置参数数组**（emit），`request` 是 waterfall 的投影载荷
（`agent`/`signal` 被剥离）。

### 4.2 转发事件全清单（api/remotes/src/remote-events.ts:26-43）

| 事件 | 模式 | 载荷（emit=位置参数；waterfall=request 投影） | 壳用途 |
|---|---|---|---|
| `api-session/added` | emit | `(summary: SessionSummary)` | 子代理过滤（origin）、会话台账 |
| `api-session/removed` | emit | `(sessionId)` | 台账清痕 |
| `api-session/status` | emit | `(agentId, running: boolean)` | 粗粒度运行态（简化版完成判定的备选） |
| `api-session/activity` | emit | `(sessionId, time)` | 活跃戳 |
| `api-session/error` | emit | `(agentId, errorChain)` | 错误通知（可选） |
| `settings/document-updated` | emit | `(ns, revision)` | 只有命名空间+单调 revision，**无键名**——主题跟随不如继续文件轮询 |
| `approval/request` | waterfall | `{toolName, callId?, reason?}`（user-approval/src/types.ts:69-78） | 审批通知 |
| `user-questions/request` | waterfall | `{questions: AskUserQuestionItem[]}`（user-questions/src/types.ts:72-76） | 提问通知 |
| `credentials/reference-updated` | emit | — | 不用 |
| `llm/adapters-updated` | emit | — | 不用 |
| `commands/change` | emit | — | 不用 |
| `agent-preset/selected` | emit | — | 不用 |
| `cordis/request-run` / `request-run-resolved` / `dynamic-package` / `dynamic-retract` / `inspect-query` / `inspect-query-resolved` | emit | — | 不用 |

SessionSummary（session-controller/src/list.ts:120-131,379-389 + types.ts:154-163）：
`{ sessionId, updatedAt, running, blank, parentSessionId?, origin?: 'subagent', cwd, …projection 字段 }`。
子代理判定 `origin === 'subagent'` 与旧 host/session-added 同法可平移（字段从
`payload.origin` 变为 `args[0].origin`）。

### 4.3 waterfall 派发语义与旁听安全（壳只读的前提）

- **fan-out**：waterfall 帧投递给**所有**开着 `$events` 流的客户端（gateway
  index.ts:519）——壳与真实浏览器 UI 各收到一份
- **结算**：任一客户端经 `POST /api/$events/result`（payload
  `{args:{clientId, eventId, outcome}}`）回 `result`/`rejected` → 立即结算；
  `next` 只在"全部已投递客户端都回了 next"后兜底结算（index.ts:531-550）
- 结论：壳**只听不回** → 审批不被抢、不悬挂；**严禁**实现 `$events/result` 回包
  （回 `result` 会抢先替用户做决定）

## 五、接口契约：`session/follow` 流（turn 级事件）

### 5.1 订阅

- endpoint `session/follow`（SessionController `namespace:'session'` +
  `@Remote({mode:'stream'}) follow`，session-controller/src/index.ts:83,115,378-383）
- open payload：`{args:{ address: SessionAddress, maxMessages?: number }}`
- SessionAddress（types.ts:387-392）：
  `{kind:'session', sessionId}` 或 `{kind:'subagent', parentSessionId, childSessionId, mode:'one-shot'|'continuable'}`——
  **壳可直接 follow 子代理会话**，不再只能靠 added 帧过滤
- 会话结束/流关闭 → 服务端 `{type:'end'}`；壳按 `api-session/added`/`removed` 维护
  follow 生命周期

### 5.2 下行帧

```json
{ "type": "snapshot", "header": SessionHeader, "cursor": 0, "records": [SessionHistoryRecord…], "hasMore": false, "projections": {…} }
{ "type": "event", "event": SessionWireEvent }
```

- `SessionWireEvent = { type, seq, time, data: JsonValue, sourceEventSeqs?, surfaceOp? }`
  （session-controller/src/types.ts:422-428）——**与旧 session/event 帧的
  `payload.event` 同构**：分类逻辑从 `event.type` + `event.data.reason.kind` 平移，几乎零改写
- snapshot 里的 records 是 `SessionEventEntry{type:'event',event}`（另有
  `SessionChunkRun{type:'chunks',…}` 打包的 assistant delta 流，壳可忽略）

### 5.3 通知分类相关的事件 data 形（core/session/src/types.ts:225-268 等）

| type | data | 通知用途 |
|---|---|---|
| `turn/start` | `{turn}` | 回合开始（"干活回合"窗口开启） |
| `turn/end` | `{turn, reason}` | 完成判定：`reason.kind` ∈ `completed` / `aborted{reason}` / `blocked` / `error{error:{message,code?}}` / `max-tokens` / `interrupted`（合并可扩展，壳按"===completed + 其余"处理） |
| `tool/call` | `{turn, step, callId, name, arguments}` | "回合内干过活"标记（arguments 为 JSON 字符串） |
| `tool/result` | `{turn, step, …}` | 同上备用 |
| `session/title` | `{title, messageSeqs, source:{kind:'user'|…}}`（session-title/src/index.ts:100） | 标题台账 |
| `user/message` / `assistant/message` / `assistant/chunk` | 结构同旧 | 不用 |

### 5.4 通知管道改造设计（notify/）

现状：ws.rs 连两条端点、mod.rs 按 `server-request` 帧分类。新设计：

- **ws.rs → mux 客户端**：连接前先 `GET /?token=` 交换 cookie（带 Host 头），upgrade 带
  `Cookie` 头；一条连接上先 open `$events`；`api-session/added`（非 subagent）时为该
  sessionId open `session/follow`
- **帧分类平移**（mod.rs）：
  - 旧 `approval/requested` / `question/requested` → `$events` waterfall 帧
    `approval/request`（取 request.toolName/reason）/`user-questions/request`
    （取 request.questions）——**只通知不回包**
  - 旧 `host/session-added`（origin 过滤）→ `$events` emit `api-session/added`
    （取 args[0].origin）
  - 旧 `session/event`（turn/start、turn/end、tool/call、session/title）→ follow 流的
    `{type:'event'}` 条目（`event.type` / `event.data.*` 同构平移）
  - turn/end 的"任务完成/回答完成"拆分逻辑（回合内是否 tool/call）原样保留
- **降级路径（可选先行）**：若想分两步走，第一步只开 `$events` 用
  `api-session/status`（agentId, running）做粗粒度完成通知（丢失"任务/回答"拆分与标题
  事件），follow 二期补
- **心跳/重连**：30s 服务端 Ping 自动复位现有探活；`{type:'end'}` 的 follow 流要能重建
  （会话仍在时重 open）；dsh 重启 → 端口+token 双变，沿用 port watch 重建连接并重换
  cookie

## 六、移动端适配（remote/mobile.css / mobile.js）

聊天渲染整体迁入新包 `@deepseek-ai/dsh-client-ui-chat`（ui-conversation 保留
input/skeleton/组装）。CSS Modules 哈希随包全变，mobile.css 一律按本地名子串
（`[class*="_stats"]` 式）匹配，理论存活率高；必须按 AGENTS.md 方法（Playwright 390px +
addStyleTag/addScriptTag 注入，不能直连 dsh 端口）复验：

1. **保持项**（源码核对类名未变）：StatsLine `.root`/`.sep`（字号 12px →
   `var(--dsh-content-font-size-secondary, 13px)`）；标签条 `.tabs`/`.tab`/`.tabActive` +
   `role="tablist"`（mobile.js 注入挂载点，ConversationSession.tsx:120-145 结构未动）；
   System prompt 折叠 ContextInjectionRow；「在文件夹中显示」locale 键
   produced.showInFolder 与 `.showFolder`；模型槽位 `data-slot="conversation.input.model"`
   与 `triggerLabel`/`triggerEffort` 类名——ModelSelect 新增 loading 态（`trigger.loading`
   文案）且 `current===null` fallback 变 `provider/model`，图标化规则要复验空态/加载态
2. **新增块需评估 700px 断点**：TurnNavigator（右侧 sticky 28px 导航轨，可能与内容列
   重叠）；TurnUsageDisclosure（token 用量展开）；TurnProcessNodeView（过程内容折叠，
   0.1.2 起默认折叠）；WidthHandle（正文宽度拖拽手柄，localStorage
   `dsh.conversation.contentWidth`，窄屏确认不溢出）；FontSizeRow（字号调节写 body CSS
   变量）
3. `dsh.sessions.current` 值多可选 `subagentAddress`——project.html 读 `sessionId`
   兼容，复验"项目/信息"标签
4. 远程链路经 cookie 后是"已鉴权远程"，welcome notice 持久化分支（scope.mode）语义不变

## 七、npm 发布后跟版 runbook

> **实施计划已就绪**：逐步任务分解见 `docs/superpowers/plans/2026-08-28-dsh-0.1.2-upgrade.md`
>（13 个任务、TDD 步骤、验收命令；本节 runbook 是它的战略视图，执行按计划文档走）。

1. **等正式版**：`curl -s https://registry.npmjs.org/@deepseek-ai/dsh` 确认目标版已发布
   （建议 `0.1.2-rc.1`+ 或 stable；alpha 期只做代码准备，且鉴权/传输必须对真实运行时联调）
2. `powershell -File scripts/follow-upstream.ps1 -DshVersion <目标版>`（钉版→清旧→重抓→
   bump→cargo test；CI 缓存 key 变化时先推 main 预热再打 tag，见 AGENTS.md）
3. **契约套件预计红项 → 改 upstream.rs**：
   - `PRESET_DIR_SEGMENTS`：真实包里 find 新位置（预期
     `@deepseek-ai/dsh-agent-presets/presets/minimal`，以 npm tarball 实际布局为准——
     apps/cli 的 `dsh.configTrees` mount 写的是 `../../packages/...` 相对路径，发布形态
     如何落盘只有真实包能回答）；presets.rs 探测基准同步改
   - `EVENTS_MUX_PATH`/`EVENTS_HOST_PATH`/`METHOD_*`/`EVENT_*`/`REASON_COMPLETED`/
     `ORIGIN_SUBAGENT`：按 §三/§四/§五 重写为 remote.mux 协议常量集
     （`/api/remote.mux`、`$events`、`$events/result`、`session/follow`、
     `api-session/added|status|…`、`approval/request`、`user-questions/request`、
     `settings/document-updated`…），upstream_contract.rs 对应探针同步重写
   - `WELCOME_NOTICE_NEEDLE`：tree_find 复验新产物（源码三元原样，构建形态待验）
   - 其余（MODEL_*、SESSION_LOG_BUTTON_NEEDLE、LOCALSTORAGE_CURRENT_SESSION_KEY、
     WORKSPACE_STORE_SEGMENTS、DSH_WEB_SUBCOMMAND 族、MCP_*/CORDIS_*、PICKER_*、
     PRESET_BROKEN/PLATFORM_NEEDLE）：源码核对均保持，预期一次过
4. **代码改造顺序**（每步可独立验收）：
   ① process.rs token 捕获 + lib.rs 导航带 token（本地 UI 冒烟）
   → ② notify/ mux 客户端（先 `$events` 粗粒度，再 follow 补齐任务/回答拆分；三类通知
   全回归）
   → ③ proxy cookie 代持（手机真机全流程：会话/审批/通知/项目标签/文件选择器）
   → ④ mobile.css/js 对新产物的复验与补规则（§六清单）
   → ⑤ presets.rs 新路径探测
5. `cargo test` + `pnpm tauri build` + `acceptance.ps1`；版本进位 0.4.8 → 0.4.9；
   CHANGELOG 收编；AGENTS.md 的「预设不能经 profile patch 影子覆盖」「dsh 事实」两条
   按新版重写

## 八、坑清单与风险评估（2026-08-28 评估）

上游 README.md:13 自述：*"DeepSeek Harness is in developer preview and iterating rapidly.
THERE WILL BE COMPATIBILITY-BREAKING CHANGES."* ——0.x 无兼容承诺是官方立场，壳的跟版税
会长期存在。

已识别的坑（按踩中概率排）：

1. **token 时序竞态**（§2.5）：stdout 就绪行晚于 HTTP 可达，token 捕获必须持续 pump +
   超时；dsh 崩溃重启循环时出行反复，token 状态机要幂等（同 token 覆盖无害、旧 token 作废）。
2. **cookie 绑端口**：壳每次启动 free_port 换端口 → authority 变 → WebView 旧 cookie 全失效。
   不能假设 cookie 已存在，每次启动都完整交换；且 WebView2 若清了站点数据，token 导航后
   cookie 可能没种上——导航后建议探测（401 则重导 token 流一次）。临时空 DSH_HOME 复刻法
   （0.4.8 用过）同理：credentials 全新 → 旧 cookie 必失效。
3. **proxy WS upgrade 的 cookie 注入易漏**：upgrade 走的是独立桥接路径，与普通 HTTP 转发
   代码不同；`/api/remote.mux` upgrade 无 cookie 直接 401，手机端表现为"页面开但全断"。
4. **壳侧快捷审批的边界**：新 waterfall fan-out 下，壳若将来实现通知点击"直接批准"会
   与浏览器/手机端抢结算（任一 result 即结算）——壳永远只听不回。
5. **契约套件盲区**：现有探针多为文件 needle 型；传输协议/鉴权这类**行为事实** tree_find
   测不到，需对真实运行时起 HTTP/WS 探测（WS 通知集成测试已有先例，可扩展）。跟版红了
   的形态会变：从"needle 失配"变"行为断言失败"。
6. **移动端新块是新增规则不是复验**：TurnNavigator sticky 右轨、WidthHandle 手柄是默认
   开启的新元素，700px 下布局冲突要新写规则，别按"复验零改动"估工作量。
7. **alpha 期返工风险**：照 alpha.1 契约写实现，正式版可能再变（上游正在对 0.1.2 定型）。
   缓解：notify 的 mux 客户端收在独立模块，正式版联调时只动适配层；不动 UI/移动端。
8. **唯一无法预验的盲区**：npm 发布形态（预设落盘位置/configTrees 相对路径在发布包的
   解析、CSS 哈希、WELCOME_NOTICE_NEEDLE 产物形态）——文档已全部标注"以真实包为准"。

结论（2026-08-28）：dsh 本体工程质量高（类型化 RPC、patch 组合、鉴权设计认真），但它是
"自己即终端产品"的设计，**没有给嵌入者的稳定接口**——壳的一切依赖面都是逆向所得。迁移
不适合"快"，适合"稳"：逆向面 90% 预期一次过（契约套件守住），但鉴权/通知管道/远程代理
三处是重写级工作量，联调必须等真实 npm 包。

## 九、证据索引（file:line，均为 0.1.2-alpha.1 树；旧版对照 b150a55）

- **CLI/旗标**：web-app/src/startup.ts:50-54,73-77（--host 0.0.0.0 拒绝）；apps/cli/package.json
  （bin.dsh=lib/bin.js、files 收缩、dsh.configTrees）；apps/cli/src/args.ts（web alias）
- **webserver**：host/webserver/src/index.ts:59-68,124-130（Config）、WebRoute/WebUpgradeRoute:42-57
- **鉴权**：client/connection/src/browser-auth.ts（token:50-57、authenticatedUrl:224-231、
  authorizeIndex:240-287、isAuthenticated:289-304、cookie 构造:106-149、401 文案:306-314、
  secret 持久化:190-213）；rpc-host.ts:96-99（requestRejection）；api-request-trust.ts:91-118
  （Host 栅栏）；loopback-hostname.ts:9-18；connection index.ts:70-106（ConnectionConfig 默认）
- **URL 行**：bundle/web-app/src/index.ts:45-56（printUrl 默认）、:159-161（localWebUrl）、
  :264-290（announceReady/打印/openBrowser）
- **静态服务**：host/frontend-static/src/index.ts:61-100（仅 index 鉴权、穿越 403）、:139
- **RPC 信封**：client/connection/src/rpc.ts:60-105（envelope 类型）、rpc-schema.ts
  （zod 校验）、rpc-host.ts:214-282（rpcFetchHandler：POST/415/400/method=endpoint）
- **mux**：api/gateway/src/stream-protocol.ts:6-15,24-30,41-66,80-137（路径/帧/结果）；
  stream-server.ts:22-96（noServer、心跳、item/end）、:197-208（upgrade 拒绝）；
  gateway/src/index.ts:114,175-178（心跳默认 30s）、:209-233（upgrade 挂载+鉴权）、
  :280-336（typert 分发/端点命名 `<namespace>/<method>`:130-131）、:398-405（$events 空 args）、
  :461-550（waterfall fan-out/结算）；client/stream-client.ts:85-116,225-227,345-347
- **$events**：api/remotes/src/remote-events.ts:26-43（转发清单）、:45-100（emit/waterfall
  桥接）；client remote-events.ts:106-217（openStream/$events 结果回包 answer:232-249）
- **session.* Remote**：api/session-controller/src/index.ts:83,115（namespace），
  :208-392（@Remote 方法清单），:378-392（follow/control 流）；types.ts:387-392（SessionAddress）、
  :441-445（SessionFollowRequest）、:383-405,418-428（SessionFollowFrame/SessionHistoryRecord/
  SessionWireEvent）、:154-163（SessionSummary）；list.ts:120-131,379-389（summaryFor/listFields）；
  history.ts:87-169（follow 实现）；subagent/src/child-agent.ts:151（origin:'subagent'）
- **事件 data 形**：core/session/src/types.ts:155-177（TurnEndReasonMap）、:225-268
  （turn/start、turn/end、tool/call 等）；session/session-title/src/index.ts:100,191-196
  （session/title data）
- **审批/提问**：interaction/user-approval/src/types.ts:69-99（ApprovalRequestEvent +
  'approval/request' waterfall 声明）；interaction/user-questions/src/types.ts:55-99
- **预设**：preset/agent-presets/package.json（files: lib+presets）；apps/cli/src/profile-boot.ts
  （SHIPPED_PRESET_ROOT 重写块已删，diff -60 行）；presets/ 目录 diff 为空
- **picker**：bundle/web-app/cordis.patch.yml:78-79；host/directory-picker-browse/src/index.ts
  :35,:222；client/ui-directory-picker-browse/src/client/DirectoryBrowser.tsx
  :118,:292,:582,:834,:968,:998、client/index.ts:51,:66；native win32-dialog-bindings.ts（「开」字修复）
- **设置/语言**：settings/settings-file/src/index.ts:56,:210-227；client/ui-theme/src/
  theme-settings.ts:9-44；client/locale/src/locale-settings.ts:6-19、client/index.ts:52,254
  （LocaleDict/registry）
- **UI needle**：ui-settings-models/src/onboarding-copy.ts:2-11（welcome notice 原值）；
  ui-settings/src/client/settings-scope.ts:291（isLoopback 三元）；ui-model-selection/src/
  client/ModelSelect.tsx:87-93,:195-236；ui-chat/src/client/chat/StatsLine.module.css:11-16；
  ui-conversation/src/client/skeleton/ConversationSession.tsx:120-145（标签条）；
  session-query/session-log-export/src/client/HeaderAction.module.css（原样）；
  api/session-controller/src/client/sessions/service.ts:227-229（localStorage 键）；
  client/connection/src/client/index.ts:172（isLoopback 新计算式）

*源码树留存：`/tmp/dsh-old/deepseek-ai-deepseek-harness-b150a55`（rc.2）、
`/tmp/dsh-src/deepseek-ai-deepseek-harness-cd5ef81`（alpha.1），实现期查证用。*

## 十、壳侧框架自评（DSHDesktop，2026-08-28，0.1.2 预研期间的复盘）

本节是对**壳自身**设计的评估，写于 0.1.2 预研完成时——这次预研像一次压力测试，
把壳与上游的每一处接触面都翻了一遍，正好借此复盘哪些设计扛住了、哪些欠了债。
评估基准：壳的使命是"寄生在一个快速迭代、无兼容承诺的上游之上，给最终用户提供
稳定开箱体验"。离开这个约束谈优劣没有意义。

### 10.1 总评

壳的核心决策——**内嵌官方 Web UI 而非自绘界面、进程监督而非托管、逆向事实集中
管理而非散落**——在上述约束下都是对的。可靠性细节扎实（三层进程清理、看门狗、
统一诊断层），文档与方法学完整。最大的结构性弱点只有一个：**对 dsh 的依赖没有
防腐层，逆向面散布在六个模块里**。0.1.2 实锤了这一点：上游一次传输层重写，壳的
通知管道整体报废、三条链路连锁伤。0.1.2 重写窗口就是还这笔债成本最低的时机。

### 10.2 设计做对的地方

1. **upstream.rs 单一事实源 + upstream_contract.rs 契约套件——全项目最好的设计**。
   把"依赖不稳定上游的内部实现"这个架构级风险，转化为"一个文件 + 一套对真实运行时
   的探测测试"：事实带出处注释，漂移时 DRIFT 输出直接指出改哪条常量、影响哪个模块。
   本次预研直接验证了价值：50+ 条事实逐条对照两棵上游源码树核，90% 的结论几分钟出，
   预研本身就能产出契约文档（即本文）。没有这套机制，跟版是考古；有了它，跟版是
   机械流程。**这条经验要守住：任何新的 dsh 接触面必须先落 upstream.rs 常量 +
   契约探针，再写消费代码。**
2. **可靠性纵深是真实的，不是纸面设计**。三层进程清理（Job Object 内核级连带回收 →
   NSIS 钩子按路径清扫且排除调用方自身 PID → 退出时 taskkill）各层独立，0.1.9~0.1.12
   的 "Unable to uninstall!" 事故系列就是靠这套层层加固终结的；WS 看门狗
   （PING_IDLE/PONG_TIMEOUT）解决回环 TCP 半开假死；events.log 作为壳侧诊断与 dsh
   进程事件的统一持久层，让"应用卡启动先看日志"成为可靠的第一动作。这些全是实踩
   出来的，且设计上互相独立——任何一层失效还有下一层。
3. **"轮询优先于事件"的跟随设计经受住了考验**。主题/语言直接轮询 settings.yaml，
   简单可靠。本次确认上游 `settings/document-updated` 事件只有 (ns, revision) 而无键名
   （§4.2）——当初若做成事件跟随反而要返工。对不可控上游，文件落盘契约比内存事件
   稳定，这个直觉值得作为后续的原则固定下来：**能轮询的持久事实不依赖上游事件**。
4. **文档文化与方法学是这个项目真正的护城河**。AGENTS.md 每条坑带根因、复现方法和
   修复版本（"验证不能直连 dsh 端口"、"env key 会静默污染复刻"、PowerShell BOM、
   Process.MainWindowHandle 陷阱……），本次预研全程在吃这个红利——mobile 适配的
   Playwright 注入复刻法直接决定了 §六 复验清单的可执行性。反过来这也是个警示：
   这些方法学没有自动化，离开文档换台机器，验收结果就不可信（见 10.3.4）。
5. **版本与发布流程的工程化**。固定进位规则（每 10 个小版本进一位）消除了
   "这版算不算 breaking"的争论；CI 运行时缓存的预热机制（main 预热 → tag 消费）虽然
   认知负担重，但把 release 时长从不可控变成可预算；acceptance.ps1 端到端验收 +
   verify-* 回归脚本族形成了发布门禁的闭环。
6. **平台 trait 的抽象点选得准**（可执行名/杀树/子进程配置/triplet），成本低、
   不碍事、真有多平台需求时进程管理层可直接复用。

### 10.3 架构级弱点与债（按严重度）

1. **逆向面未收窄——最大的一笔债**。upstream.rs 管住了"事实在哪"，没管住"谁能用"：
   notify/mod.rs 直接解析 dsh 帧字符串、mobile.css 直接匹配 CSS Modules 本地类名、
   welcome.rs 直接正则 client.js、pickerpatch.rs 直接原地改写上游文件、proxy.rs 直接
   改写产物。上游任何一次内部重构都是多模块连锁伤，0.1.2 就是实证（通知管道报废 +
   process/lib/proxy 三处随鉴权改）。
   **修法**（0.1.2 重写时执行）：把"与 dsh 的一切交互"收口到一个 adapter 模块——
   传输（mux 客户端 + cookie 交换）、事件词表到壳类型的映射、文件格式读取（settings/
   workspace/credentials）。UI 与业务层（通知分类、主题、远程页）只依赖壳自己的类型，
   不出现任何 dsh 帧字面量。CSS 类名注入是例外（必须匹配产物），但要把规则集中并
   保持"子串匹配 + 契约守门"的纪律。
2. **proxy.rs 五合一 + 三凭证分散**。当前 540 行同时承担 token 门岗、反向代理、WS 桥、
   HTML/JS 注入、welcome 改写五件事；0.1.2 要再加 cookie 代持（§2.6）。此后壳同时持有
   三种凭证——dsh launch token（进程级，stdout 捕获）、proxy 自身 token（持久）、
   dsh cookie（30 天，authority 绑定）——生命周期各不相同，管理逻辑分散在
   process/proxy/notify 三处。
   **修法**（同窗口执行）：proxy 拆出凭证模块（三种凭证的获取/轮换/失效探测集中），
   注入改写逻辑独立；proxy 主体只留转发与路由。
3. **运行时补丁器是借来的脆弱性，但风险已控**。pickerpatch.rs 原地改写上游包内文件、
   mobile 注入改写响应流，本质都是"对上游做手术"，上游一个无警告的改动就会让补丁
   静默失效。公允地说：签名门控 + marker 幂等的设计已经把最坏结果控制在"停手而非
   损坏"，且补丁器只信 upstream.rs 的 needle、契约套件守门——这是对的姿势。剩下的
   成本是每次上游更新都要人肉确认补丁存活（§七 runbook 已含），以及一个心理陷阱：
   **补丁停手是静默的**（手机端回到未适配态但不报错），依赖 §六 复验清单兜住。
4. **验收链路对环境敏感，bus factor = 1**。verify-* 脚本的方法学约束（按窗口类名
   枚举而非 PID、临时空 DSH_HOME 复刻、env 摘除、fixture CJS 桩、Playwright 注入
   复刻手机）都是必要的，但它们是文档不是代码——换机器或换人，验收结果就不可信。
   单人项目的现实风险，文档是唯一保险，AGENTS.md 的维护纪律比任何代码都重要。
   中期可考虑把高频复刻法脚本化（手机环境注入已有雏形）。
5. **测试基线成本偏高但布局合理**。201 个测试含真运行时契约探测、真进程集成、
   会弹窗的对照组；CI 缓存有 main 预热的隐式依赖。跑一轮不便宜——这是守门能力的
   对价，目前划算。0.1.2 后契约套件会多一类"行为断言"（HTTP/WS 探测，§八.5），
   跑时更长，注意别让测试慢到没人肯跑：行为探测做成可选特性或并行化，别全量塞进
   每次 cargo test。
6. **跨平台预留是纸面的**。toast 激活（协议激活 + AUMID）、DWM 标题栏、NSIS 钩子、
   移动端注入全是 Windows 特有实现，platform trait 只盖住了进程管理一小片。真要
   跨平台时实际工作量会是当初预估的数倍——不影响当下，但别把"预留了"当"低成本"，
   启用前先做一次真实的工作量盘点。
7. **Svelte 侧七个页面 + hash 路由**：轻量合适，与 Rust 侧经 invoke/事件桥接，无
   明显坑。唯一注意点：远程页的 UI 状态与代理注入的移动端样式是两套逻辑，改远程
   功能时两处都要过（0.4.6~0.4.8 三连修的教训）。

### 10.4 与 0.1.2 迁移的关系：重写窗口 = 还债窗口

0.1.2 强迫壳重写通知管道与鉴权链路（§二、§五），这是还 10.3.1/10.3.2 两笔债成本
最低的时机——反正要动这些代码。建议的债务偿还与功能改造合并顺序：

1. **adapter 模块先行**：mux 客户端 + cookie 交换 + 事件词表映射收进独立模块
   （如 `notify/transport.rs` 或 `dsh_adapter/`），upstream.rs 常量只被它引用；
2. **凭证模块**：三种凭证（launch token / proxy token / dsh cookie）集中管理，
   process/proxy/notify 经它存取；
3. 在此之上再实现 §5.4 的通知分类平移与 §2.6 的三条链路改造——业务层代码应当
   感受不到协议变化（分类逻辑从旧帧到新流本就是同构平移，正好验证 adapter 的
   隔离效果）。

如果 0.1.2 正式版来临时间紧，允许先按 §七 runbook 做功能改造、adapter 后补——
但要记录欠账，避免 adapter 变成"永远的下一次"。

### 10.5 结论

壳是一个**为"寄生在不稳定上游之上"专门设计的框架**：防御工事（契约套件、诊断层、
文档、验收方法学）完整且经过实战，可靠性记录良好。欠的债集中于依赖面收窄与
proxy 的复合膨胀，都不是致命伤且有明确的修法与时机。保持两条纪律——新接触面必落
upstream.rs、能轮询的持久事实不依赖上游事件——这套壳可以陪着 dsh 走完整个 0.x。
