//! 页面错误桥 + UI 启动心跳（0.5.11）：主窗口导航进 dsh UI 后，壳对页面内部是
//! "黑盒"——进程 Ready ≠ UI 活着。431 故障实锤（2026-09-09）：dsh 每进程
//! Set-Cookie 新名 dsh-auth-<hash>（30 天），WebView2 cookie 罐只进不出攒满后
//! Cookie 头 + 插件 bundle 组合 URL 超 Node 16KB 请求头上限 → dsh 回 431 → 页面里
//! bundle <script> 加载失败，主窗口 "Failed to load plugins"，而壳侧日志只有
//! "Ready" 一行，定位成本全花在"进程好了但窗口里是什么"上。三件事：
//! - INIT_SCRIPT：document-start 注入（initialization_script），捕获页面错误经
//!   invoke 落 events.log（限流 + 800 字符截断 + token 脱敏，只写不读）
//! - 心跳：#root 挂载成功上报 ui_boot_ok；Ready 导航 30s 后仍无心跳 → lib.rs 的
//!   看门狗自愈一次（清 dsh-auth cookie + 带 token 重导航，one-shot 不自旋）
//! - 两条命令只往 events.log 单一 sink 追加行，不回传前端、不读任何数据——这是
//!   远程源（dsh-remote.json）放行它们的安全前提

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::State;

/// document-start 注入脚本（挂 WebviewWindowBuilder::initialization_script，每次导航
/// 都跑、先于页面脚本、绕页面 CSP）。invoke 句柄惰性解析（zoom.rs 同款
/// __TAURI__ → __TAURI_INTERNALS__ 回退）：document-start 时刻 IPC 桥未必就绪，
/// 真出错/心跳时早已可用。
pub(crate) const INIT_SCRIPT: &str = r#"(() => {
  if (window.__dshPageBridge) return;
  window.__dshPageBridge = true;
  const tauriInvoke = () => {
    const t = window.__TAURI__;
    return (t && t.core && t.core.invoke)
      ? t.core.invoke.bind(t.core)
      : window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
  };
  const send = (kind, msg) => {
    try {
      const invoke = tauriInvoke();
      if (invoke) invoke('report_page_error', { kind, message: String(msg).slice(0, 800) });
    } catch (_) {}
  };
  const describe = (v) => {
    if (typeof v === 'string') return v;
    if (v && (v.stack || v.message)) return v.stack || v.message;
    try { return JSON.stringify(v); } catch (_) { return String(v); }
  };
  // 捕获相才接得到资源加载失败（script/link 的 error 事件不冒泡）——431 打死的
  // bundle <script> 走这条，带完整组合 URL，是定位关键证据
  window.addEventListener('error', (e) => {
    const t = e.target;
    if (t && t !== window && (t.src || t.href)) {
      send('resource', (t.tagName || '?') + ' ' + (t.src || t.href));
    } else if (e.message) {
      send('error', e.message + (e.filename ? ' @ ' + e.filename + ':' + e.lineno : ''));
    }
  }, true);
  window.addEventListener('unhandledrejection', (e) => send('unhandledrejection', describe(e.reason)));
  const origError = console.error.bind(console);
  console.error = (...args) => {
    send('console.error', args.map(describe).join(' '));
    origError(...args);
  };
  // UI 启动心跳：只在 dsh UI 源探 #root 挂载——本地 splash 页（tauri.localhost）
  // 也有 #root，不按源隔离会秒报假心跳。90s 封顶停止轮询
  if (location.hostname === '127.0.0.1') {
    const t0 = Date.now();
    const timer = setInterval(() => {
      if (Date.now() - t0 > 90000) { clearInterval(timer); return; }
      const root = document.getElementById('root');
      if (root && root.childNodes.length > 0) {
        clearInterval(timer);
        try {
          const invoke = tauriInvoke();
          if (invoke) invoke('ui_boot_ok', { elapsedMs: Date.now() - t0 });
        } catch (_) {}
      }
    }, 500);
  }
})();
"#;

static NAV_MS: AtomicU64 = AtomicU64::new(0); // 最近一次主窗口导航的墙钟毫秒
static BOOT_MS: AtomicU64 = AtomicU64::new(0); // 最近一次 ui_boot_ok 上报的墙钟毫秒

/// Ready 导航落点记录（看门狗基准），返回记录的毫秒值
pub(crate) fn note_navigation() -> u64 {
    let ms = now_millis();
    NAV_MS.store(ms, Ordering::SeqCst);
    ms
}

/// 最近一次导航的毫秒值：看门狗用它认"我是不是最新一轮导航的看门狗"——
/// dsh 在 30s 窗口内重启会产生两只看门狗，旧的发现 NAV_MS 已换即让位退出，
/// 否则旧狗会拿上一进程的 stale token 把正在加载的新页面导航去 401
pub(crate) fn nav_millis() -> u64 {
    NAV_MS.load(Ordering::SeqCst)
}

pub(crate) fn boot_millis() -> u64 {
    BOOT_MS.load(Ordering::SeqCst)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 页面错误限流：每自然分钟最多 60 行（431 那种一次事故 ~45 模块报错也装得下），
/// 满额当分钟落一行抑制提示后丢弃——防死循环页面把 events.log 刷爆
const PAGE_ERR_CAP_PER_MIN: u32 = 60;
const PAGE_MSG_MAX_CHARS: usize = 800;

#[derive(Debug, PartialEq, Eq)]
enum Allow {
    Line,
    CapNotice,
    Drop,
}

struct MinuteBucket {
    minute: u64,
    count: u32,
}

impl MinuteBucket {
    fn allow(&mut self, minute: u64) -> Allow {
        if self.minute != minute {
            *self = MinuteBucket { minute, count: 0 };
        }
        self.count += 1;
        if self.count > PAGE_ERR_CAP_PER_MIN {
            Allow::Drop
        } else if self.count == PAGE_ERR_CAP_PER_MIN {
            Allow::CapNotice
        } else {
            Allow::Line
        }
    }
}

static PAGE_ERR_BUCKET: Mutex<MinuteBucket> = Mutex::new(MinuteBucket { minute: 0, count: 0 });

#[tauri::command]
pub fn report_page_error(
    state: State<'_, Arc<dyn crate::platform::Platform>>,
    kind: String,
    message: String,
) {
    // 不用 SharedState：首启部署阶段页面已加载而 SharedState 还没 manage，
    // Platform 单例从第一个 setup 语句起就可用
    let log = state.runtime_base_dir().join("events.log");
    match PAGE_ERR_BUCKET.lock().unwrap().allow(now_millis() / 60_000) {
        Allow::Drop => {}
        Allow::CapNotice => crate::append_debug_line(
            &log,
            "[page] page error rate cap reached; suppressed until next minute",
        ),
        Allow::Line => crate::append_debug_line(&log, &format_page_err_line(&kind, &message)),
    }
}

#[tauri::command]
pub fn ui_boot_ok(state: State<'_, Arc<dyn crate::platform::Platform>>, elapsed_ms: u64) {
    BOOT_MS.store(now_millis(), Ordering::SeqCst);
    // JS 侧数值不可信，钳到 10 分钟内
    let secs = elapsed_ms.min(600_000) as f64 / 1000.0;
    crate::append_debug_line(
        &state.runtime_base_dir().join("events.log"),
        &format!("[dshdesktop] dsh UI booted ({secs:.1}s)"),
    );
}

/// 一行页面错误：脱敏 → 压平换行 → 按 char 截断。纯函数便于单测。
fn format_page_err_line(kind: &str, message: &str) -> String {
    // 页面 URL 带 ?token=，e.filename / 资源 src 会原样把 token 带出——
    // 落盘前先脱敏（与 remote 透传日志同一脱敏器）
    let msg = crate::remote::redact_token(message);
    // 压平换行防日志注入成多行（多出来的行绕过时间戳/脱敏管线）
    let flat = flatten_newlines(&msg);
    let message: String = flat.chars().take(PAGE_MSG_MAX_CHARS).collect();
    let kind: String = flatten_newlines(kind).chars().take(24).collect();
    format!("[page:{kind}] {message}")
}

/// \r\n / \r / \n 统一压成空格
fn flatten_newlines(s: &str) -> String {
    s.replace("\r\n", " ").replace('\r', " ").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minute_bucket_line_then_cap_notice_then_drop() {
        let mut b = MinuteBucket { minute: 0, count: 0 };
        // 同一分钟：前 59 次放行，第 60 次落抑制提示，第 61 次起丢弃
        for _ in 0..PAGE_ERR_CAP_PER_MIN - 1 {
            assert_eq!(b.allow(100), Allow::Line);
        }
        assert_eq!(b.allow(100), Allow::CapNotice);
        assert_eq!(b.allow(100), Allow::Drop);
        assert_eq!(b.allow(100), Allow::Drop);
        // 换分钟重置
        assert_eq!(b.allow(101), Allow::Line);
    }

    #[test]
    fn page_err_line_redacts_token() {
        let line = format_page_err_line("resource", "SCRIPT http://127.0.0.1:1234/?token=secret123 @ 1:1");
        assert!(line.contains("token=<redacted>"), "实际：{line}");
        assert!(!line.contains("secret123"), "实际：{line}");
        assert!(line.starts_with("[page:resource] "), "实际：{line}");
    }

    #[test]
    fn page_err_line_flattens_newlines() {
        // kind 与 message 都要压平，日志一行一条不可被拆行
        let line = format_page_err_line("con\nsole", "a\r\nb\rc\nd");
        assert!(!line.contains('\n') && !line.contains('\r'), "实际：{line}");
        assert!(line.starts_with("[page:con sole] "), "实际：{line}");
    }

    #[test]
    fn page_err_line_truncates_multibyte_safely() {
        // 900 个多字节字符：按 char 截断不 panic、不切半个字（无 U+FFFD）
        let msg = "字".repeat(900);
        let line = format_page_err_line("console.error", &msg);
        let prefix_len = "[page:console.error] ".chars().count();
        assert_eq!(line.chars().count(), prefix_len + PAGE_MSG_MAX_CHARS);
        assert!(!line.contains('\u{FFFD}'), "多字节被切半，实际：{line}");
    }

    #[test]
    fn init_script_anchors() {
        // 锚定关键要素，防手改脚本时静默丢能力
        assert!(INIT_SCRIPT.contains("report_page_error"));
        assert!(INIT_SCRIPT.contains("ui_boot_ok"));
        assert!(INIT_SCRIPT.contains("127.0.0.1"));
        assert!(INIT_SCRIPT.contains("unhandledrejection"));
        assert!(INIT_SCRIPT.contains("console.error"));
        assert!(INIT_SCRIPT.contains("__TAURI_INTERNALS__"));
        assert!(INIT_SCRIPT.contains("getElementById('root')"));
        assert!(INIT_SCRIPT.contains("__dshPageBridge"));
    }
}