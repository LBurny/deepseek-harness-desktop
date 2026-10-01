//! 通知装配：sink 门控（前台判定 + 各类型规则 + 提示音）与 mux 事件源接线。

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{Emitter, Manager};
use tokio::sync::watch;

use super::{Notification, NotifySink};
use crate::dsh::dsh_session::DshCreds;
use crate::{dsh::runtime, features::settings, notify, platform};

pub fn build_sink(
    handle: &tauri::AppHandle,
    platform: Arc<dyn platform::Platform>,
    log: PathBuf,
) -> NotifySink {
    let sink_handle = handle.clone();
    let sink_platform = platform;
    let notify_log = log;
    Arc::new(move |n: Notification| {
        // 前台 = 本应用任一窗口处于聚焦态（主窗口可见但失焦 = 用户已切走，算后台）；
        // 各类型按自己的规则（开关 + 时机）决定是否打扰
        let foreground = ["main", "settings", "diagnostics", "skills", "mcp", "remote"]
            .iter()
            .filter_map(|l| sink_handle.get_webview_window(l))
            .any(|w| w.is_focused().unwrap_or(false));
        let settings = sink_handle
            .try_state::<settings::SettingsState>()
            .map(|s| s.get())
            .unwrap_or_default();
        // 壳侧通知诊断：落盘 + 全局日志环（面板回填可见）；面板开着时实时推。
        // Arc 化是为了随播放调用下沉进 platform 后台线程（真实播放结果上报用）
        let diag: crate::platform::SoundDiag = {
            let log = notify_log.clone();
            let handle = sink_handle.clone();
            Arc::new(move |line: String| {
                crate::logging::append_debug_line(&log, &line);
                let _ = handle.emit("dsh-log", &line);
            })
        };
        // 各类型按自己的规则门控；被抑制的也记一行（现场诊断"没提醒"的抓手：
        // 之前静默 return，日志里完全看不到）
        let allowed = match n.kind {
            notify::NotifyKind::Approval => settings.notify.approval.allows(foreground),
            notify::NotifyKind::Question => settings.notify.question.allows(foreground),
            notify::NotifyKind::TaskCompleted => settings.notify.turn_done.allows(foreground),
            notify::NotifyKind::AnswerCompleted => settings.notify.answer_done.allows(foreground),
        };
        if !allowed {
            diag(format!("Notify suppressed: {:?} foreground={foreground}", n.kind));
            return;
        }
        // 全部通知统一挂提示音（含任务确认/选项选择——dsh 卡住等用户输入时
        // 静音提醒等于没提醒）。柔和自定义音：静音 toast + 播放内置 wav；
        // 文件缺失（如 dev 未拷贝资源）降级为系统默认预设。每次播放落一行
        // events.log（时间戳+路径+耗时）：现场对"哪条没声音"定位用。
        let toast_sound = if let Some(rel) = settings.completion_sound.custom_wav() {
            match resolve_custom_sound(&sink_handle, rel) {
                Some(p) => {
                    // 真实播放结果（成功耗时/失败重试）由 platform 后台线程经
                    // diag 落 events.log；这里的 Err 只剩"文件缺失"（同步前置
                    // 检查），降级系统默认提示音，宁可系统音也不静音
                    if sink_platform
                        .play_sound_file(&p, Some(diag.clone()))
                        .is_err()
                    {
                        notify::toast::ToastSound::Default
                    } else {
                        notify::toast::ToastSound::Silent
                    }
                }
                None => {
                    diag(format!(
                        "[{}] play sound: {} not found -> toast Default",
                        crate::logging::local_stamp(),
                        rel
                    ));
                    notify::toast::ToastSound::Default
                }
            }
        } else if settings.completion_sound.toast_sound_name().is_some() {
            notify::toast::ToastSound::Default
        } else {
            notify::toast::ToastSound::Silent
        };
        diag(format!("Notify: {:?} {}", n.kind, n.body));
        // toast::show 内置 Activated 处理：点击通知弹出并聚焦主窗口。
        // show 失败不再静默吞（WinRT 通知被系统策略拦下时至少留痕可查）
        if let Err(e) =
            notify::toast::show(&sink_handle, &n.title, &n.body, toast_sound, Some(diag.clone()))
        {
            diag(format!("toast show failed: {e}"));
        }
    })
}

pub fn spawn_mux(
    handle: &tauri::AppHandle,
    sink: NotifySink,
    creds_rx: watch::Receiver<Option<Arc<DshCreds>>>,
    log: PathBuf,
) {
    // mux（会话事件 → 通知/标题）：0.1.2 单 WS 承载 $events + N 条 session/follow
    let book = Arc::new(std::sync::Mutex::new(notify::SessionBook::default()));
    let (follow_tx, follow_rx) = tokio::sync::mpsc::unbounded_channel();
    let handler_book = book.clone();
    let handler_follow: notify::mux::FollowTx = Arc::new(follow_tx);
    let mux_handler: notify::mux::MuxHandler = Arc::new(move |frame, sink| match frame {
        notify::mux::MuxFrame::EventStream(v) => {
            notify::handle_event_frame(&v, sink, &handler_book, &handler_follow)
        }
        notify::mux::MuxFrame::Follow { session_id, value } => {
            notify::handle_follow_frame(&session_id, &value, sink, &handler_book)
        }
    });
    let reconnect_book = book.clone();
    let emit_handle_for_mux = handle.clone();
    let mux_log = log;
    tauri::async_runtime::spawn(
        Box::new(notify::mux::MuxSource {
            events: notify::mux::MuxEvents {
                handler: mux_handler,
                // （重）连后子代理基线不可知：清空集合，fail-open 宁多弹不漏弹
                on_connect: Some(Arc::new(move || {
                    reconnect_book.lock().unwrap().clear_subagents();
                })),
                follow_rx,
            },
            // 只记端口/事件名（token/cookie 不进日志）
            on_log: Some(Arc::new(move |line| {
                crate::logging::append_debug_line(&mux_log, &line);
                let _ = emit_handle_for_mux.emit("dsh-log", &line);
            })),
        })
        .run(sink, creds_rx),
    );
}

/// 解析内置音效资源（如 sounds/bip-bop-01.wav）的实际路径：resource_dir（剥 \\?\）
/// 或可执行文件旁；都不存在返回 None（调用侧降级）。
pub(crate) fn resolve_custom_sound(handle: &tauri::AppHandle, rel: &str) -> Option<PathBuf> {
    let from_resource = handle
        .path()
        .resource_dir()
        .ok()
        .map(|d| runtime::strip_verbatim(&d).join(rel));
    let from_exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join(rel)));
    [from_resource, from_exe].into_iter().flatten().find(|p| p.is_file())
}
