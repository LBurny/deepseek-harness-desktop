//! 启动期补丁：签名门控 + marker 幂等，顺序即契约。

pub mod mcpgate;
pub mod oiacache;
pub mod picker;
pub mod pickerpatch;
pub mod welcome;

/// 按序跑全部启动补丁：顺序即契约（与 0.5.24 前 lib.rs 内联顺序一致），
/// 每步签名门控 + marker 幂等，失败只记 events.log 不阻断启动。
/// 补丁随上游跟版增删只动本文件（revealshow/locks 退役先例）。
pub(crate) fn run_all(paths: &crate::dsh::runtime::RuntimePaths, log: &std::path::Path) {
    // 内测声明豁免播种：预写 ui-onboarding.welcomeNoticeVersion（当前文案
    // 版本提取自运行时 client.js），否则 dsh 在未确认时每次启动弹"内测声明"
    // 对话框。须在主题播种（它只在文件缺失时写）之后、spawn_supervised 之前；
    // 失败只记 events.log 不阻断启动（回退为 dsh 原生弹一次）。
    match welcome::seed_welcome_notice(&paths.home, &paths.dsh_bin) {
        Ok(welcome::WelcomeOutcome::AlreadySeeded) => {}
        Ok(welcome::WelcomeOutcome::Seeded) => {
            crate::logging::append_debug_line(log, "welcome: seeded notice ack")
        }
        Err(e) => crate::logging::append_debug_line(log, &format!("welcome: seed failed: {e}")),
    }
    // 目录选择器钉 browse：native 是弹在电脑屏幕上的系统对话框，手机远程端
    // 不可见不可用（新建项目选不了文件夹）。必须在 spawn_supervised 之前完成；
    // 失败只记 events.log 不阻断启动（回退现状）。
    match picker::ensure_browse_picker(&paths.home) {
        Ok(picker::PickerOutcome::AlreadyPinned) => {}
        Ok(picker::PickerOutcome::Pinned) => {
            crate::logging::append_debug_line(log, "picker: pinned browse interaction")
        }
        Err(e) => crate::logging::append_debug_line(log, &format!("picker: pin failed: {e}")),
    }
    // browse 选择器运行时补丁：盘符哨兵层级 + 隐藏条目默认显示
    // （细节见 pickerpatch.rs 头注）。客户端/ host 签名门控、整组停手；
    // 必须在 spawn_supervised 之前完成，失败只记 events.log。
    let browse_outcome = pickerpatch::patch_browse_picker(paths);
    if browse_outcome != pickerpatch::BrowsePatchOutcome::AlreadyPatched {
        crate::logging::append_debug_line(
            log,
            &format!("pickerpatch: browse drives/hidden -> {browse_outcome:?}"),
        );
    }
    // MCP 就绪门禁补丁：dsh 就绪行刻意等全部插件 settle（含 MCP 首次
    // 连接），npx 型 MCP 的 registry 解析把启动拖到数十秒。签名门控、
    // 失败只记 events.log（回退上游行为）；必须在 spawn_supervised 之前。
    let mcpgate_outcome = mcpgate::patch_mcp_ready_gate(paths);
    if mcpgate_outcome != mcpgate::McpGateOutcome::AlreadyPatched {
        crate::logging::append_debug_line(
            log,
            &format!("mcpgate: nonblocking ready -> {mcpgate_outcome:?}"),
        );
    }
    // open-in-app 可用性缓存补丁：按钮等每进程一次的 ~2.9s 冷探测，
    // 持久化后第二次起首帧即渲染（细节见 upstream.rs 段注）。签名门控、
    // 失败只记 events.log；必须在 spawn_supervised 之前。
    let oiacache_outcome = oiacache::patch_oia_apps_cache(paths);
    if oiacache_outcome != oiacache::OiaCacheOutcome::AlreadyPatched {
        crate::logging::append_debug_line(
            log,
            &format!("oiacache: persist apps -> {oiacache_outcome:?}"),
        );
    }
}
