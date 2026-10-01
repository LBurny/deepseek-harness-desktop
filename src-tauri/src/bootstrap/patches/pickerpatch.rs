//! 启动期原地补丁 browse 目录选择器的两个包内文件（与 presets.rs 同款的
//! 签名门控 + marker 幂等；dsh 自更新还原文件后下次启动自动重打）。
//!
//! 补丁内容（背景见 upstream.rs 的" browse 选择器运行时补丁"段）：
//! - host（dsh-host-directory-picker-browse/lib/index.js）：新增 "dsh:drives"
//!   哨兵层级——list() 特判返回 A-Z 可用盘符根；ancestryCrumbs 对盘符根
//!   路径前插"此电脑"面包屑 crumb。没有这一层，面包屑在 home 子树内被
//!   客户端折叠成单个"主页"，手机端想到其它盘只能手输路径。
//! - client（dsh-client-ui-directory-picker-browse/lib/client.js）：
//!   showHidden 默认 true（含每次开框的重置），手机端页脚开关由 mobile.css
//!   隐藏；displayCrumbs 折叠 home 前缀时保留哨兵 crumb 居首；哨兵 crumb
//!   文案走 locale（browser.drives）；哨兵层级禁用"打开/新建文件夹"，
//!   防把 "dsh:drives" 当工作区选中。
//!
//! 耦合规则：host 补丁引入的哨兵依赖客户端配套（本地化 crumb、禁用"打开"），
//! 所以客户端签名漂移/文件缺失时整组停手，只改 host 会产生能把哨兵选成
//! 工作区的半成品。

use crate::dsh::runtime::RuntimePaths;
use std::fs;
use std::path::{Path, PathBuf};

/// 补丁标记（各文件一处，幂等判定用）。
const HOST_MARKER: &str = "dshdesktop-browse-host: drives sentinel v1";
const CLIENT_MARKER: &str = "dshdesktop-browse-client: drives+hidden v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowsePatchOutcome {
    /// 本轮改写了至少一个文件。
    Patched,
    /// 两个文件 marker 均在，幂等跳过。
    AlreadyPatched,
    /// 任一签名 needle 缺失：上游形态变了，整组停手。
    UpstreamChanged,
    /// 包内文件不存在（fixture 运行时等布局）。
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileState {
    NeedsPatch,
    AlreadyPatched,
    UpstreamChanged,
    Missing,
}

// ── host：dsh-host-directory-picker-browse/lib/index.js ──

/// 哨兵常量定义，插在最后一个 import 之后。
const HOST_IMPORT_ANCHOR: &str =
    r#"import { DirectoryPicker, DirectoryPickerError } from "@deepseek-ai/dsh-host-directory-picker";"#;
const HOST_CONSTS: &str = r#"import { DirectoryPicker, DirectoryPickerError } from "@deepseek-ai/dsh-host-directory-picker";
// dshdesktop-browse-host: drives sentinel v1
// 远程手机端没有系统对话框，browse 又只认全限定路径——补一个盘符枚举层级：
// list("dsh:drives") 返回 A-Z 可用盘符根，面包屑对盘符根路径前插入口 crumb。
// crumb 名字只是兜底，客户端按 path 判定后走本地化文案。
const DRIVES_SENTINEL = "dsh:drives";
const DRIVES_CRUMB_NAME = "This PC";"#;

/// 面包屑到根时，win32 盘符根前插哨兵 crumb。
const HOST_CRUMBS_FROM: &str = "\t\tif (parent === current) return crumbs;";
const HOST_CRUMBS_TO: &str = r#"		if (parent === current) {
			if (process.platform === "win32" && /^[A-Za-z]:[\\/]/.test(current)) crumbs.unshift({
				name: DRIVES_CRUMB_NAME,
				path: DRIVES_SENTINEL,
				hidden: false
			});
			return crumbs;
		}"#;

/// list() 的哨兵特判分支，插在 home 取值之后（全限定校验会拒哨兵，必须先于它）。
const HOST_LIST_ANCHOR: &str = "\t\tconst home = homedir();\n";
const HOST_LIST_BRANCH: &str = r#"		const home = homedir();
		// dshdesktop: 哨兵层级——列出本机可用盘符根（不可读/未就绪的盘跳过）
		if (process.platform === "win32" && typeof path === "string" && path.replace(/[\\/]+$/, "") === DRIVES_SENTINEL) {
			const entries = [];
			for (let code = 65; code <= 90; code++) {
				if (signal?.aborted) throw asError(signal.reason);
				const letter = String.fromCharCode(code);
				const root = `${letter}:\\`;
				try {
					if (!(await raceAbort(stat(root), signal)).isDirectory()) continue;
					entries.push({ name: `${letter}:`, path: root, hidden: false });
				} catch {
					if (signal?.aborted) throw asError(signal.reason);
					continue;
				}
			}
			return {
				path: DRIVES_SENTINEL,
				home,
				crumbs: [{ name: DRIVES_CRUMB_NAME, path: DRIVES_SENTINEL, hidden: false }],
				entries,
				truncated: false
			};
		}
"#;

// ── client：dsh-client-ui-directory-picker-browse/lib/client.js ──

/// 隐藏条目默认显示（marker 顺带挂在这处）。
const CLIENT_HIDDEN_INIT_FROM: &str =
    "\t\t\tconst [showHidden, setShowHidden] = (0, react.useState)(false);";
const CLIENT_HIDDEN_INIT_TO: &str =
    "\t\t\tconst [showHidden, setShowHidden] = (0, react.useState)(true); // dshdesktop-browse-client: drives+hidden v1";
/// 每次打开对话框的重置同步改 true，否则开框即回到隐藏。
const CLIENT_HIDDEN_RESET_FROM: &str = "\t\t\t\t\tsetShowHidden(false);";
const CLIENT_HIDDEN_RESET_TO: &str = "\t\t\t\t\tsetShowHidden(true);";

/// displayCrumbs 折叠 home 前缀时保留哨兵 crumb 居首——没有它，在 home
/// 子树内面包屑只剩"主页"，爬不到盘符层。
const CLIENT_CRUMBS_FROM: &str = "\t\t\tif (homeIndex === -1) return listing.crumbs;\n\t\t\tconst tail = listing.crumbs.slice(homeIndex + 1);\n\t\t\treturn [{\n";
const CLIENT_CRUMBS_TO: &str = "\t\t\tconst drivesCrumb = listing.crumbs[0]?.path === \"dsh:drives\" ? [listing.crumbs[0]] : [];\n\t\t\tif (homeIndex === -1) return listing.crumbs;\n\t\t\tconst tail = listing.crumbs.slice(homeIndex + 1);\n\t\t\treturn [...drivesCrumb, {\n";

/// 哨兵 crumb 文案本地化（host 不知道客户端语言）。
const CLIENT_CRUMB_LABEL_FROM: &str = "\t\t\t\t\t\t\t\t\t\t\tchildren: crumb.name";
const CLIENT_CRUMB_LABEL_TO: &str = "\t\t\t\t\t\t\t\t\t\t\tchildren: crumb.path === \"dsh:drives\" ? t(\"browser.drives\") : crumb.name";

/// 哨兵层级禁用"打开"与"新建文件夹"（targetPath 是哨兵时两者都没有意义）。
const CLIENT_OPEN_DISABLED_FROM: &str =
    "\t\t\t\t\t\t\t\t\tdisabled: targetPath === null || loading || parentInert || draftPending,";
const CLIENT_OPEN_DISABLED_TO: &str =
    "\t\t\t\t\t\t\t\t\tdisabled: targetPath === null || targetPath === \"dsh:drives\" || loading || parentInert || draftPending,";
const CLIENT_NEWFOLDER_DISABLED_FROM: &str =
    "\t\t\t\t\t\t\t\t\tdisabled: parent === null || loading || parentInert || draftPending,";
const CLIENT_NEWFOLDER_DISABLED_TO: &str =
    "\t\t\t\t\t\t\t\t\tdisabled: parent === null || parent.path === \"dsh:drives\" || loading || parentInert || draftPending,";

/// locale 字典补 browser.drives 文案。
const CLIENT_LOCALE_ZH_FROM: &str = "\t\t\t\t\t\"browser.showHidden\": \"显示隐藏文件\"";
const CLIENT_LOCALE_ZH_TO: &str = "\t\t\t\t\t\"browser.drives\": \"此电脑\",\n\t\t\t\t\t\"browser.showHidden\": \"显示隐藏文件\"";
const CLIENT_LOCALE_EN_FROM: &str = "\t\t\t\t\t\"browser.showHidden\": \"Show hidden files\"";
const CLIENT_LOCALE_EN_TO: &str = "\t\t\t\t\t\"browser.drives\": \"This PC\",\n\t\t\t\t\t\"browser.showHidden\": \"Show hidden files\"";

const HOST_PATCHES: &[(&str, &str)] = &[
    (HOST_IMPORT_ANCHOR, HOST_CONSTS),
    (HOST_CRUMBS_FROM, HOST_CRUMBS_TO),
    (HOST_LIST_ANCHOR, HOST_LIST_BRANCH),
];

const CLIENT_PATCHES: &[(&str, &str)] = &[
    (CLIENT_HIDDEN_INIT_FROM, CLIENT_HIDDEN_INIT_TO),
    (CLIENT_HIDDEN_RESET_FROM, CLIENT_HIDDEN_RESET_TO),
    (CLIENT_CRUMBS_FROM, CLIENT_CRUMBS_TO),
    (CLIENT_CRUMB_LABEL_FROM, CLIENT_CRUMB_LABEL_TO),
    (CLIENT_OPEN_DISABLED_FROM, CLIENT_OPEN_DISABLED_TO),
    (CLIENT_NEWFOLDER_DISABLED_FROM, CLIENT_NEWFOLDER_DISABLED_TO),
    (CLIENT_LOCALE_ZH_FROM, CLIENT_LOCALE_ZH_TO),
    (CLIENT_LOCALE_EN_FROM, CLIENT_LOCALE_EN_TO),
];

fn file_state(content: Option<String>, marker: &str, patches: &[(&str, &str)]) -> FileState {
    let Some(content) = content else {
        return FileState::Missing;
    };
    if content.contains(marker) {
        return FileState::AlreadyPatched;
    }
    // 全量签名：任一 needle 缺失（或出现多次，替换有歧义）都视为上游变了
    if patches.iter().any(|(from, _)| content.matches(from).count() != 1) {
        return FileState::UpstreamChanged;
    }
    FileState::NeedsPatch
}

fn apply_patches(mut content: String, patches: &[(&str, &str)]) -> String {
    for (from, to) in patches {
        content = content.replacen(from, to, 1);
    }
    content
}

/// 与 presets.rs 同款 tmp+rename：dsh 侧可能正读着。
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("dshdesktop-tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn browse_files(paths: &RuntimePaths) -> Option<(PathBuf, PathBuf)> {
    // dsh_bin = <nm>/@deepseek-ai/dsh/lib/bin.js → node_modules 目录 = 上四级
    // （子包由 npm 平铺在 @deepseek-ai/ 下，与 dsh 包是兄弟而非嵌套）
    let nm = paths.dsh_bin.parent()?.parent()?.parent()?.parent()?;
    Some((
        crate::dsh::upstream::join_segments(nm, crate::dsh::upstream::PICKER_HOST_BROWSE_FILE_SEGMENTS),
        crate::dsh::upstream::join_segments(nm, crate::dsh::upstream::PICKER_CLIENT_BROWSE_FILE_SEGMENTS),
    ))
}

/// win32 运行时启动前调用；返回处理结果供记日志。任何 IO 失败只记日志不阻断启动。
pub fn patch_browse_picker(paths: &RuntimePaths) -> BrowsePatchOutcome {
    let Some((host_file, client_file)) = browse_files(paths) else {
        return BrowsePatchOutcome::Missing;
    };
    patch_files(&host_file, &client_file)
}

fn patch_files(host_file: &Path, client_file: &Path) -> BrowsePatchOutcome {
    let host_content = fs::read_to_string(host_file).ok();
    let client_content = fs::read_to_string(client_file).ok();
    let host_state = file_state(host_content.clone(), HOST_MARKER, HOST_PATCHES);
    let client_state = file_state(client_content.clone(), CLIENT_MARKER, CLIENT_PATCHES);

    // 客户端是哨兵功能的安全前提（本地化 crumb + 禁用"打开"），它漂移了
    // 整组都不能动——只打 host 会放出能把 "dsh:drives" 选成工作区的半成品。
    match client_state {
        FileState::Missing => return BrowsePatchOutcome::Missing,
        FileState::UpstreamChanged => return BrowsePatchOutcome::UpstreamChanged,
        _ => {}
    }
    match host_state {
        FileState::Missing => return BrowsePatchOutcome::Missing,
        FileState::UpstreamChanged => return BrowsePatchOutcome::UpstreamChanged,
        _ => {}
    }
    if host_state == FileState::AlreadyPatched && client_state == FileState::AlreadyPatched {
        return BrowsePatchOutcome::AlreadyPatched;
    }

    let mut patched = false;
    if host_state == FileState::NeedsPatch {
        let Some(content) = host_content else {
            return BrowsePatchOutcome::Missing;
        };
        if write_atomic(host_file, &apply_patches(content, HOST_PATCHES)).is_err() {
            return BrowsePatchOutcome::Missing;
        }
        patched = true;
    }
    if client_state == FileState::NeedsPatch {
        let Some(content) = client_content else {
            return BrowsePatchOutcome::Missing;
        };
        if write_atomic(client_file, &apply_patches(content, CLIENT_PATCHES)).is_err() {
            return BrowsePatchOutcome::Missing;
        }
        patched = true;
    }
    if patched {
        BrowsePatchOutcome::Patched
    } else {
        BrowsePatchOutcome::AlreadyPatched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_fixture() -> String {
        format!(
            "{HOST_IMPORT_ANCHOR}\n//#region lib/types/index.js\nfunction ancestryCrumbs(target) {{\n\tconst crumbs = [];\n\tlet current = target;\n\tfor (;;) {{\n\t\tconst parent = dirname(current);\n\t\tcrumbs.unshift({{}});\n{HOST_CRUMBS_FROM}\n\t\tcurrent = parent;\n\t}}\n}}\nasync function f() {{\n{HOST_LIST_ANCHOR}\t\tif (path !== void 0 && !fullyQualified(path)) throw new DirectoryPickerError(\"x\");\n}}\n"
        )
    }

    fn client_fixture() -> String {
        [
            CLIENT_HIDDEN_INIT_FROM,
            CLIENT_HIDDEN_RESET_FROM,
            CLIENT_CRUMBS_FROM,
            CLIENT_CRUMB_LABEL_FROM,
            CLIENT_OPEN_DISABLED_FROM,
            CLIENT_NEWFOLDER_DISABLED_FROM,
            CLIENT_LOCALE_ZH_FROM,
            CLIENT_LOCALE_EN_FROM,
        ]
        .join("\n")
    }

    #[test]
    fn host_patches_apply_and_mark() {
        let patched = apply_patches(host_fixture(), HOST_PATCHES);
        assert!(patched.contains(HOST_MARKER));
        assert!(patched.contains("const DRIVES_SENTINEL = \"dsh:drives\";"));
        assert!(patched.contains("path.replace(/[\\\\/]+$/, \"\") === DRIVES_SENTINEL"));
        assert!(patched.contains("crumbs.unshift({"));
        // 哨兵特判必须在全限定校验之前
        let branch = patched.find("=== DRIVES_SENTINEL) {").unwrap();
        let fq = patched.find("fullyQualified(path)) throw").unwrap();
        assert!(branch < fq);
    }

    #[test]
    fn client_patches_apply_and_mark() {
        let patched = apply_patches(client_fixture(), CLIENT_PATCHES);
        assert!(patched.contains(CLIENT_MARKER));
        assert!(patched.contains("(0, react.useState)(true);"));
        assert!(patched.contains("setShowHidden(true);"));
        assert!(patched.contains("const drivesCrumb = listing.crumbs[0]"));
        assert!(patched.contains("return [...drivesCrumb, {"));
        assert!(patched.contains("t(\"browser.drives\")"));
        assert!(patched.contains("targetPath === \"dsh:drives\""));
        assert!(patched.contains("parent.path === \"dsh:drives\""));
        assert!(patched.contains("\"browser.drives\": \"此电脑\""));
        assert!(patched.contains("\"browser.drives\": \"This PC\""));
    }

    #[test]
    fn file_state_transitions() {
        assert_eq!(
            file_state(Some(host_fixture()), HOST_MARKER, HOST_PATCHES),
            FileState::NeedsPatch
        );
        assert_eq!(
            file_state(
                Some(apply_patches(host_fixture(), HOST_PATCHES)),
                HOST_MARKER,
                HOST_PATCHES
            ),
            FileState::AlreadyPatched
        );
        // needle 变形 → 上游变了
        let drifted = host_fixture().replace(HOST_CRUMBS_FROM, "\t\tif (parent === current) return crumbs.slice();");
        assert_eq!(
            file_state(Some(drifted), HOST_MARKER, HOST_PATCHES),
            FileState::UpstreamChanged
        );
        assert_eq!(file_state(None, HOST_MARKER, HOST_PATCHES), FileState::Missing);
    }

    #[test]
    fn patch_files_all_or_nothing_and_idempotent() {
        let dir = std::env::temp_dir().join(format!("dshdesktop-pickerpatch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let host = dir.join("index.js");
        let client = dir.join("client.js");
        fs::write(&host, host_fixture()).unwrap();
        fs::write(&client, client_fixture()).unwrap();

        assert_eq!(patch_files(&host, &client), BrowsePatchOutcome::Patched);
        assert_eq!(patch_files(&host, &client), BrowsePatchOutcome::AlreadyPatched);

        // 客户端文件缺失 → 整组停手（host 已打过的保持原样，不重打）
        fs::remove_file(&client).unwrap();
        assert_eq!(patch_files(&host, &client), BrowsePatchOutcome::Missing);

        // 客户端漂移 → UpstreamChanged，host 也不动
        fs::write(&client, "upstream rewrote everything").unwrap();
        assert_eq!(patch_files(&host, &client), BrowsePatchOutcome::UpstreamChanged);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_files_missing_both() {
        let dir = std::env::temp_dir();
        assert_eq!(
            patch_files(
                &dir.join("no-such-host-index.js"),
                &dir.join("no-such-client.js")
            ),
            BrowsePatchOutcome::Missing
        );
    }

    /// 开发辅助：把补丁应用到真实运行时（DSHDESKTOP_RUNTIME_DIR 指向的树）。
    /// 应用启动时本来就会做同样的事；此测试只为手动验证/目验提供入口：
    /// `DSHDESKTOP_RUNTIME_DIR=<rt> cargo test --lib pickerpatch -- --ignored`
    #[test]
    #[ignore]
    fn apply_to_real_runtime() {
        let rt = std::env::var("DSHDESKTOP_RUNTIME_DIR").expect("set DSHDESKTOP_RUNTIME_DIR");
        let nm = Path::new(&rt).join("dsh").join("node_modules");
        let host = crate::dsh::upstream::join_segments(&nm, crate::dsh::upstream::PICKER_HOST_BROWSE_FILE_SEGMENTS);
        let client =
            crate::dsh::upstream::join_segments(&nm, crate::dsh::upstream::PICKER_CLIENT_BROWSE_FILE_SEGMENTS);
        let outcome = patch_files(&host, &client);
        assert!(matches!(
            outcome,
            BrowsePatchOutcome::Patched | BrowsePatchOutcome::AlreadyPatched
        ));
    }
}
