//! 技能管理：列出/启停/删除 <dsh-home>/skills 的技能，并从外部 agent 的技能目录导入。
//!
//! dsh 的技能发现（dsh-skill-filesystem）只认各根目录的直属条目，没有原生禁用概念；
//! 壳侧把停用技能的目录移到旁路 skills-disabled/（不是任何发现根），dsh 的 watcher
//! 观察到根目录变化后热刷新 catalog，启停即时生效、无需重启。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::State;

/// dsh 实际使用的 home（DSH_HOME），由 setup 注入（runtime_base_dir/dsh-home）
pub struct SkillsHome(pub PathBuf);

const DISABLED_DIR: &str = "skills-disabled";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkillRow {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SourceSkill {
    pub name: String,
    pub description: String,
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SourceInfo {
    pub id: String,
    pub label: String,
    pub exists: bool,
    pub skills: Vec<SourceSkill>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportItem {
    pub name: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImportResult {
    pub name: String,
    pub status: String, // "imported" | "skipped" | "error"
    pub error: Option<String>,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

/// 取 SKILL.md frontmatter 的单行键值（只认首个 --- 对之间的单行键，允许引号；
/// 多行 folded YAML 不支持——四个来源的技能都是单行）
fn parse_frontmatter_value(content: &str, key: &str) -> String {
    let mut lines = content.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    let prefix = format!("{key}:");
    for line in lines {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(v) = t.strip_prefix(prefix.as_str()) {
            return unquote(v.trim());
        }
    }
    String::new()
}

fn parse_description(content: &str) -> String {
    parse_frontmatter_value(content, "description")
}

fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

/// 扫一个技能根目录：只收"含 SKILL.md 的目录"（与 dsh discoverRoot 的跳过语义一致）；
/// name 取目录名（导入/启停/删除都按目录名操作；frontmatter 名实践中与目录名一致）
fn scan_skills_dir(dir: &Path, enabled: bool) -> Vec<SkillRow> {
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return rows;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(content) = fs::read_to_string(path.join("SKILL.md")) else {
            continue;
        };
        rows.push(SkillRow {
            name: entry.file_name().to_string_lossy().into_owned(),
            description: parse_description(&content),
            enabled,
        });
    }
    rows
}

fn list_skills_root(home: &Path) -> Vec<SkillRow> {
    let mut rows = scan_skills_dir(&home.join("skills"), true);
    rows.extend(scan_skills_dir(&home.join(DISABLED_DIR), false));
    rows.sort_by(|a, b| b.enabled.cmp(&a.enabled).then_with(|| a.name.cmp(&b.name)));
    rows
}

fn set_enabled(home: &Path, name: &str, enabled: bool) -> Result<(), String> {
    if !valid_name(name) {
        return Err(crate::i18n::pick(format!("非法技能名: {name}"), format!("Invalid skill name: {name}")));
    }
    let (from, to) = if enabled {
        (home.join(DISABLED_DIR).join(name), home.join("skills").join(name))
    } else {
        (home.join("skills").join(name), home.join(DISABLED_DIR).join(name))
    };
    if !from.is_dir() {
        return Err(crate::i18n::pick(format!("技能不存在: {name}"), format!("Skill not found: {name}")));
    }
    if to.exists() {
        return Err(crate::i18n::pick(
            format!("另一侧已存在同名技能: {name}"),
            format!("A skill with the same name already exists on the other side: {name}"),
        ));
    }
    fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::rename(&from, &to).map_err(|e| e.to_string())
}

fn delete_skill_dir(home: &Path, name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err(crate::i18n::pick(format!("非法技能名: {name}"), format!("Invalid skill name: {name}")));
    }
    for dir in [home.join("skills").join(name), home.join(DISABLED_DIR).join(name)] {
        if dir.is_dir() {
            return fs::remove_dir_all(&dir).map_err(|e| e.to_string());
        }
    }
    Err(crate::i18n::pick(format!("技能不存在: {name}"), format!("Skill not found: {name}")))
}

/// 四个导入源（用户级目录；不存在时在弹窗里灰显）
fn sources() -> Vec<(String, String, PathBuf)> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    [
        ("codex", "Codex", ".codex/skills"),
        ("claude", "Claude Code", ".claude/skills"),
        ("opencode", "OpenCode", ".config/opencode/skills"),
    ]
    .into_iter()
    .map(|(id, label, rel)| (id.to_string(), label.to_string(), home.join(rel)))
    .collect()
}

/// 启动时自动导入 ~/.dsh/skills（独立 dsh 的默认技能目录——壳隔离了 DSH_HOME，
/// 但用户心智里"我就是 dsh"，不该手动导入）。每次启动都扫描新技能；
/// marker 文件 `.skills-seeded` 记录已见过的名字：再次扫描不重复导入，
/// 用户从壳里删掉的技能也不会复活。源目录不存在 = 无操作。
pub fn seed_from_default_dsh_home(home: &Path) {
    let Some(user_home) = dirs::home_dir() else {
        return;
    };
    if let Err(e) = seed_auto_import(&user_home.join(".dsh/skills"), home) {
        // events.log 在 home 的父目录（runtime_base_dir）
        if let Some(base) = home.parent() {
            crate::logging::append_debug_line(&base.join("events.log"), &format!("skills seed failed: {e}"));
        }
    }
}

fn seed_auto_import(source_dir: &Path, home: &Path) -> Result<(), String> {
    use std::collections::HashSet;
    let marker = home.join(".skills-seeded");
    let mut seen: HashSet<String> = fs::read_to_string(&marker)
        .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();

    if source_dir.is_dir() {
        let target_root = home.join("skills");
        for row in scan_skills_dir(source_dir, true) {
            if !seen.insert(row.name.clone()) {
                continue; // 见过（含被用户删除的）——不重复导入、不复活
            }
            // 壳里已有（含停用态）：不动内容，只记入 marker
            let exists = target_root.join(&row.name).exists()
                || home.join(DISABLED_DIR).join(&row.name).exists();
            if !exists && valid_name(&row.name) {
                fs::create_dir_all(&target_root).map_err(|e| e.to_string())?;
                fs_extra::dir::copy(
                    source_dir.join(&row.name),
                    &target_root,
                    &fs_extra::dir::CopyOptions::new(),
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    let mut names: Vec<_> = seen.into_iter().collect();
    names.sort();
    fs::create_dir_all(home).map_err(|e| e.to_string())?;
    fs::write(&marker, names.join("\n") + "\n").map_err(|e| e.to_string())
}

fn source_info((id, label, dir): &(String, String, PathBuf), home: &Path) -> SourceInfo {
    let exists = dir.is_dir();
    let skills = if exists {
        scan_skills_dir(dir, true)
            .into_iter()
            .map(|row| {
                let conflict = home.join("skills").join(&row.name).exists()
                    || home.join(DISABLED_DIR).join(&row.name).exists();
                SourceSkill { name: row.name, description: row.description, conflict }
            })
            .collect()
    } else {
        Vec::new()
    };
    SourceInfo { id: id.clone(), label: label.clone(), exists, skills }
}

fn import_from(source_dir: &Path, home: &Path, items: &[ImportItem]) -> Vec<ImportResult> {
    items.iter().map(|item| import_one(source_dir, home, item)).collect()
}

fn import_one(source_dir: &Path, home: &Path, item: &ImportItem) -> ImportResult {
    let name = item.name.clone();
    let err = |e: &str| ImportResult { name: name.clone(), status: "error".into(), error: Some(e.to_string()) };
    if !valid_name(&item.name) {
        return err(&crate::i18n::pick("非法技能名", "Invalid skill name"));
    }
    let src = source_dir.join(&item.name);
    if !src.join("SKILL.md").is_file() {
        return err(&crate::i18n::pick("源目录中不存在该技能", "Skill not found in the source directory"));
    }
    let target_root = home.join("skills");
    let conflict =
        target_root.join(&item.name).exists() || home.join(DISABLED_DIR).join(&item.name).exists();
    if conflict && !item.overwrite {
        return ImportResult { name, status: "skipped".into(), error: None };
    }
    if conflict {
        // 覆盖 = 清掉两侧旧副本再复制（禁用目录里的也算冲突，防止日后启用出双份）
        let _ = fs::remove_dir_all(target_root.join(&item.name));
        let _ = fs::remove_dir_all(home.join(DISABLED_DIR).join(&item.name));
    }
    if let Err(e) = fs::create_dir_all(&target_root) {
        return err(&e.to_string());
    }
    match fs_extra::dir::copy(&src, &target_root, &fs_extra::dir::CopyOptions::new()) {
        Ok(_) => ImportResult { name, status: "imported".into(), error: None },
        Err(e) => err(&e.to_string()),
    }
}

// ---- zip 本地导入 ----

/// 解包保护：防 zip 炸弹
const MAX_ZIP_ENTRIES: usize = 10_000;
const MAX_ZIP_BYTES: u64 = 256 * 1024 * 1024;

/// zip 内发现的一个技能：prefix 是包内根前缀（"" = SKILL.md 在包根）
struct ZipSkillEntry {
    prefix: String,
    name: String,
    description: String,
}

fn invalid_zip_err() -> String {
    crate::i18n::pick("不是有效的 ZIP 文件", "Invalid ZIP file")
}

/// 条目路径归一化：enclosed_name() 已剥掉 .. / 绝对路径等穿越形态，
/// 再把 \ 统一成 / 方便前缀匹配（zip 标准是 /，个别工具会写 \）
fn normalized_entry_name(entry: &zip::read::ZipFile) -> Option<String> {
    entry.enclosed_name().map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// 识别 zip 里的技能布局：
/// - 包根有 SKILL.md → 单技能，名字取 frontmatter name（缺失回退 zip 文件名）
/// - 否则每个顶层目录（<dir>/SKILL.md）是一个技能，名字取目录名
fn discover_zip(zip_path: &Path) -> Result<Vec<ZipSkillEntry>, String> {
    let file = fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| invalid_zip_err())?;

    // (prefix, skill_md_index)
    let mut found: Vec<(String, usize)> = Vec::new();
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else { continue };
        if entry.is_dir() {
            continue;
        }
        let Some(s) = normalized_entry_name(&entry) else { continue };
        if s == "SKILL.md" {
            found.push((String::new(), i));
        } else if let Some((top, "SKILL.md")) = s.split_once('/') {
            let prefix = format!("{top}/");
            if !found.iter().any(|(p, _)| *p == prefix) {
                found.push((prefix, i));
            }
        }
    }
    if found.is_empty() {
        return Err(crate::i18n::pick(
            "压缩包中未找到技能（缺少 SKILL.md）",
            "No skill found in the archive (missing SKILL.md)",
        ));
    }

    let mut out = Vec::new();
    for (prefix, idx) in found {
        let mut entry = zip.by_index(idx).map_err(|_| invalid_zip_err())?;
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entry, &mut content).map_err(|e| e.to_string())?;
        let name = if prefix.is_empty() {
            let n = parse_frontmatter_value(&content, "name");
            if valid_name(&n) {
                n
            } else {
                let stem = zip_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if !valid_name(&stem) {
                    return Err(crate::i18n::pick(
                        "无法从压缩包确定技能名",
                        "Cannot determine a skill name from the archive",
                    ));
                }
                stem
            }
        } else {
            prefix.trim_end_matches('/').to_string()
        };
        out.push(ZipSkillEntry { prefix, name, description: parse_description(&content) });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn inspect_zip(zip_path: &Path, home: &Path) -> Result<Vec<SourceSkill>, String> {
    let entries = discover_zip(zip_path)?;
    Ok(entries
        .into_iter()
        .map(|e| SourceSkill {
            conflict: home.join("skills").join(&e.name).exists()
                || home.join(DISABLED_DIR).join(&e.name).exists(),
            name: e.name,
            description: e.description,
        })
        .collect())
}

fn import_zip(zip_path: &Path, home: &Path, items: &[ImportItem]) -> Vec<ImportResult> {
    match discover_zip(zip_path) {
        Err(e) => items
            .iter()
            .map(|i| ImportResult { name: i.name.clone(), status: "error".into(), error: Some(e.clone()) })
            .collect(),
        Ok(entries) => items.iter().map(|item| import_zip_one(zip_path, home, &entries, item)).collect(),
    }
}

fn import_zip_one(zip_path: &Path, home: &Path, entries: &[ZipSkillEntry], item: &ImportItem) -> ImportResult {
    let name = item.name.clone();
    let err = |e: &str| ImportResult { name: name.clone(), status: "error".into(), error: Some(e.to_string()) };
    if !valid_name(&item.name) {
        return err(&crate::i18n::pick("非法技能名", "Invalid skill name"));
    }
    let Some(entry) = entries.iter().find(|e| e.name == item.name) else {
        return err(&crate::i18n::pick("压缩包中不存在该技能", "Skill not found in the archive"));
    };
    let target_root = home.join("skills");
    let conflict =
        target_root.join(&item.name).exists() || home.join(DISABLED_DIR).join(&item.name).exists();
    if conflict && !item.overwrite {
        return ImportResult { name, status: "skipped".into(), error: None };
    }
    if conflict {
        let _ = fs::remove_dir_all(target_root.join(&item.name));
        let _ = fs::remove_dir_all(home.join(DISABLED_DIR).join(&item.name));
    }
    let dest = target_root.join(&item.name);
    if let Err(e) = fs::create_dir_all(&dest) {
        return err(&e.to_string());
    }
    let file = match fs::File::open(zip_path) {
        Ok(f) => f,
        Err(e) => return err(&e.to_string()),
    };
    let mut zip = match zip::ZipArchive::new(file) {
        Ok(z) => z,
        Err(_) => return err(&invalid_zip_err()),
    };
    match extract_zip_prefix(&mut zip, &entry.prefix, &dest) {
        Ok(()) => ImportResult { name, status: "imported".into(), error: None },
        Err(e) => err(&e),
    }
}

/// 把 zip 里 prefix 前缀下的所有条目解到 dest（剥掉前缀）。
/// 路径穿越条目经 enclosed_name 过滤直接跳过；规模超限报错。
fn extract_zip_prefix(
    zip: &mut zip::ZipArchive<fs::File>,
    prefix: &str,
    dest: &Path,
) -> Result<(), String> {
    let mut total: u64 = 0;
    let mut count: usize = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|_| invalid_zip_err())?;
        if entry.is_dir() {
            continue;
        }
        let Some(s) = normalized_entry_name(&entry) else { continue };
        let Some(rel) = s.strip_prefix(prefix) else { continue };
        if rel.is_empty() {
            continue;
        }
        count += 1;
        total = total.saturating_add(entry.size());
        if count > MAX_ZIP_ENTRIES || total > MAX_ZIP_BYTES {
            return Err(crate::i18n::pick(
                "压缩包内容超出大小限制",
                "Archive content exceeds size limits",
            ));
        }
        // rel 来自 enclosed_name，无 .. / 根路径，逐段 join 防分隔符歧义
        let mut target = dest.to_path_buf();
        for part in rel.split('/') {
            target.push(part);
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn list_skills(state: State<SkillsHome>) -> Vec<SkillRow> {
    list_skills_root(&state.0)
}

#[tauri::command]
pub fn list_import_sources(state: State<SkillsHome>) -> Vec<SourceInfo> {
    sources().iter().map(|s| source_info(s, &state.0)).collect()
}

#[tauri::command]
pub fn import_skills(
    state: State<SkillsHome>,
    source_id: String,
    items: Vec<ImportItem>,
) -> Vec<ImportResult> {
    let Some((.., dir)) = sources().into_iter().find(|(id, ..)| *id == source_id) else {
        return items
            .iter()
            .map(|i| ImportResult {
                name: i.name.clone(),
                status: "error".into(),
                error: Some(crate::i18n::pick("未知导入源", "Unknown import source")),
            })
            .collect();
    };
    import_from(&dir, &state.0, &items)
}

#[tauri::command]
pub fn set_skill_enabled(state: State<SkillsHome>, name: String, enabled: bool) -> Result<(), String> {
    set_enabled(&state.0, &name, enabled)
}

#[tauri::command]
pub fn delete_skill(state: State<SkillsHome>, name: String) -> Result<(), String> {
    delete_skill_dir(&state.0, &name)
}

#[tauri::command]
pub fn inspect_zip_skills(state: State<SkillsHome>, path: String) -> Result<Vec<SourceSkill>, String> {
    inspect_zip(Path::new(&path), &state.0)
}

#[tauri::command]
pub fn import_zip_skills(
    state: State<SkillsHome>,
    path: String,
    items: Vec<ImportItem>,
) -> Vec<ImportResult> {
    import_zip(Path::new(&path), &state.0, &items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, dir_name: &str, desc: &str) {
        let d = root.join(dir_name);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {dir_name}\ndescription: \"{desc}\"\n---\n\nbody\n"),
        )
        .unwrap();
    }

    // ---- parse_description ----

    #[test]
    fn parse_quoted_desc() {
        let d = parse_description("---\nname: a\ndescription: \"hello world\"\n---\nbody");
        assert_eq!(d, "hello world");
    }

    #[test]
    fn parse_unquoted_desc_with_colon() {
        let d = parse_description("---\ndescription: Use when x: y\n---\n");
        assert_eq!(d, "Use when x: y");
    }

    #[test]
    fn parse_missing_frontmatter() {
        assert_eq!(parse_description("# no frontmatter\nbody"), "");
        assert_eq!(parse_description(""), "");
    }

    // ---- list ----

    #[test]
    fn list_shows_enabled_and_disabled_sorted() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        write_skill(&home.join("skills"), "b-skill", "on");
        write_skill(&home.join(DISABLED_DIR), "a-skill", "off");
        let rows = list_skills_root(home);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "b-skill"); // 启用在前
        assert!(rows[0].enabled);
        assert_eq!(rows[1].name, "a-skill");
        assert!(!rows[1].enabled);
        assert_eq!(rows[1].description, "off");
    }

    #[test]
    fn list_skips_dir_without_skill_md() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("skills").join("empty-dir")).unwrap();
        assert!(list_skills_root(t.path()).is_empty());
    }

    // ---- set_enabled ----

    #[test]
    fn toggle_moves_dir_forth_and_back() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        write_skill(&home.join("skills"), "x", "d");
        set_enabled(home, "x", false).unwrap();
        assert!(!home.join("skills/x").exists());
        assert!(home.join(format!("{DISABLED_DIR}/x/SKILL.md")).is_file());
        set_enabled(home, "x", true).unwrap();
        assert!(home.join("skills/x/SKILL.md").is_file());
    }

    #[test]
    fn toggle_missing_or_conflict_errors() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        assert!(set_enabled(home, "ghost", false).is_err());
        write_skill(&home.join("skills"), "x", "d");
        write_skill(&home.join(DISABLED_DIR), "x", "d");
        assert!(set_enabled(home, "x", false).is_err()); // 目标已存在
        assert!(set_enabled(home, "..", false).is_err()); // 穿越名
    }

    // ---- delete ----

    #[test]
    fn delete_removes_from_either_dir() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        write_skill(&home.join("skills"), "on", "d");
        write_skill(&home.join(DISABLED_DIR), "off", "d");
        delete_skill_dir(home, "on").unwrap();
        delete_skill_dir(home, "off").unwrap();
        assert!(!home.join("skills/on").exists());
        assert!(!home.join(format!("{DISABLED_DIR}/off")).exists());
        assert!(delete_skill_dir(home, "ghost").is_err());
        assert!(delete_skill_dir(home, "a/b").is_err());
    }

    // ---- import ----

    #[test]
    fn import_copies_recursively() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        write_skill(&src, "deep", "d");
        fs::create_dir_all(src.join("deep/scripts")).unwrap();
        fs::write(src.join("deep/scripts/run.sh"), "echo hi").unwrap();
        let items = vec![ImportItem { name: "deep".into(), overwrite: false }];
        let r = import_from(&src, &home, &items);
        assert_eq!(r[0].status, "imported");
        assert!(home.join("skills/deep/scripts/run.sh").is_file());
    }

    #[test]
    fn import_conflict_skip_and_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        write_skill(&src, "x", "new-version");
        write_skill(&home.join("skills"), "x", "old-version");

        let skip = import_from(&src, &home, &[ImportItem { name: "x".into(), overwrite: false }]);
        assert_eq!(skip[0].status, "skipped");
        let body = fs::read_to_string(home.join("skills/x/SKILL.md")).unwrap();
        assert!(body.contains("old-version"));

        let ow = import_from(&src, &home, &[ImportItem { name: "x".into(), overwrite: true }]);
        assert_eq!(ow[0].status, "imported");
        let body = fs::read_to_string(home.join("skills/x/SKILL.md")).unwrap();
        assert!(body.contains("new-version"));
    }

    #[test]
    fn import_overwrite_also_clears_disabled_copy() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        write_skill(&src, "x", "new");
        write_skill(&home.join(DISABLED_DIR), "x", "old"); // 冲突在禁用目录
        let r = import_from(&src, &home, &[ImportItem { name: "x".into(), overwrite: true }]);
        assert_eq!(r[0].status, "imported");
        assert!(home.join("skills/x/SKILL.md").is_file());
        assert!(!home.join(format!("{DISABLED_DIR}/x")).exists());
    }

    #[test]
    fn import_rejects_traversal_and_missing() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        fs::create_dir_all(&src).unwrap();
        let items = vec![
            ImportItem { name: "..".into(), overwrite: false },
            ImportItem { name: "a/b".into(), overwrite: false },
            ImportItem { name: "ghost".into(), overwrite: false },
        ];
        let r = import_from(&src, &home, &items);
        assert!(r.iter().all(|x| x.status == "error"));
        assert!(!home.join("skills").exists() || fs::read_dir(home.join("skills")).unwrap().next().is_none());
    }

    // ---- source_info ----

    #[test]
    fn source_info_marks_conflict_including_disabled() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        write_skill(&src, "dup", "d");
        write_skill(&src, "fresh", "d");
        write_skill(&home.join(DISABLED_DIR), "dup", "d");
        let s = ("codex".to_string(), "Codex".to_string(), src);
        let info = source_info(&s, &home);
        assert!(info.exists);
        assert_eq!(info.skills.len(), 2);
        let dup = info.skills.iter().find(|k| k.name == "dup").unwrap();
        let fresh = info.skills.iter().find(|k| k.name == "fresh").unwrap();
        assert!(dup.conflict);
        assert!(!fresh.conflict);
    }

    #[test]
    fn source_info_missing_dir() {
        let t = tempfile::tempdir().unwrap();
        let s = ("codex".to_string(), "Codex".to_string(), t.path().join("nope"));
        let info = source_info(&s, t.path());
        assert!(!info.exists);
        assert!(info.skills.is_empty());
    }

    #[test]
    fn valid_name_rules() {
        assert!(valid_name("brainstorming"));
        assert!(!valid_name(""));
        assert!(!valid_name("."));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a\\b"));
    }

    // ---- seed_auto_import ----

    #[test]
    fn seed_imports_all_and_writes_marker() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        write_skill(&src, "a", "1");
        write_skill(&src, "b", "2");
        seed_auto_import(&src, &home).unwrap();
        assert!(home.join("skills/a/SKILL.md").is_file());
        assert!(home.join("skills/b/SKILL.md").is_file());
        let marker = fs::read_to_string(home.join(".skills-seeded")).unwrap();
        assert!(marker.contains("a") && marker.contains("b"));
    }

    #[test]
    fn seed_does_not_resurrect_deleted() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        write_skill(&src, "a", "1");
        seed_auto_import(&src, &home).unwrap();
        delete_skill_dir(&home, "a").unwrap(); // 用户从壳里删掉
        seed_auto_import(&src, &home).unwrap(); // 再扫：不得复活
        assert!(!home.join("skills/a").exists());
    }

    #[test]
    fn seed_picks_up_new_skills_later() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        write_skill(&src, "a", "1");
        seed_auto_import(&src, &home).unwrap();
        write_skill(&src, "late", "3"); // 后来在独立 dsh 里新增
        seed_auto_import(&src, &home).unwrap();
        assert!(home.join("skills/late/SKILL.md").is_file());
    }

    #[test]
    fn seed_skips_existing_without_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        write_skill(&src, "x", "new");
        write_skill(&home.join(DISABLED_DIR), "x", "mine"); // 壳里已有（含停用态）
        seed_auto_import(&src, &home).unwrap();
        let body = fs::read_to_string(home.join(format!("{DISABLED_DIR}/x/SKILL.md"))).unwrap();
        assert!(body.contains("mine"));
        assert!(!home.join("skills/x").exists());
    }

    #[test]
    fn seed_missing_source_is_noop() {
        let t = tempfile::tempdir().unwrap();
        seed_auto_import(&t.path().join("nope"), &t.path().join("home")).unwrap();
        assert!(!t.path().join("home/skills").exists());
    }

    // ---- zip 导入 ----

    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        use std::io::Write;
        let f = fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }

    const ZIP_SKILL_MD: &str = "---\nname: draw-io\ndescription: \"draw diagrams\"\n---\n\nbody\n";

    #[test]
    fn zip_folder_layout_inspect_and_import() {
        let t = tempfile::tempdir().unwrap();
        let zip_path = t.path().join("draw-io.zip");
        write_zip(
            &zip_path,
            &[
                ("draw-io/SKILL.md", ZIP_SKILL_MD),
                ("draw-io/scripts/run.sh", "echo hi"),
                ("draw-io/references/a.md", "ref"),
            ],
        );
        let home = t.path().join("home");
        let skills = inspect_zip(&zip_path, &home).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "draw-io");
        assert_eq!(skills[0].description, "draw diagrams");
        assert!(!skills[0].conflict);

        let r = import_zip(&zip_path, &home, &[ImportItem { name: "draw-io".into(), overwrite: false }]);
        assert_eq!(r[0].status, "imported");
        assert!(home.join("skills/draw-io/SKILL.md").is_file());
        assert!(home.join("skills/draw-io/scripts/run.sh").is_file());
        assert!(home.join("skills/draw-io/references/a.md").is_file());
    }

    #[test]
    fn zip_root_skill_md_uses_frontmatter_name() {
        let t = tempfile::tempdir().unwrap();
        let zip_path = t.path().join("pack.zip");
        write_zip(&zip_path, &[("SKILL.md", ZIP_SKILL_MD), ("notes.md", "n")]);
        let home = t.path().join("home");
        let skills = inspect_zip(&zip_path, &home).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "draw-io"); // frontmatter name 优先于文件名
        let r = import_zip(&zip_path, &home, &[ImportItem { name: "draw-io".into(), overwrite: false }]);
        assert_eq!(r[0].status, "imported");
        assert!(home.join("skills/draw-io/notes.md").is_file());
    }

    #[test]
    fn zip_root_skill_md_falls_back_to_zip_stem() {
        let t = tempfile::tempdir().unwrap();
        let zip_path = t.path().join("cool-skill.zip");
        write_zip(&zip_path, &[("SKILL.md", "---\ndescription: \"d\"\n---\n")]);
        let home = t.path().join("home");
        let skills = inspect_zip(&zip_path, &home).unwrap();
        assert_eq!(skills[0].name, "cool-skill");
    }

    #[test]
    fn zip_multiple_top_level_skills() {
        let t = tempfile::tempdir().unwrap();
        let zip_path = t.path().join("bundle.zip");
        write_zip(
            &zip_path,
            &[
                ("b-skill/SKILL.md", "---\ndescription: \"b\"\n---\n"),
                ("a-skill/SKILL.md", "---\ndescription: \"a\"\n---\n"),
                ("a-skill/x.py", "pass"),
            ],
        );
        let home = t.path().join("home");
        let skills = inspect_zip(&zip_path, &home).unwrap();
        assert_eq!(skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["a-skill", "b-skill"]);
        let r = import_zip(&zip_path, &home, &[ImportItem { name: "b-skill".into(), overwrite: false }]);
        assert_eq!(r[0].status, "imported");
        assert!(home.join("skills/b-skill/SKILL.md").is_file());
        assert!(!home.join("skills/a-skill").exists()); // 只导入选中的
    }

    #[test]
    fn zip_invalid_inputs_error() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        // 没有 SKILL.md
        let p1 = t.path().join("no-skill.zip");
        write_zip(&p1, &[("readme.md", "x")]);
        assert!(inspect_zip(&p1, &home).is_err());
        // 不是 zip
        let p2 = t.path().join("garbage.zip");
        fs::write(&p2, b"not a zip").unwrap();
        assert!(inspect_zip(&p2, &home).is_err());
        // import 同样报错（每个 item 一个 error）
        let r = import_zip(&p1, &home, &[ImportItem { name: "x".into(), overwrite: false }]);
        assert_eq!(r[0].status, "error");
    }

    #[test]
    fn zip_conflict_skip_then_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let zip_path = t.path().join("draw-io.zip");
        write_zip(&zip_path, &[("draw-io/SKILL.md", ZIP_SKILL_MD)]);
        let home = t.path().join("home");
        write_skill(&home.join("skills"), "draw-io", "old-version");

        let skills = inspect_zip(&zip_path, &home).unwrap();
        assert!(skills[0].conflict); // 已存在 → 标记冲突

        let skip = import_zip(&zip_path, &home, &[ImportItem { name: "draw-io".into(), overwrite: false }]);
        assert_eq!(skip[0].status, "skipped");
        let body = fs::read_to_string(home.join("skills/draw-io/SKILL.md")).unwrap();
        assert!(body.contains("old-version"));

        let ow = import_zip(&zip_path, &home, &[ImportItem { name: "draw-io".into(), overwrite: true }]);
        assert_eq!(ow[0].status, "imported");
        let body = fs::read_to_string(home.join("skills/draw-io/SKILL.md")).unwrap();
        assert!(body.contains("draw diagrams"));
    }

    #[test]
    fn zip_overwrite_also_clears_disabled_copy() {
        let t = tempfile::tempdir().unwrap();
        let zip_path = t.path().join("draw-io.zip");
        write_zip(&zip_path, &[("draw-io/SKILL.md", ZIP_SKILL_MD)]);
        let home = t.path().join("home");
        write_skill(&home.join(DISABLED_DIR), "draw-io", "old"); // 冲突在禁用目录
        let r = import_zip(&zip_path, &home, &[ImportItem { name: "draw-io".into(), overwrite: true }]);
        assert_eq!(r[0].status, "imported");
        assert!(home.join("skills/draw-io/SKILL.md").is_file());
        assert!(!home.join(format!("{DISABLED_DIR}/draw-io")).exists());
    }

    #[test]
    fn zip_traversal_entries_never_escape() {
        let t = tempfile::tempdir().unwrap();
        let zip_path = t.path().join("evil.zip");
        write_zip(
            &zip_path,
            &[("draw-io/SKILL.md", ZIP_SKILL_MD), ("draw-io/../../evil.txt", "evil")],
        );
        let home = t.path().join("home");
        let r = import_zip(&zip_path, &home, &[ImportItem { name: "draw-io".into(), overwrite: false }]);
        assert_eq!(r[0].status, "imported");
        assert!(!t.path().join("evil.txt").exists());
        assert!(!home.join("skills/evil.txt").exists());
        assert!(home.join("skills/draw-io/SKILL.md").is_file());
    }

    #[test]
    fn zip_import_rejects_bad_item_name() {
        let t = tempfile::tempdir().unwrap();
        let zip_path = t.path().join("draw-io.zip");
        write_zip(&zip_path, &[("draw-io/SKILL.md", ZIP_SKILL_MD)]);
        let home = t.path().join("home");
        let r = import_zip(
            &zip_path,
            &home,
            &[
                ImportItem { name: "..".into(), overwrite: false },
                ImportItem { name: "ghost".into(), overwrite: false }, // zip 里没有
            ],
        );
        assert!(r.iter().all(|x| x.status == "error"));
        assert!(!home.join("skills").exists());
    }
}

