//! 启动期环境自愈与播种。

pub mod patches;
pub mod plugins;
pub mod preseed;
pub mod progress;

/// 启动期环境自愈与播种总入口：补丁 → pnpm store 自愈 → 预装插件播种。
/// 全部失败只记 events.log；必须在 spawn_supervised 之前调用。
pub fn run_all(
    paths: &crate::dsh::runtime::RuntimePaths,
    preseed_src: Option<std::path::PathBuf>,
    log: &std::path::Path,
) {
    patches::run_all(paths, log);
    // pnpm store 迁移自愈：profile 的 node_modules 链接在创建时的 store，
    // 用户全局 storeDir 变更后内置 pnpm 解析到新 store，任何装/卸/更新都被
    // ERR_PNPM_UNEXPECTED_STORE 拒绝（0.5.21 用户实锤：store 迁 F: 后 dsh
    // 插件页装包全挂）。自愈 = rename 备用 → 内置 pnpm install 按清单重链 →
    // 成功删备份 / 失败回滚 rename。任何结果只落 events.log 不阻断启动；
    // 必须在 preseed 之前（新插件的 add 同样会被旧 store 拒）。
    let plugins_home = plugins::PluginsHome::new(
        paths.node_exe.clone(),
        paths.dsh_bin.clone(),
        paths.home.clone(),
    );
    let store_heal = plugins::heal_profile_store(&plugins_home);
    if !matches!(
        store_heal,
        plugins::StoreHealOutcome::Matched | plugins::StoreHealOutcome::NoProfile
    ) {
        crate::logging::append_debug_line(log, &format!("pnpm store heal: {store_heal:?}"));
    }
    // 预安装插件播种（/init 命令等）：随包插件首启种入 profile 并经官方
    // `dsh plugin add` 挂层；用户在插件面板删除后不复活（preseed.rs 头注）。
    // 必须在 spawn_supervised 之前，dsh 首次启动即挂载；失败只记 events.log。
    // dev 模式 tauri 不拷贝 bundle.resources，源目录缺失时静默无操作。
    if let Some(src) = preseed_src {
        match preseed::seed_preinstalled_plugins(&plugins_home, &src) {
            Ok(report) if !report.is_quiet() => crate::logging::append_debug_line(
                log,
                &format!("preseed: plugins -> {report:?}"),
            ),
            Ok(_) => {}
            Err(e) => crate::logging::append_debug_line(log, &format!("preseed: {e}")),
        }
    }
}
