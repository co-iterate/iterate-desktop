//! 首次接入 AI 客户端时，为用户生成一份入门知识库，并把 ji / hui / xi 三个 Skill 挂到客户端。
//!
//! 设计约束：
//! - 知识库根目录由 `config::paths::knowledge_root()` 决定（与对话日志写入位置一致）：
//!   `CUNZHI_KNOWLEDGE_DIR`，未设置时为 `~/.cunzhi-knowledge`。
//! - 只装一次：生成后写入 `.iterate-starter.json` 标记，此后归用户所有，iterate 不再覆盖任何文件。
//! - 已有知识库（存在 `experience/` 但没有标记）视为用户自建，整体跳过，不写文件也不挂 Skill。
//! - Skill 以符号链接挂到 `~/.claude/skills/<name>`、`$CODEX_HOME/skills/<name>`，用户改知识库里的
//!   SKILL.md 即对所有客户端生效；目标已存在（含断链）一律跳过。Windows 上改为复制目录。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::client_install::ClientKind;

const MARKER_FILE: &str = ".iterate-starter.json";
pub const STARTER_SKILLS: [&str; 3] = ["ji", "hui", "xi"];

macro_rules! starter_file {
    ($path:literal) => {
        ($path, include_str!(concat!("../assets/knowledge-starter/", $path)))
    };
}

const STARTER_FILES: &[(&str, &str)] = &[
    starter_file!("README.md"),
    starter_file!("AGENTS.md"),
    starter_file!("conversations/README.md"),
    starter_file!("experience/README.md"),
    starter_file!("experience/problems.md"),
    starter_file!("experience/regressions.md"),
    starter_file!("experience/patterns.md"),
    starter_file!("templates/problem_entry.md"),
    starter_file!("templates/regression_entry.md"),
    starter_file!("templates/pattern_entry.md"),
    starter_file!("prompts/skills/INDEX.md"),
    starter_file!("prompts/skills/ji/SKILL.md"),
    starter_file!("prompts/skills/hui/SKILL.md"),
    starter_file!("prompts/skills/xi/SKILL.md"),
];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StarterReport {
    /// 本次新写入的知识库文件（相对路径）。
    pub created_files: Vec<String>,
    /// 本次新挂上的 Skill 目标路径。
    pub linked_skills: Vec<PathBuf>,
    /// 用户自建知识库，整体跳过。
    pub user_owned: bool,
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Claude Code 的配置目录：`CLAUDE_CONFIG_DIR`、`~/.claude`，以及 `~/.claude-*` 这类多账号目录。
/// 从 Dock 启动的 App 拿不到 shell 里的 `CLAUDE_CONFIG_DIR`，所以要主动扫描 Home。
fn claude_config_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(configured) = env_dir("CLAUDE_CONFIG_DIR") {
        dirs.push(configured);
    }
    dirs.push(home.join(".claude"));
    if let Ok(entries) = fs::read_dir(home) {
        let mut extra: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".claude-"))
                    && (path.join("settings.json").is_file() || path.join("skills").is_dir())
            })
            .collect();
        extra.sort();
        dirs.extend(extra);
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

/// 各客户端的全局 Skill 目录；不支持 Skill 的客户端返回空。
pub fn client_skill_dirs(kind: ClientKind) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    match kind {
        ClientKind::ClaudeCode => claude_config_dirs(&home).into_iter().map(|dir| dir.join("skills")).collect(),
        ClientKind::Codex => vec![env_dir("CODEX_HOME").unwrap_or_else(|| home.join(".codex")).join("skills")],
        ClientKind::Cursor | ClientKind::Antigravity => Vec::new(),
    }
}

/// 设置页展示用的标签：默认目录只写客户端名，其他目录带上 `~/` 开头的路径。
fn skill_dir_label(kind: ClientKind, skills_dir: &Path) -> String {
    let config_dir = skills_dir.parent().unwrap_or(skills_dir);
    let default_name = match kind {
        ClientKind::ClaudeCode => ".claude",
        _ => ".codex",
    };
    let is_default = config_dir.file_name().and_then(|n| n.to_str()) == Some(default_name);
    if is_default {
        kind.display_name().to_string()
    } else {
        let shown = dirs::home_dir()
            .and_then(|home| config_dir.strip_prefix(&home).ok().map(|rel| format!("~/{}", rel.display())))
            .unwrap_or_else(|| config_dir.display().to_string());
        format!("{}（{}）", kind.display_name(), shown)
    }
}

/// 在知识库里找某个 Skill 的目录：先找同名目录；找不到时，找 SKILL.md 的
/// name 或 description 里把它列为触发词的目录（例如老知识库把 ji 放在 settle 里）。
/// 匹配到多个时不猜，返回 None。
pub fn resolve_skill_source(root: &Path, name: &str) -> Option<PathBuf> {
    let skills_root = root.join("prompts").join("skills");
    let exact = skills_root.join(name);
    if exact.join("SKILL.md").is_file() {
        return Some(exact);
    }
    let mut matches = Vec::new();
    for entry in fs::read_dir(&skills_root).ok()?.flatten() {
        let dir = entry.path();
        let Ok(text) = fs::read_to_string(dir.join("SKILL.md")) else { continue };
        let Some(front) = text.strip_prefix("---\n").and_then(|rest| rest.split("\n---").next()) else { continue };
        let hit = front.lines().any(|line| {
            let value = line
                .strip_prefix("name:")
                .or_else(|| line.strip_prefix("description:"))
                .unwrap_or("");
            value
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .any(|token| token == name)
        });
        if hit {
            matches.push(dir);
        }
    }
    (matches.len() == 1).then(|| matches.remove(0))
}

/// 把知识库里已有的 ji / hui / xi 链接到各 Skill 目录；只建链接，不往知识库写任何文件。
pub fn link_skills(root: &Path, skill_dirs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut linked = Vec::new();
    for skills_dir in skill_dirs {
        for name in STARTER_SKILLS {
            let Some(source) = resolve_skill_source(root, name) else { continue };
            let target = skills_dir.join(name);
            // symlink_metadata 能识别断链，断链也视为已占用，不替用户清理。
            if fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            fs::create_dir_all(skills_dir).map_err(|e| format!("创建 {} 失败: {e}", skills_dir.display()))?;
            link_skill(&source, &target)?;
            linked.push(target);
        }
    }
    Ok(linked)
}

#[derive(Debug, serde::Serialize)]
pub struct SkillLinkStatus {
    pub client: String,
    pub name: String,
    pub path: String,
    pub present: bool,
    /// 知识库里能找到这个 Skill 的来源目录（找不到就无法挂载）。
    pub available: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct KnowledgeOverview {
    pub root: String,
    /// "starter"：iterate 生成；"user"：用户自建；"missing"：不存在或还没初始化。
    pub state: &'static str,
    pub skills: Vec<SkillLinkStatus>,
    pub conversation_logging: bool,
}

const SKILL_CLIENTS: [ClientKind; 2] = [ClientKind::ClaudeCode, ClientKind::Codex];

fn knowledge_state(root: &Path) -> &'static str {
    if root.join(MARKER_FILE).exists() {
        "starter"
    } else if root.join("experience").exists() {
        "user"
    } else {
        "missing"
    }
}

pub fn overview() -> Result<KnowledgeOverview, String> {
    let root = crate::config::paths::knowledge_root().ok_or("无法确定知识库目录")?;
    let mut skills = Vec::new();
    for kind in SKILL_CLIENTS {
        for dir in client_skill_dirs(kind) {
            let label = skill_dir_label(kind, &dir);
            for name in STARTER_SKILLS {
                let path = dir.join(name);
                skills.push(SkillLinkStatus {
                    client: label.clone(),
                    name: name.to_string(),
                    present: fs::symlink_metadata(&path).is_ok(),
                    available: resolve_skill_source(&root, name).is_some(),
                    path: path.to_string_lossy().to_string(),
                });
            }
        }
    }
    Ok(KnowledgeOverview {
        root: root.to_string_lossy().to_string(),
        state: knowledge_state(&root),
        conversation_logging: conversation_logging_enabled(&root),
        skills,
    })
}

#[tauri::command]
pub async fn get_knowledge_overview() -> Result<KnowledgeOverview, String> {
    tokio::task::spawn_blocking(overview).await.map_err(|e| e.to_string())?
}

/// 设置页「创建知识库 / 挂载 Skill」：只给本机已存在配置目录的客户端挂 Skill。
/// 用户自建的知识库只建链接，不写入任何文件。
#[tauri::command]
pub async fn install_knowledge_starter() -> Result<KnowledgeOverview, String> {
    tokio::task::spawn_blocking(|| {
        let root = crate::config::paths::knowledge_root().ok_or("无法确定知识库目录")?;
        let skill_dirs: Vec<PathBuf> = SKILL_CLIENTS
            .iter()
            .flat_map(|kind| client_skill_dirs(*kind))
            .filter(|dir| dir.parent().is_some_and(Path::is_dir))
            .collect();
        if knowledge_state(&root) == "user" {
            link_skills(&root, &skill_dirs)?;
        } else {
            install(&root, &skill_dirs)?;
        }
        overview()
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn open_knowledge_dir() -> Result<(), String> {
    let root = crate::config::paths::knowledge_root().ok_or("无法确定知识库目录")?;
    if !root.is_dir() {
        return Err("知识库目录还不存在".to_string());
    }
    let opener = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&root)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开知识库目录: {e}"))
}

/// 为已成功接入的客户端准备入门知识库。失败只记日志，不影响 MCP 接入结果。
/// 本次新建了知识库时返回给用户看的说明（含对话记录告知）。
pub fn ensure_for_clients(kinds: &[ClientKind]) -> Option<String> {
    let Some(root) = crate::config::paths::knowledge_root() else {
        log::warn!("入门知识库：无法确定知识库目录");
        return None;
    };
    let skill_dirs: Vec<PathBuf> = kinds.iter().flat_map(|kind| client_skill_dirs(*kind)).collect();
    match install(&root, &skill_dirs) {
        Ok(report) if report.user_owned => {
            log::info!("入门知识库：{} 已是用户自建知识库，跳过", root.display());
            None
        }
        Ok(report) => {
            log::info!(
                "入门知识库：{} 新建 {} 个文件，挂载 {} 个 Skill",
                root.display(),
                report.created_files.len(),
                report.linked_skills.len()
            );
            (!report.created_files.is_empty()).then(|| created_notice(&root))
        }
        Err(error) => {
            log::warn!("入门知识库安装失败: {error}");
            None
        }
    }
}

fn created_notice(root: &Path) -> String {
    format!(
        "已在 {} 创建知识库，重启客户端后说 hui / xi / ji 即可使用。\
         iterate 会把弹窗对话（自动脱敏密钥）记在其中的 conversations/，供 hui 回溯；\
         不想记录可在「设置 → 知识库」关闭。",
        root.display()
    )
}

/// 存在即停止写对话记录；放在知识库里，MCP 服务进程和 App 都能直接看到。
pub const CONVERSATION_LOG_OPT_OUT: &str = ".no-conversation-log";

pub fn conversation_logging_enabled(root: &Path) -> bool {
    !root.join(CONVERSATION_LOG_OPT_OUT).exists()
}

#[tauri::command]
pub async fn set_conversation_logging(enabled: bool) -> Result<KnowledgeOverview, String> {
    tokio::task::spawn_blocking(move || {
        let root = crate::config::paths::knowledge_root().ok_or("无法确定知识库目录")?;
        if !root.is_dir() {
            return Err("知识库目录还不存在".to_string());
        }
        let flag = root.join(CONVERSATION_LOG_OPT_OUT);
        if enabled {
            match fs::remove_file(&flag) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("开启对话记录失败: {error}")),
            }
        } else {
            write_new(&flag, "iterate: 存在此文件时不记录弹窗对话。删除即恢复。\n")?;
        }
        overview()
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn install(root: &Path, skill_dirs: &[PathBuf]) -> Result<StarterReport, String> {
    let mut report = StarterReport::default();
    let marker = root.join(MARKER_FILE);

    if !marker.exists() {
        if root.join("experience").exists() {
            report.user_owned = true;
            return Ok(report);
        }
        for (relative, content) in STARTER_FILES {
            if write_new(&root.join(relative), content)? {
                report.created_files.push((*relative).to_string());
            }
        }
        let stamp = format!(
            "{{\n  \"schema\": \"iterate.knowledge-starter.v1\",\n  \"app_version\": \"{}\"\n}}\n",
            env!("CARGO_PKG_VERSION")
        );
        write_new(&marker, &stamp)?;
    }

    report.linked_skills = link_skills(root, skill_dirs)?;
    Ok(report)
}

/// 仅在文件不存在时写入；返回是否写入。
fn write_new(path: &Path, content: &str) -> Result<bool, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    match fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(content.as_bytes())
                .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(format!("写入 {} 失败: {error}", path.display())),
    }
}

#[cfg(unix)]
fn link_skill(source: &Path, target: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(source, target)
        .map_err(|e| format!("链接 {} -> {} 失败: {e}", target.display(), source.display()))
}

#[cfg(not(unix))]
fn link_skill(source: &Path, target: &Path) -> Result<(), String> {
    // Windows 创建符号链接需要开发者模式或管理员权限，直接复制一份。
    fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let dest = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir(&entry.path(), &dest)?;
            } else {
                fs::copy(entry.path(), dest)?;
            }
        }
        Ok(())
    }
    copy_dir(source, target).map_err(|e| format!("复制 {} 失败: {e}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_install_creates_files_marker_and_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("kb");
        let claude = temp.path().join("claude-skills");

        let report = install(&root, &[claude.clone()]).unwrap();

        assert!(!report.user_owned);
        assert_eq!(report.created_files.len(), STARTER_FILES.len());
        assert!(root.join(MARKER_FILE).is_file());
        assert!(root.join("experience/problems.md").is_file());
        for name in STARTER_SKILLS {
            assert!(claude.join(name).join("SKILL.md").is_file(), "{name} 未挂载");
        }
        let ji = fs::read_to_string(claude.join("ji/SKILL.md")).unwrap();
        assert!(ji.starts_with("---\nname: ji\n"));
    }

    #[test]
    fn conversations_dir_from_logger_does_not_block_install() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("kb");
        fs::create_dir_all(root.join("conversations/2026-11-01")).unwrap();

        let report = install(&root, &[]).unwrap();
        assert!(!report.user_owned);
        assert!(root.join("prompts/skills/hui/SKILL.md").is_file());
    }

    #[test]
    fn second_run_never_overwrites_user_edits() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("kb");
        install(&root, &[]).unwrap();
        let skill = root.join("prompts/skills/ji/SKILL.md");
        fs::write(&skill, "user edit").unwrap();
        fs::remove_file(root.join("AGENTS.md")).unwrap();

        let report = install(&root, &[]).unwrap();
        assert!(report.created_files.is_empty());
        assert_eq!(fs::read_to_string(&skill).unwrap(), "user edit");
        assert!(!root.join("AGENTS.md").exists(), "用户删掉的文件不应被补回");
    }

    #[test]
    fn existing_user_knowledge_base_is_left_alone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("kb");
        fs::create_dir_all(root.join("experience")).unwrap();
        fs::create_dir_all(root.join("prompts/skills/hui")).unwrap();
        let claude = temp.path().join("claude-skills");

        let report = install(&root, &[claude.clone()]).unwrap();
        assert!(report.user_owned);
        assert!(!root.join(MARKER_FILE).exists());
        assert!(!root.join("README.md").exists());
        assert!(!claude.exists());
    }

    #[test]
    fn user_owned_base_links_existing_skills_without_writing_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("kb");
        fs::create_dir_all(root.join("experience")).unwrap();
        let skills = root.join("prompts/skills");
        fs::create_dir_all(skills.join("hui")).unwrap();
        fs::write(skills.join("hui/SKILL.md"), "---\nname: hui\ndescription: 回溯\n---\n").unwrap();
        fs::create_dir_all(skills.join("settle")).unwrap();
        fs::write(
            skills.join("settle/SKILL.md"),
            "---\nname: settle\ndescription: 问题沉淀流程。触发词：沉淀、settle、ji、ji0、ji1。\n---\n",
        )
        .unwrap();
        let claude = temp.path().join("claude-skills");

        let linked = link_skills(&root, &[claude.clone()]).unwrap();

        assert_eq!(linked.len(), 2, "hui 和 ji(settle) 应挂上，xi 不存在应跳过");
        assert_eq!(fs::read_link(claude.join("ji")).unwrap(), skills.join("settle"));
        assert!(!claude.join("xi").exists());
        assert!(!root.join(MARKER_FILE).exists());
        assert!(!root.join("README.md").exists());
    }

    #[test]
    fn ambiguous_trigger_word_is_not_guessed() {
        let temp = tempfile::tempdir().unwrap();
        let skills = temp.path().join("prompts/skills");
        for dir in ["a", "b"] {
            fs::create_dir_all(skills.join(dir)).unwrap();
            fs::write(skills.join(dir).join("SKILL.md"), "---\nname: x\ndescription: 触发词：ji\n---\n").unwrap();
        }
        assert!(resolve_skill_source(temp.path(), "ji").is_none());
        assert!(resolve_skill_source(temp.path(), "jiang").is_none());
    }

    #[test]
    fn extra_claude_config_dirs_are_detected() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".claude-pro2/skills")).unwrap();
        fs::create_dir_all(home.path().join(".claude-empty")).unwrap();
        let dirs = claude_config_dirs(home.path());
        assert!(dirs.contains(&home.path().join(".claude")));
        assert!(dirs.contains(&home.path().join(".claude-pro2")));
        assert!(!dirs.contains(&home.path().join(".claude-empty")));
    }

    #[test]
    fn existing_client_skill_is_not_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("kb");
        let claude = temp.path().join("claude-skills");
        fs::create_dir_all(claude.join("hui")).unwrap();
        fs::write(claude.join("hui/SKILL.md"), "mine").unwrap();

        let report = install(&root, &[claude.clone()]).unwrap();
        assert_eq!(report.linked_skills.len(), 2);
        assert_eq!(fs::read_to_string(claude.join("hui/SKILL.md")).unwrap(), "mine");
    }
}
