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
async fn ready_logs_boot_timing_breakdown() {
    // Ready 时必须落一条耗时分解日志（诊断面板"上次启动"行的数据源）：
    // [dshdesktop] ready: port=N total=Xs http=Ys token=Zs
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
    wait_event(
        &events,
        |e| matches!(e, ProcessEvent::Log(l) if l.contains("[dshdesktop] ready: port=")),
        Duration::from_secs(5),
    );
    let lines: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            ProcessEvent::Log(l) => Some(l.clone()),
            _ => None,
        })
        .collect();
    let line = lines
        .iter()
        .find(|l| l.contains("[dshdesktop] ready: port="))
        .expect("ready 分解行必须存在");
    let dto = dshdesktop_lib::diagnostics::parse_boot_timing(line)
        .unwrap_or_else(|| panic!("分解行必须可解析：{line}"));
    assert_eq!(dto.port, port, "分解行端口须与 Ready 端口一致：{line}");
    assert!(dto.total_s > 0.0, "total 必须为正：{line}");
    assert!(dto.total_s >= dto.http_s, "total 必须覆盖 http 段：{line}");
    assert!(dto.total_s >= dto.token_s, "total 必须覆盖 token 段：{line}");
    proc.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npm_cold_install_gets_extended_token_budget() {
    // npm 冷装（npx 未缓存包）可能静默下载数分钟：见到 "will be installed" 警告行后
    // wait_token 必须切到长预算，不能再按普通静默预算杀树（否则快装完的进程被杀、
    // 白等一轮再靠缓存余温重启——0.5.5 实踩）
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("fake-dsh.npm-install-warn"), "").unwrap();
    // 警告行后静默 4s 才打 token：超过普通静默预算（1.5s），低于冷装预算（8s）
    std::fs::write(work.path().join("fake-dsh.token-delay"), "4000").unwrap();
    let (events, emit) = collect_events();
    let (token_tx, _token_rx) = watch::channel::<Option<Arc<str>>>(None);
    let proc = DshProcess::spawn_supervised_with_timeouts(
        Arc::new(TestPlatform),
        fixture_paths(work.path()),
        token_tx,
        emit,
        dshdesktop_lib::process::BootTimeouts {
            silence: Duration::from_millis(1500),
            install: Duration::from_secs(8),
            absolute: Duration::from_secs(30),
        },
    );
    wait_for_state(&proc, |s| matches!(s, DshState::Ready { .. }), Duration::from_secs(20));
    // 全程不得出现"未出现就绪 URL"的杀树记录
    let killed = events.lock().unwrap().iter().any(|e| {
        matches!(e, ProcessEvent::Log(l) if l.contains("未出现就绪 URL"))
    });
    assert!(!killed, "冷装模式下不得按普通静默预算杀树");
    proc.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn silent_boot_still_killed_after_silence_budget() {
    // 无冷装信号的纯静默卡死：超过静默预算照样杀树重试（旧行为保留——真卡死不能干等）
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("fake-dsh.token-delay"), "8000").unwrap();
    let (events, emit) = collect_events();
    let (token_tx, _token_rx) = watch::channel::<Option<Arc<str>>>(None);
    let proc = DshProcess::spawn_supervised_with_timeouts(
        Arc::new(TestPlatform),
        fixture_paths(work.path()),
        token_tx,
        emit,
        dshdesktop_lib::process::BootTimeouts {
            silence: Duration::from_millis(1500),
            install: Duration::from_secs(8),
            absolute: Duration::from_secs(30),
        },
    );
    // token 8s 才打，静默预算 1.5s → 第一轮必被杀并重试
    wait_event(
        &events,
        |e| matches!(e, ProcessEvent::Log(l) if l.contains("未出现就绪 URL")),
        Duration::from_secs(15),
    );
    proc.stop().await;
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

/// 回归（0.5.1 实踩）：dsh 重启后旧进程的 launch token 不得被当成新进程的用——
/// token 按进程轮换，若 Ready 门控读到缓存旧 token，主窗口会带旧 token 导航落
/// 401 页、mux 凭据交换 401 死循环。第二进程给就绪行加 3s 延迟拉开窗口：
/// 拿旧 token 抢跑的 Ready 会在 token 打印前发出，断言立刻抓到。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_replaces_stale_token() {
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("fake-dsh.token"), "token-alpha-aaaa").unwrap();
    let (events, emit) = collect_events();
    let (token_tx, token_rx) = watch::channel::<Option<Arc<str>>>(None);
    let proc = DshProcess::spawn_supervised(
        Arc::new(TestPlatform),
        fixture_paths(work.path()),
        token_tx,
        emit,
    );
    wait_for_state(&proc, |s| matches!(s, DshState::Ready { .. }), Duration::from_secs(30));
    assert_eq!(proc.token().as_deref(), Some("token-alpha-aaaa"));

    std::fs::write(work.path().join("fake-dsh.token"), "token-bravo-bbbb").unwrap();
    std::fs::write(work.path().join("fake-dsh.token-delay"), "3000").unwrap();
    proc.restart().await;
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
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        proc.token().as_deref(),
        Some("token-bravo-bbbb"),
        "第二次 Ready 发出时 token 必须是新进程的（旧 token 抢跑即回归）"
    );
    assert_eq!(
        token_rx.borrow().as_deref(),
        Some("token-bravo-bbbb"),
        "token 广播端同样不得残留旧 token"
    );
    proc.stop().await;
    wait_for_state(&proc, |s| matches!(s, DshState::Stopped), Duration::from_secs(15));
}
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
