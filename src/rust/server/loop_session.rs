//! Loop session 共享模块 — 供 HTTP server 和 MCP 子进程模式复用

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopSession {
    pub loop_prompt: String,
    /// AI 上一轮发送的消息（用于 auto_continue 时提供上下文）
    #[serde(default)]
    pub last_ai_message: String,
    /// 当前迭代次数
    #[serde(default)]
    pub iteration_count: u32,
    /// 最大迭代次数（到了强制弹窗，默认 10）
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// 绑定的目标快照；None 时保持旧 prompt-loop 行为
    #[serde(default)]
    pub goal: Option<LoopGoalSnapshot>,
    /// 上一轮目标进展签名，用于检测空转
    #[serde(default)]
    pub last_progress_signature: Option<String>,
    /// 连续无进展轮次
    #[serde(default)]
    pub stagnant_iterations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoopGoalSnapshot {
    pub goal_id: String,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub status_text: Option<String>,
    #[serde(default)]
    pub progress_percent: Option<f64>,
    #[serde(default)]
    pub progress_label: Option<String>,
    #[serde(default)]
    pub project_path: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub codex_thread_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopGoalBinding {
    Strong,
    Weak,
    Stale,
}

pub fn default_max_iterations() -> u32 {
    10
}

fn normalize_context_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn is_terminal_goal_status(status: &str) -> bool {
    let trimmed = status.trim();
    let lower = trimmed.to_lowercase();
    matches!(
        lower.as_str(),
        "complete" | "completed" | "done" | "cancelled" | "canceled" | "failed"
    ) || contains_any(trimmed, &["已完成", "完成", "已取消", "取消", "失败"])
}

pub fn is_blocked_goal_status(status: &str) -> bool {
    let trimmed = status.trim();
    let lower = trimmed.to_lowercase();
    matches!(lower.as_str(), "blocked" | "stuck" | "stalled")
        || contains_any(trimmed, &["阻塞", "卡住", "停滞"])
}

pub fn goal_progress_signature(goal: &LoopGoalSnapshot) -> String {
    let progress = goal
        .progress_percent
        .map(|value| format!("{:.2}", value))
        .unwrap_or_default();
    [
        goal.goal_id.as_str(),
        goal.status.as_str(),
        goal.phase.as_deref().unwrap_or_default(),
        goal.status_text.as_deref().unwrap_or_default(),
        progress.as_str(),
        goal.progress_label.as_deref().unwrap_or_default(),
    ]
    .join("|")
}

pub fn classify_persistent_goal_binding(
    goal: &LoopGoalSnapshot,
    request_id: Option<&str>,
    codex_thread_id: Option<&str>,
) -> (LoopGoalBinding, &'static str) {
    let request_ids: Vec<&str> = request_id.into_iter().collect();
    classify_persistent_goal_binding_for_request_ids(goal, &request_ids, codex_thread_id)
}

pub fn classify_persistent_goal_binding_for_request_ids(
    goal: &LoopGoalSnapshot,
    request_ids: &[&str],
    codex_thread_id: Option<&str>,
) -> (LoopGoalBinding, &'static str) {
    if is_terminal_goal_status(&goal.status) {
        return (LoopGoalBinding::Stale, "terminal_goal");
    }

    let request_ids: Vec<String> = request_ids
        .iter()
        .filter_map(|request_id| normalize_context_id(Some(request_id)))
        .collect();
    let codex_thread_id = normalize_context_id(codex_thread_id);
    let goal_request_id = normalize_context_id(goal.request_id.as_deref());
    let goal_codex_thread_id = normalize_context_id(goal.codex_thread_id.as_deref());

    if let (Some(current), Some(goal_thread)) =
        (codex_thread_id.as_ref(), goal_codex_thread_id.as_ref())
    {
        if current == goal_thread {
            return (LoopGoalBinding::Strong, "codex_thread_id_match");
        }
    }

    if let Some(goal_request) = goal_request_id.as_ref() {
        if request_ids.iter().any(|current| current == goal_request) {
            return (LoopGoalBinding::Strong, "request_id_match");
        }
    }

    if let (Some(current), Some(goal_thread)) =
        (codex_thread_id.as_ref(), goal_codex_thread_id.as_ref())
    {
        if current != goal_thread {
            return (LoopGoalBinding::Stale, "codex_thread_id_mismatch");
        }
    }

    if goal_request_id.is_some() && !request_ids.is_empty() {
        return (LoopGoalBinding::Stale, "request_id_mismatch");
    }

    if goal
        .source
        .as_deref()
        .map(|source| source == "apns_live_activity_update")
        .unwrap_or(false)
    {
        return (LoopGoalBinding::Weak, "apns_progress_without_context");
    }

    (LoopGoalBinding::Weak, "missing_request_or_thread_match")
}

pub fn classify_loop_start_goal_binding(
    goal: &LoopGoalSnapshot,
    request_ids: &[&str],
    codex_thread_id: Option<&str>,
) -> (LoopGoalBinding, &'static str) {
    let (binding, reason) =
        classify_persistent_goal_binding_for_request_ids(goal, request_ids, codex_thread_id);
    match (binding, reason) {
        (LoopGoalBinding::Strong, _) => (binding, reason),
        (LoopGoalBinding::Stale, "terminal_goal" | "codex_thread_id_mismatch") => (binding, reason),
        _ => (LoopGoalBinding::Strong, "active_project_goal"),
    }
}

/// 文件持久化的 loop sessions——所有端口共享
pub fn loop_sessions_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".cunzhi")
        .join("loop_sessions.json")
}

pub fn read_loop_sessions() -> HashMap<String, LoopSession> {
    let path = loop_sessions_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

pub fn write_loop_sessions(sessions: &HashMap<String, LoopSession>) {
    let path = loop_sessions_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(sessions) {
        let _ = std::fs::write(&path, json);
    }
}

pub fn log_loop_debug(msg: &str) -> std::io::Result<()> {
    use std::io::Write;
    let path = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".cunzhi")
        .join("loop_debug.log");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(
        f,
        "[{}] {}",
        chrono::Local::now().format("%H:%M:%S%.3f"),
        msg
    )?;
    Ok(())
}

pub fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

pub fn truncate_for_log(value: &str, max_chars: usize) -> String {
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

pub fn is_exit_loop_message(message: &str) -> bool {
    let lower = message.to_lowercase();
    contains_any(
        &lower,
        &["exit loop", "stop loop", "exit the loop", "stop the loop"],
    ) || contains_any(message, &["退出循环", "停止循环", "结束循环", "关闭循环"])
}

pub fn is_loop_start_source(source: &str) -> bool {
    matches!(source.trim(), "popup_loop_start" | "web_bridge_loop_start")
}

pub fn is_loop_stop_source(source: &str) -> bool {
    matches!(
        source.trim(),
        "popup_loop_stop" | "web_bridge_loop_stop" | "explicit_conversation_end"
    )
}

pub fn is_completed_message(message: &str) -> bool {
    // 只在短消息（≤200字符）中检测完成信号，长报告不算完成
    let trimmed = message.trim();
    if trimmed.len() > 200 {
        return false;
    }
    let lower = trimmed.to_lowercase();
    contains_any(
        &lower,
        &[
            "done",
            "finished",
            "completed",
            "all tasks completed",
            "task completed",
        ],
    ) || contains_any(trimmed, &["已完成", "任务结束", "任务已结束"])
}

pub fn needs_user_attention_from_message(message: &str, has_options: bool) -> bool {
    if has_options {
        return true;
    }

    let message = message.trim();
    if message.is_empty() {
        return false;
    }

    let lower = message.to_lowercase();
    contains_any(
        &lower,
        &[
            "please provide",
            "please confirm",
            "please clarify",
            "need your input",
            "need your confirmation",
            "choose one",
        ],
    ) || contains_any(
        message,
        &[
            "请输入",
            "请选择",
            "请确认",
            "请提供",
            "请补充",
            "请明确",
            "请说明",
            "需要你",
            "需要您",
            "要不要",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_start_source_accepts_popup_and_web_bridge() {
        assert!(is_loop_start_source("popup_loop_start"));
        assert!(is_loop_start_source("web_bridge_loop_start"));
        assert!(is_loop_start_source(" web_bridge_loop_start "));
        assert!(!is_loop_start_source("popup_goal"));
        assert!(!is_loop_start_source("popup_goal_submit"));
        assert!(!is_loop_start_source("web_bridge_goal_submit"));
        assert!(!is_loop_start_source("web_bridge_continue"));
        assert!(!is_loop_start_source("popup_continue"));
    }

    #[test]
    fn loop_stop_source_accepts_popup_and_web_bridge() {
        assert!(is_loop_stop_source("popup_loop_stop"));
        assert!(is_loop_stop_source("web_bridge_loop_stop"));
        assert!(is_loop_stop_source("explicit_conversation_end"));
        assert!(!is_loop_stop_source("loop_auto_continue"));
    }

    fn test_goal() -> LoopGoalSnapshot {
        LoopGoalSnapshot {
            goal_id: "goal-1".to_string(),
            title: "test goal".to_string(),
            status: "running".to_string(),
            phase: Some("verify".to_string()),
            status_text: None,
            progress_percent: Some(37.0),
            progress_label: Some("37%".to_string()),
            project_path: Some("/tmp/project".to_string()),
            request_id: Some("req-1".to_string()),
            codex_thread_id: Some("thread-1".to_string()),
            source: Some("manual".to_string()),
        }
    }

    #[test]
    fn persistent_goal_binding_requires_request_or_thread_match() {
        let goal = test_goal();

        assert_eq!(
            classify_persistent_goal_binding(&goal, Some("req-1"), None),
            (LoopGoalBinding::Strong, "request_id_match")
        );
        assert_eq!(
            classify_persistent_goal_binding(&goal, None, Some("thread-1")),
            (LoopGoalBinding::Strong, "codex_thread_id_match")
        );
        assert_eq!(
            classify_persistent_goal_binding(&goal, Some("other-req"), None),
            (LoopGoalBinding::Stale, "request_id_mismatch")
        );
    }

    #[test]
    fn persistent_goal_binding_accepts_any_matching_request_id() {
        let goal = test_goal();

        assert_eq!(
            classify_persistent_goal_binding_for_request_ids(
                &goal,
                &["outer-request", "req-1"],
                None
            ),
            (LoopGoalBinding::Strong, "request_id_match")
        );
    }

    #[test]
    fn persistent_goal_binding_rejects_when_all_request_ids_mismatch() {
        let goal = test_goal();

        assert_eq!(
            classify_persistent_goal_binding_for_request_ids(
                &goal,
                &["outer-request", "serve-other"],
                None
            ),
            (LoopGoalBinding::Stale, "request_id_mismatch")
        );
    }

    #[test]
    fn loop_start_goal_binding_accepts_waiting_goal_with_drifted_request_id() {
        let mut goal = test_goal();
        goal.phase = Some("waiting_for_user".to_string());

        assert_eq!(
            classify_loop_start_goal_binding(&goal, &["outer-request", "serve-new"], None),
            (LoopGoalBinding::Strong, "active_project_goal")
        );
    }

    #[test]
    fn loop_start_goal_binding_accepts_running_goal_with_request_mismatch() {
        let goal = test_goal();

        assert_eq!(
            classify_loop_start_goal_binding(&goal, &["outer-request", "serve-new"], None),
            (LoopGoalBinding::Strong, "active_project_goal")
        );
    }

    #[test]
    fn loop_start_goal_binding_keeps_thread_mismatch_stale() {
        let mut goal = test_goal();
        goal.phase = Some("waiting_for_user".to_string());

        assert_eq!(
            classify_loop_start_goal_binding(
                &goal,
                &["outer-request", "serve-new"],
                Some("thread-other")
            ),
            (LoopGoalBinding::Stale, "codex_thread_id_mismatch")
        );
    }

    #[test]
    fn loop_start_goal_binding_keeps_terminal_goal_stale() {
        let mut goal = test_goal();
        goal.status = "completed".to_string();

        assert_eq!(
            classify_loop_start_goal_binding(&goal, &["outer-request", "serve-new"], None),
            (LoopGoalBinding::Stale, "terminal_goal")
        );
    }

    #[test]
    fn codex_thread_match_overrides_changed_request_id() {
        let goal = test_goal();

        assert_eq!(
            classify_persistent_goal_binding(&goal, Some("new-req"), Some("thread-1")),
            (LoopGoalBinding::Strong, "codex_thread_id_match")
        );
    }

    #[test]
    fn apns_goal_without_context_is_weak_not_strong() {
        let mut goal = test_goal();
        goal.request_id = None;
        goal.codex_thread_id = None;
        goal.source = Some("apns_live_activity_update".to_string());

        assert_eq!(
            classify_persistent_goal_binding(&goal, Some("req-1"), Some("thread-1")),
            (LoopGoalBinding::Weak, "apns_progress_without_context")
        );
    }

    #[test]
    fn terminal_goal_is_stale() {
        let mut goal = test_goal();
        goal.status = "completed".to_string();

        assert_eq!(
            classify_persistent_goal_binding(&goal, Some("req-1"), None),
            (LoopGoalBinding::Stale, "terminal_goal")
        );
    }
}
