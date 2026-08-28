use crate::platform::Platform;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("bundled runtime incomplete: missing {0}")]
    Incomplete(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("copy error: {0}")]
    Copy(#[from] fs_extra::error::Error),
}

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub node_exe: PathBuf,
    pub dsh_bin: PathBuf,
    pub home: PathBuf,
    pub work_dir: PathBuf,
    /// 远程访问隧道（cloudflared）；可能不存在，缺失检查在使用方（remote 模块）
    pub cloudflared_exe: PathBuf,
}

/// 确定运行时路径。
/// 安装目录可写（默认的按用户安装）时**原地运行**内嵌运行时，省掉约 300MB 的
/// 部署副本；安装目录只读（如装到 Program Files）时回退为复制到应用数据目录
/// （目标目录的 `.version` 与 app_version 不一致时全量重新复制）。
/// on_copy_progress：回退部署时按字节回调（copied, total），用于首启进度条。
pub fn ensure_runtime(
    platform: &dyn Platform,
    source_dir: &Path,
    app_version: &str,
    on_copy_progress: Option<&dyn Fn(u64, u64)>,
) -> Result<RuntimePaths, RuntimeError> {
    // Tauri 的 resource_dir 在 Windows 上带 \\?\ 扩展前缀；Node 的模块加载器
    // 不认它（会把入口解析成盘符相对路径，EISDIR lstat 'F:'），必须剥掉
    let source_dir = &strip_verbatim(source_dir);
    validate_source(source_dir, platform)?;
    let base = platform.runtime_base_dir();
    let home = base.join("dsh-home");
    fs::create_dir_all(&home)?;

    if is_writable(source_dir) {
        // 清理旧版本留下的部署副本（带 .version 标记的才是我们创建的）
        let legacy = base.join("runtime");
        if legacy.join(".version").is_file() {
            let _ = fs::remove_dir_all(&legacy);
        }
        return Ok(paths_for(source_dir, home, base, platform));
    }

    let target = base.join("runtime");
    deploy_if_needed(source_dir, &target, app_version, on_copy_progress)?;
    Ok(paths_for(&target, home, base, platform))
}

fn paths_for(
    runtime_dir: &Path,
    home: PathBuf,
    base: PathBuf,
    platform: &dyn Platform,
) -> RuntimePaths {
    RuntimePaths {
        node_exe: runtime_dir.join(platform.node_exe_name()),
        dsh_bin: crate::upstream::dsh_bin(runtime_dir),
        home,
        // cwd 保持在可写的应用数据目录，与运行时本体解耦
        work_dir: base,
        cloudflared_exe: runtime_dir.join(platform.cloudflared_exe_name()),
    }
}

/// 剥离 Windows 扩展路径前缀 \\?\（仅当剩余部分是常规盘符绝对路径，如 C:\…）；
/// \\?\UNC\ 等形态原样保留。与 dunce::simplified 的保守策略一致。
pub(crate) fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    p.to_path_buf()
}

/// 目录可写性探测：尝试创建临时文件（create_new 避免误伤同名文件）。
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".dshdesktop-write-probe");
    match fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 部署副本（只读安装目录的回退路径）：版本不一致则全量重新复制。
fn deploy_if_needed(
    source_dir: &Path,
    target: &Path,
    app_version: &str,
    on_copy_progress: Option<&dyn Fn(u64, u64)>,
) -> Result<(), RuntimeError> {
    let version_file = target.join(".version");
    let current = fs::read_to_string(&version_file).unwrap_or_default();
    if current.trim() == app_version {
        return Ok(());
    }
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    fs::create_dir_all(target)?;
    let mut opts = fs_extra::dir::CopyOptions::new();
    opts.content_only = true;
    match on_copy_progress {
        Some(cb) => {
            fs_extra::dir::copy_with_progress(source_dir, target, &opts, |t| {
                cb(t.copied_bytes, t.total_bytes);
                fs_extra::dir::TransitProcessResult::ContinueOrAbort
            })?;
        }
        None => {
            fs_extra::dir::copy(source_dir, target, &opts)?;
        }
    }
    fs::write(&version_file, app_version)?;
    Ok(())
}

fn validate_source(src: &Path, platform: &dyn Platform) -> Result<(), RuntimeError> {
    let node = src.join(platform.node_exe_name());
    if !node.is_file() {
        return Err(RuntimeError::Incomplete(node.display().to_string()));
    }
    let bin = crate::upstream::dsh_bin(src);
    if !bin.is_file() {
        return Err(RuntimeError::Incomplete(bin.display().to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPlatform {
        base: PathBuf,
    }

    impl Platform for TestPlatform {
        fn node_exe_name(&self) -> &'static str {
            "node.exe"
        }
        fn cloudflared_exe_name(&self) -> &'static str {
            "cloudflared.exe"
        }
        fn runtime_base_dir(&self) -> PathBuf {
            self.base.clone()
        }
        fn resource_runtime_dir(&self, _: &Path) -> PathBuf {
            PathBuf::from("unused-in-tests")
        }
        fn runtime_triplet(&self) -> &'static str {
            "windows-x64"
        }
        fn kill_process_tree(&self, _pid: u32) {}
        fn system_dark_mode(&self) -> bool {
            false
        }
        fn system_prefers_chinese(&self) -> bool {
            true
        }
        fn play_sound_file(&self, _path: &Path, _diag: Option<crate::platform::SoundDiag>) -> Result<(), String> {
            Ok(())
        }
    }

    fn make_source() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().to_path_buf();
        fs::write(src.join("node.exe"), b"fake").unwrap();
        let bin = src
            .join("dsh")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("bin.js"), b"// fake").unwrap();
        (dir, src)
    }

    #[test]
    fn in_place_when_source_writable() {
        let (_s, src) = make_source();
        let base = tempfile::tempdir().unwrap();
        let p = TestPlatform { base: base.path().to_path_buf() };
        let paths = ensure_runtime(&p, &src, "0.1.0", None).unwrap();
        assert_eq!(paths.node_exe, src.join("node.exe"), "应原地运行而非复制");
        assert_eq!(paths.cloudflared_exe, src.join("cloudflared.exe"));
        assert!(!base.path().join("runtime").exists(), "不应产生部署副本");
        assert!(paths.home.is_dir());
    }

    #[test]
    fn in_place_cleans_legacy_deploy() {
        let (_s, src) = make_source();
        let base = tempfile::tempdir().unwrap();
        // 模拟旧版本留下的部署副本
        let legacy = base.path().join("runtime");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join(".version"), "0.0.9").unwrap();
        fs::write(legacy.join("stale.bin"), b"old").unwrap();
        let p = TestPlatform { base: base.path().to_path_buf() };
        ensure_runtime(&p, &src, "0.1.0", None).unwrap();
        assert!(!legacy.exists(), "历史部署副本应被清理");
    }

    #[test]
    fn tempdir_is_writable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_writable(dir.path()));
    }

    /// std::fs::canonicalize 在 Windows 上返回带 \\?\ 前缀的路径（与 Tauri
    /// resource_dir 同形态），须被剥成常规路径，否则 Node 无法加载入口
    #[cfg(windows)]
    #[test]
    fn verbatim_source_path_is_normalized() {
        let (_s, src) = make_source();
        let canonical = std::fs::canonicalize(&src).unwrap();
        assert!(canonical.to_string_lossy().starts_with(r"\\?\"));
        let base = tempfile::tempdir().unwrap();
        let p = TestPlatform { base: base.path().to_path_buf() };
        let paths = ensure_runtime(&p, &canonical, "0.1.0", None).unwrap();
        assert!(!paths.node_exe.to_string_lossy().starts_with(r"\\?\"));
        assert!(paths.node_exe.is_file());
        assert!(paths.dsh_bin.is_file());
    }

    #[test]
    fn strip_verbatim_only_for_drive_paths() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\F:\DSHDesktop\runtime")),
            PathBuf::from(r"F:\DSHDesktop\runtime")
        );
        // 非盘符形态保留原样
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"C:\plain\path")),
            PathBuf::from(r"C:\plain\path")
        );
    }

    #[test]
    fn deploy_copies_when_version_differs() {
        let (_s, src) = make_source();
        let target = tempfile::tempdir().unwrap();
        let target = target.path().join("runtime");
        deploy_if_needed(&src, &target, "0.1.0", None).unwrap();
        assert!(target.join("node.exe").is_file());
        assert!(target.join(".version").is_file());
    }

    #[test]
    fn deploy_skips_when_version_same() {
        let (_s, src) = make_source();
        let target = tempfile::tempdir().unwrap();
        let target = target.path().join("runtime");
        deploy_if_needed(&src, &target, "0.1.0", None).unwrap();
        let marker = target.join("marker.txt");
        fs::write(&marker, b"keep me").unwrap();
        deploy_if_needed(&src, &target, "0.1.0", None).unwrap();
        assert!(marker.is_file(), "版本相同不应重新复制");
    }

    #[test]
    fn deploy_recopies_when_version_changes() {
        let (_s, src) = make_source();
        let target = tempfile::tempdir().unwrap();
        let target = target.path().join("runtime");
        deploy_if_needed(&src, &target, "0.1.0", None).unwrap();
        let marker = target.join("marker.txt");
        fs::write(&marker, b"stale").unwrap();
        deploy_if_needed(&src, &target, "0.2.0", None).unwrap();
        assert!(!marker.exists(), "版本变化应全量重新复制");
    }

    #[test]
    fn missing_bin_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("node.exe"), b"fake").unwrap();
        let base = tempfile::tempdir().unwrap();
        let p = TestPlatform { base: base.path().to_path_buf() };
        let err = ensure_runtime(&p, dir.path(), "0.1.0", None).unwrap_err();
        assert!(matches!(err, RuntimeError::Incomplete(_)));
    }

    #[test]
    fn deploy_reports_copy_progress() {
        use std::cell::RefCell;
        let (_s, src) = make_source();
        // 多放几个有内容的文件，确保有字节可复制
        for i in 0..3 {
            fs::write(src.join(format!("file{i}.bin")), vec![7u8; 1024]).unwrap();
        }
        let target = tempfile::tempdir().unwrap();
        let target = target.path().join("runtime");
        let calls = RefCell::new(Vec::<(u64, u64)>::new());
        let cb = |copied: u64, total: u64| calls.borrow_mut().push((copied, total));
        deploy_if_needed(&src, &target, "0.1.0", Some(&cb)).unwrap();
        let calls = calls.borrow();
        assert!(!calls.is_empty(), "复制过程应有进度回调");
        let total = calls[0].1;
        assert!(total > 0);
        let mut last = 0;
        for &(copied, t) in calls.iter() {
            assert_eq!(t, total, "总量应稳定");
            assert!(copied >= last, "已复制字节数应单调不减");
            last = copied;
        }
        assert_eq!(last, total, "最后一次回调应达到总量");
    }
}
