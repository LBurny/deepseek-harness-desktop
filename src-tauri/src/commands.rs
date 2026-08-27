use crate::diagnostics::{BootstrapInfo, SharedState, StatusDto};
use crate::theme::ShellUiState;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

/// 本地页面的主题/语言快照：页面加载即取，之后靠 shell-ui-state 事件增量更新
#[tauri::command]
pub fn get_shell_ui_state(state: State<ShellUiState>) -> crate::theme::UiSnapshot {
    state.get()
}

#[tauri::command]
pub fn get_bootstrap_error(state: State<BootstrapInfo>) -> Option<String> {
    state.error()
}

#[tauri::command]
pub fn is_first_launch(state: State<SharedState>) -> bool {
    state.first_launch
}

#[tauri::command]
pub fn get_status(state: State<SharedState>) -> StatusDto {
    StatusDto {
        state: format!("{:?}", state.process.state()),
        port: state.process.port(),
        pid: state.process.pid(),
        version: state.version.clone(),
    }
}

#[tauri::command]
pub async fn restart_dsh(state: State<'_, SharedState>) -> Result<(), String> {
    state.process.restart().await;
    Ok(())
}

#[tauri::command]
pub fn get_recent_logs(state: State<SharedState>) -> Vec<String> {
    // events.log = 壳侧诊断 + dsh 进程输出的统一持久层（1MB 截断），
    // 读尾部即得跨会话的近期历史；面板开着期间的增量走 dsh-log 实时事件
    let log = state
        .runtime
        .home
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("events.log");
    crate::diagnostics::read_log_tail(&log, 500)
}

#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let m = app.autolaunch();
    // auto-launch 0.5 的 disable() 无条件 RegDeleteValueW：值不存在时返回
    // ERROR_FILE_NOT_FOUND（用户看到"系统找不到指定的文件 (os error 2)"）。
    // 从未开过自启动时每次保存设置都会踩到——目标态已达成即视为成功，
    // 顺带避免每次保存都重写一遍注册表。
    if m.is_enabled().unwrap_or(false) == enabled {
        return Ok(());
    }
    let r = if enabled { m.enable() } else { m.disable() };
    r.map_err(|e| e.to_string())
}

#[cfg(all(test, windows))]
mod tests {
    /// 上游行为锚定：auto-launch 0.5 的 disable() 对不存在的 Run 值返回
    /// os error 2（"系统找不到指定的文件"）——set_autostart 因此必须先比对
    /// 目标态（已达成即 Ok），不能无条件透传 disable()。若上游改为幂等，
    /// 此测试失败即提醒壳侧防御可以简化。
    #[test]
    fn autolaunch_disable_on_missing_value_is_file_not_found() {
        let al = auto_launch::AutoLaunch::new(
            "dshdesktop-never-enabled-test",
            "C:\\nonexistent\\dshdesktop.exe",
            &[] as &[&str],
        );
        let err = al.disable().unwrap_err();
        let auto_launch::Error::Io(e) = err else {
            panic!("期望 Io 错误，实际: {err:?}")
        };
        assert_eq!(e.raw_os_error(), Some(2));
    }
}
