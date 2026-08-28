use crate::diagnostics::SharedState;
use crate::i18n;
use crate::remote::RemoteManager;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const MENU_OPEN: &str = "open";
pub const MENU_DIAGNOSTICS: &str = "diagnostics";
pub const MENU_PLUGINS: &str = "plugins";
pub const MENU_SKILLS: &str = "skills";
pub const MENU_MCP: &str = "mcp";
pub const MENU_REMOTE_ENABLE: &str = "remote-enable";
pub const MENU_REMOTE_DISABLE: &str = "remote-disable";
pub const MENU_REMOTE_COPY: &str = "remote-copy";
pub const MENU_REMOTE_QR: &str = "remote-qr";
pub const MENU_REMOTE_RESET: &str = "remote-reset";
pub const MENU_RESTART: &str = "restart";
pub const MENU_SETTINGS: &str = "settings";
pub const MENU_QUIT: &str = "quit";

/// 远程访问子菜单的五个菜单项句柄。Windows 托盘菜单项文案不可改（locale 切换只能
/// 重建整个菜单），但 enabled 状态可改——菜单重建后须替换本结构里的句柄。
pub struct TrayRemoteItems(pub Mutex<RemoteItems>);

pub struct RemoteItems {
    pub enable: MenuItem<tauri::Wry>,
    pub disable: MenuItem<tauri::Wry>,
    pub copy: MenuItem<tauri::Wry>,
    pub qr: MenuItem<tauri::Wry>,
    pub reset: MenuItem<tauri::Wry>,
}

pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let (menu, remote_items) = build_menu(app)?;
    app.manage(TrayRemoteItems(Mutex::new(remote_items)));
    TrayIconBuilder::with_id("main-tray")
        .tooltip("DSHDesktop")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN => show_main(app),
            MENU_DIAGNOSTICS => open_diagnostics(app),
            MENU_PLUGINS => open_plugins(app),
            MENU_SKILLS => open_skills(app),
            MENU_MCP => open_mcp(app),
            MENU_SETTINGS => open_settings(app),
            MENU_REMOTE_ENABLE => {
                if let Some(rm) = app.try_state::<RemoteManager>() {
                    let rm = rm.inner().clone();
                    tauri::async_runtime::spawn(async move {
                        rm.start().await;
                    });
                }
            }
            MENU_REMOTE_DISABLE => {
                if let Some(rm) = app.try_state::<RemoteManager>() {
                    let rm = rm.inner().clone();
                    tauri::async_runtime::spawn(async move {
                        rm.stop().await;
                    });
                }
            }
            MENU_REMOTE_COPY => {
                if let Some(rm) = app.try_state::<RemoteManager>() {
                    if crate::remote::copy_link_to_clipboard(&rm).is_ok() {
                        let _ = crate::notify::toast::show(
                            app,
                            &i18n::pick("远程访问", "Remote access"),
                            &i18n::pick("链接已复制到剪贴板", "Link copied to clipboard"),
                            crate::notify::toast::ToastSound::Silent,
                            None,
                        );
                    }
                }
            }
            MENU_REMOTE_QR => open_remote(app),
            MENU_REMOTE_RESET => {
                if let Some(rm) = app.try_state::<RemoteManager>() {
                    if rm.reset_link().is_ok() {
                        let _ = crate::notify::toast::show(
                            app,
                            &i18n::pick("远程访问", "Remote access"),
                            &i18n::pick(
                                "链接已重置，旧链接与已连接的设备即刻失效",
                                "Link reset — the old link and connected devices are revoked",
                            ),
                            crate::notify::toast::ToastSound::Silent,
                            None,
                        );
                    }
                }
            }
            MENU_RESTART => {
                if let Some(state) = app.try_state::<SharedState>() {
                    let proc = state.process.clone();
                    tauri::async_runtime::spawn(async move {
                        proc.restart().await;
                    });
                }
            }
            MENU_QUIT => quit_app(app),
            _ => {}
        })
        // 左键单击托盘图标 = 打开主界面（隐藏到托盘后一次点击即回；窗口未销毁，
        // 位置保持隐藏前状态）；菜单改由右键弹出（show_menu_on_left_click(false)）
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 按当前全局语言构建托盘菜单。locale 变化时由 theme 关注循环调用
/// `apply_locale` 重建（Windows 托盘菜单不支持改文案，只能重建）。
/// 返回菜单本体与远程访问子菜单项句柄（enabled 状态可改，句柄由 TrayRemoteItems 持有）。
fn build_menu(app: &AppHandle) -> tauri::Result<(Menu<tauri::Wry>, RemoteItems)> {
    let open = MenuItem::with_id(
        app,
        MENU_OPEN,
        i18n::pick("打开主界面", "Open main window"),
        true,
        None::<&str>,
    )?;
    let diagnostics = MenuItem::with_id(
        app,
        MENU_DIAGNOSTICS,
        i18n::pick("诊断面板", "Diagnostics"),
        true,
        None::<&str>,
    )?;
    let plugins = MenuItem::with_id(
        app,
        MENU_PLUGINS,
        i18n::pick("插件管理", "Plugins"),
        true,
        None::<&str>,
    )?;
    let skills = MenuItem::with_id(
        app,
        MENU_SKILLS,
        i18n::pick("技能管理", "Skills"),
        true,
        None::<&str>,
    )?;
    let mcp = MenuItem::with_id(app, MENU_MCP, i18n::pick("MCP管理", "MCP servers"), true, None::<&str>)?;
    // 远程访问子菜单：开启/关闭互斥启用，复制/二维码/重置仅 Up 可用（初始 off 态）
    let remote_items = RemoteItems {
        enable: MenuItem::with_id(
            app,
            MENU_REMOTE_ENABLE,
            i18n::pick("开启远程访问", "Start remote access"),
            true,
            None::<&str>,
        )?,
        disable: MenuItem::with_id(
            app,
            MENU_REMOTE_DISABLE,
            i18n::pick("关闭远程访问", "Stop remote access"),
            false,
            None::<&str>,
        )?,
        copy: MenuItem::with_id(
            app,
            MENU_REMOTE_COPY,
            i18n::pick("复制远程链接", "Copy remote link"),
            false,
            None::<&str>,
        )?,
        qr: MenuItem::with_id(
            app,
            MENU_REMOTE_QR,
            i18n::pick("显示二维码", "Show QR code"),
            false,
            None::<&str>,
        )?,
        reset: MenuItem::with_id(
            app,
            MENU_REMOTE_RESET,
            i18n::pick("重置远程链接", "Reset remote link"),
            false,
            None::<&str>,
        )?,
    };
    let remote = Submenu::with_items(
        app,
        i18n::pick("远程访问", "Remote access"),
        true,
        &[
            &remote_items.enable,
            &remote_items.disable,
            &remote_items.copy,
            &remote_items.qr,
            &remote_items.reset,
        ],
    )?;
    let restart = MenuItem::with_id(
        app,
        MENU_RESTART,
        i18n::pick("重启服务", "Restart service"),
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(
        app,
        MENU_SETTINGS,
        i18n::pick("其它设置", "Other settings"),
        true,
        None::<&str>,
    )?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, i18n::pick("退出", "Quit"), true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &diagnostics, &plugins, &skills, &mcp, &remote, &restart, &settings, &sep, &quit],
    )?;
    Ok((menu, remote_items))
}

/// dsh 语言切换后：重建托盘菜单、刷新本地窗口标题（语言相关）。
pub fn apply_locale(app: &AppHandle, _locale: &str) {
    if let Ok((menu, remote_items)) = build_menu(app) {
        if let Some(tray) = app.tray_by_id("main-tray") {
            let _ = tray.set_menu(Some(menu));
        }
        // 菜单重建后旧句柄失效：替换并按当前远程访问状态重设 enabled
        if let Some(state) = app.try_state::<TrayRemoteItems>() {
            *state.0.lock().unwrap() = remote_items;
            if let Some(rm) = app.try_state::<RemoteManager>() {
                update_remote_items(app, &rm.status().phase);
            }
        }
    }
    let titles: &[(&str, &str, &str)] = &[
        ("diagnostics", "DSHDesktop 诊断面板", "DSHDesktop Diagnostics"),
        ("settings", "DSHDesktop 设置", "DSHDesktop Settings"),
        ("plugins", "DSHDesktop 插件管理", "DSHDesktop Plugins"),
        ("skills", "DSHDesktop 技能管理", "DSHDesktop Skills"),
        ("mcp", "DSHDesktop MCP 管理", "DSHDesktop MCP Manager"),
        ("remote", "DSHDesktop 远程访问", "DSHDesktop Remote Access"),
    ];
    for (label, zh, en) in titles {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.set_title(&i18n::pick(*zh, *en));
        }
    }
}

/// 远程访问状态变化时刷新子菜单项的 enabled（文案静态，只有开关互斥与链接项可用性）
pub fn update_remote_items(app: &AppHandle, phase: &str) {
    let Some(state) = app.try_state::<TrayRemoteItems>() else {
        return;
    };
    let g = state.0.lock().unwrap();
    let (enable, disable, link) = match phase {
        "starting" => (false, true, false),
        "up" => (false, true, true),
        _ => (true, false, false), // off / error
    };
    let _ = g.enable.set_enabled(enable);
    let _ = g.disable.set_enabled(disable);
    let _ = g.copy.set_enabled(link);
    let _ = g.qr.set_enabled(link);
    let _ = g.reset.set_enabled(link);
}

fn window_title(zh: &str, en: &str) -> String {
    i18n::pick(zh, en)
}

/// 打开主界面并聚焦：托盘左键/菜单"打开主界面"/点击系统通知共用
/// （toast 的 Activated 回调从 WinRT 线程经 run_on_main_thread 调进来）
pub(crate) fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        // 置顶抖动（短暂 always_on_top 再取消）：点击 toast 激活时前台在 Shell，
        // 运行中实例的 SetForegroundWindow 被前台锁拒绝——窗口会弹出但被压在
        // 当前前台应用下面（机器 B 实测"老是覆盖在下面"）。强制置顶一次把窗口
        // 顶到 Z 序顶端，焦点若仍被拒窗口也可见可点。
        let _ = w.set_always_on_top(true);
        let _ = w.set_focus();
        let _ = w.set_always_on_top(false);
    }
}

/// 主题一致的建窗引导：窗口 visible(false) 延迟到 on_page_load 才显示，但 WebView2
/// 预绘制白底 + app.css 首帧按系统色兜底，在"系统浅色 + dsh 深色"组合下页面
/// JS 置 data-theme 之前会整页白几秒。创建时按壳当前解析主题双管齐下：
/// background_color 管 WebView2 的白底，初始化脚本在首帧前写死 data-theme
/// 与 color-scheme，压住 CSS 的浅色变量分支。
fn theme_bootstrap(app: &AppHandle) -> (String, tauri::window::Color) {
    let dark = app
        .try_state::<crate::theme::ShellUiState>()
        .map(|s| s.get().theme == "dark")
        .unwrap_or(true);
    let name = if dark { "dark" } else { "light" };
    let script = format!(
        "document.documentElement.dataset.theme='{name}';\
         document.documentElement.style.colorScheme='{name}';"
    );
    // 与 src/app.css 两个主题的 --bg 一致
    let color = if dark {
        tauri::window::Color(15, 17, 21, 255)
    } else {
        tauri::window::Color(245, 246, 248, 255)
    };
    (script, color)
}

fn open_diagnostics(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("diagnostics") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // 创建时隐藏：页面加载完成（lib.rs on_page_load）再显示，防白闪
    let (script, color) = theme_bootstrap(app);
    let _ = WebviewWindowBuilder::new(app, "diagnostics", WebviewUrl::App("index.html#/diagnostics".into()))
        .title(window_title("DSHDesktop 诊断面板", "DSHDesktop Diagnostics"))
        .inner_size(900.0, 640.0)
        // 无 window-state 记录时创建即居中；有记录时插件 restore 在创建后、可见前覆盖
        .center()
        .visible(false)
        .initialization_script(&script)
        .background_color(color)
        .build();
}

fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // 创建时隐藏：页面加载完成（lib.rs on_page_load）再显示，防白闪
    let (script, color) = theme_bootstrap(app);
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html#/settings".into()))
        .title(window_title("DSHDesktop 设置", "DSHDesktop Settings"))
        .inner_size(760.0, 700.0)
        .center()
        .visible(false)
        .initialization_script(&script)
        .background_color(color)
        .build();
}

fn open_plugins(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("plugins") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // 创建时隐藏：页面加载完成（lib.rs on_page_load）再显示，防白闪
    let (script, color) = theme_bootstrap(app);
    let _ = WebviewWindowBuilder::new(app, "plugins", WebviewUrl::App("index.html#/plugins".into()))
        .title(window_title("DSHDesktop 插件管理", "DSHDesktop Plugins"))
        .inner_size(900.0, 680.0)
        .center()
        .visible(false)
        .initialization_script(&script)
        .background_color(color)
        .build();
}

fn open_skills(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("skills") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // 创建时隐藏：页面加载完成（lib.rs on_page_load）再显示，防白闪
    let (script, color) = theme_bootstrap(app);
    let _ = WebviewWindowBuilder::new(app, "skills", WebviewUrl::App("index.html#/skills".into()))
        .title(window_title("DSHDesktop 技能管理", "DSHDesktop Skills"))
        .inner_size(860.0, 640.0)
        .center()
        .visible(false)
        .initialization_script(&script)
        .background_color(color)
        .build();
}

fn open_mcp(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("mcp") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // 创建时隐藏：页面加载完成（lib.rs on_page_load）再显示，防白闪
    let (script, color) = theme_bootstrap(app);
    let _ = WebviewWindowBuilder::new(app, "mcp", WebviewUrl::App("index.html#/mcp".into()))
        .title(window_title("DSHDesktop MCP 管理", "DSHDesktop MCP Manager"))
        .inner_size(900.0, 680.0)
        .center()
        .visible(false)
        .initialization_script(&script)
        .background_color(color)
        .build();
}

fn open_remote(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("remote") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // 创建时隐藏：页面加载完成（lib.rs on_page_load）再显示，防白闪
    let (script, color) = theme_bootstrap(app);
    let _ = WebviewWindowBuilder::new(app, "remote", WebviewUrl::App("index.html#/remote".into()))
        .title(window_title("DSHDesktop 远程访问", "DSHDesktop Remote Access"))
        .inner_size(460.0, 600.0)
        .center()
        .visible(false)
        .initialization_script(&script)
        .background_color(color)
        .build();
}

pub(crate) fn quit_app(app: &AppHandle) {
    let Some(state) = app.try_state::<SharedState>() else {
        app.exit(0);
        return;
    };
    let proc = state.process.clone();
    let remote = app.try_state::<RemoteManager>().map(|s| s.inner().clone());
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // 先关远程访问（杀 cloudflared 进程树 + 停鉴权代理），链接即刻失效
        if let Some(rm) = remote {
            rm.stop().await;
        }
        proc.stop().await;
        // 给监督循环留出杀进程树的时间，超时兜底强制退出
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        handle.exit(0);
    });
}
