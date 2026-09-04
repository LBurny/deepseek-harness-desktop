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
use tokio::sync::{watch, Notify};

const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_FAILURES: u32 = 5;
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// wait_token 的超时预算。判定基准是"静默"（stdout/stderr 持续无输出）而非固定
/// 时长：npm 冷装（npx 解析未缓存包）会静默下载数分钟，固定 60s 会把快装完的
/// 进程杀树、白等一轮再靠缓存余温重启（0.5.5 实踩）。见到 npm 冷装警告行
/// （is_mcp_install_line）后切换到 install 长预算；absolute 防"一直打印但永远
/// 不就绪"的病态进程把启动挂死。
#[derive(Debug, Clone, Copy)]
pub struct BootTimeouts {
    /// 普通静默预算（等价旧的固定 60s 语义）
    pub silence: Duration,
    /// 冷装模式预算（npm fetch 可能长时间无输出）
    pub install: Duration,
    /// 从 wait 开始的绝对上限
    pub absolute: Duration,
}

impl Default for BootTimeouts {
    fn default() -> Self {
        Self {
            silence: READY_TIMEOUT,
            install: Duration::from_secs(600),
            absolute: Duration::from_secs(600),
        }
    }
}

/// npm 冷装警告行：npx 在线解析未缓存包时打印（非 TTY 下警告后直接安装）。
/// 见到它说明本次启动在联网下载 MCP 组件——wait_token 切 install 预算（本模块）、
/// splash 换"正在下载"文案（lib.rs bridge_event）。
pub(crate) fn is_mcp_install_line(line: &str) -> bool {
    line.contains("npm warn exec") && line.contains("will be installed")
}

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
    /// 0.1.2 BrowserAuth launch token（stdout 就绪行捕获；Ready 时必已就绪）
    token: Mutex<Option<Arc<str>>>,
    /// 就绪 token 的广播端（pump 命中时更新；None=调用方未订阅）
    token_tx: Option<Arc<watch::Sender<Option<Arc<str>>>>>,
    on_event: Arc<dyn Fn(ProcessEvent) + Send + Sync>,
    shutdown: AtomicBool,
    stop: Notify,
    restart: Notify,
    /// 最近一次子进程 stdout/stderr 活动时间（wait_token 静默判定的基准）；spawn 时重置
    last_activity: Mutex<std::time::Instant>,
    /// 本次启动是否已见到 npm 冷装警告行（wait_token 切长预算）；spawn 时重置
    mcp_install: AtomicBool,
    timeouts: BootTimeouts,
}

#[derive(Clone)]
pub struct DshProcess {
    inner: Arc<Inner>,
}

impl DshProcess {
    pub fn spawn_supervised(
        platform: Arc<dyn Platform>,
        paths: RuntimePaths,
        token_tx: watch::Sender<Option<Arc<str>>>,
        events: impl Fn(ProcessEvent) + Send + Sync + 'static,
    ) -> Self {
        Self::spawn_supervised_with_timeouts(platform, paths, token_tx, events, BootTimeouts::default())
    }

    /// 带自定义启动预算的构造（集成测试用短预算覆盖静默/冷装分支；生产走
    /// spawn_supervised 的 BootTimeouts::default()）
    #[doc(hidden)]
    pub fn spawn_supervised_with_timeouts(
        platform: Arc<dyn Platform>,
        paths: RuntimePaths,
        token_tx: watch::Sender<Option<Arc<str>>>,
        events: impl Fn(ProcessEvent) + Send + Sync + 'static,
        timeouts: BootTimeouts,
    ) -> Self {
        let this = Self {
            inner: Arc::new(Inner {
                platform,
                paths,
                state: Mutex::new(DshState::Starting),
                pid: AtomicU32::new(0),
                token: Mutex::new(None),
                token_tx: Some(Arc::new(token_tx)),
                on_event: Arc::new(events),
                shutdown: AtomicBool::new(false),
                stop: Notify::new(),
                restart: Notify::new(),
                last_activity: Mutex::new(std::time::Instant::now()),
                mcp_install: AtomicBool::new(false),
                timeouts,
            }),
        };
        let runner = this.clone();
        tokio::spawn(async move { runner.supervise_loop().await });
        this
    }

    pub fn state(&self) -> DshState {
        self.inner.state.lock().unwrap().clone()
    }

    /// 0.1.2 launch token（stdout 就绪行捕获）；`DshState::Ready` 发出时必已就绪
    pub fn token(&self) -> Option<Arc<str>> {
        self.inner.token.lock().unwrap().clone()
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
            // launch token 按 dsh 进程轮换：spawn 前清掉上一进程的缓存与广播端，
            // 否则 wait_token 拿旧 token 秒过、Ready 抢跑——主窗口带旧 token 导航
            // 落 dsh 401 页，mux/代理凭据交换 401 死循环（0.5.1 重启后实踩）。
            // 旧进程此时已被 kill+wait 回收、其 stdout 泵已 EOF，无竞态。
            *self.inner.token.lock().unwrap() = None;
            if let Some(tx) = &self.inner.token_tx {
                tx.send_if_modified(|cur| cur.take().is_some());
            }
            // 静默判定基准与冷装标记随新进程重置（旧进程的泵已 EOF，无竞态）
            *self.inner.last_activity.lock().unwrap() = std::time::Instant::now();
            self.inner.mcp_install.store(false, Ordering::SeqCst);
            self.set_state(DshState::Starting);
            // 启动计时：从进入 Starting 到 Ready 的分解（HTTP 绑定 / 等 token 行），
            // Ready 时落日志——诊断面板"上次启动"行与用户报"启动慢"的定位数据源
            let spawned_at = tokio::time::Instant::now();
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
                let http_elapsed = spawned_at.elapsed();
                // 0.1.2 契约：HTTP 就绪后 stdout 才打就绪行（token 唯一来源）。
                // Ready 必须等 token——导航要 ?token= 过 BrowserAuth，没 token 的
                // Ready 只会让主窗口落 401 页
                if !self.wait_token().await {
                    self.log(
                        "[dshdesktop] dsh stdout 未出现就绪 URL（token）——上游 READY_URL_PREFIX 契约漂移？",
                    );
                    self.inner.platform.kill_process_tree(pid);
                    let _ = child.wait().await;
                    self.inner.pid.store(0, Ordering::SeqCst);
                    failures += 1;
                    if failures >= MAX_FAILURES {
                        self.set_state(DshState::Failed("dsh token never captured".into()));
                        break;
                    }
                    if !self.wait_backoff(&mut backoff).await {
                        break;
                    }
                    continue;
                }
                failures = 0;
                backoff = Duration::from_millis(500);
                // 分解行先于 Ready 落日志：观察者看到 Ready 时该行必已存在
                let total = spawned_at.elapsed();
                self.log(format_boot_timing(
                    port,
                    total,
                    http_elapsed,
                    total.saturating_sub(http_elapsed),
                ));
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

    /// 等 stdout 就绪行捕获到 token；期间响应 stop（返回 false=应终止循环）。
    /// 判定是"静默超时"而非固定时长：持续有输出（哪怕慢）就等下去；见到 npm 冷装
    /// 警告行后切 install 长预算（fetch 可能数分钟无输出）；absolute 封顶防病态
    /// 进程永远挂着。50ms 轮询而非 Notify 纯等待——notify_waiters 只唤醒已注册的
    /// waiter，检查与注册之间的通知会丢（错过一次就是干等到超时）
    async fn wait_token(&self) -> bool {
        let started = tokio::time::Instant::now();
        loop {
            if self.token().is_some() {
                return true;
            }
            let silence = self.inner.last_activity.lock().unwrap().elapsed();
            let budget = if self.inner.mcp_install.load(Ordering::SeqCst) {
                self.inner.timeouts.install
            } else {
                self.inner.timeouts.silence
            };
            if silence >= budget || started.elapsed() >= self.inner.timeouts.absolute {
                return false;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                _ = self.inner.stop.notified() => return false,
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
        let emit = self.inner.on_event.clone();
        let this = self.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // 静默判定基准：任何输出都刷新活动时间
                *this.inner.last_activity.lock().unwrap() = std::time::Instant::now();
                // npm 冷装信号：wait_token 据此切 install 长预算（下载可能数分钟无输出）
                if is_mcp_install_line(&line) {
                    this.inner.mcp_install.store(true, Ordering::SeqCst);
                }
                // 0.1.2 起 stdout 就绪行是 launch token 唯一来源：先捕获再脱敏。
                // 链接即凭据，原始行不得进入任何日志面（dsh-log/events.log/诊断环）
                if let Some((_, token)) = crate::dsh_session::parse_ready_line(&line) {
                    let t: Arc<str> = token.into();
                    *this.inner.token.lock().unwrap() = Some(t.clone());
                    if let Some(tx) = &this.inner.token_tx {
                        tx.send_if_modified(|cur| {
                            if cur.as_deref() == Some(t.as_ref()) {
                                false
                            } else {
                                *cur = Some(t.clone());
                                true
                            }
                        });
                    }
                }
                emit(ProcessEvent::Log(crate::remote::redact_token(&line)));
            }
        });
    }

    fn set_state(&self, s: DshState) {
        *self.inner.state.lock().unwrap() = s.clone();
        (self.inner.on_event)(ProcessEvent::StateChanged(s));
    }

    fn log(&self, line: impl Into<String>) {
        (self.inner.on_event)(ProcessEvent::Log(line.into()));
    }
}

/// Ready 时落的启动耗时分解行（诊断面板"上次启动"的数据源）。格式被锚定：
/// 改动须同步 diagnostics::parse_boot_timing、process.rs 与 tests/process.rs 的测试。
fn format_boot_timing(port: u16, total: Duration, http: Duration, token: Duration) -> String {
    format!(
        "[dshdesktop] ready: port={port} total={:.2}s http={:.2}s token={:.2}s",
        total.as_secs_f64(),
        http.as_secs_f64(),
        token.as_secs_f64()
    )
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
    fn mcp_install_line_detection() {
        // npx 冷装的判定行（npm 非 TTY 下警告后直接安装）：shell 靠它切长超时预算
        // + splash 换"正在下载"文案，判定错放=杀树回归
        assert!(super::is_mcp_install_line(
            "npm warn exec The following package was not found and will be installed: @playwright/mcp@0.0.80"
        ));
        // 无关行不误判
        assert!(!super::is_mcp_install_line("listening http://127.0.0.1:31817"));
        assert!(!super::is_mcp_install_line("Context7 Documentation MCP Server v4.0.5 running on stdio"));
        assert!(!super::is_mcp_install_line("npm warn deprecated something"));
    }

    #[test]
    fn boot_timing_line_format() {
        // 启动耗时分解的统一格式：诊断面板按此解析（diagnostics::parse_boot_timing），
        // 改格式必须同步解析器与测试
        let line = super::format_boot_timing(
            31817,
            Duration::from_millis(12340),
            Duration::from_millis(1230),
            Duration::from_millis(11110),
        );
        assert_eq!(line, "[dshdesktop] ready: port=31817 total=12.34s http=1.23s token=11.11s");
    }

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
