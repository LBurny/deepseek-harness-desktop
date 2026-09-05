//! 检查更新：GitHub `releases/latest` API 查版本，手动更新直接下载 NSIS 安装包。
//!
//! - 目标是**公开发布仓库** deepseek-harness-desktop-releases（0.5.3 起）：源码仓库
//!   私有期间匿名 API 必 404（0.4.x~0.5.2 已知限制），公开仓库匿名可读，检查更新恢复
//! - reqwest 走系统代理：访问的是外网 GitHub（回环才需要 no_proxy，见 remote/proxy.rs），
//!   国内用户挂代理时代理反而是通路的必要条件
//! - GitHub API 必须带 User-Agent，否则一律 403
//! - 安装包选择：assets 中 `*_x64-setup.exe`（多平台包出现后需按 triplet 扩展）
//! - 下载原子落盘：先写 `<name>.part`，完成后 rename 成正式名；进度按百分比变化
//!   节流 emit（与 lib.rs 复制运行时的 copy_cb 同款），避免刷爆 IPC
//! - 检查结果/下载成败都记 events.log（诊断面板之外的最后手段）

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

const RELEASES_LATEST_API: &str =
    "https://api.github.com/repos/LBurny/deepseek-harness-desktop-releases/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/LBurny/deepseek-harness-desktop-releases/releases";
/// 进度事件名：前端 Settings 页监听；负载 { downloaded, total }，total=0 表示长度未知
const PROGRESS_EVENT: &str = "update-download-progress";

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub has_update: bool,
    pub release_url: String,
    pub asset_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone, Serialize)]
struct DownloadProgress {
    downloaded: u64,
    total: u64,
}

/// 解析 "v0.1.13" / "0.1.13" / "1.2.3-rc.1" 为数字段序列；含非数字段 → None。
/// 预发布后缀（-rc.1 等）直接忽略——本仓库发布只用干净 tag。
fn parse_version(s: &str) -> Option<Vec<u64>> {
    let core = s
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()?;
    if core.is_empty() {
        return None;
    }
    core.split('.').map(|p| p.parse::<u64>().ok()).collect()
}

/// latest 是否比 current 新：逐段数值比较，短序列缺段按 0 补齐；
/// 任一侧解析失败按"不是新版"处理，宁可漏报不要误报。
fn is_newer(current: &str, latest: &str) -> bool {
    let (Some(c), Some(l)) = (parse_version(current), parse_version(latest)) else {
        return false;
    };
    for i in 0..c.len().max(l.len()) {
        let a = c.get(i).copied().unwrap_or(0);
        let b = l.get(i).copied().unwrap_or(0);
        if a != b {
            return b > a;
        }
    }
    false
}

fn pick_setup_asset(rel: &Release) -> Option<&Asset> {
    rel.assets.iter().find(|a| a.name.ends_with("_x64-setup.exe"))
}

/// tag "v0.1.13" → "0.1.13"，与 CARGO_PKG_VERSION 的显示格式对齐
fn display_version(tag: &str) -> &str {
    tag.trim().trim_start_matches(['v', 'V'])
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("DSHDesktop/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

async fn fetch_latest(client: &reqwest::Client) -> Result<Release, String> {
    let resp = client
        .get(RELEASES_LATEST_API)
        .timeout(std::time::Duration::from_secs(15))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| {
            crate::i18n::pick(
                format!("网络请求失败：{e}"),
                format!("Network request failed: {e}"),
            )
        })?;
    if !resp.status().is_success() {
        return Err(crate::i18n::pick(
            format!("GitHub 返回 {}", resp.status()),
            format!("GitHub responded with {}", resp.status()),
        ));
    }
    resp.json::<Release>().await.map_err(|e| {
        crate::i18n::pick(
            format!("发布信息解析失败：{e}"),
            format!("Failed to parse release info: {e}"),
        )
    })
}

#[tauri::command]
pub async fn check_update() -> Result<UpdateInfo, String> {
    let client = http_client()?;
    let rel = fetch_latest(&client).await?;
    let current = env!("CARGO_PKG_VERSION").to_string();
    let asset_size = pick_setup_asset(&rel).map(|a| a.size);
    let has_update = is_newer(&current, &rel.tag_name);
    let latest = display_version(&rel.tag_name).to_string();
    Ok(UpdateInfo { current, latest, has_update, release_url: rel.html_url, asset_size })
}

/// 流式写入 .part，按百分比变化节流 emit 进度；返回已下载字节数。
async fn stream_to_file(
    app: &AppHandle,
    resp: reqwest::Response,
    part: &Path,
) -> Result<u64, String> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(part).await.map_err(|e| {
        crate::i18n::pick(
            format!("无法写入下载目录：{e}"),
            format!("Cannot write to the download folder: {e}"),
        )
    })?;
    let _ = app.emit(PROGRESS_EVENT, DownloadProgress { downloaded: 0, total });
    let mut downloaded = 0u64;
    let mut last_pct = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            crate::i18n::pick(format!("下载中断：{e}"), format!("Download interrupted: {e}"))
        })?;
        file.write_all(&chunk).await.map_err(|e| {
            crate::i18n::pick(
                format!("写入失败：{e}"),
                format!("Failed to write file: {e}"),
            )
        })?;
        downloaded += chunk.len() as u64;
        let pct = if total > 0 { downloaded * 100 / total } else { 0 };
        if pct != last_pct {
            last_pct = pct;
            let _ = app.emit(PROGRESS_EVENT, DownloadProgress { downloaded, total });
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(downloaded)
}

/// 手动更新：重新拉一次 latest（不缓存，避免拿到过期资产地址），
/// 把安装包下载到系统下载目录；完成后返回安装包完整路径。
#[tauri::command]
pub async fn download_update(
    app: AppHandle,
    platform: tauri::State<'_, Arc<dyn crate::platform::Platform>>,
) -> Result<String, String> {
    let log = platform.runtime_base_dir().join("events.log");
    let client = http_client()?;
    let rel = fetch_latest(&client).await?;
    if !is_newer(env!("CARGO_PKG_VERSION"), &rel.tag_name) {
        return Err(crate::i18n::pick("当前已是最新版本", "Already up to date").into());
    }
    let asset = pick_setup_asset(&rel).ok_or_else(|| {
        crate::i18n::pick(
            "该版本没有 Windows 安装包",
            "No Windows installer asset in this release",
        )
    })?;
    crate::append_debug_line(
        &log,
        &format!("Update: downloading {} from {}", asset.name, asset.browser_download_url),
    );

    let resp = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| {
            crate::i18n::pick(
                format!("网络请求失败：{e}"),
                format!("Network request failed: {e}"),
            )
        })?;
    if !resp.status().is_success() {
        return Err(crate::i18n::pick(
            format!("下载地址返回 {}", resp.status()),
            format!("Download URL responded with {}", resp.status()),
        ));
    }

    let dir = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
    let part = dir.join(format!("{}.part", asset.name));
    let final_path = dir.join(&asset.name);
    let downloaded = match stream_to_file(&app, resp, &part).await {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            crate::append_debug_line(&log, &format!("Update: download failed: {e}"));
            return Err(e);
        }
    };
    // rename 不覆盖同名旧文件（上一次下载的同版本包）：先删再改名
    if final_path.exists() {
        let _ = std::fs::remove_file(&final_path);
    }
    std::fs::rename(&part, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        crate::i18n::pick(
            format!("安装包落盘失败：{e}"),
            format!("Failed to finalize the installer: {e}"),
        )
    })?;
    // 收尾强制 100%：content-length 与实际字节数不一致时流内可能到不了 100；
    // max(1) 防前端 total=0 除零
    let _ = app.emit(
        PROGRESS_EVENT,
        DownloadProgress { downloaded, total: downloaded.max(1) },
    );
    crate::append_debug_line(
        &log,
        &format!("Update: downloaded {} bytes -> {}", downloaded, final_path.display()),
    );
    Ok(final_path.to_string_lossy().to_string())
}

/// 安装包启动参数（与 Tauri 官方 updater 插件对齐，其恒传 /UPDATE）：
/// - `/UPDATE`：模板进入更新模式——跳过"先卸载旧版"页与 ExecWait 旧卸载器，
///   直接覆盖安装。旧卸载器不参与 => ①不可能再因旧卸载器 Delete 主程序撞
///   文件锁而弹 "Unable to uninstall!"（0.5.9 实踩）；②常驻隧道与
///   remote-session.json 不动，远程链接跨更新保持（0.5.8 语义落地）。
/// - `/P`：被动模式，只显进度条免逐页点击（GUI 模式下"已安装"页仍显示且
///   单选钮被 PageLeaveReinstall 强制忽略，UX 误导，故不用裸 /UPDATE）。
/// - `/R`：被动/静默模式装完由 .onInstSuccess 自动拉起主程序，形成
///   "点立即安装 → 进度条 → 新版自动起来"闭环。
pub(crate) fn installer_args() -> &'static [&'static str] {
    &["/UPDATE", "/P", "/R"]
}

/// 运行已下载的安装包，随后本进程走正常退出流程（quit_app：停 dsh、1.5s 后 exit）。
/// 必须退出：安装包是本进程的子进程，而 NSIS 钩子会 taskkill 主程序——本进程不死，
/// 旧版钩子的 /T 会连整棵进程树（含安装器与 _?= 原地运行的旧卸载器）一起杀掉，
/// 覆盖安装中途凭空消失。本进程先死后，钩子找不到 DSHDesktop.exe，杀树无从谈起。
/// （Job Object 不会误杀安装器：install_update 用裸 spawn，未挂进 KILL_ON_JOB_CLOSE。）
#[tauri::command]
pub fn install_update(app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !name.ends_with("_x64-setup.exe") || !p.is_file() {
        return Err(crate::i18n::pick("安装包路径无效", "Invalid installer path").into());
    }
    std::process::Command::new(&p)
        .args(installer_args())
        .spawn()
        .map_err(|e| {
            crate::i18n::pick(
                format!("启动安装包失败：{e}"),
                format!("Failed to launch the installer: {e}"),
            )
        })?;
    crate::tray::quit_app(&app);
    Ok(())
}

/// 在系统浏览器中打开 releases 页
#[tauri::command]
pub fn open_update_page() -> Result<(), String> {
    open_url(RELEASES_PAGE)
}

#[cfg(windows)]
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    // rundll32 是 GUI 子系统进程，不会闪控制台窗口
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 启动时自动检查（设置开启时，默认关）：有新版弹 toast 提示到其它设置的检查更新下载；
/// 失败只记 events.log，不打断启动、不打扰用户。
pub async fn check_on_launch(app: AppHandle, log: PathBuf) {
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => {
            crate::append_debug_line(&log, &format!("Update: client init failed: {e}"));
            return;
        }
    };
    match fetch_latest(&client).await {
        Ok(rel) => {
            let current = env!("CARGO_PKG_VERSION");
            if is_newer(current, &rel.tag_name) {
                let latest = display_version(&rel.tag_name);
                crate::append_debug_line(
                    &log,
                    &format!("Update: new version {latest} available (current {current})"),
                );
                let _ = crate::notify::toast::show(
                    &app,
                    &crate::i18n::pick("DSHDesktop 有新版本", "DSHDesktop update available"),
                    &crate::i18n::pick(
                        format!("v{latest} 已发布，请在其它设置的检查更新中下载"),
                        format!("v{latest} is available, download it from Check for updates in Other settings"),
                    ),
                    crate::notify::toast::ToastSound::Silent,
                    None,
                );
            } else {
                crate::append_debug_line(&log, &format!("Update: up to date ({current})"));
            }
        }
        Err(e) => crate::append_debug_line(&log, &format!("Update: check on launch failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_accepts_common_shapes() {
        assert_eq!(parse_version("0.1.13"), Some(vec![0, 1, 13]));
        assert_eq!(parse_version("v0.1.13"), Some(vec![0, 1, 13]));
        assert_eq!(parse_version("V1.2"), Some(vec![1, 2]));
        assert_eq!(parse_version(" 1.2.3 "), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("1.2.3-rc.1"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("1.2.3+build5"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn parse_version_rejects_garbage() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("v"), None);
        assert_eq!(parse_version("1.x.3"), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn is_newer_compares_segment_by_segment() {
        assert!(is_newer("0.1.13", "v0.1.14"));
        assert!(is_newer("0.1.13", "0.2.0"));
        assert!(is_newer("0.9.9", "1.0.0"));
        assert!(is_newer("0.1.13", "0.1.13.1")); // 长序列多一段且更大
        assert!(!is_newer("0.1.13", "0.1.13"));
        assert!(!is_newer("0.1.13", "v0.1.12"));
        assert!(!is_newer("0.2.0", "0.1.99"));
        assert!(!is_newer("0.1.13", "garbage")); // 解析失败不误报
        assert!(!is_newer("garbage", "0.1.14"));
    }

    #[test]
    fn pick_setup_asset_selects_x64_nsis_installer() {
        let rel: Release = serde_json::from_str(
            r#"{
                "tag_name": "v0.1.13",
                "html_url": "https://github.com/LBurny/deepseek-harness-desktop/releases/tag/v0.1.13",
                "assets": [
                    {
                        "name": "DSHDesktop_0.1.13_x64-setup.exe",
                        "browser_download_url": "https://github.com/LBurny/deepseek-harness-desktop/releases/download/v0.1.13/DSHDesktop_0.1.13_x64-setup.exe",
                        "size": 59881350
                    },
                    {
                        "name": "DSHDesktop_0.1.13_x64-setup.exe.sha256",
                        "browser_download_url": "https://example.com/x.sha256",
                        "size": 99
                    }
                ]
            }"#,
        )
        .unwrap();
        let a = pick_setup_asset(&rel).unwrap();
        assert_eq!(a.name, "DSHDesktop_0.1.13_x64-setup.exe");
        assert_eq!(a.size, 59881350);
        // sha256 校验文件不会被误选（名字以 .sha256 结尾而非 .exe）
        assert!(a.browser_download_url.ends_with(".exe"));
    }

    #[test]
    fn pick_setup_asset_none_when_missing() {
        let rel: Release = serde_json::from_str(
            r#"{
                "tag_name": "v9.9.9",
                "html_url": "https://example.com",
                "assets": [{ "name": "DSHDesktop_9.9.9_aarch64.dmg", "browser_download_url": "https://example.com/x" }]
            }"#,
        )
        .unwrap();
        assert!(pick_setup_asset(&rel).is_none());
    }

    #[test]
    fn nsis_hook_taskkill_never_kills_process_tree() {
        // install_update 把安装包拉成本进程的子进程；NSIS 钩子里的 taskkill 若带 /T，
        // 覆盖安装/升级时会连整棵进程树一起杀——包括安装器与 _?= 原地运行的旧卸载器
        // 自身（点击"立即安装"后安装中途凭空消失）。子进程回收由 KILL_ON_JOB_CLOSE
        // Job（>=0.1.9）与钩子里的按路径清扫（<=0.1.8 孤儿）兜底，/T 不得回归。
        let hooks = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/windows/nsis-hooks.nsh"
        ))
        .unwrap();
        let taskkill_line = hooks
            .lines()
            .find(|l| l.contains("taskkill.exe"))
            .expect("preinstall hook must taskkill the running app");
        assert!(
            !taskkill_line.contains("/T"),
            "taskkill must not kill the whole tree (installer self-kill): {taskkill_line}"
        );
    }

    #[test]
    fn install_update_runs_installer_in_update_mode() {
        // 0.5.10 修复：install_update 必须带 /UPDATE——Tauri NSIS 模板仅在更新模式下
        // 跳过"先卸载旧版"步骤（PageLeaveReinstall 的 ExecWait _?= 旧卸载器 +
        // FileExists 主程序检查，两个触发条件：卸载器退出码非 0 / exe 卸载后仍存在）。
        // 不带 /UPDATE 时：①整个卸载步骤的时序竞态窗口都会暴露（模板
        // CheckIfAppIsRunning 杀进程后仅 500ms 就 Delete 主程序，而系统组件对刚退出
        // 的进程映像持柄 1~3s，Delete 静默失败、退出码仍 0 → 弹 "Unable to
        // uninstall!"）；②旧卸载器 POSTUNINSTALL 按"真卸载"清常驻隧道与
        // remote-session.json，远程链接每次更新都失效（0.5.8 语义被破坏，0.5.8→0.5.9
        // 实锤断链）。/UPDATE 下旧卸载器根本不运行，两个问题同源消失。Tauri 官方
        // updater 插件恒传 /UPDATE（plugins-workspace updater.rs updater_parameters）。
        // /P=被动模式只显进度条（GUI 模式下"已安装"页仍显示且单选钮失效，UX 误导）；
        // /R=被动/静默模式装完自动拉起主程序（.onInstSuccess），形成"点立即安装→
        // 进度条→新版自动起来"闭环。
        let args = installer_args();
        assert!(args.contains(&"/UPDATE"), "missing /UPDATE: {args:?}");
        assert!(args.contains(&"/P"), "missing /P: {args:?}");
        assert!(args.contains(&"/R"), "missing /R: {args:?}");
    }

    #[test]
    fn nsis_hook_waits_for_exe_file_unlock_after_kill() {
        // 0.5.10 修复：进程死亡≠ exe 文件锁释放（Defender/PCA 会对刚退出的进程映像
        // 保持短暂句柄，本机实测 19MB 主程序+机械盘锁窗口 ~1.3s）。模板
        // CheckIfAppIsRunning 杀完主程序仅 500ms 就 Delete——快速连点+应用正在
        // 自行退出时 Delete 撞锁静默失败（退出码仍 0）→ 模板 FileExists 复检弹
        // "Unable to uninstall!"；/UPDATE 覆盖路径 File 写主程序撞锁则直接
        // "Can't write" 中止。钩子在等净进程后必须再等主程序/runtime 两件 exe
        // 可独占打开，让 Delete/File 落在锁释放后。
        let hooks = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/windows/nsis-hooks.nsh"
        ))
        .unwrap();
        assert!(
            hooks.contains("[System.IO.File]::Open"),
            "hooks must probe exe file locks after killing processes"
        );
    }

    #[test]
    fn nsis_hook_preinstall_purges_runtime_tree() {
        // 0.5.10：/UPDATE 覆盖安装不再经过旧卸载器（POSTUNINSTALL 的 RMDir runtime
        // 不执行），PREINSTALL 必须自己清 runtime 树，否则旧版独有文件跨版本混杂
        // （dsh 自更新残留同理）。非更新路径下旧卸载器已删净，重复 RMDir 是无害 no-op。
        let hooks = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/windows/nsis-hooks.nsh"
        ))
        .unwrap();
        let count = hooks
            .matches("RMDir /r /REBOOTOK \"$INSTDIR\\runtime\"")
            .count();
        assert!(
            count >= 2,
            "runtime purge must exist in both PREINSTALL and POSTUNINSTALL, found {count}"
        );
    }

    #[test]
    fn nsis_hook_postuninstall_keeps_tunnel_when_invoked_by_setup() {
        // 0.5.10 修复：手动双击新安装包走"先卸载"时，旧卸载器的父进程是新安装器
        // （DSHDesktop_*_x64-setup.exe）——这是升级不是真卸载，常驻隧道与
        // remote-session.json 必须留活，否则手动更新一样断链（/UPDATE 标志只覆盖
        // 应用内更新流）。真卸载（设置/开始菜单，自我复制到 %TEMP%）父进程链已死
        // 或为 explorer，不匹配模式 → 照常清理。
        let hooks = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/windows/nsis-hooks.nsh"
        ))
        .unwrap();
        assert!(
            hooks.contains("DSHDesktop_*_x64-setup.exe"),
            "POSTUNINSTALL must detect setup.exe parent to keep the tunnel on upgrades"
        );
    }

    #[test]
    fn release_tolerates_missing_assets_and_unknown_fields() {
        // assets 缺省 → 空列表；GitHub 响应里的其它字段（body/published_at…）被忽略
        let rel: Release =
            serde_json::from_str(r#"{ "tag_name": "v1.0.0", "html_url": "u", "body": "notes" }"#)
                .unwrap();
        assert!(rel.assets.is_empty());
        assert_eq!(display_version(&rel.tag_name), "1.0.0");
    }

    #[test]
    fn update_targets_public_releases_repo() {
        // 源码仓库私有：检查更新/手动更新/GitHub 下载必须指向公开发布仓库——
        // 指回源码私有仓库则匿名 API 一律 404（0.4.x~0.5.2 已知限制实踩，0.5.3 起分离）
        assert!(RELEASES_LATEST_API.contains("/LBurny/deepseek-harness-desktop-releases/"));
        assert!(RELEASES_PAGE.ends_with("/LBurny/deepseek-harness-desktop-releases/releases"));
    }
}
