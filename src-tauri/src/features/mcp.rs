//! MCP server 管理：读写 <dsh-home>/profiles/web/cordis.patch.yml 中
//! name == '@deepseek-ai/dsh-mcp-client' 的 insert 条目。dsh 的 HMR
//! （dsh-app-boot watchUserPatches + chokidar）监听该文件，改后自动
//! disconnect+reconnect，无需重启。
//!
//! 启停语义：cordis-plugin-loader 原生支持 entry 级 disabled: true
//! （disabled 的 entry 不启动 fiber），热重载后生效。
//! 通用 patch 条目读写在 patchstore.rs（框架重组时拆出）。

use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::State;

/// dsh 实际使用的 home（DSH_HOME），与 skills::SkillsHome 同源
pub struct McpHome(pub PathBuf);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub server_name: String,
    pub transport: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRow {
    pub server_name: String,
    pub transport: String,
    pub summary: String,
    pub enabled: bool,
    pub config: McpServerConfig, // 编辑表单预填用完整配置
}

/// 所有 insert 列表里的 MCP 插件条目位置：(op 索引, insert 列表内索引)
fn mcp_positions(entries: &[Value]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (oi, op) in entries.iter().enumerate() {
        if let Some(list) = op.get(crate::dsh::upstream::CORDIS_OP_INSERT).and_then(Value::as_sequence) {
            for (ii, e) in list.iter().enumerate() {
                if e.get("name").and_then(Value::as_str) == Some(crate::dsh::upstream::MCP_PLUGIN_NAME) {
                    out.push((oi, ii));
                }
            }
        }
    }
    out
}

fn entry_at<'a>(entries: &'a [Value], pos: (usize, usize)) -> &'a Value {
    &entries[pos.0][crate::dsh::upstream::CORDIS_OP_INSERT][pos.1]
}

fn to_row(e: &Value) -> McpServerRow {
    let cfg = &e["config"];
    let transport = cfg["transport"].as_str().unwrap_or("stdio").to_string();
    let summary = if transport == "streamable-http" {
        cfg["url"].as_str().unwrap_or("").to_string()
    } else {
        let cmd = cfg["command"].as_str().unwrap_or("").to_string();
        let args = cfg["args"]
            .as_sequence()
            .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        [cmd, args]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let str_map = |v: &Value| -> BTreeMap<String, String> {
        v.as_mapping()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.as_str()?.to_string(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    let config = McpServerConfig {
        server_name: cfg["serverName"].as_str().unwrap_or("").to_string(),
        transport: transport.clone(),
        command: cfg["command"].as_str().map(str::to_string),
        args: cfg["args"]
            .as_sequence()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        env: str_map(&cfg["env"]),
        cwd: cfg["cwd"].as_str().map(str::to_string),
        url: cfg["url"].as_str().map(str::to_string),
        headers: str_map(&cfg["headers"]),
    };
    McpServerRow {
        server_name: config.server_name.clone(),
        transport,
        summary,
        enabled: !matches!(
            e.get(crate::dsh::upstream::CORDIS_ENTRY_DISABLED),
            Some(Value::Bool(true))
        ),
        config,
    }
}

fn list_servers(home: &Path) -> Result<Vec<McpServerRow>, String> {
    let entries = crate::patchstore::read_patch(&crate::patchstore::patch_path(home))?;
    Ok(mcp_positions(&entries)
        .iter()
        .map(|&p| to_row(entry_at(&entries, p)))
        .collect())
}

/// （picker.rs 共存测试/诊断）列出全部 serverName；解析失败按空处理
#[cfg(test)]
pub(crate) fn list_servers_in(home: &Path) -> Vec<String> {
    list_servers(home)
        .map(|rows| rows.into_iter().map(|r| r.server_name).collect())
        .unwrap_or_default()
}

fn valid_server_name(n: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[A-Za-z0-9_-]{1,32}$").unwrap()).is_match(n)
}

fn find_server(entries: &[Value], server_name: &str) -> Option<(usize, usize)> {
    mcp_positions(entries).into_iter().find(|&p| {
        entry_at(entries, p)["config"]["serverName"].as_str() == Some(server_name)
    })
}

fn set_server_enabled(home: &Path, server_name: &str, enabled: bool) -> Result<(), String> {
    let path = crate::patchstore::patch_path(home);
    let mut entries = crate::patchstore::read_patch(&path)?;
    let pos = find_server(&entries, server_name).ok_or_else(|| {
        crate::i18n::pick(
            format!("MCP server 不存在: {server_name}"),
            format!("MCP server not found: {server_name}"),
        )
    })?;
    let e = entries[pos.0][crate::dsh::upstream::CORDIS_OP_INSERT]
        .as_sequence_mut()
        .and_then(|s| s.get_mut(pos.1))
        .unwrap();
    let map = e.as_mapping_mut().unwrap();
    if enabled {
        map.remove(Value::String(crate::dsh::upstream::CORDIS_ENTRY_DISABLED.into()));
    } else {
        map.insert(
            Value::String(crate::dsh::upstream::CORDIS_ENTRY_DISABLED.into()),
            Value::Bool(true),
        );
    }
    crate::patchstore::write_patch(&path, &entries)
}

fn delete_server(home: &Path, server_name: &str) -> Result<(), String> {
    let path = crate::patchstore::patch_path(home);
    let mut entries = crate::patchstore::read_patch(&path)?;
    let (oi, ii) = find_server(&entries, server_name).ok_or_else(|| {
        crate::i18n::pick(
            format!("MCP server 不存在: {server_name}"),
            format!("MCP server not found: {server_name}"),
        )
    })?;
    entries[oi][crate::dsh::upstream::CORDIS_OP_INSERT]
        .as_sequence_mut()
        .unwrap()
        .remove(ii);
    if entries[oi][crate::dsh::upstream::CORDIS_OP_INSERT]
        .as_sequence()
        .is_some_and(|s| s.is_empty())
    {
        entries.remove(oi); // 该 op 只插了这一个条目：连同 op 一起删
    }
    crate::patchstore::write_patch(&path, &entries)
}

fn upsert_server(home: &Path, original: Option<&str>, cfg: &McpServerConfig) -> Result<(), String> {
    if !valid_server_name(&cfg.server_name) {
        return Err(crate::i18n::pick(
            format!(
                "serverName 须匹配 [A-Za-z0-9_-]{{1,32}}: {:?}",
                cfg.server_name
            ),
            format!(
                "serverName must match [A-Za-z0-9_-]{{1,32}}: {:?}",
                cfg.server_name
            ),
        ));
    }
    let stdio = match cfg.transport.as_str() {
        "stdio" => true,
        "streamable-http" => false,
        t => {
            return Err(crate::i18n::pick(
                format!("不支持的 transport: {t}（仅 stdio / streamable-http）"),
                format!("Unsupported transport: {t} (only stdio / streamable-http)"),
            ))
        }
    };
    if stdio && cfg.command.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(crate::i18n::pick("stdio 需要 command", "stdio requires a command").into());
    }
    if !stdio && cfg.url.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(crate::i18n::pick("streamable-http 需要 url", "streamable-http requires a url").into());
    }
    let path = crate::patchstore::patch_path(home);
    let mut entries = crate::patchstore::read_patch(&path)?;
    let edit_pos = original.and_then(|o| find_server(&entries, o));
    // 唯一性：除被编辑条目自身外，不得占用该 serverName
    if let Some(p) = find_server(&entries, &cfg.server_name) {
        if Some(p) != edit_pos {
            return Err(crate::i18n::pick(
                format!("serverName 已存在: {}", cfg.server_name),
                format!("serverName already exists: {}", cfg.server_name),
            ));
        }
    }
    if original.is_some() && edit_pos.is_none() {
        return Err(crate::i18n::pick(
            format!("MCP server 不存在: {}", original.unwrap()),
            format!("MCP server not found: {}", original.unwrap()),
        ));
    }
    // 编辑时以旧 config 为底保留高级键（toolCallTimeoutMs/reconnect 等）；新建用空 mapping
    let mut map = edit_pos
        .and_then(|p| entry_at(&entries, p)["config"].as_mapping().cloned())
        .unwrap_or_default();
    for k in ["command", "args", "env", "cwd", "url", "headers"] {
        map.remove(Value::String(k.into()));
    }
    map.insert(
        Value::String("serverName".into()),
        Value::String(cfg.server_name.clone()),
    );
    map.insert(
        Value::String("transport".into()),
        Value::String(cfg.transport.clone()),
    );
    let put_str = |map: &mut serde_yaml::Mapping, k: &str, v: &Option<String>| {
        if let Some(v) = v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            map.insert(Value::String(k.into()), Value::String(v.to_string()));
        }
    };
    let put_map = |map: &mut serde_yaml::Mapping, k: &str, v: &BTreeMap<String, String>| {
        if !v.is_empty() {
            map.insert(Value::String(k.into()), serde_yaml::to_value(v).unwrap());
        }
    };
    if stdio {
        put_str(&mut map, "command", &cfg.command);
        if !cfg.args.is_empty() {
            map.insert(
                Value::String("args".into()),
                cfg.args.iter().map(|a| Value::String(a.clone())).collect(),
            );
        }
        put_map(&mut map, "env", &cfg.env);
        put_str(&mut map, "cwd", &cfg.cwd);
    } else {
        put_str(&mut map, "url", &cfg.url);
        put_map(&mut map, "headers", &cfg.headers);
    }

    if let Some(p) = edit_pos {
        // 原地改：保留旧 disabled 标志；重命名时更新 id
        let e = entries[p.0][crate::dsh::upstream::CORDIS_OP_INSERT]
            .as_sequence_mut()
            .and_then(|s| s.get_mut(p.1))
            .unwrap();
        let m = e.as_mapping_mut().unwrap();
        m.insert(Value::String("config".into()), Value::Mapping(map));
        if original != Some(cfg.server_name.as_str()) {
            m.insert(
                Value::String("id".into()),
                Value::String(format!("mcp-{}", cfg.server_name)),
            );
        }
        return crate::patchstore::write_patch(&path, &entries);
    }
    let mut e = serde_yaml::Mapping::new();
    e.insert(
        Value::String("id".into()),
        Value::String(format!("mcp-{}", cfg.server_name)),
    );
    e.insert(
        Value::String("name".into()),
        Value::String(crate::dsh::upstream::MCP_PLUGIN_NAME.into()),
    );
    e.insert(Value::String("config".into()), Value::Mapping(map));
    let mut op = serde_yaml::Mapping::new();
    op.insert(
        Value::String(crate::dsh::upstream::CORDIS_OP_INSERT.into()),
        Value::Sequence(vec![Value::Mapping(e)]),
    );
    entries.push(Value::Mapping(op));
    crate::patchstore::write_patch(&path, &entries)
}

/// 启动时把独立 dsh（~/.dsh）两个 patch 层里的 MCP server 同步进壳。
/// 与技能种子同理：用户心智"我就是 dsh"；marker 防复活、不覆盖已有。
pub fn seed_from_default_dsh_home(home: &Path) {
    let Some(user_home) = dirs::home_dir() else {
        return;
    };
    if let Err(e) = seed_auto_import(&user_home.join(".dsh"), home) {
        // events.log 在 home 的父目录（runtime_base_dir）
        if let Some(base) = home.parent() {
            crate::logging::append_debug_line(&base.join("events.log"), &format!("mcp seed failed: {e}"));
        }
    }
}

fn seed_auto_import(user_dsh_home: &Path, home: &Path) -> Result<(), String> {
    use std::collections::HashSet;
    let marker = home.join(".mcp-seeded");
    let mut seen: HashSet<String> = fs::read_to_string(&marker)
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let target_path = crate::patchstore::patch_path(home);
    let mut target = crate::patchstore::read_patch(&target_path)?;
    let mut changed = false;
    let layers = [
        crate::dsh::upstream::join_segments(user_dsh_home, crate::dsh::upstream::MCP_PATCH_SEGMENTS),
        user_dsh_home.join("cordis.patch.yml"),
    ];
    for path in layers {
        // 解析失败（如含无法处理的语法）的源文件跳过，不影响另一层
        let Ok(entries) = crate::patchstore::read_patch(&path) else {
            continue;
        };
        for pos in mcp_positions(&entries) {
            let e = entry_at(&entries, pos);
            if e.get(crate::dsh::upstream::CORDIS_ENTRY_DISABLED).is_some() {
                continue; // 源里禁用（含 !!js 表达式）的不同步，也不记 marker
            }
            let Some(name) = e["config"]["serverName"].as_str().map(str::to_string) else {
                continue;
            };
            if !valid_server_name(&name) || !seen.insert(name.clone()) {
                continue;
            }
            if find_server(&target, &name).is_none() {
                let mut op = serde_yaml::Mapping::new();
                op.insert(
                    Value::String(crate::dsh::upstream::CORDIS_OP_INSERT.into()),
                    Value::Sequence(vec![e.clone()]),
                );
                target.push(Value::Mapping(op));
                changed = true;
            }
        }
    }
    if changed {
        crate::patchstore::write_patch(&target_path, &target)?;
    }
    let mut names: Vec<_> = seen.into_iter().collect();
    names.sort();
    fs::create_dir_all(home).map_err(|e| e.to_string())?;
    fs::write(&marker, names.join("\n") + "\n").map_err(|e| e.to_string())
}

// ---- 从其它工具导入 ----

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpSourceServer {
    pub name: String,
    pub transport: String,
    pub summary: String,
    pub supported: bool,
    pub reason: Option<String>,
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct McpSourceInfo {
    pub id: String,
    pub label: String,
    pub exists: bool,
    pub servers: Vec<McpSourceServer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpImportItem {
    pub name: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct McpImportResult {
    pub name: String,
    pub status: String, // "imported" | "skipped" | "error"
    pub error: Option<String>,
}

fn mcp_sources() -> Vec<(String, String, PathBuf)> {
    let Some(h) = dirs::home_dir() else {
        return Vec::new();
    };
    [
        ("claude", "Claude Code", h.join(".claude.json")),
        ("codex", "Codex", h.join(".codex/config.toml")),
        ("opencode", "OpenCode", h.join(".config/opencode/opencode.json")),
    ]
    .into_iter()
    .map(|(id, label, p)| (id.to_string(), label.to_string(), p))
    .collect()
}

fn base_config(name: &str, transport: &str) -> McpServerConfig {
    McpServerConfig {
        server_name: name.to_string(),
        transport: transport.to_string(),
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        url: None,
        headers: BTreeMap::new(),
    }
}

fn json_str_map(v: &serde_json::Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn json_str_vec(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// 解析一个导入源文件为 (name, Result<config, 不支持原因>) 列表
fn parse_source(source_id: &str, text: &str) -> Vec<(String, Result<McpServerConfig, String>)> {
    match source_id {
        "claude" => parse_claude(text),
        "codex" => parse_codex(text),
        "opencode" => parse_opencode(text),
        _ => Vec::new(),
    }
}

fn parse_claude(text: &str) -> Vec<(String, Result<McpServerConfig, String>)> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(servers) = json["mcpServers"].as_object() else {
        return Vec::new();
    };
    servers
        .iter()
        .map(|(name, v)| {
            let ty = v["type"]
                .as_str()
                .unwrap_or(if v["url"].is_string() { "http" } else { "stdio" });
            let cfg = match ty {
                "stdio" => {
                    let mut c = base_config(name, "stdio");
                    c.command = v["command"].as_str().map(str::to_string);
                    c.args = json_str_vec(&v["args"]);
                    c.env = json_str_map(&v["env"]);
                    Ok(c)
                }
                "http" => {
                    let mut c = base_config(name, "streamable-http");
                    c.url = v["url"].as_str().map(str::to_string);
                    c.headers = json_str_map(&v["headers"]);
                    Ok(c)
                }
                other => Err(crate::i18n::pick(
                    format!("dsh 不支持该类型（{other}），仅 stdio / streamable-http"),
                    format!("dsh does not support this type ({other}), only stdio / streamable-http"),
                )),
            };
            (name.clone(), cfg)
        })
        .collect()
}

fn parse_codex(text: &str) -> Vec<(String, Result<McpServerConfig, String>)> {
    let Ok(doc) = toml::from_str::<toml::Value>(text) else {
        return Vec::new();
    };
    let Some(servers) = doc.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    servers
        .iter()
        .filter(|(_, v)| v.get("enabled").and_then(toml::Value::as_bool) != Some(false))
        .map(|(name, v)| {
            let mut c = base_config(name, "stdio"); // codex 只有 stdio
            c.command = v.get("command").and_then(toml::Value::as_str).map(str::to_string);
            c.args = v
                .get("args")
                .and_then(toml::Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            c.env = v
                .get("env")
                .and_then(toml::Value::as_table)
                .map(|t| {
                    t.iter()
                        .filter_map(|(k, x)| Some((k.clone(), x.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let r = if c.command.is_none() {
                Err(crate::i18n::pick("Codex 条目缺少 command", "Codex entry is missing command"))
            } else {
                Ok(c)
            };
            (name.clone(), r)
        })
        .collect()
}

fn parse_opencode(text: &str) -> Vec<(String, Result<McpServerConfig, String>)> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(servers) = json["mcp"].as_object() else {
        return Vec::new();
    };
    servers
        .iter()
        .filter(|(_, v)| v["enabled"].as_bool() != Some(false))
        .map(|(name, v)| {
            let cfg = match v["type"].as_str() {
                Some("local") => {
                    let cmd = json_str_vec(&v["command"]);
                    let mut c = base_config(name, "stdio");
                    c.command = cmd.first().cloned();
                    c.args = cmd[1..].to_vec();
                    c.env = json_str_map(&v["environment"]);
                    if c.command.is_none() {
                        Err(crate::i18n::pick(
                            "OpenCode local 条目缺少 command",
                            "OpenCode local entry is missing command",
                        ))
                    } else {
                        Ok(c)
                    }
                }
                Some("remote") => {
                    let mut c = base_config(name, "streamable-http");
                    c.url = v["url"].as_str().map(str::to_string);
                    c.headers = json_str_map(&v["headers"]);
                    Ok(c)
                }
                other => Err(crate::i18n::pick(
                    format!(
                        "dsh 不支持该类型（{}），仅 local / remote",
                        other.unwrap_or("?")
                    ),
                    format!(
                        "dsh does not support this type ({}), only local / remote",
                        other.unwrap_or("?")
                    ),
                )),
            };
            (name.clone(), cfg)
        })
        .collect()
}

fn config_summary(c: &McpServerConfig) -> String {
    if c.transport == "streamable-http" {
        c.url.clone().unwrap_or_default()
    } else {
        [c.command.clone().unwrap_or_default(), c.args.join(" ")]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn apply_imported(
    home: &Path,
    parsed: &[(String, Result<McpServerConfig, String>)],
    items: &[McpImportItem],
) -> Vec<McpImportResult> {
    items
        .iter()
        .map(|item| {
            let name = item.name.clone();
            let err = |e: String| McpImportResult {
                name: name.clone(),
                status: "error".into(),
                error: Some(e),
            };
            let Some((_, cfg)) = parsed.iter().find(|(n, _)| *n == item.name) else {
                return err(crate::i18n::pick(
                    "源配置中不存在该 server",
                    "Server not found in the source config",
                ));
            };
            let cfg = match cfg {
                Ok(c) => c,
                Err(e) => return err(e.clone()),
            };
            let Ok(entries) = crate::patchstore::read_patch(&crate::patchstore::patch_path(home)) else {
                return err(crate::i18n::pick(
                    "cordis.patch.yml 解析失败，请手工编辑",
                    "Failed to parse cordis.patch.yml, please edit manually",
                ));
            };
            let conflict = find_server(&entries, &name).is_some();
            if conflict && !item.overwrite {
                return McpImportResult { name, status: "skipped".into(), error: None };
            }
            if conflict {
                let _ = delete_server(home, &name); // 覆盖 = 全新配置，不保留旧高级键
            }
            match upsert_server(home, None, cfg) {
                Ok(()) => McpImportResult { name, status: "imported".into(), error: None },
                Err(e) => err(e),
            }
        })
        .collect()
}

// ---- Tauri 命令 ----

#[tauri::command]
pub fn list_mcp_servers(state: State<McpHome>) -> Result<Vec<McpServerRow>, String> {
    list_servers(&state.0)
}

#[tauri::command]
pub fn upsert_mcp_server(
    state: State<McpHome>,
    original_name: Option<String>,
    config: McpServerConfig,
) -> Result<(), String> {
    upsert_server(&state.0, original_name.as_deref(), &config)
}

#[tauri::command]
pub fn set_mcp_enabled(
    state: State<McpHome>,
    server_name: String,
    enabled: bool,
) -> Result<(), String> {
    set_server_enabled(&state.0, &server_name, enabled)
}

#[tauri::command]
pub fn delete_mcp_server(state: State<McpHome>, server_name: String) -> Result<(), String> {
    delete_server(&state.0, &server_name)
}

#[tauri::command]
pub fn list_mcp_import_sources(state: State<McpHome>) -> Vec<McpSourceInfo> {
    mcp_sources()
        .iter()
        .map(|(id, label, path)| {
            let exists = path.is_file();
            let servers = if exists {
                let parsed = fs::read_to_string(path)
                    .map(|t| parse_source(id, &t))
                    .unwrap_or_default();
                let existing = crate::patchstore::read_patch(&crate::patchstore::patch_path(&state.0)).unwrap_or_default();
                parsed
                    .into_iter()
                    .map(|(name, cfg)| {
                        let (transport, summary, supported, reason) = match &cfg {
                            Ok(c) => (c.transport.clone(), config_summary(c), true, None),
                            Err(e) => (String::new(), String::new(), false, Some(e.clone())),
                        };
                        let conflict = find_server(&existing, &name).is_some();
                        McpSourceServer { name, transport, summary, supported, reason, conflict }
                    })
                    .collect()
            } else {
                Vec::new()
            };
            McpSourceInfo { id: id.clone(), label: label.clone(), exists, servers }
        })
        .collect()
}

#[tauri::command]
pub fn import_mcp_servers(
    state: State<McpHome>,
    source_id: String,
    items: Vec<McpImportItem>,
) -> Vec<McpImportResult> {
    let Some((.., path)) = mcp_sources().into_iter().find(|(id, ..)| *id == source_id) else {
        return items
            .iter()
            .map(|i| McpImportResult {
                name: i.name.clone(),
                status: "error".into(),
                error: Some(crate::i18n::pick("未知导入源", "Unknown import source")),
            })
            .collect();
    };
    let parsed = fs::read_to_string(&path)
        .map(|t| parse_source(&source_id, &t))
        .unwrap_or_default();
    apply_imported(&state.0, &parsed, &items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn write_patch_str(home: &Path, s: &str) {
        let p = crate::patchstore::patch_path(home);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    const TWO_SERVERS: &str = r#"
- insert:
    - id: mcp-playwright
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: playwright
        transport: stdio
        command: npx
        args: ['-y', '@playwright/mcp']
- insert:
    - id: mcp-docs
      name: '@deepseek-ai/dsh-mcp-client'
      disabled: true
      config:
        serverName: docs
        transport: streamable-http
        url: https://example.com/mcp
"#;

    #[test]
    fn list_reads_both_transports_and_disabled() {
        let t = tempfile::tempdir().unwrap();
        write_patch_str(t.path(), TWO_SERVERS);
        let rows = list_servers(t.path()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].server_name, "playwright");
        assert_eq!(rows[0].summary, "npx -y @playwright/mcp");
        assert!(rows[0].enabled);
        assert_eq!(rows[0].config.args, vec!["-y", "@playwright/mcp"]); // 编辑预填用完整配置
        assert_eq!(rows[1].server_name, "docs");
        assert_eq!(rows[1].transport, "streamable-http");
        assert_eq!(rows[1].summary, "https://example.com/mcp");
        assert!(!rows[1].enabled);
    }

    #[test]
    fn list_missing_file_is_empty() {
        let t = tempfile::tempdir().unwrap();
        assert!(list_servers(t.path()).unwrap().is_empty());
    }

    #[test]
    fn list_ignores_non_mcp_entries() {
        let t = tempfile::tempdir().unwrap();
        write_patch_str(t.path(), "- insert:\n    - id: other\n      name: 'some-plugin'\n");
        assert!(list_servers(t.path()).unwrap().is_empty());
    }

    #[test]
    fn list_bom_tolerated() {
        let t = tempfile::tempdir().unwrap();
        write_patch_str(
            t.path(),
            "\u{feff}- insert:\n    - id: mcp-a\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: a\n        transport: stdio\n        command: x\n",
        );
        assert_eq!(list_servers(t.path()).unwrap().len(), 1);
    }

    // ---- 写操作 ----

    fn cfg_stdio(name: &str) -> McpServerConfig {
        McpServerConfig {
            server_name: name.into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            args: vec!["-y".into(), "pkg".into()],
            env: BTreeMap::from([("API_KEY".into(), "sk-1".into())]),
            cwd: None,
            url: None,
            headers: BTreeMap::new(),
        }
    }

    #[test]
    fn toggle_writes_disabled_flag() {
        let t = tempfile::tempdir().unwrap();
        write_patch_str(t.path(), TWO_SERVERS);
        set_server_enabled(t.path(), "playwright", false).unwrap();
        let rows = list_servers(t.path()).unwrap();
        assert!(!rows[0].enabled && !rows[1].enabled);
        set_server_enabled(t.path(), "docs", true).unwrap();
        assert!(list_servers(t.path()).unwrap()[1].enabled);
        assert!(set_server_enabled(t.path(), "ghost", true).is_err());
    }

    #[test]
    fn delete_removes_entry_and_empty_op() {
        let t = tempfile::tempdir().unwrap();
        write_patch_str(t.path(), TWO_SERVERS);
        delete_server(t.path(), "playwright").unwrap();
        let text = fs::read_to_string(crate::patchstore::patch_path(t.path())).unwrap();
        assert!(!text.contains("playwright") && text.contains("docs"));
        delete_server(t.path(), "docs").unwrap();
        assert_eq!(fs::read_to_string(crate::patchstore::patch_path(t.path())).unwrap(), "[]\n");
        assert!(delete_server(t.path(), "ghost").is_err());
    }

    #[test]
    fn upsert_add_validates_and_writes() {
        let t = tempfile::tempdir().unwrap();
        upsert_server(t.path(), None, &cfg_stdio("ctx7")).unwrap();
        let rows = list_servers(t.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].summary, "npx -y pkg");
        // 重名拒绝
        assert!(upsert_server(t.path(), None, &cfg_stdio("ctx7")).is_err());
        // 非法名 / 非法 transport / stdio 缺 command / http 缺 url
        for bad in ["", "has space", "这个", &"x".repeat(33)] {
            let mut c = cfg_stdio("ok");
            c.server_name = bad.to_string();
            assert!(upsert_server(t.path(), None, &c).is_err(), "{bad}");
        }
        let mut c = cfg_stdio("ok2");
        c.transport = "sse".into();
        assert!(upsert_server(t.path(), None, &c).is_err());
        let mut c = cfg_stdio("ok3");
        c.command = None;
        assert!(upsert_server(t.path(), None, &c).is_err());
        let mut c = cfg_stdio("ok4");
        c.transport = "streamable-http".into();
        assert!(upsert_server(t.path(), None, &c).is_err()); // 缺 url
    }

    #[test]
    fn upsert_edit_preserves_advanced_keys_and_drops_other_transport() {
        let t = tempfile::tempdir().unwrap();
        write_patch_str(
            t.path(),
            "- insert:\n    - id: mcp-a\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: a\n        transport: stdio\n        command: npx\n        toolCallTimeoutMs: 90000\n",
        );
        let mut c = cfg_stdio("a");
        c.transport = "streamable-http".into();
        c.command = None;
        c.args = vec![];
        c.env = BTreeMap::new();
        c.url = Some("https://x/mcp".into());
        upsert_server(t.path(), Some("a"), &c).unwrap();
        let entries = crate::patchstore::read_patch(&crate::patchstore::patch_path(t.path())).unwrap();
        let cfg = &entry_at(&entries, mcp_positions(&entries)[0])["config"];
        assert_eq!(cfg["toolCallTimeoutMs"].as_u64(), Some(90000)); // 高级键保留
        assert!(cfg.get("command").is_none()); // 另一传输的键被清掉
        assert_eq!(cfg["url"].as_str(), Some("https://x/mcp"));
        // 编辑成与别的条目重名 → 拒绝
        upsert_server(t.path(), None, &cfg_stdio("b")).unwrap();
        let dup = cfg_stdio("b");
        assert!(upsert_server(t.path(), Some("a"), &dup).is_err());
    }

    #[test]
    fn delete_keeps_shared_insert_op_when_siblings_remain() {
        // 真实 ~/.dsh 文件的形态：一个 insert 列表里插多个 MCP 条目
        let t = tempfile::tempdir().unwrap();
        write_patch_str(
            t.path(),
            "- insert:\n    - id: mcp-playwright\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: playwright\n        transport: stdio\n        command: npx\n    - id: mcp-context7\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: context7\n        transport: stdio\n        command: npx\n",
        );
        delete_server(t.path(), "playwright").unwrap();
        let rows = list_servers(t.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].server_name, "context7");
    }

    // ---- 启动种子（~/.dsh → 壳 dsh-home）----

    fn seed_src(dir: &Path, rel: &str, s: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn seed_imports_both_layers_and_writes_marker() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        seed_src(&src, "profiles/web/cordis.patch.yml", TWO_SERVERS); // docs 带 disabled → 跳过
        seed_src(
            &src,
            "cordis.patch.yml",
            "- insert:\n    - id: mcp-home1\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: home1\n        transport: streamable-http\n        url: https://h/mcp\n",
        );
        seed_auto_import(&src, &home).unwrap();
        let names: Vec<_> = list_servers(&home).unwrap().into_iter().map(|r| r.server_name).collect();
        assert_eq!(names, vec!["playwright", "home1"]);
        let marker = fs::read_to_string(home.join(".mcp-seeded")).unwrap();
        // 源里 disabled 的条目不导入也不记 marker——日后在 ~/.dsh 启用时应能同步进来
        assert!(marker.contains("playwright") && marker.contains("home1") && !marker.contains("docs"));
    }

    #[test]
    fn seed_does_not_resurrect_deleted_or_overwrite_existing() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        seed_src(&src, "profiles/web/cordis.patch.yml", TWO_SERVERS);
        seed_auto_import(&src, &home).unwrap();
        delete_server(&home, "playwright").unwrap();
        // 用户自建同名 docs（源里它是 disabled 版本，不得覆盖）
        upsert_server(&home, None, &cfg_stdio("docs")).unwrap();
        seed_auto_import(&src, &home).unwrap();
        let rows = list_servers(&home).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].server_name, "docs");
        assert!(rows[0].enabled);
        assert_eq!(rows[0].summary, "npx -y pkg"); // 用户自建的配置未被覆盖
    }

    #[test]
    fn seed_skips_unparseable_source_file() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let home = t.path().join("home");
        seed_src(&src, "cordis.patch.yml", "not: a: sequence: [");
        seed_src(&src, "profiles/web/cordis.patch.yml", TWO_SERVERS);
        seed_auto_import(&src, &home).unwrap();
        assert_eq!(list_servers(&home).unwrap().len(), 1);
    }

    #[test]
    fn seed_missing_source_is_noop() {
        let t = tempfile::tempdir().unwrap();
        seed_auto_import(&t.path().join("nope"), &t.path().join("home")).unwrap();
        assert!(list_servers(&t.path().join("home")).unwrap().is_empty());
    }

    // ---- 三源导入 ----

    #[test]
    fn parse_claude_maps_types() {
        let text = r#"{"mcpServers":{
            "pw":{"type":"stdio","command":"npx","args":["-y","@playwright/mcp"],"env":{"K":"V"}},
            "docs":{"type":"http","url":"https://x/mcp","headers":{"Authorization":"Bearer t"}},
            "infer":{"command":"npx","args":["-y","pkg"]},
            "old":{"type":"sse","url":"https://x/sse"}
        }}"#;
        let v = parse_source("claude", text);
        assert_eq!(v.len(), 4);
        let by = |n: &str| v.iter().find(|(name, _)| name == n).map(|(_, c)| c).unwrap();
        let pw = by("pw").as_ref().unwrap();
        assert_eq!(pw.transport, "stdio");
        assert_eq!(pw.env["K"], "V");
        let docs = by("docs").as_ref().unwrap();
        assert_eq!(docs.transport, "streamable-http");
        assert_eq!(docs.headers["Authorization"], "Bearer t");
        assert_eq!(by("infer").as_ref().unwrap().transport, "stdio"); // 无 type 按有无 url 推断
        assert!(by("old").is_err()); // sse
    }

    #[test]
    fn parse_codex_toml() {
        let text = "[mcp_servers.pw]\ncommand = \"npx\"\nargs = [\"-y\", \"@playwright/mcp\"]\nenabled = true\n\n[mcp_servers.pw.env]\nK = \"V\"\n\n[mcp_servers.off]\ncommand = \"x\"\nenabled = false\n";
        let v = parse_source("codex", text);
        assert_eq!(v.len(), 1); // enabled=false 不列出
        let c = v[0].1.as_ref().unwrap();
        assert_eq!(c.command.as_deref(), Some("npx"));
        assert_eq!(c.args, vec!["-y", "@playwright/mcp"]);
        assert_eq!(c.env["K"], "V");
    }

    #[test]
    fn parse_opencode_local_remote() {
        let text = r#"{"mcp":{
            "pw":{"type":"local","command":["npx","-y","@playwright/mcp"],"environment":{"K":"V"}},
            "docs":{"type":"remote","url":"https://x/mcp","headers":{"A":"b"}},
            "off":{"type":"local","command":["x"],"enabled":false}
        }}"#;
        let v = parse_source("opencode", text);
        assert_eq!(v.len(), 2); // enabled:false 不列出
        let by = |n: &str| v.iter().find(|(name, _)| name == n).map(|(_, c)| c).unwrap();
        let pw = by("pw").as_ref().unwrap();
        assert_eq!(pw.command.as_deref(), Some("npx"));
        assert_eq!(pw.args, vec!["-y", "@playwright/mcp"]);
        let docs = by("docs").as_ref().unwrap();
        assert_eq!(docs.transport, "streamable-http");
        assert_eq!(docs.headers["A"], "b");
    }

    #[test]
    fn import_conflict_skip_overwrite_and_unsupported() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        upsert_server(home, None, &cfg_stdio("pw")).unwrap();
        let parsed = parse_source(
            "claude",
            r#"{"mcpServers":{
                "pw":{"type":"stdio","command":"npx","args":["-y","new"]},
                "fresh":{"type":"http","url":"https://x/mcp"},
                "bad":{"type":"sse","url":"https://x/sse"}
            }}"#,
        );
        let items = vec![
            McpImportItem { name: "pw".into(), overwrite: false },
            McpImportItem { name: "fresh".into(), overwrite: false },
            McpImportItem { name: "bad".into(), overwrite: false },
            McpImportItem { name: "ghost".into(), overwrite: false },
        ];
        let r = apply_imported(home, &parsed, &items);
        assert_eq!(r[0].status, "skipped");
        assert_eq!(r[1].status, "imported");
        assert_eq!(r[2].status, "error"); // sse 不支持
        assert_eq!(r[3].status, "error"); // 源里没有
        // overwrite：pw 的 args 被新配置替换（不保留旧配置）
        let r = apply_imported(home, &parsed, &[McpImportItem { name: "pw".into(), overwrite: true }]);
        assert_eq!(r[0].status, "imported");
        let entries = crate::patchstore::read_patch(&crate::patchstore::patch_path(home)).unwrap();
        let c = &entry_at(&entries, find_server(&entries, "pw").unwrap())["config"];
        assert_eq!(c["args"][0].as_str(), Some("-y"));
        assert_eq!(c["args"][1].as_str(), Some("new"));
    }
}
