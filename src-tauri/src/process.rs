use crate::diagnostics::LogRing;
use crate::platform::Platform;
use crate::port::{free_port, wait_ready};
use crate::runtime::RuntimePaths;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_FAILURES: u32 = 5;
const READY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshState {
    Starting,
    Ready { port: u16 },
    Failed(String),
    Stopped,
}

#[derive(Debug, Clone)]
pub enum ProcessEvent {
    StateChanged(DshState),
    Log(String),
}

struct Inner {
    platform: Arc<dyn Platform>,
    paths: RuntimePaths,
    state: Mutex<DshState>,
    pid: AtomicU32,
    log_ring: LogRing,
    on_event: Arc<dyn Fn(ProcessEvent) + Send + Sync>,
    shutdown: AtomicBool,
    stop: Notify,
    restart: Notify,
}

#[derive(Clone)]
pub struct DshProcess {
    inner: Arc<Inner>,
}

impl DshProcess {
    pub fn spawn_supervised(
        platform: Arc<dyn Platform>,
        paths: RuntimePaths,
        log_ring: LogRing,
        events: impl Fn(ProcessEvent) + Send + Sync + 'static,
    ) -> Self {
        let this = Self {
            inner: Arc::new(Inner {
                platform,
                paths,
                state: Mutex::new(DshState::Starting),
                pid: AtomicU32::new(0),
                log_ring,
                on_event: Arc::new(events),
                shutdown: AtomicBool::new(false),
                stop: Notify::new(),
                restart: Notify::new(),
            }),
        };
        let runner = this.clone();
        tokio::spawn(async move { runner.supervise_loop().await });
        this
    }

    pub fn state(&self) -> DshState {
        self.inner.state.lock().unwrap().clone()
    }

    pub fn port(&self) -> Option<u16> {
        match self.state() {
            DshState::Ready { port } => Some(port),
            _ => None,
        }
    }

    pub fn pid(&self) -> Option<u32> {
        match self.inner.pid.load(Ordering::SeqCst) {
            0 => None,
            p => Some(p),
        }
    }

    pub async fn stop(&self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.inner.stop.notify_one();
    }

    pub async fn restart(&self) {
        let alive = !matches!(self.state(), DshState::Failed(_) | DshState::Stopped);
        if alive {
            self.inner.restart.notify_one();
        } else {
            // 监督循环已退出（Failed/Stopped），重新拉起
            self.inner.shutdown.store(false, Ordering::SeqCst);
            let this = self.clone();
            tokio::spawn(async move { this.supervise_loop().await });
        }
    }

    async fn supervise_loop(&self) {
        let mut failures = 0u32;
        let mut backoff = Duration::from_millis(500);
        loop {
            if self.inner.shutdown.load(Ordering::SeqCst) {
                break;
            }
            self.set_state(DshState::Starting);
            let port = match free_port() {
                Ok(p) => p,
                Err(e) => {
                    self.set_state(DshState::Failed(format!("no free port: {e}")));
                    break;
                }
            };
            self.log(format!(
                "[dshdesktop] starting dsh web --port {port} (node={}, bin={}, cwd={})",
                self.inner.paths.node_exe.display(),
                self.inner.paths.dsh_bin.display(),
                self.inner.paths.work_dir.display()
            ));
            let mut cmd = Command::new(&self.inner.paths.node_exe);
            cmd.arg(&self.inner.paths.dsh_bin)
                .arg(crate::upstream::DSH_WEB_SUBCOMMAND)
                .arg(crate::upstream::DSH_PORT_FLAG)
                .arg(port.to_string())
                // dsh-web-app 默认把 Web UI 丢给系统默认浏览器；壳内嵌 WebView
                // 就是浏览器，必须抑制（否则每次启动额外弹浏览器标签页）
                .arg(crate::upstream::DSH_NO_OPEN_FLAG)
                .env("DSH_HOME", &self.inner.paths.home)
                // 子进程 PATH 前置两层：内嵌 node 目录（npx/npm/node 绑定运行时
                // 自带版本——dsh 派生的 MCP 命令常以 `npx` 配置，运行时若不带
                // npx.cmd 会落到系统 PATH 上的任意 node 版本，引擎不兼容即崩，
                // 机器 B 系统全局 node v16 实测）+ profile 的 node_modules/.bin
                //（插件自带 CLI 按名解析，否则装完 modlens 后终端敲不到）。
                .env(
                    "PATH",
                    dsh_child_path(
                        self.inner.paths.node_exe.parent().unwrap_or(Path::new(".")),
                        &self.inner.paths.home,
                        std::env::var_os("PATH"),
                    ),
                )
                .current_dir(&self.inner.paths.work_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            self.inner.platform.configure_child_command(&mut cmd);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    self.set_state(DshState::Failed(format!("spawn failed: {e}")));
                    break;
                }
            };
            let pid = child.id().unwrap_or(0);
            // 挂进 KILL_ON_JOB_CLOSE Job：本进程被强杀时 node 整树由内核连带回收
            self.inner.platform.register_child(pid);
            self.inner.pid.store(pid, Ordering::SeqCst);
            if let Some(out) = child.stdout.take() {
                self.spawn_pump(out);
            }
            if let Some(err) = child.stderr.take() {
                self.spawn_pump(err);
            }

            if wait_ready(port, READY_TIMEOUT).await {
                failures = 0;
                backoff = Duration::from_millis(500);
                self.set_state(DshState::Ready { port });
            } else {
                self.log("[dshdesktop] dsh not ready within 60s, killing");
                self.inner.platform.kill_process_tree(pid);
                let _ = child.wait().await;
                self.inner.pid.store(0, Ordering::SeqCst);
                failures += 1;
                if failures >= MAX_FAILURES {
                    self.set_state(DshState::Failed("dsh failed to become ready".into()));
                    break;
                }
                if !self.wait_backoff(&mut backoff).await {
                    break;
                }
                continue;
            }

            tokio::select! {
                status = child.wait() => {
                    self.log(format!("[dshdesktop] dsh exited: {status:?}"));
                }
                _ = self.inner.stop.notified() => {
                    self.inner.platform.kill_process_tree(pid);
                    let _ = child.wait().await;
                    self.inner.pid.store(0, Ordering::SeqCst);
                    self.set_state(DshState::Stopped);
                    return;
                }
                _ = self.inner.restart.notified() => {
                    self.inner.platform.kill_process_tree(pid);
                    let _ = child.wait().await;
                    self.inner.pid.store(0, Ordering::SeqCst);
                    continue;
                }
            }
            self.inner.pid.store(0, Ordering::SeqCst);

            if self.inner.shutdown.load(Ordering::SeqCst) {
                self.set_state(DshState::Stopped);
                break;
            }
            failures += 1;
            if failures >= MAX_FAILURES {
                self.set_state(DshState::Failed("too many consecutive crashes".into()));
                break;
            }
            self.log(format!("[dshdesktop] restarting in {}ms", backoff.as_millis()));
            if !self.wait_backoff(&mut backoff).await {
                break;
            }
        }
    }

    /// 退避等待，期间响应 stop / restart。返回 false 表示收到 stop，循环应终止。
    async fn wait_backoff(&self, backoff: &mut Duration) -> bool {
        let current = *backoff;
        *backoff = (current * 2).min(MAX_BACKOFF);
        tokio::select! {
            _ = tokio::time::sleep(current) => true,
            _ = self.inner.restart.notified() => true,
            _ = self.inner.stop.notified() => {
                self.set_state(DshState::Stopped);
                false
            }
        }
    }

    fn spawn_pump<R: AsyncRead + Unpin + Send + 'static>(&self, reader: R) {
        let ring = self.inner.log_ring.clone();
        let emit = self.inner.on_event.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                ring.push_line(line.clone());
                emit(ProcessEvent::Log(line));
            }
        });
    }

    fn set_state(&self, s: DshState) {
        *self.inner.state.lock().unwrap() = s.clone();
        (self.inner.on_event)(ProcessEvent::StateChanged(s));
    }

    fn log(&self, line: impl Into<String>) {
        let line = line.into();
        self.inner.log_ring.push_line(line.clone());
        (self.inner.on_event)(ProcessEvent::Log(line));
    }
}

/// dsh 子进程的 PATH：内嵌 node 目录与 profile 插件的 .bin 目录前置为前两项
/// （node 目录在前——npx/node 必须赢过系统 PATH 上的旧版本），其余项原样保留。
/// base 为父进程 PATH（None 表示未设置，结果只含前两项）。
fn dsh_child_path(node_dir: &Path, home: &Path, base: Option<OsString>) -> OsString {
    let profile = crate::upstream::join_segments(home, crate::upstream::PROFILE_DIR_SEGMENTS);
    let bin = crate::upstream::join_segments(&profile, crate::upstream::PROFILE_BIN_DIR_SEGMENTS);
    let mut entries = vec![node_dir.to_path_buf(), bin];
    if let Some(ref b) = base {
        entries.extend(std::env::split_paths(b));
    }
    // split_paths 拆出的项不含分隔符，join 实际不会失败；真失败退回原 PATH 也比丢光强
    std::env::join_paths(entries).unwrap_or_else(|_| base.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn child_path_prepends_node_dir_and_profile_bin_and_preserves_base() {
        // 内嵌 node 目录必须最前：dsh 派生的 MCP 命令常以 `npx` 配置，若系统 PATH
        // 上的旧 node 抢先解析（机器 B 全局 node v16 实测），引擎不兼容直接崩；
        // profile .bin 次之：插件自带 CLI（如 modlens）按名解析依赖它
        let node_dir = PathBuf::from(r"C:\app\runtime\windows-x64");
        let home = PathBuf::from(r"C:\dsh-home");
        let base = Some(OsString::from(r"C:\Windows;C:\Users\x\AppData\Roaming\npm"));
        let out = dsh_child_path(&node_dir, &home, base);
        let entries: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(entries[0], node_dir, "内嵌 node 目录必须是 PATH 首项，实际：{entries:?}");
        assert_eq!(
            entries[1],
            home.join("profiles").join("web").join("node_modules").join(".bin"),
            "profile 的 .bin 必须是 PATH 第二项，实际：{entries:?}"
        );
        assert_eq!(entries[2], PathBuf::from(r"C:\Windows"), "原有 PATH 项须保留且顺序不变");
        assert_eq!(entries[3], PathBuf::from(r"C:\Users\x\AppData\Roaming\npm"));
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn child_path_without_base_is_node_dir_and_profile_bin() {
        let node_dir = PathBuf::from(r"C:\app\runtime\windows-x64");
        let home = PathBuf::from(r"C:\dsh-home");
        let out = dsh_child_path(&node_dir, &home, None);
        let entries: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], node_dir, "实际：{entries:?}");
        assert!(entries[1].ends_with(".bin"), "实际：{entries:?}");
    }

    #[test]
    fn runtime_ships_npx_and_npm_for_mcp_servers() {
        // 运行时必须自带 npm/npx（fetch-runtime 从 node 发行包拷出）：dsh 派生的
        // MCP server 常以 `npx ...` 配置，子进程 PATH 前置了内嵌 node 目录，缺
        // npx.cmd 就会退回系统 PATH 的任意 node 版本，引擎不兼容即崩（机器 B
        // 全局 node v16 实测）。fixture/dev 流程可能没有暂存运行时，缺席即跳过。
        let rt = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/windows-x64"));
        if !rt.is_dir() {
            return;
        }
        assert!(rt.join("npx.cmd").is_file(), "运行时缺 npx.cmd，MCP `npx` 会落到系统旧 node（实际：{rt:?}）");
        assert!(rt.join("npm.cmd").is_file());
        assert!(rt.join("node_modules/npm/bin/npx-cli.js").is_file());
    }
}
