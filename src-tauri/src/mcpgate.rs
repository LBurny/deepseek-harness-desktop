//! 启动期原地补丁 dsh-mcp-client 的就绪门禁（与 pickerpatch.rs 同款签名门控
//! + marker 幂等；dsh 自更新还原文件后下次启动自动重打）。
//!
//! 背景与上游事实见 upstream.rs 的"MCP 就绪门禁运行时补丁"段。要点：
//! failOnStartupError=false（默认）时 apply() 的 `await connection.ready`
//! 除拖延 loader settle→就绪行外无功能作用；补丁把它改成后台观察，
//! failOnStartupError=true 的配置保留上游语义。
//!
//! 漂移停手语义同 pickerpatch：上游改版导致 needle 失配即整组停手（回退
//! 上游行为=启动重新变慢，不产出半补丁）。补丁内容变更须换 marker 版本
//! （v1→v2），且 from-needle 必须仍锚上游原文（已被 v1 改写的文件会判
//! UpstreamChanged 停手，须先人工还原——与 pickerpatch 同限制）。

use crate::runtime::RuntimePaths;
use std::fs;
use std::path::{Path, PathBuf};

/// 补丁标记（幂等判定用）。
const MARKER: &str = "dshdesktop-mcpgate: nonblocking ready v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpGateOutcome {
    /// 本轮改写了文件。
    Patched,
    /// marker 已在，幂等跳过。
    AlreadyPatched,
    /// needle 缺失或多次出现：上游形态变了，停手。
    UpstreamChanged,
    /// 包内文件不存在（fixture 运行时等布局）。
    Missing,
}

/// from：apply() 尾部两行（上游原文，逐字含 Tab/LF）。
const GATE_FROM: &str = concat!(
    "\tconst outcome = await connection.ready;\n",
    "\tif (outcome.error !== void 0 && config.failOnStartupError) throw new Error(`mcp-client(${config.serverName}): initial connection or tool synchronization failed`, { cause: outcome.error });",
);

/// to：failOnStartupError=true 走原逻辑；false 后台观察（连接/注册/重连由
/// startConnection 内部链自理，见 upstream.rs 段注）。
const GATE_TO: &str = concat!(
    "\t// dshdesktop-mcpgate: nonblocking ready v1\n",
    "\tif (config.failOnStartupError) {\n",
    "\t\tconst outcome = await connection.ready;\n",
    "\t\tif (outcome.error !== void 0) throw new Error(`mcp-client(${config.serverName}): initial connection or tool synchronization failed`, { cause: outcome.error });\n",
    "\t} else {\n",
    "\t\tconnection.ready.then((outcome) => {\n",
    "\t\t\tif (outcome.error !== void 0) ctx.logger.error(`mcp-client(${config.serverName}): initial connection or tool synchronization failed (boot not blocked): ${String(outcome.error)}`);\n",
    "\t\t});\n",
    "\t}",
);

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
    if content.matches(GATE_FROM).count() != 1 {
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

fn mcp_client_file(paths: &RuntimePaths) -> Option<PathBuf> {
    // dsh_bin = <nm>/@deepseek-ai/dsh/lib/bin.js → node_modules 目录 = 上四级
    let nm = paths.dsh_bin.parent()?.parent()?.parent()?.parent()?;
    Some(crate::upstream::join_segments(
        nm,
        crate::upstream::MCP_CLIENT_FILE_SEGMENTS,
    ))
}

/// win32 运行时启动前调用；任何 IO 失败只记日志不阻断启动。
pub fn patch_mcp_ready_gate(paths: &RuntimePaths) -> McpGateOutcome {
    let Some(file) = mcp_client_file(paths) else {
        return McpGateOutcome::Missing;
    };
    patch_file(&file)
}

fn patch_file(file: &Path) -> McpGateOutcome {
    let content = match fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => return McpGateOutcome::Missing,
    };
    match file_state(Some(content.clone())) {
        FileState::Missing => McpGateOutcome::Missing,
        FileState::UpstreamChanged => McpGateOutcome::UpstreamChanged,
        FileState::AlreadyPatched => McpGateOutcome::AlreadyPatched,
        FileState::NeedsPatch => {
            let patched = content.replacen(GATE_FROM, GATE_TO, 1);
            if write_atomic(file, &patched).is_err() {
                return McpGateOutcome::Missing;
            }
            McpGateOutcome::Patched
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        format!(
            "async function apply(ctx, config) {{\n\tconst connection = startConnection(ctx, config, reconnect);\n{GATE_FROM}\n}}\n"
        )
    }

    #[test]
    fn patch_applies_and_marks() {
        let patched = fixture().replacen(GATE_FROM, GATE_TO, 1);
        assert!(patched.contains(MARKER));
        // true 分支保留上游语义
        assert!(patched.contains("if (config.failOnStartupError) {"));
        assert!(patched.contains("\t\tconst outcome = await connection.ready;"));
        // false 分支后台观察，不再 await
        assert!(patched.contains("connection.ready.then((outcome) => {"));
        // 顶层（单 Tab 缩进）不再残留裸 await 门禁——注意必须带 \n 前缀判定，
        // 因为 true 分支的双 Tab 行包含单 Tab needle 作为子串
        assert!(!patched.contains("\n\tconst outcome = await connection.ready;"));
        assert!(patched.contains("\n\t\tconst outcome = await connection.ready;"));
    }

    #[test]
    fn file_state_transitions() {
        assert_eq!(file_state(Some(fixture())), FileState::NeedsPatch);
        assert_eq!(
            file_state(Some(fixture().replacen(GATE_FROM, GATE_TO, 1))),
            FileState::AlreadyPatched
        );
        let drifted = fixture().replace(
            "\tconst outcome = await connection.ready;",
            "\tconst outcome = await connection.settled;",
        );
        assert_eq!(file_state(Some(drifted)), FileState::UpstreamChanged);
        assert_eq!(file_state(None), FileState::Missing);
    }

    #[test]
    fn patch_file_idempotent() {
        let dir = std::env::temp_dir().join(format!("dshdesktop-mcpgate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("index.js");
        fs::write(&file, fixture()).unwrap();
        assert_eq!(patch_file(&file), McpGateOutcome::Patched);
        assert_eq!(patch_file(&file), McpGateOutcome::AlreadyPatched);
        fs::write(&file, "upstream rewrote everything").unwrap();
        assert_eq!(patch_file(&file), McpGateOutcome::UpstreamChanged);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_file_missing() {
        let dir = std::env::temp_dir();
        assert_eq!(
            patch_file(&dir.join("no-such-mcp-client-index.js")),
            McpGateOutcome::Missing
        );
    }

    /// 开发辅助：把补丁应用到真实运行时（DSHDESKTOP_RUNTIME_DIR 指向的树）。
    /// `DSHDESKTOP_RUNTIME_DIR=<rt> cargo test --lib mcpgate -- --ignored`
    #[test]
    #[ignore]
    fn apply_to_real_runtime() {
        let rt = std::env::var("DSHDESKTOP_RUNTIME_DIR").expect("set DSHDESKTOP_RUNTIME_DIR");
        let nm = Path::new(&rt).join("dsh").join("node_modules");
        let file = crate::upstream::join_segments(&nm, crate::upstream::MCP_CLIENT_FILE_SEGMENTS);
        let outcome = patch_file(&file);
        assert!(matches!(
            outcome,
            McpGateOutcome::Patched | McpGateOutcome::AlreadyPatched
        ));
    }
}
