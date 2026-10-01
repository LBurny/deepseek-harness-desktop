//! 内测声明豁免播种：dsh 的 welcome notice（"内测声明"对话框）在
//! ui-settings-general 条目的 welcomeNoticeVersion（设置命名空间见
//! upstream::WELCOME_NOTICE_NAMESPACE；0.2.0 起设置存 profile patch 条目，
//! 旧平面 settings.yaml 是一次性遗留导入通道）≠ 当前文案版本时，每次启动都
//! 弹窗。壳面向最终用户——启动时从运行时 client.js 提取当前文案版本，经
//! patchstore::upsert_settings_entry 预写进 profile patch（Value 级 merge，其余条目
//! 与键不动），桌面用户永不见该对话框；上游 bump 文案版本时提取自动跟随、
//! 仍豁免。needle 由契约测试守门（tests/upstream_contract.rs），提取/写盘
//! 失败只记 events.log 不阻断启动。

use std::fs;
use std::path::{Path, PathBuf};

pub enum WelcomeOutcome {
    /// 已是最新文案版本（或重复运行），未改盘
    AlreadySeeded,
    /// 写入/更新了 welcomeNoticeVersion
    Seeded,
}

/// client.js 防御性读取上限（构建产物正常 ~50KB）。
const CLIENT_JS_CAP: u64 = 16 * 1024 * 1024;

fn client_js_path(dsh_bin: &Path) -> Option<PathBuf> {
    // dsh_bin = <nm>/@deepseek-ai/dsh/lib/bin.js → 上溯四级到 node_modules
    let nm = dsh_bin.ancestors().nth(4)?;
    Some(crate::upstream::join_segments(
        nm,
        crate::upstream::WELCOME_NOTICE_CLIENT_SEGMENTS,
    ))
}

/// 从 client.js 提取 WELCOME_NOTICE_VERSION 的字面值（如 2026-08-13.1）。
fn extract_notice_version(dsh_bin: &Path) -> Result<String, String> {
    let path = client_js_path(dsh_bin).ok_or("dsh_bin 路径层级异常")?;
    let meta = fs::metadata(&path).map_err(|e| format!("client.js 读取失败: {e}"))?;
    if meta.len() > CLIENT_JS_CAP {
        return Err(format!("client.js 超出 {CLIENT_JS_CAP} 字节上限"));
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("client.js 读取失败: {e}"))?;
    let needle = crate::upstream::WELCOME_NOTICE_VERSION_NEEDLE;
    let start = text
        .find(needle)
        .ok_or("client.js 未找到 WELCOME_NOTICE_VERSION needle（上游形态变了）")?;
    let rest = &text[start + needle.len()..];
    let value: String = rest.chars().take_while(|&c| c != '"').collect();
    // 文案版本约定为日期序号（2026-08-13.1）；宽松校验防截到错位内容
    if value.is_empty()
        || value.len() > 32
        || !value
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == '.')
    {
        return Err(format!("WELCOME_NOTICE_VERSION 值形态异常: {value:?}"));
    }
    Ok(value)
}

/// 播种/更新 ui-settings-general 条目的 welcomeNoticeVersion（profile patch，
/// 经 patchstore::upsert_settings_entry Value 级 merge）。patch 损坏时显式报错不动盘。
pub fn seed_welcome_notice(home: &Path, dsh_bin: &Path) -> Result<WelcomeOutcome, String> {
    let version = extract_notice_version(dsh_bin)?;
    let stored = crate::patchstore::settings_entry_str(
        home,
        crate::upstream::WELCOME_NOTICE_NAMESPACE,
        crate::upstream::WELCOME_NOTICE_ACK_FIELD,
    );
    if stored.as_deref() == Some(version.as_str()) {
        return Ok(WelcomeOutcome::AlreadySeeded);
    }
    crate::patchstore::upsert_settings_entry(
        home,
        crate::upstream::WELCOME_NOTICE_NAMESPACE,
        crate::upstream::SETTINGS_GENERAL_PKG,
        &[(
            crate::upstream::WELCOME_NOTICE_ACK_FIELD,
            serde_yaml::Value::String(version),
        )],
    )?;
    Ok(WelcomeOutcome::Seeded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个最小运行时树：<tmp>/nm/@deepseek-ai/dsh/lib/bin.js +
    /// <tmp>/nm/@deepseek-ai/dsh-client-ui-settings-models/lib/client.js
    fn fixture_runtime() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let nm = dir.path().join("nm");
        let bin = nm.join("@deepseek-ai/dsh/lib/bin.js");
        let client = nm.join("@deepseek-ai/dsh-client-ui-settings-models/lib/client.js");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::create_dir_all(client.parent().unwrap()).unwrap();
        fs::write(&bin, "// entry").unwrap();
        fs::write(
            &client,
            "const WELCOME_NOTICE_VERSION = \"2099-01-02.3\";\n",
        )
        .unwrap();
        (dir, bin)
    }

    #[test]
    fn extracts_version_from_client_js() {
        let (_d, bin) = fixture_runtime();
        assert_eq!(extract_notice_version(&bin).unwrap(), "2099-01-02.3");
    }

    #[test]
    fn seeds_missing_file_then_already_seeded() {
        let (_d, bin) = fixture_runtime();
        let home = tempfile::tempdir().unwrap();
        let first = seed_welcome_notice(home.path(), &bin).unwrap();
        assert!(matches!(first, WelcomeOutcome::Seeded));
        // 播种落点：profiles/web/cordis.patch.yml 的 ui-settings-general 条目
        let patch = home
            .path()
            .join("profiles")
            .join("web")
            .join("cordis.patch.yml");
        let text = fs::read_to_string(&patch).unwrap();
        assert!(text.contains("id: ui-settings-general"), "实际文件：{text}");
        assert!(text.contains("welcomeNoticeVersion: 2099-01-02.3"), "实际文件：{text}");
        assert!(text.contains("dsh-client-ui-settings-general"), "实际文件：{text}");
        let second = seed_welcome_notice(home.path(), &bin).unwrap();
        assert!(matches!(second, WelcomeOutcome::AlreadySeeded));
    }

    #[test]
    fn preserves_other_entries_and_upgrades_old_version() {
        let (_d, bin) = fixture_runtime();
        let home = tempfile::tempdir().unwrap();
        // 既有 patch：mcp 的 insert-op 行 + ui-theme 条目（带 fontSize 键） +
        // 旧版本的 ui-settings-general 条目——播种只动 welcomeNoticeVersion
        let dir = home.path().join("profiles").join("web");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("cordis.patch.yml"),
            "- insert:\n    - id: mcp-client\n      name: '@deepseek-ai/dsh-mcp-client'\n- id: ui-theme\n  name: '@deepseek-ai/dsh-client-ui-theme'\n  config:\n    preference: dark\n    fontSize: 15\n- id: ui-settings-general\n  name: '@deepseek-ai/dsh-client-ui-settings-general'\n  config:\n    welcomeNoticeVersion: 2000-01-01.1\n",
        )
        .unwrap();
        let outcome = seed_welcome_notice(home.path(), &bin).unwrap();
        assert!(matches!(outcome, WelcomeOutcome::Seeded));
        let text = fs::read_to_string(dir.join("cordis.patch.yml")).unwrap();
        assert!(text.contains("preference: dark"), "mcp/ui-theme 条目不得被动：{text}");
        assert!(text.contains("fontSize: 15"), "条目内其它键不得被清：{text}");
        assert!(text.contains("welcomeNoticeVersion: 2099-01-02.3"), "实际文件：{text}");
    }

    #[test]
    fn tolerates_bom_and_skips_corrupt_file() {
        let (_d, bin) = fixture_runtime();
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("profiles").join("web");
        fs::create_dir_all(&dir).unwrap();
        // BOM：剥掉后正常播种
        fs::write(
            dir.join("cordis.patch.yml"),
            "\u{FEFF}- id: ui-theme\n  name: x\n  config:\n    preference: light\n",
        )
        .unwrap();
        seed_welcome_notice(home.path(), &bin).unwrap();
        let text = fs::read_to_string(dir.join("cordis.patch.yml")).unwrap();
        assert!(text.contains("preference: light"), "实际文件：{text}");
        // 损坏文件：不动盘、显式报错
        fs::write(dir.join("cordis.patch.yml"), ":\n  - [unclosed").unwrap();
        assert!(seed_welcome_notice(home.path(), &bin).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("cordis.patch.yml")).unwrap(),
            ":\n  - [unclosed"
        );
    }

    #[test]
    fn errors_when_client_js_missing_or_needle_drifted() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("nm/@deepseek-ai/dsh/lib/bin.js");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, "// entry").unwrap();
        let home = tempfile::tempdir().unwrap();
        assert!(seed_welcome_notice(home.path(), &bin).is_err());
        // needle 漂移（上游改名/压缩形态变化）：报错而不是写错值
        let client = dir
            .path()
            .join("nm/@deepseek-ai/dsh-client-ui-settings-models/lib/client.js");
        fs::create_dir_all(client.parent().unwrap()).unwrap();
        fs::write(&client, "const RENAMED = \"1\";").unwrap();
        assert!(seed_welcome_notice(home.path(), &bin).is_err());
    }
}
