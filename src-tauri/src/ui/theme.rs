use crate::platform::Platform;
use crate::ui::state::{resolve, resolve_locale, system_theme, theme_name, ShellUiState, UiSnapshot};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Theme};

/// 跟随 dsh 的主题与语言设置（profile patch 条目 ui-theme/locale 的
/// config.preference，见 upstream.rs 设置存储段注），同步所有窗口标题栏深浅色、
/// 托盘菜单深浅色，并向本地页面广播 shell-ui-state。
/// preference=system（或缺省）时主题解析平台系统主题、语言解析系统 UI 语言。
/// 采用 2s 轮询：文件极小、改动极少，轮询比 inotify 简单且跨平台无差异。
pub fn spawn_theme_follower(app: &AppHandle, platform: Arc<dyn Platform>, dsh_home: PathBuf) {
    let initial = system_theme(platform.as_ref());
    apply(app, initial);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // 首轮强制同步：即便与启动快照相同，也要广播一次、并让托盘菜单按
        // 最终解析的语言重建（tray 在 ShellUiState 之前创建，可能是旧语言）
        let first = UiSnapshot {
            theme: theme_name(resolve(platform.as_ref(), &dsh_home)).to_string(),
            locale: resolve_locale(platform.as_ref(), &dsh_home),
        };
        sync_ui_snapshot(&app, first, true);
        loop {
            let resolved = resolve(platform.as_ref(), &dsh_home);
            apply(&app, resolved);
            sync_ui_snapshot(
                &app,
                UiSnapshot {
                    theme: theme_name(resolved).to_string(),
                    locale: resolve_locale(platform.as_ref(), &dsh_home),
                },
                false,
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

/// 快照有变化（或 force）才落库并广播；locale 变化额外重建托盘菜单与本地窗口标题。
fn sync_ui_snapshot(app: &AppHandle, snap: UiSnapshot, force: bool) {
    let Some(state) = app.try_state::<ShellUiState>() else {
        return;
    };
    let old = {
        let mut guard = state.0.lock().unwrap();
        if !force && *guard == snap {
            return;
        }
        std::mem::replace(&mut *guard, snap.clone())
    };
    crate::i18n::set_locale(&snap.locale);
    let _ = app.emit("shell-ui-state", &snap);
    if force || old.locale != snap.locale {
        crate::ui::tray::apply_locale(app, &snap.locale);
    }
}

fn apply(app: &AppHandle, theme: Theme) {
    #[cfg(windows)]
    apply_windows(app, theme);
    #[cfg(not(windows))]
    {
        // 遍历所有窗口而非写死 label——新增本地窗口（如 skills）自动跟随
        for w in app.webview_windows().values() {
            let _ = w.set_theme(Some(theme));
        }
    }
}

/// Windows 上双管齐下：
/// 1) set_theme 同步 tao 内部主题状态——否则 tao 可能在窗口事件（显示/聚焦）后用
///    缓存的旧状态覆盖可视效果；隐藏窗口上 set_theme 可能报错甚至 panic，须兜住。
/// 2) 直接对 HWND 设置 DWMWA_USE_IMMERSIVE_DARK_MODE——无缓存、幂等，对隐藏窗口
///    同样生效，是标题栏颜色的权威来源。
/// 注意 DwmSetWindowAttribute 只改属性、不触发非客户区重绘：标题栏会保持旧色直到
/// 下一次激活（用户点一下才变色）。tao 内部用伪造 WM_NCACTIVATE 触发重绘，但该法
/// 在部分时序/焦点状态下不生效（winit/Electron 均因此改用 SWP_FRAMECHANGED）。
/// 所以主题实际变化时必须 SWP_FRAMECHANGED + RedrawWindow 强制重绘非客户区
/// （Chromium/Windows Terminal 同款）；轮询同值时不重复强制，否则可见窗口的
/// 标题栏每 2s 闪一次。
#[cfg(windows)]
fn apply_windows(app: &AppHandle, theme: Theme) {
    use std::sync::atomic::Ordering;
    let dark = matches!(theme, Theme::Dark);
    let prev = LAST_APPLIED.swap(if dark { 2 } else { 1 }, Ordering::SeqCst);
    let changed = prev == 0 || (prev == 2) != dark;
    // 遍历所有窗口而非写死 label——新增本地窗口（如 skills）自动跟随
    for w in app.webview_windows().values() {
        apply_window_theme(w, theme, changed);
    }
    if changed {
        set_menu_app_mode(dark);
        // 现场诊断：主题切换应用留痕，便于排查"属性已改但标题栏没换色"类问题
        if let Some(p) = app.try_state::<Arc<dyn Platform>>() {
            crate::logging::append_debug_line(
                &p.runtime_base_dir().join("events.log"),
                &format!(
                    "Theme: applied {} (force NC redraw)",
                    if dark { "dark" } else { "light" }
                ),
            );
        }
    }
}

/// 最近一次已应用的解析主题：0=尚未应用，1=light，2=dark。
/// 轮询每 2s 跑，只有主题真正变化才对全窗口强制非客户区重绘。
#[cfg(windows)]
static LAST_APPLIED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// 对单个窗口落主题：同步 tao 状态 + 写 DWM 属性；force_redraw 时强制非客户区重绘。
#[cfg(windows)]
fn apply_window_theme(w: &tauri::WebviewWindow, theme: Theme, force_redraw: bool) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_USE_IMMERSIVE_DARK_MODE_LEGACY: u32 = 19; // Win10 20H1 之前
    let value: i32 = matches!(theme, Theme::Dark) as i32;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = w.set_theme(Some(theme));
    }));
    if let Ok(hwnd) = w.hwnd() {
        let hwnd = hwnd.0 as _;
        unsafe {
            let hr = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &value as *const i32 as *const _,
                std::mem::size_of::<i32>() as u32,
            );
            if hr != 0 {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE_LEGACY,
                    &value as *const i32 as *const _,
                    std::mem::size_of::<i32>() as u32,
                );
            }
            if force_redraw {
                force_nc_redraw(hwnd);
            }
        }
    }
}

/// 强制非客户区（标题栏）立即按当前 DWM 属性重绘，无需激活窗口（NOACTIVATE
/// 不抢焦点）。对隐藏窗口只是打标记，显示时按新属性绘制，无副作用。
#[cfg(windows)]
fn force_nc_redraw(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::Graphics::Gdi::{
        RedrawWindow, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        let _ = RedrawWindow(
            hwnd,
            std::ptr::null(),
            std::ptr::null_mut(),
            RDW_FRAME | RDW_INVALIDATE | RDW_UPDATENOW,
        );
    }
}

/// 窗口显示前的主题落位：窗口 visible(false) 建窗、on_page_load 才 show。
/// 新建窗口的 DWM 属性来自系统主题（tao 建窗行为），与 dsh 解析主题可能相反
/// （系统浅色 + dsh 深色），等 2s 轮询修正会让新窗口带着错误标题栏出生。
/// 在首个可见帧前把属性写对；窗口若已可见（主窗口整页导航重载）则同时强制重绘。
#[cfg(windows)]
pub(crate) fn apply_before_show(app: &AppHandle, w: &tauri::WebviewWindow) {
    let theme = app
        .try_state::<ShellUiState>()
        .map(|s| {
            if s.get().theme == "dark" {
                Theme::Dark
            } else {
                Theme::Light
            }
        })
        .unwrap_or(Theme::Dark); // 与 tray.rs theme_bootstrap 的缺省一致
    let visible = w.is_visible().unwrap_or(false);
    apply_window_theme(w, theme, visible);
}

#[cfg(not(windows))]
pub(crate) fn apply_before_show(_app: &AppHandle, _w: &tauri::WebviewWindow) {}

/// 托盘菜单深浅色：PreferredAppMode 2=ForceDark / 3=ForceLight（1903+）。
/// 不用 AllowDark——它跟随系统而非 dsh 主题。uxtheme 常年驻留进程，无需 FreeLibrary。
#[cfg(windows)]
fn set_menu_app_mode(dark: bool) {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    unsafe {
        let uxtheme = LoadLibraryA(b"uxtheme.dll\0".as_ptr());
        if uxtheme.is_null() {
            return;
        }
        // MAKEINTRESOURCEA(135) = SetPreferredAppMode，136 = FlushMenuThemes
        let set_mode = GetProcAddress(uxtheme, 135usize as *const u8);
        if let Some(f) = set_mode {
            let f: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            f(if dark { 2 } else { 3 });
            if let Some(fl) = GetProcAddress(uxtheme, 136usize as *const u8) {
                let fl: unsafe extern "system" fn() = std::mem::transmute(fl);
                fl();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实窗口冒烟：DWM 属性写入可读回，force_nc_redraw 不崩溃。
    /// 重绘的视觉效果（切换后标题栏立即换色、无需点击）由
    /// scripts/verify-titlebar-theme.ps1 目验——像素级断言无法在单测里做。
    #[cfg(windows)]
    #[test]
    fn force_nc_redraw_on_real_window() {
        use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DwmSetWindowAttribute};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW,
            WS_OVERLAPPEDWINDOW,
        };
        unsafe {
            let name: Vec<u16> = "dshdesktop-theme-test\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                320,
                200,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            let dark: i32 = 1;
            DwmSetWindowAttribute(hwnd, 20, &dark as *const i32 as *const _, 4);
            force_nc_redraw(hwnd);
            let mut got: i32 = 0;
            assert_eq!(
                DwmGetWindowAttribute(hwnd, 20, &mut got as *mut i32 as *mut _, 4),
                0
            );
            assert_eq!(got, 1);
            DestroyWindow(hwnd);
        }
    }
}
