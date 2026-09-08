use chrono::{Local, Utc};
use std::path::Path;
use std::process::Output;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

use super::{background_command, is_checkpoint_index_path, CheckpointMetadata};

const INDEX_LOCK_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(150),
    Duration::from_millis(500),
    Duration::from_millis(1_000),
];

fn generate_checkpoint_id() -> String {
    let ts = Utc::now().format("%Y%m%d%H%M%S");
    let suffix: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    format!("cp_{}_{}", ts, suffix)
}

fn maybe_auto_push(workspace: &str) -> String {
    let remote_check = background_command("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(workspace)
        .output();

    let Ok(remote_check) = remote_check else {
        return "push_check_failed".to_string();
    };

    if !remote_check.status.success() {
        return "not_configured".to_string();
    }

    let push_result = background_command("git")
        .args(["push", "origin", "HEAD", "--quiet"])
        .current_dir(workspace)
        .output();

    match push_result {
        Ok(output) if output.status.success() => "pushed".to_string(),
        Ok(_) => "push_failed".to_string(),
        Err(_) => "push_failed".to_string(),
    }
}

fn is_gitlink_path(workspace: &str, rel_path: &str) -> bool {
    let output = background_command("git")
        .args(["ls-files", "--stage", "--", rel_path])
        .current_dir(workspace)
        .output();

    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.starts_with("160000 ")),
        _ => false,
    }
}

fn push_exclude_pathspec(paths: &mut Vec<String>, workspace: &str, rel_path: &str) {
    paths.push(format!(":(exclude){}", rel_path));
    if !is_gitlink_path(workspace, rel_path) {
        paths.push(format!(":(exclude){}/**", rel_path));
    }
}

fn checkpoint_commit_paths(workspace: &str) -> Vec<String> {
    let mut paths = vec![".".to_string()];
    push_exclude_pathspec(&mut paths, workspace, ".cunzhi-knowledge");
    push_exclude_pathspec(&mut paths, workspace, ".cunzhi-memory");
    paths
}

fn output_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn is_index_lock_error(output: &Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr.contains("index.lock") || stderr.contains("Another git process seems to be running")
}

fn index_lock_failure(action: &str, output: &Output) -> String {
    format!(
        "{} 失败: Git index 被锁定（.git/index.lock）。请先确认没有活跃 git 进程；如果确认无占用且锁文件陈旧，再删除锁文件后重试。原始错误: {}",
        action,
        output_stderr(output)
    )
}

fn run_git_with_index_lock_retry(
    workspace: &str,
    args: &[&str],
    pathspecs: &[String],
    action: &str,
) -> Result<Output, String> {
    for attempt in 0..=INDEX_LOCK_RETRY_DELAYS.len() {
        let output = background_command("git")
            .args(args)
            .args(pathspecs)
            .current_dir(workspace)
            .output()
            .map_err(|e| format!("执行 {} 失败: {}", action, e))?;

        if output.status.success() || !is_index_lock_error(&output) {
            return Ok(output);
        }

        if let Some(delay) = INDEX_LOCK_RETRY_DELAYS.get(attempt) {
            thread::sleep(*delay);
        } else {
            return Err(index_lock_failure(action, &output));
        }
    }

    unreachable!("index lock retry loop should return");
}

fn filter_checkpoint_relevant_status(raw: &str) -> String {
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

fn create_checkpoint_with_subject(
    workspace: &str,
    subject_label: Option<&str>,
    request_id: Option<&str>,
) -> Result<Option<CheckpointMetadata>, String> {
    let git_dir = Path::new(workspace).join(".git");
    if !git_dir.exists() {
        return Ok(None);
    }

    let status = background_command("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(workspace)
        .output()
        .map_err(|e| format!("执行 git status 失败: {}", e))?;

    let filtered_status =
        filter_checkpoint_relevant_status(&String::from_utf8_lossy(&status.stdout));
    if filtered_status.trim().is_empty() {
        return Ok(None);
    }

    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let time_str = Local::now().format("%H:%M:%S").to_string();
    let checkpoint_id = generate_checkpoint_id();
    let subject_suffix = subject_label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("自动检查点 {}", time_str));
    let subject = format!("iterate-checkpoint:{} | {}", timestamp, subject_suffix);
    let mut body_lines = vec![format!("checkpoint_id: {}", checkpoint_id)];
    if let Some(request_id) = request_id.map(str::trim).filter(|value| !value.is_empty()) {
        body_lines.push(format!("turn_id: {}", request_id));
    }
    let body = body_lines.join("\n");
    let message = format!("{}\n\n{}", subject, body);

    let checkpoint_paths = checkpoint_commit_paths(workspace);
    let add_result = run_git_with_index_lock_retry(
        workspace,
        &["add", "-A", "--"],
        &checkpoint_paths,
        "git add",
    )?;

    if !add_result.status.success() {
        return Err(format!("git add 失败: {}", output_stderr(&add_result)));
    }

    let commit_result = run_git_with_index_lock_retry(
        workspace,
        &["commit", "-m", &message, "--"],
        &checkpoint_paths,
        "git commit",
    )?;

    if !commit_result.status.success() {
        return Err(format!(
            "git commit 失败: {}",
            output_stderr(&commit_result)
        ));
    }

    let hash_result = background_command("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace)
        .output()
        .map_err(|e| format!("执行 git rev-parse 失败: {}", e))?;

    if !hash_result.status.success() {
        let stderr = String::from_utf8_lossy(&hash_result.stderr);
        return Err(format!("git rev-parse 失败: {}", stderr.trim()));
    }

    let commit_hash = String::from_utf8_lossy(&hash_result.stdout)
        .trim()
        .to_string();
    let push_status = maybe_auto_push(workspace);

    Ok(Some(CheckpointMetadata {
        checkpoint_id,
        commit_hash,
        commit_subject: subject,
        push_status,
    }))
}

/// 自动创建 commit 型检查点。
///
/// 仅在目标目录是 Git 仓库且存在未提交更改时执行。
pub fn auto_create_checkpoint(
    workspace: &str,
    request_id: Option<&str>,
) -> Result<Option<CheckpointMetadata>, String> {
    create_checkpoint_with_subject(workspace, None, request_id)
}

/// 创建带自定义显示文案的 commit 型检查点。
pub fn create_named_checkpoint(
    workspace: &str,
    label: &str,
    request_id: Option<&str>,
) -> Result<Option<CheckpointMetadata>, String> {
    create_checkpoint_with_subject(workspace, Some(label), request_id)
}

#[cfg(test)]
mod tests {
    use super::{
        auto_create_checkpoint, checkpoint_commit_paths, create_named_checkpoint,
        filter_checkpoint_relevant_status,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;

    fn run_git(repo: &PathBuf, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git command should run")
    }

    fn run_git_in(path: &std::path::Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .expect("git command should run")
    }

    #[test]
    fn creates_iterate_checkpoint_commit_when_repo_is_dirty() {
        let repo =
            std::env::temp_dir().join(format!("cunzhi-auto-checkpoint-{}", std::process::id()));

        if repo.exists() {
            let _ = fs::remove_dir_all(&repo);
        }
        fs::create_dir_all(&repo).expect("temp repo dir should exist");

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

        let checkpoint =
            auto_create_checkpoint(repo.to_str().expect("utf8 path"), Some("req_test"))
                .expect("helper should succeed")
                .expect("dirty repo should create commit");

        assert!(checkpoint.commit_subject.contains("iterate-checkpoint:"));
        assert!(checkpoint.checkpoint_id.starts_with("cp_"));
        assert!(!checkpoint.commit_hash.is_empty());
        assert_eq!(checkpoint.push_status, "not_configured");

        let head = run_git(&repo, &["log", "-1", "--pretty=%s"]);
        let head_subject = String::from_utf8_lossy(&head.stdout);
        assert!(head_subject.contains("iterate-checkpoint:"));
        assert_eq!(head_subject.trim(), checkpoint.commit_subject);

        let head_body = run_git(&repo, &["log", "-1", "--pretty=%b"]);
        let head_body = String::from_utf8_lossy(&head_body.stdout);
        assert!(head_body.contains(&format!("checkpoint_id: {}", checkpoint.checkpoint_id)));
        assert!(head_body.contains("turn_id: req_test"));

        let clean = auto_create_checkpoint(repo.to_str().expect("utf8 path"), None)
            .expect("clean repo check should succeed");
        assert!(clean.is_none());

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn creates_checkpoint_and_pushes_when_origin_exists() {
        let base = std::env::temp_dir().join(format!("cunzhi-auto-push-{}", std::process::id()));
        let bare = base.join("origin.git");
        let repo = base.join("worktree");

        if base.exists() {
            let _ = fs::remove_dir_all(&base);
        }
        fs::create_dir_all(&base).expect("base dir should exist");

        assert!(
            run_git_in(&base, &["init", "--bare", bare.to_str().unwrap()])
                .status
                .success()
        );
        fs::create_dir_all(&repo).expect("repo dir should exist");
        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );
        assert!(
            run_git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").expect("seed file should be written");
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());
        assert!(run_git(&repo, &["push", "origin", "HEAD"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").expect("dirty file should be written");

        let checkpoint =
            auto_create_checkpoint(repo.to_str().expect("utf8 path"), Some("req_push"))
                .expect("helper should succeed")
                .expect("dirty repo should create commit");

        assert_eq!(checkpoint.push_status, "pushed");

        let remote_head = Command::new("git")
            .args([
                "--git-dir",
                bare.to_str().unwrap(),
                "log",
                "-1",
                "--pretty=%s",
            ])
            .output()
            .expect("git log should run");
        let remote_subject = String::from_utf8_lossy(&remote_head.stdout);
        assert_eq!(remote_subject.trim(), checkpoint.commit_subject);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn debounced_monitor_creates_checkpoint_after_stable_dirty_state() {
        let root =
            std::env::temp_dir().join(format!("cunzhi-monitor-checkpoint-{}", std::process::id()));
        let repo = root.join("repo");

        if root.exists() {
            let _ = fs::remove_dir_all(&root);
        }
        fs::create_dir_all(&repo).expect("temp repo dir should exist");

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

        let mut entry = super::super::MonitorEntry {
            last_status: String::new(),
            last_change_at: std::time::Instant::now(),
            last_seen_at: std::time::Instant::now(),
            last_request_id: Some("req_monitor".to_string()),
        };

        super::super::evaluate_monitor_cycle_with_checkpoint(
            repo.to_str().expect("utf8 path"),
            &mut entry,
            Duration::from_millis(0),
            |project_path, request_id| {
                auto_create_checkpoint(project_path, request_id)
                    .expect("monitor checkpoint helper should succeed")
            },
        );
        super::super::evaluate_monitor_cycle_with_checkpoint(
            repo.to_str().expect("utf8 path"),
            &mut entry,
            Duration::from_millis(0),
            |project_path, request_id| {
                auto_create_checkpoint(project_path, request_id)
                    .expect("monitor checkpoint helper should succeed")
            },
        );

        let head = run_git(&repo, &["log", "-1", "--pretty=%B"]);
        let head_body = String::from_utf8_lossy(&head.stdout);
        assert!(head_body.contains("checkpoint_id:"));
        assert!(head_body.contains("turn_id: req_monitor"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn creates_named_checkpoint_commit_when_message_is_provided() {
        let repo =
            std::env::temp_dir().join(format!("cunzhi-named-checkpoint-{}", std::process::id()));

        if repo.exists() {
            let _ = fs::remove_dir_all(&repo);
        }
        fs::create_dir_all(&repo).expect("temp repo dir should exist");

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

        let checkpoint =
            create_named_checkpoint(repo.to_str().expect("utf8 path"), "手动检查点", None)
                .expect("helper should succeed")
                .expect("dirty repo should create commit");

        assert!(checkpoint.commit_subject.contains("手动检查点"));

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn filter_checkpoint_status_ignores_generated_registry_files() {
        let raw = " M .cunzhi-knowledge\n M .cunzhi-memory/checkpoints.jsonl\n";
        assert_eq!(filter_checkpoint_relevant_status(raw), "");
    }

    #[test]
    fn filter_checkpoint_status_ignores_untracked_generated_paths() {
        let raw = "?? .cunzhi-knowledge/conversations/log.md\n?? .cunzhi-memory/checkpoints.jsonl\n?? .cunzhi-memory/checkpoint_links.jsonl\n?? .cunzhi-memory/app-workflow-runs/run/status.json\n";
        assert_eq!(filter_checkpoint_relevant_status(raw), "");
    }

    #[test]
    fn filter_checkpoint_status_keeps_real_workspace_changes() {
        let raw = " M .cunzhi-memory/checkpoints.jsonl\nM  src/main.rs\n";
        assert_eq!(filter_checkpoint_relevant_status(raw), "M  src/main.rs");
    }

    #[test]
    fn checkpoint_paths_do_not_recurse_into_gitlink_submodule() {
        let repo =
            std::env::temp_dir().join(format!("cunzhi-gitlink-pathspec-{}", std::process::id()));

        if repo.exists() {
            let _ = fs::remove_dir_all(&repo);
        }
        fs::create_dir_all(&repo).expect("temp repo dir should exist");

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
        let head = run_git(&repo, &["rev-parse", "HEAD"]);
        let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
        assert!(run_git(
            &repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{},.cunzhi-knowledge", head),
            ],
        )
        .status
        .success());
        assert!(run_git(&repo, &["commit", "-m", "add knowledge gitlink"])
            .status
            .success());

        let paths = checkpoint_commit_paths(repo.to_str().expect("utf8 path"));
        assert!(paths
            .iter()
            .any(|path| path == ":(exclude).cunzhi-knowledge"));
        assert!(!paths
            .iter()
            .any(|path| path == ":(exclude).cunzhi-knowledge/**"));

        fs::write(repo.join("demo.txt"), "v2\n").expect("dirty file should be written");
        auto_create_checkpoint(repo.to_str().expect("utf8 path"), Some("req_gitlink"))
            .expect("gitlink pathspec should not fail")
            .expect("real change should create checkpoint");

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn reports_index_lock_with_actionable_message() {
        let repo = std::env::temp_dir().join(format!("cunzhi-index-lock-{}", std::process::id()));

        if repo.exists() {
            let _ = fs::remove_dir_all(&repo);
        }
        fs::create_dir_all(&repo).expect("temp repo dir should exist");

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
        fs::write(repo.join(".git").join("index.lock"), "").expect("lock should be written");

        let err = auto_create_checkpoint(repo.to_str().expect("utf8 path"), Some("req_lock"))
            .expect_err("stale index lock should return an error");
        assert!(err.contains("Git index 被锁定"));
        assert!(err.contains(".git/index.lock"));

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn checkpoint_excludes_generated_paths_when_real_change_exists() {
        let repo =
            std::env::temp_dir().join(format!("cunzhi-generated-excluded-{}", std::process::id()));

        if repo.exists() {
            let _ = fs::remove_dir_all(&repo);
        }
        fs::create_dir_all(&repo).expect("temp repo dir should exist");

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
        fs::create_dir_all(repo.join(".cunzhi-memory")).expect("memory dir should exist");
        fs::write(
            repo.join(".cunzhi-memory").join("checkpoints.jsonl"),
            "{}\n",
        )
        .expect("registry file should be written");
        fs::write(
            repo.join(".cunzhi-memory").join("checkpoint_links.jsonl"),
            "{}\n",
        )
        .expect("checkpoint link file should be written");
        fs::create_dir_all(
            repo.join(".cunzhi-memory")
                .join("app-workflow-runs")
                .join("run"),
        )
        .expect("memory run dir should exist");
        fs::write(
            repo.join(".cunzhi-memory")
                .join("app-workflow-runs")
                .join("run")
                .join("status.json"),
            "{}\n",
        )
        .expect("memory run file should be written");
        fs::create_dir_all(repo.join(".cunzhi-knowledge").join("conversations"))
            .expect("knowledge dir should exist");
        fs::write(
            repo.join(".cunzhi-knowledge")
                .join("conversations")
                .join("log.md"),
            "generated\n",
        )
        .expect("knowledge log should be written");

        auto_create_checkpoint(repo.to_str().expect("utf8 path"), Some("req_generated"))
            .expect("helper should succeed")
            .expect("real change should create commit");

        let committed_files = run_git(&repo, &["show", "--name-only", "--pretty=format:", "HEAD"]);
        let committed_files = String::from_utf8_lossy(&committed_files.stdout);
        assert!(committed_files.contains("demo.txt"));
        assert!(!committed_files.contains(".cunzhi-memory/checkpoints.jsonl"));
        assert!(!committed_files.contains(".cunzhi-memory/checkpoint_links.jsonl"));
        assert!(!committed_files.contains(".cunzhi-memory/app-workflow-runs/run/status.json"));
        assert!(!committed_files.contains(".cunzhi-knowledge/conversations/log.md"));

        let followup = auto_create_checkpoint(repo.to_str().expect("utf8 path"), None)
            .expect("generated-only follow-up should succeed");
        assert!(followup.is_none());

        let _ = fs::remove_dir_all(&repo);
    }
}
