use rusqlite::{params, Connection, OpenFlags};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const ENV_ENABLED: &str = "ITERATE_CODEX_GOAL_OBSERVER";
const ENV_PROJECT: &str = "ITERATE_CODEX_GOAL_PROJECT";
const ENV_CODEX_HOME: &str = "ITERATE_CODEX_GOAL_HOME";
const ENV_INTERVAL_MS: &str = "ITERATE_CODEX_GOAL_INTERVAL_MS";
const DEFAULT_INTERVAL_MS: u64 = 1500;
const DEFAULT_LIMIT: i64 = 20;
const PROBE_EVERY_TICKS: u64 = 20;

#[derive(Debug, Clone)]
struct CodexGoalObserverConfig {
    project_path: String,
    codex_home: PathBuf,
    interval_ms: u64,
    limit: i64,
}

#[derive(Debug)]
struct CodexGoalSnapshot {
    total_goal_count: i64,
    project_goals: Vec<CodexGoalRow>,
}

#[derive(Debug, Clone)]
struct CodexGoalRow {
    thread_id: String,
    goal_id: String,
    objective: String,
    status: String,
    token_budget: Option<i64>,
    tokens_used: i64,
    time_used_seconds: i64,
    created_at_ms: i64,
    updated_at_ms: i64,
    cwd: Option<String>,
    thread_title: Option<String>,
    cli_version: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct SeenGoal {
    updated_at_ms: i64,
    status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GoalDecision {
    action: &'static str,
    reason: &'static str,
    previous_status: Option<&'static str>,
    previous_updated_at_ms: Option<i64>,
}

pub fn start_codex_goal_observer(app: tauri::AppHandle) {
    let Some(config) = CodexGoalObserverConfig::from_env() else {
        return;
    };

    emit_observer_log(
        "start",
        json!({
            "project_path": config.project_path.as_str(),
            "codex_home": config.codex_home.display().to_string(),
            "interval_ms": config.interval_ms,
            "limit": config.limit,
            "mode": "live_goal_adapter",
            "writes_live_goal": true,
            "writes_baseline_live_goal": false,
        }),
    );

    tauri::async_runtime::spawn(async move {
        run_codex_goal_observer(app, config).await;
    });
}

impl CodexGoalObserverConfig {
    fn from_env() -> Option<Self> {
        if env_is_disabled(ENV_ENABLED) {
            emit_observer_log(
                "disabled",
                json!({
                    "env": ENV_ENABLED,
                    "value": std::env::var(ENV_ENABLED).ok(),
                }),
            );
            return None;
        }

        let Some(project_path) = env_string(ENV_PROJECT)
            .or_else(project_path_from_args)
            .or_else(project_path_from_current_dir)
            .map(normalize_project_path)
        else {
            emit_observer_log(
                "skipped",
                json!({
                    "reason": "missing_project_scope",
                    "required_env": ENV_PROJECT,
                    "accepted_arg": "--workspace <path>",
                    "fallback": "current_dir",
                    "env_codex_home": std::env::var("CODEX_HOME").ok(),
                    "note": "observer does not scan all Codex projects by default",
                }),
            );
            return None;
        };

        let codex_home = env_string(ENV_CODEX_HOME)
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
            .unwrap_or_else(|| PathBuf::from(".codex"));

        Some(Self {
            project_path,
            codex_home,
            interval_ms: parse_env_u64(ENV_INTERVAL_MS, DEFAULT_INTERVAL_MS),
            limit: DEFAULT_LIMIT,
        })
    }

    fn goals_db_path(&self) -> PathBuf {
        codex_sqlite_db_path(&self.codex_home, "goals_1.sqlite")
    }

    fn state_db_path(&self) -> PathBuf {
        codex_sqlite_db_path(&self.codex_home, "state_5.sqlite")
    }
}

fn codex_sqlite_db_path(codex_home: &Path, file_name: &str) -> PathBuf {
    let sqlite_path = codex_home.join("sqlite").join(file_name);
    if sqlite_path.exists() {
        sqlite_path
    } else {
        codex_home.join(file_name)
    }
}

async fn run_codex_goal_observer(app: tauri::AppHandle, config: CodexGoalObserverConfig) {
    let mut seen: HashMap<String, SeenGoal> = HashMap::new();
    let mut tick: u64 = 0;
    let mut baseline = true;

    loop {
        let emit_probe = baseline || tick % PROBE_EVERY_TICKS == 0;
        let config_for_query = config.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            read_codex_goal_snapshot(&config_for_query)
        })
        .await
        {
            Ok(Ok(snapshot)) => {
                handle_snapshot(&app, &config, snapshot, &mut seen, baseline, emit_probe);
            }
            Ok(Err(error)) => {
                emit_observer_log(
                    "error",
                    json!({
                        "reason": error.to_string(),
                        "project_path": config.project_path.as_str(),
                        "codex_home": config.codex_home.display().to_string(),
                        "goals_db_path": config.goals_db_path().display().to_string(),
                        "state_db_path": config.state_db_path().display().to_string(),
                    }),
                );
            }
            Err(error) => {
                emit_observer_log(
                    "error",
                    json!({
                        "reason": format!("observer task join failed: {error}"),
                        "project_path": config.project_path.as_str(),
                    }),
                );
            }
        }

        baseline = false;
        tick = tick.saturating_add(1);
        tokio::time::sleep(Duration::from_millis(config.interval_ms)).await;
    }
}

fn read_codex_goal_snapshot(config: &CodexGoalObserverConfig) -> anyhow::Result<CodexGoalSnapshot> {
    let goals_db_path = config.goals_db_path();
    let state_db_path = config.state_db_path();

    if !goals_db_path.exists() {
        anyhow::bail!("goals DB missing: {}", goals_db_path.display());
    }
    if !state_db_path.exists() {
        anyhow::bail!("state DB missing: {}", state_db_path.display());
    }

    let conn = Connection::open_with_flags(
        goals_db_path.as_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let state_uri = sqlite_readonly_uri(state_db_path.as_path());
    conn.execute(
        &format!(
            "ATTACH DATABASE {} AS state",
            sql_string_literal(state_uri.as_str())
        ),
        [],
    )?;

    let total_goal_count =
        conn.query_row("SELECT COUNT(*) FROM thread_goals", [], |row| row.get(0))?;

    let mut stmt = conn.prepare(
        "SELECT
            g.thread_id,
            g.goal_id,
            g.objective,
            g.status,
            g.token_budget,
            g.tokens_used,
            g.time_used_seconds,
            g.created_at_ms,
            g.updated_at_ms,
            t.cwd,
            t.title,
            t.cli_version
         FROM thread_goals g
         LEFT JOIN state.threads t ON t.id = g.thread_id
         WHERE t.cwd = ?1
         ORDER BY g.updated_at_ms DESC
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(params![config.project_path.as_str(), config.limit], |row| {
            Ok(CodexGoalRow {
                thread_id: row.get(0)?,
                goal_id: row.get(1)?,
                objective: row.get(2)?,
                status: row.get(3)?,
                token_budget: row.get(4)?,
                tokens_used: row.get(5)?,
                time_used_seconds: row.get(6)?,
                created_at_ms: row.get(7)?,
                updated_at_ms: row.get(8)?,
                cwd: row.get(9)?,
                thread_title: row.get(10)?,
                cli_version: row.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CodexGoalSnapshot {
        total_goal_count,
        project_goals: rows,
    })
}

fn handle_snapshot(
    app: &tauri::AppHandle,
    config: &CodexGoalObserverConfig,
    snapshot: CodexGoalSnapshot,
    seen: &mut HashMap<String, SeenGoal>,
    baseline: bool,
    emit_probe: bool,
) {
    if emit_probe {
        emit_observer_log(
            "probe",
            json!({
                "project_path": config.project_path.as_str(),
                "codex_home": config.codex_home.display().to_string(),
                "goals_db_path": config.goals_db_path().display().to_string(),
                "state_db_path": config.state_db_path().display().to_string(),
                "thread_goals_count": snapshot.total_goal_count,
                "project_goal_count": snapshot.project_goals.len(),
                "latest_thread_cli_version": snapshot
                    .project_goals
                    .iter()
                    .find_map(|goal| goal.cli_version.as_deref()),
                "mode": "live_goal_adapter",
            }),
        );
    }

    if snapshot.project_goals.is_empty() && baseline {
        emit_observer_log(
            "decision",
            json!({
                "action": "skip",
                "reason": "no_project_goals",
                "project_path": config.project_path.as_str(),
            }),
        );
        return;
    }

    for goal in snapshot.project_goals {
        let key = goal_key(&goal);
        let previous = seen.get(key.as_str()).copied();
        let decision = decide_goal(goal.status.as_str(), goal.updated_at_ms, previous, baseline);
        seen.insert(
            key,
            SeenGoal {
                status: intern_status(goal.status.as_str()),
                updated_at_ms: goal.updated_at_ms,
            },
        );

        if decision.action == "unchanged" {
            continue;
        }

        emit_observer_log("candidate", candidate_payload(&goal, &config.project_path));
        emit_observer_log(
            "decision",
            json!({
                "action": decision.action,
                "reason": decision.reason,
                "thread_id": goal.thread_id,
                "goal_id": goal.goal_id,
                "status": goal.status,
                "mapped_live_goal_status": map_codex_status_to_live_goal_status(goal.status.as_str()),
                "updated_at_ms": goal.updated_at_ms,
                "updated_at": ms_to_rfc3339(goal.updated_at_ms),
                "previous_status": decision.previous_status,
                "previous_updated_at_ms": decision.previous_updated_at_ms,
                "project_path": config.project_path.as_str(),
            }),
        );

        if should_apply_live_goal_decision(decision.action) {
            apply_goal_to_live_goal(app, &config.project_path, &goal, decision.action);
        }
    }
}

fn should_apply_live_goal_decision(action: &str) -> bool {
    matches!(action, "start" | "update" | "status_change")
}

fn apply_goal_to_live_goal(
    app: &tauri::AppHandle,
    project_path: &str,
    goal: &CodexGoalRow,
    decision_action: &str,
) {
    let update = crate::ui::live_goal::CodexGoalObserverUpdate {
        goal_id: goal.goal_id.clone(),
        title: goal.objective.clone(),
        status: map_codex_status_to_live_goal_status(goal.status.as_str()).to_string(),
        project_path: project_path.to_string(),
        codex_thread_id: goal.thread_id.clone(),
        token_budget: i64_to_u64(goal.token_budget),
        tokens_used: i64_to_u64(Some(goal.tokens_used)),
        time_used_seconds: i64_to_u64(Some(goal.time_used_seconds)),
        created_at_ms: goal.created_at_ms,
        updated_at_ms: goal.updated_at_ms,
    };

    match crate::ui::live_goal::apply_codex_goal_observer_update(app, update) {
        Ok(Some(snapshot)) => emit_observer_log(
            "apply",
            json!({
                "applied": true,
                "decision_action": decision_action,
                "goal_id": goal.goal_id.as_str(),
                "thread_id": goal.thread_id.as_str(),
                "live_goal_status": snapshot.status,
                "live_goal_source": snapshot.source,
                "project_path": project_path,
            }),
        ),
        Ok(None) => emit_observer_log(
            "apply",
            json!({
                "applied": false,
                "decision_action": decision_action,
                "goal_id": goal.goal_id.as_str(),
                "thread_id": goal.thread_id.as_str(),
                "reason": "live_goal_guard_skipped",
                "project_path": project_path,
            }),
        ),
        Err(error) => emit_observer_log(
            "apply_error",
            json!({
                "decision_action": decision_action,
                "goal_id": goal.goal_id.as_str(),
                "thread_id": goal.thread_id.as_str(),
                "error": error,
                "project_path": project_path,
            }),
        ),
    }
}

fn candidate_payload(goal: &CodexGoalRow, project_path: &str) -> serde_json::Value {
    json!({
        "thread_id": goal.thread_id,
        "goal_id": goal.goal_id,
        "status": goal.status,
        "mapped_live_goal_status": map_codex_status_to_live_goal_status(goal.status.as_str()),
        "objective_preview": preview_objective(goal.objective.as_str()),
        "token_budget": goal.token_budget,
        "tokens_used": goal.tokens_used,
        "time_used_seconds": goal.time_used_seconds,
        "created_at_ms": goal.created_at_ms,
        "created_at": ms_to_rfc3339(goal.created_at_ms),
        "updated_at_ms": goal.updated_at_ms,
        "updated_at": ms_to_rfc3339(goal.updated_at_ms),
        "cwd": goal.cwd.as_deref(),
        "project_match": goal.cwd.as_deref() == Some(project_path),
        "thread_title": goal.thread_title.as_deref(),
        "cli_version": goal.cli_version.as_deref(),
    })
}

fn decide_goal(
    status: &str,
    updated_at_ms: i64,
    previous: Option<SeenGoal>,
    baseline: bool,
) -> GoalDecision {
    let status = intern_status(status);
    let Some(previous) = previous else {
        return GoalDecision {
            action: if baseline { "baseline" } else { "start" },
            reason: if baseline {
                "initial_watch_snapshot"
            } else {
                "new_goal_seen"
            },
            previous_status: None,
            previous_updated_at_ms: None,
        };
    };

    if updated_at_ms > previous.updated_at_ms {
        return GoalDecision {
            action: if status == previous.status {
                "update"
            } else {
                "status_change"
            },
            reason: if status == previous.status {
                "updated_at_advanced"
            } else {
                "status_changed"
            },
            previous_status: Some(previous.status),
            previous_updated_at_ms: Some(previous.updated_at_ms),
        };
    }

    if status != previous.status {
        return GoalDecision {
            action: "status_change",
            reason: "status_changed_without_updated_at_advance",
            previous_status: Some(previous.status),
            previous_updated_at_ms: Some(previous.updated_at_ms),
        };
    }

    GoalDecision {
        action: "unchanged",
        reason: "no_change",
        previous_status: Some(previous.status),
        previous_updated_at_ms: Some(previous.updated_at_ms),
    }
}

fn map_codex_status_to_live_goal_status(status: &str) -> &'static str {
    match status {
        "active" => "running",
        "paused" => "paused",
        "blocked" | "usage_limited" | "budget_limited" => "blocked",
        "complete" => "completed",
        _ => "unknown",
    }
}

fn goal_key(goal: &CodexGoalRow) -> String {
    format!("{}:{}", goal.thread_id, goal.goal_id)
}

fn intern_status(status: &str) -> &'static str {
    match status {
        "active" => "active",
        "paused" => "paused",
        "blocked" => "blocked",
        "usage_limited" => "usage_limited",
        "budget_limited" => "budget_limited",
        "complete" => "complete",
        _ => "unknown",
    }
}

fn i64_to_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn preview_objective(objective: &str) -> String {
    objective
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(96)
        .collect()
}

fn ms_to_rfc3339(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|date| date.to_rfc3339())
}

fn normalize_project_path(value: String) -> String {
    let path = PathBuf::from(value.trim());
    path.canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn sqlite_readonly_uri(path: &Path) -> String {
    format!("file:{}?mode=ro", path.to_string_lossy())
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn env_string(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn project_path_from_args() -> Option<String> {
    project_path_from_arg_values(std::env::args().skip(1))
}

fn project_path_from_current_dir() -> Option<String> {
    std::env::current_dir()
        .ok()
        .and_then(|path| path.to_str().map(|value| value.to_string()))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn project_path_from_arg_values<I>(args: I) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--workspace" {
            return args
                .next()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
        }

        if let Some(value) = arg.strip_prefix("--workspace=") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    None
}

fn parse_env_u64(key: &str, fallback: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_is_disabled(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(false)
}

fn emit_observer_log(event: &str, payload: serde_json::Value) {
    let entry = json!({
        "event": event,
        "observed_at_ms": chrono::Utc::now().timestamp_millis(),
        "payload": payload,
    });
    log::info!("[CodexGoalObserver] {}", entry);
    crate::utils::append_timeline_debug_log(
        format!("rust/codex_goal_observer::{event}").as_str(),
        entry,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn maps_codex_status_to_live_goal_status() {
        assert_eq!(map_codex_status_to_live_goal_status("active"), "running");
        assert_eq!(map_codex_status_to_live_goal_status("paused"), "paused");
        assert_eq!(map_codex_status_to_live_goal_status("blocked"), "blocked");
        assert_eq!(
            map_codex_status_to_live_goal_status("usage_limited"),
            "blocked"
        );
        assert_eq!(
            map_codex_status_to_live_goal_status("budget_limited"),
            "blocked"
        );
        assert_eq!(
            map_codex_status_to_live_goal_status("complete"),
            "completed"
        );
        assert_eq!(map_codex_status_to_live_goal_status("weird"), "unknown");
    }

    #[test]
    fn decides_baseline_start_update_and_status_change() {
        let baseline = decide_goal("active", 100, None, true);
        assert_eq!(baseline.action, "baseline");

        let start = decide_goal("active", 100, None, false);
        assert_eq!(start.action, "start");

        let update = decide_goal(
            "active",
            200,
            Some(SeenGoal {
                status: "active",
                updated_at_ms: 100,
            }),
            false,
        );
        assert_eq!(update.action, "update");

        let status_change = decide_goal(
            "complete",
            300,
            Some(SeenGoal {
                status: "active",
                updated_at_ms: 100,
            }),
            false,
        );
        assert_eq!(status_change.action, "status_change");
    }

    #[test]
    fn unchanged_decision_does_not_emit() {
        let decision = decide_goal(
            "active",
            100,
            Some(SeenGoal {
                status: "active",
                updated_at_ms: 100,
            }),
            false,
        );
        assert_eq!(decision.action, "unchanged");
    }

    #[test]
    fn does_not_apply_baseline_decision_to_live_goal() {
        assert!(!should_apply_live_goal_decision("baseline"));
        assert!(should_apply_live_goal_decision("start"));
        assert!(should_apply_live_goal_decision("update"));
        assert!(should_apply_live_goal_decision("status_change"));
    }

    #[test]
    fn parses_workspace_from_args() {
        assert_eq!(
            project_path_from_arg_values(vec![
                "--serve".to_string(),
                "--workspace".to_string(),
                "/Users/example/cunzhi".to_string(),
            ]),
            Some("/Users/example/cunzhi".to_string())
        );
        assert_eq!(
            project_path_from_arg_values(vec![
                "--serve".to_string(),
                "--workspace=/Users/example/cunzhi".to_string(),
            ]),
            Some("/Users/example/cunzhi".to_string())
        );
        assert_eq!(project_path_from_arg_values(Vec::<String>::new()), None);
    }

    #[test]
    fn reads_workspace_from_current_dir() {
        let current_dir = project_path_from_current_dir().expect("current dir should be available");
        assert!(!current_dir.trim().is_empty());
        assert!(Path::new(&current_dir).is_absolute());
    }

    #[test]
    fn codex_sqlite_db_path_prefers_sqlite_subdir() {
        let home = tempdir().expect("temp codex home");
        let old_path = home.path().join("state_5.sqlite");
        let sqlite_path = home.path().join("sqlite").join("state_5.sqlite");
        std::fs::write(&old_path, "").expect("write old state db");
        std::fs::create_dir_all(sqlite_path.parent().unwrap()).expect("create sqlite dir");
        std::fs::write(&sqlite_path, "").expect("write sqlite state db");

        assert_eq!(
            codex_sqlite_db_path(home.path(), "state_5.sqlite"),
            sqlite_path
        );
    }
}
