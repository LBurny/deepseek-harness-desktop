use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::watch;

pub mod commands;
pub mod diagnostics;
pub mod download;
pub mod i18n;
pub mod mcp;
pub mod notify;
pub mod platform;
pub mod plugins;
pub mod port;
pub mod picker;
pub mod pickerpatch;
pub mod presets;
pub mod process;
pub mod progress;
pub mod remote;
pub mod runtime;
pub mod settings;
pub mod skills;
pub mod theme;
pub mod tray;
pub mod upstream;
pub mod update;
pub mod welcome;
pub mod zoom;

use notify::{Notification, NotifySink, NotifySource};
use process::{DshState, ProcessEvent};
use progress::ProgressPayload;

pub fn run() {
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
                    append_debug_line(
                        &p.runtime_base_dir().join("events.log"),
                        &format!(
                            "[{}] toast activated (protocol) -> show main",
                            local_stamp()
                        ),
                    );
                }
            }
            // 必须走 tray::show_main——它内含 platform bring_to_front（两段择时 +
            // AttachThreadInput + TOPMOST 抖动）。裸 show+set_focus 在前台锁下
            // 无声失败：窗口可见时被浏览器等前台应用压在原地（机器 B 实测"在
            // 浏览器下面"——这里曾内联三件套，根本没调到 show_main，加固全落空）
            tray::show_main(app);
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
            commands::get_shell_ui_state,
            commands::get_status,
            commands::restart_dsh,
            commands::get_recent_logs,
            commands::open_log_file,
            commands::get_autostart,
            commands::set_autostart,
            commands::get_bootstrap_error,
            commands::is_first_launch,
            zoom::zoom_ui,
            settings::get_shell_settings,
            settings::set_shell_settings,
            settings::preview_completion_sound,
            skills::list_skills,
            skills::list_import_sources,
            skills::import_skills,
            skills::set_skill_enabled,
            skills::delete_skill,
            skills::inspect_zip_skills,
            skills::import_zip_skills,
            mcp::list_mcp_servers,
            mcp::upsert_mcp_server,
            mcp::set_mcp_enabled,
            mcp::delete_mcp_server,
            mcp::list_mcp_import_sources,
            mcp::import_mcp_servers,
            plugins::get_plugin_status,
            plugins::list_plugins,
            plugins::search_plugins,
            plugins::install_plugin,
            plugins::uninstall_plugin,
            plugins::update_plugins,
            remote::start_remote,
            remote::stop_remote,
            remote::get_remote_status,
            remote::copy_remote_link,
            remote::get_remote_qr,
            remote::reset_remote_link,
            update::check_update,
            update::download_update,
            update::install_update,
            update::open_update_page,
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
                "settings" | "diagnostics" | "skills" | "plugins" | "mcp" | "remote"
            ) {
                // 新建窗口的 DWM 标题栏属性来自系统主题，与 dsh 解析主题可能相反
                // （系统浅色+dsh 深色）；首个可见帧前落对，标题栏出生即正确
                if let Some(w) = webview.app_handle().get_webview_window(webview.label()) {
                    theme::apply_before_show(webview.app_handle(), &w);
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
                    theme::apply_before_show(webview.app_handle(), &w);
                }
                let _ = webview.window().show();
                let _ = webview.window().set_focus();
                // 每次整页加载后重注入（SPA 内导航不重载页面，不会重复触发）。
                // 钩子内嵌当前快捷键设置；manage 之前的首帧用默认设置兜底
                let settings = webview
                    .app_handle()
                    .try_state::<settings::SettingsState>()
                    .map(|s| s.get())
                    .unwrap_or_default();
                let _ = webview.eval(zoom::hook_js(&settings));
                // 启动首帧补应用持久化缩放；也兜住 WebView2 重建后 zoom 丢失
                if let Some(state) = webview.app_handle().try_state::<zoom::ZoomState>() {
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
            WebviewWindowBuilder::new(&handle, "main", WebviewUrl::App("index.html".into()))
                .title("DSHDesktop")
                .inner_size(1100.0, 780.0)
                .min_inner_size(900.0, 600.0)
                .center()
                .visible(false)
                .on_download(download::handler(download_log))
                .build()?;
            tray::setup_tray(&handle)?;
            handle.manage(diagnostics::BootstrapInfo::default());
            handle.manage(platform.clone());
            handle.manage(zoom::ZoomState::new(platform.runtime_base_dir()));
            handle.manage(settings::SettingsState::new(platform.runtime_base_dir()));
            // 技能管理的根目录 = 壳注入给 dsh 的 DSH_HOME（与 runtime.rs 的 home 同源）
            handle.manage(skills::SkillsHome(platform.runtime_base_dir().join("dsh-home")));
            // 自动导入独立 dsh 默认目录（~/.dsh/skills）的技能：每次启动只补新技能，
            // 已见过的记在 .skills-seeded，用户在壳里删掉的不会复活
            skills::seed_from_default_dsh_home(&platform.runtime_base_dir().join("dsh-home"));
            // MCP 同理：同步 ~/.dsh 两个 cordis.patch.yml 层里的 dsh-mcp-client 条目，
            // marker .mcp-seeded 防复活；壳侧管理状态与技能同根（McpHome）
            handle.manage(mcp::McpHome(platform.runtime_base_dir().join("dsh-home")));
            mcp::seed_from_default_dsh_home(&platform.runtime_base_dir().join("dsh-home"));
            let version = app.package_info().version.to_string();
            let home_url = app
                .get_webview_window("main")
                .and_then(|w| w.url().ok())
                .unwrap_or_else(|| Url::parse("http://tauri.localhost/").unwrap());

            // dsh 就绪端口通道：Ready（含重启后）时更新，通知 WS 订阅器
            let (port_tx, port_rx) = watch::channel::<Option<u16>>(None);
            // toast 点击激活链路（URL Protocol + AUMID 显示名）开机自检，失败落
            // events.log；幂等且 exe 换路径后自动纠正
            notify::toast::ensure_activation_registered(&handle);
            // 事件调试日志路径（与 diagnostics 用的同一份：runtime_base_dir/events.log）
            let notify_log = platform.runtime_base_dir().join("events.log");
            let sink_handle = handle.clone();
            let sink_platform = platform.clone();
            let sink: NotifySink = Arc::new(move |n: Notification| {
                // 前台 = 本应用任一窗口处于聚焦态（主窗口可见但失焦 = 用户已切走，算后台）；
                // 各类型按自己的规则（开关 + 时机）决定是否打扰
                let foreground = ["main", "settings", "diagnostics", "skills", "plugins", "mcp", "remote"]
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
                        append_debug_line(&log, &line);
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
                                local_stamp(),
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
            });
            // mux（会话事件 → 通知/标题）+ host（子代理标记）双下行流，共享 SessionBook
            let book = Arc::new(std::sync::Mutex::new(notify::SessionBook::default()));
            let mux_book = book.clone();
            let mux_handler: notify::FrameHandler = Arc::new(move |frame, sink| {
                notify::handle_mux_frame(frame, sink, &mux_book);
            });
            let host_book = book.clone();
            let host_handler: notify::FrameHandler = Arc::new(move |frame, _| {
                notify::handle_host_frame(frame, &host_book);
            });
            let reconnect_book = book.clone();
            tauri::async_runtime::spawn(
                Box::new(notify::ws::WsSource {
                    path: upstream::EVENTS_MUX_PATH,
                    handler: mux_handler,
                    on_connect: None,
                })
                .run(sink.clone(), port_rx.clone()),
            );
            tauri::async_runtime::spawn(
                Box::new(notify::ws::WsSource {
                    path: upstream::EVENTS_HOST_PATH,
                    handler: host_handler,
                    on_connect: Some(Arc::new(move || {
                        reconnect_book.lock().unwrap().clear_subagents();
                    })),
                })
                .run(sink, port_rx.clone()),
            );

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
                let pct = progress::copy_percent(copied, total);
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
            let paths = match runtime::ensure_runtime(
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
                    handle.state::<diagnostics::BootstrapInfo>().set_error(msg.clone());
                    let _ = handle.emit(
                        "dsh-progress",
                        ProgressPayload::new("error", msg, None),
                    );
                    return Ok(());
                }
            };
            let deployed = deployed.load(Ordering::SeqCst);
            // 插件管理：node/dsh/pnpm 全部来自壳分发运行时（与 DshProcess 同源路径）
            handle.manage(plugins::PluginsHome::new(
                paths.node_exe.clone(),
                paths.dsh_bin.clone(),
                paths.home.clone(),
            ));

                        // 首启播种主题：settings.yaml 不存在时按系统深浅色预写 ui-theme.preference，
            // 否则 dsh 缺省渲染浅色而壳标题栏跟随系统（深色时不一致）。
            // 必须在 spawn_supervised 之前，dsh 首次启动即读到。
            theme::seed_theme_preference(&paths.home, platform.system_dark_mode());
            // 本地页面的主题/语言快照：先落库再启动关注循环（循环首轮即广播解析值）
            handle.manage(theme::ShellUiState::new(platform.as_ref(), &paths.home));
            theme::spawn_theme_follower(&handle, platform.clone(), paths.home.clone());
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
            // 内测声明豁免播种：预写 ui-onboarding.welcomeNoticeVersion（当前文案
            // 版本提取自运行时 client.js），否则 dsh 在未确认时每次启动弹"内测声明"
            // 对话框。须在主题播种（它只在文件缺失时写）之后、spawn_supervised 之前；
            // 失败只记 events.log 不阻断启动（回退为 dsh 原生弹一次）。
            match welcome::seed_welcome_notice(&paths.home, &paths.dsh_bin) {
                Ok(welcome::WelcomeOutcome::AlreadySeeded) => {}
                Ok(welcome::WelcomeOutcome::Seeded) => {
                    append_debug_line(&debug_log, "welcome: seeded notice ack")
                }
                Err(e) => append_debug_line(&debug_log, &format!("welcome: seed failed: {e}")),
            }
            // 目录选择器钉 browse：native 是弹在电脑屏幕上的系统对话框，手机远程端
            // 不可见不可用（新建项目选不了文件夹）。必须在 spawn_supervised 之前完成；
            // 失败只记 events.log 不阻断启动（回退现状）。
            match picker::ensure_browse_picker(&paths.home) {
                Ok(picker::PickerOutcome::AlreadyPinned) => {}
                Ok(picker::PickerOutcome::Pinned) => {
                    append_debug_line(&debug_log, "picker: pinned browse interaction")
                }
                Err(e) => append_debug_line(&debug_log, &format!("picker: pin failed: {e}")),
            }
            // browse 选择器运行时补丁：盘符哨兵层级 + 隐藏条目默认显示
            // （细节见 pickerpatch.rs 头注）。客户端/ host 签名门控、整组停手；
            // 必须在 spawn_supervised 之前完成，失败只记 events.log。
            let browse_outcome = pickerpatch::patch_browse_picker(&paths);
            if browse_outcome != pickerpatch::BrowsePatchOutcome::AlreadyPatched {
                append_debug_line(
                    &debug_log,
                    &format!("pickerpatch: browse drives/hidden -> {browse_outcome:?}"),
                );
            }
            // block_on 提供 tokio runtime 上下文，spawn_supervised 内部的 tokio::spawn 依赖它
            let proc = tauri::async_runtime::block_on(async {
                process::DshProcess::spawn_supervised(
                    platform.clone(),
                    paths.clone(),
                    move |event| {
                        append_debug_log(&debug_log, &event);
                        bridge_event(&emit_handle, &nav_home, &port_tx, deployed, event);
                    },
                )
            });
            // dsh-home 先克隆出来：paths 随 SharedState move，远程代理的
            // "项目"标签端点（project.rs）要靠它解析 storages/workspace.json
            let dsh_home = paths.home.clone();
            handle.manage(diagnostics::SharedState {
                process: proc,
                runtime: paths,
                version,
                home_url,
                first_launch,
            });
            // 远程访问管理器：托盘/命令驱动 start/stop；状态变更广播给前端并记事件日志
            handle.manage(remote::RemoteManager::new(
                platform.clone(),
                cloudflared_exe,
                vec![],
                remote_work_dir,
                dsh_home,
                port_rx,
                Box::new(move |ev| match ev {
                    // 链接即凭据：日志只记非敏感字段，隧道输出过 token 脱敏
                    remote::RemoteEvent::Log(l) => {
                        append_debug_line(&remote_log, &remote::redact_token(&l))
                    }
                    remote::RemoteEvent::Status(st) => {
                        append_debug_line(
                            &remote_log,
                            &format!(
                                "Remote: phase={} url={:?} error={:?} proxy_port={:?}",
                                st.phase, st.url, st.error, st.proxy_port
                            ),
                        );
                        tray::update_remote_items(&remote_handle, &st.phase);
                        // Up/Error 给 toast（其余状态变化是中间态，不打扰）；
                        // toast 会留在系统通知中心，正文不带链接，只提示去托盘复制
                        match st.phase.as_str() {
                            "up" => {
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
            ));
            // 启动时检查更新（设置默认关）：异步查 GitHub，有新版弹 toast 指向
            // 其它设置页；失败只记 events.log，不打断启动流程
            if handle
                .state::<settings::SettingsState>()
                .get()
                .check_update_on_launch
            {
                let update_handle = handle.clone();
                let update_log = platform.runtime_base_dir().join("events.log");
                tauri::async_runtime::spawn(async move {
                    update::check_on_launch(update_handle, update_log).await;
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
                        .try_state::<settings::SettingsState>()
                        .map(|s| matches!(s.get().close_behavior, settings::CloseBehavior::Quit))
                        .unwrap_or(false);
                    if quit {
                        tray::quit_app(window.app_handle());
                    } else {
                        let _ = window.hide();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running DSHDesktop");
}

/// 追加一行到调试日志；超过 1MB 时截断重来（只用于现场诊断，不求完备）。
pub(crate) fn append_debug_line(path: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > 1024 * 1024 {
            let _ = std::fs::remove_file(path);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}


/// 本地时间戳 HH:MM:SS.mmm：声音链路诊断用——用户在界面上"点第几下没声音"
/// 需要与日志行逐条对齐，无时间戳无法对应（Windows GetLocalTime，其它平台退
/// 化为 UNIX 秒）。
pub(crate) fn local_stamp() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st = SYSTEMTIME {
            wYear: 0,
            wMonth: 0,
            wDayOfWeek: 0,
            wDay: 0,
            wHour: 0,
            wMinute: 0,
            wSecond: 0,
            wMilliseconds: 0,
        };
        unsafe { GetLocalTime(&mut st) };
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        )
    }
    #[cfg(not(windows))]
    {
        let s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("unix:{s}")
    }
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

fn append_debug_log(path: &PathBuf, event: &ProcessEvent) {
    let line = match event {
        ProcessEvent::StateChanged(s) => format!("{s:?}"),
        ProcessEvent::Log(l) => l.clone(),
    };
    append_debug_line(path, &line);
}

fn bridge_event(
    handle: &tauri::AppHandle,
    home_url: &Url,
    port_tx: &watch::Sender<Option<u16>>,
    deployed: bool,
    event: ProcessEvent,
) {
    match event {
        ProcessEvent::Log(line) => {
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
                let _ = port_tx.send(Some(port));
                let _ = handle.emit("dsh-ready", serde_json::json!({ "port": port }));
                let _ = handle.emit(
                    "dsh-progress",
                    ProgressPayload::new("ready", i18n::pick("正在打开界面…", "Opening interface…"), Some(100)),
                );
                if let Some(w) = handle.get_webview_window("main") {
                    if let Ok(url) = Url::parse(&format!("http://127.0.0.1:{port}/")) {
                        let _ = w.navigate(url);
                    }
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
