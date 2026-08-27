use crate::process::DshProcess;
use crate::runtime::RuntimePaths;
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tauri::Url;

const LOG_TAIL_LINES: usize = 500;
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

/// 启动引导信息：无论运行时是否就绪都会注册，
/// 供前端启动画面主动查询（事件可能早于前端 listen 而丢失）。
#[derive(Default)]
pub struct BootstrapInfo(pub std::sync::Mutex<Option<String>>);

impl BootstrapInfo {
    pub fn set_error(&self, msg: String) {
        *self.0.lock().unwrap() = Some(msg.clone());
    }
    pub fn error(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
