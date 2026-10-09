//! Selected settings exchange with backend-owned previews and explicit local apply.
use super::{transport, Result};
use crate::config::AppConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Read,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Appearance,
    Window,
    Audio,
    Shortcuts,
    Reply,
    PromptTemplates,
    PromptLibrary,
    GhostSuggestions,
    SpeechReplacements,
    SpeechCorrections,
    SpeechVocabulary,
    AutoContinue,
    ClipboardBackup,
    McpTools,
    AutoCheckpoint,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub categories: Vec<Category>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub schema_version: u32,
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub categories: BTreeMap<Category, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Difference {
    pub category: Category,
    pub local: Value,
    pub incoming: Value,
    /// Matching local entries whose content will be replaced.
    pub conflicts: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub preview_id: String,
    pub pairing_binding: String,
    pub content_hash: String,
    pub local_hash: String,
    pub snapshot: Snapshot,
    pub local_snapshot: BTreeMap<Category, Value>,
    pub differences: Vec<Difference>,
    pub warnings: Vec<String>,
}

// The later apply command must retrieve this backend-owned document by ID, not
// trust a client-supplied snapshot or hash. Closing the process discards previews.
static PREVIEWS: OnceLock<Mutex<BTreeMap<String, (Instant, Preview)>>> = OnceLock::new();
fn retain_preview(preview: &Preview) -> Result<()> {
    let mut cache = PREVIEWS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "预览缓存不可用")?;
    cache.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(900));
    if cache.len() >= 4 {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (time, _))| *time)
            .map(|(id, _)| id.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(
        preview.preview_id.clone(),
        (Instant::now(), preview.clone()),
    );
    Ok(())
}

fn selections(mut categories: Vec<Category>) -> Result<Vec<Category>> {
    if categories.is_empty() || categories.len() > 15 {
        return Err("请选择有效的同步分类".into());
    }
    categories.sort();
    if categories.windows(2).any(|p| p[0] == p[1]) {
        return Err("同步分类重复".into());
    }
    Ok(categories)
}

fn hash(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(hex::encode(ring::digest::digest(
        &ring::digest::SHA256,
        &bytes,
    )))
}

pub fn bounded_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err("设置快照超过 1 MiB，请减少所选分类".into());
    }
    Ok(bytes)
}

// Reading optional libraries never creates files, normalizes statistics or hides corrupt JSON.
fn read_optional(path: &Path, empty: Value) -> Result<Value> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(empty),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take((MAX_SNAPSHOT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err("本机内容库过大".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "本机内容库 JSON 无效".into())
}

// A schema consists exclusively of allowed primitive fields. Missing required content
// is an error; optional fields have explicit defaults. Unknown local fields are ignored.
fn entry_schema(category: Category) -> &'static [(&'static str, char, bool)] {
    use Category::*;
    match category {
        PromptTemplates => &[
            ("id", 's', true),
            ("name", 's', true),
            ("content", 's', true),
            ("description", 's', false),
            ("sort_order", 'n', false),
            ("type", 's', true),
            ("condition_text", 's', false),
            ("template_true", 's', false),
            ("template_false", 's', false),
            ("current_state", 'b', true),
            ("is_active", 'b', true),
        ],
        PromptLibrary => &[
            ("id", 's', true),
            ("name", 's', true),
            ("content", 's', true),
            ("category", 's', true),
        ],
        GhostSuggestions => &[
            ("id", 's', true),
            ("key", 's', true),
            ("description", 's', false),
            ("enabled", 'b', true),
            ("sort_order", 'n', false),
        ],
        SpeechReplacements => &[
            ("id", 's', true),
            ("spokenPhrase", 's', true),
            ("outputText", 's', true),
            ("isEnabled", 'b', true),
        ],
        SpeechCorrections => &[
            ("id", 's', true),
            ("observedText", 's', true),
            ("intendedText", 's', true),
            ("contextTerms", 'a', false),
            ("isEnabled", 'b', false),
        ],
        SpeechVocabulary => &[("term", 's', true)],
        _ => &[],
    }
}

fn project_entries(category: Category, value: &Value) -> Result<Value> {
    let entries = value.as_array().ok_or("内容库必须是数组")?;
    let mut out = Vec::new();
    for entry in entries {
        let object = entry.as_object().ok_or("内容条目必须是对象")?;
        let mut row = serde_json::Map::new();
        for &(name, kind, required) in entry_schema(category) {
            let fallback = match kind {
                'n' => json!(0),
                'b' => json!(true),
                'a' => json!([]),
                _ => json!(""),
            };
            let v = match object.get(name).filter(|v| !v.is_null()) {
                Some(v) => v.clone(),
                None if !required => fallback,
                None => return Err(format!("内容条目缺少 {name}")),
            };
            if !primitive_matches(kind, &v) {
                return Err(format!("内容字段 {name} 类型无效"));
            }
            row.insert(name.into(), v);
        }
        out.push(Value::Object(row));
    }
    Ok(Value::Array(out))
}

fn primitive_matches(kind: char, value: &Value) -> bool {
    match kind {
        's' => value.is_string(),
        'b' => value.is_boolean(),
        'n' => value.is_number(),
        'a' => value
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string)),
        _ => false,
    }
}

fn config_category(config: &AppConfig, category: Category) -> Option<Value> {
    use Category::*;
    let ui = &config.ui_config;
    let reply = &config.reply_config;
    Some(match category {
        Appearance => {
            json!({"theme":ui.theme,"font_family":if ui.font_config.font_family == "custom" {"system"} else {&ui.font_config.font_family},"font_size":ui.font_config.font_size})
        }
        Window => {
            let w = &ui.window_config;
            json!({"always_on_top":ui.always_on_top,"auto_resize":w.auto_resize,"max_width":w.max_width,"max_height":w.max_height,"min_width":w.min_width,"min_height":w.min_height,"fixed":w.fixed,"fixed_width":w.fixed_width,"fixed_height":w.fixed_height,"free_width":w.free_width,"free_height":w.free_height})
        }
        Audio => json!({"notification_enabled":config.audio_config.notification_enabled}),
        Reply => {
            json!({"continue_prompt":reply.continue_prompt,"loop_prompt":reply.loop_prompt,"goal_prompt_template":reply.goal_prompt_template})
        }
        AutoContinue => {
            json!({"enable_continue_reply":reply.enable_continue_reply,"auto_continue_threshold":reply.auto_continue_threshold})
        }
        ClipboardBackup => {
            json!({"copy_submission_to_clipboard":reply.copy_submission_to_clipboard})
        }
        AutoCheckpoint => {
            json!({"auto_checkpoint_enabled":config.checkpoint_config.auto_checkpoint_enabled})
        }
        McpTools => {
            let tools: BTreeMap<_, _> = crate::config::default_mcp_tools()
                .into_iter()
                .map(|(key, default)| {
                    let enabled = config
                        .mcp_config
                        .tools
                        .get(&key)
                        .copied()
                        .unwrap_or(default);
                    (key, enabled)
                })
                .collect();
            json!({"tools":tools})
        }
        Shortcuts => {
            let bindings: BTreeMap<_,_> = config.shortcut_config.shortcuts.iter().map(|(id,s)| (id.clone(),json!({"id":s.id,"name":s.name,"description":s.description,"action":s.action,"enabled":s.enabled,"scope":s.scope,"key_combination":{"key":s.key_combination.key,"ctrl":s.key_combination.ctrl,"alt":s.key_combination.alt,"shift":s.key_combination.shift,"meta":s.key_combination.meta}}))).collect();
            json!({"global_enabled":config.shortcut_config.global_enabled,"shortcuts":bindings})
        }
        _ => return None,
    })
}

fn local_categories(categories: &[Category]) -> Result<BTreeMap<Category, Value>> {
    let config = crate::config::load_standalone_config().map_err(|e| e.to_string())?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let library_dir = Path::new(&home).join(".cunzhi");
    let mut values = BTreeMap::new();
    for &category in categories {
        let value = if let Some(value) = config_category(&config, category) {
            value
        } else {
            use Category::*;
            let raw = match category {
                PromptTemplates => Value::Array(config.custom_prompt_config.prompts.iter().map(|p| json!({"id":p.id,"name":p.name,"content":p.content,"description":p.description,"sort_order":p.sort_order,"type":p.r#type,"condition_text":p.condition_text,"template_true":p.template_true,"template_false":p.template_false,"current_state":p.current_state,"is_active":p.is_active})).collect()),
                PromptLibrary => read_optional(&library_dir.join("prompt-library.json"),json!({"items":[]}))?.get("items").cloned().ok_or("提示词库缺少 items")?,
                GhostSuggestions => read_optional(&crate::ghost_suggestions::ghost_suggestions_path(),json!({"suggestions":[]}))?.get("suggestions").cloned().ok_or("幽灵词库缺少 suggestions")?,
                SpeechReplacements => read_optional(&crate::speech_memory::speech_memory_path(),json!([]))?,
                SpeechCorrections => read_optional(&crate::speech_memory::speech_correction_memory_path(),json!([]))?,
                SpeechVocabulary => read_optional(&crate::speech_memory::speech_vocabulary_path(),json!({"entries":[]}))?.get("entries").cloned().ok_or("语音词库缺少 entries")?,
                _ => unreachable!(),
            };
            let entries = project_entries(category, &raw)?;
            if category == Category::PromptTemplates {
                json!({"enabled":config.custom_prompt_config.enabled,"max_prompts":config.custom_prompt_config.max_prompts,"prompts":entries})
            } else {
                entries
            }
        };
        values.insert(category, value);
    }
    Ok(values)
}

/// Local-only command used by the settings screen; the network route is TLS paired.
#[tauri::command]
pub fn settings_sync_export(categories: Vec<Category>) -> Result<Snapshot> {
    let categories = selections(categories)?;
    // Do not call settings(): its first-read initialization would write files.
    let identity: super::Settings = super::read_json(&super::directory()?.join("settings.json"))?;
    let snapshot = Snapshot {
        version: super::VERSION,
        schema_version: 1,
        device_id: identity.device_id,
        device_name: transport::connection_config()?.device_name,
        platform: std::env::consts::OS.into(),
        categories: local_categories(&categories)?,
    };
    bounded_bytes(&snapshot)?;
    Ok(snapshot)
}

// Reject extra/missing remote fields, including nested fields, rather than retaining
// unknown payloads for a later writer. Dynamic shortcut IDs only contain this schema.
fn same_shape(value: &Value, schema: &Value) -> bool {
    match (value, schema) {
        (Value::Object(v), Value::Object(s)) => {
            v.len() == s.len()
                && s.iter()
                    .all(|(k, t)| v.get(k).is_some_and(|x| same_shape(x, t)))
        }
        (Value::String(_), Value::String(_)) | (Value::Bool(_), Value::Bool(_)) => true,
        (Value::Number(v), Value::Number(s)) => {
            if s.is_u64() {
                v.is_u64()
            } else {
                true
            }
        }
        _ => false,
    }
}

fn validate_snapshot(snapshot: &Snapshot, categories: &[Category]) -> Result<()> {
    if snapshot.version != super::VERSION
        || snapshot.schema_version != 1
        || snapshot.device_id.is_empty()
        || snapshot.categories.keys().copied().collect::<Vec<_>>() != categories
    {
        return Err("对端快照身份、版本或分类无效".into());
    }
    bounded_bytes(snapshot)?;
    for (&category, value) in &snapshot.categories {
        if category == Category::PromptTemplates {
            let obj = value.as_object().ok_or("模板格式无效")?;
            let entries = value.get("prompts").ok_or("模板缺少 prompts")?;
            if obj.len() != 3
                || !value.get("enabled").is_some_and(Value::is_boolean)
                || !value.get("max_prompts").is_some_and(Value::is_u64)
                || project_entries(category, entries)? != *entries
            {
                return Err("模板包含未知或缺失字段".into());
            }
        } else if !entry_schema(category).is_empty() {
            if project_entries(category, value)? != *value {
                return Err("对端内容包含未知或缺失字段".into());
            }
        } else if category == Category::Shortcuts {
            let object = value.as_object().ok_or("快捷键格式无效")?;
            let bindings = value
                .get("shortcuts")
                .and_then(Value::as_object)
                .ok_or("快捷键格式无效")?;
            let schema = json!({"id":"","name":"","description":"","action":"","enabled":true,"scope":"","key_combination":{"key":"","ctrl":true,"alt":true,"shift":true,"meta":true}});
            if object.len() != 2
                || !value.get("global_enabled").is_some_and(Value::is_boolean)
                || !bindings.values().all(|v| same_shape(v, &schema))
            {
                return Err("快捷键包含未知或缺失字段".into());
            }
        } else {
            let schema = config_category(&AppConfig::default(), category).ok_or("未知分类")?;
            if !same_shape(value, &schema) {
                return Err("对端设置包含未知、缺失或类型无效字段".into());
            }
        }
    }
    Ok(())
}

fn conflict_keys(category: Category) -> &'static [&'static str] {
    use Category::*;
    match category {
        PromptTemplates | PromptLibrary => &["id", "name"],
        GhostSuggestions => &["id", "key"],
        SpeechReplacements => &["id", "spokenPhrase"],
        SpeechCorrections => &["id", "observedText"],
        SpeechVocabulary => &["term"],
        _ => &[],
    }
}

fn conflicts(category: Category, local: &Value, incoming: &Value) -> Vec<Value> {
    let local = if category == Category::PromptTemplates {
        &local["prompts"]
    } else {
        local
    };
    let incoming = if category == Category::PromptTemplates {
        &incoming["prompts"]
    } else {
        incoming
    };
    let Some(existing) = local.as_array() else {
        return vec![];
    };
    incoming
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| {
            existing.iter().any(|old| {
                old != *item
                    && conflict_keys(category).iter().any(|key| {
                        let a = old.get(*key).and_then(Value::as_str).unwrap_or("").trim();
                        let b = item.get(*key).and_then(Value::as_str).unwrap_or("").trim();
                        !a.is_empty() && a == b
                    })
            })
        })
        .cloned()
        .collect()
}

fn preview(
    snapshot: Snapshot,
    local: BTreeMap<Category, Value>,
    binding: String,
) -> Result<Preview> {
    let categories: Vec<_> = snapshot.categories.keys().copied().collect();
    validate_snapshot(&snapshot, &categories)?;
    let differences: Vec<Difference> = snapshot
        .categories
        .iter()
        .filter_map(|(&category, incoming)| {
            let before = local.get(&category)?;
            (before != incoming).then(|| Difference {
                category,
                local: before.clone(),
                incoming: incoming.clone(),
                conflicts: conflicts(category, before, incoming),
            })
        })
        .collect();
    let mut warnings=vec!["配对端的所选设置将替换本机对应类别：同名内容被覆盖，本机独有条目被删除，来源为空时清空该类别。未勾选类别、IP、凭据、配对和本机路径不变。".into()];
    for category in categories {
        use Category::*;
        let warning = match category {
            Appearance => {
                "自定义字体不导出，来源自定义字体按系统字体预览；目标端缺少字体时使用系统回退。"
            }
            Window => "将改变本机窗口尺寸、置顶和自动调整。",
            Audio => "将改变提醒发声开关；本机音效文件保持不变。",
            Shortcuts => "不同操作系统的快捷键可能无效或冲突，不自动转换 Ctrl/Cmd。",
            Reply | PromptTemplates | PromptLibrary => {
                "提示词会改变以后发送给 AI 的附加指令，请检查内容。"
            }
            AutoContinue => "将改变自动继续及其触发阈值。",
            ClipboardBackup => "将改变提交内容自动复制到本机剪贴板的行为。",
            McpTools => "将改变 MCP 工具的可用状态。",
            AutoCheckpoint => "自动检查点开启后会按现有机制创建 Git 检查点。",
            SpeechReplacements | SpeechCorrections => {
                "不复制对端训练或使用统计；内容未变的规则保留本机统计，新建或改变内容的规则训练次数归零。语音替换规则须在本机训练至少 4 次后才生效。"
            }
            SpeechVocabulary => "只导入词汇内容，不复制学习统计。",
            _ => continue,
        };
        warnings.push(warning.into());
    }
    Ok(Preview {
        preview_id: uuid::Uuid::new_v4().to_string(),
        pairing_binding: binding,
        content_hash: hash(&snapshot)?,
        local_hash: hash(&local)?,
        snapshot,
        local_snapshot: local,
        differences,
        warnings,
    })
}

#[tauri::command]
pub async fn settings_sync_preview(categories: Vec<Category>) -> Result<Preview> {
    let categories = selections(categories)?;
    let binding = transport::route_key()?;
    let mut response = transport::peer(
        "/api/settings-sync/snapshot",
        Some(&json!({"categories":categories})),
    )
    .await?;
    // These are locally attached transport observations, never snapshot content.
    response
        .as_object_mut()
        .ok_or("对端快照格式无效")?
        .remove("connected_ip");
    response.as_object_mut().unwrap().remove("using_backup");
    let snapshot: Snapshot = serde_json::from_value(response).map_err(|_| "对端快照字段无效")?;
    validate_snapshot(&snapshot, &categories)?;
    if transport::route_key()? != binding {
        return Err("配对或连接已改变，请重新预览".into());
    }
    let local = local_categories(&categories)?;
    let result = preview(snapshot, local, binding)?;
    retain_preview(&result)?;
    Ok(result)
}

/// Shared by content writers and imports; all read/modify/write work belongs
/// inside this lock, including the comparison against the preview.
pub(crate) fn content_lock() -> Result<std::fs::File> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    super::lock(&Path::new(&home).join(".cunzhi"), "settings-content.lock")
}

#[derive(Serialize)]
pub struct ApplyResult {
    pub category: Category,
    pub success: bool,
    pub message: String,
}

fn matches_entry(category: Category, a: &Value, b: &Value) -> bool {
    conflict_keys(category).iter().any(|key| {
        let left = a.get(*key).and_then(Value::as_str).unwrap_or("").trim();
        let right = b.get(*key).and_then(Value::as_str).unwrap_or("").trim();
        !left.is_empty() && left == right
    })
}

fn replace_entries(category: Category, current: &[Value], incoming: &Value) -> Result<Vec<Value>> {
    let mut replacement = Vec::new();
    let now = chrono::Utc::now().to_rfc3339();
    for entry in incoming.as_array().ok_or("内容必须是数组")? {
        if replacement.iter().any(|old| matches_entry(category, old, entry)) {
            return Err("配对端内容含重复标识或名称，请先在配对端修正".into());
        }
        // Keep local metadata only for unchanged content, never import peer statistics.
        if let Some(old) = current.iter().find(|old| matches_entry(category, old, entry)) {
            if project_entries(category, &json!([old]))? == json!([entry]) {
                replacement.push(old.clone());
                continue;
            }
        }
        let mut entry = entry.clone();
        let row = entry.as_object_mut().ok_or("条目无效")?;
        for key in conflict_keys(category) {
            let value = row.get(*key).and_then(Value::as_str).unwrap_or("").trim().to_string();
            if value.is_empty() { return Err(format!("条目 {key} 不能为空")); }
        }
        match category {
            Category::PromptTemplates => {
                row.insert("created_at".into(), json!(now));
                row.insert("updated_at".into(), json!(now));
            }
            Category::GhostSuggestions => {
                crate::ghost_suggestions::validate_key(row["key"].as_str().unwrap())?;
                row.insert("created_at".into(), json!(now));
                row.insert("updated_at".into(), json!(now));
                serde_json::from_value::<crate::ghost_suggestions::GhostSuggestion>(Value::Object(row.clone()))
                    .map_err(|e| format!("幽灵词条无效：{e}"))?;
            }
            Category::SpeechReplacements | Category::SpeechCorrections => {
                row.insert("trainingCount".into(), json!(0));
                row.insert("createdAt".into(), json!(now));
                row.insert("updatedAt".into(), json!(now));
            }
            Category::SpeechVocabulary => {
                if row["term"].as_str().unwrap().chars().count() > 48 { return Err("语音词汇最多 48 个字符".into()); }
                row.insert("count".into(), json!(0));
                row.insert("first_seen_at".into(), json!(now));
                row.insert("last_seen_at".into(), json!(now));
            }
            _ => {}
        }
        replacement.push(entry);
    }
    Ok(replacement)
}

fn update_config_category(config: &mut AppConfig, category: Category, incoming: &Value) -> Result<()> {
    use Category::*;
    let mut value = serde_json::to_value(&*config).map_err(|e| e.to_string())?;
    let section = match category {
        Appearance => {
            value["ui_config"]["theme"] = incoming["theme"].clone();
            for key in ["font_family", "font_size"] {
                value["ui_config"]["font_config"][key] = incoming[key].clone();
            }
            None
        }
        Window => {
            value["ui_config"]["always_on_top"] = incoming["always_on_top"].clone();
            for (key, field) in incoming.as_object().ok_or("窗口设置无效")? {
                if key != "always_on_top" { value["ui_config"]["window_config"][key] = field.clone(); }
            }
            None
        }
        Audio => Some("audio_config"),
        Reply | AutoContinue | ClipboardBackup => Some("reply_config"),
        AutoCheckpoint => Some("checkpoint_config"),
        Shortcuts => Some("shortcut_config"),
        McpTools => Some("mcp_config"),
        PromptTemplates => {
            let current = value["custom_prompt_config"]["prompts"].as_array().ok_or("本机模板无效")?;
            let entries = replace_entries(category, current, &incoming["prompts"])?;
            value["custom_prompt_config"]["prompts"] = json!(entries);
            value["custom_prompt_config"]["enabled"] = incoming["enabled"].clone();
            value["custom_prompt_config"]["max_prompts"] = incoming["max_prompts"].clone();
            None
        }
        _ => return Err("未知配置分类".into()),
    };
    if let Some(section) = section {
        for (key, field) in incoming.as_object().ok_or("配置分类无效")? {
            if category == McpTools && key == "tools" {
                for (id, enabled) in field.as_object().ok_or("工具配置无效")? {
                    value[section]["tools"][id] = enabled.clone();
                }
            } else { value[section][key] = field.clone(); }
        }
    }
    let next: AppConfig = serde_json::from_value(value).map_err(|e| format!("设置值无效：{e}"))?;
    if category == Appearance {
        if !["light", "dark"].contains(&next.ui_config.theme.as_str())
            || !crate::constants::font::FONT_FAMILIES.iter().any(|(id, _, _)| *id == next.ui_config.font_config.font_family && *id != "custom")
            || !crate::constants::font::FONT_SIZES.iter().any(|(id, _, _)| *id == next.ui_config.font_config.font_size) {
            return Err("对端主题或字体选项不受本机支持".into());
        }
    }
    if category == Window {
        let w = &next.ui_config.window_config;
        if ![w.min_width,w.min_height,w.max_width,w.max_height,w.fixed_width,w.fixed_height,w.free_width,w.free_height].iter().all(|v| v.is_finite() && *v > 0.0)
            || w.min_width > w.max_width || w.min_height > w.max_height {
            return Err("对端窗口尺寸无效".into());
        }
    }
    *config = next;
    Ok(())
}

fn apply_content(category: Category, incoming: &Value) -> Result<String> {
    use Category::*;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let (path, key, empty) = match category {
        PromptLibrary => (Path::new(&home).join(".cunzhi/prompt-library.json"), Some("items"), json!({"version":1,"items":[]})),
        GhostSuggestions => (crate::ghost_suggestions::ghost_suggestions_path(), Some("suggestions"), json!({"version":1,"defaultSeedVersion":0,"suggestions":[]})),
        SpeechReplacements => (crate::speech_memory::speech_memory_path(), None, json!([])),
        SpeechCorrections => (crate::speech_memory::speech_correction_memory_path(), None, json!([])),
        SpeechVocabulary => (crate::speech_memory::speech_vocabulary_path(), Some("entries"), json!({"version":1,"entries":[]})),
        _ => return Err("未知内容分类".into()),
    };
    let mut document = read_optional(&path, empty)?;
    let current = match key { Some(key) => &document[key], None => &document }.as_array().ok_or("本机内容无效")?;
    let entries = replace_entries(category, current, incoming)?;
    if category == SpeechVocabulary && entries.len() > 500 { return Err("来源语音词库超过 500 条，请减少来源词汇".into()); }
    let count = entries.len();
    if &entries != current {
        if let Some(key) = key {
            document[key] = json!(entries);
            document[if category == SpeechVocabulary { "updated_at" } else { "updatedAt" }] = json!(chrono::Utc::now().to_rfc3339());
        } else { document = json!(entries); }
        super::atomic_json(&path, &document)?;
    }
    Ok(format!("已替换为 {count} 条"))
}

#[tauri::command]
pub fn settings_sync_apply(preview_id: String, state: tauri::State<'_, crate::config::AppState>, app: tauri::AppHandle) -> Result<Vec<ApplyResult>> {
    use tauri::{Emitter, Manager};
    let document = PREVIEWS.get_or_init(Default::default).lock().map_err(|_| "预览缓存不可用")?
        .remove(&preview_id).filter(|(time, _)| time.elapsed() < Duration::from_secs(900))
        .map(|(_, value)| value).ok_or("预览已过期或已使用，请重新预览")?;
    let _connection = super::lock(&super::directory()?, "connection.lock")?;
    if transport::route_key()? != document.pairing_binding { return Err("配对或地址已改变，请重新预览".into()); }
    let _contents = content_lock()?;
    let local_keys: Vec<_> = document.local_snapshot.keys().copied().collect();
    if local_categories(&local_keys)? != document.local_snapshot {
        return Err("本机所选设置或关联内容库已改变，请重新预览".into());
    }
    let mut results = Vec::new();
    for (&category, incoming) in &document.snapshot.categories {
        let outcome = (|| -> Result<String> {
            if local_categories(&[category])?[&category] != document.local_snapshot[&category] {
                return Err("本机设置已改变，请重新预览".into());
            }
            if incoming == &document.local_snapshot[&category] { return Ok("无变化".into()); }
            if config_category(&AppConfig::default(), category).is_some() || category == Category::PromptTemplates {
                let mut memory = state.config.lock().map_err(|_| "本机设置锁不可用")?;
                let baseline = memory.save_baseline.clone().unwrap_or(serde_json::to_value(AppConfig::default()).map_err(|e| e.to_string())?);
                if serde_json::to_value(&*memory).map_err(|e| e.to_string())? != baseline {
                    return Err("本机窗口还有未保存设置，请先保存或重新加载".into());
                }
                let saved = crate::config::update_config_locked(|config| {
                    let current = if category == Category::PromptTemplates {
                        // Content lock also serializes template imports; the file
                        // lock protects ordinary template edits.
                        let raw = json!({"enabled":config.custom_prompt_config.enabled,"max_prompts":config.custom_prompt_config.max_prompts,"prompts":project_entries(category, &serde_json::to_value(&config.custom_prompt_config.prompts)?) .map_err(anyhow::Error::msg)?});
                        raw
                    } else { config_category(config, category).unwrap() };
                    anyhow::ensure!(current == document.local_snapshot[&category], "本机设置已改变，请重新预览");
                    update_config_category(config, category, incoming).map_err(anyhow::Error::msg)
                }).map_err(|e| e.to_string())?;
                *memory = saved;
                state.global_shortcut_enabled.store(memory.shortcut_config.global_enabled, std::sync::atomic::Ordering::Relaxed);
                if category == Category::Window {
                    if let Some(window) = app.get_webview_window("main") {
                        let w = &memory.ui_config.window_config;
                        let apply = (|| -> std::result::Result<(), tauri::Error> {
                            window.set_always_on_top(memory.ui_config.always_on_top)?;
                            window.set_min_size(Some(tauri::LogicalSize::new(w.min_width, w.min_height)))?;
                            window.set_max_size(Some(tauri::LogicalSize::new(w.max_width, w.max_height)))?;
                            let (width, height) = if w.fixed { (w.fixed_width, w.fixed_height) } else { (w.free_width, w.free_height) };
                            window.set_size(tauri::LogicalSize::new(width, height))?;
                            Ok(())
                        })();
                        if let Err(error) = apply { return Ok(format!("设置已保存；当前窗口应用失败：{error}，下次打开时生效")); }
                    }
                }
                Ok("已复制所选设置".into())
            } else { apply_content(category, incoming) }
        })();
        results.push(ApplyResult { category, success: outcome.is_ok(), message: outcome.unwrap_or_else(|e| e) });
    }
    let _ = app.emit("settings-sync-applied", &results);
    if results.iter().any(|r| r.success && r.category == Category::PromptTemplates) {
        crate::bridge::ws::broadcast_custom_prompt_config_changed(&app);
    }
    if results.iter().any(|r| r.success && r.category == Category::GhostSuggestions) {
        crate::bridge::ws::broadcast_ghost_suggestions_changed(&app, crate::ghost_suggestions::load_store_value());
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_library_read_and_preview_do_not_write_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.json");
        assert_eq!(read_optional(&path, json!([])).unwrap(), json!([]));
        assert!(!path.exists());
        std::fs::write(&path, b"{broken").unwrap();
        assert!(read_optional(&path, json!([])).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
        let snapshot = Snapshot {
            version: 2,
            schema_version: 1,
            device_id: "fixture".into(),
            device_name: "fixture".into(),
            platform: "windows".into(),
            categories: BTreeMap::from([(Category::Audio, json!({"notification_enabled":true}))]),
        };
        let mut document = preview(
            snapshot,
            BTreeMap::from([(Category::Audio, json!({"notification_enabled":false}))]),
            "fixture-binding".into(),
        )
        .unwrap();
        retain_preview(&document).unwrap();
        document.snapshot.device_id = "client-mutated".into();
        let cache = PREVIEWS.get().unwrap().lock().unwrap();
        assert_eq!(cache[&document.preview_id].1.snapshot.device_id, "fixture");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn explicit_projection_excludes_secrets_and_unknown_fields() {
        let mut config = AppConfig::default();
        config.browser_ws_config.token = "SECRET_SENTINEL".into();
        config.mcp_config.acemcp_token = Some("SECRET_SENTINEL".into());
        config.audio_config.custom_url = "SECRET_SENTINEL".into();
        config.ui_config.font_config.custom_font_family = "SECRET_SENTINEL".into();
        config
            .mcp_config
            .tools
            .insert("SECRET_SENTINEL".into(), true);
        for c in [
            Category::Appearance,
            Category::Audio,
            Category::McpTools,
            Category::Reply,
        ] {
            assert!(!config_category(&config, c)
                .unwrap()
                .to_string()
                .contains("SECRET_SENTINEL"));
        }
        let projected=project_entries(Category::SpeechReplacements,&json!([{"id":"a","spokenPhrase":"hi","outputText":"hello","isEnabled":true,"trainingCount":99,"token":"SECRET_SENTINEL"}])).unwrap();
        assert!(!projected.to_string().contains("SECRET_SENTINEL"));
        assert!(projected[0].get("trainingCount").is_none());
    }
    #[test]
    fn rejects_unknown_missing_duplicate_and_oversized_inputs() {
        assert!(
            serde_json::from_value::<ExportRequest>(json!({"categories":["credentials"]})).is_err()
        );
        assert!(selections(vec![Category::Audio, Category::Audio]).is_err());
        assert!(project_entries(Category::PromptLibrary, &json!([{"id":"a"}])).is_err());
        assert!(bounded_bytes(&"a".repeat(MAX_SNAPSHOT_BYTES)).is_err());
        let mut snapshot = Snapshot {
            version: 2,
            schema_version: 1,
            device_id: "peer".into(),
            device_name: "peer".into(),
            platform: "windows".into(),
            categories: BTreeMap::from([(
                Category::Audio,
                json!({"notification_enabled":true,"token":"secret"}),
            )]),
        };
        assert!(validate_snapshot(&snapshot, &[Category::Audio]).is_err());
        snapshot
            .categories
            .insert(Category::Audio, json!({"notification_enabled":true}));
        assert!(validate_snapshot(&snapshot, &[Category::Audio]).is_ok());
        assert!(validate_snapshot(&snapshot, &[Category::Window]).is_err());
    }
    #[test]
    fn preview_keeps_local_conflicts_and_binds_identity_and_content() {
        let local = json!([{"id":"local","name":" Same ","content":"local","category":"one"}]);
        let remote = json!([{"id":"remote","name":"Same","content":"remote","category":"two"}]);
        let snapshot = Snapshot {
            version: 2,
            schema_version: 1,
            device_id: "peer-a".into(),
            device_name: "A".into(),
            platform: "macos".into(),
            categories: BTreeMap::from([(Category::PromptLibrary, remote)]),
        };
        let p = preview(
            snapshot.clone(),
            BTreeMap::from([(Category::PromptLibrary, local.clone())]),
            "binding".into(),
        )
        .unwrap();
        assert_eq!(p.local_snapshot[&Category::PromptLibrary], local);
        assert_eq!(p.differences[0].conflicts.len(), 1);
        assert_eq!(p.pairing_binding, "binding");
        let mut changed = snapshot;
        changed.device_id = "peer-b".into();
        assert_ne!(p.content_hash, hash(&changed).unwrap());
        assert!(conflicts(
            Category::PromptLibrary,
            &local,
            &json!([{"id":"other","name":"same"}])
        )
        .is_empty());
    }
}
