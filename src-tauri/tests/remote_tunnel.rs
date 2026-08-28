use dshdesktop_lib::platform::Platform;
use dshdesktop_lib::remote::tunnel::{TunnelEvent, TunnelProcess, TunnelState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
        false
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
    let out = std::process::Command::new("where")
        .arg("node")
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    PathBuf::from(
        stdout
            .lines()
            .next()
            .expect("node not found on PATH")
            .trim(),
    )
}

type Events = Arc<Mutex<Vec<TunnelEvent>>>;

fn spawn_fake(work: &Path) -> (TunnelProcess, Events) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-cloudflared.cjs");
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let proc = TunnelProcess::spawn_supervised(
        Arc::new(TestPlatform),
        system_node(),
        vec![fixture.to_string_lossy().into_owned()],
        "http://127.0.0.1:12345".into(),
        work.to_path_buf(),
        move |e| ev.lock().unwrap().push(e),
    );
    (proc, events)
}

fn wait_state(
    p: &TunnelProcess,
    pred: impl Fn(&TunnelState) -> bool,
    timeout: Duration,
) -> TunnelState {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let s = p.state();
        if pred(&s) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "state not reached within {timeout:?}; last: {:?}",
        p.state()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnel_reports_up_url_and_stops() {
    let work = tempfile::tempdir().unwrap();
    let (proc, _events) = spawn_fake(work.path());
    let s = wait_state(
        &proc,
        |s| matches!(s, TunnelState::Up { .. }),
        Duration::from_secs(30),
    );
    let TunnelState::Up { url } = s else {
        unreachable!()
    };
    assert!(
        url.starts_with("https://") && url.contains(".trycloudflare.com"),
        "应解析出隧道 URL，实际 {url}"
    );
    proc.stop().await;
    wait_state(
        &proc,
        |s| matches!(s, TunnelState::Stopped),
        Duration::from_secs(15),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnel_restarts_after_crash() {
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("fake-cloudflared.exit-after"), "1500").unwrap();
    let (proc, events) = spawn_fake(work.path());
    // 首轮 Up（URL 打印后 1.5s 才崩溃）
    wait_state(
        &proc,
        |s| matches!(s, TunnelState::Up { .. }),
        Duration::from_secs(30),
    );
    // 崩溃 → 退避 → 拉起第二轮 → 数到两个 Up 事件
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let up_count = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, TunnelEvent::StateChanged(TunnelState::Up { .. })))
            .count();
        if up_count >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "30s 内未见第二次 Up（崩溃后未重启）"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    proc.stop().await;
    wait_state(
        &proc,
        |s| matches!(s, TunnelState::Stopped),
        Duration::from_secs(15),
    );
}
