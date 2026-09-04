//! 远程访问：Cloudflare Quick Tunnel + 壳内嵌 token 鉴权反向代理。
//! 链路与安全模型见 docs/design.zh-CN.md；概览：
//!   手机浏览器 ─HTTPS→ Cloudflare ─→ cloudflared(纯出站)
//!     → 127.0.0.1:proxy(proxy.rs，token 门岗) → 127.0.0.1:dsh(完整 Web UI)
//!
//! RemoteManager 管生命周期：每次 start 重新生成 token、起代理与隧道监督；
//! 隧道 URL 就绪后拼出带 token 的链接；stop/退出应用即整体关停，链接立即失效。
//! 链接泄露时用 reset_link 原地轮换 token 并掐断现有会话（域名不变）。
pub mod project;
pub mod proxy;
pub mod tunnel;

use crate::dsh_session::DshCreds;
use crate::platform::Platform;
use proxy::{spawn_proxy, ProxyHandle};
use rand::Rng;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use tauri::State;
use tokio::sync::watch;
use tunnel::{TunnelEvent, TunnelProcess, TunnelState};

/// 每次开启远程访问重新生成的会话凭据：256-bit 随机，64 字符小写 hex
pub fn generate_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 常数时间比较（等长时逐字节异或）；长度不等直接 false
pub(crate) fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 远程链接 = 隧道 URL + 首次访问凭据
pub fn compose_link(url: &str, token: &str) -> String {
    format!("{url}/?token={token}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Off,
    Starting,
    Up,
    Error,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Off => "off",
            Phase::Starting => "starting",
            Phase::Up => "up",
            Phase::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteStatus {
    /// "off" | "starting" | "up" | "error"
    pub phase: String,
    /// 隧道公网 URL（不含 token）
    pub url: Option<String>,
    /// 完整访问链接（含 token），仅 Up 时存在；链接即凭据，勿分享
    pub link: Option<String>,
    pub error: Option<String>,
    /// 本地鉴权代理端口（诊断用）
    pub proxy_port: Option<u16>,
}

#[derive(Debug)]
pub enum RemoteEvent {
    Status(RemoteStatus),
    Log(String),
}

struct Inner {
    phase: Phase,
    token: Option<Arc<str>>,
    url: Option<String>,
    error: Option<String>,
    proxy_port: Option<u16>,
    proxy: Option<ProxyHandle>,
    tunnel: Option<TunnelProcess>,
}

impl Inner {
    fn dto(&self) -> RemoteStatus {
        let link = match (self.phase, &self.url, &self.token) {
            (Phase::Up, Some(url), Some(t)) => Some(compose_link(url, t)),
            _ => None,
        };
        RemoteStatus {
            phase: self.phase.as_str().into(),
            url: self.url.clone(),
            link,
            error: self.error.clone(),
            proxy_port: self.proxy_port,
        }
    }
}

/// on_event 与状态分离存放：回调在锁外触发，允许回调里回查 status()（托盘更新即如此）
struct Shared {
    inner: Mutex<Inner>,
    on_event: Box<dyn Fn(RemoteEvent) + Send + Sync>,
}

#[derive(Clone)]
pub struct RemoteManager {
    shared: Arc<Shared>,
    platform: Arc<dyn Platform>,
    tunnel_exe: PathBuf,
    /// 隧道子进程的前置参数；生产为空，测试注入 fixture 脚本路径（exe=node）
    tunnel_prefix: Vec<String>,
    work_dir: PathBuf,
    /// dsh-home（代理上 project.rs 的 resolve/list/file 解析 workspace.json 用）
    dsh_home: PathBuf,
    /// dsh 凭据（端口 + launch token；0.1.2 起代理据此代持 dsh-auth cookie）
    creds: watch::Receiver<Option<Arc<DshCreds>>>,
}

impl RemoteManager {
    pub fn new(
        platform: Arc<dyn Platform>,
        tunnel_exe: PathBuf,
        tunnel_prefix: Vec<String>,
        work_dir: PathBuf,
        dsh_home: PathBuf,
        creds: watch::Receiver<Option<Arc<DshCreds>>>,
        on_event: Box<dyn Fn(RemoteEvent) + Send + Sync>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    phase: Phase::Off,
                    token: None,
                    url: None,
                    error: None,
                    proxy_port: None,
                    proxy: None,
                    tunnel: None,
                }),
                on_event,
            }),
            platform,
            tunnel_exe,
            tunnel_prefix,
            work_dir,
            dsh_home,
            creds,
        }
    }

    pub fn status(&self) -> RemoteStatus {
        self.shared.inner.lock().unwrap().dto()
    }

    pub async fn start(&self) -> RemoteStatus {
        {
            let mut g = self.shared.inner.lock().unwrap();
            if matches!(g.phase, Phase::Starting | Phase::Up) {
                return g.dto();
            }
            // 先占位再异步起服务：并发 start 在锁内看到 Starting 直接返回，
            // 否则两个 start 会各起一套代理/隧道，先起的那套句柄被覆盖丢失
            g.phase = Phase::Starting;
            g.error = None;
        }
        if !self.tunnel_exe.is_file() {
            return self.transition_error(format!(
                "cloudflared 缺失（{}），请重新安装或重新运行 fetch-runtime",
                self.tunnel_exe.display()
            ));
        }
        let token: Arc<str> = generate_token().into();
        let proxy = match spawn_proxy(
            token.clone(),
            self.creds.clone(),
            self.dsh_home.clone(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        {
            Ok(p) => p,
            Err(e) => return self.transition_error(format!("远程代理启动失败：{e}")),
        };
        let proxy_port = proxy.port;
        let target = format!("http://127.0.0.1:{proxy_port}");
        let weak: Weak<Shared> = Arc::downgrade(&self.shared);
        let tunnel = TunnelProcess::spawn_supervised(
            self.platform.clone(),
            self.tunnel_exe.clone(),
            self.tunnel_prefix.clone(),
            target,
            self.work_dir.clone(),
            move |ev| {
                let Some(shared) = weak.upgrade() else { return };
                handle_tunnel_event(&shared, ev);
            },
        );
        let st = {
            let mut g = self.shared.inner.lock().unwrap();
            g.phase = Phase::Starting;
            g.token = Some(token);
            g.url = None;
            g.error = None;
            g.proxy_port = Some(proxy_port);
            g.proxy = Some(proxy);
            g.tunnel = Some(tunnel);
            g.dto()
        };
        (self.shared.on_event)(RemoteEvent::Status(st.clone()));
        st
    }

    pub async fn stop(&self) -> RemoteStatus {
        let (proxy, tunnel) = {
            let mut g = self.shared.inner.lock().unwrap();
            g.phase = Phase::Off;
            g.token = None;
            g.url = None;
            g.error = None;
            g.proxy_port = None;
            (g.proxy.take(), g.tunnel.take())
        };
        if let Some(t) = tunnel {
            t.stop().await;
        }
        if let Some(p) = proxy {
            p.shutdown().await;
        }
        let st = self.status();
        (self.shared.on_event)(RemoteEvent::Status(st.clone()));
        st
    }

    /// 重置访问链接：原地轮换 token 并掐断所有已建立会话（代理门岗逐请求读
    /// 最新 token，旧链接/旧 cookie 立即失效；WS 桥接被 drain 掐断）。
    /// 隧道与域名保持不变，无需重新建立。链接泄露后的吊销手段。
    pub fn reset_link(&self) -> Result<RemoteStatus, String> {
        let st = {
            let mut g = self.shared.inner.lock().unwrap();
            if !matches!(g.phase, Phase::Starting | Phase::Up) {
                return Err("远程访问未开启".into());
            }
            let Some(proxy) = &g.proxy else {
                return Err("远程访问代理未就绪，请稍后重试".into());
            };
            let token: Arc<str> = generate_token().into();
            proxy.reset_token(token.clone());
            g.token = Some(token);
            g.dto()
        };
        (self.shared.on_event)(RemoteEvent::Status(st.clone()));
        Ok(st)
    }

    fn transition_error(&self, msg: String) -> RemoteStatus {
        let st = {
            let mut g = self.shared.inner.lock().unwrap();
            g.phase = Phase::Error;
            g.error = Some(msg);
            g.dto()
        };
        (self.shared.on_event)(RemoteEvent::Status(st.clone()));
        st
    }
}

fn handle_tunnel_event(shared: &Shared, ev: TunnelEvent) {
    match ev {
        TunnelEvent::Log(l) => (shared.on_event)(RemoteEvent::Log(l)),
        TunnelEvent::StateChanged(ts) => {
            let st = {
                let mut g = shared.inner.lock().unwrap();
                // stop() 后迟到的隧道事件不得把状态复活（旧隧道停杀与事件上报有窗口期）
                if g.phase == Phase::Off {
                    return;
                }
                match ts {
                    TunnelState::Up { url } => {
                        g.phase = Phase::Up;
                        g.url = Some(url);
                        g.error = None;
                    }
                    // 隧道崩溃重连：token 与代理保留，链接随新 URL 重新生成
                    TunnelState::Starting => {
                        g.phase = Phase::Starting;
                        g.url = None;
                    }
                    TunnelState::Failed(msg) => {
                        g.phase = Phase::Error;
                        g.url = None;
                        g.error = Some(msg);
                    }
                    // 仅由 stop() 触发，那里已落 Off 并上报
                    TunnelState::Stopped => return,
                }
                g.dto()
            };
            (shared.on_event)(RemoteEvent::Status(st));
        }
    }
}

#[tauri::command]
pub async fn start_remote(state: State<'_, RemoteManager>) -> Result<RemoteStatus, String> {
    Ok(state.start().await)
}

#[tauri::command]
pub async fn stop_remote(state: State<'_, RemoteManager>) -> Result<RemoteStatus, String> {
    Ok(state.stop().await)
}

#[tauri::command]
pub fn get_remote_status(state: State<'_, RemoteManager>) -> RemoteStatus {
    state.status()
}

#[tauri::command]
pub fn copy_remote_link(state: State<'_, RemoteManager>) -> Result<(), String> {
    copy_link_to_clipboard(&state)
}

/// 重置访问链接（token 轮换 + 掐断现有会话），隧道与域名不变
#[tauri::command]
pub fn reset_remote_link(state: State<'_, RemoteManager>) -> Result<RemoteStatus, String> {
    state.reset_link()
}

/// 托盘菜单与 invoke 命令共用的复制逻辑
pub fn copy_link_to_clipboard(mgr: &RemoteManager) -> Result<(), String> {
    let link = mgr.status().link.ok_or("远程访问尚未就绪")?;
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("剪贴板不可用：{e}"))?;
    cb.set_text(link).map_err(|e| format!("复制失败：{e}"))
}

/// 当前链接的二维码（SVG 字符串）；仅 Up 态可用
#[tauri::command]
pub fn get_remote_qr(state: State<'_, RemoteManager>) -> Result<String, String> {
    let link = state.status().link.ok_or("远程访问尚未就绪")?;
    let code = qrcode::QrCode::new(link.as_bytes()).map_err(|e| format!("二维码生成失败：{e}"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(320, 320)
        .build())
}

/// 写 events.log 前脱敏：链接即凭据，日志里不能出现 token
/// （cloudflared 的请求日志可能带 ?token= 查询串）
pub(crate) fn redact_token(s: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"(?i)token=[^&\s"']+"#).unwrap());
    re.replace_all(s, "token=<redacted>").into_owned()
}

#[cfg(test)]
mod tests {
    use super::redact_token;

    #[test]
    fn redact_token_strips_credential() {
        assert_eq!(
            redact_token("dest=https://a-b-c.trycloudflare.com/?token=abc123xyz&type=http"),
            "dest=https://a-b-c.trycloudflare.com/?token=<redacted>&type=http"
        );
        assert_eq!(
            redact_token("link https://x.trycloudflare.com/?token=deadbeef"),
            "link https://x.trycloudflare.com/?token=<redacted>"
        );
        assert_eq!(redact_token("no token here"), "no token here");
    }
}
