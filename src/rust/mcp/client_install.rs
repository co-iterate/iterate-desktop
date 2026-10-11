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
}

impl ClientKind {
    pub const ALL: [ClientKind; 3] = [ClientKind::ClaudeCode, ClientKind::Codex, ClientKind::Cursor];

    pub fn id(self) -> &'static str {
        match self {
            ClientKind::ClaudeCode => "claude-code",
            ClientKind::Codex => "codex",
            ClientKind::Cursor => "cursor",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ClientKind::ClaudeCode => "Claude Code",
            ClientKind::Codex => "Codex",
            ClientKind::Cursor => "Cursor",
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
}

#[derive(Debug, Clone, Serialize)]
pub struct AppLocationStatus {
    pub ok: bool,
    pub bundled_server: Option<String>,
    pub stable_server: String,
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
    })
}

pub fn stable_server_path() -> PathBuf {
    iterate_bridge_state_dir().join("bin").join("mcp-server")
}

fn bundled_server_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    Some(exe.with_file_name("mcp-server"))
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
    let Some(bundled) = bundled_server_path() else {
        return AppLocationStatus {
            ok: false,
            bundled_server: None,
            stable_server,
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

fn backup_config(kind: ClientKind, path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let dir = iterate_bridge_state_dir().join("mcp-client-backups");
    fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败: {e}"))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().trim_start_matches('.').to_string())
        .unwrap_or_else(|| "config".to_string());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let backup = dir.join(format!("{}-{}-{}", kind.id(), stamp, file_name));
    fs::copy(path, &backup).map_err(|e| format!("备份 {} 失败: {e}", path.display()))?;
    prune_backups(&dir, kind);
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
    }
}

fn configured_command(kind: ClientKind, text: &str) -> Option<String> {
    match kind {
        ClientKind::Codex => codex_configured_command(text),
        ClientKind::ClaudeCode | ClientKind::Cursor => json_configured_command(text),
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
        ClientKind::ClaudeCode | ClientKind::Cursor => {
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
        ClientKind::Cursor => Some(merge_json_config(&original, command, false)?),
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

    ids.iter()
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
                    message: kind.restart_hint().to_string(),
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
        .collect()
}

fn remove_entry(kind: ClientKind, text: &str) -> Result<Option<String>, String> {
    match kind {
        ClientKind::Codex => remove_codex_server(text),
        ClientKind::ClaudeCode | ClientKind::Cursor => remove_json_server(text),
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
        set_opted_out(kind, true)?;
        Ok(backup)
    });
    match result {
        Ok(backup) => ClientActionResult {
            id: id.to_string(),
            name: kind.display_name().to_string(),
            ok: true,
            message: "已移除，重启客户端后生效".to_string(),
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
}
