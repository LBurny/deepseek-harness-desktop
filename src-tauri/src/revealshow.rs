//! 启动期原地补丁 dsh-native-command 的子进程选项：explorer.exe 豁免
//! windowsHide（与 pickerpatch.rs/mcpgate.rs/oiacache.rs 同款签名门控 +
//! marker 幂等；dsh 自更新还原文件后下次启动自动重打）。
//!
//! 症状：文件卡片菜单「在文件资源管理器中显示」/「打开所在文件夹」点了没反应
//! ——UI 照常回「已请求在文件管理器中显示」，桌面不弹任何窗口。
//! 链路：客户端 POST /api/present.open?…&action=reveal →
//! dsh-client-ui-deliverables host handler → sessionController.openWorkspacePath
//! → 本包 revealNativePath() → run("explorer.exe", ["/select,", <file url>])
//! → runNativeCommand() 的 execFile(…, { encoding:"utf8", signal,
//! windowsHide: true })。
//!
//! 实测根因（本机 Win10 19045 单变量对照，其余变量全不动）：windowsHide=true 时
//! Explorer 窗口确实建出来了、路径与选中文件都对，但 IsWindowVisible=false
//! ——窗口存在而不可见，HTTP 204 照常返回（壳只见"成功"）；同一条调用置
//! windowsHide=false 或省略即窗口可见。机制：libuv 的 windowsHide=true 在子进程
//! STARTUPINFO 上置 STARTF_USESHOWWINDOW + SW_HIDE，而 Explorer 的文件夹窗口走
//! SW_SHOWDEFAULT，继承了这份隐藏显示态。
//!
//! 只豁免 explorer.exe：同一 runner 里 powershell.exe 的 Invoke-Item
//! （openNativePath，即「用默认应用打开」）实测 windowsHide=true 仍可见——它经
//! ShellExecute 交给已运行的桌面 explorer；而 powershell/cmd 等控制台应用的控制台
//! 窗口必须保持隐藏（壳硬性"不闪控制台"约定），故不能全局置 false。
//!
//! 上游未修（最新 npm 包仍逐字是 `windowsHide: true`，出处见 upstream.rs 段注）。
//! 漂移停手语义同 pickerpatch：needle 失配即停手（回退上游行为=reveal 静默不可见），
//! 不产出半补丁。补丁内容变更须换 marker 版本（v1→v2），且 from-needle 必须仍锚
//! 上游原文（已被 v1 改写的文件会判 UpstreamChanged 停手，须先人工还原）。

use crate::runtime::RuntimePaths;
use std::fs;
use std::path::{Path, PathBuf};

/// 补丁标记（幂等判定用；尾随在补丁行注释里）。
const MARKER: &str = "dshdesktop-revealshow: visible explorer window v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevealShowOutcome {
    /// 本轮改写了文件。
    Patched,
    /// marker 已在，幂等跳过。
    AlreadyPatched,
    /// needle 缺失或多次出现：上游形态变了，停手。
    UpstreamChanged,
    /// 包内文件不存在（fixture 运行时等布局）。
    Missing,
}

/// from：runNativeCommand 的 execFile 选项行（上游原文，逐字含 2-Tab/LF）。
const HIDE_FROM: &str = "\t\twindowsHide: true";

/// to：同款选项 + explorer.exe 豁免。`command` 是同函数体的首形参名（上游若重命名
/// 该形参须同步改本串）；`$` 锚 + 大小写不敏感 = 只豁免以 explorer.exe 结尾的命令。
const HIDE_TO: &str = "\t\twindowsHide: !/explorer\\.exe$/i.test(command) // dshdesktop-revealshow: visible explorer window v1";

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
    if content.matches(HIDE_FROM).count() != 1 {
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

fn native_command_file(paths: &RuntimePaths) -> Option<PathBuf> {
    // dsh_bin = <nm>/@deepseek-ai/dsh/lib/bin.js → node_modules 目录 = 上四级
    let nm = paths.dsh_bin.parent()?.parent()?.parent()?.parent()?;
    Some(crate::upstream::join_segments(
        nm,
        crate::upstream::NATIVE_COMMAND_FILE_SEGMENTS,
    ))
}

/// win32 运行时启动前调用；任何 IO 失败只记日志不阻断启动。
pub fn patch_explorer_window(paths: &RuntimePaths) -> RevealShowOutcome {
    let Some(file) = native_command_file(paths) else {
        return RevealShowOutcome::Missing;
    };
    patch_file(&file)
}

fn patch_file(file: &Path) -> RevealShowOutcome {
    let content = match fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => return RevealShowOutcome::Missing,
    };
    match file_state(Some(content.clone())) {
        FileState::Missing => RevealShowOutcome::Missing,
        FileState::UpstreamChanged => RevealShowOutcome::UpstreamChanged,
        FileState::AlreadyPatched => RevealShowOutcome::AlreadyPatched,
        FileState::NeedsPatch => {
            let patched = content.replacen(HIDE_FROM, HIDE_TO, 1);
            if write_atomic(file, &patched).is_err() {
                return RevealShowOutcome::Missing;
            }
            RevealShowOutcome::Patched
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        format!(
            "const runNativeCommand = (command, args, signal) => new Promise((resolve, reject) => {{\n\texecFile(command, [...args], {{\n\t\tencoding: \"utf8\",\n\t\tsignal,\n{HIDE_FROM}\n\t}}, (error, stdout, stderr) => {{\n\t\tresolve({{ stdout, stderr }});\n\t}});\n}});\n"
        )
    }

    /// 复刻补丁行的判定语义（`!/explorer\.exe$/i.test(command)`）：命令名以
    /// explorer.exe 结尾（忽略大小写）才隐藏=false。下面的断言先钉住补丁行里就是
    /// 那个表达式，再用本函数核对各命令的取值（纯字符串断言，不跑 node）。
    fn hides_console(cmd: &str) -> bool {
        !cmd.to_ascii_lowercase().ends_with("explorer.exe")
    }

    #[test]
    fn patch_applies_and_marks() {
        let patched = fixture().replacen(HIDE_FROM, HIDE_TO, 1);
        assert!(patched.contains(MARKER));
        assert!(patched.contains("!/explorer\\.exe$/i.test(command)"));
        // 不是全局关掉隐藏（控制台应用的控制台窗口仍由 windowsHide 压着）
        assert!(!patched.contains("windowsHide: false"));
    }

    #[test]
    fn patched_line_stays_wellformed_option() {
        let patched = fixture().replacen(HIDE_FROM, HIDE_TO, 1);
        // 补丁行以换行收尾，紧随其后是选项字面量的收尾回调行——逗号/花括号/
        // 换行都没被吃掉，也没有多余内容溢到下一行。
        assert!(patched.contains(&format!("{HIDE_TO}\n")));
        assert!(patched.contains("\n\t}, (error, stdout, stderr) => {"));
    }

    #[test]
    fn exemption_is_command_scoped() {
        let patched = fixture().replacen(HIDE_FROM, HIDE_TO, 1);
        assert!(patched.contains("!/explorer\\.exe$/i.test(command)"));
        // 仅 explorer.exe（含全路径/大写形态）被豁免
        assert!(!hides_console("explorer.exe"));
        assert!(!hides_console("C:\\Windows\\explorer.exe"));
        assert!(!hides_console("EXPLORER.EXE"));
        // 其余命令维持上游行为：windowsHide 为 true（控制台窗口保持隐藏）
        assert!(hides_console("powershell.exe"));
        assert!(hides_console("cmd.exe"));
        assert!(hides_console("xdg-open"));
    }

    #[test]
    fn file_state_transitions() {
        assert_eq!(file_state(Some(fixture())), FileState::NeedsPatch);
        assert_eq!(
            file_state(Some(fixture().replacen(HIDE_FROM, HIDE_TO, 1))),
            FileState::AlreadyPatched
        );
        // 该行形态变了（如上游改成平台表达式）→ 上游动了 runner，停手
        let drifted = fixture().replace(HIDE_FROM, "\t\twindowsHide: win32");
        assert_eq!(file_state(Some(drifted)), FileState::UpstreamChanged);
        assert_eq!(file_state(None), FileState::Missing);
    }

    #[test]
    fn patch_file_idempotent() {
        let dir =
            std::env::temp_dir().join(format!("dshdesktop-revealshow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("index.js");
        fs::write(&file, fixture()).unwrap();
        assert_eq!(patch_file(&file), RevealShowOutcome::Patched);
        let once = fs::read(&file).unwrap();
        assert_eq!(patch_file(&file), RevealShowOutcome::AlreadyPatched);
        // 第二次是纯 no-op：字节不变
        assert_eq!(fs::read(&file).unwrap(), once);
        fs::write(&file, "upstream rewrote everything").unwrap();
        assert_eq!(patch_file(&file), RevealShowOutcome::UpstreamChanged);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_file_missing() {
        let dir = std::env::temp_dir();
        assert_eq!(
            patch_file(&dir.join("no-such-native-command-index.js")),
            RevealShowOutcome::Missing
        );
    }

    /// 开发辅助：把补丁应用到真实运行时（DSHDESKTOP_RUNTIME_DIR 指向的树）。
    /// `DSHDESKTOP_RUNTIME_DIR=<rt> cargo test --lib revealshow -- --ignored`
    #[test]
    #[ignore]
    fn apply_to_real_runtime() {
        let rt = std::env::var("DSHDESKTOP_RUNTIME_DIR").expect("set DSHDESKTOP_RUNTIME_DIR");
        let nm = Path::new(&rt).join("dsh").join("node_modules");
        let file =
            crate::upstream::join_segments(&nm, crate::upstream::NATIVE_COMMAND_FILE_SEGMENTS);
        let outcome = patch_file(&file);
        assert!(matches!(
            outcome,
            RevealShowOutcome::Patched | RevealShowOutcome::AlreadyPatched
        ));
    }
}
