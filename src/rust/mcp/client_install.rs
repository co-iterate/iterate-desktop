//! 把 iterate 的 mcp-server 接入用户本机的 AI 客户端（Claude Code / Codex / Cursor）。
//!
//! 设计约束：
//! - 只在用户确认后写入；用户主动移除的客户端记为 opted_out，后续不会自动加回。
//! - 各客户端配置统一指向稳定入口 `<bridge_state_dir>/bin/mcp-server`（指向 App 包内二进制的符号链接），
//!   App 升级或被挪动后只需修复这一个链接，不用再改各家配置。
//! - 每次写入前备份原文件，合并时只动 `iterate-zhi` 条目，原子写入并回读校验。
//! - Codex 的 TOML 用 toml_edit 按键路径修改（新 Mac 默认没有 python3，也不能用正则切表）。
//! - 写入前确认文件自读取后未被其他程序改动，避免覆盖客户端并发写入的内容。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use toml_edit::{value as toml_value, Array, DocumentMut, InlineTable, Item, Table, Value as TomlValue};

use crate::config::paths::iterate_bridge_state_dir;

pub const SERVER_NAME: &str = "iterate-zhi";
const CODEX_TIMEOUT_SECONDS: u64 = 315_360_000;
/// Claude Code 对 stdio MCP 默认 30 分钟空闲即断开，call_zhi 等人回复时会被掐断。
/// 单服务器 `timeout`（毫秒）会一并抬高空闲超时，只影响 iterate-zhi（P-2026-9338）。
const CLAUDE_CODE_TIMEOUT_MS: i64 = 2_147_483_647;
const MAX_BACKUPS_PER_CLIENT: usize = 10;
const CLAUDE_CLI_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientKind {
    ClaudeCode,
    Codex,
    Cursor,
    Antigravity,
}

impl ClientKind {
    pub const ALL: [ClientKind; 4] = [
        ClientKind::ClaudeCode,
        ClientKind::Codex,
        ClientKind::Cursor,
        ClientKind::Antigravity,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ClientKind::ClaudeCode => "claude-code",
            ClientKind::Codex => "codex",
            ClientKind::Cursor => "cursor",
            ClientKind::Antigravity => "antigravity",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ClientKind::ClaudeCode => "Claude Code",
            ClientKind::Codex => "Codex",
            ClientKind::Cursor => "Cursor",
            ClientKind::Antigravity => "Antigravity",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    fn restart_hint(self) -> &'static str {
        match self {
            ClientKind::ClaudeCode => "重新打开 Claude Code 会话后生效",
            ClientKind::Codex => "重启 Codex 后生效",
            ClientKind::Cursor => "重启 Cursor 后生效",
            ClientKind::Antigravity => "重新打开 agy 或 Antigravity 后生效",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientStatus {
    pub id: String,
    pub name: String,
    pub detected: bool,
    pub configured: bool,
    /// 已配置且指向当前 App 的 mcp-server（直接路径或稳定入口均算）。
    pub points_to_current: bool,
    /// 配置指向 iterate 的稳定入口，但入口已失效（链接缺失或 App 被挪走）。
    pub needs_repair: bool,
    pub opted_out: bool,
    pub config_path: String,
    pub configured_command: Option<String>,
    /// 调用规则文件（Codex：AGENTS.md；Claude Code：CLAUDE.md）；其他客户端没有。
    pub rules_path: Option<String>,
    /// installed / manual / missing；没有规则文件的客户端为 None。
    pub rules_state: Option<String>,
    /// Claude Code 的权限文件 ~/.claude/settings.json；其他客户端为 None。
    pub permission_path: Option<String>,
    /// installed / manual / missing / invalid：call_zhi 是否已在 permissions.allow 免确认。
    pub permission_state: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppLocationStatus {
    pub ok: bool,
    pub bundled_server: Option<String>,
    pub stable_server: String,
    /// 稳定入口已存在且指向当前 App；手动配置其他客户端时优先用它，否则用 App 包内路径。
    pub stable_server_ready: bool,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientOverview {
    pub app_location: AppLocationStatus,
    pub clients: Vec<ClientStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientActionResult {
    pub id: String,
    pub name: String,
    pub ok: bool,
    pub message: String,
    pub backup_path: Option<String>,
}

// ---------------------------------------------------------------------------
// 路径与 App 位置
// ---------------------------------------------------------------------------

fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "无法获取用户主目录".to_string())
}

fn codex_home() -> Result<PathBuf, String> {
    if let Ok(value) = std::env::var("CODEX_HOME") {
        let value = value.trim();
        if !value.is_empty() {
            return Ok(PathBuf::from(value));
        }
    }
    Ok(home_dir()?.join(".codex"))
}

fn client_config_path(kind: ClientKind) -> Result<PathBuf, String> {
    Ok(match kind {
        ClientKind::ClaudeCode => home_dir()?.join(".claude.json"),
        ClientKind::Codex => codex_home()?.join("config.toml"),
        ClientKind::Cursor => home_dir()?.join(".cursor").join("mcp.json"),
        // agy CLI 与新版 Antigravity 共用的用户级配置（`agy mcp add` 写的就是这里）。
        ClientKind::Antigravity => home_dir()?.join(".gemini").join("config").join("mcp_config.json"),
    })
}

pub fn stable_server_path() -> PathBuf {
    iterate_bridge_state_dir().join("bin").join("mcp-server")
}

fn bundled_server_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    Some(exe.with_file_name(bundled_server_file_name()))
}

/// Windows 上可执行文件带 `.exe`；少了它，一键接入会一直报“App 包内缺少 mcp-server”。
fn bundled_server_file_name() -> String {
    format!("mcp-server{}", std::env::consts::EXE_SUFFIX)
}

/// App 从 DMG 直接运行或被系统随机化路径（App Translocation）时，路径随时会失效，不能写进客户端配置。
fn unstable_location_reason(path: &Path) -> Option<&'static str> {
    let text = path.to_string_lossy();
    if text.contains("/AppTranslocation/") {
        return Some("iterate 正在隔离路径中运行，请先把 iterate 拖进“应用程序”文件夹再打开");
    }
    if text.starts_with("/Volumes/") {
        return Some("iterate 正在从安装镜像中运行，请先把 iterate 拖进“应用程序”文件夹再打开");
    }
    None
}

pub fn app_location_status() -> AppLocationStatus {
    let stable_server = stable_server_path().to_string_lossy().to_string();
    let stable_server_ready = is_current_server_command(&stable_server);
    let Some(bundled) = bundled_server_path() else {
        return AppLocationStatus {
            ok: false,
            bundled_server: None,
            stable_server,
            stable_server_ready,
            problem: Some("无法定位 iterate 自身的安装位置".to_string()),
        };
    };
    let problem = if let Some(reason) = unstable_location_reason(&bundled) {
        Some(reason.to_string())
    } else if !bundled.is_file() {
        Some(format!("App 包内缺少 mcp-server：{}", bundled.display()))
    } else {
        None
    };
    AppLocationStatus {
        ok: problem.is_none(),
        bundled_server: Some(bundled.to_string_lossy().to_string()),
        stable_server,
        stable_server_ready,
        problem,
    }
}

/// 让稳定入口指向当前 App 包内的 mcp-server。返回稳定入口路径。
pub fn ensure_stable_server_link() -> Result<PathBuf, String> {
    let location = app_location_status();
    if let Some(problem) = location.problem {
        return Err(problem);
    }
    let target = PathBuf::from(location.bundled_server.expect("checked above"));
    link_stable_server(target)
}

/// 稳定入口用符号链接实现，只在 Unix 上提供；其它平台直接使用 App 包内的 mcp-server。
#[cfg(not(unix))]
fn link_stable_server(target: PathBuf) -> Result<PathBuf, String> {
    Ok(target)
}

#[cfg(unix)]
fn link_stable_server(target: PathBuf) -> Result<PathBuf, String> {
    let link = stable_server_path();
    let parent = link.parent().expect("stable path has a parent");
    fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;

    if let Ok(existing) = fs::read_link(&link) {
        if existing == target {
            return Ok(link);
        }
    }

    // 先建临时链接再 rename，避免出现“旧链接已删、新链接未建”的窗口。
    let temp = parent.join(format!(".mcp-server.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&temp);
    std::os::unix::fs::symlink(&target, &temp)
        .map_err(|e| format!("创建稳定入口失败: {e}"))?;
    fs::rename(&temp, &link).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("更新稳定入口失败: {e}")
    })?;
    Ok(link)
}

/// App 启动时调用：只有用户此前接入过（稳定入口或偏好文件存在）才刷新，不会擅自创建。
/// 链接被误删时也能据偏好文件重建。
pub fn refresh_stable_server_link_if_present() {
    let link = stable_server_path();
    if fs::symlink_metadata(&link).is_err() && !preferences_path().exists() {
        return;
    }
    if let Err(error) = ensure_stable_server_link() {
        log::warn!("刷新 MCP 稳定入口失败: {error}");
    }
}

/// 实际解析到当前 App 包内、且可执行的 mcp-server 才算“已接入”。
/// 稳定入口失效（链接缺失、App 已挪走）时返回 false，界面会提示修复。
fn is_current_server_command(command: &str) -> bool {
    let Some(bundled) = bundled_server_path() else {
        return false;
    };
    let resolved = match (Path::new(command).canonicalize(), bundled.canonicalize()) {
        (Ok(a), Ok(b)) if a == b => a,
        _ => return false,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(resolved)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        fs::metadata(resolved).map(|meta| meta.is_file()).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// 用户选择（opt-out）状态
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct ClientPreferences {
    #[serde(default)]
    opted_out: BTreeMap<String, bool>,
    /// ~/.claude/settings.json 里的 call_zhi 白名单是 iterate 加的；用户自己加的不记，移除时不删。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    claude_permission_owned: bool,
}

fn preferences_path() -> PathBuf {
    iterate_bridge_state_dir().join("mcp-clients.json")
}

fn load_preferences() -> ClientPreferences {
    fs::read_to_string(preferences_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_preferences(preferences: &ClientPreferences) -> Result<(), String> {
    let path = preferences_path();
    let text = serde_json::to_string_pretty(preferences).map_err(|e| e.to_string())?;
    atomic_write(&path, text.as_bytes())
}

fn set_opted_out(kind: ClientKind, value: bool) -> Result<(), String> {
    let mut preferences = load_preferences();
    if value {
        preferences.opted_out.insert(kind.id().to_string(), true);
    } else {
        preferences.opted_out.remove(kind.id());
    }
    save_preferences(&preferences)
}

// ---------------------------------------------------------------------------
// 文件读写：解析符号链接、备份、原子写入
// ---------------------------------------------------------------------------

/// dotfiles 用户常把配置做成符号链接；直接 rename 会把链接替换成普通文件，所以写到真实目标。
/// 目标不存在的悬空链接无法安全处理，直接拒绝。
fn resolve_write_target(path: &Path) -> Result<PathBuf, String> {
    let is_symlink = fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if !is_symlink {
        return Ok(path.to_path_buf());
    }
    path.canonicalize().map_err(|_| {
        format!(
            "{} 是指向不存在文件的符号链接，为避免破坏你的 dotfiles 已停止写入",
            path.display()
        )
    })
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
    write_config(path, content, None)
}

/// 原子写入。`expected` 为读取时的原文时，替换前再读一次比对；
/// 期间若被客户端改过，就放弃写入，避免覆盖它刚写的内容。
fn write_config(path: &Path, content: &[u8], expected: Option<&str>) -> Result<(), String> {
    let target = resolve_write_target(path)?;
    let parent = target
        .parent()
        .ok_or_else(|| format!("无效路径: {}", target.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    let original_permissions = fs::metadata(&target).ok().map(|meta| meta.permissions());

    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("创建临时文件失败: {e}"))?;
    temp.write_all(content)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|e| format!("写入临时文件失败: {e}"))?;
    if let Some(permissions) = original_permissions {
        let _ = fs::set_permissions(temp.path(), permissions);
    }
    if let Some(expected) = expected {
        if read_optional(&target)? != expected {
            return Err(format!(
                "{} 刚被其他程序修改，为避免覆盖已停止写入，请稍后重试",
                path.display()
            ));
        }
    }
    temp.persist(&target)
        .map_err(|e| format!("写入 {} 失败: {}", target.display(), e.error))?;
    Ok(())
}

fn backup_dir() -> PathBuf {
    iterate_bridge_state_dir().join("mcp-client-backups")
}

fn backup_config(kind: ClientKind, path: &Path) -> Result<Option<PathBuf>, String> {
    backup_config_into(&backup_dir(), kind, path)
}

fn backup_config_into(dir: &Path, kind: ClientKind, path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }
    fs::create_dir_all(dir).map_err(|e| format!("创建备份目录失败: {e}"))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().trim_start_matches('.').to_string())
        .unwrap_or_else(|| "config".to_string());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let backup = dir.join(format!("{}-{}-{}", kind.id(), stamp, file_name));
    fs::copy(path, &backup).map_err(|e| format!("备份 {} 失败: {e}", path.display()))?;
    prune_backups(dir, kind);
    Ok(Some(backup))
}

fn prune_backups(dir: &Path, kind: ClientKind) {
    let prefix = format!("{}-", kind.id());
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut backups: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    // 文件名带时间戳，字典序即时间序。
    backups.sort();
    while backups.len() > MAX_BACKUPS_PER_CLIENT {
        let _ = fs::remove_file(backups.remove(0));
    }
}

fn read_optional(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("读取 {} 失败: {error}", path.display())),
    }
}

// ---------------------------------------------------------------------------
// JSON 客户端（Claude Code / Cursor）
// ---------------------------------------------------------------------------

fn parse_json_object(text: &str) -> Result<Map<String, Value>, String> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err("配置文件顶层不是 JSON 对象，为避免破坏原配置已停止写入".to_string()),
        Err(error) => Err(format!("配置文件不是合法 JSON（{error}），为避免破坏原配置已停止写入")),
    }
}

/// 合并 iterate-zhi 条目：保留用户已有的 args/env 等字段，只更新 command。
pub fn merge_json_server_entry(
    existing: Option<&Value>,
    command: &str,
    include_stdio_type: bool,
) -> Value {
    let mut entry = match existing {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    entry.insert("command".to_string(), Value::String(command.to_string()));
    entry.entry("args").or_insert_with(|| json!([]));
    if include_stdio_type {
        entry.entry("type").or_insert_with(|| json!("stdio"));
    }
    Value::Object(entry)
}

/// Claude Code 条目：在通用合并之外补上 `timeout`；用户自己设过的值不覆盖。
pub fn claude_code_server_entry(existing: Option<&Value>, command: &str) -> Value {
    let mut entry = merge_json_server_entry(existing, command, true);
    if let Value::Object(map) = &mut entry {
        map.entry("timeout").or_insert_with(|| json!(CLAUDE_CODE_TIMEOUT_MS));
    }
    entry
}

fn json_servers_mut(root: &mut Map<String, Value>) -> Result<&mut Map<String, Value>, String> {
    match root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()))
    {
        Value::Object(servers) => Ok(servers),
        _ => Err("配置中的 mcpServers 不是对象，为避免破坏原配置已停止写入".to_string()),
    }
}

fn render_json(root: Map<String, Value>) -> Result<String, String> {
    let mut output = serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| e.to_string())?;
    output.push('\n');
    Ok(output)
}

pub fn merge_json_config(text: &str, command: &str, include_stdio_type: bool) -> Result<String, String> {
    let mut root = parse_json_object(text)?;
    let servers = json_servers_mut(&mut root)?;
    refuse_remote_entry(servers.get(SERVER_NAME).and_then(|e| e.get("url")).is_some())?;
    let merged = merge_json_server_entry(servers.get(SERVER_NAME), command, include_stdio_type);
    servers.insert(SERVER_NAME.to_string(), merged);
    render_json(root)
}

/// 已有远程（URL）方式的条目与本地 command 互斥；自动迁移会产生混合配置，交给用户手动处理。
fn refuse_remote_entry(has_url: bool) -> Result<(), String> {
    if has_url {
        Err("已有远程（URL）方式的 iterate-zhi 配置，为避免产生混合配置不自动迁移，请先手动移除该条目".to_string())
    } else {
        Ok(())
    }
}

/// 把事先保存好的完整条目写进最新内容，其他条目和字段原样保留。
pub fn upsert_json_entry(text: &str, entry: Value) -> Result<String, String> {
    let mut root = parse_json_object(text)?;
    json_servers_mut(&mut root)?.insert(SERVER_NAME.to_string(), entry);
    render_json(root)
}

pub fn remove_json_server(text: &str) -> Result<Option<String>, String> {
    let mut root = parse_json_object(text)?;
    let removed = match root.get_mut("mcpServers") {
        Some(Value::Object(servers)) => servers.remove(SERVER_NAME).is_some(),
        _ => false,
    };
    if !removed {
        return Ok(None);
    }
    let mut output = serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| e.to_string())?;
    output.push('\n');
    Ok(Some(output))
}

/// 条目存在即返回它的入口：本地 command，或远程 url。
fn json_configured_command(text: &str) -> Option<String> {
    let root: Value = serde_json::from_str(text).ok()?;
    let entry = root.get("mcpServers")?.get(SERVER_NAME)?;
    ["command", "url"]
        .into_iter()
        .find_map(|key| entry.get(key)?.as_str().map(ToOwned::to_owned))
        .or_else(|| Some(String::new()))
}

// ---------------------------------------------------------------------------
// Codex（TOML）：用 toml_edit 按键路径修改，保留用户的注释与格式。
// 不用正则切表：缩进表头、带注释的表头、多行字符串里的“伪表头”都会让正则误删或产出重复定义。
// ---------------------------------------------------------------------------

/// 历史上用过的条目名，优先复用已有的那个，避免同一台机器出现两份 iterate 配置。
const CODEX_SERVER_KEYS: [&str; 2] = [SERVER_NAME, "cunzhi"];

fn parse_codex_document(text: &str) -> Result<DocumentMut, String> {
    text.parse::<DocumentMut>()
        .map_err(|e| format!("Codex 配置不是合法 TOML（{e}），为避免破坏原配置已停止写入"))
}

/// 取（必要时创建）子表。标准表 `[a.b]` 和内联表 `a = { b = {...} }` 都支持：
/// 新建的子表沿用父表的写法，避免把标准表塞进内联表里产出非法 TOML。
fn codex_child_table<'a>(parent: &'a mut Item, key: &str, label: &str) -> Result<&'a mut Item, String> {
    let inline_parent = parent.is_inline_table();
    let table = parent
        .as_table_like_mut()
        .ok_or_else(|| format!("Codex 配置中 {label} 的上级不是表，为避免破坏原配置已停止写入"))?;
    if !table.contains_key(key) {
        let child = if inline_parent {
            Item::Value(TomlValue::InlineTable(InlineTable::new()))
        } else {
            let mut child = Table::new();
            child.set_implicit(true);
            Item::Table(child)
        };
        table.insert(key, child);
    }
    let child = table.get_mut(key).expect("刚确认或插入的键必然存在");
    if !child.is_table_like() {
        return Err(format!("Codex 配置中的 {label} 不是表，为避免破坏原配置已停止写入"));
    }
    Ok(child)
}

/// 标准表需要显式输出表头；内联表无此概念。
fn codex_mark_explicit(table: &mut Item) {
    if let Some(table) = table.as_table_mut() {
        table.set_implicit(false);
    }
}

fn codex_set(table: &mut Item, key: &str, value: Item) {
    if let Some(table) = table.as_table_like_mut() {
        table.insert(key, value);
    }
}

pub fn merge_codex_config(text: &str, command: &str) -> Result<String, String> {
    let mut doc = parse_codex_document(text)?;
    let servers = codex_child_table(doc.as_item_mut(), "mcp_servers", "mcp_servers")?;
    let key = CODEX_SERVER_KEYS
        .into_iter()
        .find(|key| servers.as_table_like().is_some_and(|t| t.contains_key(key)))
        .unwrap_or(SERVER_NAME);

    let server = codex_child_table(servers, key, key)?;
    refuse_remote_entry(server.as_table_like().is_some_and(|t| t.contains_key("url")))?;
    codex_mark_explicit(server);
    codex_set(server, "command", toml_value(command));
    if !server.as_table_like().is_some_and(|t| t.contains_key("args")) {
        codex_set(server, "args", toml_value(Array::new()));
    }
    codex_set(server, "tool_timeout_sec", toml_value(CODEX_TIMEOUT_SECONDS as i64));

    let tools = codex_child_table(server, "tools", "tools")?;
    let call_zhi = codex_child_table(tools, "call_zhi", "tools.call_zhi")?;
    codex_mark_explicit(call_zhi);
    // 用户显式设置过的审批模式不放宽。
    if !call_zhi.as_table_like().is_some_and(|t| t.contains_key("approval_mode")) {
        codex_set(call_zhi, "approval_mode", toml_value("approve"));
    }

    let output = doc.to_string();
    // 写盘前再整份解析一次，确保产物本身合法。
    parse_codex_document(&output)?;
    Ok(output)
}

pub fn remove_codex_server(text: &str) -> Result<Option<String>, String> {
    let mut doc = parse_codex_document(text)?;
    let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_like_mut) else {
        return Ok(None);
    };
    let mut removed = false;
    for key in CODEX_SERVER_KEYS {
        removed |= servers.remove(key).is_some();
    }
    if !removed {
        return Ok(None);
    }
    let output = doc.to_string();
    parse_codex_document(&output)?;
    Ok(Some(output))
}

/// 手动配置的 Codex 常漏掉 `tool_timeout_sec`，Codex 默认只等 60 秒，弹窗没回复就被它放弃，
/// 之后在弹窗里点发送也送不回去。低于这个值的一律补成长等待（覆盖 60/120/600 这类默认或抄来的值）。
const CODEX_MIN_HEALTHY_TIMEOUT_SECONDS: f64 = 86_400.0;

/// 只修已存在的本地 iterate-zhi 条目的等待时间；没有条目、远程条目、或已足够长时返回 None。
pub fn heal_codex_timeout(text: &str) -> Result<Option<String>, String> {
    let mut doc = parse_codex_document(text)?;
    let Some(servers) = doc.get_mut("mcp_servers") else {
        return Ok(None);
    };
    let Some(key) = CODEX_SERVER_KEYS
        .into_iter()
        .find(|key| servers.as_table_like().is_some_and(|t| t.contains_key(key)))
    else {
        return Ok(None);
    };
    let Some(server) = servers.as_table_like_mut().and_then(|t| t.get_mut(key)) else {
        return Ok(None);
    };
    let Some(table) = server.as_table_like() else {
        return Ok(None);
    };
    if table.contains_key("url") {
        return Ok(None);
    }
    let current = table.get("tool_timeout_sec").and_then(|item| {
        item.as_integer()
            .map(|value| value as f64)
            .or_else(|| item.as_float())
    });
    if current.is_some_and(|value| value >= CODEX_MIN_HEALTHY_TIMEOUT_SECONDS) {
        return Ok(None);
    }
    codex_set(server, "tool_timeout_sec", toml_value(CODEX_TIMEOUT_SECONDS as i64));
    let output = doc.to_string();
    parse_codex_document(&output)?;
    Ok(Some(output))
}

fn codex_configured_command(text: &str) -> Option<String> {
    let doc = text.parse::<DocumentMut>().ok()?;
    let servers = doc.get("mcp_servers")?.as_table_like()?;
    // 条目存在即返回它的入口：本地 command，或远程 url。
    CODEX_SERVER_KEYS.into_iter().find_map(|key| {
        let server = servers.get(key)?.as_table_like()?;
        ["command", "url"]
            .into_iter()
            .find_map(|field| server.get(field)?.as_str().map(ToOwned::to_owned))
            .or_else(|| Some(String::new()))
    })
}

// ---------------------------------------------------------------------------
// Claude Code CLI
// ---------------------------------------------------------------------------

/// GUI App 的 PATH 只有系统目录，按常见安装位置找 `claude`。
fn find_claude_cli() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [
        home.join(".local/bin/claude"),
        home.join(".claude/local/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        home.join(".npm-global/bin/claude"),
        home.join(".bun/bin/claude"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Antigravity CLI（agy）同样按常见安装位置找；额度读取也复用这里。
pub fn find_agy_cli() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [
        home.join(".local/bin/agy"),
        PathBuf::from("/opt/homebrew/bin/agy"),
        PathBuf::from("/usr/local/bin/agy"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn cli_path_env() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        parts.push(home.join(".local/bin").to_string_lossy().to_string());
    }
    parts.push("/opt/homebrew/bin".to_string());
    parts.push("/usr/local/bin".to_string());
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(":")
}

fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<(), String> {
    let mut child = command
        .env("PATH", cli_path_env())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 claude 命令失败: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
                }
                return Err(format!("claude 命令失败（{status}）：{}", stderr.trim()));
            }
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("claude 命令超时".to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => return Err(format!("等待 claude 命令失败: {error}")),
        }
    }
}

fn claude_cli_upsert(cli: &Path, entry: &Value) -> Result<(), String> {
    // add-json 遇到同名条目会报错，先移除；失败（不存在）可忽略。
    let mut remove = Command::new(cli);
    remove.args(["mcp", "remove", SERVER_NAME, "-s", "user"]);
    let _ = run_with_timeout(remove, CLAUDE_CLI_TIMEOUT);

    let mut add = Command::new(cli);
    add.args(["mcp", "add-json", SERVER_NAME, &entry.to_string(), "-s", "user"]);
    run_with_timeout(add, CLAUDE_CLI_TIMEOUT)
}

// ---------------------------------------------------------------------------
// 调用规则（Codex 的 AGENTS.md、Claude Code 的 CLAUDE.md）
// ---------------------------------------------------------------------------

// Codex 只在模型搜到工具时才把 MCP instructions 拼进工具描述，模型不知道要循环调用 call_zhi；
// Claude Code 虽然会把 instructions 放进系统提示词，但 VS Code 插件等入口不一定带上。
// 所以两家都在全局提示词文件里写同一段带标记的规则，断开时整段删掉；标记段外的内容一字不动。
const CODEX_RULES_BEGIN: &str = "<!-- iterate-zhi:begin（由 iterate 维护；在 iterate 里断开 Codex 会自动删除本段） -->";
const CLAUDE_RULES_BEGIN: &str =
    "<!-- iterate-zhi:begin（由 iterate 维护；在 iterate 里断开 Claude Code 会自动删除本段） -->";
const RULES_END: &str = "<!-- iterate-zhi:end -->";
/// 用户照说明书手写进 CLAUDE.md 的规则也以这个标题开头；已有时不再追加第二份。
const RULES_HEADING: &str = "## iterate 交互协议";
/// Codex 与 Claude Code 共用的一份「iterate 交互协议」。
const IZHI_RULES_BODY: &str = "## iterate 交互协议

本规则只约束用户直接交互的前台主会话；子代理、后台任务、定时任务不调用 call_zhi。

1. 每次回复都以调用 iterate-zhi 的 `call_zhi` 作为最后一步：完成、等待用户决定或交还控制权时都一样，不要只用文字收尾。
2. 用户最新消息去除首尾空白后正好是 `zhi` 时，立即调用 `call_zhi`，不先执行其他任务。
3. `message` 写当前完整结果、状态或待决事项；`project_path` 填当前工作区绝对路径，未知时不要猜；需要选择时把简短、互斥的选项放进 `predefined_options`。
4. 拿到返回后：有附加图片先看图片；`继续对话: true` 时把用户输入当作新指令继续工作，做完再次调用 `call_zhi`，如此循环；`继续对话: false` 时结束本轮。
5. `call_zhi` 会一直等用户回复，这是正常的：保持原调用，不要另开第二个调用，也不要用轮询代替。
6. 删除、覆盖、提交、推送、发布等不可逆操作前，先通过 `call_zhi` 说明影响并取得同意。";

/// 一份由 iterate 维护的全局提示词文件。
#[derive(Debug, Clone, Copy)]
struct RulesFile {
    kind: ClientKind,
    begin: &'static str,
    /// 文件里已有用户手写的同名章节（没有 iterate 标记）时不追加，避免同一份规则出现两遍。
    respect_manual: bool,
}

const CODEX_RULES: RulesFile = RulesFile {
    kind: ClientKind::Codex,
    begin: CODEX_RULES_BEGIN,
    respect_manual: false,
};

const CLAUDE_RULES: RulesFile = RulesFile {
    kind: ClientKind::ClaudeCode,
    begin: CLAUDE_RULES_BEGIN,
    respect_manual: true,
};

fn rules_file(kind: ClientKind) -> Option<RulesFile> {
    match kind {
        ClientKind::Codex => Some(CODEX_RULES),
        ClientKind::ClaudeCode => Some(CLAUDE_RULES),
        ClientKind::Cursor | ClientKind::Antigravity => None,
    }
}

fn rules_path(rules: RulesFile) -> Result<PathBuf, String> {
    match rules.kind {
        ClientKind::Codex => Ok(codex_home()?.join("AGENTS.md")),
        _ => Ok(home_dir()?.join(".claude").join("CLAUDE.md")),
    }
}

fn rules_block(begin: &str) -> String {
    format!("{begin}\n{IZHI_RULES_BODY}\n{RULES_END}")
}

/// 标记段的字节范围（含两端标记）；只认第一对完整的标记。
fn rules_span(text: &str, begin: &str) -> Option<(usize, usize)> {
    let start = text.find(begin)?;
    let end = text[start..].find(RULES_END)? + start + RULES_END.len();
    Some((start, end))
}

/// 已有标记段就原位替换，否则追加到末尾；标记段外的用户内容一字不动。
fn upsert_rules(text: &str, begin: &str) -> String {
    let block = rules_block(begin);
    if let Some((start, end)) = rules_span(text, begin) {
        return format!("{}{}{}", &text[..start], block, &text[end..]);
    }
    let trimmed = text.trim_end_matches('\n');
    if trimmed.is_empty() {
        format!("{block}\n")
    } else {
        format!("{trimmed}\n\n{block}\n")
    }
}

/// 删掉标记段及它前面为分隔而补的空行；没有标记段时返回 None。
fn remove_rules(text: &str, begin: &str) -> Option<String> {
    let (start, end) = rules_span(text, begin)?;
    let before = text[..start].trim_end_matches('\n');
    let after = text[end..].trim_start_matches('\n');
    Some(match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => after.to_string(),
        (false, true) => format!("{before}\n"),
        (false, false) => format!("{before}\n\n{after}"),
    })
}

/// 没有 iterate 标记、但已有用户手写的「iterate 交互协议」章节。
fn has_manual_rules(text: &str, begin: &str) -> bool {
    rules_span(text, begin).is_none()
        && text.lines().any(|line| line.trim_start().starts_with(RULES_HEADING))
}

pub fn upsert_codex_rules(text: &str) -> String {
    upsert_rules(text, CODEX_RULES_BEGIN)
}

pub fn remove_codex_rules(text: &str) -> Option<String> {
    remove_rules(text, CODEX_RULES_BEGIN)
}

pub fn upsert_claude_rules(text: &str) -> String {
    upsert_rules(text, CLAUDE_RULES_BEGIN)
}

pub fn remove_claude_rules(text: &str) -> Option<String> {
    remove_rules(text, CLAUDE_RULES_BEGIN)
}

/// 规则文件当前状态，给界面显示：installed（有 iterate 标记段）/ manual（用户手写）/ missing。
fn rules_state(rules: RulesFile, text: &str) -> &'static str {
    if rules_span(text, rules.begin).is_some() {
        "installed"
    } else if rules.respect_manual && has_manual_rules(text, rules.begin) {
        "manual"
    } else {
        "missing"
    }
}

enum RulesWritten {
    /// 已写入或本来就是最新内容。
    Installed(PathBuf),
    /// 用户已手写同名章节，没有追加。
    KeptManual(PathBuf),
}

fn write_rules(rules: RulesFile) -> Result<RulesWritten, String> {
    let path = rules_path(rules)?;
    resolve_write_target(&path)?;
    let original = read_optional(&path)?;
    if rules.respect_manual && has_manual_rules(&original, rules.begin) {
        return Ok(RulesWritten::KeptManual(path));
    }
    let updated = upsert_rules(&original, rules.begin);
    if updated != original {
        backup_config(rules.kind, &path)?;
        write_config(&path, updated.as_bytes(), Some(&original))?;
    }
    Ok(RulesWritten::Installed(path))
}

fn delete_rules(rules: RulesFile) -> Result<(), String> {
    let path = rules_path(rules)?;
    resolve_write_target(&path)?;
    let original = read_optional(&path)?;
    let Some(updated) = remove_rules(&original, rules.begin) else {
        return Ok(());
    };
    backup_config(rules.kind, &path)?;
    write_config(&path, updated.as_bytes(), Some(&original))
}

// ---------------------------------------------------------------------------
// Claude Code 权限白名单（~/.claude/settings.json → permissions.allow）
// ---------------------------------------------------------------------------

// 新用户第一次 call_zhi 会被 Claude Code 的权限确认拦住（无头模式下直接报
// “you haven't granted it yet”），zhi 循环第一步就断。一键接入时顺手把 call_zhi 加进用户级白名单。
// 只放行 call_zhi 这一个工具（与 Codex 只给 tools.call_zhi 设 approval_mode 对齐），
// 不放行整个 iterate-zhi 服务器：其余工具（checkpoint / sync / task 等）仍按用户自己的权限设置走。
pub const CLAUDE_ZHI_PERMISSION: &str = "mcp__iterate-zhi__call_zhi";
/// 用户自己写的、同样覆盖 call_zhi 的规则：整服务器放行或通配。
const CLAUDE_ZHI_PERMISSION_ALIASES: [&str; 3] = [CLAUDE_ZHI_PERMISSION, "mcp__iterate-zhi", "mcp__iterate-zhi__*"];

/// 与 CLAUDE.md 同目录（同样不读 CLAUDE_CONFIG_DIR，和现有 ~/.claude.json、~/.claude/CLAUDE.md 的写法一致）。
fn claude_settings_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".claude").join("settings.json"))
}

fn permission_rules<'a>(root: &'a Map<String, Value>, list: &str) -> impl Iterator<Item = &'a str> {
    root.get("permissions")
        .and_then(|permissions| permissions.get(list))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

fn covers_call_zhi(rule: &str) -> bool {
    CLAUDE_ZHI_PERMISSION_ALIASES.contains(&rule.trim())
}

#[derive(Debug, PartialEq, Eq)]
pub enum PermissionEdit {
    /// 已把 call_zhi 加进 allow，附带写回的完整内容。
    Added(String),
    /// allow 里已有覆盖 call_zhi 的规则（我们上次加的，或用户自己加的），不重复写。
    AlreadyAllowed,
    /// 用户在 deny / ask 里明确限制过 call_zhi：尊重用户选择，不放宽。
    UserRestricted,
}

/// 计算加入白名单后的 settings.json。坏文件（非 JSON、顶层或 permissions / allow 类型不对）一律报错不写。
pub fn add_claude_permission(text: &str) -> Result<PermissionEdit, String> {
    let mut root = parse_json_object(text)?;
    if ["deny", "ask"]
        .into_iter()
        .any(|list| permission_rules(&root, list).any(covers_call_zhi))
    {
        return Ok(PermissionEdit::UserRestricted);
    }
    if permission_rules(&root, "allow").any(covers_call_zhi) {
        return Ok(PermissionEdit::AlreadyAllowed);
    }
    let permissions = match root
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()))
    {
        Value::Object(permissions) => permissions,
        _ => return Err("settings.json 中的 permissions 不是对象，为避免破坏原配置已停止写入".to_string()),
    };
    match permissions.entry("allow").or_insert_with(|| json!([])) {
        Value::Array(allow) => allow.push(Value::String(CLAUDE_ZHI_PERMISSION.to_string())),
        _ => return Err("settings.json 中的 permissions.allow 不是数组，为避免破坏原配置已停止写入".to_string()),
    }
    Ok(PermissionEdit::Added(render_json(root)?))
}

/// 只删 allow 里与 CLAUDE_ZHI_PERMISSION 完全相同的条目；其他条目（含整服务器放行）原样保留。
/// 没有可删的条目时返回 None。
pub fn remove_claude_permission(text: &str) -> Result<Option<String>, String> {
    let mut root = parse_json_object(text)?;
    let Some(Value::Array(allow)) = root
        .get_mut("permissions")
        .and_then(|permissions| permissions.get_mut("allow"))
    else {
        return Ok(None);
    };
    let before = allow.len();
    allow.retain(|rule| rule.as_str() != Some(CLAUDE_ZHI_PERMISSION));
    if allow.len() == before {
        return Ok(None);
    }
    Ok(Some(render_json(root)?))
}

/// 给界面显示：installed（iterate 加的）/ manual（用户自己设过）/ missing / invalid（文件坏了，不会写）。
pub fn claude_permission_state(text: &str, owned: bool) -> &'static str {
    let Ok(root) = parse_json_object(text) else {
        return "invalid";
    };
    let user_restricted = ["deny", "ask"]
        .into_iter()
        .any(|list| permission_rules(&root, list).any(covers_call_zhi));
    let has_ours = permission_rules(&root, "allow").any(|rule| rule == CLAUDE_ZHI_PERMISSION);
    if user_restricted {
        "manual"
    } else if owned && has_ours {
        "installed"
    } else if permission_rules(&root, "allow").any(covers_call_zhi) {
        "manual"
    } else {
        "missing"
    }
}

fn set_claude_permission_owned(value: bool) -> Result<(), String> {
    let mut preferences = load_preferences();
    if preferences.claude_permission_owned == value {
        return Ok(());
    }
    preferences.claude_permission_owned = value;
    save_preferences(&preferences)
}

enum PermissionWritten {
    /// iterate 加的（本次新加，或上次接入时加的仍在）。
    Installed(PathBuf),
    /// 用户自己放行或限制过，没有改动。
    KeptUser(PathBuf),
}

/// 在指定文件上加白名单。返回写入结果，以及 iterate 是否新加了这一条（需要记为 owned）。
fn write_claude_permission_at(
    path: &Path,
    backups: &Path,
    owned: bool,
) -> Result<(PermissionWritten, bool), String> {
    resolve_write_target(path)?;
    let original = read_optional(path)?;
    match add_claude_permission(&original)? {
        PermissionEdit::Added(updated) => {
            // 文件不存在时 backup 返回 None（没有可备份的），直接新建。
            backup_config_into(backups, ClientKind::ClaudeCode, path)?;
            write_config(path, updated.as_bytes(), Some(&original))?;
            Ok((PermissionWritten::Installed(path.to_path_buf()), true))
        }
        PermissionEdit::AlreadyAllowed if claude_permission_state(&original, owned) == "installed" => {
            Ok((PermissionWritten::Installed(path.to_path_buf()), false))
        }
        PermissionEdit::AlreadyAllowed | PermissionEdit::UserRestricted => {
            Ok((PermissionWritten::KeptUser(path.to_path_buf()), false))
        }
    }
}

/// 只删 iterate 自己加的那条（owned 为 true 时）；接入前用户就有的同名规则不动，
/// 与 CLAUDE.md 里手写的 iterate 交互协议同样处理。返回是否真的改了文件。
fn delete_claude_permission_at(path: &Path, backups: &Path, owned: bool) -> Result<bool, String> {
    if !owned {
        return Ok(false);
    }
    resolve_write_target(path)?;
    let original = read_optional(path)?;
    let Some(updated) = remove_claude_permission(&original)? else {
        return Ok(false);
    };
    backup_config_into(backups, ClientKind::ClaudeCode, path)?;
    write_config(path, updated.as_bytes(), Some(&original))?;
    Ok(true)
}

fn write_claude_permission() -> Result<PermissionWritten, String> {
    let path = claude_settings_path()?;
    let owned = load_preferences().claude_permission_owned;
    let (written, added) = write_claude_permission_at(&path, &backup_dir(), owned)?;
    if added {
        // 记下“这一条是 iterate 加的”，移除时只删自己加的。
        set_claude_permission_owned(true)?;
    }
    Ok(written)
}

fn delete_claude_permission() -> Result<(), String> {
    let owned = load_preferences().claude_permission_owned;
    delete_claude_permission_at(&claude_settings_path()?, &backup_dir(), owned)?;
    if owned {
        set_claude_permission_owned(false)?;
    }
    Ok(())
}

fn permission_message() -> String {
    match write_claude_permission() {
        Ok(PermissionWritten::Installed(path)) => {
            format!("；已在 {} 允许 call_zhi 免确认调用（{CLAUDE_ZHI_PERMISSION}）", path.display())
        }
        Ok(PermissionWritten::KeptUser(path)) => {
            format!("；{} 里已有你自己设置的 call_zhi 权限，没有改动", path.display())
        }
        Err(error) => {
            log::warn!("写入 Claude Code 权限白名单失败: {error}");
            format!("；call_zhi 免确认未写入（{error}），第一次调用时 Claude Code 会先请你确认")
        }
    }
}

// ---------------------------------------------------------------------------
// 检测 / 状态
// ---------------------------------------------------------------------------

fn client_detected(kind: ClientKind) -> bool {
    let Ok(home) = home_dir() else {
        return false;
    };
    match kind {
        ClientKind::ClaudeCode => {
            home.join(".claude.json").is_file() || home.join(".claude").is_dir() || find_claude_cli().is_some()
        }
        ClientKind::Codex => codex_home().map(|path| path.is_dir()).unwrap_or(false),
        ClientKind::Cursor => {
            home.join(".cursor").is_dir() || Path::new("/Applications/Cursor.app").is_dir()
        }
        ClientKind::Antigravity => {
            find_agy_cli().is_some()
                || home.join(".gemini").join("config").is_dir()
                || Path::new("/Applications/Antigravity.app").is_dir()
        }
    }
}

fn configured_command(kind: ClientKind, text: &str) -> Option<String> {
    match kind {
        ClientKind::Codex => codex_configured_command(text),
        ClientKind::ClaudeCode | ClientKind::Cursor | ClientKind::Antigravity => json_configured_command(text),
    }
}

fn client_status(kind: ClientKind, preferences: &ClientPreferences) -> ClientStatus {
    let path = client_config_path(kind).ok();
    let command = path
        .as_ref()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| configured_command(kind, &text));
    let points_to_current = command.as_deref().map(is_current_server_command).unwrap_or(false);
    // 配置本来就指向 iterate 的稳定入口、但入口已失效：这是我们自己的条目，可以放心修复。
    let needs_repair = !points_to_current
        && command.as_deref().map(|c| Path::new(c) == stable_server_path()).unwrap_or(false);
    let (rules_path_text, rules_state_text) = match rules_file(kind) {
        Some(rules) => {
            let path = rules_path(rules).ok();
            let text = path.as_ref().and_then(|path| fs::read_to_string(path).ok()).unwrap_or_default();
            (
                path.map(|p| p.to_string_lossy().to_string()),
                Some(rules_state(rules, &text).to_string()),
            )
        }
        None => (None, None),
    };
    let (permission_path, permission_state) = match kind {
        ClientKind::ClaudeCode => {
            let path = claude_settings_path().ok();
            let text = path.as_ref().and_then(|path| fs::read_to_string(path).ok()).unwrap_or_default();
            (
                path.map(|p| p.to_string_lossy().to_string()),
                Some(claude_permission_state(&text, preferences.claude_permission_owned).to_string()),
            )
        }
        _ => (None, None),
    };
    ClientStatus {
        id: kind.id().to_string(),
        name: kind.display_name().to_string(),
        detected: client_detected(kind),
        configured: command.is_some(),
        points_to_current,
        needs_repair,
        opted_out: preferences.opted_out.get(kind.id()).copied().unwrap_or(false),
        config_path: path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        configured_command: command,
        rules_path: rules_path_text,
        rules_state: rules_state_text,
        permission_path,
        permission_state,
    }
}

pub fn overview() -> ClientOverview {
    let preferences = load_preferences();
    ClientOverview {
        app_location: app_location_status(),
        clients: ClientKind::ALL
            .into_iter()
            .map(|kind| client_status(kind, &preferences))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// 接入 / 移除
// ---------------------------------------------------------------------------

fn verify_points_to(kind: ClientKind, path: &Path, command: &str) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("回读 {} 失败: {e}", path.display()))?;
    // 先确认整份文件仍可解析，再看条目。
    match kind {
        ClientKind::Codex => {
            parse_codex_document(&text)?;
        }
        ClientKind::ClaudeCode | ClientKind::Cursor | ClientKind::Antigravity => {
            parse_json_object(&text)?;
        }
    }
    match configured_command(kind, &text) {
        Some(actual) if actual == command => Ok(()),
        Some(actual) => Err(format!("回读校验失败：配置指向 {actual}")),
        None => Err("回读校验失败：配置中没有 iterate-zhi 条目".to_string()),
    }
}

fn with_backup_hint(error: String, backup: &Option<PathBuf>) -> String {
    match backup {
        Some(path) => format!("{error}（原配置备份：{}）", path.display()),
        None => error,
    }
}

fn connect_client(kind: ClientKind, command: &str) -> Result<Option<PathBuf>, String> {
    let path = client_config_path(kind)?;
    resolve_write_target(&path)?;
    let original = read_optional(&path)?;
    // 坏文件一律不碰：先解析成功，再备份、再写。
    let merged = match kind {
        ClientKind::Codex => Some(merge_codex_config(&original, command)?),
        ClientKind::Cursor | ClientKind::Antigravity => Some(merge_json_config(&original, command, false)?),
        ClientKind::ClaudeCode => {
            parse_json_object(&original)?;
            None
        }
    };
    let backup = backup_config(kind, &path)?;

    let written = match (kind, merged) {
        (ClientKind::ClaudeCode, _) => connect_claude_code(&path, &original, command),
        (_, Some(merged)) => write_config(&path, merged.as_bytes(), Some(&original)),
        (_, None) => unreachable!("非 Claude Code 客户端总会先算出合并结果"),
    };
    written
        .and_then(|_| verify_points_to(kind, &path, command))
        .map_err(|error| with_backup_hint(error, &backup))?;
    Ok(backup)
}

fn connect_claude_code(path: &Path, original: &str, command: &str) -> Result<(), String> {
    let root = parse_json_object(original)?;
    let existing = root
        .get("mcpServers")
        .and_then(|servers| servers.get(SERVER_NAME))
        .cloned();
    refuse_remote_entry(existing.as_ref().and_then(|e| e.get("url")).is_some())?;
    // 完整条目（含用户原有的 args / env）在这里就保存下来，CLI 中途失败也不会丢。
    let entry = claude_code_server_entry(existing.as_ref(), command);

    // 优先官方 CLI：Claude Code 运行中也会写 ~/.claude.json，由它自己写最安全，也不会打乱 key 顺序。
    let cli_error = match find_claude_cli() {
        Some(cli) => match claude_cli_upsert(&cli, &entry) {
            Ok(()) => return Ok(()),
            Err(error) => error,
        },
        None => "未找到 claude 命令".to_string(),
    };
    log::info!("Claude Code CLI 写入未成功，改用直接合并: {cli_error}");

    // 回退：基于最新内容（可能已被 CLI 的 remove 改过）写回完整条目，写前做冲突检查。
    let latest = read_optional(path)?;
    // 目标条目只允许处于三种已知状态：原样、已被 CLI 删除、已是我们要写的新条目。
    // 其他情况说明期间有别的程序改过 iterate-zhi，不能用旧条目覆盖它。
    let latest_entry = parse_json_object(&latest)?
        .get("mcpServers")
        .and_then(|servers| servers.get(SERVER_NAME))
        .cloned();
    let known_state = latest_entry.is_none()
        || latest_entry == existing
        || latest_entry.as_ref() == Some(&entry);
    if !known_state {
        return Err(format!(
            "Claude Code 命令失败（{cli_error}），且 iterate-zhi 条目期间被其他程序修改，为避免覆盖已停止，请重试"
        ));
    }
    let merged = upsert_json_entry(&latest, entry)?;
    write_config(path, merged.as_bytes(), Some(&latest))
        .map_err(|error| format!("Claude Code 命令失败（{cli_error}），直接写入也失败：{error}"))
}

/// 本进程内的接入 / 移除操作串行执行，避免两次点击各自通过冲突检查后互相覆盖。
/// 跨进程（客户端自己写配置）仍只能靠 write_config 的读取比对缩小窗口，无法完全消除。
static CONFIG_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn config_write_guard() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn connect_clients(ids: &[String]) -> Vec<ClientActionResult> {
    let _guard = config_write_guard();
    let stable = match ensure_stable_server_link() {
        Ok(link) => link,
        Err(problem) => {
            return ids
                .iter()
                .map(|id| ClientActionResult {
                    id: id.clone(),
                    name: ClientKind::from_id(id)
                        .map(|k| k.display_name())
                        .unwrap_or(id.as_str())
                        .to_string(),
                    ok: false,
                    message: problem.clone(),
                    backup_path: None,
                })
                .collect();
        }
    };
    let command = stable.to_string_lossy().to_string();

    let results: Vec<ClientActionResult> = ids
        .iter()
        .map(|id| {
            let Some(kind) = ClientKind::from_id(id) else {
                return ClientActionResult {
                    id: id.clone(),
                    name: id.clone(),
                    ok: false,
                    message: "未知客户端".to_string(),
                    backup_path: None,
                };
            };
            let outcome = connect_client(kind, &command);
            if outcome.is_ok() {
                if let Err(error) = set_opted_out(kind, false) {
                    log::warn!("保存 MCP 客户端偏好失败: {error}");
                }
            }
            match outcome {
                Ok(backup) => ClientActionResult {
                    id: id.clone(),
                    name: kind.display_name().to_string(),
                    ok: true,
                    message: connected_message(kind),
                    backup_path: backup.map(|p| p.to_string_lossy().to_string()),
                },
                Err(error) => ClientActionResult {
                    id: id.clone(),
                    name: kind.display_name().to_string(),
                    ok: false,
                    message: error,
                    backup_path: None,
                },
            }
        })
        .collect();

    let connected: Vec<ClientKind> = results
        .iter()
        .filter(|result| result.ok)
        .filter_map(|result| ClientKind::from_id(&result.id))
        .collect();
    let mut results = results;
    if !connected.is_empty() {
        // 首次建库时把说明（含对话记录告知）挂在第一个成功项上，避免每个客户端重复一遍。
        if let Some(notice) = super::knowledge_starter::ensure_for_clients(&connected) {
            if let Some(first) = results.iter_mut().find(|result| result.ok) {
                first.message = format!("{}\n{}", first.message, notice);
            }
        }
    }
    results
}

/// MCP 已接入后的提示。Codex / Claude Code 还要写调用规则；规则写失败不回滚 MCP，只在提示里说明。
/// Claude Code 另外把 call_zhi 加进权限白名单；同样失败不回滚，只在提示里说明。
fn connected_message(kind: ClientKind) -> String {
    let mut message = kind.restart_hint().to_string();
    if let Some(rules) = rules_file(kind) {
        message.push_str(&match write_rules(rules) {
            Ok(RulesWritten::Installed(path)) => format!("；已在 {} 写入 call_zhi 调用规则", path.display()),
            Ok(RulesWritten::KeptManual(path)) => {
                format!("；{} 里已有你手写的 iterate 交互协议，没有重复写入", path.display())
            }
            Err(error) => format!(
                "；调用规则未写入（{error}），{} 可能不会持续调用 call_zhi",
                kind.display_name()
            ),
        });
    }
    if kind == ClientKind::ClaudeCode {
        message.push_str(&permission_message());
    }
    message
}

/// App 启动时调用：Codex 里已经有本地 iterate-zhi 条目（不管是一键接入还是照文档手动写的）时，
/// 补上长等待和 AGENTS.md 调用规则，用户不用再自己改配置。用户在 iterate 里断开过 Codex 就不碰。
pub fn heal_codex_if_configured() {
    if load_preferences().opted_out.get(ClientKind::Codex.id()).copied().unwrap_or(false) {
        return;
    }
    let _guard = config_write_guard();
    let Ok(path) = client_config_path(ClientKind::Codex) else {
        return;
    };
    let original = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return,
    };
    match codex_configured_command(&original) {
        Some(command) if !command.is_empty() && !command.starts_with("http") => {}
        _ => return,
    }
    match heal_codex_timeout(&original) {
        Ok(Some(healed)) => {
            let outcome = resolve_write_target(&path)
                .and_then(|_| backup_config(ClientKind::Codex, &path))
                .and_then(|_| write_config(&path, healed.as_bytes(), Some(&original)));
            match outcome {
                Ok(()) => log::info!("已为 Codex 的 iterate-zhi 补上长等待（tool_timeout_sec），重启 Codex 后生效"),
                Err(error) => log::warn!("补 Codex 长等待失败: {error}"),
            }
        }
        Ok(None) => {}
        Err(error) => log::warn!("Codex 配置无法解析，跳过自动修复: {error}"),
    }
    let rules_missing = rules_path(CODEX_RULES)
        .ok()
        .map(|rules| read_optional(&rules).map(|text| rules_span(&text, CODEX_RULES_BEGIN).is_none()).unwrap_or(false))
        .unwrap_or(false);
    if rules_missing {
        match write_rules(CODEX_RULES) {
            Ok(RulesWritten::Installed(rules) | RulesWritten::KeptManual(rules)) => {
                log::info!("已在 {} 补写 call_zhi 调用规则", rules.display())
            }
            Err(error) => log::warn!("补写 Codex 调用规则失败: {error}"),
        }
    }
}

fn remove_entry(kind: ClientKind, text: &str) -> Result<Option<String>, String> {
    match kind {
        ClientKind::Codex => remove_codex_server(text),
        ClientKind::ClaudeCode | ClientKind::Cursor | ClientKind::Antigravity => remove_json_server(text),
    }
}

fn disconnect_entry(kind: ClientKind) -> Result<Option<PathBuf>, String> {
    let path = client_config_path(kind)?;
    resolve_write_target(&path)?;
    let original = read_optional(&path)?;
    if remove_entry(kind, &original)?.is_none() {
        return Ok(None);
    }
    let backup = backup_config(kind, &path)?;

    if kind == ClientKind::ClaudeCode {
        if let Some(cli) = find_claude_cli() {
            let mut remove = Command::new(cli);
            remove.args(["mcp", "remove", SERVER_NAME, "-s", "user"]);
            if let Err(error) = run_with_timeout(remove, CLAUDE_CLI_TIMEOUT) {
                log::info!("Claude Code CLI 移除未成功，改用直接修改: {error}");
            }
        }
    }

    // 始终基于最新内容重新计算，不使用执行 CLI 之前的旧快照。
    let latest = read_optional(&path)?;
    if let Some(updated) = remove_entry(kind, &latest)? {
        write_config(&path, updated.as_bytes(), Some(&latest))
            .map_err(|error| with_backup_hint(error, &backup))?;
    }
    Ok(backup)
}

pub fn disconnect_client(id: &str) -> ClientActionResult {
    let _guard = config_write_guard();
    let Some(kind) = ClientKind::from_id(id) else {
        return ClientActionResult {
            id: id.to_string(),
            name: id.to_string(),
            ok: false,
            message: "未知客户端".to_string(),
            backup_path: None,
        };
    };
    // 条目移除成功后，才记住用户的选择（后续不再自动加回）。
    let result = disconnect_entry(kind).and_then(|backup| {
        if let Some(rules) = rules_file(kind) {
            delete_rules(rules)?;
        }
        set_opted_out(kind, true)?;
        Ok(backup)
    });
    // 白名单删不掉（settings.json 坏了等）不影响 MCP 条目已移除的结果，只在提示里说明。
    let permission_note = match (&result, kind) {
        (Ok(_), ClientKind::ClaudeCode) => match delete_claude_permission() {
            Ok(()) => String::new(),
            Err(error) => {
                log::warn!("移除 Claude Code 权限白名单失败: {error}");
                format!("；settings.json 里的 {CLAUDE_ZHI_PERMISSION} 未能删除（{error}）")
            }
        },
        _ => String::new(),
    };
    match result {
        Ok(backup) => ClientActionResult {
            id: id.to_string(),
            name: kind.display_name().to_string(),
            ok: true,
            message: format!("已移除，重启客户端后生效{permission_note}"),
            backup_path: backup.map(|p| p.to_string_lossy().to_string()),
        },
        Err(error) => ClientActionResult {
            id: id.to_string(),
            name: kind.display_name().to_string(),
            ok: false,
            message: error,
            backup_path: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_mcp_client_overview() -> Result<ClientOverview, String> {
    tokio::task::spawn_blocking(overview)
        .await
        .map_err(|e| format!("读取客户端状态失败: {e}"))
}

#[tauri::command]
pub async fn connect_mcp_clients(clients: Vec<String>) -> Result<Vec<ClientActionResult>, String> {
    tokio::task::spawn_blocking(move || connect_clients(&clients))
        .await
        .map_err(|e| format!("接入客户端失败: {e}"))
}

#[tauri::command]
pub async fn disconnect_mcp_client(client: String) -> Result<ClientActionResult, String> {
    tokio::task::spawn_blocking(move || disconnect_client(&client))
        .await
        .map_err(|e| format!("移除客户端失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> DocumentMut {
        text.parse::<DocumentMut>().expect("output must stay valid TOML")
    }

    fn server<'a>(doc: &'a DocumentMut, key: &str) -> &'a dyn toml_edit::TableLike {
        doc["mcp_servers"][key].as_table_like().expect("server table")
    }

    // --- Codex：手动配置缺长等待时自动补上 ---

    #[test]
    fn codex_heal_adds_missing_timeout_and_keeps_everything_else() {
        let input = "# 手写的注释\n[mcp_servers.iterate-zhi]\ncommand = 'C:\\Users\\username\\AppData\\Local\\iterate\\bin\\mcp-server.exe'\nargs = []\n\n[mcp_servers.other]\ncommand = \"/other\"\n";
        let output = heal_codex_timeout(input).unwrap().expect("should heal");
        assert!(output.starts_with("# 手写的注释\n"));
        let doc = parse(&output);
        let s = server(&doc, "iterate-zhi");
        assert_eq!(s.get("tool_timeout_sec").unwrap().as_integer(), Some(315_360_000));
        assert!(s.get("command").unwrap().as_str().unwrap().ends_with("mcp-server.exe"));
        assert!(server(&doc, "other").get("tool_timeout_sec").is_none());
    }

    #[test]
    fn codex_heal_raises_short_timeouts_only() {
        let short = "[mcp_servers.\"iterate-zhi\"]\ncommand = \"/x\"\ntool_timeout_sec = 60\n";
        let healed = heal_codex_timeout(short).unwrap().expect("60s must be raised");
        assert_eq!(server(&parse(&healed), "iterate-zhi").get("tool_timeout_sec").unwrap().as_integer(), Some(315_360_000));

        for ok in [
            "[mcp_servers.\"iterate-zhi\"]\ncommand = \"/x\"\ntool_timeout_sec = 315360000.0\n",
            "[mcp_servers.\"iterate-zhi\"]\ncommand = \"/x\"\ntool_timeout_sec = 604800\n",
        ] {
            assert_eq!(heal_codex_timeout(ok).unwrap(), None, "{ok}");
        }
    }

    #[test]
    fn codex_heal_leaves_unrelated_or_remote_configs_alone() {
        for text in [
            "",
            "[mcp_servers.other]\ncommand = \"/other\"\n",
            "[mcp_servers.\"iterate-zhi\"]\nurl = \"https://example.com/mcp\"\n",
        ] {
            assert_eq!(heal_codex_timeout(text).unwrap(), None, "{text}");
        }
        assert!(heal_codex_timeout("not = [valid").is_err());
    }

    #[test]
    fn codex_heal_supports_legacy_key_and_inline_tables() {
        let legacy = "[mcp_servers.cunzhi]\ncommand = \"/x\"\n";
        let healed = heal_codex_timeout(legacy).unwrap().unwrap();
        assert_eq!(server(&parse(&healed), "cunzhi").get("tool_timeout_sec").unwrap().as_integer(), Some(315_360_000));

        let inline = "mcp_servers = { \"iterate-zhi\" = { command = \"/x\" } }\n";
        let healed = heal_codex_timeout(inline).unwrap().unwrap();
        assert_eq!(server(&parse(&healed), "iterate-zhi").get("tool_timeout_sec").unwrap().as_integer(), Some(315_360_000));
    }

    #[test]
    fn bundled_server_name_matches_platform_executable() {
        let name = bundled_server_file_name();
        if cfg!(windows) {
            assert_eq!(name, "mcp-server.exe");
        } else {
            assert_eq!(name, "mcp-server");
        }
    }

    // --- Codex：覆盖 scripts/merge-codex-mcp-config.test.mjs 的原有场景 ---

    #[test]
    fn codex_preserves_existing_options_while_updating_binary_path() {
        let output = merge_codex_config(
            r#"[mcp_servers."iterate-zhi"]
command = "/old/mcp-server"
args = ["5411"]
tool_timeout_sec = 315360000.0
disabled = false

[mcp_servers."iterate-zhi".tools.call_zhi]
approval_mode = "approve"

[mcp_servers.other]
command = "/other"
"#,
            "/new/mcp-server",
        )
        .unwrap();
        let doc = parse(&output);
        let s = server(&doc, "iterate-zhi");
        assert_eq!(s.get("command").unwrap().as_str(), Some("/new/mcp-server"));
        assert_eq!(s.get("args").unwrap().to_string().trim(), r#"["5411"]"#);
        assert_eq!(s.get("tool_timeout_sec").unwrap().as_integer(), Some(315_360_000));
        assert_eq!(s.get("disabled").unwrap().as_bool(), Some(false));
        assert_eq!(doc["mcp_servers"]["iterate-zhi"]["tools"]["call_zhi"]["approval_mode"].as_str(), Some("approve"));
        assert_eq!(server(&doc, "other").get("command").unwrap().as_str(), Some("/other"));
    }

    #[test]
    fn codex_fresh_config_auto_approves_call_zhi() {
        let output = merge_codex_config("", "/new/mcp-server").unwrap();
        let doc = parse(&output);
        let s = server(&doc, SERVER_NAME);
        assert_eq!(s.get("command").unwrap().as_str(), Some("/new/mcp-server"));
        assert_eq!(s.get("args").unwrap().as_array().map(|a| a.len()), Some(0));
        assert_eq!(s.get("tool_timeout_sec").unwrap().as_integer(), Some(315_360_000));
        assert_eq!(doc["mcp_servers"][SERVER_NAME]["tools"]["call_zhi"]["approval_mode"].as_str(), Some("approve"));
    }

    #[test]
    fn codex_adds_default_approval_to_existing_tool_block() {
        let output = merge_codex_config(
            "[mcp_servers.\"iterate-zhi\"]\ncommand = \"/old\"\n\n[mcp_servers.\"iterate-zhi\".tools.call_zhi]\nenabled = true\n",
            "/new",
        )
        .unwrap();
        let doc = parse(&output);
        let call = &doc["mcp_servers"]["iterate-zhi"]["tools"]["call_zhi"];
        assert_eq!(call["enabled"].as_bool(), Some(true));
        assert_eq!(call["approval_mode"].as_str(), Some("approve"));
    }

    #[test]
    fn codex_preserves_explicit_user_approval_mode() {
        let output = merge_codex_config(
            "[mcp_servers.\"iterate-zhi\"]\ncommand = \"/old\"\n\n[mcp_servers.\"iterate-zhi\".tools.call_zhi]\napproval_mode = \"prompt\"\n",
            "/new",
        )
        .unwrap();
        assert_eq!(parse(&output)["mcp_servers"]["iterate-zhi"]["tools"]["call_zhi"]["approval_mode"].as_str(), Some("prompt"));
    }

    #[test]
    fn codex_keeps_comments_unrelated_settings_and_is_idempotent() {
        let initial = "# 我的配置\nmodel = \"gpt-6\" # 默认模型\n\n[mcp_servers.other]\ncommand = \"/other\"\n";
        let once = merge_codex_config(initial, "/new").unwrap();
        let twice = merge_codex_config(&once, "/new").unwrap();
        assert!(once.starts_with("# 我的配置\nmodel = \"gpt-6\" # 默认模型\n"));
        assert!(once.contains("[mcp_servers.other]\ncommand = \"/other\"\n"));
        assert_eq!(once, twice);
        assert_eq!(codex_configured_command(&once).as_deref(), Some("/new"));
    }

    #[test]
    fn codex_reuses_legacy_cunzhi_entry_instead_of_adding_second_one() {
        let output = merge_codex_config("[mcp_servers.cunzhi]\ncommand = \"/old\"\nargs = [\"5311\"]", "/new").unwrap();
        let doc = parse(&output);
        assert!(doc["mcp_servers"].get(SERVER_NAME).is_none());
        assert_eq!(server(&doc, "cunzhi").get("command").unwrap().as_str(), Some("/new"));
        assert_eq!(codex_configured_command(&output).as_deref(), Some("/new"));
    }

    #[test]
    fn codex_path_with_spaces_and_unicode_round_trips() {
        let command = "/Users/example/测试 用户/Library/Application Support/iterate/bin/mcp-server";
        let output = merge_codex_config("", command).unwrap();
        assert_eq!(codex_configured_command(&output).as_deref(), Some(command));
    }

    // --- Codex：审查发现的回归 ---

    #[test]
    fn codex_indented_following_table_is_not_swallowed() {
        let initial = "[mcp_servers.iterate-zhi]\ncommand = \"/old\"\n\n  [mcp_servers.other]\n  command = \"/other\"\n";
        let merged = merge_codex_config(initial, "/new").unwrap();
        let doc = parse(&merged);
        assert_eq!(server(&doc, "other").get("command").unwrap().as_str(), Some("/other"));
        assert!(server(&doc, "other").get("tool_timeout_sec").is_none());

        let removed = remove_codex_server(&merged).unwrap().expect("entry existed");
        let doc = parse(&removed);
        assert!(doc["mcp_servers"].get(SERVER_NAME).is_none());
        assert_eq!(server(&doc, "other").get("command").unwrap().as_str(), Some("/other"));
    }

    #[test]
    fn codex_header_with_trailing_comment_is_updated_in_place() {
        let initial = "[mcp_servers.iterate-zhi] # user comment\ncommand = \"/old\"\n";
        let output = merge_codex_config(initial, "/new").unwrap();
        assert!(output.contains("# user comment"));
        assert_eq!(output.matches("[mcp_servers.iterate-zhi]").count(), 1);
        assert_eq!(codex_configured_command(&output).as_deref(), Some("/new"));
    }

    #[test]
    fn codex_fake_header_inside_multiline_string_is_ignored() {
        let initial = "instructions = \"\"\"\n[mcp_servers.iterate-zhi]\ncommand = \"/example\"\n\"\"\"\n";
        assert_eq!(codex_configured_command(initial), None);
        assert!(remove_codex_server(initial).unwrap().is_none());
        let output = merge_codex_config(initial, "/new").unwrap();
        let doc = parse(&output);
        assert!(doc["instructions"].as_str().unwrap().contains("/example"));
        assert_eq!(codex_configured_command(&output).as_deref(), Some("/new"));
    }

    #[test]
    fn codex_invalid_or_non_table_config_is_refused() {
        assert!(merge_codex_config("[broken\ncommand =", "/new").is_err());
        assert!(remove_codex_server("[broken\n").is_err());
        assert!(merge_codex_config("mcp_servers = \"oops\"\n", "/new").is_err());
        assert!(merge_codex_config("[mcp_servers]\niterate-zhi = \"oops\"\n", "/new").is_err());
        assert!(merge_codex_config("[mcp_servers.iterate-zhi]\ncommand = \"/o\"\ntools = 1\n", "/new").is_err());
    }

    #[test]
    fn codex_inline_servers_table_is_merged_in_place() {
        let text = "mcp_servers = { other = { command = \"/o\" } }\n";
        let output = merge_codex_config(text, "/new").unwrap();
        let doc: DocumentMut = output.parse().unwrap();
        let servers = doc["mcp_servers"].as_inline_table().expect("保持内联写法");
        assert_eq!(servers.get("other").unwrap().as_inline_table().unwrap().get("command").unwrap().as_str(), Some("/o"));
        assert_eq!(codex_configured_command(&output).as_deref(), Some("/new"));
        let server = servers.get(SERVER_NAME).unwrap().as_inline_table().unwrap();
        assert_eq!(server.get("tool_timeout_sec").unwrap().as_integer(), Some(CODEX_TIMEOUT_SECONDS as i64));
        let call_zhi = server.get("tools").unwrap().as_inline_table().unwrap().get("call_zhi").unwrap().as_inline_table().unwrap();
        assert_eq!(call_zhi.get("approval_mode").unwrap().as_str(), Some("approve"));
        assert_eq!(merge_codex_config(&output, "/new").unwrap(), output, "重复接入应幂等");
    }

    #[test]
    fn codex_inline_server_entry_keeps_user_choices() {
        let text = "[mcp_servers]\niterate-zhi = { command = \"/old\", args = [\"--x\"], tools = { call_zhi = { approval_mode = \"prompt\" } } }\n";
        let output = merge_codex_config(text, "/new").unwrap();
        let doc: DocumentMut = output.parse().unwrap();
        let server = doc["mcp_servers"][SERVER_NAME].as_inline_table().expect("保持内联写法");
        assert_eq!(server.get("command").unwrap().as_str(), Some("/new"));
        assert_eq!(server.get("args").unwrap().as_array().unwrap().len(), 1);
        let call_zhi = server.get("tools").unwrap().as_inline_table().unwrap().get("call_zhi").unwrap().as_inline_table().unwrap();
        assert_eq!(call_zhi.get("approval_mode").unwrap().as_str(), Some("prompt"));
        assert_eq!(remove_codex_server(&output).unwrap().map(|t| t.contains(SERVER_NAME)), Some(false));
    }

    #[test]
    fn codex_remove_does_not_touch_similarly_named_server() {
        let text = "[mcp_servers.iterate-zhi-dev]\ncommand = \"/dev\"\n";
        assert!(remove_codex_server(text).unwrap().is_none());
    }

    // --- JSON（Claude Code / Cursor）---

    #[test]
    fn json_merge_creates_entry_in_empty_file() {
        let output = merge_json_config("", "/stable/mcp-server", false).unwrap();
        assert_eq!(json_configured_command(&output).as_deref(), Some("/stable/mcp-server"));
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["mcpServers"][SERVER_NAME]["args"], json!([]));
        assert!(value["mcpServers"][SERVER_NAME].get("type").is_none());
    }

    #[test]
    fn json_merge_keeps_other_servers_and_user_fields() {
        let initial = r#"{
  "numStartups": 12,
  "mcpServers": {
    "other": { "command": "/other" },
    "iterate-zhi": { "command": "/old", "args": ["5311"], "env": { "A": "1" } }
  }
}"#;
        let output = merge_json_config(initial, "/stable/mcp-server", true).unwrap();
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["numStartups"], json!(12));
        assert_eq!(value["mcpServers"]["other"]["command"], json!("/other"));
        let entry = &value["mcpServers"][SERVER_NAME];
        assert_eq!(entry["command"], json!("/stable/mcp-server"));
        assert_eq!(entry["args"], json!(["5311"]));
        assert_eq!(entry["env"]["A"], json!("1"));
        assert_eq!(entry["type"], json!("stdio"));
    }

    /// CLI remove 成功、add 失败后，回退用事先保存的完整条目，自定义 args/env 不丢。
    #[test]
    fn claude_code_entry_gets_long_timeout_but_keeps_user_value() {
        let fresh = claude_code_server_entry(None, "/stable");
        assert_eq!(fresh["timeout"], json!(CLAUDE_CODE_TIMEOUT_MS));
        assert_eq!(fresh["type"], json!("stdio"));

        let existing = json!({"command": "/old", "timeout": 60000, "env": {"A": "1"}});
        let kept = claude_code_server_entry(Some(&existing), "/stable");
        assert_eq!(kept["timeout"], json!(60000));
        assert_eq!(kept["command"], json!("/stable"));
        assert_eq!(kept["env"]["A"], json!("1"));

        // Cursor 等其他 JSON 客户端不写 timeout。
        let cursor = merge_json_config("", "/stable", false).unwrap();
        let value: Value = serde_json::from_str(&cursor).unwrap();
        assert!(value["mcpServers"][SERVER_NAME].get("timeout").is_none());
    }

    #[test]
    fn json_upsert_restores_saved_entry_after_cli_removed_it() {
        let original = r#"{"mcpServers":{"iterate-zhi":{"command":"/old","args":["5311"],"env":{"A":"1"}},"other":{"command":"/o"}}}"#;
        let root = parse_json_object(original).unwrap();
        let entry = merge_json_server_entry(root["mcpServers"].get(SERVER_NAME), "/stable", true);
        // 模拟 CLI 已把条目删掉，且 Claude 期间又写入了新字段。
        let after_remove = r#"{"mcpServers":{"other":{"command":"/o"}},"newField":true}"#;
        let output = upsert_json_entry(after_remove, entry).unwrap();
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["newField"], json!(true));
        assert_eq!(value["mcpServers"]["other"]["command"], json!("/o"));
        let restored = &value["mcpServers"][SERVER_NAME];
        assert_eq!(restored["command"], json!("/stable"));
        assert_eq!(restored["args"], json!(["5311"]));
        assert_eq!(restored["env"]["A"], json!("1"));
    }

    #[test]
    fn remote_url_entries_count_as_configured_and_are_not_migrated() {
        let json = r#"{"mcpServers":{"iterate-zhi":{"type":"http","url":"https://relay.example/mcp"}}}"#;
        assert_eq!(json_configured_command(json).as_deref(), Some("https://relay.example/mcp"));
        assert!(merge_json_config(json, "/stable", true).is_err());

        let toml = "[mcp_servers.iterate-zhi]\nurl = \"https://relay.example/mcp\"\n";
        assert_eq!(codex_configured_command(toml).as_deref(), Some("https://relay.example/mcp"));
        assert!(merge_codex_config(toml, "/stable").is_err());
    }

    #[test]
    fn json_merge_refuses_invalid_or_unexpected_files() {
        assert!(merge_json_config("{ not json", "/x", false).is_err());
        assert!(merge_json_config("[]", "/x", false).is_err());
        assert!(merge_json_config(r#"{"mcpServers": []}"#, "/x", false).is_err());
    }

    #[test]
    fn json_remove_only_drops_iterate_entry() {
        let merged = merge_json_config(r#"{"mcpServers":{"other":{"command":"/o"}}}"#, "/x", false).unwrap();
        let removed = remove_json_server(&merged).unwrap().expect("entry existed");
        let value: Value = serde_json::from_str(&removed).unwrap();
        assert!(value["mcpServers"].get(SERVER_NAME).is_none());
        assert_eq!(value["mcpServers"]["other"]["command"], json!("/o"));
        assert!(remove_json_server(&removed).unwrap().is_none());
    }

    // --- 位置与文件写入 ---

    #[test]
    fn translocated_or_dmg_locations_are_rejected() {
        assert!(unstable_location_reason(Path::new(
            "/private/var/folders/x/AppTranslocation/ABC/d/iterate.app/Contents/MacOS/mcp-server"
        ))
        .is_some());
        assert!(unstable_location_reason(Path::new("/Volumes/iterate/iterate.app/Contents/MacOS/mcp-server")).is_some());
        assert!(unstable_location_reason(Path::new("/Applications/iterate.app/Contents/MacOS/mcp-server")).is_none());
    }

    #[test]
    fn atomic_write_follows_symlinked_config_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        fs::write(&real, "{}").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        atomic_write(&link, b"{\"a\":1}").unwrap();

        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "{\"a\":1}");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn dangling_symlink_config_is_refused_and_left_intact() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(dir.path().join("missing.json"), &link).unwrap();

        assert!(atomic_write(&link, b"{}").is_err());
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(!dir.path().join("missing.json").exists());
    }

    #[test]
    fn write_is_abandoned_when_file_changed_since_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{\"v\":1}").unwrap();
        let snapshot = read_optional(&path).unwrap();
        // 读取之后，客户端又写了一次。
        fs::write(&path, "{\"v\":2}").unwrap();

        assert!(write_config(&path, b"{\"v\":3}", Some(&snapshot)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"v\":2}");

        let fresh = read_optional(&path).unwrap();
        write_config(&path, b"{\"v\":3}", Some(&fresh)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"v\":3}");
    }

    #[test]
    fn broken_stable_link_is_not_reported_as_current() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("mcp-server");
        std::os::unix::fs::symlink(dir.path().join("gone"), &link).unwrap();
        assert!(!is_current_server_command(&link.to_string_lossy()));
    }

    // --- Codex 调用规则（AGENTS.md）---

    #[test]
    fn codex_rules_created_in_empty_agents_md() {
        let output = upsert_codex_rules("");
        assert!(output.starts_with(CODEX_RULES_BEGIN));
        assert!(output.contains("call_zhi"));
        assert!(output.ends_with(&format!("{RULES_END}\n")));
    }

    #[test]
    fn codex_rules_appended_after_user_content_and_idempotent() {
        let user = "# 我的规则\n\n- 用中文回答\n";
        let once = upsert_codex_rules(user);
        assert!(once.starts_with(user.trim_end()));
        assert_eq!(upsert_codex_rules(&once), once, "重复接入不应叠加第二段");
        assert_eq!(once.matches(CODEX_RULES_BEGIN).count(), 1);
    }

    #[test]
    fn codex_rules_replaced_in_place_keeping_surrounding_text() {
        let old = format!("前文\n\n{CODEX_RULES_BEGIN}\n旧规则\n{RULES_END}\n\n后文\n");
        let output = upsert_codex_rules(&old);
        assert!(output.starts_with("前文\n\n"));
        assert!(output.ends_with("\n\n后文\n"));
        assert!(!output.contains("旧规则"));
        assert!(output.contains(IZHI_RULES_BODY));
    }

    #[test]
    fn codex_rules_removal_restores_user_content() {
        let user = "# 我的规则\n\n- 用中文回答\n";
        assert_eq!(remove_codex_rules(&upsert_codex_rules(user)).as_deref(), Some(user));
        assert_eq!(remove_codex_rules(&upsert_codex_rules("")).as_deref(), Some(""));

        let middle = format!("前文\n\n{}\n\n后文\n", rules_block(CODEX_RULES_BEGIN));
        assert_eq!(remove_codex_rules(&middle).as_deref(), Some("前文\n\n后文\n"));
    }

    #[test]
    fn codex_rules_removal_ignores_files_without_block() {
        assert_eq!(remove_codex_rules("# 我的规则\n"), None);
        // 只有开始标记、缺结束标记时不动文件，避免误删用户内容。
        assert_eq!(remove_codex_rules(&format!("{CODEX_RULES_BEGIN}\n用户续写的内容\n")), None);
    }

    // --- Claude Code 调用规则（CLAUDE.md）---

    #[test]
    fn claude_rules_share_the_codex_protocol_text() {
        let codex = upsert_codex_rules("");
        let claude = upsert_claude_rules("");
        assert!(claude.starts_with(CLAUDE_RULES_BEGIN));
        assert!(claude.ends_with(&format!("{RULES_END}\n")));
        // 两家只有开始标记不同，正文是同一个常量。
        assert!(codex.contains(IZHI_RULES_BODY) && claude.contains(IZHI_RULES_BODY));
        assert_eq!(codex.replace(CODEX_RULES_BEGIN, ""), claude.replace(CLAUDE_RULES_BEGIN, ""));
    }

    #[test]
    fn claude_rules_created_in_empty_claude_md() {
        let output = upsert_claude_rules("");
        assert!(output.contains("call_zhi"));
        assert_eq!(output.matches(CLAUDE_RULES_BEGIN).count(), 1);
        assert_eq!(output.matches(RULES_END).count(), 1);
    }

    #[test]
    fn claude_rules_appended_after_user_content_and_idempotent() {
        let user = "# 我的偏好\n\n- 用中文回答\n- 不要用 emoji\n";
        let once = upsert_claude_rules(user);
        assert!(once.starts_with(user.trim_end()));
        let twice = upsert_claude_rules(&once);
        assert_eq!(twice, once, "重复接入不应叠加第二段");
        assert_eq!(twice.matches(CLAUDE_RULES_BEGIN).count(), 1);
        assert_eq!(twice.matches(RULES_HEADING).count(), 1);
    }

    #[test]
    fn claude_rules_replaced_in_place_keeping_surrounding_text() {
        let old = format!("前文\n\n{CLAUDE_RULES_BEGIN}\n旧规则\n{RULES_END}\n\n后文\n");
        let output = upsert_claude_rules(&old);
        assert!(output.starts_with("前文\n\n"));
        assert!(output.ends_with("\n\n后文\n"));
        assert!(!output.contains("旧规则"));
        assert!(output.contains(IZHI_RULES_BODY));
    }

    #[test]
    fn claude_rules_removal_restores_user_content() {
        let user = "# 我的偏好\n\n- 用中文回答\n";
        assert_eq!(remove_claude_rules(&upsert_claude_rules(user)).as_deref(), Some(user));
        assert_eq!(remove_claude_rules(&upsert_claude_rules("")).as_deref(), Some(""));
        let middle = format!("前文\n\n{}\n\n后文\n", rules_block(CLAUDE_RULES_BEGIN));
        assert_eq!(remove_claude_rules(&middle).as_deref(), Some("前文\n\n后文\n"));
    }

    #[test]
    fn claude_rules_removal_ignores_files_without_block() {
        assert_eq!(remove_claude_rules("# 我的偏好\n"), None);
        assert_eq!(remove_claude_rules(&format!("{CLAUDE_RULES_BEGIN}\n用户续写的内容\n")), None);
        // Codex 的标记段不归 Claude Code 管，反之亦然。
        assert_eq!(remove_claude_rules(&upsert_codex_rules("")), None);
        assert_eq!(remove_codex_rules(&upsert_claude_rules("")), None);
    }

    #[test]
    fn claude_rules_state_detects_hand_written_protocol() {
        let manual = "# 我的偏好\n\n## iterate 交互协议（必须遵守）\n1. 每次回复都调用 call_zhi\n";
        assert_eq!(rules_state(CLAUDE_RULES, manual), "manual");
        assert_eq!(rules_state(CLAUDE_RULES, &upsert_claude_rules(manual)), "installed");
        assert_eq!(rules_state(CLAUDE_RULES, "# 我的偏好\n"), "missing");
        assert_eq!(rules_state(CLAUDE_RULES, ""), "missing");
        // Codex 保持原行为：没有标记段就算缺失，接入时照常补写。
        assert_eq!(rules_state(CODEX_RULES, manual), "missing");
    }

    // --- Claude Code 权限白名单（~/.claude/settings.json）---

    fn allow_list(text: &str) -> Vec<String> {
        let root: Value = serde_json::from_str(text).unwrap();
        root["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    fn added(edit: PermissionEdit) -> String {
        match edit {
            PermissionEdit::Added(text) => text,
            other => panic!("应当新加白名单，实际 {other:?}"),
        }
    }

    #[test]
    fn permission_entry_is_the_narrow_call_zhi_rule() {
        assert_eq!(CLAUDE_ZHI_PERMISSION, format!("mcp__{SERVER_NAME}__call_zhi"));
    }

    #[test]
    fn permission_added_to_missing_or_empty_settings() {
        for text in ["", "  \n", "{}"] {
            let output = added(add_claude_permission(text).unwrap());
            assert_eq!(allow_list(&output), vec![CLAUDE_ZHI_PERMISSION]);
            assert!(output.ends_with('\n'));
        }
    }

    #[test]
    fn permission_add_preserves_other_keys_and_entries() {
        let original = r#"{
  "model": "opus",
  "permissions": {
    "allow": ["Bash(git status)", "mcp__other__tool"],
    "deny": ["Bash(rm -rf /)"],
    "defaultMode": "acceptEdits"
  },
  "hooks": {"Stop": []},
  "env": {"FOO": "1"}
}"#;
        let output = added(add_claude_permission(original).unwrap());
        let root: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            allow_list(&output),
            vec!["Bash(git status)", "mcp__other__tool", CLAUDE_ZHI_PERMISSION]
        );
        assert_eq!(root["model"], "opus");
        assert_eq!(root["permissions"]["deny"], json!(["Bash(rm -rf /)"]));
        assert_eq!(root["permissions"]["defaultMode"], "acceptEdits");
        assert_eq!(root["hooks"], json!({"Stop": []}));
        assert_eq!(root["env"], json!({"FOO": "1"}));
    }

    #[test]
    fn permission_add_is_deduped() {
        let once = added(add_claude_permission("{}").unwrap());
        assert_eq!(add_claude_permission(&once).unwrap(), PermissionEdit::AlreadyAllowed);
        // 用户自己的整服务器放行或通配也已覆盖 call_zhi，不再追加更窄的一条。
        for rule in ["mcp__iterate-zhi", "mcp__iterate-zhi__*", " mcp__iterate-zhi__call_zhi "] {
            let text = json!({"permissions": {"allow": [rule]}}).to_string();
            assert_eq!(add_claude_permission(&text).unwrap(), PermissionEdit::AlreadyAllowed, "{rule}");
        }
    }

    #[test]
    fn permission_add_respects_user_deny_or_ask() {
        for list in ["deny", "ask"] {
            let text = json!({"permissions": {list: ["mcp__iterate-zhi__call_zhi"]}}).to_string();
            assert_eq!(add_claude_permission(&text).unwrap(), PermissionEdit::UserRestricted);
            assert_eq!(claude_permission_state(&text, false), "manual");
        }
    }

    #[test]
    fn permission_add_refuses_invalid_settings() {
        for text in [
            "{ not json",
            "[]",
            r#"{"permissions": []}"#,
            r#"{"permissions": {"allow": "mcp__iterate-zhi__call_zhi,Bash"}}"#,
        ] {
            assert!(add_claude_permission(text).is_err(), "{text}");
        }
        assert_eq!(claude_permission_state("{ not json", false), "invalid");
    }

    #[test]
    fn permission_remove_only_drops_our_exact_entry() {
        let text = json!({
            "permissions": {"allow": ["Bash(ls)", CLAUDE_ZHI_PERMISSION, "mcp__iterate-zhi", "mcp__other__call_zhi"]},
            "model": "opus"
        })
        .to_string();
        let output = remove_claude_permission(&text).unwrap().unwrap();
        assert_eq!(allow_list(&output), vec!["Bash(ls)", "mcp__iterate-zhi", "mcp__other__call_zhi"]);
        assert_eq!(serde_json::from_str::<Value>(&output).unwrap()["model"], "opus");
        assert_eq!(remove_claude_permission(&output).unwrap(), None);
        assert_eq!(remove_claude_permission("").unwrap(), None);
        assert_eq!(remove_claude_permission(r#"{"model":"opus"}"#).unwrap(), None);
        assert!(remove_claude_permission("{ not json").is_err());
    }

    #[test]
    fn permission_state_distinguishes_ours_from_users() {
        let ours = added(add_claude_permission("{}").unwrap());
        assert_eq!(claude_permission_state(&ours, true), "installed");
        assert_eq!(claude_permission_state(&ours, false), "manual");
        assert_eq!(claude_permission_state(r#"{"permissions":{"allow":["mcp__iterate-zhi"]}}"#, true), "manual");
        assert_eq!(claude_permission_state("", false), "missing");
        assert_eq!(claude_permission_state(r#"{"permissions":{"allow":["Bash(ls)"]}}"#, true), "missing");
    }

    #[test]
    fn permission_file_round_trip_backs_up_dedupes_and_removes_only_ours() {
        let dir = tempfile::tempdir().unwrap();
        let backups = dir.path().join("backups");
        let settings = dir.path().join(".claude").join("settings.json");

        // 1. 文件不存在：新建，没有可备份的原文件
        let (written, added_now) = write_claude_permission_at(&settings, &backups, false).unwrap();
        assert!(matches!(written, PermissionWritten::Installed(_)));
        assert!(added_now);
        assert_eq!(allow_list(&fs::read_to_string(&settings).unwrap()), vec![CLAUDE_ZHI_PERMISSION]);
        assert!(!backups.exists());

        // 2. 再点一次：不重复、不改文件、不再备份
        let before = fs::read_to_string(&settings).unwrap();
        let (written, added_now) = write_claude_permission_at(&settings, &backups, true).unwrap();
        assert!(matches!(written, PermissionWritten::Installed(_)));
        assert!(!added_now);
        assert_eq!(fs::read_to_string(&settings).unwrap(), before);
        assert!(!backups.exists());

        // 3. 已有用户配置：先备份原文，再追加；其他内容保留
        let user = "{\n  \"permissions\": {\n    \"allow\": [\"Bash(ls)\"]\n  },\n  \"model\": \"opus\"\n}\n";
        fs::write(&settings, user).unwrap();
        let (_, added_now) = write_claude_permission_at(&settings, &backups, true).unwrap();
        assert!(added_now);
        let backup_files: Vec<_> = fs::read_dir(&backups).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(backup_files.len(), 1);
        let backup_name = backup_files[0].file_name().unwrap().to_string_lossy().to_string();
        assert!(backup_name.starts_with("claude-code-") && backup_name.ends_with("-settings.json"), "{backup_name}");
        assert_eq!(fs::read_to_string(&backup_files[0]).unwrap(), user);
        assert_eq!(allow_list(&fs::read_to_string(&settings).unwrap()), vec!["Bash(ls)", CLAUDE_ZHI_PERMISSION]);

        // 4. 移除：只删我们那条，用户条目和其他键原样
        // （与 ~/.claude.json 的直接合并一样经 render_json 输出：两格缩进、末尾换行；serde_json 未开 preserve_order，键按字母序）
        assert!(delete_claude_permission_at(&settings, &backups, true).unwrap());
        let after_remove = fs::read_to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&after_remove).unwrap(),
            serde_json::from_str::<Value>(user).unwrap()
        );
        assert!(after_remove.ends_with("}\n"));
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 2);

        // 5. 接入前用户就有同一条：不记为 iterate 的，移除时保留
        let users_own = json!({"permissions": {"allow": [CLAUDE_ZHI_PERMISSION]}}).to_string();
        fs::write(&settings, &users_own).unwrap();
        let (written, added_now) = write_claude_permission_at(&settings, &backups, false).unwrap();
        assert!(matches!(written, PermissionWritten::KeptUser(_)));
        assert!(!added_now);
        assert!(!delete_claude_permission_at(&settings, &backups, false).unwrap());
        assert_eq!(fs::read_to_string(&settings).unwrap(), users_own);

        // 6. 坏 JSON：报错，文件原样，不备份
        let broken = "{ \"permissions\": { \"allow\": [ ";
        fs::write(&settings, broken).unwrap();
        let backups_before = fs::read_dir(&backups).unwrap().count();
        assert!(write_claude_permission_at(&settings, &backups, false).is_err());
        assert!(delete_claude_permission_at(&settings, &backups, true).is_err());
        assert_eq!(fs::read_to_string(&settings).unwrap(), broken);
        assert_eq!(fs::read_dir(&backups).unwrap().count(), backups_before);
    }

    #[test]
    fn preferences_keep_permission_ownership_alongside_opt_outs() {
        let parsed: ClientPreferences = serde_json::from_str(r#"{"opted_out":{"codex":true}}"#).unwrap();
        assert!(!parsed.claude_permission_owned, "旧版偏好文件默认不是 iterate 加的");
        let mut prefs = parsed;
        prefs.claude_permission_owned = true;
        let text = serde_json::to_string(&prefs).unwrap();
        let back: ClientPreferences = serde_json::from_str(&text).unwrap();
        assert!(back.claude_permission_owned);
        assert_eq!(back.opted_out.get("codex"), Some(&true));
    }

    /// 端到端：在隔离的 HOME 里走真实的写入 / 断开路径（不会碰真实的 ~/.claude/CLAUDE.md）。
    /// 运行：HOME=<临时目录> ITERATE_CONFIG_DIR=<临时目录>/cfg ITERATE_ISOLATED_HOME=<同一临时目录> \
    ///       cargo test --lib isolated_home_rules_round_trip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn isolated_home_rules_round_trip() {
        let home = std::env::var("HOME").unwrap();
        let isolated = std::env::var("ITERATE_ISOLATED_HOME").expect("需要 ITERATE_ISOLATED_HOME");
        assert_eq!(home, isolated, "HOME 必须指向隔离目录");
        assert!(std::env::var("ITERATE_CONFIG_DIR").unwrap().starts_with(&isolated));
        assert_eq!(dirs::home_dir().unwrap(), PathBuf::from(&isolated));

        let claude_md = PathBuf::from(&isolated).join(".claude").join("CLAUDE.md");
        let agents_md = PathBuf::from(&isolated).join(".codex").join("AGENTS.md");
        assert_eq!(rules_path(CLAUDE_RULES).unwrap(), claude_md);

        // 1. 文件不存在：创建
        let _ = fs::remove_file(&claude_md);
        println!("[1] {}", connected_message(ClientKind::ClaudeCode));
        let first = fs::read_to_string(&claude_md).unwrap();
        assert_eq!(first.matches(CLAUDE_RULES_BEGIN).count(), 1);
        println!("--- CLAUDE.md（新建）---\n{first}--- end ---");

        // 2. 二次接入：内容不变
        println!("[2] {}", connected_message(ClientKind::ClaudeCode));
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), first);

        // 3. 用户已有内容：追加在后面，保留原文
        let user = "# 我的偏好\n\n- 用中文回答\n";
        fs::write(&claude_md, user).unwrap();
        println!("[3] {}", connected_message(ClientKind::ClaudeCode));
        println!("[3] {}", connected_message(ClientKind::ClaudeCode));
        let merged = fs::read_to_string(&claude_md).unwrap();
        assert!(merged.starts_with(user.trim_end()));
        assert_eq!(merged.matches(RULES_HEADING).count(), 1);
        let status = overview().clients.into_iter().find(|c| c.id == "claude-code").unwrap();
        println!("[3] overview: rules_path={:?} rules_state={:?}", status.rules_path, status.rules_state);
        assert_eq!(status.rules_state.as_deref(), Some("installed"));

        // 4. 断开：整段删除，用户原文原样留下
        let settings = PathBuf::from(&isolated).join(".claude").join("settings.json");
        let with_permission = fs::read_to_string(&settings).unwrap();
        println!("--- settings.json（接入后）---\n{with_permission}--- end ---");
        assert_eq!(with_permission.matches(CLAUDE_ZHI_PERMISSION).count(), 1, "多次接入只留一条");
        assert_eq!(status.permission_state.as_deref(), Some("installed"));
        let result = disconnect_client("claude-code");
        println!("[4] disconnect ok={} {}", result.ok, result.message);
        assert!(result.ok);
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), user);
        assert!(!fs::read_to_string(&settings).unwrap().contains(CLAUDE_ZHI_PERMISSION));

        // 5. 手写过同名章节：不重复写入，断开时也不动
        let manual = "## iterate 交互协议（必须遵守）\n1. 每次回复都调用 call_zhi\n";
        fs::write(&claude_md, manual).unwrap();
        println!("[5] {}", connected_message(ClientKind::ClaudeCode));
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), manual);
        assert!(disconnect_client("claude-code").ok);
        assert_eq!(fs::read_to_string(&claude_md).unwrap(), manual);

        // 6. Codex 走同一套函数，写 AGENTS.md
        fs::create_dir_all(agents_md.parent().unwrap()).unwrap();
        fs::write(&agents_md, "# codex 偏好\n").unwrap();
        println!("[6] {}", connected_message(ClientKind::Codex));
        println!("[6] {}", connected_message(ClientKind::Codex));
        let codex = fs::read_to_string(&agents_md).unwrap();
        assert_eq!(codex.matches(CODEX_RULES_BEGIN).count(), 1);
        assert!(codex.contains(IZHI_RULES_BODY));
        assert!(disconnect_client("codex").ok);
        assert_eq!(fs::read_to_string(&agents_md).unwrap(), "# codex 偏好\n");
        println!("[ok] 隔离 HOME 端到端通过：{isolated}");
    }
}
