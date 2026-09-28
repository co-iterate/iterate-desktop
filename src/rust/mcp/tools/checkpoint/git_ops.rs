use super::auto_commit;
use chrono::{DateTime, Duration, Utc};
use ring::digest;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

const CHECKPOINT_SUBJECT_PREFIX: &str = "iterate-checkpoint:";
const RESTORE_MODE: &str = "restore_snapshot";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub checkpoint_id: Option<String>,
    pub name: String,
    pub timestamp: DateTime<Utc>,
    pub files: Vec<String>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct RegistryCheckpointEntry {
    checkpoint_id: String,
    checkpoint_commit: String,
    checkpoint_message: String,
    logged_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreFileChange {
    pub path: String,
    pub action: String,
    pub exists_in_commit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreCheckpointResult {
    pub ok: bool,
    pub dry_run: bool,
    pub target_commit: String,
    #[serde(default = "default_restore_mode")]
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_checkpoint: Option<Checkpoint>,
    pub will_create_safety_checkpoint: bool,
    pub changed_files: Vec<RestoreFileChange>,
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_plan_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct RestorePlanFingerprint<'a> {
    mode: &'a str,
    head_before: &'a Option<String>,
    status_snapshot: &'a str,
    target_commit: &'a str,
    changed_files: &'a [RestoreFileChange],
    selected_file_state: &'a [RestoreFileState],
    warnings: &'a [String],
}

#[derive(Debug, Clone, Serialize)]
struct RestoreFileState {
    path: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct RollbackEvent {
    event_id: String,
    created_at: String,
    project_path: String,
    mode: String,
    target_commit: String,
    selected_files: Vec<String>,
    file_actions: Vec<RestoreFileChange>,
    head_before: Option<String>,
    head_after: Option<String>,
    safety_checkpoint_id: Option<String>,
    safety_commit: Option<String>,
    restore_plan_id: Option<String>,
    restore_plan_hash: Option<String>,
    result: String,
    warnings: Vec<String>,
    undo_of_event_id: Option<String>,
}

fn default_restore_mode() -> String {
    RESTORE_MODE.to_string()
}

fn ensure_git_repo(project_path: &str) -> Result<(), String> {
    let path = Path::new(project_path);
    if !path.exists() {
        return Err(format!("项目路径不存在: {}", project_path));
    }

    if !path.join(".git").exists() {
        return Err("不是 Git 仓库".to_string());
    }

    Ok(())
}

fn registry_path(project_path: &str) -> PathBuf {
    Path::new(project_path)
        .join(".cunzhi-memory")
        .join("checkpoints.jsonl")
}

fn rollback_events_path(project_path: &str) -> PathBuf {
    Path::new(project_path)
        .join(".cunzhi-memory")
        .join("rollback_events.jsonl")
}

fn sha256_hex(input: &[u8]) -> String {
    let hash = digest::digest(&digest::SHA256, input);
    hex::encode(hash.as_ref())
}

fn restore_plan_hash(
    head_before: &Option<String>,
    status_snapshot: &str,
    target_commit: &str,
    targets: &[RestoreFileChange],
    selected_file_state: &[RestoreFileState],
    warnings: &[String],
) -> Result<String, String> {
    let fingerprint = RestorePlanFingerprint {
        mode: RESTORE_MODE,
        head_before,
        status_snapshot,
        target_commit,
        changed_files: targets,
        selected_file_state,
        warnings,
    };
    let encoded =
        serde_json::to_vec(&fingerprint).map_err(|e| format!("生成恢复预览指纹失败: {}", e))?;
    Ok(sha256_hex(&encoded))
}

fn restore_plan_id(plan_hash: &str) -> String {
    let prefix_len = plan_hash.len().min(16);
    format!("rp_{}", &plan_hash[..prefix_len])
}

fn restore_plan_expires_at() -> String {
    (Utc::now() + Duration::minutes(15)).to_rfc3339()
}

fn rollback_event_id() -> String {
    let ts = Utc::now().format("%Y%m%d%H%M%S");
    let suffix: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    format!("rb_{}_{}", ts, suffix)
}

fn selected_file_state_snapshot(
    project_path: &str,
    targets: &[RestoreFileChange],
) -> Result<Vec<RestoreFileState>, String> {
    let mut states = Vec::new();
    for target in targets {
        let absolute = Path::new(project_path).join(&target.path);
        let state = match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link = fs::read_link(&absolute)
                    .map_err(|e| format!("读取选中文件链接失败 {}: {}", target.path, e))?;
                format!("symlink:{}", link.display())
            }
            Ok(metadata) if metadata.is_file() => {
                let content = fs::read(&absolute)
                    .map_err(|e| format!("读取选中文件状态失败 {}: {}", target.path, e))?;
                format!("file:{}", sha256_hex(&content))
            }
            Ok(metadata) if metadata.is_dir() => {
                format!("dir:{}", metadata.len())
            }
            Ok(_) => "other".to_string(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "absent".to_string(),
            Err(error) => {
                return Err(format!("读取选中文件状态失败 {}: {}", target.path, error));
            }
        };
        states.push(RestoreFileState {
            path: target.path.clone(),
            state,
        });
    }
    states.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(states)
}

fn write_rollback_event(project_path: &str, event: &RollbackEvent) -> Result<(), String> {
    let path = rollback_events_path(project_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 rollback event 目录失败: {}", e))?;
    }

    let line =
        serde_json::to_string(event).map_err(|e| format!("序列化 rollback event 失败: {}", e))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开 rollback event 文件失败: {}", e))?;
    writeln!(file, "{}", line).map_err(|e| format!("写入 rollback event 失败: {}", e))
}

fn get_checkpoint_files_internal(
    project_path: &str,
    commit_hash: &str,
) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .args(["show", "--pretty=format:", "--name-only", commit_hash])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("获取 commit 文件列表失败: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("获取 commit 文件列表失败: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn is_valid_relative_checkpoint_path(path: &str) -> bool {
    let rel = Path::new(path);
    !path.trim().is_empty()
        && !rel.is_absolute()
        && !rel.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn git_head_hash(project_path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.is_empty() {
        None
    } else {
        Some(hash)
    }
}

fn checkpoint_status_without_index_files(project_path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let filtered = raw
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                return None;
            }

            let path = trimmed.get(3..).unwrap_or("").trim();
            if super::is_checkpoint_index_path(path) {
                return None;
            }

            Some(trimmed.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n");

    Some(filtered)
}

fn commit_contains_file(project_path: &str, commit_hash: &str, rel: &str) -> bool {
    let object_spec = format!("{}:{}", commit_hash, rel);
    Command::new("git")
        .args(["cat-file", "-e", &object_spec])
        .current_dir(project_path)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn restore_target_action(project_path: &str, commit_hash: &str, rel: &str) -> RestoreFileChange {
    let exists_in_commit = commit_contains_file(project_path, commit_hash, rel);
    RestoreFileChange {
        path: rel.to_string(),
        action: if exists_in_commit {
            "restore".to_string()
        } else {
            "delete".to_string()
        },
        exists_in_commit,
    }
}

fn selected_restore_targets(
    project_path: &str,
    commit_hash: &str,
    selected_files: Option<&[String]>,
) -> Result<(Vec<RestoreFileChange>, Vec<String>), String> {
    let checkpoint_files = get_checkpoint_files_internal(project_path, commit_hash)?;
    let had_selection_input = selected_files.is_some();
    if checkpoint_files.is_empty() && !had_selection_input {
        return Err("该 checkpoint 没有可恢复的文件".to_string());
    }

    let checkpoint_set: HashSet<String> = checkpoint_files.iter().cloned().collect();
    let mut warnings = Vec::new();
    let mut selected_set = HashSet::new();

    if let Some(files) = selected_files {
        for file in files {
            let trimmed = file.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !is_valid_relative_checkpoint_path(trimmed) {
                warnings.push(format!("忽略非法文件路径: {}", trimmed));
                continue;
            }
            if checkpoint_set.contains(trimmed) {
                selected_set.insert(trimmed.to_string());
                continue;
            }

            if commit_contains_file(project_path, commit_hash, trimmed) {
                warnings.push(format!(
                    "选中文件不在 checkpoint 变更列表中，将按目标提交快照恢复: {}",
                    trimmed
                ));
                selected_set.insert(trimmed.to_string());
                continue;
            }

            warnings.push(format!(
                "选中文件在目标提交中不存在，确认后将删除: {}",
                trimmed
            ));
            selected_set.insert(trimmed.to_string());
        }
    }

    let mut target_files: Vec<String> = if !had_selection_input {
        checkpoint_files
    } else {
        let mut files = checkpoint_files
            .into_iter()
            .filter(|file| selected_set.contains(file))
            .collect::<Vec<_>>();
        files.extend(
            selected_set
                .iter()
                .filter(|file| !checkpoint_set.contains(*file))
                .cloned(),
        );
        files
    };
    target_files.sort();

    let changes = target_files
        .iter()
        .map(|rel| restore_target_action(project_path, commit_hash, rel))
        .collect::<Vec<_>>();

    Ok((changes, warnings))
}

fn parse_checkpoint_id(message: &str) -> Option<String> {
    message.lines().find_map(|line| {
        line.trim()
            .strip_prefix("checkpoint_id:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn read_checkpoint_commit(project_path: &str, commit_hash: &str) -> Option<Checkpoint> {
    let output = Command::new("git")
        .args([
            "show",
            "-s",
            "--no-show-signature",
            "--format=%H%n%cI%n%s%n%B",
            commit_hash,
        ])
        .current_dir(project_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let full_hash = lines.next()?.trim().to_string();
    let timestamp_raw = lines.next()?.trim();
    let subject = lines.next()?.trim().to_string();

    if !subject.starts_with(CHECKPOINT_SUBJECT_PREFIX) {
        return None;
    }

    let message = lines.collect::<Vec<_>>().join("\n");
    let timestamp = DateTime::parse_from_rfc3339(timestamp_raw)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let files = get_checkpoint_files_internal(project_path, &full_hash).unwrap_or_default();

    Some(Checkpoint {
        id: full_hash,
        checkpoint_id: parse_checkpoint_id(&message),
        name: subject.clone(),
        timestamp,
        files,
        message: subject,
    })
}

fn git_log_checkpoint_hashes(project_path: &str) -> Vec<String> {
    let output = Command::new("git")
        .args([
            "log",
            "--all",
            "--max-count=300",
            "--grep=iterate-checkpoint:",
            "--format=%H",
        ])
        .current_dir(project_path)
        .output();

    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn reflog_checkpoint_hashes(project_path: &str) -> Vec<String> {
    let output = Command::new("git")
        .args([
            "reflog",
            "--all",
            "--max-count=300",
            "--grep-reflog=iterate-checkpoint:",
            "--format=%H",
        ])
        .current_dir(project_path)
        .output();

    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn create_checkpoint(project_path: &str, message: &str) -> Result<Checkpoint, String> {
    ensure_git_repo(project_path)?;

    let checkpoint = auto_commit::create_named_checkpoint(project_path, message, None)?
        .ok_or_else(|| "没有需要保存的更改".to_string())?;
    let files = get_checkpoint_files_internal(project_path, &checkpoint.commit_hash)?;

    Ok(Checkpoint {
        id: checkpoint.commit_hash,
        checkpoint_id: Some(checkpoint.checkpoint_id),
        name: checkpoint.commit_subject.clone(),
        timestamp: Utc::now(),
        files,
        message: checkpoint.commit_subject,
    })
}

pub fn list_checkpoints(project_path: &str) -> Result<Vec<Checkpoint>, String> {
    ensure_git_repo(project_path)?;

    let registry_path = registry_path(project_path);
    let mut checkpoints = Vec::new();
    let mut seen_commits = HashSet::new();

    if registry_path.exists() {
        let content = fs::read_to_string(&registry_path)
            .map_err(|e| format!("读取 checkpoint registry 失败: {}", e))?;

        for line in content.lines().rev() {
            let Ok(record) = serde_json::from_str::<RegistryCheckpointEntry>(line) else {
                continue;
            };
            if !seen_commits.insert(record.checkpoint_commit.clone()) {
                continue;
            }

            let files = get_checkpoint_files_internal(project_path, &record.checkpoint_commit)
                .unwrap_or_default();
            let timestamp = DateTime::parse_from_rfc3339(&record.logged_at)
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            checkpoints.push(Checkpoint {
                id: record.checkpoint_commit,
                checkpoint_id: Some(record.checkpoint_id),
                name: record.checkpoint_message.clone(),
                timestamp,
                files,
                message: record.checkpoint_message,
            });
        }
    }

    for commit_hash in git_log_checkpoint_hashes(project_path)
        .into_iter()
        .chain(reflog_checkpoint_hashes(project_path))
    {
        if !seen_commits.insert(commit_hash.clone()) {
            continue;
        }
        if let Some(checkpoint) = read_checkpoint_commit(project_path, &commit_hash) {
            checkpoints.push(checkpoint);
        }
    }

    checkpoints.sort_by(|left, right| right.timestamp.cmp(&left.timestamp));

    Ok(checkpoints)
}

pub fn get_checkpoint_files(project_path: &str, commit_hash: &str) -> Result<Vec<String>, String> {
    ensure_git_repo(project_path)?;
    get_checkpoint_files_internal(project_path, commit_hash)
}

pub fn restore_checkpoint(_project_path: &str, _commit_hash: &str) -> Result<(), String> {
    Err(
        "请先执行恢复预览，并通过 restore_checkpoint_safe 携带 expected_plan_hash 确认恢复"
            .to_string(),
    )
}

pub fn restore_checkpoint_safe(
    project_path: &str,
    commit_hash: &str,
    dry_run: bool,
    create_safety_checkpoint: bool,
    selected_files: Option<Vec<String>>,
    expected_plan_hash: Option<String>,
) -> Result<RestoreCheckpointResult, String> {
    ensure_git_repo(project_path)?;

    let head_before = git_head_hash(project_path);
    let (targets, mut warnings) =
        selected_restore_targets(project_path, commit_hash, selected_files.as_deref())?;
    if targets.is_empty() {
        return Err("没有可恢复的文件".to_string());
    }
    let status_snapshot = checkpoint_status_without_index_files(project_path).unwrap_or_default();
    let dirty_before_restore = !status_snapshot.trim().is_empty();
    let selected_file_state = selected_file_state_snapshot(project_path, &targets)?;
    let plan_hash = restore_plan_hash(
        &head_before,
        &status_snapshot,
        commit_hash,
        &targets,
        &selected_file_state,
        &warnings,
    )?;
    let plan_id = restore_plan_id(&plan_hash);
    if !dry_run {
        if let Some(expected) = expected_plan_hash.as_deref() {
            if expected != plan_hash {
                return Err("恢复预览已过期，请重新预览".to_string());
            }
        }
    }

    let restore_count = targets.iter().filter(|item| item.exists_in_commit).count();
    let delete_count = targets.len().saturating_sub(restore_count);
    let diff_summary = Some(format!(
        "{} restore, {} delete",
        restore_count, delete_count
    ));

    let mut safety_checkpoint = None;
    if !dry_run && create_safety_checkpoint && dirty_before_restore {
        let safety =
            auto_commit::create_named_checkpoint(project_path, "恢复前 safety checkpoint", None)?
                .map(|meta| {
                    let files = get_checkpoint_files_internal(project_path, &meta.commit_hash)
                        .unwrap_or_default();
                    Checkpoint {
                        id: meta.commit_hash,
                        checkpoint_id: Some(meta.checkpoint_id),
                        name: meta.commit_subject.clone(),
                        timestamp: Utc::now(),
                        files,
                        message: meta.commit_subject,
                    }
                });
        safety_checkpoint = safety;
    }

    if !dry_run {
        for rel in targets.iter().map(|item| item.path.as_str()) {
            let exists_in_commit = commit_contains_file(project_path, commit_hash, rel);

            if exists_in_commit {
                let output = Command::new("git")
                    .args(["checkout", commit_hash, "--", rel])
                    .current_dir(project_path)
                    .output()
                    .map_err(|e| format!("执行 git checkout 失败: {}", e))?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("恢复 checkpoint 失败: {}", stderr.trim()));
                }
                continue;
            }

            let target = Path::new(project_path).join(rel);
            let _ = Command::new("git")
                .args(["rm", "-f", "--cached", "--ignore-unmatch", "--", rel])
                .current_dir(project_path)
                .output();
            if let Ok(meta) = fs::metadata(&target) {
                let _ = if meta.is_dir() {
                    fs::remove_dir_all(&target)
                } else {
                    fs::remove_file(&target)
                };
            }
        }
    }

    let head_after = if dry_run {
        head_before.clone()
    } else {
        git_head_hash(project_path)
    };

    if !dry_run {
        let event = RollbackEvent {
            event_id: rollback_event_id(),
            created_at: Utc::now().to_rfc3339(),
            project_path: project_path.to_string(),
            mode: RESTORE_MODE.to_string(),
            target_commit: commit_hash.to_string(),
            selected_files: targets.iter().map(|item| item.path.clone()).collect(),
            file_actions: targets.clone(),
            head_before: head_before.clone(),
            head_after,
            safety_checkpoint_id: safety_checkpoint
                .as_ref()
                .and_then(|checkpoint| checkpoint.checkpoint_id.clone()),
            safety_commit: safety_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.id.clone()),
            restore_plan_id: Some(plan_id.clone()),
            restore_plan_hash: Some(plan_hash.clone()),
            result: "ok".to_string(),
            warnings: warnings.clone(),
            undo_of_event_id: None,
        };
        if let Err(error) = write_rollback_event(project_path, &event) {
            warnings.push(error);
        }
    }

    Ok(RestoreCheckpointResult {
        ok: true,
        dry_run,
        target_commit: commit_hash.to_string(),
        mode: RESTORE_MODE.to_string(),
        head_before,
        safety_checkpoint,
        will_create_safety_checkpoint: create_safety_checkpoint && dirty_before_restore,
        changed_files: targets,
        warnings,
        diff_summary,
        restore_plan_id: Some(plan_id),
        restore_plan_hash: Some(plan_hash),
        status_snapshot: Some(status_snapshot),
        plan_expires_at: if dry_run {
            Some(restore_plan_expires_at())
        } else {
            None
        },
    })
}

pub fn has_uncommitted_changes(project_path: &str) -> bool {
    let path = Path::new(project_path);
    if !path.exists() || !path.join(".git").exists() {
        return false;
    }

    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_path)
        .output();

    match output {
        Ok(out) => !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        Err(_) => false,
    }
}

pub fn delete_checkpoint(_project_path: &str, _commit_hash: &str) -> Result<(), String> {
    Err("Git 型 checkpoint 不支持直接删除".to_string())
}

#[cfg(test)]
mod tests {
    use super::{list_checkpoints, restore_checkpoint, restore_checkpoint_safe, RESTORE_MODE};
    use crate::mcp::tools::checkpoint::auto_commit::auto_create_checkpoint;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::tempdir;

    fn run_git(repo: &PathBuf, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git command should run")
    }

    fn normalized_file_content(path: &PathBuf) -> String {
        fs::read_to_string(path).unwrap().replace("\r\n", "\n")
    }

    #[test]
    fn list_checkpoints_reads_registry_entries() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").unwrap();
        let checkpoint = auto_create_checkpoint(repo.to_str().unwrap(), Some("req_test"))
            .unwrap()
            .unwrap();

        let registry_dir = repo.join(".cunzhi-memory");
        fs::create_dir_all(&registry_dir).unwrap();
        fs::write(
            registry_dir.join("checkpoints.jsonl"),
            format!(
                "{{\"request_id\":\"req_test\",\"checkpoint_id\":\"{}\",\"checkpoint_commit\":\"{}\",\"checkpoint_message\":\"{}\",\"project_path\":\"{}\",\"logged_at\":\"2026-04-09T00:00:00+08:00\"}}\n",
                checkpoint.checkpoint_id,
                checkpoint.commit_hash,
                checkpoint.commit_subject,
                repo.to_string_lossy()
            ),
        )
        .unwrap();

        let checkpoints = list_checkpoints(repo.to_str().unwrap()).unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].id, checkpoint.commit_hash);
        assert!(checkpoints[0].files.iter().any(|file| file == "demo.txt"));
    }

    #[test]
    fn list_checkpoints_reads_git_log_entries_without_registry() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").unwrap();
        let checkpoint = auto_create_checkpoint(repo.to_str().unwrap(), Some("req_log"))
            .unwrap()
            .unwrap();

        let checkpoints = list_checkpoints(repo.to_str().unwrap()).unwrap();
        let listed = checkpoints
            .iter()
            .find(|item| item.id == checkpoint.commit_hash)
            .expect("git log checkpoint should be listed without registry");
        assert_eq!(
            listed.checkpoint_id.as_deref(),
            Some(checkpoint.checkpoint_id.as_str())
        );
        assert!(listed.files.iter().any(|file| file == "demo.txt"));
    }

    #[test]
    fn list_checkpoints_reads_reflog_entries_without_branch_ref() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").unwrap();
        let checkpoint = auto_create_checkpoint(repo.to_str().unwrap(), Some("req_reflog"))
            .unwrap()
            .unwrap();

        assert!(run_git(&repo, &["reset", "--hard", "HEAD~1"])
            .status
            .success());

        let checkpoints = list_checkpoints(repo.to_str().unwrap()).unwrap();
        let listed = checkpoints
            .iter()
            .find(|item| item.id == checkpoint.commit_hash)
            .expect("reflog checkpoint should be listed after branch reset");
        assert_eq!(
            listed.checkpoint_id.as_deref(),
            Some(checkpoint.checkpoint_id.as_str())
        );
        assert!(listed.files.iter().any(|file| file == "demo.txt"));
    }

    #[test]
    fn restore_checkpoint_rejects_direct_execution() {
        let error = restore_checkpoint("/tmp/demo", "abc123").unwrap_err();
        assert!(error.contains("请先执行恢复预览"));
    }

    #[test]
    fn restore_checkpoint_safe_restores_file_content_with_plan_hash() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").unwrap();
        let checkpoint = auto_create_checkpoint(repo.to_str().unwrap(), Some("req_restore"))
            .unwrap()
            .unwrap();

        fs::write(repo.join("demo.txt"), "v3\n").unwrap();

        let preview = restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &checkpoint.commit_hash,
            true,
            true,
            None,
            None,
        )
        .unwrap();
        restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &checkpoint.commit_hash,
            false,
            true,
            None,
            preview.restore_plan_hash,
        )
        .unwrap();

        assert_eq!(normalized_file_content(&repo.join("demo.txt")), "v2\n");
    }

    #[test]
    fn restore_checkpoint_safe_dry_run_does_not_modify_worktree() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").unwrap();
        let checkpoint = auto_create_checkpoint(repo.to_str().unwrap(), Some("req_preview"))
            .unwrap()
            .unwrap();

        fs::write(repo.join("demo.txt"), "v3\n").unwrap();

        let preview = restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &checkpoint.commit_hash,
            true,
            true,
            None,
            None,
        )
        .unwrap();

        assert!(preview.ok);
        assert!(preview.dry_run);
        assert!(preview.will_create_safety_checkpoint);
        assert_eq!(preview.changed_files.len(), 1);
        assert_eq!(preview.mode, RESTORE_MODE);
        assert!(preview.restore_plan_id.is_some());
        assert!(preview.restore_plan_hash.is_some());
        assert!(preview.status_snapshot.is_some());
        assert!(preview.plan_expires_at.is_some());
        assert!(preview.safety_checkpoint.is_none());
        assert_eq!(normalized_file_content(&repo.join("demo.txt")), "v3\n");

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn restore_checkpoint_safe_rejects_stale_expected_plan_hash() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").unwrap();
        let checkpoint = auto_create_checkpoint(repo.to_str().unwrap(), Some("req_stale"))
            .unwrap()
            .unwrap();

        fs::write(repo.join("demo.txt"), "v3\n").unwrap();

        let preview = restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &checkpoint.commit_hash,
            true,
            true,
            None,
            None,
        )
        .unwrap();
        let plan_hash = preview.restore_plan_hash.unwrap();

        fs::write(repo.join("demo.txt"), "v4\n").unwrap();

        let result = restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &checkpoint.commit_hash,
            false,
            true,
            None,
            Some(plan_hash),
        );

        assert_eq!(
            result.unwrap_err(),
            "恢复预览已过期，请重新预览".to_string()
        );
        assert_eq!(normalized_file_content(&repo.join("demo.txt")), "v4\n");

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn restore_checkpoint_safe_restores_selected_file_from_snapshot_tree() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        fs::write(repo.join("other.txt"), "other v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt", "other.txt"])
            .status
            .success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("other.txt"), "other v2\n").unwrap();
        assert!(run_git(&repo, &["add", "other.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "checkpoint-like"])
            .status
            .success());
        let target_commit = String::from_utf8_lossy(&run_git(&repo, &["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();

        fs::write(repo.join("demo.txt"), "v3 dirty\n").unwrap();

        let preview = restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &target_commit,
            true,
            true,
            Some(vec!["demo.txt".to_string()]),
            None,
        )
        .unwrap();

        assert_eq!(preview.changed_files.len(), 1);
        assert_eq!(preview.changed_files[0].path, "demo.txt");
        assert_eq!(preview.changed_files[0].action, "restore");
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.contains("选中文件不在 checkpoint 变更列表中")));

        restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &target_commit,
            false,
            true,
            Some(vec!["demo.txt".to_string()]),
            preview.restore_plan_hash,
        )
        .unwrap();

        assert_eq!(normalized_file_content(&repo.join("demo.txt")), "v1\n");

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn restore_checkpoint_safe_creates_safety_checkpoint_when_dirty() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").unwrap();
        let checkpoint = auto_create_checkpoint(repo.to_str().unwrap(), Some("req_safe"))
            .unwrap()
            .unwrap();

        fs::write(repo.join("demo.txt"), "v3\n").unwrap();

        let result = restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &checkpoint.commit_hash,
            false,
            true,
            None,
            None,
        )
        .unwrap();

        assert!(result.ok);
        assert!(!result.dry_run);
        assert!(result.will_create_safety_checkpoint);
        assert!(result.safety_checkpoint.is_some());
        assert_eq!(normalized_file_content(&repo.join("demo.txt")), "v2\n");

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn restore_checkpoint_safe_writes_rollback_event() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("demo");
        fs::create_dir_all(&repo).unwrap();

        assert!(run_git(&repo, &["init"]).status.success());
        assert!(run_git(&repo, &["config", "user.name", "Codex Test"])
            .status
            .success());
        assert!(
            run_git(&repo, &["config", "user.email", "codex@example.com"])
                .status
                .success()
        );

        fs::write(repo.join("demo.txt"), "v1\n").unwrap();
        assert!(run_git(&repo, &["add", "demo.txt"]).status.success());
        assert!(run_git(&repo, &["commit", "-m", "seed"]).status.success());

        fs::write(repo.join("demo.txt"), "v2\n").unwrap();
        let checkpoint = auto_create_checkpoint(repo.to_str().unwrap(), Some("req_event"))
            .unwrap()
            .unwrap();

        fs::write(repo.join("demo.txt"), "v3\n").unwrap();
        let preview = restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &checkpoint.commit_hash,
            true,
            true,
            Some(vec!["demo.txt".to_string()]),
            None,
        )
        .unwrap();

        let result = restore_checkpoint_safe(
            repo.to_str().unwrap(),
            &checkpoint.commit_hash,
            false,
            true,
            Some(vec!["demo.txt".to_string()]),
            preview.restore_plan_hash.clone(),
        )
        .unwrap();

        assert!(result.ok);
        assert_eq!(normalized_file_content(&repo.join("demo.txt")), "v2\n");

        let event_path = repo.join(".cunzhi-memory").join("rollback_events.jsonl");
        let content = fs::read_to_string(event_path).unwrap();
        let event: serde_json::Value =
            serde_json::from_str(content.lines().last().unwrap()).unwrap();
        assert_eq!(event["mode"], RESTORE_MODE);
        assert_eq!(event["target_commit"], checkpoint.commit_hash);
        assert_eq!(event["selected_files"][0], "demo.txt");
        assert_eq!(event["file_actions"][0]["path"], "demo.txt");
        assert_eq!(event["result"], "ok");
        assert_eq!(
            event["restore_plan_hash"],
            preview.restore_plan_hash.unwrap()
        );
        assert!(event["safety_commit"].as_str().is_some());
        assert!(event["safety_checkpoint_id"].as_str().is_some());

        let _ = fs::remove_dir_all(&repo);
    }
}
