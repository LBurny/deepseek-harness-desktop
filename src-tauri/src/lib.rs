use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::watch;

pub mod bootstrap;
pub mod dsh;
pub mod eventbridge;
pub mod features;
pub mod i18n;
pub mod logging;
pub mod notify;
pub mod patchstore;
pub mod platform;
pub mod redact;
pub mod remote;
pub mod ui;

use dsh::dsh_session::DshCreds;
use bootstrap::progress::ProgressPayload;

pub fn run() {
    // 诊断 CDP 开关（0.5.11）：runtime_base_dir/debug-cdp 空文件存在 → WebView2
    // 浏览器进程带 --remote-debugging-port=9222 启动，可挂 DevTools 协议看主窗口
    // 网络层（431 实锤靠这招）。必须在任何 webview 创建前 set_var——WebView2 创建
    // 浏览器进程时读一次。9222 对本机所有进程开放页面调试（能读到页面内 token），
    // 仅排障期间放置 marker，排完删掉。
    {
        let p: Arc<dyn platform::Platform> = platform::current().into();
        let dir = p.runtime_base_dir();
        if dir.join("debug-cdp").exists() {
            std::env::set_var(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                "--remote-debugging-port=9222",
            );
            let _ = std::fs::create_dir_all(&dir);
            crate::logging::append_debug_line(
                &dir.join("events.log"),
                "[dshdesktop] debug-cdp marker found: WebView2 CDP listening on 9222",
            );
        }
    }
    // 协议激活的二次实例：Windows 响应用户点击 toast 拉起本进程，它手握前台
    // 权限，但随即被下面的 single-instance 插件拦截退出——权限随之作废。赶在
    // 拦截前把前台权限广播出去（ASFW_ANY），运行中实例随后 bring_to_front 的
    // SetForegroundWindow 才是合法调用（前台锁只认前台进程或其授权方；
    // Chromium/VSCode 的单实例激活同款机制）。普通启动/非前台进程调用只是
    // 返回失败，无害。
    #[cfg(windows)]
    if std::env::args().any(|a| a == notify::toast::PROTOCOL_ARG) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
            );
        }
    }
    tauri::Builder::default()
        // 单实例必须最先注册；第二次启动时聚焦已有主窗口（用户双开，或点击系统
        // 通知 toast 的协议激活——后者带 --dshdesktop-protocol 标记，落一行日志）
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.iter().any(|a| a == notify::toast::PROTOCOL_ARG) {
                if let Some(p) = app.try_state::<std::sync::Arc<dyn platform::Platform>>() {
                    crate::logging::append_debug_line(
                        &p.runtime_base_dir().join("events.log"),
                        &format!(
                            "[{}] toast activated (protocol) -> show main",
                            crate::logging::local_stamp()
                        ),
                    );
                }
            }
            // 必须走 ui::tray::show_main——它内含 platform bring_to_front（两段择时 +
            // AttachThreadInput + TOPMOST 抖动）。裸 show+set_focus 在前台锁下
            // 无声失败：窗口可见时被浏览器等前台应用压在原地（机器 B 实测"在
            // 浏览器下面"——这里曾内联三件套，根本没调到 show_main，加固全落空）
            ui::tray::show_main(app);
        }))
        .plugin(tauri_plugin_notification::init())
        // 窗口几何记忆：缩放/移动实时入缓存，退出时落盘，下次启动建窗时恢复。
        // 不含 VISIBLE——托盘隐藏态下退出会把"隐藏"记住，下次启动主窗口不出来
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // 原生文件对话框（技能管理"本地导入 ZIP"选文件用）
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            ui::state::get_shell_ui_state,
            features::diagnostics::get_status,
            features::diagnostics::restart_dsh,
            features::diagnostics::get_recent_logs,
            features::diagnostics::get_last_boot_timing,
            features::diagnostics::open_log_file,
            features::diagnostics::get_autostart,
            features::diagnostics::set_autostart,
            features::diagnostics::get_bootstrap_error,
            features::diagnostics::is_first_launch,
            ui::zoom::zoom_ui,
            ui::pagebridge::report_page_error,
            ui::pagebridge::ui_boot_ok,
            features::settings::get_shell_settings,
            features::settings::set_shell_settings,
            features::settings::preview_completion_sound,
            features::skills::list_skills,
            features::skills::list_import_sources,
            features::skills::import_skills,
            features::skills::set_skill_enabled,
            features::skills::delete_skill,
            features::skills::inspect_zip_skills,
            features::skills::import_zip_skills,
            features::mcp::list_mcp_servers,
            features::mcp::upsert_mcp_server,
            features::mcp::set_mcp_enabled,
            features::mcp::delete_mcp_server,
            features::mcp::list_mcp_import_sources,
            features::mcp::import_mcp_servers,
            remote::start_remote,
            remote::stop_remote,
            remote::get_remote_status,
            remote::copy_remote_link,
            remote::get_remote_qr,
            remote::reset_remote_link,
            features::update::check_update,
            features::update::download_update,
            features::update::install_update,
            features::update::open_update_page,
        ])
        .on_page_load(|webview, payload| {
            if !matches!(
                payload.event(),
                tauri::webview::PageLoadEvent::Finished
            ) {
                return;
            }
            // 设置/诊断窗口创建时是隐藏的（防白闪），首帧加载完成后显示并聚焦。
            // 注意此处回调参数是 &Webview：show() 只控制 webview 控件可见性，
            // 必须经 .window() 拿到 Window 才能把窗口本身显示出来
            if matches!(
                webview.label(),
                "settings" | "diagnostics" | "skills" | "mcp" | "remote"
            ) {
                // 新建窗口的 DWM 标题栏属性来自系统主题，与 dsh 解析主题可能相反
                // （系统浅色+dsh 深色）；首个可见帧前落对，标题栏出生即正确
                if let Some(w) = webview.app_handle().get_webview_window(webview.label()) {
                    ui::theme::apply_before_show(webview.app_handle(), &w);
                }
                let _ = webview.window().show();
                let _ = webview.window().set_focus();
                return;
            }
            // 缩放钩子只注入主窗口（splash 与远程 dsh UI 两个阶段的同一窗口）；
            // 诊断/设置窗口不注入——否则设置窗口里录制 Ctrl+Shift+= 会先被钩子拦截
            if webview.label() == "main" {
                // 主窗口创建时隐藏（tauri.conf visible:false）：window-state 的 restore
                // 在 window_created 时排队执行，早于首个 Finished，此刻几何已是记忆值——
                // 直接 show 就不会有"默认尺寸闪一帧再跳变"（探针实测默认尺寸会可见 ~370ms）
                if let Some(w) = webview.app_handle().get_webview_window("main") {
                    ui::theme::apply_before_show(webview.app_handle(), &w);
                }
                let _ = webview.window().show();
                let _ = webview.window().set_focus();
                // 每次整页加载后重注入（SPA 内导航不重载页面，不会重复触发）。
                // 钩子内嵌当前快捷键设置；manage 之前的首帧用默认设置兜底
                let settings = webview
                    .app_handle()
                    .try_state::<features::settings::SettingsState>()
                    .map(|s| s.get())
                    .unwrap_or_default();
                let _ = webview.eval(ui::zoom::hook_js(&settings));
                // 启动首帧补应用持久化缩放；也兜住 WebView2 重建后 zoom 丢失
                if let Some(state) = webview.app_handle().try_state::<ui::zoom::ZoomState>() {
                    let _ = webview.set_zoom(state.get());
                }
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let platform: Arc<dyn platform::Platform> = platform::current().into();
            // 主窗口由代码创建（tauri.conf 的 windows 已清空）：on_download 只能挂在
            // builder 上，conf 声明的窗口无法附加——没有它 WebView2 把 Session log 等
            // 下载静默吞掉（wry 默认放行且抑制下载 UI，用户看不到文件去向）。
            // 几何/标题与原 conf 一致；visible(false)+center() 语义不变，window-state
            // 的 restore 仍在创建事件排队、早于首个可见帧（托盘按需窗口同款模式）。
            let download_log = platform.runtime_base_dir().join("events.log");
            // window.open/带 target=_blank 的锚点外链交系统浏览器：wry 对新窗口请求
            // 无 handler 默认 SetHandled(true) 吞掉——dsh 网页搜索结果链接、消息里
            // markdown 外链点了"没反应"的根因。放行 http/https 经 open_url 打开，
            // WebView2 子窗一律 Deny（壳内永不弹新窗）；scheme 门在 update.rs。
            let extlink_log = download_log.clone();
            WebviewWindowBuilder::new(&handle, "main", WebviewUrl::App("index.html".into()))
                .title("DSHDesktop")
                .inner_size(1100.0, 780.0)
                .min_inner_size(900.0, 600.0)
                .center()
                .visible(false)
                // document-start 注入每次导航都跑、先于页面脚本、绕页面 CSP：
                // 错误桥（dsh UI 报错落 events.log）+ #root 挂载心跳，细节见
                // pagebridge.rs 头注
                .initialization_script(ui::pagebridge::INIT_SCRIPT)
                .on_download(ui::download::handler(download_log))
                .on_new_window(move |url, _| {
                    if features::update::external_open_allowed(&url) {
                        let host = url.host_str().unwrap_or("?").to_owned();
                        match features::update::open_url(url.as_str()) {
                            Ok(()) => crate::logging::append_debug_line(
                                &extlink_log,
                                &format!("[extlink] 系统浏览器打开 {host}"),
                            ),
                            Err(e) => {
                                crate::logging::append_debug_line(&extlink_log, &format!("[extlink] 打开失败: {e}"))
                            }
                        }
                    }
                    tauri::webview::NewWindowResponse::Deny
                })
                .build()?;
            ui::tray::setup_tray(&handle)?;
            handle.manage(features::diagnostics::BootstrapInfo::default());
            handle.manage(platform.clone());
            handle.manage(ui::zoom::ZoomState::new(platform.runtime_base_dir()));
            handle.manage(features::settings::SettingsState::new(platform.runtime_base_dir()));
            // 技能管理的根目录 = 壳注入给 dsh 的 DSH_HOME（与 runtime.rs 的 home 同源）
            handle.manage(features::skills::SkillsHome(platform.runtime_base_dir().join("dsh-home")));
            // 自动导入独立 dsh 默认目录（~/.dsh/skills）的技能：每次启动只补新技能，
            // 已见过的记在 .skills-seeded，用户在壳里删掉的不会复活
            features::skills::seed_from_default_dsh_home(&platform.runtime_base_dir().join("dsh-home"));
            // MCP 同理：同步 ~/.dsh 两个 cordis.patch.yml 层里的 dsh-mcp-client 条目，
            // marker .mcp-seeded 防复活；壳侧管理状态与技能同根（McpHome）
            handle.manage(features::mcp::McpHome(platform.runtime_base_dir().join("dsh-home")));
            features::mcp::seed_from_default_dsh_home(&platform.runtime_base_dir().join("dsh-home"));
            let version = app.package_info().version.to_string();
            let home_url = app
                .get_webview_window("main")
                .and_then(|w| w.url().ok())
                .unwrap_or_else(|| Url::parse("http://tauri.localhost/").unwrap());

            // dsh 凭据通道：Ready（含重启后）时更新，通知 WS 订阅器与远程代理。
            // 0.1.2 起 token 是鉴权前提，port+token 合流成 DshCreds 一并下发
            let (creds_tx, creds_rx) = watch::channel::<Option<Arc<DshCreds>>>(None);
            // 0.1.2 BrowserAuth launch token 通道：stdout 就绪行捕获（Ready 时必已就绪）
            let (token_tx, token_rx) = watch::channel::<Option<Arc<str>>>(None);
            // toast 点击激活链路（URL Protocol + AUMID 显示名）开机自检，失败落
            // events.log；幂等且 exe 换路径后自动纠正
            notify::toast::ensure_activation_registered(&handle);
            // 事件调试日志路径（与 diagnostics 用的同一份：runtime_base_dir/events.log）
            let notify_log = platform.runtime_base_dir().join("events.log");
            let mux_log = notify_log.clone();
            let sink = notify::sink::build_sink(&handle, platform.clone(), notify_log);
            notify::sink::spawn_mux(&handle, sink, creds_rx.clone(), mux_log);

            let source = std::env::var_os("DSHDESKTOP_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    platform.resource_runtime_dir(&app.path().resource_dir().unwrap_or_default())
                });

            // 首启判定：ensure_runtime 首次运行才会创建 dsh-home；此前不存在即首启
            let first_launch = !platform.runtime_base_dir().join("dsh-home").exists();

            let _ = handle.emit(
                "dsh-progress",
                ProgressPayload::new("runtime", i18n::pick("正在准备运行时…", "Preparing runtime…"), Some(0)),
            );

            // 回退部署（只读安装目录）时按字节进度 emit；节流：百分比变化才发，
            // 避免复制数千个小文件时刷爆 IPC
            let deployed = Arc::new(AtomicBool::new(false));
            let last_pct = Arc::new(AtomicU8::new(0));
            let dep = deployed.clone();
            let lp = last_pct.clone();
            let copy_emit = handle.clone();
            let copy_cb = move |copied: u64, total: u64| {
                dep.store(true, Ordering::SeqCst);
                let pct = bootstrap::progress::copy_percent(copied, total);
                if pct != lp.swap(pct, Ordering::SeqCst) {
                    let _ = copy_emit.emit(
                        "dsh-progress",
                        ProgressPayload::new(
                            "runtime",
                            i18n::pick(
                                "正在部署运行时（仅首次安装需要复制依赖）…",
                                "Deploying runtime (only needed on first install)…",
                            ),
                            Some(pct),
                        ),
                    );
                }
            };
            let paths = match dsh::runtime::ensure_runtime(
                platform.as_ref(),
                &source,
                &version,
                Some(&copy_cb),
            ) {
                Ok(p) => p,
                Err(e) => {
                    // 不退出：记录错误供启动画面查询，窗口停在启动画面
                    let msg = i18n::pick(
                        format!("运行时就绪失败：{e}"),
                        format!("Runtime setup failed: {e}"),
                    );
                    handle.state::<features::diagnostics::BootstrapInfo>().set_error(msg.clone());
                    let _ = handle.emit(
                        "dsh-progress",
                        ProgressPayload::new("error", msg, None),
                    );
                    return Ok(());
                }
            };
            let deployed = deployed.load(Ordering::SeqCst);

            // 本地页面的主题/语言快照：先落库再启动关注循环（循环首轮即广播解析值）。
            // 主题/语言读 profile patch 条目（upstream.rs 设置存储段注）；首启主题
            // 播种已退役——0.2.0 起 dsh 缺省 preference=system，与壳缺省一致。
            handle.manage(ui::state::ShellUiState::new(platform.as_ref(), &paths.home));
            ui::theme::spawn_theme_follower(&handle, platform.clone(), paths.home.clone());
            let emit_handle = handle.clone();
            let nav_home = home_url.clone();
            // 远程访问用的运行时信息（paths 随后被 SharedState 取走，先克隆出来）
            let cloudflared_exe = paths.cloudflared_exe.clone();
            let remote_work_dir = paths.work_dir.clone();
            let remote_log = paths
                .home
                .parent()
                .map(|p| p.join("events.log"))
                .unwrap_or_else(|| PathBuf::from("events.log"));
            let remote_handle = handle.clone();
            // 事件调试日志：诊断面板之外的最后手段（面板本身依赖应用内交互才能看到）
            let debug_log = paths
                .home
                .parent()
                .map(|p| p.join("events.log"))
                .unwrap_or_else(|| PathBuf::from("events.log"));
            let preseed_src = handle
                .path()
                .resource_dir()
                .ok()
                .map(|d| dsh::runtime::strip_verbatim(&d).join("preseed-plugins"));
            bootstrap::run_all(&paths, preseed_src, &debug_log);
            // block_on 提供 tokio runtime 上下文，spawn_supervised 内部的 tokio::spawn 依赖它
            let proc = tauri::async_runtime::block_on(async {
                dsh::process::DshProcess::spawn_supervised(
                    platform.clone(),
                    paths.clone(),
                    token_tx,
                    move |event| {
                        crate::logging::append_debug_log(&debug_log, &event);
                        eventbridge::bridge_event(&emit_handle, &nav_home, &creds_tx, &token_rx, deployed, &debug_log, event);
                    },
                )
            });
            // dsh-home 先克隆出来：paths 随 SharedState move，远程代理的
            // "项目"标签端点（project.rs）要靠它解析 storages/workspace.json
            let dsh_home = paths.home.clone();
            handle.manage(features::diagnostics::SharedState {
                process: proc,
                runtime: paths,
                version,
                home_url,
                first_launch,
            });
            // 远程访问管理器：托盘/命令驱动 start/stop；状态变更广播给前端并记事件日志
            let remote_mgr = remote::RemoteManager::new(
                platform.clone(),
                cloudflared_exe,
                vec![],
                remote_work_dir,
                dsh_home,
                creds_rx,
                Box::new(move |ev| match ev {
                    // 链接即凭据：日志只记非敏感字段，隧道输出过 token 脱敏
                    remote::RemoteEvent::Log(l) => {
                        crate::logging::append_debug_line(&remote_log, &crate::redact::redact_token(&l))
                    }
                    remote::RemoteEvent::Status(st) => {
                        crate::logging::append_debug_line(
                            &remote_log,
                            &format!(
                                "Remote: phase={} url={:?} error={:?} proxy_port={:?}",
                                st.phase, st.url, st.error, st.proxy_port
                            ),
                        );
                        ui::tray::update_remote_items(&remote_handle, &st.phase);
                        // Up/Error 给 toast（其余状态变化是中间态，不打扰）；
                        // toast 会留在系统通知中心，正文不带链接，只提示去托盘复制
                        match st.phase.as_str() {
                            "up" => {
                                if st.resumed {
                                    // 自动恢复（收养复活，链接未变）不打扰用户：
                                    // 不弹 toast，仅落日志（用户反馈 0.5.8：重启应用
                                    // 自动连回上次链接属后台行为，弹窗是噪音）
                                    crate::logging::append_debug_line(
                                        &remote_log,
                                        "Remote: auto-resumed, toast skipped (log only)",
                                    );
                                } else {
                                    // 全新开启/重生换域名=链接已变，引导去托盘复制
                                    let _ = notify::toast::show(
                                        &remote_handle,
                                        &i18n::pick("远程访问已开启", "Remote access is on"),
                                        &i18n::pick(
                                            "链接已就绪，请从托盘菜单复制",
                                            "Link ready — copy it from the tray menu",
                                        ),
                                        notify::toast::ToastSound::Silent,
                                        None,
                                    );
                                }
                            }
                            "error" => {
                                let _ = notify::toast::show(
                                    &remote_handle,
                                    &i18n::pick("远程访问开启失败", "Remote access failed"),
                                    &st.error.clone().unwrap_or_default(),
                                    notify::toast::ToastSound::Silent,
                                    None,
                                );
                            }
                            _ => {}
                        }
                        let _ = remote_handle.emit("remote-status", st);
                    }
                }),
            );
            // 会话持久化：上次退出时远程开着 → 原地复活（收养存活隧道，链接不变）；
            // 复活失败内部回退全新开（域名换、token 沿用）。begin_resume 先同步
            // 占 Starting 并发事件，用户此刻手动点"开启"也只会幂等返回不双开
            if remote_mgr.has_session() {
                remote_mgr.begin_resume();
                let rm = remote_mgr.clone();
                tauri::async_runtime::spawn(async move {
                    rm.resume_or_start().await;
                });
            }
            handle.manage(remote_mgr);
            // 启动时检查更新（设置默认开）：异步查 GitHub，有新版弹 toast 指向
            // 其它设置页；失败只记 events.log，不打断启动流程
            if handle
                .state::<features::settings::SettingsState>()
                .get()
                .check_update_on_launch
            {
                let update_handle = handle.clone();
                let update_log = platform.runtime_base_dir().join("events.log");
                tauri::async_runtime::spawn(async move {
                    features::update::check_on_launch(update_handle, update_log).await;
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口关窗行为由设置决定：默认最小化到托盘（保持后台运行），
            // 也可配置为直接退出程序；诊断/设置窗口关窗 = 销毁
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let quit = window
                        .app_handle()
                        .try_state::<features::settings::SettingsState>()
                        .map(|s| matches!(s.get().close_behavior, features::settings::CloseBehavior::Quit))
                        .unwrap_or(false);
                    if quit {
                        ui::tray::quit_app(window.app_handle());
                    } else {
                        let _ = window.hide();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running DSHDesktop");
}
