//! cloudflared quick tunnel 监督：spawn、URL 解析、指数退避重启、杀树停止。
//! 结构对齐 process.rs 的 DshProcess，差异：就绪信号不是端口可达，而是 stdout 里
//! 出现 trycloudflare URL（quick tunnel 的 URL 印在日志横幅里）；每次重启 URL 都会变。
//!
//! 常驻模式（persistent=true，远程会话持久化用）：spawn 时**不挂** KILL_ON_JOB_CLOSE
//! Job——防孤儿原则的唯一例外，刻意让隧道比应用进程活得久（应用退出/覆盖更新后
//! 域名不变，下次启动由 adopt 收养复活）。常驻隧道必须从数据目录副本运行（不从
//! 安装目录，否则 NSIS 覆写冲突+按路径清扫会杀它），副本管理在 session.rs。
//! adopt 收养外部存活的隧道进程：无 Child 句柄，看门狗按 PID 轮询探活，死后
//! 衔接正常监督循环退避重生（新域名）；stop 时杀树并轮询等死。
//!
//! 测试注入：`prefix_args` 在生产为空；测试传 fixture 脚本路径，exe 用 node。

use crate::platform::Platform;
use regex::Regex;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_FAILURES: u32 = 5;
/// quick tunnel 通常 10s 内出 URL；给 60s 兜底，超时杀树重启
const UP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelState {
    Starting,
    Up { url: String },
    Failed(String),
    Stopped,
}

#[derive(Debug, Clone)]
pub enum TunnelEvent {
    StateChanged(TunnelState),
    Log(String),
}

/// 从 cloudflared 日志行解析 quick tunnel URL
pub(crate) fn parse_tunnel_url(line: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"https://[a-z0-9-]+\.trycloudflare\.com").unwrap());
    re.find(line).map(|m| m.as_str().to_string())
}

struct Inner {
    platform: Arc<dyn Platform>,
    exe: PathBuf,
    prefix_args: Vec<String>,
    target: String,
    work_dir: PathBuf,
    /// 常驻模式：不挂 Job Object，隧道刻意比应用进程活得久（见模块头注释）
    persistent: bool,
    state: Mutex<TunnelState>,
    pid: AtomicU32,
    on_event: Box<dyn Fn(TunnelEvent) + Send + Sync>,
    shutdown: AtomicBool,
    stop: Notify,
    /// stdout 泵发现隧道 URL 时通知监督循环进入稳态
    up: Notify,
}

#[derive(Clone)]
pub struct TunnelProcess {
    inner: Arc<Inner>,
}

impl TunnelProcess {
    pub fn spawn_supervised(
        platform: Arc<dyn Platform>,
        exe: PathBuf,
        prefix_args: Vec<String>,
        target: String,
        work_dir: PathBuf,
        persistent: bool,
        events: impl Fn(TunnelEvent) + Send + Sync + 'static,
    ) -> Self {
        let this = Self {
            inner: Arc::new(Inner {
                platform,
                exe,
                prefix_args,
                target,
                work_dir,
                persistent,
                state: Mutex::new(TunnelState::Starting),
                pid: AtomicU32::new(0),
                on_event: Box::new(events),
                shutdown: AtomicBool::new(false),
                stop: Notify::new(),
                up: Notify::new(),
            }),
        };
        let runner = this.clone();
        tokio::spawn(async move { runner.supervise_loop().await });
        this
    }

    /// 收养上个应用会话遗留的存活隧道进程：立即 Up（URL 为收养值），看门狗按
    /// PID 轮询探活（无 Child 句柄可 wait），死后衔接 supervise_loop 退避重生。
    /// 调用侧必须先校验 pid 镜像路径与记录一致（防 PID 复用收养错进程）。
    #[allow(clippy::too_many_arguments)]
    pub fn adopt(
        platform: Arc<dyn Platform>,
        pid: u32,
        url: String,
        exe: PathBuf,
        prefix_args: Vec<String>,
        target: String,
        work_dir: PathBuf,
        persistent: bool,
        events: impl Fn(TunnelEvent) + Send + Sync + 'static,
    ) -> Self {
        let this = Self {
            inner: Arc::new(Inner {
                platform,
                exe,
                prefix_args,
                target,
                work_dir,
                persistent,
                state: Mutex::new(TunnelState::Up { url }),
                pid: AtomicU32::new(pid),
                on_event: Box::new(events),
                shutdown: AtomicBool::new(false),
                stop: Notify::new(),
                up: Notify::new(),
            }),
        };
        let runner = this.clone();
        tokio::spawn(async move { runner.adopt_watch_loop().await });
        this
    }

    pub fn state(&self) -> TunnelState {
        self.inner.state.lock().unwrap().clone()
    }

    /// 当前隧道进程 PID（收养模式下为被收养进程；0 = 未运行）。状态落盘用。
    pub fn current_pid(&self) -> u32 {
        self.inner.pid.load(Ordering::SeqCst)
    }

    /// 收养看门狗：3s 轮询探活；死后掉进 supervise_loop 重生；stop 时杀树等死。
    async fn adopt_watch_loop(&self) {
        loop {
            if self.inner.shutdown.load(Ordering::SeqCst) {
                return;
            }
            let pid = self.inner.pid.load(Ordering::SeqCst);
            if pid == 0 || !self.inner.platform.process_alive(pid) {
                self.log("[dshdesktop] adopted tunnel process gone, respawning");
                self.inner.pid.store(0, Ordering::SeqCst);
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                _ = self.inner.stop.notified() => {
                    let pid = self.inner.pid.swap(0, Ordering::SeqCst);
                    if pid != 0 {
                        self.inner.platform.kill_process_tree(pid);
                        // 无 Child 句柄，轮询等死（GetExitCodeProcess 版探活在
                        // 对象被引用时也能给出真实退出码，不依赖句柄全释放）
                        let t0 = std::time::Instant::now();
                        while self.inner.platform.process_alive(pid)
                            && t0.elapsed() < Duration::from_secs(3)
                        {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                    self.set_state(TunnelState::Stopped);
                    return;
                }
            }
        }
        // supervise_loop 开头即 set_state(Starting) + 重新 spawn，天然衔接重生
        self.supervise_loop().await
    }

    pub async fn stop(&self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.inner.stop.notify_one();
    }

    async fn supervise_loop(&self) {
        let mut failures = 0u32;
        let mut backoff = Duration::from_millis(500);
        loop {
            if self.inner.shutdown.load(Ordering::SeqCst) {
                break;
            }
            self.set_state(TunnelState::Starting);
            self.log(format!(
                "[dshdesktop] starting cloudflared tunnel --url {} (exe={})",
                self.inner.target,
                self.inner.exe.display()
            ));
            let mut cmd = Command::new(&self.inner.exe);
            cmd.args(&self.inner.prefix_args)
                .arg("tunnel")
                .arg("--url")
                .arg(&self.inner.target)
                .arg("--no-autoupdate")
                .current_dir(&self.inner.work_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            self.inner.platform.configure_child_command(&mut cmd);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    self.set_state(TunnelState::Failed(format!("spawn failed: {e}")));
                    break;
                }
            };
            let pid = child.id().unwrap_or(0);
            // 非常驻才挂 KILL_ON_JOB_CLOSE Job：本进程被强杀时 cloudflared 由内核
            // 连带回收，否则孤儿进程锁住 runtime 目录导致卸载重装失败。
            // 常驻模式（会话持久化）恰恰相反——刻意让它活得比应用久。
            if !self.inner.persistent {
                self.inner.platform.register_child(pid);
            }
            self.inner.pid.store(pid, Ordering::SeqCst);
            if let Some(out) = child.stdout.take() {
                self.spawn_pump(out);
            }
            if let Some(err) = child.stderr.take() {
                self.spawn_pump(err);
            }

            // 阶段一：等 URL 出现（stdout 泵负责解析并 up.notify）或超时
            tokio::select! {
                _ = self.inner.up.notified() => {
                    failures = 0;
                    backoff = Duration::from_millis(500);
                }
                _ = tokio::time::sleep(UP_TIMEOUT) => {
                    self.log("[dshdesktop] tunnel url not seen within 60s, killing");
                    self.inner.platform.kill_process_tree(pid);
                    let _ = child.wait().await;
                    self.inner.pid.store(0, Ordering::SeqCst);
                    failures += 1;
                    if failures >= MAX_FAILURES {
                        self.set_state(TunnelState::Failed("cloudflared did not report a tunnel url".into()));
                        break;
                    }
                    if !self.wait_backoff(&mut backoff).await {
                        break;
                    }
                    continue;
                }
                status = child.wait() => {
                    self.log(format!("[dshdesktop] cloudflared exited before up: {status:?}"));
                    self.inner.pid.store(0, Ordering::SeqCst);
                    failures += 1;
                    if failures >= MAX_FAILURES {
                        self.set_state(TunnelState::Failed("cloudflared crashed repeatedly".into()));
                        break;
                    }
                    if !self.wait_backoff(&mut backoff).await {
                        break;
                    }
                    continue;
                }
                _ = self.inner.stop.notified() => {
                    self.kill_and_wait(pid, &mut child).await;
                    self.set_state(TunnelState::Stopped);
                    return;
                }
            }

            // 阶段二：稳态（URL 已上报），守进程退出
            tokio::select! {
                status = child.wait() => {
                    self.log(format!("[dshdesktop] cloudflared exited: {status:?}"));
                }
                _ = self.inner.stop.notified() => {
                    self.kill_and_wait(pid, &mut child).await;
                    self.set_state(TunnelState::Stopped);
                    return;
                }
            }
            self.inner.pid.store(0, Ordering::SeqCst);

            if self.inner.shutdown.load(Ordering::SeqCst) {
                self.set_state(TunnelState::Stopped);
                break;
            }
            failures += 1;
            if failures >= MAX_FAILURES {
                self.set_state(TunnelState::Failed("too many consecutive crashes".into()));
                break;
            }
            self.log(format!(
                "[dshdesktop] restarting tunnel in {}ms",
                backoff.as_millis()
            ));
            if !self.wait_backoff(&mut backoff).await {
                break;
            }
        }
    }

    async fn kill_and_wait(&self, pid: u32, child: &mut tokio::process::Child) {
        self.inner.platform.kill_process_tree(pid);
        let _ = child.wait().await;
        self.inner.pid.store(0, Ordering::SeqCst);
    }

    /// 退避等待，期间响应 stop。返回 false 表示收到 stop，循环应终止。
    async fn wait_backoff(&self, backoff: &mut Duration) -> bool {
        let current = *backoff;
        *backoff = (current * 2).min(MAX_BACKOFF);
        tokio::select! {
            _ = tokio::time::sleep(current) => true,
            _ = self.inner.stop.notified() => {
                self.set_state(TunnelState::Stopped);
                false
            }
        }
    }

    fn spawn_pump<R: AsyncRead + Unpin + Send + 'static>(&self, reader: R) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // URL 只在未 Up 时解析上报；稳态下的重复横幅不打扰状态
                let already_up = matches!(*inner.state.lock().unwrap(), TunnelState::Up { .. });
                if !already_up {
                    if let Some(url) = parse_tunnel_url(&line) {
                        *inner.state.lock().unwrap() = TunnelState::Up { url: url.clone() };
                        (inner.on_event)(TunnelEvent::StateChanged(TunnelState::Up { url }));
                        inner.up.notify_one();
                    }
                }
                (inner.on_event)(TunnelEvent::Log(line));
            }
        });
    }

    fn set_state(&self, s: TunnelState) {
        *self.inner.state.lock().unwrap() = s.clone();
        (self.inner.on_event)(TunnelEvent::StateChanged(s));
    }

    fn log(&self, line: impl Into<String>) {
        (self.inner.on_event)(TunnelEvent::Log(line.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::parse_tunnel_url;

    #[test]
    fn parses_url_lines() {
        assert_eq!(
            parse_tunnel_url(
                "2026-08-17T00:00:00Z INF |  https://abc-def-123.trycloudflare.com  |"
            ),
            Some("https://abc-def-123.trycloudflare.com".into())
        );
        assert_eq!(
            parse_tunnel_url("Visit it at https://a-b-c-d-e-f.trycloudflare.com now"),
            Some("https://a-b-c-d-e-f.trycloudflare.com".into())
        );
        assert_eq!(parse_tunnel_url("INF no url here"), None);
        assert_eq!(parse_tunnel_url("https://example.com"), None);
    }
}
