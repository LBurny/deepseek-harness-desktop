//! 通知链路集成测试（0.1.2 传输）：MuxSource × 假 dsh 服务器（support/mod.rs）。
//! 旧版（fake-dsh.cjs + 双 WsSource）随 0.1.1 传输退场；fake-dsh.cjs 仅剩
//! tests/process.rs（进程监督）还在用。

use dshdesktop_lib::dsh_session::DshCreds;
use dshdesktop_lib::notify::mux::{MuxEvents, MuxFrame, MuxSource};
use dshdesktop_lib::notify::{
    handle_event_frame, handle_follow_frame, Notification, NotifyKind, NotifySink, SessionBook,
};
use dshdesktop_lib::port::free_port;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

mod support;

/// 等待 pred 对当前收集到的通知集合成立，超时返回 false
async fn wait_for(
    collected: &Arc<Mutex<Vec<Notification>>>,
    timeout: Duration,
    pred: impl Fn(&[Notification]) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if pred(&collected.lock().unwrap()) {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 轮询直到 fake 侧 open 记录满足 pred（流真正 open 后再注入才不会丢帧）
async fn wait_open(fake: &support::FakeDsh, timeout: Duration, needle: &str) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if fake.opens.lock().unwrap().iter().any(|o| o.contains(needle)) {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct Harness {
    fake: support::FakeDsh,
    collected: Arc<Mutex<Vec<Notification>>>,
    scripted_tx: mpsc::UnboundedSender<Value>,
    /// watch 发送端保活：creds_rx.changed() 在发送端被 drop 后立即报 Closed，
    /// MuxSource 会按"creds 永不再变"退出循环（与 WsSource 同语义）
    _creds_tx: watch::Sender<Option<Arc<DshCreds>>>,
}

/// 起假 dsh + MuxSource（与 lib.rs 接线同款：handler 分流 $events/follow，
/// on_connect 清子代理基线 fail-open）
async fn spawn_harness() -> Harness {
    let port = free_port().unwrap();
    let (scripted_tx, scripted_rx) = mpsc::unbounded_channel();
    let fake = support::spawn_fake_dsh(port, support::ScriptedFrames(scripted_rx)).await;
    let (creds_tx, creds_rx) = watch::channel(Some(Arc::new(DshCreds {
        port,
        token: support::FIXTURE_TOKEN.into(),
    })));
    let collected: Arc<Mutex<Vec<Notification>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_store = collected.clone();
    let sink: NotifySink = Arc::new(move |n| sink_store.lock().unwrap().push(n));
    let book = Arc::new(Mutex::new(SessionBook::default()));
    let (follow_tx, follow_rx) = mpsc::unbounded_channel();
    let handler_book = book.clone();
    let handler_follow: dshdesktop_lib::notify::mux::FollowTx = Arc::new(follow_tx);
    let handler: dshdesktop_lib::notify::mux::MuxHandler = Arc::new(move |frame, sink| {
        match frame {
            MuxFrame::EventStream(v) => {
                handle_event_frame(&v, sink, &handler_book, &handler_follow)
            }
            MuxFrame::Follow { session_id, value } => {
                handle_follow_frame(&session_id, &value, sink, &handler_book)
            }
        }
    });
    let mux = MuxSource {
        events: MuxEvents {
            handler,
            on_connect: None,
            follow_rx,
        },
        on_log: None,
    };
    tokio::spawn(Box::new(mux).run(sink, creds_rx));
    Harness {
        fake,
        collected,
        scripted_tx,
        _creds_tx: creds_tx,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mux_connects_and_opens_event_stream() {
    let h = spawn_harness().await;
    assert!(
        wait_open(&h.fake, Duration::from_secs(10), "$events").await,
        "MuxSource 连上后应 open $events，实际 opens：{:?}",
        h.fake.opens.lock().unwrap()
    );
    h.fake.shutdown.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_approval_emits_notification() {
    let h = spawn_harness().await;
    // 等 $events 流开好再注入（广播只送达已 open 的流）
    assert!(wait_open(&h.fake, Duration::from_secs(15), "$events").await);
    h.scripted_tx
        .send(json!({"type":"waterfall","event":"approval/request","eventId":"e1",
            "request":{"toolName":"bash","reason":"run tests"}}))
        .unwrap();
    let got = wait_for(&h.collected, Duration::from_secs(15), |list| {
        list.iter()
            .any(|n| matches!(n.kind, NotifyKind::Approval) && n.body.contains("批准"))
    })
    .await;
    h.fake.shutdown.notify_one();
    assert!(
        got,
        "15s 内未收到 approval 通知，实际：{:?}",
        *h.collected.lock().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_turn_end_notifies_by_worked_state() {
    let h = spawn_harness().await;
    assert!(wait_open(&h.fake, Duration::from_secs(15), "$events").await);
    // api-session/added（origin: null = 主会话）→ 应触发 follow open
    h.scripted_tx
        .send(json!({"type":"emit","event":"api-session/added",
            "args":[{"sessionId":"s1","origin":null,"blank":false,"running":false}]}))
        .unwrap();
    assert!(
        wait_open(&h.fake, Duration::from_secs(15), "session/follow:s1").await,
        "主会话 added 应触发 session/follow open，实际 opens：{:?}",
        h.fake.opens.lock().unwrap()
    );
    // 干活回合：tool/call → turn/end(completed) → 任务完成
    //（假服务器把 scripted 帧广播给所有 open 流，follow 流 item 包裹后到 handler）
    h.scripted_tx
        .send(json!({"type":"event","event":{"type":"session/title","seq":1,"time":0,
            "data":{"title":"fx 主会话"}}}))
        .unwrap();
    h.scripted_tx
        .send(json!({"type":"event","event":{"type":"turn/start","seq":2,"time":0,
            "data":{"turn":1}}}))
        .unwrap();
    h.scripted_tx
        .send(json!({"type":"event","event":{"type":"tool/call","seq":3,"time":0,
            "data":{"callId":"c1","name":"bash","arguments":"{}"}}}))
        .unwrap();
    h.scripted_tx
        .send(json!({"type":"event","event":{"type":"turn/end","seq":4,"time":0,
            "data":{"turn":1,"reason":{"kind":"completed"}}}}))
        .unwrap();
    let got = wait_for(&h.collected, Duration::from_secs(15), |list| {
        list.iter()
            .any(|n| matches!(n.kind, NotifyKind::TaskCompleted))
    })
    .await;
    h.fake.shutdown.notify_one();
    let list = h.collected.lock().unwrap();
    assert!(got, "15s 内未收到任务完成通知，实际：{:?}", *list);
    assert!(
        list.iter()
            .any(|n| n.body == "「fx 主会话」任务完成"),
        "干活回合应带标题报任务完成，实际：{:?}",
        *list
    );
}