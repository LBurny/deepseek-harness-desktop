//! dsh 上游内部事实的单一来源（跟版门禁）。
//!
//! 跟版流程：fetch-runtime.ps1 抓新版 → `cargo test` →
//! tests/upstream_contract.rs 红了就对照本文件逐条改（每条注明上游出处
//! 与影响面）。事实清单的文档形态见 docs/design.zh-CN.md §15。
//!
//! 当前事实基线：@deepseek-ai/dsh 0.1.5-rc.2（子包为浮动区间，抓取时解析到
//! 最新 rc；npm latest 标签可能滞后，fetch-runtime.ps1 须显式 -DshVersion）。
//!
//! 0.1.2 跟版（2026-09-04 执行）：契约与 runbook 见
//! docs/upstream-0.1.2-alpha.1-prep.zh-CN.md（§二~§五）——大改三件：
//! Web 鉴权（launch token + cookie，无关闭开关）、事件传输重写
//!（/api/remote.mux + $events + per-session follow，events.mux/host 移除）、
//! shipped 预设搬进 dsh-agent-presets 包。旧词表（EVENTS_MUX_PATH 族）已随接线完成删除。
//!
//! 0.1.5 跟版（2026-09-12 起执行）：契约与影响面见 docs/upstream-0.1.5-prep.zh-CN.md
//!（§三逐项对照、§四按模块影响），执行计划见 docs/superpowers/plans/ 下 2026-09-12
//! 的执行计划——逆向面几乎不动（40 余条探针仅 1 条红），改动集中在四处：
//! 会话日志入口从"下载按钮"变"更多菜单"（CSS 本地名 sessionLogButton → moreButton，
//! mobile.css 规则随之）、新增流式上传路由（壳代理须开流式旁路，见 remote/proxy.rs）、
//! 面板槽位体系重排（conversation/details → keyed main + rightbar + sidebar.panellist）、
//! 会话格式升到 V3（用户数据单向，升级后不可降级读取）。

use std::path::{Path, PathBuf};

/// 多段相对路径拼接（各段不含分隔符，跨平台安全）。
pub fn join_segments(base: &Path, segments: &[&str]) -> PathBuf {
    segments.iter().fold(base.to_path_buf(), |p, s| p.join(s))
}

// ── 包与入口 ──────────────────────────────────────────────
/// npm 包在运行时树内的位置（runtime/<triplet>/ 之下）。
/// 上游出处：npm 包名 @deepseek-ai/dsh，fetch-runtime.ps1 用 --prefix dsh 安装。
pub const DSH_PKG_SEGMENTS: &[&str] = &["dsh", "node_modules", "@deepseek-ai", "dsh"];
/// 入口脚本的包内相对路径。上游出处：package.json 的 bin.dsh = lib/bin.js。
/// 影响：runtime.rs 的 paths_for/validate_source（经下方 dsh_bin 助手自动跟随）。
pub const DSH_BIN_SEGMENTS: &[&str] = &["lib", "bin.js"];
/// dsh 声明的 Node 主版本下限（package.json engines.node），契约测试断言用。
pub const DSH_NODE_MAJOR_FLOOR: u32 = 22;

pub fn dsh_pkg_dir(runtime_dir: &Path) -> PathBuf {
    join_segments(runtime_dir, DSH_PKG_SEGMENTS)
}
pub fn dsh_bin(runtime_dir: &Path) -> PathBuf {
    join_segments(&dsh_pkg_dir(runtime_dir), DSH_BIN_SEGMENTS)
}

/// npm 前缀的 node_modules（插件与依赖的落盘处；契约探测在这里搜
/// client.js needle / ui-theme 键 / MCP 插件 package.json）。
pub const DSH_NM_SEGMENTS: &[&str] = &["dsh", "node_modules"];
pub fn dsh_node_modules_dir(runtime_dir: &Path) -> PathBuf {
    join_segments(runtime_dir, DSH_NM_SEGMENTS)
}

// ── 进程命令形（process.rs spawn）────────────────────────
/// 上游出处：bin.js 的 web 子命令，仅绑 127.0.0.1。
pub const DSH_WEB_SUBCOMMAND: &str = "web";
pub const DSH_PORT_FLAG: &str = "--port";
/// dsh-web-app 的 openBrowser 默认 true（rc.8 起；startup.js webCommand 定义
/// `--no-open` 反向开关），就绪后把 Web UI 丢给系统默认浏览器——壳内嵌 WebView
/// 就是浏览器，spawn 必须带此旗标，否则每次启动额外弹系统浏览器。
/// 影响：process.rs spawn 参数。
pub const DSH_NO_OPEN_FLAG: &str = "--no-open";

// ── 事件语义（notify/mod.rs 分类；0.1.2 起经 mux 下行）────
/// session/follow 流 value.event.type 值（旧版（0.1.1 线）时为 session/event 的
/// payload.event.type；数据形状同源，仅信封不同）
/// 上游出处：dsh-agent-loop/lib/index.js 的 session.append（turn/start 带
/// {turn}，turn/end 带 {turn, reason}）；tool/call 由工具执行时追加
/// （带 callId/name/arguments），每轮 LLM 调用产出的工具调用都发一帧。
/// 影响：notify/mod.rs 区分"干活回合"与"纯回答回合"（回答完成/任务完成拆分）。
pub const EVENT_TURN_START: &str = "turn/start";
pub const EVENT_TURN_END: &str = "turn/end";
pub const EVENT_TOOL_CALL: &str = "tool/call";
pub const EVENT_SESSION_TITLE: &str = "session/title";
/// turn/end 的 data.reason.kind 完成值
pub const REASON_COMPLETED: &str = "completed";
/// 会话 origin 子代理标记（旧版（0.1.1 线）：host/session-added payload.origin；
/// 0.1.2：$events 的 api-session/added args[0].origin）
pub const ORIGIN_SUBAGENT: &str = "subagent";

// ── 0.1.2 传输与鉴权（BrowserAuth + 单一 mux；prep 文档 §二/§三）────────────
/// 0.1.2 起全部远程调用走单一 WS mux（旧 events.mux/events.host 两端点已移除）。
/// 上游出处：api/gateway/src/stream-protocol.ts:6-15（路径/帧形）；非升级 GET
/// 由 requestRejection 拦（无 cookie 401）。影响：notify/mux.rs、proxy.rs 桥接目标。
pub const DSH_MUX_PATH: &str = "/api/remote.mux";
/// mux 上的全局事件流端点（cordis 事件桥）。上游出处：api/gateway/src/index.ts:398-405
///（open 必须空 args）、:461-550（waterfall fan-out/结算）；client/stream-client.ts:97,116。
/// 注意：壳只监听 $events，**严禁**实现 $events/result 回包——任一客户端回 result
/// 即抢先替用户结算审批（prep §4.3/§八.4 边界）。
pub const EVENT_STREAM_ENDPOINT: &str = "$events";
/// waterfall 结算回包端点（仅契约文档意义；壳不调用，见上条）。
pub const EVENT_RESULT_ENDPOINT: &str = "$events/result";
/// turn 级会话事件流端点（per-session follow，取代旧全局 session/event 广播）。
/// 上游出处：api/session-controller/src/index.ts:83,115,378-383（@Remote stream）；
/// 地址形 {kind:'session', sessionId}（types.ts:387-392）。影响：notify/mux.rs。
pub const METHOD_SESSION_FOLLOW: &str = "session/follow";
/// HTTP RPC 探针端点（POST /api/<endpoint> 信封校验用）。
pub const METHOD_SESSION_LIST: &str = "session/list";
/// stdout 就绪行前缀（launch token 的唯一来源；printUrl 默认开，
/// bundle/web-app/src/index.ts:45-56,281；就绪行晚于 HTTP 绑定，捕获必须持续 pump）。
/// 影响：process.rs/dsh_session.rs 的 token 捕获；fetch-runtime.ps1 冒烟同款解析。
pub const READY_URL_PREFIX: &str = "dsh web: ";
/// dsh 会话 cookie 名前缀：dsh-auth-<base64url(sha256(authority))>，authority = Host
/// 头（127.0.0.1:<port>）——**换端口即换新 cookie 名**。上游出处：
/// client/connection/src/browser-auth.ts:16,106-108。影响：dsh_session.rs 换取与识别。
pub const DSH_AUTH_COOKIE_PREFIX: &str = "dsh-auth-";

// ── 0.1.5 新增 HTTP 面（remote/proxy.rs）──────────────────────────
/// 流式请求体上传路由。上游出处：@deepseek-ai/dsh-client-file-upload 的
/// FILE_UPLOAD_PATH，路由注册带 `requestBody:"streaming"`（dsh-client-connection
/// 对 buffered 路由仍有 300MB 上限，streaming 路由不受该上限约束）。
/// 影响面：proxy.rs 必须为它开流式旁路——整读会撞壳侧 REPLAY_BODY_LIMIT（64MB）
/// 报 502，且大文件在壳进程里整份驻留（远程手机端上传大文件的必经之路）。
pub const UPLOAD_STREAM_PATH: &str = "/api/session/uploadFileBinary";

/// 流式路由判定（代理转发路径用；query 不参与路径比较）。
pub fn is_streaming_body_route(path_and_query: &str) -> bool {
    path_and_query.split('?').next().unwrap_or("") == UPLOAD_STREAM_PATH
}

/// "在应用中打开"路由族。上游出处：@deepseek-ai/dsh-host-open-in-app 的
/// OPEN_IN_APP_* 常量；三条都**在鉴权门外**（与静态资产同层）。
/// 影响面：代理是通用反代、无需特判（列出供手机端适配评估——手机没有可拉起的
/// 桌面应用，该入口在 ≤700px 下应隐藏；兼作上游改版对照）。
pub const OPEN_IN_APP_APPS_ROUTE: &str = "/open-in-app/apps";
pub const OPEN_IN_APP_ICON_PREFIX: &str = "/open-in-app/icon";
pub const OPEN_IN_APP_OPEN_ROUTE: &str = "/open-in-app/open";

// ── 面板槽位（0.1.5 重排；client 端 slot 名，picker/mobile 适配对照用）──
/// 上游出处：@deepseek-ai/dsh-client-ui-layout 的 slot 注册 +
/// dsh-client-ui-sidebar 的 slots 契约。0.1.2 时代是 conversation + details
/// （单值槽）；0.1.5 改为 keyed `main`（保留 key `conversation`）+ `rightbar`，
/// sidebar 内新增 `panellist`，**details 槽删除**（原 Detail 面板移除）。
/// 影响面：本仓代码不引用槽位名；但 picker.rs 钉的 browse 表面挂在 ui-workspace 的
/// directory-flow 槽位——槽位体系重排后须在真机点一次目录选择器确认仍能起来。
pub const PANEL_SLOT_MAIN: &str = "main";
pub const PANEL_SLOT_MAIN_CONVERSATION_KEY: &str = "conversation";
pub const PANEL_SLOT_RIGHTBAR: &str = "rightbar";
pub const SIDEBAR_PANELLIST_SLOT: &str = "sidebar.panellist";

// ── 会话格式版本（契约哨兵；壳不读写会话日志，只钉漂移）──────────
/// 上游出处：@deepseek-ai/dsh-session 的 SESSION_FORMAT_VERSION（0.1.2 = 0，
/// 0.1.5 = 3）。恢复旧会话时生成新格式日志并**保留原文件**，但升级后的会话
/// 不支持降级读取——用户数据单向。影响面：跟版/回滚决策（发版后别回退 dsh 版本）。
pub const SESSION_FORMAT_VERSION_EXPECTED: u32 = 3;

// ── 0.1.2 事件词表（$events 流上的 cordis 事件名；prep §4.2 转发清单）──────
/// 会话新增（emit，args[0]=SessionSummary；origin=="subagent" 标记子代理——取代旧
/// host/session-added 的 payload.origin）。api/remotes/src/remote-events.ts:26-43。
pub const EVENT_API_SESSION_ADDED: &str = "api-session/added";
/// 会话移除（emit，args[0]=sessionId）。影响：notify/mod.rs 台账清痕 + follow 关闭。
pub const EVENT_API_SESSION_REMOVED: &str = "api-session/removed";
/// 粗粒度运行态（emit，(agentId, running:boolean)；完成判定的降级备选，壳未用）。
pub const EVENT_API_SESSION_STATUS: &str = "api-session/status";
/// 审批请求（waterfall，request={toolName, callId?, reason?}；壳只听不回）。
pub const EVENT_APPROVAL_REQUEST: &str = "approval/request";
/// 提问请求（waterfall，request={questions[]}；壳只听不回）。
pub const EVENT_USER_QUESTIONS_REQUEST: &str = "user-questions/request";
/// 设置文档更新（emit，(ns, revision)——**无键名**，主题/语言跟随继续走文件轮询，
/// 不订阅此事件；列出仅供契约探针核对转发清单）。
pub const EVENT_SETTINGS_UPDATED: &str = "settings/document-updated";
/// 0.1.5 新增转发事件（emit，(agentId, active:boolean)）：目标暂停/恢复的激活态变化。
/// 上游出处：api/remotes/src/remote-events.ts 的 API_REMOTE_FORWARDED_EVENTS。
/// 影响面：壳对未订阅事件按"忽略"处理（notify/mod.rs 只认自己那张表），列此仅为
/// 记录上游词表增量，勿据此加通知（目标暂停不是用户回合完成）。
pub const EVENT_GOAL_ACTIVATION_CHANGED: &str = "goal/activation-changed";

// ── 设置文件（theme.rs 跟随 + 首启播种）──────────────────
pub const SETTINGS_FILE: &str = "settings.yaml";
pub const KEY_UI_THEME: &str = "ui-theme";
pub const KEY_LOCALE: &str = "locale";
pub const KEY_PREFERENCE: &str = "preference";

// ── 内测声明（welcome.rs 首启豁免播种）────────────────────
/// dsh-client-ui-settings-models 的 welcome notice（"内测声明"对话框）：设置
/// 命名空间 ui-onboarding 的 welcomeNoticeVersion ≠ 当前文案版本时每次启动弹窗
/// （dsh-client-ui-settings-models/lib/client.js 的 WelcomeNoticeStore）。
/// 壳面向最终用户，启动时把运行时里提取的文案版本预写进 settings.yaml。
/// 影响：welcome.rs。
pub const WELCOME_NOTICE_NAMESPACE: &str = "ui-onboarding";
pub const WELCOME_NOTICE_ACK_FIELD: &str = "welcomeNoticeVersion";
/// 文案版本提取 needle（client.js 未压缩，形如 `WELCOME_NOTICE_VERSION = "2026-08-13.1"`）。
pub const WELCOME_NOTICE_VERSION_NEEDLE: &str = "WELCOME_NOTICE_VERSION = \"";
/// 定义文案版本的插件包路径（相对 dsh 前缀的 node_modules）。
pub const WELCOME_NOTICE_CLIENT_SEGMENTS: &[&str] =
    &["@deepseek-ai", "dsh-client-ui-settings-models", "lib", "client.js"];

// ── MCP（mcp.rs 读写 cordis.patch.yml）───────────────────
/// 补丁文件的 DSH_HOME 相对路径。上游出处：cordis profile 加载器。
pub const MCP_PATCH_SEGMENTS: &[&str] = &["profiles", "web", "cordis.patch.yml"];
/// insert 条目里定位 MCP 客户端的名字。影响：mcp.rs 全部读写。
pub const MCP_PLUGIN_NAME: &str = "@deepseek-ai/dsh-mcp-client";
/// cordis patch op 的插入键与条目级启停键（loader 原生语义，HMR 热生效）。
pub const CORDIS_OP_INSERT: &str = "insert";
pub const CORDIS_ENTRY_DISABLED: &str = "disabled";

// ── 目录选择器（picker.rs 启动钉 browse；mobile.css 适配其对话框）────
// 上游出处：dsh-web-app/cordis.patch.yml 的 `- id: directory-picker` 行（name 为
// dsh-host-directory-picker-auto，启动期一次性决议：绑 127.0.0.1 + win32 ⇒ native
// Win32 系统对话框——弹在电脑屏幕上，远程手机端不可见不可用）；官方 pin 方式见
// apps/web/tests/pin-browse-picker.overlay.yml 与该 bundle patch 的行注释
//（"Mount -native or -browse directly in an overlay to pin the interaction"）。
// 影响面：行 id 写错 = disable 落空（桌面回到原生对话框、手机照旧不可用）；
// browse 包名写错 = insert 行解析不到插件，dsh 启动报错。
/// shipped bundle patch 里 auto 行的 id（picker.rs 的 disable 目标）。
pub const PICKER_AUTO_ROW_ID: &str = "directory-picker";
/// 钉入的 browse 后端行 id 与包名（host 侧列目录/建目录能力）。
pub const PICKER_BROWSE_HOST_ROW_ID: &str = "directory-picker-browse";
pub const PICKER_BROWSE_HOST_PKG: &str = "@deepseek-ai/dsh-host-directory-picker-browse";
/// 钉入的 browse 网页表面行 id 与包名（占用 ui-workspace 的 directory-flow 槽位）。
pub const PICKER_BROWSE_SURFACE_ROW_ID: &str = "ui-directory-picker-browse";
pub const PICKER_BROWSE_SURFACE_PKG: &str = "@deepseek-ai/dsh-client-ui-directory-picker-browse";

// ── browse 选择器运行时补丁（pickerpatch.rs；MARKER 与补丁内容是我方产物）──
// browse 对话框是 picker.rs 钉入的官方 overlay 形态，但两件事实让它在
// Windows/远程场景不够用：(1) host 侧 list() 只认"全限定路径"，没有任何
// 盘符枚举入口——面包屑在 home 子树内被 displayCrumbs 折叠成单个"主页"，
// 爬到其它盘只能手输路径（移动端正面的痛点）；(2) 客户端 showHidden
// 默认 false 且每次打开对话框重置，页脚"显示隐藏文件"开关在手机一行
// 布局里挤占"新建文件夹"。补丁 = 原地改写两个包内文件（dsh 自更新还原后
// 下次启动重打）：host 增加 "dsh:drives" 哨兵层级（list 特判返回 A-Z 可用
// 盘符根；ancestryCrumbs 对盘符根路径前插"此电脑"面包屑），客户端默认
// 显示隐藏条目 + 面包屑保留并本地化哨兵 crumb + 哨兵层级禁用"打开/新建
// 文件夹"（避免把哨兵当工作区选中）。
/// host 端 browse 后端与客户端对话框的包内文件路径（相对 dsh 运行时
/// node_modules 目录——与 WELCOME_NOTICE_CLIENT_SEGMENTS 同一基准，npm
/// 把子包平铺在 @deepseek-ai/ 下，dsh 包根内不嵌套）。
pub const PICKER_HOST_BROWSE_FILE_SEGMENTS: &[&str] = &[
    "@deepseek-ai",
    "dsh-host-directory-picker-browse",
    "lib",
    "index.js",
];
pub const PICKER_CLIENT_BROWSE_FILE_SEGMENTS: &[&str] = &[
    "@deepseek-ai",
    "dsh-client-ui-directory-picker-browse",
    "lib",
    "client.js",
];
/// 盘符枚举哨兵路径：host list() 的特判入参，客户端面包屑点击/打开禁用判定共用。
pub const PICKER_DRIVES_SENTINEL: &str = "dsh:drives";
/// 补丁锚定 needle（上游出处：两文件的 rc 原文；任一缺失 = 上游形态变了，整组停手）。
/// host：list() 全限定校验行（哨兵分支插在它与 homedir 之间）。
pub const PICKER_HOST_LIST_NEEDLE: &str =
    "if (path !== void 0 && !fullyQualified(path)) throw new DirectoryPickerError";
/// host：ancestryCrumbs 到根即返回行（盘符根前插"此电脑" crumb 的锚点）。
pub const PICKER_HOST_CRUMBS_NEEDLE: &str = "if (parent === current) return crumbs;";
/// client：showHidden 初值与开框重置（默认显示隐藏 = 两处都改 true）。
pub const PICKER_CLIENT_HIDDEN_INIT_NEEDLE: &str =
    "const [showHidden, setShowHidden] = (0, react.useState)(false);";
pub const PICKER_CLIENT_HIDDEN_RESET_NEEDLE: &str = "setShowHidden(false);";
/// client：displayCrumbs 的 home 折叠（哨兵 crumb 须在折叠后仍居首）。
pub const PICKER_CLIENT_CRUMBS_NEEDLE: &str = "if (homeIndex === -1) return listing.crumbs;";
/// client：面包屑渲染（哨兵 crumb 走本地化文案而非 host 写死的名字）。
pub const PICKER_CLIENT_CRUMB_LABEL_NEEDLE: &str = "children: crumb.name";
/// client：页脚两个按钮的禁用表达式（哨兵层级禁"打开/新建文件夹"）。
pub const PICKER_CLIENT_OPEN_DISABLED_NEEDLE: &str =
    "disabled: targetPath === null || loading || parentInert || draftPending,";
pub const PICKER_CLIENT_NEWFOLDER_DISABLED_NEEDLE: &str =
    "disabled: parent === null || loading || parentInert || draftPending,";
/// client：locale 字典行（注册 browser.drives 文案）。
pub const PICKER_CLIENT_LOCALE_ZH_NEEDLE: &str = r#""browser.showHidden": "显示隐藏文件""#;
pub const PICKER_CLIENT_LOCALE_EN_NEEDLE: &str = r#""browser.showHidden": "Show hidden files""#;

// ── 预设签名（presets.rs 只读探测；补丁器已于 rc.8 退役，MARKER 与补丁
// 内容曾是我方产物，随补丁器一并删除）──────────────────────────────────
/// minimal 预设目录的 **node_modules 相对路径**（基准 = dsh_node_modules_dir）。
/// 0.1.2 起预设从 dsh 包搬入独立的 @deepseek-ai/dsh-agent-presets 包（apps/cli 的
/// files 收缩为 lib/*.js；上游出处：preset/agent-presets/package.json files:
/// lib+presets）。发布形态 2026-09-04 rc.1 真实包实测落盘：
/// dsh/node_modules/@deepseek-ai/dsh-agent-presets/presets/minimal。
/// 影响：tests/upstream_contract.rs probe_presets 的目录拼装（presets.rs 本身
/// 只消费最终目录，签名判定逻辑不变）。
pub const PRESET_DIR_SEGMENTS: &[&str] = &["@deepseek-ai", "dsh-agent-presets", "presets", "minimal"];
pub const PRESET_COMPOSITION_FILE: &str = "agent.cordis.yml";
/// 破损签名：引用了 PTY 持久 bash 工具。rc.8 起该行仍在但带 win32 禁用门控
/// （上游已自修），所以单凭此 needle 命中不再意味着需要补丁。
pub const PRESET_BROKEN_NEEDLE: &str = "dsh-tool-bash-persistent";
/// 平台分支特征（内容出现 win32）。rc.8 起命中是**期望状态**（上游自修的
/// 证据）；若哪天不再命中且破损签名仍在 = 上游回退了修复，契约套件翻红。
pub const PRESET_PLATFORM_NEEDLE: &str = "win32";
/// 0.1.5 起 minimal 预设删掉整个 filesystem 组（fs-local + str-replace-editor），
/// 只剩持久 shell——"极简模式"从"持久 shell + 文件编辑"降为单工具（**用户可感**，
/// 上游默认工具调整的一部分）。探针断言该串**不再出现**在 minimal 的
/// agent.cordis.yml 里：它重新出现 = 上游把文件工具装回去了，发版说明口径要跟着改。
pub const PRESET_MINIMAL_EDITOR_ABSENT_NEEDLE: &str = "str-replace-editor";

// ── 远程代理（remote/proxy.rs 的 bundle 改写）────────────
/// dsh 内测声明（WelcomeNoticeStore）的持久化选择三元式。
/// 须含 `ctx.remote.$host.` 前缀，否则替换后残留 `ctx.remote.$host."host"` 直接
/// 语法错误（旧版 receiver 是 `connection.`；0.1.2 构建形态变为
/// `ctx.remote.$host.isLoopback`——2026-09-04 rc.1 实测落盘
/// dsh-client-ui-settings/lib/client.js，全 node_modules 的 client.js 唯一命中）。
/// 影响：proxy.rs 改写失效时内测声明每次远程连接都弹（功能不崩，静默退化）。
pub const WELCOME_NOTICE_NEEDLE: &[u8] = br#"ctx.remote.$host.isLoopback ? "host" : "memory""#;

/// dsh Web UI 入口文档的 viewport meta content 属性值（不含外层引号）。
/// 实测落盘：@deepseek-ai/dsh-web-frontend/dist/index.html（Vite 模板
/// `<meta name="viewport" content="width=device-width, initial-scale=1" />`）。
/// 影响：proxy.rs 把它改写为禁缩放形态（VIEWPORT_META_REPLACEMENT）——iOS
/// WKWebView（含微信内置浏览器）聚焦 font-size<16px 的输入框时自动放大整页，
/// 收起键盘后缩放不复原，标签栏/输入框被推出可视区（0.5.4 手机实拍实踩）。
/// 上游若改 content 值，改写静默失效，契约探针翻红。
pub const VIEWPORT_META_NEEDLE: &[u8] = br#"content="width=device-width, initial-scale=1""#;
/// 禁缩放形态：maximum-scale=1 阻断聚焦自动缩放，user-scalable=no 阻断双指
/// 缩放（app 式远程 UI 不需要）；桌面浏览器本就不认这两条，远程桌面访问无感。
pub const VIEWPORT_META_REPLACEMENT: &[u8] =
    br#"content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no""#;

/// dsh Web UI 入口文档的 React 挂载点（Vite 模板原样，独占一行前的完整元素）。
/// 实测落盘：@deepseek-ai/dsh-web-frontend/dist/index.html。
/// 影响：proxy.rs 在它之后注入远程加载过渡页（splash）——隧道首连要下载
/// ~5MB（SPA ~1.2MB + 插件 bundle ~3.7MB），白屏几十秒观感如卡死；React
/// 挂载完成（#root 出现子节点）后 splash 自动淡出移除。needle 漂移则
/// splash 整体不注入（页面回到白屏等待，功能不损）；契约探针守门。
pub const SPA_ROOT_MOUNT_NEEDLE: &[u8] = br#"<div id="root"></div>"#;

// ── 插件管理（plugins.rs；dsh plugin 官方入口 + 壳内置 pnpm）────────
/// plugin 子命令与 profile 参数。上游出处：bin.js command("plugin") +
/// requiredOption("--profile <name>")，参数原样透传给 pnpm。
pub const DSH_PLUGIN_SUBCOMMAND: &str = "plugin";
pub const DSH_PLUGIN_PROFILE_FLAG: &str = "--profile";
/// 壳面板管理的目标 profile（= `dsh web` 的别名 profile）。
pub const DSH_WEB_PROFILE_NAME: &str = "web";
/// profile 目录与清单文件。上游出处：dsh-app-boot 的 initProfile/writeProfileManifest。
pub const PROFILE_DIR_SEGMENTS: &[&str] = &["profiles", "web"];
pub const PROFILE_MANIFEST_FILE: &str = "package.json";
/// 清单 JSON 路径：已装依赖 / 插件层列表（reconcile 只认声明 dsh.bundle 的包）。
pub const MANIFEST_DEPENDENCIES_KEY: &str = "dependencies";
pub const MANIFEST_BUNDLES_POINTER: &str = "/dsh/profile/bundles";
/// profile 内插件依赖的 bin 目录段（pnpm 在 profile 目录装包后生成的
/// node_modules/.bin 标准布局，插件自带 CLI 的 shim 落在此处，Windows 为
/// 无扩展/.CMD/.ps1 三件套）。上游出处：dsh plugin 子命令内部以 pnpm add
/// 写进 profile 目录。影响：process.rs spawn dsh 时把它前置进子进程 PATH，
/// 会话终端/工具子进程才能按名解析插件 CLI；pnpm 若改 .bin 布局则解析不到
/// （静默退化，不崩——用户只能满路径调用）。
pub const PROFILE_BIN_DIR_SEGMENTS: &[&str] = &["node_modules", ".bin"];
/// 壳内置 pnpm 两个文件（fetch-runtime.ps1 产出，位于 node.exe 同目录）。
pub const PNPM_JS_FILE: &str = "pnpm.cjs";
pub const PNPM_CMD_FILE: &str = "pnpm.cmd";

// ── 远程"项目"标签（remote/project.rs + project.html）────────────────
/// 工作区注册表的 DSH_HOME 相对路径。上游出处：dsh-workspace 的 storage unit
/// 落盘机制（unit.name=="workspace"，storages/<unit>.json）。schema：
/// tables.workspaces[<id>] = { path, title, sessionIds[], createdAt, updatedAt }。
/// 影响面：写错 = resolve 永远 404，"项目"标签空态。
pub const WORKSPACE_STORE_SEGMENTS: &[&str] = &["storages", "workspace.json"];
/// SPA 在 localStorage 记当前会话的键（值形 {"sessionId":"session-…"}，切会话即写）。
/// 实测落盘：0.1.2 在 @deepseek-ai/dsh-client-runtime（该包 0.1.2 后停发），
/// **0.1.5 起在 @deepseek-ai/dsh-api-session-controller/lib/client.js** ——探针是
/// 全树 tree_find，换包不影响判绿，但别照旧注释去找那个已消失的包。
/// 影响面：改名 = project.html 取不到当前会话（面板显示"未找到会话"）。
pub const LOCALSTORAGE_CURRENT_SESSION_KEY: &str = "dsh.sessions.current";
/// workspace.json schema 锚点字段（dsh-workspace/lib/index.js 内必现）。
pub const WORKSPACE_SCHEMA_NEEDLE: &str = "sessionIds";

// ── 移动端注入样式锚点（remote/mobile.css）─────────────────────────
/// 会话头部"更多操作"图标按钮的 CSS Modules 本地名。实测落盘：
/// @deepseek-ai/dsh-session-log-export/lib/client.js（CSS 文本与 JSX className
/// 均必现；全 node_modules 仅此一包命中，无同名歧义）。
/// **0.1.5 语义变更**：旧版是"Session log 下载按钮"（本地名 sessionLogButton，
/// 文字药丸）；0.1.5 改为 28px 圆角图标按钮 moreButton（aria-label=header.more），
/// 下载动作收进它弹出的菜单，整条注册进槽位 conversation.session.header.utilities。
/// 影响面：mobile.css 的 [class*="_moreButton"] 规则（手机端不翻 session 日志，
/// 且该按钮悬浮盖住"N 个后台任务运行中"文案）——上游改名则规则静默失效
/// （按钮复原显示，功能不损）；契约套件 tree_find 守门。
pub const SESSION_HEADER_MORE_BUTTON_NEEDLE: &str = "moreButton";

/// 输入栏模型槽位的 data-slot 语义钩子。实测落盘：
/// @deepseek-ai/dsh-client-ui-conversation/lib/client.js（InputBar 以
/// renderSlot("conversation.input.model") 渲染，槽位注册同文件）。影响面：
/// mobile.css 的模型选择器图标化/菜单包含块上移两组规则——上游改钩子名则
/// 静默失效（回到未适配态：全名药丸窄屏截断、菜单左缘越界）。
pub const MODEL_SLOT_HOOK: &str = "conversation.input.model";
/// 模型触发器"模型名"文案段的 CSS Modules 本地名。实测落盘：
/// @deepseek-ai/dsh-client-ui-model-selection/lib/client.js（trigger 内
/// span.triggerLabel，文案 = 模型名或 trigger.fallback）。影响面：同
/// MODEL_SLOT_HOOK 的图标化规则——改名则模型名露出。
pub const MODEL_TRIGGER_LABEL_NEEDLE: &str = "triggerLabel";
/// 模型触发器"推理等级"文案段的 CSS Modules 本地名（同包同 trigger 内
/// span.triggerEffort，无显式等级时显示 effort.providerDefault 文案
/// "Default"——无 API key 机器上模型选择器露出 "Default" 文本的根源）。
/// 影响面：图标化规则须把两段文案一起隐藏，漏掉此段则裸文本药丸把
/// trailing 组挤换行（0.4.7 实踩）。
pub const MODEL_TRIGGER_EFFORT_NEEDLE: &str = "triggerEffort";

// ── 预装 /init 插件的 UI 折叠锚点（resources/preseed-plugins/dsh-command-init）──
/// 消息渲染的"用户气泡 vs 折叠上下文行"分支。实测落盘：
/// @deepseek-ai/dsh-client-ui-chat/lib/client.js（0.1.2 起聊天渲染从
/// ui-conversation 拆入 ui-chat 包，2026-09-04 rc.1 实测命中；messageDefinition.start：
/// `event.data.source.kind !== "user"` → context 节点 → ContextInjectionRow
/// 折叠行；kind=="user" 才渲染完整气泡）。影响面：/init 注入的长提示词靠
/// source.kind="plugin" 落进折叠的「上下文注入」行——分支改掉则提示词重新
/// 渲染成完整气泡（功能不损、美观回退）；契约套件 tree_find 守门。
pub const CONTEXT_INJECTION_BRANCH_NEEDLE: &str = r#"source.kind !== "user""#;
/// 折叠行标题的 locale 键（同文件 "message.contextInjection"，zh="上下文注入"）。
/// 影响面：键消失意味着 ContextInjectionRow 整条渲染路径改版，同上。
pub const CONTEXT_INJECTION_TITLE_NEEDLE: &str = "contextInjection";
/// notice form 摘要的读取函数（同文件 noticeSummary(source)，读 source.summary
/// 显示在折叠行标题旁）。影响面：改名则 /init 折叠行只剩插件名、一行摘要丢失。
pub const NOTICE_SUMMARY_NEEDLE: &str = "noticeSummary";
/// 命令定义的可选附件旗标名。上游出处：@deepseek-ai/dsh-commands 的
/// register(definition) 校验（0.1.2 是 `input.images` 布尔，0.1.5 改名
/// `input.attachments`）。我方 /init 插件**不带**该旗标（只用
/// name/description/input.hint/handler），两边都兼容——本常量是兼容哨兵：
/// 上游再动命令旗标/注册面时探针翻红，提醒复核预装插件。
pub const COMMANDS_ATTACHMENT_FLAG: &str = "attachments";

// ── dsh 跨进程写锁（locks.rs 陈旧锁自愈的依据）─────────────────
/// 锁文件的命名：目标文件的**兄弟**路径 `<filename>.lock`。上游出处：
/// @deepseek-ai/dsh-atomic-write/lib/index.js 的 withFileLock（`wx` 独占创建、
/// 内容 `${process.pid}\n`、只在 finally 里 rm）。关键性质：**竞争方永不删除
/// 已存在的锁**（源码注释原文 orphan recovery is an operator action），过期只
/// 报错、不猜所有权。影响面：持有方被硬杀（Windows 的 TerminateProcess）即永久
/// 残留 → 之后每次启动都在 boot 阶段超时，应用再也起不来（机器 B 实踩：
/// `.credentials.yaml.lock` 卡住 dsh-client-connection 的 apply，插件树加载失败、
/// 进程直接退出，壳只看到"就绪行没出现"）。locks.rs::heal_stale_locks 在每次
/// spawn 前按 pid 存活性清理；上游一旦自己做孤儿恢复、或改锁名/内容格式，
/// tests/upstream_contract.rs 的锁探针会翻红，届时撤掉自愈。
pub const LOCK_FILE_SUFFIX: &str = ".lock";
/// 凭证写入的锁等待预算（上游常量 DOCUMENT_LOCK_WAIT_MS = 30000）。出处同上的
/// 调用方 @deepseek-ai/dsh-credentials-local/lib/index.js。影响面：诊断面板/日志
/// 里解释"卡 30s 才报错"的数值来源。
pub const CREDENTIALS_LOCK_WAIT_SECS: u64 = 30;
/// 陈旧锁扫描的最大目录深度（相对 DSH_HOME；0=根层）与跳过的目录名。
/// 锁的落点都在 home 根与浅层（credentials.yaml / settings.yaml 及其它状态文件），
/// node_modules 下是插件树（深且无锁），整棵跳过。
pub const LOCK_SCAN_MAX_DEPTH: usize = 3;
pub const LOCK_SCAN_SKIP_DIRS: &[&str] = &["node_modules"];
