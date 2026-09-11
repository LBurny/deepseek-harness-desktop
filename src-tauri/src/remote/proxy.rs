//! token 门岗反向代理：远程访问的唯一入口。
//! 鉴权流：
//!   1. 请求带有效 cookie（__dsh_remote=<token>）→ 直接转发到 dsh
//!   2. 无有效 cookie 但 ?token= 匹配 → 302 到剥离 token 的同路径 + 种 cookie
//!      （token 只出现在首次点击的链接里，不留在地址栏/历史，后续 WS 也凭同源 cookie）
//!   3. ?token= 存在但不匹配 → 固定延迟 500ms 后 403（防在线爆破）
//!   4. 无任何凭据 → 403 门页
//! dsh 凭据经 creds watch 通道动态读取（0.1.2 BrowserAuth：cookie 由 launch token
//! 现换，缓存代持 + 401 失效重换重放一次）：dsh 重启换端口时代理不需要重启。
//! WS 升级请求（/api/remote.mux）不走 HTTP 转发：握手在代理终结（cookie 门岗对
//! 握手生效），与 dsh 另建 WS 后逐帧双向搬运（bridge_upgrade/bridge，dsh 侧
//! 升级同样须带 dsh-auth cookie——漏注入手机端表现为"页面开但全断"）。
//! 响应侧两处壳侧增强（只影响经代理的远程访问，桌面壳直连 dsh 不经这里）：
//!   - HTML 文档：注入 mobile.css/mobile.js 移动端适配 + splash 加载过渡页
//!     （React 挂载点 needle 命中才注入，SPA 挂载完成后自动淡出移除）
//!   - 大体积文本资产（≥4KB 的 js/css/json/svg/html）：代理侧缓冲 gzip——dsh
//!     服务端不做任何压缩，隧道首连 ~5MB identity 是远程白屏几十秒的根因之一

use crate::dsh_session::{self, DshCreds};
use super::token_eq;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequest, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::{watch, Notify};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message as DshMessage;

pub const COOKIE_NAME: &str = "__dsh_remote";
/// cookie 有效期 30 天：会话 cookie（无 Max-Age）会被手机浏览器在进程回收时
/// 丢弃——地址栏已被 302 剥掉 token，cookie 一丢即 403"链接无效或已过期"，
/// 同一次开启期间手机端被迫反复回电脑扫码。长效化后收藏地址栏 URL 也能直接用；
/// 吊销语义不变（reset_link 轮换 token，旧 cookie 值即刻不匹配）。
const COOKIE_MAX_AGE_SECS: u32 = 30 * 24 * 3600;
/// 错误 token 的固定响应延迟，拖慢在线猜测
const WRONG_TOKEN_DELAY: Duration = Duration::from_millis(500);
/// 请求体缓冲上限：重放（401 换 cookie 后重发一次）需要整读请求体；超过该
/// 体积的上传类请求不重放（cookie 失效表现为一次 401，刷新页面即恢复）
const REPLAY_BODY_LIMIT: usize = 64 * 1024 * 1024;

const GATE_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>DSHDesktop</title></head>\
<body style=\"font-family:sans-serif;display:flex;justify-content:center;align-items:center;height:100vh;margin:0\">\
<p>DSHDesktop 远程访问：链接无效或已失效。<br>\
若电脑端远程访问仍在开启，改点最初那条带 ?token= 的完整链接即可重新进入；<br>\
若已在电脑上重开远程访问，旧链接整体作废，请在电脑托盘菜单复制新链接。</p></body></html>";

/// 内测声明三元式 needle 已上移 crate::upstream::WELCOME_NOTICE_NEEDLE（单一事实源，
/// 含为何须带 `connection.` 前缀的说明）。改写语义：隧道场景 dsh 选 "memory" 持久化，
/// 确认记录不落 settings.yaml、每次连接都弹声明；改写为 "host" 后远程端与桌面端
/// 共用同一份持久化确认（桌面是回环源本就已写 host）。
const WELCOME_NOTICE_REPL: &[u8] = br#""host""#;
/// 只对不超过该体积的插件 bundle 做缓冲改写，超出原样透传（声明照弹，不破坏功能）。
/// 0.1.2 起合并加载：单条 combo 请求即全量插件客户端（实测 3.7MB），留增长余量
const REWRITE_BUFFER_LIMIT: u64 = 16 * 1024 * 1024;

/// 缓存击穿参数：dsh 把 bundle 响应标为 `immutable, max-age=1y`，而壳侧改写产物
/// 随壳版本变化（如内测声明持久化三元式）。未带 dshv 的 bundle 请求 302 到带
/// `dshv=<壳版本>` 的同 URL 强制重取一次；带参请求在转发前剥掉该参数（真 dsh
/// 对组合 URL 的 query 逐字校验，多余参数 404——0.5.4 真机实测），dsh 永远看不到。
const CACHE_BUST_PARAM: &str = "dshv";
const CACHE_BUST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 移动端适配样式（同目录 mobile.css，编译期内嵌）：远程访问的 dsh Web UI 在手机上
/// 有两处实测破版——设置弹窗内容区被 188px 固定导航列压到一字一行竖排、narrow 模式
/// 展开侧栏把主区压到 110px。桌面壳窗口最小宽 900px，命中不到 700px 断点，注入只
/// 影响经代理的远程访问。选择器锚 role/data-* 语义钩子与 CSS Modules 本地名子串，
/// dsh 版本更新哈希变化不失效；上游改名则静默回到未适配状态。
const MOBILE_CSS: &str = include_str!("mobile.css");
/// 信息标签页脚本：≤700px 时在"对话/轨迹"旁加"信息"标签，统计行克隆进面板
/// （克隆而非搬家——React 对被移节点 removeChild 必崩）。失效时 CSS 换行兜底。
const MOBILE_JS: &str = include_str!("mobile.js");
/// 注入标记：测试断言与排查时识别（注释节点，无渲染影响）
const MOBILE_INJECT_MARKER: &str = "<!-- dshdesktop-mobile -->";

/// 远程加载过渡页（splash）：dsh 服务端不做任何压缩，经隧道的远程首连要下载
/// ~5MB（SPA ~1.2MB + 插件 bundle ~3.7MB——bundle 因壳侧改写还被迫走 identity），
/// 弱网白屏几十秒、观感如卡死（0.5.5 手机实拍反馈）。rewrite_html_document 把
/// React 挂载点（upstream::SPA_ROOT_MOUNT_NEEDLE，命中才注入，漂移则整体跳过）
/// 替换为 挂载点+splash 覆盖层（spinner+标题+12s 后才淡入的弱网提示，深浅色随
/// prefers-color-scheme，文案语言随 navigator.language）；React 挂载完成
/// （#root 出现子节点）后 splash.js 淡出移除。桌面壳直连 dsh 不经代理，只有
/// 远程访问看到它。
const SPLASH_CSS: &str = include_str!("splash.css");
const SPLASH_JS: &str = include_str!("splash.js");
/// splash 覆盖层标记（hint 文案由 splash.js 按 navigator.language 填）
const SPLASH_HTML: &str = concat!(
    r#"<div id="dsh-splash" role="status"><div class="dsh-splash-spinner"></div>"#,
    r#"<p class="dsh-splash-title">DeepSeek Harness</p><p class="dsh-splash-hint"></p></div>"#,
);

/// splash 替换串 = 挂载点原样 + splash 覆盖层 + 卸载脚本。脚本紧随标记注入，
/// 解析到即执行——此时 #root 与 splash 都已存在，无需等 DOMContentLoaded
fn splash_replacement() -> Vec<u8> {
    let mut v = Vec::with_capacity(
        crate::upstream::SPA_ROOT_MOUNT_NEEDLE.len() + SPLASH_HTML.len() + SPLASH_JS.len() + 16,
    );
    v.extend_from_slice(crate::upstream::SPA_ROOT_MOUNT_NEEDLE);
    v.extend_from_slice(SPLASH_HTML.as_bytes());
    v.extend_from_slice(b"<script>");
    v.extend_from_slice(SPLASH_JS.as_bytes());
    v.extend_from_slice(b"</script>");
    v
}

/// 代理侧 gzip 的体积门槛：小于它压缩收益不抵 CPU 与首字节延迟；上限复用
/// REWRITE_BUFFER_LIMIT（缓冲边界一致）。dsh 服务端不做任何压缩（0.1.2 全包
/// grep 无 gzip/deflate 落盘代码），经隧道的远程首连 ~5MB 文本资产全走
/// identity——splash 管观感，gzip 管实际时长（flate2 随 zip 已在依赖树）
const GZIP_MIN_SIZE: u64 = 4 * 1024;

/// 响应 content-type 是否值得压缩（字体/图片等自带压缩的格式不在列）
fn gzip_eligible_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase())
        .is_some_and(|ct| {
            matches!(
                ct.as_str(),
                "application/javascript"
                    | "text/javascript"
                    | "text/css"
                    | "application/json"
                    | "image/svg+xml"
                    | "text/html"
            )
        })
}

/// 客户端（浏览器）是否宣告接受 gzip：必须在转发路径剥 accept-encoding 之前
/// 从原始请求头捕获（按 token 匹配，"xgzip" 不误伤）
fn client_accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim().starts_with("gzip")))
}

/// 同步压缩核心（单测直接覆盖）；失败回 None，调用方回退 identity 原样服务
fn gzip_compress(body: Vec<u8>) -> Option<Vec<u8>> {
    use flate2::write::GzEncoder;
    use std::io::Write;
    let mut enc = GzEncoder::new(
        Vec::with_capacity(body.len() / 3 + 64),
        flate2::Compression::default(),
    );
    enc.write_all(&body).ok()?;
    enc.finish().ok()
}

/// spawn_blocking 压缩（3.7MB 约一两百毫秒，别堵执行器线程）
async fn gzip_bytes(body: Vec<u8>) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || gzip_compress(body))
        .await
        .ok()
        .flatten()
}

/// 插件客户端 bundle 路径。两种形态都要命中：
/// - 单插件（0.1.1 及以前）：`/plugins/<id>/client.js[?rev=N]`
/// - 合并加载（0.1.2 起，真机实测）：`/plugins/??<a>/client.js,<b>/client.js,...&rev=N`
///   ——path 部分只剩 "/plugins/"，组合清单整体在 query 里；漏掉 combo 形态则改写
///   静默失效（0.5.3 实踩：远程端内测声明每次连接都弹）
fn is_plugin_client_bundle(path_and_query: &str) -> bool {
    if path_and_query.starts_with("/plugins/??") {
        return true;
    }
    let path = path_and_query.split('?').next().unwrap_or(path_and_query);
    path.starts_with("/plugins/") && path.ends_with("/client.js")
}

/// 转发前剥掉壳自己的缓存击穿参数（dsh 对 query 逐字校验，多余参数 404）。
/// 其余参数（含组合清单本体）原样保序保留。
fn strip_cache_bust(path_and_query: &str) -> String {
    let Some((path, query)) = path_and_query.split_once('?') else {
        return path_and_query.to_string();
    };
    let kept: Vec<String> = url::form_urlencoded::parse(query.as_bytes())
        .filter(|(k, _)| k != CACHE_BUST_PARAM)
        .map(|(k, v)| {
            if v.is_empty() {
                k.to_string()
            } else {
                format!("{k}={v}")
            }
        })
        .collect();
    if kept.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

/// 是否该做缓存击穿重定向：GET/HEAD、非 WS 升级、bundle 形态、且未带 buster
fn wants_cache_bust(req: &Request, path_and_query: &str) -> bool {
    if !matches!(*req.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        return false;
    }
    if wants_websocket(req.headers()) {
        return false;
    }
    if !is_plugin_client_bundle(path_and_query) {
        return false;
    }
    let has_buster = req.uri().query().is_some_and(|q| {
        url::form_urlencoded::parse(q.as_bytes()).any(|(k, _)| k == CACHE_BUST_PARAM)
    });
    !has_buster
}

/// 浏览器文档导航请求（accept 含 text/html）：HTML 改写路径的候选
fn wants_html_document(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"))
}

/// 当前有效 token 的共享单元：重置链接时原地轮换，门岗逐请求读最新值
type TokenCell = Arc<RwLock<Arc<str>>>;

#[derive(Clone)]
pub(crate) struct ProxyState {
    pub(crate) token: TokenCell,
    /// 重置链接时 notify_waiters 掐断所有已建立的 WS 桥接
    pub(crate) drain: Arc<Notify>,
    /// dsh 凭据（端口 + launch token；0.1.2 起 cookie 由 token 现换）
    pub(crate) creds: watch::Receiver<Option<Arc<DshCreds>>>,
    /// 代持的 dsh-auth cookie（端口绑定；401 失效即清缓存重换）
    pub(crate) dsh_cookie: Arc<Mutex<Option<String>>>,
    pub(crate) client: reqwest::Client,
    /// dsh-home（project.rs 解析 storages/workspace.json 用），与 skills/mcp 同源
    pub(crate) dsh_home: PathBuf,
}

pub struct ProxyHandle {
    pub port: u16,
    token: TokenCell,
    drain: Arc<Notify>,
    stop: Arc<Notify>,
    stopped: Arc<Notify>,
}

impl ProxyHandle {
    /// 优雅停服并等待监听器与空闲连接关闭（消费自身；句柄不可复用）。
    /// hyper 的 graceful shutdown 会立即关闭空闲 keep-alive 连接，只等在途请求。
    pub async fn shutdown(self) {
        // 先掐断长驻的 WS 桥接，否则 stopped 要等它们自然结束
        self.drain.notify_waiters();
        self.stop.notify_one();
        self.stopped.notified().await;
    }

    /// 重置访问链接：轮换 token（旧链接/旧 cookie 立即失效）并掐断所有
    /// 已建立的 WS 桥接；监听器与端口不变，隧道无需重启、域名不变。
    pub fn reset_token(&self, new_token: Arc<str>) {
        *self.token.write().unwrap() = new_token;
        self.drain.notify_waiters();
    }
}

pub async fn spawn_proxy(
    token: Arc<str>,
    creds: watch::Receiver<Option<Arc<DshCreds>>>,
    dsh_home: PathBuf,
    bind: SocketAddr,
) -> std::io::Result<ProxyHandle> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let port = listener.local_addr()?.port();
    let token_cell: TokenCell = Arc::new(RwLock::new(token));
    let drain = Arc::new(Notify::new());
    let stop = Arc::new(Notify::new());
    let stopped = Arc::new(Notify::new());
    let stop2 = stop.clone();
    let stopped2 = stopped.clone();
    let state = ProxyState {
        token: token_cell.clone(),
        drain: drain.clone(),
        creds,
        dsh_cookie: Arc::new(Mutex::new(None)),
        client: reqwest::Client::builder()
            // 3xx 原样透传给浏览器，不由代理代为跟随
            .redirect(reqwest::redirect::Policy::none())
            // 转发目标是本机 127.0.0.1 的 dsh：必须绕开系统代理（HTTP_PROXY 等），
            // 否则用户开了系统代理时转发会被劫持到代理软件上
            .no_proxy()
            .build()
            .expect("reqwest client"),
        dsh_home,
    };
    let app = Router::new()
        // 壳自有路由：手机端"项目"标签的单页与只读文件 API（project.rs）
        .route(crate::remote::project::PAGE_PATH, get(crate::remote::project::page))
        .route(crate::remote::project::API_RESOLVE_PATH, get(crate::remote::project::resolve))
        .route(crate::remote::project::API_LIST_PATH, get(crate::remote::project::list))
        .route(crate::remote::project::API_FILE_PATH, get(crate::remote::project::file))
        .fallback(handler)
        // 门岗必须是覆盖全 Router 的中间件：壳自有路由（/__dsh-desktop/*）若只
        // 靠 fallback 内部判断会绕过鉴权直接暴露
        .layer(middleware::from_fn_with_state(state.clone(), gate_middleware))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                stop2.notified().await;
            })
            .await
            .ok();
        stopped2.notify_one();
    });
    Ok(ProxyHandle {
        port,
        token: token_cell,
        drain,
        stop,
        stopped,
    })
}

/// token 门岗中间件（鉴权流与本文件头注释一致）：cookie 放行 / ?token= 播种 302 /
/// 错 token 延迟 403 / 无凭据 403。覆盖自有路由与转发 fallback 的全部请求。
async fn gate_middleware(State(st): State<ProxyState>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let query_token = req
        .uri()
        .query()
        .and_then(|q| url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == "token"))
        .map(|(_, v)| v.into_owned());

    // 逐请求读最新 token：重置链接后旧凭据即刻失效
    let current_token = st.token.read().unwrap().clone();
    let cookie_ok = cookie_authed(headers, &current_token);
    match (cookie_ok, query_token) {
        (true, _) => {
            // 缓存击穿：bundle 响应被 dsh 标 immutable 一年，壳侧改写产物随壳版本
            // 变化——未带 buster 的 bundle 请求 302 到带 dshv 的同 URL 强制重取
            if wants_cache_bust(&req, &path_and_query) {
                let sep = if path_and_query.contains('?') { '&' } else { '?' };
                return (
                    StatusCode::FOUND,
                    [
                        (
                            header::LOCATION,
                            format!("{path_and_query}{sep}{CACHE_BUST_PARAM}={CACHE_BUST_VERSION}"),
                        ),
                        (
                            header::CACHE_CONTROL,
                            "no-store".to_string(),
                        ),
                    ],
                )
                    .into_response();
            }
            next.run(req).await
        }
        (false, Some(t)) if token_eq(&t, &current_token) => {
            // 302 剥离 token + 种 cookie；浏览器地址栏不留凭据
            let location = strip_token_query(&path_and_query);
            (
                StatusCode::FOUND,
                [
                    (header::LOCATION, location),
                    (
                        header::SET_COOKIE,
                        format!(
                            "{COOKIE_NAME}={t}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}"
                        ),
                    ),
                ],
            )
                .into_response()
        }
        (false, Some(_)) => {
            tokio::time::sleep(WRONG_TOKEN_DELAY).await;
            gate()
        }
        (false, None) => gate(),
    }
}

async fn handler(State(st): State<ProxyState>, req: Request) -> Response {
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    // WS 升级请求（浏览器连 /api/events.*）：HTTP 转发路径会剥离逐跳头导致升级降级，
    // 改为在代理终结握手、按帧桥接到 dsh
    if wants_websocket(req.headers()) {
        return bridge_upgrade(st, req, path_and_query).await;
    }

    forward(st, req, &path_and_query).await
}

/// 代持 cookie 的获取：有缓存用缓存；无则从 creds 取 token 现换（0.1.2
/// BrowserAuth）。creds 未就绪或换取失败 → None（转发不带 cookie，dsh 会回
/// 401，由调用方决定透传或重放）。cookie 只在内存，不落日志。
async fn ensure_cookie(st: &ProxyState, force_refresh: bool) -> Option<String> {
    if !force_refresh {
        let cached = st.dsh_cookie.lock().unwrap().clone();
        if let Some(c) = cached {
            return Some(c);
        }
    }
    let Some(c) = st.creds.borrow().clone() else {
        return None;
    };
    match dsh_session::exchange_cookie(c.port, &c.token).await {
        Ok(k) => {
            *st.dsh_cookie.lock().unwrap() = Some(k.clone());
            Some(k)
        }
        Err(_) => {
            // token 未就绪/已轮换：不缓存失败态，下次请求重试
            *st.dsh_cookie.lock().unwrap() = None;
            None
        }
    }
}

async fn forward(st: ProxyState, req: Request, path_and_query: &str) -> Response {
    // 壳侧缓存击穿参数只在代理与浏览器之间有意义，转发前剥掉（dsh query 逐字校验）
    let path_and_query = &strip_cache_bust(path_and_query);
    // 流式上传路由（dsh 0.1.5 的 /api/session/uploadFileBinary，requestBody:
    // "streaming"）走独立路径：请求体不可重放、可能远超 REPLAY_BODY_LIMIT，
    // 绝不能在这里整读缓冲（旧实现 64MiB 上限把手机大文件上传打成 502）
    if crate::upstream::is_streaming_body_route(path_and_query) {
        return forward_streaming(st, req, path_and_query).await;
    }
    let Some(c) = st.creds.borrow().clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "dsh 未就绪").into_response();
    };
    let url = format!("http://127.0.0.1:{}{}", c.port, path_and_query);
    let rewrite_bundle = is_plugin_client_bundle(path_and_query);
    let wants_html = wants_html_document(req.headers());
    let method = req.method().clone();
    // 重放（401 换 cookie 后重发一次）需要整读请求体与请求头副本（into_body 消费 req）
    let req_headers = req.headers().clone();
    // 代理侧 gzip 的两个前提从原始请求头捕获：压缩意愿（转发路径会剥掉
    // accept-encoding，之后就拿不到）与 Range（区间请求不做变换）
    let client_gzip = client_accepts_gzip(&req_headers);
    let has_range = req_headers.contains_key(header::RANGE);
    let body_bytes = match axum::body::to_bytes(req.into_body(), REPLAY_BODY_LIMIT).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_GATEWAY, "读取请求体失败").into_response(),
    };
    let cookie = ensure_cookie(&st, false).await;
    let res = send_forwarded(
        &st,
        &req_headers,
        &method,
        &url,
        cookie.as_deref(),
        reqwest::Body::from(body_bytes.clone()),
        rewrite_bundle,
    )
    .await;
    let mut res = match res {
        Some(r) => r,
        None => return (StatusCode::BAD_GATEWAY, "dsh 连接失败").into_response(),
    };
    // 401：代持 cookie 失效（token 轮换/dsh 重启换端口）→ 清缓存重换一次并
    // 重放（只重放一次防环）；换不到新 cookie 则把 401 原样透传给浏览器
    if res.status() == StatusCode::UNAUTHORIZED {
        st.dsh_cookie.lock().unwrap().take();
        if let Some(fresh) = ensure_cookie(&st, true).await {
            if Some(&fresh) != cookie.as_ref() {
                res = match send_forwarded(
                    &st,
                    &req_headers,
                    &method,
                    &url,
                    Some(&fresh),
                    reqwest::Body::from(body_bytes.clone()),
                    rewrite_bundle,
                )
                .await
                {
                    Some(r) => r,
                    None => return (StatusCode::BAD_GATEWAY, "dsh 连接失败").into_response(),
                };
            }
        }
    }
    // 插件 bundle：缓冲改写内测声明持久化三元式（仅 identity + 体积上限内）；
    // 改写产物在代理侧补回 gzip（为改写向 dsh 求的是 identity，浏览器实际收压缩流）
    if rewrite_bundle
        && res.status().is_success()
        && res.headers().get(header::CONTENT_ENCODING).is_none()
        && res.content_length().map_or(true, |n| n <= REWRITE_BUFFER_LIMIT)
    {
        return rewrite_plugin_bundle(res, client_gzip).await;
    }
    // HTML 文档（dsh 对所有路径回同一 SPA 入口）：缓冲注入移动端适配样式
    if wants_html
        && res.status().is_success()
        && is_html_document(res.headers())
        && res.headers().get(header::CONTENT_ENCODING).is_none()
        && res.content_length().map_or(true, |n| n <= REWRITE_BUFFER_LIMIT)
    {
        return rewrite_html_document(res).await;
    }
    // 透传路径的代理侧 gzip：dsh 不压缩任何响应，远程首连 ~5MB 文本资产全走
    // identity（0.5.5 手机实拍白屏几十秒的根因之一）。仅 GET 成功响应 + 客户端
    // 宣告接受 + 无 Range + 文本类型 + 未编码 + 已知长度在门槛内才缓冲压缩；
    // 压缩失败/读取失败回退 identity 缓冲体或 502，绝不发半包
    if client_gzip
        && method == reqwest::Method::GET
        && !has_range
        && res.status() == StatusCode::OK
        && res.headers().get(header::CONTENT_ENCODING).is_none()
        && gzip_eligible_content_type(res.headers())
        && res
            .content_length()
            .is_some_and(|n| (GZIP_MIN_SIZE..=REWRITE_BUFFER_LIMIT).contains(&n))
    {
        let builder = buffered_builder(&res);
        return match res.bytes().await {
            Ok(bytes) => match gzip_bytes(bytes.to_vec()).await {
                Some(gz) => builder
                    .header(header::CONTENT_ENCODING, "gzip")
                    .header(header::VARY, "accept-encoding")
                    .body(Body::from(gz))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                // 压缩失败：identity 缓冲体兜底（客户端没收到压缩承诺，语义一致）
                None => builder
                    .body(Body::from(bytes))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
            },
            Err(_) => (StatusCode::BAD_GATEWAY, "dsh 连接失败").into_response(),
        };
    }
    let mut builder = Response::builder().status(res.status());
    for (name, value) in res.headers() {
        if matches!(
            name.as_str(),
            "connection" | "transfer-encoding" | "keep-alive" | "upgrade"
        ) {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from_stream(res.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// 流式上传转发（dsh 0.1.5 起 `POST /api/session/uploadFileBinary` 走
/// `requestBody: "streaming"`，dsh 对它不限体积）。与 forward() 的两点根本差异：
///   1. 请求体逐块透传、不整读缓冲——手机大文件上传不再撞 REPLAY_BODY_LIMIT，
///      壳进程也不再囤积至多 64MiB；
///   2. 体一旦被消费就无法重放，故不做 401 重放，改为上传前强制换一次 cookie：
///      一次廉价的回环 cookie 交换，抵掉整个用户文件因 cookie 轮换而丢失的风险。
///      换不到 cookie（creds 未就绪/交换失败）仍照常转发，dsh 的 401 原样透传——
///      绝不把它误报成 502。
/// rewrite_bundle=false：上传路由不是插件 bundle 形态，无需 identity 改写。
async fn forward_streaming(st: ProxyState, req: Request, path_and_query: &str) -> Response {
    let Some(c) = st.creds.borrow().clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "dsh 未就绪").into_response();
    };
    let url = format!("http://127.0.0.1:{}{}", c.port, path_and_query);
    // 消费请求体前取方法/头副本；cookie 主动换新（不缓存旧值赌 401）
    let method = req.method().clone();
    let req_headers = req.headers().clone();
    let cookie = ensure_cookie(&st, true).await;
    let body = reqwest::Body::wrap_stream(req.into_body().into_data_stream());
    let res = send_forwarded(
        &st,
        &req_headers,
        &method,
        &url,
        cookie.as_deref(),
        body,
        false,
    )
    .await;
    let res = match res {
        Some(r) => r,
        // 传输层失败（dsh 拒连/中途断链）：502，与 forward() 一致
        None => return (StatusCode::BAD_GATEWAY, "dsh 连接失败").into_response(),
    };
    // 响应照 forward() 尾部逐块回传：复制状态+头、剥逐跳头、体走 bytes_stream。
    // 响应体流若中途出错，hyper 会中止该响应——客户端得到断连而非貌似完整的
    // 截断 200（状态行发出后无法再改 502，这是 HTTP 语义边界；本路由响应体是
    // dsh 回的小 JSON，实际不会走到）。绝不记录 token/cookie 值。
    let mut builder = Response::builder().status(res.status());
    for (name, value) in res.headers() {
        if matches!(
            name.as_str(),
            "connection" | "transfer-encoding" | "keep-alive" | "upgrade"
        ) {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from_stream(res.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// 按剥头规则构造并发出转发请求（cookie 由调用方注入；body 由调用方决定整读或
/// 流式——缓冲体交给 401 重放，流式体不可重放见 forward_streaming）。
/// rewrite_bundle 由调用方按 path_and_query 判定传入——本函数拿到的 url 带
/// scheme://host 前缀，就地重算会恒为 false（曾致 accept-encoding 不剥、真 dsh
/// 压缩响应后改写路径整体失效）
async fn send_forwarded(
    st: &ProxyState,
    headers: &HeaderMap,
    method: &reqwest::Method,
    url: &str,
    cookie: Option<&str>,
    body: reqwest::Body,
    rewrite_bundle: bool,
) -> Option<reqwest::Response> {
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    let mut out = st.client.request(method.clone(), url);
    for (name, value) in headers {
        // host/content-length 由 reqwest 按目标与 body 重算；逐跳头不透传。
        // origin/referer/sec-fetch-* 必须剥掉：dsh 有浏览器信任栅栏
        // （dsh-client-connection isTrustedApiRequest），Origin.host ≠ Host 头
        // 或 sec-fetch-site: cross-site 的 /api 请求一律 403。经隧道访问时浏览器
        // 带的是 trycloudflare 域名的 Origin，不剥则页面所有 RPC 调用全灭。
        // 剥掉后请求在 dsh 眼里是无 Origin 的 loopback 客户端，合法放行。
        if matches!(
            name.as_str(),
            "host"
                | "connection"
                | "content-length"
                | "transfer-encoding"
                | "upgrade"
                | "keep-alive"
                | "te"
                | "trailer"
                | "proxy-authorization"
                | "origin"
                | "referer"
                | "sec-fetch-site"
                | "sec-fetch-mode"
                | "sec-fetch-dest"
                | "sec-fetch-user"
                // 代持 cookie 只能由本函数注入：转发浏览器自带的同名头会顶掉
                // dsh-auth cookie（0.1.2 起手机端必须靠代理代持的 cookie 过门）
                | "cookie"
        ) {
            continue;
        }
        // 改写路径要求 identity 原文：剥 accept-encoding 防压缩，剥条件请求头防 304
        if (rewrite_bundle || wants_html)
            && matches!(
                name.as_str(),
                "accept-encoding" | "if-none-match" | "if-modified-since"
            )
        {
            continue;
        }
        out = out.header(name, value);
    }
    if let Some(c) = cookie {
        out = out.header(header::COOKIE, dsh_session::cookie_header(c));
    }
    out.body(body).send().await.ok()
}

/// 缓冲插件 bundle 响应并改写内测声明三元式。content-length/etag 作废（改写后长度与
/// 内容都变）；needle 不存在时原样返回（dsh 改版换了写法就静默失效，声明照弹但不破坏页面）。
/// client_gzip（浏览器宣告接受 gzip）且产物越过 GZIP_MIN_SIZE 时在代理侧补回
/// gzip——改写路径向 dsh 求的是 identity（剥了 accept-encoding），不补则 3.7MB
/// bundle 在隧道里裸奔（0.5.5 手机实拍首连白屏几十秒）
async fn rewrite_plugin_bundle(res: reqwest::Response, client_gzip: bool) -> Response {
    let builder = buffered_builder(&res);
    match res.bytes().await {
        Ok(bytes) if bytes.len() as u64 <= REWRITE_BUFFER_LIMIT => {
            let body = replace_all(&bytes, crate::upstream::WELCOME_NOTICE_NEEDLE, WELCOME_NOTICE_REPL)
                .unwrap_or_else(|| bytes.to_vec());
            if client_gzip && body.len() as u64 >= GZIP_MIN_SIZE {
                // 克隆一份进压缩线程：压缩失败时原件还要回退 identity 服务
                if let Some(gz) = gzip_bytes(body.clone()).await {
                    return builder
                        .header(header::CONTENT_ENCODING, "gzip")
                        .header(header::VARY, "accept-encoding")
                        .body(Body::from(gz))
                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
                }
            }
            builder
                .body(Body::from(body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        // 读取失败或体积超限：改不了，502/原样都比半包强
        Ok(bytes) => builder
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => (StatusCode::BAD_GATEWAY, "dsh 连接失败").into_response(),
    }
}

fn is_html_document(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"))
}

/// 缓冲改写响应的公共头部构造：逐跳头之外再剥 content-length/etag（缓冲改写后
/// 长度与摘要都作废）与 accept-ranges（变换后的体无法按原字节区间服务）
fn buffered_builder(res: &reqwest::Response) -> axum::http::response::Builder {
    let mut builder = Response::builder().status(res.status());
    for (name, value) in res.headers() {
        if matches!(
            name.as_str(),
            "connection" | "transfer-encoding" | "keep-alive" | "upgrade" | "content-length" | "etag" | "accept-ranges"
        ) {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
}

/// 缓冲 HTML 文档并往 </head> 前注入移动端适配样式与信息标签页脚本；找不到
/// </head> 原样返回（dsh 改版换了文档结构就静默失效，页面回到未适配状态但
/// 不破坏功能）。注入前先把 viewport meta 改写为禁缩放形态——iOS WKWebView
/// 聚焦 font-size<16px 的输入框（composer 实测 14px）自动放大整页且收键盘
/// 不复原，标签栏/输入框被推出可视区（0.5.4 手机实拍实踩）；needle 未命中
/// 原样透传，上游改版由契约探针翻红。
/// 加载过渡页（splash）：React 挂载点 needle（SPA_ROOT_MOUNT_NEEDLE）命中时，
/// 挂载点后注入 splash 覆盖层+卸载脚本、head 注入补 splash 样式；needle 漂移
/// 则 splash 整体不注入（回到白屏等待，功能不损），契约探针守门。
async fn rewrite_html_document(res: reqwest::Response) -> Response {
    let builder = buffered_builder(&res);
    match res.bytes().await {
        Ok(bytes) if bytes.len() as u64 <= REWRITE_BUFFER_LIMIT => {
            let doc = replace_all(
                &bytes,
                crate::upstream::VIEWPORT_META_NEEDLE,
                crate::upstream::VIEWPORT_META_REPLACEMENT,
            )
            .unwrap_or_else(|| bytes.to_vec());
            let (doc, splash) = match replace_all(
                &doc,
                crate::upstream::SPA_ROOT_MOUNT_NEEDLE,
                &splash_replacement(),
            ) {
                Some(doc) => (doc, true),
                None => (doc, false),
            };
            let body = match find_subslice_ci(&doc, b"</head>") {
                Some(pos) => {
                    let mut out = Vec::with_capacity(
                        doc.len() + MOBILE_CSS.len() + MOBILE_JS.len() + SPLASH_CSS.len() + 96,
                    );
                    out.extend_from_slice(&doc[..pos]);
                    out.extend_from_slice(MOBILE_INJECT_MARKER.as_bytes());
                    out.extend_from_slice(b"<style>");
                    out.extend_from_slice(MOBILE_CSS.as_bytes());
                    if splash {
                        out.extend_from_slice(b"\n");
                        out.extend_from_slice(SPLASH_CSS.as_bytes());
                    }
                    out.extend_from_slice(b"</style><script>");
                    out.extend_from_slice(MOBILE_JS.as_bytes());
                    out.extend_from_slice(b"</script>");
                    out.extend_from_slice(&doc[pos..]);
                    out
                }
                None => doc,
            };
            builder
                .body(Body::from(body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Ok(bytes) => builder
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => (StatusCode::BAD_GATEWAY, "dsh 连接失败").into_response(),
    }
}

/// ASCII 大小写不敏感的子串查找
fn find_subslice_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

/// 字节级全量替换；needle 一次都没命中时返回 None（调用方原样透传）
fn replace_all(haystack: &[u8], needle: &[u8], repl: &[u8]) -> Option<Vec<u8>> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut found = false;
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            out.extend_from_slice(repl);
            i += needle.len();
            found = true;
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out.extend_from_slice(&haystack[i..]);
    found.then_some(out)
}

/// WS 升级请求的握手在代理处终结（鉴权即门岗），与 dsh 另建 WS 后逐帧双向搬运；
/// 任一方向断开即整体断开，dsh 端口在桥接时重新读取（dsh 重启换端口不影响）
async fn bridge_upgrade(st: ProxyState, req: Request, path_and_query: String) -> Response {
    let ws = match WebSocketUpgrade::from_request(req, &st).await {
        Ok(ws) => ws,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid websocket upgrade").into_response(),
    };
    ws.on_upgrade(move |client| bridge(client, st, path_and_query))
}

async fn bridge(client: WebSocket, st: ProxyState, path_and_query: String) {
    // 提前注册 drain 等待（enable 即在 Notify 上挂号）：重置/停服若在
    // 下面的 connect 期间触发，select 开始时仍能立刻收到，桥接不漏掐
    let drained = st.drain.notified();
    tokio::pin!(drained);
    drained.as_mut().enable();

    let Some(c) = st.creds.borrow().clone() else {
        return;
    };
    let url = format!("ws://127.0.0.1:{}{}", c.port, path_and_query);
    // 0.1.2 起 dsh 侧 WS 升级也须带 dsh-auth cookie（BrowserAuth 门内）——
    // 漏注入手机端表现为"页面开但全断"（prep §八.3）。cookie 现换不缓存
    // （桥接生命周期长，dsh 重启换端口后旧桥已由 drain/断线终结，新桥用新值）
    let cookie = ensure_cookie(&st, false).await;
    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(_) => return,
    };
    if let Some(c) = &cookie {
        if let Ok(v) = HeaderValue::from_str(&dsh_session::cookie_header(c)) {
            req.headers_mut().insert("cookie", v);
        }
    }
    let Ok((dsh, _resp)) = tokio_tungstenite::connect_async(req).await else {
        return;
    };
    let (mut client_tx, mut client_rx) = client.split();
    let (mut dsh_tx, mut dsh_rx) = dsh.split();
    let up = async {
        while let Some(Ok(msg)) = client_rx.next().await {
            if dsh_tx.send(to_dsh(msg)).await.is_err() {
                break;
            }
        }
    };
    let down = async {
        while let Some(Ok(msg)) = dsh_rx.next().await {
            let Some(msg) = to_client(msg) else {
                continue;
            };
            if client_tx.send(msg).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
        // 重置链接/停服：掐断桥接，两端 socket 随 future 析构关闭
        _ = drained => {}
    }
    let _ = client_tx.send(Message::Close(None)).await;
    let _ = dsh_tx.send(DshMessage::Close(None)).await;
}

fn wants_websocket(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// axum 与 tungstenite 各自定义 Message/Utf8Bytes 类型，按帧内容互转
fn to_dsh(msg: Message) -> DshMessage {
    match msg {
        Message::Text(t) => DshMessage::Text(t.as_str().to_owned().into()),
        Message::Binary(b) => DshMessage::Binary(b),
        Message::Ping(p) => DshMessage::Ping(p),
        Message::Pong(p) => DshMessage::Pong(p),
        Message::Close(_) => DshMessage::Close(None),
    }
}

fn to_client(msg: DshMessage) -> Option<Message> {
    let msg = match msg {
        DshMessage::Text(t) => Message::Text(t.as_str().to_owned().into()),
        DshMessage::Binary(b) => Message::Binary(b),
        DshMessage::Ping(p) => Message::Ping(p),
        DshMessage::Pong(p) => Message::Pong(p),
        DshMessage::Close(_) => Message::Close(None),
        // 原始帧不经 WebSocketStream 读出，仅写入侧存在；无对应 axum 类型，跳过
        DshMessage::Frame(_) => return None,
    };
    Some(msg)
}

fn gate() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        GATE_HTML,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, uri: &str, upgrade_ws: bool) -> Request {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "127.0.0.1:1");
        if upgrade_ws {
            b = b
                .header(header::CONNECTION, "Upgrade")
                .header(header::UPGRADE, "websocket");
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn bundle_matcher_covers_single_and_combo_forms() {
        // 单插件（0.1.1 及以前）
        assert!(is_plugin_client_bundle("/plugins/fake/client.js"));
        assert!(is_plugin_client_bundle("/plugins/fake/client.js?rev=1"));
        // 合并加载（0.1.2 起）：path 部分只剩 "/plugins/"，清单在 query 里
        assert!(is_plugin_client_bundle(
            "/plugins/??@deepseek-ai/a/client.js,@deepseek-ai/b/client.js&rev=b6deae2120c2"
        ));
        // 非 bundle：其余资产与纯根路径
        assert!(!is_plugin_client_bundle("/plugins/fake/icon.png"));
        assert!(!is_plugin_client_bundle("/plugins/events"));
        assert!(!is_plugin_client_bundle("/plugins/"));
    }

    #[test]
    fn strip_cache_bust_removes_only_buster() {
        assert_eq!(
            strip_cache_bust("/plugins/??@a/client.js,@b/client.js&rev=abc&dshv=0.5.4"),
            "/plugins/??@a/client.js,@b/client.js&rev=abc"
        );
        assert_eq!(
            strip_cache_bust("/plugins/fake/client.js?rev=1&dshv=0.5.4"),
            "/plugins/fake/client.js?rev=1"
        );
        // 无 query / 无该参数：原样
        assert_eq!(strip_cache_bust("/plugins/fake/client.js"), "/plugins/fake/client.js");
        assert_eq!(
            strip_cache_bust("/plugins/fake/client.js?rev=1"),
            "/plugins/fake/client.js?rev=1"
        );
    }

    #[test]
    fn cache_bust_only_for_get_bundle_without_buster() {
        let combo = "/plugins/??@a/client.js,@b/client.js&rev=abc";
        assert!(wants_cache_bust(&req("GET", combo, false), combo));
        // HEAD 也击穿；POST 不动
        assert!(wants_cache_bust(&req("HEAD", combo, false), combo));
        assert!(!wants_cache_bust(&req("POST", combo, false), combo));
        // 带 buster：不再 302
        let busted = format!("{combo}&dshv=0.5.4");
        assert!(!wants_cache_bust(&req("GET", &busted, false), &busted));
        // WS 升级永不重定向
        assert!(!wants_cache_bust(&req("GET", combo, true), combo));
        // 非 bundle 资源不击穿
        assert!(!wants_cache_bust(
            &req("GET", "/plugins/fake/icon.png", false),
            "/plugins/fake/icon.png"
        ));
    }

    #[test]
    fn client_accepts_gzip_parses_tokens() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert(header::ACCEPT_ENCODING, v.parse().unwrap());
            m
        };
        assert!(client_accepts_gzip(&h("gzip, br")));
        assert!(client_accepts_gzip(&h("br, gzip")));
        assert!(client_accepts_gzip(&h("gzip")));
        // 子串误伤："xgzip" 不含独立 gzip token
        assert!(!client_accepts_gzip(&h("xgzip, br")));
        assert!(!client_accepts_gzip(&h("br")));
        assert!(!client_accepts_gzip(&HeaderMap::new()));
    }

    #[test]
    fn gzip_content_type_allowlist() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert(header::CONTENT_TYPE, v.parse().unwrap());
            m
        };
        assert!(gzip_eligible_content_type(&h("text/javascript")));
        assert!(gzip_eligible_content_type(&h("application/javascript; charset=utf-8")));
        assert!(gzip_eligible_content_type(&h("text/css; charset=utf-8")));
        assert!(gzip_eligible_content_type(&h("application/json")));
        assert!(gzip_eligible_content_type(&h("image/svg+xml")));
        assert!(gzip_eligible_content_type(&h("text/html; charset=utf-8")));
        // 自带压缩/二进制格式与缺失头不压
        assert!(!gzip_eligible_content_type(&h("image/png")));
        assert!(!gzip_eligible_content_type(&h("font/woff2")));
        assert!(!gzip_eligible_content_type(&h("application/octet-stream")));
        assert!(!gzip_eligible_content_type(&HeaderMap::new()));
    }

    #[test]
    fn gzip_compress_round_trip() {
        use flate2::read::GzDecoder;
        use std::io::Read;
        let body: Vec<u8> = (0..5000)
            .map(|i| format!("export const v{i} = {i};\n"))
            .collect::<String>()
            .into_bytes();
        let gz = gzip_compress(body.clone()).expect("压缩应成功");
        assert!(gz.len() < body.len() / 3, "重复文本应显著压缩");
        // gzip magic
        assert_eq!(&gz[..2], &[0x1f, 0x8b]);
        let mut out = Vec::new();
        GzDecoder::new(&gz[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, body, "解压应还原原文");
    }

    #[test]
    fn splash_replacement_keeps_mount_and_carries_assets() {
        let repl = splash_replacement();
        let s = String::from_utf8(repl).unwrap();
        assert!(s.starts_with(r#"<div id="root"></div>"#), "挂载点原样保留：{s}");
        assert!(s.contains(r#"<div id="dsh-splash""#), "含 splash 覆盖层：{s}");
        assert!(s.contains("<script>"), "含卸载脚本：{s}");
        assert!(s.contains("dsh-splash-done"), "脚本含淡出类名：{s}");
    }
}

fn cookie_authed(headers: &HeaderMap, token: &str) -> bool {
    let prefix = format!("{COOKIE_NAME}=");
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';').map(str::trim))
        .filter_map(|pair| pair.strip_prefix(prefix.as_str()))
        .any(|v| token_eq(v, token))
}

/// 从 path?query 中去掉 token 参数，其余参数原样保留
fn strip_token_query(path_and_query: &str) -> String {
    let Some((path, query)) = path_and_query.split_once('?') else {
        return path_and_query.to_string();
    };
    let kept: Vec<String> = url::form_urlencoded::parse(query.as_bytes())
        .filter(|(k, _)| k != "token")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if kept.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}
