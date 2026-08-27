use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Manager;

pub const STEP_MIN: f64 = 0.01;
pub const STEP_MAX: f64 = 0.25;
const FILE_NAME: &str = "settings.json";

fn step_default() -> f64 {
    0.02
}

/// 快捷键：code 为主（物理键位，真实键盘），key 兜底（合成按键/RDP 注入时 code 为空）。
/// 两者在录制时同时从同一个 keydown 事件取得。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub code: String,
    pub key: String,
}

impl Shortcut {
    pub fn matches(&self, code: &str, key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        if self.ctrl != ctrl || self.shift != shift || self.alt != alt {
            return false;
        }
        (!code.is_empty() && code == self.code) || (!key.is_empty() && key == self.key)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CloseBehavior {
    Background,
    Quit,
}

/// 任务完成通知的提示音。Default 直接透传 toast 的音频预设
/// （tauri-winrt-notification Sound::from_str → ms-winsoundevent:Notification.Default，
/// Windows 系统内置，不依赖用户声音方案）；Silent = 不传 sound，toast 静音。
/// 其余 17 个是壳内置音效（resources/sounds/*.wav，音源 opencode）：
/// toast 静音，由壳用 PlaySoundW 异步播放（见 platform::Platform::play_sound_file）。
/// serde 值与 wav 文件名 stem 一致；旧具名音（≤0.1.x）经 alias 迁移——
/// load() 对解析失败整份回退默认，没有 alias 老用户会丢其余全部设置。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub enum CompletionSound {
    #[serde(rename = "silent")]
    Silent,
    #[serde(rename = "default", alias = "im", alias = "mail", alias = "reminder", alias = "sms")]
    Default,
    #[serde(rename = "staplebops-01")]
    Staplebops01,
    #[default]
    #[serde(rename = "staplebops-02", alias = "chime", alias = "drop", alias = "mellow")]
    Staplebops02,
    #[serde(rename = "staplebops-03")]
    Staplebops03,
    #[serde(rename = "staplebops-04")]
    Staplebops04,
    #[serde(rename = "staplebops-05")]
    Staplebops05,
    #[serde(rename = "staplebops-06")]
    Staplebops06,
    #[serde(rename = "staplebops-07")]
    Staplebops07,
    #[serde(rename = "bip-bop-01")]
    BipBop01,
    #[serde(rename = "bip-bop-02")]
    BipBop02,
    #[serde(rename = "bip-bop-03")]
    BipBop03,
    #[serde(rename = "bip-bop-04")]
    BipBop04,
    #[serde(rename = "bip-bop-05")]
    BipBop05,
    #[serde(rename = "bip-bop-06")]
    BipBop06,
    #[serde(rename = "bip-bop-07")]
    BipBop07,
    #[serde(rename = "bip-bop-08")]
    BipBop08,
    #[serde(rename = "bip-bop-09")]
    BipBop09,
    #[serde(rename = "bip-bop-10")]
    BipBop10,
}

impl CompletionSound {
    /// 内置音效清单：(变体, 相对 resource_dir 的 wav 路径)。custom_wav 与
    /// 资源存在性测试共用这一份清单，防止两处漂移。
    pub const CUSTOM: [(CompletionSound, &'static str); 17] = [
        (CompletionSound::Staplebops01, "sounds/staplebops-01.wav"),
        (CompletionSound::Staplebops02, "sounds/staplebops-02.wav"),
        (CompletionSound::Staplebops03, "sounds/staplebops-03.wav"),
        (CompletionSound::Staplebops04, "sounds/staplebops-04.wav"),
        (CompletionSound::Staplebops05, "sounds/staplebops-05.wav"),
        (CompletionSound::Staplebops06, "sounds/staplebops-06.wav"),
        (CompletionSound::Staplebops07, "sounds/staplebops-07.wav"),
        (CompletionSound::BipBop01, "sounds/bip-bop-01.wav"),
        (CompletionSound::BipBop02, "sounds/bip-bop-02.wav"),
        (CompletionSound::BipBop03, "sounds/bip-bop-03.wav"),
        (CompletionSound::BipBop04, "sounds/bip-bop-04.wav"),
        (CompletionSound::BipBop05, "sounds/bip-bop-05.wav"),
        (CompletionSound::BipBop06, "sounds/bip-bop-06.wav"),
        (CompletionSound::BipBop07, "sounds/bip-bop-07.wav"),
        (CompletionSound::BipBop08, "sounds/bip-bop-08.wav"),
        (CompletionSound::BipBop09, "sounds/bip-bop-09.wav"),
        (CompletionSound::BipBop10, "sounds/bip-bop-10.wav"),
    ];

    /// tauri-plugin-notification builder.sound() 的取值；None 表示静音 toast
    /// （内置音效也是静音 toast，声音由壳单独播放）
    pub fn toast_sound_name(self) -> Option<&'static str> {
        match self {
            CompletionSound::Silent => None,
            CompletionSound::Default => Some("Default"),
            _ => None,
        }
    }

    /// 内置音效的 wav 资源相对路径（相对 resource_dir）；None = 非内置音
    pub fn custom_wav(self) -> Option<&'static str> {
        Self::CUSTOM.iter().find(|(v, _)| *v == self).map(|(_, p)| *p)
    }
}

/// 通知时机：Background = 仅当应用无聚焦窗口（后台）时提醒；Always = 前台也提醒
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NotifyTiming {
    #[default]
    Background,
    Always,
}

/// 一类通知的规则：开关 + 时机
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NotifyRule {
    pub enabled: bool,
    pub timing: NotifyTiming,
}

impl Default for NotifyRule {
    fn default() -> Self {
        Self { enabled: true, timing: NotifyTiming::Background }
    }
}

impl NotifyRule {
    /// foreground = 本应用任一窗口处于聚焦态
    pub fn allows(&self, foreground: bool) -> bool {
        self.enabled && (self.timing == NotifyTiming::Always || !foreground)
    }
}

/// 三类通知的独立规则：待批准 / 待回答 / 回答完毕（任务完成）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct NotifySettings {
    pub approval: NotifyRule,
    pub question: NotifyRule,
    pub turn_done: NotifyRule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellSettings {
    pub zoom_step: f64,
    pub zoom_in: Shortcut,
    pub zoom_out: Shortcut,
    pub close_behavior: CloseBehavior,
    pub notify: NotifySettings,
    pub completion_sound: CompletionSound,
    /// 启动时自动检查更新（默认关）：开启后每次启动后台查 GitHub releases，有新版弹 toast
    pub check_update_on_launch: bool,
    /// 旧版字段（≤0.1.7）：读取时迁移进 notify.turn_done.enabled，保存时不再写出
    #[serde(skip_serializing)]
    notify_on_completion: Option<bool>,
}

impl Default for ShellSettings {
    fn default() -> Self {
        Self {
            zoom_step: step_default(),
            zoom_in: Shortcut {
                ctrl: true,
                shift: true,
                alt: false,
                code: "Equal".into(),
                key: "+".into(),
            },
            zoom_out: Shortcut {
                ctrl: true,
                shift: true,
                alt: false,
                code: "Minus".into(),
                key: "_".into(),
            },
            close_behavior: CloseBehavior::Background,
            notify: NotifySettings::default(),
            completion_sound: CompletionSound::Staplebops02,
            check_update_on_launch: false,
            notify_on_completion: None,
        }
    }
}

fn file_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

impl ShellSettings {
    /// 读取设置；文件缺失/损坏 → 全默认；部分字段缺失 → 逐字段回退默认（serde default）。
    /// 步进越界在加载时 clamp，保证内存中始终合法。
    pub fn load(dir: &Path) -> Self {
        let mut s: Self = std::fs::read_to_string(file_path(dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // 旧版 notify_on_completion 布尔 → notify.turn_done.enabled（其余类型默认开）
        if let Some(b) = s.notify_on_completion.take() {
            s.notify.turn_done.enabled = b;
        }
        s.zoom_step = s.zoom_step.clamp(STEP_MIN, STEP_MAX);
        if s.validate().is_err() {
            // 配置文件被手改成非法（无修饰键/快捷键冲突）：回退默认，别带着坏状态跑
            return Self::default();
        }
        s
    }

    /// 写盘失败显式上报：旧版静默吞掉（仅丢持久化），但若写盘被环境阻断
    /// （杀软目录保护等），用户看到的是"保存成功/无关报错"而重启后设置回退，
    /// 无法感知真正原因。失败时内存值也不替换，界面状态与磁盘保持一致。
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let j = serde_json::to_string_pretty(self).map_err(std::io::Error::from)?;
        std::fs::write(file_path(dir), j)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, sc) in [
            (crate::i18n::pick("放大", "Zoom in"), &self.zoom_in),
            (crate::i18n::pick("缩小", "Zoom out"), &self.zoom_out),
        ] {
            if !(sc.ctrl || sc.shift || sc.alt) {
                return Err(crate::i18n::pick(
                    format!("{name}快捷键必须包含 Ctrl/Shift/Alt 中至少一个修饰键"),
                    format!("{name} shortcut must include at least one modifier (Ctrl/Shift/Alt)"),
                ));
            }
        }
        if self.zoom_in == self.zoom_out {
            return Err(crate::i18n::pick(
                "放大与缩小快捷键不能相同",
                "Zoom in and zoom out shortcuts must differ",
            )
            .into());
        }
        Ok(())
    }
}

/// 托管状态：内存值 + 持久化目录。set 先 clamp/校验，再落盘，最后替换内存值；
/// 校验或落盘失败时内存与磁盘都保持旧值。
pub struct SettingsState {
    dir: PathBuf,
    inner: Mutex<ShellSettings>,
}

impl SettingsState {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            inner: Mutex::new(ShellSettings::load(&dir)),
            dir,
        }
    }

    pub fn get(&self) -> ShellSettings {
        self.inner.lock().unwrap().clone()
    }

    pub fn set(&self, mut s: ShellSettings) -> Result<(), String> {
        s.zoom_step = s.zoom_step.clamp(STEP_MIN, STEP_MAX);
        s.validate()?;
        s.save(&self.dir).map_err(|e| {
            crate::i18n::pick(
                format!("设置写入失败: {e}"),
                format!("Failed to write settings file: {e}"),
            )
        })?;
        *self.inner.lock().unwrap() = s;
        Ok(())
    }
}

#[tauri::command]
pub fn get_shell_settings(state: tauri::State<SettingsState>) -> ShellSettings {
    state.get()
}

/// 保存设置；成功后重注入主窗口的缩放钩子（快捷键定义内嵌在脚本里，
/// 必须重注入才生效；钩子内部热替换监听器，不会叠加）
#[tauri::command]
pub fn set_shell_settings(
    app: tauri::AppHandle,
    state: tauri::State<SettingsState>,
    next: ShellSettings,
) -> Result<(), String> {
    state.set(next)?;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.eval(crate::zoom::hook_js(&state.get()));
    }
    Ok(())
}

/// 试听任务完成提示音：内置预设走 toast 音频属性；壳内置音效弹静音 toast
/// 并由壳播放内置 wav（文件缺失降级系统默认预设）。
#[tauri::command]
pub fn preview_completion_sound(
    app: tauri::AppHandle,
    platform: tauri::State<std::sync::Arc<dyn crate::platform::Platform>>,
    sound: CompletionSound,
) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    let mut builder = app
        .notification()
        .builder()
        .title("DSHDesktop")
        .body(crate::i18n::pick("任务完成提示音试听", "Completion sound preview"));
    if let Some(rel) = sound.custom_wav() {
        match crate::resolve_custom_sound(&app, rel) {
            Some(p) => platform.play_sound_file(&p)?,
            None => builder = builder.sound("Default"),
        }
    } else if let Some(name) = sound.toast_sound_name() {
        builder = builder.sound(name);
    }
    builder.show().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_defaults_when_missing_or_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        // 文件不存在 → 全默认
        let s = ShellSettings::load(dir.path());
        assert!((s.zoom_step - 0.02).abs() < 1e-9);
        assert!(s.zoom_in.ctrl && s.zoom_in.shift && !s.zoom_in.alt);
        assert_eq!(s.zoom_in.code, "Equal");
        assert_eq!(s.zoom_in.key, "+");
        assert_eq!(s.zoom_out.code, "Minus");
        assert_eq!(s.zoom_out.key, "_");
        assert!(matches!(s.close_behavior, CloseBehavior::Background));
        // 损坏 JSON → 全默认
        std::fs::write(dir.path().join("settings.json"), "not json").unwrap();
        let s = ShellSettings::load(dir.path());
        assert!((s.zoom_step - 0.02).abs() < 1e-9);
    }

    #[test]
    fn load_partial_fields_filled_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), r#"{ "zoom_step": 0.05 }"#).unwrap();
        let s = ShellSettings::load(dir.path());
        assert!((s.zoom_step - 0.05).abs() < 1e-9);
        assert_eq!(s.zoom_in.code, "Equal"); // 未提供的字段回退默认
    }

    #[test]
    fn step_clamped_to_1_25_percent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), r#"{ "zoom_step": 0.5 }"#).unwrap();
        assert!((ShellSettings::load(dir.path()).zoom_step - 0.25).abs() < 1e-9);
        std::fs::write(dir.path().join("settings.json"), r#"{ "zoom_step": 0.001 }"#).unwrap();
        assert!((ShellSettings::load(dir.path()).zoom_step - 0.01).abs() < 1e-9);
    }

    #[test]
    fn validate_rejects_modifierless_and_conflicting_shortcuts() {
        let mut s = ShellSettings::default();
        s.zoom_in = Shortcut { ctrl: false, shift: false, alt: false, code: "KeyZ".into(), key: "z".into() };
        assert!(s.validate().is_err()); // 无修饰键

        let mut s = ShellSettings::default();
        s.zoom_out = s.zoom_in.clone();
        assert!(s.validate().is_err()); // in/out 冲突

        let mut s = ShellSettings::default();
        s.zoom_in = Shortcut { ctrl: true, shift: false, alt: true, code: "KeyZ".into(), key: "z".into() };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ShellSettings::default();
        s.zoom_step = 0.03;
        s.close_behavior = CloseBehavior::Quit;
        s.zoom_in = Shortcut { ctrl: true, shift: false, alt: true, code: "KeyQ".into(), key: "q".into() };
        s.save(dir.path()).unwrap();
        let s2 = ShellSettings::load(dir.path());
        assert!((s2.zoom_step - 0.03).abs() < 1e-9);
        assert!(matches!(s2.close_behavior, CloseBehavior::Quit));
        assert_eq!(s2.zoom_in.code, "KeyQ");
        assert!(s2.zoom_in.alt && !s2.zoom_in.shift);
    }

    #[test]
    fn state_set_validates_clamps_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let st = SettingsState::new(dir.path().to_path_buf());
        // 越界步进在 set 时 clamp，且同步落盘
        let mut s = st.get();
        s.zoom_step = 0.5;
        st.set(s).unwrap();
        assert!((st.get().zoom_step - 0.25).abs() < 1e-9);
        assert!((ShellSettings::load(dir.path()).zoom_step - 0.25).abs() < 1e-9);
        // 非法设置（无修饰键）被拒绝，内存值不变
        let mut bad = st.get();
        bad.zoom_in = Shortcut { ctrl: false, shift: false, alt: false, code: "KeyZ".into(), key: "z".into() };
        assert!(st.set(bad).is_err());
        assert!(st.get().zoom_in.ctrl);
    }

    #[test]
    fn completion_notify_defaults_on_and_sound_default() {
        let s = ShellSettings::default();
        assert!(s.notify.turn_done.enabled);
        assert_eq!(s.completion_sound, CompletionSound::Staplebops02);
    }

    #[test]
    fn notify_rule_allows_matrix() {
        let on_bg = NotifyRule { enabled: true, timing: NotifyTiming::Background };
        let on_always = NotifyRule { enabled: true, timing: NotifyTiming::Always };
        let off = NotifyRule { enabled: false, timing: NotifyTiming::Always };
        assert!(on_bg.allows(false) && !on_bg.allows(true)); // 仅后台：前台不弹
        assert!(on_always.allows(false) && on_always.allows(true)); // 总是
        assert!(!off.allows(false) && !off.allows(true));
    }

    #[test]
    fn notify_settings_defaults() {
        let s = ShellSettings::default();
        for rule in [s.notify.approval, s.notify.question, s.notify.turn_done] {
            assert!(rule.enabled);
            assert_eq!(rule.timing, NotifyTiming::Background);
        }
    }

    #[test]
    fn legacy_notify_on_completion_migrates_to_turn_done() {
        let dir = tempfile::tempdir().unwrap();
        // 旧版文件（≤0.1.7）：只有 notify_on_completion 布尔，没有 notify 对象
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{ "notify_on_completion": false, "completion_sound": "sms" }"#,
        )
        .unwrap();
        let s = ShellSettings::load(dir.path());
        assert!(!s.notify.turn_done.enabled, "旧开关值应迁移到 turn_done");
        assert!(s.notify.approval.enabled, "其余类型取默认开");
        // "sms" 是已删除的旧具名音，alias 迁移到 default（同为系统预设系）
        assert_eq!(s.completion_sound, CompletionSound::Default);
        // 保存后旧字段消失、新结构落盘
        s.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(!text.contains("notify_on_completion"), "实际文件：{text}");
        assert!(text.contains(r#""turn_done""#), "实际文件：{text}");
    }

    #[test]
    fn notify_rule_serde_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ShellSettings::default();
        s.notify.turn_done.timing = NotifyTiming::Always;
        s.notify.approval.enabled = false;
        s.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(text.contains(r#""timing": "always""#), "实际文件：{text}");
        assert!(text.contains(r#""enabled": false"#), "实际文件：{text}");
        let s2 = ShellSettings::load(dir.path());
        assert_eq!(s2.notify.turn_done.timing, NotifyTiming::Always);
        assert!(!s2.notify.approval.enabled);
    }

    #[test]
    fn old_settings_file_without_notify_fields_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        // 旧版配置文件：没有 notify / completion_sound
        std::fs::write(dir.path().join("settings.json"), r#"{ "zoom_step": 0.05 }"#).unwrap();
        let s = ShellSettings::load(dir.path());
        assert!(s.notify.turn_done.enabled);
        assert_eq!(s.notify.turn_done.timing, NotifyTiming::Background);
        assert_eq!(s.completion_sound, CompletionSound::Staplebops02);
    }

    #[test]
    fn check_update_on_launch_defaults_off_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        // 旧版文件没有该字段 → 默认关
        std::fs::write(dir.path().join("settings.json"), r#"{ "zoom_step": 0.05 }"#).unwrap();
        assert!(!ShellSettings::load(dir.path()).check_update_on_launch);
        // 开启后保存/读取往返一致
        let mut s = ShellSettings::default();
        s.check_update_on_launch = true;
        s.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(text.contains(r#""check_update_on_launch": true"#), "实际文件：{text}");
        assert!(ShellSettings::load(dir.path()).check_update_on_launch);
    }

    #[test]
    fn completion_sound_roundtrip_and_serde_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ShellSettings::default();
        s.notify.turn_done.enabled = false;
        s.completion_sound = CompletionSound::Staplebops02;
        s.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(text.contains(r#""completion_sound": "staplebops-02""#), "实际文件：{text}");
        let s2 = ShellSettings::load(dir.path());
        assert!(!s2.notify.turn_done.enabled);
        assert_eq!(s2.completion_sound, CompletionSound::Staplebops02);
    }

    #[test]
    fn invalid_sound_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{ "completion_sound": "loud-noise" }"#,
        )
        .unwrap();
        let s = ShellSettings::load(dir.path());
        assert_eq!(s.completion_sound, CompletionSound::Staplebops02);
        assert!(s.notify.turn_done.enabled);
    }

    #[test]
    fn toast_sound_name_mapping() {
        assert_eq!(CompletionSound::Silent.toast_sound_name(), None);
        assert_eq!(CompletionSound::Default.toast_sound_name(), Some("Default"));
        // 内置音效一律静音 toast，声音由壳播放
        assert_eq!(CompletionSound::Staplebops02.toast_sound_name(), None);
        assert_eq!(CompletionSound::BipBop10.toast_sound_name(), None);
    }

    #[test]
    fn custom_soft_sounds_use_wav_not_toast_presets() {
        // 内置音效：toast 静音（None），由壳播放内置 wav；路径与 CUSTOM 清单一一对应
        for (s, wav) in CompletionSound::CUSTOM {
            assert_eq!(s.toast_sound_name(), None);
            assert_eq!(s.custom_wav(), Some(wav));
        }
        // 内置 toast 预设不对应自定义 wav
        assert_eq!(CompletionSound::Default.custom_wav(), None);
        assert_eq!(CompletionSound::Silent.custom_wav(), None);
    }

    #[test]
    fn bundled_sound_layout_matches_probe_path() {
        // resolve_custom_sound 在 resource_dir()/exe 旁的 sounds/ 下找 wav（custom_wav
        // 的相对路径），所以 bundle.resources 必须把 resources/sounds 映射为安装根的
        // sounds/。列表形式会原样保留目录层级，安装后落在 resources/sounds/ ——
        // 探测不到，自定义音静默降级系统默认（≤0.1.16 实踩）。
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(conf["bundle"]["resources"]["resources/sounds"], "sounds");
        for (s, _) in CompletionSound::CUSTOM {
            assert!(s.custom_wav().unwrap().starts_with("sounds/"));
        }
    }

    #[test]
    fn custom_soft_sounds_serde_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ShellSettings::default();
        s.completion_sound = CompletionSound::BipBop01;
        s.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(text.contains(r#""completion_sound": "bip-bop-01""#), "实际文件：{text}");
        assert_eq!(ShellSettings::load(dir.path()).completion_sound, CompletionSound::BipBop01);
    }

    #[test]
    fn legacy_sound_values_migrate_and_keep_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        // 旧具名音值必须能解析（alias 迁移），否则 load() 整份回退默认丢用户其余设置
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{ "zoom_step": 0.05, "notify": { "approval": { "enabled": false, "timing": "always" } }, "completion_sound": "chime" }"#,
        )
        .unwrap();
        let s = ShellSettings::load(dir.path());
        assert_eq!(s.completion_sound, CompletionSound::Staplebops02);
        assert_eq!(s.zoom_step, 0.05, "其余设置不得丢失");
        assert!(!s.notify.approval.enabled, "其余设置不得丢失");
        // 系统预设系旧值 → default
        std::fs::write(dir.path().join("settings.json"), r#"{ "completion_sound": "mail" }"#).unwrap();
        assert_eq!(ShellSettings::load(dir.path()).completion_sound, CompletionSound::Default);
    }

    #[test]
    fn bundled_sound_files_exist_on_disk() {
        // custom_wav 指向的每个 wav 必须真实存在（防加变体忘放资源文件）
        for (_, wav) in CompletionSound::CUSTOM {
            let p = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/")).join(wav);
            assert!(p.is_file(), "缺少资源文件：{}", p.display());
        }
    }

    #[test]
    fn save_reports_io_error_instead_of_silently_dropping() {
        let dir = tempfile::tempdir().unwrap();
        // 落盘目录被同名文件占用 → create_dir_all 必失败；旧版静默吞掉这个错误
        let blocker = dir.path().join("blocked");
        std::fs::write(&blocker, "x").unwrap();
        assert!(ShellSettings::default().save(&blocker).is_err());
        // set 同样透出写盘错误，且内存值不被替换
        let st = SettingsState::new(blocker);
        let mut s = st.get();
        s.zoom_step = 0.05;
        assert!(st.set(s).is_err());
        assert!((st.get().zoom_step - 0.02).abs() < 1e-9);
    }

    #[test]
    fn shortcut_matches_by_code_or_key() {
        let sc = Shortcut { ctrl: true, shift: true, alt: false, code: "Equal".into(), key: "+".into() };
        // 真实键盘：code 命中
        assert!(sc.matches("Equal", "+", true, true, false));
        // 合成按键（code 为空）：key 命中
        assert!(sc.matches("", "+", true, true, false));
        // 修饰键不符 / 键不符 → 不命中
        assert!(!sc.matches("Equal", "+", true, false, false));
        assert!(!sc.matches("Minus", "_", true, true, false));
    }
}
