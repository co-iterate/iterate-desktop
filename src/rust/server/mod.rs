//! HTTP 服务器模块 - 类似 Infinite WF 的端口监听模式
//!
//! 工作流程：
//! 1. iterate --serve [port] 启动 HTTP 服务器
//! 2. AI 调用 iterate --bridge --port [port] --message "xxx"
//! 3. 脚本发送 HTTP 请求到服务器
//! 4. 服务器弹出 GUI 等待用户输入
//! 5. 用户输入后返回 JSON 响应给脚本
//! 6. 脚本输出结果给 AI

pub mod commands;
pub mod loop_session;

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tower_http::cors::{Any, CorsLayer};

use loop_session::{
    classify_loop_start_goal_binding, default_max_iterations, goal_progress_signature,
    is_blocked_goal_status, is_completed_message, is_exit_loop_message, is_loop_start_source,
    is_loop_stop_source, is_terminal_goal_status, log_loop_debug, read_loop_sessions,
    truncate_for_log, write_loop_sessions, LoopGoalBinding, LoopGoalSnapshot, LoopSession,
};

const HTML_ARTIFACT_CAPABILITY: &str = "html_artifact";
const SERVE_RESPONSE_ROUTE_TTL_SECS: i64 = 30 * 60;

fn instance_debug_log(tag: &str, message: impl AsRef<str>) {
    let line = format!(
        "{} [server:{}] {} {}\n",
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

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeMetadata {
    pid: u32,
    started_at: String,
    exe_path: Option<String>,
    exe_mtime: Option<String>,
    exe_sha256: Option<String>,
}

impl RuntimeMetadata {
    fn collect() -> Self {
        let exe_path = std::env::current_exe().ok();
        Self {
            pid: std::process::id(),
            started_at: Utc::now().to_rfc3339(),
            exe_path: exe_path.as_ref().map(|path| path.display().to_string()),
            exe_mtime: exe_path
                .as_ref()
                .and_then(|path| file_modified_at(path.as_path())),
            exe_sha256: exe_path
                .as_ref()
                .and_then(|path| file_sha256(path.as_path())),
        }
    }
}

fn file_modified_at(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let datetime: DateTime<Utc> = system_time_to_utc(modified);
    Some(datetime.to_rfc3339())
}

fn system_time_to_utc(time: SystemTime) -> DateTime<Utc> {
    time.into()
}

fn file_sha256(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    Some(format!("sha256:{}", hex::encode(digest.as_ref())))
}

/// 服务器状态
pub struct ServerState {
    /// 请求队列发送端
    request_tx: mpsc::Sender<DialogRequest>,
    /// 是否有 AI 正在等待用户响应
    is_busy: bool,
    /// 当前占用的 agent_id
    current_agent: Option<String>,
    /// 开始占用的时间
    busy_since: Option<String>,
    /// 当前 MCP 父请求 ID，用于跨 mcp-server / serve / GUI 日志关联
    active_request_id: Option<String>,
    /// 当前请求工作区
    active_workspace: Option<String>,
    /// 当前请求消息长度
    active_message_len: Option<usize>,
    /// 当前交互生命周期阶段
    interaction_phase: InteractionPhase,
    /// 当前阶段开始时间
    phase_since: Option<String>,
    /// 当前 serve 内部请求 ID
    active_serve_request_id: Option<String>,
    /// popup ready 时间
    ready_since: Option<String>,
    /// serve 运行时元数据，用于区分同版本不同构建
    runtime: RuntimeMetadata,
}

/// 当前请求的交互生命周期阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionPhase {
    Idle,
    Queued,
    StartingGui,
    WaitingUser,
    Responded,
    Cleaning,
    Failed,
}

/// serve handler 回传给 HTTP status 的内部生命周期事件。
#[derive(Debug, Clone)]
pub struct InteractionLifecycleEvent {
    pub phase: InteractionPhase,
    pub serve_request_id: Option<String>,
}

impl InteractionLifecycleEvent {
    pub fn new(phase: InteractionPhase, serve_request_id: impl Into<Option<String>>) -> Self {
        Self {
            phase,
            serve_request_id: serve_request_id.into(),
        }
    }
}

fn reset_active_interaction(state: &mut ServerState) {
    state.is_busy = false;
    state.current_agent = None;
    state.busy_since = None;
    state.active_request_id = None;
    state.active_workspace = None;
    state.active_message_len = None;
    state.interaction_phase = InteractionPhase::Idle;
    state.phase_since = None;
    state.active_serve_request_id = None;
    state.ready_since = None;
}

fn apply_lifecycle_event(state: &mut ServerState, event: InteractionLifecycleEvent) {
    let now = chrono::Utc::now().to_rfc3339();
    let phase = event.phase;
    let serve_request_id = event.serve_request_id.clone();
    state.interaction_phase = event.phase;
    state.phase_since = Some(now.clone());
    if let Some(serve_request_id) = serve_request_id.clone() {
        state.active_serve_request_id = Some(serve_request_id);
    }
    if phase == InteractionPhase::WaitingUser && state.ready_since.is_none() {
        state.ready_since = Some(now);
    }
    let project_path = state.active_workspace.as_deref();
    match phase {
        InteractionPhase::WaitingUser => {
            let _ = crate::ui::live_goal::mark_live_goal_waiting_for_user(
                project_path,
                serve_request_id.as_deref(),
            );
        }
        InteractionPhase::Responded | InteractionPhase::Cleaning => {
            let _ = crate::ui::live_goal::mark_live_goal_user_response_received(
                project_path,
                serve_request_id.as_deref(),
            );
        }
        InteractionPhase::Failed => {
            let _ = crate::ui::live_goal::mark_live_goal_user_interaction_failed(
                project_path,
                serve_request_id.as_deref(),
            );
        }
        InteractionPhase::Idle | InteractionPhase::Queued | InteractionPhase::StartingGui => {}
    }
}

fn active_interaction_matches(state: &ServerState, request_id: &str, workspace: &str) -> bool {
    if !state.is_busy {
        return false;
    }

    if request_id.is_empty() {
        state.active_request_id.is_none() && state.active_workspace.as_deref() == Some(workspace)
    } else {
        state.active_request_id.as_deref() == Some(request_id)
    }
}

struct DialogCleanupGuard {
    state: Arc<Mutex<ServerState>>,
    request_id: String,
    workspace: String,
    lifecycle_task: Option<JoinHandle<()>>,
    active: bool,
}

impl DialogCleanupGuard {
    fn new(
        state: Arc<Mutex<ServerState>>,
        request_id: String,
        workspace: String,
        lifecycle_task: JoinHandle<()>,
    ) -> Self {
        Self {
            state,
            request_id,
            workspace,
            lifecycle_task: Some(lifecycle_task),
            active: true,
        }
    }

    fn abort_lifecycle(&mut self) {
        if let Some(task) = self.lifecycle_task.take() {
            task.abort();
        }
    }

    fn disarm(&mut self) {
        self.abort_lifecycle();
        self.active = false;
    }
}

impl Drop for DialogCleanupGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        self.abort_lifecycle();
        let state = Arc::clone(&self.state);
        let request_id = self.request_id.clone();
        let workspace = self.workspace.clone();
        tokio::spawn(async move {
            let mut state_guard = state.lock().await;
            if active_interaction_matches(&state_guard, &request_id, &workspace) {
                instance_debug_log(
                    "[handle-dialog-drop-cleanup]",
                    format!(
                        "request_id={}, workspace={:?}, phase={:?}; resetting active state",
                        request_id, workspace, state_guard.interaction_phase
                    ),
                );
                reset_active_interaction(&mut state_guard);
            }
        });
    }
}

/// 对话请求
#[derive(Debug, Serialize, Deserialize)]
pub struct DialogRequest {
    /// MCP 父请求 ID，用于跨层关联日志
    #[serde(default)]
    pub request_id: String,
    /// 消息内容
    pub message: String,
    /// 预定义选项
    #[serde(default)]
    pub options: Vec<String>,
    /// 工作区路径
    #[serde(default)]
    pub workspace: String,
    /// 是否支持 Markdown
    #[serde(default = "default_true")]
    pub is_markdown: bool,
    /// 调用方 Codex home（只传路径，不传 token），用于按当前 MCP 账号查询额度
    #[serde(default)]
    pub codex_home: Option<String>,
    /// 调用本次 MCP 的 Codex 会话 ID，用于回到原会话
    #[serde(default)]
    pub codex_thread_id: Option<String>,
    /// 调用本次 MCP 的 Codex 会话 deep link
    #[serde(default)]
    pub codex_deeplink: Option<String>,
    /// 调用本次 MCP 的对话标题
    #[serde(default)]
    pub conversation_title: Option<String>,
    /// 当前请求对应的 checkpoint ID
    #[serde(default)]
    pub checkpoint_id: Option<String>,
    /// 当前请求对应的 checkpoint commit
    #[serde(default)]
    pub checkpoint_commit: Option<String>,
    /// 当前请求对应的 checkpoint subject/message
    #[serde(default)]
    pub checkpoint_message: Option<String>,
    /// 当前请求是否处于循环态（内部透传给 popup 使用）
    #[serde(default)]
    pub loop_active: bool,
    /// 当前请求是否必须强制弹出并接管窗口（例如 loop 完成交付）
    #[serde(default)]
    pub force_popup: bool,
    /// 响应通道（内部使用）
    #[serde(skip)]
    pub response_tx: Option<oneshot::Sender<DialogResponse>>,
    /// 生命周期状态通道（内部使用）
    #[serde(skip)]
    pub lifecycle_tx: Option<mpsc::UnboundedSender<InteractionLifecycleEvent>>,
}

fn default_true() -> bool {
    true
}

fn normalize_loop_scope(workspace: &str) -> Option<String> {
    let trimmed = workspace.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn needs_user_attention(request: &DialogRequest) -> bool {
    loop_session::needs_user_attention_from_message(&request.message, !request.options.is_empty())
}

fn loop_goal_from_live_goal(goal: crate::ui::live_goal::LiveGoalSnapshot) -> LoopGoalSnapshot {
    LoopGoalSnapshot {
        goal_id: goal.id,
        title: goal.title,
        status: goal.status,
        phase: goal.phase,
        status_text: goal.status_text,
        progress_percent: goal.progress_percent,
        progress_label: goal.progress_label,
        project_path: goal.project_path,
        request_id: goal.request_id,
        codex_thread_id: goal.codex_thread_id,
        source: Some(goal.source),
    }
}

fn resolve_strong_loop_goal_for_request_context(
    workspace: &str,
    request_ids: &[&str],
    codex_thread_id: Option<&str>,
) -> Option<LoopGoalSnapshot> {
    let live_goal = crate::ui::live_goal::live_goal_snapshot_for_project_strict(Some(workspace))?;
    let goal = loop_goal_from_live_goal(live_goal);
    let (binding, reason) = classify_loop_start_goal_binding(&goal, request_ids, codex_thread_id);
    let _ = log_loop_debug(&format!(
        "goal candidate: goal_id={} binding={:?} reason={} request_ids={:?} codex_thread_id={:?}",
        goal.goal_id, binding, reason, request_ids, codex_thread_id
    ));

    match binding {
        LoopGoalBinding::Strong => Some(goal),
        LoopGoalBinding::Weak | LoopGoalBinding::Stale => None,
    }
}

fn read_current_bound_loop_goal(
    request: &DialogRequest,
    bound_goal: &LoopGoalSnapshot,
) -> Result<LoopGoalSnapshot, &'static str> {
    let live_goal =
        crate::ui::live_goal::live_goal_snapshot_for_project_strict(Some(&request.workspace))
            .ok_or("goal_missing_or_project_mismatch")?;
    let current_goal = loop_goal_from_live_goal(live_goal);
    if current_goal.goal_id != bound_goal.goal_id {
        return Err("goal_id_changed");
    }
    Ok(current_goal)
}

fn format_loop_goal_context(goal: &LoopGoalSnapshot, stagnant_iterations: u32) -> String {
    format!(
        "绑定目标:\n- id: {}\n- title: {}\n- status: {}\n- phase: {}\n- progress: {}\n- source: {}\n- stagnant_iterations: {}",
        goal.goal_id,
        goal.title,
        goal.status,
        goal.phase.as_deref().unwrap_or(""),
        goal.progress_label.as_deref().unwrap_or(""),
        goal.source.as_deref().unwrap_or(""),
        stagnant_iterations
    )
}

/// 对话响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogResponse {
    /// 是否继续
    pub keep_going: bool,
    /// 用户输入
    #[serde(default)]
    pub user_input: String,
    /// 响应来源（如 popup_continue / popup_loop_start / loop_auto_continue）
    #[serde(default)]
    pub response_source: String,
    /// 选中的选项
    #[serde(default)]
    pub selected_options: Vec<String>,
    /// 附加的文件路径
    #[serde(default)]
    pub file_paths: Vec<String>,
    /// 附加的图片路径
    #[serde(default)]
    pub image_paths: Vec<String>,
    /// conversation / timeline metadata returned by the popup response path.
    #[serde(default)]
    pub metadata: crate::mcp::ResponseMetadata,
    /// 错误信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Default for DialogResponse {
    fn default() -> Self {
        Self {
            keep_going: false,
            user_input: String::new(),
            response_source: String::new(),
            selected_options: vec![],
            file_paths: vec![],
            image_paths: vec![],
            metadata: crate::mcp::ResponseMetadata::default(),
            error: None,
        }
    }
}

/// 健康检查
async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "cunzhi"
    }))
}

/// 端口占用状态查询
async fn status_check(State(state): State<Arc<Mutex<ServerState>>>) -> Json<serde_json::Value> {
    let state = state.lock().await;
    let loop_sessions = read_loop_sessions();
    let loop_info: Vec<serde_json::Value> = loop_sessions
        .iter()
        .map(|(k, v)| {
            serde_json::json!({
                "scope": k,
                "iteration": v.iteration_count,
                "max_iterations": v.max_iterations,
            })
        })
        .collect();
    Json(serde_json::json!({
        "is_busy": state.is_busy,
        "current_agent": state.current_agent,
        "busy_since": state.busy_since,
        "active_request_id": state.active_request_id,
        "active_workspace": state.active_workspace,
        "active_message_len": state.active_message_len,
        "interaction_phase": state.interaction_phase,
        "phase_since": state.phase_since,
        "active_serve_request_id": state.active_serve_request_id,
        "ready_since": state.ready_since,
        "active_loop_scopes": loop_sessions.len(),
        "loop_sessions": loop_info,
        "version": env!("CARGO_PKG_VERSION"),
        "runtime": state.runtime.clone(),
        "capabilities": {
            HTML_ARTIFACT_CAPABILITY: true,
            "html_artifact_blocks": true
        },
        "frontend_capabilities": [HTML_ARTIFACT_CAPABILITY]
    }))
}

#[derive(Debug, Deserialize)]
struct RenewServeResponseRouteRequest {
    request_id: String,
    #[serde(default)]
    project_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServeResponseRoute {
    request_id: String,
    project_path: String,
    response_file: String,
    created_at: i64,
    #[serde(default)]
    original_created_at: Option<i64>,
    #[serde(default)]
    renewed_at: Option<i64>,
    #[serde(default)]
    renewed_by: Option<String>,
    #[serde(default)]
    renewal_count: Option<u64>,
}

#[derive(Debug, Serialize)]
struct RenewServeResponseRouteResponse {
    ok: bool,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_path: Option<String>,
    route_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_file: Option<String>,
    renewed: bool,
    route_age_secs: i64,
    route_ttl_secs: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    renewal_count: Option<u64>,
}

fn sanitize_request_id_for_route_file(request_id: &str) -> String {
    request_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn serve_response_route_file(request_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "iterate_response_route_{}.json",
        sanitize_request_id_for_route_file(request_id)
    ))
}

fn normalize_path_for_compare(path: &str) -> String {
    let path = PathBuf::from(path);
    let path = if path.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    } else {
        path
    };
    std::fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn path_is_inside_temp(path: &Path) -> bool {
    let Some(path) = normalize_absolute_path_without_parent(path) else {
        return false;
    };
    let Some(temp_dir) = normalize_absolute_path_without_parent(&std::env::temp_dir()) else {
        return false;
    };
    path == temp_dir || path.starts_with(temp_dir)
}

fn normalize_absolute_path_without_parent(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => return None,
        }
    }

    Some(normalized)
}

fn renew_serve_response_route_file(
    request_id: &str,
    expected_project: &str,
) -> Result<RenewServeResponseRouteResponse, RenewServeResponseRouteResponse> {
    let route_file = serve_response_route_file(request_id);
    let route_file_str = route_file.display().to_string();
    let now = Utc::now().timestamp();
    let content = match fs::read_to_string(&route_file) {
        Ok(content) => content,
        Err(_) => {
            return Err(RenewServeResponseRouteResponse {
                ok: false,
                status: "rejected",
                reason: Some("response_route_missing"),
                request_id: request_id.to_string(),
                project_path: Some(expected_project.to_string()),
                route_file: route_file_str,
                response_file: None,
                renewed: false,
                route_age_secs: 0,
                route_ttl_secs: SERVE_RESPONSE_ROUTE_TTL_SECS,
                original_created_at: None,
                renewal_count: None,
            });
        }
    };
    let mut route: ServeResponseRoute = match serde_json::from_str(&content) {
        Ok(route) => route,
        Err(_) => {
            return Err(RenewServeResponseRouteResponse {
                ok: false,
                status: "rejected",
                reason: Some("response_route_invalid"),
                request_id: request_id.to_string(),
                project_path: Some(expected_project.to_string()),
                route_file: route_file_str,
                response_file: None,
                renewed: false,
                route_age_secs: 0,
                route_ttl_secs: SERVE_RESPONSE_ROUTE_TTL_SECS,
                original_created_at: None,
                renewal_count: None,
            });
        }
    };

    let response_file = PathBuf::from(&route.response_file);
    let response_file_str = Some(response_file.display().to_string());
    let route_age_secs = now.saturating_sub(route.created_at);

    if route.request_id != request_id {
        return Err(RenewServeResponseRouteResponse {
            ok: false,
            status: "rejected",
            reason: Some("response_route_request_id_mismatch"),
            request_id: request_id.to_string(),
            project_path: Some(expected_project.to_string()),
            route_file: route_file_str,
            response_file: response_file_str,
            renewed: false,
            route_age_secs,
            route_ttl_secs: SERVE_RESPONSE_ROUTE_TTL_SECS,
            original_created_at: route.original_created_at,
            renewal_count: route.renewal_count,
        });
    }

    if normalize_path_for_compare(&route.project_path)
        != normalize_path_for_compare(expected_project)
    {
        return Err(RenewServeResponseRouteResponse {
            ok: false,
            status: "rejected",
            reason: Some("response_route_project_mismatch"),
            request_id: request_id.to_string(),
            project_path: Some(expected_project.to_string()),
            route_file: route_file_str,
            response_file: response_file_str,
            renewed: false,
            route_age_secs,
            route_ttl_secs: SERVE_RESPONSE_ROUTE_TTL_SECS,
            original_created_at: route.original_created_at,
            renewal_count: route.renewal_count,
        });
    }

    if !path_is_inside_temp(&response_file) {
        return Err(RenewServeResponseRouteResponse {
            ok: false,
            status: "rejected",
            reason: Some("response_file_outside_temp_dir"),
            request_id: request_id.to_string(),
            project_path: Some(expected_project.to_string()),
            route_file: route_file_str,
            response_file: response_file_str,
            renewed: false,
            route_age_secs,
            route_ttl_secs: SERVE_RESPONSE_ROUTE_TTL_SECS,
            original_created_at: route.original_created_at,
            renewal_count: route.renewal_count,
        });
    }

    let original_created_at = route.original_created_at.or(Some(route.created_at));
    let renewal_count = route.renewal_count.unwrap_or(0).saturating_add(1);
    route.created_at = now;
    route.original_created_at = original_created_at;
    route.renewed_at = Some(now);
    route.renewed_by = Some("iterate-server".to_string());
    route.renewal_count = Some(renewal_count);

    if fs::write(
        &route_file,
        format!("{}\n", serde_json::to_string(&route).unwrap_or_default()),
    )
    .is_err()
    {
        return Err(RenewServeResponseRouteResponse {
            ok: false,
            status: "rejected",
            reason: Some("response_route_write_failed"),
            request_id: request_id.to_string(),
            project_path: Some(expected_project.to_string()),
            route_file: route_file_str,
            response_file: response_file_str,
            renewed: false,
            route_age_secs,
            route_ttl_secs: SERVE_RESPONSE_ROUTE_TTL_SECS,
            original_created_at,
            renewal_count: Some(renewal_count),
        });
    }

    Ok(RenewServeResponseRouteResponse {
        ok: true,
        status: "renewed",
        reason: None,
        request_id: request_id.to_string(),
        project_path: Some(expected_project.to_string()),
        route_file: route_file_str,
        response_file: response_file_str,
        renewed: true,
        route_age_secs: 0,
        route_ttl_secs: SERVE_RESPONSE_ROUTE_TTL_SECS,
        original_created_at,
        renewal_count: Some(renewal_count),
    })
}

async fn renew_serve_response_route(
    State(state): State<Arc<Mutex<ServerState>>>,
    Json(request): Json<RenewServeResponseRouteRequest>,
) -> Response {
    let request_id = request.request_id.trim();
    if request_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "ok": false,
                "status": "rejected",
                "reason": "missing_request_id"
            })),
        )
            .into_response();
    }

    let state = state.lock().await;
    if !state.is_busy || state.interaction_phase != InteractionPhase::WaitingUser {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "ok": false,
                "status": "rejected",
                "reason": "target_not_waiting",
                "request_id": request_id
            })),
        )
            .into_response();
    }
    if state.active_serve_request_id.as_deref() != Some(request_id) {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "ok": false,
                "status": "rejected",
                "reason": "target_request_id_mismatch",
                "request_id": request_id,
                "active_serve_request_id": state.active_serve_request_id
            })),
        )
            .into_response();
    }

    let Some(active_workspace) = state.active_workspace.clone() else {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "ok": false,
                "status": "rejected",
                "reason": "target_workspace_missing",
                "request_id": request_id
            })),
        )
            .into_response();
    };
    if let Some(project_path) = request.project_path.as_deref() {
        if normalize_path_for_compare(project_path) != normalize_path_for_compare(&active_workspace)
        {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "ok": false,
                    "status": "rejected",
                    "reason": "target_workspace_mismatch",
                    "request_id": request_id,
                    "active_workspace": active_workspace
                })),
            )
                .into_response();
        }
    }
    drop(state);

    match renew_serve_response_route_file(request_id, &active_workspace) {
        Ok(response) => Json(response).into_response(),
        Err(response) => (StatusCode::CONFLICT, Json(response)).into_response(),
    }
}

/// 处理对话请求
/// 构造带 Content-Length 的 JSON 响应（避免 chunked transfer encoding）
fn json_response(resp: &DialogResponse) -> Response {
    let body = serde_json::to_string(resp).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn handle_dialog(
    State(state): State<Arc<Mutex<ServerState>>>,
    Json(mut request): Json<DialogRequest>,
) -> Response {
    instance_debug_log(
        "[handle-dialog-entry]",
        format!(
            "request_id={}, workspace={:?}, options_len={}, loop_active={}, force_popup={}, message_len={}",
            request.request_id,
            request.workspace,
            request.options.len(),
            request.loop_active,
            request.force_popup,
            request.message.len()
        ),
    );
    let scope = normalize_loop_scope(&request.workspace);
    let _ = log_loop_debug(&format!(
        "handle_dialog entry: workspace={:?} scope={:?}",
        request.workspace, scope
    ));
    if crate::ui::live_goal::should_auto_complete_live_goal_from_report(&request.message) {
        match crate::ui::live_goal::complete_live_goal_from_report(
            Some(request.workspace.as_str()),
            Some(request.request_id.as_str()),
        ) {
            Ok(Some(goal)) => instance_debug_log(
                "[live-goal-auto-completed]",
                format!(
                    "request_id={}, workspace={:?}, goal_id={}, title={:?}",
                    request.request_id, request.workspace, goal.id, goal.title
                ),
            ),
            Ok(None) => instance_debug_log(
                "[live-goal-auto-complete-skipped]",
                format!(
                    "request_id={}, workspace={:?}, reason=no_matching_active_goal",
                    request.request_id, request.workspace
                ),
            ),
            Err(error) => instance_debug_log(
                "[live-goal-auto-complete-failed]",
                format!(
                    "request_id={}, workspace={:?}, error={}",
                    request.request_id, request.workspace, error
                ),
            ),
        }
    }

    // v1：循环态通过文件持久化，所有端口共享同一份 loop state
    {
        if let Some(scope_key) = scope.as_ref() {
            let mut sessions = read_loop_sessions();
            let _ = log_loop_debug(&format!(
                "checking sessions: scope_key={:?} sessions_len={} has_key={}",
                scope_key,
                sessions.len(),
                sessions.contains_key(scope_key)
            ));
            if let Some(loop_session) = sessions.get(scope_key).cloned() {
                let is_exit = is_exit_loop_message(&request.message);
                let is_completed = is_completed_message(&request.message);
                let needs_attention = needs_user_attention(&request);
                let msg_preview = truncate_for_log(&request.message, 100);
                let _ = log_loop_debug(&format!(
                    "found session: is_exit={} is_completed={} needs_attention={} msg_preview={:?}",
                    is_exit, is_completed, needs_attention, msg_preview
                ));
                if is_exit {
                    sessions.remove(scope_key);
                    write_loop_sessions(&sessions);
                    let _ = log_loop_debug(
                        "exit loop detected, removed session, returning keep_going=false",
                    );
                    return json_response(&DialogResponse {
                        keep_going: false,
                        user_input: "循环已停止。".to_string(),
                        response_source: "loop_exit".to_string(),
                        ..Default::default()
                    });
                } else if is_completed {
                    sessions.remove(scope_key);
                    write_loop_sessions(&sessions);
                    request.loop_active = false;
                    request.force_popup = true;
                    let _ = log_loop_debug(
                        "completed during loop, removed session, showing popup for final delivery",
                    );
                } else if needs_attention {
                    request.loop_active = true;
                    let _ = log_loop_debug("needs attention, showing popup with loop_active=true");
                } else {
                    // 递增迭代计数
                    let new_count = loop_session.iteration_count + 1;
                    let max_iter = loop_session.max_iterations;
                    let mut next_goal = loop_session.goal.clone();
                    let mut next_progress_signature = loop_session.last_progress_signature.clone();
                    let mut next_stagnant_iterations = loop_session.stagnant_iterations;
                    let mut goal_popup_reason: Option<&'static str> = None;
                    let mut clear_session_for_goal = false;
                    let mut goal_completed = false;

                    if let Some(bound_goal) = loop_session.goal.as_ref() {
                        match read_current_bound_loop_goal(&request, bound_goal) {
                            Ok(current_goal) => {
                                if is_terminal_goal_status(&current_goal.status) {
                                    next_progress_signature =
                                        Some(goal_progress_signature(&current_goal));
                                    next_goal = Some(current_goal);
                                    goal_popup_reason = Some("goal_completed");
                                    clear_session_for_goal = true;
                                    goal_completed = true;
                                } else if is_blocked_goal_status(&current_goal.status) {
                                    next_progress_signature =
                                        Some(goal_progress_signature(&current_goal));
                                    next_goal = Some(current_goal);
                                    goal_popup_reason = Some("goal_blocked");
                                } else {
                                    let signature = goal_progress_signature(&current_goal);
                                    next_stagnant_iterations =
                                        if loop_session.last_progress_signature.as_deref()
                                            == Some(signature.as_str())
                                        {
                                            loop_session.stagnant_iterations.saturating_add(1)
                                        } else {
                                            0
                                        };
                                    next_progress_signature = Some(signature);
                                    next_goal = Some(current_goal);

                                    if next_stagnant_iterations >= 2 {
                                        goal_popup_reason = Some("goal_stagnant");
                                    }
                                }
                            }
                            Err(reason) => {
                                goal_popup_reason = Some(reason);
                                clear_session_for_goal = true;
                            }
                        }
                    }

                    // 保存本轮 AI 消息和迭代计数到 session
                    let mut updated_sessions = sessions.clone();
                    if clear_session_for_goal {
                        updated_sessions.remove(scope_key);
                    } else if let Some(s) = updated_sessions.get_mut(scope_key) {
                        s.last_ai_message = request.message.clone();
                        s.iteration_count = new_count;
                        s.goal = next_goal.clone();
                        s.last_progress_signature = next_progress_signature.clone();
                        s.stagnant_iterations = next_stagnant_iterations;
                    }
                    write_loop_sessions(&updated_sessions);

                    // 安全阀：达到最大迭代次数时强制弹窗
                    if let Some(reason) = goal_popup_reason {
                        request.loop_active = !goal_completed;
                        request.force_popup = true;
                        let _ = log_loop_debug(&format!(
                            "goal-bound loop forcing popup: reason={} iteration={}/{}",
                            reason, new_count, max_iter
                        ));
                    } else if new_count >= max_iter {
                        request.loop_active = true;
                        let _ = log_loop_debug(&format!(
                            "max_iterations reached ({}/{}), forcing popup",
                            new_count, max_iter
                        ));
                    } else {
                        // 构造包含上下文的 auto_continue 响应
                        let iteration_info = format!("[迭代 {}/{}]", new_count, max_iter);
                        let goal_context = next_goal
                            .as_ref()
                            .map(|goal| format_loop_goal_context(goal, next_stagnant_iterations));
                        let current_ai_message = request.message.trim();
                        let base_prompt = if let Some(goal_context) = goal_context {
                            format!(
                                "{}\n\n{}\n\n原始循环请求:\n{}",
                                iteration_info, goal_context, loop_session.loop_prompt
                            )
                        } else {
                            format!("{}\n\n{}", iteration_info, loop_session.loop_prompt)
                        };
                        let context_prompt = if current_ai_message.is_empty() {
                            base_prompt
                        } else {
                            format!(
                                "{}\n\n---\nAI 上一轮输出（供参考）:\n{}",
                                base_prompt,
                                truncate_for_log(current_ai_message, 2000)
                            )
                        };
                        let _ = log_loop_debug(&format!(
                            "auto_continue iteration {}/{}, prompt_len={}",
                            new_count,
                            max_iter,
                            context_prompt.len()
                        ));
                        return json_response(&DialogResponse {
                            keep_going: true,
                            user_input: context_prompt,
                            response_source: "loop_auto_continue".to_string(),
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }

    let (response_tx, response_rx) = oneshot::channel();
    request.response_tx = Some(response_tx);
    let (lifecycle_tx, mut lifecycle_rx) = mpsc::unbounded_channel();
    request.lifecycle_tx = Some(lifecycle_tx);
    let request_id_for_log = request.request_id.clone();
    let workspace_for_log = request.workspace.clone();
    let message_len_for_log = request.message.len();
    let codex_thread_id_for_goal = request.codex_thread_id.clone();
    let lifecycle_state = Arc::clone(&state);
    let lifecycle_request_id = request_id_for_log.clone();
    let lifecycle_task = tokio::spawn(async move {
        while let Some(event) = lifecycle_rx.recv().await {
            let mut state_guard = lifecycle_state.lock().await;
            if state_guard.active_request_id.as_deref() != Some(lifecycle_request_id.as_str()) {
                continue;
            }
            apply_lifecycle_event(&mut state_guard, event);
        }
    });
    let mut cleanup_guard = DialogCleanupGuard::new(
        Arc::clone(&state),
        request_id_for_log.clone(),
        workspace_for_log.clone(),
        lifecycle_task,
    );

    // 设置占用状态
    {
        let mut state_guard = state.lock().await;
        instance_debug_log(
            "[handle-dialog-before-enqueue]",
            format!(
                "request_id={}, workspace={:?}, was_busy={}, current_agent={:?}, busy_since={:?}, active_request_id={:?}, active_workspace={:?}",
                request_id_for_log,
                request.workspace,
                state_guard.is_busy,
                state_guard.current_agent,
                state_guard.busy_since,
                state_guard.active_request_id,
                state_guard.active_workspace
            ),
        );
        state_guard.is_busy = true;
        state_guard.current_agent = Some("ai".to_string());
        state_guard.busy_since = Some(chrono::Utc::now().to_rfc3339());
        state_guard.active_request_id = if request_id_for_log.is_empty() {
            None
        } else {
            Some(request_id_for_log.clone())
        };
        state_guard.active_workspace = if workspace_for_log.is_empty() {
            None
        } else {
            Some(workspace_for_log.clone())
        };
        state_guard.active_message_len = Some(message_len_for_log);
        state_guard.interaction_phase = InteractionPhase::Queued;
        state_guard.phase_since = state_guard.busy_since.clone();
        state_guard.active_serve_request_id = None;
        state_guard.ready_since = None;

        // 发送请求到处理队列
        if state_guard.request_tx.send(request).await.is_err() {
            instance_debug_log(
                "[handle-dialog-enqueue-failed]",
                format!("request_id={}, request_tx.send failed", request_id_for_log),
            );
            reset_active_interaction(&mut state_guard);
            cleanup_guard.disarm();
            return json_response(&DialogResponse {
                keep_going: false,
                error: Some("Server is shutting down".to_string()),
                ..Default::default()
            });
        }
    }

    // 等待响应
    let response = match response_rx.await {
        Ok(response) => response,
        Err(_) => {
            instance_debug_log(
                "[handle-dialog-response-channel-closed]",
                format!(
                    "request_id={}, workspace={:?}, message_len={}",
                    request_id_for_log, workspace_for_log, message_len_for_log
                ),
            );
            DialogResponse {
                keep_going: false,
                error: Some("Request was cancelled".to_string()),
                ..Default::default()
            }
        }
    };
    instance_debug_log(
        "[handle-dialog-response-received]",
        format!(
            "request_id={}, response_source={:?}, keep_going={}, user_input_len={}, error_present={}",
            request_id_for_log,
            response.response_source,
            response.keep_going,
            response.user_input.len(),
            response.error.is_some()
        ),
    );

    // 根据 popup 返回结果维护 loop 状态（文件持久化）
    let _ = log_loop_debug(&format!(
        "response received: response_source={:?} user_input_len={} scope={:?}",
        response.response_source,
        response.user_input.len(),
        scope
    ));
    {
        if let Some(scope_key) = scope.as_ref() {
            let mut sessions = read_loop_sessions();
            match response.response_source.as_str() {
                source if is_loop_start_source(source) => {
                    let loop_prompt = response.user_input.trim();
                    let _ = log_loop_debug(&format!(
                        "loop_start: source={:?} scope_key={:?} loop_prompt_len={}",
                        response.response_source,
                        scope_key,
                        loop_prompt.len()
                    ));
                    if !loop_prompt.is_empty() {
                        let active_serve_request_id_for_goal =
                            { state.lock().await.active_serve_request_id.clone() };
                        let mut request_ids_for_goal = vec![request_id_for_log.as_str()];
                        if let Some(serve_request_id) = active_serve_request_id_for_goal.as_deref()
                        {
                            request_ids_for_goal.push(serve_request_id);
                        }
                        let goal = resolve_strong_loop_goal_for_request_context(
                            &workspace_for_log,
                            &request_ids_for_goal,
                            codex_thread_id_for_goal.as_deref(),
                        );
                        let last_progress_signature = goal.as_ref().map(goal_progress_signature);
                        sessions.insert(
                            scope_key.clone(),
                            LoopSession {
                                loop_prompt: loop_prompt.to_string(),
                                last_ai_message: String::new(),
                                iteration_count: 0,
                                max_iterations: default_max_iterations(),
                                goal,
                                last_progress_signature,
                                stagnant_iterations: 0,
                            },
                        );
                        write_loop_sessions(&sessions);
                        let _ = log_loop_debug("loop session saved to file");
                    }
                }
                source if is_loop_stop_source(source) => {
                    sessions.remove(scope_key);
                    write_loop_sessions(&sessions);
                    let _ = log_loop_debug(
                        "loop session removed from file, returning keep_going=false",
                    );
                    let mut state_guard = state.lock().await;
                    reset_active_interaction(&mut state_guard);
                    cleanup_guard.disarm();
                    return json_response(&DialogResponse {
                        keep_going: false,
                        user_input: response.user_input.clone(),
                        response_source: response.response_source.clone(),
                        ..Default::default()
                    });
                }
                _ => {}
            }
        }

        let mut state_guard = state.lock().await;
        reset_active_interaction(&mut state_guard);
    }
    cleanup_guard.disarm();
    instance_debug_log(
        "[handle-dialog-finish]",
        format!("request_id={}, state released", request_id_for_log),
    );

    json_response(&response)
}

/// 启动 HTTP 服务器
pub async fn start_server(
    port: u16,
    request_tx: mpsc::Sender<DialogRequest>,
    workspace: Option<String>,
) -> anyhow::Result<()> {
    instance_debug_log(
        "[http-start-begin]",
        format!("port={}, workspace={:?}", port, workspace),
    );
    let state = Arc::new(Mutex::new(ServerState {
        request_tx,
        is_busy: false,
        current_agent: None,
        busy_since: None,
        active_request_id: None,
        active_workspace: None,
        active_message_len: None,
        interaction_phase: InteractionPhase::Idle,
        phase_since: None,
        active_serve_request_id: None,
        ready_since: None,
        runtime: RuntimeMetadata::collect(),
    }));

    // CORS 配置
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/status", get(status_check))
        .route(
            "/api/serve-response-route/renew",
            post(renew_serve_response_route),
        )
        .route("/api/dialog", post(handle_dialog))
        .layer(cors)
        .with_state(state);

    instance_debug_log(
        "[http-bind-begin]",
        format!("port={}, workspace={:?}", port, workspace),
    );
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    instance_debug_log(
        "[http-bind-success]",
        format!("port={}, workspace={:?}", port, workspace),
    );

    // 注册端口文件（包含项目路径用于映射）
    register_port(port, workspace.as_deref())?;
    instance_debug_log(
        "[http-register-port-success]",
        format!("port={}, workspace={:?}", port, workspace),
    );

    println!("Server listening on http://127.0.0.1:{}", port);

    match axum::serve(listener, app).await {
        Ok(()) => {
            instance_debug_log(
                "[http-serve-stop]",
                format!("port={}, workspace={:?}, result=ok", port, workspace),
            );
        }
        Err(error) => {
            instance_debug_log(
                "[http-serve-error]",
                format!("port={}, workspace={:?}, error={}", port, workspace, error),
            );
            return Err(error.into());
        }
    }

    // 清理端口文件
    unregister_port(port);
    instance_debug_log(
        "[http-unregister-port]",
        format!("port={}, workspace={:?}", port, workspace),
    );

    Ok(())
}

/// 注册端口文件（让脚本能发现服务器）
/// 文件内容为项目路径，用于端口↔项目映射
pub fn register_port(port: u16, workspace: Option<&str>) -> anyhow::Result<()> {
    let port_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join(".cunzhi_ports");

    std::fs::create_dir_all(&port_dir)?;
    // 写入项目路径，如果没有则写入空字符串
    let content = workspace.unwrap_or("");
    std::fs::write(port_dir.join(port.to_string()), content)?;

    Ok(())
}

/// 注销端口文件
fn unregister_port(port: u16) {
    if let Some(home) = dirs::home_dir() {
        let port_file = home.join(".cunzhi_ports").join(port.to_string());
        let _ = std::fs::remove_file(port_file);
    }
}

/// 查找可用端口
pub fn find_available_port(start: u16) -> u16 {
    for port in start..start + 100 {
        if std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok() {
            return port;
        }
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> ServerState {
        let (request_tx, _request_rx) = mpsc::channel(1);
        ServerState {
            request_tx,
            is_busy: true,
            current_agent: Some("ai".to_string()),
            busy_since: Some(chrono::Utc::now().to_rfc3339()),
            active_request_id: Some("parent-123".to_string()),
            active_workspace: Some("/tmp/workspace".to_string()),
            active_message_len: Some(3),
            interaction_phase: InteractionPhase::Queued,
            phase_since: Some(chrono::Utc::now().to_rfc3339()),
            active_serve_request_id: None,
            ready_since: None,
            runtime: RuntimeMetadata::collect(),
        }
    }

    #[test]
    fn lifecycle_event_tracks_serve_request_and_ready_since() {
        let mut state = test_state();
        apply_lifecycle_event(
            &mut state,
            InteractionLifecycleEvent::new(
                InteractionPhase::StartingGui,
                Some("serve-123".to_string()),
            ),
        );
        assert_eq!(state.interaction_phase, InteractionPhase::StartingGui);
        assert_eq!(state.active_serve_request_id.as_deref(), Some("serve-123"));
        assert!(state.ready_since.is_none());

        apply_lifecycle_event(
            &mut state,
            InteractionLifecycleEvent::new(
                InteractionPhase::WaitingUser,
                Some("serve-123".to_string()),
            ),
        );
        assert_eq!(state.interaction_phase, InteractionPhase::WaitingUser);
        assert!(state.ready_since.is_some());
    }

    #[test]
    fn reset_active_interaction_returns_state_to_idle() {
        let mut state = test_state();
        apply_lifecycle_event(
            &mut state,
            InteractionLifecycleEvent::new(
                InteractionPhase::WaitingUser,
                Some("serve-123".to_string()),
            ),
        );
        reset_active_interaction(&mut state);
        assert!(!state.is_busy);
        assert_eq!(state.interaction_phase, InteractionPhase::Idle);
        assert!(state.active_request_id.is_none());
        assert!(state.active_serve_request_id.is_none());
        assert!(state.ready_since.is_none());
    }

    #[test]
    fn active_interaction_match_uses_request_id_or_empty_id_workspace() {
        let mut state = test_state();
        assert!(active_interaction_matches(
            &state,
            "parent-123",
            "/different/workspace"
        ));
        assert!(!active_interaction_matches(
            &state,
            "different-parent",
            "/tmp/workspace"
        ));

        state.active_request_id = None;
        assert!(active_interaction_matches(&state, "", "/tmp/workspace"));
        assert!(!active_interaction_matches(&state, "", "/tmp/other"));

        state.is_busy = false;
        assert!(!active_interaction_matches(&state, "", "/tmp/workspace"));
    }

    #[test]
    fn renew_serve_response_route_file_refreshes_timestamp_and_keeps_original() {
        let project = tempfile::tempdir().expect("project tempdir");
        let request_id = format!("serve-renew-test-{}", std::process::id());
        let route_file = serve_response_route_file(&request_id);
        let response_file =
            std::env::temp_dir().join(format!("iterate_response_{}.json", request_id));
        let old_created_at = Utc::now().timestamp() - SERVE_RESPONSE_ROUTE_TTL_SECS - 60;
        let route = ServeResponseRoute {
            request_id: request_id.clone(),
            project_path: project.path().display().to_string(),
            response_file: response_file.display().to_string(),
            created_at: old_created_at,
            original_created_at: None,
            renewed_at: None,
            renewed_by: None,
            renewal_count: None,
        };
        fs::write(&route_file, serde_json::to_string(&route).unwrap()).expect("write route");

        let response =
            renew_serve_response_route_file(&request_id, &project.path().display().to_string())
                .expect("renew route");
        assert!(response.ok);
        assert!(response.renewed);
        assert_eq!(response.original_created_at, Some(old_created_at));
        assert_eq!(response.renewal_count, Some(1));

        let renewed: ServeResponseRoute =
            serde_json::from_str(&fs::read_to_string(&route_file).unwrap()).unwrap();
        assert!(renewed.created_at > old_created_at);
        assert_eq!(renewed.original_created_at, Some(old_created_at));
        assert_eq!(renewed.renewed_by.as_deref(), Some("iterate-server"));
        assert_eq!(renewed.renewal_count, Some(1));

        let _ = fs::remove_file(route_file);
        let _ = fs::remove_file(response_file);
    }

    #[test]
    fn renew_serve_response_route_file_rejects_parent_dir_response_file() {
        let project = tempfile::tempdir().expect("project tempdir");
        let request_id = format!("serve-renew-parent-test-{}", std::process::id());
        let route_file = serve_response_route_file(&request_id);
        let response_file = std::env::temp_dir()
            .join(format!("iterate_response_parent_{}", request_id))
            .join("../../outside-response.json");
        let old_created_at = Utc::now().timestamp() - SERVE_RESPONSE_ROUTE_TTL_SECS - 60;
        let route = ServeResponseRoute {
            request_id: request_id.clone(),
            project_path: project.path().display().to_string(),
            response_file: response_file.display().to_string(),
            created_at: old_created_at,
            original_created_at: None,
            renewed_at: None,
            renewed_by: None,
            renewal_count: None,
        };
        fs::write(&route_file, serde_json::to_string(&route).unwrap()).expect("write route");

        let response =
            renew_serve_response_route_file(&request_id, &project.path().display().to_string())
                .expect_err("reject parent dir response file");

        assert!(!response.ok);
        assert!(!response.renewed);
        assert_eq!(response.reason, Some("response_file_outside_temp_dir"));

        let stored: ServeResponseRoute =
            serde_json::from_str(&fs::read_to_string(&route_file).unwrap()).unwrap();
        assert_eq!(stored.created_at, old_created_at);
        assert_eq!(stored.renewal_count, None);

        let _ = fs::remove_file(route_file);
    }
}
