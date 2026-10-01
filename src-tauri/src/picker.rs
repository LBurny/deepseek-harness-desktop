//! 目录选择器钉 browse：dsh 的 directory-picker-auto 在 win32+回环下决议为
//! native（Win32 系统对话框弹在电脑屏幕上），手机远程端永远看不到——新建项目
//! 无法选文件夹。修复 = 上游官方 pin 方式：禁用 auto 行 + insert browse 对
//! （网页内嵌浏览对话框，桌面/手机同形）。启动时幂等确保，spawn 前完成；
//! 与 mcp.rs 管理同一 cordis.patch.yml，条目互不命中（mcp 只认 mcp-client 的
//! insert 条目），Value 级共存。

use serde_yaml::Value;
use std::path::Path;

#[derive(Debug, PartialEq)]
pub enum PickerOutcome {
    Pinned,
    AlreadyPinned,
}

fn patch_path(home: &Path) -> std::path::PathBuf {
    crate::upstream::join_segments(home, crate::upstream::MCP_PATCH_SEGMENTS)
}

/// 任意 insert 列表里 row id 命中的位置：(op 索引, 列表内索引)
fn inserted_pos(entries: &[Value], row_id: &str) -> Option<(usize, usize)> {
    for (oi, op) in entries.iter().enumerate() {
        if let Some(list) = op
            .get(crate::upstream::CORDIS_OP_INSERT)
            .and_then(Value::as_sequence)
        {
            for (ii, e) in list.iter().enumerate() {
                if e.get("id").and_then(Value::as_str) == Some(row_id) {
                    return Some((oi, ii));
                }
            }
        }
    }
    None
}

/// 幂等确保 pin 补丁：auto 行 disabled:true + browse 对 insert（缺谁补谁，
/// 用户手加的 disabled 摘掉即修复）。只在有变化时写盘（避免无谓触发 HMR）。
pub fn ensure_browse_picker(home: &Path) -> Result<PickerOutcome, String> {
    let path = patch_path(home);
    let mut entries = crate::patchstore::read_patch(&path)?;
    let mut changed = false;

    // 1) auto 行禁用：无 insert 键的 id 定向补丁（形态与上游 overlay 一致）
    let auto = crate::upstream::PICKER_AUTO_ROW_ID;
    match entries.iter().position(|op| {
        op.get(crate::upstream::CORDIS_OP_INSERT).is_none()
            && op.get("id").and_then(Value::as_str) == Some(auto)
    }) {
        Some(i) => {
            let map = entries[i].as_mapping_mut().unwrap();
            let key = Value::String(crate::upstream::CORDIS_ENTRY_DISABLED.into());
            if map.get(&key) != Some(&Value::Bool(true)) {
                map.insert(key, Value::Bool(true));
                changed = true;
            }
        }
        None => {
            let mut op = serde_yaml::Mapping::new();
            op.insert(Value::String("id".into()), Value::String(auto.into()));
            op.insert(
                Value::String(crate::upstream::CORDIS_ENTRY_DISABLED.into()),
                Value::Bool(true),
            );
            entries.push(Value::Mapping(op));
            changed = true;
        }
    }

    // 2) browse 对：缺谁补谁；已存在但带 disabled 的摘掉（用户误关即修复）
    for (row_id, pkg) in [
        (
            crate::upstream::PICKER_BROWSE_HOST_ROW_ID,
            crate::upstream::PICKER_BROWSE_HOST_PKG,
        ),
        (
            crate::upstream::PICKER_BROWSE_SURFACE_ROW_ID,
            crate::upstream::PICKER_BROWSE_SURFACE_PKG,
        ),
    ] {
        match inserted_pos(&entries, row_id) {
            Some((oi, ii)) => {
                let e = entries[oi][crate::upstream::CORDIS_OP_INSERT]
                    .as_sequence_mut()
                    .and_then(|s| s.get_mut(ii))
                    .unwrap();
                let map = e.as_mapping_mut().unwrap();
                if map
                    .remove(Value::String(crate::upstream::CORDIS_ENTRY_DISABLED.into()))
                    .is_some()
                {
                    changed = true;
                }
            }
            None => {
                let mut e = serde_yaml::Mapping::new();
                e.insert(Value::String("id".into()), Value::String(row_id.into()));
                e.insert(Value::String("name".into()), Value::String(pkg.into()));
                let mut op = serde_yaml::Mapping::new();
                op.insert(
                    Value::String(crate::upstream::CORDIS_OP_INSERT.into()),
                    Value::Sequence(vec![Value::Mapping(e)]),
                );
                entries.push(Value::Mapping(op));
                changed = true;
            }
        }
    }

    if !changed {
        return Ok(PickerOutcome::AlreadyPinned);
    }
    crate::patchstore::write_patch(&path, &entries)?;
    Ok(PickerOutcome::Pinned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn patch_text(home: &Path) -> String {
        fs::read_to_string(patch_path(home)).unwrap()
    }

    fn patch_entries(home: &Path) -> Vec<Value> {
        serde_yaml::from_str(&patch_text(home)).unwrap()
    }

    #[test]
    fn empty_file_pins_both_ops() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(ensure_browse_picker(t.path()).unwrap(), PickerOutcome::Pinned);
        let entries = patch_entries(t.path());
        // op0：禁用 auto 行（无 insert 键的 id 定向补丁，形态同上游 overlay）
        let auto = crate::upstream::PICKER_AUTO_ROW_ID;
        assert!(entries.iter().any(|op| {
            op.get(crate::upstream::CORDIS_OP_INSERT).is_none()
                && op.get("id").and_then(Value::as_str) == Some(auto)
                && op.get(crate::upstream::CORDIS_ENTRY_DISABLED) == Some(&Value::Bool(true))
        }));
        // browse 对都插进来了
        for row_id in [
            crate::upstream::PICKER_BROWSE_HOST_ROW_ID,
            crate::upstream::PICKER_BROWSE_SURFACE_ROW_ID,
        ] {
            assert!(inserted_pos(&entries, row_id).is_some(), "{row_id} 未插入");
        }
        let (oi, ii) = inserted_pos(&entries, crate::upstream::PICKER_BROWSE_HOST_ROW_ID).unwrap();
        assert_eq!(
            entries[oi][crate::upstream::CORDIS_OP_INSERT][ii]["name"],
            Value::String(crate::upstream::PICKER_BROWSE_HOST_PKG.into())
        );
        let (oi, ii) =
            inserted_pos(&entries, crate::upstream::PICKER_BROWSE_SURFACE_ROW_ID).unwrap();
        assert_eq!(
            entries[oi][crate::upstream::CORDIS_OP_INSERT][ii]["name"],
            Value::String(crate::upstream::PICKER_BROWSE_SURFACE_PKG.into())
        );
    }

    #[test]
    fn idempotent_second_run_writes_nothing() {
        let t = tempfile::tempdir().unwrap();
        ensure_browse_picker(t.path()).unwrap();
        let before = patch_text(t.path());
        assert_eq!(
            ensure_browse_picker(t.path()).unwrap(),
            PickerOutcome::AlreadyPinned
        );
        assert_eq!(patch_text(t.path()), before);
    }

    #[test]
    fn coexists_with_mcp_entries() {
        let t = tempfile::tempdir().unwrap();
        let p = patch_path(t.path());
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(
            &p,
            "- insert:\n    - id: mcp-pw\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: pw\n        transport: stdio\n        command: npx\n",
        )
        .unwrap();
        ensure_browse_picker(t.path()).unwrap();
        let text = patch_text(t.path());
        assert!(text.contains("mcp-pw") && text.contains("serverName: pw"));
        // mcp.rs 视角不受影响：仍能列出 pw
        assert_eq!(crate::mcp::list_servers_in(t.path()), vec!["pw"]);
    }

    #[test]
    fn repairs_partial_and_user_disabled() {
        let t = tempfile::tempdir().unwrap();
        let p = patch_path(t.path());
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        // 部分态：只有 host 行且被用户手贱 disabled；auto 行未禁用、surface 缺失
        fs::write(
            &p,
            "- insert:\n    - id: directory-picker-browse\n      name: '@deepseek-ai/dsh-host-directory-picker-browse'\n      disabled: true\n",
        )
        .unwrap();
        assert_eq!(ensure_browse_picker(t.path()).unwrap(), PickerOutcome::Pinned);
        let entries = patch_entries(t.path());
        // host 行的 disabled 被摘掉
        let (oi, ii) = inserted_pos(&entries, crate::upstream::PICKER_BROWSE_HOST_ROW_ID).unwrap();
        assert!(entries[oi][crate::upstream::CORDIS_OP_INSERT][ii]
            .get(crate::upstream::CORDIS_ENTRY_DISABLED)
            .is_none());
        // surface 行补齐
        assert!(inserted_pos(&entries, crate::upstream::PICKER_BROWSE_SURFACE_ROW_ID).is_some());
        // auto 禁用补上
        assert!(entries.iter().any(|op| {
            op.get(crate::upstream::CORDIS_OP_INSERT).is_none()
                && op.get("id").and_then(Value::as_str)
                    == Some(crate::upstream::PICKER_AUTO_ROW_ID)
                && op.get(crate::upstream::CORDIS_ENTRY_DISABLED) == Some(&Value::Bool(true))
        }));
    }

    #[test]
    fn unparseable_file_errors_and_untouched() {
        let t = tempfile::tempdir().unwrap();
        let p = patch_path(t.path());
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "not: a: sequence: [").unwrap();
        assert!(ensure_browse_picker(t.path()).is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "not: a: sequence: [");
    }

    #[test]
    fn bom_tolerated() {
        let t = tempfile::tempdir().unwrap();
        let p = patch_path(t.path());
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "\u{feff}[]\n").unwrap();
        ensure_browse_picker(t.path()).unwrap();
        assert!(patch_text(t.path()).contains(crate::upstream::PICKER_BROWSE_HOST_PKG));
    }
}
