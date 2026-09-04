//! 远程会话持久化：开启远程时把会话事实落盘（remote-session.json），
//! 应用重启后据此原地复活——token/域名/代理端口不变，手机端链接继续可用。
//!
//! 语义边界：
//! - token 只在两处轮换：手动"关闭远程"后再开、手动"重置链接"；resume 及
//!   resume 失败回退都沿用旧 token（"除非主动重置，链接不失效"）。
//! - 状态文件含 token（敏感凭据）：只允许写这里，绝不落 events.log。
//! - cloudflared 从数据目录副本（tunnel/cloudflared.exe）常驻运行：不从安装
//!   目录跑（覆盖安装时 NSIS 会按路径清扫 $INSTDIR 且要覆写文件），不挂
//!   KILL_ON_JOB_CLOSE Job（刻意比应用活得久，防孤儿原则的唯一例外）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 跨重启保持的会话事实。tunnel_pid 是收养凭据；cloudflared_exe 用于防
/// PID 复用误判（比对进程镜像路径）与确认副本还在。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub token: String,
    pub proxy_port: u16,
    pub tunnel_pid: u32,
    pub tunnel_url: String,
    pub cloudflared_exe: PathBuf,
}

/// 读状态文件：不存在/损坏/字段缺失一律 None（调用侧按"无会话"回退全新开）
pub fn load(path: &Path) -> Option<SessionState> {
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// tmp+rename 原子写；失败由调用侧记日志（不写日志防 token 外泄到 events.log）
pub fn save(path: &Path, st: &SessionState) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(st).expect("SessionState 序列化不会失败"))?;
    std::fs::rename(&tmp, path)
}

/// 删除状态文件；不存在不算错误（手动关闭远程 = 会话结束）
pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// 隧道常驻副本：缺或尺寸与源不符才复制（正在运行的旧副本不会被覆写——
/// 调用侧保证只在隧道未运行时调用）。返回副本路径 `<dir>/cloudflared.exe`。
pub fn ensure_tunnel_copy(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let target = dir.join("cloudflared.exe");
    let stale = match (std::fs::metadata(src), std::fs::metadata(&target)) {
        (Ok(s), Ok(t)) => s.len() != t.len(),
        (Ok(_), Err(_)) => true,
        (Err(e), _) => return Err(e),
    };
    if stale {
        std::fs::copy(src, &target)?;
    }
    Ok(target)
}

/// 镜像路径比对：大小写不敏感 + 剥 `\\?\` 扩展前缀（QueryFullProcessImageNameW
/// 与手工落盘的路径形态可能不同）
pub fn image_path_matches(recorded: &Path, actual: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let s = p.to_string_lossy().to_lowercase();
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
    }
    norm(recorded) == norm(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_corrupt_tolerance() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("remote-session.json");
        assert!(load(&p).is_none(), "不存在应 None");
        let st = SessionState {
            token: "abc".into(),
            proxy_port: 12345,
            tunnel_pid: 999,
            tunnel_url: "https://a-b-c.trycloudflare.com".into(),
            cloudflared_exe: PathBuf::from("C:\\x\\cloudflared.exe"),
        };
        save(&p, &st).unwrap();
        let got = load(&p).unwrap();
        assert_eq!(got.token, "abc");
        assert_eq!(got.proxy_port, 12345);
        assert_eq!(got.tunnel_pid, 999);
        assert_eq!(got.tunnel_url, "https://a-b-c.trycloudflare.com");
        assert_eq!(got.cloudflared_exe, PathBuf::from("C:\\x\\cloudflared.exe"));
        std::fs::write(&p, b"{not json").unwrap();
        assert!(load(&p).is_none(), "损坏应 None 不 panic");
        clear(&p);
        assert!(!p.exists());
        clear(&p); // 幂等
    }

    #[test]
    fn tunnel_copy_refresh_rules() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.exe");
        std::fs::write(&src, b"v1-content").unwrap();
        let copy = ensure_tunnel_copy(&src, dir.path()).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"v1-content");
        // 同尺寸不刷新（隧道常驻期间旧副本不被打扰的语义根基）
        std::fs::write(&src, b"v2-same-ln").unwrap();
        let copy2 = ensure_tunnel_copy(&src, dir.path()).unwrap();
        assert_eq!(
            std::fs::read(&copy2).unwrap(),
            b"v1-content",
            "同尺寸不刷新"
        );
        std::fs::write(&src, b"v3-longer-content").unwrap();
        let copy3 = ensure_tunnel_copy(&src, dir.path()).unwrap();
        assert_eq!(
            std::fs::read(&copy3).unwrap(),
            b"v3-longer-content",
            "尺寸变则刷新"
        );
    }

    #[test]
    fn image_path_compare_tolerant() {
        assert!(image_path_matches(
            Path::new(r"C:\App\cloudflared.exe"),
            Path::new(r"c:\app\cloudflared.exe")
        ));
        assert!(image_path_matches(
            Path::new(r"C:\App\cloudflared.exe"),
            Path::new(r"\\?\C:\App\cloudflared.exe")
        ));
        assert!(!image_path_matches(
            Path::new(r"C:\App\cloudflared.exe"),
            Path::new(r"C:\App\node.exe")
        ));
    }
}
