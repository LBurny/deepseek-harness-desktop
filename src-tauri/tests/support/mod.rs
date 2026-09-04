#![allow(dead_code)]
//! 测试专用假 dsh 服务器（进程内 axum，0.1.2 BrowserAuth + remote.mux 全仿真）。
//! 行为对应 docs/upstream-0.1.2-alpha.1-prep.zh-CN.md §二（鉴权）/§三（mux 传输）：
//!   - GET / 无凭证 401；?token= 交换 303 + Set-Cookie dsh-auth-*
//!   - 带 cookie 的页面/插件 bundle 200（</head> 注入点、内测声明 needle 供改写测试）
//!   - /api/* 无 cookie 401、错 Origin 403（栅栏仿真）、POST 信封 200 server-response
//!   - GET /api/remote.mux WS：无 cookie 401；带 cookie 接受，open $events 先推 ready，
//!     scripted 通道的 JSON 逐条以 item 包裹推给所有已 open 的流（$events 与
//!     session/follow 同源，测试按 value 形状自行区分）

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, Notify, RwLock};
use tokio_tungstenite::tungstenite::Message as WsMessage;

pub const FIXTURE_TOKEN: &str = "fixture-token-0123456789abcdef";

/// 脚本化下行：测试持发送端注入 emit/waterfall/snapshot/event 帧，
/// 服务器（ScriptedFrames 里的接收端）逐条广播给所有 open 的流
pub struct ScriptedFrames(pub mpsc::UnboundedReceiver<Value>);

pub struct FakeDsh {
    pub port: u16,
    pub shutdown: Arc<Notify>,
    /// 客户端 open 记录（"$events" / "session/follow:<sessionId>"），供测试断言
    pub opens: Arc<Mutex<Vec<String>>>,
    /// /api 命中记录 (路径, 是否携带 dsh-auth cookie)，供代理转发断言
    pub api_hits: Arc<Mutex<Vec<(String, bool)>>>,
    /// 插件 bundle 命中记录（原始 path+query），供"缓存击穿参数须剥掉再转发"断言
    pub plugin_hits: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone)]
struct FakeState {
    /// scripted 帧广播端（upgrade 后每连接订阅一份，多连接互不抢帧）
    scripted_tx: broadcast::Sender<Value>,
    /// 假 cookie 名（存在性检查用，不验签——dsh_session 常量保证前缀契约一致）
    cookie_name: Arc<str>,
    /// open 帧记录（跨连接共享）
    opens: Arc<Mutex<Vec<String>>>,
    /// /api 命中记录 (路径, 是否携带 dsh-auth cookie)，供代理转发断言
    api_hits: Arc<Mutex<Vec<(String, bool)>>>,
    /// 插件 bundle 命中记录（原始 path+query）
    plugin_hits: Arc<Mutex<Vec<String>>>,
}

/// cookie 名固定假形态（真值 = dsh-auth-<base64url(sha256(authority))>，测试只认前缀）
fn fake_cookie_name(port: u16) -> String {
    format!(
        "{}fake{:04x}",
        dshdesktop_lib::upstream::DSH_AUTH_COOKIE_PREFIX,
        port
    )
}

/// 测试侧拼 Cookie 头用
pub fn cookie_name_for(port: u16) -> String {
    fake_cookie_name(port)
}

fn has_auth_cookie(headers: &HeaderMap, name: &str) -> bool {
    let Some(raw) = headers.get(axum::http::header::COOKIE) else {
        return false;
    };
    let Ok(s) = raw.to_str() else { return false };
    s.split(';').any(|pair| {
        let pair = pair.trim();
        match pair.split_once('=') {
            Some((n, v)) => n == name && !v.is_empty(),
            None => false,
        }
    })
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "dsh web authentication required").into_response()
}

/// 浏览器信任栅栏仿真（dsh-client-connection isTrustedApiRequest）：Origin 存在
/// 且 host 与 Host 头不一致 → 403。无 Origin（非浏览器客户端）放行。
fn fence_reject(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return false;
    };
    let Ok(origin) = origin.to_str() else { return true };
    let Some(host) = headers.get(axum::http::header::HOST) else { return true };
    match (url::Url::parse(origin), host.to_str()) {
        (Ok(u), Ok(h)) => {
            let origin_host = u.host_str().unwrap_or("");
            let authority = match u.port_or_known_default() {
                Some(p) => format!("{origin_host}:{p}"),
                None => origin_host.to_string(),
            };
            origin_host != h && authority != h
        }
        _ => true,
    }
}

async fn page(State(st): State<FakeState>, headers: HeaderMap, RawQuery(q): RawQuery) -> Response {
    st.api_hits.lock().unwrap().push(("/".to_string(), has_auth_cookie(&headers, &st.cookie_name)));
    let is_query = q
        .as_deref()
        .unwrap_or("")
        .split('&')
        .any(|kv| kv == format!("token={FIXTURE_TOKEN}"));
    if is_query {
        // token 交换：303 + Set-Cookie（值固定假签名，测试只验头形与后续携带）
        let cookie = format!(
            "{}=v1.fake.sig; Path=/; HttpOnly; SameSite=Strict",
            st.cookie_name
        );
        return (
            StatusCode::SEE_OTHER,
            [("location", HeaderValue::from_static("/"))],
            [("set-cookie", HeaderValue::from_str(&cookie).unwrap())],
        )
            .into_response();
    }
    if !has_auth_cookie(&headers, &st.cookie_name) {
        return unauthorized();
    }
    (
        StatusCode::OK,
        [(
            "content-type",
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        "<!doctype html><html><head><title>fake dsh</title></head><body><div id=\"root\"></div></body></html>",
    )
        .into_response()
}

async fn plugin_client(
    State(st): State<FakeState>,
    uri: axum::http::Uri,
) -> Response {
    // 记录原始 path+query：测试断言代理转发时已剥掉壳侧缓存击穿参数（dsh 对
    // 组合 URL 的 query 逐字校验，多余参数 404——0.5.4 真机实测）
    st.plugin_hits.lock().unwrap().push(uri.to_string());
    // 静态资产无鉴权门（真实 dsh /assets/* 无门）；0.1.2 needle 形态供代理改写测试
    (
        StatusCode::OK,
        [(
            "content-type",
            HeaderValue::from_static("application/javascript; charset=utf-8"),
        )],
        "const w = new WelcomeNoticeStore(ctx.remote.$host, ctx.remote.$host.isLoopback ? \"host\" : \"memory\");\n",
    )
        .into_response()
}

async fn app_page() -> Response {
    // SPA 入口文档模拟（代理移动端注入测试）：含 </head> 与 dsh 同款 viewport meta
    // （iOS 自动缩放防线改写对象，值对齐 upstream::VIEWPORT_META_NEEDLE）
    (
        StatusCode::OK,
        [(
            "content-type",
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>fake</title></head><body><div id=\"root\"></div></body></html>",
    )
        .into_response()
}

async fn app_page_nohead() -> Response {
    // 无 </head> 的 HTML：注入应静默跳过、原文透传
    (
        StatusCode::OK,
        [(
            "content-type",
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        "<!doctype html><html><body>no head</body></html>",
    )
        .into_response()
}

async fn api(
    State(st): State<FakeState>,
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let path = format!("/api/{path}");
    let has_cookie = has_auth_cookie(&headers, &st.cookie_name);
    st.api_hits.lock().unwrap().push((path.clone(), has_cookie));
    if !has_auth_cookie(&headers, &st.cookie_name) {
        return unauthorized();
    }
    if fence_reject(&headers) {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    // RPC 信封校验后恒 200 server-response（result.ok 恒真；value 空对象）
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let rpc_id = v.get("rpcId").cloned().unwrap_or(json!("unknown"));
    (
        StatusCode::OK,
        [(
            "content-type",
            HeaderValue::from_static("application/json"),
        )],
        json!({"type":"server-response","rpcId":rpc_id,"result":{"ok":true,"value":{}}}).to_string(),
    )
        .into_response()
}

/// streamId → endpoint（$events / session/follow）
type Streams = Arc<RwLock<HashMap<String, String>>>;

async fn ws_mux(
    State(st): State<FakeState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !has_auth_cookie(&headers, &st.cookie_name) {
        return unauthorized();
    }
    upgrade.on_upgrade(move |socket| mux_socket(st, socket))
}

async fn mux_socket(st: FakeState, socket: WebSocket) {
    let (mut tx, mut rx) = socket.split();
    let streams: Streams = Arc::new(RwLock::new(HashMap::new()));
    let mut scripted = st.scripted_tx.subscribe();
    loop {
        tokio::select! {
            // 客户端帧：open/cancel/close
            msg = rx.next() => {
                let Some(Ok(Message::Text(text))) = msg else { break };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("open") => {
                        let (Some(id), Some(endpoint)) = (
                            v.get("streamId").and_then(|s| s.as_str()).map(String::from),
                            v.get("endpoint").and_then(|e| e.as_str()).map(String::from),
                        ) else { continue };
                        if endpoint == dshdesktop_lib::upstream::EVENT_STREAM_ENDPOINT {
                            // $events：连接即推 ready（真 dsh 契约，notify/mux.rs 靠它判连通）
                            let ready = json!({"type":"item","streamId":id,"value":{
                                "type":"ready","clientId":"fake",
                                "host":{"home":"C:\\\\fake","isLoopback":true}}});
                            if tx.send(Message::Text(ready.to_string().into())).await.is_err() {
                                break;
                            }
                            st.opens.lock().unwrap().push(endpoint.clone());
                        } else if endpoint == dshdesktop_lib::upstream::METHOD_SESSION_FOLLOW {
                            let sid = v
                                .pointer("/payload/args/request/address/sessionId")
                                .and_then(|s| s.as_str())
                                .unwrap_or("?")
                                .to_string();
                            st.opens.lock().unwrap().push(format!("{endpoint}:{sid}"));
                        }
                        streams.write().await.insert(id, endpoint);
                    }
                    Some("cancel") => {
                        if let Some(id) = v.get("streamId").and_then(|s| s.as_str()) {
                            streams.write().await.remove(id);
                        }
                    }
                    _ => {}
                }
            }
            // scripted 帧：广播给所有 open 流（item 包裹）
            frame = scripted.recv() => {
                let Ok(v) = frame else { break };
                let ids: Vec<String> = streams.read().await.keys().cloned().collect();
                for id in ids {
                    let item = json!({"type":"item","streamId":id,"value":v});
                    if tx.send(Message::Text(item.to_string().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

pub async fn spawn_fake_dsh(port: u16, scripted: ScriptedFrames) -> FakeDsh {
    let (scripted_tx, _) = broadcast::channel::<Value>(64);
    // 中继：单接收端（mpsc）→ 广播（多连接各订阅一份）
    let st_tx = scripted_tx.clone();
    tokio::spawn(async move {
        let mut rx = scripted.0;
        while let Some(v) = rx.recv().await {
            if st_tx.send(v).is_err() {
                break;
            }
        }
    });
    let shutdown = Arc::new(Notify::new());
    let opens = Arc::new(Mutex::new(Vec::new()));
    let api_hits = Arc::new(Mutex::new(Vec::new()));
    let plugin_hits = Arc::new(Mutex::new(Vec::new()));
    let state = FakeState {
        scripted_tx,
        cookie_name: Arc::from(fake_cookie_name(port)),
        opens: opens.clone(),
        api_hits: api_hits.clone(),
        plugin_hits: plugin_hits.clone(),
    };
    let app = Router::new()
        .route("/", axum::routing::get(page))
        .route("/app", axum::routing::get(app_page))
        .route("/app-nohead", axum::routing::get(app_page_nohead))
        .route("/plugins/fake/client.js", axum::routing::get(plugin_client))
        // 0.1.2 合并加载形态：/plugins/??<a>/client.js,<b>/client.js&rev=N 的
        // path 部分只剩 "/plugins/"，组合清单整体在 query 里（真机实测）
        .route("/plugins/", axum::routing::get(plugin_client))
        .route("/api/{*rest}", axum::routing::any(api))
        .route(
            dshdesktop_lib::upstream::DSH_MUX_PATH,
            axum::routing::get(ws_mux),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .expect("bind fake dsh");
    let stop = shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { stop.notified().await })
            .await
            .ok();
    });
    FakeDsh {
        port,
        shutdown,
        opens,
        api_hits,
        plugin_hits,
    }
}

type WsHalves = (
    futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
);

/// 测试侧便捷客户端：带假 cookie 连 remote.mux（握手重试至 5s，防挂载竞态）
pub async fn connect_mux(port: u16, cookie_name: &str) -> Result<WsSplit, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let url = format!(
        "ws://127.0.0.1:{}{}",
        port,
        dshdesktop_lib::upstream::DSH_MUX_PATH
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut req = url
            .clone()
            .into_client_request()
            .map_err(|e| format!("request: {e}"))?;
        let cookie = format!("{cookie_name}=v1.fake.sig");
        req.headers_mut().insert(
            "cookie",
            HeaderValue::from_str(&cookie).map_err(|e| e.to_string())?,
        );
        match tokio_tungstenite::connect_async(req).await {
            Ok((s, _)) => return Ok(s.split()),
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!("connect: {e}"));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

type WsSplit = (
    futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
);