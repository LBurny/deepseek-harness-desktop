//! 壳事件日志设施：events.log 统一追加、时间戳、截断。

use std::path::PathBuf;

use crate::dsh::process::ProcessEvent;

/// 该行是否已自带时间戳前缀。壳侧不少行自带 `[HH:MM:SS.mmm]`（bring_to_front /
/// 播放诊断等），cloudflared 透传行自带 RFC3339 UTC 前缀——这些不再二次加盖；
/// 其余行（Starting / [dshdesktop] … / dsh stdout 透传等）由 append_debug_line
/// 统一补本地时间戳，启动时序排查全靠它。
pub(crate) fn needs_local_stamp(line: &str) -> bool {
    let b = line.as_bytes();
    // [HH:MM:SS.mmm]
    if b.len() >= 14 && b[0] == b'[' && b[13] == b']' {
        let inner = &line[1..13];
        let digit_at = |i: usize| inner.as_bytes()[i].is_ascii_digit();
        if digit_at(0)
            && digit_at(1)
            && inner.as_bytes()[2] == b':'
            && digit_at(3)
            && digit_at(4)
            && inner.as_bytes()[5] == b':'
            && digit_at(6)
            && digit_at(7)
            && inner.as_bytes()[8] == b'.'
            && digit_at(9)
            && digit_at(10)
            && digit_at(11)
        {
            return false;
        }
    }
    // RFC3339：YYYY-MM-DDTHH:MM:SS（cloudflared UTC 行）
    if b.len() >= 20
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
    {
        return false;
    }
    true
}

/// 追加一行到调试日志；超过 1MB 时截断重来（只用于现场诊断，不求完备）。
/// 无时间戳的行统一补 `[HH:MM:SS.mmm]` 本地时间前缀（needs_local_stamp 判定）。
pub(crate) fn append_debug_line(path: &std::path::Path, line: &str) {
    use std::io::Write;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > 1024 * 1024 {
            let _ = std::fs::remove_file(path);
        }
    }
    let stamped;
    let line = if needs_local_stamp(line) {
        stamped = format!("[{}] {line}", local_stamp());
        &stamped
    } else {
        line
    };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// 本地时间戳 HH:MM:SS.mmm：声音链路诊断用——用户在界面上"点第几下没声音"
/// 需要与日志行逐条对齐，无时间戳无法对应（Windows GetLocalTime，其它平台退
/// 化为 UNIX 秒）。
pub(crate) fn local_stamp() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st = SYSTEMTIME {
            wYear: 0,
            wMonth: 0,
            wDayOfWeek: 0,
            wDay: 0,
            wHour: 0,
            wMinute: 0,
            wSecond: 0,
            wMilliseconds: 0,
        };
        unsafe { GetLocalTime(&mut st) };
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        )
    }
    #[cfg(not(windows))]
    {
        let s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("unix:{s}")
    }
}

pub(crate) fn append_debug_log(path: &PathBuf, event: &ProcessEvent) {
    let line = match event {
        ProcessEvent::StateChanged(s) => format!("{s:?}"),
        ProcessEvent::Log(l) => l.clone(),
    };
    append_debug_line(path, &line);
}

#[cfg(test)]
mod tests {
    #[test]
    fn needs_local_stamp_detects_existing_stamps() {
        // 壳侧自带本地时间戳的行（bring_to_front / 播放诊断）不重复盖
        assert!(!crate::logging::needs_local_stamp("[19:13:54.425] bring_to_front: begin hwnd=0x1"));
        // cloudflared 透传行自带 RFC3339 UTC 前缀，不重复盖
        assert!(!crate::logging::needs_local_stamp("2026-09-04T11:13:44Z INF Thank you"));
        assert!(!crate::logging::needs_local_stamp("2026-09-04T11:13:44.123Z INF x"));
    }

    #[test]
    fn needs_local_stamp_marks_bare_lines() {
        // 方括号但内容不是时间戳
        assert!(crate::logging::needs_local_stamp("[dshdesktop] starting dsh web --port 31817"));
        // 生命周期状态行 / dsh stdout 透传（无时间戳）
        assert!(crate::logging::needs_local_stamp("Starting"));
        assert!(crate::logging::needs_local_stamp("Ready { port: 31817 }"));
        assert!(crate::logging::needs_local_stamp("Context7 Documentation MCP Server v4.0.5 running on stdio"));
        assert!(crate::logging::needs_local_stamp("dsh web: http://127.0.0.1:31817/?token=<redacted>"));
    }

    #[test]
    fn append_debug_line_stamps_unstamped_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.log");
        crate::logging::append_debug_line(&path, "hello world");
        let s = std::fs::read_to_string(&path).unwrap();
        assert!(s.starts_with('['), "无时间戳的行应补 [HH:MM:SS.mmm] 前缀，实际：{s}");
        assert!(s.ends_with("] hello world\n"), "实际：{s}");
    }

    #[test]
    fn append_debug_line_preserves_existing_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.log");
        crate::logging::append_debug_line(&path, "[19:13:54.425] begin");
        crate::logging::append_debug_line(&path, "2026-09-04T11:13:44Z INF cloudflared");
        let s = std::fs::read_to_string(&path).unwrap();
        assert_eq!(s, "[19:13:54.425] begin\n2026-09-04T11:13:44Z INF cloudflared\n", "自带时间戳的行不得二次加盖，实际：{s}");
    }
}
