//! 插件操作执行层：dsh plugin 官方入口的壳侧封装（npm/cordis 插件）。
//! 装/卸/更新走 `dsh plugin --profile web <pnpm args>`（对账逻辑归上游）；
//! pnpm 由壳内置（pnpm.cjs/pnpm.cmd，node.exe 同目录），spawn 时 PATH 前置。
//!
//! 只服务 preseed.rs 的预安装播种（marker 语义见 preseed.rs 头注）。壳自己的
//! 插件管理面板已随 0.5.21 移除——dsh 0.2.0 自带插件管理页（安装/配置/启停/
//! 运行时卸载），面板与配套 Tauri 命令（list/search/install/uninstall/update/
//! get_plugin_status）成为重复维护面，按"上游接管即退役"惯例删除。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 插件命令的执行环境（node/dsh/pnpm 全部来自壳分发运行时）。
pub struct PluginsHome {
    pub node_exe: PathBuf,
    pub dsh_bin: PathBuf,
    pub home: PathBuf,
    /// 存放 pnpm.cjs/pnpm.cmd 的目录（= node_exe 所在目录）。
    pub pnpm_dir: PathBuf,
    /// 串行锁：同一时刻只允许一个装/卸/更新操作（防并发写 profile）。
    pub busy: Mutex<()>,
}

impl PluginsHome {
    pub fn new(node_exe: PathBuf, dsh_bin: PathBuf, home: PathBuf) -> Self {
        let pnpm_dir = node_exe
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        Self { node_exe, dsh_bin, home, pnpm_dir, busy: Mutex::new(()) }
    }
    pub fn profile_dir(&self) -> PathBuf {
        crate::upstream::join_segments(&self.home, crate::upstream::PROFILE_DIR_SEGMENTS)
    }
    pub fn manifest_path(&self) -> PathBuf {
        self.profile_dir().join(crate::upstream::PROFILE_MANIFEST_FILE)
    }
    /// 内置 pnpm 入口脚本（fetch-runtime 产出：<pnpm_dir>/pnpm/bin/pnpm.cjs，
    /// 依赖同包 dist/，不能摊平到根目录）。
    pub fn pnpm_js(&self) -> PathBuf {
        self.pnpm_dir
            .join("pnpm")
            .join("bin")
            .join(crate::upstream::PNPM_JS_FILE)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginOpResult {
    pub exit_code: i32,
    pub output: String, // stdout+stderr 合并，截断
}

const OP_OUTPUT_CAP: usize = 200 * 1024;

fn validate_spec(spec: &str) -> Result<(), String> {
    let s = spec.trim();
    if s.is_empty() {
        return Err("包名不能为空".into());
    }
    if s.len() > 200 {
        return Err("包名过长（≤200 字符）".into());
    }
    if s.starts_with('-') {
        return Err("包名不能以 - 开头（防参数注入）".into());
    }
    Ok(())
}

/// 跑 `node bin.js plugin --profile web <args...>`：DSH_HOME 注入 + PATH 前置
/// pnpm 目录（dsh 内部 spawnSync("pnpm") 经 PATH+PATHEXT 解析到 pnpm.cmd）。
/// 无 shell（杜绝注入）；CREATE_NO_WINDOW（防闪控制台，同 process.rs）；
/// 子进程挂全局 Job Object（壳被杀时连带回收，防孤儿锁 profile）。
pub fn run_plugin_op(home: &PluginsHome, args: &[&str]) -> Result<PluginOpResult, String> {
    if !home.node_exe.is_file() {
        return Err(format!("运行时缺失：{}", home.node_exe.display()));
    }
    if !home.dsh_bin.is_file() {
        return Err(format!("运行时缺失：{}", home.dsh_bin.display()));
    }
    let pnpm_js = home.pnpm_js();
    let pnpm_cmd = home.pnpm_dir.join(crate::upstream::PNPM_CMD_FILE);
    if !pnpm_js.is_file() || !pnpm_cmd.is_file() {
        return Err("内置 pnpm 缺失（pnpm.cmd / pnpm\\bin\\pnpm.cjs）——请重装 DSHDesktop 或重跑 fetch-runtime.ps1".into());
    }
    let mut full_path = home.pnpm_dir.clone().into_os_string();
    full_path.push(";");
    full_path.push(std::env::var_os("PATH").unwrap_or_default());
    let mut cmd = tokio::process::Command::new(&home.node_exe);
    cmd.arg(&home.dsh_bin)
        .arg(crate::upstream::DSH_PLUGIN_SUBCOMMAND)
        .arg(crate::upstream::DSH_PLUGIN_PROFILE_FLAG)
        .arg(crate::upstream::DSH_WEB_PROFILE_NAME)
        .args(args)
        .env("DSH_HOME", &home.home)
        .env("PATH", &full_path);
    // wait_with_output 只读 pipe 出来的句柄；tokio spawn 默认继承父进程
    // stdio，不显式 pipe 则 output 恒为空（“看下方输出”永远没内容）。
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::platform::current().configure_child_command(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("启动 dsh plugin 失败：{e}"))?;
    if let Some(pid) = child.id() {
        crate::platform::current().register_child(pid);
    }
    let out = tauri::async_runtime::block_on(child.wait_with_output())
        .map_err(|e| format!("等待 dsh plugin 结束失败：{e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if text.len() > OP_OUTPUT_CAP {
        text.truncate(OP_OUTPUT_CAP);
    }
    Ok(PluginOpResult { exit_code: out.status.code().unwrap_or(-1), output: text })
}

// ── pnpm store 迁移自愈（0.5.22）────────────────────────
// profile 的 node_modules 链接在**创建时**的 store（node_modules/.modules.yaml
// 记录 storeDir）。用户全局 pnpm storeDir 变更后（迁盘、pnpm 默认路径变化），
// 内置 pnpm 解析到新 store，对旧链的 node_modules 做任何 install/update 都被
// ERR_PNPM_UNEXPECTED_STORE 拒绝——dsh 自带插件页装包全挂（0.5.21 实锤：
// 9/21 store 迁 F: 后 9/4 建的 profile 首次装包即报错）。自愈 = rename
// node_modules 备用 → 内置 pnpm install（按清单+lockfile 从新 store 重链）→
// 成功删备份；失败回滚 rename（node_modules 原样保留：dsh 照常可跑，只是装包
// 仍报错，现状不劣化）。任何结果只落 events.log，不阻断启动。

/// heal_profile_store 的结果（lib.rs 落 events.log）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreHealOutcome {
    /// 无 node_modules/.modules.yaml（首启或 profile 未初始化）——无可检
    NoProfile,
    /// storeDir 一致，无需处理
    Matched,
    /// `pnpm store path` 失败或输出为空——无法判定，不动盘（保守）
    CannotDetermine,
    /// 检测到 store 迁移，已按新 store 重链成功
    Healed { old_store: String, new_store: String },
    /// 重链失败：已回滚 rename，node_modules 原样保留
    RolledBack { detail: String },
}

/// store 路径比较归一化：去首尾空白与收尾分隔符、小写（Windows 不分大小写）。
fn norm_store_path(p: &str) -> String {
    p.trim()
        .trim_end_matches(['/', '\\'])
        .to_ascii_lowercase()
}

/// 从 .modules.yaml 提取 storeDir（pnpm 11 落 JSON 形态；缺失/损坏 = None）。
fn recorded_store_dir(nm_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(nm_dir.join(".modules.yaml")).ok()?;
    serde_yaml::from_str::<serde_yaml::Value>(&text)
        .ok()?
        .get("storeDir")?
        .as_str()
        .map(str::to_string)
}

/// 直接跑内置 pnpm 子命令（`store path` 探测 / `install` 重链）。
/// cwd = profile 目录（install 的落点）；不注入 DSH_HOME（pnpm 不需要）；
/// 无 shell、CREATE_NO_WINDOW、挂 Job Object，与 run_plugin_op 同款纪律。
/// 5 分钟预算：install 走网络时 pnpm 自带请求超时与重试，正常远快于此；
/// 超时按失败处理（调用方回滚）。
fn run_pnpm_direct(home: &PluginsHome, args: &[&str]) -> Result<(i32, String), String> {
    let mut cmd = tokio::process::Command::new(&home.node_exe);
    cmd.arg(home.pnpm_js()).args(args);
    cmd.current_dir(home.profile_dir());
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::platform::current().configure_child_command(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("启动内置 pnpm 失败：{e}"))?;
    if let Some(pid) = child.id() {
        crate::platform::current().register_child(pid);
    }
    let out = tauri::async_runtime::block_on(async {
        match tokio::time::timeout(std::time::Duration::from_secs(300), child.wait_with_output())
            .await
        {
            Ok(r) => r.map_err(|e| format!("等待内置 pnpm 失败：{e}")),
            Err(_) => Err("内置 pnpm 5 分钟未结束".to_string()),
        }
    })?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if text.len() > OP_OUTPUT_CAP {
        text.truncate(OP_OUTPUT_CAP);
    }
    Ok((out.status.code().unwrap_or(-1), text))
}

/// install/回滚失败时把备用 node_modules 还原回原位（install 可能留下半成品
/// node_modules 挡路，先清掉再 rename）。
fn restore_node_modules(stale: &Path, nm_dir: &Path) {
    if nm_dir.is_dir() {
        let _ = fs::remove_dir_all(nm_dir);
    }
    let _ = fs::rename(stale, nm_dir);
}

/// 检测并自愈 profile 的 pnpm store 迁移（详见上方段注；lib.rs 每次 spawn 前
/// 调用，结果落 events.log）。同步阻塞——运行在 setup 阶段，splash 已可见。
pub fn heal_profile_store(home: &PluginsHome) -> StoreHealOutcome {
    let nm_dir = home.profile_dir().join("node_modules");
    let Some(recorded) = recorded_store_dir(&nm_dir) else {
        return StoreHealOutcome::NoProfile;
    };
    let (code, out) = match run_pnpm_direct(home, &["store", "path"]) {
        Ok(r) => r,
        Err(_) => return StoreHealOutcome::CannotDetermine,
    };
    if code != 0 {
        return StoreHealOutcome::CannotDetermine;
    }
    let Some(current) = out.lines().rev().map(str::trim).find(|l| !l.is_empty()) else {
        return StoreHealOutcome::CannotDetermine;
    };
    if norm_store_path(&recorded) == norm_store_path(current) {
        return StoreHealOutcome::Matched;
    }
    // 迁移确认：rename 备用 → install → 成功删备份 / 失败回滚
    let stale = home.profile_dir().join("node_modules.dshdesktop-stale");
    let _ = fs::remove_dir_all(&stale); // 上次异常残留
    if let Err(e) = fs::rename(&nm_dir, &stale) {
        return StoreHealOutcome::RolledBack {
            detail: format!("node_modules 改名备用失败（被占用？）：{e}"),
        };
    }
    let (code, out) = match run_pnpm_direct(home, &["install"]) {
        Ok(r) => r,
        Err(e) => {
            restore_node_modules(&stale, &nm_dir);
            return StoreHealOutcome::RolledBack {
                detail: format!("内置 pnpm install 启动失败：{e}"),
            };
        }
    };
    if code == 0 {
        let _ = fs::remove_dir_all(&stale); // 尽力而为；删不掉只是垃圾
        StoreHealOutcome::Healed { old_store: recorded, new_store: current.to_string() }
    } else {
        let mut detail = format!("pnpm install 退出码 {code}：");
        let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = if lines.len() > 8 { lines[lines.len() - 8..].join(" | ") } else { lines.join(" | ") };
        detail.push_str(&tail);
        restore_node_modules(&stale, &nm_dir);
        StoreHealOutcome::RolledBack { detail }
    }
}

/// 持锁执行（命令与单测共用）：锁被占时直接报"进行中"。
fn run_op_guarded(home: &PluginsHome, args: &[&str]) -> Result<PluginOpResult, String> {
    let _guard = home
        .busy
        .try_lock()
        .map_err(|_| "已有插件操作在进行中，请等它完成".to_string())?;
    run_plugin_op(home, args)
}

pub fn install_plugin_impl(home: &PluginsHome, spec: &str) -> Result<PluginOpResult, String> {
    validate_spec(spec)?;
    run_op_guarded(home, &["add", spec.trim()])
}

pub fn uninstall_plugin_impl(home: &PluginsHome, name: &str) -> Result<PluginOpResult, String> {
    validate_spec(name)?;
    run_op_guarded(home, &["remove", name.trim()])
}

pub fn update_plugins_impl(home: &PluginsHome) -> Result<PluginOpResult, String> {
    run_op_guarded(home, &["update"])
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn system_node() -> PathBuf {
        let out = std::process::Command::new("where").arg("node").output().unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        PathBuf::from(stdout.lines().next().expect("node not found on PATH").trim())
    }

    fn test_home(tag: &str) -> (PluginsHome, PathBuf) {
        let work = std::env::temp_dir().join(format!("dshd-plugins-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let home = PluginsHome {
            node_exe: system_node(),
            dsh_bin: work.join("bin.js"),
            home: work.join("home"),
            pnpm_dir: work.join("bin"),
            busy: Mutex::new(()),
        };
        (home, work)
    }

    #[test]
    fn validate_rejects_empty_flaglike_and_long() {
        assert!(validate_spec("").is_err());
        assert!(validate_spec("  ").is_err());
        assert!(validate_spec("-g").is_err());
        assert!(validate_spec(&"a".repeat(201)).is_err());
        assert!(validate_spec("  @deepseek-ai/dsh-mcp-client  ").is_ok());
    }

    #[test]
    fn run_missing_pnpm_errors() {
        let (home, work) = test_home("run-missing-pnpm");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(&home.dsh_bin, "dummy").unwrap(); // node/dsh 齐备，缺的只有 pnpm
        std::fs::create_dir_all(&home.pnpm_dir).unwrap();
        assert!(run_plugin_op(&home, &["add", "x"]).is_err());
        assert!(run_plugin_op(&home, &["add", "x"]).unwrap_err().contains("pnpm"));
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn busy_lock_serializes() {
        let (home, work) = test_home("busy");
        let _g = home.busy.try_lock().unwrap(); // 模拟已有操作在跑
        assert!(run_op_guarded(&home, &["add", "x"]).is_err());
        assert!(run_op_guarded(&home, &["add", "x"]).unwrap_err().contains("进行中"));
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn run_captures_child_output() {
        // “看下方输出”依赖捕获子进程 stdout/stderr；tokio spawn 默认继承
        // 父进程 stdio，不显式 pipe 则 output 恒为空。
        let (home, work) = test_home("capture");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            &home.dsh_bin,
            r#"console.log("out-marker"); console.error("err-marker"); process.exit(3);"#,
        )
        .unwrap();
        std::fs::create_dir_all(home.pnpm_dir.join("pnpm").join("bin")).unwrap();
        std::fs::write(home.pnpm_dir.join("pnpm").join("bin").join("pnpm.cjs"), "").unwrap();
        std::fs::write(home.pnpm_dir.join("pnpm.cmd"), "").unwrap();
        let r = run_plugin_op(&home, &["add", "x"]).unwrap();
        assert_eq!(r.exit_code, 3);
        assert!(r.output.contains("out-marker"), "stdout 未被捕获：{:?}", r.output);
        assert!(r.output.contains("err-marker"), "stderr 未被捕获：{:?}", r.output);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn op_result_serializes_camel_case() {
        // IPC 消费方按 camelCase 读键；snake_case 会让 exitCode 恒为 undefined
        // ——成功被误判为失败（0.4.x 实踩过的同类问题）。
        let v = serde_json::to_value(PluginOpResult { exit_code: 0, output: "x".into() }).unwrap();
        assert!(v.get("exitCode").is_some(), "IPC 读 exitCode，实际键：{v}");
    }

}
