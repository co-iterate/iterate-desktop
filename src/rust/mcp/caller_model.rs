//! 读出调用 `call_zhi` 的 AI 此刻用的模型，用于弹窗头部的「clauding · Opus 5.5」。
//!
//! MCP 协议不带模型信息，只能读各客户端自己写在本机的状态：
//! - Claude Code：会话 transcript 里每条 assistant 消息都带 `message.model`，取最后一条。
//! - Codex：state DB 的 `threads.model`（按线程精确）。
//! - Antigravity CLI（agy）：`~/.gemini/antigravity-cli/settings.json` 的 `model`。
//!   这是全局当前设置，不按会话区分；同时开两个不同模型的 agy 时可能不准。
//!
//! 读不到就返回 None，调用方只显示客户端名，不猜模型。

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// transcript 可能有几十 MB，只读末尾这么多字节找最后一条 assistant 消息。
const TRANSCRIPT_TAIL_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerModel {
    /// 客户端报告的原始模型 ID，例如 `claude-opus-5-5`。
    pub id: String,
    /// 头部显示用的短名，例如 `Opus 5.5`。
    pub label: String,
}

impl CallerModel {
    fn from_id(id: &str) -> Option<Self> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        Some(Self {
            id: id.to_string(),
            label: model_label(id),
        })
    }
}

/// `claude-opus-5-5` → `Opus 5.5`，`gpt-6.1-terra` → `GPT-6.1 Terra`；
/// 已经是人类可读的名字（含空格，如 agy 的 `Gemini 3.8 Flash (High)`）原样保留。
pub fn model_label(id: &str) -> String {
    let id = id.trim();
    if id.contains(' ') {
        return id.to_string();
    }
    if let Some(rest) = id.strip_prefix("claude-") {
        let mut parts = rest.split('-');
        if let Some(family) = parts.next() {
            // 版本号是短数字段；8 位日期快照后缀（如 `20261001`）不算版本。
            let version: Vec<&str> = parts
                .take_while(|part| {
                    part.len() <= 2 && part.chars().all(|ch| ch.is_ascii_digit())
                })
                .collect();
            let family = capitalize(family);
            return if version.is_empty() {
                family
            } else {
                format!("{family} {}", version.join("."))
            };
        }
    }
    if let Some(rest) = id.strip_prefix("gpt-") {
        let mut parts = rest.splitn(2, '-');
        let version = parts.next().unwrap_or_default();
        return match parts.next() {
            Some(variant) => format!("GPT-{version} {}", capitalize(variant)),
            None => format!("GPT-{version}"),
        };
    }
    id.to_string()
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn claude_projects_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir).join("projects"));
    }
    dirs::home_dir().map(|home| home.join(".claude").join("projects"))
}

/// Claude 会话当前的模型。transcript 放在 `projects/<编码后的 cwd>/<sessionId>.jsonl`，
/// cwd 的编码规则不对外承诺，所以按会话 ID 在各项目目录里找。
pub fn claude_session_model(session_id: &str) -> Option<CallerModel> {
    let session_id = super::caller_identity::normalize_claude_session_id(session_id)?;
    let projects = claude_projects_dir()?;
    let file_name = format!("{session_id}.jsonl");
    let transcript = std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file_name))
        .find(|path| path.is_file())?;
    last_assistant_model_in_transcript(&transcript)
}

fn last_assistant_model_in_transcript(path: &Path) -> Option<CallerModel> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    // 从中间开始读时第一行可能被截断，解析失败会被跳过。
    text.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        let model = value.pointer("/message/model").and_then(Value::as_str)?;
        // 合成消息（如中断提示）的 model 是 `<synthetic>`，不代表真实模型。
        if model.starts_with('<') {
            return None;
        }
        CallerModel::from_id(model)
    })
}

/// Codex 线程当前的模型，读 state DB 的 `threads.model`。
pub fn codex_thread_model(state_db_path: &Path, thread_id: &str) -> Option<CallerModel> {
    if !state_db_path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(state_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let model = conn
        .query_row(
            "SELECT model FROM threads WHERE id = ?1",
            params![thread_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()?;
    CallerModel::from_id(&model)
}

fn antigravity_cli_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        home.join(".gemini")
            .join("antigravity-cli")
            .join("settings.json")
    })
}

/// agy 的当前模型（全局设置，`/model` 切换后会写回这里）。
pub fn antigravity_cli_model() -> Option<CallerModel> {
    antigravity_cli_model_from_settings(&antigravity_cli_settings_path()?)
}

fn antigravity_cli_model_from_settings(path: &Path) -> Option<CallerModel> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    CallerModel::from_id(value.get("model")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn model_ids_get_short_labels() {
        assert_eq!(model_label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_label("claude-sonnet-5-5-20261001"), "Sonnet 5.5");
        assert_eq!(model_label("claude-fable-5-1"), "Fable 5.1");
        assert_eq!(model_label("gpt-6.1-terra"), "GPT-6.1 Terra");
        assert_eq!(model_label("gpt-6.1"), "GPT-6.1");
        assert_eq!(model_label("Gemini 3.8 Flash (High)"), "Gemini 3.8 Flash (High)");
        assert_eq!(model_label("o9-mini"), "o9-mini");
    }

    #[test]
    fn transcript_uses_the_last_real_assistant_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut file = std::fs::File::create(&path).unwrap();
        for line in [
            r#"{"type":"assistant","message":{"model":"claude-sonnet-5-5"}}"#,
            r#"{"type":"user","message":{"content":"switch"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5"}}"#,
            r#"{"type":"assistant","message":{"model":"<synthetic>"}}"#,
            r#"{"type":"system"}"#,
        ] {
            writeln!(file, "{line}").unwrap();
        }
        let model = last_assistant_model_in_transcript(&path).unwrap();
        assert_eq!(model.id, "claude-opus-5-5");
        assert_eq!(model.label, "Opus 5.5");
    }

    #[test]
    fn codex_model_comes_from_the_thread_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state_5.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, model TEXT);
             INSERT INTO threads VALUES ('t1', 'gpt-6.1-terra'), ('t2', NULL);",
        )
        .unwrap();
        drop(conn);
        assert_eq!(codex_thread_model(&db, "t1").unwrap().label, "GPT-6.1 Terra");
        assert_eq!(codex_thread_model(&db, "t2"), None);
        assert_eq!(codex_thread_model(&db, "missing"), None);
    }

    #[test]
    fn antigravity_model_reads_cli_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"model":"Gemini 3.8 Flash (High)","theme":"dark"}"#).unwrap();
        let model = antigravity_cli_model_from_settings(&path).unwrap();
        assert_eq!(model.label, "Gemini 3.8 Flash (High)");
        std::fs::write(&path, r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(antigravity_cli_model_from_settings(&path), None);
    }
}
