//! 系统 toast 封装（WinRT 直连）：弹通知 + **点击 toast 回到主界面**。
//!
//! 点击激活走 **协议激活**（activationType="protocol" launch="dshdesktop://open"）：
//! 系统按 HKCU 注册的 URL Protocol 拉起本 exe → 二次实例被 single-instance 插件
//! 拒掉、参数递给运行中实例 → 回调里 show+聚焦主窗口（与托盘左键等效）。
//! 选这条路的依据（机器 A 实测）：in-process `Activated` 回调对未打包 Win32 应用
//! 不可靠——AUMID 无注册、注册表键补齐两种条件下点击 toast 均无回调（toast 被点
//! 掉、进了通知中心、回调不触发）；协议激活是 Win10 原生支持的路由，且应用未
//! 运行时点击还能顺带拉起应用，行为更完整。
//!
//! 声音与 AUMID 映射逐项对齐 tauri-plugin-notification 的 Windows 后端
//! （notify-rust），保证弹出的 toast 外观与替换前完全一致：
//! - 未设声音的 toast 一律 `<audio silent="true"/>`（静音）——自定义 wav 由壳
//!   另行 PlaySoundW，toast 本体保持静音
//! - `Default` = 省略 audio 元素，系统默认提示音
//! - 已安装应用（非 target 目录）用应用 identifier 作 AUMID；dev 落回
//!   powershell AUMID（未注册的 AUMID 在 Win10 上弹不出 toast）

use crate::platform::SoundDiag;

/// toast 提示音档位
pub(crate) enum ToastSound {
    /// 静音 toast（自定义 wav 由壳另行播放）
    Silent,
    /// 系统默认提示音
    Default,
}

/// 协议激活时随二次实例传入的参数标记（single-instance 回调里识别"点击通知
/// 而来"用；协议本身忽略该参数）
pub(crate) const PROTOCOL_ARG: &str = "--dshdesktop-protocol";

/// 协议 scheme（HKCU 注册的 URL Protocol 名）
const PROTOCOL_SCHEME: &str = "dshdesktop";

/// XML 文本转义（title/body 来自通知内容/会话标题，可能含 & < > " '）
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn toast_xml(title: &str, body: &str, sound: &ToastSound) -> String {
    let audio = match sound {
        ToastSound::Silent => r#"<audio silent="true"/>"#,
        ToastSound::Default => "",
    };
    format!(
        r#"<toast activationType="protocol" launch="{PROTOCOL_SCHEME}://open">
    <visual><binding template="ToastGeneric">
        <text>{}</text>
        <text>{}</text>
    </binding></visual>
    {}
</toast>"#,
        xml_escape(title),
        xml_escape(body),
        audio
    )
}

#[cfg(windows)]
mod imp {
    use super::*;
    use tauri::Manager;
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    /// dev 退回的系统已注册 AUMID（PowerShell 开始菜单快捷方式自带，
    /// 未注册的 identifier 在 Win10 上弹不出 toast；dev 下 toast 源显示为
    /// PowerShell，仅影响开发体验）
    const POWERSHELL_APP_ID: &str =
        "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe";

    /// AUMID 解析：与插件 desktop.rs 的分支一致——exe 落在 target/{debug,release}
    /// 视为 dev（identifier 未注册），退回 powershell；其余用应用 identifier
    fn toast_app_id(app: &tauri::AppHandle) -> String {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let dir = dir.display().to_string();
                let dev = dir.ends_with(r"\target\debug") || dir.ends_with(r"\target\release");
                if !dev {
                    return app.config().identifier.clone();
                }
            }
        }
        POWERSHELL_APP_ID.to_string()
    }

    /// exe 是否为已安装形态（非 target/{debug,release} 目录）
    fn installed_exe() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.display().to_string();
        let dev = dir.ends_with(r"\target\debug") || dir.ends_with(r"\target\release");
        if dev { None } else { Some(exe) }
    }

    /// 弹一条系统 toast；点击通知 = 协议激活 → 主窗口弹出并聚焦。激活过程由
    /// 系统完成（无需进程内回调）；show 失败返回 Err 由调用侧落日志。
    pub(crate) fn show(
        app: &tauri::AppHandle,
        title: &str,
        body: &str,
        sound: ToastSound,
        _diag: Option<SoundDiag>,
    ) -> Result<(), String> {
        let doc = XmlDocument::new().map_err(|e| e.to_string())?;
        doc.LoadXml(&HSTRING::from(toast_xml(title, body, &sound)))
            .map_err(|e| e.to_string())?;
        let toast = ToastNotification::CreateToastNotification(&doc).map_err(|e| e.to_string())?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
            toast_app_id(app),
        ))
        .map_err(|e| e.to_string())?;
        notifier.Show(&toast).map_err(|e| e.to_string())
    }

    /// 启动时确保点击激活链路可用（HKCU，幂等，仅已安装形态写入）：
    /// - `dshdesktop://` URL Protocol → 本 exe：toast 点击拉起协议，二次实例
    ///   经 single-instance 把激活递给运行中实例；exe 路径变化（重装/换目录）
    ///   后下次启动自动纠正
    /// - AUMID 显示名键：toast 源显示名在机器间的启发式回退不一致，注册后
    ///   恒为应用名
    /// dev（target 目录）不写——避免覆盖已安装版的注册指向。
    /// 失败只落 events.log（toast 仍会弹，仅点击不激活），不打断启动。
    pub(crate) fn ensure_activation_registered(app: &tauri::AppHandle) {
        use winreg::enums::HKEY_CURRENT_USER;
        let Some(exe) = installed_exe() else {
            return;
        };
        let name = app.package_info().name.clone();
        let identifier = app.config().identifier.clone();
        let result = (|| -> std::io::Result<()> {
            let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
            let proto = hkcu.create_subkey(r"Software\Classes\dshdesktop")?.0;
            proto.set_value("", &"URL:DSHDesktop")?;
            proto.set_value("URL Protocol", &"")?;
            let cmd = hkcu
                .create_subkey(r"Software\Classes\dshdesktop\shell\open\command")?
                .0;
            cmd.set_value(
                "",
                &format!("\"{}\" \"{}\" \"%1\"", exe.display(), PROTOCOL_ARG),
            )?;
            let aumid = hkcu
                .create_subkey(format!(r"Software\Classes\AppUserModelId\{identifier}"))?
                .0;
            aumid.set_value("DisplayName", &name)?;
            aumid.set_value("IconUri", &exe.display().to_string())?;
            Ok(())
        })();
        if let Err(e) = result {
            let log = app
                .try_state::<std::sync::Arc<dyn crate::platform::Platform>>()
                .map(|p| p.runtime_base_dir().join("events.log"));
            if let Some(log) = log {
                crate::append_debug_line(
                    &log,
                    &format!(
                        "[{}] toast: activation registration failed: {e}",
                        crate::local_stamp()
                    ),
                );
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub(crate) fn show(
        _app: &tauri::AppHandle,
        _title: &str,
        _body: &str,
        _sound: ToastSound,
        _diag: Option<SoundDiag>,
    ) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn ensure_activation_registered(_app: &tauri::AppHandle) {}
}

pub(crate) use imp::{ensure_activation_registered, show};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_xml_has_protocol_activation_and_silence() {
        let xml = toast_xml("标题<b>&特殊", "正文", &ToastSound::Silent);
        assert!(xml.contains(r#"activationType="protocol""#));
        assert!(xml.contains(r#"launch="dshdesktop://open""#));
        assert!(xml.contains(r#"<audio silent="true"/>"#));
        // XML 转义：标题里的特殊字符必须不破坏结构
        assert!(xml.contains("标题&lt;b&gt;&amp;特殊"));
        assert!(!xml.contains("标题<b>"));
        // default 档 = 省略 audio 元素（系统默认提示音）
        let xml = toast_xml("t", "b", &ToastSound::Default);
        assert!(!xml.contains("<audio"));
    }
}