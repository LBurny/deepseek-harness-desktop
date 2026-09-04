use dshdesktop_lib::platform::Platform;
use dshdesktop_lib::port::free_port;
use dshdesktop_lib::remote::{compose_link, RemoteEvent, RemoteManager, RemoteStatus};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

mod support;

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
    make_manager_with_platform(Arc::new(TestPlatform), tunnel_exe, tunnel_prefix, work, dsh_port)
}

/// 收养/复活路径必须用真平台：TestPlatform 桩的 process_alive 恒 false，
/// 会把存活的被收养隧道误判死
fn make_manager_real(
    tunnel_exe: PathBuf,
    tunnel_prefix: Vec<String>,
    work: &Path,
    dsh_port: Option<u16>,
) -> (RemoteManager, Arc<Mutex<Vec<RemoteStatus>>>) {
    make_manager_with_platform(
        Arc::from(dshdesktop_lib::platform::current()),
        tunnel_exe,
        tunnel_prefix,
        work,
        dsh_port,
    )
}

fn make_manager_with_platform(
    platform: Arc<dyn Platform>,
    tunnel_exe: PathBuf,
    tunnel_prefix: Vec<String>,
    work: &Path,
    dsh_port: Option<u16>,
) -> (RemoteManager, Arc<Mutex<Vec<RemoteStatus>>>) {
    // 0.1.2 起 RemoteManager 拿的是 DshCreds（端口+token）：fixture dsh 无鉴权门，
    // token 随便给（cookie 交换失败只影响注入，不影响转发）
    let creds = dsh_port.map(|port| {
        Arc::new(dshdesktop_lib::dsh_session::DshCreds {
            port,
            token: support::FIXTURE_TOKEN.into(),
        })
    });
    let (_tx, rx) = watch::channel(creds);
    let statuses: Arc<Mutex<Vec<RemoteStatus>>> = Arc::new(Mutex::new(Vec::new()));
    let st = statuses.clone();
    let mgr = RemoteManager::new(
        platform,
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

/// 等 fixture dsh 就绪（15s 超时）
async fn wait_dsh_ready(dsh_port: u16) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(r) = get_direct(&format!("http://127.0.0.1:{dsh_port}/")).await {
            if r.status().is_success() {
                return;
            }
        }
        assert!(Instant::now() < deadline, "fixture dsh 15s 内未就绪");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
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
    wait_dsh_ready(dsh_port).await;

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
    wait_dsh_ready(dsh_port).await;

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

/// 会话文件生命周期：start→up 落盘（字段与 status 一致）→ reset_link 更新 token
/// （url/pid 不变）→ stop 删除文件。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_file_lifecycle() {
    use dshdesktop_lib::remote::session::{self, SessionState};
    let dsh_port = free_port().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut dsh = spawn_fixture_dsh(dsh_port, work.path());
    wait_dsh_ready(dsh_port).await;

    let (mgr, _statuses) = make_manager(
        system_node(),
        vec![fixture("fake-cloudflared.cjs")
            .to_string_lossy()
            .into_owned()],
        work.path(),
        Some(dsh_port),
    );
    let session_path = work.path().join("remote-session.json");
    assert!(!mgr.has_session(), "未开启前不应有会话文件");

    mgr.start().await;
    let s = wait_phase(&mgr, "up", Duration::from_secs(30)).await;
    let st: SessionState = session::load(&session_path).expect("Up 后状态文件应落盘");
    let link = s.link.as_deref().unwrap();
    let token = link.rsplit("?token=").next().unwrap();
    assert_eq!(st.token, token, "落盘 token 与链接一致");
    assert_eq!(st.tunnel_url, s.url.unwrap());
    assert_eq!(st.proxy_port, s.proxy_port.unwrap());
    assert_ne!(st.tunnel_pid, 0, "落盘应有隧道 PID");
    assert!(st.cloudflared_exe.is_file(), "落盘的常驻副本应存在");
    assert!(mgr.has_session());

    // 重置链接：状态文件 token 跟随轮换，url/pid 不变
    let s2 = mgr.reset_link().expect("Up 态重置应成功");
    let new_token = s2.link.unwrap().rsplit("?token=").next().unwrap().to_string();
    let st2 = session::load(&session_path).unwrap();
    assert_eq!(st2.token, new_token, "重置后落盘 token 应更新");
    assert_eq!(st2.tunnel_url, st.tunnel_url, "重置不动域名");
    assert_eq!(st2.tunnel_pid, st.tunnel_pid, "重置不动隧道进程");

    mgr.stop().await;
    assert!(!session_path.exists(), "stop 后状态文件应删除");
    let _ = dsh.kill();
}

/// 应用重启复活：手工 spawn 假隧道（模拟上个应用进程遗留的存活常驻隧道）+
/// 手写状态文件 → resume_or_start 收养 → phase=up、resumed=true、链接与落盘
/// 状态逐字节一致、门岗真实可用。stop 收尾会杀收养隧道并删文件。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_adopts_surviving_tunnel() {
    use dshdesktop_lib::remote::session::{self, SessionState};
    let dsh_port = free_port().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut dsh = spawn_fixture_dsh(dsh_port, work.path());
    wait_dsh_ready(dsh_port).await;

    let mut tunnel = std::process::Command::new(system_node())
        .arg(fixture("fake-cloudflared.cjs"))
        .current_dir(work.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let tunnel_pid = tunnel.id();
    // exe 必须填 node.exe 真实路径：resume 镜像校验比对的是进程镜像
    let token = "deadbeef".repeat(8);
    session::save(
        &work.path().join("remote-session.json"),
        &SessionState {
            token: token.clone(),
            proxy_port: free_port().unwrap(),
            tunnel_pid,
            tunnel_url: "https://abc-def-123.trycloudflare.com".into(),
            cloudflared_exe: system_node(),
        },
    )
    .unwrap();

    let (mgr, _statuses) = make_manager_real(
        system_node(),
        vec![fixture("fake-cloudflared.cjs")
            .to_string_lossy()
            .into_owned()],
        work.path(),
        Some(dsh_port),
    );
    assert!(mgr.has_session());
    mgr.begin_resume();
    let s = mgr.resume_or_start().await;
    assert_eq!(s.phase, "up", "复活应直接 up：{:?}", s.error);
    assert!(s.resumed, "复活 resumed=true");
    let link = s.link.unwrap();
    assert_eq!(
        link,
        format!("https://abc-def-123.trycloudflare.com/?token={token}"),
        "链接应与落盘状态逐字节一致"
    );
    // 门岗真实可用：带 token 访问代理 → 302 播种
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    let r = http
        .get(format!(
            "http://127.0.0.1:{}/?token={token}",
            s.proxy_port.unwrap()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);

    mgr.stop().await;
    assert!(!work.path().join("remote-session.json").exists());
    let _ = tunnel.kill();
    let _ = dsh.kill();
}

/// resume 回退：状态文件指向已死 PID → 全新开隧道（域名经 URL 覆写换新）、
/// token 沿用（只有手动重置才换）、resumed=false；新 Up 后状态文件重写。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_falls_back_when_tunnel_dead() {
    use dshdesktop_lib::remote::session::{self, SessionState};
    let dsh_port = free_port().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut dsh = spawn_fixture_dsh(dsh_port, work.path());
    wait_dsh_ready(dsh_port).await;

    // 已死 PID（真实存在过但已退出）
    let mut tmp = std::process::Command::new(system_node())
        .arg("-e")
        .arg("0")
        .spawn()
        .unwrap();
    let dead_pid = tmp.id();
    tmp.wait().unwrap();
    let token = "cafe01".repeat(8);
    session::save(
        &work.path().join("remote-session.json"),
        &SessionState {
            token: token.clone(),
            proxy_port: free_port().unwrap(),
            tunnel_pid: dead_pid,
            tunnel_url: "https://dead-link-000.trycloudflare.com".into(),
            cloudflared_exe: system_node(),
        },
    )
    .unwrap();
    // 重生换域名模拟（真实 quick tunnel 每次 spawn 域名都变）
    std::fs::write(
        work.path().join("fake-cloudflared.url"),
        "https://fresh-999.trycloudflare.com",
    )
    .unwrap();

    let (mgr, _statuses) = make_manager_real(
        system_node(),
        vec![fixture("fake-cloudflared.cjs")
            .to_string_lossy()
            .into_owned()],
        work.path(),
        Some(dsh_port),
    );
    mgr.begin_resume();
    let s0 = mgr.resume_or_start().await;
    assert_eq!(s0.phase, "starting", "回退全新开是异步的，先 starting");
    let s = wait_phase(&mgr, "up", Duration::from_secs(30)).await;
    let link = s.link.unwrap();
    assert!(
        link.starts_with("https://fresh-999.trycloudflare.com/"),
        "域名应换新：{link}"
    );
    assert!(
        link.ends_with(&format!("?token={token}")),
        "token 应沿用：{link}"
    );
    assert!(!s.resumed, "回退全新开 resumed=false");
    // 新 Up 已重写状态文件（新域名新 PID）
    let st = session::load(&work.path().join("remote-session.json")).unwrap();
    assert_eq!(st.tunnel_url, "https://fresh-999.trycloudflare.com");
    assert_eq!(st.token, token);
    mgr.stop().await;
    let _ = dsh.kill();
}
