pub mod auto_commit;
pub mod commands;
pub mod git_ops;
pub mod links;

use crate::mcp::tools::interaction::{append_conversation_log, ConversationEntry};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CheckpointMetadata {
    pub checkpoint_id: String,
    pub commit_hash: String,
    pub commit_subject: String,
    pub push_status: String,
}

/// 自动检查点总开关。读取磁盘配置，关闭时所有自动 checkpoint 行为（zhi 自动提交 +
/// 后台文件监控）都不执行。每次读盘以保证独立 MCP 进程能及时感知 GUI 端的开关变更。
fn auto_checkpoint_enabled() -> bool {
    crate::config::load_standalone_config()
        .map(|config| config.checkpoint_config.auto_checkpoint_enabled)
        .unwrap_or(true)
}

pub(crate) fn is_checkpoint_index_path(path: &str) -> bool {
    path == ".cunzhi-knowledge"
        || path.starts_with(".cunzhi-knowledge/")
        || path == ".cunzhi-memory"
        || path.starts_with(".cunzhi-memory/")
}

#[derive(Debug, Clone)]
struct MonitorEntry {
    last_status: String,
    last_change_at: Instant,
    last_seen_at: Instant,
    last_request_id: Option<String>,
}

static AUTO_CHECKPOINT_MONITOR: Lazy<Mutex<HashMap<String, MonitorEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static AUTO_CHECKPOINT_MONITOR_STARTED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
pub(crate) static STANDALONE_CONFIG_ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn monitor_poll_interval() -> Duration {
    std::env::var("ITERATE_CHECKPOINT_MONITOR_POLL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(2))
}

fn monitor_debounce_window() -> Duration {
    std::env::var("ITERATE_CHECKPOINT_MONITOR_DEBOUNCE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(5))
}

fn monitor_ttl() -> Duration {
    Duration::from_secs(60 * 30)
}

fn git_status_porcelain(project_path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(project_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    Some(filter_monitor_relevant_status(&raw))
}

fn filter_monitor_relevant_status(raw: &str) -> String {
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                return None;
            }

            let path = trimmed.get(3..).unwrap_or("").trim();
            if is_checkpoint_index_path(path) {
                return None;
            }

            Some(trimmed.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn log_monitor_checkpoint(
    project_path: &str,
    request_id: Option<&str>,
    checkpoint: &CheckpointMetadata,
) {
    let entry = ConversationEntry {
        conversation_id: None,
        current_node_id: None,
        timeline_route_id: None,
        run_id: None,
        generation: None,
        stale_of: None,
        superseded_by: None,
        ai_message: "后台自动检查点：检测到稳定改动，已创建工作区 checkpoint。".to_string(),
        user_response: String::new(),
        project_path: Some(project_path.to_string()),
        image_count: 0,
        file_paths: vec![],
        image_paths: vec![],
        selected_options: vec![],
        request_id: request_id.map(ToOwned::to_owned),
        checkpoint_id: Some(checkpoint.checkpoint_id.clone()),
        checkpoint_commit: Some(checkpoint.commit_hash.clone()),
        push_status: Some(checkpoint.push_status.clone()),
        response_source: Some("checkpoint_monitor".to_string()),
        workspace_checkpoint_message: Some(checkpoint.commit_subject.clone()),
    };

    append_conversation_log(&entry);
}

fn evaluate_monitor_cycle(project_path: &str, entry: &mut MonitorEntry, debounce: Duration) {
    evaluate_monitor_cycle_with_checkpoint(project_path, entry, debounce, maybe_auto_checkpoint);
}

fn evaluate_monitor_cycle_with_checkpoint<F>(
    project_path: &str,
    entry: &mut MonitorEntry,
    debounce: Duration,
    mut create_checkpoint: F,
) where
    F: FnMut(&str, Option<&str>) -> Option<CheckpointMetadata>,
{
    let Some(status) = git_status_porcelain(project_path) else {
        return;
    };

    if status != entry.last_status {
        entry.last_status = status;
        entry.last_change_at = Instant::now();
        return;
    }

    if entry.last_status.trim().is_empty() || entry.last_change_at.elapsed() < debounce {
        return;
    }

    let request_id = entry.last_request_id.as_deref();
    if let Some(checkpoint) = create_checkpoint(project_path, request_id) {
        log_monitor_checkpoint(project_path, request_id, &checkpoint);
    }
    entry.last_status.clear();
    entry.last_change_at = Instant::now();
}

fn start_auto_checkpoint_monitor_if_needed() {
    if AUTO_CHECKPOINT_MONITOR_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    thread::spawn(|| loop {
        let poll_interval = monitor_poll_interval();
        let debounce = monitor_debounce_window();
        let ttl = monitor_ttl();
        let project_paths: Vec<String> = {
            let mut guard = AUTO_CHECKPOINT_MONITOR
                .lock()
                .expect("auto checkpoint monitor lock poisoned");
            guard.retain(|_, entry| entry.last_seen_at.elapsed() <= ttl);
            guard.keys().cloned().collect()
        };

        for project_path in project_paths {
            let mut guard = AUTO_CHECKPOINT_MONITOR
                .lock()
                .expect("auto checkpoint monitor lock poisoned");
            if let Some(entry) = guard.get_mut(&project_path) {
                evaluate_monitor_cycle(&project_path, entry, debounce);
            }
        }

        thread::sleep(poll_interval);
    });
}

pub fn touch_auto_checkpoint_monitor(project_path: &str, request_id: Option<&str>) {
    if project_path.trim().is_empty() {
        return;
    }

    if !auto_checkpoint_enabled() {
        return;
    }

    start_auto_checkpoint_monitor_if_needed();

    let mut guard = AUTO_CHECKPOINT_MONITOR
        .lock()
        .expect("auto checkpoint monitor lock poisoned");
    let now = Instant::now();
    let entry = guard
        .entry(project_path.to_string())
        .or_insert_with(|| MonitorEntry {
            last_status: String::new(),
            last_change_at: now,
            last_seen_at: now,
            last_request_id: None,
        });
    entry.last_seen_at = now;
    if let Some(request_id) = request_id.map(str::trim).filter(|value| !value.is_empty()) {
        entry.last_request_id = Some(request_id.to_string());
    }
}

/// 统一的 commit 型自动 checkpoint 入口。
///
/// 所有上层入口（standalone MCP / CLI bridge / full zhi）都应优先走这里，
/// 避免各自维护一套不同的自动保存语义。成功时返回与 `git log -1 --pretty=%s` 一致的完整 subject。
pub fn maybe_auto_checkpoint(
    project_path: &str,
    request_id: Option<&str>,
) -> Option<CheckpointMetadata> {
    if !auto_checkpoint_enabled() {
        return None;
    }
    match auto_commit::auto_create_checkpoint(project_path, request_id) {
        Ok(msg) => msg,
        Err(err) => {
            eprintln!("[Checkpoint] 自动创建检查点失败: {}", err);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        filter_monitor_relevant_status, maybe_auto_checkpoint, STANDALONE_CONFIG_ENV_LOCK,
    };
    use crate::config::{save_standalone_config, AppConfig};
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    struct EnvRestore {
        home: Option<OsString>,
        xdg_config_home: Option<OsString>,
        iterate_config_dir: Option<OsString>,
    }

    impl EnvRestore {
        fn capture() -> Self {
            Self {
                home: std::env::var_os("HOME"),
                xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
                iterate_config_dir: std::env::var_os("ITERATE_CONFIG_DIR"),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            if let Some(home) = self.home.as_ref() {
                std::env::set_var("HOME", home);
            } else {
                std::env::remove_var("HOME");
            }

            if let Some(xdg_config_home) = self.xdg_config_home.as_ref() {
                std::env::set_var("XDG_CONFIG_HOME", xdg_config_home);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }

            if let Some(iterate_config_dir) = self.iterate_config_dir.as_ref() {
                std::env::set_var("ITERATE_CONFIG_DIR", iterate_config_dir);
            } else {
                std::env::remove_var("ITERATE_CONFIG_DIR");
            }
        }
    }

    fn run_git(repo: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git command should run")
    }

    #[test]
    fn monitor_ignores_knowledge_only_changes() {
        let raw = " M .cunzhi-knowledge\n";
        assert_eq!(filter_monitor_relevant_status(raw), "");
    }

    #[test]
    fn monitor_ignores_registry_only_changes() {
        let raw = " M .cunzhi-memory/checkpoints.jsonl\n";
        assert_eq!(filter_monitor_relevant_status(raw), "");
    }

    #[test]
    fn monitor_ignores_untracked_generated_paths() {
        let raw = "?? .cunzhi-knowledge/conversations/log.md\n?? .cunzhi-memory/checkpoints.jsonl\n?? .cunzhi-memory/checkpoint_links.jsonl\n?? .cunzhi-memory/app-workflow-runs/run/status.json\n";
        assert_eq!(filter_monitor_relevant_status(raw), "");
    }

    #[test]
    fn monitor_keeps_non_knowledge_changes() {
        let raw = " M .cunzhi-knowledge\n M .cunzhi-memory/checkpoints.jsonl\nM  src/main.rs\n";
        assert_eq!(filter_monitor_relevant_status(raw), "M  src/main.rs");
    }

    #[test]
    fn maybe_auto_checkpoint_respects_standalone_config_switch_in_temp_repo() {
        let _guard = STANDALONE_CONFIG_ENV_LOCK
            .lock()
            .expect("standalone config env lock should not be poisoned");
        let _restore = EnvRestore::capture();

        let root = std::env::temp_dir().join(format!(
            "cunzhi-auto-checkpoint-switch-sandbox-{}",
            std::process::id()
        ));
        if root.exists() {
            let _ = fs::remove_dir_all(&root);
        }
        let home = root.join("home");
        let repo = root.join("repo");
        fs::create_dir_all(&home).expect("temp home should exist");
        fs::create_dir_all(&repo).expect("temp repo should exist");
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("ITERATE_CONFIG_DIR", home.join(".config/cunzhi"));

        let mut config = AppConfig::default();
        config.checkpoint_config.auto_checkpoint_enabled = false;
        save_standalone_config(&config).expect("disabled config should be saved");

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").expect("seed file should be written");
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").expect("dirty file should be written");
        let disabled_checkpoint =
            maybe_auto_checkpoint(repo.to_str().expect("utf8 path"), Some("req_disabled"));
        assert!(disabled_checkpoint.is_none());

        let head = run_git(&repo, &["log", "-1", "--pretty=%s"]);
        let head_subject = String::from_utf8_lossy(&head.stdout);
        assert_eq!(head_subject.trim(), "seed");

        config.checkpoint_config.auto_checkpoint_enabled = true;
        save_standalone_config(&config).expect("enabled config should be saved");

        let enabled_checkpoint =
            maybe_auto_checkpoint(repo.to_str().expect("utf8 path"), Some("req_enabled"))
                .expect("enabled dirty repo should create checkpoint");
        assert!(enabled_checkpoint
            .commit_subject
            .contains("iterate-checkpoint:"));

        let head = run_git(&repo, &["log", "-1", "--pretty=%s"]);
        let head_subject = String::from_utf8_lossy(&head.stdout);
        assert_eq!(head_subject.trim(), enabled_checkpoint.commit_subject);

        let _ = fs::remove_dir_all(&root);
    }
}
