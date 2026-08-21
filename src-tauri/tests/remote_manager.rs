use dshdesktop_lib::platform::Platform;
use dshdesktop_lib::port::free_port;
use dshdesktop_lib::remote::{compose_link, RemoteEvent, RemoteManager, RemoteStatus};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
        false
    }
    fn play_sound_file(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }
}

fn system_node() -> PathBuf {
    let out = Command::new("where").arg("node").output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    PathBuf::from(
        stdout
            .lines()
            .next()
            .expect("node not found on PATH")
            .trim(),
    )
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// 直连（绕开系统代理）GET，供 fixture 就绪探测用
async fn get_direct(url: &str) -> reqwest::Result<reqwest::Response> {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
}

fn spawn_fixture_dsh(port: u16, work: &Path) -> std::process::Child {
    Command::new(system_node())
        .arg(fixture("fake-dsh.cjs"))
        .arg("web")
        .arg("--port")
        .arg(port.to_string())
        .current_dir(work)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn make_manager(
    tunnel_exe: PathBuf,
    tunnel_prefix: Vec<String>,
    work: &Path,
    dsh_port: Option<u16>,
) -> (RemoteManager, Arc<Mutex<Vec<RemoteStatus>>>) {
    let (_tx, rx) = watch::channel(dsh_port);
    let statuses: Arc<Mutex<Vec<RemoteStatus>>> = Arc::new(Mutex::new(Vec::new()));
    let st = statuses.clone();
    let mgr = RemoteManager::new(
        Arc::new(TestPlatform),
        tunnel_exe,
        tunnel_prefix,
        work.to_path_buf(),
        work.to_path_buf(), // dsh_home：本套件不经项目端点，复用 work 即可
        rx,
        Box::new(move |ev| {
            if let RemoteEvent::Status(s) = ev {
                st.lock().unwrap().push(s);
            }
        }),
    );
    (mgr, statuses)
}

async fn wait_phase(mgr: &RemoteManager, phase: &str, timeout: Duration) -> RemoteStatus {
    let deadline = Instant::now() + timeout;
    loop {
        let s = mgr.status();
        if s.phase == phase {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "{phase} 未在 {timeout:?} 内到达，当前：{:?}",
            mgr.status()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
fn compose_link_appends_token() {
    assert_eq!(
        compose_link("https://x-y-z.trycloudflare.com", "abc123"),
        "https://x-y-z.trycloudflare.com/?token=abc123"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_without_cloudflared_errors() {
    let work = tempfile::tempdir().unwrap();
    let (mgr, _statuses) = make_manager(
        work.path().join("no-such-cloudflared.exe"),
        vec![],
        work.path(),
        None,
    );
    let s = mgr.start().await;
    assert_eq!(s.phase, "error");
    assert!(
        s.error.unwrap().contains("cloudflared"),
        "错误应指出缺失文件"
    );
    assert!(s.link.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_chain_up_then_off() {
    // fixture dsh + 假 cloudflared（node 跑 fake-cloudflared.cjs）打通除真隧道外全链路
    let dsh_port = free_port().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut dsh = spawn_fixture_dsh(dsh_port, work.path());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(r) = get_direct(&format!("http://127.0.0.1:{dsh_port}/")).await {
            if r.status().is_success() {
                break;
            }
        }
        assert!(Instant::now() < deadline, "fixture dsh 15s 内未就绪");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let (mgr, _statuses) = make_manager(
        system_node(),
        vec![fixture("fake-cloudflared.cjs")
            .to_string_lossy()
            .into_owned()],
        work.path(),
        Some(dsh_port),
    );
    mgr.start().await;
    let s = wait_phase(&mgr, "up", Duration::from_secs(30)).await;
    let link = s.link.as_deref().unwrap().to_string();
    assert!(
        link.contains(".trycloudflare.com/?token="),
        "链接形态不对：{link}"
    );
    let proxy_port = s.proxy_port.expect("Up 时应有代理端口");

    // 复现浏览器首次点击：带 token 访问代理 → 302 + cookie → 带 cookie 拿到 dsh 内容
    let token = link.rsplit("?token=").next().unwrap();
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // 测试环境可能设了系统代理（HTTP_PROXY），访问 127.0.0.1 必须直连
        .no_proxy()
        .build()
        .unwrap();
    let r = http
        .get(format!("http://127.0.0.1:{proxy_port}/?token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);
    let r = http
        .get(format!("http://127.0.0.1:{proxy_port}/"))
        .header("cookie", format!("__dsh_remote={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok");

    // 幂等 start：不改变 token/链接
    let s2 = mgr.start().await;
    assert_eq!(
        s2.link.as_deref(),
        Some(link.as_str()),
        "重复 start 不应换 token"
    );

    let s3 = mgr.stop().await;
    assert_eq!(s3.phase, "off");
    assert!(s3.link.is_none());
    // stop 后代理应已关停：换新客户端（旧客户端的 keep-alive 连接会复用成功，不代表在服务）
    let fresh = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let refused = loop {
        match fresh
            .get(format!("http://127.0.0.1:{proxy_port}/"))
            .header("cookie", format!("__dsh_remote={token}"))
            .send()
            .await
        {
            Err(_) => break true,
            Ok(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await
            }
            Ok(_) => break false,
        }
    };
    assert!(refused, "stop 后代理不应再响应新连接");

    let _ = dsh.kill();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reset_link_requires_running() {
    let work = tempfile::tempdir().unwrap();
    let (mgr, _statuses) = make_manager(
        work.path().join("no-such-cloudflared.exe"),
        vec![],
        work.path(),
        None,
    );
    assert!(mgr.reset_link().is_err(), "off 态重置应报错");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reset_link_rotates_token_keeps_url() {
    let dsh_port = free_port().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut dsh = spawn_fixture_dsh(dsh_port, work.path());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(r) = get_direct(&format!("http://127.0.0.1:{dsh_port}/")).await {
            if r.status().is_success() {
                break;
            }
        }
        assert!(Instant::now() < deadline, "fixture dsh 15s 内未就绪");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let (mgr, _statuses) = make_manager(
        system_node(),
        vec![fixture("fake-cloudflared.cjs")
            .to_string_lossy()
            .into_owned()],
        work.path(),
        Some(dsh_port),
    );
    mgr.start().await;
    let s = wait_phase(&mgr, "up", Duration::from_secs(30)).await;
    let old_link = s.link.unwrap();
    let old_token = old_link.rsplit("?token=").next().unwrap().to_string();
    let proxy_port = s.proxy_port.unwrap();

    // 重置：token 轮换、域名与代理端口不变
    let s2 = mgr.reset_link().expect("Up 态重置应成功");
    assert_eq!(s2.phase, "up");
    let new_link = s2.link.unwrap();
    assert_eq!(
        new_link.split("?token=").next(),
        old_link.split("?token=").next(),
        "重置不应换隧道域名"
    );
    assert_eq!(s2.proxy_port, Some(proxy_port), "重置不应重启代理");
    let new_token = new_link.rsplit("?token=").next().unwrap().to_string();
    assert_ne!(old_token, new_token, "token 必须轮换");

    // 门岗即刻生效：旧凭据 403，新凭据放行
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    let r = http
        .get(format!("http://127.0.0.1:{proxy_port}/"))
        .header("cookie", format!("__dsh_remote={old_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "旧 cookie 应立即失效");
    let r = http
        .get(format!("http://127.0.0.1:{proxy_port}/?token={new_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302, "新 token 应正常种 cookie");

    mgr.stop().await;
    let _ = dsh.kill();
}
