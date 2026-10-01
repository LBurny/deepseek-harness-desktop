//! 插件命令端到端：真实 dsh bin.js 的 plugin 模式 + 假 pnpm.cmd。
//! 断言：profile 初始化、pnpm 收到正确参数且 cwd=profile 目录、退出码透传、
//! DSH_HOME/PATH 注入生效。无运行时（runtime/windows-x64 缺失）则 skip。

use dshdesktop_lib::bootstrap::plugins::{
    heal_profile_store, install_plugin_impl, uninstall_plugin_impl, update_plugins_impl,
    PluginsHome, StoreHealOutcome,
};
use dshdesktop_lib::dsh::upstream::dsh_bin;
use std::path::PathBuf;
use std::sync::Mutex;

fn runtime_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DSHDESKTOP_RUNTIME_DIR") {
        return Some(PathBuf::from(d));
    }
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("windows-x64");
    p.is_dir().then_some(p)
}

fn system_node() -> PathBuf {
    let out = std::process::Command::new("where").arg("node").output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    PathBuf::from(stdout.lines().next().expect("node not found on PATH").trim())
}

/// 假 pnpm.cmd：把 cwd 与参数追加写进 pnpm.log（%~dp0 = pnpm.cmd 所在目录，
/// 无需环境变量注入）；存在 exit-1 控制文件时退出码 1。
/// `echo(` 语法避免 `echo %CD% >>` 在行尾追加空格。
const FAKE_PNPM_CMD: &str = "@echo off\r\necho(%CD%>>\"%~dp0pnpm.log\"\r\necho(%*>>\"%~dp0pnpm.log\"\r\nif exist \"%~dp0exit-1\" exit /b 1\r\nexit /b 0\r\n";

/// 假 pnpm.cjs：壳侧 store 自愈直接经 node 调它（绕过 .cmd）。按首个参数分支：
/// `store path` 打印 store.txt 内容（模拟用户全局配置解析出的 store）；
/// `install` 在 cwd 建 node_modules（模拟重链成功）并服从 exit-1 控制。
/// 参数与结果都追加进同一个 pnpm.log 供断言。
const FAKE_PNPM_CJS: &str = "const fs = require('fs');\r\nconst path = require('path');\r\nconst args = process.argv.slice(2);\r\nconst root = path.join(__dirname, '..', '..');\r\nfs.appendFileSync(path.join(root, 'pnpm.log'), args.join(' ') + '\\n');\r\nif (args[0] === 'store') {\r\n  try { console.log(fs.readFileSync(path.join(root, 'store.txt'), 'utf8').trim()); } catch (e) { console.log(''); }\r\n  process.exit(0);\r\n}\r\nif (args[0] === 'install') {\r\n  fs.mkdirSync(path.join(process.cwd(), 'node_modules'), { recursive: true });\r\n  const st = fs.readFileSync(path.join(root, 'store.txt'), 'utf8').trim();\r\n  fs.writeFileSync(path.join(process.cwd(), 'node_modules', '.modules.yaml'), JSON.stringify({ storeDir: st }));\r\n  process.exit(fs.existsSync(path.join(root, 'exit-1')) ? 1 : 0);\r\n}\r\nprocess.exit(0);\r\n";

fn test_home(tag: &str) -> (PluginsHome, PathBuf) {
    let work = std::env::temp_dir().join(format!("dshd-plugins-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("bin").join("pnpm").join("bin")).unwrap();
    std::fs::write(work.join("bin").join("pnpm.cmd"), FAKE_PNPM_CMD).unwrap();
    std::fs::write(
        work.join("bin").join("pnpm").join("bin").join("pnpm.cjs"),
        FAKE_PNPM_CJS,
    )
    .unwrap();
    let home = PluginsHome {
        node_exe: system_node(),
        dsh_bin: dsh_bin(&runtime_dir().expect("runtime dir")),
        home: work.join("home"),
        pnpm_dir: work.join("bin"),
        busy: Mutex::new(()),
    };
    (home, work)
}

#[test]
fn install_forwards_args_with_profile_cwd_and_inits_profile() {
    let Some(_rt) = runtime_dir() else {
        eprintln!("skipped: 无真实运行时");
        return;
    };
    let (home, work) = test_home("install");
    let res = install_plugin_impl(&home, "some-pkg").unwrap();
    assert_eq!(res.exit_code, 0, "输出：{}", res.output);
    let log = std::fs::read_to_string(home.pnpm_dir.join("pnpm.log")).unwrap();
    // 0.2.0 起 dsh 在 add 前先跑一次 npm 元数据探测（pnpm view <pkg> …），且对
    // pnpm 的每个参数都加引号——断言不钉探测步、引号剥离后匹配，两次调用都须
    // 落在 profile 目录。
    let plain = log.replace('"', "");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines[0], home.profile_dir().display().to_string(), "cwd 应为 profile 目录");
    assert!(plain.contains("add some-pkg"), "add 参数应原样透传，日志：{log}");
    assert!(plain.lines().all(|l| l == "add some-pkg"
        || l.starts_with("view some-pkg")
        || l.contains("profiles")
        || l.starts_with("install some-pkg")), "意外的 pnpm 调用：{plain}");
    assert!(home.manifest_path().is_file(), "dsh 应初始化 profile 清单");
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn uninstall_and_update_forward_verbatim() {
    let Some(_rt) = runtime_dir() else {
        eprintln!("skipped");
        return;
    };
    let (home, work) = test_home("others");
    assert_eq!(uninstall_plugin_impl(&home, "foo").unwrap().exit_code, 0);
    assert_eq!(update_plugins_impl(&home).unwrap().exit_code, 0);
    let log = std::fs::read_to_string(home.pnpm_dir.join("pnpm.log")).unwrap();
    let plain = log.replace('"', "");
    assert!(plain.contains("remove foo"), "日志：{log}");
    assert!(plain.contains("update"), "日志：{log}");
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn pnpm_failure_exit_code_passthrough() {
    let Some(_rt) = runtime_dir() else {
        eprintln!("skipped");
        return;
    };
    let (home, work) = test_home("fail");
    std::fs::write(home.pnpm_dir.join("exit-1"), "").unwrap();
    let res = install_plugin_impl(&home, "boom").unwrap();
    assert_eq!(res.exit_code, 1);
    let _ = std::fs::remove_dir_all(&work);
}

/// store 迁移自愈：.modules.yaml 记录的 storeDir 与内置 pnpm 解析值不一致时，
/// rename 备用 → install 重链 → 成功删备份；失败回滚（node_modules 原样还原）。
/// store 的"当前解析值"由假 pnpm.cjs 的 store.txt 控制，install 由它合成。
#[test]
fn store_migration_heal_flows() {
    let (home, work) = test_home("storeheal");
    let profile = home.profile_dir();
    let nm = profile.join("node_modules");
    let stale = profile.join("node_modules.dshdesktop-stale");
    std::fs::create_dir_all(&nm).unwrap();
    std::fs::write(nm.join(".modules.yaml"), r#"{"storeDir": "C:\\old-store\\v11"}"#).unwrap();
    let log_path = home.pnpm_dir.join("pnpm.log");
    let log = || std::fs::read_to_string(&log_path).unwrap_or_default();

    // ① storeDir 与内置 pnpm 解析一致 → Matched，不跑 install
    std::fs::write(home.pnpm_dir.join("store.txt"), "C:\\old-store\\v11\n").unwrap();
    assert_eq!(heal_profile_store(&home), StoreHealOutcome::Matched);
    assert!(!log().contains("install"), "日志：{}", log());

    // ② 迁移 → Healed：fake install 建新 node_modules、备份已清、日志有 install
    std::fs::write(home.pnpm_dir.join("store.txt"), "F:\\new-store\\v11\n").unwrap();
    assert_eq!(
        heal_profile_store(&home),
        StoreHealOutcome::Healed {
            old_store: "C:\\old-store\\v11".into(),
            new_store: "F:\\new-store\\v11".into(),
        }
    );
    assert!(log().contains("install"), "日志：{}", log());
    assert!(nm.is_dir(), "install 应重建 node_modules");
    assert!(nm.join(".modules.yaml").is_file(), "重链后应记录新 storeDir（真 pnpm 同款）");
    assert!(!stale.exists(), "成功后备用目录应删除");

    // ③ 再迁移 + install 失败 → 回滚：迁移前的 node_modules 原样还原
    std::fs::write(home.pnpm_dir.join("store.txt"), "G:\\third-store\\v11\n").unwrap();
    std::fs::write(home.pnpm_dir.join("exit-1"), "").unwrap();
    let outcome = heal_profile_store(&home);
    assert!(matches!(outcome, StoreHealOutcome::RolledBack { .. }), "实际 {outcome:?}");
    assert!(nm.join(".modules.yaml").is_file(), "回滚后 .modules.yaml 应还原");
    assert!(
        std::fs::read_to_string(nm.join(".modules.yaml")).unwrap().contains("new-store"),
        "还原的是迁移前那份（F:），不是半成品"
    );
    assert!(!stale.exists(), "回滚后备用目录应清掉");
    std::fs::remove_file(home.pnpm_dir.join("exit-1")).unwrap();

    // ④ 无 node_modules（首启/profile 未初始化）→ NoProfile
    std::fs::remove_dir_all(&nm).unwrap();
    assert_eq!(heal_profile_store(&home), StoreHealOutcome::NoProfile);

    let _ = std::fs::remove_dir_all(&work);
}
