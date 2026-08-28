use super::Platform;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Media::Audio::{
    HWAVEOUT, WAVEFORMATEX, WAVEOUTCAPSW, WAVERR_STILLPLAYING,
};
use windows_sys::Win32::Media::Audio::{CALLBACK_EVENT, WAVE_MAPPER};

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

    fn play_sound_file(&self, path: &Path, diag: Option<super::SoundDiag>) -> Result<(), String> {
        // 同步前置校验：文件缺失立即 Err，调用侧可立刻降级（toast 系统默认音）
        if !path.is_file() {
            return Err(crate::i18n::pick(
                format!("音效文件不存在: {}", path.display()),
                format!("Sound file not found: {}", path.display()),
            ));
        }
        // 播放放到专用线程上自管 waveOut（调用侧立即返回，不阻塞预览/通知）。
        // 背景：PlaySoundW 四轮翻车史——SND_NOSTOP 忙时放弃（0.3.x）、SND_ASYNC
        // 缓冲悬垂（0.4.2）、SND_ASYNC 工作线程首播静默吞错（0.4.2 修后仍复现）、
        // SND_SYNC 下 winmm 缓存的设备句柄在机器 B 上失效（首次有声、后续全
        // 静默，日志却全 ok 播满时长——0.4.5 实测）。自管 waveOut 每次播放新开
        // 设备句柄（WAVE_MAPPER 取当前默认设备），彻底绕开 PlaySound 的隐藏
        // 状态机；打开/写入/收尾每一步都有真实 MMSYSERR 错误码可落日志，播放
        // 到哪个设备也记进日志，"没声音"从此有因可查。
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let line = play_wave_with_retry(&path);
            if let Some(diag) = &diag {
                diag(line);
            }
        });
        Ok(())
    }
}

#[cfg(not(windows))]
compile_error!("windows.rs 只在 Windows 编译");

/// 当前正在播放的会话：新播放 waveOutReset 旧的（打断语义，与 PlaySound 默认
/// 行为一致）。Arc 让旧播放线程安全判断"仍是自己"再注销，避免误删新会话。
/// HWAVEOUT 是 winmm 内部句柄，其 API 自带跨线程同步（waveOutReset 即在别的
/// 线程调用），跨线程持有安全。
struct PlaySession {
    handle: HWAVEOUT,
}
unsafe impl Send for PlaySession {}
unsafe impl Sync for PlaySession {}
static CURRENT_PLAY: std::sync::Mutex<Option<std::sync::Arc<PlaySession>>> =
    std::sync::Mutex::new(None);

/// 解析 PCM WAV 为 (格式, 采样数据)。本壳 17 个内置音效均为 PCM 16-bit 单声道
/// 44.1kHz；其余编码（IEEE float、ADPCM 等）给真实错误而不是静默。
/// chunk 遍历容忍非 4 对齐（奇数长度补位）与截断的 data 块（按实际文件长度取）。
fn parse_wav(path: &Path) -> Result<(WAVEFORMATEX, Vec<u8>), String> {
    let raw = std::fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    if raw.len() < 12 || &raw[0..4] != b"RIFF" || &raw[8..12] != b"WAVE" {
        return Err("不是 RIFF/WAVE 文件".into());
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None;
    let mut data: Option<Vec<u8>> = None;
    while pos + 8 <= raw.len() {
        let id = &raw[pos..pos + 4];
        let sz = u32::from_le_bytes(raw[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        match id {
            b"fmt " if sz >= 16 && body + 16 <= raw.len() => {
                let b = &raw[body..body + 16];
                fmt = Some((
                    u16::from_le_bytes(b[0..2].try_into().unwrap()),
                    u16::from_le_bytes(b[2..4].try_into().unwrap()),
                    u32::from_le_bytes(b[4..8].try_into().unwrap()),
                    u16::from_le_bytes(b[12..14].try_into().unwrap()),
                    u16::from_le_bytes(b[14..16].try_into().unwrap()),
                ));
            }
            b"data" => {
                let start = body.min(raw.len());
                let end = body.saturating_add(sz).min(raw.len());
                data = Some(raw[start..end].to_vec());
            }
            _ => {}
        }
        pos = body.saturating_add(sz).saturating_add(sz & 1);
    }
    let (tag, channels, rate, align, bits) = fmt.ok_or("WAV 缺少 fmt 块")?;
    if tag != 1 {
        return Err(format!("非 PCM 编码（format tag {tag}）"));
    }
    let data = data.filter(|d| !d.is_empty()).ok_or("WAV 缺少 data 块")?;
    Ok((
        WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * align as u32,
            nBlockAlign: align,
            wBitsPerSample: bits,
            cbSize: 0,
        },
        data,
    ))
}

/// 单次同步播放：成功返回实际播放到的设备名，失败返回真实错误。每次
/// waveOutOpen 新开设备句柄（WAVE_MAPPER 取当前默认设备）、播完即关——
/// 绕开 PlaySound 的进程级设备缓存（机器 B：缓存句柄失效 = 首次有声、
/// 后续"播满时长却无声"）。
fn play_wave_once(path: &Path) -> Result<String, String> {
    use windows_sys::Win32::Media::Audio::{
        waveOutClose, waveOutGetDevCapsW, waveOutGetID, waveOutOpen, waveOutPrepareHeader,
        waveOutReset, waveOutUnprepareHeader, waveOutWrite, WAVEHDR,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    let (fmt, mut data) = parse_wav(path)?;
    // 打断上一次播放（若仍在响）；旧线程的等待会被 WOM_DONE 唤醒并自行收尾
    if let Some(prev) = CURRENT_PLAY.lock().unwrap().take() {
        unsafe { waveOutReset(prev.handle) };
    }
    unsafe {
        let done = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
        if done.is_null() {
            return Err("CreateEventW 失败".into());
        }
        let mut handle: HWAVEOUT = std::ptr::null_mut();
        let open = waveOutOpen(
            &mut handle,
            WAVE_MAPPER,
            &fmt,
            done as usize,
            0,
            CALLBACK_EVENT,
        );
        if open != 0 {
            CloseHandle(done);
            return Err(format!("waveOutOpen 失败 (MMSYSERR {open})"));
        }
        // 设备名此刻查询（播到哪个设备是间歇无声的关键线索），句柄关闭后失效
        let device = {
            let mut id: u32 = u32::MAX;
            let mut name = "unknown device".to_string();
            if waveOutGetID(handle, &mut id) == 0 {
                let mut caps: WAVEOUTCAPSW = std::mem::zeroed();
            if waveOutGetDevCapsW(
                id as usize,
                &mut caps,
                std::mem::size_of::<WAVEOUTCAPSW>() as u32,
            ) == 0
            {
                // WAVEOUTCAPSW 是 packed(1)，字段引用未对齐——用 read_unaligned 取
                let pname =
                    std::ptr::addr_of!(caps.szPname).read_unaligned();
                name = pname
                    .iter()
                    .take_while(|&&c| c != 0)
                    .map(|&c| char::from_u32(c as u32).unwrap_or('\u{FFFD}'))
                    .collect();
            }
            }
            name
        };
        let session = std::sync::Arc::new(PlaySession { handle });
        *CURRENT_PLAY.lock().unwrap() = Some(session.clone());
        let mut header = WAVEHDR {
            lpData: data.as_mut_ptr(),
            dwBufferLength: data.len() as u32,
            dwBytesRecorded: 0,
            dwUser: 0,
            dwFlags: 0,
            dwLoops: 0,
            lpNext: std::ptr::null_mut(),
            reserved: 0,
        };
        let hdr_len = std::mem::size_of::<WAVEHDR>() as u32;
        // 失败收尾：停掉播放、关句柄，返回真实错误码
        macro_rules! fail {
            ($step:expr, $code:expr) => {{
                waveOutReset(handle);
                waveOutClose(handle);
                CloseHandle(done);
                CURRENT_PLAY.lock().unwrap().take_if(|s| std::sync::Arc::ptr_eq(s, &session));
                return Err(format!("{} 失败 (MMSYSERR {})", $step, $code));
            }};
        }
        let r = waveOutPrepareHeader(handle, &mut header, hdr_len);
        if r != 0 {
            fail!("waveOutPrepareHeader", r);
        }
        let r = waveOutWrite(handle, &mut header, hdr_len);
        if r != 0 {
            fail!("waveOutWrite", r);
        }
        // 等播完（WOM_DONE 置事件）；上限 = 音频时长 + 3s 余量，防设备僵死挂线程
        let wait_ms = (data.len() as f64 / fmt.nAvgBytesPerSec.max(1) as f64 * 1000.0) as u32
            + 3000;
        WaitForSingleObject(done, wait_ms);
        let mut r = waveOutUnprepareHeader(handle, &mut header, hdr_len);
        let mut spins = 0;
        while r == WAVERR_STILLPLAYING && spins < 50 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            r = waveOutUnprepareHeader(handle, &mut header, hdr_len);
            spins += 1;
        }
        waveOutReset(handle);
        waveOutClose(handle);
        CloseHandle(done);
        CURRENT_PLAY
            .lock()
            .unwrap()
            .take_if(|s| std::sync::Arc::ptr_eq(s, &session));
        Ok(device)
    }
}

/// 播放 + 一次重试（自愈设备瞬时不可用），返回落 events.log 的诊断行
/// （时间戳 + 路径 + 耗时 + 结果 + 实际播放设备）。
fn play_wave_with_retry(path: &Path) -> String {
    let t0 = std::time::Instant::now();
    let mut last_err = String::from("未播放");
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        match play_wave_once(path) {
            Ok(device) => {
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                return format!(
                    "[{}] play sound: {} ok ({ms:.0}ms, device={device})",
                    crate::local_stamp(),
                    path.display()
                );
            }
            Err(e) => last_err = e,
        }
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    format!(
        "[{}] play sound: {} failed: {last_err} ({ms:.0}ms，重试 1 次仍失败)",
        crate::local_stamp(),
        path.display()
    )
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
