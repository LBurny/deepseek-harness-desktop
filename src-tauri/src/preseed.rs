//! 预安装插件播种：随安装包分发（bundle.resources 映射 resources/preseed-plugins
//! → <install>/preseed-plugins/）的 dsh 插件在首启时种入用户 profile——文件同步到
//! $DSH_HOME/profiles/plugins/<name>，再走官方 `dsh plugin --profile web add <path>`
//! （pnpm link 依赖；插件以 bundle 形态分发，自带 dsh.bundle.patch 自我挂载，
//! reconcile 自动把它并进 dsh.profile.bundles 层列表）。
//!
//! 因为走 bundle 层而不是往 cordis.patch.yml 写 insert，用户在插件管理面板删除
//! （dsh plugin remove）时 reconcile 会自动把层摘掉，无任何残留挂载点。
//!
//! marker 文件 profiles/plugins/.plugins-preseeded 记录种过的名字：依赖还在 = 已装
//! （同步文件，壳升级带的提示词修订随之下发）；依赖消失而 marker 在 = 用户主动删除
//! → 不复活（语义同 skills.rs 的 .skills-seeded）。
//!
//! 源目录不存在 = 无操作（dev 模式 tauri 不拷贝 bundle.resources，同 sounds 静默降级）；
//! 单个插件失败不拖累其余，整体失败由调用侧记 events.log，不阻断启动。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::plugins::PluginsHome;

const MARKER_FILE: &str = ".plugins-preseeded";

#[derive(Default, Debug, PartialEq, Eq)]
pub struct SeedReport {
    /// 本次新装（首次播种）的插件名。
    pub installed: Vec<String>,
    /// 已装且文件有更新（壳升级下发了新版插件文件）的插件名。
    pub synced: Vec<String>,
    /// 用户删除后按不复活原则跳过的插件名。
    pub skipped_removed: Vec<String>,
}

impl SeedReport {
    pub fn is_quiet(&self) -> bool {
        self.installed.is_empty() && self.synced.is_empty() && self.skipped_removed.is_empty()
    }
}

/// 生产入口：add 走壳分发运行时的官方 `dsh plugin add`（同插件管理面板）。
pub fn seed_preinstalled_plugins(home: &PluginsHome, source_dir: &Path) -> Result<SeedReport, String> {
    let mut add = |name: &str, dest: &Path| -> Result<(), String> {
        let spec = dest.to_string_lossy().into_owned();
        let out = crate::plugins::run_plugin_op(home, &["add", &spec])?;
        if out.exit_code != 0 {
            return Err(format!(
                "dsh plugin add {name} 失败（exit {}）：{}",
                out.exit_code,
                out.output.trim()
            ));
        }
        Ok(())
    };
    seed_inner(home, source_dir, &mut add)
}

fn seed_inner(
    home: &PluginsHome,
    source_dir: &Path,
    add: &mut dyn FnMut(&str, &Path) -> Result<(), String>,
) -> Result<SeedReport, String> {
    let mut report = SeedReport::default();
    if !source_dir.is_dir() {
        return Ok(report);
    }
    let plugins_root = home.home.join("profiles").join("plugins");
    let marker_path = plugins_root.join(MARKER_FILE);
    let mut seen: BTreeSet<String> = fs::read_to_string(&marker_path)
        .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    let installed_deps = read_dependency_names(&home.manifest_path());

    let mut entries: Vec<PathBuf> = fs::read_dir(source_dir)
        .map_err(|e| format!("读取预安装插件目录失败：{e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();

    let mut first_error: Option<String> = None;
    for src in entries {
        let name = src.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let was_seen = seen.contains(&name);
        let is_installed = installed_deps.contains(&name);
        if was_seen && !is_installed {
            // 用户在插件面板删过：不复活、不碰文件
            report.skipped_removed.push(name);
            continue;
        }
        let dest = plugins_root.join(&name);
        let changed = match sync_plugin_files(&src, &dest) {
            Ok(changed) => changed,
            Err(e) => {
                first_error.get_or_insert(format!("同步预安装插件 {name} 文件失败：{e}"));
                continue;
            }
        };
        if is_installed {
            seen.insert(name.clone());
            if changed {
                // 壳升级下发了新版插件文件
                report.synced.push(name);
            }
            continue;
        }
        match add(&name, &dest) {
            Ok(()) => {
                seen.insert(name.clone());
                report.installed.push(name);
            }
            Err(e) => {
                // 不记 marker：下次启动重试
                first_error.get_or_insert(e);
            }
        }
    }

    fs::create_dir_all(&plugins_root).map_err(|e| format!("创建 {} 失败：{e}", plugins_root.display()))?;
    let names: Vec<_> = seen.into_iter().collect();
    fs::write(&marker_path, names.join("\n") + "\n").map_err(|e| format!("写 marker 失败：{e}"))?;

    match first_error {
        Some(e) => Err(e),
        None => Ok(report),
    }
}

/// 读 profile manifest 的 dependencies 键名集合；文件缺失/损坏按空集合处理
/// （损坏时后面的 dsh plugin add 会以真实错误失败，比这里抢先报错更准确）。
fn read_dependency_names(manifest_path: &Path) -> BTreeSet<String> {
    let Ok(bytes) = fs::read(manifest_path) else {
        return BTreeSet::new();
    };
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|m| {
            m.get(crate::upstream::MANIFEST_DEPENDENCIES_KEY)?
                .as_object()
                .map(|o| o.keys().cloned().collect())
        })
        .unwrap_or_default()
}

/// 递归把 src 覆盖复制到 dest（逐文件字节比对，相同跳过），返回是否有文件被写入。
/// 不删除 dest 里多出的文件：用户可能在插件目录里放了自己的东西；卸载清理由
/// dsh plugin remove 管，不归播种。
fn sync_plugin_files(src: &Path, dest: &Path) -> Result<bool, String> {
    let mut changed = false;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let s = entry.path();
        let d = dest.join(entry.file_name());
        if s.is_dir() {
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            changed |= sync_plugin_files(&s, &d)?;
        } else {
            let new = fs::read(&s).map_err(|e| e.to_string())?;
            let same = fs::read(&d).map(|old| old == new).unwrap_or(false);
            if !same {
                if let Some(parent) = d.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::write(&d, &new).map_err(|e| e.to_string())?;
                changed = true;
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_home() -> (tempfile::TempDir, PluginsHome) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("dsh-home");
        // manifest 所在目录由 dsh plugin add 初始化；测试里按需要自建
        let h = PluginsHome::new(dir.path().join("node.exe"), dir.path().join("bin.js"), home);
        (dir, h)
    }

    fn write_source_plugin(root: &Path, name: &str, files: &[(&str, &str)]) {
        for (rel, content) in files {
            let p = root.join(name).join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
    }

    fn write_manifest(home: &PluginsHome, deps: &[&str]) {
        let path = home.manifest_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let dep_map: serde_json::Map<String, serde_json::Value> = deps
            .iter()
            .map(|d| (d.to_string(), serde_json::Value::String(format!("link:x/{d}"))))
            .collect();
        let m = serde_json::json!({
            "name": "dsh-profile-web",
            "dependencies": serde_json::Value::Object(dep_map),
        });
        fs::write(path, serde_json::to_string_pretty(&m).unwrap()).unwrap();
    }

    fn marker_names(home: &PluginsHome) -> Vec<String> {
        fs::read_to_string(home.home.join("profiles/plugins").join(MARKER_FILE))
            .unwrap()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn first_seed_copies_installs_and_marks() {
        let (dir, home) = fixture_home();
        let src = dir.path().join("src");
        write_source_plugin(&src, "dsh-command-init", &[("package.json", "{}"), ("index.js", "v1")]);
        let mut added: Vec<String> = Vec::new();
        let report = seed_inner(&home, &src, &mut |name, dest| {
            added.push(format!("{name}@{}", dest.file_name().unwrap().to_string_lossy()));
            Ok(())
        })
        .unwrap();
        assert_eq!(report.installed, vec!["dsh-command-init"]);
        assert_eq!(added, vec!["dsh-command-init@dsh-command-init"]);
        let dest = home.home.join("profiles/plugins/dsh-command-init");
        assert_eq!(fs::read_to_string(dest.join("index.js")).unwrap(), "v1");
        assert_eq!(marker_names(&home), vec!["dsh-command-init"]);
    }

    #[test]
    fn failed_add_is_not_marked_and_retried_next_launch() {
        let (dir, home) = fixture_home();
        let src = dir.path().join("src");
        write_source_plugin(&src, "dsh-command-init", &[("index.js", "v1")]);
        let mut calls = 0;
        let err = seed_inner(&home, &src, &mut |_, _| {
            calls += 1;
            Err("pnpm 不在".into())
        })
        .unwrap_err();
        assert!(err.contains("pnpm"), "{err}");
        assert!(home.home.join("profiles/plugins/dsh-command-init/index.js").is_file(), "文件仍应同步");
        assert!(marker_names(&home).is_empty(), "失败不记 marker");
        // 下次启动重试成功
        seed_inner(&home, &src, &mut |_, _| Ok(())).unwrap();
        assert_eq!(marker_names(&home), vec!["dsh-command-init"]);
        assert_eq!(calls, 1);
    }

    #[test]
    fn installed_plugin_is_synced_not_readded() {
        let (dir, home) = fixture_home();
        let src = dir.path().join("src");
        write_source_plugin(&src, "dsh-command-init", &[("index.js", "v1")]);
        seed_inner(&home, &src, &mut |_, _| Ok(())).unwrap();
        write_manifest(&home, &["dsh-command-init"]);
        // 壳升级：源文件变了
        write_source_plugin(&src, "dsh-command-init", &[("index.js", "v2")]);
        let mut adds = 0;
        let report = seed_inner(&home, &src, &mut |_, _| {
            adds += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(adds, 0, "已装不再 add");
        assert_eq!(report.synced, vec!["dsh-command-init"]);
        let dest = home.home.join("profiles/plugins/dsh-command-init/index.js");
        assert_eq!(fs::read_to_string(dest).unwrap(), "v2", "文件随壳升级更新");
    }

    #[test]
    fn user_removed_plugin_is_not_resurrected() {
        let (dir, home) = fixture_home();
        let src = dir.path().join("src");
        write_source_plugin(&src, "dsh-command-init", &[("index.js", "v1")]);
        seed_inner(&home, &src, &mut |_, _| Ok(())).unwrap();
        write_manifest(&home, &["dsh-command-init"]);
        // 用户在插件面板删除：manifest 依赖消失
        write_manifest(&home, &[]);
        let mut adds = 0;
        let report = seed_inner(&home, &src, &mut |_, _| {
            adds += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(adds, 0, "用户删除后不得重新 add");
        assert_eq!(report.skipped_removed, vec!["dsh-command-init"]);
        assert_eq!(report.installed.len(), 0);
    }

    #[test]
    fn self_installed_plugin_is_recorded_without_add() {
        let (dir, home) = fixture_home();
        let src = dir.path().join("src");
        write_source_plugin(&src, "dsh-command-init", &[("index.js", "v1")]);
        // 用户自己先装了（marker 没有、dep 有）
        write_manifest(&home, &["dsh-command-init"]);
        let mut adds = 0;
        let report = seed_inner(&home, &src, &mut |_, _| {
            adds += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(adds, 0);
        assert_eq!(report.synced, vec!["dsh-command-init"]);
        assert_eq!(marker_names(&home), vec!["dsh-command-init"], "补记 marker 以便日后识别删除");
    }

    #[test]
    fn missing_source_is_noop() {
        let (dir, home) = fixture_home();
        let report = seed_preinstalled_plugins(&home, &dir.path().join("不存在")).unwrap();
        assert!(report.is_quiet());
    }

    #[test]
    fn bundled_preseed_layout_matches_resource_mapping() {
        // 播种源在 resource_dir()/preseed-plugins/<name>/，依赖 bundle.resources 把
        // resources/preseed-plugins 映射为安装根 preseed-plugins/——映射缺失/错位则
        // 首启静默无插件（同 0.1.16 sounds 映射坑，锚定测试同
        // settings.rs bundled_sound_layout_matches_probe_path）。
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            conf["bundle"]["resources"]["resources/preseed-plugins"],
            "preseed-plugins"
        );
        // 每个随包插件必须声明 dsh.bundle 且 patch 文件存在——否则 dsh plugin add
        // 只装依赖不挂层（reconcile 只收 bundle），命令永远不会出现在面板里。
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/preseed-plugins");
        let mut count = 0;
        for entry in std::fs::read_dir(&root).unwrap() {
            let dir = entry.unwrap().path();
            if !dir.is_dir() {
                continue;
            }
            count += 1;
            let manifest: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.join("package.json")).unwrap(),
            )
            .unwrap();
            let patch = manifest["dsh"]["bundle"]["patch"]
                .as_str()
                .unwrap_or_else(|| panic!("{} 缺 dsh.bundle.patch 声明", dir.display()));
            assert!(dir.join(patch).is_file(), "{} 指向的 patch 文件不存在", dir.display());
        }
        assert!(count > 0, "preseed-plugins 源目录不应为空");
    }

    /// /init 插件注入的提示词必须走 plugin+notice 源：UI 把非 user 源渲染成
    /// 一行折叠的「上下文注入」（可展开），kind:"user" 则是完整气泡（0.4.9
    /// 用户实测长提示词气泡太丑）。上游渲染分支漂移由契约套件
    /// probe_preseed_plugin_needles 守门（upstream::CONTEXT_INJECTION_*）。
    #[test]
    fn init_plugin_injects_collapsed_plugin_source() {
        let index = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/preseed-plugins/dsh-command-init/index.js"
        ))
        .unwrap();
        assert!(index.contains(r#"kind: "plugin""#), "index.js 应以 plugin 源注入才折叠");
        assert!(index.contains(r#"form: "notice""#), "index.js 应以 notice form 提供折叠行摘要");
        assert!(!index.contains(r#"kind: "user""#), "kind:user 会渲染成完整用户气泡");
    }
}
