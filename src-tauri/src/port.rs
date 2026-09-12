use std::io;
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

/// 记忆端口文件名（壳数据目录，与 events.log / ui-zoom.txt 同级）。
const REMEMBERED_FILE: &str = "dsh-port.txt";
/// 记忆端口的下限：1024 以下留给系统服务，文件写坏了也不去抢。
const MIN_REMEMBERED_PORT: u16 = 1024;
/// 复用端口的探活重试：上一个进程刚走时它的监听套接字可能还在关闭途中
/// （或残留 TCB 尚未回收），短暂重试避免把"马上就能用"的端口误判成被占用。
const PROBE_RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// 复用探活的总预算。只在上次端口不可用时才走满，代价是一次启动延迟；
/// 真的被别人长期占着时 port.rs 会换新端口并在 Ready 后改写记忆（一轮收敛）。
pub const REUSE_GRACE: Duration = Duration::from_secs(2);

/// 让 OS 分配一个空闲端口。返回后到实际使用之间存在小竞态窗口，
/// 调用方需在启动失败时重试另一个端口。
pub fn free_port() -> io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// 端口是否可绑。与 dsh 的监听同语义（Node/libuv 在 Windows 上按 0 旗标绑定，
/// 不设 SO_REUSEADDR），所以探测通过基本等于 dsh 绑得上；仍留竞态窗口，
/// 调用方失败时换端口重试。
fn is_port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// 选一个绑定端口：记忆端口仍空闲就复用它，否则让 OS 分配新端口。
///
/// 复用是**稳定 Web 源站**的前提：dsh 客户端的偏好（"打开方式"选择、会话宽度
/// 等）走浏览器 localStorage，而 localStorage 按 origin（含端口）隔离——每次
/// 启动换随机端口等于每次都是新源站，用户选过的偏好重启即回默认值
/// （上游 `dsh-client-store` 的 persist 只写 localStorage，无服务端副本）。
///
/// `grace` 内反复探活，覆盖"上个进程刚退、端口即将可用"的窗口；仍不可用则回退
/// 随机端口（调用方据此改写记忆，下一轮收敛到新端口）。
pub async fn pick_port(remembered: Option<u16>, grace: Duration) -> io::Result<u16> {
    let Some(preferred) = remembered.filter(|p| *p >= MIN_REMEMBERED_PORT) else {
        return free_port();
    };
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        if is_port_free(preferred) {
            return Ok(preferred);
        }
        if tokio::time::Instant::now() >= deadline {
            return free_port();
        }
        tokio::time::sleep(PROBE_RETRY_INTERVAL).await;
    }
}

/// 读上次真正绑上的端口。缺失 / 内容损坏 / 低于下限 → None（回到随机端口）。
pub fn load_remembered(dir: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(dir.join(REMEMBERED_FILE)).ok()?;
    raw.trim()
        .parse::<u16>()
        .ok()
        .filter(|p| *p >= MIN_REMEMBERED_PORT)
}

/// 记住本次真正绑上的端口（下次启动复用，保住源站上的客户端偏好）。
/// 写失败不致命——只影响下次启动回到随机端口，由调用方落日志。
pub fn remember(dir: &Path, port: u16) -> io::Result<()> {
    std::fs::write(dir.join(REMEMBERED_FILE), format!("{port}\n"))
}

/// 轮询 http://127.0.0.1:port/ 直到拿到任意 HTTP 响应或超时。
pub async fn wait_ready(port: u16, timeout: Duration) -> bool {
    let url = format!("http://127.0.0.1:{port}/");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap_or_default();
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if client.get(&url).send().await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::thread;

    #[test]
    fn free_port_returns_bindable_port() {
        let p = free_port().unwrap();
        assert!(TcpListener::bind(("127.0.0.1", p)).is_ok());
    }

    #[test]
    fn remembered_port_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        // 从未写过 → None
        assert_eq!(load_remembered(dir.path()), None);
        remember(dir.path(), 24974).unwrap();
        assert_eq!(load_remembered(dir.path()), Some(24974));
        // 改写（换新端口后收敛）
        remember(dir.path(), 31000).unwrap();
        assert_eq!(load_remembered(dir.path()), Some(31000));
    }

    #[test]
    fn remembered_port_rejects_garbage_and_reserved() {
        let dir = tempfile::tempdir().unwrap();
        // 内容损坏 / 空 / 越界 → None（回到随机端口，不去抢低端口）
        for bad in ["abc", "", "0", "80", "99999", "-1"] {
            std::fs::write(dir.path().join("dsh-port.txt"), bad).unwrap();
            assert_eq!(load_remembered(dir.path()), None, "内容 {bad:?} 应被拒");
        }
        // 带换行/空格的正常值照收（落盘形态）
        std::fs::write(dir.path().join("dsh-port.txt"), " 24974 \n").unwrap();
        assert_eq!(load_remembered(dir.path()), Some(24974));
    }

    #[tokio::test]
    async fn pick_port_reuses_free_remembered() {
        let free = free_port().unwrap();
        let picked = pick_port(Some(free), REUSE_GRACE).await.unwrap();
        assert_eq!(picked, free);
    }

    #[tokio::test]
    async fn pick_port_falls_back_when_remembered_occupied() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let taken = listener.local_addr().unwrap().port();
        // 占用者活着 → 换随机端口（grace 传 0：不为此在启动路径上白等）
        let picked = pick_port(Some(taken), Duration::ZERO).await.unwrap();
        assert_ne!(picked, taken);
        assert!(TcpListener::bind(("127.0.0.1", picked)).is_ok());
    }

    #[tokio::test]
    async fn pick_port_waits_out_brief_occupancy() {
        // 上个进程刚退：端口短暂不可绑，grace 内应等到它（而不是立刻换端口）
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            drop(listener);
        });
        let picked = pick_port(Some(port), REUSE_GRACE).await.unwrap();
        assert_eq!(picked, port);
    }

    #[tokio::test]
    async fn pick_port_ignores_reserved_remembered() {
        // 低端口（<1024）不信任：即便写坏也不复用
        let picked = pick_port(Some(80), Duration::ZERO).await.unwrap();
        assert!(picked >= MIN_REMEMBERED_PORT);
    }

    fn spawn_mini_http() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut s) = stream {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
                }
            }
        });
        port
    }

    #[tokio::test]
    async fn wait_ready_true_when_http_responds() {
        let port = spawn_mini_http();
        assert!(wait_ready(port, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn wait_ready_false_on_timeout() {
        // 绑定但不 accept：端口被占用、TCP 能连上但没有 HTTP 响应，确定性触发超时
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!wait_ready(port, Duration::from_millis(600)).await);
    }
}
