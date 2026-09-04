//! 0.1.2 事件源：单 WS mux 连接（/api/remote.mux）承载 $events 全局下行 +
//! N 条 session/follow 会话流。传输契约（prep 文档 §三）：
//!   客户端帧 {"type":"open","streamId","endpoint","payload":{"args":…}} / {"type":"cancel",…}
//!   服务端帧 {"type":"item","streamId","value"} / {"type":"end",…} / {"type":"error",…}
//!   $events open（空 args）后首条 item 是 ready；follow 的 args 是
//!   {request:{address:{kind:"session",sessionId}}}（typert wire 名 request）。
//! 鉴权：WS 升级须带 dsh-auth cookie（BrowserAuth 无关闭开关，回环也在门内）；
//! cookie 经 dsh_session::exchange_cookie 由 launch token 换取，本层每次重连
//! 现换（token 变化经 creds watch 下发，缓存旧值反而是 401 陷阱）。
//! 连接层只管传输与信封：帧语义全在 handler（notify::handle_event_frame 等）。

use super::NotifySink;
use crate::dsh_session::{self, DshCreds};
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const RECONNECT_DELAY: Duration = Duration::from_secs(5);
/// 看门狗（自 ws.rs 平移）：静默超 PING_IDLE 发 Ping 探活；PONG_TIMEOUT 内仍无
/// 任何下行帧判死重连。0.1.2 服务端每 30s 心跳 Ping——任意下行帧都复位探活，
/// 天然兼容；对端假死时靠这里兜底，否则"从此再无通知直到重启应用"。
const PING_IDLE: Duration = Duration::from_secs(60);
const PONG_TIMEOUT: Duration = Duration::from_secs(30);

/// follow 流管理命令（handler 层经 FollowTx 下发，run 循环 select 消费）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowCmd {
    /// 开（或重开）一条 session/follow 流：登记 id 集，重连时逐条重 open
    Open(String),
    /// 会话已移除：cancel 流并从 id 集摘除（重连不再重 open）
    Close(String),
}

/// handler 持有的 follow 下发端（Arc 化便于多处克隆）
pub type FollowTx = Arc<mpsc::UnboundedSender<FollowCmd>>;

/// mux 逻辑帧回调：连接层只管传输与信封，语义全在 handler
pub type MuxHandler = Arc<dyn Fn(MuxFrame, &NotifySink) + Send + Sync>;

pub enum MuxFrame {
    /// $events 流条目：{type:'ready'|'emit'|'waterfall'|'cancel', …}（JSON 值原样）
    EventStream(serde_json::Value),
    /// session/follow 流条目：{type:'snapshot'|'event', …}（JSON 值原样）
    Follow {
        session_id: String,
        value: serde_json::Value,
    },
}

pub struct MuxEvents {
    pub handler: MuxHandler,
    /// 每次（重）连建立后触发（清子代理基线，fail-open 同旧 host 流语义）
    pub on_connect: Option<Arc<dyn Fn() + Send + Sync>>,
    /// follow 命令接收端（lib.rs 建通道：tx 进 handler 闭包，rx 交给 run）
    pub follow_rx: mpsc::UnboundedReceiver<FollowCmd>,
}

pub struct MuxSource {
    pub events: MuxEvents,
    /// 运行诊断（events.log 追加；None 则只 eprintln）。只记端口/事件名，不记
    /// token/cookie——链接即凭据
    pub on_log: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

impl MuxSource {
    /// 连接循环（独立于旧 NotifySource trait——ws.rs 占用旧 trait 形状直到 Task 8 退场）
    pub fn run(
        self: Box<Self>,
        sink: NotifySink,
        creds: watch::Receiver<Option<Arc<DshCreds>>>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(self.run_impl(sink, creds))
    }

    async fn run_impl(
        self,
        sink: NotifySink,
        mut creds: watch::Receiver<Option<Arc<DshCreds>>>,
    ) {
        let MuxEvents {
            handler,
            on_connect,
            mut follow_rx,
        } = self.events;
        let log = self.on_log;
        let log_fn = move |line: String| {
            if let Some(f) = &log {
                f(line);
            } else {
                eprintln!("[mux] {line}");
            }
        };
        // 已登记的 follow 会话（重连重 open；error/end 帧与 Close 命令摘除）
        let mut follows: Vec<String> = Vec::new();
        loop {
            // 1) 等 creds（None → changed；watch 关闭即退出）
            let Some(c) = creds.borrow().clone() else {
                if creds.changed().await.is_err() {
                    return;
                }
                continue;
            };
            // 2) token 换 cookie（每次重连现换——cookie 绑端口、token 绑进程，缓存必过期）
            let cookie = match dsh_session::exchange_cookie(c.port, &c.token).await {
                Ok(k) => k,
                Err(e) => {
                    log_fn(format!(
                        "[mux] cookie 交换失败（端口 {}）：{e}，{RECONNECT_DELAY:?} 后重试",
                        c.port
                    ));
                    tokio::select! {
                        _ = tokio::time::sleep(RECONNECT_DELAY) => {}
                        r = creds.changed() => if r.is_err() { return },
                    }
                    continue;
                }
            };
            // 3) WS 连接（带 cookie）
            let ws = match connect_mux(c.port, &cookie).await {
                Ok(s) => s,
                Err(_) => {
                    tokio::select! {
                        _ = tokio::time::sleep(RECONNECT_DELAY) => {}
                        r = creds.changed() => if r.is_err() { return },
                    }
                    continue;
                }
            };
            let (mut tx, mut rx) = ws.split();
            // 4) open $events（空 args）
            let events_stream_id = next_stream_id();
            let open = serde_json::json!({
                "type": "open",
                "streamId": events_stream_id,
                "endpoint": crate::upstream::EVENT_STREAM_ENDPOINT,
                "payload": { "args": {} }
            });
            if tx.send(Message::Text(open.to_string().into())).await.is_err() {
                continue;
            }
            // 5) 重 open 已登记 follow（fail-open：重连后 id 集保留，会话可能仍在）
            let mut streams: HashMap<String, StreamKind> = HashMap::new();
            streams.insert(events_stream_id.clone(), StreamKind::Events);
            for sid in &follows {
                let id = next_stream_id();
                let open = serde_json::json!({
                    "type": "open",
                    "streamId": id,
                    "endpoint": crate::upstream::METHOD_SESSION_FOLLOW,
                    "payload": { "args": { "request": { "address": { "kind": "session", "sessionId": sid } } } }
                });
                if tx.send(Message::Text(open.to_string().into())).await.is_err() {
                    break;
                }
                streams.insert(id, StreamKind::Follow(sid.clone()));
            }
            if let Some(on_connect) = &on_connect {
                on_connect();
            }
            // 6) 收发循环 + 看门狗
            let mut awaiting_pong = false;
            loop {
                tokio::select! {
                    msg = rx.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                awaiting_pong = false;
                                match classify_frame(&text, &streams) {
                                    Frame::Item(frame) => handler(frame, &sink),
                                    // follow 流终结（会话关闭/不存在）：从登记摘除，
                                    // 重连不再重 open；会话再活跃时 $events 会重新通知
                                    Frame::FollowEnded(sid) => {
                                        follows.retain(|s| s != &sid);
                                        let ids: Vec<String> = streams
                                            .iter()
                                            .filter(|(_, k)| matches!(k, StreamKind::Follow(s) if s == &sid))
                                            .map(|(id, _)| id.clone())
                                            .collect();
                                        for id in ids {
                                            streams.remove(&id);
                                        }
                                    }
                                    Frame::Ignored => {}
                                }
                            }
                            // Ping/Pong/Binary：连接活着，复位探活（tungstenite 自答 Pong）
                            Some(Ok(_)) => { awaiting_pong = false; }
                            // 关闭/出错：重连
                            _ => break,
                        }
                    }
                    cmd = follow_rx.recv() => {
                        let Some(cmd) = cmd else { return }; // 发送端全灭：退出
                        match cmd {
                            FollowCmd::Open(sid) => {
                                if follows.contains(&sid) {
                                    continue;
                                }
                                let id = next_stream_id();
                                let open = serde_json::json!({
                                    "type": "open",
                                    "streamId": id,
                                    "endpoint": crate::upstream::METHOD_SESSION_FOLLOW,
                                    "payload": { "args": { "request": { "address": { "kind": "session", "sessionId": sid } } } }
                                });
                                if tx.send(Message::Text(open.to_string().into())).await.is_err() {
                                    break;
                                }
                                streams.insert(id, StreamKind::Follow(sid.clone()));
                                follows.push(sid);
                            }
                            FollowCmd::Close(sid) => {
                                follows.retain(|s| s != &sid);
                                let ids: Vec<String> = streams
                                    .iter()
                                    .filter(|(_, k)| matches!(k, StreamKind::Follow(s) if s == &sid))
                                    .map(|(id, _)| id.clone())
                                    .collect();
                                for id in ids {
                                    streams.remove(&id);
                                    let cancel = serde_json::json!({"type":"cancel","streamId":id});
                                    let _ = tx.send(Message::Text(cancel.to_string().into())).await;
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(if awaiting_pong { PONG_TIMEOUT } else { PING_IDLE }) => {
                        if awaiting_pong {
                            break; // Ping 后仍无响应：连接已死
                        }
                        if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                            break;
                        }
                        awaiting_pong = true;
                    }
                    r = creds.changed() => {
                        if r.is_err() {
                            return;
                        }
                        break; // dsh 重启（端口/token 变了），用新 creds 重连
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum StreamKind {
    Events,
    Follow(String),
}

static STREAM_COUNTER: AtomicU64 = AtomicU64::new(0);

/// streamId 只需连接内唯一（服务端按连接隔离路由），进程级计数器自增即可
fn next_stream_id() -> String {
    format!("dsh-{}", STREAM_COUNTER.fetch_add(1, Ordering::Relaxed))
}

type WsHalf = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 连 remote.mux：带 cookie（BrowserAuth 门内；栅栏允许 loopback 无 Origin 的客户端）
async fn connect_mux(port: u16, cookie: &str) -> Result<WsHalf, String> {
    let url = format!("ws://127.0.0.1:{port}{}", crate::upstream::DSH_MUX_PATH);
    let mut req = url
        .into_client_request()
        .map_err(|e| format!("ws request: {e}"))?;
    req.headers_mut().insert(
        "cookie",
        HeaderValue::from_str(cookie).map_err(|e| format!("cookie 头: {e}"))?,
    );
    let (stream, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| format!("ws connect: {e}"))?;
    Ok(stream)
}

enum Frame {
    Item(MuxFrame),
    /// follow 流的 end/error 帧（会话终结）；session_id 供登记摘除
    FollowEnded(String),
    Ignored,
}

/// 服务端帧分类（连接层只认信封，不动 value 语义）：
///   {"type":"item","streamId",value} → 按流映射转 MuxFrame
///   {"type":"end"|"error","streamId"} → follow 流终结（会话关闭/不存在）
///   其余（ready 层的 Ping 等已在上一级消化）→ 忽略
fn classify_frame(text: &str, streams: &HashMap<String, StreamKind>) -> Frame {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return Frame::Ignored;
    };
    let Some(kind) = v.get("type").and_then(|t| t.as_str()) else {
        return Frame::Ignored;
    };
    let Some(id) = v.get("streamId").and_then(|s| s.as_str()) else {
        return Frame::Ignored;
    };
    match kind {
        "item" => {
            let value = v.get("value").cloned().unwrap_or_default();
            match streams.get(id) {
                Some(StreamKind::Events) => Frame::Item(MuxFrame::EventStream(value)),
                Some(StreamKind::Follow(sid)) => Frame::Item(MuxFrame::Follow {
                    session_id: sid.clone(),
                    value,
                }),
                None => Frame::Ignored,
            }
        }
        "end" | "error" => match streams.get(id) {
            Some(StreamKind::Follow(sid)) => Frame::FollowEnded(sid.clone()),
            _ => Frame::Ignored,
        },
        _ => Frame::Ignored,
    }
}