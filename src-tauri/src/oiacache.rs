//! 启动期原地补丁 open-in-app 客户端：apps 可用性 store 加 persist
//! （与 pickerpatch.rs/mcpgate.rs 同款签名门控 + marker 幂等；dsh 自更新
//! 还原文件后下次启动自动重打）。
//!
//! 背景与上游事实见 upstream.rs 的"open-in-app 可用性缓存补丁"段。要点：
//! 按钮渲染前等 apps 可用性（每进程一次 ~2.9s 冷探测）；补丁让它从
//! localStorage 缓存首帧即渲染，真实探测落地后静默校正。
//!
//! 漂移停手语义同 pickerpatch：needle 失配即停手（回退上游行为=按钮恢复
//! 晚出现），不产出半补丁。补丁内容变更须换 marker 版本（v1→v2）。

use crate::runtime::RuntimePaths;
use std::fs;
use std::path::{Path, PathBuf};

/// 补丁标记（幂等判定用；尾随在补丁行注释里）。
const MARKER: &str = "dshdesktop-oiacache: persist apps v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OiaCacheOutcome {
    /// 本轮改写了文件。
    Patched,
    /// marker 已在，幂等跳过。
    AlreadyPatched,
    /// needle 缺失或多次出现：上游形态变了，停手。
    UpstreamChanged,
    /// 包内文件不存在（fixture 运行时等布局）。
    Missing,
}

/// from：apps store 的 null 初值行（上游原文，逐字含 3-Tab/LF）。
const STORE_FROM: &str =
    "\t\t\tapps = (0, _deepseek_ai_dsh_client_store.createSnapshotStore)(null);";

/// to：同款初值 + persist（persist 走 JSON，数组透明持久化；choice store
/// 是同文件既有先例）。
const STORE_TO: &str = "\t\t\tapps = (0, _deepseek_ai_dsh_client_store.createSnapshotStore)(null, { persist: { name: \"dsh.open-in-app.apps\" } }); // dshdesktop-oiacache: persist apps v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileState {
    NeedsPatch,
    AlreadyPatched,
    UpstreamChanged,
    Missing,
}

fn file_state(content: Option<String>) -> FileState {
    let Some(content) = content else {
        return FileState::Missing;
    };
    if content.contains(MARKER) {
        return FileState::AlreadyPatched;
    }
    if content.matches(STORE_FROM).count() != 1 {
        return FileState::UpstreamChanged;
    }
    FileState::NeedsPatch
}

/// 与 pickerpatch 同款 tmp+rename：dsh 侧可能正读着。
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("dshdesktop-tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn oia_client_file(paths: &RuntimePaths) -> Option<PathBuf> {
    // dsh_bin = <nm>/@deepseek-ai/dsh/lib/bin.js → node_modules 目录 = 上四级
    let nm = paths.dsh_bin.parent()?.parent()?.parent()?.parent()?;
    Some(crate::upstream::join_segments(
        nm,
        crate::upstream::OIA_CLIENT_FILE_SEGMENTS,
    ))
}

/// win32 运行时启动前调用；任何 IO 失败只记日志不阻断启动。
pub fn patch_oia_apps_cache(paths: &RuntimePaths) -> OiaCacheOutcome {
    let Some(file) = oia_client_file(paths) else {
        return OiaCacheOutcome::Missing;
    };
    patch_file(&file)
}

fn patch_file(file: &Path) -> OiaCacheOutcome {
    let content = match fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => return OiaCacheOutcome::Missing,
    };
    match file_state(Some(content.clone())) {
        FileState::Missing => OiaCacheOutcome::Missing,
        FileState::UpstreamChanged => OiaCacheOutcome::UpstreamChanged,
        FileState::AlreadyPatched => OiaCacheOutcome::AlreadyPatched,
        FileState::NeedsPatch => {
            let patched = content.replacen(STORE_FROM, STORE_TO, 1);
            if write_atomic(file, &patched).is_err() {
                return OiaCacheOutcome::Missing;
            }
            OiaCacheOutcome::Patched
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        format!(
            "var OpenInAppController = class {{\n\t\tchoice = (0, _deepseek_ai_dsh_client_store.createSnapshotStore)(\"\", {{ persist: {{ name: \"dsh.open-in-app.choice\" }} }});\n{STORE_FROM}\n}};\n"
        )
    }

    #[test]
    fn patch_applies_and_marks() {
        let patched = fixture().replacen(STORE_FROM, STORE_TO, 1);
        assert!(patched.contains(MARKER));
        assert!(patched.contains("persist: { name: \"dsh.open-in-app.apps\" }"));
        // 初值仍是 null（无缓存时行为同上游：按钮等首个探测）
        assert!(patched.contains("createSnapshotStore)(null, { persist:"));
    }

    #[test]
    fn file_state_transitions() {
        assert_eq!(file_state(Some(fixture())), FileState::NeedsPatch);
        assert_eq!(
            file_state(Some(fixture().replacen(STORE_FROM, STORE_TO, 1))),
            FileState::AlreadyPatched
        );
        let drifted = fixture().replace("createSnapshotStore)(null);", "createSnapshotStore)([]);");
        assert_eq!(file_state(Some(drifted)), FileState::UpstreamChanged);
        assert_eq!(file_state(None), FileState::Missing);
    }

    #[test]
    fn patch_file_idempotent() {
        let dir = std::env::temp_dir().join(format!("dshdesktop-oiacache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("client.js");
        fs::write(&file, fixture()).unwrap();
        assert_eq!(patch_file(&file), OiaCacheOutcome::Patched);
        assert_eq!(patch_file(&file), OiaCacheOutcome::AlreadyPatched);
        fs::write(&file, "upstream rewrote everything").unwrap();
        assert_eq!(patch_file(&file), OiaCacheOutcome::UpstreamChanged);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_file_missing() {
        let dir = std::env::temp_dir();
        assert_eq!(
            patch_file(&dir.join("no-such-oia-client.js")),
            OiaCacheOutcome::Missing
        );
    }

    /// 开发辅助：把补丁应用到真实运行时（DSHDESKTOP_RUNTIME_DIR 指向的树）。
    /// `DSHDESKTOP_RUNTIME_DIR=<rt> cargo test --lib oiacache -- --ignored`
    #[test]
    #[ignore]
    fn apply_to_real_runtime() {
        let rt = std::env::var("DSHDESKTOP_RUNTIME_DIR").expect("set DSHDESKTOP_RUNTIME_DIR");
        let nm = Path::new(&rt).join("dsh").join("node_modules");
        let file = crate::upstream::join_segments(&nm, crate::upstream::OIA_CLIENT_FILE_SEGMENTS);
        let outcome = patch_file(&file);
        assert!(matches!(
            outcome,
            OiaCacheOutcome::Patched | OiaCacheOutcome::AlreadyPatched
        ));
    }
}
