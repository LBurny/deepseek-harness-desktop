use super::{FrameHandler, NotifySink, NotifySource};
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

const RECONNECT_DELAY: Duration = Duration::from_secs(5);
/// 看门狗：静默超过 PING_IDLE 发一帧 Ping 探活；发出后 PONG_TIMEOUT 内仍无任何
/// 下行帧（含 Pong/心跳）即判定连接已死，强制重连。回环 TCP 半开（对端假死不断
/// FIN）时 `stream.next()` 会永久挂起，没有看门狗就是"从此再无通知直到重启应用"。
const PING_IDLE: Duration = Duration::from_secs(60);
const PONG_TIMEOUT: Duration = Duration::from_secs(30);

/// 订阅 dsh 的事件下行流（dsh 的浏览器信任栅栏允许 loopback + 无 Origin 的请求，
/// Rust 客户端天然满足）。mux 与 host 两个端点共用本实现，区别只在 path 与帧处理：
///   - /api/events.mux：会话事件（approval/question、turn/end、session/title…）
///   - /api/events.host：主机事件（session-added 的 origin 标记子代理…）
/// 断线自动重连，dsh 重启换端口后跟随新端口。
/// on_connect 在每次（重）连建立后触发——host 流用来清空子代理集合（基线不可知，fail-open）。
pub struct WsSource {
    pub path: &'static str,
    pub handler: FrameHandler,
    pub on_connect: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl NotifySource for WsSource {
    fn run(
        self: Box<Self>,
        sink: NotifySink,
        mut port: watch::Receiver<Option<u16>>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            loop {
                let Some(p) = *port.borrow() else {
                    // dsh 尚未就绪，等端口出现
                    if port.changed().await.is_err() {
                        return;
                    }
                    continue;
                };
                let url = format!("ws://127.0.0.1:{p}{}", self.path);
                let ws = match tokio_tungstenite::connect_async(&url).await {
                    Ok((stream, _)) => stream,
                    Err(_) => {
                        tokio::select! {
                            _ = tokio::time::sleep(RECONNECT_DELAY) => {}
                            _ = port.changed() => {}
                        }
                        continue;
                    }
                };
                if let Some(on_connect) = &self.on_connect {
                    on_connect();
                }
                let mut stream = ws;
                // 任何下行帧都会复位探活计时；发过 Ping 后就等 Pong（或任意帧）
                let mut awaiting_pong = false;
                loop {
                    tokio::select! {
                        msg = stream.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    awaiting_pong = false;
                                    (self.handler)(&text, &sink);
                                }
                                // Pong/Ping/Binary 等非文本帧：连接活着，只复位探活计时
                                Some(Ok(_)) => {
                                    awaiting_pong = false;
                                }
                                // 连接关闭/出错/结束：重连
                                _ => break,
                            }
                        }
                        _ = tokio::time::sleep(if awaiting_pong { PONG_TIMEOUT } else { PING_IDLE }) => {
                            if awaiting_pong {
                                break; // Ping 后仍无响应：连接已死，重连
                            }
                            if stream.send(Message::Ping(Vec::new().into())).await.is_err() {
                                break;
                            }
                            awaiting_pong = true;
                        }
                        changed = port.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            break; // 端口变了，用新端口重连
                        }
                    }
                }
            }
        })
    }
}
