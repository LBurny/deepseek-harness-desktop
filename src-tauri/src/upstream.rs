//! dsh 上游内部事实的单一来源（跟版门禁）。
//!
//! 跟版流程：fetch-runtime.ps1 抓新版 → `cargo test` →
//! tests/upstream_contract.rs 红了就对照本文件逐条改（每条注明上游出处
//! 与影响面）。事实清单的文档形态见 docs/design.zh-CN.md §15。
//!
//! 当前事实基线：@deepseek-ai/dsh 0.1.1-rc.2（子包为浮动区间，抓取时解析到
//! 最新 rc；npm latest 标签可能滞后，fetch-runtime.ps1 须显式 -DshVersion）。
//!
//! 下一版预研：上游已打 dsh-v0.1.2-alpha.1（npm 未发布），逐条漂移核对与跟版
//! runbook 见 docs/upstream-0.1.2-alpha.1-prep.zh-CN.md——大改三件：Web 鉴权
//! （launch token + cookie）、事件传输重写（/api/remote.mux + $events +
//! per-session follow）、shipped 预设搬进 dsh-agent-presets 包。

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

// ── 事件通道（lib.rs 接线；帧事实 notify/mod.rs 分类）────
/// 会话事件下行（GET 返回 426，仅 WS）。影响：lib.rs WsSource、notify/ws.rs。
pub const EVENTS_MUX_PATH: &str = "/api/events.mux";
/// 主机事件下行（子代理 origin 标记）。影响：同上。
pub const EVENTS_HOST_PATH: &str = "/api/events.host";
/// server-request 帧：{"type":"server-request","method":<payload.type>,"payload":{...}}
pub const METHOD_APPROVAL: &str = "approval/requested";
pub const METHOD_QUESTION: &str = "question/requested";
pub const METHOD_SESSION_EVENT: &str = "session/event";
pub const METHOD_HOST_SESSION_ADDED: &str = "host/session-added";
pub const METHOD_HOST_SESSION_REMOVED: &str = "host/session-removed";
/// session/event 的 payload.event.type 值
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
/// host/session-added 的 payload.origin 子代理标记
pub const ORIGIN_SUBAGENT: &str = "subagent";

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
/// minimal 预设目录的包内相对路径。
pub const PRESET_DIR_SEGMENTS: &[&str] = &["config", "agent-presets", "minimal"];
pub const PRESET_COMPOSITION_FILE: &str = "agent.cordis.yml";
/// 破损签名：引用了 PTY 持久 bash 工具。rc.8 起该行仍在但带 win32 禁用门控
/// （上游已自修），所以单凭此 needle 命中不再意味着需要补丁。
pub const PRESET_BROKEN_NEEDLE: &str = "dsh-tool-bash-persistent";
/// 平台分支特征（内容出现 win32）。rc.8 起命中是**期望状态**（上游自修的
/// 证据）；若哪天不再命中且破损签名仍在 = 上游回退了修复，契约套件翻红。
pub const PRESET_PLATFORM_NEEDLE: &str = "win32";

// ── 远程代理（remote/proxy.rs 的 bundle 改写）────────────
/// dsh 内测声明（WelcomeNoticeStore）的持久化选择三元式。
/// 须含 `connection.` 前缀，否则替换后残留 `connection."host"` 直接语法错误。
/// 影响：proxy.rs 改写失效时内测声明每次远程连接都弹（功能不崩，静默退化）。
pub const WELCOME_NOTICE_NEEDLE: &[u8] = br#"connection.isLoopback ? "host" : "memory""#;

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
/// 实测落盘：@deepseek-ai/dsh-client-runtime/lib/client.js。影响面：改名 =
/// project.html 取不到当前会话（面板显示"未找到会话"）；契约套件 tree_find 守门。
pub const LOCALSTORAGE_CURRENT_SESSION_KEY: &str = "dsh.sessions.current";
/// workspace.json schema 锚点字段（dsh-workspace/lib/index.js 内必现）。
pub const WORKSPACE_SCHEMA_NEEDLE: &str = "sessionIds";

// ── 移动端注入样式锚点（remote/mobile.css）─────────────────────────
/// 会话头部"Session log"下载按钮的 CSS Modules 本地名。实测落盘：
/// @deepseek-ai/dsh-session-log-export/lib/client.js（按钮 CSS 文本内必现，
/// 且 height:32px/min-width:111px 写死）。影响面：mobile.css 的
/// [class*="_sessionLogButton"] 缩小规则——上游改名则规则静默失效
/// （药丸回原生尺寸，功能不损）；契约套件 tree_find 守门。
pub const SESSION_LOG_BUTTON_NEEDLE: &str = "sessionLogButton";

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
/// @deepseek-ai/dsh-client-ui-conversation/lib/client.js（messageDefinition.start：
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
