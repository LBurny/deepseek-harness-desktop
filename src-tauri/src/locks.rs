//! dsh 陈旧写锁自愈（0.5.13，机器 B 实踩）。
//!
//! 症状：应用再也起不来。events.log 里 dsh 反复以
//! `dsh: plugin tree failed to load: failed to apply loader entry connection
//! (@deepseek-ai/dsh-client-connection): atomic-write: timed out waiting for the
//! writer lock at <DSH_HOME>\.credentials.yaml.lock` 退出，壳只看到
//! "就绪行没出现（token）"，重试多少次都一样。
//!
//! 根因（上游事实见 upstream.rs 的 LOCK_FILE_SUFFIX 出处）：dsh 的跨进程写锁是
//! 目标文件的兄弟路径 `<file>.lock`——wx 独占创建、内容为 `${process.pid}\n`、
//! 只在 finally 里删，且**竞争方永不删除已存在的锁**（上游注释原文：orphan
//! recovery is an operator action）。Windows 上壳只能用 `taskkill /T /F`
//! （TerminateProcess）硬杀 dsh，它装在 profile-boot 里的 SIGTERM/SIGINT 优雅退场
//! 拿不到信号，于是"恰好持锁时被杀"就把锁文件永久留在盘上：之后每次启动都在
//! boot 阶段等锁超时（`.credentials.yaml` 的写入预算 30s），插件树加载失败、
//! 进程直接退出——用户视角是"应用坏了"。
//!
//! 硬杀窗口不大但天天都在：每次退出应用、每次 token 超时重试、安装器清进程都走
//! taskkill /F；而 boot 阶段（MCP 冷装可达数分钟）用户等不及直接退出，正好落进窗口。
//!
//! 策略（保守优先）：只在 Windows 上跑，只碰 DSH_HOME 下深度受限的 `*.lock` 文件，
//! 且满足下列之一才删：
//!   ①锁内容是可解析的 pid，且该进程已退出（或被无关进程复用了 pid）；
//!   ②锁内容不是 pid，且文件确实够老（UNPARSEABLE_LOCK_MIN_AGE）——覆盖"创建瞬间
//!     被硬杀、内容还没写全"的半成品。
//! pid 仍活着且镜像像 dsh（node）的一律保留：那可能是用户自己终端里跑的 dsh，
//! 删掉会破坏上游的写序列化。读不到镜像信息（权限/竞态）同样按真持有者保守处理。
//! 每条判定都经 append_debug_line 落 events.log，误删可追溯。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::platform::Platform;
use crate::upstream;

/// 读不出 pid 的锁文件要老到这个程度才敢删（创建与写内容之间的窗口）。
pub const UNPARSEABLE_LOCK_MIN_AGE: Duration = Duration::from_secs(300);

/// 对一个锁文件的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockAction {
    /// 锁里的 pid 已退出（或该 pid 被无关进程复用）→ 删除
    RemovedStaleOwner,
    /// 锁内容不是 pid 且文件够老 → 删除
    RemovedUnreadable,
    /// 持有者进程还活着且像 dsh → 保留（真持有者，删了破坏上游写序列化）
    KeptLiveOwner,
    /// 内容不是 pid 但未确认够老 → 保留（可能别的进程正在创建）
    KeptUnconfirmed,
    /// 该删但删不掉（权限/句柄占用）→ 保留并记日志
    RemoveFailed,
}

impl LockAction {
    pub fn removed(self) -> bool {
        matches!(self, Self::RemovedStaleOwner | Self::RemovedUnreadable)
    }
}

/// 一次自愈扫描对某个锁文件的处置记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockFinding {
    pub path: PathBuf,
    pub pid: Option<u32>,
    pub action: LockAction,
}

impl LockFinding {
    /// events.log 行（只带文件名与 pid，不含锁内容其它字节）。
    pub fn log_line(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());
        match self.action {
            LockAction::RemovedStaleOwner => format!(
                "[locks] 清理陈旧锁 {name}（pid={} 已退出，dsh 硬杀残留）",
                self.pid.map_or("?".to_string(), |p| p.to_string())
            ),
            LockAction::RemovedUnreadable => format!("[locks] 清理陈旧锁 {name}（内容无可解析 pid 且已过期）"),
            LockAction::KeptLiveOwner => format!(
                "[locks] 保留 {name}（pid={} 存活且为 node 进程）",
                self.pid.map_or("?".to_string(), |p| p.to_string())
            ),
            LockAction::KeptUnconfirmed => format!("[locks] 保留 {name}（内容无可解析 pid，尚未确认过期）"),
            LockAction::RemoveFailed => format!("[locks] 陈旧锁 {name} 删除失败（权限/占用），保留"),
        }
    }
}

/// 纯判定：pid（锁内容里读到的）+ 持有者是否存活 + 文件年龄 → 处置。
/// `age=None` 表示读不到修改时间——没 pid 又不知年龄时不敢删（KeptUnconfirmed）。
pub fn decide_lock_action(pid: Option<u32>, owner_alive: bool, age: Option<Duration>) -> LockAction {
    match pid {
        Some(_) if owner_alive => LockAction::KeptLiveOwner,
        Some(_) => LockAction::RemovedStaleOwner,
        None => match age {
            Some(a) if a >= UNPARSEABLE_LOCK_MIN_AGE => LockAction::RemovedUnreadable,
            _ => LockAction::KeptUnconfirmed,
        },
    }
}

/// 扫 DSH_HOME（深度受限、跳过 node_modules）清理陈旧锁。返回每个锁文件的处置，
/// 调用侧负责落日志。失败一律不抛：清理是尽力而为，绝不能让启动路径崩。
pub fn heal_stale_locks(home: &Path, platform: &dyn Platform) -> Vec<LockFinding> {
    // 非 Windows 不进：Platform::process_alive 在桩平台恒 false，会误删活锁。
    // 将来 macos/linux 实现真 liveness 再把这道门拆掉（platform/*.rs 启用时）。
    if !cfg!(windows) {
        return Vec::new();
    }
    heal_with(home, &|pid| {
        platform.process_alive(pid) && owner_looks_like_dsh(platform, pid)
    })
}

/// 持有者存活且镜像名是内嵌 node（dsh 就是 node 进程；插件 CLI 也是）。
/// 读不到镜像路径时保守当"是"（权限不足 / 刚好退出，宁可留着下次再判）。
fn owner_looks_like_dsh(platform: &dyn Platform, pid: u32) -> bool {
    match platform.process_image_path(pid) {
        Some(p) => p
            .file_name()
            .map(|n| n.eq_ignore_ascii_case(platform.node_exe_name()))
            .unwrap_or(false),
        None => true,
    }
}

/// 自愈的实现骨架：liveness 判定注入，便于用假判定做确定性的单元测试。
fn heal_with(home: &Path, owner_alive: &dyn Fn(u32) -> bool) -> Vec<LockFinding> {
    let mut findings = Vec::new();
    for path in find_lock_files(home) {
        let pid = read_lock_pid(&path);
        let alive = pid.map(owner_alive).unwrap_or(false);
        let age = lock_age(&path);
        let action = decide_lock_action(pid, alive, age);
        let action = if action.removed() {
            match fs::remove_file(&path) {
                Ok(()) => action,
                Err(_) => LockAction::RemoveFailed,
            }
        } else {
            action
        };
        findings.push(LockFinding { path, pid, action });
    }
    findings
}

/// 锁内容首行的 pid。写失败/空文件/被改写一律 None（交由年龄判定兜底）。
fn read_lock_pid(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    let first = text.lines().next()?.trim();
    let pid: u32 = first.parse().ok()?;
    // 0/4 是系统保留 pid（System Idle / System），不可能是 dsh
    if pid == 0 || pid == 4 {
        return None;
    }
    Some(pid)
}

fn lock_age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

fn find_lock_files(home: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(home, 0, &mut found);
    found.sort();
    found
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > upstream::LOCK_SCAN_MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            if is_skipped_dir(&path) {
                continue;
            }
            walk(&path, depth + 1, found);
        } else if kind.is_file() && has_lock_suffix(&path) {
            found.push(path);
        }
        // 符号链接/junction 既不进也不认：避免顺着链接爬出 DSH_HOME
    }
}

fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            upstream::LOCK_SCAN_SKIP_DIRS
                .iter()
                .any(|s| n.eq_ignore_ascii_case(s))
        })
        .unwrap_or(false)
}

fn has_lock_suffix(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(upstream::LOCK_FILE_SUFFIX))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(dead_pid: u32) -> impl Fn(u32) -> bool {
        move |pid| pid != dead_pid
    }

    #[test]
    fn decide_removes_dead_owner_and_keeps_live_one() {
        assert_eq!(
            decide_lock_action(Some(42), false, Some(Duration::from_secs(1))),
            LockAction::RemovedStaleOwner
        );
        assert_eq!(
            decide_lock_action(Some(42), true, Some(Duration::from_secs(1))),
            LockAction::KeptLiveOwner
        );
    }

    #[test]
    fn decide_only_removes_unreadable_lock_once_old() {
        assert_eq!(
            decide_lock_action(None, false, Some(Duration::from_secs(1))),
            LockAction::KeptUnconfirmed
        );
        assert_eq!(
            decide_lock_action(None, false, Some(UNPARSEABLE_LOCK_MIN_AGE)),
            LockAction::RemovedUnreadable
        );
        // 读不到年龄：不敢删
        assert_eq!(
            decide_lock_action(None, false, None),
            LockAction::KeptUnconfirmed
        );
    }

    #[test]
    fn heal_removes_only_the_stale_lock() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let stale = home.join(".credentials.yaml.lock");
        let live = home.join(".settings.yaml.lock");
        let other = home.join(".credentials.yaml");
        fs::write(&stale, "4242\n").unwrap();
        fs::write(&live, "7\n").unwrap();
        fs::write(&other, "secret: x\n").unwrap();

        let findings = heal_with(home, &fake(4242));

        assert!(!stale.exists(), "死 pid 的锁应被删除");
        assert!(live.exists(), "活 pid 的锁必须保留");
        assert!(other.exists(), "非 .lock 文件不该被碰");
        assert_eq!(
            findings.iter().find(|f| f.path == stale).unwrap().action,
            LockAction::RemovedStaleOwner
        );
        assert_eq!(
            findings.iter().find(|f| f.path == live).unwrap().action,
            LockAction::KeptLiveOwner
        );
        assert_eq!(findings.len(), 2, "非 .lock 文件不进清单");
    }

    #[test]
    fn heal_reads_pid_from_first_line_only() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let lock = home.join("a.lock");
        fs::write(&lock, "4242\ngarbage\n").unwrap();
        let findings = heal_with(home, &fake(4242));
        assert_eq!(findings[0].pid, Some(4242));
        assert!(!lock.exists());
    }

    #[test]
    fn heal_keeps_garbage_content_until_confirmed_old() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let lock = home.join("empty.lock");
        fs::write(&lock, "").unwrap();
        let findings = heal_with(home, &fake(4242));
        assert!(lock.exists(), "刚建出来、内容没写全的锁不能马上删");
        assert_eq!(findings[0].action, LockAction::KeptUnconfirmed);
        assert_eq!(findings[0].pid, None);
    }

    #[test]
    fn heal_ignores_reserved_pids() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::write(home.join("sys.lock"), "4\n").unwrap();
        let findings = heal_with(home, &fake(4242));
        assert_eq!(findings[0].pid, None, "pid 4 是系统进程，不该被当成持有者");
    }

    #[test]
    fn heal_skips_node_modules_and_respects_depth_cap() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let deep_home = home.join("a").join("b").join("c");
        fs::create_dir_all(&deep_home).unwrap();
        fs::write(deep_home.join("in-range.lock"), "4242\n").unwrap();
        let past_cap = deep_home.join("d");
        fs::create_dir_all(&past_cap).unwrap();
        fs::write(past_cap.join("too-deep.lock"), "4242\n").unwrap();
        let nm = home.join("profiles").join("plugins").join("p").join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("skip.lock"), "4242\n").unwrap();

        let findings = heal_with(home, &fake(4242));
        let names: Vec<String> = findings
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(names, vec!["in-range.lock".to_string()], "只认深度内的锁，node_modules 整棵跳过");
        assert!(past_cap.join("too-deep.lock").exists());
        assert!(nm.join("skip.lock").exists());
    }

    #[test]
    fn heal_on_missing_home_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let findings = heal_with(&dir.path().join("nope"), &fake(1));
        assert!(findings.is_empty());
    }

    /// 真实平台接线：死 pid 清理、活 pid（本测试进程）不在 node 镜像名单里时
    /// 也会清（那是 pid 复用），读不到的锁不动。
    #[test]
    fn heal_with_real_platform_wiring() {
        let platform = crate::platform::current();
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let dead = home.join(".credentials.yaml.lock");
        fs::write(&dead, "4294967295\n").unwrap();
        let fresh = home.join(".settings.yaml.lock");
        fs::write(&fresh, "").unwrap();

        let findings = heal_stale_locks(home, platform.as_ref());

        assert!(!dead.exists(), "不存在的 pid 必须清掉（这正是机器 B 的锁）");
        assert!(fresh.exists(), "内容空缺年龄未确认的锁不能动");
        assert!(findings.iter().any(|f| f.action == LockAction::RemovedStaleOwner));
    }

    #[test]
    fn log_line_never_leaks_lock_body() {
        let finding = LockFinding {
            path: PathBuf::from(r"C:\x\dsh-home\.credentials.yaml.lock"),
            pid: Some(1234),
            action: LockAction::RemovedStaleOwner,
        };
        let line = finding.log_line();
        assert!(line.contains(".credentials.yaml.lock"));
        assert!(line.contains("1234"));
        assert!(!line.contains('\n'), "单行，别把日志拆断");
    }
}
