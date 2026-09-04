use dshdesktop_lib::platform::Platform;
use dshdesktop_lib::process::{DshProcess, DshState, ProcessEvent};
use dshdesktop_lib::runtime::RuntimePaths;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

struct TestPlatform;

impl Platform for TestPlatform {
    fn node_exe_name(&self) -> &'static str {
        "node.exe"
    }
    fn cloudflared_exe_name(&self) -> &'static str {
        "cloudflared.exe"
    }
    fn runtime_base_dir(&self) -> PathBuf {
        PathBuf::from(".")
    }
    fn resource_runtime_dir(&self, _: &Path) -> PathBuf {
        PathBuf::from(".")
    }
    fn runtime_triplet(&self) -> &'static str {
        "windows-x64"
    }
    fn kill_process_tree(&self, pid: u32) {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    fn system_dark_mode(&self) -> bool {
        false
    }
    fn system_prefers_chinese(&self) -> bool {
        true
    }
    fn play_sound_file(
        &self,
        _path: &Path,
        _diag: Option<dshdesktop_lib::platform::SoundDiag>,
    ) -> Result<(), String> {
        Ok(())
    }
}

fn system_node() -> PathBuf {
    let out = std::process::Command::new("where").arg("node").output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    PathBuf::from(stdout.lines().next().expect("node not found on PATH").trim())
}

fn fixture_paths(work: &Path) -> RuntimePaths {
    RuntimePaths {
        node_exe: system_node(),
        dsh_bin: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("fake-dsh.cjs"),
        home: work.join("home"),
        work_dir: work.to_path_buf(),
        cloudflared_exe: work.join("cloudflared.exe"),
    }
}

type Events = Arc<Mutex<Vec<ProcessEvent>>>;

fn collect_events() -> (Events, impl Fn(ProcessEvent) + Send + Sync + 'static) {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    (events, move |e| ev.lock().unwrap().push(e))
}

fn wait_for_state(p: &DshProcess, pred: impl Fn(&DshState) -> bool, timeout: Duration) -> DshState {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let s = p.state();
        if pred(&s) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("state not reached within {:?}; last: {:?}", timeout, p.state());
}

fn wait_event(events: &Events, pred: impl Fn(&ProcessEvent) -> bool, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if events.lock().unwrap().iter().any(&pred) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("event not seen within {timeout:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dsh_becomes_ready_and_stops() {
    let work = tempfile::tempdir().unwrap();
    let (events, emit) = collect_events();
    let (token_tx, _token_rx) = watch::channel::<Option<Arc<str>>>(None);
    let proc = DshProcess::spawn_supervised(
        Arc::new(TestPlatform),
        fixture_paths(work.path()),
        token_tx,
        emit,
    );
    let s = wait_for_state(&proc, |s| matches!(s, DshState::Ready { .. }), Duration::from_secs(30));
    let DshState::Ready { port } = s else { unreachable!() };
    let resp = reqwest::get(format!("http://127.0.0.1:{port}/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(proc.pid().is_some());
    wait_event(&events, |e| matches!(e, ProcessEvent::Log(l) if l.contains("listening")), Duration::from_secs(5));
    proc.stop().await;
    wait_for_state(&proc, |s| matches!(s, DshState::Stopped), Duration::from_secs(15));
    assert!(proc.pid().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dsh_restarts_after_crash() {
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("fake-dsh.exit-after"), "1500").unwrap();
    let (events, emit) = collect_events();
    let (token_tx, _token_rx) = watch::channel::<Option<Arc<str>>>(None);
    let proc = DshProcess::spawn_supervised(
        Arc::new(TestPlatform),
        fixture_paths(work.path()),
        token_tx,
        emit,
    );
    wait_for_state(&proc, |s| matches!(s, DshState::Ready { .. }), Duration::from_secs(30));
    // 等崩溃发生
    wait_event(&events, |e| matches!(e, ProcessEvent::Log(l) if l.contains("dsh exited")), Duration::from_secs(30));
    // 等第二次 Ready（数两个 Ready 事件）
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ready_count = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, ProcessEvent::StateChanged(DshState::Ready { .. })))
            .count();
        if ready_count >= 2 {
            break;
        }
        assert!(Instant::now() < deadline, "no second Ready within 30s");
        std::thread::sleep(Duration::from_millis(50));
    }
    let port = proc.port().expect("should be Ready again");
    let resp = reqwest::get(format!("http://127.0.0.1:{port}/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    proc.stop().await;
    wait_for_state(&proc, |s| matches!(s, DshState::Stopped), Duration::from_secs(15));
}

/// 0.1.2 契约：Ready 态发出时 token 必已捕获（导航要带 ?token= 才能过 BrowserAuth），
/// 且原始就绪行里的 token 不得进入日志事件（链接即凭据）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ready_implies_token_captured_and_logs_redacted() {
    let work = tempfile::tempdir().unwrap();
    let (events, emit) = collect_events();
    let (token_tx, _token_rx) = watch::channel::<Option<Arc<str>>>(None);
    let proc = DshProcess::spawn_supervised(
        Arc::new(TestPlatform),
        fixture_paths(work.path()),
        token_tx,
        emit,
    );
    wait_for_state(&proc, |s| matches!(s, DshState::Ready { .. }), Duration::from_secs(30));
    assert_eq!(
        proc.token().as_deref(),
        Some("fixture-token-0123456789abcdef"),
        "Ready 时 token 必须已捕获（fake-dsh 就绪行）"
    );
    let logs: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            ProcessEvent::Log(l) => Some(l.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !logs.iter().any(|l| l.contains("fixture-token-0123456789abcdef")),
        "日志事件不得含明文 token，实际：{logs:?}"
    );
    assert!(
        logs.iter().any(|l| l.contains("token=<redacted>")),
        "就绪行应脱敏后仍进日志，实际：{logs:?}"
    );
    proc.stop().await;
    wait_for_state(&proc, |s| matches!(s, DshState::Stopped), Duration::from_secs(15));
}
