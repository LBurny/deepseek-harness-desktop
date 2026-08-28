//! 手工验证脚本（不进 cargo test）。两种模式：
//! 1. 默认：弹一条**协议激活** toast（activationType="protocol" launch=
//!    "dshdesktop://open"），实机点击后 Windows 拉起协议 → 已安装的
//!    DSHDesktop.exe 二次实例被 single-instance 拒掉 → 主窗口弹出。
//!    这段 interop 与 notify/toast.rs 的最终实现同款。
//! 2. `--play <wav路径>`：用壳的 waveOut 播放器连播 3 次（每次新开设备
//!    句柄，第 2 次在播放中启动以验证打断语义），打印每次的诊断行
//!    （耗时/设备名/错误码）。
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 2 && args[1] == "--play" {
        play_three(&args[2]);
        return;
    }
    show_proto_toast();
}

fn show_proto_toast() {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    let xml = r#"<toast activationType="protocol" launch="dshdesktop://open">
    <visual><binding template="ToastGeneric">
        <image id="1" placement="appLogoOverride" src="file:///H:/My_Software/DSHDesktop/src-tauri/icons/128x128.png" alt="app"/>
        <text>DSHDesktop proto activation test</text>
        <text>Click this toast (within 60s)</text>
    </binding></visual>
    <audio silent="true"/>
</toast>"#;

    let doc = XmlDocument::new().expect("XmlDocument::new");
    doc.LoadXml(&HSTRING::from(xml)).expect("LoadXml");
    let toast = ToastNotification::CreateToastNotification(&doc).expect("create toast");
    let notifier =
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from("com.dshdesktop.desktop"))
            .expect("notifier");
    notifier.Show(&toast).expect("show");
    println!("PROTO-TOAST-SHOWN, waiting for click...");
    let _ = std::io::stdout().flush();
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    println!("done waiting");
}

fn play_three(wav: &str) {
    let platform = dshdesktop_lib::platform::current();
    let path = std::path::PathBuf::from(wav);
    for i in 1..=3 {
        let diag: dshdesktop_lib::platform::SoundDiag =
            std::sync::Arc::new(move |line: String| {
                println!("{line}");
                let _ = std::io::stdout().flush();
            });
        // i=2 时上一次可能仍在播放：顺带验证打断语义（新播停旧播）
        platform
            .play_sound_file(&path, Some(diag))
            .expect("play_sound_file dispatch failed");
        std::thread::sleep(std::time::Duration::from_millis(if i == 1 {
            300
        } else {
            1500
        }));
    }
    println!("PLAY-DONE");
}