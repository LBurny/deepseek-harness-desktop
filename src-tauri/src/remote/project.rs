//! 远程"项目"标签的后端：会话→工作区解析、目录列举、文件读取（一律只读）。
//! 路由挂在 proxy.rs（/__dsh-desktop/*），token 门岗中间件先行。
//! 两个上游事实（localStorage 键名、workspace.json 路径与 schema）收口
//! crate::upstream，契约套件守门。

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use super::proxy::ProxyState;

pub const PAGE_PATH: &str = "/__dsh-desktop/project";
pub const API_RESOLVE_PATH: &str = "/__dsh-desktop/api/resolve";
pub const API_LIST_PATH: &str = "/__dsh-desktop/api/list";
pub const API_FILE_PATH: &str = "/__dsh-desktop/api/file";

/// 自包含项目浏览单页（inline JS+CSS，零外部请求——隧道/离线都可用）
const PROJECT_HTML: &str = include_str!("project.html");

/// 每目录最多返回条目数（超出截断，客户端提示"仅显示前 N 条"）
const LIST_CAP: usize = 2000;
/// 文本类预览体积上限（非 download）：超出 413——md 渲染/行号构建都是
/// 主线程活，手机 DOM 几万行会卡死
const TEXT_PREVIEW_CAP: u64 = 8 * 1024 * 1024;
/// 整体读内存的硬上限（含 download）：零新依赖不引流式读，超大文件拒服
const FILE_CAP: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct WorkspaceRef {
    pub title: String,
    pub root: PathBuf,
}

#[derive(Deserialize)]
struct Store {
    tables: Option<Tables>,
}
#[derive(Deserialize)]
struct Tables {
    workspaces: Option<HashMap<String, Ws>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ws {
    path: String,
    title: Option<String>,
    session_ids: Option<Vec<String>>,
}

/// 每调用现读现解析 workspace.json（文件极小，工作区增删即时生效）；
/// 缺文件/坏 JSON/sid 未命中归一 None（与"无工作区"同语义，客户端空态）。
pub(crate) fn resolve_workspace(dsh_home: &Path, sid: &str) -> Option<WorkspaceRef> {
    let p = crate::upstream::join_segments(dsh_home, crate::upstream::WORKSPACE_STORE_SEGMENTS);
    let text = std::fs::read_to_string(p).ok()?;
    let store: Store = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let workspaces = store.tables?.workspaces?;
    workspaces.values().find_map(|ws| {
        if !ws.session_ids.as_ref()?.iter().any(|id| id == sid) {
            return None;
        }
        let root = PathBuf::from(&ws.path);
        // title 缺失时回退目录名，再退完整路径（工作区在盘符根时 file_name 为 None）
        let title = ws
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ws.path.clone());
        Some(WorkspaceRef { title, root })
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    Forbidden,
    Permission,
    TooLarge(u64),
    Io(String),
}

/// rel 只允许 Normal 组件（拒绝盘符/根/.. /.)；拼接后 canonicalize 且必须
/// 仍在 canonical(root) 内——junction/符号链接逃逸被解析后的前缀检查兜住。
fn contained(root: &Path, rel: &str) -> Result<PathBuf, FsError> {
    if Path::new(rel)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(FsError::Forbidden);
    }
    let root_c = std::fs::canonicalize(root).map_err(map_io)?;
    let cand_c = std::fs::canonicalize(root.join(rel)).map_err(map_io)?;
    if is_within(&root_c, &cand_c) {
        Ok(cand_c)
    } else {
        Err(FsError::Forbidden)
    }
}

/// canonical 路径包含判定：剥 \\?\ verbatim（两侧），小写化（Windows 不敏感），
/// 等值或带分隔符边界的前缀（"0107-evil" 不能命中 "0107"）。
fn is_within(root_c: &Path, cand_c: &Path) -> bool {
    let norm = |p: &Path| {
        crate::runtime::strip_verbatim(p)
            .to_string_lossy()
            .to_lowercase()
    };
    let (r, c) = (norm(root_c), norm(cand_c));
    c == r || c.starts_with(&format!("{r}{}", std::path::MAIN_SEPARATOR))
}

fn map_io(e: std::io::Error) -> FsError {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => FsError::NotFound,
        PermissionDenied => FsError::Permission,
        _ => FsError::Io(e.to_string()),
    }
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Entry {
    name: String,
    kind: &'static str, // "dir" | "file"
    size: u64,
    mtime: u64, // unix 秒；取不到为 0
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Listing {
    entries: Vec<Entry>,
    truncated: bool,
}

/// 目录优先、名称（小写）排序；单条目的 IO/元数据失败只跳过该条——
/// fs-local 的教训：ACL 拒绝项不得拖垮整列。
fn list_dir(root: &Path, rel: &str, cap: usize) -> Result<Listing, FsError> {
    let dir = contained(root, rel)?;
    if !std::fs::metadata(&dir).map_err(map_io)?.is_dir() {
        return Err(FsError::NotFound); // rel 指到文件：客户端传参错误
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for item in std::fs::read_dir(&dir).map_err(map_io)? {
        if entries.len() >= cap {
            truncated = true;
            break;
        }
        let Ok(item) = item else { continue };
        let Ok(md) = item.metadata() else { continue };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        entries.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            kind: if md.is_dir() { "dir" } else { "file" },
            size: md.len(),
            mtime,
        });
    }
    entries.sort_by(|a, b| {
        (a.kind != "dir")
            .cmp(&(b.kind != "dir"))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(Listing { entries, truncated })
}

/// 预览 MIME 判定（扩展名小写匹配）。project.html 的 JS 侧有一份镜像表
/// （决定预览形态），两处同步维护。
fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase());
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("svg") => "image/svg+xml",
        Some("avif") => "image/avif",
        Some("pdf") => "application/pdf",
        Some("md" | "markdown") => "text/markdown; charset=utf-8",
        Some(
            "txt" | "log" | "json" | "jsonc" | "csv" | "tsv" | "yaml" | "yml" | "toml"
            | "xml" | "html" | "htm" | "css" | "js" | "jsx" | "ts" | "tsx" | "mjs"
            | "cjs" | "py" | "rs" | "go" | "java" | "c" | "h" | "cpp" | "hpp" | "cs"
            | "sh" | "ps1" | "bat" | "cmd" | "ini" | "cfg" | "conf" | "env" | "sql"
            | "vue" | "svelte" | "lock" | "editorconfig" | "gitignore" | "dockerfile",
        ) => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn is_textual(mime: &str) -> bool {
    mime.starts_with("text/")
}

/// 体积闸门：文本预览受 TEXT_PREVIEW_CAP（download 豁免），一切受 FILE_CAP。
fn check_size(mime: &str, size: u64, download: bool) -> Result<(), FsError> {
    if size > FILE_CAP || (!download && is_textual(mime) && size > TEXT_PREVIEW_CAP) {
        return Err(FsError::TooLarge(size));
    }
    Ok(())
}

/// RFC 5987 filename*：ASCII 白名单直通，其余 UTF-8 百分号编码（手轮，零新依赖）
fn content_disposition(name: &str) -> String {
    let mut enc = String::with_capacity(name.len());
    for &b in name.as_bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                enc.push(b as char)
            }
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    format!("attachment; filename*=UTF-8''{enc}")
}

// ── HTTP handlers（token 门岗中间件先行；客户端只能发 sid+相对路径）──────

#[derive(Deserialize)]
pub struct FsQuery {
    sid: String,
    rel: Option<String>,
    download: Option<String>,
}

pub(crate) async fn page() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        PROJECT_HTML,
    )
        .into_response()
}

pub(crate) async fn resolve(State(st): State<ProxyState>, Query(q): Query<FsQuery>) -> Response {
    match resolve_workspace(&st.dsh_home, &q.sid) {
        Some(ws) => Json(serde_json::json!({ "title": ws.title, "root": ws.root })).into_response(),
        None => err(StatusCode::NOT_FOUND, "not_found"),
    }
}

pub(crate) async fn list(State(st): State<ProxyState>, Query(q): Query<FsQuery>) -> Response {
    let Some(ws) = resolve_workspace(&st.dsh_home, &q.sid) else {
        return err(StatusCode::NOT_FOUND, "not_found");
    };
    match list_dir(&ws.root, q.rel.as_deref().unwrap_or(""), LIST_CAP) {
        Ok(l) => Json(l).into_response(),
        Err(e) => fs_err(e),
    }
}

pub(crate) async fn file(State(st): State<ProxyState>, Query(q): Query<FsQuery>) -> Response {
    let Some(ws) = resolve_workspace(&st.dsh_home, &q.sid) else {
        return err(StatusCode::NOT_FOUND, "not_found");
    };
    let path = match contained(&ws.root, q.rel.as_deref().unwrap_or("")) {
        Ok(p) => p,
        Err(e) => return fs_err(e),
    };
    let md = match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return err(StatusCode::NOT_FOUND, "not_found"), // 目录当文件请求
        Err(e) => return fs_err(map_io(e)),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mime = mime_for(&name);
    let download = q.download.is_some();
    if let Err(e) = check_size(mime, md.len(), download) {
        return fs_err(e);
    }
    // 64MB 上限内的整体读：阻塞最坏几十毫秒，仍放 blocking 池（与转发同 runtime）
    let bytes = match tokio::task::spawn_blocking(move || std::fs::read(path)).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => return fs_err(map_io(e)),
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "io"),
    };
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store");
    if download {
        builder = builder.header(header::CONTENT_DISPOSITION, content_disposition(&name));
    }
    builder
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn err(status: StatusCode, code: &str) -> Response {
    (status, Json(serde_json::json!({ "error": code }))).into_response()
}

fn fs_err(e: FsError) -> Response {
    match e {
        FsError::NotFound => err(StatusCode::NOT_FOUND, "not_found"),
        FsError::Forbidden => err(StatusCode::FORBIDDEN, "forbidden"),
        FsError::Permission => err(StatusCode::FORBIDDEN, "permission"),
        FsError::TooLarge(size) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({ "error": "too_large", "size": size })),
        )
            .into_response(),
        FsError::Io(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "io"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with_store() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let ws_a = home.path().join("ws-a");
        std::fs::create_dir_all(ws_a.join("src")).unwrap();
        std::fs::write(ws_a.join("README.md"), "# 标题").unwrap();
        std::fs::write(ws_a.join("src").join("main.rs"), "fn main() {}").unwrap();
        let store = home.path().join("storages");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            store.join("workspace.json"),
            format!(
                r#"{{"unit":{{"name":"workspace","version":2}},"tables":{{"workspaces":{{"w1":{{"path":{},"title":"A 项目","sessionIds":["session-aaa"]}}}}}}}}"#,
                serde_json::to_string(&ws_a.to_string_lossy()).unwrap()
            ),
        )
        .unwrap();
        home
    }

    #[test]
    fn resolve_hit_miss_bom_corrupt() {
        let home = home_with_store();
        let ws = resolve_workspace(home.path(), "session-aaa").unwrap();
        assert_eq!(ws.title, "A 项目");
        assert!(ws.root.ends_with("ws-a"));
        assert!(resolve_workspace(home.path(), "session-zzz").is_none());
        // BOM 容忍
        let p = home.path().join("storages").join("workspace.json");
        let raw = std::fs::read(&p).unwrap();
        let mut bom = b"\xef\xbb\xbf".to_vec();
        bom.extend_from_slice(&raw);
        std::fs::write(&p, &bom).unwrap();
        assert!(resolve_workspace(home.path(), "session-aaa").is_some());
        // 坏 JSON / 缺文件
        std::fs::write(&p, "not json").unwrap();
        assert!(resolve_workspace(home.path(), "session-aaa").is_none());
        std::fs::remove_file(&p).unwrap();
        assert!(resolve_workspace(home.path(), "session-aaa").is_none());
    }

    #[test]
    fn contained_rejects_escape_and_absolute() {
        let home = home_with_store();
        let root = home.path().join("ws-a");
        assert!(contained(&root, "").is_ok()); // 根自身
        assert!(contained(&root, "src").is_ok());
        assert_eq!(contained(&root, ".."), Err(FsError::Forbidden));
        assert_eq!(contained(&root, "src/../../x"), Err(FsError::Forbidden));
        assert_eq!(contained(&root, "/etc/passwd"), Err(FsError::Forbidden));
        assert_eq!(contained(&root, "C:/Windows"), Err(FsError::Forbidden));
        assert_eq!(contained(&root, "./src"), Err(FsError::Forbidden)); // 只允许 Normal 组件
        assert_eq!(contained(&root, "nope.txt"), Err(FsError::NotFound));
    }

    #[test]
    fn is_within_case_and_prefix_boundary() {
        // 纯函数：canonical 后大小写差异容忍；"0107-evil" 不算在 "0107" 内
        let r = Path::new(r"\\?\I:\TempItem\0107");
        assert!(is_within(r, Path::new(r"\\?\i:\tempitem\0107\src")));
        assert!(is_within(r, Path::new(r"I:\TempItem\0107")));
        assert!(!is_within(r, Path::new(r"I:\TempItem\0107-evil")));
        assert!(!is_within(r, Path::new(r"I:\TempItem")));
    }

    #[test]
    fn list_dir_sorts_dirs_first_and_caps() {
        let home = home_with_store();
        let root = home.path().join("ws-a");
        let l = list_dir(&root, "", 10).unwrap();
        assert_eq!(l.entries[0].kind, "dir"); // src 在 README.md 前
        assert_eq!(l.entries[0].name, "src");
        assert!(!l.truncated);
        let l = list_dir(&root, "", 1).unwrap();
        assert!(l.truncated && l.entries.len() == 1);
        assert_eq!(list_dir(&root, "..", 10), Err(FsError::Forbidden));
        assert_eq!(list_dir(&root, "README.md", 10), Err(FsError::NotFound)); // rel 指到文件
    }

    #[test]
    fn mime_and_disposition_and_size_gate() {
        assert_eq!(mime_for("a.PNG"), "image/png");
        assert_eq!(mime_for("b.md"), "text/markdown; charset=utf-8");
        assert_eq!(mime_for("c.TS"), "text/plain; charset=utf-8");
        assert_eq!(mime_for("d.exe"), "application/octet-stream");
        assert_eq!(mime_for("Makefile"), "application/octet-stream");
        assert_eq!(
            content_disposition("走势图 01.png"),
            "attachment; filename*=UTF-8''%E8%B5%B0%E5%8A%BF%E5%9B%BE%2001.png"
        );
        assert_eq!(
            content_disposition("a-b_c.txt"),
            "attachment; filename*=UTF-8''a-b_c.txt"
        );
        assert!(check_size("text/plain; charset=utf-8", TEXT_PREVIEW_CAP + 1, false).is_err());
        assert!(check_size("image/png", TEXT_PREVIEW_CAP + 1, false).is_ok()); // 图片不受文本 cap
        assert!(check_size("image/png", FILE_CAP + 1, false).is_err()); // 但受硬 cap
        assert!(check_size("text/plain; charset=utf-8", FILE_CAP + 1, true).is_err()); // download 也受限
        assert!(check_size("text/plain; charset=utf-8", TEXT_PREVIEW_CAP + 1, true).is_ok()); // download 豁免文本 cap
    }
}
