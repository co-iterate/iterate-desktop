use chrono::Local;
use serde::Serialize;
use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// 对话日志条目
pub struct ConversationEntry {
    pub conversation_id: Option<String>,
    pub current_node_id: Option<String>,
    pub timeline_route_id: Option<String>,
    pub run_id: Option<String>,
    pub generation: Option<u64>,
    pub stale_of: Option<String>,
    pub superseded_by: Option<String>,
    pub ai_message: String,
    pub user_response: String,
    pub project_path: Option<String>,
    pub image_count: usize,
    pub file_paths: Vec<String>,
    pub image_paths: Vec<String>,
    pub selected_options: Vec<String>,
    pub request_id: Option<String>,
    pub checkpoint_id: Option<String>,
    pub checkpoint_commit: Option<String>,
    pub push_status: Option<String>,
    pub response_source: Option<String>,
    /// 与本回合 `zhi` 弹出前工作区自动检查点 commit subject 一致（若有）；便于 `che` / `rg iterate-checkpoint`
    pub workspace_checkpoint_message: Option<String>,
}

#[derive(Serialize)]
struct ConversationBlockMetadata<'a> {
    schema: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_node_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeline_route_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stale_of: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    superseded_by: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint_commit: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_source: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_path: Option<&'a str>,
}

#[derive(Serialize)]
struct CheckpointRegistryEntry<'a> {
    conversation_id: Option<&'a str>,
    request_id: &'a str,
    checkpoint_id: &'a str,
    checkpoint_commit: &'a str,
    checkpoint_message: &'a str,
    push_status: Option<&'a str>,
    project_path: &'a str,
    logged_at: String,
}

fn find_knowledge_dir(_project_path: Option<&str>) -> Option<PathBuf> {
    let configured = std::env::var_os("CUNZHI_KNOWLEDGE_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|path| path.join(".cunzhi-knowledge")));

    configured.filter(|path| path.is_dir())
}

fn strip_auto_prompt(input: &str) -> Cow<'_, str> {
    const EXACT_SENTINELS: &[&str] = &[
        "<!-- CONTEXT_INJECTION_START -->",
        "<!-- AUTO_PROMPT_START -->",
    ];
    const LINE_MARKERS: &[&str] = &[
        "✔️不明白的地方反问我",
        "✔️继续调用 zhi",
        "✔️请记住",
        "✔继续调用 zhi",
        "快捷触发词",
    ];

    let mut cut = EXACT_SENTINELS
        .iter()
        .filter_map(|marker| input.find(marker))
        .min();

    let mut offset = 0;
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if LINE_MARKERS
            .iter()
            .any(|marker| trimmed.starts_with(marker))
        {
            cut = Some(cut.map_or(offset, |value| value.min(offset)));
            break;
        }
        offset += line.len();
    }

    match cut {
        Some(pos) => Cow::Owned(input[..pos].trim_end().to_string()),
        None => Cow::Borrowed(input),
    }
}

fn resolve_project_name(project_path: Option<&str>) -> String {
    project_path
        .filter(|path| !path.trim().is_empty())
        .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.to_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".to_string())
}

fn resolve_device_id() -> String {
    let raw = Command::new("hostname")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    raw.trim()
        .split('.')
        .next()
        .unwrap_or("unknown")
        .to_string()
}

fn append_checkpoint_registry(entry: &ConversationEntry) {
    let Some(project_path) = entry.project_path.as_deref() else {
        log::info!(
            "[CHE-DEBUG][registry] skip: missing project_path request_id={:?}",
            entry.request_id
        );
        return;
    };
    let Some(request_id) = entry.request_id.as_deref() else {
        log::info!(
            "[CHE-DEBUG][registry] skip: missing request_id project_path={}",
            project_path
        );
        return;
    };
    let Some(checkpoint_id) = entry.checkpoint_id.as_deref() else {
        log::info!(
            "[CHE-DEBUG][registry] skip: missing checkpoint_id project_path={} request_id={}",
            project_path,
            request_id
        );
        return;
    };
    let Some(checkpoint_commit) = entry.checkpoint_commit.as_deref() else {
        log::info!(
            "[CHE-DEBUG][registry] skip: missing checkpoint_commit project_path={} request_id={} checkpoint_id={}",
            project_path, request_id, checkpoint_id
        );
        return;
    };
    let Some(checkpoint_message) = entry.workspace_checkpoint_message.as_deref() else {
        log::info!(
            "[CHE-DEBUG][registry] skip: missing checkpoint_message project_path={} request_id={} checkpoint_id={} checkpoint_commit={}",
            project_path, request_id, checkpoint_id, checkpoint_commit
        );
        return;
    };

    let registry_dir = PathBuf::from(project_path).join(".cunzhi-memory");
    if fs::create_dir_all(&registry_dir).is_err() {
        log::warn!(
            "[CHE-DEBUG][registry] create_dir_all failed: {}",
            registry_dir.display()
        );
        return;
    }

    let registry_path = registry_dir.join("checkpoints.jsonl");
    let record = CheckpointRegistryEntry {
        request_id,
        conversation_id: entry.conversation_id.as_deref(),
        checkpoint_id,
        checkpoint_commit,
        checkpoint_message,
        push_status: entry.push_status.as_deref(),
        project_path,
        logged_at: Local::now().to_rfc3339(),
    };

    let Ok(line) = serde_json::to_string(&record) else {
        log::warn!(
            "[CHE-DEBUG][registry] serialize failed project_path={} request_id={} checkpoint_id={}",
            project_path,
            request_id,
            checkpoint_id
        );
        return;
    };

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(registry_path)
    {
        let _ = writeln!(file, "{}", line);
        log::info!(
            "[CHE-DEBUG][registry] appended project_path={} request_id={} checkpoint_id={} checkpoint_commit={}",
            project_path, request_id, checkpoint_id, checkpoint_commit
        );
    } else {
        log::warn!(
            "[CHE-DEBUG][registry] open failed project_path={} request_id={} checkpoint_id={}",
            project_path,
            request_id,
            checkpoint_id
        );
    }
}

fn build_user_section(entry: &ConversationEntry) -> Option<String> {
    let cleaned = strip_auto_prompt(&entry.user_response).trim().to_string();
    if !cleaned.is_empty() {
        return Some(cleaned);
    }

    if !entry.selected_options.is_empty() {
        return Some(format!("选中的选项: {}", entry.selected_options.join(", ")));
    }

    None
}

fn paths_missing_from_text(paths: &[String], existing_text: Option<&str>) -> Vec<String> {
    let existing_text = existing_text.unwrap_or("");
    paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty() && !existing_text.contains(*path))
        .map(ToString::to_string)
        .collect()
}

fn format_path_block(label: &str, paths: &[String]) -> Option<String> {
    let lines = paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .map(|path| format!("- {}", path))
        .collect::<Vec<_>>();

    if lines.is_empty() {
        None
    } else {
        Some(format!("{}：\n{}", label, lines.join("\n")))
    }
}

fn write_git_checkpoint(
    knowledge_dir: &PathBuf,
    conv_file: &PathBuf,
    date_str: &str,
    time_str: &str,
) {
    let Some(conv_file_str) = conv_file.to_str() else {
        return;
    };

    let _ = Command::new("git")
        .args(["add", conv_file_str])
        .current_dir(knowledge_dir)
        .output();
    let _ = Command::new("git")
        .args([
            "commit",
            "-m",
            &format!("auto: conversation {}_{}", date_str, time_str),
            "--quiet",
            "--no-verify",
        ])
        .current_dir(knowledge_dir)
        .output();
}

fn build_conversation_meta_comment(entry: &ConversationEntry) -> Option<String> {
    let metadata = ConversationBlockMetadata {
        schema: "cunzhi.conversation.v1",
        conversation_id: entry.conversation_id.as_deref(),
        current_node_id: entry.current_node_id.as_deref(),
        request_id: entry.request_id.as_deref(),
        timeline_route_id: entry.timeline_route_id.as_deref(),
        run_id: entry.run_id.as_deref(),
        generation: entry.generation,
        stale_of: entry.stale_of.as_deref(),
        superseded_by: entry.superseded_by.as_deref(),
        checkpoint_id: entry.checkpoint_id.as_deref(),
        checkpoint_commit: entry.checkpoint_commit.as_deref(),
        response_source: entry.response_source.as_deref(),
        project_path: entry.project_path.as_deref(),
    };

    let json = serde_json::to_string(&metadata).ok()?;
    Some(format!(
        "<!-- cunzhi-meta: {} -->",
        json.replace("--", "\\u002d\\u002d")
    ))
}

/// 追加对话日志，写入 `.cunzhi-knowledge/conversations/YYYY-MM-DD/{project}__{device}.md`
pub fn append_conversation_log(entry: &ConversationEntry) {
    if entry.ai_message.trim().is_empty() {
        log::info!("[CHE-DEBUG][conversation] skip: empty ai_message");
        return;
    }

    let knowledge_dir = match find_knowledge_dir(entry.project_path.as_deref()) {
        Some(dir) => dir,
        None => {
            log::warn!(
                "[CHE-DEBUG][conversation] skip: knowledge_dir not found project_path={:?} request_id={:?}",
                entry.project_path, entry.request_id
            );
            return;
        }
    };

    let conversations_dir = knowledge_dir.join("conversations");
    if fs::create_dir_all(&conversations_dir).is_err() {
        return;
    }

    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let time_str = now.format("%H:%M:%S").to_string();
    let project_name = resolve_project_name(entry.project_path.as_deref());
    let device_id = resolve_device_id();

    let mut content = format!(
        "## {}  @ {}\n{}\n\n### 🤖 AI\n{}\n",
        time_str,
        project_name,
        build_conversation_meta_comment(entry).unwrap_or_default(),
        entry.ai_message
    );

    if let Some(ref msg) = entry.workspace_checkpoint_message {
        if !msg.trim().is_empty() {
            content.push_str(msg.trim());
            content.push_str("\n");
        }
    }

    let user_section = build_user_section(entry);
    if let Some(user_section) = user_section.as_ref() {
        content.push_str(&format!("\n### 👤 用户\n{}\n", user_section));
    }

    let missing_file_paths = paths_missing_from_text(&entry.file_paths, user_section.as_deref());
    let missing_image_paths = paths_missing_from_text(&entry.image_paths, user_section.as_deref());
    if let Some(block) = format_path_block("附加文件路径", &missing_file_paths) {
        content.push_str(&format!("\n{}\n", block));
    }
    if let Some(block) = format_path_block("附加图片路径", &missing_image_paths) {
        content.push_str(&format!("\n{}\n", block));
    }

    if entry.image_count > 0 && entry.image_paths.is_empty() {
        content.push_str(&format!("\n📷 *附图 {} 张*\n", entry.image_count));
    }

    content.push_str("\n---\n");

    let date_dir = conversations_dir.join(&date_str);
    if fs::create_dir_all(&date_dir).is_err() {
        return;
    }

    let conv_file = date_dir.join(format!("{}__{}.md", project_name, device_id));
    log::info!(
        "[CHE-DEBUG][conversation] writing file={} request_id={:?} checkpoint_id={:?} checkpoint_commit={:?} checkpoint_subject_present={}",
        conv_file.display(),
        entry.request_id,
        entry.checkpoint_id,
        entry.checkpoint_commit,
        entry
            .workspace_checkpoint_message
            .as_ref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    );
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&conv_file)
    {
        let _ = file.write_all(content.as_bytes());
        append_checkpoint_registry(entry);
        write_git_checkpoint(&knowledge_dir, &conv_file, &date_str, &time_str);
        log::info!(
            "[CHE-DEBUG][conversation] appended file={} request_id={:?} conversation_id={:?} current_node_id={:?} timeline_route_id={:?}",
            conv_file.display(),
            entry.request_id,
            entry.conversation_id,
            entry.current_node_id,
            entry.timeline_route_id
        );
    } else {
        log::warn!(
            "[CHE-DEBUG][conversation] open failed file={} request_id={:?}",
            conv_file.display(),
            entry.request_id
        );
    }
}

/// 保留兼容接口；当前写入为实时落盘，无额外 flush 动作。
pub fn force_sync_conversations() {}

/// 返回当前项目今天的对话日志文件路径（如果存在）
pub fn get_conversation_log_path(project_path: Option<&str>) -> Option<String> {
    let knowledge_dir = find_knowledge_dir(project_path)?;
    let now = Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let project_name = resolve_project_name(project_path);
    let device_id = resolve_device_id();

    let conv_file = knowledge_dir
        .join("conversations")
        .join(&date_str)
        .join(format!("{}__{}.md", project_name, device_id));

    if conv_file.exists() {
        conv_file.to_str().map(|s| s.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_conversation_log, get_conversation_log_path, strip_auto_prompt, ConversationEntry,
    };
    use crate::config::{save_standalone_config, AppConfig};
    use crate::mcp::tools::checkpoint;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    static KNOWLEDGE_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct KnowledgeEnvGuard {
        previous: Option<OsString>,
    }

    impl KnowledgeEnvGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os("CUNZHI_KNOWLEDGE_DIR");
            std::env::set_var("CUNZHI_KNOWLEDGE_DIR", path);
            Self { previous }
        }
    }

    struct ConfigEnvGuard {
        previous_home: Option<OsString>,
        previous_xdg_config_home: Option<OsString>,
        previous_iterate_config_dir: Option<OsString>,
    }

    impl ConfigEnvGuard {
        fn set(home: &Path) -> Self {
            let previous_home = std::env::var_os("HOME");
            let previous_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");
            let previous_iterate_config_dir = std::env::var_os("ITERATE_CONFIG_DIR");
            std::env::set_var("HOME", home);
            std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
            std::env::set_var("ITERATE_CONFIG_DIR", home.join(".config/cunzhi"));
            Self {
                previous_home,
                previous_xdg_config_home,
                previous_iterate_config_dir,
            }
        }
    }

    impl Drop for ConfigEnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous_home.take() {
                std::env::set_var("HOME", previous);
            } else {
                std::env::remove_var("HOME");
            }

            if let Some(previous) = self.previous_xdg_config_home.take() {
                std::env::set_var("XDG_CONFIG_HOME", previous);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }

            if let Some(previous) = self.previous_iterate_config_dir.take() {
                std::env::set_var("ITERATE_CONFIG_DIR", previous);
            } else {
                std::env::remove_var("ITERATE_CONFIG_DIR");
            }
        }
    }

    impl Drop for KnowledgeEnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("CUNZHI_KNOWLEDGE_DIR", previous);
            } else {
                std::env::remove_var("CUNZHI_KNOWLEDGE_DIR");
            }
        }
    }

    #[test]
    fn strip_auto_prompt_removes_line_markers() {
        let input = "正常内容\n✔️继续调用 zhi\n后续提示";
        assert_eq!(strip_auto_prompt(input), "正常内容");
    }

    #[test]
    fn strip_auto_prompt_keeps_normal_text() {
        let input = "正常内容\n第二行";
        assert_eq!(strip_auto_prompt(input), "正常内容\n第二行");
    }

    #[test]
    fn append_conversation_log_writes_markdown_file() {
        let _lock = KNOWLEDGE_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        let temp = tempdir().unwrap();
        let project_dir = temp.path().join("demo-project");
        let project_knowledge_dir = project_dir.join(".cunzhi-knowledge");
        fs::create_dir_all(&project_knowledge_dir).unwrap();
        let knowledge_dir = temp.path().join("global-knowledge");
        fs::create_dir_all(&knowledge_dir).unwrap();
        let _env = KnowledgeEnvGuard::set(&knowledge_dir);

        append_conversation_log(&ConversationEntry {
            ai_message: "AI 内容".to_string(),
            user_response: "用户回复".to_string(),
            project_path: Some(project_dir.to_string_lossy().to_string()),
            image_count: 1,
            file_paths: vec![],
            image_paths: vec![],
            selected_options: vec![],
            conversation_id: None,
            current_node_id: None,
            timeline_route_id: None,
            run_id: None,
            generation: None,
            stale_of: None,
            superseded_by: None,
            request_id: None,
            checkpoint_id: None,
            checkpoint_commit: None,
            push_status: None,
            response_source: None,
            workspace_checkpoint_message: None,
        });

        let conv_path = get_conversation_log_path(Some(project_dir.to_str().unwrap()))
            .expect("conversation path should exist");
        let content = fs::read_to_string(conv_path).unwrap();
        assert!(content.contains("### 🤖 AI\nAI 内容"));
        assert!(content.contains("### 👤 用户\n用户回复"));
        assert!(content.contains("📷 *附图 1 张*"));
        assert!(
            !project_knowledge_dir.join("conversations").exists(),
            "conversation should not be written to project-local knowledge"
        );
    }

    #[test]
    fn append_conversation_log_writes_attachment_paths() {
        let _lock = KNOWLEDGE_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        let temp = tempdir().unwrap();
        let project_dir = temp.path().join("demo-project");
        let project_knowledge_dir = project_dir.join(".cunzhi-knowledge");
        fs::create_dir_all(&project_knowledge_dir).unwrap();
        let knowledge_dir = temp.path().join("global-knowledge");
        fs::create_dir_all(&knowledge_dir).unwrap();
        let _env = KnowledgeEnvGuard::set(&knowledge_dir);

        append_conversation_log(&ConversationEntry {
            ai_message: "AI 内容".to_string(),
            user_response: "看这些附件".to_string(),
            project_path: Some(project_dir.to_string_lossy().to_string()),
            image_count: 1,
            file_paths: vec!["/tmp/spec.md".to_string()],
            image_paths: vec!["/Users/test/.cunzhi/images/image_123_0.png".to_string()],
            selected_options: vec![],
            conversation_id: None,
            current_node_id: None,
            timeline_route_id: None,
            run_id: None,
            generation: None,
            stale_of: None,
            superseded_by: None,
            request_id: None,
            checkpoint_id: None,
            checkpoint_commit: None,
            push_status: None,
            response_source: None,
            workspace_checkpoint_message: None,
        });

        let conv_path = get_conversation_log_path(Some(project_dir.to_str().unwrap()))
            .expect("conversation path should exist");
        let content = fs::read_to_string(conv_path).unwrap();
        assert!(content.contains("附加文件路径：\n- /tmp/spec.md"));
        assert!(content.contains("附加图片路径：\n- /Users/test/.cunzhi/images/image_123_0.png"));
        assert!(
            !content.contains("📷 *附图 1 张*"),
            "real image paths should replace count-only fallback"
        );
        assert!(
            !project_knowledge_dir.join("conversations").exists(),
            "conversation should not be written to project-local knowledge"
        );
    }

    #[test]
    fn append_conversation_log_includes_workspace_checkpoint_line() {
        let _lock = KNOWLEDGE_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        let temp = tempdir().unwrap();
        let project_dir = temp.path().join("demo-project");
        let project_knowledge_dir = project_dir.join(".cunzhi-knowledge");
        fs::create_dir_all(&project_knowledge_dir).unwrap();
        let knowledge_dir = temp.path().join("global-knowledge");
        fs::create_dir_all(&knowledge_dir).unwrap();
        let _env = KnowledgeEnvGuard::set(&knowledge_dir);

        let cp = "iterate-checkpoint:2099-01-01T00:00:00Z | 自动检查点 08:00:00";
        append_conversation_log(&ConversationEntry {
            ai_message: "ping".to_string(),
            user_response: "pong".to_string(),
            project_path: Some(project_dir.to_string_lossy().to_string()),
            image_count: 0,
            file_paths: vec![],
            image_paths: vec![],
            selected_options: vec![],
            conversation_id: Some("tree-1".to_string()),
            current_node_id: Some("node-1".to_string()),
            timeline_route_id: Some("thread-1".to_string()),
            run_id: Some("run-1".to_string()),
            generation: Some(7),
            stale_of: None,
            superseded_by: Some("run-2".to_string()),
            request_id: Some("req-1".to_string()),
            checkpoint_id: Some("cp_test".to_string()),
            checkpoint_commit: Some("abc123".to_string()),
            push_status: Some("not_configured".to_string()),
            response_source: Some("popup_submit".to_string()),
            workspace_checkpoint_message: Some(cp.to_string()),
        });

        let conv_path = get_conversation_log_path(Some(project_dir.to_str().unwrap()))
            .expect("conversation path should exist");
        let content = fs::read_to_string(conv_path).unwrap();
        assert!(content.contains(cp));
        assert!(content.contains("iterate-checkpoint:"));
        assert!(content.contains("<!-- cunzhi-meta:"));
        assert!(content.contains("\"conversation_id\":\"tree-1\""));
        assert!(content.contains("\"current_node_id\":\"node-1\""));
        assert!(content.contains("\"timeline_route_id\":\"thread-1\""));
        assert!(content.contains("\"run_id\":\"run-1\""));
        assert!(content.contains("\"generation\":7"));
        assert!(content.contains("\"superseded_by\":\"run-2\""));
        let registry =
            fs::read_to_string(project_dir.join(".cunzhi-memory").join("checkpoints.jsonl"))
                .expect("checkpoint registry should exist");
        assert!(registry.contains("\"conversation_id\":\"tree-1\""));
        assert!(registry.contains("\"checkpoint_id\":\"cp_test\""));
        assert!(registry.contains("\"checkpoint_commit\":\"abc123\""));
        assert!(registry.contains("\"push_status\":\"not_configured\""));
        assert!(
            !project_knowledge_dir.join("conversations").exists(),
            "conversation should not be written to project-local knowledge"
        );
    }

    #[test]
    fn e2e_checkpoint_subject_matches_git_log_and_conversation_md() {
        let _lock = KNOWLEDGE_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        let _config_lock = checkpoint::STANDALONE_CONFIG_ENV_LOCK
            .lock()
            .expect("standalone config env lock should not be poisoned");
        let temp = tempdir().unwrap();
        let project = temp.path().join("git-proj");
        fs::create_dir_all(&project).unwrap();
        let project_knowledge_dir = project.join(".cunzhi-knowledge");
        fs::create_dir_all(&project_knowledge_dir).unwrap();
        let knowledge_dir = temp.path().join("global-knowledge");
        fs::create_dir_all(&knowledge_dir).unwrap();
        let _env = KnowledgeEnvGuard::set(&knowledge_dir);
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let _config_env = ConfigEnvGuard::set(&home);
        let mut config = AppConfig::default();
        config.checkpoint_config.auto_checkpoint_enabled = true;
        save_standalone_config(&config).expect("enabled config should be saved");

        let run_git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&project)
                .status()
                .expect("git runs")
                .success()
        };
        assert!(run_git(&["init"]));
        assert!(run_git(&["config", "user.email", "e2e@example.com"]));
        assert!(run_git(&["config", "user.name", "e2e"]));
        fs::write(project.join("tracked.txt"), "v1\n").unwrap();
        assert!(run_git(&["add", "tracked.txt"]));
        assert!(run_git(&["commit", "-m", "seed"]));
        fs::write(project.join("tracked.txt"), "v2\n").unwrap();

        let checkpoint =
            checkpoint::maybe_auto_checkpoint(project.to_str().unwrap(), Some("req-e2e"))
                .expect("dirty repo should yield checkpoint message");
        assert!(checkpoint.commit_subject.contains("iterate-checkpoint:"));

        let out = Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(&project)
            .output()
            .expect("git log");
        let subject = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(
            checkpoint.commit_subject, subject,
            "logger 应用收到与 git subject 相同的字符串"
        );

        append_conversation_log(&ConversationEntry {
            ai_message: "e2e ai".to_string(),
            user_response: "e2e user".to_string(),
            project_path: Some(project.to_string_lossy().to_string()),
            image_count: 0,
            file_paths: vec![],
            image_paths: vec![],
            selected_options: vec![],
            conversation_id: Some("tree-e2e".to_string()),
            current_node_id: Some("node-e2e".to_string()),
            timeline_route_id: Some("thread-e2e".to_string()),
            run_id: None,
            generation: None,
            stale_of: None,
            superseded_by: None,
            request_id: Some("req-e2e".to_string()),
            checkpoint_id: Some(checkpoint.checkpoint_id.clone()),
            checkpoint_commit: Some(checkpoint.commit_hash.clone()),
            push_status: Some(checkpoint.push_status.clone()),
            response_source: Some("popup_submit".to_string()),
            workspace_checkpoint_message: Some(checkpoint.commit_subject.clone()),
        });

        let conv_path = get_conversation_log_path(Some(project.to_str().unwrap()))
            .expect("conversation path should exist");
        let md = fs::read_to_string(conv_path).unwrap();
        assert!(
            md.contains(&checkpoint.commit_subject),
            "对话 md 应包含与 commit subject 一致的整行: {}",
            checkpoint.commit_subject
        );
        let registry = fs::read_to_string(project.join(".cunzhi-memory").join("checkpoints.jsonl"))
            .expect("checkpoint registry should exist");
        assert!(registry.contains("\"request_id\":\"req-e2e\""));
        assert!(registry.contains(&checkpoint.checkpoint_id));
        assert!(registry.contains("\"push_status\":\"not_configured\""));
        assert!(
            !project_knowledge_dir.join("conversations").exists(),
            "conversation should not be written to project-local knowledge"
        );
    }
}
