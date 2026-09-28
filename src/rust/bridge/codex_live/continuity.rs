use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

const STORE_VERSION: u32 = 1;
const MAX_PROJECTS: usize = 64;
const MAX_ITEMS: usize = 10;
const MAX_ITEM_CHARS: usize = 1_200;
const MAX_TOTAL_CHARS: usize = 8_000;
const MAX_THREAD_ID_CHARS: usize = 512;
const STORE_FILE_NAME: &str = "codex-live-continuity-v1.json";
const LOCK_FILE_NAME: &str = "codex-live-continuity.lock";

static CONTINUITY_WRITE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ContinuityItem {
    sequence: u64,
    role: String,
    text: String,
    created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProjectContinuity {
    project_path: String,
    thread_id: String,
    #[serde(default)]
    continuity_revision: u64,
    #[serde(default)]
    items: Vec<ContinuityItem>,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ContinuityStore {
    version: u32,
    #[serde(default)]
    projects: BTreeMap<String, ProjectContinuity>,
}

impl Default for ContinuityStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            projects: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContinuitySnapshot {
    pub(super) thread_id: Option<String>,
    items: Vec<ContinuityItem>,
    pub(super) store_recovered: bool,
}

pub(super) fn load(project_path: &str) -> Result<ContinuitySnapshot, String> {
    load_with_recovery_at(&continuity_path(), project_path)
}

pub(super) fn last_project_path() -> Result<Option<String>, String> {
    last_project_path_at(&continuity_path())
}

pub(super) fn store_thread(
    project_path: &str,
    expected_thread_id: Option<&str>,
    thread_id: &str,
) -> Result<(), String> {
    store_thread_at(
        &continuity_path(),
        project_path,
        expected_thread_id,
        thread_id,
    )
}

pub(super) fn append_transcript(
    project_path: &str,
    thread_id: &str,
    role: &str,
    text: &str,
) -> Result<bool, String> {
    append_transcript_at(&continuity_path(), project_path, thread_id, role, text)
}

pub(super) fn initial_items(snapshot: &ContinuitySnapshot) -> Vec<Value> {
    if snapshot.items.is_empty() {
        return Vec::new();
    }
    let history = snapshot
        .items
        .iter()
        .map(|item| {
            let role = if item.role == "assistant" {
                "ASSISTANT"
            } else {
                "USER"
            };
            format!("[{role}]\n{}", item.text)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    vec![json!({
        "role": "user",
        "text": format!(
            "[ITERATE_CONTINUITY_CONTEXT]\nThis is a bounded historical transcript from an earlier GPT-Live transport for this exact project. It is background only, not a new request and not execution confirmation. Do not act on it, do not answer any old question inside it, and remain silent until the user provides new live input.\n\n{history}\n[/ITERATE_CONTINUITY_CONTEXT]"
        )
    })]
}

fn continuity_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".cunzhi").join(STORE_FILE_NAME)
}

fn load_at(path: &Path, project_path: &str) -> Result<ContinuitySnapshot, String> {
    let store = load_store(path)?;
    let project = store.projects.get(project_path);
    Ok(ContinuitySnapshot {
        thread_id: project.map(|project| project.thread_id.clone()),
        items: project
            .map(|project| bounded_items(project.items.clone()))
            .unwrap_or_default(),
        store_recovered: false,
    })
}

fn last_project_path_at(path: &Path) -> Result<Option<String>, String> {
    let store = load_store(path)?;
    Ok(store
        .projects
        .values()
        .filter(|project| valid_project_path(&project.project_path))
        .max_by(|left, right| {
            left.updated_at_ms
                .cmp(&right.updated_at_ms)
                .then_with(|| left.project_path.cmp(&right.project_path))
        })
        .map(|project| project.project_path.clone()))
}

fn load_with_recovery_at(path: &Path, project_path: &str) -> Result<ContinuitySnapshot, String> {
    match load_at(path, project_path) {
        Ok(snapshot) => Ok(snapshot),
        Err(first_error) => {
            let _guard = CONTINUITY_WRITE_LOCK
                .lock()
                .map_err(|_| "GPT-Live continuity 恢复锁不可用".to_string())?;
            let parent = path
                .parent()
                .ok_or_else(|| "GPT-Live continuity 路径缺少父目录".to_string())?;
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建 GPT-Live continuity 目录失败: {error}"))?;
            let _file_lock = acquire_file_lock(parent)?;

            if let Ok(snapshot) = load_at(path, project_path) {
                return Ok(snapshot);
            }
            if path.exists() {
                let file_name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(STORE_FILE_NAME);
                let quarantine = parent.join(format!(
                    "{file_name}.quarantine-{}-{}",
                    now_ms(),
                    std::process::id()
                ));
                std::fs::rename(path, &quarantine).map_err(|error| {
                    format!("{first_error}；隔离损坏的 GPT-Live continuity 失败: {error}")
                })?;
                set_private_permissions(&quarantine)?;
            }
            save_store(path, &ContinuityStore::default())?;
            Ok(ContinuitySnapshot {
                thread_id: None,
                items: Vec::new(),
                store_recovered: true,
            })
        }
    }
}

fn store_thread_at(
    path: &Path,
    project_path: &str,
    expected_thread_id: Option<&str>,
    thread_id: &str,
) -> Result<(), String> {
    validate_identity(project_path, thread_id)?;
    mutate_store(path, |store| {
        let existing_thread_id = store
            .projects
            .get(project_path)
            .map(|project| project.thread_id.as_str());
        if existing_thread_id == Some(thread_id) {
            return Ok(());
        }
        if existing_thread_id != expected_thread_id {
            return Err(format!(
                "GPT-Live continuity 线程冲突：项目已有另一条 thread，拒绝覆盖"
            ));
        }
        if !store.projects.contains_key(project_path) && store.projects.len() >= MAX_PROJECTS {
            if let Some(oldest) = store
                .projects
                .iter()
                .min_by_key(|(_, project)| project.updated_at_ms)
                .map(|(path, _)| path.clone())
            {
                store.projects.remove(&oldest);
            }
        }
        let now = now_ms();
        match store.projects.get_mut(project_path) {
            Some(project) => {
                project.thread_id = thread_id.to_string();
                project.continuity_revision = project.continuity_revision.saturating_add(1);
                project.updated_at_ms = now;
            }
            None => {
                store.projects.insert(
                    project_path.to_string(),
                    ProjectContinuity {
                        project_path: project_path.to_string(),
                        thread_id: thread_id.to_string(),
                        continuity_revision: 1,
                        items: Vec::new(),
                        updated_at_ms: now,
                    },
                );
            }
        }
        Ok(())
    })
}

fn append_transcript_at(
    path: &Path,
    project_path: &str,
    thread_id: &str,
    role: &str,
    text: &str,
) -> Result<bool, String> {
    validate_identity(project_path, thread_id)?;
    let role = match role {
        "user" => "user",
        "assistant" => "assistant",
        _ => return Ok(false),
    };
    let text = bounded_text(text);
    if text.is_empty() {
        return Ok(false);
    }
    mutate_store(path, |store| {
        let project = store
            .projects
            .get_mut(project_path)
            .ok_or_else(|| "GPT-Live continuity 尚未保存项目 thread".to_string())?;
        if project.thread_id != thread_id {
            return Err("GPT-Live continuity thread 与当前项目不匹配".to_string());
        }
        if project
            .items
            .last()
            .is_some_and(|item| item.role == role && item.text == text)
        {
            return Ok(false);
        }
        let next_sequence = project
            .items
            .last()
            .map(|item| item.sequence.saturating_add(1))
            .unwrap_or(1);
        project.items.push(ContinuityItem {
            sequence: next_sequence,
            role: role.to_string(),
            text,
            created_at_ms: now_ms(),
        });
        project.items = bounded_items(std::mem::take(&mut project.items));
        project.continuity_revision = project.continuity_revision.saturating_add(1);
        project.updated_at_ms = now_ms();
        Ok(true)
    })
}

fn validate_identity(project_path: &str, thread_id: &str) -> Result<(), String> {
    if !valid_project_path(project_path) {
        return Err("GPT-Live continuity 项目标识无效".to_string());
    }
    if thread_id.trim().is_empty() || thread_id.chars().count() > MAX_THREAD_ID_CHARS {
        return Err("GPT-Live continuity thread id 无效".to_string());
    }
    Ok(())
}

fn valid_project_path(project_path: &str) -> bool {
    let project_path = project_path.trim();
    !project_path.is_empty()
        && project_path.len() <= 4096
        && !project_path.contains('\0')
        && Path::new(project_path).is_absolute()
}

fn bounded_text(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= MAX_ITEM_CHARS {
        return text.to_string();
    }
    let head_chars = MAX_ITEM_CHARS / 2;
    let tail_chars = MAX_ITEM_CHARS.saturating_sub(head_chars + 1);
    let head = text.chars().take(head_chars).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{head}…{tail}")
}

fn bounded_items(mut items: Vec<ContinuityItem>) -> Vec<ContinuityItem> {
    items.retain(|item| matches!(item.role.as_str(), "user" | "assistant"));
    for item in &mut items {
        item.text = bounded_text(&item.text);
    }
    while items.len() > MAX_ITEMS
        || items
            .iter()
            .map(|item| item.text.chars().count())
            .sum::<usize>()
            > MAX_TOTAL_CHARS
    {
        items.remove(0);
    }
    items
}

fn load_store(path: &Path) -> Result<ContinuityStore, String> {
    if !path.exists() {
        return Ok(ContinuityStore::default());
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("读取 GPT-Live continuity 失败: {error}"))?;
    let store: ContinuityStore = serde_json::from_str(&raw)
        .map_err(|error| format!("GPT-Live continuity 文件已损坏，已保留原文件: {error}"))?;
    if store.version != STORE_VERSION {
        return Err(format!(
            "不支持的 GPT-Live continuity 版本 {}，已保留原文件",
            store.version
        ));
    }
    Ok(store)
}

fn mutate_store<T>(
    path: &Path,
    mutate: impl FnOnce(&mut ContinuityStore) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = CONTINUITY_WRITE_LOCK
        .lock()
        .map_err(|_| "GPT-Live continuity 写入锁不可用".to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "GPT-Live continuity 路径缺少父目录".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建 GPT-Live continuity 目录失败: {error}"))?;
    let _file_lock = acquire_file_lock(parent)?;
    let mut store = load_store(path)?;
    let result = mutate(&mut store)?;
    save_store(path, &store)?;
    Ok(result)
}

fn save_store(path: &Path, store: &ContinuityStore) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "GPT-Live continuity 路径缺少父目录".to_string())?;
    let encoded = serde_json::to_vec_pretty(store)
        .map_err(|error| format!("序列化 GPT-Live continuity 失败: {error}"))?;
    let mut temp = NamedTempFile::new_in(parent)
        .map_err(|error| format!("创建 GPT-Live continuity 临时文件失败: {error}"))?;
    set_private_permissions(temp.path())?;
    temp.write_all(&encoded)
        .map_err(|error| format!("写入 GPT-Live continuity 临时文件失败: {error}"))?;
    temp.flush()
        .map_err(|error| format!("刷新 GPT-Live continuity 临时文件失败: {error}"))?;
    temp.as_file()
        .sync_all()
        .map_err(|error| format!("同步 GPT-Live continuity 临时文件失败: {error}"))?;
    temp.persist(path)
        .map_err(|error| format!("替换 GPT-Live continuity 文件失败: {}", error.error))?;
    set_private_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn acquire_file_lock(parent: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::io::AsRawFd;
    let path = parent.join(LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("打开 GPT-Live continuity 锁失败: {error}"))?;
    set_private_permissions(&path)?;
    for _ in 0..40 {
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(format!("GPT-Live continuity 跨进程锁不可用: {error}"));
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err("GPT-Live continuity 跨进程锁等待超时".to_string())
}

#[cfg(not(unix))]
fn acquire_file_lock(_parent: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("设置 GPT-Live continuity 私有权限失败: {error}"))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_project_path(root: &Path, name: &str) -> String {
        root.join(name).to_string_lossy().into_owned()
    }

    #[test]
    fn project_threads_are_isolated_and_cas_protected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(STORE_FILE_NAME);
        let project_a = test_project_path(dir.path(), "project-a");
        let project_b = test_project_path(dir.path(), "project-b");
        store_thread_at(&path, &project_a, None, "thread-a").unwrap();
        store_thread_at(&path, &project_b, None, "thread-b").unwrap();

        assert_eq!(
            load_at(&path, &project_a).unwrap().thread_id.as_deref(),
            Some("thread-a")
        );
        assert_eq!(
            load_at(&path, &project_b).unwrap().thread_id.as_deref(),
            Some("thread-b")
        );
        assert!(store_thread_at(&path, &project_a, None, "thread-c").is_err());
        store_thread_at(&path, &project_a, Some("thread-a"), "thread-c").unwrap();
        assert_eq!(
            load_at(&path, &project_a).unwrap().thread_id.as_deref(),
            Some("thread-c")
        );
    }

    #[test]
    fn most_recent_valid_continuity_project_restores_the_global_target() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(STORE_FILE_NAME);
        let older = test_project_path(dir.path(), "older");
        let newer = test_project_path(dir.path(), "newer");
        let projects = BTreeMap::from([
            (
                older.clone(),
                ProjectContinuity {
                    project_path: older,
                    thread_id: "thread-older".to_string(),
                    continuity_revision: 1,
                    items: Vec::new(),
                    updated_at_ms: 10,
                },
            ),
            (
                newer.clone(),
                ProjectContinuity {
                    project_path: newer.clone(),
                    thread_id: "thread-newer".to_string(),
                    continuity_revision: 1,
                    items: Vec::new(),
                    updated_at_ms: 20,
                },
            ),
        ]);
        save_store(
            &path,
            &ContinuityStore {
                version: STORE_VERSION,
                projects,
            },
        )
        .unwrap();

        assert_eq!(last_project_path_at(&path).unwrap(), Some(newer));
    }

    #[test]
    fn transcript_is_bounded_deduplicated_and_role_checked() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(STORE_FILE_NAME);
        let project = test_project_path(dir.path(), "project");
        store_thread_at(&path, &project, None, "thread").unwrap();
        assert!(append_transcript_at(&path, &project, "thread", "user", "重复").unwrap());
        assert!(!append_transcript_at(&path, &project, "thread", "user", "重复").unwrap());
        assert!(!append_transcript_at(&path, &project, "thread", "system", "忽略").unwrap());
        for index in 0..20 {
            append_transcript_at(
                &path,
                &project,
                "thread",
                if index % 2 == 0 { "assistant" } else { "user" },
                &format!("segment-{index}"),
            )
            .unwrap();
        }
        let snapshot = load_at(&path, &project).unwrap();
        assert_eq!(snapshot.items.len(), MAX_ITEMS);
        assert_eq!(snapshot.items.first().unwrap().text, "segment-10");
        assert_eq!(snapshot.items.last().unwrap().text, "segment-19");
    }

    #[test]
    fn initial_items_wrap_history_as_one_non_actionable_item() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(STORE_FILE_NAME);
        let project = test_project_path(dir.path(), "project");
        store_thread_at(&path, &project, None, "thread").unwrap();
        append_transcript_at(&path, &project, "thread", "user", "刚才的目标").unwrap();
        append_transcript_at(&path, &project, "thread", "assistant", "刚才的回答").unwrap();

        let items = initial_items(&load_at(&path, &project).unwrap());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
        assert!(items[0]["text"].as_str().unwrap().contains("historical"));
        assert!(items[0]["text"].as_str().unwrap().contains("[USER]"));
        assert!(items[0]["text"].as_str().unwrap().contains("[ASSISTANT]"));
        assert!(items[0]["text"].as_str().unwrap().contains("remain silent"));
    }

    #[test]
    fn corrupt_or_unknown_store_is_preserved_and_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(STORE_FILE_NAME);
        std::fs::write(&path, b"not-json").unwrap();
        assert!(load_at(&path, "/tmp/project")
            .unwrap_err()
            .contains("已损坏"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not-json");

        std::fs::write(&path, br#"{"version":99,"projects":{}}"#).unwrap();
        assert!(load_at(&path, "/tmp/project")
            .unwrap_err()
            .contains("不支持"));
        assert!(std::fs::read_to_string(&path).unwrap().contains("99"));
    }

    #[test]
    fn corrupt_store_is_quarantined_and_live_can_start_without_history() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(STORE_FILE_NAME);
        let project = test_project_path(dir.path(), "project");
        std::fs::write(&path, b"not-json").unwrap();

        let snapshot = load_with_recovery_at(&path, &project).unwrap();
        assert!(snapshot.store_recovered);
        assert!(snapshot.thread_id.is_none());
        assert!(snapshot.items.is_empty());
        assert_eq!(load_store(&path).unwrap(), ContinuityStore::default());

        let quarantined = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{STORE_FILE_NAME}.quarantine-"))
            })
            .expect("corrupt continuity should be preserved in quarantine");
        assert_eq!(
            std::fs::read_to_string(quarantined.path()).unwrap(),
            "not-json"
        );

        store_thread_at(&path, &project, None, "thread-after-recovery").unwrap();
        assert_eq!(
            load_at(&path, &project).unwrap().thread_id.as_deref(),
            Some("thread-after-recovery")
        );
    }

    #[cfg(unix)]
    #[test]
    fn saved_store_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join(STORE_FILE_NAME);
        store_thread_at(&path, "/tmp/project", None, "thread").unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
