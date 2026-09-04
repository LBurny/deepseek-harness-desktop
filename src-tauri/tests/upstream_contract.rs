//! 真实运行时上游契约探测：验证 src/upstream.rs 的事实对当前捆绑的 dsh 仍成立。
//! 跟版门禁——fetch-runtime 抓新版后跑 cargo test，本套件红 = 上游变了，
//! 按失败输出的指引改 src/upstream.rs（必要时动对应消费模块）。
//!
//! 运行时定位：DSHDESKTOP_RUNTIME_DIR → <repo>/src-tauri/runtime/windows-x64。
//! 都没有则整套件 skip。CI 的 fetch-runtime 在 cargo test 之前，故 CI 一定真跑。

use dshdesktop_lib::presets::{self, SignatureState};
use dshdesktop_lib::upstream;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DSH_READY_TIMEOUT: Duration = Duration::from_secs(60);
const FRAME_OBSERVE_WINDOW: Duration = Duration::from_secs(8);

/// 漂移收集器：跑完所有探测项统一报告（跟版时要一次看全，而不是修一个发现下一个）。
#[derive(Default)]
struct Checker {
    failures: Vec<String>,
}

impl Checker {
    fn check(&mut self, name: &str, ok: bool, detail: impl Display, advice: &str) {
        if ok {
            eprintln!("[ok] {name}: {detail}");
        } else {
            let msg = format!("[DRIFT] {name}: {detail} → {advice}");
            eprintln!("{msg}");
            self.failures.push(msg);
        }
    }
}

fn runtime_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DSHDESKTOP_RUNTIME_DIR") {
        let d = PathBuf::from(d);
        if d.join("node.exe").is_file() {
            return Some(d);
        }
    }
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("windows-x64");
    bundled.join("node.exe").is_file().then_some(bundled)
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// 递归找首个含 needle 的文件。only_name 限定文件名时只读同名文件（大目录下的
/// 性能关键）；超过 max_file 字节的文件跳过。
fn tree_find(
    dir: &Path,
    needle: &[u8],
    only_name: Option<&str>,
    max_file: u64,
    depth: usize,
) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let mut dirs = Vec::new();
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            dirs.push(p);
            continue;
        }
        if let Some(name) = only_name {
            if p.file_name().and_then(|n| n.to_str()) != Some(name) {
                continue;
            }
        }
        let Ok(meta) = p.metadata() else { continue };
        if meta.len() > max_file {
            continue;
        }
        let Ok(content) = fs::read(&p) else { continue };
        if contains_subslice(&content, needle) {
            return Some(p);
        }
    }
    dirs.into_iter()
        .find_map(|d| tree_find(&d, needle, only_name, max_file, depth - 1))
}

struct Dsh {
    child: Child,
    port: u16,
    home: tempfile::TempDir,
    client: reqwest::Client,
    /// stdout 就绪行解析出的 launch token（0.1.2 BrowserAuth：无它 GET / 恒 401）
    token: String,
    /// token 交换来的会话 cookie（"dsh-auth-…=v1.…"），后续 HTTP/WS 请求都要带
    cookie: String,
}

impl Dsh {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }
}

impl Drop for Dsh {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn spawn_dsh(rt: &Path) -> Result<Dsh, String> {
    let home = tempfile::tempdir().map_err(|e| e.to_string())?;
    let port = dshdesktop_lib::port::free_port().map_err(|e| e.to_string())?;
    let mut child = Command::new(rt.join("node.exe"))
        .arg(upstream::dsh_bin(rt))
        .arg(upstream::DSH_WEB_SUBCOMMAND)
        .arg(upstream::DSH_PORT_FLAG)
        .arg(port.to_string())
        // 与 process.rs 的 spawn 形一致：抑制 dsh 默认弹系统浏览器
        //（dsh-web-app rc.8 起 openBrowser 默认 true，不探它契约套件每跑一次弹一次浏览器）
        .arg(upstream::DSH_NO_OPEN_FLAG)
        .env("DSH_HOME", home.path())
        .current_dir(home.path())
        // 0.1.2 起 stdout 是 launch token 的唯一来源，必须 piped 持续 pump
        //（就绪行晚于 HTTP 绑定——Loader 树装配完才打印，先探测到的是 401）
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let stdout = child.stdout.take().ok_or("stdout 应已 piped")?;
    let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if line_tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    // 系统代理（Clash 等）会劫持 127.0.0.1——回环请求必须 no_proxy；
    // 303 不跟随（token 交换要亲手校验状态码与 Set-Cookie）
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    // 就绪判定 = 拿到 token → 换到 cookie → 带 cookie GET / 得 200。
    // 探针复用生产解析器/交换器（dsh_session）：壳用同一条代码路径，漂移一并红。
    let deadline = Instant::now() + DSH_READY_TIMEOUT;
    let mut token: Option<String> = None;
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    while Instant::now() < deadline {
        while let Ok(l) = line_rx.try_recv() {
            if token.is_none() {
                if let Some((p, t)) = dshdesktop_lib::dsh_session::parse_ready_line(&l) {
                    if p != port {
                        eprintln!("[note] 就绪行端口 {p} 与已选端口 {port} 不符，沿用已选");
                    }
                    token = Some(t);
                }
            }
            tail.push_back(l);
            if tail.len() > 50 {
                tail.pop_front();
            }
        }
        if let Some(t) = &token {
            if let Ok(cookie) = dshdesktop_lib::dsh_session::exchange_cookie(port, t).await {
                let authed = client
                    .get(format!("http://127.0.0.1:{port}/"))
                    .header(reqwest::header::COOKIE, &cookie)
                    .send()
                    .await;
                if matches!(&authed, Ok(r) if r.status().is_success()) {
                    let dsh = Dsh {
                        child,
                        port,
                        home,
                        client,
                        token: t.clone(),
                        cookie,
                    };
                    return Ok(dsh);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!(
        "dsh web {DSH_READY_TIMEOUT:?} 未就绪（token={}；stdout 尾：{}）",
        token.is_some(),
        tail.into_iter().collect::<Vec<_>>().join(" | ").chars().take(300).collect::<String>()
    ))
}

/// 入口形态探测（不需起 dsh）；返回 dsh 版本号供报告用。
fn probe_entry(rt: &Path, c: &mut Checker) -> String {
    let node = rt.join("node.exe");
    c.check(
        "node.exe 存在",
        node.is_file(),
        node.display(),
        "运行时布局变了：查 fetch-runtime.ps1",
    );
    let bin = upstream::dsh_bin(rt);
    c.check(
        "bin.js 入口存在",
        bin.is_file(),
        bin.display(),
        "改 upstream::DSH_PKG_SEGMENTS/DSH_BIN_SEGMENTS（影响 runtime.rs）",
    );
    let help = Command::new(&node).arg(&bin).arg("--help").output();
    c.check(
        "bin.js --help 可执行",
        matches!(&help, Ok(o) if o.status.success()),
        format!("{help:?}").chars().take(120).collect::<String>(),
        "入口/命令形变了：查 DSH_BIN_SEGMENTS 与 DSH_WEB_SUBCOMMAND（影响 process.rs）",
    );

    let pkg_path = upstream::dsh_pkg_dir(rt).join("package.json");
    let pkg: Option<serde_json::Value> = fs::read_to_string(&pkg_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let version = pkg
        .as_ref()
        .and_then(|p| p["version"].as_str())
        .unwrap_or("?")
        .to_string();
    let recorded = fs::read_to_string(rt.join("RUNTIME_VERSIONS.txt")).unwrap_or_default();
    let recorded_dsh = recorded
        .lines()
        .find_map(|l| l.strip_prefix("dsh "))
        .unwrap_or("?");
    c.check(
        "package.json 版本 == RUNTIME_VERSIONS.txt",
        version == recorded_dsh,
        format!("package.json={version}, txt={recorded_dsh}"),
        "重跑 fetch-runtime.ps1（版本记录未同步）",
    );
    // engines.node：发布 tarball 不携带该声明（rc.6 实测，"^22.19 || >=24" 是上游仓库
    // 文档事实）。存在才校验下限——上游哪天开始声明并抬高要求时这里能红。
    if let Some(engines) = pkg.as_ref().and_then(|p| p["engines"]["node"].as_str()) {
        let node_major = |v: &str| {
            v.trim()
                .trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|m| m.parse::<u32>().ok())
                .unwrap_or(0)
        };
        let node_ver = Command::new(&node)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default();
        c.check(
            "内嵌 Node 满足 engines.node 下限",
            node_major(&node_ver) >= upstream::DSH_NODE_MAJOR_FLOOR,
            format!("engines.node={engines:?}, node={}", node_ver.trim()),
            "抬高 fetch-runtime.ps1 的 -NodeVersion 与 upstream::DSH_NODE_MAJOR_FLOOR",
        );
    } else {
        eprintln!("[note] 发布包未声明 engines.node：Node 下限事实以 §15 文档（^22.19 || >=24）为准");
    }

    // dsh-web-app rc.8 起 openBrowser 默认 true：壳靠 --no-open 抑制系统浏览器弹出
    let web_help = Command::new(&node)
        .arg(&bin)
        .arg(upstream::DSH_WEB_SUBCOMMAND)
        .arg("--help")
        .output();
    c.check(
        "web --help 含 --no-open（壳依赖它抑制系统浏览器弹出）",
        matches!(&web_help, Ok(o) if String::from_utf8_lossy(&o.stdout).contains(upstream::DSH_NO_OPEN_FLAG)),
        format!("{web_help:?}").chars().take(120).collect::<String>(),
        "--no-open 旗标变了：改 upstream::DSH_NO_OPEN_FLAG（影响 process.rs）",
    );

    // 内测声明豁免播种的事实：client.js 位置 + 版本 needle + 命名空间/字段名
    let welcome_client = upstream::join_segments(
        &upstream::dsh_node_modules_dir(rt),
        upstream::WELCOME_NOTICE_CLIENT_SEGMENTS,
    );
    let welcome_text = fs::read_to_string(&welcome_client).unwrap_or_default();
    c.check(
        "内测声明 client.js 存在且含文案版本 needle",
        !welcome_text.is_empty() && welcome_text.contains(upstream::WELCOME_NOTICE_VERSION_NEEDLE),
        welcome_client.display(),
        "文案版本形态变了：改 upstream::WELCOME_NOTICE_*（影响 welcome.rs）",
    );
    c.check(
        "内测声明命名空间 ui-onboarding / 字段 welcomeNoticeVersion 未变",
        welcome_text.contains(upstream::WELCOME_NOTICE_NAMESPACE)
            && welcome_text.contains(upstream::WELCOME_NOTICE_ACK_FIELD),
        format!("needle 命中但命名空间/字段缺失（client.js {} bytes）", welcome_text.len()),
        "设置键变了：改 upstream::WELCOME_NOTICE_NAMESPACE/ACK_FIELD（影响 welcome.rs）",
    );
    version
}

async fn probe_http(dsh: &Dsh, c: &mut Checker) {
    // 1) 无凭证 GET / → 401 + 提示文案（0.1.2 BrowserAuth 无关闭开关，回环也在门内）
    match dsh.client.get(dsh.url("/")).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            c.check(
                "无凭证 GET / → 401（BrowserAuth 门）",
                status.as_u16() == 401 && body.contains("dsh web authentication required"),
                format!("status={status}, body {} bytes", body.len()),
                "鉴权门形态变了：查 upstream::READY_URL_PREFIX/DSH_AUTH_COOKIE_PREFIX 与 prep §二（影响 dsh_session/process/notify/proxy 全链路）",
            );
        }
        Err(e) => c.check("GET / 可达", false, e.to_string(), "dsh 服务形态变了"),
    }
    // 2) token 交换：303 + Set-Cookie(dsh-auth-*，HttpOnly，SameSite=Strict)
    match dsh
        .client
        .get(dsh.url(&format!("/?token={}", dsh.token)))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let cookies: Vec<String> = resp
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok().map(String::from))
                .collect();
            let hit = cookies
                .iter()
                .find(|s| s.starts_with(upstream::DSH_AUTH_COOKIE_PREFIX));
            c.check(
                "GET /?token= → 303 + dsh-auth cookie（HttpOnly/SameSite=Strict）",
                status == 303
                    && hit.is_some_and(|s| s.contains("HttpOnly") && s.contains("SameSite=Strict")),
                format!(
                    "status={status}, set-cookie={:?}",
                    cookies
                        .iter()
                        .map(|s| s.chars().take(20).collect::<String>())
                        .collect::<Vec<_>>()
                ),
                "token 交换形态变了：prep §2.2（影响 dsh_session::exchange_cookie 与壳三条链路）",
            );
        }
        Err(e) => c.check("GET /?token= 可达", false, e.to_string(), "同上"),
    }
    // 3) 带 cookie GET / → 200 + </head>（proxy 注入点）；顺带取静态资产路径
    let mut asset_path: Option<String> = None;
    match dsh
        .client
        .get(dsh.url("/"))
        .header(reqwest::header::COOKIE, &dsh.cookie)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            c.check(
                "带 cookie GET / → 200",
                status.is_success(),
                format!("status={status}"),
                "cookie 链路失效：查 dsh_session 换取与 Host 栅栏（prep §2.3）",
            );
            c.check(
                "HTML 含 </head>（proxy 注入点）",
                body.to_lowercase().contains("</head>"),
                format!("body {} bytes", body.len()),
                "dsh 文档结构变了：remote/proxy.rs 注入要适配",
            );
            asset_path = body.match_indices("/assets/").next().and_then(|(i, _)| {
                let tail = &body[i..];
                tail.find(".js").map(|j| tail[..j + 3].to_string()).filter(|s| {
                    s.chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || "._-/".contains(ch))
                })
            });
        }
        Err(e) => c.check("带 cookie GET / 可达", false, e.to_string(), "同上"),
    }
    // 4) 静态资产无门（prep §2.3：仅 index 鉴权，/assets/* 公开）——无 cookie 也应 200
    match asset_path {
        Some(a) => match dsh.client.get(dsh.url(&a)).send().await {
            Ok(r) => c.check(
                "静态资产无鉴权门（无 cookie GET assets js → 200）",
                r.status().is_success(),
                format!("{a} status={}", r.status()),
                "静态门形态变了：prep §2.3（proxy 静态转发与页面首屏假设要重估）",
            ),
            Err(e) => c.check("静态资产可达", false, e.to_string(), "同上"),
        },
        None => c.check(
            "index HTML 引用 /assets/*.js",
            false,
            "未找到资产引用",
            "前端打包形态变了：找新的资产路径形态再定探针",
        ),
    }
    // 5) RPC 信封：带 cookie POST /api/session/list → 恒 200 + server-response + result.ok。
    // 0.1.2 的 typert 网关按描述符 wire 名传参：session/list 的形参 wire 名是
    // `_request`（SessionListRequest={cursor?}），不是裸 args（typert.host.js:874）
    let envelope = serde_json::json!({
        "type": "client-request",
        "rpcId": "probe-1",
        "method": upstream::METHOD_SESSION_LIST,
        "payload": { "args": { "_request": {} } }
    });
    match dsh
        .client
        .post(dsh.url(&format!("/api/{}", upstream::METHOD_SESSION_LIST)))
        .header(reqwest::header::COOKIE, &dsh.cookie)
        .json(&envelope)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let v: serde_json::Value = resp.json().await.unwrap_or_default();
            let ok = v.get("type").and_then(|t| t.as_str()) == Some("server-response")
                && v.pointer("/result/ok").and_then(|o| o.as_bool()) == Some(true);
            c.check(
                "RPC 信封：POST /api/session/list → server-response + result.ok",
                status.is_success() && ok,
                format!(
                    "status={status}, resp={}",
                    v.to_string().chars().take(120).collect::<String>()
                ),
                "RPC 信封变了：prep §3.1（影响一切 /api 调用；壳 proxy 只转发不组包，但鉴权/栅栏形态即此）",
            );
        }
        Err(e) => c.check("POST /api/session/list 可达", false, e.to_string(), "同上"),
    }
    // 6) Host/Origin 栅栏：带 cookie 但错 Origin 的 /api 请求 → 403
    // （remote/proxy.rs 剥浏览器标记头的依据；栅栏 403 优先于鉴权 401）
    match dsh
        .client
        .get(dsh.url(&format!("/api/{}", upstream::METHOD_SESSION_LIST)))
        .header(reqwest::header::COOKIE, &dsh.cookie)
        .header("Origin", "http://evil.invalid")
        .send()
        .await
    {
        Ok(resp) => c.check(
            "/api 信任栅栏：错 Origin → 403",
            resp.status().as_u16() == 403,
            format!("status={}", resp.status()),
            "栅栏行为变了：remote/proxy.rs 的剥头策略要重估",
        ),
        Err(e) => c.check("/api 信任栅栏探测可达", false, e.to_string(), "同上"),
    }
}

async fn probe_ws(dsh: &Dsh, c: &mut Checker) {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;
    let url = format!("ws://127.0.0.1:{}{}", dsh.port, upstream::DSH_MUX_PATH);
    // 1) 无 cookie → 握手被拒（401 原始 HTTP 响应 + Connection: close，prep §2.3）
    let rejected = tokio_tungstenite::connect_async(&url).await.is_err();
    c.check(
        "无 cookie 连 remote.mux → 握手被拒",
        rejected,
        "",
        "WS 鉴权门失效或端点消失：改 upstream::DSH_MUX_PATH（影响 notify/mux.rs 与 proxy 桥接）",
    );
    // 2) 带 cookie → 101。保留启动竞态宽限：WS 升级通道挂载可能晚于 HTTP 路由
    //（旧套件在 CI 快机上撞过这个窗），5s 内重试，持续失败才算漂移
    let mut stream = None;
    let mut last_err = String::new();
    let connect_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < connect_deadline {
        let mut req = match url.clone().into_client_request() {
            Ok(r) => r,
            Err(e) => {
                last_err = e.to_string();
                break;
            }
        };
        req.headers_mut()
            .insert("cookie", dsh.cookie.parse().unwrap());
        match tokio_tungstenite::connect_async(req).await {
            Ok((s, _)) => {
                stream = Some(s);
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    let Some(mut stream) = stream else {
        c.check(
            "带 cookie 连 remote.mux → 101",
            false,
            format!("connect 失败（5s 重试后）: {last_err}"),
            "WS 端点/鉴权形态变了：改 upstream::DSH_MUX_PATH 与 cookie 注入（notify/mux.rs、proxy.rs）",
        );
        return;
    };
    c.check("带 cookie 连 remote.mux → 101", true, "", "");
    // 3) open $events → 首条 item 必须是 ready（clientId + host）；观察窗内
    //    对 emit/waterfall 帧断言形状（空闲 dsh 可能无帧——观察到才断言）
    let open_events = serde_json::json!({
        "type": "open",
        "streamId": "probe-events",
        "endpoint": upstream::EVENT_STREAM_ENDPOINT,
        "payload": { "args": {} }
    });
    if stream
        .send(Message::Text(open_events.to_string().into()))
        .await
        .is_err()
    {
        c.check("open $events 发送", false, "", "mux 协议变了：prep §3.2");
        return;
    }
    let mut ready_seen = false;
    let mut observed = 0u32;
    let deadline = Instant::now() + FRAME_OBSERVE_WINDOW;
    while Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remain, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                if v.get("type").and_then(|t| t.as_str()) != Some("item") {
                    continue;
                }
                if v.get("streamId").and_then(|s| s.as_str()) != Some("probe-events") {
                    continue;
                }
                let value = v.get("value").cloned().unwrap_or_default();
                let vtype = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if !ready_seen && vtype == "ready" {
                    ready_seen = true;
                    let shaped = value.get("clientId").and_then(|i| i.as_str()).is_some()
                        && value.get("host").is_some_and(|h| h.is_object());
                    c.check(
                        "$events 首条 item = ready（clientId + host）",
                        shaped,
                        text.chars().take(120).collect::<String>(),
                        "ready 帧形状变了：prep §4.1（影响 notify/mux.rs 连接初始化）",
                    );
                    continue;
                }
                if matches!(vtype, "emit" | "waterfall") {
                    observed += 1;
                    let shaped = value.get("event").and_then(|e| e.as_str()).is_some();
                    c.check(
                        "$events 下行帧含事件名（emit/waterfall）",
                        shaped,
                        format!("type={vtype} event={:?}", value.get("event")),
                        "帧格式变了：改 upstream 事件词表常量（影响 notify/mod.rs 分类）",
                    );
                    if observed >= 3 {
                        break;
                    }
                }
            }
            Ok(Some(Ok(_))) => {} // Ping/Pong/Binary：0.1.2 起服务端 30s 心跳 Ping，不算
            _ => break,
        }
    }
    if !ready_seen {
        c.check(
            "$events 首条 item = ready",
            false,
            "观察窗内未见 ready 帧",
            "流初始化形态变了：prep §4.1（open 必须空 args；非空 signature-invalid）",
        );
    }
    if observed == 0 {
        eprintln!("[note] $events 观察窗内无 emit/waterfall 帧（空闲 dsh 正常）：帧分类正确性由 tests/notify_ws.rs 的脚本化假服务器保障");
    }
    // 4) open session/follow（不存在的 sessionId）→ 收 error 或 end 帧即形状正确。
    // 形参 wire 名是 `request`（SessionFollowRequest={address,maxMessages?}，
    // typert.host.js:822）——args 必须包一层 request，裸 address 会被网关拒为
    // arguments-invalid（0.1.2-rc.1 实测）。error 帧字段集（alpha.2 RemoteError
    // 统一封装）：0.1.2-rc.1 实测 = ["code","details","message"]。
    let open_follow = serde_json::json!({
        "type": "open",
        "streamId": "probe-follow",
        "endpoint": upstream::METHOD_SESSION_FOLLOW,
        "payload": { "args": { "request": { "address": { "kind": "session", "sessionId": "contract-nonexistent" } } } }
    });
    let _ = stream
        .send(Message::Text(open_follow.to_string().into()))
        .await;
    let deadline = Instant::now() + FRAME_OBSERVE_WINDOW;
    let mut terminated = false;
    while Instant::now() < deadline && !terminated {
        let remain = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remain, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                if v.get("streamId").and_then(|s| s.as_str()) != Some("probe-follow") {
                    continue;
                }
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("error") => {
                        terminated = true;
                        let err = v.get("error").cloned().unwrap_or_default();
                        let fields: Vec<String> = err
                            .as_object()
                            .map(|m| m.keys().cloned().collect())
                            .unwrap_or_default();
                        eprintln!("[note] follow error 帧字段集（回写 prep §3.2 用）: {err}");
                        c.check(
                            "session/follow 不存在会话 → error 帧",
                            !fields.is_empty(),
                            format!("error 字段集 {fields:?}"),
                            "error 帧形状变了：notify/mux.rs 的 follow 错误处理要适配",
                        );
                    }
                    Some("end") => {
                        terminated = true;
                        c.check("session/follow 不存在会话 → end 帧", true, "", "");
                    }
                    _ => {}
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => break,
        }
    }
    if !terminated {
        c.check(
            "session/follow 不存在会话 → error/end 帧",
            false,
            "观察窗内未见终结帧",
            "follow 错误语义变了：prep §5.1（影响 notify/mux.rs 的 follow 重建逻辑）",
        );
    }
}

fn probe_presets(rt: &Path, c: &mut Checker) {
    // 0.1.2 起预设独立成包：node_modules/@deepseek-ai/dsh-agent-presets/presets/minimal
    let dir = upstream::join_segments(
        &upstream::dsh_node_modules_dir(rt),
        upstream::PRESET_DIR_SEGMENTS,
    );
    let state = presets::preset_signature_state(&dir);
    // rc.8 起上游 minimal 预设自带 win32 门控（bash/pwsh 分行按 process.platform
    // 互斥禁用），我方补丁器已退役——此处断言 UpstreamHandled 作回归哨兵
    c.check(
        "minimal 预设签名 = UpstreamHandled（rc.8 起上游自修 win32；0.1.2 起独立成包）",
        matches!(state, SignatureState::UpstreamHandled),
        format!("实际 {state:?}"),
        "NeedsPatch=上游回退了 win32 修复→从 git 历史恢复 presets 补丁器并重评；Missing=预设目录变了→改 PRESET_DIR_SEGMENTS",
    );
}

fn probe_mcp(rt: &Path, dsh_home: &Path, c: &mut Checker) {
    // 插件仍是 dsh 的声明依赖（弱探测：node_modules 里的 package.json）
    let nm = upstream::dsh_node_modules_dir(rt);
    let hit = tree_find(&nm, upstream::MCP_PLUGIN_NAME.as_bytes(), Some("package.json"), 1 << 20, 3);
    c.check(
        "dsh 依赖树仍含 MCP 客户端插件",
        hit.is_some(),
        format!("hit={hit:?}"),
        "插件改名/移除：改 upstream::MCP_PLUGIN_NAME（影响 mcp.rs）",
    );
    // dsh 首启生成的 patch 文件：顶层必须是 op 序列（mcp.rs read_patch 的前提）。
    // 空文件/纯注释空序列都合法（dsh 首启生成 `[]`）。
    let patch = upstream::join_segments(dsh_home, upstream::MCP_PATCH_SEGMENTS);
    match fs::read_to_string(&patch) {
        Ok(text) => {
            let parsed = serde_yaml::from_str::<serde_yaml::Value>(&text);
            let ok = text.trim().is_empty()
                || matches!(&parsed, Ok(serde_yaml::Value::Sequence(_)))
                || matches!(&parsed, Ok(serde_yaml::Value::Null)); // 纯注释文件解析为 Null
            c.check(
                "cordis.patch.yml 顶层为 op 序列（mcp.rs 读写前提）",
                ok,
                format!("parse_ok={}", parsed.is_ok()),
                "cordis patch 结构变了：改 MCP_PATCH_SEGMENTS/CORDIS_OP_INSERT（影响 mcp.rs）",
            );
        }
        Err(e) => c.check(
            "cordis.patch.yml 首启生成",
            false,
            e.to_string(),
            "dsh 不再生成 patch 文件：查 MCP_PATCH_SEGMENTS（影响 mcp.rs/种子逻辑）",
        ),
    }
}

fn probe_remote_needles(rt: &Path, c: &mut Checker) {
    // proxy.rs 的 bundle 改写前提：插件 client.js 里仍含内测声明三元式
    // （实测落盘：node_modules/@deepseek-ai/dsh-client-ui-settings/lib/client.js）
    let nm = upstream::dsh_node_modules_dir(rt);
    let hit = tree_find(&nm, upstream::WELCOME_NOTICE_NEEDLE, Some("client.js"), 4 << 20, 4);
    c.check(
        "插件 client.js 仍含 WelcomeNotice 三元式 needle",
        hit.is_some(),
        format!("hit={hit:?}"),
        "dsh 改了持久化选择写法：proxy.rs 改写失效（声明每次远程连接都弹），改 upstream::WELCOME_NOTICE_NEEDLE",
    );
    // theme.rs 依赖的设置键仍在包内出现（实测落盘：dsh-client-ui-theme/lib/client.js 等）
    let hit = tree_find(&nm, upstream::KEY_UI_THEME.as_bytes(), Some("client.js"), 4 << 20, 4);
    c.check(
        "插件 client.js 仍含 ui-theme 设置键",
        hit.is_some(),
        format!("hit={hit:?}"),
        "主题键改名：改 upstream::KEY_UI_THEME（影响 theme.rs 跟随与首启播种）",
    );
    // proxy.rs 的 HTML 注入改写前提：入口文档 viewport meta 仍是 Vite 模板原值
    // （实测落盘：dsh-web-frontend/dist/index.html）——变了则禁缩放改写静默失效，
    // iOS 聚焦输入框自动放大整页不复原（0.5.4 手机实拍实踩）
    let index_html: PathBuf = [".", "@deepseek-ai", "dsh-web-frontend", "dist", "index.html"]
        .iter()
        .collect();
    let index_html = nm.join(index_html);
    let hit = std::fs::read(&index_html)
        .ok()
        .map(|b| {
            b.windows(upstream::VIEWPORT_META_NEEDLE.len())
                .any(|w| w == upstream::VIEWPORT_META_NEEDLE)
        })
        .unwrap_or(false);
    c.check(
        "入口文档 viewport meta 仍是 Vite 模板原值",
        hit,
        format!("path={index_html:?}"),
        "dsh 改了 viewport meta：proxy.rs 禁缩放改写失效（iOS 聚焦输入框整页放大），改 upstream::VIEWPORT_META_NEEDLE",
    );
    // splash 注入前提：入口文档的 React 挂载点仍是 Vite 模板原值
    // （变了则 proxy.rs 跳过 splash 注入，远程首连回到白屏等待）
    let hit = std::fs::read(&index_html)
        .ok()
        .map(|b| {
            b.windows(upstream::SPA_ROOT_MOUNT_NEEDLE.len())
                .any(|w| w == upstream::SPA_ROOT_MOUNT_NEEDLE)
        })
        .unwrap_or(false);
    c.check(
        "入口文档仍含 React 挂载点 <div id=\"root\"></div>",
        hit,
        format!("path={index_html:?}"),
        "dsh 改了挂载点结构：proxy.rs splash 注入整体跳过（远程首连回到白屏），改 upstream::SPA_ROOT_MOUNT_NEEDLE",
    );
    // mobile.css 隐藏 Session log 药丸的锚点：CSS Modules 本地名仍在插件包内
    // （实测落盘：@deepseek-ai/dsh-session-log-export/lib/client.js）
    let hit = tree_find(
        &nm,
        upstream::SESSION_LOG_BUTTON_NEEDLE.as_bytes(),
        Some("client.js"),
        4 << 20,
        4,
    );
    c.check(
        "插件 client.js 仍含 sessionLogButton 本地名",
        hit.is_some(),
        format!("hit={hit:?}"),
        "上游改了类名：mobile.css 的 [class*=\"_sessionLogButton\"] 隐藏规则静默失效（按钮复原显示），改 upstream::SESSION_LOG_BUTTON_NEEDLE 与 mobile.css 的选择器",
    );
    // mobile.css 模型选择器图标化的三个锚点：data-slot 语义钩子
    // （dsh-client-ui-conversation/lib/client.js）+ 触发器两段文案的
    // CSS Modules 本地名（dsh-client-ui-model-selection/lib/client.js）
    for (needle, desc, advice) in [
        (
            upstream::MODEL_SLOT_HOOK,
            "插件 client.js 仍含 conversation.input.model 槽位钩子",
            "上游改了槽位钩子：mobile.css 模型选择器图标化/菜单定位两组规则静默失效，改 upstream::MODEL_SLOT_HOOK 与 mobile.css 的选择器",
        ),
        (
            upstream::MODEL_TRIGGER_LABEL_NEEDLE,
            "插件 client.js 仍含 triggerLabel 本地名",
            "上游改了类名：图标化后模型名露出，改 upstream::MODEL_TRIGGER_LABEL_NEEDLE 与 mobile.css 的选择器",
        ),
        (
            upstream::MODEL_TRIGGER_EFFORT_NEEDLE,
            "插件 client.js 仍含 triggerEffort 本地名",
            "上游改了类名：图标化后推理等级文案露出（无 key 机器显示 \"Default\" 裸文本药丸挤换行），改 upstream::MODEL_TRIGGER_EFFORT_NEEDLE 与 mobile.css 的选择器",
        ),
    ] {
        let hit = tree_find(&nm, needle.as_bytes(), Some("client.js"), 4 << 20, 4);
        c.check(desc, hit.is_some(), format!("hit={hit:?}"), advice);
    }
}

/// 预装 /init 插件的 UI 折叠锚点（preseed 插件靠 source.kind="plugin"+notice
/// 把长提示词渲染成一行折叠的「上下文注入」；上游改了渲染分支即红）
fn probe_preseed_plugin_needles(rt: &Path, c: &mut Checker) {
    let nm = upstream::dsh_node_modules_dir(rt);
    for (needle, desc, advice) in [
        (
            upstream::CONTEXT_INJECTION_BRANCH_NEEDLE,
            "会话 UI 仍有 source.kind!==\"user\" → 折叠上下文行分支",
            "上游改了消息渲染分支：/init 注入的提示词可能重新渲染成完整用户气泡，改 upstream::CONTEXT_INJECTION_BRANCH_NEEDLE（影响 preseed 插件 dsh-command-init 的 source 策略）",
        ),
        (
            upstream::CONTEXT_INJECTION_TITLE_NEEDLE,
            "会话 UI 仍有 contextInjection locale 键（折叠行标题）",
            "上游改了 ContextInjectionRow 渲染路径：/init 折叠行标题丢失或整条改版，改 upstream::CONTEXT_INJECTION_TITLE_NEEDLE（影响 preseed 插件 dsh-command-init）",
        ),
        (
            upstream::NOTICE_SUMMARY_NEEDLE,
            "会话 UI 仍有 noticeSummary（notice 折叠行摘要）",
            "上游改了 notice 摘要读取：/init 折叠行只剩插件名、摘要丢失，改 upstream::NOTICE_SUMMARY_NEEDLE（影响 preseed 插件 dsh-command-init）",
        ),
    ] {
        let hit = tree_find(&nm, needle.as_bytes(), Some("client.js"), 4 << 20, 4);
        c.check(desc, hit.is_some(), format!("hit={hit:?}"), advice);
    }
}

/// dsh plugin 子命令（plugins.rs 的装/卸/更新依赖它；上游改版即红）
fn probe_plugins_cli(rt: &Path, c: &mut Checker) {
    let text = fs::read_to_string(upstream::dsh_bin(rt)).unwrap_or_default();
    c.check(
        "bin.js 定义 plugin 子命令",
        text.contains(r#"command("plugin")"#),
        "bin.js 找不到 command(\"plugin\")",
        "上游改了 plugin 入口：查 bin.js 并改 upstream::DSH_PLUGIN_SUBCOMMAND/FLAG（影响 plugins.rs）",
    );
    c.check(
        "plugin 子命令要求 --profile",
        text.contains(r#"requiredOption("--profile <name>","#),
        "bin.js 找不到 requiredOption(\"--profile <name>\")",
        "同上：改 upstream::DSH_PLUGIN_PROFILE_FLAG（影响 plugins.rs）",
    );
}

/// 目录选择器钉 browse 的前提（picker.rs/mobile.css 依赖；上游改版即红）
fn probe_picker(rt: &Path, c: &mut Checker) {
    let nm = upstream::dsh_node_modules_dir(rt);
    // 1) shipped bundle patch 仍有 id=directory-picker 的 auto 行（picker.rs 的
    //    disable 目标）。bundle patch 形态：顶层 op 序列，行嵌在 insert 列表里。
    let bundle_patch = nm
        .join("@deepseek-ai")
        .join("dsh-web-app")
        .join("cordis.patch.yml");
    let found_auto = fs::read_to_string(&bundle_patch)
        .ok()
        .and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok())
        .and_then(|v| v.as_sequence().cloned())
        .map(|ops| {
            ops.iter().any(|op| {
                op.get("insert")
                    .and_then(serde_yaml::Value::as_sequence)
                    .is_some_and(|rows| {
                        rows.iter().any(|r| {
                            r.get("id").and_then(serde_yaml::Value::as_str)
                                == Some(upstream::PICKER_AUTO_ROW_ID)
                                && r
                                    .get("name")
                                    .and_then(serde_yaml::Value::as_str)
                                    .is_some_and(|n| n.ends_with("directory-picker-auto"))
                        })
                    })
            })
        })
        .unwrap_or(false);
    c.check(
        "bundle patch 含 id=directory-picker 的 auto 行",
        found_auto,
        format!("path={bundle_patch:?}"),
        "上游改了行 id 或撤掉 auto：改 upstream::PICKER_AUTO_ROW_ID（picker.rs 的 disable 目标）；若上游默认 browse 了，删除 picker.rs 与本探测",
    );
    // 2) browse 对的两个包仍在依赖闭包（insert 行能被 Loader 解析的前提）
    for pkg in [
        upstream::PICKER_BROWSE_HOST_PKG,
        upstream::PICKER_BROWSE_SURFACE_PKG,
    ] {
        let dir = nm.join("@deepseek-ai").join(pkg.rsplit('/').next().unwrap());
        c.check(
            &format!("browse 包存在：{pkg}"),
            dir.join("package.json").is_file(),
            format!("dir={dir:?}"),
            "包改名/移除：改 upstream::PICKER_BROWSE_*_PKG（picker.rs 的 insert 目标）",
        );
    }
}

/// pickerpatch.rs 的原地补丁签名（两包内文件的 needle 全量核对；上游改版即红）
fn probe_pickerpatch(rt: &Path, c: &mut Checker) {
    let nm = upstream::dsh_node_modules_dir(rt);
    let host = upstream::join_segments(&nm, upstream::PICKER_HOST_BROWSE_FILE_SEGMENTS);
    let client = upstream::join_segments(&nm, upstream::PICKER_CLIENT_BROWSE_FILE_SEGMENTS);
    let host_text = fs::read_to_string(&host).unwrap_or_default();
    let client_text = fs::read_to_string(&client).unwrap_or_default();
    c.check(
        "browse host index.js 存在且含哨兵补丁锚点",
        host_text.contains(upstream::PICKER_HOST_LIST_NEEDLE)
            && host_text.contains(upstream::PICKER_HOST_CRUMBS_NEEDLE),
        format!("path={}", host.display()),
        "host browse 形态变了：改 upstream::PICKER_HOST_*_NEEDLE 与 pickerpatch.rs 的 HOST_* 替换串",
    );
    let client_ok = [
        upstream::PICKER_CLIENT_HIDDEN_INIT_NEEDLE,
        upstream::PICKER_CLIENT_HIDDEN_RESET_NEEDLE,
        upstream::PICKER_CLIENT_CRUMBS_NEEDLE,
        upstream::PICKER_CLIENT_CRUMB_LABEL_NEEDLE,
        upstream::PICKER_CLIENT_OPEN_DISABLED_NEEDLE,
        upstream::PICKER_CLIENT_NEWFOLDER_DISABLED_NEEDLE,
        upstream::PICKER_CLIENT_LOCALE_ZH_NEEDLE,
        upstream::PICKER_CLIENT_LOCALE_EN_NEEDLE,
    ]
    .iter()
    .all(|n| client_text.contains(n));
    c.check(
        "browse client.js 存在且含全部补丁锚点（8 处）",
        !client_text.is_empty() && client_ok,
        format!("path={}", client.display()),
        "client browse 形态变了：改 upstream::PICKER_CLIENT_*_NEEDLE 与 pickerpatch.rs 的 CLIENT_* 替换串",
    );
}

/// 远程"项目"标签依赖的上游事实（project.rs/project.html；上游改版即红）
fn probe_project(rt: &Path, dsh_home: &Path, c: &mut Checker) {
    let nm = upstream::dsh_node_modules_dir(rt);
    // 1) SPA 记当前会话的 localStorage 键（实测落盘：dsh-client-runtime/lib/client.js）
    let hit = tree_find(
        &nm,
        upstream::LOCALSTORAGE_CURRENT_SESSION_KEY.as_bytes(),
        Some("client.js"),
        4 << 20,
        4,
    );
    c.check(
        "SPA 仍用 localStorage 键 dsh.sessions.current 记当前会话",
        hit.is_some(),
        format!("hit={hit:?}"),
        "键名改了：改 upstream::LOCALSTORAGE_CURRENT_SESSION_KEY（project.html 取不到当前会话→'项目'标签空态）",
    );
    // 2) 工作区注册表 schema 锚点字段仍在 dsh-workspace 包内
    let ws_lib = nm
        .join("@deepseek-ai")
        .join("dsh-workspace")
        .join("lib")
        .join("index.js");
    let text = fs::read_to_string(&ws_lib).unwrap_or_default();
    c.check(
        "dsh-workspace 仍含 sessionIds 字段（workspace.json schema 锚点）",
        text.contains(upstream::WORKSPACE_SCHEMA_NEEDLE),
        format!("path={}", ws_lib.display()),
        "schema 变了：核对 storages/workspace.json 实际结构，改 project.rs 的 Store/Ws 与 upstream 常量",
    );
    // 3) 契约环境若已产生 workspace.json，逐项校验 schema（无则宽容通过）
    let store = upstream::join_segments(dsh_home, upstream::WORKSPACE_STORE_SEGMENTS);
    if let Ok(t) = fs::read_to_string(&store) {
        let ok = serde_json::from_str::<serde_json::Value>(&t)
            .ok()
            .and_then(|v| v.get("tables")?.get("workspaces")?.as_object().cloned())
            .is_some_and(|ws| {
                ws.values().all(|w| {
                    w.get("path").is_some_and(|p| p.is_string())
                        && w.get("sessionIds").is_some_and(|s| s.is_array())
                })
            });
        c.check(
            "storages/workspace.json schema（tables.workspaces[*].path/sessionIds）",
            ok,
            format!("path={}", store.display()),
            "存储结构变了：改 upstream::WORKSPACE_STORE_SEGMENTS 与 project.rs 解析（'项目'标签解析不到工作区）",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_contract() {
    let Some(rt) = runtime_dir() else {
        eprintln!("skipped: 无真实运行时（DSHDESKTOP_RUNTIME_DIR 未设且 runtime/windows-x64 不存在）");
        return;
    };
    let mut c = Checker::default();
    let version = probe_entry(&rt, &mut c);
    probe_plugins_cli(&rt, &mut c);
    probe_picker(&rt, &mut c);
    probe_pickerpatch(&rt, &mut c);
    probe_presets(&rt, &mut c);
    probe_remote_needles(&rt, &mut c);
    probe_preseed_plugin_needles(&rt, &mut c);
    match spawn_dsh(&rt).await {
        Ok(dsh) => {
            probe_http(&dsh, &mut c).await;
            probe_ws(&dsh, &mut c).await;
            probe_mcp(&rt, dsh.home.path(), &mut c);
            probe_project(&rt, dsh.home.path(), &mut c);
        }
        Err(e) => c.check(
            "dsh web 启动",
            false,
            e,
            "入口/命令形/Node 版本：查 upstream::DSH_* 与 process.rs",
        ),
    }
    if !c.failures.is_empty() {
        panic!(
            "上游契约漂移（dsh {version}）{} 项：\n\n{}",
            c.failures.len(),
            c.failures.join("\n")
        );
    }
    eprintln!("上游契约全绿（dsh {version}）");
}
