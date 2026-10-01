//! 进程事件桥：DshState → 进度事件/带 token 导航/凭据下发 + 431 cookie 防护 + UI 心跳看门狗。与 pagebridge（页面→壳）成对。

use std::sync::Arc;
use std::time::Duration;

use tauri::{Emitter, Manager, Url};
use tokio::sync::watch;

use crate::dsh::dsh_session::DshCreds;
use crate::i18n;
use crate::ui::pagebridge;
use crate::dsh::process::{DshState, ProcessEvent};
use crate::bootstrap::progress::{self, ProgressPayload};

/// 删 WebView2 cookie 罐里所有 dsh-auth-* cookie，返回删除个数。dsh 每个进程
/// Set-Cookie 一个新名（dsh-auth-<hash>，30 天 Max-Age），cookie 不分端口、罐子
/// 只进不出，攒满 ~66 个（≈15KB）后 Cookie 头 + bundle 组合 URL 超 Node 16KB
/// 请求头上限 → dsh 431 → 主窗口 "Failed to load plugins"（2026-09-09 实锤，
/// 66 个 cookie 实测）。拿 cookies() 列全罐（含 HttpOnly），逐个 delete_cookie
/// （wry 经 ICoreWebView2CookieManager.DeleteCookie 按 name+domain+path 精确删，
/// cookies() 回传的 Cookie 已带 domain/path，原样传回即中）。失败返回 0 不阻断
/// 导航——最坏情形等于回到修复前现状（几周后再次积累）。必须跑在非主线程的
/// async 上下文：Windows 同步命令/事件回调里调 cookies() 会死锁（tauri 文档
/// + wry#583），本函数由 Ready 的 async spawn 任务调用。
fn prune_stale_dsh_cookies(w: &tauri::WebviewWindow) -> usize {
    let stale = match w.cookies() {
        Ok(list) => list
            .into_iter()
            .filter(|c| is_stale_dsh_cookie(c.name()))
            .collect::<Vec<_>>(),
        Err(_) => return 0,
    };
    let n = stale.len();
    for c in stale {
        let _ = w.delete_cookie(c);
    }
    n
}

/// 431 防护只清 dsh 浏览器侧 auth cookie：名字随进程轮换（dsh-auth-<hash>）。
/// 远程代理门岗 cookie（proxy::COOKIE_NAME = "__dsh_remote"）与其它站点 cookie
/// 一律不动——误删会踢掉已登录的远程页，只能靠完整链接重进。
fn is_stale_dsh_cookie(name: &str) -> bool {
    name.starts_with("dsh-auth-")
}

pub fn bridge_event(
    handle: &tauri::AppHandle,
    home_url: &tauri::Url,
    creds_tx: &watch::Sender<Option<Arc<DshCreds>>>,
    token_rx: &watch::Receiver<Option<Arc<str>>>,
    deployed: bool,
    debug_log: &std::path::Path,
    event: ProcessEvent,
) {
    match event {
        ProcessEvent::Log(line) => {
            // npm 冷装（npx 解析未缓存包）会把 token 等待拉长到分钟级：splash 从
            // "正在启动 dsh 服务…"换成下载文案，用户不再以为卡死。与 process.rs
            // wait_token 的 install 长预算共用同一判定行。
            if crate::dsh::process::is_mcp_install_line(&line) {
                let _ = handle.emit(
                    "dsh-progress",
                    ProgressPayload::new(
                        "starting",
                        i18n::pick(
                            "正在下载 MCP 组件（首次联网安装，可能较慢）…",
                            "Downloading MCP components (first-time online install, may be slow)…",
                        ),
                        None,
                    ),
                );
            }
            let _ = handle.emit("dsh-log", line);
        }
        ProcessEvent::StateChanged(state) => match state {
            DshState::Starting => {
                let _ = handle.emit(
                    "dsh-progress",
                    ProgressPayload::new(
                        "starting",
                        i18n::pick("正在启动 dsh 服务…", "Starting dsh service…"),
                        Some(progress::starting_percent(deployed)),
                    ),
                );
            }
            DshState::Ready { port } => {
                let _ = handle.emit("dsh-ready", serde_json::json!({ "port": port }));
                let _ = handle.emit(
                    "dsh-progress",
                    ProgressPayload::new("ready", i18n::pick("正在打开界面…", "Opening interface…"), Some(100)),
                );
                if let Some(w) = handle.get_webview_window("main") {
                    // 0.1.2 导航必须带 ?token= 过 BrowserAuth。Ready 门控保证 token
                    // 已捕获，这里 30s 等待只是兜底（超时退回裸 URL 不白屏——会落
                    // dsh 401 页但可读）。token 到手同时发 DshCreds（端口+token 合流，
                    // 订阅方：MuxSource 通知流、远程代理 cookie 代持）
                    let mut token_rx = token_rx.clone();
                    let creds_tx = creds_tx.clone();
                    let w = w.clone();
                    let handle = handle.clone();
                    // owned 化后再进 async move 块：spawn 的 future 必须 'static
                    let debug_log = debug_log.to_path_buf();
                    tauri::async_runtime::spawn(async move {
                        // 0.5.11：导航前先清掉罐里的陈年 dsh-auth-* cookie——dsh 每个
                        // 进程 Set-Cookie 一个新名（30 天 Max-Age），cookie 不分端口、
                        // WebView2 罐子只进不出；攒到 ~66 个（≈15KB）时 Cookie 头 +
                        // bundle 组合 URL 超过 Node 16KB 请求头上限 → dsh 431 → 主窗口
                        // "Failed to load plugins"（2026-09-09 实锤）。下面 ?token=
                        // 导航会立即补发新 cookie，罐子此后恒 ≤1 个。
                        let pruned = prune_stale_dsh_cookies(&w);
                        if pruned > 0 {
                            crate::logging::append_debug_line(
                                &debug_log,
                                &format!("[dshdesktop] pruned {pruned} stale dsh-auth cookies (431 guard)"),
                            );
                        }
                        let mut token: Option<Arc<str>> = token_rx.borrow().clone();
                        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                        while token.is_none() && tokio::time::Instant::now() < deadline {
                            if token_rx.changed().await.is_err() {
                                break;
                            }
                            token = token_rx.borrow().clone();
                        }
                        if let Some(t) = &token {
                            creds_tx.send_replace(Some(Arc::new(DshCreds {
                                port,
                                token: t.clone(),
                            })));
                        }
                        let url_str = match &token {
                            Some(t) => format!("http://127.0.0.1:{port}/?token={t}"),
                            None => {
                                let _ = handle.emit(
                                    "dsh-log",
                                    "[dshdesktop] 30s 未捕获 launch token，退回裸 URL（预期落 dsh 鉴权页）",
                                );
                                format!("http://127.0.0.1:{port}/")
                            }
                        };
                        if let Ok(url) = Url::parse(&url_str) {
                            let _ = w.navigate(url);
                        }
                        // UI 心跳看门狗（0.5.11）：导航 30s 后 pagebridge 还没收到
                        // #root 挂载上报 = "进程 Ready 但 UI 死"（431 同类故障）。
                        // 自愈一次：清 dsh-auth cookie + 带 token 重导航。one-shot
                        // 不自旋——再死等 dsh 监督重启走下一轮；30s 内 dsh
                        // Failed/Stopped 把窗口牵回本地 splash 则放弃（别导航到死端口）
                        let nav_ms = pagebridge::note_navigation();
                        let heal_w = w.clone();
                        let heal_log = debug_log.clone();
                        let heal_token = token.clone();
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            if pagebridge::boot_millis() >= nav_ms {
                                return;
                            }
                            // 30s 窗口内 dsh 重启会产生新一轮 Ready→导航→看门狗：
                            // NAV_MS 已换 = 本狗看的是旧导航，让位给新狗——否则
                            // 会拿上一进程的 stale token 把正在加载的新页面导航去 401
                            if pagebridge::nav_millis() != nav_ms {
                                return;
                            }
                            if heal_w
                                .url()
                                .map(|u| u.host_str() != Some("127.0.0.1"))
                                .unwrap_or(true)
                            {
                                return;
                            }
                            let pruned = prune_stale_dsh_cookies(&heal_w);
                            crate::logging::append_debug_line(
                                &heal_log,
                                &format!("[dshdesktop] UI boot heartbeat missing 30s; self-heal: pruned {pruned} dsh-auth cookies, re-navigating (port={port})"),
                            );
                            let s = match &heal_token {
                                Some(t) => format!("http://127.0.0.1:{port}/?token={t}"),
                                None => format!("http://127.0.0.1:{port}/"),
                            };
                            if let Ok(url) = Url::parse(&s) {
                                let _ = heal_w.navigate(url);
                            }
                        });
                    });
                }
            }
            DshState::Failed(msg) => {
                if let Some(w) = handle.get_webview_window("main") {
                    let _ = w.navigate(home_url.clone());
                }
                let _ = handle.emit(
                    "dsh-progress",
                    ProgressPayload::new(
                        "error",
                        i18n::pick(format!("启动失败：{msg}"), format!("Failed to start: {msg}")),
                        None,
                    ),
                );
            }
            DshState::Stopped => {
                let _ = handle.emit(
                    "dsh-progress",
                    ProgressPayload::new("stopped", i18n::pick("dsh 服务已停止", "dsh service stopped"), None),
                );
            }
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stale_cookie_predicate_spares_remote_gate_cookie() {
        // dsh 浏览器侧 auth cookie 名随进程轮换（dsh-auth-<hash>，2026-09-09
        // 实机抓到 66 个）；远程代理门岗 cookie 是 __dsh_remote（proxy::COOKIE_NAME），
        // 431 防护绝不能误删它，否则已登录的远程页被踢、只能靠完整链接重进
        assert!(crate::eventbridge::is_stale_dsh_cookie("dsh-auth-UOBWB7hnni8G1RB"));
        assert!(crate::eventbridge::is_stale_dsh_cookie("dsh-auth-x3d1aKAzkPuvABx"));
        assert!(!crate::eventbridge::is_stale_dsh_cookie("__dsh_remote"));
        assert!(!crate::eventbridge::is_stale_dsh_cookie("session"));
    }
}
