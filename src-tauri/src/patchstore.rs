//! cordis.patch.yml 通用条目读写：Value 级保留、tmp+rename 原子写。
//! 设置存储（ui-theme/locale/onboarding/picker overlay 等）与 MCP 条目共用此层；
//! 只认 <dsh-home>/profiles/web/cordis.patch.yml，别读 cordis.yml（每次启动被重写为空序列）。

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn patch_path(home: &Path) -> PathBuf {
    crate::dsh::upstream::join_segments(home, crate::dsh::upstream::MCP_PATCH_SEGMENTS)
}

/// （picker.rs 同文件复用）读 patch 顶层 op 序列；文件不存在/空 = 空序列，BOM 容忍
pub(crate) fn read_patch(path: &Path) -> Result<Vec<Value>, String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(Vec::new()); // 文件不存在 = 空补丁
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text.as_str());
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_yaml::from_str(text).map_err(|e| {
        crate::i18n::pick(
            format!("cordis.patch.yml 解析失败，请手工编辑该文件：{e}"),
            format!("Failed to parse cordis.patch.yml, please edit the file manually: {e}"),
        )
    })
}

/// （picker.rs 同文件复用）tmp+rename 原子写；空序列落 `[]\n`
pub(crate) fn write_patch(path: &Path, entries: &[Value]) -> Result<(), String> {
    let text = if entries.is_empty() {
        "[]\n".to_string()
    } else {
        serde_yaml::to_string(entries).map_err(|e| e.to_string())?
    };
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("yml.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

// ── profile patch 通用条目读写（0.2.0 设置迁移后 theme/i18n/welcome 复用）──
// dsh 0.2.0 起 settings.yaml 是一次性遗留导入通道（dsh-settings 每次启动见它就
// rename 成 .imported 并把各 section 导入 profile 同名条目），壳的偏好读取与
// 播种一律走本文件。这里的"条目"是**顶层直排行** `- id: <id>` + `name` +
// `config`（dsh config-editor 的 documentPath 写形），与上面 MCP 的
// `- insert:` op 行同文件共存、层级不同：read_patch 的顶层序列两者都是成员，
// 本节只认带 "id" 且不带 insert 键的直排行。

/// 顶层直排条目 `- id: <id>` 的位置（排除 insert-op 行）。
fn settings_entry_index(entries: &[Value], id: &str) -> Option<usize> {
    entries.iter().position(|e| {
        e.get("id").and_then(Value::as_str) == Some(id) && e.get("insert").is_none()
    })
}

/// 顶层条目 config 里的字符串字段（壳读 ui-theme/locale/welcomeNoticeVersion 偏好）。
/// 文件缺失/损坏/条目缺失/字段缺失一律 None（调用方按各自缺省语义兜底）。
pub(crate) fn settings_entry_str(home: &Path, id: &str, field: &str) -> Option<String> {
    let entries = read_patch(&patch_path(home)).ok()?;
    let row = entries.get(settings_entry_index(&entries, id)?)?;
    row.get("config")?
        .get(field)?
        .as_str()
        .map(|s| s.trim().to_string())
}

/// 播种/合并顶层条目的 config 字段：Value 级 merge（保留条目内其它键与文件里
/// 其它条目），条目不存在则按 dsh config-editor 的写形追加 `- id` + `name` +
/// `config`。文件缺失则创建（含 profiles/web 目录）；损坏（解析失败）返回
/// Err 不动盘——绝不拿整文件重写去覆盖手工/上游内容。
pub(crate) fn upsert_settings_entry(
    home: &Path,
    id: &str,
    pkg: &str,
    values: &[(&str, Value)],
) -> Result<(), String> {
    let path = patch_path(home);
    let mut entries = read_patch(&path)?;
    let idx = settings_entry_index(&entries, id);
    if let Some(i) = idx {
        let row = entries.get_mut(i).expect("index 来自同一序列");
        merge_entry_config(row, values)?;
    } else {
        let mut row = serde_yaml::Mapping::new();
        row.insert(Value::String("id".into()), Value::String(id.into()));
        row.insert(Value::String("name".into()), Value::String(pkg.into()));
        row.insert(
            Value::String("config".into()),
            Value::Mapping(serde_yaml::Mapping::new()),
        );
        let mut row = Value::Mapping(row);
        merge_entry_config(&mut row, values)?;
        entries.push(row);
    }
    write_patch(&path, &entries)
}

/// 往单个条目行合并 config 键值（config 缺则建；config 非 mapping 视为形态异常）。
fn merge_entry_config(row: &mut Value, values: &[(&str, Value)]) -> Result<(), String> {
    let map = row
        .as_mapping_mut()
        .ok_or_else(|| "cordis.patch.yml 条目形态异常（非 mapping）".to_string())?;
    if map.get(Value::String("config".into())).is_none() {
        map.insert(
            Value::String("config".into()),
            Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    let cfg = map
        .get_mut(Value::String("config".into()))
        .and_then(Value::as_mapping_mut)
        .ok_or_else(|| "cordis.patch.yml 条目 config 形态异常（非 mapping）".to_string())?;
    for (k, v) in values {
        cfg.insert(Value::String((*k).into()), v.clone());
    }
    Ok(())
}
