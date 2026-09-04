use crate::process::DshProcess;
use crate::runtime::RuntimePaths;
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tauri::Url;

/// 诊断面板回填的行数
pub(crate) const LOG_TAIL_LINES: usize = 500;
/// 读尾部的单次上限：诊断日志行可能很长（dsh-log 的 JSON 块），512KB 足够
/// 覆盖 500 行，读不到更多就放弃最老的
const LOG_TAIL_BYTES: u64 = 512 * 1024;

/// 应用级共享状态，注册为 Tauri managed state。
pub struct SharedState {
    pub process: DshProcess,
    pub runtime: RuntimePaths,
    pub version: String,
    pub home_url: Url,
    /// 是否首次启动（ensure_runtime 之前 dsh-home 不存在）；供启动画面决定是否显示进度条
    pub first_launch: bool,
}

/// events.log 的最后 n 行（诊断面板回填数据源）。events.log 是壳侧诊断 +
/// dsh 进程输出的统一持久层（超 1MB 整体清空，只留最近），读它即得到跨会话
/// 的近期历史；面板开着期间的增量走 dsh-log 实时事件。文件不存在/不可读 → 空。
pub fn read_log_tail(path: &Path, n: usize) -> Vec<String> {
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(LOG_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let mut lines: Vec<String> = String::from_utf8_lossy(&buf)
        .lines()
        .map(str::to_string)
        .collect();
    // 从中间起步读的块首行大概率被截断（文件比块大时）；是完整文件则保留首行
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    if lines.len() > n {
        let skip = lines.len() - n;
        lines.drain(0..skip);
    }
    lines
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusDto {
    pub state: String,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub version: String,
}

/// 一次 dsh 启动的耗时分解（诊断面板"上次启动"行）。数据源是 process.rs 在
/// Ready 前落的 `[dshdesktop] ready: port=N total=Xs http=Ys token=Zs` 日志行。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootTimingDto {
    pub port: u16,
    pub total_s: f64,
    pub http_s: f64,
    pub token_s: f64,
    /// 行首 [HH:MM:SS.mmm] 本地时间戳（events.log 统一盖章后有；实时事件流没有）
    pub at: Option<String>,
}

/// 解析启动耗时分解行；格式锚定 process::format_boot_timing。非分解行/残缺行 → None。
pub fn parse_boot_timing(line: &str) -> Option<BootTimingDto> {
    // 行首时间戳（events.log 里的形态）提取出来单独展示；实时事件流没有这层
    let at = if !crate::needs_local_stamp(line) && line.starts_with('[') {
        line[1..].find(']').map(|i| line[1..1 + i].to_string())
    } else {
        None
    };
    let marker = "[dshdesktop] ready: port=";
    let idx = line.find(marker)?;
    let mut it = line[idx + marker.len()..].split_whitespace();
    let port: u16 = it.next()?.parse().ok()?;
    let take_s = |tok: Option<&str>, key: &str| -> Option<f64> {
        tok?.strip_prefix(key)?.strip_suffix('s')?.parse().ok()
    };
    let total_s = take_s(it.next(), "total=")?;
    let http_s = take_s(it.next(), "http=")?;
    let token_s = take_s(it.next(), "token=")?;
    Some(BootTimingDto {
        port,
        total_s,
        http_s,
        token_s,
        at,
    })
}

/// 日志尾部里最近一次启动的耗时分解（倒序扫描，Ready 分解行每轮启动一条）。
pub fn last_boot_timing(lines: &[String]) -> Option<BootTimingDto> {
    lines.iter().rev().find_map(|l| parse_boot_timing(l))
}

/// 启动引导信息：无论运行时是否就绪都会注册，
/// 供前端启动画面主动查询（事件可能早于前端 listen 而丢失）。
#[derive(Default)]
pub struct BootstrapInfo(pub std::sync::Mutex<Option<String>>);

impl BootstrapInfo {
    pub fn set_error(&self, msg: String) {
        *self.0.lock().unwrap() = Some(msg);
    }
    pub fn error(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_boot_timing_parses_stamped_ready_line() {
        // events.log 里的形态：统一时间戳前缀 + process.rs 的分解行
        let dto = parse_boot_timing(
            "[19:15:28.123] [dshdesktop] ready: port=31817 total=12.34s http=1.23s token=11.11s",
        )
        .expect("标准分解行必须可解析");
        assert_eq!(dto.port, 31817);
        assert!((dto.total_s - 12.34).abs() < 1e-9, "实际 {}", dto.total_s);
        assert!((dto.http_s - 1.23).abs() < 1e-9, "实际 {}", dto.http_s);
        assert!((dto.token_s - 11.11).abs() < 1e-9, "实际 {}", dto.token_s);
        assert_eq!(dto.at.as_deref(), Some("19:15:28.123"), "行首时间戳须提取");
    }

    #[test]
    fn parse_boot_timing_accepts_unstamped_line() {
        // dsh-log 实时事件（未经 append_debug_line 盖章）也应可解析
        let dto = parse_boot_timing("[dshdesktop] ready: port=64439 total=2.50s http=0.80s token=1.70s")
            .expect("无时间戳前缀的分解行必须可解析");
        assert_eq!(dto.port, 64439);
        assert!(dto.at.is_none());
    }

    #[test]
    fn parse_boot_timing_ignores_unrelated_lines() {
        assert!(parse_boot_timing("[dshdesktop] starting dsh web --port 31817").is_none());
        assert!(parse_boot_timing("Ready { port: 31817 }").is_none());
        assert!(parse_boot_timing("[19:15:28.123] [dshdesktop] dsh exited: ExitStatus(0)").is_none());
        // 缺字段/坏数值不得 panic，一律 None
        assert!(parse_boot_timing("[dshdesktop] ready: port=1 total=2.0s").is_none());
        assert!(parse_boot_timing("[dshdesktop] ready: port=abc total=2.0s http=1.0s token=1.0s").is_none());
    }

    #[test]
    fn last_boot_timing_picks_most_recent() {
        let lines = vec![
            "[dshdesktop] ready: port=1 total=1.00s http=0.50s token=0.50s".to_string(),
            "[dshdesktop] starting dsh web --port 2".to_string(),
            "[dshdesktop] ready: port=2 total=3.00s http=1.00s token=2.00s".to_string(),
        ];
        let dto = last_boot_timing(&lines).expect("应取到最近一次");
        assert_eq!(dto.port, 2, "必须取最后一次 Ready 的分解行");
        assert!(last_boot_timing(&[]).is_none());
        assert!(last_boot_timing(&["Starting".to_string()]).is_none());
    }

    #[test]
    fn read_log_tail_returns_last_n_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.log");
        std::fs::write(&path, (0..600).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")).unwrap();
        let tail = read_log_tail(&path, LOG_TAIL_LINES);
        assert_eq!(tail.len(), LOG_TAIL_LINES);
        assert_eq!(tail[0], "line 100");
        assert_eq!(tail[LOG_TAIL_LINES - 1], "line 599");
    }

    #[test]
    fn read_log_tail_handles_missing_and_small_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_log_tail(&dir.path().join("missing.log"), 10).is_empty());
        let small = dir.path().join("small.log");
        std::fs::write(&small, "a\nb\nc").unwrap();
        let tail = read_log_tail(&small, 10);
        assert_eq!(tail, vec!["a", "b", "c"]);
    }

    #[test]
    fn read_log_tail_drops_partial_first_line_at_chunk_boundary() {
        // 尾块从长行中间起步：首条必是被截断的残行，必须剔除而非把乱码当数据
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.log");
        let mut content = "x".repeat(600_000);
        content.push('\n');
        content.push_str(&(1..=502).map(|i| format!("L{i}")).collect::<Vec<_>>().join("\n"));
        std::fs::write(&path, content).unwrap();
        let tail = read_log_tail(&path, LOG_TAIL_LINES);
        assert_eq!(tail.len(), LOG_TAIL_LINES);
        assert!(tail.iter().all(|l| l.starts_with('L')), "不得含残行：{:?}", &tail[0..2]);
        assert_eq!(tail[0], "L3", "实际首条：{:?}", tail[0]);
        assert_eq!(tail[LOG_TAIL_LINES - 1], "L502");
    }

    #[test]
    fn append_debug_line_truncates_beyond_1mb() {
        // events.log 超 1MB 整体清空重写：持久层只留"最近"的数据，不无限堆积
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.log");
        std::fs::write(&path, vec![b'x'; 1024 * 1024 + 4096]).unwrap();
        crate::append_debug_line(&path, "fresh line");
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() < 1024 * 1024, "截断后应远小于 1MB，实际 {}", meta.len());
        let s = std::fs::read_to_string(&path).unwrap();
        assert!(s.ends_with("] fresh line\n"), "旧内容应被清空、只留新行（带统一时间戳），实际：{s}");
    }
}
