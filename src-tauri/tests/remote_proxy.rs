//! 远程代理集成测试（0.1.2）：cookie 代持 + 401 重放 + WS 桥接注入。
//! 假 dsh 用 support::spawn_fake_dsh（进程内 axum，0.1.2 鉴权/栅栏/mux 全仿真），
//! 旧 fake-dsh.cjs 只剩 tests/process.rs 与 remote_manager.rs 在用。

use dshdesktop_lib::dsh_session::DshCreds;
use dshdesktop_lib::port::free_port;
use dshdesktop_lib::remote::proxy::{spawn_proxy, ProxyHandle, COOKIE_NAME};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

mod support;

/// 起代理（creds None 模拟 dsh 未就绪）；返回 (句柄, 代理 token, creds 发送端)。
/// creds 发送端必须由调用方持有（drop 后 watch 关闭，代理读末值仍可工作，
/// 但中途换端口的重放用例需要活着的发送端）
async fn start_proxy(
    creds: Option<Arc<DshCreds>>,
) -> (
    ProxyHandle,
    Arc<str>,
    watch::Sender<Option<Arc<DshCreds>>>,
) {
    let token: Arc<str> = dshdesktop_lib::remote::generate_token().into();
    let (tx, rx) = watch::channel(creds);
    // dsh-home 用一次性临时目录（keep 后不自动删除，测试进程结束由 OS 清理）
    let home = tempfile::tempdir().unwrap().keep();
    let handle = spawn_proxy(token.clone(), rx, home, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    (handle, token, tx)
}

/// 起假 dsh，返回 (FakeDsh, scripted 注入端)
async fn spawn_dsh() -> (support::FakeDsh, mpsc::UnboundedSender<Value>) {
    let port = free_port().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let fake = support::spawn_fake_dsh(port, support::ScriptedFrames(rx)).await;
    (fake, tx)
}

fn creds_for(port: u16) -> Arc<DshCreds> {
    Arc::new(DshCreds {
        port,
        token: support::FIXTURE_TOKEN.into(),
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // 测试环境可能设了系统代理（HTTP_PROXY），访问 127.0.0.1 必须直连
        .no_proxy()
        .build()
        .unwrap()
}

async fn get_direct(url: &str) -> reqwest::Result<reqwest::Response> {
    client().get(url).send().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gate_covers_shell_routes() {
    // 门岗中间件化后：壳自有路由（/__dsh-desktop/*）同样被 token 门岗拦截。
    // 若门岗退回 fallback 内部判断，这些显式路由会绕过鉴权——本测试就是那道闸
    let (handle, _token, _tx) = start_proxy(None).await;
    let base = format!("http://127.0.0.1:{}", handle.port);
    for path in [
        "/__dsh-desktop/project",
        "/__dsh-desktop/api/resolve?sid=x",
        "/__dsh-desktop/api/list?sid=x",
        "/__dsh-desktop/api/file?sid=x&rel=a.txt",
    ] {
        let res = get_direct(&format!("{base}{path}")).await.unwrap();
        assert_eq!(res.status(), 403, "无凭据访问 {path} 必须 403");
    }
    handle.shutdown().await;
}

#[test]
fn token_is_64_hex_and_unique() {
    let a = dshdesktop_lib::remote::generate_token();
    let b = dshdesktop_lib::remote::generate_token();
    assert_eq!(a.len(), 64);
    assert!(
        a.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "应为小写 hex：{a}"
    );
    assert_ne!(a, b, "两次生成不应相同");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gate_requires_token() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let http = client();

    // 1. 无凭据 → 403，失效页应指引改点原始完整链接（cookie 丢失场景旧链接仍有效）
    let r = http.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(r.status(), 403, "无凭据应 403");
    let body = r.text().await.unwrap();
    assert!(
        body.contains("完整链接"),
        "失效页应提示改点带 token 的完整链接，实际 {body}"
    );

    // 2. 错误 token → 403，且有 ≥400ms 的防爆破延迟
    let t0 = Instant::now();
    let r = http
        .get(format!("{base}/?token=wrong"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "错误 token 应 403");
    assert!(
        t0.elapsed() >= Duration::from_millis(400),
        "错误 token 应有延迟"
    );

    // 3. 正确 token → 302 到去 token 的地址 + 种 cookie
    let r = http
        .get(format!("{base}/some/path?a=1&token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302, "正确 token 应 302，实际 {:?}", r.status());
    let loc = r
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(loc, "/some/path?a=1", "Location 应剥离 token，实际 {loc}");
    let set_cookie = r
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        set_cookie.contains(&format!("{COOKIE_NAME}={token}")),
        "应种 cookie，实际 {set_cookie}"
    );
    assert!(set_cookie.contains("HttpOnly"), "cookie 应 HttpOnly");
    assert!(
        set_cookie.contains("Max-Age=2592000"),
        "cookie 应为 30 天长效（手机浏览器杀进程不再掉登录），实际 {set_cookie}"
    );

    // 4. 带 cookie → 200 且转发到 dsh（0.1.2 假服务器回 SPA 入口 HTML）
    let r = http
        .get(format!("{base}/"))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "带 cookie 应放行，实际 {:?}", r.status());
    assert!(r.text().await.unwrap().contains("<html>"));

    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dsh_down_returns_503() {
    let (proxy, token, _tx) = start_proxy(None).await;
    let http = client();
    let r = http
        .get(format!("http://127.0.0.1:{}/", proxy.port))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503, "dsh 未就绪应 503，实际 {:?}", r.status());
    proxy.shutdown().await;
}

/// 真实 dsh 有浏览器信任栅栏：/api 请求若 Origin.host ≠ Host 头（或
/// sec-fetch-site: cross-site）→ 403。经隧道远程访问时浏览器带的是
/// trycloudflare 域名的 Origin，代理转发前必须剥掉这些浏览器标记头。
/// 0.1.2 增补：转发还必须携带代持的 dsh-auth cookie（假服务器侧记录验证）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forward_strips_browser_marker_headers_and_carries_dsh_cookie() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let r = client()
        .post(format!("{base}/api/agentPreset.list"))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .header("content-type", "application/json")
        // 模拟经隧道访问的浏览器：同源 POST 自动带 Origin/Referer/Sec-Fetch-*
        .header("origin", "https://random-sub.trycloudflare.com")
        .header("referer", "https://random-sub.trycloudflare.com/")
        .header("sec-fetch-site", "same-origin")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-dest", "empty")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "带隧道 Origin 的 RPC 经代理后不应触发 dsh 栅栏，实际 {:?}",
        r.status()
    );
    // 假服务器侧：这次 /api 转发必须带上了代持的 dsh-auth cookie
    let hits = fake.api_hits.lock().unwrap().clone();
    assert!(
        hits.iter()
            .any(|(path, authed)| path.contains("agentPreset.list") && *authed),
        "转发应携带 dsh-auth cookie，实际 hits：{:?}",
        hits
    );
    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_bridged_with_cookie() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let url = format!("ws://127.0.0.1:{}/api/remote.mux", proxy.port);
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert("cookie", format!("{COOKIE_NAME}={token}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("带 cookie 的 WS 握手应成功");
    // 客户端 open $events 应经桥到达假 dsh（桥侧须注入 dsh-auth cookie，0.1.2 起
    // 漏注入 = 页面开但全断）并回 ready 帧
    ws.send(Message::Text(
        json!({"type":"open","streamId":"p1","endpoint":"$events","payload":{"args":{}}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let got_ready = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(t) = msg {
                if t.as_str().contains("\"type\":\"ready\"") {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    let bridged = fake
        .opens
        .lock()
        .unwrap()
        .iter()
        .any(|o| o.contains("$events"));

    fake.shutdown.notify_one();
    proxy.shutdown().await;
    assert!(bridged, "桥接应已建立并 open $events");
    assert!(got_ready, "open $events 后应经桥收到 ready 帧");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_rejected_without_cookie() {
    let (proxy, _token, _tx) = start_proxy(None).await;
    let url = format!("ws://127.0.0.1:{}/api/remote.mux", proxy.port);
    let req = url.into_client_request().unwrap();
    let result = tokio_tungstenite::connect_async(req).await;
    assert!(result.is_err(), "无 cookie 的 WS 握手应被拒");
    proxy.shutdown().await;
}

/// 远程访问的页面源是隧道域名（非 loopback），dsh 的"内测声明"因此用内存
/// 确认、每次访问都弹窗。代理把插件 bundle 里的持久化选择三元式
/// `isLoopback ? "host" : "memory"` 改写为 `"host"`，确认落 settings.yaml。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rewrites_welcome_notice_persistence_in_plugin_bundle() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let http = client();

    // 插件 bundle：三元式应被改写为 "host"（0.1.2 needle 带 $host 接收者前缀）；
    // 带 buster 模拟 302 击穿后的重取（缺 buster 会被 302，见 cache_bust 测试）
    let r = http
        .get(format!(
            "{base}/plugins/fake/client.js?rev=1&dshv={}",
            env!("CARGO_PKG_VERSION")
        ))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .header("accept-encoding", "gzip, br") // 浏览器常态；改写路径须剥掉求 identity
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers().get("content-encoding").is_none(),
        "改写路径不应带压缩编码"
    );
    let body = r.text().await.unwrap();
    assert!(
        body.contains(r#"ctx.remote.$host, "host""#),
        "三元式应被改写为 \"host\"，实际：{body}"
    );
    assert!(
        !body.contains("isLoopback"),
        "改写后不应残留 isLoopback 三元式：{body}"
    );

    // 普通路径：内容原样透传不受影响
    let r = http
        .get(format!("{base}/"))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("<html>"));

    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

/// 0.1.2 起插件客户端 bundle 合并加载：`/plugins/??<a>/client.js,<b>/client.js&rev=N`
/// （path 部分只剩 "/plugins/"，组合清单整体在 query 里）。改写 matcher 必须命中
/// combo 形态，否则三元式不被改写、远程端内测声明每次连接都弹（0.5.3 实踩）。
/// 带 buster 的请求转发时须已剥掉 dshv（真 dsh 对组合 URL query 逐字校验，多余参数 404）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rewrites_welcome_notice_in_combo_bundle() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let http = client();

    let combo = "/plugins/??@deepseek-ai/dsh-client-ui-settings/client.js,@deepseek-ai/dsh-client-ui-session/client.js&rev=b6deae2120c2";
    let r = http
        .get(format!(
            "{base}{combo}&dshv={}",
            env!("CARGO_PKG_VERSION")
        ))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .header("accept-encoding", "gzip, br")
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "combo bundle（带 buster）应转发成功，实际 {:?}",
        r.status()
    );
    assert!(
        r.headers().get("content-encoding").is_none(),
        "改写路径不应带压缩编码"
    );
    let body = r.text().await.unwrap();
    assert!(
        body.contains(r#"ctx.remote.$host, "host""#),
        "combo bundle 的三元式应被改写，实际：{body}"
    );
    assert!(!body.contains("isLoopback"), "改写后不应残留三元式：{body}");
    let hits = fake.plugin_hits.lock().unwrap().clone();
    assert!(
        hits.iter().any(|h| h == combo),
        "转发到 dsh 的 combo URL 不应携带 dshv，实际 hits：{:?}",
        hits
    );

    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

/// 缓存击穿：dsh 把 bundle 响应标为 immutable（一年），而壳侧改写产物随壳版本
/// 变化——未带 dshv 的 bundle 请求 302 到带 dshv=<壳版本> 的同 URL 强制重取；
/// 带 buster 的请求直接放行；WS 升级与非 bundle 资源永不重定向。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plugin_bundle_requests_are_cache_busted() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let http = client();
    let cookie = format!("{COOKIE_NAME}={token}");
    let ver = env!("CARGO_PKG_VERSION");

    // 1. 单插件形态缺 buster → 302 追加 dshv，重定向本身 no-store
    let r = http
        .get(format!("{base}/plugins/fake/client.js?rev=1"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302, "缺 buster 的 bundle 请求应 302");
    let loc = r
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        loc,
        format!("/plugins/fake/client.js?rev=1&dshv={ver}"),
        "Location 应带壳版本 buster"
    );
    assert_eq!(
        r.headers().get("cache-control").map(|v| v.to_str().unwrap()),
        Some("no-store"),
        "302 本身不得被缓存"
    );

    // 2. combo 形态缺 buster → 同样 302
    let combo = "/plugins/??@deepseek-ai/a/client.js,@deepseek-ai/b/client.js&rev=abc";
    let r = http
        .get(format!("{base}{combo}"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302, "combo bundle 缺 buster 也应 302");
    let loc = r
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(loc, format!("{combo}&dshv={ver}"));

    // 3. 带 buster → 不再 302，直接转发
    let r = http
        .get(format!("{base}/plugins/fake/client.js?rev=1&dshv={ver}"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "带 buster 应直接转发，实际 {:?}", r.status());

    // 4. 非 bundle 的 /plugins/ 资源不 302（fake 对未知路径回 404 = 转发结果）
    let r = http
        .get(format!("{base}/plugins/fake/icon.png"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "非 bundle 资源不应被 302 击穿");

    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

/// 移动端适配：代理转发 HTML 文档时往 </head> 前注入 mobile.css（设置弹窗全屏化、
/// 侧栏抽屉化等 @media ≤700px 规则）。无 </head> 或非 HTML 一律原文透传。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn injects_mobile_css_into_html_documents() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let http = client();

    // 1. HTML 文档：注入移动端样式（浏览器文档请求带 accept: text/html 与压缩意愿）
    let r = http
        .get(format!("{base}/app"))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .header("accept", "text/html,application/xhtml+xml")
        .header("accept-encoding", "gzip, br") // 改写路径须剥掉求 identity
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers().get("content-encoding").is_none(),
        "改写路径不应带压缩编码"
    );
    let content_length = r
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    let body = r.text().await.unwrap();
    if let Some(n) = content_length {
        // hyper 对完整缓冲体会自动重算 content-length：若存在则必须等于新长度
        assert_eq!(n, body.len(), "content-length 应与改写后体长一致");
    }
    assert!(
        body.contains("<!-- dshdesktop-mobile --><style>"),
        "应注入样式标记，实际：{body}"
    );
    assert!(
        body.contains("max-width: 700px") && body.contains("</head>"),
        "注入内容应含移动端媒体查询且保留 </head>：{body}"
    );
    assert!(
        body.contains("_millerRow"),
        "注入内容应含目录浏览对话框的适配规则（picker.rs 钉的 browse 交互）：{body}"
    );
    assert!(
        body.contains("_composerStack\"] textarea"),
        "注入内容应含输入框 ≥16px 字号规则（iOS 聚焦缩放双保险）：{body}"
    );
    assert!(
        body.contains("</style><script>") && body.contains("data-dshmobile-tab"),
        "应注入信息标签页脚本：{body}"
    );
    assert!(
        body.find("<!-- dshdesktop-mobile -->").unwrap() < body.find("</head>").unwrap(),
        "样式应注入到 </head> 之前"
    );
    // viewport meta 改写：禁缩放防 iOS WKWebView 聚焦 <16px 输入框放大整页不复原
    assert!(
        body.contains(
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no\">"
        ),
        "viewport meta 应改写为禁缩放形态：{body}"
    );
    assert!(
        !body.contains("content=\"width=device-width, initial-scale=1\""),
        "原始 viewport meta 不应残留：{body}"
    );
    // 加载过渡页（splash）：挂载点原样保留、splash 紧随其后（隧道首连 ~5MB
    // 白屏几十秒的观感对策，React 挂载后 splash.js 自动淡出移除）
    assert!(
        body.contains(r#"<div id="root"></div><div id="dsh-splash""#),
        "splash 覆盖层应紧随挂载点注入：{body}"
    );
    assert!(
        body.contains("dsh-splash-spin") && body.contains("dsh-splash-done"),
        "splash 样式与卸载脚本应就位：{body}"
    );
    assert!(
        body.contains("DeepSeek Harness"),
        "splash 标题应就位：{body}"
    );
    // 手机端隐藏 Session 日志下载按钮（药丸悬浮盖住"N 个后台任务运行中"文案）
    assert!(
        body.contains(r#"_sessionLogButton"][class*="_sessionLogButton"]"#)
            && body.contains("display: none;"),
        "移动端适配应含 sessionLog 按钮隐藏规则：{body}"
    );

    // 2. 无 </head> 的 HTML：原文透传
    let r = http
        .get(format!("{base}/app-nohead"))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert_eq!(body, "<!doctype html><html><body>no head</body></html>");

    // 3. 非 HTML（text/html 以外的 SPA 入口也注入；text/plain 不受影响）
    let r = http
        .get(format!("{base}/"))
        .header("cookie", format!("{COOKIE_NAME}={token}"))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("<html>"));

    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

/// 代理侧 gzip：dsh 服务端不做任何压缩，远程首连 ~5MB 文本资产全走 identity
/// （0.5.5 手机实拍白屏几十秒的根因之一）。对 ≥4KB 文本资产（含插件 bundle
/// 改写产物）在代理侧缓冲 gzip；小体积/二进制/Range 请求/客户端未宣告接受的
/// 一律 identity 透传。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gzip_compresses_large_text_assets() {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let gunzip = |bytes: &[u8]| {
        let mut out = Vec::new();
        GzDecoder::new(bytes).read_to_end(&mut out).unwrap();
        out
    };
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let http = client();
    let cookie = format!("{COOKIE_NAME}={token}");
    let expect: Vec<u8> = (0..5000)
        .map(|i| format!("export const v{i} = {i};\n"))
        .collect::<String>()
        .into_bytes();

    // 1. 大 JS 资产 + 客户端接受 gzip → 压缩，解压还原 dsh 原文
    let r = http
        .get(format!("{base}/assets/big.js"))
        .header("cookie", &cookie)
        .header("accept-encoding", "gzip, br")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers().get("content-encoding").unwrap(),
        "gzip",
        "大文本资产应压缩"
    );
    assert_eq!(
        r.headers().get("vary").map(|v| v.to_str().unwrap()),
        Some("accept-encoding")
    );
    let gz = r.bytes().await.unwrap();
    assert!(
        gz.len() < expect.len() / 3,
        "重复文本应显著压缩（{} < {}）",
        gz.len(),
        expect.len()
    );
    assert_eq!(gunzip(&gz), expect, "解压应还原 dsh 原文");

    // 2. 同资产、客户端未宣告接受 → identity 原文透传
    let r = http
        .get(format!("{base}/assets/big.js"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers().get("content-encoding").is_none());
    assert_eq!(r.bytes().await.unwrap().as_ref(), expect.as_slice());

    // 3. Range 请求不做变换（压缩后区间语义会破）
    let r = http
        .get(format!("{base}/assets/big.js"))
        .header("cookie", &cookie)
        .header("accept-encoding", "gzip")
        .header("range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert!(r.headers().get("content-encoding").is_none(), "Range 请求不应压缩");

    // 4. 小体积文本（<4KB）低于门槛 → identity
    let r = http
        .get(format!("{base}/assets/small.js"))
        .header("cookie", &cookie)
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert!(
        r.headers().get("content-encoding").is_none(),
        "小资产不应压缩"
    );

    // 5. 二进制资产（自带压缩格式）不在白名单 → identity
    let r = http
        .get(format!("{base}/assets/logo.png"))
        .header("cookie", &cookie)
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert!(
        r.headers().get("content-encoding").is_none(),
        "图片不应压缩"
    );
    assert_eq!(r.bytes().await.unwrap().len(), 8192);

    // 6. 插件 bundle 改写路径：产物同样补回 gzip（带 buster 模拟击穿后的重取），
    //    且解压后三元式已改写
    let r = http
        .get(format!(
            "{base}/plugins/big/client.js?rev=1&dshv={}",
            env!("CARGO_PKG_VERSION")
        ))
        .header("cookie", &cookie)
        .header("accept-encoding", "gzip, br")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers().get("content-encoding").unwrap(),
        "gzip",
        "bundle 改写产物应压缩"
    );
    let body = String::from_utf8(gunzip(&r.bytes().await.unwrap())).unwrap();
    assert!(
        body.contains(r#"ctx.remote.$host, "host""#),
        "解压后三元式应已改写：{body}"
    );
    assert!(!body.contains("isLoopback"), "解压后不应残留三元式：{body}");

    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_stops_listener() {
    let (proxy, _token, _creds_tx) = start_proxy(Some(creds_for(1))).await;
    let url = format!("http://127.0.0.1:{}/", proxy.port);
    let r = get_direct(&url).await.unwrap();
    assert_eq!(r.status(), 403);
    let port = proxy.port;
    proxy.shutdown().await;
    // 端口应已释放，可自行 bind（立即 drop 掉，否则后续请求会连到这个探针上干等）
    let rebind = std::net::TcpListener::bind(format!("127.0.0.1:{port}"));
    assert!(rebind.is_ok(), "shutdown 后端口应释放：{rebind:?}");
    drop(rebind);
    // 新连接应被拒（须用绕开系统代理的客户端，否则请求会被代理软件接管）
    let res = get_direct(&url).await;
    assert!(res.is_err(), "shutdown 后新连接应被拒，实际 {res:?}");
}

/// 重置链接（token 轮换）：旧 token 与旧 cookie 立即失效，新 token 正常种 cookie。
/// 代理与隧道都不重启，端口不变。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reset_token_revokes_old_credential() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, old_token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let http = client();
    let old_port = proxy.port;

    // 重置前：旧凭据可用
    let r = http
        .get(format!("{base}/"))
        .header("cookie", format!("{COOKIE_NAME}={old_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "重置前旧 cookie 应放行");

    let new_token: Arc<str> = dshdesktop_lib::remote::generate_token().into();
    proxy.reset_token(new_token.clone());
    assert_eq!(proxy.port, old_port, "重置不应重启代理（端口不变）");

    // 旧 cookie → 403
    let r = http
        .get(format!("{base}/"))
        .header("cookie", format!("{COOKIE_NAME}={old_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "重置后旧 cookie 应失效");

    // 旧链接里的 token → 403
    let r = http
        .get(format!("{base}/?token={old_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "重置后旧 token 应失效");

    // 新 token → 302 + 种新 cookie（同样长效）
    let r = http
        .get(format!("{base}/?token={new_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302, "新 token 应 302 种 cookie");
    let set_cookie = r
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        set_cookie.contains("Max-Age=2592000"),
        "重置后新 cookie 同样应为 30 天长效，实际 {set_cookie}"
    );
    let r = http
        .get(format!("{base}/"))
        .header("cookie", format!("{COOKIE_NAME}={new_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "新 cookie 应放行");

    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

/// 重置链接必须掐断已建立的 WS 桥接——否则链接泄露时攻击者已开的页面
/// 仍能持续收事件流，重置形同虚设。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reset_token_drops_live_ws() {
    let (fake, _tx) = spawn_dsh().await;
    let (proxy, token, _creds_tx) = start_proxy(Some(creds_for(fake.port))).await;
    let url = format!("ws://127.0.0.1:{}/api/remote.mux", proxy.port);
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert("cookie", format!("{COOKIE_NAME}={token}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("带 cookie 的 WS 握手应成功");

    // 确认桥接已通（open $events → ready 帧回来）
    ws.send(Message::Text(
        json!({"type":"open","streamId":"p1","endpoint":"$events","payload":{"args":{}}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(t) = msg {
                if t.as_str().contains("\"type\":\"ready\"") {
                    return true;
                }
            }
        }
        false
    })
    .await;
    assert!(matches!(first, Ok(true)), "桥接应已建立（ready 帧回来）：{first:?}");

    let new_token: Arc<str> = dshdesktop_lib::remote::generate_token().into();
    proxy.reset_token(new_token);

    // 桥接被掐断：读侧应在 5s 内收到 Close 或连接错误/EOF
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return true,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "重置后已建立的 WS 应在 5s 内被掐断");

    fake.shutdown.notify_one();
    proxy.shutdown().await;
}

/// 0.1.2 cookie 代持的失效重放：dsh 重启换端口后旧 cookie 全失效——代理转发得
/// 401 → 清缓存按新 creds 重换 → 重放一次拿到 200（只重放一次防环）
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_cookie_refreshes_on_401() {
    let (fake_a, _tx) = spawn_dsh().await;
    let (fake_b, _tx2) = spawn_dsh().await;
    let (proxy, proxy_token, creds_tx) = start_proxy(Some(creds_for(fake_a.port))).await;
    let base = format!("http://127.0.0.1:{}", proxy.port);
    let http = client();

    // 第一次：走 fake A，缓存下 A 的 cookie
    let r = http
        .get(format!("{base}/"))
        .header("cookie", format!("{COOKIE_NAME}={proxy_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "首次转发应成功（cookie A 已缓存）");

    // dsh 重启：creds 换到 B（端口变了 → A 的 cookie 绑定旧 authority，必 401）
    creds_tx.send_replace(Some(creds_for(fake_b.port)));
    let r = http
        .get(format!("{base}/"))
        .header("cookie", format!("{COOKIE_NAME}={proxy_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "401 后应重换 cookie 并重放成功，实际 {:?}",
        r.status()
    );
    // B 侧页面命中记录：重放成功的请求应带上了 B 的新 cookie
    let hits = fake_b.api_hits.lock().unwrap().clone();
    assert!(
        hits.iter().any(|(path, authed)| path == "/" && *authed),
        "重放请求应携带 B 的新 cookie，实际 hits：{:?}",
        hits
    );
    let rejected = hits
        .iter()
        .filter(|(path, _)| path == "/")
        .any(|(_, authed)| !authed);
    assert!(rejected, "旧 cookie 应先在 B 上被拒 401，实际 hits：{:?}", hits);

    fake_a.shutdown.notify_one();
    fake_b.shutdown.notify_one();
    proxy.shutdown().await;
}