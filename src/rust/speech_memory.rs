use chrono::Utc;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::NamedTempFile;

const SPEECH_VOCABULARY_VERSION: u32 = 1;
const MAX_SPEECH_VOCABULARY_ENTRIES: usize = 500;
const MAX_SPEECH_VOCABULARY_TERM_LENGTH: usize = 48;

static SPEECH_VOCABULARY_WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static SPEECH_HISTORY_WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static GPT_LIVE_HISTORY_WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static SPEECH_MEMORY_TABLE_WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeechVocabularyEntry {
    pub term: String,
    pub count: u64,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeechVocabularyStore {
    pub version: u32,
    pub updated_at: String,
    pub entries: Vec<SpeechVocabularyEntry>,
}

fn speech_memory_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".cunzhi")
}

pub fn speech_memory_path() -> PathBuf {
    speech_memory_dir().join("speech-muscle-memory.json")
}

pub fn speech_correction_memory_path() -> PathBuf {
    speech_memory_dir().join("speech-correction-memory.json")
}

pub fn speech_vocabulary_path() -> PathBuf {
    speech_memory_dir().join("speech-vocabulary.json")
}

pub fn speech_history_path_for_date(date: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".cunzhi-knowledge")
        .join("speech")
        .join(format!("{date}.md"))
}

pub fn gpt_live_history_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".cunzhi-knowledge")
        .join("speech")
        .join("GPT-Live.md")
}

fn load_entries_from(path: PathBuf) -> Result<Vec<Value>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {}", e))?;
    let value: Value = serde_json::from_str(&raw).map_err(|e| format!("解析失败: {}", e))?;
    Ok(value.as_array().cloned().unwrap_or_default())
}

fn save_entries_to(path: PathBuf, entries: Vec<Value>) -> Result<Vec<Value>, String> {
    let dir = speech_memory_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;

    let raw = serde_json::to_string_pretty(&entries).map_err(|e| format!("序列化失败: {}", e))?;
    atomic_write(&path, raw.as_bytes())?;
    Ok(entries)
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "写入路径缺少父目录".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    let mut temp = NamedTempFile::new_in(parent).map_err(|e| format!("创建临时文件失败: {e}"))?;
    temp.write_all(bytes)
        .map_err(|e| format!("写入临时文件失败: {e}"))?;
    temp.flush().map_err(|e| format!("刷新临时文件失败: {e}"))?;
    temp.persist(path)
        .map_err(|e| format!("替换记忆文件失败: {}", e.error))?;
    Ok(())
}

/// 跨进程互斥：GUI 主进程、`--bridge-only` 进程和独立弹窗进程都会写记忆表，
/// 进程内 Mutex 之外还需要 flock 文件锁，否则 load-modify-save 会互相覆盖。
#[cfg(unix)]
fn acquire_table_file_lock() -> Result<std::fs::File, String> {
    use std::os::unix::io::AsRawFd;
    let dir = speech_memory_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join("speech-memory.lock"))
        .map_err(|e| format!("打开语音记忆锁文件失败: {e}"))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result != 0 {
        return Err("语音记忆文件锁不可用".to_string());
    }
    Ok(file)
}

#[cfg(not(unix))]
fn acquire_table_file_lock() -> Result<(), String> {
    Ok(())
}

pub fn load_entries() -> Result<Vec<Value>, String> {
    load_entries_from(speech_memory_path())
}

pub fn save_entries(entries: Vec<Value>) -> Result<Vec<Value>, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let _guard = SPEECH_MEMORY_TABLE_WRITE_LOCK
        .lock()
        .map_err(|_| "语音记忆写入锁不可用".to_string())?;
    let _file_lock = acquire_table_file_lock()?;
    save_entries_to(speech_memory_path(), entries)
}

pub fn save_entries_checked(entries: Vec<Value>, expected: Option<Value>) -> Result<Vec<Value>, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let _guard = SPEECH_MEMORY_TABLE_WRITE_LOCK.lock().map_err(|_| "语音记忆写入锁不可用")?;
    let _file_lock = acquire_table_file_lock()?;
    let current = Value::Array(load_entries()?);
    if expected.as_ref() != Some(&current) { return Err("语音规则或训练计数已改变，请重新加载后重试".into()); }
    save_entries_to(speech_memory_path(), entries)
}

pub fn load_correction_entries() -> Result<Vec<Value>, String> {
    load_entries_from(speech_correction_memory_path())
}

pub fn save_correction_entries(entries: Vec<Value>) -> Result<Vec<Value>, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let _guard = SPEECH_MEMORY_TABLE_WRITE_LOCK
        .lock()
        .map_err(|_| "语音记忆写入锁不可用".to_string())?;
    let _file_lock = acquire_table_file_lock()?;
    save_entries_to(speech_correction_memory_path(), entries)
}

fn entry_str<'a>(entry: &'a Value, key: &str) -> &'a str {
    entry.get(key).and_then(Value::as_str).unwrap_or("")
}

fn entry_count(entry: &Value, key: &str) -> u64 {
    match entry.get(key) {
        Some(value) => value
            .as_u64()
            .or_else(|| value.as_f64().map(|number| number.max(0.0) as u64))
            .unwrap_or(0),
        None => 0,
    }
}

fn find_entry_mut<'a>(
    entries: &'a mut [Value],
    id: Option<&str>,
    fallback: impl Fn(&Value) -> bool,
) -> Option<&'a mut Value> {
    match id {
        Some(id) if !id.is_empty() => entries
            .iter_mut()
            .find(|entry| entry_str(entry, "id") == id),
        _ => entries.iter_mut().find(|entry| fallback(entry)),
    }
}

fn apply_muscle_memory_hit(
    entries: &mut [Value],
    id: Option<&str>,
    spoken_phrase: Option<&str>,
) -> bool {
    let phrase = spoken_phrase.unwrap_or("");
    let target = find_entry_mut(entries, id, |entry| {
        !phrase.is_empty() && entry_str(entry, "spokenPhrase") == phrase
    });
    let Some(entry) = target else {
        return false;
    };
    let next = entry_count(entry, "trainingCount").saturating_add(1);
    let Some(object) = entry.as_object_mut() else {
        return false;
    };
    object.insert("trainingCount".to_string(), Value::from(next));
    true
}

fn apply_correction_memory_counter(
    entries: &mut [Value],
    id: Option<&str>,
    observed_text: Option<&str>,
    intended_text: Option<&str>,
    counter_key: &str,
    now: &str,
) -> bool {
    let observed = observed_text.unwrap_or("");
    let intended = intended_text.unwrap_or("");
    let target = find_entry_mut(entries, id, |entry| {
        !observed.is_empty()
            && !intended.is_empty()
            && entry_str(entry, "observedText") == observed
            && entry_str(entry, "intendedText") == intended
    });
    let Some(entry) = target else {
        return false;
    };
    let next = entry_count(entry, counter_key).saturating_add(1);
    let Some(object) = entry.as_object_mut() else {
        return false;
    };
    object.insert(counter_key.to_string(), Value::from(next));
    object.insert("updatedAt".to_string(), Value::from(now.to_string()));
    true
}

/// 锁内原子自增：替代前端整表 load-modify-save 的命中回写。
pub fn record_muscle_memory_hit(
    id: Option<String>,
    spoken_phrase: Option<String>,
) -> Result<Vec<Value>, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let _guard = SPEECH_MEMORY_TABLE_WRITE_LOCK
        .lock()
        .map_err(|_| "语音记忆写入锁不可用".to_string())?;
    let _file_lock = acquire_table_file_lock()?;
    let mut entries = load_entries_from(speech_memory_path())?;
    if apply_muscle_memory_hit(&mut entries, id.as_deref(), spoken_phrase.as_deref()) {
        return save_entries_to(speech_memory_path(), entries);
    }
    Ok(entries)
}

pub fn record_correction_memory_hit(
    id: Option<String>,
    observed_text: Option<String>,
    intended_text: Option<String>,
) -> Result<Vec<Value>, String> {
    record_correction_memory_counter(id, observed_text, intended_text, "hitCount")
}

/// feedback: "confirm" 增加 confirmCount，"reject" 增加 rejectCount。
/// rejectCount 此前在全代码库没有任何写入方，postprocess 的 rejectCount==0 门槛因此永久满足；
/// 这里先接通写入通道，UI 侧的"记住 / 忽略"闭环在观察值功能中接线。
pub fn record_correction_memory_feedback(
    id: Option<String>,
    observed_text: Option<String>,
    intended_text: Option<String>,
    feedback: String,
) -> Result<Vec<Value>, String> {
    let counter_key = match feedback.as_str() {
        "confirm" => "confirmCount",
        "reject" => "rejectCount",
        other => return Err(format!("未知的纠错反馈类型: {other}")),
    };
    record_correction_memory_counter(id, observed_text, intended_text, counter_key)
}

fn record_correction_memory_counter(
    id: Option<String>,
    observed_text: Option<String>,
    intended_text: Option<String>,
    counter_key: &str,
) -> Result<Vec<Value>, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let _guard = SPEECH_MEMORY_TABLE_WRITE_LOCK
        .lock()
        .map_err(|_| "语音记忆写入锁不可用".to_string())?;
    let _file_lock = acquire_table_file_lock()?;
    let mut entries = load_entries_from(speech_correction_memory_path())?;
    let now = Utc::now().to_rfc3339();
    if apply_correction_memory_counter(
        &mut entries,
        id.as_deref(),
        observed_text.as_deref(),
        intended_text.as_deref(),
        counter_key,
        &now,
    ) {
        return save_entries_to(speech_correction_memory_path(), entries);
    }
    Ok(entries)
}

fn default_vocabulary_store() -> SpeechVocabularyStore {
    SpeechVocabularyStore {
        version: SPEECH_VOCABULARY_VERSION,
        updated_at: Utc::now().to_rfc3339(),
        entries: Vec::new(),
    }
}

pub fn load_vocabulary_store() -> Result<SpeechVocabularyStore, String> {
    let path = speech_vocabulary_path();
    if !path.exists() {
        return Ok(default_vocabulary_store());
    }

    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取语音词典失败: {e}"))?;
    let store: SpeechVocabularyStore =
        serde_json::from_str(&raw).map_err(|e| format!("解析语音词典失败: {e}"))?;
    Ok(normalize_vocabulary_store(store))
}

pub fn record_vocabulary_terms(terms: Vec<String>) -> Result<SpeechVocabularyStore, String> {
    update_vocabulary_terms(terms, true)
}

pub fn merge_vocabulary_terms(terms: Vec<String>) -> Result<SpeechVocabularyStore, String> {
    update_vocabulary_terms(terms, false)
}

pub fn append_speech_history_markdown(text: String) -> Result<PathBuf, String> {
    let normalized = normalize_speech_history_text(&text);
    if normalized.is_empty() {
        return Err("语音最终稿为空".to_string());
    }

    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let time = now.format("%H:%M:%S").to_string();
    let path = speech_history_path_for_date(&date);
    let _guard = SPEECH_HISTORY_WRITE_LOCK
        .lock()
        .map_err(|_| "语音 Markdown 写入锁不可用".to_string())?;
    append_speech_history_at(&path, &date, &time, &normalized)?;
    Ok(path)
}

pub fn append_gpt_live_transcript_markdown(role: &str, text: &str) -> Result<PathBuf, String> {
    let role_label = match role {
        "user" => "你",
        "assistant" => "GPT-Live",
        _ => return Err("GPT-Live 转写角色无效".to_string()),
    };
    let normalized = normalize_speech_history_text(text);
    if normalized.is_empty() {
        return Err("GPT-Live 最终转写为空".to_string());
    }

    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let time = now.format("%H:%M:%S").to_string();
    let path = gpt_live_history_path();
    let _guard = GPT_LIVE_HISTORY_WRITE_LOCK
        .lock()
        .map_err(|_| "GPT-Live Markdown 写入锁不可用".to_string())?;
    append_gpt_live_history_at(&path, &date, &time, role_label, &normalized)?;
    Ok(path)
}

fn normalize_speech_history_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(20_000)
        .collect()
}

fn append_speech_history_at(
    path: &std::path::Path,
    date: &str,
    time: &str,
    text: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "语音 Markdown 缺少父目录".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建语音 Markdown 目录失败: {error}"))?;
    let needs_header = std::fs::metadata(path)
        .map(|metadata| metadata.len() == 0)
        .unwrap_or(true);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("打开语音 Markdown 失败: {error}"))?;
    if needs_header {
        writeln!(
            file,
            "# iterate Mac 语音记录 · {date}\n\n> 仅保存最终文字，不包含录音。\n"
        )
        .map_err(|error| format!("写入语音 Markdown 标题失败: {error}"))?;
    }
    writeln!(file, "- **{time}**：{text}")
        .map_err(|error| format!("追加语音 Markdown 失败: {error}"))?;
    file.flush()
        .map_err(|error| format!("刷新语音 Markdown 失败: {error}"))?;
    Ok(())
}

fn append_gpt_live_history_at(
    path: &std::path::Path,
    date: &str,
    time: &str,
    role_label: &str,
    text: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "GPT-Live Markdown 缺少父目录".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建 GPT-Live Markdown 目录失败: {error}"))?;
    let needs_header = std::fs::metadata(path)
        .map(|metadata| metadata.len() == 0)
        .unwrap_or(true);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("打开 GPT-Live Markdown 失败: {error}"))?;
    if needs_header {
        writeln!(
            file,
            "# iterate GPT-Live 实时语音记录\n\n> 自动保存用户与 GPT-Live 的最终转写，不包含录音。\n\n## 实时对话记录（自动追加）\n"
        )
        .map_err(|error| format!("写入 GPT-Live Markdown 标题失败: {error}"))?;
    }
    writeln!(file, "- **{date} {time} · {role_label}**：{text}")
        .map_err(|error| format!("追加 GPT-Live Markdown 失败: {error}"))?;
    file.flush()
        .map_err(|error| format!("刷新 GPT-Live Markdown 失败: {error}"))?;
    Ok(())
}

fn update_vocabulary_terms(
    terms: Vec<String>,
    increment_existing: bool,
) -> Result<SpeechVocabularyStore, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let _guard = SPEECH_VOCABULARY_WRITE_LOCK
        .lock()
        .map_err(|_| "语音词典写入锁不可用".to_string())?;
    let mut store = load_vocabulary_store()?;
    let now = Utc::now().to_rfc3339();
    let mut changed = false;

    for raw_term in terms.into_iter().take(100) {
        let Some(term) = normalize_vocabulary_term(&raw_term) else {
            continue;
        };
        let lookup = vocabulary_lookup_key(&term);
        if let Some(entry) = store
            .entries
            .iter_mut()
            .find(|entry| vocabulary_lookup_key(&entry.term) == lookup)
        {
            if increment_existing {
                entry.count = entry.count.saturating_add(1);
                entry.last_seen_at = now.clone();
                entry.term = term;
                changed = true;
            }
        } else {
            store.entries.push(SpeechVocabularyEntry {
                term,
                count: 1,
                first_seen_at: now.clone(),
                last_seen_at: now.clone(),
            });
            changed = true;
        }
    }

    if changed {
        store.updated_at = now;
        store = normalize_vocabulary_store(store);
        let raw =
            serde_json::to_vec_pretty(&store).map_err(|e| format!("序列化语音词典失败: {e}"))?;
        atomic_write(&speech_vocabulary_path(), &raw)?;
    }

    Ok(store)
}

fn normalize_vocabulary_store(mut store: SpeechVocabularyStore) -> SpeechVocabularyStore {
    let mut normalized = Vec::<SpeechVocabularyEntry>::new();
    for entry in store.entries {
        let Some(term) = normalize_vocabulary_term(&entry.term) else {
            continue;
        };
        let lookup = vocabulary_lookup_key(&term);
        if let Some(existing) = normalized
            .iter_mut()
            .find(|existing| vocabulary_lookup_key(&existing.term) == lookup)
        {
            existing.count = existing.count.saturating_add(entry.count.max(1));
            if entry.last_seen_at > existing.last_seen_at {
                existing.last_seen_at = entry.last_seen_at;
                existing.term = term;
            }
            if !entry.first_seen_at.is_empty()
                && (existing.first_seen_at.is_empty()
                    || entry.first_seen_at < existing.first_seen_at)
            {
                existing.first_seen_at = entry.first_seen_at;
            }
            continue;
        }
        normalized.push(SpeechVocabularyEntry {
            term,
            count: entry.count.max(1),
            first_seen_at: entry.first_seen_at,
            last_seen_at: entry.last_seen_at,
        });
    }

    normalized.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| right.last_seen_at.cmp(&left.last_seen_at))
            .then_with(|| left.term.to_lowercase().cmp(&right.term.to_lowercase()))
    });
    normalized.truncate(MAX_SPEECH_VOCABULARY_ENTRIES);
    store.version = SPEECH_VOCABULARY_VERSION;
    store.entries = normalized;
    store
}

fn vocabulary_lookup_key(value: &str) -> String {
    value.trim().to_lowercase()
}

fn normalize_vocabulary_term(value: &str) -> Option<String> {
    let term = value.trim().to_string();
    let char_count = term.chars().count();
    if term.is_empty()
        || char_count > MAX_SPEECH_VOCABULARY_TERM_LENGTH
        || (char_count < 2 && term != "回" && term != "派")
        || term.contains(['\n', '\r', '/', '\\', '@', '?', '='])
    {
        return None;
    }

    if !term
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '.' | '_' | ':' | '-'))
    {
        return None;
    }

    let lower = term.to_lowercase();
    let sensitive_markers = [
        "token",
        "secret",
        "password",
        "passwd",
        "apikey",
        "api_key",
        "private",
        "credential",
        "auth",
    ];
    if sensitive_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return None;
    }

    let digits = term
        .chars()
        .filter(|character| character.is_ascii_digit())
        .count();
    if term.chars().all(|character| character.is_ascii_digit()) || digits * 10 > char_count * 6 {
        return None;
    }

    let is_long_hex =
        char_count >= 12 && term.chars().all(|character| character.is_ascii_hexdigit());
    if is_long_hex {
        return None;
    }

    Some(term)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_vocabulary_filters_paths_secrets_and_runtime_ids() {
        assert_eq!(
            normalize_vocabulary_term("Codex"),
            Some("Codex".to_string())
        );
        assert_eq!(
            normalize_vocabulary_term("迭代语音"),
            Some("迭代语音".to_string())
        );
        assert_eq!(normalize_vocabulary_term("/Users/test/private"), None);
        assert_eq!(normalize_vocabulary_term("auth_token"), None);
        assert_eq!(normalize_vocabulary_term("019f51bab1657453"), None);
        assert_eq!(normalize_vocabulary_term("123456"), None);
    }

    #[test]
    fn speech_vocabulary_normalization_merges_case_insensitively_and_ranks_counts() {
        let store = normalize_vocabulary_store(SpeechVocabularyStore {
            version: 0,
            updated_at: "now".to_string(),
            entries: vec![
                SpeechVocabularyEntry {
                    term: "codex".to_string(),
                    count: 2,
                    first_seen_at: "2026-01-01".to_string(),
                    last_seen_at: "2026-01-02".to_string(),
                },
                SpeechVocabularyEntry {
                    term: "Codex".to_string(),
                    count: 3,
                    first_seen_at: "2026-01-03".to_string(),
                    last_seen_at: "2026-01-04".to_string(),
                },
                SpeechVocabularyEntry {
                    term: "Tauri".to_string(),
                    count: 1,
                    first_seen_at: "2026-01-01".to_string(),
                    last_seen_at: "2026-01-01".to_string(),
                },
            ],
        });

        assert_eq!(store.version, SPEECH_VOCABULARY_VERSION);
        assert_eq!(store.entries.len(), 2);
        assert_eq!(store.entries[0].term, "Codex");
        assert_eq!(store.entries[0].count, 5);
    }

    #[test]
    fn muscle_memory_hit_increments_first_match_only_and_preserves_unknown_fields() {
        let mut entries = vec![
            serde_json::json!({ "id": "a", "spokenPhrase": "call zhi", "trainingCount": 3, "custom": "keep" }),
            serde_json::json!({ "spokenPhrase": "call zhi", "trainingCount": 7 }),
        ];

        assert!(apply_muscle_memory_hit(&mut entries, Some("a"), None));
        assert_eq!(entries[0]["trainingCount"], 4);
        assert_eq!(entries[0]["custom"], "keep");
        assert_eq!(entries[1]["trainingCount"], 7);

        assert!(apply_muscle_memory_hit(
            &mut entries,
            None,
            Some("call zhi")
        ));
        assert_eq!(entries[0]["trainingCount"], 5);
        assert_eq!(entries[1]["trainingCount"], 7);

        assert!(!apply_muscle_memory_hit(
            &mut entries,
            None,
            Some("missing")
        ));
        assert!(!apply_muscle_memory_hit(&mut entries, None, None));
    }

    #[test]
    fn correction_memory_counters_update_matching_entry_and_stamp_updated_at() {
        let mut entries = vec![serde_json::json!({
            "id": "c1",
            "observedText": "sell",
            "intendedText": "style",
            "hitCount": 1,
            "confirmCount": 3,
        })];

        assert!(apply_correction_memory_counter(
            &mut entries,
            None,
            Some("sell"),
            Some("style"),
            "hitCount",
            "2026-07-26T00:00:00Z",
        ));
        assert_eq!(entries[0]["hitCount"], 2);
        assert_eq!(entries[0]["updatedAt"], "2026-07-26T00:00:00Z");

        assert!(apply_correction_memory_counter(
            &mut entries,
            Some("c1"),
            None,
            None,
            "rejectCount",
            "2026-07-26T00:00:01Z",
        ));
        assert_eq!(entries[0]["rejectCount"], 1);

        assert!(!apply_correction_memory_counter(
            &mut entries,
            None,
            Some("sell"),
            Some("elsewhere"),
            "hitCount",
            "2026-07-26T00:00:02Z",
        ));
    }

    #[test]
    fn correction_memory_counter_handles_float_counts_from_legacy_entries() {
        let mut entries = vec![serde_json::json!({
            "id": "legacy",
            "observedText": "a",
            "intendedText": "b",
            "hitCount": 2.0,
        })];

        assert!(apply_correction_memory_counter(
            &mut entries,
            Some("legacy"),
            None,
            None,
            "hitCount",
            "2026-07-26T00:00:00Z",
        ));
        assert_eq!(entries[0]["hitCount"], 3);
    }

    #[test]
    fn speech_history_is_searchable_markdown_without_multiline_structure_injection() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("2026-07-14.md");
        let text = normalize_speech_history_text("第一行\n\n# 第二行");
        append_speech_history_at(&path, "2026-07-14", "10:30:00", &text)
            .expect("append speech history");
        let saved = std::fs::read_to_string(path).expect("read speech history");

        assert!(saved.contains("# iterate Mac 语音记录 · 2026-07-14"));
        assert!(saved.contains("- **10:30:00**：第一行 # 第二行"));
        assert_eq!(saved.matches("# iterate Mac 语音记录").count(), 1);
    }

    #[test]
    fn gpt_live_history_records_both_roles_as_single_line_final_transcripts() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("GPT-Live.md");
        let user_text = normalize_speech_history_text("第一行\n\n# 第二行");
        append_gpt_live_history_at(&path, "2026-08-03", "21:30:00", "你", &user_text)
            .expect("append user transcript");
        append_gpt_live_history_at(
            &path,
            "2026-08-03",
            "21:30:01",
            "GPT-Live",
            "是否现在开始执行？",
        )
        .expect("append assistant transcript");
        let saved = std::fs::read_to_string(path).expect("read GPT-Live history");

        assert!(saved.contains("# iterate GPT-Live 实时语音记录"));
        assert!(saved.contains("## 实时对话记录（自动追加）"));
        assert!(saved.contains("- **2026-08-03 21:30:00 · 你**：第一行 # 第二行"));
        assert!(saved.contains("- **2026-08-03 21:30:01 · GPT-Live**：是否现在开始执行？"));
        assert_eq!(saved.matches("# iterate GPT-Live 实时语音记录").count(), 1);
    }
}
