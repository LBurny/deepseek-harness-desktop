//! 主窗口下载处理。
//!
//! dsh Web UI 的"Session log"导出走浏览器下载（blob + anchor download）。
//! 壳不显式接管时，wry 的默认 download_started_handler 静默放行并
//! SetHandled(true)：WebView2 自己的下载 UI 被抑制，文件无声落盘（甚至可能
//! 落到 runtime 默认路径），用户完全无从得知去向。这里显式接管：目标统一改到
//! 系统下载目录并去重防覆盖，完成/失败都弹 toast 并记 events.log。

use crate::{append_debug_line, i18n};
use std::path::{Path, PathBuf};
use tauri::webview::DownloadEvent;
use tauri::{Manager, Webview};

/// 生成挂到主窗口 WebviewWindowBuilder 的下载处理器；log_path 为 events.log。
pub fn handler(
    log_path: PathBuf,
) -> impl Fn(Webview, DownloadEvent<'_>) -> bool + Send + Sync + 'static {
    move |webview, event| {
        match event {
            DownloadEvent::Requested { url, destination } => {
                let name = sanitize_name(
                    destination
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                if let Some(dir) = dirs::download_dir() {
                    *destination = unique_path(&dir, &name);
                }
                append_debug_line(
                    &log_path,
                    &format!("Download: requested {url} -> {}", destination.display()),
                );
                true
            }
            DownloadEvent::Finished { url, path, success } => {
                append_debug_line(
                    &log_path,
                    &format!("Download: finished {url} success={success} path={path:?}"),
                );
                let (title, body) = if success {
                    let p = path.as_deref().map(|p| p.display().to_string()).unwrap_or_default();
                    let name = path
                        .as_deref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| url.path().to_string());
                    (
                        i18n::pick("下载完成", "Download complete"),
                        i18n::pick(
                            format!("{name}\n已保存到：{p}"),
                            format!("{name}\nSaved to: {p}"),
                        ),
                    )
                } else {
                    (
                        i18n::pick("下载失败", "Download failed"),
                        i18n::pick(
                            format!("未能保存文件：{url}"),
                            format!("Could not save file: {url}"),
                        ),
                    )
                };
                let _ = crate::notify::toast::show(
                    webview.app_handle(),
                    &title,
                    &body,
                    crate::notify::toast::ToastSound::Silent,
                    None,
                );
                true
            }
            _ => true,
        }
    }
}

/// 文件名卫生：空名兜底，Windows 非法字符替换为下划线。
fn sanitize_name(name: String) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if cleaned.is_empty() {
        "download.bin".to_string()
    } else {
        cleaned
    }
}

/// 目标已存在时追加 " (n)" 序号，避免静默覆盖上一次导出。
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for i in 1..1000u32 {
        let c = dir.join(format!("{stem} ({i}){ext}"));
        if !c.exists() {
            return c;
        }
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_illegal_chars() {
        assert_eq!(sanitize_name("a<b>:c\".zip".into()), "a_b__c_.zip");
        assert_eq!(sanitize_name("session log.zip".into()), "session log.zip");
        assert_eq!(sanitize_name("".into()), "download.bin");
        assert_eq!(sanitize_name("  ".into()), "download.bin");
        assert_eq!(sanitize_name("会话-导出.zip".into()), "会话-导出.zip");
    }

    #[test]
    fn unique_path_plain_when_free() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_path(dir.path(), "s.zip"),
            dir.path().join("s.zip")
        );
    }

    #[test]
    fn unique_path_dedupes_with_counter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s.zip"), b"a").unwrap();
        std::fs::write(dir.path().join("s (1).zip"), b"b").unwrap();
        assert_eq!(
            unique_path(dir.path(), "s.zip"),
            dir.path().join("s (2).zip")
        );
    }

    #[test]
    fn unique_path_handles_extensionless() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("log"), b"a").unwrap();
        assert_eq!(unique_path(dir.path(), "log"), dir.path().join("log (1)"));
    }
}
