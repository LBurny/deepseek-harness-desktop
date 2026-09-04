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
/// 错误 token 的固定响应延迟，拖慢在线猜测
const WRONG_TOKEN_DELAY: Duration = Duration::from_millis(500);
/// 请求体缓冲上限：重放（401 换 cookie 后重发一次）需要整读请求体；超过该
/// 体积的上传类请求不重放（cookie 失效表现为一次 401，刷新页面即恢复）
const REPLAY_BODY_LIMIT: usize = 64 * 1024 * 1024;

const GATE_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>DSHDesktop</title></head>\
<body style=\"font-family:sans-serif;display:flex;justify-content:center;align-items:center;height:100vh;margin:0\">\
<p>DSHDesktop 远程访问：链接无效或已过期。<br>请在电脑托盘菜单重新生成链接。</p></body></html>";

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
                        format!("{COOKIE_NAME}={t}; HttpOnly; Secure; SameSite=Lax; Path=/"),
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
    let Some(c) = st.creds.borrow().clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "dsh 未就绪").into_response();
    };
    let url = format!("http://127.0.0.1:{}{}", c.port, path_and_query);
    let rewrite_bundle = is_plugin_client_bundle(path_and_query);
    let wants_html = wants_html_document(req.headers());
    let method = req.method().clone();
    // 重放（401 换 cookie 后重发一次）需要整读请求体与请求头副本（into_body 消费 req）
    let req_headers = req.headers().clone();
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
        &body_bytes,
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
                    &body_bytes,
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
    // 插件 bundle：缓冲改写内测声明持久化三元式（仅 identity + 体积上限内）
    if rewrite_bundle
        && res.status().is_success()
        && res.headers().get(header::CONTENT_ENCODING).is_none()
        && res.content_length().map_or(true, |n| n <= REWRITE_BUFFER_LIMIT)
    {
        return rewrite_plugin_bundle(res).await;
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

/// 按剥头规则构造并发出转发请求（cookie 由调用方注入；body 整读以便 401 重放）。
/// rewrite_bundle 由调用方按 path_and_query 判定传入——本函数拿到的 url 带
/// scheme://host 前缀，就地重算会恒为 false（曾致 accept-encoding 不剥、真 dsh
/// 压缩响应后改写路径整体失效）
async fn send_forwarded(
    st: &ProxyState,
    headers: &HeaderMap,
    method: &reqwest::Method,
    url: &str,
    cookie: Option<&str>,
    body_bytes: &[u8],
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
    out.body(reqwest::Body::from(body_bytes.to_vec()))
        .send()
        .await
        .ok()
}

/// 缓冲插件 bundle 响应并改写内测声明三元式。content-length/etag 作废（改写后长度与
/// 内容都变）；needle 不存在时原样返回（dsh 改版换了写法就静默失效，声明照弹但不破坏页面）。
async fn rewrite_plugin_bundle(res: reqwest::Response) -> Response {
    let builder = buffered_builder(&res);
    match res.bytes().await {
        Ok(bytes) if bytes.len() as u64 <= REWRITE_BUFFER_LIMIT => {
            let body = replace_all(&bytes, crate::upstream::WELCOME_NOTICE_NEEDLE, WELCOME_NOTICE_REPL)
                .unwrap_or_else(|| bytes.to_vec());
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
/// 长度与摘要都作废）
fn buffered_builder(res: &reqwest::Response) -> axum::http::response::Builder {
    let mut builder = Response::builder().status(res.status());
    for (name, value) in res.headers() {
        if matches!(
            name.as_str(),
            "connection" | "transfer-encoding" | "keep-alive" | "upgrade" | "content-length" | "etag"
        ) {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
}

/// 缓冲 HTML 文档并往 </head> 前注入移动端适配样式与信息标签页脚本；找不到
/// </head> 原样返回（dsh 改版换了文档结构就静默失效，页面回到未适配状态但
/// 不破坏功能）。
async fn rewrite_html_document(res: reqwest::Response) -> Response {
    let builder = buffered_builder(&res);
    match res.bytes().await {
        Ok(bytes) if bytes.len() as u64 <= REWRITE_BUFFER_LIMIT => {
            let body = match find_subslice_ci(&bytes, b"</head>") {
                Some(pos) => {
                    let mut out =
                        Vec::with_capacity(bytes.len() + MOBILE_CSS.len() + MOBILE_JS.len() + 96);
                    out.extend_from_slice(&bytes[..pos]);
                    out.extend_from_slice(MOBILE_INJECT_MARKER.as_bytes());
                    out.extend_from_slice(b"<style>");
                    out.extend_from_slice(MOBILE_CSS.as_bytes());
                    out.extend_from_slice(b"</style><script>");
                    out.extend_from_slice(MOBILE_JS.as_bytes());
                    out.extend_from_slice(b"</script>");
                    out.extend_from_slice(&bytes[pos..]);
                    out
                }
                None => bytes.to_vec(),
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

    fn pq(r: &Request) -> String {
        r.uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap()
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
