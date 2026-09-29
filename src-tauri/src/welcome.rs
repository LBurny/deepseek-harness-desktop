//! 内测声明豁免播种：dsh 的 welcome notice（"内测声明"对话框）在 settings.yaml
//! 的 welcomeNoticeVersion（设置命名空间见 upstream::WELCOME_NOTICE_NAMESPACE，
//! 0.2.0 起是 ui-settings-general，旧装机残留的 ui-onboarding 段无害）≠ 当前
//! 文案版本时，每次启动都弹窗。壳面向最终用户——启动时从运行时 client.js 提取
//! 当前文案版本，预写进 settings.yaml（Value 级改写，其余键不动），桌面用户
//! 永不见该对话框；上游 bump 文案版本时提取自动跟随、仍豁免。needle 由契约
//! 测试守门（tests/upstream_contract.rs），提取/写盘失败只记 events.log 不阻断启动。

use serde_yaml::{Mapping, Value};
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

/// 播种/更新 settings.yaml 的 welcomeNoticeVersion（命名空间随
/// upstream::WELCOME_NOTICE_NAMESPACE）。文件缺失则新建（仅含该节）；
/// 损坏（YAML 解析失败/根不是 map）则不动盘报错。
pub fn seed_welcome_notice(home: &Path, dsh_bin: &Path) -> Result<WelcomeOutcome, String> {
    let version = extract_notice_version(dsh_bin)?;
    let path = home.join(crate::upstream::SETTINGS_FILE);
    let mut root: Mapping = match fs::read_to_string(&path) {
        // yaml-rust/serde_yaml 不接受 UTF-8 BOM（主题轮询同款坑），容忍剥掉
        Ok(text) => match serde_yaml::from_str::<Value>(text.strip_prefix('\u{FEFF}').unwrap_or(&text)) {
            Ok(Value::Mapping(m)) => m,
            Ok(_) => return Err("settings.yaml 根不是 map，跳过播种".into()),
            Err(e) => return Err(format!("settings.yaml 解析失败，跳过播种: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Mapping::new(),
        Err(e) => return Err(format!("settings.yaml 读取失败: {e}")),
    };

    let ns = Value::String(crate::upstream::WELCOME_NOTICE_NAMESPACE.into());
    let field = Value::String(crate::upstream::WELCOME_NOTICE_ACK_FIELD.into());
    let mut section = root
        .get(&ns)
        .map(|v| match v {
            Value::Mapping(m) => m.clone(),
            // 畸形节 dsh 侧按空节解码（decodeWelcomeSection），直接替换
            _ => Mapping::new(),
        })
        .unwrap_or_default();

    if section.get(&field).and_then(Value::as_str) == Some(version.as_str()) {
        return Ok(WelcomeOutcome::AlreadySeeded);
    }

    section.insert(field, Value::String(version));
    root.insert(ns, Value::Mapping(section));

    let text = serde_yaml::to_string(&Value::Mapping(root)).map_err(|e| e.to_string())?;
    fs::create_dir_all(home).map_err(|e| e.to_string())?;
    // tmp+rename 原子写：dsh-settings-file 可能正读着（外部编辑热发布）
    let tmp = path.with_extension("yaml.dshdesktop-tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
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
        let text = fs::read_to_string(home.path().join("settings.yaml")).unwrap();
        assert!(
            text.contains(&format!("{}:", crate::upstream::WELCOME_NOTICE_NAMESPACE)),
            "实际文件：{text}"
        );
        assert!(text.contains("welcomeNoticeVersion: 2099-01-02.3"), "实际文件：{text}");
        let second = seed_welcome_notice(home.path(), &bin).unwrap();
        assert!(matches!(second, WelcomeOutcome::AlreadySeeded));
    }

    #[test]
    fn preserves_other_sections_and_upgrades_old_version() {
        let (_d, bin) = fixture_runtime();
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("settings.yaml"),
            format!(
                "ui-theme:\n  preference: dark\n{}:\n  welcomeNoticeVersion: 2000-01-01.1\n",
                crate::upstream::WELCOME_NOTICE_NAMESPACE
            ),
        )
        .unwrap();
        let outcome = seed_welcome_notice(home.path(), &bin).unwrap();
        assert!(matches!(outcome, WelcomeOutcome::Seeded));
        let text = fs::read_to_string(home.path().join("settings.yaml")).unwrap();
        assert!(text.contains("preference: dark"), "实际文件：{text}");
        assert!(text.contains("welcomeNoticeVersion: 2099-01-02.3"), "实际文件：{text}");
    }

    #[test]
    fn tolerates_bom_and_skips_corrupt_file() {
        let (_d, bin) = fixture_runtime();
        let home = tempfile::tempdir().unwrap();
        // BOM：剥掉后正常播种
        fs::write(
            home.path().join("settings.yaml"),
            "\u{FEFF}ui-theme:\n  preference: light\n",
        )
        .unwrap();
        seed_welcome_notice(home.path(), &bin).unwrap();
        let text = fs::read_to_string(home.path().join("settings.yaml")).unwrap();
        assert!(text.contains("preference: light"), "实际文件：{text}");
        // 损坏文件：不动盘、显式报错
        fs::write(home.path().join("settings.yaml"), ":\n  - [unclosed").unwrap();
        assert!(seed_welcome_notice(home.path(), &bin).is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("settings.yaml")).unwrap(),
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
