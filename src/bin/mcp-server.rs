//! 独立的 iterate MCP 服务器
//!
//! 通过 HTTP 调用 iterate 的对话 API，让其他 AI 工具可以使用 iterate 的 zhi 功能

use anyhow::{Context, Result};
use chrono::Local;
use cunzhi::mcp::codex_deeplink::{
    codex_thread_deeplink, extract_codex_thread_id_from_metas, extract_codex_thread_id_from_value,
    normalize_codex_thread_deeplink, normalize_codex_thread_id,
};
use cunzhi::mcp::codex_home::codex_home_from_process_or_parent_env;
use cunzhi::mcp::tools::checkpoint;
use cunzhi::mcp::tools::interaction::{append_conversation_log, ConversationEntry};
use cunzhi::mcp::utils::generate_request_id;
use cunzhi::mcp::ResponseMetadata;
use cunzhi::utils::append_timeline_debug_log;
use rmcp::{
    model::*, service::RequestContext, transport::stdio, RoleServer, ServerHandler, ServiceExt,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::env;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// MCP server 进程启动后是否已拉取过 .cunzhi-knowledge（0=未拉，1=已拉，u64::MAX=进行中）
static LAST_KNOWLEDGE_PULL: AtomicU64 = AtomicU64::new(0);

const HTML_ARTIFACT_CAPABILITY: &str = "html_artifact";

fn instance_debug_log(tag: &str, message: impl AsRef<str>) {
    let line = format!(
        "{} [mcp-server:{}] {} {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        std::process::id(),
        tag,
        message.as_ref()
    );
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/iterate-instance-debug.log")
    {
        let _ = file.write_all(line.as_bytes());
    }
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// iterate HTTP API 的对话请求
#[derive(Debug, Serialize, Deserialize)]
struct DialogRequest {
    #[serde(default)]
    request_id: String,
    message: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    workspace: String,
    #[serde(default = "default_true")]
    is_markdown: bool,
    #[serde(default)]
    codex_home: Option<String>,
    #[serde(default)]
    codex_thread_id: Option<String>,
    #[serde(default)]
    codex_deeplink: Option<String>,
    #[serde(default)]
    conversation_title: Option<String>,
    #[serde(default)]
    checkpoint_id: Option<String>,
    #[serde(default)]
    checkpoint_commit: Option<String>,
    #[serde(default)]
    checkpoint_message: Option<String>,
}

fn default_true() -> bool {
    true
}

fn codex_home_from_env() -> Option<String> {
    codex_home_from_process_or_parent_env()
}

fn iterate_home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }

    dirs::home_dir()
}

fn default_codex_home() -> Option<PathBuf> {
    codex_home_from_env()
        .map(PathBuf::from)
        .or_else(|| iterate_home_dir().map(|home| home.join(".codex")))
}

fn normalize_conversation_title(title: Option<&str>) -> Option<String> {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(ToOwned::to_owned)
}

fn codex_thread_title_from_session_index(codex_home: &Path, thread_id: &str) -> Option<String> {
    let file = std::fs::File::open(codex_home.join("session_index.jsonl")).ok()?;
    let mut matched_title = None;

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if entry.get("id").and_then(|value| value.as_str()) != Some(thread_id) {
            continue;
        }

        if let Some(title) =
            normalize_conversation_title(entry.get("thread_name").and_then(|value| value.as_str()))
        {
            matched_title = Some(title);
        }
    }

    matched_title
}

fn codex_thread_title(thread_id: &str) -> Option<String> {
    default_codex_home().and_then(|home| codex_thread_title_from_session_index(&home, thread_id))
}

fn codex_state_db_candidates(codex_home: &Path) -> Vec<PathBuf> {
    vec![
        codex_home.join("sqlite").join("state_5.sqlite"),
        codex_home.join("state_5.sqlite"),
    ]
}

fn normalize_workspace_path_for_lookup(project_path: &str) -> String {
    let trimmed = project_path.trim();
    let path = PathBuf::from(trimmed);
    path.canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[derive(Debug, Clone)]
struct CodexThreadCandidate {
    thread_id: String,
    updated_at_ms: i64,
}

fn latest_codex_thread_candidate_from_state_db(
    state_db_path: &Path,
    project_path: &str,
) -> Option<CodexThreadCandidate> {
    if !state_db_path.exists() {
        return None;
    }

    let normalized_project_path = normalize_workspace_path_for_lookup(project_path);
    let conn = Connection::open_with_flags(state_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let (thread_id, updated_at_ms) = conn
        .query_row(
            "SELECT id, COALESCE(updated_at_ms, updated_at * 1000, 0) AS updated_at_ms
             FROM threads
             WHERE archived = 0
               AND COALESCE(thread_source, '') != 'subagent'
               AND (cwd = ?1 OR cwd = ?2)
             ORDER BY COALESCE(updated_at_ms, updated_at * 1000, 0) DESC, id DESC
             LIMIT 1",
            params![project_path, normalized_project_path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .ok()
        .flatten()?;

    normalize_codex_thread_id(&thread_id).map(|thread_id| CodexThreadCandidate {
        thread_id,
        updated_at_ms,
    })
}

fn latest_codex_thread_fallback_for_project(project_path: &str) -> Option<CodexThreadFallback> {
    let codex_home = default_codex_home()?;
    let mut best: Option<(CodexThreadFallback, i64)> = None;

    for state_db_path in codex_state_db_candidates(&codex_home) {
        if let Some(candidate) =
            latest_codex_thread_candidate_from_state_db(&state_db_path, project_path)
        {
            let should_replace = best.as_ref().is_none_or(|(fallback, updated_at_ms)| {
                candidate.updated_at_ms > *updated_at_ms
                    || (candidate.updated_at_ms == *updated_at_ms
                        && candidate.thread_id > fallback.thread_id)
            });

            if should_replace {
                best = Some((
                    CodexThreadFallback {
                        thread_id: candidate.thread_id,
                        state_db_path,
                    },
                    candidate.updated_at_ms,
                ));
            }
        }
    }

    best.map(|(fallback, _)| fallback)
}

/// iterate HTTP API 的对话响应
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DialogResponse {
    keep_going: bool,
    #[serde(default)]
    user_input: String,
    #[serde(default)]
    response_source: String,
    #[serde(default)]
    selected_options: Vec<String>,
    #[serde(default)]
    file_paths: Vec<String>,
    #[serde(default)]
    image_paths: Vec<String>,
    #[serde(default)]
    metadata: ResponseMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn is_goal_response_source(source: &str) -> bool {
    let normalized = source.trim().to_ascii_lowercase();
    normalized.contains("goal_submit")
        || normalized == "goal"
        || normalized == "goal_start"
        || normalized.ends_with("_goal")
}

fn format_path_list(label: &str, paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }

    Some(format!(
        "{}：\n{}",
        label,
        paths
            .iter()
            .filter(|path| !path.trim().is_empty())
            .map(|path| format!("- {}", path.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn paths_missing_from_input(user_input: &str, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty() && !user_input.contains(*path))
        .map(ToString::to_string)
        .collect()
}

fn collapse_extra_blank_lines(text: &str) -> String {
    let mut lines = Vec::new();
    let mut blank_count = 0;

    for line in text.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                lines.push(String::new());
            }
        } else {
            blank_count = 0;
            lines.push(line.to_string());
        }
    }

    lines.join("\n").trim().to_string()
}

fn normalize_goal_closing_spacing(text: &str) -> String {
    let mut normalized = text.to_string();
    while normalized.contains("\n\n》") {
        normalized = normalized.replace("\n\n》", "》");
    }
    while normalized.contains("\n》") {
        normalized = normalized.replace("\n》", "》");
    }
    normalized
}

fn strip_goal_image_reference_context(user_input: &str) -> String {
    let lines: Vec<&str> = user_input.lines().collect();
    let mut kept = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        let starts_legacy_image_block = trimmed.starts_with("附加图片：")
            && (trimmed.contains("images 附件")
                || lines
                    .get(index + 1)
                    .map(|next| next.trim() == "附件地址：")
                    .unwrap_or(false));

        if starts_legacy_image_block {
            let mut preserve_goal_close = trimmed.ends_with('》');
            index += 1;

            if lines
                .get(index)
                .map(|next| next.trim() == "附件地址：")
                .unwrap_or(false)
            {
                index += 1;
            }

            while index < lines.len() {
                let nested = lines[index].trim();
                if nested.starts_with("- images[")
                    || nested == "（见 images 附件）"
                    || nested == "（见 images 附件）》"
                {
                    preserve_goal_close = preserve_goal_close || nested.ends_with('》');
                    index += 1;
                    continue;
                }
                break;
            }

            if preserve_goal_close {
                kept.push("》");
            }
            continue;
        }

        kept.push(line);
        index += 1;
    }

    normalize_goal_closing_spacing(&collapse_extra_blank_lines(&kept.join("\n")))
}

fn format_missing_selected_options(
    user_input: &str,
    selected_options: &[String],
) -> Option<String> {
    let missing_options = selected_options
        .iter()
        .map(|option| option.trim())
        .filter(|option| !option.is_empty() && !user_input.contains(option))
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if missing_options.is_empty() {
        return None;
    }

    Some(format!(
        "选中的选项：\n{}",
        missing_options
            .iter()
            .map(|option| format!("- {}", option))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn inject_goal_response_context(
    user_input: &str,
    selected_options: &[String],
    file_paths: &[String],
    image_paths: &[String],
) -> String {
    let cleaned_user_input = strip_goal_image_reference_context(user_input);
    let mut blocks = Vec::new();

    if let Some(options_block) =
        format_missing_selected_options(&cleaned_user_input, selected_options)
    {
        blocks.push(options_block);
    }

    let missing_file_paths = paths_missing_from_input(&cleaned_user_input, file_paths);
    if let Some(file_block) = format_path_list("附加文件路径", &missing_file_paths) {
        blocks.push(file_block);
    }

    let missing_image_paths = paths_missing_from_input(&cleaned_user_input, image_paths);
    if let Some(image_block) = format_path_list("附加图片路径", &missing_image_paths) {
        blocks.push(image_block);
    }

    if blocks.is_empty() {
        return cleaned_user_input;
    }

    let context_text = blocks.join("\n\n");
    if let Some(close_index) = cleaned_user_input.rfind('》') {
        let (before, after) = cleaned_user_input.split_at(close_index);
        format!("{}\n\n{}{}", before.trim_end(), context_text, after)
    } else {
        format!("{}\n\n{}", cleaned_user_input.trim_end(), context_text)
    }
}

fn prepend_selected_options_to_user_input(user_input: &str, selected_options: &[String]) -> String {
    let missing_options = selected_options
        .iter()
        .map(|option| option.trim())
        .filter(|option| !option.is_empty() && !user_input.contains(option))
        .collect::<Vec<_>>();

    if missing_options.is_empty() {
        return user_input.to_string();
    }

    let prefix = format!("选中的选项: {}", missing_options.join(" / "));
    if user_input.trim().is_empty() {
        prefix
    } else {
        format!("{}\n\n{}", prefix, user_input)
    }
}

fn enrich_goal_response_with_attachment_paths(response: &mut DialogResponse) {
    if response.user_input.trim().is_empty()
        || !is_goal_response_source(&response.response_source)
        || (response.selected_options.is_empty()
            && response.file_paths.is_empty()
            && response.image_paths.is_empty())
    {
        return;
    }

    response.user_input = inject_goal_response_context(
        &response.user_input,
        &response.selected_options,
        &response.file_paths,
        &response.image_paths,
    );
}

/// sync 工具的参数（同步 .cunzhi-knowledge）
#[derive(Debug, Deserialize)]
struct SyncArgs {
    /// 项目路径（可选，用于定位 .cunzhi-knowledge）
    #[serde(default)]
    project_path: Option<String>,
    /// 操作方向：pull / push / both（默认 both）
    #[serde(default)]
    direction: Option<String>,
}

/// checkpoint 工具的参数（创建 git 检查点）
#[derive(Debug, Deserialize)]
struct CheckpointArgs {
    /// 项目路径（必填，要创建检查点的项目根目录）
    project_path: String,
    /// 提交信息（可选，默认自动生成）
    #[serde(default)]
    message: Option<String>,
}

/// web_fetch 工具的参数
#[derive(Debug, Deserialize)]
struct WebFetchArgs {
    url: String,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default = "default_max_chars")]
    max_chars: usize,
}

fn default_timeout() -> u64 {
    15
}
fn default_max_chars() -> usize {
    50000
}

/// cron_manage 工具的参数
#[derive(Debug, Deserialize)]
struct CronManageArgs {
    action: String, // "list" | "add" | "remove"
    #[serde(default)]
    schedule: Option<String>, // cron 表达式，如 "0 6 * * *"
    #[serde(default)]
    command: Option<String>, // 要执行的命令
    #[serde(default)]
    label: Option<String>, // 任务标签（用于标识和删除）
}

/// call_zhi 工具的参数
#[derive(Debug, Deserialize)]
struct CallZhiArgs {
    /// AI 消息内容
    message: String,
    /// 项目路径（必填）
    project_path: String,
    /// 预定义选项（可选）
    #[serde(default)]
    predefined_options: Vec<String>,
    /// 是否使用 Markdown 格式（默认 true）
    #[serde(default = "default_true")]
    is_markdown: bool,
    /// 调用本次 MCP 的 Codex 会话 ID（可选；通常自动从 MCP metadata 捕获）
    #[serde(default)]
    codex_thread_id: Option<String>,
    /// 调用本次 MCP 的 Codex 会话 deep link（可选；通常自动生成）
    #[serde(default)]
    codex_deeplink: Option<String>,
    /// 调用本次 MCP 的对话标题（可选；未提供时按 Codex 会话 ID 自动提取）
    #[serde(default)]
    conversation_title: Option<String>,
}

#[derive(Debug, Clone)]
struct CodexThreadFallback {
    thread_id: String,
    state_db_path: PathBuf,
}

/// 扫描已注册的端口，返回所有可能的端口
async fn scan_registered_ports() -> Vec<u16> {
    let port_dir = match iterate_home_dir() {
        Some(home) => home.join(".cunzhi_ports"),
        None => return vec![5311],
    };

    if !port_dir.exists() {
        return vec![5311];
    }

    let mut ports = vec![];
    if let Ok(entries) = std::fs::read_dir(&port_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(port) = name.parse::<u16>() {
                    ports.push(port);
                }
            }
        }
    }

    // 按端口号排序
    ports.sort();

    // 如果没有找到任何端口，返回默认端口
    if ports.is_empty() {
        vec![5311]
    } else {
        ports
    }
}

/// 检测端口是否有服务运行
async fn check_port_running(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{}/health", port);
    let client = build_local_probe_client();

    match client.get(&url).send().await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

async fn probe_port_health_detail(port: u16) -> (bool, String) {
    let url = format!("http://127.0.0.1:{}/health", port);
    let client = build_local_probe_client();

    match client.get(&url).send().await {
        Ok(response) => {
            let status = response.status();
            (status.is_success(), format!("http_status={}", status))
        }
        Err(error) => (false, format!("error={}", error)),
    }
}

/// 检测端口是否空闲（通过 /status 接口检测）
async fn check_port_idle(port: u16) -> bool {
    probe_port_status(port)
        .await
        .map(|status| !status.is_busy && port_status_supports_required_capabilities(&status))
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
struct PortStatusProbe {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    runtime: Option<RuntimeStatusProbe>,
    #[serde(default)]
    is_busy: bool,
    #[serde(default)]
    busy_since: Option<String>,
    #[serde(default)]
    active_request_id: Option<String>,
    #[serde(default)]
    active_workspace: Option<String>,
    #[serde(default)]
    interaction_phase: Option<String>,
    #[serde(default)]
    phase_since: Option<String>,
    #[serde(default)]
    active_serve_request_id: Option<String>,
    #[serde(default)]
    ready_since: Option<String>,
    #[serde(default)]
    capabilities: serde_json::Value,
    #[serde(default)]
    frontend_capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RuntimeStatusProbe {
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    exe_path: Option<String>,
    #[serde(default)]
    exe_mtime: Option<String>,
    #[serde(default)]
    exe_sha256: Option<String>,
}

fn port_status_has_capability(status: &PortStatusProbe, capability: &str) -> bool {
    if status
        .capabilities
        .get(capability)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return true;
    }

    status
        .frontend_capabilities
        .iter()
        .any(|value| value == capability)
}

fn port_status_supports_required_capabilities(status: &PortStatusProbe) -> bool {
    port_status_has_capability(status, HTML_ARTIFACT_CAPABILITY)
}

const DEFAULT_STALE_INTERACTION_TRANSIENT_SECS: i64 = 30;

async fn probe_port_status(port: u16) -> Option<PortStatusProbe> {
    if !check_port_running(port).await {
        return None;
    }

    let url = format!("http://127.0.0.1:{}/status", port);
    let client = build_local_probe_client();
    let response = client.get(&url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    response.json::<PortStatusProbe>().await.ok()
}

fn registered_workspace_for_port(port: u16) -> Option<String> {
    let path = iterate_home_dir()?
        .join(".cunzhi_ports")
        .join(port.to_string());
    let workspace = std::fs::read_to_string(path).ok()?;
    let workspace = workspace.trim().to_string();
    if workspace.is_empty() {
        None
    } else {
        Some(workspace)
    }
}

fn short_hash(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };
    const PREFIX_LEN: usize = 19;
    if value.len() <= PREFIX_LEN {
        value.to_string()
    } else {
        format!("{}...", &value[..PREFIX_LEN])
    }
}

fn runtime_diagnostics_from_status(
    port: u16,
    request_workspace: &str,
    registered_workspace: Option<&str>,
    status: Option<&PortStatusProbe>,
) -> String {
    let version = status
        .and_then(|status| status.version.as_deref())
        .unwrap_or("-");
    let runtime = status.and_then(|status| status.runtime.as_ref());
    let pid = runtime
        .and_then(|runtime| runtime.pid)
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "-".to_string());
    let exe_path = runtime
        .and_then(|runtime| runtime.exe_path.as_deref())
        .unwrap_or("-");
    let exe_mtime = runtime
        .and_then(|runtime| runtime.exe_mtime.as_deref())
        .unwrap_or("-");
    let exe_sha256 = short_hash(runtime.and_then(|runtime| runtime.exe_sha256.as_deref()));
    let registered_workspace = registered_workspace.unwrap_or("-");

    format!(
        "port={} request_workspace={} registered_workspace={} version={} pid={} exe_path={} exe_mtime={} exe_sha256={}",
        port,
        request_workspace,
        registered_workspace,
        version,
        pid,
        exe_path,
        exe_mtime,
        exe_sha256
    )
}

async fn selected_runtime_diagnostics(port: u16, request_workspace: &str) -> String {
    let registered_workspace = registered_workspace_for_port(port);
    let status = probe_port_status(port).await;
    runtime_diagnostics_from_status(
        port,
        request_workspace,
        registered_workspace.as_deref(),
        status.as_ref(),
    )
}

fn is_stale_busy_status(status: &PortStatusProbe) -> bool {
    if !status.is_busy {
        return false;
    }

    let active_request_id = status
        .active_request_id
        .as_deref()
        .map(str::trim)
        .unwrap_or("");
    if active_request_id.is_empty() {
        return true;
    }

    let interaction_phase = status
        .interaction_phase
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match interaction_phase {
        Some("idle") => true,
        Some("failed") => true,
        Some("waiting_user") => false,
        Some("queued" | "starting_gui" | "responded" | "cleaning") => {
            transient_phase_age_secs(status)
                .map(|age_secs| age_secs >= stale_interaction_transient_secs())
                .unwrap_or(false)
        }
        Some(_) => false,
        None => false,
    }
}

fn stale_interaction_transient_secs() -> i64 {
    std::env::var("ITERATE_STALE_INTERACTION_TRANSIENT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_STALE_INTERACTION_TRANSIENT_SECS)
}

fn busy_since_age_secs(status: &PortStatusProbe) -> Option<i64> {
    rfc3339_age_secs(status.busy_since.as_deref())
}

fn phase_since_age_secs(status: &PortStatusProbe) -> Option<i64> {
    rfc3339_age_secs(status.phase_since.as_deref())
}

fn transient_phase_age_secs(status: &PortStatusProbe) -> Option<i64> {
    phase_since_age_secs(status).or_else(|| busy_since_age_secs(status))
}

fn rfc3339_age_secs(timestamp: Option<&str>) -> Option<i64> {
    let timestamp = timestamp?.trim();
    if timestamp.is_empty() {
        return None;
    }

    let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()?
        .with_timezone(&chrono::Utc);
    Some((chrono::Utc::now() - timestamp).num_seconds())
}

async fn prune_dead_registered_ports() {
    let Some(home) = iterate_home_dir() else {
        return;
    };
    let port_dir = home.join(".cunzhi_ports");
    let Ok(entries) = std::fs::read_dir(&port_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(port) = name.parse::<u16>() else {
            continue;
        };

        match probe_port_status(port).await {
            None => {
                let _ = std::fs::remove_file(path);
            }
            Some(status) if is_stale_busy_status(&status) => {
                eprintln!(
                    "[route-debug] port={} stale_busy phase={:?} phase_since={:?} busy_since={:?} active_request_id={:?} active_serve_request_id={:?} active_workspace={:?} ready_since={:?} -> prune registration",
                    port,
                    status.interaction_phase,
                    status.phase_since,
                    status.busy_since,
                    status.active_request_id,
                    status.active_serve_request_id,
                    status.active_workspace,
                    status.ready_since
                );
                instance_debug_log(
                    "[route-stale-pruned]",
                    format!(
                        "port={}, phase={:?}, phase_since={:?}, busy_since={:?}, active_request_id={:?}, active_serve_request_id={:?}, active_workspace={:?}, ready_since={:?}",
                        port,
                        status.interaction_phase,
                        status.phase_since,
                        status.busy_since,
                        status.active_request_id,
                        status.active_serve_request_id,
                        status.active_workspace,
                        status.ready_since
                    ),
                );
                let _ = std::fs::remove_file(path);
            }
            Some(_) => {}
        }
    }
}

fn build_local_probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        // health/status only probe localhost and should never inherit system proxies
        .no_proxy()
        .timeout(std::time::Duration::from_secs(1))
        .build()
        .unwrap_or_default()
}

fn serve_startup_timeout() -> std::time::Duration {
    std::env::var("ITERATE_SERVE_STARTUP_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|| std::time::Duration::from_secs(10))
}

async fn wait_child_exit_with_timeout(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Ok(None) => return None,
            Err(_) => return None,
        }
    }
}

/// 找到一个未被使用的端口
async fn find_free_port() -> u16 {
    // 从 5311 开始扫描，找到第一个没有服务运行的端口
    for port in 5311..5400 {
        if !check_port_running(port).await {
            // 端口没有服务运行，可以使用
            return port;
        }
    }
    5311
}

/// 启动新的 iterate 服务（带 workspace）
async fn start_iterate_service_with_workspace(port: u16, workspace: &str) -> bool {
    // 构建参数（确保生命周期足够长）
    let port_str = port.to_string();
    let mut args = vec!["--serve", "--port", port_str.as_str()];

    let workspace_flag = "--workspace".to_string();
    if !workspace.is_empty() {
        args.push(workspace_flag.as_str());
        args.push(workspace);
    }

    instance_debug_log(
        "[spawn-serve-begin]",
        format!("port={}, workspace={:?}, args={:?}", port, workspace, args),
    );

    let launchers = iterate_launch_candidates();
    instance_debug_log(
        "[spawn-serve-candidates]",
        format!(
            "port={}, workspace={:?}, launchers=[{}]",
            port,
            workspace,
            display_paths(&launchers)
        ),
    );
    let mut child = None;
    let mut selected_launcher = None;

    for launcher in launchers {
        let result = Command::new(&launcher)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        match result {
            Ok(spawned) => {
                instance_debug_log(
                    "[spawn-serve-success]",
                    format!(
                        "launcher={}, child_pid={}, port={}, workspace={:?}",
                        launcher.display(),
                        spawned.id(),
                        port,
                        workspace
                    ),
                );
                selected_launcher = Some(launcher.display().to_string());
                child = Some(spawned);
                break;
            }
            Err(error) => {
                instance_debug_log(
                    "[spawn-serve-fallback]",
                    format!(
                        "launcher={}, error={}, port={}, workspace={:?}",
                        launcher.display(),
                        error,
                        port,
                        workspace
                    ),
                );
            }
        }
    }

    let mut child = if let Some(child) = child {
        child
    } else {
        instance_debug_log(
            "[spawn-serve-failed]",
            format!(
                "all launchers failed, port={}, workspace={:?}",
                port, workspace
            ),
        );
        return false;
    };

    // 等待服务启动
    let deadline = std::time::Instant::now() + serve_startup_timeout();
    let wait_started = std::time::Instant::now();
    let mut attempts = 0u32;
    let mut child_exit_logged = false;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        attempts += 1;
        let (healthy, health_detail) = probe_port_health_detail(port).await;
        if healthy {
            instance_debug_log(
                "[spawn-serve-ready]",
                format!(
                    "port={} is listening, workspace={:?}, attempts={}, elapsed_ms={}, launcher={:?}, {}",
                    port,
                    workspace,
                    attempts,
                    wait_started.elapsed().as_millis(),
                    selected_launcher,
                    health_detail
                ),
            );
            return true;
        }

        if !child_exit_logged {
            match child.try_wait() {
                Ok(Some(status)) => {
                    child_exit_logged = true;
                    instance_debug_log(
                        "[spawn-serve-child-exited-before-ready]",
                        format!(
                            "port={}, workspace={:?}, child_pid={}, status={}, attempts={}, elapsed_ms={}, launcher={:?}, {}",
                            port,
                            workspace,
                            child.id(),
                            status,
                            attempts,
                            wait_started.elapsed().as_millis(),
                            selected_launcher,
                            health_detail
                        ),
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    child_exit_logged = true;
                    instance_debug_log(
                        "[spawn-serve-child-status-error]",
                        format!(
                            "port={}, workspace={:?}, child_pid={}, error={}, attempts={}, elapsed_ms={}, launcher={:?}",
                            port,
                            workspace,
                            child.id(),
                            error,
                            attempts,
                            wait_started.elapsed().as_millis(),
                            selected_launcher
                        ),
                    );
                }
            }
        }

        if attempts == 1 || attempts % 10 == 0 {
            instance_debug_log(
                "[spawn-serve-wait]",
                format!(
                    "port={}, workspace={:?}, attempts={}, elapsed_ms={}, child_pid={}, launcher={:?}, {}",
                    port,
                    workspace,
                    attempts,
                    wait_started.elapsed().as_millis(),
                    child.id(),
                    selected_launcher,
                    health_detail
                ),
            );
        }
    }

    instance_debug_log(
        "[spawn-serve-timeout]",
        format!(
            "port={} did not become ready, workspace={:?}, attempts={}, elapsed_ms={}, child_pid={}, launcher={:?}",
            port,
            workspace,
            attempts,
            wait_started.elapsed().as_millis(),
            child.id(),
            selected_launcher
        ),
    );

    let child_pid = child.id();
    let kill_result = child.kill();
    let wait_status =
        wait_child_exit_with_timeout(&mut child, std::time::Duration::from_secs(2)).await;
    instance_debug_log(
        "[spawn-serve-timeout-cleanup]",
        format!(
            "port={}, workspace={:?}, child_pid={}, kill_ok={}, wait_status={:?}",
            port,
            workspace,
            child_pid,
            kill_result.is_ok(),
            wait_status
        ),
    );
    false
}

fn iterate_launch_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(path) = std::env::var("ITERATE_UI_COMMAND") {
        push_executable_candidate(&mut candidates, PathBuf::from(path));
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            push_executable_candidate(&mut candidates, exe_dir.join("iterate"));
        }
    }

    push_executable_candidate(
        &mut candidates,
        PathBuf::from("/Applications/iterate.app/Contents/MacOS/iterate"),
    );
    candidates.push(PathBuf::from("iterate"));

    candidates
}

fn push_executable_candidate(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if is_executable(&path) && !candidates.iter().any(|candidate| candidate == &path) {
        candidates.push(path);
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("exe"))
            .unwrap_or(false)
    }
}

/// 启动新的 iterate 服务（不带 workspace，向后兼容）
async fn start_iterate_service(port: u16) -> bool {
    start_iterate_service_with_workspace(port, "").await
}

fn normalize_workspace_path(workspace: &str) -> Option<PathBuf> {
    cunzhi::utils::workspace::normalize_workspace_path(workspace)
}

fn workspace_depth(path: &Path) -> usize {
    cunzhi::utils::workspace::workspace_depth(path)
}

fn port_registration_matches_workspace(port: u16, workspace: &str) -> bool {
    let Some(home) = iterate_home_dir() else {
        return true;
    };
    let path = home.join(".cunzhi_ports").join(port.to_string());
    let Ok(registered_workspace) = std::fs::read_to_string(path) else {
        return true;
    };
    let registered_workspace = registered_workspace.trim();
    if registered_workspace.is_empty() {
        return true;
    }

    match (
        normalize_workspace_path(workspace),
        normalize_workspace_path(registered_workspace),
    ) {
        (Some(workspace_path), Some(registered_path)) => {
            workspace_path.starts_with(registered_path)
        }
        _ => registered_workspace == workspace,
    }
}

struct PortAllocationLock {
    lock_path: PathBuf,
    _file: std::fs::File,
}

impl Drop for PortAllocationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

async fn acquire_port_allocation_lock() -> Option<PortAllocationLock> {
    let lock_path = iterate_home_dir()?
        .join(".cunzhi_ports")
        .join(".alloc.lock");
    if let Some(parent) = lock_path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return None;
        }
    }

    for _ in 0..80 {
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(file) => {
                return Some(PortAllocationLock {
                    lock_path,
                    _file: file,
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // 避免异常退出留下陈旧锁导致永久阻塞
                if let Ok(meta) = std::fs::metadata(&lock_path) {
                    if let Ok(modified) = meta.modified() {
                        if modified
                            .elapsed()
                            .map(|elapsed| elapsed > std::time::Duration::from_secs(30))
                            .unwrap_or(false)
                        {
                            let _ = std::fs::remove_file(&lock_path);
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(_) => return None,
        }
    }

    None
}

/// 根据 workspace 查找或创建端口（workspace-based routing）
async fn find_or_create_port_for_workspace(workspace: &str, preferred_port: u16) -> u16 {
    // 全流程加锁，避免并发请求同时选中同一端口
    let _lock = match acquire_port_allocation_lock().await {
        Some(lock) => {
            instance_debug_log(
                "[route-lock-acquired]",
                format!("workspace={}, preferred_port={}", workspace, preferred_port),
            );
            Some(lock)
        }
        None => {
            eprintln!("[mcp-server] WARNING: 无法获取端口分配锁，继续执行但可能存在并发竞态");
            instance_debug_log(
                "[route-lock-missing]",
                format!("workspace={}, preferred_port={}", workspace, preferred_port),
            );
            None
        }
    };

    prune_dead_registered_ports().await;

    // 1. 优先查找该 workspace 已注册的端口
    let existing_workspace_ports = find_ports_for_workspace(workspace).await;
    let existing_workspace_port = existing_workspace_ports.first().copied();
    eprintln!(
        "[route-debug] workspace={} preferred_port={} existing_workspace_ports={:?}",
        workspace, preferred_port, existing_workspace_ports
    );
    instance_debug_log(
        "[route-begin]",
        format!(
            "workspace={}, preferred_port={}, existing_workspace_ports={:?}",
            workspace, preferred_port, existing_workspace_ports
        ),
    );
    for port in &existing_workspace_ports {
        if check_port_idle(*port).await {
            eprintln!(
                "[route-debug] workspace={} selected_port={} reason=existing_workspace_idle",
                workspace, port
            );
            instance_debug_log(
                "[route-selected]",
                format!(
                    "workspace={}, selected_port={}, reason=existing_workspace_idle",
                    workspace, port
                ),
            );
            return *port;
        }
    }

    if !existing_workspace_ports.is_empty() {
        eprintln!(
            "[route-debug] workspace={} existing_workspace_ports={:?} all busy, will start a new port",
            workspace, existing_workspace_ports
        );
        instance_debug_log(
            "[route-busy-new-port]",
            format!(
                "workspace={}, preferred_port={}, existing_workspace_ports={:?}",
                workspace, preferred_port, existing_workspace_ports
            ),
        );
    }

    // 2. 没有可复用空闲端口时，继续在本 workspace 的端口池后方开新端口，
    // 避免把当前请求塞回 busy 队列，也避免跨 workspace 复用其他实例。
    let candidates = build_workspace_port_candidates(&existing_workspace_ports, preferred_port);

    for candidate in candidates {
        if check_port_running(candidate).await {
            continue;
        }

        // 用 TCP connect 探测端口是否被非 iterate 服务占用（无 bind-drop TOCTOU 竞态）
        if std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], candidate)),
            std::time::Duration::from_millis(80),
        )
        .is_ok()
        {
            continue;
        }

        if start_iterate_service_with_workspace(candidate, workspace).await {
            eprintln!(
                "[route-debug] workspace={} selected_port={} reason=candidate_started",
                workspace, candidate
            );
            instance_debug_log(
                "[route-selected]",
                format!(
                    "workspace={}, selected_port={}, reason=candidate_started",
                    workspace, candidate
                ),
            );
            return candidate;
        }
    }

    // 5. 启动失败时，如果当前 workspace 已有运行实例，则优先回退到已注册端口；
    // 否则再回退到 CLI 传入端口，避免误导到硬编码 5311。
    let fallback_port = existing_workspace_port.unwrap_or(preferred_port);
    eprintln!(
        "[route-debug] workspace={} selected_port={} reason=fallback",
        workspace, fallback_port
    );
    instance_debug_log(
        "[route-selected]",
        format!(
            "workspace={}, selected_port={}, reason=fallback, existing_workspace_port={:?}, preferred_port={}",
            workspace, fallback_port, existing_workspace_port, preferred_port
        ),
    );
    fallback_port
}

fn build_workspace_port_candidates(
    existing_workspace_ports: &[u16],
    preferred_port: u16,
) -> Vec<u16> {
    let start_port = existing_workspace_ports
        .iter()
        .copied()
        .max()
        .map(|port| port.saturating_add(1))
        .unwrap_or(preferred_port);

    let mut candidates = Vec::new();
    if start_port < 5400 {
        candidates.extend(start_port..5400);
    }
    candidates.extend(5311..start_port);
    candidates
}

/// 根据 workspace 查找对应端口
async fn find_port_for_workspace(workspace: &str) -> Option<u16> {
    find_ports_for_workspace(workspace).await.into_iter().next()
}

/// 根据 workspace 查找对应端口列表，优先返回更具体的路径匹配
async fn find_ports_for_workspace(workspace: &str) -> Vec<u16> {
    let Some(workspace_path) = normalize_workspace_path(workspace) else {
        return Vec::new();
    };
    let ports = scan_registered_ports_with_workspace().await;
    let mut matches: Vec<(usize, u16)> = Vec::new();

    for (port, ws) in ports {
        let Some(candidate_path) = normalize_workspace_path(&ws) else {
            eprintln!(
                "[route-debug] workspace={} scanned_port={} raw_workspace={:?} normalized=false",
                workspace, port, ws
            );
            continue;
        };

        eprintln!(
            "[route-debug] workspace={} scanned_port={} candidate_workspace={} workspace_match={} port_running_precheck=pending",
            workspace,
            port,
            candidate_path.display(),
            workspace_path.starts_with(&candidate_path)
        );

        if !workspace_path.starts_with(&candidate_path) {
            continue;
        }

        if !check_port_running(port).await {
            eprintln!(
                "[route-debug] workspace={} scanned_port={} candidate_workspace={} workspace_match=true port_running=false",
                workspace,
                port,
                candidate_path.display()
            );
            continue;
        }

        let depth = workspace_depth(&candidate_path);
        matches.push((depth, port));
    }

    matches.sort_by(|(depth_a, port_a), (depth_b, port_b)| {
        depth_b.cmp(depth_a).then_with(|| port_a.cmp(port_b))
    });

    matches.into_iter().map(|(_, port)| port).collect()
}

/// 扫描已注册的端口及其 workspace 映射
async fn scan_registered_ports_with_workspace() -> Vec<(u16, String)> {
    let mut ports = Vec::new();
    if let Some(home) = iterate_home_dir() {
        let dir = home.join(".cunzhi_ports");
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Ok(port) = name.parse::<u16>() {
                        let workspace = std::fs::read_to_string(entry.path())
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        if !workspace.is_empty() {
                            ports.push((port, workspace));
                        }
                    }
                }
            }
        }
    }
    ports.sort_by_key(|(p, _)| *p);
    ports
}

/// 智能检测可用端口，如果没有可用端口则自动启动新服务
async fn find_available_port(preferred_port: u16) -> u16 {
    prune_dead_registered_ports().await;

    // 先检查首选端口是否空闲（服务运行且没有正在进行的对话）
    if check_port_idle(preferred_port).await {
        return preferred_port;
    }

    // 扫描已注册的端口，找到空闲的
    let ports = scan_registered_ports().await;
    for port in ports {
        if check_port_idle(port).await {
            return port;
        }
    }

    // 没有空闲端口，自动启动新服务
    let new_port = find_free_port().await;
    if start_iterate_service(new_port).await {
        for _ in 0..20 {
            if check_port_idle(new_port).await {
                return new_port;
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // 新启动的服务仍然是比旧注册端口更合理的兜底。
        return new_port;
    }

    // 启动失败，返回首选端口（让后续错误处理提示用户）
    preferred_port
}

/// 查找 .cunzhi-knowledge 目录
fn find_knowledge_dir(workspace: &str) -> Option<PathBuf> {
    // 优先级：workspace/.cunzhi-knowledge → cwd/.cunzhi-knowledge → home/.cunzhi-knowledge
    let candidates = [
        if workspace.is_empty() {
            None
        } else {
            Some(PathBuf::from(workspace).join(".cunzhi-knowledge"))
        },
        std::env::current_dir()
            .ok()
            .map(|p| p.join(".cunzhi-knowledge")),
        iterate_home_dir().map(|p| p.join(".cunzhi-knowledge")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// 截断用户输入中的 Auto Prompt 注入内容
/// 策略：1) 精确 sentinel 优先 2) 行首 marker 匹配 3) 取最早位置
fn strip_auto_prompt(input: &str) -> Cow<'_, str> {
    // 精确 sentinel（任意位置匹配）
    const EXACT_SENTINELS: &[&str] = &[
        "<!-- CONTEXT_INJECTION_START -->",
        "<!-- AUTO_PROMPT_START -->",
    ];
    // 行首 marker（只在行首匹配，避免误伤正常文本）
    const LINE_MARKERS: &[&str] = &[
        "✔️不明白的地方反问我",
        "✔️继续调用 zhi",
        "✔️请记住",
        "✔继续调用 zhi",
        "快捷触发词",
    ];

    // 先找精确 sentinel 的最早位置
    let mut cut = EXACT_SENTINELS.iter().filter_map(|m| input.find(m)).min();

    // 再找行首 marker 的最早位置
    let mut offset = 0;
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if LINE_MARKERS.iter().any(|m| trimmed.starts_with(m)) {
            cut = Some(cut.map_or(offset, |v| v.min(offset)));
            break;
        }
        offset += line.len();
    }

    match cut {
        Some(pos) => Cow::Owned(input[..pos].trim_end().to_string()),
        None => Cow::Borrowed(input),
    }
}

/// 记录对话到 conversations/YYYY-MM-DD.md
fn record_conversation(
    workspace: &str,
    ai_message: &str,
    dialog_response: &DialogResponse,
    request_id: Option<String>,
    timeline_route_id: Option<String>,
    workspace_checkpoint: Option<checkpoint::CheckpointMetadata>,
) {
    let project_path = (!workspace.trim().is_empty()).then(|| workspace.to_string());
    eprintln!(
        "[CHE-DEBUG][record_conversation] workspace={:?} request_id={:?} checkpoint_present={} checkpoint_commit={:?} checkpoint_subject={:?} user_input_len={} selected_options={} image_count={}",
        project_path,
        request_id,
        workspace_checkpoint.is_some(),
        workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_hash.as_str()),
        workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_subject.as_str()),
        dialog_response.user_input.len(),
        dialog_response.selected_options.len(),
        dialog_response.image_paths.len()
    );

    let entry = ConversationEntry {
        conversation_id: dialog_response
            .metadata
            .conversation_id
            .clone()
            .or_else(|| dialog_response.metadata.tree_id.clone()),
        current_node_id: dialog_response
            .metadata
            .current_node_id
            .clone()
            .or_else(|| dialog_response.metadata.node_id.clone()),
        timeline_route_id: dialog_response
            .metadata
            .timeline_route_id
            .clone()
            .or_else(|| dialog_response.metadata.conversation_route_id.clone())
            .or(timeline_route_id),
        run_id: dialog_response.metadata.run_id.clone(),
        generation: dialog_response.metadata.generation,
        stale_of: dialog_response.metadata.stale_of.clone(),
        superseded_by: dialog_response.metadata.superseded_by.clone(),
        ai_message: ai_message.to_string(),
        user_response: strip_auto_prompt(&dialog_response.user_input).to_string(),
        project_path,
        image_count: dialog_response.image_paths.len(),
        file_paths: dialog_response.file_paths.clone(),
        image_paths: dialog_response.image_paths.clone(),
        selected_options: dialog_response.selected_options.clone(),
        request_id,
        checkpoint_id: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.clone()),
        checkpoint_commit: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_hash.clone()),
        push_status: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.push_status.clone()),
        response_source: (!dialog_response.response_source.trim().is_empty())
            .then(|| dialog_response.response_source.clone())
            .or_else(|| dialog_response.metadata.source.clone()),
        workspace_checkpoint_message: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_subject.clone()),
    };

    append_conversation_log(&entry);
}

async fn call_zhi(
    args: CallZhiArgs,
    caller_codex_thread_id: Option<String>,
    argument_codex_thread_id: Option<String>,
) -> Result<CallToolResult, ErrorData> {
    #[cfg(target_os = "windows")]
    cunzhi::app::windows_lifecycle::activate_mcp_launch().map_err(|error| {
        ErrorData::internal_error(format!("重新启动 iterate 失败: {error}"), None)
    })?;

    // 进程启动后第一次 call_zhi 时拉取 .cunzhi-knowledge（cold start，只拉一次）
    // 0=未拉，u64::MAX=进行中，1=已拉
    if LAST_KNOWLEDGE_PULL
        .compare_exchange(0, u64::MAX, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let workspace = args.project_path.as_str();
        let knowledge_dir = find_knowledge_dir(workspace);
        if let Some(kdir) = knowledge_dir {
            let result = Command::new("git")
                .args(["pull", "--rebase", "--autostash", "--quiet"])
                .current_dir(&kdir)
                .output();
            // 写入同步日志（成功/失败都记录，方便排查）
            let log_msg = match &result {
                Ok(o) if o.status.success() => {
                    format!("[knowledge-sync] OK: pulled {}\n", kdir.display())
                }
                Ok(o) => format!(
                    "[knowledge-sync] FAIL: git pull exited {}: {}\n",
                    o.status,
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => format!("[knowledge-sync] ERROR: {}\n", e),
            };
            if let Ok(mut f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/cunzhi_knowledge_sync.log")
            {
                let _ = f.write_all(log_msg.as_bytes());
            }
        }
        LAST_KNOWLEDGE_PULL.store(1, Ordering::SeqCst);
    }

    let explicit_codex_thread_id = args
        .codex_thread_id
        .as_deref()
        .and_then(normalize_codex_thread_id);
    let caller_codex_thread_id =
        caller_codex_thread_id.and_then(|value| normalize_codex_thread_id(&value));
    let argument_codex_thread_id =
        argument_codex_thread_id.and_then(|value| normalize_codex_thread_id(&value));
    let latest_state_codex_thread = if explicit_codex_thread_id.is_none()
        && caller_codex_thread_id.is_none()
        && argument_codex_thread_id.is_none()
    {
        latest_codex_thread_fallback_for_project(&args.project_path)
    } else {
        None
    };
    let live_goal_codex_thread_id = if explicit_codex_thread_id.is_none()
        && caller_codex_thread_id.is_none()
        && argument_codex_thread_id.is_none()
        && latest_state_codex_thread.is_none()
    {
        cunzhi::ui::live_goal::live_goal_codex_thread_id_for_project(Some(&args.project_path))
            .and_then(|value| normalize_codex_thread_id(&value))
    } else {
        None
    };
    let codex_thread_id = explicit_codex_thread_id
        .clone()
        .or_else(|| caller_codex_thread_id.clone())
        .or_else(|| argument_codex_thread_id.clone())
        .or_else(|| {
            latest_state_codex_thread
                .as_ref()
                .map(|fallback| fallback.thread_id.clone())
        })
        .or_else(|| live_goal_codex_thread_id.clone());
    let request_id = generate_request_id();
    append_timeline_debug_log(
        "rust/bin_mcp_server::call_zhi_route_context",
        serde_json::json!({
            "request_id": request_id.as_str(),
            "project_path": args.project_path.as_str(),
            "explicit_codex_thread_id": explicit_codex_thread_id.as_deref(),
            "caller_codex_thread_id": caller_codex_thread_id.as_deref(),
            "argument_codex_thread_id": argument_codex_thread_id.as_deref(),
            "latest_state_codex_thread_id": latest_state_codex_thread.as_ref().map(|fallback| fallback.thread_id.as_str()),
            "latest_state_db_path": latest_state_codex_thread.as_ref().map(|fallback| fallback.state_db_path.display().to_string()),
            "live_goal_codex_thread_id": live_goal_codex_thread_id.as_deref(),
            "resolved_codex_thread_id": codex_thread_id.as_deref(),
        }),
    );
    let codex_deeplink = args
        .codex_deeplink
        .as_deref()
        .and_then(normalize_codex_thread_deeplink)
        .or_else(|| codex_thread_id.as_deref().and_then(codex_thread_deeplink));
    let conversation_title = normalize_conversation_title(args.conversation_title.as_deref())
        .or_else(|| codex_thread_id.as_deref().and_then(codex_thread_title));
    checkpoint::touch_auto_checkpoint_monitor(&args.project_path, Some(&request_id));
    let workspace_checkpoint =
        checkpoint::maybe_auto_checkpoint(&args.project_path, Some(&request_id));
    eprintln!(
        "[CHE-DEBUG][call_zhi] request_id={} project_path={} checkpoint_present={} checkpoint_id={:?} checkpoint_commit={:?} checkpoint_subject={:?} push_status={:?}",
        request_id,
        args.project_path,
        workspace_checkpoint.is_some(),
        workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.as_str()),
        workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_hash.as_str()),
        workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_subject.as_str()),
        workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.push_status.as_str()),
    );

    // 根据 workspace 查找或分配端口（workspace-based routing）
    let preferred_port = env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<u16>().ok())
        .unwrap_or(5311);
    let port = find_or_create_port_for_workspace(&args.project_path, preferred_port).await;
    eprintln!(
        "[MCP-Loop-Debug] call_zhi: project_path={:?} preferred_port={} routed_port={}",
        args.project_path, preferred_port, port
    );

    let workspace = args.project_path.clone();
    let request = DialogRequest {
        request_id: request_id.clone(),
        message: args.message.clone(),
        options: args.predefined_options,
        workspace: workspace.clone(),
        is_markdown: args.is_markdown,
        codex_home: codex_home_from_env(),
        codex_thread_id,
        codex_deeplink,
        conversation_title,
        checkpoint_id: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.clone()),
        checkpoint_commit: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_hash.clone()),
        checkpoint_message: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_subject.clone()),
    };

    let mut dialog_response = send_dialog_request(port, &request)?;
    let explicit_end = cunzhi::conversation::is_explicit_conversation_end_response(
        &dialog_response.user_input,
        &dialog_response.selected_options,
    );
    let popup_closed =
        cunzhi::conversation::is_popup_closed_response_source(&dialog_response.response_source);
    if explicit_end || popup_closed {
        let end_source = if explicit_end {
            cunzhi::conversation::EXPLICIT_CONVERSATION_END_SOURCE
        } else {
            cunzhi::conversation::POPUP_CLOSED_SOURCE
        };
        dialog_response.keep_going = false;
        dialog_response.response_source = end_source.to_string();
        dialog_response.selected_options.clear();
        dialog_response.file_paths.clear();
        dialog_response.image_paths.clear();
        dialog_response.metadata.source = Some(end_source.to_string());
    } else {
        enrich_goal_response_with_attachment_paths(&mut dialog_response);
    }
    eprintln!(
        "[MCP-Loop-Debug] dialog_response: response_source={:?} keep_going={}",
        dialog_response.response_source, dialog_response.keep_going
    );

    // 检查是否有错误
    if let Some(error) = dialog_response.error {
        return Err(ErrorData::internal_error(
            format!("iterate 错误: {}", error),
            None,
        ));
    }

    // 构造返回内容
    let mut content_parts = vec![];

    let display_user_input = if is_goal_response_source(&dialog_response.response_source) {
        dialog_response.user_input.clone()
    } else {
        prepend_selected_options_to_user_input(
            &dialog_response.user_input,
            &dialog_response.selected_options,
        )
    };

    // 添加用户输入
    if !display_user_input.is_empty() {
        content_parts.push(format!("用户输入: {}", display_user_input));
    }

    // 添加文件路径
    if !dialog_response.file_paths.is_empty() {
        content_parts.push(format!(
            "附加文件: {}",
            dialog_response.file_paths.join(", ")
        ));
    }

    // 添加图片路径
    if !dialog_response.image_paths.is_empty() {
        content_parts.push(format!(
            "附加图片: {}",
            dialog_response.image_paths.join(", ")
        ));
    }

    // 添加继续状态
    content_parts.push(format!("继续对话: {}", dialog_response.keep_going));
    if !dialog_response.response_source.is_empty() {
        content_parts.push(format!("响应来源: {}", dialog_response.response_source));
    }
    if std::env::var("ITERATE_SHOW_RUNTIME_DIAGNOSTICS")
        .ok()
        .as_deref()
        == Some("1")
    {
        let runtime_diagnostics = selected_runtime_diagnostics(port, &workspace).await;
        content_parts.push(format!("运行时诊断: {}", runtime_diagnostics));
    }

    let content = if content_parts.is_empty() {
        "用户取消了操作".to_string()
    } else {
        content_parts.join("\n")
    };

    // [Hook] 记录对话到 conversations/
    eprintln!(
        "[CHE-DEBUG][call_zhi] about_to_record_conversation request_id={} response_source={:?} keep_going={} content_len={}",
        request_id,
        dialog_response.response_source,
        dialog_response.keep_going,
        content.len()
    );
    record_conversation(
        &workspace,
        &args.message,
        &dialog_response,
        Some(request_id),
        request.codex_thread_id.clone(),
        workspace_checkpoint,
    );

    Ok(CallToolResult::success(vec![Content::text(content)]))
}

fn send_dialog_request(port: u16, request: &DialogRequest) -> Result<DialogResponse, ErrorData> {
    let body = serde_json::to_string(request)
        .map_err(|e| ErrorData::internal_error(format!("序列化请求失败: {}", e), None))?;

    let addr = format!("127.0.0.1:{}", port);
    instance_debug_log(
        "[dialog-http-begin]",
        format!(
            "request_id={}, port={}, workspace={:?}, body_len={}, options_len={}, codex_home_present={}",
            request.request_id,
            port,
            request.workspace,
            body.len(),
            request.options.len(),
            request.codex_home.is_some()
        ),
    );
    let mut stream = TcpStream::connect(&addr).map_err(|e| {
        instance_debug_log(
            "[dialog-http-connect-error]",
            format!(
                "request_id={}, port={}, addr={}, error={}",
                request.request_id, port, addr, e
            ),
        );
        ErrorData::internal_error(
            format!(
                "无法连接到 iterate 服务器 (端口 {}): {}. 请确保 iterate 应用已启动",
                port, e
            ),
            None,
        )
    })?;

    let request_text = format!(
        "POST /api/dialog HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    stream.write_all(request_text.as_bytes()).map_err(|e| {
        instance_debug_log(
            "[dialog-http-write-error]",
            format!(
                "request_id={}, port={}, error={}",
                request.request_id, port, e
            ),
        );
        ErrorData::internal_error(format!("发送请求失败: {}", e), None)
    })?;

    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|e| {
        instance_debug_log(
            "[dialog-http-read-error]",
            format!(
                "request_id={}, port={}, error={}",
                request.request_id, port, e
            ),
        );
        ErrorData::internal_error(format!("读取响应失败: {}", e), None)
    })?;

    let (header, raw_body) = response.split_once("\r\n\r\n").ok_or_else(|| {
        instance_debug_log(
            "[dialog-http-parse-error]",
            format!(
                "request_id={}, port={}, response_len={}, response_prefix={:?}",
                request.request_id,
                port,
                response.len(),
                truncate_for_error(&response, 200)
            ),
        );
        ErrorData::internal_error("解析 HTTP 响应失败".to_string(), None)
    })?;

    if let Some(status_line) = header.lines().next() {
        instance_debug_log(
            "[dialog-http-status]",
            format!(
                "request_id={}, port={}, status_line={:?}, header_len={}, raw_body_len={}",
                request.request_id,
                port,
                status_line,
                header.len(),
                raw_body.len()
            ),
        );
        if !status_line.contains(" 200 ") {
            return Err(ErrorData::internal_error(
                format!("iterate 服务器返回错误: {}", status_line),
                None,
            ));
        }
    }

    // 处理 chunked transfer encoding：axum 快速返回时可能使用 chunked
    let body = if header.to_lowercase().contains("transfer-encoding: chunked") {
        decode_chunked_body(raw_body)
    } else {
        raw_body.to_string()
    };

    let parsed = serde_json::from_str(&body).map_err(|e| {
        instance_debug_log(
            "[dialog-http-json-error]",
            format!(
                "request_id={}, port={}, error={}, body_len={}, body_prefix={:?}",
                request.request_id,
                port,
                e,
                body.len(),
                truncate_for_error(&body, 200)
            ),
        );
        ErrorData::internal_error(
            format!(
                "解析响应失败: {} body={:?}",
                e,
                truncate_for_error(&body, 200)
            ),
            None,
        )
    })?;
    instance_debug_log(
        "[dialog-http-success]",
        format!(
            "request_id={}, port={}, body_len={}",
            request.request_id,
            port,
            body.len()
        ),
    );
    Ok(parsed)
}

/// 解码 HTTP chunked transfer encoding body
fn decode_chunked_body(raw: &str) -> String {
    let mut result = String::new();
    let mut remaining = raw;
    loop {
        // 跳过前导空白
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }
        // 读取 chunk size（十六进制）
        let size_end = remaining.find("\r\n").unwrap_or(remaining.len());
        let size_str = &remaining[..size_end];
        let chunk_size = usize::from_str_radix(size_str.trim(), 16).unwrap_or(0);
        if chunk_size == 0 {
            break;
        }
        remaining = &remaining[size_end + 2..]; // skip \r\n after size
        if remaining.len() >= chunk_size {
            result.push_str(&remaining[..chunk_size]);
            remaining = &remaining[chunk_size..];
            // skip trailing \r\n after chunk data
            if remaining.starts_with("\r\n") {
                remaining = &remaining[2..];
            }
        } else {
            // incomplete chunk, take what we have
            result.push_str(remaining);
            break;
        }
    }
    result
}

fn truncate_for_error(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut truncated = String::new();

    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return value.to_string();
        };
        truncated.push(ch);
    }

    if chars.next().is_some() {
        truncated.push_str("...");
    }

    truncated
}

/// 查找 .cunzhi-knowledge 目录（优先 project_path，其次 cwd，最后 home）
fn find_knowledge_dir_for(project_path: Option<&str>) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = [
        project_path.map(|p| PathBuf::from(p).join(".cunzhi-knowledge")),
        std::env::current_dir()
            .ok()
            .map(|p| p.join(".cunzhi-knowledge")),
        iterate_home_dir().map(|p| p.join(".cunzhi-knowledge")),
    ]
    .into_iter()
    .flatten()
    .collect();

    candidates.into_iter().find(|p| p.exists())
}

/// 在 Markdown 内容中按 ## 段落搜索关键词
/// 支持多词 OR 搜索：查询词按空格拆分，任一词命中即返回该段落
fn search_in_sections(content: &str, query: &str) -> Vec<String> {
    let mut matches = Vec::new();
    // 拆分查询词（支持多词 OR 搜索）
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return matches;
    }
    let sections: Vec<&str> = content.split("\n## ").collect();
    for (i, section) in sections.iter().enumerate() {
        let lower = section.to_lowercase();
        // 任一词命中即匹配
        if terms.iter().any(|t| lower.contains(t.as_str())) {
            let lines: Vec<&str> = section.lines().collect();
            if !lines.is_empty() {
                let title = if i == 0 {
                    lines[0].trim_start_matches("# ").to_string()
                } else {
                    format!("## {}", lines[0])
                };
                let summary: Vec<&str> = lines.iter().take(15).copied().collect();
                let truncated = if lines.len() > 15 { "\n..." } else { "" };
                matches.push(format!("{}\n{}{}", title, summary.join("\n"), truncated));
            }
        }
    }
    matches.truncate(8);
    matches
}

/// web_fetch 工具：抓取网页内容
async fn web_fetch(args: WebFetchArgs) -> Result<CallToolResult, ErrorData> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(args.timeout_secs))
        .user_agent("Mozilla/5.0 (compatible; CunZhi/1.0; +https://cunzhi.ai)")
        .build()
        .map_err(|e| ErrorData::internal_error(format!("创建 HTTP 客户端失败: {}", e), None))?;

    let response = client
        .get(&args.url)
        .send()
        .await
        .map_err(|e| ErrorData::internal_error(format!("HTTP 请求失败: {}", e), None))?;

    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    if !status.is_success() {
        return Ok(CallToolResult::error(vec![Content::text(format!(
            "HTTP 请求失败: {} {}\nURL: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown"),
            args.url
        ))]));
    }

    let body = response
        .text()
        .await
        .map_err(|e| ErrorData::internal_error(format!("读取响应体失败: {}", e), None))?;

    // 简单的 HTML→纯文本提取
    let text = if content_type.contains("html") {
        strip_html_tags(&body)
    } else {
        body
    };

    // 截断过长内容
    let truncated = if text.len() > args.max_chars {
        format!(
            "{}\n\n--- 内容已截断（共 {} 字符，显示前 {} 字符）---",
            &text[..args.max_chars],
            text.len(),
            args.max_chars
        )
    } else {
        text
    };

    let result_json = serde_json::json!({
        "url": args.url,
        "status": status.as_u16(),
        "content_type": content_type,
        "content_length": truncated.len(),
        "content": truncated
    });

    Ok(CallToolResult::success(vec![Content::text(
        result_json.to_string(),
    )]))
}

/// 简单的 HTML 标签剥离
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_buf = String::new();

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
            continue;
        }
        if ch == '>' && in_tag {
            in_tag = false;
            let tag_lower = tag_buf.to_lowercase();
            if tag_lower.starts_with("script") {
                in_script = true;
            } else if tag_lower.starts_with("/script") {
                in_script = false;
            } else if tag_lower.starts_with("style") {
                in_style = true;
            } else if tag_lower.starts_with("/style") {
                in_style = false;
            }
            if tag_lower.starts_with("br")
                || tag_lower.starts_with("p")
                || tag_lower.starts_with("/p")
                || tag_lower.starts_with("/div")
                || tag_lower.starts_with("/h")
                || tag_lower.starts_with("/li")
                || tag_lower.starts_with("/tr")
            {
                result.push('\n');
            }
            continue;
        }
        if in_tag {
            tag_buf.push(ch);
            continue;
        }
        if in_script || in_style {
            continue;
        }
        result.push(ch);
    }

    // 压缩连续空白行
    let mut cleaned = String::with_capacity(result.len());
    let mut prev_blank = false;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank {
                cleaned.push('\n');
                prev_blank = true;
            }
        } else {
            cleaned.push_str(trimmed);
            cleaned.push('\n');
            prev_blank = false;
        }
    }

    cleaned.trim().to_string()
}

/// cron_manage 工具：管理定时任务
fn cron_manage(args: CronManageArgs) -> Result<CallToolResult, ErrorData> {
    match args.action.as_str() {
        "list" => cron_list_jobs(),
        "add" => cron_add_job(args),
        "remove" => cron_remove_job(args),
        _ => Ok(CallToolResult::error(vec![Content::text(format!(
            "未知操作: {}。支持的操作: list, add, remove",
            args.action
        ))])),
    }
}

fn cron_list_jobs() -> Result<CallToolResult, ErrorData> {
    let output = Command::new("crontab")
        .arg("-l")
        .output()
        .map_err(|e| ErrorData::internal_error(format!("执行 crontab -l 失败: {}", e), None))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() && stderr.contains("no crontab") {
        return Ok(CallToolResult::success(vec![Content::text(
            serde_json::json!({ "jobs": [], "message": "当前没有定时任务" }).to_string(),
        )]));
    }

    let jobs: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .enumerate()
        .map(|(i, line)| {
            let label = if line.contains("# ") {
                line.rsplit("# ").next().unwrap_or("").trim().to_string()
            } else {
                String::new()
            };
            serde_json::json!({
                "index": i,
                "entry": line.trim(),
                "label": label
            })
        })
        .collect();

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::json!({ "jobs": jobs, "count": jobs.len() }).to_string(),
    )]))
}

fn cron_add_job(args: CronManageArgs) -> Result<CallToolResult, ErrorData> {
    let schedule = args
        .schedule
        .ok_or_else(|| ErrorData::invalid_params("缺少 schedule 参数".to_string(), None))?;
    let command = args
        .command
        .ok_or_else(|| ErrorData::invalid_params("缺少 command 参数".to_string(), None))?;
    let label = args.label.unwrap_or_else(|| "cunzhi".to_string());

    let new_entry = format!("{} {} # {}", schedule, command, label);

    // 读取现有 crontab
    let existing = Command::new("crontab").arg("-l").output().ok();
    let mut entries: Vec<String> = existing
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    // 如果已有相同标签，先移除
    entries.retain(|l| !l.contains(&format!("# {}", label)));
    entries.push(new_entry.clone());

    // 写回 crontab
    let joined = entries.join("\n") + "\n";
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ErrorData::internal_error(format!("启动 crontab 失败: {}", e), None))?;

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(joined.as_bytes())
        .map_err(|e| ErrorData::internal_error(format!("写入 crontab 失败: {}", e), None))?;

    child
        .wait()
        .map_err(|e| ErrorData::internal_error(format!("等待 crontab 失败: {}", e), None))?;

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::json!({
            "success": true,
            "entry": new_entry,
            "message": format!("已添加定时任务: {}", label)
        })
        .to_string(),
    )]))
}

fn cron_remove_job(args: CronManageArgs) -> Result<CallToolResult, ErrorData> {
    let label = args
        .label
        .ok_or_else(|| ErrorData::invalid_params("缺少 label 参数".to_string(), None))?;

    let existing = Command::new("crontab").arg("-l").output().ok();
    let entries_before: Vec<String> = existing
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    let count_before = entries_before.len();
    let entries_after: Vec<String> = entries_before
        .into_iter()
        .filter(|l| !l.contains(&format!("# {}", label)))
        .collect();
    let removed = count_before - entries_after.len();

    if removed == 0 {
        return Ok(CallToolResult::error(vec![Content::text(
            serde_json::json!({
                "success": false,
                "message": format!("未找到标签为 '{}' 的定时任务", label)
            })
            .to_string(),
        )]));
    }

    let joined = if entries_after.is_empty() {
        String::new()
    } else {
        entries_after.join("\n") + "\n"
    };

    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ErrorData::internal_error(format!("启动 crontab 失败: {}", e), None))?;

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(joined.as_bytes())
        .map_err(|e| ErrorData::internal_error(format!("写入 crontab 失败: {}", e), None))?;

    child
        .wait()
        .map_err(|e| ErrorData::internal_error(format!("等待 crontab 失败: {}", e), None))?;

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::json!({
            "success": true,
            "removed": removed,
            "message": format!("已移除 {} 条标签为 '{}' 的定时任务", removed, label)
        })
        .to_string(),
    )]))
}

/// sync 工具：同步 .cunzhi-knowledge 知识库
/// 按 sync-knowledge/SKILL.md 规则：
/// 1. 检查本地变更（git status）
/// 2. 有变更则先 commit
/// 3. fetch + pull --no-rebase
/// 4. push（如有本地提交）
fn sync_knowledge(args: SyncArgs) -> Result<CallToolResult, ErrorData> {
    let knowledge_dir = match find_knowledge_dir_for(args.project_path.as_deref()) {
        Some(d) => d,
        None => {
            return Ok(CallToolResult::success(vec![Content::text(
                "⚠️ 未找到 .cunzhi-knowledge 目录，无法同步",
            )]))
        }
    };

    let direction = args.direction.as_deref().unwrap_or("pull");
    let mut results = Vec::new();

    // 步骤 1：检查本地变更
    let status_out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&knowledge_dir)
        .output()
        .map_err(|e| ErrorData::internal_error(format!("git status 失败: {}", e), None))?;

    let has_local_changes = !String::from_utf8_lossy(&status_out.stdout)
        .trim()
        .is_empty();

    // 步骤 2：有本地变更则先 commit（仅在 push 或 both 时，pull 时不偶尔 commit）
    if has_local_changes && (direction == "push" || direction == "both") {
        let _ = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&knowledge_dir)
            .output();

        let now = Local::now();
        let commit_msg = format!("sync: {}", now.format("%Y-%m-%d %H:%M"));
        let commit_out = Command::new("git")
            .args(["commit", "-m", &commit_msg, "--quiet", "--no-verify"])
            .current_dir(&knowledge_dir)
            .output();

        if commit_out.map(|o| o.status.success()).unwrap_or(false) {
            results.push("✅ 本地变更已提交".to_string());
        }
    } else if !has_local_changes {
        results.push("✅ 本地无变更".to_string());
    }

    // 步骤 3：fetch + pull --no-rebase
    if direction == "pull" || direction == "both" {
        let fetch_out = Command::new("git")
            .args(["fetch", "--quiet"])
            .current_dir(&knowledge_dir)
            .output();

        if fetch_out.map(|o| o.status.success()).unwrap_or(false) {
            results.push("✅ Fetched from origin".to_string());
        }

        let pull_out = Command::new("git")
            .args(["pull", "--no-rebase", "--quiet"])
            .current_dir(&knowledge_dir)
            .output()
            .map_err(|e| ErrorData::internal_error(format!("git pull 失败: {}", e), None))?;

        if pull_out.status.success() {
            let stdout = String::from_utf8_lossy(&pull_out.stdout);
            if stdout.contains("Already up to date") || stdout.trim().is_empty() {
                results.push("✅ Already up to date".to_string());
            } else {
                results.push("✅ 已拉取最新更新".to_string());
            }
        } else {
            let stderr = String::from_utf8_lossy(&pull_out.stderr);
            // 检查是否有 merge 冲突
            if stderr.contains("CONFLICT") || stderr.contains("conflict") {
                results.push(format!("⚠️ Merge 冲突，需要手动解决: {}", stderr.trim()));
            } else {
                results.push(format!("⚠️ git pull 失败: {}", stderr.trim()));
            }
        }
    }

    // 步骤 4：push（如有本地提交）
    if direction == "push" || direction == "both" {
        let push_out = Command::new("git")
            .args(["push", "--quiet"])
            .current_dir(&knowledge_dir)
            .output()
            .map_err(|e| ErrorData::internal_error(format!("git push 失败: {}", e), None))?;

        if push_out.status.success() {
            results.push("🚀 已推送到 GitHub".to_string());
        } else {
            let stderr = String::from_utf8_lossy(&push_out.stderr);
            results.push(format!("⚠️ git push 失败: {}", stderr.trim()));
        }
    }

    Ok(CallToolResult::success(vec![Content::text(format!(
        "## 🔄 知识库同步\n\n目录: `{}`\n\n{}",
        knowledge_dir.display(),
        results.join("\n")
    ))]))
}

/// checkpoint 工具：在项目中创建 git 检查点
fn create_checkpoint(args: CheckpointArgs) -> Result<CallToolResult, ErrorData> {
    let project_dir = PathBuf::from(&args.project_path);
    if !project_dir.exists() {
        return Err(ErrorData::invalid_params(
            format!("项目路径不存在: {}", args.project_path),
            None,
        ));
    }

    let now = Local::now();
    let commit_msg = args
        .message
        .unwrap_or_else(|| format!("checkpoint: {}", now.format("%Y-%m-%d %H:%M:%S")));

    match checkpoint::git_ops::create_checkpoint(&args.project_path, &commit_msg) {
        Ok(checkpoint) => {
            let short_hash = checkpoint.id.chars().take(9).collect::<String>();
            Ok(CallToolResult::success(vec![Content::text(format!(
                "✅ 检查点已创建\n\n- 提交: `{}`\n- 信息: {}\n- 路径: {}",
                short_hash, checkpoint.message, args.project_path
            ))]))
        }
        Err(err) if err == "没有需要保存的更改" => {
            Ok(CallToolResult::success(vec![Content::text(
                "ℹ️ 没有未提交的改动，无需创建检查点",
            )]))
        }
        Err(err) => Err(ErrorData::internal_error(
            format!("创建 checkpoint 失败: {}", err),
            None,
        )),
    }
}

/// MCP 服务器处理器
#[derive(Clone)]
struct IterateZhiServer {
    default_port: u16,
}

impl ServerHandler for IterateZhiServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_server_info(Implementation::new("iterate-zhi", "1.0.0"))
            .with_instructions(format!(
                "iterate MCP 服务器 - 通过 HTTP 调用 iterate 的 zhi 功能。默认端口: {}",
                self.default_port
            ))
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ServerInfo, ErrorData> {
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let zhi_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "要显示给用户的 AI 消息内容"
                },
                "project_path": {
                    "type": "string",
                    "description": "项目路径（必填），用于显示项目名和创建 git 检查点"
                },
                "predefined_options": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    },
                    "description": "预定义的选项列表（可选），用户可以快速选择"
                },
                "is_markdown": {
                    "type": "boolean",
                    "description": "消息是否使用 Markdown 格式（默认 true）"
                },
                "conversation_title": {
                    "type": "string",
                    "description": "当前对话标题（可选；Codex 会话未传时会自动提取）"
                }
            },
            "required": ["message", "project_path"]
        });

        let sync_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "project_path": {
                    "type": "string",
                    "description": "项目路径（可选，用于定位 .cunzhi-knowledge）"
                },
                "direction": {
                    "type": "string",
                    "enum": ["pull", "push", "both"],
                    "description": "同步方向：pull（默认，拉取最新）/ push（推送本地改动）/ both（先拉后推）"
                }
            }
        });

        let checkpoint_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "project_path": {
                    "type": "string",
                    "description": "项目根目录（必填）"
                },
                "message": {
                    "type": "string",
                    "description": "提交信息（可选，默认自动生成）"
                }
            },
            "required": ["project_path"]
        });

        let mut tools = vec![];
        if let serde_json::Value::Object(schema_map) = zhi_schema {
            tools.push(Tool::new(
                Cow::Borrowed("call_zhi"),
                Cow::Borrowed("调用 iterate 的 zhi 功能，弹出 GUI 让用户输入"),
                Arc::new(schema_map),
            ));
        }
        if let serde_json::Value::Object(schema_map) = sync_schema {
            tools.push(Tool::new(
                Cow::Borrowed("sync"),
                Cow::Borrowed(
                    "同步 .cunzhi-knowledge 知识库（git pull/push）。触发词: sync。默认先 pull 再 push。",
                ),
                Arc::new(schema_map),
            ));
        }
        if let serde_json::Value::Object(schema_map) = checkpoint_schema {
            tools.push(Tool::new(
                Cow::Borrowed("checkpoint"),
                Cow::Borrowed("在项目中创建 Git 型检查点。在重要操作前调用，确保代码可回滚。"),
                Arc::new(schema_map),
            ));
        }

        // web_fetch 工具
        let web_fetch_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "要抓取的网页 URL"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "超时时间（秒），默认 15"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "最大返回字符数，默认 50000"
                }
            },
            "required": ["url"]
        });
        if let serde_json::Value::Object(schema_map) = web_fetch_schema {
            tools.push(Tool::new(
                Cow::Borrowed("web_fetch"),
                Cow::Borrowed(
                    "抓取网页内容。支持 HTML→纯文本提取、超时控制和内容截断。用于获取网页信息。",
                ),
                Arc::new(schema_map),
            ));
        }

        // cron_manage 工具
        let cron_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "add", "remove"],
                    "description": "操作类型：list=列出所有定时任务，add=添加定时任务，remove=移除定时任务"
                },
                "schedule": {
                    "type": "string",
                    "description": "cron 表达式（add 时必填），如 '0 6 * * *' 表示每天早上6点"
                },
                "command": {
                    "type": "string",
                    "description": "要执行的命令（add 时必填）"
                },
                "label": {
                    "type": "string",
                    "description": "任务标签（用于标识和删除），默认 'cunzhi'"
                }
            },
            "required": ["action"]
        });
        if let serde_json::Value::Object(schema_map) = cron_schema {
            tools.push(Tool::new(
                Cow::Borrowed("cron_manage"),
                Cow::Borrowed(
                    "管理系统定时任务（crontab）。支持列出、添加、移除定时任务。可用于设置闹钟、定时脚本、定期清理等。",
                ),
                Arc::new(schema_map),
            ));
        }

        // task 工具 - 文件持久化任务系统
        let task_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "操作类型: list(列出任务), add(添加), update(更新), done(完成), remove(删除)"
                },
                "project_path": {
                    "type": "string",
                    "description": "项目路径（必需）"
                },
                "task_id": {
                    "type": "string",
                    "description": "任务ID（update/done/remove时必需）"
                },
                "subject": {
                    "type": "string",
                    "description": "任务主题（add时必需）"
                },
                "status": {
                    "type": "string",
                    "description": "状态: pending/in_progress/done/blocked"
                },
                "priority": {
                    "type": "string",
                    "description": "优先级: high/medium/low"
                },
                "blocked_by": {
                    "type": "string",
                    "description": "阻塞原因（可选）"
                }
            },
            "required": ["action", "project_path"]
        });
        if let serde_json::Value::Object(schema_map) = task_schema {
            tools.push(Tool::new(
                Cow::Borrowed("task"),
                Cow::Borrowed(
                    "文件持久化任务系统。任务存储在 .cunzhi-memory/tasks.json，跨会话持久。支持 list/add/update/done/remove。",
                ),
                Arc::new(schema_map),
            ));
        }

        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match request.name.as_ref() {
            "call_zhi" => {
                let caller_codex_thread_id =
                    extract_codex_thread_id_from_metas(std::iter::once(&context.meta));
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let argument_codex_thread_id = extract_codex_thread_id_from_value(&arguments_value);

                let args: CallZhiArgs = serde_json::from_value(arguments_value)
                    .map_err(|e| ErrorData::invalid_params(format!("参数解析失败: {}", e), None))?;

                call_zhi(args, caller_codex_thread_id, argument_codex_thread_id).await
            }
            "sync" => {
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let args: SyncArgs = serde_json::from_value(arguments_value)
                    .map_err(|e| ErrorData::invalid_params(format!("参数解析失败: {}", e), None))?;

                sync_knowledge(args)
            }
            "checkpoint" => {
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let args: CheckpointArgs = serde_json::from_value(arguments_value)
                    .map_err(|e| ErrorData::invalid_params(format!("参数解析失败: {}", e), None))?;

                create_checkpoint(args)
            }
            "web_fetch" => {
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let args: WebFetchArgs = serde_json::from_value(arguments_value)
                    .map_err(|e| ErrorData::invalid_params(format!("参数解析失败: {}", e), None))?;

                web_fetch(args).await
            }
            "cron_manage" => {
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let args: CronManageArgs = serde_json::from_value(arguments_value)
                    .map_err(|e| ErrorData::invalid_params(format!("参数解析失败: {}", e), None))?;

                cron_manage(args)
            }
            "task" => {
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let task_request: cunzhi::mcp::tools::task::task::TaskRequest =
                    serde_json::from_value(arguments_value).map_err(|e| {
                        ErrorData::invalid_params(format!("参数解析失败: {}", e), None)
                    })?;

                cunzhi::mcp::tools::task::TaskTool::handle(task_request).await
            }
            _ => Err(ErrorData::invalid_request(
                format!("未知的工具: {}", request.name),
                None,
            )),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 解析命令行参数
    let args: Vec<String> = env::args().collect();
    let default_port = if args.len() > 1 {
        args[1].parse::<u16>().unwrap_or(5311)
    } else {
        5311
    };

    // 创建服务器处理器
    let handler = IterateZhiServer { default_port };
    instance_debug_log(
        "[stdio-start-begin]",
        format!(
            "args={:?}, default_port={}, current_exe={:?}, cwd={:?}",
            args,
            default_port,
            env::current_exe().ok(),
            env::current_dir().ok()
        ),
    );

    // 启动 stdio 传输
    let service = handler.serve(stdio()).await.context("MCP 服务器启动失败")?;
    instance_debug_log("[stdio-started]", format!("default_port={}", default_port));

    // 等待服务器关闭或收到终止信号
    tokio::select! {
        result = service.waiting() => {
            match result {
                Ok(reason) => {
                    instance_debug_log("[stdio-stopped]", format!("reason={:?}", reason));
                }
                Err(error) => {
                    instance_debug_log(
                        "[stdio-stopped-error]",
                        format!("error={}", error),
                    );
                    Err(error).context("MCP 服务器运行失败")?;
                }
            }
        }
        _ = tokio::signal::ctrl_c() => {
            // 收到 Ctrl+C，正常退出
            instance_debug_log("[stdio-ctrl-c]", "received ctrl_c");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> &'static Mutex<()> {
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_test_env() -> std::sync::MutexGuard<'static, ()> {
        env_lock().lock().unwrap_or_else(|err| err.into_inner())
    }

    #[test]
    fn enrich_goal_response_adds_saved_image_paths_inside_target() {
        let mut response = DialogResponse {
            keep_going: true,
            user_input: "进入 GoalRun 目标模式。\n\n目标：\n《看这张图\n\n附加图片：1 张\n附件地址：\n- images[0]\n（见 images 附件）》\n\n执行规则：继续".to_string(),
            response_source: "web_bridge_goal_submit".to_string(),
            selected_options: vec![],
            file_paths: vec![],
            image_paths: vec!["/Users/test/.cunzhi/images/image_123_0.jpg".to_string()],
            metadata: ResponseMetadata::default(),
            error: None,
        };

        enrich_goal_response_with_attachment_paths(&mut response);

        assert!(!response.user_input.contains("附件地址："));
        assert!(!response.user_input.contains("images[0]"));
        assert!(!response.user_input.contains("见 images 附件"));
        assert!(response
            .user_input
            .contains("附加图片路径：\n- /Users/test/.cunzhi/images/image_123_0.jpg"));
        assert!(response
            .user_input
            .contains("附加图片路径：\n- /Users/test/.cunzhi/images/image_123_0.jpg》"));
        assert!(response.user_input.contains("》\n\n执行规则"));
    }

    #[test]
    fn enrich_goal_response_adds_selected_options_inside_target() {
        let mut response = DialogResponse {
            keep_going: true,
            user_input: "进入 GoalRun 目标模式。\n\n目标：\n《修复目标提交》\n\n执行规则：继续"
                .to_string(),
            response_source: "web_bridge_goal_submit".to_string(),
            selected_options: vec!["桌面一起做".to_string(), "手机一起做".to_string()],
            file_paths: vec![],
            image_paths: vec![],
            metadata: ResponseMetadata::default(),
            error: None,
        };

        enrich_goal_response_with_attachment_paths(&mut response);

        assert!(response
            .user_input
            .contains("目标：\n《修复目标提交\n\n选中的选项：\n- 桌面一起做\n- 手机一起做》"));
    }

    #[test]
    fn enrich_goal_response_does_not_duplicate_selected_options() {
        let mut response = DialogResponse {
            keep_going: true,
            user_input: "进入 GoalRun 目标模式。\n\n目标：\n《修复目标提交\n\n选中的选项：\n- 桌面一起做》\n\n执行规则：继续".to_string(),
            response_source: "web_bridge_goal_submit".to_string(),
            selected_options: vec!["桌面一起做".to_string()],
            file_paths: vec![],
            image_paths: vec![],
            metadata: ResponseMetadata::default(),
            error: None,
        };

        enrich_goal_response_with_attachment_paths(&mut response);

        assert_eq!(response.user_input.matches("桌面一起做").count(), 1);
        assert_eq!(response.user_input.matches("选中的选项：").count(), 1);
    }

    #[test]
    fn enrich_goal_response_adds_missing_image_path_when_file_path_already_present() {
        let mut response = DialogResponse {
            keep_going: true,
            user_input: "进入 GoalRun 目标模式。\n\n目标：\n《修复目标提交\n\n相关文件：\n@/tmp/spec.md》\n\n执行规则：继续".to_string(),
            response_source: "web_bridge_goal_submit".to_string(),
            selected_options: vec![],
            file_paths: vec!["/tmp/spec.md".to_string()],
            image_paths: vec!["/Users/test/.cunzhi/images/image_123_0.jpg".to_string()],
            metadata: ResponseMetadata::default(),
            error: None,
        };

        enrich_goal_response_with_attachment_paths(&mut response);

        assert_eq!(response.user_input.matches("/tmp/spec.md").count(), 1);
        assert!(!response.user_input.contains("images[0]"));
        assert!(!response.user_input.contains("见 images 附件"));
        assert!(response
            .user_input
            .contains("附加图片路径：\n- /Users/test/.cunzhi/images/image_123_0.jpg》"));
    }

    #[test]
    fn strip_goal_image_reference_context_removes_single_line_legacy_context() {
        let stripped = strip_goal_image_reference_context(
            "进入 GoalRun 目标模式。\n\n目标：\n《修复\n\n附加图片：1 张（见 images 附件）》\n\n执行规则：继续",
        );

        assert!(!stripped.contains("见 images 附件"));
        assert_eq!(stripped.matches("修复").count(), 1);
        assert!(stripped.contains("目标：\n《修复》"));
    }

    #[test]
    fn prepend_selected_options_to_user_input_places_options_before_text() {
        let display = prepend_selected_options_to_user_input(
            "✔️不明白的地方反问我，先不着急编码",
            &["先做 T7".to_string()],
        );

        assert_eq!(
            display,
            "选中的选项: 先做 T7\n\n✔️不明白的地方反问我，先不着急编码"
        );
    }

    #[test]
    fn enrich_goal_response_skips_non_goal_sources() {
        let mut response = DialogResponse {
            keep_going: true,
            user_input: "普通回复".to_string(),
            response_source: "popup_submit".to_string(),
            selected_options: vec![],
            file_paths: vec![],
            image_paths: vec!["/Users/test/.cunzhi/images/image_123_0.jpg".to_string()],
            metadata: ResponseMetadata::default(),
            error: None,
        };

        enrich_goal_response_with_attachment_paths(&mut response);

        assert_eq!(response.user_input, "普通回复");
    }

    struct HomeEnvGuard {
        original: Option<OsString>,
    }

    impl Drop for HomeEnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                env::set_var("HOME", value);
            } else {
                env::remove_var("HOME");
            }
        }
    }

    fn set_test_home(path: &Path) -> HomeEnvGuard {
        let original = env::var_os("HOME");
        env::set_var("HOME", path);
        HomeEnvGuard { original }
    }

    fn create_state_db(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create state db parent");
        }
        let conn = Connection::open(path).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL,
                archived INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                updated_at_ms INTEGER,
                thread_source TEXT
            );",
        )
        .expect("create threads table");
    }

    fn insert_thread(
        state_db_path: &Path,
        id: &str,
        cwd: &str,
        updated_at_ms: i64,
        thread_source: &str,
        archived: i64,
    ) {
        let conn = Connection::open(state_db_path).expect("open state db");
        conn.execute(
            "INSERT INTO threads (id, cwd, archived, updated_at, updated_at_ms, thread_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                cwd,
                archived,
                updated_at_ms / 1000,
                updated_at_ms,
                thread_source
            ],
        )
        .expect("insert thread");
    }

    fn write_workspace_port(port_dir: &Path, port: u16, workspace: &Path) {
        std::fs::write(
            port_dir.join(port.to_string()),
            workspace.display().to_string(),
        )
        .expect("write port mapping");
    }

    fn spawn_health_server() -> (u16, thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind health server");
        listener
            .set_nonblocking(true)
            .expect("set nonblocking health server");
        let port = listener.local_addr().expect("health server addr").port();

        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buffer = [0_u8; 1024];
                        let _ = stream.read(&mut buffer);
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                        );
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        (port, handle)
    }

    fn busy_status(
        active_request_id: Option<&str>,
        interaction_phase: Option<&str>,
        timestamp: &str,
    ) -> PortStatusProbe {
        PortStatusProbe {
            version: None,
            runtime: None,
            is_busy: true,
            busy_since: Some(timestamp.to_string()),
            active_request_id: active_request_id.map(ToOwned::to_owned),
            active_workspace: Some("/Users/test/project".to_string()),
            interaction_phase: interaction_phase.map(ToOwned::to_owned),
            phase_since: Some(timestamp.to_string()),
            active_serve_request_id: Some("serve-123".to_string()),
            ready_since: None,
            capabilities: serde_json::Value::Null,
            frontend_capabilities: Vec::new(),
        }
    }

    fn idle_status_with_html_artifact_capability() -> PortStatusProbe {
        PortStatusProbe {
            version: None,
            runtime: None,
            is_busy: false,
            busy_since: None,
            active_request_id: None,
            active_workspace: None,
            interaction_phase: Some("idle".to_string()),
            phase_since: None,
            active_serve_request_id: None,
            ready_since: None,
            capabilities: serde_json::json!({
                HTML_ARTIFACT_CAPABILITY: true
            }),
            frontend_capabilities: Vec::new(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn find_port_for_workspace_skips_dead_registered_port() {
        let _env_guard = lock_test_env();
        let home = tempdir().expect("temp home");
        let _home_guard = set_test_home(home.path());

        let workspace = home.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let workspace = workspace.canonicalize().expect("canonicalize workspace");

        let port_dir = home.path().join(".cunzhi_ports");
        std::fs::create_dir_all(&port_dir).expect("create port dir");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve dead port");
        let dead_port = listener.local_addr().expect("dead port addr").port();
        drop(listener);

        write_workspace_port(&port_dir, dead_port, &workspace);

        assert_eq!(
            find_port_for_workspace(workspace.to_str().unwrap()).await,
            None
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn find_ports_for_workspace_returns_all_running_matches_in_order() {
        let _env_guard = lock_test_env();
        let home = tempdir().expect("temp home");
        let _home_guard = set_test_home(home.path());

        let workspace_root = home.path().join("workspace");
        let nested_workspace = workspace_root.join("nested");
        std::fs::create_dir_all(&nested_workspace).expect("create nested workspace");
        let workspace_root = workspace_root
            .canonicalize()
            .expect("canonicalize workspace root");
        let nested_workspace = nested_workspace
            .canonicalize()
            .expect("canonicalize nested workspace");

        let port_dir = home.path().join(".cunzhi_ports");
        std::fs::create_dir_all(&port_dir).expect("create port dir");

        let (root_port, root_server) = spawn_health_server();
        let (nested_port, nested_server) = spawn_health_server();

        write_workspace_port(&port_dir, root_port, &workspace_root);
        write_workspace_port(&port_dir, nested_port, &nested_workspace);

        assert_eq!(
            find_ports_for_workspace(nested_workspace.to_str().unwrap()).await,
            vec![nested_port, root_port]
        );

        let _ = root_server.join();
        let _ = nested_server.join();
    }

    #[test]
    fn workspace_port_candidates_continue_after_busy_cluster() {
        let candidates = build_workspace_port_candidates(&[5316, 5317, 5318], 5311);
        assert_eq!(candidates.first().copied(), Some(5319));
    }

    #[test]
    fn port_status_supports_html_artifact_capability_object() {
        assert!(port_status_supports_required_capabilities(
            &idle_status_with_html_artifact_capability()
        ));
    }

    #[test]
    fn port_status_rejects_legacy_service_without_capabilities() {
        let legacy_status = PortStatusProbe {
            version: None,
            runtime: None,
            is_busy: false,
            busy_since: None,
            active_request_id: None,
            active_workspace: None,
            interaction_phase: Some("idle".to_string()),
            phase_since: None,
            active_serve_request_id: None,
            ready_since: None,
            capabilities: serde_json::Value::Null,
            frontend_capabilities: Vec::new(),
        };

        assert!(!port_status_supports_required_capabilities(&legacy_status));
    }

    #[test]
    fn runtime_diagnostics_includes_selected_port_and_runtime_metadata() {
        let status = PortStatusProbe {
            version: Some("0.5.8".to_string()),
            runtime: Some(RuntimeStatusProbe {
                pid: Some(1234),
                exe_path: Some("/Applications/iterate.app/Contents/MacOS/iterate".to_string()),
                exe_mtime: Some("2026-06-02T08:00:00Z".to_string()),
                exe_sha256: Some("sha256:abcdef0123456789abcdef".to_string()),
            }),
            is_busy: false,
            busy_since: None,
            active_request_id: None,
            active_workspace: None,
            interaction_phase: Some("idle".to_string()),
            phase_since: None,
            active_serve_request_id: None,
            ready_since: None,
            capabilities: serde_json::json!({
                HTML_ARTIFACT_CAPABILITY: true
            }),
            frontend_capabilities: Vec::new(),
        };

        let diagnostics = runtime_diagnostics_from_status(
            5312,
            "/Users/test/project",
            Some("/Users/test/other"),
            Some(&status),
        );

        assert!(diagnostics.contains("port=5312"));
        assert!(diagnostics.contains("request_workspace=/Users/test/project"));
        assert!(diagnostics.contains("registered_workspace=/Users/test/other"));
        assert!(diagnostics.contains("version=0.5.8"));
        assert!(diagnostics.contains("pid=1234"));
        assert!(diagnostics.contains("/Applications/iterate.app/Contents/MacOS/iterate"));
        assert!(diagnostics.contains("sha256:abcdef012"));
    }

    #[test]
    fn stale_busy_status_respects_interaction_phase() {
        let missing_request_id = busy_status(None, None, "2026-05-07T23:20:07Z");
        let old_legacy_active_request_id =
            busy_status(Some("req_123"), None, "2000-01-01T00:00:00Z");
        let old_waiting_user = busy_status(
            Some("req_123"),
            Some("waiting_user"),
            "2000-01-01T00:00:00Z",
        );
        let old_starting_gui = busy_status(
            Some("req_123"),
            Some("starting_gui"),
            "2000-01-01T00:00:00Z",
        );
        let fresh_starting_gui = busy_status(
            Some("req_123"),
            Some("starting_gui"),
            &chrono::Utc::now().to_rfc3339(),
        );
        let failed = busy_status(
            Some("req_123"),
            Some("failed"),
            &chrono::Utc::now().to_rfc3339(),
        );

        assert!(is_stale_busy_status(&missing_request_id));
        assert!(!is_stale_busy_status(&old_legacy_active_request_id));
        assert!(!is_stale_busy_status(&old_waiting_user));
        assert!(is_stale_busy_status(&old_starting_gui));
        assert!(!is_stale_busy_status(&fresh_starting_gui));
        assert!(is_stale_busy_status(&failed));
    }

    #[test]
    fn workspace_port_candidates_start_from_preferred_when_workspace_has_no_ports() {
        let candidates = build_workspace_port_candidates(&[], 5311);
        assert_eq!(candidates.first().copied(), Some(5311));
    }

    #[test]
    fn call_zhi_does_not_short_circuit_independent_popups() {
        let source = include_str!("mcp-server.rs");
        for forbidden in [
            concat!("acquire_zhi_", "singleflight"),
            concat!("call_zhi_", "singleflight_duplicate"),
            concat!("zhi_", "singleflight_merged"),
        ] {
            assert!(
                !source.contains(forbidden),
                "call_zhi must not restore project/thread-level popup merging: {forbidden}"
            );
        }
    }

    #[test]
    fn port_registration_matches_workspace_rejects_other_workspace() {
        let _env_guard = lock_test_env();
        let home = tempdir().expect("temp home");
        let _home_guard = set_test_home(home.path());

        let workspace = home.path().join("workspace");
        let other_workspace = home.path().join("other");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::create_dir_all(&other_workspace).expect("create other workspace");
        let workspace = workspace.canonicalize().expect("canonicalize workspace");
        let other_workspace = other_workspace
            .canonicalize()
            .expect("canonicalize other workspace");

        let port_dir = home.path().join(".cunzhi_ports");
        std::fs::create_dir_all(&port_dir).expect("create port dir");
        write_workspace_port(&port_dir, 5311, &other_workspace);

        assert!(!port_registration_matches_workspace(
            5311,
            workspace.to_str().unwrap()
        ));
        assert!(port_registration_matches_workspace(
            5311,
            other_workspace.to_str().unwrap()
        ));
        assert!(port_registration_matches_workspace(
            5312,
            workspace.to_str().unwrap()
        ));
    }

    #[test]
    fn latest_codex_thread_fallback_uses_new_sqlite_state_dir() {
        let _env_guard = lock_test_env();
        let home = tempdir().expect("temp home");
        let _home_guard = set_test_home(home.path());

        let codex_home = home.path().join(".codex");
        let state_db = codex_home.join("sqlite").join("state_5.sqlite");
        create_state_db(&state_db);

        let workspace = home.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let workspace = workspace.canonicalize().expect("canonicalize workspace");
        let workspace_str = workspace.to_str().unwrap();

        insert_thread(
            &state_db,
            "019ec000-0000-7000-8000-000000000001",
            workspace_str,
            1_000,
            "user",
            0,
        );
        insert_thread(
            &state_db,
            "019ec000-0000-7000-8000-000000000002",
            workspace_str,
            3_000,
            "subagent",
            0,
        );
        insert_thread(
            &state_db,
            "019ec000-0000-7000-8000-000000000003",
            workspace_str,
            4_000,
            "user",
            1,
        );
        insert_thread(
            &state_db,
            "019ec000-0000-7000-8000-000000000004",
            workspace_str,
            2_000,
            "user",
            0,
        );

        let fallback =
            latest_codex_thread_fallback_for_project(workspace_str).expect("thread fallback");
        assert_eq!(fallback.thread_id, "019ec000-0000-7000-8000-000000000004");
        assert_eq!(fallback.state_db_path, state_db);
    }

    #[test]
    fn latest_codex_thread_fallback_uses_newest_thread_across_state_dbs() {
        let _env_guard = lock_test_env();
        let home = tempdir().expect("temp home");
        let _home_guard = set_test_home(home.path());

        let codex_home = home.path().join(".codex");
        let sqlite_state_db = codex_home.join("sqlite").join("state_5.sqlite");
        let root_state_db = codex_home.join("state_5.sqlite");
        create_state_db(&sqlite_state_db);
        create_state_db(&root_state_db);

        let workspace = home.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let workspace = workspace.canonicalize().expect("canonicalize workspace");
        let workspace_str = workspace.to_str().unwrap();

        insert_thread(
            &sqlite_state_db,
            "019ec000-0000-7000-8000-000000000005",
            workspace_str,
            5_000,
            "user",
            0,
        );
        insert_thread(
            &root_state_db,
            "019ec000-0000-7000-8000-000000000006",
            workspace_str,
            6_000,
            "user",
            0,
        );
        insert_thread(
            &root_state_db,
            "019ec000-0000-7000-8000-000000000007",
            workspace_str,
            7_000,
            "subagent",
            0,
        );
        insert_thread(
            &root_state_db,
            "019ec000-0000-7000-8000-000000000008",
            workspace_str,
            8_000,
            "user",
            1,
        );

        let fallback =
            latest_codex_thread_fallback_for_project(workspace_str).expect("thread fallback");
        assert_eq!(fallback.thread_id, "019ec000-0000-7000-8000-000000000006");
        assert_eq!(fallback.state_db_path, root_state_db);
    }

    #[test]
    fn codex_thread_title_uses_latest_non_empty_session_index_name() {
        let codex_home = tempdir().expect("temp codex home");
        std::fs::write(
            codex_home.path().join("session_index.jsonl"),
            concat!(
                "not-json\n",
                "{\"id\":\"thread-a\",\"thread_name\":\"旧标题\"}\n",
                "{\"id\":\"thread-b\",\"thread_name\":\"其他标题\"}\n",
                "{\"id\":\"thread-a\",\"thread_name\":\"   \"}\n",
                "{\"id\":\"thread-a\",\"thread_name\":\" 当前对话标题 \"}\n",
            ),
        )
        .expect("write session index");

        assert_eq!(
            codex_thread_title_from_session_index(codex_home.path(), "thread-a").as_deref(),
            Some("当前对话标题")
        );
        assert_eq!(
            codex_thread_title_from_session_index(codex_home.path(), "missing"),
            None
        );
    }
}
