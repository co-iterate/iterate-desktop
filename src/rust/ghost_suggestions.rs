use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const DEFAULT_SEED_VERSION: u32 = 0;
const MAX_KEY_LENGTH: usize = 32;

static GHOST_SUGGESTION_KEY_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:[\p{Letter}\p{Number}]|\.[\p{Letter}\p{Number}])[\p{Letter}\p{Number}_.:-]*$")
        .expect("valid ghost suggestion key regex")
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhostSuggestion {
    pub id: String,
    pub key: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub sort_order: u32,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhostSuggestionStore {
    pub version: u32,
    #[serde(rename = "defaultSeedVersion")]
    pub default_seed_version: u32,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub suggestions: Vec<GhostSuggestion>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertGhostSuggestionRequest {
    #[serde(default)]
    pub key: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub sort_order: Option<u32>,
    #[serde(rename = "expectedUpdatedAt")]
    pub expected_updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateGhostSuggestionRequest {
    pub key: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    #[serde(rename = "expectedUpdatedAt")]
    pub expected_updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoveGhostSuggestionRequest {
    #[serde(rename = "expectedUpdatedAt")]
    pub expected_updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReorderGhostSuggestionsRequest {
    #[serde(default)]
    pub ids: Vec<String>,
    #[serde(rename = "expectedUpdatedAt")]
    pub expected_updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReplaceGhostSuggestionsRequest {
    #[serde(default)]
    pub suggestions: Vec<GhostSuggestion>,
    #[serde(rename = "expectedUpdatedAt")]
    pub expected_updated_at: Option<String>,
}

pub const CONFLICT_ERROR_CODE: &str = "ghost_suggestions_conflict";
pub const NOT_FOUND_ERROR_CODE: &str = "ghost_suggestion_not_found";
pub const REORDER_ERROR_CODE: &str = "ghost_suggestions_reorder_invalid";
pub const REPLACE_ERROR_CODE: &str = "ghost_suggestions_replace_invalid";

fn default_enabled() -> bool {
    true
}

pub fn ghost_suggestions_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    Path::new(&home)
        .join(".cunzhi")
        .join("ghost-suggestions.json")
}

pub fn default_store() -> GhostSuggestionStore {
    GhostSuggestionStore {
        version: 1,
        default_seed_version: DEFAULT_SEED_VERSION,
        updated_at: Utc::now().to_rfc3339(),
        suggestions: Vec::new(),
    }
}

pub fn load_store_content() -> Result<String, String> {
    let path = ghost_suggestions_path();
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| format!("读取幽灵补全词表失败: {e}"))
    } else {
        let store = default_store();
        serde_json::to_string(&store).map_err(|e| format!("序列化默认幽灵补全词表失败: {e}"))
    }
}

pub fn load_store_value() -> Value {
    match load_store() {
        Ok(store) => serde_json::to_value(store)
            .unwrap_or_else(|_| serde_json::to_value(default_store()).unwrap_or(Value::Null)),
        Err(_) => serde_json::to_value(default_store()).unwrap_or(Value::Null),
    }
}

pub fn save_store_from_content(content: String) -> Result<Value, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let value: Value =
        serde_json::from_str(&content).map_err(|e| format!("解析幽灵补全词表失败: {e}"))?;
    let mut store = normalize_store_value(&value);
    if store.updated_at.is_empty() {
        store.updated_at = Utc::now().to_rfc3339();
    }
    write_store(&store)?;
    store_to_value(&store)
}

pub fn upsert_ghost_suggestion(request: UpsertGhostSuggestionRequest) -> Result<Value, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let key = normalize_key(&request.key);
    validate_key(&key)?;

    let now = Utc::now().to_rfc3339();
    let mut store = load_store()?;
    ensure_expected_updated_at(&store, request.expected_updated_at.as_deref())?;
    let key_lower = key.to_lowercase();
    let description = request.description.unwrap_or_default().trim().to_string();

    if let Some(existing) = store
        .suggestions
        .iter_mut()
        .find(|suggestion| suggestion.key.to_lowercase() == key_lower)
    {
        existing.key = key;
        if !description.is_empty() {
            existing.description = description;
        }
        if let Some(enabled) = request.enabled {
            existing.enabled = enabled;
        }
        if let Some(sort_order) = request.sort_order {
            existing.sort_order = sort_order;
        }
        existing.updated_at = now.clone();
    } else {
        let sort_order = request
            .sort_order
            .unwrap_or_else(|| store.suggestions.len() as u32 + 1);
        store.suggestions.push(GhostSuggestion {
            id: create_id(&key, &now),
            key,
            description,
            enabled: request.enabled.unwrap_or(true),
            sort_order,
            created_at: now.clone(),
            updated_at: now.clone(),
        });
    }

    store.updated_at = now;
    store.suggestions = normalize_sort(store.suggestions);
    write_store(&store)?;
    store_to_value(&store)
}

pub fn update_ghost_suggestion(
    id: &str,
    request: UpdateGhostSuggestionRequest,
) -> Result<Value, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let mut store = load_store()?;
    ensure_expected_updated_at(&store, request.expected_updated_at.as_deref())?;

    let Some(index) = store
        .suggestions
        .iter()
        .position(|suggestion| suggestion.id == id)
    else {
        return Err(NOT_FOUND_ERROR_CODE.to_string());
    };

    let next_key = request.key.as_deref().map(normalize_key);
    if let Some(next_key) = next_key.as_ref() {
        validate_key(next_key)?;
        let next_key_lower = next_key.to_lowercase();
        if store
            .suggestions
            .iter()
            .enumerate()
            .any(|(other_index, item)| {
                other_index != index && item.key.to_lowercase() == next_key_lower
            })
        {
            return Err("触发词已存在".to_string());
        }
    }

    let now = Utc::now().to_rfc3339();
    let suggestion = &mut store.suggestions[index];
    if let Some(next_key) = next_key {
        suggestion.key = next_key;
    }
    if let Some(description) = request.description {
        suggestion.description = description.trim().to_string();
    }
    if let Some(enabled) = request.enabled {
        suggestion.enabled = enabled;
    }
    suggestion.updated_at = now.clone();

    store.updated_at = now;
    store.suggestions = normalize_sort(store.suggestions);
    write_store(&store)?;
    store_to_value(&store)
}

pub fn remove_ghost_suggestion(
    id: &str,
    request: RemoveGhostSuggestionRequest,
) -> Result<Value, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let mut store = load_store()?;
    ensure_expected_updated_at(&store, request.expected_updated_at.as_deref())?;

    let original_len = store.suggestions.len();
    store.suggestions.retain(|suggestion| suggestion.id != id);
    if store.suggestions.len() == original_len {
        return Err(NOT_FOUND_ERROR_CODE.to_string());
    }

    store.updated_at = Utc::now().to_rfc3339();
    store.suggestions = normalize_sort(store.suggestions);
    write_store(&store)?;
    store_to_value(&store)
}

pub fn reorder_ghost_suggestions(request: ReorderGhostSuggestionsRequest) -> Result<Value, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let mut store = load_store()?;
    ensure_expected_updated_at(&store, request.expected_updated_at.as_deref())?;

    let ids: Vec<String> = request
        .ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    let unique_ids: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
    let current_ids: std::collections::HashSet<&str> = store
        .suggestions
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    if ids.len() != store.suggestions.len()
        || unique_ids.len() != ids.len()
        || unique_ids != current_ids
    {
        return Err(REORDER_ERROR_CODE.to_string());
    }

    let sort_positions: std::collections::HashMap<&str, u32> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index as u32 + 1))
        .collect();
    let now = Utc::now().to_rfc3339();
    for suggestion in &mut store.suggestions {
        suggestion.sort_order = sort_positions
            .get(suggestion.id.as_str())
            .copied()
            .unwrap_or(suggestion.sort_order);
        suggestion.updated_at = now.clone();
    }

    store.updated_at = now;
    store.suggestions = normalize_sort(store.suggestions);
    write_store(&store)?;
    store_to_value(&store)
}

pub fn replace_ghost_suggestions(request: ReplaceGhostSuggestionsRequest) -> Result<Value, String> {
    let _sync_lock = crate::cross_device::settings_sync::content_lock()?;
    let mut store = load_store()?;
    ensure_expected_updated_at(&store, request.expected_updated_at.as_deref())?;

    let now = Utc::now().to_rfc3339();
    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_keys = std::collections::HashSet::new();
    let mut suggestions = Vec::with_capacity(request.suggestions.len());

    for (index, mut suggestion) in request.suggestions.into_iter().enumerate() {
        suggestion.id = suggestion.id.trim().to_string();
        suggestion.key = normalize_key(&suggestion.key);
        suggestion.description = suggestion.description.trim().to_string();
        if suggestion.id.is_empty()
            || !seen_ids.insert(suggestion.id.clone())
            || validate_key(&suggestion.key).is_err()
            || !seen_keys.insert(suggestion.key.to_lowercase())
        {
            return Err(REPLACE_ERROR_CODE.to_string());
        }

        suggestion.sort_order = index as u32 + 1;
        if suggestion.created_at.trim().is_empty() {
            suggestion.created_at = now.clone();
        }
        suggestion.updated_at = now.clone();
        suggestions.push(suggestion);
    }

    store.updated_at = now;
    store.suggestions = suggestions;
    write_store(&store)?;
    store_to_value(&store)
}

fn load_store() -> Result<GhostSuggestionStore, String> {
    let content = load_store_content()?;
    let value: Value =
        serde_json::from_str(&content).map_err(|e| format!("解析幽灵补全词表失败: {e}"))?;
    Ok(normalize_store_value(&value))
}

fn write_store(store: &GhostSuggestionStore) -> Result<(), String> {
    let path = ghost_suggestions_path();
    let parent = path
        .parent()
        .ok_or_else(|| "幽灵补全词表路径缺少父目录".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("创建幽灵补全目录失败: {e}"))?;
    let json =
        serde_json::to_string_pretty(store).map_err(|e| format!("序列化幽灵补全词表失败: {e}"))?;

    let mut temp_file =
        NamedTempFile::new_in(parent).map_err(|e| format!("创建幽灵补全临时文件失败: {e}"))?;
    temp_file
        .write_all(json.as_bytes())
        .map_err(|e| format!("写入幽灵补全临时文件失败: {e}"))?;
    temp_file
        .as_file_mut()
        .sync_all()
        .map_err(|e| format!("同步幽灵补全临时文件失败: {e}"))?;
    temp_file
        .persist(&path)
        .map(|_| ())
        .map_err(|e| format!("替换幽灵补全词表失败: {}", e.error))
}

fn ensure_expected_updated_at(
    store: &GhostSuggestionStore,
    expected_updated_at: Option<&str>,
) -> Result<(), String> {
    let Some(expected_updated_at) = expected_updated_at
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };

    if store.updated_at == expected_updated_at {
        Ok(())
    } else {
        Err(CONFLICT_ERROR_CODE.to_string())
    }
}

fn normalize_store_value(value: &Value) -> GhostSuggestionStore {
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
    let default_seed_version = value
        .get("defaultSeedVersion")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_SEED_VERSION as u64) as u32;
    let updated_at = value
        .get("updatedAt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let suggestions = value
        .get("suggestions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| normalize_suggestion_value(item, index))
                .collect()
        })
        .unwrap_or_default();

    GhostSuggestionStore {
        version,
        default_seed_version,
        updated_at,
        suggestions: normalize_sort(dedupe_suggestions(suggestions)),
    }
}

fn normalize_suggestion_value(value: &Value, index: usize) -> Option<GhostSuggestion> {
    let object = value.as_object()?;
    let key = normalize_key(object.get("key")?.as_str()?);
    if validate_key(&key).is_err() {
        return None;
    }

    let now = Utc::now().to_rfc3339();
    Some(GhostSuggestion {
        id: object
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| create_id(&key, &now)),
        key,
        description: object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        enabled: object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        sort_order: object
            .get("sort_order")
            .and_then(Value::as_u64)
            .filter(|order| *order > 0)
            .map(|order| order as u32)
            .unwrap_or(index as u32 + 1),
        created_at: object
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or(&now)
            .to_string(),
        updated_at: object
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or(&now)
            .to_string(),
    })
}

fn normalize_key(key: &str) -> String {
    key.trim().to_string()
}

pub(crate) fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("触发词不能为空".to_string());
    }
    if key.chars().count() > MAX_KEY_LENGTH {
        return Err(format!("触发词不能超过 {MAX_KEY_LENGTH} 个字符"));
    }
    if !GHOST_SUGGESTION_KEY_PATTERN.is_match(key) {
        return Err(
            "触发词需以文字、数字或点号文件扩展名开头，仅支持文字、数字、下划线、点、冒号和短横线"
                .to_string(),
        );
    }
    Ok(())
}

fn normalize_sort(mut items: Vec<GhostSuggestion>) -> Vec<GhostSuggestion> {
    items.sort_by_key(|item| item.sort_order);
    items
        .into_iter()
        .enumerate()
        .map(|(index, mut item)| {
            item.sort_order = index as u32 + 1;
            item
        })
        .collect()
}

fn dedupe_suggestions(items: Vec<GhostSuggestion>) -> Vec<GhostSuggestion> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.key.to_lowercase()))
        .collect()
}

fn create_id(key: &str, timestamp: &str) -> String {
    let key_part: String = key
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "backend_{}_{}",
        key_part,
        timestamp.replace([':', '.', '-'], "")
    )
}

fn store_to_value(store: &GhostSuggestionStore) -> Result<Value, String> {
    serde_json::to_value(store).map_err(|e| format!("序列化幽灵补全词表失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use serde_json::json;
    use std::ffi::OsString;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static HOME_ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    struct HomeGuard {
        previous: Option<OsString>,
        _temp_dir: TempDir,
    }

    impl HomeGuard {
        fn new() -> Self {
            let temp_dir = tempfile::tempdir().expect("temp HOME");
            let previous = std::env::var_os("HOME");
            std::env::set_var("HOME", temp_dir.path());

            Self {
                previous,
                _temp_dir: temp_dir,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.as_ref() {
                std::env::set_var("HOME", previous);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }

    fn with_temp_home<T>(test: impl FnOnce() -> T) -> T {
        let _lock = HOME_ENV_LOCK.lock().expect("HOME env lock");
        let _guard = HomeGuard::new();
        test()
    }

    fn store_from_value(value: Value) -> GhostSuggestionStore {
        serde_json::from_value(value).expect("ghost suggestion store")
    }

    fn read_store_file() -> GhostSuggestionStore {
        let content =
            std::fs::read_to_string(ghost_suggestions_path()).expect("ghost suggestions file");
        serde_json::from_str(&content).expect("ghost suggestions JSON")
    }

    #[test]
    fn save_store_from_content_normalizes_invalid_duplicates_and_sort_order() {
        with_temp_home(|| {
            let store = json!({
                "version": 3,
                "defaultSeedVersion": 9,
                "updatedAt": "2026-05-21T00:00:00Z",
                "suggestions": [
                    {
                        "id": "hui-first",
                        "key": " hui ",
                        "description": " 项目记忆 ",
                        "enabled": true,
                        "sort_order": 20,
                        "created_at": "old",
                        "updated_at": "old"
                    },
                    {
                        "id": "invalid",
                        "key": "bad key",
                        "description": "invalid",
                        "sort_order": 1
                    },
                    {
                        "id": "hui-duplicate",
                        "key": "HUI",
                        "description": "duplicate",
                        "sort_order": 1
                    },
                    {
                        "id": "dot-md",
                        "key": ".md",
                        "description": " Markdown ",
                        "enabled": false,
                        "sort_order": 5
                    },
                    {
                        "id": "alpha",
                        "key": "alpha",
                        "description": " Alpha ",
                        "enabled": true,
                        "sort_order": 50
                    }
                ]
            });

            let result = save_store_from_content(store.to_string()).expect("save store");
            let saved = store_from_value(result);

            assert_eq!(saved.version, 3);
            assert_eq!(saved.default_seed_version, 9);
            assert_eq!(saved.updated_at, "2026-05-21T00:00:00Z");
            assert_eq!(saved.suggestions.len(), 3);

            assert_eq!(saved.suggestions[0].id, "dot-md");
            assert_eq!(saved.suggestions[0].key, ".md");
            assert_eq!(saved.suggestions[0].description, "Markdown");
            assert!(!saved.suggestions[0].enabled);
            assert_eq!(saved.suggestions[0].sort_order, 1);

            assert_eq!(saved.suggestions[1].id, "hui-first");
            assert_eq!(saved.suggestions[1].key, "hui");
            assert_eq!(saved.suggestions[1].description, "项目记忆");
            assert_eq!(saved.suggestions[1].sort_order, 2);

            assert_eq!(saved.suggestions[2].id, "alpha");
            assert_eq!(saved.suggestions[2].key, "alpha");
            assert_eq!(saved.suggestions[2].description, "Alpha");
            assert_eq!(saved.suggestions[2].sort_order, 3);

            assert_eq!(read_store_file().suggestions.len(), 3);
        });
    }

    #[test]
    fn upsert_inserts_and_updates_existing_suggestion_case_insensitively() {
        with_temp_home(|| {
            let inserted = upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                key: " activity ".to_string(),
                description: Some(" project ".to_string()),
                enabled: Some(false),
                sort_order: Some(5),
                expected_updated_at: None,
            })
            .expect("insert suggestion");
            let inserted_store = store_from_value(inserted);

            assert_eq!(inserted_store.suggestions.len(), 1);
            assert_eq!(inserted_store.suggestions[0].key, "activity");
            assert_eq!(inserted_store.suggestions[0].description, "project");
            assert!(!inserted_store.suggestions[0].enabled);
            assert_eq!(inserted_store.suggestions[0].sort_order, 1);

            let original_id = inserted_store.suggestions[0].id.clone();
            let original_created_at = inserted_store.suggestions[0].created_at.clone();

            let updated = upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                key: "Activity".to_string(),
                description: Some("".to_string()),
                enabled: Some(true),
                sort_order: Some(10),
                expected_updated_at: None,
            })
            .expect("update suggestion");
            let updated_store = store_from_value(updated);

            assert_eq!(updated_store.suggestions.len(), 1);
            assert_eq!(updated_store.suggestions[0].id, original_id);
            assert_eq!(updated_store.suggestions[0].created_at, original_created_at);
            assert_eq!(updated_store.suggestions[0].key, "Activity");
            assert_eq!(updated_store.suggestions[0].description, "project");
            assert!(updated_store.suggestions[0].enabled);
            assert_eq!(updated_store.suggestions[0].sort_order, 1);

            assert_eq!(read_store_file().suggestions[0].key, "Activity");
        });
    }

    #[test]
    fn write_store_leaves_only_parseable_canonical_file() {
        with_temp_home(|| {
            upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                key: "activity".to_string(),
                description: Some("project".to_string()),
                enabled: None,
                sort_order: None,
                expected_updated_at: None,
            })
            .expect("insert suggestion");

            let path = ghost_suggestions_path();
            let raw = std::fs::read_to_string(&path).expect("store file");
            let parsed: Value = serde_json::from_str(&raw).expect("parseable store file");
            assert_eq!(parsed["suggestions"][0]["key"], "activity");

            let mut files = std::fs::read_dir(path.parent().expect("store parent"))
                .expect("store directory")
                .map(|entry| {
                    entry
                        .expect("directory entry")
                        .file_name()
                        .to_string_lossy()
                        .to_string()
                })
                .collect::<Vec<_>>();
            files.sort();
            assert_eq!(files, vec!["ghost-suggestions.json".to_string()]);
        });
    }

    #[test]
    fn upsert_rejects_missing_or_invalid_keys_before_writing() {
        with_temp_home(|| {
            let missing = upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                key: String::new(),
                description: None,
                enabled: None,
                sort_order: None,
                expected_updated_at: None,
            })
            .expect_err("missing key should fail");
            assert!(missing.contains("触发词不能为空"));

            let invalid = upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                key: "bad key".to_string(),
                description: None,
                enabled: None,
                sort_order: None,
                expected_updated_at: None,
            })
            .expect_err("invalid key should fail");
            assert!(invalid.contains("触发词需以文字"));

            assert!(!ghost_suggestions_path().exists());
        });
    }

    #[test]
    fn id_mutations_reorder_preserves_manual_order_and_rejects_stale_writes() {
        with_temp_home(|| {
            let alpha = store_from_value(
                upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                    key: "alpha".to_string(),
                    description: Some("first".to_string()),
                    enabled: None,
                    sort_order: None,
                    expected_updated_at: None,
                })
                .expect("insert alpha"),
            );
            let alpha_id = alpha.suggestions[0].id.clone();

            let beta = store_from_value(
                upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                    key: "beta".to_string(),
                    description: Some("second".to_string()),
                    enabled: None,
                    sort_order: None,
                    expected_updated_at: Some(alpha.updated_at),
                })
                .expect("insert beta"),
            );
            let beta_id = beta.suggestions[1].id.clone();

            let reordered = store_from_value(
                reorder_ghost_suggestions(ReorderGhostSuggestionsRequest {
                    ids: vec![beta_id.clone(), alpha_id.clone()],
                    expected_updated_at: Some(beta.updated_at),
                })
                .expect("reorder suggestions"),
            );
            assert_eq!(reordered.suggestions[0].id, beta_id);
            assert_eq!(reordered.suggestions[1].id, alpha_id);

            let updated = store_from_value(
                update_ghost_suggestion(
                    &alpha_id,
                    UpdateGhostSuggestionRequest {
                        key: Some("gamma_edited".to_string()),
                        description: Some(String::new()),
                        enabled: Some(false),
                        expected_updated_at: Some(reordered.updated_at),
                    },
                )
                .expect("update alpha"),
            );
            assert_eq!(updated.suggestions[0].id, beta_id);
            assert_eq!(updated.suggestions[1].id, alpha_id);
            assert_eq!(updated.suggestions[1].key, "gamma_edited");
            assert_eq!(updated.suggestions[1].description, "");
            assert!(!updated.suggestions[1].enabled);

            let stale_remove = remove_ghost_suggestion(
                &beta_id,
                RemoveGhostSuggestionRequest {
                    expected_updated_at: Some("2026-05-20T00:00:00Z".to_string()),
                },
            )
            .expect_err("stale remove");
            assert_eq!(stale_remove, CONFLICT_ERROR_CODE);

            let removed = store_from_value(
                remove_ghost_suggestion(
                    &beta_id,
                    RemoveGhostSuggestionRequest {
                        expected_updated_at: Some(updated.updated_at),
                    },
                )
                .expect("remove beta"),
            );
            assert_eq!(removed.suggestions.len(), 1);
            assert_eq!(removed.suggestions[0].id, alpha_id);
        });
    }

    #[test]
    fn reorder_requires_the_complete_current_id_set() {
        with_temp_home(|| {
            let store = store_from_value(
                upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                    key: "only".to_string(),
                    description: None,
                    enabled: None,
                    sort_order: None,
                    expected_updated_at: None,
                })
                .expect("insert suggestion"),
            );

            let invalid = reorder_ghost_suggestions(ReorderGhostSuggestionsRequest {
                ids: vec!["missing".to_string()],
                expected_updated_at: Some(store.updated_at),
            })
            .expect_err("invalid id set");
            assert_eq!(invalid, REORDER_ERROR_CODE);
        });
    }

    #[test]
    fn replace_store_applies_batch_changes_atomically_and_supports_snapshot_undo() {
        with_temp_home(|| {
            let alpha = store_from_value(
                upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                    key: "alpha".to_string(),
                    description: None,
                    enabled: Some(true),
                    sort_order: None,
                    expected_updated_at: None,
                })
                .expect("insert alpha"),
            );
            let beta = store_from_value(
                upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                    key: "beta".to_string(),
                    description: None,
                    enabled: Some(true),
                    sort_order: None,
                    expected_updated_at: Some(alpha.updated_at),
                })
                .expect("insert beta"),
            );
            let original = store_from_value(
                upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                    key: "gamma".to_string(),
                    description: None,
                    enabled: Some(true),
                    sort_order: None,
                    expected_updated_at: Some(beta.updated_at),
                })
                .expect("insert gamma"),
            );

            let mut gamma = original.suggestions[2].clone();
            gamma.enabled = false;
            let desired = vec![gamma, original.suggestions[0].clone()];
            let changed = store_from_value(
                replace_ghost_suggestions(ReplaceGhostSuggestionsRequest {
                    suggestions: desired,
                    expected_updated_at: Some(original.updated_at.clone()),
                })
                .expect("apply atomic batch"),
            );

            assert_eq!(
                changed
                    .suggestions
                    .iter()
                    .map(|item| item.key.as_str())
                    .collect::<Vec<_>>(),
                vec!["gamma", "alpha"]
            );
            assert!(!changed.suggestions[0].enabled);

            let stale = replace_ghost_suggestions(ReplaceGhostSuggestionsRequest {
                suggestions: original.suggestions.clone(),
                expected_updated_at: Some(original.updated_at.clone()),
            })
            .expect_err("stale snapshot must not overwrite a newer batch");
            assert_eq!(stale, CONFLICT_ERROR_CODE);
            assert_eq!(read_store_file().suggestions.len(), 2);

            let restored = store_from_value(
                replace_ghost_suggestions(ReplaceGhostSuggestionsRequest {
                    suggestions: original.suggestions,
                    expected_updated_at: Some(changed.updated_at),
                })
                .expect("undo from full snapshot"),
            );
            assert_eq!(
                restored
                    .suggestions
                    .iter()
                    .map(|item| item.key.as_str())
                    .collect::<Vec<_>>(),
                vec!["alpha", "beta", "gamma"]
            );
            assert!(restored.suggestions.iter().all(|item| item.enabled));
        });
    }

    #[test]
    fn write_mutations_do_not_overwrite_invalid_store_file() {
        with_temp_home(|| {
            let path = ghost_suggestions_path();
            std::fs::create_dir_all(path.parent().expect("store parent")).expect("store dir");
            std::fs::write(&path, "{not-json").expect("invalid store");

            let error = upsert_ghost_suggestion(UpsertGhostSuggestionRequest {
                key: "safe".to_string(),
                description: None,
                enabled: None,
                sort_order: None,
                expected_updated_at: None,
            })
            .expect_err("invalid store should block write");

            assert!(error.contains("解析幽灵补全词表失败"));
            assert_eq!(
                std::fs::read_to_string(&path).expect("store content"),
                "{not-json"
            );
        });
    }

    #[test]
    fn load_store_value_falls_back_to_default_when_file_is_invalid_json() {
        with_temp_home(|| {
            let path = ghost_suggestions_path();
            std::fs::create_dir_all(path.parent().expect("store parent")).expect("store dir");
            std::fs::write(&path, "{not-json").expect("invalid store");

            let value = load_store_value();
            let store = store_from_value(value);

            assert_eq!(store.version, 1);
            assert_eq!(store.default_seed_version, DEFAULT_SEED_VERSION);
            assert!(store.suggestions.is_empty());
        });
    }
}
