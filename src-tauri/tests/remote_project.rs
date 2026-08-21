//! 远程"项目"标签端点集成测试：token 门岗 + resolve/list/file 全链路。
//! home 布局：<tmp>/storages/workspace.json（w1 指向 <tmp>/ws-a）+ 工作区文件树。

use dshdesktop_lib::remote::generate_token;
use dshdesktop_lib::remote::proxy::{spawn_proxy, ProxyHandle, COOKIE_NAME};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::watch;

fn make_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let ws = home.path().join("ws-a");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("README.md"), "# 你好\n\n正文**加粗**。").unwrap();
    std::fs::write(ws.join("src").join("main.py"), "print('hi')\n").unwrap();
    std::fs::write(ws.join(".hidden.txt"), "dot").unwrap(); // 点开头条目须默认列出
    std::fs::write(ws.join("图 1.png"), [0x89, 0x50, 0x4E, 0x47]).unwrap();
    // 超 8MB 文本预览 cap（FILE_CAP 64MB 分支由 check_size 单测覆盖，避免大文件拖慢套件）
    std::fs::write(ws.join("big.txt"), vec![b'x'; 9 * 1024 * 1024]).unwrap();
    std::fs::create_dir_all(home.path().join("storages")).unwrap();
    std::fs::write(
        home.path().join("storages").join("workspace.json"),
        format!(
            r#"{{"tables":{{"workspaces":{{"w1":{{"path":{},"title":"A 项目","sessionIds":["session-aaa"]}}}}}}}}"#,
            serde_json::to_string(&ws.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    home
}

async fn start(home: &Path) -> (ProxyHandle, Arc<str>) {
    let token: Arc<str> = generate_token().into();
    let (_tx, rx) = watch::channel(None);
    let h = spawn_proxy(
        token.clone(),
        rx,
        home.to_path_buf(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    (h, token)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy() // 系统代理下访问回环必须直连
        .build()
        .unwrap()
}

/// rel 查询参数手工拼 URL，非 ASCII 必须百分号编码（测试内手轮，不引依赖）
fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// mobile.js/css 的注入钩子锚定：改断任一处，"项目"标签静默消失
#[test]
fn mobile_injection_anchors_present() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("remote");
    let js = std::fs::read_to_string(dir.join("mobile.js")).unwrap();
    for needle in [
        "data-dshmobile-project-tab",
        "data-dshmobile-project-open",
        "/__dsh-desktop/project",
        "'项目'",
    ] {
        assert!(js.contains(needle), "mobile.js 缺 {needle}");
    }
    // 标签顺序：项目插在信息之前（insertBefore 信息按钮）
    assert!(
        js.contains("insertBefore"),
        "mobile.js 须用 insertBefore 保证项目标签在信息之前"
    );
    let css = std::fs::read_to_string(dir.join("mobile.css")).unwrap();
    for needle in ["[data-dshmobile-project-tab]", "[data-dshmobile-project-open]"] {
        assert!(css.contains(needle), "mobile.css 缺 {needle}");
    }
}

/// project.html 与服务端路由常量/localStorage 键的同源性：任一处漂移，
/// "项目"面板取不到数据（空态）或取不到当前会话
#[test]
fn project_page_anchors_and_constants() {
    use dshdesktop_lib::remote::project as pj;
    let html = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("remote")
            .join("project.html"),
    )
    .unwrap();
    assert!(html.contains("dshdesktop-project"), "页面缺内嵌标记");
    // 页面按 '<前缀>/<kind>' 拼 API（const API = '/__dsh-desktop/api'）：
    // 前缀与三个 kind 都必须与服务端路由常量一致
    let prefix = "/__dsh-desktop/api";
    assert!(html.contains(prefix), "project.html 缺 API 前缀 {prefix}");
    for (kind, path) in [
        ("resolve", pj::API_RESOLVE_PATH),
        ("list", pj::API_LIST_PATH),
        ("file", pj::API_FILE_PATH),
    ] {
        assert_eq!(
            path,
            format!("{prefix}/{kind}"),
            "服务端路由 {path} 与 project.html 的拼法漂移"
        );
        assert!(
            html.contains(&format!("api('{kind}'")),
            "project.html 未调用 api('{kind}'…)（预览/列表能力退化）"
        );
    }
    assert!(
        html.contains(dshdesktop_lib::upstream::LOCALSTORAGE_CURRENT_SESSION_KEY),
        "project.html 须用 upstream::LOCALSTORAGE_CURRENT_SESSION_KEY 取当前会话"
    );
    // 渲染器锚点：md 渲染排版 / md 相对路径图片拼 file 端点
    for f in ["renderMd", "joinRel"] {
        assert!(html.contains(f), "project.html 缺 {f}（md 预览能力退化）");
    }
}

/// Session log 药丸缩小规则锚定：选择器/关键属性/断点位置，
/// 断任一处手机上的药丸回原生 32px 高、重新贴近标签栏
#[test]
fn session_log_pill_shrink_rule() {
    let css = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("remote")
            .join("mobile.css"),
    )
    .unwrap();
    // 选择器锚 = "_" + 上游 CSS Modules 本地名（upstream.rs 常量，契约套件守门）
    let anchor = format!(
        "[class*=\"_{}\"]",
        dshdesktop_lib::upstream::SESSION_LOG_BUTTON_NEEDLE
    );
    // 必须双写凑 0-2-0 优先级：上游样式由 JS 运行时注入、文档序在我们之后，
    // 单写平级必输（改断的表现是规则在页面里但计算样式仍是 32px）
    let doubled = format!("{anchor}{anchor}");
    let pos = css.find(&doubled).unwrap_or_else(|| {
        panic!("mobile.css 缺双写选择器 {doubled}（单写优先级压不过上游运行时注入样式）")
    });
    let end = css[pos..].find('}').map(|i| pos + i).unwrap();
    let block = &css[pos..end];
    // 上游把 height:32px/min-width:111px 写死在按钮 CSS 里，必须显式覆盖这两项
    for prop in ["height: 26px", "min-width: 0", "font-size: 12px"] {
        assert!(block.contains(prop), "Session log 规则块缺 {prop}");
    }
    // 规则须在 700px 移动端断点内（桌面与宽屏远程保持原生尺寸）
    let media = css.find("@media (max-width: 700px)").unwrap();
    assert!(pos > media, "Session log 规则须落在 700px 断点内");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_endpoints_end_to_end() {
    let home = make_home();
    let (h, token) = start(home.path()).await;
    let base = format!("http://127.0.0.1:{}", h.port);
    let cookie = format!("{COOKIE_NAME}={token}");

    // 项目页
    let r = client()
        .get(format!("{base}/__dsh-desktop/project"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(body.contains("dshdesktop-project"), "页面须含内嵌标记");

    // resolve：命中 / 未命中
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/resolve?sid=session-aaa"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let j: serde_json::Value = r.json().await.unwrap();
    assert_eq!(j["title"], "A 项目");
    assert!(j["root"].as_str().unwrap().replace('/', "\\").contains("ws-a"));
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/resolve?sid=session-zzz"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // list：根目录（目录优先）/ 子目录 / 逃逸 403 / 缺失 404
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/list?sid=session-aaa&rel="))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let j: serde_json::Value = r.json().await.unwrap();
    let entries = j["entries"].as_array().unwrap();
    assert_eq!(entries[0]["kind"], "dir");
    assert_eq!(entries[0]["name"], "src");
    assert!(entries.iter().any(|e| e["name"] == "README.md"));
    // 点开头隐藏条目默认列出（与 pickerpatch 的"默认显示隐藏文件"语义一致）
    assert!(
        entries.iter().any(|e| e["name"] == ".hidden.txt"),
        "点开头条目须默认列出"
    );
    // 条目带字节数与类型——前端列表按它显示 1.2K 等尺寸、按 kind 分图标
    let md = entries.iter().find(|e| e["name"] == "README.md").unwrap();
    assert_eq!(md["kind"], "file");
    assert_eq!(
        md["size"].as_u64().unwrap(),
        "# 你好\n\n正文**加粗**。".len() as u64
    );
    assert_eq!(j["truncated"], false);
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/list?sid=session-aaa&rel=src"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    for bad in ["..", "../..", "/abs", "C:/Windows"] {
        let r = client()
            .get(format!("{base}/__dsh-desktop/api/list?sid=session-aaa&rel={bad}"))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "rel={bad} 必须 403");
    }
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/list?sid=session-aaa&rel=nope"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // file：md 文本 / 图片 MIME / no-store / 413 / download 头
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/file?sid=session-aaa&rel=README.md"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/markdown; charset=utf-8");
    assert_eq!(r.headers()["cache-control"], "no-store");
    assert_eq!(r.text().await.unwrap(), "# 你好\n\n正文**加粗**。");
    let r = client()
        .get(format!(
            "{base}/__dsh-desktop/api/file?sid=session-aaa&rel={}",
            urlencoding("图 1.png")
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["content-type"], "image/png");
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/file?sid=session-aaa&rel=big.txt"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"],
        "too_large"
    );
    // 同一文件带 download 放行（≤64MB）：预览 cap 不限制下载通道
    let r = client()
        .get(format!(
            "{base}/__dsh-desktop/api/file?sid=session-aaa&rel=big.txt&download=1"
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    assert_eq!(r.bytes().await.unwrap().len(), 9 * 1024 * 1024);
    // 目录当文件请求 → 404
    let r = client()
        .get(format!(
            "{base}/__dsh-desktop/api/file?sid=session-aaa&rel=src"
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let r = client()
        .get(format!(
            "{base}/__dsh-desktop/api/file?sid=session-aaa&rel={}&download=1",
            urlencoding("图 1.png")
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let cd = r.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        cd.starts_with("attachment; filename*=UTF-8''%E5%9B%BE%201.png"),
        "实际 {cd}"
    );
    // file 的逃逸同样 403
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/file?sid=session-aaa&rel=.."))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // 门岗：无 cookie 一律 403；?token= 播种 302 在壳路由同样生效
    let r = client()
        .get(format!("{base}/__dsh-desktop/api/resolve?sid=session-aaa"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = client()
        .get(format!("{base}/__dsh-desktop/project?token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 302);
    assert!(r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .contains(COOKIE_NAME));
    assert_eq!(r.headers()["location"], "/__dsh-desktop/project");

    h.shutdown().await;
}
