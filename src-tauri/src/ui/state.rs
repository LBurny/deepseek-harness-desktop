//! 壳界面状态：主题/语言解析与快照（本地页面与托盘读取）。

use std::path::Path;

use tauri::Theme;

use crate::platform::Platform;

/// 壳界面状态快照：解析后的主题（dark/light）与语言（zh/en）。
/// 经 `get_shell_ui_state` 命令与 `shell-ui-state` 事件同步给本地页面。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UiSnapshot {
    pub theme: String,
    pub locale: String,
}

pub struct ShellUiState(pub std::sync::Mutex<UiSnapshot>);

impl ShellUiState {
    /// 启动时的初始解析：profile patch 可能尚不存在（首启），
    /// 此时按系统解析；关注循环 2s 后会再校正并广播。
    pub fn new(platform: &dyn Platform, dsh_home: &Path) -> Self {
        let locale = resolve_locale(platform, dsh_home);
        // 立即写入全局语言：托盘菜单/窗口标题/通知等先按解析值渲染，
        // 首轮轮询（force 同步）再校正——若设置里是 en，等 2s 后才英文也可接受
        crate::i18n::set_locale(&locale);
        Self(std::sync::Mutex::new(UiSnapshot {
            theme: theme_name(resolve(platform, dsh_home)).to_string(),
            locale,
        }))
    }

    pub fn get(&self) -> UiSnapshot {
        self.0.lock().unwrap().clone()
    }
}

/// 本地页面的主题/语言快照：页面加载即取，之后靠 shell-ui-state 事件增量更新
#[tauri::command]
pub fn get_shell_ui_state(state: tauri::State<ShellUiState>) -> UiSnapshot {
    state.get()
}

pub(crate) fn theme_name(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        _ => "light",
    }
}

pub(crate) fn system_theme(platform: &dyn Platform) -> Theme {
    if platform.system_dark_mode() {
        Theme::Dark
    } else {
        Theme::Light
    }
}

/// 首启播种已退役（0.2.0 跟版）：dsh-client-ui-theme 的 DEFAULT_PREFERENCE 自
/// 0.2.0 起为 "system"，与壳的缺省解析一致——"深标题栏 + 浅内容"的首启不一致
/// 根因消失。且 settings.yaml 已变成一次性遗留导入通道（每次启动被 dsh 改名
/// 导入），往里播种会在每次启动把旧值灌回 profile、覆盖用户现选主题（0.5.19
/// 实锤"深色模式启动后 UI 未跟着变深色"），壳不再写它。

pub(crate) fn resolve(platform: &dyn Platform, home: &Path) -> Theme {
    match home_preference(home).as_deref() {
        Some("dark") => Theme::Dark,
        Some("light") => Theme::Light,
        // "system"（0.2.0 schema 缺省值）、缺条目、缺文件 → 跟随系统
        _ => system_theme(platform),
    }
}

/// dsh 语言：locale.preference ∈ {zh,en}；缺省跟随系统 UI 语言
/// （dsh 侧缺省是"跟随浏览器"，WebView2 的浏览器语言同样来自系统）。
pub(crate) fn resolve_locale(platform: &dyn Platform, home: &Path) -> String {
    match home_str(home, crate::dsh::upstream::KEY_LOCALE, crate::dsh::upstream::KEY_PREFERENCE).as_deref() {
        Some("en") => "en".to_string(),
        Some("zh") => "zh".to_string(),
        _ => {
            if platform.system_prefers_chinese() {
                "zh".to_string()
            } else {
                "en".to_string()
            }
        }
    }
}

/// profile patch 条目的 preference 字段（ui-theme/locale；详见 upstream.rs
/// 设置存储段注）。读失败/缺条目一律 None → 调用方按缺省（system）兜底。
fn home_preference(home: &Path) -> Option<String> {
    home_str(home, crate::dsh::upstream::KEY_UI_THEME, crate::dsh::upstream::KEY_PREFERENCE)
}

fn home_str(home: &Path, entry_id: &str, field: &str) -> Option<String> {
    crate::patchstore::settings_entry_str(home, entry_id, field)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 profile patch：可选插入 ui-theme/locale 直排条目 + 一行 insert-op
    /// （模拟 mcp 播种行，验证两类行共存时读取不受干扰）。
    fn write_patch(home: &Path, extra_rows: &str) {
        let dir = home.join("profiles").join("web");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("cordis.patch.yml"),
            format!("- insert:\n    - id: mcp-client\n      name: '@deepseek-ai/dsh-mcp-client'\n{extra_rows}"),
        )
        .unwrap();
    }

    #[test]
    fn reads_theme_preference_from_profile_patch() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // 缺文件 → None
        assert_eq!(home_preference(home), None);
        // 有 patch 但无 ui-theme 条目 → None
        write_patch(home, "");
        assert_eq!(home_preference(home), None);
        // ui-theme 条目（dsh config-editor 写形：id + name + config）
        write_patch(
            home,
            "- id: ui-theme\n  name: '@deepseek-ai/dsh-client-ui-theme'\n  config:\n    preference: dark\n    fontSize: 15\n",
        );
        assert_eq!(home_preference(home).as_deref(), Some("dark"));
        // 同条目其它键（fontSize）与其余条目（insert-op）不受影响——读不写，此处只验证读
        std::fs::write(
            home.join("profiles").join("web").join("cordis.patch.yml"),
            "- insert:\n    - id: mcp-client\n      name: '@deepseek-ai/dsh-mcp-client'\n- id: ui-theme\n  name: '@deepseek-ai/dsh-client-ui-theme'\n  config:\n    preference: light\n",
        )
        .unwrap();
        assert_eq!(home_preference(home).as_deref(), Some("light"));
    }

    #[test]
    fn reads_theme_preference_with_bom() {
        // dsh/壳写的是无 BOM UTF-8，但外部编辑工具（PowerShell utf8）会加 BOM；
        // read_patch 统一剥除
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let dir2 = home.join("profiles").join("web");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(
            dir2.join("cordis.patch.yml"),
            "\u{FEFF}- id: ui-theme\n  name: x\n  config:\n    preference: dark\n",
        )
        .unwrap();
        assert_eq!(home_preference(home).as_deref(), Some("dark"));
    }

    #[test]
    fn corrupt_patch_is_none() {
        // 损坏文件：读偏好返回 None（不 panic、不部分解析）
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let dir2 = home.join("profiles").join("web");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join("cordis.patch.yml"), ":\n  - [unclosed").unwrap();
        assert_eq!(home_preference(home), None);
    }

    #[test]
    fn reads_locale_preference() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert_eq!(
            home_str(home, crate::dsh::upstream::KEY_LOCALE, crate::dsh::upstream::KEY_PREFERENCE),
            None
        );
        write_patch(
            home,
            "- id: locale\n  name: '@deepseek-ai/dsh-client-locale'\n  config:\n    preference: en\n",
        );
        assert_eq!(
            home_str(home, crate::dsh::upstream::KEY_LOCALE, crate::dsh::upstream::KEY_PREFERENCE).as_deref(),
            Some("en")
        );
    }
}
