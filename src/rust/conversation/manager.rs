use super::{ConversationNode, ConversationTree, NodeMetadata, NodeType, TimelineImageAttachment};
use crate::mcp::types::ImageAttachment;
use crate::utils::append_timeline_debug_log;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

const CONVERSATION_STATE_FILE_NAME: &str = "conversation-state.json";
const TIMELINE_ATTACHMENTS_DIR_NAME: &str = "timeline-attachments";
const ROUTE_MAP_PREVIEW_LIMIT: usize = 6;
const MAX_NODES_PER_TREE: usize = 30;
const MIN_TIMELINE_MIGRATION_FREE_BYTES: u64 = 256 * 1024 * 1024;

struct ConversationStateFileLock {
    file: File,
}

impl Drop for ConversationStateFileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedConversationState {
    #[serde(default)]
    trees: HashMap<String, ConversationTree>,
    #[serde(default)]
    current_tree_id: Option<String>,
    #[serde(default)]
    request_tree_map: HashMap<String, String>,
    #[serde(default)]
    project_tree_map: HashMap<String, String>,
}

struct ConversationState {
    trees: HashMap<String, ConversationTree>,
    current_tree_id: Option<String>,
    request_tree_map: HashMap<String, String>,
    project_tree_map: HashMap<String, String>,
    dedupe_keys: HashMap<String, HashMap<String, String>>,
}

impl Default for ConversationState {
    fn default() -> Self {
        Self {
            trees: HashMap::new(),
            current_tree_id: None,
            request_tree_map: HashMap::new(),
            project_tree_map: HashMap::new(),
            dedupe_keys: HashMap::new(),
        }
    }
}

impl ConversationState {
    fn from_persisted(mut persisted: PersistedConversationState) -> Self {
        persisted
            .request_tree_map
            .retain(|_, tree_id| persisted.trees.contains_key(tree_id));
        persisted
            .project_tree_map
            .retain(|_, tree_id| persisted.trees.contains_key(tree_id));

        let current_tree_id = persisted
            .current_tree_id
            .filter(|tree_id| persisted.trees.contains_key(tree_id))
            .or_else(|| persisted.trees.keys().next().cloned());

        let dedupe_keys = persisted
            .trees
            .keys()
            .cloned()
            .map(|tree_id| (tree_id, HashMap::new()))
            .collect();

        Self {
            trees: persisted.trees,
            current_tree_id,
            request_tree_map: persisted.request_tree_map,
            project_tree_map: persisted.project_tree_map,
            dedupe_keys,
        }
    }

    fn to_persisted(&self) -> PersistedConversationState {
        PersistedConversationState {
            trees: self.trees.clone(),
            current_tree_id: self.current_tree_id.clone(),
            request_tree_map: self.request_tree_map.clone(),
            project_tree_map: self.project_tree_map.clone(),
        }
    }
}

pub struct ConversationManager {
    state: Arc<RwLock<ConversationState>>,
    persistence_path: Option<PathBuf>,
}

pub struct AddNodeOutcome {
    pub node_id: String,
    pub reused: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineImageMigrationReport {
    pub state_path: String,
    pub backup_path: Option<String>,
    pub images_externalized: usize,
    pub images_already_externalized: usize,
    pub bytes_externalized: u64,
    pub state_bytes_before: u64,
    pub state_bytes_after: u64,
}

#[derive(Default)]
struct AddNodeOptions {
    upsert_assistant_by_request_content: bool,
    move_current_on_reuse: bool,
}

impl ConversationManager {
    pub fn new() -> Self {
        let persistence_path = Self::conversation_state_path();
        Self::new_with_persistence_path(persistence_path)
    }

    pub fn new_with_forced_persistence() -> Self {
        let persistence_path = Self::default_conversation_state_path();
        Self::new_with_persistence_path(persistence_path)
    }

    fn new_with_persistence_path(persistence_path: Option<PathBuf>) -> Self {
        let initial_state = Self::load_state_from_disk(persistence_path.as_deref());
        Self {
            state: Arc::new(RwLock::new(initial_state)),
            persistence_path,
        }
    }

    fn should_persist_state() -> bool {
        let force_enable = std::env::var("ITERATE_FORCE_CONVERSATION_PERSISTENCE")
            .map(|value| value.trim() == "1")
            .unwrap_or(false);
        if force_enable {
            return true;
        }

        if cfg!(test) {
            return false;
        }

        let is_standalone_runtime = std::env::var("ITERATE_STANDALONE_MODE").is_ok()
            || std::env::var("ITERATE_MCP_REQUEST_FILE").is_ok();
        if !is_standalone_runtime {
            return false;
        }

        std::env::var("ITERATE_DISABLE_CONVERSATION_PERSISTENCE")
            .map(|value| value.trim() != "1")
            .unwrap_or(true)
    }

    fn default_conversation_state_path() -> Option<PathBuf> {
        if let Ok(override_path) = std::env::var("ITERATE_CONVERSATION_STATE_FILE") {
            let trimmed = override_path.trim();
            if !trimmed.is_empty() {
                return Some(PathBuf::from(trimmed));
            }
        }

        let base_dir = dirs::config_dir().or_else(dirs::home_dir)?;
        Some(base_dir.join("cunzhi").join(CONVERSATION_STATE_FILE_NAME))
    }

    fn conversation_state_path() -> Option<PathBuf> {
        if !Self::should_persist_state() {
            return None;
        }

        Self::default_conversation_state_path()
    }

    fn timeline_attachments_dir(state_path: &Path) -> Result<PathBuf, String> {
        let parent = state_path
            .parent()
            .ok_or_else(|| "时间线状态路径缺少父目录".to_string())?;
        Ok(parent.join(TIMELINE_ATTACHMENTS_DIR_NAME))
    }

    fn timeline_image_extension(media_type: &str) -> &'static str {
        let normalized = media_type.trim().to_ascii_lowercase();
        if normalized.contains("jpeg") || normalized.contains("jpg") {
            "jpg"
        } else if normalized.contains("gif") {
            "gif"
        } else if normalized.contains("webp") {
            "webp"
        } else if normalized.contains("svg") {
            "svg"
        } else {
            "png"
        }
    }

    fn decode_timeline_image_data(data: &str) -> Result<Vec<u8>, String> {
        let encoded = if data.trim_start().starts_with("data:") {
            data.split_once(',')
                .map(|(_, payload)| payload)
                .ok_or_else(|| "时间线图片 data URL 缺少 payload".to_string())?
        } else {
            data
        };
        let decoded = BASE64
            .decode(encoded.trim())
            .map_err(|error| format!("解码时间线图片失败: {}", error))?;
        if decoded.is_empty() {
            return Err("时间线图片内容为空".to_string());
        }
        Ok(decoded)
    }

    fn inline_timeline_image(image: ImageAttachment) -> TimelineImageAttachment {
        TimelineImageAttachment {
            data: Some(image.data),
            media_type: image.media_type,
            filename: image.filename,
            content_hash: None,
            relative_path: None,
            byte_len: None,
        }
    }

    fn externalize_timeline_image(
        state_path: &Path,
        image: TimelineImageAttachment,
    ) -> Result<TimelineImageAttachment, String> {
        let Some(data) = image.data.as_deref() else {
            return Ok(image);
        };
        let bytes = Self::decode_timeline_image_data(data)?;
        let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
        let hash_hex = hex::encode(digest.as_ref());
        let extension = Self::timeline_image_extension(&image.media_type);
        let attachments_dir = Self::timeline_attachments_dir(state_path)?;
        std::fs::create_dir_all(&attachments_dir)
            .map_err(|error| format!("创建时间线附件目录失败: {}", error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&attachments_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("设置时间线附件目录权限失败: {}", error))?;
        }

        let file_name = format!("{}.{}", hash_hex, extension);
        let attachment_path = attachments_dir.join(&file_name);
        if attachment_path.exists() {
            let existing = std::fs::read(&attachment_path)
                .map_err(|error| format!("读取已有时间线附件失败: {}", error))?;
            let existing_digest = ring::digest::digest(&ring::digest::SHA256, &existing);
            if existing_digest.as_ref() != digest.as_ref() {
                return Err(format!(
                    "时间线附件 hash 冲突: {}",
                    attachment_path.display()
                ));
            }
        } else {
            let tmp_path = attachments_dir.join(format!(
                ".{}.{}.{}.tmp",
                file_name,
                std::process::id(),
                Uuid::new_v4()
            ));
            let write_result: Result<(), String> = (|| {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp_path)
                    .map_err(|error| format!("创建时间线附件临时文件失败: {}", error))?;
                file.write_all(&bytes)
                    .map_err(|error| format!("写入时间线附件失败: {}", error))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(std::fs::Permissions::from_mode(0o600))
                        .map_err(|error| format!("设置时间线附件权限失败: {}", error))?;
                }
                file.sync_all()
                    .map_err(|error| format!("同步时间线附件失败: {}", error))?;
                std::fs::rename(&tmp_path, &attachment_path)
                    .map_err(|error| format!("原子替换时间线附件失败: {}", error))?;
                #[cfg(unix)]
                File::open(&attachments_dir)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| format!("同步时间线附件目录失败: {}", error))?;
                Ok(())
            })();
            if write_result.is_err() {
                let _ = std::fs::remove_file(&tmp_path);
            }
            write_result?;
        }

        Ok(TimelineImageAttachment {
            data: None,
            media_type: image.media_type,
            filename: image.filename,
            content_hash: Some(format!("sha256:{}", hash_hex)),
            relative_path: Some(format!("{}/{}", TIMELINE_ATTACHMENTS_DIR_NAME, file_name)),
            byte_len: Some(bytes.len() as u64),
        })
    }

    fn validate_externalized_timeline_image(
        state_path: &Path,
        image: &TimelineImageAttachment,
    ) -> Result<(), String> {
        if image.data.is_some() {
            return Err("时间线图片仍是内嵌数据，不能按外置引用校验".to_string());
        }
        let relative_path = image
            .relative_path
            .as_deref()
            .ok_or_else(|| "外置时间线图片缺少 relative_path".to_string())?;
        let relative = Path::new(relative_path);
        if relative.is_absolute()
            || !relative.starts_with(TIMELINE_ATTACHMENTS_DIR_NAME)
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!("外置时间线图片路径不安全: {}", relative_path));
        }
        let expected_hash = image
            .content_hash
            .as_deref()
            .and_then(|hash| hash.strip_prefix("sha256:"))
            .ok_or_else(|| "外置时间线图片缺少 sha256 hash".to_string())?;
        let expected_len = image
            .byte_len
            .ok_or_else(|| "外置时间线图片缺少 byte_len".to_string())?;
        let parent = state_path
            .parent()
            .ok_or_else(|| "时间线状态路径缺少父目录".to_string())?;
        let attachment_path = parent.join(relative);
        let bytes = std::fs::read(&attachment_path).map_err(|error| {
            format!(
                "读取外置时间线图片失败 ({}): {}",
                attachment_path.display(),
                error
            )
        })?;
        if bytes.len() as u64 != expected_len {
            return Err(format!(
                "外置时间线图片大小不匹配 ({}): 期望 {}，实际 {}",
                attachment_path.display(),
                expected_len,
                bytes.len()
            ));
        }
        let actual_hash = hex::encode(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
        if actual_hash != expected_hash {
            return Err(format!(
                "外置时间线图片 hash 不匹配: {}",
                attachment_path.display()
            ));
        }
        Ok(())
    }

    pub fn prepare_timeline_images(
        &self,
        images: Option<Vec<ImageAttachment>>,
    ) -> Option<Vec<TimelineImageAttachment>> {
        let images = images?;
        if images.is_empty() {
            return None;
        }
        let Some(state_path) = self.persistence_path.as_deref() else {
            return Some(
                images
                    .into_iter()
                    .map(Self::inline_timeline_image)
                    .collect(),
            );
        };

        Some(
            images
                .into_iter()
                .map(|image| {
                    let inline = Self::inline_timeline_image(image);
                    match Self::externalize_timeline_image(state_path, inline.clone()) {
                        Ok(stored) => stored,
                        Err(error) => {
                            log::warn!("时间线图片外置失败，保留内嵌副本: {}", error);
                            inline
                        }
                    }
                })
                .collect(),
        )
    }

    #[cfg(unix)]
    fn available_bytes(path: &Path) -> Option<u64> {
        let path = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return None;
        }
        let stat = unsafe { stat.assume_init() };
        Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }

    #[cfg(not(unix))]
    fn available_bytes(_path: &Path) -> Option<u64> {
        None
    }

    pub fn migrate_inline_timeline_images(&self) -> Result<TimelineImageMigrationReport, String> {
        let state_path = self
            .persistence_path
            .as_deref()
            .ok_or_else(|| "当前 ConversationManager 未启用持久化".to_string())?;
        let state_bytes_before = std::fs::metadata(state_path)
            .map(|metadata| metadata.len())
            .map_err(|error| format!("读取时间线状态大小失败: {}", error))?;
        let required_free = state_bytes_before
            .saturating_mul(3)
            .max(MIN_TIMELINE_MIGRATION_FREE_BYTES);
        if let Some(available) = Self::available_bytes(
            state_path
                .parent()
                .ok_or_else(|| "时间线状态路径缺少父目录".to_string())?,
        ) {
            if available < required_free {
                return Err(format!(
                    "时间线图片迁移空间不足: 可用 {} 字节，至少需要 {} 字节",
                    available, required_free
                ));
            }
        }

        let _lock = Self::lock_conversation_state(state_path)?;
        let mut persisted = Self::read_persisted_state_for_merge(state_path)?;
        let mut images_externalized = 0usize;
        let mut images_already_externalized = 0usize;
        let mut bytes_externalized = 0u64;

        let inline_image_count = persisted
            .trees
            .values()
            .flat_map(|tree| tree.nodes.values())
            .filter_map(|node| node.metadata.images.as_ref())
            .flatten()
            .filter(|image| image.data.is_some())
            .count();
        if inline_image_count == 0 {
            let mut images_already_externalized = 0usize;
            for image in persisted
                .trees
                .values()
                .flat_map(|tree| tree.nodes.values())
                .filter_map(|node| node.metadata.images.as_ref())
                .flatten()
            {
                Self::validate_externalized_timeline_image(state_path, image)?;
                images_already_externalized += 1;
            }
            return Ok(TimelineImageMigrationReport {
                state_path: state_path.to_string_lossy().to_string(),
                backup_path: None,
                images_externalized: 0,
                images_already_externalized,
                bytes_externalized: 0,
                state_bytes_before,
                state_bytes_after: state_bytes_before,
            });
        }

        let backup_path = state_path.with_file_name(format!(
            "{}.pre-image-ref-migration-{}",
            state_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(CONVERSATION_STATE_FILE_NAME),
            Utc::now().timestamp_millis()
        ));
        std::fs::copy(state_path, &backup_path)
            .map_err(|error| format!("备份时间线状态失败: {}", error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&backup_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("设置时间线备份权限失败: {}", error))?;
        }
        OpenOptions::new()
            .write(true)
            .open(&backup_path)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("同步时间线备份失败: {}", error))?;
        #[cfg(unix)]
        if let Some(parent) = backup_path.parent() {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("同步时间线备份目录失败: {}", error))?;
        }

        for tree in persisted.trees.values_mut() {
            for node in tree.nodes.values_mut() {
                let Some(images) = node.metadata.images.as_mut() else {
                    continue;
                };
                for image in images.iter_mut() {
                    if image.data.is_none() {
                        Self::validate_externalized_timeline_image(state_path, image)?;
                        images_already_externalized += 1;
                        continue;
                    }
                    let stored = Self::externalize_timeline_image(state_path, image.clone())?;
                    bytes_externalized =
                        bytes_externalized.saturating_add(stored.byte_len.unwrap_or_default());
                    *image = stored;
                    images_externalized += 1;
                }
            }
        }

        if images_externalized > 0 {
            Self::atomic_write_persisted_state(state_path, &persisted)?;
        }
        let state_bytes_after = std::fs::metadata(state_path)
            .map(|metadata| metadata.len())
            .map_err(|error| format!("读取迁移后时间线状态大小失败: {}", error))?;

        Ok(TimelineImageMigrationReport {
            state_path: state_path.to_string_lossy().to_string(),
            backup_path: Some(backup_path.to_string_lossy().to_string()),
            images_externalized,
            images_already_externalized,
            bytes_externalized,
            state_bytes_before,
            state_bytes_after,
        })
    }

    fn map_preview(map: &HashMap<String, String>) -> Vec<String> {
        map.keys()
            .take(ROUTE_MAP_PREVIEW_LIMIT)
            .cloned()
            .collect::<Vec<_>>()
    }

    fn load_state_from_disk(state_path: Option<&Path>) -> ConversationState {
        let Some(state_path) = state_path else {
            return ConversationState::default();
        };

        let raw = match std::fs::read_to_string(&state_path) {
            Ok(content) => content,
            Err(_) => return ConversationState::default(),
        };

        match serde_json::from_str::<PersistedConversationState>(&raw) {
            Ok(persisted) => {
                let mut state = ConversationState::from_persisted(persisted);
                // Auto-prune on load: trim each tree to MAX_NODES_PER_TREE
                for (tree_id, tree) in state.trees.iter_mut() {
                    if tree.nodes.len() > MAX_NODES_PER_TREE {
                        let mut all_nodes: Vec<(String, String)> = tree
                            .nodes
                            .iter()
                            .map(|(id, n)| (id.clone(), n.timestamp.clone()))
                            .collect();
                        all_nodes.sort_by(|a, b| a.1.cmp(&b.1));
                        let to_remove = all_nodes.len() - MAX_NODES_PER_TREE;
                        let remove_ids: HashSet<String> = all_nodes
                            .iter()
                            .take(to_remove)
                            .map(|(id, _)| id.clone())
                            .collect();
                        for id in &remove_ids {
                            tree.nodes.remove(id);
                        }
                        tree.branches
                            .retain(|parent, _| !remove_ids.contains(parent));
                        for children in tree.branches.values_mut() {
                            children.retain(|child| !remove_ids.contains(child));
                        }
                        let orphaned_ids: Vec<String> = tree
                            .nodes
                            .iter()
                            .filter_map(|(id, node)| {
                                node.parent_id
                                    .as_ref()
                                    .filter(|pid| !tree.nodes.contains_key(pid.as_str()))
                                    .map(|_| id.clone())
                            })
                            .collect();
                        for id in orphaned_ids {
                            if let Some(node) = tree.nodes.get_mut(&id) {
                                node.parent_id = None;
                            }
                        }
                        if remove_ids.contains(&tree.current_node_id) {
                            if let Some(newest) = all_nodes.last() {
                                tree.current_node_id = newest.0.clone();
                            }
                        }
                        if let Some(tree_dedupes) = state.dedupe_keys.get_mut(tree_id.as_str()) {
                            tree_dedupes.retain(|_, nid| !remove_ids.contains(nid));
                        }
                        eprintln!(
                            "[Conversation][Manager] load_state pruned tree {}: removed {} nodes, kept {}",
                            tree_id, to_remove, tree.nodes.len()
                        );
                    }
                }
                append_timeline_debug_log(
                    "rust/conversation::manager/load_state_from_disk:success",
                    serde_json::json!({
                        "state_path": state_path.to_string_lossy().to_string(),
                        "tree_count": state.trees.len(),
                        "request_map_size": state.request_tree_map.len(),
                        "project_map_size": state.project_tree_map.len(),
                    }),
                );
                state
            }
            Err(err) => {
                append_timeline_debug_log(
                    "rust/conversation::manager/load_state_from_disk:failed",
                    serde_json::json!({
                        "state_path": state_path.to_string_lossy().to_string(),
                        "error": err.to_string(),
                    }),
                );
                ConversationState::default()
            }
        }
    }

    fn conversation_state_lock_path(state_path: &Path) -> PathBuf {
        state_path.with_file_name(format!(
            "{}.lock",
            state_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(CONVERSATION_STATE_FILE_NAME)
        ))
    }

    fn lock_conversation_state(state_path: &Path) -> Result<ConversationStateFileLock, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(Self::conversation_state_lock_path(state_path))
            .map_err(|error| format!("打开时间线状态锁失败: {}", error))?;

        #[cfg(unix)]
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(format!(
                "锁定时间线状态失败: {}",
                std::io::Error::last_os_error()
            ));
        }

        Ok(ConversationStateFileLock { file })
    }

    fn read_persisted_state_for_merge(
        state_path: &Path,
    ) -> Result<PersistedConversationState, String> {
        let raw = match std::fs::read(state_path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PersistedConversationState::default())
            }
            Err(error) => return Err(format!("读取时间线状态失败: {}", error)),
        };
        serde_json::from_slice(&raw).map_err(|error| format!("解析时间线状态失败: {}", error))
    }

    fn current_node_timestamp(tree: &ConversationTree) -> Option<&str> {
        tree.nodes
            .get(&tree.current_node_id)
            .map(|node| node.timestamp.as_str())
    }

    fn prune_merged_tree(tree: &mut ConversationTree) {
        if tree.nodes.len() <= MAX_NODES_PER_TREE {
            return;
        }

        let mut all_nodes = tree
            .nodes
            .iter()
            .map(|(id, node)| (id.clone(), node.timestamp.clone()))
            .collect::<Vec<_>>();
        all_nodes.sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));
        let remove_ids = all_nodes
            .iter()
            .take(all_nodes.len() - MAX_NODES_PER_TREE)
            .map(|(id, _)| id.clone())
            .collect::<HashSet<_>>();

        tree.nodes.retain(|id, _| !remove_ids.contains(id));
        tree.branches
            .retain(|parent, _| !remove_ids.contains(parent));
        for children in tree.branches.values_mut() {
            children.retain(|child| !remove_ids.contains(child));
        }
        let retained_ids = tree.nodes.keys().cloned().collect::<HashSet<_>>();
        for node in tree.nodes.values_mut() {
            if node
                .parent_id
                .as_ref()
                .is_some_and(|parent| !retained_ids.contains(parent))
            {
                node.parent_id = None;
            }
        }
        if !tree.nodes.contains_key(&tree.current_node_id) {
            tree.current_node_id = all_nodes
                .iter()
                .rev()
                .find(|(id, _)| tree.nodes.contains_key(id))
                .map(|(id, _)| id.clone())
                .unwrap_or_default();
        }
    }

    fn merge_conversation_tree(target: &mut ConversationTree, incoming: ConversationTree) {
        let target_current_timestamp = Self::current_node_timestamp(target).map(ToOwned::to_owned);
        let incoming_current_timestamp =
            Self::current_node_timestamp(&incoming).map(ToOwned::to_owned);
        let incoming_current_node_id = incoming.current_node_id.clone();

        if incoming.created_at < target.created_at {
            target.created_at = incoming.created_at.clone();
        }
        if incoming.updated_at > target.updated_at {
            target.updated_at = incoming.updated_at.clone();
        }
        for (node_id, node) in incoming.nodes {
            target.nodes.entry(node_id).or_insert(node);
        }
        for (parent_id, children) in incoming.branches {
            let target_children = target.branches.entry(parent_id).or_default();
            for child in children {
                if !target_children.contains(&child) {
                    target_children.push(child);
                }
            }
        }

        if incoming_current_timestamp > target_current_timestamp
            || (target.current_node_id.is_empty() && !incoming_current_node_id.is_empty())
        {
            target.current_node_id = incoming_current_node_id;
        }
        Self::prune_merged_tree(target);
    }

    fn tree_recency_key(
        trees: &HashMap<String, ConversationTree>,
        tree_id: &str,
    ) -> Option<(String, String)> {
        trees
            .get(tree_id)
            .map(|tree| (tree.updated_at.clone(), tree_id.to_string()))
    }

    fn merge_route_map(
        target: &mut HashMap<String, String>,
        incoming: HashMap<String, String>,
        trees: &HashMap<String, ConversationTree>,
    ) {
        for (route, incoming_tree_id) in incoming {
            if !trees.contains_key(&incoming_tree_id) {
                continue;
            }
            let replace = target.get(&route).is_none_or(|target_tree_id| {
                Self::tree_recency_key(trees, &incoming_tree_id)
                    >= Self::tree_recency_key(trees, target_tree_id)
            });
            if replace {
                target.insert(route, incoming_tree_id);
            }
        }
        target.retain(|_, tree_id| trees.contains_key(tree_id));
    }

    fn merge_persisted_states(
        mut persisted: PersistedConversationState,
        snapshot: PersistedConversationState,
        replace_tree_ids: &HashSet<String>,
    ) -> PersistedConversationState {
        let incoming_current_tree_id = snapshot.current_tree_id.clone();
        for (tree_id, incoming_tree) in snapshot.trees {
            if replace_tree_ids.contains(&tree_id) {
                persisted.trees.insert(tree_id, incoming_tree);
            } else if let Some(target_tree) = persisted.trees.get_mut(&tree_id) {
                Self::merge_conversation_tree(target_tree, incoming_tree);
            } else {
                persisted.trees.insert(tree_id, incoming_tree);
            }
        }

        Self::merge_route_map(
            &mut persisted.request_tree_map,
            snapshot.request_tree_map,
            &persisted.trees,
        );
        Self::merge_route_map(
            &mut persisted.project_tree_map,
            snapshot.project_tree_map,
            &persisted.trees,
        );

        if let Some(incoming_tree_id) = incoming_current_tree_id {
            let replace_current =
                persisted
                    .current_tree_id
                    .as_ref()
                    .is_none_or(|current_tree_id| {
                        Self::tree_recency_key(&persisted.trees, &incoming_tree_id)
                            >= Self::tree_recency_key(&persisted.trees, current_tree_id)
                    });
            if replace_current && persisted.trees.contains_key(&incoming_tree_id) {
                persisted.current_tree_id = Some(incoming_tree_id);
            }
        }
        if persisted
            .current_tree_id
            .as_ref()
            .is_some_and(|tree_id| !persisted.trees.contains_key(tree_id))
        {
            persisted.current_tree_id = persisted.trees.keys().next().cloned();
        }
        persisted
    }

    fn atomic_write_persisted_state(
        state_path: &Path,
        snapshot: &PersistedConversationState,
    ) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(snapshot)
            .map_err(|error| format!("序列化时间线状态失败: {}", error))?;
        let file_name = state_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(CONVERSATION_STATE_FILE_NAME);
        let tmp_path = state_path.with_file_name(format!(
            ".{}.{}.{}.tmp",
            file_name,
            std::process::id(),
            Uuid::new_v4()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
                .map_err(|error| format!("创建时间线临时文件失败: {}", error))?;
            file.write_all(&json)
                .map_err(|error| format!("写入时间线临时文件失败: {}", error))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(|error| format!("设置时间线文件权限失败: {}", error))?;
            }
            file.sync_all()
                .map_err(|error| format!("同步时间线临时文件失败: {}", error))?;
            std::fs::rename(&tmp_path, state_path)
                .map_err(|error| format!("原子替换时间线状态失败: {}", error))?;
            #[cfg(unix)]
            if let Some(parent) = state_path.parent() {
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| format!("同步时间线目录失败: {}", error))?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        result
    }

    fn persist_state_snapshot_to_path_with_replacements(
        state_path: Option<&Path>,
        snapshot: PersistedConversationState,
        replace_tree_ids: &HashSet<String>,
    ) {
        let Some(state_path) = state_path else {
            return;
        };
        let result = (|| {
            let parent_dir = state_path
                .parent()
                .ok_or_else(|| "时间线状态路径缺少父目录".to_string())?;
            std::fs::create_dir_all(parent_dir)
                .map_err(|error| format!("创建时间线状态目录失败: {}", error))?;
            let _lock = Self::lock_conversation_state(state_path)?;
            let persisted = Self::read_persisted_state_for_merge(state_path)?;
            let merged = Self::merge_persisted_states(persisted, snapshot, replace_tree_ids);
            Self::atomic_write_persisted_state(state_path, &merged)
        })();

        if let Err(error) = result {
            append_timeline_debug_log(
                "rust/conversation::manager/persist_state_snapshot:failed",
                serde_json::json!({
                    "state_path": state_path.to_string_lossy().to_string(),
                    "error": error,
                }),
            );
        }
    }

    fn persist_state_snapshot_to_path(
        state_path: Option<&Path>,
        snapshot: PersistedConversationState,
    ) {
        Self::persist_state_snapshot_to_path_with_replacements(
            state_path,
            snapshot,
            &HashSet::new(),
        );
    }

    fn persist_state_snapshot(&self, snapshot: PersistedConversationState) {
        Self::persist_state_snapshot_to_path(self.persistence_path.as_deref(), snapshot);
    }

    fn normalize_route_part(value: Option<&str>) -> Option<String> {
        value
            .map(str::trim)
            .filter(|part| !part.is_empty() && *part != "Unknown")
            .map(ToOwned::to_owned)
    }

    fn create_tree_with_parts(
        trees: &mut HashMap<String, ConversationTree>,
        dedupe_keys: &mut HashMap<String, HashMap<String, String>>,
        current_tree_id: &mut Option<String>,
    ) -> String {
        let now = Utc::now().to_rfc3339();
        let tree_id = Uuid::new_v4().to_string();
        let tree = ConversationTree {
            id: tree_id.clone(),
            created_at: now.clone(),
            updated_at: now,
            current_node_id: String::new(),
            nodes: HashMap::new(),
            branches: HashMap::new(),
        };

        trees.insert(tree_id.clone(), tree);
        dedupe_keys.insert(tree_id.clone(), HashMap::new());
        *current_tree_id = Some(tree_id.clone());
        tree_id
    }

    fn create_tree_locked(state: &mut ConversationState) -> String {
        Self::create_tree_with_parts(
            &mut state.trees,
            &mut state.dedupe_keys,
            &mut state.current_tree_id,
        )
    }

    pub async fn create_tree(&self) -> String {
        let mut state = self.state.write().await;
        let tree_id = Self::create_tree_locked(&mut state);
        let snapshot = state.to_persisted();
        drop(state);
        self.persist_state_snapshot(snapshot);
        tree_id
    }

    pub async fn create_tree_for_route(
        &self,
        request_id: Option<String>,
        project_path: Option<String>,
    ) -> String {
        let normalized_request_id = Self::normalize_route_part(request_id.as_deref());
        let normalized_project_path = Self::normalize_route_part(project_path.as_deref());

        let mut state = self.state.write().await;
        append_timeline_debug_log(
            "rust/conversation::manager/create_tree_for_route:start",
            serde_json::json!({
                "request_id_raw": request_id,
                "project_path_raw": project_path,
                "request_id_normalized": normalized_request_id,
                "project_path_normalized": normalized_project_path,
                "tree_count": state.trees.len(),
                "request_map_size": state.request_tree_map.len(),
                "project_map_size": state.project_tree_map.len(),
                "request_map_keys_preview": Self::map_preview(&state.request_tree_map),
                "project_map_keys_preview": Self::map_preview(&state.project_tree_map),
            }),
        );

        if let Some(request_key) = normalized_request_id.as_ref() {
            if let Some(tree_id) = state.request_tree_map.get(request_key).cloned() {
                if state.trees.contains_key(&tree_id) {
                    if let Some(project_key) = normalized_project_path.as_ref() {
                        state
                            .project_tree_map
                            .insert(project_key.clone(), tree_id.clone());
                    }
                    state.current_tree_id = Some(tree_id.clone());
                    append_timeline_debug_log(
                        "rust/conversation::manager/create_tree_for_route:reuse_by_request",
                        serde_json::json!({
                            "request_key": request_key,
                            "project_key": normalized_project_path,
                            "tree_id": tree_id,
                        }),
                    );
                    let snapshot = state.to_persisted();
                    drop(state);
                    self.persist_state_snapshot(snapshot);
                    return tree_id;
                }
                state.request_tree_map.remove(request_key);
                append_timeline_debug_log(
                    "rust/conversation::manager/create_tree_for_route:stale_request_mapping_removed",
                    serde_json::json!({
                        "request_key": request_key,
                        "dangling_tree_id": tree_id,
                    }),
                );
            }
        }

        if normalized_request_id.is_none() {
            if let Some(project_key) = normalized_project_path.as_ref() {
                if let Some(tree_id) = state.project_tree_map.get(project_key).cloned() {
                    if state.trees.contains_key(&tree_id) {
                        state.current_tree_id = Some(tree_id.clone());
                        append_timeline_debug_log(
                            "rust/conversation::manager/create_tree_for_route:reuse_by_project",
                            serde_json::json!({
                                "request_key": normalized_request_id,
                                "project_key": project_key,
                                "tree_id": tree_id,
                            }),
                        );
                        let snapshot = state.to_persisted();
                        drop(state);
                        self.persist_state_snapshot(snapshot);
                        return tree_id;
                    }
                    state.project_tree_map.remove(project_key);
                    append_timeline_debug_log(
                        "rust/conversation::manager/create_tree_for_route:stale_project_mapping_removed",
                        serde_json::json!({
                            "project_key": project_key,
                            "dangling_tree_id": tree_id,
                        }),
                    );
                }
            }
        } else if normalized_project_path.is_some() {
            append_timeline_debug_log(
                "rust/conversation::manager/create_tree_for_route:skip_project_fallback_for_request",
                serde_json::json!({
                    "request_key": normalized_request_id,
                    "project_key": normalized_project_path,
                }),
            );
        }

        let tree_id = Self::create_tree_locked(&mut state);
        if let Some(request_key) = normalized_request_id.clone() {
            state.request_tree_map.insert(request_key, tree_id.clone());
        }
        if let Some(project_key) = normalized_project_path.clone() {
            state.project_tree_map.insert(project_key, tree_id.clone());
        }
        append_timeline_debug_log(
            "rust/conversation::manager/create_tree_for_route:create_new",
            serde_json::json!({
                "request_key": normalized_request_id,
                "project_key": normalized_project_path,
                "tree_id": tree_id,
                "tree_count": state.trees.len(),
                "request_map_size": state.request_tree_map.len(),
                "project_map_size": state.project_tree_map.len(),
            }),
        );
        let snapshot = state.to_persisted();
        drop(state);
        self.persist_state_snapshot(snapshot);
        tree_id
    }

    pub async fn create_tree_for_request(&self, request_id: Option<String>) -> String {
        self.create_tree_for_route(request_id, None).await
    }

    pub async fn get_or_create_tree_for_request(&self, request_id: Option<&str>) -> String {
        self.get_or_create_tree_for_route(request_id, None).await
    }

    pub async fn get_or_create_tree_for_route(
        &self,
        request_id: Option<&str>,
        project_path: Option<&str>,
    ) -> String {
        self.create_tree_for_route(
            request_id.map(ToOwned::to_owned),
            project_path.map(ToOwned::to_owned),
        )
        .await
    }

    pub async fn get_tree_for_route(
        &self,
        request_id: Option<&str>,
        project_path: Option<&str>,
    ) -> Option<String> {
        let normalized_request_id = Self::normalize_route_part(request_id);
        let normalized_project_path = Self::normalize_route_part(project_path);
        let state = self.state.read().await;

        if let Some(request_key) = normalized_request_id.as_ref() {
            if let Some(tree_id) = state.request_tree_map.get(request_key) {
                if state.trees.contains_key(tree_id) {
                    return Some(tree_id.clone());
                }
            }
            return None;
        }

        if let Some(project_key) = normalized_project_path.as_ref() {
            if let Some(tree_id) = state.project_tree_map.get(project_key).cloned() {
                if state.trees.contains_key(&tree_id) {
                    return Some(tree_id.clone());
                }
            }
        }

        None
    }

    pub async fn get_tree_id_by_request_id(&self, request_id: &str) -> Option<String> {
        self.state
            .read()
            .await
            .request_tree_map
            .get(request_id)
            .cloned()
    }

    pub async fn get_current_node_id(&self, tree_id: &str) -> Option<String> {
        self.state.read().await.trees.get(tree_id).and_then(|tree| {
            if tree.current_node_id.is_empty() {
                None
            } else {
                Some(tree.current_node_id.clone())
            }
        })
    }

    pub async fn get_node(&self, tree_id: &str, node_id: &str) -> Option<ConversationNode> {
        self.state
            .read()
            .await
            .trees
            .get(tree_id)
            .and_then(|tree| tree.nodes.get(node_id))
            .cloned()
    }

    pub async fn add_node(
        &self,
        tree_id: &str,
        parent_id: Option<String>,
        node_type: NodeType,
        content: String,
        is_markdown: bool,
        metadata: NodeMetadata,
    ) -> Result<String, String> {
        self.add_node_with_options(
            tree_id,
            parent_id,
            node_type,
            content,
            is_markdown,
            metadata,
            AddNodeOptions::default(),
        )
        .await
        .map(|outcome| outcome.node_id)
    }

    pub async fn ensure_assistant_request_node(
        &self,
        tree_id: &str,
        content: String,
        is_markdown: bool,
        metadata: NodeMetadata,
    ) -> Result<AddNodeOutcome, String> {
        self.add_node_with_options(
            tree_id,
            None,
            NodeType::Assistant,
            content,
            is_markdown,
            metadata,
            AddNodeOptions {
                upsert_assistant_by_request_content: true,
                move_current_on_reuse: false,
            },
        )
        .await
    }

    async fn add_node_with_options(
        &self,
        tree_id: &str,
        parent_id: Option<String>,
        node_type: NodeType,
        content: String,
        is_markdown: bool,
        metadata: NodeMetadata,
        options: AddNodeOptions,
    ) -> Result<AddNodeOutcome, String> {
        let request_id_for_mapping = metadata.request_id.clone();
        let project_path_for_mapping = metadata.project_path.clone();
        eprintln!(
            "[Conversation][Manager] add_node start: tree_id={}, node_type={}, parent_id={:?}, request_id={:?}, content_len={}",
            tree_id,
            node_type.as_key(),
            parent_id,
            request_id_for_mapping,
            content.chars().count()
        );
        let mut state = self.state.write().await;
        let ConversationState {
            trees,
            request_tree_map,
            project_tree_map,
            dedupe_keys,
            ..
        } = &mut *state;
        let Some(tree) = trees.get_mut(tree_id) else {
            log::warn!(
                "[Conversation] add_node 失败: tree 不存在 (tree_id={}, node_type={})",
                tree_id,
                node_type.as_key()
            );
            eprintln!(
                "[Conversation][Manager] add_node failed: tree not found (tree_id={}, node_type={})",
                tree_id,
                node_type.as_key()
            );
            return Err("Tree not found".to_string());
        };

        let resolved_parent_id = if let Some(pid) = parent_id {
            Some(pid)
        } else if tree.current_node_id.is_empty() {
            None
        } else {
            Some(tree.current_node_id.clone())
        };

        if let Some(ref pid) = resolved_parent_id {
            if !tree.nodes.contains_key(pid) {
                log::warn!(
                    "[Conversation] add_node 失败: parent 不存在 (tree_id={}, parent_id={}, node_type={})",
                    tree_id,
                    pid,
                    node_type.as_key()
                );
                eprintln!(
                    "[Conversation][Manager] add_node failed: parent not found (tree_id={}, parent_id={}, node_type={})",
                    tree_id,
                    pid,
                    node_type.as_key()
                );
                return Err(format!("Parent node not found: {}", pid));
            }
        }

        let request_key = metadata
            .request_id
            .as_deref()
            .map(str::trim)
            .filter(|rid| !rid.is_empty())
            .unwrap_or("unknown-request")
            .to_string();
        let node_timestamp = Utc::now().to_rfc3339();
        let dedupe_key = build_dedupe_key(
            request_key.as_str(),
            &node_type,
            resolved_parent_id.as_deref(),
            &node_timestamp,
        );
        let tree_dedupe = dedupe_keys.entry(tree_id.to_string()).or_default();

        if options.upsert_assistant_by_request_content
            && matches!(node_type, NodeType::Assistant)
            && request_key != "unknown-request"
        {
            let existing_id = tree
                .nodes
                .values()
                .filter(|node| {
                    node.node_type == NodeType::Assistant
                        && node
                            .metadata
                            .request_id
                            .as_deref()
                            .map(str::trim)
                            .filter(|rid| !rid.is_empty())
                            == Some(request_key.as_str())
                        && node.content == content
                })
                .max_by(|left, right| left.timestamp.cmp(&right.timestamp))
                .map(|node| node.id.clone());

            if let Some(existing_id) = existing_id {
                if options.move_current_on_reuse {
                    tree.current_node_id = existing_id.clone();
                }
                tree.updated_at = Utc::now().to_rfc3339();
                if let Some(rid) = Self::normalize_route_part(request_id_for_mapping.as_deref()) {
                    request_tree_map.insert(rid, tree_id.to_string());
                }
                if let Some(project_path) =
                    Self::normalize_route_part(project_path_for_mapping.as_deref())
                {
                    project_tree_map.insert(project_path, tree_id.to_string());
                }
                log::info!(
                    "[Conversation] ensure_assistant_request_node 复用节点: tree_id={}, node_id={}, request_key={}",
                    tree_id,
                    existing_id,
                    request_key
                );
                eprintln!(
                    "[Conversation][Manager] ensure_assistant_request_node reused: tree_id={}, node_id={}, request_key={}",
                    tree_id,
                    existing_id,
                    request_key
                );
                append_timeline_debug_log(
                    "rust/conversation::manager/ensure_assistant_request_node:reused",
                    serde_json::json!({
                        "tree_id": tree_id,
                        "node_id": existing_id,
                        "request_key": request_key,
                    }),
                );
                let snapshot = state.to_persisted();
                drop(state);
                self.persist_state_snapshot(snapshot);
                return Ok(AddNodeOutcome {
                    node_id: existing_id,
                    reused: true,
                });
            }
        }

        if let Some(existing_id) = tree_dedupe.get(&dedupe_key).cloned() {
            if tree.nodes.contains_key(&existing_id) {
                tree.current_node_id = existing_id.clone();
                tree.updated_at = Utc::now().to_rfc3339();
                if let Some(rid) = Self::normalize_route_part(request_id_for_mapping.as_deref()) {
                    request_tree_map.insert(rid, tree_id.to_string());
                }
                if let Some(project_path) =
                    Self::normalize_route_part(project_path_for_mapping.as_deref())
                {
                    project_tree_map.insert(project_path, tree_id.to_string());
                }
                log::info!(
                    "[Conversation] add_node 命中去重键，复用节点: tree_id={}, node_id={}, node_type={}, request_key={}",
                    tree_id,
                    existing_id,
                    node_type.as_key(),
                    request_key
                );
                eprintln!(
                    "[Conversation][Manager] add_node dedup hit: tree_id={}, node_id={}, node_type={}, request_key={}",
                    tree_id,
                    existing_id,
                    node_type.as_key(),
                    request_key
                );
                let snapshot = state.to_persisted();
                drop(state);
                self.persist_state_snapshot(snapshot);
                return Ok(AddNodeOutcome {
                    node_id: existing_id,
                    reused: true,
                });
            }
        }

        let node_type_key = node_type.as_key().to_string();
        let node_id = Uuid::new_v4().to_string();
        let mut metadata = metadata;
        if metadata.conversation_id.is_none() {
            metadata.conversation_id = Some(tree_id.to_string());
        }

        let node = ConversationNode {
            id: node_id.clone(),
            parent_id: resolved_parent_id.clone(),
            timestamp: node_timestamp,
            node_type,
            content,
            is_markdown,
            metadata,
        };

        tree.nodes.insert(node_id.clone(), node);
        if let Some(parent) = resolved_parent_id.clone() {
            let children = tree.branches.entry(parent).or_insert_with(Vec::new);
            if !children.contains(&node_id) {
                children.push(node_id.clone());
            }
        }
        tree.current_node_id = node_id.clone();
        tree.updated_at = Utc::now().to_rfc3339();
        tree_dedupe.insert(dedupe_key, node_id.clone());

        if let Some(rid) = Self::normalize_route_part(request_id_for_mapping.as_deref()) {
            request_tree_map.insert(rid, tree_id.to_string());
        }
        if let Some(project_path) = Self::normalize_route_part(project_path_for_mapping.as_deref())
        {
            project_tree_map.insert(project_path, tree_id.to_string());
        }

        // Auto-prune: keep only the most recent MAX_NODES_PER_TREE nodes
        let pruned_count = if tree.nodes.len() > MAX_NODES_PER_TREE {
            let mut all_nodes: Vec<(String, String)> = tree
                .nodes
                .iter()
                .map(|(id, n)| (id.clone(), n.timestamp.clone()))
                .collect();
            all_nodes.sort_by(|a, b| a.1.cmp(&b.1));
            let to_remove = all_nodes.len() - MAX_NODES_PER_TREE;
            let remove_ids: HashSet<String> = all_nodes
                .iter()
                .take(to_remove)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &remove_ids {
                tree.nodes.remove(id);
            }
            tree.branches
                .retain(|parent, _| !remove_ids.contains(parent));
            for children in tree.branches.values_mut() {
                children.retain(|child| !remove_ids.contains(child));
            }
            // Fix orphaned parent_id references
            let orphaned_ids: Vec<String> = tree
                .nodes
                .iter()
                .filter_map(|(id, node)| {
                    node.parent_id
                        .as_ref()
                        .filter(|pid| !tree.nodes.contains_key(pid.as_str()))
                        .map(|_| id.clone())
                })
                .collect();
            for id in orphaned_ids {
                if let Some(node) = tree.nodes.get_mut(&id) {
                    node.parent_id = None;
                }
            }
            if remove_ids.contains(&tree.current_node_id) {
                tree.current_node_id = node_id.clone();
            }
            // Clean dedupe keys for removed nodes
            if let Some(tree_dedupes) = dedupe_keys.get_mut(tree_id) {
                tree_dedupes.retain(|_, nid| !remove_ids.contains(nid));
            }
            to_remove
        } else {
            0
        };

        log::info!(
            "[Conversation] add_node 成功: tree_id={}, node_id={}, parent_id={:?}, node_type={}, request_key={}, total_nodes={}, pruned={}",
            tree_id,
            node_id,
            resolved_parent_id,
            node_type_key,
            request_key,
            tree.nodes.len(),
            pruned_count
        );
        eprintln!(
            "[Conversation][Manager] add_node success: tree_id={}, node_id={}, parent_id={:?}, node_type={}, request_key={}, total_nodes={}, pruned={}",
            tree_id,
            node_id,
            resolved_parent_id,
            node_type_key,
            request_key,
            tree.nodes.len(),
            pruned_count
        );

        let snapshot = state.to_persisted();
        drop(state);
        self.persist_state_snapshot(snapshot);

        Ok(AddNodeOutcome {
            node_id,
            reused: false,
        })
    }

    pub async fn switch_to_node(
        &self,
        tree_id: &str,
        node_id: &str,
    ) -> Result<ConversationNode, String> {
        let mut state = self.state.write().await;
        let node = {
            let tree = state
                .trees
                .get_mut(tree_id)
                .ok_or_else(|| "Tree not found".to_string())?;
            let node = tree
                .nodes
                .get(node_id)
                .ok_or_else(|| "Node not found".to_string())?
                .clone();
            tree.current_node_id = node_id.to_string();
            tree.updated_at = Utc::now().to_rfc3339();
            node
        };
        let snapshot = state.to_persisted();
        drop(state);
        self.persist_state_snapshot(snapshot);
        Ok(node)
    }

    pub async fn get_node_path(
        &self,
        tree_id: &str,
        node_id: &str,
    ) -> Result<Vec<ConversationNode>, String> {
        append_timeline_debug_log(
            "rust/conversation::manager/get_node_path:start",
            serde_json::json!({
                "tree_id": tree_id,
                "node_id": node_id,
            }),
        );
        let state = self.state.read().await;
        let Some(tree) = state.trees.get(tree_id) else {
            append_timeline_debug_log(
                "rust/conversation::manager/get_node_path:failed",
                serde_json::json!({
                    "reason": "tree_not_found",
                    "tree_id": tree_id,
                    "node_id": node_id,
                }),
            );
            return Err("Tree not found".to_string());
        };

        let mut path = Vec::new();
        let mut visited = HashSet::new();
        let mut cursor = Some(node_id.to_string());

        while let Some(current_id) = cursor {
            if !visited.insert(current_id.clone()) {
                append_timeline_debug_log(
                    "rust/conversation::manager/get_node_path:failed",
                    serde_json::json!({
                        "reason": "cycle_detected",
                        "tree_id": tree_id,
                        "node_id": node_id,
                        "current_id": current_id,
                    }),
                );
                return Err("Detected a cycle in conversation tree".to_string());
            }
            let Some(node) = tree.nodes.get(&current_id) else {
                append_timeline_debug_log(
                    "rust/conversation::manager/get_node_path:failed",
                    serde_json::json!({
                        "reason": "node_not_found",
                        "tree_id": tree_id,
                        "node_id": node_id,
                        "current_id": current_id,
                    }),
                );
                return Err(format!("Node {} not found", current_id));
            };
            path.push(node.clone());
            cursor = node.parent_id.clone();
        }

        path.reverse();
        append_timeline_debug_log(
            "rust/conversation::manager/get_node_path:success",
            serde_json::json!({
                "tree_id": tree_id,
                "node_id": node_id,
                "path_len": path.len(),
                "path": path
                    .iter()
                    .map(|node| serde_json::json!({
                        "id": node.id.clone(),
                        "parent_id": node.parent_id.clone(),
                        "node_type": node.node_type.as_key(),
                    }))
                    .collect::<Vec<_>>(),
            }),
        );
        Ok(path)
    }

    pub async fn clear_tree(&self, tree_id: &str) -> Result<usize, String> {
        let mut state = self.state.write().await;
        let tree = state
            .trees
            .get_mut(tree_id)
            .ok_or_else(|| format!("Tree not found: {}", tree_id))?;

        let node_count = tree.nodes.len();
        tree.nodes.clear();
        tree.branches.clear();
        tree.current_node_id = String::new();

        // 清理该 tree 的 dedupe_keys
        state.dedupe_keys.remove(tree_id);

        append_timeline_debug_log(
            "rust/conversation::manager/clear_tree:success",
            serde_json::json!({
                "tree_id": tree_id,
                "cleared_node_count": node_count,
            }),
        );

        // 持久化
        let snapshot = state.to_persisted();
        let persistence_path = self.persistence_path.clone();
        let replace_tree_ids = HashSet::from([tree_id.to_string()]);
        tokio::spawn(async move {
            ConversationManager::persist_state_snapshot_to_path_with_replacements(
                persistence_path.as_deref(),
                snapshot,
                &replace_tree_ids,
            );
        });

        Ok(node_count)
    }
}

fn build_dedupe_key(
    request_key: &str,
    node_type: &NodeType,
    parent_id: Option<&str>,
    timestamp: &str,
) -> String {
    let parent = parent_id.unwrap_or("root");
    format!(
        "{}:{}:{}:{}",
        request_key,
        node_type.as_key(),
        parent,
        timestamp
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_or_create_tree_for_request_is_stable() {
        let manager = ConversationManager::new();

        let first = manager.get_or_create_tree_for_request(Some("req-1")).await;
        let second = manager.get_or_create_tree_for_request(Some("req-1")).await;

        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn get_or_create_tree_for_route_keeps_request_isolation_with_same_project() {
        let manager = ConversationManager::new();

        let first = manager
            .get_or_create_tree_for_route(Some("req-1"), Some("/tmp/project"))
            .await;
        let second = manager
            .get_or_create_tree_for_route(Some("req-2"), Some("/tmp/project"))
            .await;

        assert_ne!(first, second);
        assert_eq!(
            manager.get_tree_id_by_request_id("req-1").await,
            Some(first.clone())
        );
        assert_eq!(
            manager.get_tree_id_by_request_id("req-2").await,
            Some(second.clone())
        );
        assert_eq!(
            manager.get_tree_for_route(None, Some("/tmp/project")).await,
            Some(second)
        );
    }

    #[tokio::test]
    async fn get_or_create_tree_for_route_without_project_keeps_request_isolation() {
        let manager = ConversationManager::new();

        let first = manager
            .get_or_create_tree_for_route(Some("req-1"), None)
            .await;
        let second = manager
            .get_or_create_tree_for_route(Some("req-2"), None)
            .await;

        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn get_tree_for_route_returns_none_when_route_missing() {
        let manager = ConversationManager::new();
        let tree = manager
            .get_or_create_tree_for_route(Some("req-1"), Some("/tmp/project"))
            .await;

        assert_eq!(
            manager
                .get_tree_for_route(Some("missing-request"), Some("/tmp/project"))
                .await,
            None
        );
        assert_eq!(
            manager.get_tree_for_route(None, Some("/tmp/project")).await,
            Some(tree)
        );
        assert_eq!(
            manager
                .get_tree_for_route(Some("missing-request"), Some("/tmp/missing"))
                .await,
            None
        );
        assert_eq!(manager.get_tree_for_route(None, None).await, None);
    }

    #[tokio::test]
    async fn same_project_different_requests_do_not_share_parent_chain() {
        let manager = ConversationManager::new();
        let project_path = "/tmp/project";

        let first_tree = manager
            .get_or_create_tree_for_route(Some("req-1"), Some(project_path))
            .await;
        let first_assistant = manager
            .add_node(
                &first_tree,
                None,
                NodeType::Assistant,
                "first assistant".to_string(),
                true,
                NodeMetadata {
                    project_path: Some(project_path.to_string()),
                    request_id: Some("req-1".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("first assistant node should be created");
        let first_user = manager
            .add_node(
                &first_tree,
                None,
                NodeType::User,
                "first user".to_string(),
                false,
                NodeMetadata {
                    project_path: Some(project_path.to_string()),
                    request_id: Some("req-1".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("first user node should be created");
        assert_eq!(
            manager.get_current_node_id(&first_tree).await.as_deref(),
            Some(first_user.as_str())
        );

        let second_tree = manager
            .get_or_create_tree_for_route(Some("req-2"), Some(project_path))
            .await;
        assert_ne!(first_tree, second_tree);
        assert_eq!(manager.get_current_node_id(&second_tree).await, None);

        let second_assistant = manager
            .add_node(
                &second_tree,
                None,
                NodeType::Assistant,
                "second assistant".to_string(),
                true,
                NodeMetadata {
                    project_path: Some(project_path.to_string()),
                    request_id: Some("req-2".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("second assistant node should be created");
        let second_node = manager
            .get_node(&second_tree, &second_assistant)
            .await
            .expect("second assistant node should exist");
        assert_eq!(second_node.parent_id, None);

        let second_path = manager
            .get_node_path(&second_tree, &second_assistant)
            .await
            .expect("second path should resolve");
        assert_eq!(second_path.len(), 1);
        assert_eq!(second_path[0].id, second_assistant);
        assert!(second_path
            .iter()
            .all(|node| node.metadata.request_id.as_deref() == Some("req-2")));
        assert!(!second_path
            .iter()
            .any(|node| node.id == first_assistant || node.id == first_user));
    }

    #[tokio::test]
    async fn repeated_same_content_creates_new_nodes() {
        let manager = ConversationManager::new();
        let tree_id = manager.get_or_create_tree_for_request(Some("req-2")).await;

        let metadata = NodeMetadata {
            request_id: Some("req-2".to_string()),
            ..NodeMetadata::default()
        };

        let first = manager
            .add_node(
                &tree_id,
                None,
                NodeType::User,
                "same content".to_string(),
                false,
                metadata.clone(),
            )
            .await
            .expect("first node should be created");
        let second = manager
            .add_node(
                &tree_id,
                None,
                NodeType::User,
                "same content".to_string(),
                false,
                metadata,
            )
            .await
            .expect("second node should be created");

        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn add_node_populates_conversation_id_when_missing() {
        let manager = ConversationManager::new();
        let tree_id = manager.get_or_create_tree_for_request(Some("req-3")).await;

        let node_id = manager
            .add_node(
                &tree_id,
                None,
                NodeType::Assistant,
                "hello".to_string(),
                true,
                NodeMetadata {
                    request_id: Some("req-3".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("node should be created");

        let node = manager
            .get_node(&tree_id, &node_id)
            .await
            .expect("node should exist");
        assert_eq!(
            node.metadata.conversation_id.as_deref(),
            Some(tree_id.as_str())
        );
    }

    #[tokio::test]
    async fn ensure_assistant_request_node_reuses_same_request_content_without_rewinding_current() {
        let manager = ConversationManager::new();
        let tree_id = manager
            .get_or_create_tree_for_request(Some("req-upsert"))
            .await;
        let metadata = NodeMetadata {
            request_id: Some("req-upsert".to_string()),
            source: Some("frontend_sync_fallback".to_string()),
            ..NodeMetadata::default()
        };

        let first = manager
            .ensure_assistant_request_node(
                &tree_id,
                "assistant prompt".to_string(),
                true,
                metadata.clone(),
            )
            .await
            .expect("first assistant node should be created");
        assert!(!first.reused);

        let second = manager
            .ensure_assistant_request_node(
                &tree_id,
                "assistant prompt".to_string(),
                true,
                metadata.clone(),
            )
            .await
            .expect("second assistant node should be reused");
        assert!(second.reused);
        assert_eq!(second.node_id, first.node_id);

        let user_id = manager
            .add_node(
                &tree_id,
                Some(first.node_id.clone()),
                NodeType::User,
                "phone reply".to_string(),
                false,
                NodeMetadata {
                    request_id: Some("req-upsert".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("user child should be created");
        assert_eq!(
            manager.get_current_node_id(&tree_id).await.as_deref(),
            Some(user_id.as_str())
        );

        let third = manager
            .ensure_assistant_request_node(&tree_id, "assistant prompt".to_string(), true, metadata)
            .await
            .expect("third assistant node should still be reused");
        assert!(third.reused);
        assert_eq!(third.node_id, first.node_id);
        assert_eq!(
            manager.get_current_node_id(&tree_id).await.as_deref(),
            Some(user_id.as_str())
        );

        let node_count = manager
            .state
            .read()
            .await
            .trees
            .get(&tree_id)
            .expect("tree should exist")
            .nodes
            .len();
        assert_eq!(node_count, 2);
    }

    #[tokio::test]
    async fn forced_persistence_reuses_disk_tree_for_route() {
        let temp = tempfile::tempdir().expect("temp dir");
        let state_file = temp.path().join("conversation-state.json");
        let persistence_path = Some(state_file.clone());

        let standalone_manager =
            ConversationManager::new_with_persistence_path(persistence_path.clone());
        let tree_id = standalone_manager
            .get_or_create_tree_for_route(Some("serve-1"), Some("/tmp/project"))
            .await;
        let assistant_id = standalone_manager
            .add_node(
                &tree_id,
                None,
                NodeType::Assistant,
                "assistant prompt".to_string(),
                true,
                NodeMetadata {
                    project_path: Some("/tmp/project".to_string()),
                    request_id: Some("serve-1".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("assistant node should persist");

        let bridge_manager = ConversationManager::new_with_persistence_path(persistence_path);
        let reused_tree_id = bridge_manager
            .get_or_create_tree_for_route(Some("serve-1"), Some("/tmp/project"))
            .await;
        assert_eq!(reused_tree_id, tree_id);
        let user_id = bridge_manager
            .add_node(
                &reused_tree_id,
                Some(assistant_id),
                NodeType::User,
                "phone reply".to_string(),
                false,
                NodeMetadata {
                    project_path: Some("/tmp/project".to_string()),
                    request_id: Some("serve-1".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("user node should persist");

        let raw = std::fs::read_to_string(&state_file).expect("state file should exist");
        let persisted: PersistedConversationState =
            serde_json::from_str(&raw).expect("state should parse");
        let tree = persisted
            .trees
            .get(&tree_id)
            .expect("tree should stay visible");
        assert_eq!(tree.current_node_id, user_id);
        assert_eq!(tree.nodes.len(), 2);
        assert_eq!(persisted.request_tree_map.get("serve-1"), Some(&tree_id));
        assert_eq!(
            persisted.project_tree_map.get("/tmp/project"),
            Some(&tree_id)
        );

        let new_request_tree_id = bridge_manager
            .get_or_create_tree_for_route(Some("serve-2"), Some("/tmp/project"))
            .await;
        assert_ne!(new_request_tree_id, tree_id);
        assert_eq!(
            bridge_manager.get_tree_id_by_request_id("serve-2").await,
            Some(new_request_tree_id.clone())
        );
        assert_eq!(
            bridge_manager
                .get_tree_for_route(Some("missing-serve"), Some("/tmp/project"))
                .await,
            None
        );
        assert_eq!(
            bridge_manager
                .get_tree_for_route(None, Some("/tmp/project"))
                .await,
            Some(new_request_tree_id)
        );
    }

    #[tokio::test]
    async fn stale_process_snapshots_preserve_both_nodes_on_the_same_timeline() {
        let temp = tempfile::tempdir().expect("temp dir");
        let state_file = temp.path().join("conversation-state.json");
        let persistence_path = Some(state_file.clone());

        let seed_manager = ConversationManager::new_with_persistence_path(persistence_path.clone());
        let tree_id = seed_manager
            .get_or_create_tree_for_route(Some("serve-shared"), Some("/tmp/project"))
            .await;
        let assistant_id = seed_manager
            .add_node(
                &tree_id,
                None,
                NodeType::Assistant,
                "assistant prompt".to_string(),
                true,
                NodeMetadata {
                    project_path: Some("/tmp/project".to_string()),
                    request_id: Some("serve-shared".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("seed assistant node should persist");

        let desktop_manager =
            ConversationManager::new_with_persistence_path(persistence_path.clone());
        let bridge_manager = ConversationManager::new_with_persistence_path(persistence_path);

        let desktop_node_id = desktop_manager
            .add_node(
                &tree_id,
                Some(assistant_id.clone()),
                NodeType::User,
                "desktop reply".to_string(),
                false,
                NodeMetadata {
                    project_path: Some("/tmp/project".to_string()),
                    request_id: Some("serve-shared".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("desktop node should persist");
        let bridge_node_id = bridge_manager
            .add_node(
                &tree_id,
                Some(assistant_id.clone()),
                NodeType::User,
                "bridge reply".to_string(),
                false,
                NodeMetadata {
                    project_path: Some("/tmp/project".to_string()),
                    request_id: Some("serve-shared".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("bridge node should persist");

        let raw = std::fs::read_to_string(&state_file).expect("state file should exist");
        let persisted: PersistedConversationState =
            serde_json::from_str(&raw).expect("state should parse");
        let tree = persisted
            .trees
            .get(&tree_id)
            .expect("shared timeline should remain visible");
        assert_eq!(tree.nodes.len(), 3);
        assert!(tree.nodes.contains_key(&assistant_id));
        assert!(tree.nodes.contains_key(&desktop_node_id));
        assert!(tree.nodes.contains_key(&bridge_node_id));
        let children = tree
            .branches
            .get(&assistant_id)
            .expect("assistant should keep both replies");
        assert!(children.contains(&desktop_node_id));
        assert!(children.contains(&bridge_node_id));
    }

    #[tokio::test]
    async fn persistent_manager_externalizes_new_timeline_images() {
        let temp = tempfile::tempdir().expect("temp dir");
        let state_file = temp.path().join("conversation-state.json");
        let manager = ConversationManager::new_with_persistence_path(Some(state_file.clone()));
        let tree_id = manager
            .get_or_create_tree_for_request(Some("serve-image"))
            .await;
        let images = manager
            .prepare_timeline_images(Some(vec![ImageAttachment {
                data: "aGVsbG8=".to_string(),
                media_type: "image/png".to_string(),
                filename: Some("hello.png".to_string()),
            }]))
            .expect("prepared image");
        assert_eq!(images.len(), 1);
        assert!(images[0].data.is_none());
        assert_eq!(images[0].byte_len, Some(5));
        assert!(images[0]
            .content_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("sha256:")));

        manager
            .add_node(
                &tree_id,
                None,
                NodeType::User,
                "with image".to_string(),
                false,
                NodeMetadata {
                    images: Some(images.clone()),
                    request_id: Some("serve-image".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("image node should persist");

        let relative_path = images[0]
            .relative_path
            .as_deref()
            .expect("relative attachment path");
        assert_eq!(
            std::fs::read(temp.path().join(relative_path)).expect("attachment bytes"),
            b"hello"
        );
        let raw = std::fs::read_to_string(&state_file).expect("persisted state");
        assert!(!raw.contains("aGVsbG8="));
        assert!(raw.contains("timeline-attachments/"));
    }

    #[tokio::test]
    async fn inline_timeline_image_migration_is_atomic_and_idempotent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let state_file = temp.path().join("conversation-state.json");
        let manager = ConversationManager::new_with_persistence_path(Some(state_file.clone()));
        let tree_id = manager
            .get_or_create_tree_for_request(Some("serve-legacy-image"))
            .await;
        manager
            .add_node(
                &tree_id,
                None,
                NodeType::User,
                "legacy image".to_string(),
                false,
                NodeMetadata {
                    images: Some(vec![TimelineImageAttachment {
                        data: Some("bGVnYWN5".to_string()),
                        media_type: "image/png".to_string(),
                        filename: Some("legacy.png".to_string()),
                        content_hash: None,
                        relative_path: None,
                        byte_len: None,
                    }]),
                    request_id: Some("serve-legacy-image".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("legacy node should persist");
        assert!(std::fs::read_to_string(&state_file)
            .expect("legacy state")
            .contains("bGVnYWN5"));

        let report = manager
            .migrate_inline_timeline_images()
            .expect("migration should succeed");
        assert_eq!(report.images_externalized, 1);
        assert_eq!(report.bytes_externalized, 6);
        let backup_path = report.backup_path.expect("migration backup");
        assert!(std::path::Path::new(&backup_path).exists());
        assert!(std::fs::read_to_string(&backup_path)
            .expect("backup state")
            .contains("bGVnYWN5"));

        let migrated_raw = std::fs::read_to_string(&state_file).expect("migrated state");
        assert!(!migrated_raw.contains("bGVnYWN5"));
        let migrated: PersistedConversationState =
            serde_json::from_str(&migrated_raw).expect("migrated state should parse");
        let image = migrated
            .trees
            .get(&tree_id)
            .and_then(|tree| tree.nodes.values().next())
            .and_then(|node| node.metadata.images.as_ref())
            .and_then(|images| images.first())
            .expect("migrated image reference");
        assert!(image.data.is_none());
        assert_eq!(image.byte_len, Some(6));

        let second = manager
            .migrate_inline_timeline_images()
            .expect("second migration should be a no-op");
        assert_eq!(second.images_externalized, 0);
        assert_eq!(second.images_already_externalized, 1);
        assert!(second.backup_path.is_none());
    }

    #[tokio::test]
    async fn clearing_a_persisted_timeline_does_not_restore_merged_nodes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let state_file = temp.path().join("conversation-state.json");
        let manager = ConversationManager::new_with_persistence_path(Some(state_file.clone()));
        let tree_id = manager
            .get_or_create_tree_for_request(Some("serve-clear"))
            .await;
        manager
            .add_node(
                &tree_id,
                None,
                NodeType::Assistant,
                "clear me".to_string(),
                false,
                NodeMetadata {
                    request_id: Some("serve-clear".to_string()),
                    ..NodeMetadata::default()
                },
            )
            .await
            .expect("node should persist before clear");

        assert_eq!(manager.clear_tree(&tree_id).await.unwrap(), 1);
        for _ in 0..50 {
            let cleared = std::fs::read_to_string(&state_file)
                .ok()
                .and_then(|raw| serde_json::from_str::<PersistedConversationState>(&raw).ok())
                .and_then(|state| state.trees.get(&tree_id).map(|tree| tree.nodes.is_empty()))
                .unwrap_or(false);
            if cleared {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("cleared timeline should stay empty on disk");
    }
}
