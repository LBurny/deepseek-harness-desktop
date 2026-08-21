//! 手动目验辅助：起一个新代码的代理，指向本机正在运行的真实 dsh 与真实
//! dsh-home，打印带 token 的访问链接，挂 10 分钟供浏览器/Playwright 目验
//! "项目"标签。用法：
//!   set DSH_PORT=24203 && cargo run --example proxy_preview
//! （DSH_HOME 默认 %LOCALAPPDATA%\DSHDesktop\dsh-home，可用 DSH_HOME 环境变量覆盖）

use dshdesktop_lib::remote::generate_token;
use dshdesktop_lib::remote::proxy::spawn_proxy;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::watch;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let dsh_port: u16 = std::env::var("DSH_PORT")
        .expect("set DSH_PORT=<运行中 dsh 的端口>")
        .parse()
        .unwrap();
    let home = std::env::var("DSH_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("LOCALAPPDATA").unwrap())
                .join("DSHDesktop")
                .join("dsh-home")
        });
    let token: std::sync::Arc<str> = generate_token().into();
    let (_tx, rx) = watch::channel(Some(dsh_port));
    let h = spawn_proxy(token.clone(), rx, home, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    println!(
        "PREVIEW_URL=http://127.0.0.1:{}/?token={}",
        h.port, token
    );
    // 挂住供目验；超时自动退出
    tokio::time::sleep(Duration::from_secs(600)).await;
    h.shutdown().await;
}
