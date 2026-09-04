use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::upstream;
use crate::upstream::{
    EVENT_API_SESSION_ADDED, EVENT_API_SESSION_REMOVED, EVENT_APPROVAL_REQUEST,
    EVENT_USER_QUESTIONS_REQUEST,
};
use mux::FollowTx;

pub mod mux;
pub mod toast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    /// 待批准（approval/request waterfall）：按 settings.notify.approval 规则
    Approval,
    /// 待回答（user-questions/request waterfall）：按 settings.notify.question 规则
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

/// 会话台账：子代理集合（$events 的 api-session/added origin=subagent）+
/// 会话标题（follow 流的 session/title 事件）+
/// 当前回合是否干过活（turn/start 清零、tool/call 置位）。
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

/// ===== 0.1.2 结构化帧处理（mux 传输；信封已在 mux.rs 分类，这里只动 value）=====

/// $events 流条目处理（emit/waterfall）：
///   - emit `api-session/added`：origin=="subagent" 记台账；否则请求开 follow
///     （0.1.2 会话事件全走 session/follow 下行，mux 流只报会话生命周期）
///   - emit `api-session/removed`：摘台账 + 关 follow
///   - waterfall `approval/request` / `user-questions/request`：Attention 通知。
///     **严禁回应 waterfall**：本层永不发 `$events/result`——任一客户端回 result
///     即抢先替用户结算审批（prep §八.4 边界）
pub fn handle_event_frame(
    v: &serde_json::Value,
    sink: &NotifySink,
    book: &Mutex<SessionBook>,
    follow_tx: &FollowTx,
) {
    match v.get("type").and_then(|t| t.as_str()) {
        Some("emit") => {
            let name = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
            let args = v.get("args").and_then(|a| a.as_array());
            match name {
                EVENT_API_SESSION_ADDED => {
                    let Some(s) = args.and_then(|a| a.first()) else { return };
                    let Some(id) = s.get("sessionId").and_then(|x| x.as_str()) else {
                        return;
                    };
                    if s.get("origin").and_then(|o| o.as_str()) == Some(upstream::ORIGIN_SUBAGENT)
                    {
                        book.lock().unwrap().add_subagent(id);
                    } else {
                        // 非子代理会话：开 follow 流（子代理事件仍由 book 过滤，不开流）
                        let _ = follow_tx.send(mux::FollowCmd::Open(id.into()));
                    }
                }
                EVENT_API_SESSION_REMOVED => {
                    let Some(id) = args.and_then(|a| a.first()).and_then(|x| x.as_str()) else {
                        return;
                    };
                    book.lock().unwrap().remove(id);
                    let _ = follow_tx.send(mux::FollowCmd::Close(id.into()));
                }
                _ => {}
            }
        }
        Some("waterfall") => {
            let kind = match v.get("event").and_then(|e| e.as_str()) {
                Some(EVENT_APPROVAL_REQUEST) => NotifyKind::Approval,
                Some(EVENT_USER_QUESTIONS_REQUEST) => NotifyKind::Question,
                _ => return,
            };
            sink(Notification {
                title: "DSHDesktop".into(),
                body: summarize_waterfall(kind),
                kind,
            });
        }
        _ => {}
    }
}

/// session/follow 流条目处理：value.type=="event" 的 value.event 即旧
/// session/event 的 event 对象（turn/start / tool/call / turn/end / session/title）。
/// snapshot 帧忽略——历史重放不是新通知。
pub fn handle_follow_frame(
    session_id: &str,
    v: &serde_json::Value,
    sink: &NotifySink,
    book: &Mutex<SessionBook>,
) {
    if v.get("type").and_then(|t| t.as_str()) != Some("event") {
        return; // snapshot / 其他
    }
    let Some(event) = v.get("event") else { return };
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
            let b = book.lock().unwrap();
            if b.is_subagent(session_id) {
                return;
            }
            // 干活回合（回合内有 tool/call）= 任务完成；纯文字回答 = 回答完成。
            // follow（重）连窗口期内 turn/start 可能缺失：工具帧若都在断连前发出，
            // 会被误判成回答完成——两规则默认均开，fail-open 只是分类可能偏，不漏弹。
            let worked = b.has_worked(session_id);
            let body = match b.title(session_id) {
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
            drop(b);
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

/// approval/question 通知摘要（0.1.2 waterfall 的 request 内容不进文案，
/// 通知只提示"有东西等你处理"——详情回界面看，与旧版文案一致）
fn summarize_waterfall(kind: NotifyKind) -> String {
    match kind {
        NotifyKind::Approval => crate::i18n::pick(
            "dsh 有一个操作等待你批准",
            "dsh has an operation waiting for your approval",
        ),
        NotifyKind::Question => {
            crate::i18n::pick("dsh 有一个问题等待你回答", "dsh has a question waiting for you")
        }
        NotifyKind::TaskCompleted | NotifyKind::AnswerCompleted => String::new(),
    }
}

/// approval/question 帧的可读摘要（dsh 帧结构：{"type":"server-request","method":..,"payload":..}）
#[cfg(test)]
mod tests {
    use super::*;

    fn collecting_sink() -> (NotifySink, Arc<Mutex<Vec<Notification>>>) {
        let store = Arc::new(Mutex::new(Vec::new()));
        let s = store.clone();
        (Arc::new(move |n| s.lock().unwrap().push(n)), store)
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

    // ===== 0.1.2 结构化帧用例（对齐旧用例逐一平移，断言不变）=====

    use mux::FollowCmd;

    fn api_added(id: &str, origin: Option<&str>) -> serde_json::Value {
        serde_json::json!({"type":"emit","event":"api-session/added",
            "args":[{"sessionId":id,"origin":origin,"blank":false,"running":false}]})
    }

    fn api_removed(id: &str) -> serde_json::Value {
        serde_json::json!({"type":"emit","event":"api-session/removed","args":[id]})
    }

    fn waterfall(name: &str, request: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type":"waterfall","event":name,"eventId":"e1","request":request})
    }

    fn follow_event(id: &str, event: serde_json::Value) -> (String, serde_json::Value) {
        (id.into(), serde_json::json!({"type":"event","event":event}))
    }

    fn ev_turn_end(seq: u32, kind: &str) -> serde_json::Value {
        serde_json::json!({"type":"turn/end","seq":seq,"time":0,"data":{"turn":1,"reason":{"kind":kind}}})
    }

    fn ev_turn_start(seq: u32) -> serde_json::Value {
        serde_json::json!({"type":"turn/start","seq":seq,"time":0,"data":{"turn":1}})
    }

    fn ev_tool_call(seq: u32) -> serde_json::Value {
        serde_json::json!({"type":"tool/call","seq":seq,"time":0,"data":{"callId":"c","name":"bash","arguments":"{}"}})
    }

    fn ev_title(seq: u32, text: &str) -> serde_json::Value {
        serde_json::json!({"type":"session/title","seq":seq,"time":0,"data":{"title":text}})
    }

    fn opener_channel() -> (FollowTx, Arc<Mutex<Vec<FollowCmd>>>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        std::thread::spawn(move || {
            while let Some(cmd) = rx.blocking_recv() {
                s.lock().unwrap().push(cmd);
            }
        });
        (Arc::new(tx), seen)
    }

    #[test]
    fn event_waterfall_attention_notifies() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        let (tx, _seen) = opener_channel();
        handle_event_frame(
            &waterfall("approval/request", serde_json::json!({"toolName":"bash"})),
            &sink,
            &book,
            &tx,
        );
        handle_event_frame(
            &waterfall("user-questions/request", serde_json::json!({"questions":[]}),),
            &sink,
            &book,
            &tx,
        );
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 2);
        assert!(matches!(got[0].kind, NotifyKind::Approval));
        assert!(matches!(got[1].kind, NotifyKind::Question));
        assert_eq!(got[0].body, "dsh 有一个操作等待你批准");
        assert_eq!(got[1].body, "dsh 有一个问题等待你回答");
        // Attention 永不回应 $events/result（本层无回包路径——只能靠不实现保证）
    }

    #[test]
    fn event_added_opens_follow_or_marks_subagent() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        let (tx, seen) = opener_channel();
        // 主会话：开 follow
        handle_event_frame(&api_added("s-main", None), &sink, &book, &tx);
        // 子代理：记台账不开流
        handle_event_frame(&api_added("s-sub", Some("subagent")), &sink, &book, &tx);
        // removed：摘台账 + 关 follow
        handle_event_frame(&api_removed("s-main"), &sink, &book, &tx);
        // 收集线程可能晚一拍：轮询到 2 条命令（Open + Close）为止
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let n = seen.lock().unwrap().len();
            if n >= 2 || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let cmds = seen.lock().unwrap().clone();
        assert_eq!(
            cmds,
            vec![
                FollowCmd::Open("s-main".into()),
                FollowCmd::Close("s-main".into())
            ],
            "只有非子代理会话触发 follow open；removed 触发 Close"
        );
        assert!(store.lock().unwrap().is_empty());
    }

    #[test]
    fn follow_worked_turn_notifies_as_task_completed() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        let (sid, v) = follow_event("s1", ev_title(1, "修 bug"));
        handle_follow_frame(&sid, &v, &sink, &book);
        let (sid, v) = follow_event("s1", ev_turn_start(2));
        handle_follow_frame(&sid, &v, &sink, &book);
        let (sid, v) = follow_event("s1", ev_tool_call(3));
        handle_follow_frame(&sid, &v, &sink, &book);
        let (sid, v) = follow_event("s1", ev_turn_end(4, "completed"));
        handle_follow_frame(&sid, &v, &sink, &book);
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].kind, NotifyKind::TaskCompleted));
        assert_eq!(got[0].body, "「修 bug」任务完成");
        assert_eq!(got[0].title, "DSHDesktop");
    }

    #[test]
    fn follow_pure_reply_notifies_as_answer_completed() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        let (sid, v) = follow_event("s1", ev_title(1, "修 bug"));
        handle_follow_frame(&sid, &v, &sink, &book);
        let (sid, v) = follow_event("s1", ev_turn_end(2, "completed"));
        handle_follow_frame(&sid, &v, &sink, &book);
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].kind, NotifyKind::AnswerCompleted));
        assert_eq!(got[0].body, "「修 bug」回答完成");
    }

    #[test]
    fn follow_turn_start_clears_worked_between_turns() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        let (sid, v) = follow_event("s1", ev_turn_start(1));
        handle_follow_frame(&sid, &v, &sink, &book);
        let (sid, v) = follow_event("s1", ev_tool_call(2));
        handle_follow_frame(&sid, &v, &sink, &book);
        let (sid, v) = follow_event("s1", ev_turn_end(3, "completed"));
        handle_follow_frame(&sid, &v, &sink, &book);
        let (sid, v) = follow_event("s1", ev_turn_start(4));
        handle_follow_frame(&sid, &v, &sink, &book);
        let (sid, v) = follow_event("s1", ev_turn_end(5, "completed"));
        handle_follow_frame(&sid, &v, &sink, &book);
        let got = store.lock().unwrap();
        assert_eq!(got.len(), 2);
        assert!(matches!(got[0].kind, NotifyKind::TaskCompleted));
        assert_eq!(got[0].body, "dsh 任务完成");
        assert!(matches!(got[1].kind, NotifyKind::AnswerCompleted));
        assert_eq!(got[1].body, "dsh 回答完成");
    }

    #[test]
    fn follow_turn_end_not_completed_is_silent() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        for kind in ["aborted", "error", "blocked", "max-tokens"] {
            let (sid, v) = follow_event("s1", ev_turn_end(1, kind));
            handle_follow_frame(&sid, &v, &sink, &book);
        }
        assert!(store.lock().unwrap().is_empty());
    }

    #[test]
    fn follow_snapshot_and_garbage_are_ignored() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        // snapshot（历史重放）不触发通知
        let (sid, v) = (
            "s1".to_string(),
            serde_json::json!({"type":"snapshot","header":{},"cursor":0,"records":[]}),
        );
        handle_follow_frame(&sid, &v, &sink, &book);
        // 垃圾输入
        let (sid, v) = follow_event("s1", serde_json::json!({"type":"assistant/chunk","seq":1,"time":0,"data":{}}));
        handle_follow_frame(&sid, &v, &sink, &book);
        handle_follow_frame("s1", &serde_json::json!({}), &sink, &book);
        handle_event_frame(
            &serde_json::json!({"type":"emit","event":"settings/document-updated","args":[]}),
            &sink,
            &book,
            &opener_channel().0,
        );
        assert!(store.lock().unwrap().is_empty());
    }

    #[test]
    fn follow_skips_subagent_turn_completed() {
        let (sink, store) = collecting_sink();
        let book = Mutex::new(SessionBook::default());
        let (tx, _seen) = opener_channel();
        handle_event_frame(&api_added("sub1", Some("subagent")), &sink, &book, &tx);
        let (sid, v) = follow_event("sub1", ev_turn_end(1, "completed"));
        handle_follow_frame(&sid, &v, &sink, &book);
        assert!(store.lock().unwrap().is_empty());
        // removed 后同 id 不再按子代理过滤
        handle_event_frame(&api_removed("sub1"), &sink, &book, &tx);
        let (sid, v) = follow_event("sub1", ev_turn_end(2, "completed"));
        handle_follow_frame(&sid, &v, &sink, &book);
        assert_eq!(store.lock().unwrap().len(), 1);
    }
}
