//! 命令注册三处同步锚定：build.rs（AppManifest::commands）/ capabilities/default.json
//! （allow-* 逐项放行）/ lib.rs invoke_handler。远程 IPC 一律走 ACL 的副作用是本地
//! 命令也全部 ACL 化——任何一处漏登记，前端 invoke 就是 silent 失败（0.4.x 实踩过
//! exitCode 恒 undefined 的同类问题）。
use std::collections::BTreeSet;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// build.rs 里 commands(&[...]) 块中的全部命令名（snake_case）
fn build_rs_commands() -> BTreeSet<String> {
    let text = std::fs::read_to_string(manifest_dir().join("build.rs")).unwrap();
    let start = text.find("commands(&[").expect("build.rs 缺 commands 块");
    let block = &text[start..text[start..].find("])").map(|i| start + i).unwrap()];
    block
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// capabilities/<file> 里 allow-* 权限对应的命令名（kebab → snake）
fn capability_commands(file: &str) -> BTreeSet<String> {
    let text = std::fs::read_to_string(manifest_dir().join("capabilities").join(file)).unwrap();
    text.split('"')
        .filter_map(|tok| {
            tok.strip_prefix("allow-")
                .map(|kebab| kebab.replace('-', "_"))
        })
        .collect()
}

/// lib.rs invoke_handler! 块中的命令名（`module::name,` 取末段）
fn invoke_handler_commands() -> BTreeSet<String> {
    let text = std::fs::read_to_string(manifest_dir().join("src").join("lib.rs")).unwrap();
    let start = text.find("generate_handler![").expect("lib.rs 缺 invoke_handler 块");
    let block = &text[start..text[start..].find(']').map(|i| start + i).unwrap()];
    block
        .lines()
        .filter_map(|l| {
            let l = l.trim().trim_end_matches(',');
            l.rsplit("::").next().filter(|name| {
                !name.is_empty()
                    && l.contains("::")
                    && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            })
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn commands_registered_in_all_three_places() {
    let build = build_rs_commands();
    let caps = capability_commands("default.json");
    let handlers = invoke_handler_commands();
    assert!(
        !build.is_empty() && !caps.is_empty() && !handlers.is_empty(),
        "三处命令清单都应解析出非空集合（build={} caps={} handlers={}）",
        build.len(),
        caps.len(),
        handlers.len()
    );
    assert_eq!(
        build, caps,
        "build.rs 与 capabilities/default.json 不一致：{:?} vs {:?}",
        build, caps
    );
    assert_eq!(
        build, handlers,
        "build.rs 与 lib.rs invoke_handler 不一致：{:?} vs {:?}",
        build, handlers
    );
}

#[test]
fn remote_capability_only_exposes_zoom() {
    // dsh-remote.json 只对远程 dsh 源开放 zoom_ui；多开任何命令都是远程面扩大
    let remote = capability_commands("dsh-remote.json");
    assert_eq!(
        remote,
        BTreeSet::from(["zoom_ui".to_string()]),
        "远程 capability 应只放行 zoom_ui，实际：{remote:?}"
    );
}
