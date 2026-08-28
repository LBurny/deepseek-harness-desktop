use std::path::{Path, PathBuf};

/// 播放结果上报回调：把真实播放结果（成功耗时/失败原因）交调用侧落 events.log
/// 并推诊断面板实时流。播放是后台线程完成的，错误无法走同步返回值，只能经此
/// 上报；None = 不上报（测试/桩实现用）。
pub type SoundDiag = std::sync::Arc<dyn Fn(String) + Send + Sync>;

/// 平台抽象层：所有平台相关的路径、可执行文件名、进程树回收都收敛在这里。
/// 后期支持 macOS/Linux 时，新增 platform/macos.rs / platform/linux.rs 实现本 trait，
/// 并替换下面的 compile_error! 占位。
pub trait Platform: Send + Sync {
    fn node_exe_name(&self) -> &'static str;
    /// 远程访问隧道可执行文件名（随 runtime 内嵌分发）
    fn cloudflared_exe_name(&self) -> &'static str;
    /// 应用数据根目录（Windows: %LOCALAPPDATA%\DSHDesktop）
    fn runtime_base_dir(&self) -> PathBuf;
    /// 安装包资源目录中内嵌运行时所在路径：<resource_dir>/runtime/<triplet>
    fn resource_runtime_dir(&self, resource_dir: &Path) -> PathBuf;
    fn runtime_triplet(&self) -> &'static str;
    /// 回收整个进程树（dsh 可能派生 python 等子进程）
    fn kill_process_tree(&self, pid: u32);
    /// 子进程创建前的平台化配置（Windows 设 CREATE_NO_WINDOW 隐藏控制台窗口）
    fn configure_child_command(&self, _cmd: &mut tokio::process::Command) {}
    /// 注册刚 spawn 的子进程：保证本进程以任何方式退出（含被安装器/任务管理器
    /// 强杀）时由系统连带回收，杜绝孤儿进程锁住 runtime 目录导致重装失败。
    /// Windows 用 Job Object + KILL_ON_JOB_CLOSE；其它平台默认 no-op。
    fn register_child(&self, _pid: u32) {}
    /// 系统是否处于深色模式（dsh 主题为 system 时用来解析）
    fn system_dark_mode(&self) -> bool;
    /// 系统 UI 语言是否中文（dsh locale.preference 缺省时用来解析，对齐 dsh 的"跟随浏览器"）
    fn system_prefers_chinese(&self) -> bool;
    /// 播放一个 wav 文件（柔和通知提示音，全部通知类型共用）：文件缺失立即返回
    /// Err（调用侧降级 toast 系统默认音）；真实播放结果（含失败原因/重试）经
    /// diag 上报。非阻塞：派发后立即返回，不卡调用线程。
    fn play_sound_file(&self, path: &Path, diag: Option<SoundDiag>) -> Result<(), String>;
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::WindowsPlatform;

#[cfg(target_os = "macos")]
compile_error!("macOS platform implementation pending: add platform/macos.rs implementing Platform");
#[cfg(target_os = "linux")]
compile_error!("Linux platform implementation pending: add platform/linux.rs implementing Platform");

pub fn current() -> Box<dyn Platform> {
    #[cfg(windows)]
    {
        Box::new(WindowsPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_platform_basics() {
        let p = current();
        assert_eq!(p.node_exe_name(), "node.exe");
        assert_eq!(p.cloudflared_exe_name(), "cloudflared.exe");
        assert_eq!(p.runtime_triplet(), "windows-x64");
        assert!(p.runtime_base_dir().ends_with("DSHDesktop"));
        let r = p.resource_runtime_dir(Path::new("C:\\res"));
        assert_eq!(r, PathBuf::from("C:\\res").join("runtime").join("windows-x64"));
    }

    #[test]
    fn play_sound_file_missing_errors() {
        let p = current();
        assert!(p
            .play_sound_file(Path::new("C:\\nonexistent\\nope.wav"), None)
            .is_err());
    }

    /// 锚定"派发即返回"契约：真实播放走后台线程，调用侧毫秒级返回不卡预览/通知。
    /// 顺带覆盖文件存在时 Ok 返回路径（播放本身无音频设备也会在后台线程失败重试，
    /// 不影响本断言）。
    #[test]
    fn play_sound_file_dispatches_without_blocking() {
        let p = current();
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("fake.wav");
        std::fs::write(&wav, b"RIFF....WAVEjunk").unwrap();
        let t0 = std::time::Instant::now();
        let r = p.play_sound_file(&wav, None).unwrap();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let _ = r;
        assert!(ms < 1000.0, "play_sound_file 派发不应阻塞调用线程，实测 {ms:.0}ms");
    }
}
