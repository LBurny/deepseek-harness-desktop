use futures::future::BoxFuture;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::watch;

use crate::upstream;

pub mod toast;
pub mod ws;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    /// 待批准（approval/requested）：按 settings.notify.approval 规则
    Approval,
    /// 待回答（question/requested）：按 settings.notify.question 规则
    Question,
    /// 干活回合正常完成（回合内有过 tool/call）：按 settings.notify.turn_done 规则
    TaskCompleted,
    /// 纯回答回合正常完成（回合内无任何工具调用）：按 settings.notify.answer_done 规则
    AnswerCompleted,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub kind: NotifyKind,
}

pub type NotifySink = Arc<dyn Fn(Notification) + Send + Sync>;
/// WS 帧处理器：mux/host 两个端点各配各的；sink 形参仅供 mux 使用
pub type FrameHandler = Arc<dyn Fn(&str, &NotifySink) + Send + Sync>;

/// 通知来源适配器接口。dsh 上游接口不稳定（开发者预览），
/// 后期可加 FileWatchSource（解析 session jsonl）等替代实现。
pub trait NotifySource: Send {
    /// port 通过 watch 通道下发：dsh 每次 Ready（含重启换端口）都会更新。
    fn run(self: Box<Self>, sink: NotifySink, port: watch::Receiver<Option<u16>>) -> BoxFuture<'static, ()>;
}

/// 需要用户关注的事件（server-request 帧的 method），粗筛快路径
static ATTENTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#""(method|type)"\s*:\s*"({}|{})""#,
        upstream::METHOD_APPROVAL,
        upstream::METHOD_QUESTION
    ))
    .unwrap()
});

/// 会话台账：子代理集合（来自 events.host 的 origin=subagent）+
/// 会话标题（来自 mux 的 session/title 事件）+
/// 当前回合是否干过活（mux 的 turn/start 清零、tool/call 置位）。
#[derive(Default)]
pub struct SessionBook {
    subagents: HashSet<String>,
    titles: HashMap<String, String>,
    /// 当前回合内出现过 tool/call 的会话：turn/end 时据此拆分
    /// 任务完成（干过活）/ 回答完成（纯文字回答）
    worked: HashSet<String>,
}

impl SessionBook {
    pub fn add_subagent(&mut self, id: &str) {
        self.subagents.insert(id.into());
    }
    pub fn remove(&mut self, id: &str) {
        self.subagents.remove(id);
        self.titles.remove(id);
        self.worked.remove(id);
    }
    pub fn set_title(&mut self, id: &str, title: &str) {
        self.titles.insert(id.into(), title.into());
    }
    pub fn is_subagent(&self, id: &str) -> bool {
        self.subagents.contains(id)
    }
    pub fn title(&self, id: &str) -> Option<String> {
        self.titles.get(id).cloned()
    }
    /// 回合开始：清上一轮的干活痕迹（mux 帧按序到达，turn/start 先于本轮全部事件）
    pub fn clear_worked(&mut self, id: &str) {
        self.worked.remove(id);
    }
    /// 本回合出现过工具调用
    pub fn mark_worked(&mut self, id: &str) {
        self.worked.insert(id.into());
    }
    pub fn has_worked(&self, id: &str) -> bool {
        self.worked.contains(id)
    }
    /// host 流（重）连后基线不可知：清空子代理集合，fail-open（宁多弹不漏弹）。
    /// worked 不随 host 重连清空——它由 mux 流维护，与 host 基线无关。
    pub fn clear_subagents(&mut self) {
        self.subagents.clear();
    }
}

/// mux 帧处理：approval/question → Attention 通知；session/event 里关心
/// turn/start（清干活痕迹）、tool/call（置干活痕迹）、turn/end
/// (reason.kind=="completed" 时按是否干过活拆分任务完成/回答完成) 与
/// session/title。
/// 先 contains 粗筛、命中才 JSON 解析——流式期间每 token 一帧，不能逢帧解析。
pub fn handle_mux_frame(frame: &str, sink: &NotifySink, book: &Mutex<SessionBook>) {
    if ATTENTION_RE.is_match(frame) {
        let kind = if frame.contains(upstream::METHOD_APPROVAL) {
            NotifyKind::Approval
        } else {
            NotifyKind::Question
        };
        sink(Notification {
            title: "DSHDesktop".into(),
            body: summarize_attention(frame),
            kind,
        });
        return;
    }
    if !frame.contains(upstream::METHOD_SESSION_EVENT) {
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(frame) else {
        return;
    };
    if v.get("method").and_then(|m| m.as_str()) != Some(upstream::METHOD_SESSION_EVENT) {
        return;
    }
    let Some(payload) = v.get("payload") else { return };
    let Some(session_id) = payload.get("sessionId").and_then(|s| s.as_str()) else {
        return;
    };
    let Some(event) = payload.get("event") else { return };
    match event.get("type").and_then(|t| t.as_str()) {
        Some(upstream::EVENT_TURN_START) => {
            book.lock().unwrap().clear_worked(session_id);
        }
        Some(upstream::EVENT_TOOL_CALL) => {
            book.lock().unwrap().mark_worked(session_id);
        }
        Some(upstream::EVENT_TURN_END) => {
            let completed = event
                .get("data")
                .and_then(|d| d.get("reason"))
                .and_then(|r| r.get("kind"))
                .and_then(|k| k.as_str())
                == Some(upstream::REASON_COMPLETED);
            if !completed {
                return;
            }
            let book = book.lock().unwrap();
            if book.is_subagent(session_id) {
                return;
            }
            // 干活回合（回合内有 tool/call）= 任务完成；纯文字回答 = 回答完成。
            // WS（重）连窗口期内 turn/start 可能缺失：工具帧若都在断连前发出，
            // 会被误判成回答完成——两规则默认均开，fail-open 只是分类可能偏，不漏弹。
            let worked = book.has_worked(session_id);
            let body = match book.title(session_id) {
                Some(t) => {
                    if worked {
                        crate::i18n::pick(format!("「{t}」任务完成"), format!("“{t}” task completed"))
                    } else {
                        crate::i18n::pick(format!("「{t}」回答完成"), format!("“{t}” reply completed"))
                    }
                }
                None => {
                    if worked {
                        crate::i18n::pick("dsh 任务完成", "dsh finished the task")
                    } else {
                        crate::i18n::pick("dsh 回答完成", "dsh completed its reply")
                    }
                }
            };
            let kind = if worked {
                NotifyKind::TaskCompleted
            } else {
                NotifyKind::AnswerCompleted
            };
            sink(Notification {
                title: "DSHDesktop".into(),
                body,
                kind,
            });
        }
        Some(upstream::EVENT_SESSION_TITLE) => {
            if let Some(title) = event
                .get("data")
                .and_then(|d| d.get("title"))
                .and_then(|t| t.as_str())
            {
                book.lock().unwrap().set_title(session_id, title);
            }
        }
        _ => {}
    }
}

/// host 帧处理：只跟踪 session-added(origin=="subagent") / session-removed
pub fn handle_host_frame(frame: &str, book: &Mutex<SessionBook>) {
    let added = frame.contains(upstream::METHOD_HOST_SESSION_ADDED);
    if !added && !frame.contains(upstream::METHOD_HOST_SESSION_REMOVED) {
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(frame) else {
        return;
    };
    let method = v.get("method").and_then(|m| m.as_str());
    let Some(payload) = v.get("payload") else { return };
    let Some(id) = payload.get("sessionId").and_then(|s| s.as_str()) else {
        return;
    };
    match method {
        Some(upstream::METHOD_HOST_SESSION_ADDED)
            if payload.get("origin").and_then(|o| o.as_str()) == Some(upstream::ORIGIN_SUBAGENT) =>
        {
            book.lock().unwrap().add_subagent(id);
        }
        Some(upstream::METHOD_HOST_SESSION_REMOVED) => book.lock().unwrap().remove(id),
        _ => {}
    }
}

/// approval/question 帧的可读摘要（dsh 帧结构：{"type":"server-request","method":..,"payload":..}）
fn summarize_attention(frame: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(frame).ok();
    let method = parsed
        .as_ref()
        .and_then(|v| v.get("method"))
        .and_then(|t| t.as_str())
        .map(String::from);
    match method.as_deref() {
        Some("approval/requested") => crate::i18n::pick(
            "dsh 有一个操作等待你批准",
            "dsh has an operation waiting for your approval",
        ),
        Some("question/requested") => {
            crate::i18n::pick("dsh 有一个问题等待你回答", "dsh has a question waiting for you")
        }
        Some(m) => crate::i18n::pick(format!("dsh 事件：{m}"), format!("dsh event: {m}")),
        None => frame.chars().take(80).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collecting_sink() -> (NotifySink, Arc<Mutex<Vec<Notification>>>) {
        let store = Arc::new(Mutex::new(Vec::new()));
        let s = store.clone();
        (Arc::new(move |n| s.lock().unwrap().push(n)), store)
    }

    const APPROVAL: &str =
        r#"{"type":"server-request","rpcId":"x","method":"approval/requested","payload":{}}"#;
    const QUESTION: &str =
        r#"{"type":"server-request","method":"question/requested","payload":{}}"#;

    fn session_event(session_id: &str, event: &str) -> String {
        format!(
            r#"{{"type":"server-request","method":"session/event","payload":{{"type":"session/event","sessionId":"{session_id}","event":{event}}}}}"#
        )
    }

    fn turn_end(seq: u32, kind: &str) -> String {
        format!(
            r#"{{"type":"turn/end","seq":{seq},"time":0,"data":{{"turn":1,"reason":{{"kind":"{kind}"}}}}}}"#
        )
    }

    fn turn_start(seq: u32) -> String {
        format!(r#"{{"type":"turn/start","seq":{seq},"time":0,"data":{{"turn":1}}}}"#)
    }

    fn tool_call(seq: u32) -> String {
        format!(
            r#"{{"type":"tool/call","seq":{seq},"time":0,"data":{{"callId":"c{seq}","name":"bash","arguments":"{{}}"}}}}"#
        )
    }

    #[test]
    fn mux_attention_events_notify() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        handle_mux_frame(APPROVAL, &sink, &book);
        handle_mux_frame(QUESTION, &sink, &book);
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 2);
        assert!(matches!(got[0].kind, NotifyKind::Approval));
        assert!(matches!(got[1].kind, NotifyKind::Question));
        assert_eq!(got[0].body, "dsh 有一个操作等待你批准");
        assert_eq!(got[1].body, "dsh 有一个问题等待你回答");
    }

    #[test]
    fn mux_worked_turn_notifies_as_task_completed() {
        // 回合内有过 tool/call → 任务完成（走 turn_done 规则）
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        handle_mux_frame(
            &session_event(
                "s1",
                r#"{"type":"session/title","seq":1,"time":0,"data":{"title":"修 bug"}}"#,
            ),
            &sink,
            &book,
        );
        handle_mux_frame(&session_event("s1", &turn_start(2)), &sink, &book);
        handle_mux_frame(&session_event("s1", &tool_call(3)), &sink, &book);
        handle_mux_frame(&session_event("s1", &turn_end(4, "completed")), &sink, &book);
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].kind, NotifyKind::TaskCompleted));
        assert_eq!(got[0].body, "「修 bug」任务完成");
        assert_eq!(got[0].title, "DSHDesktop");
    }

    #[test]
    fn mux_pure_reply_notifies_as_answer_completed() {
        // 无 tool/call 的回合 → 回答完成（走 answer_done 规则）
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        handle_mux_frame(
            &session_event(
                "s1",
                r#"{"type":"session/title","seq":1,"time":0,"data":{"title":"修 bug"}}"#,
            ),
            &sink,
            &book,
        );
        handle_mux_frame(&session_event("s1", &turn_end(2, "completed")), &sink, &book);
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].kind, NotifyKind::AnswerCompleted));
        assert_eq!(got[0].body, "「修 bug」回答完成");
        assert_eq!(got[0].title, "DSHDesktop");
    }

    #[test]
    fn mux_turn_start_clears_worked_between_turns() {
        // 上一轮干过活、新一轮 turn/start 后纯回答 → 应算回答完成而不是任务完成
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        handle_mux_frame(&session_event("s1", &turn_start(1)), &sink, &book);
        handle_mux_frame(&session_event("s1", &tool_call(2)), &sink, &book);
        handle_mux_frame(&session_event("s1", &turn_end(3, "completed")), &sink, &book);
        handle_mux_frame(&session_event("s1", &turn_start(4)), &sink, &book);
        handle_mux_frame(&session_event("s1", &turn_end(5, "completed")), &sink, &book);
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 2);
        assert!(matches!(got[0].kind, NotifyKind::TaskCompleted));
        assert_eq!(got[0].body, "dsh 任务完成");
        assert!(matches!(got[1].kind, NotifyKind::AnswerCompleted));
        assert_eq!(got[1].body, "dsh 回答完成");
    }

    #[test]
    fn mux_task_completed_without_title_uses_fallback() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        handle_mux_frame(&session_event("s1", &tool_call(1)), &sink, &book);
        handle_mux_frame(&session_event("s1", &turn_end(2, "completed")), &sink, &book);
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].body, "dsh 任务完成");
    }

    #[test]
    fn mux_turn_end_not_completed_is_silent() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        for kind in ["aborted", "error", "blocked", "max-tokens"] {
            handle_mux_frame(&session_event("s1", &turn_end(1, kind)), &sink, &book);
        }
        assert!(store.lock().unwrap().is_empty());
    }

    #[test]
    fn mux_ignores_other_session_events_and_garbage() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        handle_mux_frame(
            &session_event("s1", r#"{"type":"assistant/chunk","seq":1,"time":0,"data":{}}"#),
            &sink,
            &book,
        );
        handle_mux_frame(
            r#"{"type":"server-request","method":"host/session-added","payload":{}}"#,
            &sink,
            &book,
        );
        handle_mux_frame("garbage", &sink, &book);
        handle_mux_frame(r#"{"method":"session/event","payload": 非法"#, &sink, &book);
        assert!(store.lock().unwrap().is_empty());
    }

    #[test]
    fn mux_skips_subagent_turn_completed() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        handle_host_frame(
            r#"{"type":"server-request","method":"host/session-added","payload":{"type":"host/session-added","sessionId":"sub1","blank":false,"origin":"subagent"}}"#,
            &book,
        );
        handle_mux_frame(&session_event("sub1", &turn_end(1, "completed")), &sink, &book);
        assert!(store.lock().unwrap().is_empty());
        // removed 后同 id 不再按子代理过滤
        handle_host_frame(
            r#"{"type":"server-request","method":"host/session-removed","payload":{"type":"host/session-removed","sessionId":"sub1"}}"#,
            &book,
        );
        handle_mux_frame(&session_event("sub1", &turn_end(2, "completed")), &sink, &book);
        assert_eq!(store.lock().unwrap().len(), 1);
    }

    #[test]
    fn host_added_without_origin_is_not_subagent() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        handle_host_frame(
            r#"{"type":"server-request","method":"host/session-added","payload":{"type":"host/session-added","sessionId":"m1","blank":true}}"#,
            &book,
        );
        handle_mux_frame(&session_event("m1", &turn_end(1, "completed")), &sink, &book);
        assert_eq!(store.lock().unwrap().len(), 1);
    }

    #[test]
    fn host_ignores_garbage() {
        let book = Mutex::new(SessionBook::default());
        handle_host_frame("garbage", &book);
        handle_host_frame(r#"{"method":"host/session-added","payload": 非法"#, &book);
        handle_host_frame(
            r#"{"type":"server-request","method":"host/session-status","payload":{"sessionId":"s","running":true}}"#,
            &book,
        );
        assert!(!book.lock().unwrap().is_subagent("s"));
    }

    #[test]
    fn session_book_titles_and_clear() {
        let mut b = SessionBook::default();
        b.add_subagent("a");
        assert!(b.is_subagent("a"));
        b.clear_subagents();
        assert!(!b.is_subagent("a"));
        b.set_title("s", "标题");
        assert_eq!(b.title("s").as_deref(), Some("标题"));
        b.remove("s");
        assert_eq!(b.title("s"), None);
    }
}
