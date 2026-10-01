use crate::features::settings::{SettingsState, ShellSettings};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

/// WebView2 ZoomFactor 有效区间内的安全边界
pub const ZOOM_MIN: f64 = 0.25;
pub const ZOOM_MAX: f64 = 5.0;
const FILE_NAME: &str = "ui-zoom.txt";

fn clamp_zoom(v: f64) -> f64 {
    if v.is_nan() {
        1.0
    } else {
        v.clamp(ZOOM_MIN, ZOOM_MAX)
    }
}

fn file_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// 读取持久化缩放；文件缺失/损坏时回退 1.0（越界值走 clamp）
fn load(dir: &Path) -> f64 {
    std::fs::read_to_string(file_path(dir))
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(clamp_zoom)
        .unwrap_or(1.0)
}

/// 写盘失败不影响缩放本身（缩放是易失便利功能，下次启动回退 100% 可接受）
fn save(dir: &Path, v: f64) {
    let _ = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(file_path(dir), format!("{v:.4}")));
}

pub struct ZoomState {
    dir: PathBuf,
    value: Mutex<f64>,
}

impl ZoomState {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            value: Mutex::new(load(&dir)),
            dir,
        }
    }

    pub fn get(&self) -> f64 {
        *self.value.lock().unwrap()
    }

    pub fn adjust(&self, delta: f64) -> f64 {
        let mut g = self.value.lock().unwrap();
        *g = clamp_zoom(*g + delta);
        save(&self.dir, *g);
        *g
    }

    pub fn set(&self, v: f64) -> f64 {
        let mut g = self.value.lock().unwrap();
        *g = clamp_zoom(v);
        save(&self.dir, *g);
        *g
    }
}

/// 把缩放值应用到主窗口 webview（窗口还没建好时令其静默失败，
/// 首帧 on_page_load 会统一补应用）
pub fn apply_to_main(app: &AppHandle, v: f64) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_zoom(v);
    }
}

#[tauri::command]
pub fn zoom_ui(
    app: AppHandle,
    state: State<ZoomState>,
    settings: State<SettingsState>,
    direction: &str,
) -> Result<(), String> {
    let step = settings.get().zoom_step;
    let delta = match direction {
        "in" => step,
        "out" => -step,
        other => {
            return Err(crate::i18n::pick(
                format!("未知缩放方向: {other}"),
                format!("Unknown zoom direction: {other}"),
            ))
        }
    };
    let v = state.adjust(delta);
    apply_to_main(&app, v);
    Ok(())
}

/// 生成注入每个页面（本地 splash 与远程 dsh UI 通用）的快捷键钩子脚本。
/// 快捷键定义随当前设置内嵌为 JSON；匹配逻辑与 Shortcut::matches 对齐——
/// code 为主（真实键盘），key 兜底（合成按键/RDP 注入时 code 为空），meta 永不命中。
/// 步进不写死在脚本里：zoom_ui 命令在调用时读取设置，改步进无需重注入。
/// 改快捷键需重注入：监听器可热替换（__dshZoomHookHandler 存旧 handler，
/// 重复注入时先 removeEventListener 再挂新的，不会叠加）。
/// capture 阶段拦截并阻止冒泡，避免页面自身处理器重复响应。
pub fn hook_js(settings: &ShellSettings) -> String {
    let cfg = serde_json::json!({
        "in": settings.zoom_in,
        "out": settings.zoom_out,
    });
    HOOK_TEMPLATE.replace("__ZOOM_CFG__", &cfg.to_string())
}

const HOOK_TEMPLATE: &str = r#"(() => {
  const cfg = __ZOOM_CFG__;
  const match = (sc, e) => {
    if (e.metaKey) return false;
    if (!!e.ctrlKey !== sc.ctrl || !!e.shiftKey !== sc.shift || !!e.altKey !== sc.alt) return false;
    return (e.code && e.code === sc.code) || (e.key && e.key === sc.key);
  };
  const handler = (e) => {
    const direction = match(cfg.in, e) ? 'in' : match(cfg.out, e) ? 'out' : null;
    if (!direction) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    const t = window.__TAURI__;
    const invoke = t && t.core && t.core.invoke
      ? t.core.invoke.bind(t.core)
      : window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
    if (invoke) invoke('zoom_ui', { direction });
  };
  if (window.__dshZoomHookHandler) {
    window.removeEventListener('keydown', window.__dshZoomHookHandler, true);
  }
  window.__dshZoomHookHandler = handler;
  window.addEventListener('keydown', handler, true);
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::settings::Shortcut;

    #[test]
    fn clamp_bounds_and_nan() {
        assert_eq!(clamp_zoom(0.01), ZOOM_MIN);
        assert_eq!(clamp_zoom(99.0), ZOOM_MAX);
        assert_eq!(clamp_zoom(f64::NAN), 1.0);
    }

    #[test]
    fn adjust_accumulates_and_clamps() {
        let dir = tempfile::tempdir().unwrap();
        let s = ZoomState::new(dir.path().to_path_buf());
        assert_eq!(s.get(), 1.0);
        let v = s.adjust(0.02);
        assert!((v - 1.02).abs() < 1e-9);
        assert!((s.adjust(0.02) - 1.04).abs() < 1e-9);
        assert_eq!(s.adjust(-10.0), ZOOM_MIN); // 大步越界被 clamp
        assert_eq!(s.get(), ZOOM_MIN);
    }

    #[test]
    fn set_persists_and_loads_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = ZoomState::new(dir.path().to_path_buf());
        s.set(1.04);
        let s2 = ZoomState::new(dir.path().to_path_buf());
        assert!((s2.get() - 1.04).abs() < 1e-9);
    }

    #[test]
    fn load_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        // 文件不存在 → 1.0
        assert_eq!(load(dir.path()), 1.0);
        // 垃圾内容 → 1.0
        std::fs::write(dir.path().join("ui-zoom.txt"), "abc").unwrap();
        assert_eq!(load(dir.path()), 1.0);
        // 越界值 → clamp
        std::fs::write(dir.path().join("ui-zoom.txt"), "42").unwrap();
        assert_eq!(load(dir.path()), ZOOM_MAX);
    }

    #[test]
    fn hook_js_embeds_default_shortcuts() {
        let js = hook_js(&ShellSettings::default());
        assert!(js.contains("Equal") && js.contains("+"));
        assert!(js.contains("Minus") && js.contains("_"));
        assert!(js.contains("zoom_ui"));
        // 热替换：重复注入时先摘旧监听器再挂新的（设置保存后立即生效）
        assert!(js.contains("removeEventListener") && js.contains("__dshZoomHookHandler"));
        // 方向负载：步进不写死在脚本里，由命令在调用时从设置读取
        assert!(js.contains("'in'") && js.contains("'out'"));
        assert!(js.contains("direction"));
    }

    #[test]
    fn hook_js_embeds_custom_shortcuts() {
        let mut s = ShellSettings::default();
        s.zoom_in = Shortcut { ctrl: true, shift: false, alt: true, code: "KeyQ".into(), key: "q".into() };
        let js = hook_js(&s);
        assert!(js.contains("KeyQ") && js.contains(r#""ctrl":true"#));
        assert!(!js.contains("Equal"));
    }
}
