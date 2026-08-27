use dshdesktop_lib::notify::ws::WsSource;
use dshdesktop_lib::notify::{
    handle_host_frame, handle_mux_frame, FrameHandler, Notification, NotifyKind, NotifySink,
    NotifySource, SessionBook,
};
use dshdesktop_lib::port::free_port;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

fn system_node() -> PathBuf {
    let out = Command::new("where").arg("node").output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    PathBuf::from(stdout.lines().next().expect("node not found on PATH").trim())
}

fn spawn_fixture(port: u16, work: &std::path::Path) -> std::process::Child {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-dsh.cjs");
    Command::new(system_node())
        .arg(fixture)
        .arg("web")
        .arg("--port")
        .arg(port.to_string())
        .current_dir(work)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

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

/// 起 mux+host 双 WS 源（与 lib.rs 接线同款），返回 (端口通道发送端, 通知收集器)；
/// 发送端必须由调用方持有——drop 后 watch 关闭，两个源会立即退出
fn spawn_sources(port: u16) -> (watch::Sender<Option<u16>>, Arc<Mutex<Vec<Notification>>>) {
    let (tx, rx) = watch::channel(Some(port));
    let collected: Arc<Mutex<Vec<Notification>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_store = collected.clone();
    let sink: NotifySink = Arc::new(move |n| sink_store.lock().unwrap().push(n));
    let book = Arc::new(Mutex::new(SessionBook::default()));
    let mux_book = book.clone();
    let mux_handler: FrameHandler = Arc::new(move |frame, sink| {
        handle_mux_frame(frame, sink, &mux_book);
    });
    let host_book = book.clone();
    let host_handler: FrameHandler = Arc::new(move |frame, _| {
        handle_host_frame(frame, &host_book);
    });
    tokio::spawn(
        Box::new(WsSource {
            path: "/api/events.mux",
            handler: mux_handler,
            on_connect: None,
        })
        .run(sink.clone(), rx.clone()),
    );
    tokio::spawn(
        Box::new(WsSource {
            path: "/api/events.host",
            handler: host_handler,
            on_connect: None,
        })
        .run(sink, rx),
    );
    (tx, collected)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_source_receives_filtered_events() {
    let port = free_port().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut child = spawn_fixture(port, work.path());
    let (_tx, collected) = spawn_sources(port);

    let got = wait_for(&collected, Duration::from_secs(15), |list| {
        list.iter()
            .any(|n| matches!(n.kind, NotifyKind::Approval) && n.body.contains("批准"))
    })
    .await;

    let _ = child.kill();
    assert!(
        got,
        "15s 内未收到 approval/requested 通知，实际：{:?}",
        *collected.lock().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turn_completion_notifies_and_filters_subagent() {
    let port = free_port().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut child = spawn_fixture(port, work.path());
    let (_tx, collected) = spawn_sources(port);

    // fixture 每 2s 一轮：主会话干活回合(tool/call→completed)=任务完成
    // + 主会话纯回答回合(completed)=回答完成 + 主会话 aborted(静默)
    // + 子代理完成(过滤)
    let got = wait_for(&collected, Duration::from_secs(15), |list| {
        list.iter()
            .any(|n| matches!(n.kind, NotifyKind::TaskCompleted | NotifyKind::AnswerCompleted))
    })
    .await;
    // 再等两轮多，收集足够样本检查漏判/误判
    tokio::time::sleep(Duration::from_millis(4500)).await;

    let _ = child.kill();
    let list = collected.lock().unwrap();
    assert!(got, "15s 内未收到回合完成通知，实际：{:?}", *list);
    let tasks: Vec<_> = list
        .iter()
        .filter(|n| matches!(n.kind, NotifyKind::TaskCompleted))
        .collect();
    let answers: Vec<_> = list
        .iter()
        .filter(|n| matches!(n.kind, NotifyKind::AnswerCompleted))
        .collect();
    assert!(
        !tasks.is_empty() && tasks.iter().all(|n| n.body == "「fx 主会话」任务完成"),
        "干活回合应全部报任务完成且带标题，实际：{:?}",
        tasks
    );
    assert!(
        !answers.is_empty() && answers.iter().all(|n| n.body == "「fx 主会话」回答完成"),
        "纯回答回合应全部报回答完成且带标题，实际：{:?}",
        answers
    );
    assert!(
        !list.iter().any(|n| n.body.contains("子代理")),
        "子代理会话不应触发通知，实际：{:?}",
        *list
    );
}
