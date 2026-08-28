//! 手工验证脚本（不进 cargo test）：弹一条**协议激活** toast
//! （activationType="protocol" launch="dshdesktop://open"），实机点击后
//! Windows 拉起协议 → 已安装的 DSHDesktop.exe 二次实例被 single-instance
//! 拒掉、参数递给运行中实例 → 主窗口弹出。观察：主窗口是否弹出。
//! 这段 interop 与 notify/toast.rs 的最终实现同款，作为机制的独立验证。
use std::io::Write;

fn main() {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    let xml = r#"<toast activationType="protocol" launch="dshdesktop://open">
    <visual><binding template="ToastGeneric">
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