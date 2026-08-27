use super::Platform;
use std::path::{Path, PathBuf};

/// Job Object 回收：把子进程挂进带 KILL_ON_JOB_CLOSE 的 Job，本进程以任何方式
/// 退出（含被 NSIS 安装器/任务管理器强杀）时，内核在最后句柄回收时连带终止所有
/// 成员。不修这个，dsh 的 node.exe / cloudflared.exe 会以孤儿存活并锁住
/// runtime 目录，导致卸载重装写文件失败。
pub(crate) mod job {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// HANDLE 是进程级资源，跨线程共享安全；包一层满足 static 的 Send+Sync 约束
    #[derive(Clone, Copy)]
    struct SendHandle(HANDLE);
    unsafe impl Send for SendHandle {}
    unsafe impl Sync for SendHandle {}

    /// 全局 Job 句柄，刻意永不主动关闭：进程退出（含强杀）时内核回收最后句柄，
    /// 此刻才触发连带终止。空句柄表示创建失败（退化为无保护，行为同旧版）。
    static GLOBAL_JOB: OnceLock<SendHandle> = OnceLock::new();

    pub fn global_job() -> HANDLE {
        GLOBAL_JOB
            .get_or_init(|| SendHandle(unsafe { create_kill_on_close_job() }))
            .0
    }

    /// 新建带 KILL_ON_JOB_CLOSE 的 Job；失败返回空句柄
    pub unsafe fn create_kill_on_close_job() -> HANDLE {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return job;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            CloseHandle(job);
            return std::ptr::null_mut();
        }
        job
    }

    /// 把 pid 挂进 job；任何一步失败都返回 false（尽力而为，不影响主流程）。
    /// 成员的子孙进程默认自动入 Job，整树被连带回收。
    pub unsafe fn assign_pid_to_job(job: HANDLE, pid: u32) -> bool {
        if job.is_null() || pid == 0 {
            return false;
        }
        let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if proc.is_null() {
            return false;
        }
        let ok = AssignProcessToJobObject(job, proc);
        CloseHandle(proc);
        ok != 0
    }
}

pub struct WindowsPlatform;

impl Platform for WindowsPlatform {
    fn node_exe_name(&self) -> &'static str {
        "node.exe"
    }

    fn cloudflared_exe_name(&self) -> &'static str {
        "cloudflared.exe"
    }

    fn runtime_base_dir(&self) -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("DSHDesktop")
    }

    fn resource_runtime_dir(&self, resource_dir: &Path) -> PathBuf {
        resource_dir.join("runtime").join(self.runtime_triplet())
    }

    fn runtime_triplet(&self) -> &'static str {
        "windows-x64"
    }

    fn kill_process_tree(&self, pid: u32) {
        // /T 杀进程树，/F 强制；失败忽略（进程可能已退出）。
        // taskkill 是控制台子系统程序：本进程是无控制台的 GUI 程序，不带
        // CREATE_NO_WINDOW 系统会为它新分配一个可见控制台窗口（退出/重启时闪 cmd）。
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }

    fn configure_child_command(&self, cmd: &mut tokio::process::Command) {
        // tokio 的 Command 在 Windows 上自带 creation_flags
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    fn register_child(&self, pid: u32) {
        unsafe { job::assign_pid_to_job(job::global_job(), pid) };
    }

    fn system_dark_mode(&self) -> bool {
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
            .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
            .map(|v| v == 0)
            .unwrap_or(false)
    }

    fn system_prefers_chinese(&self) -> bool {
        // LANGID 低 10 位是主语言 ID，0x04 = LANG_CHINESE（简繁都算）
        const LANG_CHINESE: u16 = 0x04;
        let lang = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        lang & 0x3FF == LANG_CHINESE
    }

    fn play_sound_file(&self, path: &Path) -> Result<(), String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Media::Audio::PlaySoundW;
        const SND_ASYNC: u32 = 0x0001;
        const SND_FILENAME: u32 = 0x0002_0000;
        // 不带 SND_NOSTOP：该标志的语义是"上一声音效未播完则本次直接放弃"（不排队不混音），
        // 连续试听/连续通知时表现为部分音"没声音"。默认的新调打断旧调才是想要的行为。
        if !path.is_file() {
            return Err(crate::i18n::pick(
                format!("音效文件不存在: {}", path.display()),
                format!("Sound file not found: {}", path.display()),
            ));
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: wide 以 NUL 结尾且本调用期间存活；hmod 传 NULL（文件模式不需要模块句柄）
        let ok = unsafe { PlaySoundW(wide.as_ptr(), std::ptr::null_mut(), SND_FILENAME | SND_ASYNC) };
        if ok == 0 {
            Err(crate::i18n::pick("PlaySoundW 播放失败", "PlaySoundW playback failed").into())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::job;
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::CloseHandle;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    /// 单进程长命睡眠者（~99s），便于观察 Job 关闭是否连带终止
    fn spawn_sleeper() -> std::process::Child {
        std::process::Command::new("ping.exe")
            .args(["-n", "100", "127.0.0.1"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn sleeper")
    }

    fn wait_exited(child: &mut std::process::Child, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if child.try_wait().expect("try_wait").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    #[test]
    fn closing_kill_on_close_job_terminates_member() {
        // 复现 NSIS 强杀场景：父进程句柄全关时，Job 成员必须被内核连带终止，
        // 否则孤儿进程会锁住 runtime 目录导致重装失败。
        let job = unsafe { job::create_kill_on_close_job() };
        assert!(!job.is_null(), "CreateJobObjectW failed");
        let mut child = spawn_sleeper();
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "sleeper should start alive"
        );
        assert!(
            unsafe { job::assign_pid_to_job(job, child.id()) },
            "AssignProcessToJobObject failed"
        );
        unsafe { CloseHandle(job) };
        let exited = wait_exited(&mut child, Duration::from_secs(10));
        if !exited {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(
            exited,
            "child must be terminated when the job's last handle closes"
        );
    }

    #[test]
    fn assign_pid_to_job_rejects_bad_pid() {
        let job = unsafe { job::create_kill_on_close_job() };
        assert!(!job.is_null(), "CreateJobObjectW failed");
        assert!(!unsafe { job::assign_pid_to_job(job, 0) });
        assert!(!unsafe { job::assign_pid_to_job(job, u32::MAX) });
        assert!(!unsafe { job::assign_pid_to_job(std::ptr::null_mut(), 1234) });
        unsafe { CloseHandle(job) };
    }

    #[test]
    fn register_child_is_best_effort() {
        let p = WindowsPlatform;
        p.register_child(0); // 不存在的 pid：静默忽略，不得 panic
        p.register_child(u32::MAX);
        let mut child = spawn_sleeper();
        p.register_child(child.id());
        let _ = child.kill();
        let _ = child.wait();
    }
}
