use crate::ghost_suggestions::{self, UpsertGhostSuggestionRequest};
use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tempfile::NamedTempFile;

const LEARNING_VERSION: u32 = 2;
const ACCEPT_THRESHOLD: u64 = 2;
const TYPED_THRESHOLD: u64 = 3;
const MAX_ENTRIES: usize = 1_000;
const MAX_KEY_LENGTH: usize = 32;
const DIRECT_USER_INPUT_PROVENANCE: &str = "direct-user-input-v2";
const ACCEPTED_DESCRIPTION: &str = "自动学习 / 当前项目高频";
const TYPED_DESCRIPTION: &str = "自动学习 / 手动输入高频候选";

static LEARNING_WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static KEY_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:[\p{Letter}\p{Number}]|\.[\p{Letter}\p{Number}])[\p{Letter}\p{Number}_.:-]*$")
        .expect("valid ghost learning key regex")
});
static FILE_NAME_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[\p{Letter}\p{Number}_-]+\.[A-Za-z0-9]{1,8}$").expect("valid file name regex")
});
static UUID_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
        .expect("valid uuid regex")
});
static ISSUE_OR_RUNTIME_ID_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:p-\d{4}-\d+|t\d+|serve-\d+|sample-\d+|run-\d+)$")
        .expect("valid issue id regex")
});

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GhostSuggestionLearningEntry {
    pub key: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub accepted_count: u64,
    #[serde(default)]
    pub typed_count: u64,
    #[serde(default)]
    pub first_accepted_at: String,
    #[serde(default)]
    pub last_accepted_at: String,
    #[serde(default)]
    pub first_typed_at: String,
    #[serde(default)]
    pub last_typed_at: String,
    #[serde(default)]
    pub promoted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GhostSuggestionLearningStore {
    pub version: u32,
    pub entries: HashMap<String, GhostSuggestionLearningEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhostSuggestionLearningTerm {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecordGhostSuggestionLearningRequest {
    pub event: String,
    #[serde(default)]
    pub provenance: String,
    #[serde(default)]
    pub terms: Vec<GhostSuggestionLearningTerm>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GhostSuggestionLearningResult {
    pub state: GhostSuggestionLearningStore,
    pub promoted_keys: Vec<String>,
    #[serde(rename = "ghostSuggestions")]
    pub ghost_suggestions: Value,
}

pub fn ghost_suggestion_learning_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    Path::new(&home)
        .join(".cunzhi")
        .join("ghost-suggestion-learning.json")
}

pub fn default_store() -> GhostSuggestionLearningStore {
    GhostSuggestionLearningStore {
        version: LEARNING_VERSION,
        entries: HashMap::new(),
    }
}

pub fn load_store() -> Result<GhostSuggestionLearningStore, String> {
    let path = ghost_suggestion_learning_path();
    if !path.exists() {
        return Ok(default_store());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("读取幽灵补全学习账本失败: {error}"))?;
    let store: GhostSuggestionLearningStore =
        serde_json::from_str(&raw).map_err(|error| format!("解析幽灵补全学习账本失败: {error}"))?;
    let (store, migrated) = migrate_legacy_untrusted_typed_entries(store);
    if migrated {
        ghost_suggestions::remove_ghost_suggestions_by_description(TYPED_DESCRIPTION)?;
        write_store(&store)?;
    }
    Ok(store)
}

pub fn record_learning(
    request: RecordGhostSuggestionLearningRequest,
) -> Result<GhostSuggestionLearningResult, String> {
    let event = request.event.trim().to_lowercase();
    if event != "accepted" && event != "typed" {
        return Err("event 必须是 accepted 或 typed".to_string());
    }
    let _guard = LEARNING_WRITE_LOCK
        .lock()
        .map_err(|_| "幽灵补全学习账本写入锁不可用".to_string())?;
    let mut state = load_store()?;
    if should_ignore_unprovenanced_typed_event(&event, &request.provenance) {
        return Ok(GhostSuggestionLearningResult {
            state,
            promoted_keys: Vec::new(),
            ghost_suggestions: ghost_suggestions::load_store_value(),
        });
    }
    let allow_typed_promotion = event == "typed";
    let existing_keys = existing_ghost_suggestion_keys();
    let now = Utc::now().to_rfc3339();
    let mut seen = HashSet::new();

    for term in request.terms.into_iter().take(100) {
        let Some(key) = normalize_learning_key(&term.key) else {
            continue;
        };
        let lookup = key.to_lowercase();
        if existing_keys.contains(&lookup) || !seen.insert(lookup.clone()) {
            continue;
        }
        record_event_in_store(
            &mut state,
            &event,
            key,
            term.description.trim().to_string(),
            &now,
        );
    }

    let (promoted_keys, ghost_suggestions) =
        promote_ready_entries(&mut state, allow_typed_promotion)?;
    write_store(&state)?;
    Ok(GhostSuggestionLearningResult {
        state,
        promoted_keys,
        ghost_suggestions,
    })
}

fn should_ignore_unprovenanced_typed_event(event: &str, provenance: &str) -> bool {
    event == "typed" && provenance.trim() != DIRECT_USER_INPUT_PROVENANCE
}

pub fn merge_legacy_store(
    incoming: GhostSuggestionLearningStore,
) -> Result<GhostSuggestionLearningResult, String> {
    let _guard = LEARNING_WRITE_LOCK
        .lock()
        .map_err(|_| "幽灵补全学习账本写入锁不可用".to_string())?;
    let mut state = load_store()?;
    let incoming = normalize_store(incoming);
    for (lookup, incoming_entry) in incoming.entries {
        if let Some(current) = state.entries.get_mut(&lookup) {
            current.accepted_count = current.accepted_count.max(incoming_entry.accepted_count);
            current.typed_count = current.typed_count.max(incoming_entry.typed_count);
            current.promoted |= incoming_entry.promoted;
            if current.description.is_empty() {
                current.description = incoming_entry.description;
            }
            current.first_accepted_at = earliest_nonempty(
                &current.first_accepted_at,
                &incoming_entry.first_accepted_at,
            );
            current.last_accepted_at = current
                .last_accepted_at
                .clone()
                .max(incoming_entry.last_accepted_at);
            current.first_typed_at =
                earliest_nonempty(&current.first_typed_at, &incoming_entry.first_typed_at);
            current.last_typed_at = current
                .last_typed_at
                .clone()
                .max(incoming_entry.last_typed_at);
        } else {
            state.entries.insert(lookup, incoming_entry);
        }
    }
    state = normalize_store(state);
    let (promoted_keys, ghost_suggestions) = promote_ready_entries(&mut state, false)?;
    write_store(&state)?;
    Ok(GhostSuggestionLearningResult {
        state,
        promoted_keys,
        ghost_suggestions,
    })
}

fn record_event_in_store(
    state: &mut GhostSuggestionLearningStore,
    event: &str,
    key: String,
    description: String,
    now: &str,
) {
    let lookup = key.to_lowercase();
    let entry = state
        .entries
        .entry(lookup)
        .or_insert_with(|| GhostSuggestionLearningEntry {
            key: key.clone(),
            description: description.clone(),
            accepted_count: 0,
            typed_count: 0,
            first_accepted_at: String::new(),
            last_accepted_at: String::new(),
            first_typed_at: String::new(),
            last_typed_at: String::new(),
            promoted: false,
        });
    entry.key = key;
    if !description.is_empty() {
        entry.description = description;
    }
    if event == "accepted" {
        entry.accepted_count = entry.accepted_count.saturating_add(1);
        if entry.first_accepted_at.is_empty() {
            entry.first_accepted_at = now.to_string();
        }
        entry.last_accepted_at = now.to_string();
    } else {
        entry.typed_count = entry.typed_count.saturating_add(1);
        if entry.first_typed_at.is_empty() {
            entry.first_typed_at = now.to_string();
        }
        entry.last_typed_at = now.to_string();
    }
}

fn promote_ready_entries(
    state: &mut GhostSuggestionLearningStore,
    allow_typed_promotion: bool,
) -> Result<(Vec<String>, Value), String> {
    let mut promoted_keys = Vec::new();
    let ready = state
        .entries
        .values()
        .filter_map(|entry| {
            promotion_description(entry, allow_typed_promotion)
                .map(|description| (entry.key.clone(), description.to_string()))
        })
        .collect::<Vec<_>>();

    let mut ghost_suggestions = ghost_suggestions::load_store_value();
    for (key, description) in ready {
        ghost_suggestions =
            ghost_suggestions::upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                key: key.clone(),
                description: Some(description),
                enabled: Some(true),
                sort_order: None,
                expected_updated_at: None,
            })?;
        if let Some(entry) = state.entries.get_mut(&key.to_lowercase()) {
            entry.promoted = true;
        }
        promoted_keys.push(key);
    }

    Ok((promoted_keys, ghost_suggestions))
}

fn promotion_description(
    entry: &GhostSuggestionLearningEntry,
    allow_typed_promotion: bool,
) -> Option<&'static str> {
    if entry.promoted {
        return None;
    }
    if entry.accepted_count >= ACCEPT_THRESHOLD {
        return Some(ACCEPTED_DESCRIPTION);
    }
    if allow_typed_promotion && entry.typed_count >= TYPED_THRESHOLD {
        return Some(TYPED_DESCRIPTION);
    }
    None
}

fn existing_ghost_suggestion_keys() -> HashSet<String> {
    ghost_suggestions::load_store_value()
        .get("suggestions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("key").and_then(Value::as_str))
        .map(|key| key.trim().to_lowercase())
        .collect()
}

fn normalize_store(store: GhostSuggestionLearningStore) -> GhostSuggestionLearningStore {
    let mut merged = HashMap::<String, GhostSuggestionLearningEntry>::new();
    for mut entry in store.entries.into_values() {
        let Some(key) = normalize_learning_key(&entry.key) else {
            continue;
        };
        let lookup = key.to_lowercase();
        entry.key = key;
        if let Some(current) = merged.get_mut(&lookup) {
            current.accepted_count = current.accepted_count.saturating_add(entry.accepted_count);
            current.typed_count = current.typed_count.saturating_add(entry.typed_count);
            current.promoted |= entry.promoted;
            if current.description.is_empty() {
                current.description = entry.description;
            }
            current.first_accepted_at =
                earliest_nonempty(&current.first_accepted_at, &entry.first_accepted_at);
            current.last_accepted_at = current.last_accepted_at.clone().max(entry.last_accepted_at);
            current.first_typed_at =
                earliest_nonempty(&current.first_typed_at, &entry.first_typed_at);
            current.last_typed_at = current.last_typed_at.clone().max(entry.last_typed_at);
        } else {
            merged.insert(lookup, entry);
        }
    }
    let mut entries = merged.into_iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        let left_score = left.1.accepted_count.saturating_mul(2) + left.1.typed_count;
        let right_score = right.1.accepted_count.saturating_mul(2) + right.1.typed_count;
        right_score
            .cmp(&left_score)
            .then_with(|| right.1.last_typed_at.cmp(&left.1.last_typed_at))
    });
    entries.truncate(MAX_ENTRIES);
    GhostSuggestionLearningStore {
        version: LEARNING_VERSION,
        entries: entries.into_iter().collect(),
    }
}

fn migrate_legacy_untrusted_typed_entries(
    store: GhostSuggestionLearningStore,
) -> (GhostSuggestionLearningStore, bool) {
    let should_migrate = store.version < LEARNING_VERSION;
    let mut store = normalize_store(store);
    if !should_migrate {
        return (store, false);
    }

    store.entries.retain(|_, entry| entry.accepted_count > 0);
    for entry in store.entries.values_mut() {
        entry.promoted = entry.accepted_count >= ACCEPT_THRESHOLD;
    }
    (store, true)
}

fn normalize_learning_key(value: &str) -> Option<String> {
    let key = value.trim().to_string();
    let char_count = key.chars().count();
    if char_count < 3 || char_count > MAX_KEY_LENGTH || !KEY_PATTERN.is_match(&key) {
        return None;
    }
    if FILE_NAME_PATTERN.is_match(&key) || UUID_PATTERN.is_match(&key) {
        return None;
    }
    let lower = key.to_lowercase();
    if ISSUE_OR_RUNTIME_ID_PATTERN.is_match(&lower)
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("passwd")
        || lower.contains("apikey")
        || lower.contains("api_key")
        || lower.contains("private")
        || lower.contains("credential")
        || lower.contains("auth")
        || lower
            .split(['.', '_', ':', '-'])
            .any(|part| matches!(part, "env" | "id" | "uuid" | "hash" | "sha" | "commit"))
    {
        return None;
    }
    let digits = key
        .chars()
        .filter(|character| character.is_ascii_digit())
        .count();
    if key.chars().all(|character| character.is_ascii_digit()) || digits * 10 > char_count * 6 {
        return None;
    }
    if char_count >= 12 && key.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    Some(key)
}

fn earliest_nonempty(left: &str, right: &str) -> String {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => String::new(),
        (true, false) => right.to_string(),
        (false, true) => left.to_string(),
        (false, false) => left.min(right).to_string(),
    }
}

fn write_store(store: &GhostSuggestionLearningStore) -> Result<(), String> {
    let path = ghost_suggestion_learning_path();
    let parent = path
        .parent()
        .ok_or_else(|| "幽灵补全学习账本缺少父目录".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("创建学习账本目录失败: {error}"))?;
    let mut temp = NamedTempFile::new_in(parent)
        .map_err(|error| format!("创建学习账本临时文件失败: {error}"))?;
    let raw = serde_json::to_vec_pretty(store)
        .map_err(|error| format!("序列化幽灵补全学习账本失败: {error}"))?;
    temp.write_all(&raw)
        .map_err(|error| format!("写入学习账本临时文件失败: {error}"))?;
    temp.flush()
        .map_err(|error| format!("刷新学习账本临时文件失败: {error}"))?;
    temp.persist(path)
        .map_err(|error| format!("替换幽灵补全学习账本失败: {}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_sensitive_or_unhelpful_learning_keys() {
        assert_eq!(
            normalize_learning_key("activity"),
            Some("activity".to_string())
        );
        assert_eq!(normalize_learning_key("global_rules.md"), None);
        assert_eq!(normalize_learning_key("auth_token"), None);
        assert_eq!(normalize_learning_key("019f51bab1657453"), None);
        assert_eq!(normalize_learning_key("123456"), None);
    }

    #[test]
    fn records_counts_without_storing_submitted_sentences() {
        let mut store = default_store();
        record_event_in_store(
            &mut store,
            "typed",
            "activity".to_string(),
            TYPED_DESCRIPTION.to_string(),
            "2026-07-14T00:00:00Z",
        );
        record_event_in_store(
            &mut store,
            "accepted",
            "activity".to_string(),
            ACCEPTED_DESCRIPTION.to_string(),
            "2026-07-14T00:01:00Z",
        );

        let entry = store.entries.get("activity").expect("learning entry");
        assert_eq!(entry.typed_count, 1);
        assert_eq!(entry.accepted_count, 1);
        assert!(!serde_json::to_string(&store)
            .expect("serialize")
            .contains("submitted sentence"));
    }

    #[test]
    fn normalization_merges_case_variants_without_losing_counts() {
        let mut entries = HashMap::new();
        entries.insert(
            "first".to_string(),
            GhostSuggestionLearningEntry {
                key: "Activity".to_string(),
                description: String::new(),
                accepted_count: 1,
                typed_count: 2,
                first_accepted_at: "2026-07-14T00:00:00Z".to_string(),
                last_accepted_at: "2026-07-14T00:00:00Z".to_string(),
                first_typed_at: "2026-07-14T00:00:00Z".to_string(),
                last_typed_at: "2026-07-14T00:00:00Z".to_string(),
                promoted: false,
            },
        );
        entries.insert(
            "second".to_string(),
            GhostSuggestionLearningEntry {
                key: "activity".to_string(),
                description: TYPED_DESCRIPTION.to_string(),
                accepted_count: 2,
                typed_count: 3,
                first_accepted_at: "2026-07-13T00:00:00Z".to_string(),
                last_accepted_at: "2026-07-15T00:00:00Z".to_string(),
                first_typed_at: "2026-07-13T00:00:00Z".to_string(),
                last_typed_at: "2026-07-15T00:00:00Z".to_string(),
                promoted: true,
            },
        );

        let normalized = normalize_store(GhostSuggestionLearningStore {
            version: 0,
            entries,
        });
        let entry = normalized.entries.get("activity").expect("merged entry");
        assert_eq!(entry.accepted_count, 3);
        assert_eq!(entry.typed_count, 5);
        assert!(entry.promoted);
        assert_eq!(entry.first_typed_at, "2026-07-13T00:00:00Z");
        assert_eq!(entry.last_typed_at, "2026-07-15T00:00:00Z");
    }

    #[test]
    fn version_two_migration_discards_unaccepted_legacy_typed_entries() {
        let mut entries = HashMap::new();
        entries.insert(
            "ai-rule-term".to_string(),
            GhostSuggestionLearningEntry {
                key: "ai-rule-term".to_string(),
                description: TYPED_DESCRIPTION.to_string(),
                accepted_count: 0,
                typed_count: 3,
                first_accepted_at: String::new(),
                last_accepted_at: String::new(),
                first_typed_at: "2026-08-02T00:00:00Z".to_string(),
                last_typed_at: "2026-08-03T00:00:00Z".to_string(),
                promoted: true,
            },
        );
        entries.insert(
            "user-accepted".to_string(),
            GhostSuggestionLearningEntry {
                key: "user-accepted".to_string(),
                description: ACCEPTED_DESCRIPTION.to_string(),
                accepted_count: 1,
                typed_count: 3,
                first_accepted_at: "2026-08-03T00:00:00Z".to_string(),
                last_accepted_at: "2026-08-03T00:00:00Z".to_string(),
                first_typed_at: "2026-08-02T00:00:00Z".to_string(),
                last_typed_at: "2026-08-03T00:00:00Z".to_string(),
                promoted: true,
            },
        );

        let (migrated, changed) =
            migrate_legacy_untrusted_typed_entries(GhostSuggestionLearningStore {
                version: 1,
                entries,
            });

        assert!(changed);
        assert_eq!(migrated.version, LEARNING_VERSION);
        assert!(!migrated.entries.contains_key("ai-rule-term"));
        assert!(!migrated.entries["user-accepted"].promoted);
    }

    #[test]
    fn typed_threshold_requires_explicit_provenanced_promotion() {
        let mut store = default_store();
        record_event_in_store(
            &mut store,
            "typed",
            "activity".to_string(),
            TYPED_DESCRIPTION.to_string(),
            "2026-08-30T00:00:00Z",
        );
        record_event_in_store(
            &mut store,
            "typed",
            "activity".to_string(),
            TYPED_DESCRIPTION.to_string(),
            "2026-08-30T00:01:00Z",
        );
        record_event_in_store(
            &mut store,
            "typed",
            "activity".to_string(),
            TYPED_DESCRIPTION.to_string(),
            "2026-08-30T00:02:00Z",
        );

        let entry = store.entries.get("activity").expect("learning entry");
        assert_eq!(promotion_description(entry, false), None);
        assert_eq!(promotion_description(entry, true), Some(TYPED_DESCRIPTION));
    }

    #[test]
    fn typed_learning_requires_direct_user_input_v2_provenance() {
        assert!(should_ignore_unprovenanced_typed_event("typed", ""));
        assert!(should_ignore_unprovenanced_typed_event("typed", "legacy"));
        assert!(!should_ignore_unprovenanced_typed_event(
            "typed",
            "direct-user-input-v2"
        ));
        assert!(!should_ignore_unprovenanced_typed_event("accepted", ""));
    }
}
