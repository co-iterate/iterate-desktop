//! 手机端 `+` 选 Claude 模型时，在 Mac 的 Terminal.app 里启动 Claude Code。
//!
//! Claude 条目追加在 `/api/codex-models` 的模型列表后面，id 以 `claude:` 开头；
//! `/api/open-codex-chat` 收到这类 id 时走这里，不再交给 Codex app-server。
//! `+` 的首句是 `zhi`，让新会话立刻弹到手机上，之后由 iterate 继续指挥。

use crate::mcp::caller_identity::{shell_single_quote, TERMINAL_RUN_COMMAND_SCRIPT};
use serde_json::{json, Value};
use std::collections::HashSet;

const MODEL_ID_PREFIX: &str = "claude:";
const DEFAULT_LAUNCHER: &str = "claude";
const DEFAULT_EFFORT: &str = "high";
/// 手机端 + 请求 10 秒超时，整条启动链路要在这之内给出答复：
/// 型号探测最多 2 次 × 2.5 秒，osascript 最多同步等 3 秒，合计约 8 秒。
const SDK_PROBE_TIMEOUT_MS: u64 = 2_500;
const OSASCRIPT_REPLY_WAIT_MS: u64 = 3_000;
/// 首次控制 Terminal 时 macOS 会弹自动化授权框，osascript 会一直等到有人点；
/// 超过同步等待时间后在后台继续等，结果只写日志。
const OSASCRIPT_BACKGROUND_TIMEOUT_SECS: u64 = 120;
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Only used if Claude Code cannot report resolved model versions.
const MODELS: [(&str, &str); 4] = [
    ("opus", "Claude Opus"),
    ("fable", "Claude Fable"),
    ("sonnet", "Claude Sonnet"),
    ("haiku", "Claude Haiku"),
];

/// 手机 + 没选模型、Mac 又没装 Codex 时，默认用这个型号启动 Claude（推理档位取型号默认值）。
pub(super) const NO_MODEL_FALLBACK_ID: &str = "claude:opus";

/// 没选模型时的回退目录：优先用手机传来的绝对路径，否则用用户主目录。
pub(super) fn no_model_fallback_project(
    project_path: Option<&str>,
    home: Option<std::path::PathBuf>,
) -> Option<String> {
    project_path
        .map(str::trim)
        .filter(|path| !path.is_empty() && std::path::Path::new(path).is_absolute())
        .map(ToOwned::to_owned)
        .or_else(|| home.map(|home| home.display().to_string()))
}

pub(super) fn is_claude_model_id(model_id: &str) -> bool {
    model_id.starts_with(MODEL_ID_PREFIX)
}

fn valid_model_name(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 200
        && !model.starts_with('-')
        && model.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '-' | '_' | '.' | ':' | '/' | '@' | '[' | ']')
        })
}

fn model_display_name(model: &str, fallback: &str) -> String {
    let plain = model.strip_suffix("[1m]").unwrap_or(model);
    let parts: Vec<&str> = plain.split('-').collect();
    if parts.len() >= 3 && parts[0] == "claude" {
        let family = match parts[1] {
            "opus" => Some("Opus"),
            "fable" => Some("Fable"),
            "sonnet" => Some("Sonnet"),
            "haiku" => Some("Haiku"),
            _ => None,
        };
        if let Some(family) = family {
            let major = parts[2];
            if !major.is_empty() && major.len() <= 2 && major.chars().all(|ch| ch.is_ascii_digit())
            {
                let version = match parts.get(3) {
                    Some(minor)
                        if !minor.is_empty()
                            && minor.len() <= 2
                            && minor.chars().all(|ch| ch.is_ascii_digit()) =>
                    {
                        format!("{major}.{minor}")
                    }
                    _ => major.to_string(),
                };
                return format!("Claude {family} {version}");
            }
        }
    }
    if MODELS.iter().any(|(alias, _)| *alias == model) {
        format!("{fallback}（版本未确认）")
    } else {
        format!("{fallback} ({model})")
    }
}

fn legacy_model_id(value: &str, resolved: &str) -> Option<String> {
    let family = if MODELS.iter().any(|(alias, _)| *alias == value) {
        Some(value)
    } else {
        resolved
            .strip_prefix("claude-")
            .and_then(|name| name.split('-').next())
            .filter(|family| MODELS.iter().any(|(alias, _)| alias == family))
    }?;
    Some(format!("{MODEL_ID_PREFIX}{family}"))
}

/// Convert the live Claude Code SDK model list to the existing iOS catalog contract.
fn catalog_entries_from_sdk(models: &Value) -> Vec<Value> {
    let mut seen = HashSet::new();
    let has_non_default = models.as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry
                .get("value")
                .and_then(Value::as_str)
                .is_some_and(|value| value != "default")
        })
    });
    models
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let value = model.get("value")?.as_str()?;
            if value == "default" && has_non_default {
                return None;
            }
            let resolved = match model.get("resolvedModel") {
                Some(Value::String(value)) if valid_model_name(value) => value.as_str(),
                Some(Value::Null) if valid_model_name(value) => value,
                Some(_) => return None,
                None if valid_model_name(value) => value,
                None => return None,
            };
            if !seen.insert(resolved.to_string()) {
                return None;
            }
            let display = model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or("Claude")
                .chars()
                .filter(|ch| !ch.is_control())
                .take(80)
                .collect::<String>();
            let fallback = if display.starts_with("Claude") {
                display
            } else {
                format!("Claude {display}")
            };
            let mut seen_efforts = HashSet::new();
            let mut efforts: Vec<&str> = model
                .get("supportedEffortLevels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|effort| EFFORTS.contains(effort) && seen_efforts.insert(*effort))
                .collect();
            if model.get("supportsEffort").and_then(Value::as_bool) == Some(false) {
                efforts.clear();
            }
            if efforts.is_empty() {
                efforts.push("default");
            }
            let default = if efforts.contains(&"medium") {
                "medium"
            } else {
                efforts[0]
            };
            let legacy = legacy_model_id(value, resolved);
            let description = if legacy.as_deref() == Some("claude:fable") {
                "可能需要额外额度或在 Mac 确认；在终端启动 Claude Code"
            } else {
                "在 Mac 终端启动 Claude Code"
            };
            let id = format!("{MODEL_ID_PREFIX}{resolved}");
            Some(json!({
                "id": id, "model": id, "displayName": model_display_name(resolved, &fallback),
                "description": description, "hidden": false,
                "supportedReasoningEfforts": efforts.iter()
                    .map(|effort| json!({"reasoningEffort": effort, "description": null}))
                    .collect::<Vec<_>>(),
                "defaultReasoningEffort": default, "isDefault": false, "legacyModelID": legacy
            }))
        })
        .collect()
}

fn fallback_catalog_entries() -> Vec<Value> {
    MODELS
        .iter()
        .map(|(alias, name)| {
            let id = format!("{MODEL_ID_PREFIX}{alias}");
            json!({
                "id": id,
                "model": id,
                "displayName": format!("{name}（版本未确认）"),
                "description": if *alias == "fable" {
                    "版本未确认，可能需要额外额度或在 Mac 确认"
                } else {
                    "暂时无法读取具体版本，启动时使用系列别名"
                },
                "hidden": false,
                "supportedReasoningEfforts": EFFORTS
                    .iter()
                    .map(|effort| json!({"reasoningEffort": effort, "description": null}))
                    .collect::<Vec<_>>(),
                "defaultReasoningEffort": DEFAULT_EFFORT,
                "isDefault": false
            })
        })
        .collect()
}

/// Ask the same Claude Code launcher for its current resolved model IDs.
pub(super) async fn catalog_entries() -> Vec<Value> {
    match query_sdk_models(&configured_launcher()).await {
        Ok(models) => {
            let entries = catalog_entries_from_sdk(&models);
            if !entries.is_empty() {
                let ids: Vec<&str> = entries
                    .iter()
                    .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                    .collect();
                log::info!("[Bridge] Claude model catalog resolved: {}", ids.join(", "));
                return entries;
            }
            log::warn!("[Bridge] Claude model catalog contained no usable entries");
        }
        Err(error) => log::warn!("[Bridge] Claude model catalog unavailable: {error}"),
    }
    fallback_catalog_entries()
}

pub(super) struct ClaudeLaunch {
    pub display_name: String,
    pub effort: String,
    /// osascript 在同步等待时间内没返回（多半是 Mac 上弹了自动化授权框），已转到后台继续等。
    pub pending: bool,
}

/// 启动命令前缀：`mobile_config.claude_launch_command`，未配置时为 `claude`。
/// 这是 Mac 主人本机配置的 shell 片段（例如切换账号的包装脚本），按原样执行。
fn configured_launcher() -> String {
    crate::config::storage::load_standalone_config()
        .ok()
        .and_then(|config| config.mobile_config.claude_launch_command)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_LAUNCHER.to_string())
}

#[cfg(target_os = "macos")]
struct CatalogProcess {
    child: tokio::process::Child,
    process_group: Option<i32>,
}

#[cfg(target_os = "macos")]
impl Drop for CatalogProcess {
    fn drop(&mut self) {
        if let Some(group) = self.process_group.take() {
            // This group belongs only to the short-lived, tool-less catalog probe.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
    }
}

#[cfg(target_os = "macos")]
async fn query_sdk_models(launcher: &str) -> Result<Value, String> {
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    if launcher.is_empty() || launcher.chars().any(char::is_control) {
        return Err("无效的 Claude 启动命令".to_string());
    }
    // No user prompt, MCP servers, tools, or user/plugin hooks are run by this probe.
    let settings = shell_single_quote(r#"{"disableAllHooks":true}"#);
    let command = format!(
        "{launcher} --output-format stream-json --verbose --input-format stream-json \
         --max-turns=1 --tools= --setting-sources=user --strict-mcp-config \
         --no-session-persistence --settings {settings}"
    );
    let mut process = tokio::process::Command::new("/bin/zsh");
    process
        .args(["-lc", &command])
        .env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    if let Some(home) = dirs::home_dir() {
        process.current_dir(home);
    }
    let child = process
        .spawn()
        .map_err(|_| "无法启动 Claude 模型查询".to_string())?;
    let group = child.id().and_then(|pid| i32::try_from(pid).ok());
    let mut process = CatalogProcess {
        child,
        process_group: group,
    };
    let mut input = process.child.stdin.take().ok_or("Claude 查询输入不可用")?;
    let output = process.child.stdout.take().ok_or("Claude 查询输出不可用")?;
    let result = tokio::time::timeout(Duration::from_millis(SDK_PROBE_TIMEOUT_MS), async move {
        let request = json!({
            "type": "control_request", "request_id": "iterate-model-catalog",
            "request": {"subtype": "initialize", "systemPrompt": [""]}
        });
        input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(|_| "发送 Claude 模型查询失败".to_string())?;
        input
            .flush()
            .await
            .map_err(|_| "发送 Claude 模型查询失败".to_string())?;
        let mut lines = BufReader::new(output).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "读取 Claude 模型查询失败".to_string())?
        {
            if line.len() > 2_000_000 {
                return Err("Claude 模型查询响应过大".to_string());
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("type").and_then(Value::as_str) != Some("control_response") {
                continue;
            }
            let response = &message["response"];
            if response.get("request_id").and_then(Value::as_str) != Some("iterate-model-catalog") {
                continue;
            }
            if response.get("subtype").and_then(Value::as_str) != Some("success") {
                return Err("Claude 模型查询被拒绝".to_string());
            }
            return response
                .get("response")
                .and_then(|value| value.get("models"))
                .cloned()
                .ok_or_else(|| "Claude 模型查询未返回列表".to_string());
        }
        Err("Claude 模型查询提前退出".to_string())
    })
    .await
    .unwrap_or_else(|_| Err("Claude 模型查询超时".to_string()));
    if let Some(group) = process.process_group.take() {
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = process.child.start_kill();
    let _ = process.child.wait().await;
    result
}

#[cfg(not(target_os = "macos"))]
async fn query_sdk_models(_launcher: &str) -> Result<Value, String> {
    Err("当前系统不支持 Claude 终端模型查询".to_string())
}

fn launch_command(
    launcher: &str,
    project_path: &str,
    prompt: &str,
    alias: &str,
    effort: &str,
) -> Result<String, String> {
    if launcher.chars().any(char::is_control) {
        return Err("Claude 启动命令包含控制字符".to_string());
    }
    if project_path.chars().any(char::is_control) {
        return Err("目标项目路径包含控制字符".to_string());
    }
    if !valid_model_name(alias) || (effort != "default" && !EFFORTS.contains(&effort)) {
        return Err("Claude 模型或推理档位无效".to_string());
    }
    // Terminal 的 do script 按行执行，提示词里的换行折成空格
    let prompt: String = prompt
        .trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if prompt.is_empty() {
        return Err("Claude 启动提示词为空".to_string());
    }
    let effort_option = if effort == "default" {
        String::new()
    } else {
        format!(" --effort {effort}")
    };
    Ok(format!(
        "cd {} && {launcher} --model {}{effort_option} --permission-mode bypassPermissions {}",
        shell_single_quote(project_path),
        shell_single_quote(alias),
        shell_single_quote(&prompt)
    ))
}

fn resolve(model_id: &str, effort: Option<&str>) -> Result<(String, String), String> {
    let alias = model_id.strip_prefix(MODEL_ID_PREFIX).unwrap_or_default();
    if !valid_model_name(alias) {
        return Err(format!("不支持的 Claude 模型: {model_id}"));
    }
    let effort = effort.unwrap_or(DEFAULT_EFFORT);
    if effort != "default" && !EFFORTS.contains(&effort) {
        return Err(format!("Claude 不支持推理档位: {effort}"));
    }
    Ok((alias.to_string(), effort.to_string()))
}

fn resolve_catalog_selection(
    model_id: &str,
    effort: Option<&str>,
    entries: &[Value],
) -> Result<(String, String, String), String> {
    let entry = entries
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(model_id))
        .or_else(|| {
            entries
                .iter()
                .find(|entry| entry.get("legacyModelID").and_then(Value::as_str) == Some(model_id))
        })
        .ok_or_else(|| "当前 Claude 模型列表中没有所选版本，请重新选择".to_string())?;
    let resolved_id = entry
        .get("id")
        .and_then(Value::as_str)
        .ok_or("Claude 型号无效")?;
    let chosen_effort = effort
        .or_else(|| entry.get("defaultReasoningEffort").and_then(Value::as_str))
        .unwrap_or(DEFAULT_EFFORT);
    let (model, effort) = resolve(resolved_id, Some(chosen_effort))?;
    let supported = entry
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values.iter().any(|value| {
                value.get("reasoningEffort").and_then(Value::as_str) == Some(effort.as_str())
            })
        });
    if !supported {
        return Err("所选 Claude 型号不支持当前推理档位，请重新选择".to_string());
    }
    let name = entry
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or("Claude")
        .to_string();
    Ok((model, effort, name))
}

pub(super) async fn launch_in_terminal(
    project_path: &str,
    prompt: &str,
    model_id: &str,
    effort: Option<&str>,
) -> Result<ClaudeLaunch, String> {
    let result = launch_in_terminal_inner(project_path, prompt, model_id, effort).await;
    match &result {
        Ok(launch) => log::info!(
            "[Bridge] claude terminal launch ok: model_id={model_id}, effort={}, name={}, pending_permission={}, project={project_path}",
            launch.effort,
            launch.display_name,
            launch.pending
        ),
        Err(error) => log::warn!(
            "[Bridge] claude terminal launch rejected: model_id={model_id}, effort={}, project={project_path}, error={error}",
            effort.unwrap_or("-")
        ),
    }
    result
}

/// 型号探测失败（超时、claude 正在自动升级等）时重试一次；仍失败就按所选 ID 直接启动，
/// 不再因为“无法确认版本”把整次 + 拒掉。
async fn resolve_launch_selection(
    launcher: &str,
    model_id: &str,
    effort: Option<&str>,
) -> Result<(String, String, String), String> {
    let mut last_error = String::new();
    for attempt in 1..=2 {
        match query_sdk_models(launcher).await {
            Ok(models) => {
                let entries = catalog_entries_from_sdk(&models);
                return resolve_catalog_selection(model_id, effort, &entries);
            }
            Err(error) => {
                log::warn!("[Bridge] Claude model probe failed (attempt {attempt}/2): {error}");
                last_error = error;
            }
        }
    }
    let (alias, effort) = resolve(model_id, effort)?;
    log::warn!(
        "[Bridge] Claude model probe unavailable ({last_error}); launching unverified model={alias}, effort={effort}"
    );
    let name = model_display_name(&alias, "Claude");
    Ok((alias, effort, name))
}

async fn launch_in_terminal_inner(
    project_path: &str,
    prompt: &str,
    model_id: &str,
    effort: Option<&str>,
) -> Result<ClaudeLaunch, String> {
    let project_path = super::codex_live::validated_project_path(project_path)?;
    resolve(model_id, effort)?;
    let launcher = configured_launcher();
    let (alias, effort, name) = resolve_launch_selection(&launcher, model_id, effort).await?;
    let command = launch_command(&launcher, &project_path, prompt, &alias, &effort)?;
    log::info!(
        "[Bridge] claude terminal launch: model={alias}, effort={effort}, project={project_path}"
    );

    let child = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(TERMINAL_RUN_COMMAND_SCRIPT)
        .arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("执行 osascript 失败: {error}"))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let outcome = match tokio::time::timeout(
            std::time::Duration::from_secs(OSASCRIPT_BACKGROUND_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) if output.status.success() => Ok(()),
            Ok(Ok(output)) => Err(osascript_error(
                String::from_utf8_lossy(&output.stderr).trim(),
            )),
            Ok(Err(error)) => Err(format!("执行 osascript 失败: {error}")),
            Err(_) => Err("等待 Terminal 授权超时，osascript 已结束".to_string()),
        };
        if let Err(_unsent) = tx.send(outcome.clone()) {
            // 手机已经拿到“等待授权”的答复，这里只记录最终结果。
            match outcome {
                Ok(()) => log::info!("[Bridge] claude terminal launch completed after waiting"),
                Err(error) => {
                    log::warn!("[Bridge] claude terminal launch failed after waiting: {error}")
                }
            }
        }
    });
    let pending = match tokio::time::timeout(
        std::time::Duration::from_millis(OSASCRIPT_REPLY_WAIT_MS),
        rx,
    )
    .await
    {
        Ok(Ok(Ok(()))) => false,
        Ok(Ok(Err(error))) => return Err(error),
        Ok(Err(_)) => return Err("osascript 执行失败".to_string()),
        Err(_) => {
            log::warn!(
                "[Bridge] osascript still running after {OSASCRIPT_REPLY_WAIT_MS}ms; likely waiting on the Terminal Automation prompt"
            );
            true
        }
    };
    Ok(ClaudeLaunch {
        display_name: name,
        effort,
        pending,
    })
}

fn osascript_error(stderr: &str) -> String {
    log::warn!("[Bridge] claude terminal launch failed: {stderr}");
    if stderr.contains("-1743") {
        "Mac 上还没允许 iterate 控制 Terminal：请在 系统设置 → 隐私与安全性 → 自动化 中打开"
            .to_string()
    } else if stderr.is_empty() {
        "osascript 执行失败".to_string()
    } else {
        format!("打开 Terminal 失败: {stderr}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_model_fallback_uses_valid_claude_id() {
        assert!(is_claude_model_id(NO_MODEL_FALLBACK_ID));
        let (alias, effort) = resolve(NO_MODEL_FALLBACK_ID, None).unwrap();
        assert_eq!(alias, "opus");
        assert_eq!(effort, DEFAULT_EFFORT);
    }

    #[test]
    fn no_model_fallback_project_prefers_absolute_phone_path() {
        let home = Some(std::path::PathBuf::from("/Users/example/me"));
        assert_eq!(
            no_model_fallback_project(Some("/Users/example/me/proj"), home.clone()).as_deref(),
            Some("/Users/example/me/proj")
        );
        assert_eq!(
            no_model_fallback_project(Some("main_page"), home.clone()).as_deref(),
            Some("/Users/example/me")
        );
        assert_eq!(
            no_model_fallback_project(Some("  "), home.clone()).as_deref(),
            Some("/Users/example/me")
        );
        assert_eq!(
            no_model_fallback_project(None, home).as_deref(),
            Some("/Users/example/me")
        );
        assert_eq!(no_model_fallback_project(None, None), None);
    }

    use super::*;

    #[test]
    fn sdk_catalog_displays_and_pins_actual_versions() {
        let models = json!([
            {"value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default"},
            {"value":"opus","resolvedModel":"claude-opus-5-5","displayName":"Opus",
             "supportedEffortLevels":["low","medium","high","xhigh","max"]},
            {"value":"claude-fable-5-1[1m]","resolvedModel":"claude-fable-5-1","displayName":"Fable",
             "supportedEffortLevels":["medium","high"]},
            {"value":"bad","resolvedModel":"claude-opus-5-5; rm -rf ~","displayName":"Bad"}
        ]);
        let entries = catalog_entries_from_sdk(&models);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["id"], "claude:claude-opus-5-5");
        assert_eq!(entries[0]["displayName"], "Claude Opus 5.5");
        assert_eq!(entries[0]["legacyModelID"], "claude:opus");
        assert_eq!(entries[1]["displayName"], "Claude Fable 5.1");
        assert_eq!(entries[1]["legacyModelID"], "claude:fable");
        assert!(entries[1]["description"].as_str().unwrap().contains("额外"));
        assert_eq!(
            entries[1]["supportedReasoningEfforts"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            resolve_catalog_selection("claude:opus", Some("max"), &entries).unwrap(),
            (
                "claude-opus-5-5".to_string(),
                "max".to_string(),
                "Claude Opus 5.5".to_string()
            )
        );
        assert!(resolve_catalog_selection("claude:unknown-model", Some("max"), &entries).is_err());
        assert!(
            resolve_catalog_selection("claude:claude-fable-5-1", Some("max"), &entries).is_err()
        );
    }

    #[test]
    fn unresolved_alias_is_honestly_marked_and_no_effort_is_supported() {
        let entries = catalog_entries_from_sdk(&json!([{
            "value":"haiku","resolvedModel":null,"displayName":"Haiku","supportsEffort":false,
            "supportedEffortLevels":["max"]
        }]));
        assert_eq!(entries[0]["id"], "claude:haiku");
        assert!(entries[0]["displayName"]
            .as_str()
            .unwrap()
            .contains("版本未确认"));
        assert_eq!(entries[0]["defaultReasoningEffort"], "default");
        assert_eq!(
            entries[0]["supportedReasoningEfforts"][0]["reasoningEffort"],
            "default"
        );
    }

    #[test]
    fn a_single_default_model_is_still_selectable() {
        let entries = catalog_entries_from_sdk(&json!([{
            "value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default",
            "supportedEffortLevels":["medium","max"]
        }]));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["displayName"], "Claude Opus 5.5");
        assert_eq!(entries[0]["legacyModelID"], "claude:opus");
    }

    #[test]
    fn catalog_entries_match_codex_model_shape() {
        let entries = fallback_catalog_entries();
        assert_eq!(entries.len(), MODELS.len());
        let opus = &entries[0];
        assert_eq!(opus["id"], "claude:opus");
        assert_eq!(opus["displayName"], "Claude Opus（版本未确认）");
        assert_eq!(opus["hidden"], false);
        assert_eq!(opus["defaultReasoningEffort"], DEFAULT_EFFORT);
        assert_eq!(
            opus["supportedReasoningEfforts"][4]["reasoningEffort"],
            "max"
        );
        assert!(entries
            .iter()
            .all(|entry| is_claude_model_id(entry["id"].as_str().unwrap())));
    }

    #[test]
    fn resolve_accepts_exact_versions_and_rejects_unsafe_input() {
        assert_eq!(
            resolve("claude:claude-opus-5-5", Some("max")).unwrap(),
            ("claude-opus-5-5".to_string(), "max".to_string())
        );
        assert_eq!(resolve("claude:sonnet", None).unwrap().1, DEFAULT_EFFORT);
        assert_eq!(
            resolve("claude:haiku", Some("default")).unwrap().1,
            "default"
        );
        assert!(resolve("claude:opus; rm -rf ~", None).is_err());
        assert!(resolve("claude:opus", Some("ultra")).is_err());
        assert!(!is_claude_model_id("gpt-6.1-terra"));
    }

    #[test]
    fn launch_command_quotes_path_and_prompt() {
        assert_eq!(
            launch_command(
                "claude-pro2",
                "/Users/example/it's here",
                "zhi",
                "opus",
                "max"
            )
            .unwrap(),
            "cd '/Users/example/it'\\''s here' && claude-pro2 --model 'opus' --effort max \
             --permission-mode bypassPermissions 'zhi'"
        );
        assert!(
            launch_command("claude", "/tmp", "修 bug\n'; rm -rf ~", "opus", "high")
                .unwrap()
                .ends_with("'修 bug '\\''; rm -rf ~'")
        );
        assert!(launch_command("claude\n", "/tmp", "zhi", "opus", "max").is_err());
        assert!(launch_command("claude", "/tmp\nx", "zhi", "opus", "max").is_err());
        assert!(launch_command("claude", "/tmp", " \n ", "opus", "max").is_err());
        assert!(
            !launch_command("claude", "/tmp", "zhi", "claude-haiku-4-5", "default")
                .unwrap()
                .contains("--effort")
        );
        assert!(launch_command("claude", "/tmp", "zhi", "opus;touch /tmp/x", "max").is_err());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn discovery_reads_a_control_response_without_sending_a_prompt() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fake-claude");
        std::fs::write(&script, r##"#!/bin/sh
IFS= read -r request
case "$request" in *'"subtype":"initialize"'*) ;; *) exit 3;; esac
case " $* " in *' --tools= '* ) ;; *) exit 4;; esac
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"iterate-model-catalog","response":{"models":[{"value":"opus","resolvedModel":"claude-opus-5-5","displayName":"Opus","supportedEffortLevels":["medium","max"]}]}}}'
"##).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let models = query_sdk_models(&shell_single_quote(script.to_str().unwrap()))
            .await
            .unwrap();
        let entries = catalog_entries_from_sdk(&models);
        assert_eq!(entries[0]["displayName"], "Claude Opus 5.5");
        assert_eq!(entries[0]["id"], "claude:claude-opus-5-5");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires an explicitly selected local Claude Code launcher"]
    async fn live_discovery_reports_resolved_model_ids() {
        let launcher = std::env::var("ITERATE_TEST_CLAUDE_LAUNCHER")
            .expect("set ITERATE_TEST_CLAUDE_LAUNCHER to the launcher being checked");
        let models = query_sdk_models(&launcher).await.unwrap();
        let entries = catalog_entries_from_sdk(&models);
        assert!(!entries.is_empty(), "Claude Code returned no usable models");
        for entry in entries {
            println!("{}: {}", entry["displayName"], entry["id"]);
        }
    }
}
