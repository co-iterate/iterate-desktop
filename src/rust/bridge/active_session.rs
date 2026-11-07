use super::mcp_state_extract::extract_timeline_route_id_from_mcp_state;
use super::route_part::{normalize_route_part, project_paths_match};
use super::time_parse::parse_rfc3339;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const ACTIVE_SESSION_RETENTION_HOURS: i64 = 6;
const ACTIVE_SESSION_MAX_ENTRIES: usize = 12;
const REGISTERED_MCP_PORT_REQUEST_ID_PREFIX: &str = "registered-port-";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ActiveSessionEntry {
    pub(super) request_id: String,
    pub(super) project_path: String,
    pub(super) project_name: String,
    pub(super) title: String,
    pub(super) payload: serde_json::Value,
    pub(super) last_active_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedMcpActionTimelineRoute {
    pub(super) route_id: String,
    pub(super) source: &'static str,
}

fn active_session_cutoff() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::hours(ACTIVE_SESSION_RETENTION_HOURS)
}

fn should_keep_active_session(entry: &ActiveSessionEntry) -> bool {
    parse_rfc3339(&entry.last_active_at)
        .map(|ts| ts >= active_session_cutoff())
        .unwrap_or(false)
}

pub(super) fn is_registered_mcp_port_request_id(value: &str) -> bool {
    value
        .strip_prefix(REGISTERED_MCP_PORT_REQUEST_ID_PREFIX)
        .and_then(|port| port.parse::<u16>().ok())
        .is_some()
}

pub(super) fn prune_active_session_registry(registry: &mut HashMap<String, ActiveSessionEntry>) {
    registry.retain(|_, entry| should_keep_active_session(entry));

    if registry.len() <= ACTIVE_SESSION_MAX_ENTRIES {
        return;
    }

    let mut ordered = registry
        .iter()
        .map(|(key, entry)| {
            (
                key.clone(),
                parse_rfc3339(&entry.last_active_at).unwrap_or_else(|| {
                    chrono::DateTime::<chrono::Utc>::from(std::time::UNIX_EPOCH)
                }),
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.1.cmp(&a.1));
    ordered.truncate(ACTIVE_SESSION_MAX_ENTRIES);

    let keep = ordered
        .into_iter()
        .map(|(key, _)| key)
        .collect::<HashSet<_>>();
    registry.retain(|key, _| keep.contains(key));
}

pub(super) fn remove_active_session_entry(
    registry: &mut HashMap<String, ActiveSessionEntry>,
    request_id: &str,
) -> bool {
    registry.remove(request_id).is_some()
}

pub(super) fn lookup_active_session_entry(
    registry: &HashMap<String, ActiveSessionEntry>,
    request_id: Option<&str>,
    project_path: Option<&str>,
    fallback_route: Option<&str>,
) -> Option<ActiveSessionEntry> {
    if let Some(rid) = normalize_route_part(request_id) {
        let should_fallback_to_project =
            is_registered_mcp_port_request_id(&rid) && normalize_route_part(project_path).is_some();
        if !should_fallback_to_project {
            if let Some(entry) = registry.get(&rid) {
                return Some(entry.clone());
            }
            if let Some(entry) = registry.values().find(|entry| entry.request_id == rid) {
                return Some(entry.clone());
            }
            return None;
        }
    }

    if let Some(path) = project_path {
        if let Some(entry) = registry
            .values()
            .filter(|entry| project_paths_match(&entry.project_path, path))
            .max_by_key(|entry| {
                parse_rfc3339(&entry.last_active_at)
                    .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from(std::time::UNIX_EPOCH))
            })
        {
            return Some(entry.clone());
        }
    }

    if let Some(route_key) = fallback_route {
        if let Some(entry) = registry.get(route_key) {
            return Some(entry.clone());
        }
        if let Some(entry) = registry
            .values()
            .find(|entry| entry.request_id == route_key)
        {
            return Some(entry.clone());
        }
        if let Some(entry) = registry
            .values()
            .filter(|entry| project_paths_match(&entry.project_path, route_key))
            .max_by_key(|entry| {
                parse_rfc3339(&entry.last_active_at)
                    .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from(std::time::UNIX_EPOCH))
            })
        {
            return Some(entry.clone());
        }
    }

    None
}

#[cfg(test)]
pub(super) fn lookup_active_session_payload(
    registry: &HashMap<String, ActiveSessionEntry>,
    request_id: Option<&str>,
    project_path: Option<&str>,
    fallback_route: Option<&str>,
) -> Option<serde_json::Value> {
    lookup_active_session_entry(registry, request_id, project_path, fallback_route)
        .map(|entry| entry.payload)
}

fn active_session_entry_for_route_key<'a>(
    registry: &'a HashMap<String, ActiveSessionEntry>,
    route_key: Option<&str>,
) -> Option<&'a ActiveSessionEntry> {
    let route_key = normalize_route_part(route_key)?;
    registry.get(&route_key).or_else(|| {
        registry.values().find(|entry| {
            normalize_route_part(Some(&entry.request_id)).as_deref() == Some(route_key.as_str())
        })
    })
}

fn active_session_entry_matches_project(
    entry: &ActiveSessionEntry,
    project_path: Option<&str>,
) -> bool {
    let Some(project_key) = normalize_route_part(project_path) else {
        return true;
    };
    normalize_route_part(Some(&entry.project_path)).as_deref() == Some(project_key.as_str())
}

fn route_id_from_active_session_entry(entry: &ActiveSessionEntry) -> Option<String> {
    extract_timeline_route_id_from_mcp_state(&entry.payload)
        .or_else(|| normalize_route_part(Some(&entry.request_id)))
}

fn route_from_active_session_entry(
    entry: &ActiveSessionEntry,
    project_path: Option<&str>,
    source: &'static str,
) -> Option<ResolvedMcpActionTimelineRoute> {
    if !active_session_entry_matches_project(entry, project_path) {
        return None;
    }

    route_id_from_active_session_entry(entry)
        .map(|route_id| ResolvedMcpActionTimelineRoute { route_id, source })
}

pub(super) fn resolve_mcp_action_timeline_route_id(
    payload: &serde_json::Value,
    request_id: Option<&str>,
    project_path: Option<&str>,
    fallback_route: Option<&str>,
    registry: &HashMap<String, ActiveSessionEntry>,
) -> Option<ResolvedMcpActionTimelineRoute> {
    if let Some(route_id) =
        super::mcp_state_extract::extract_timeline_route_id_from_mcp_action(payload)
    {
        return Some(ResolvedMcpActionTimelineRoute {
            route_id,
            source: "explicit_action_route",
        });
    }

    if let Some(entry) =
        active_session_entry_for_route_key(registry, request_id).and_then(|entry| {
            route_from_active_session_entry(entry, project_path, "active_session_request_id")
        })
    {
        return Some(entry);
    }

    if let Some(entry) =
        active_session_entry_for_route_key(registry, fallback_route).and_then(|entry| {
            route_from_active_session_entry(entry, project_path, "active_session_fallback_route")
        })
    {
        return Some(entry);
    }

    let project_key = normalize_route_part(project_path)?;
    let mut project_routes = registry
        .values()
        .filter(|entry| should_keep_active_session(entry))
        .filter(|entry| {
            normalize_route_part(Some(&entry.project_path)).as_deref() == Some(project_key.as_str())
        })
        .filter_map(route_id_from_active_session_entry)
        .collect::<Vec<_>>();
    project_routes.sort();
    project_routes.dedup();

    if project_routes.len() == 1 {
        return Some(ResolvedMcpActionTimelineRoute {
            route_id: project_routes.remove(0),
            source: "active_session_unique_project_route",
        });
    }

    None
}

pub(super) fn update_active_session_registry(
    registry: &mut HashMap<String, ActiveSessionEntry>,
    payload: &serde_json::Value,
) {
    let request = payload.get("request");
    let request_id = request
        .and_then(|r| r.get("id"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|rid| !rid.is_empty());
    let project_path = request
        .and_then(|r| r.get("project_path"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != "." && *path != "Unknown");
    let message = request
        .and_then(|r| r.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    let (Some(request_id), Some(project_path)) = (request_id, project_path) else {
        log::info!("[Bridge] active-session 忽略：缺少 request_id/project_path");
        return;
    };

    if message.is_empty() || is_inactive_session_message(message) {
        let removed = remove_active_session_entry(registry, request_id);
        log::info!(
            "[Bridge] active-session 跳过：request_id={}, project_path={}, reason={}, removed={}",
            request_id,
            project_path,
            if message.is_empty() {
                "empty_message"
            } else {
                "inactive_message"
            },
            removed
        );
        prune_active_session_registry(registry);
        return;
    }

    let project_name = project_path
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(project_path)
        .to_string();
    let title = message.chars().take(50).collect::<String>();
    let entry = ActiveSessionEntry {
        request_id: request_id.to_string(),
        project_path: project_path.to_string(),
        project_name,
        title,
        payload: payload.clone(),
        last_active_at: chrono::Utc::now().to_rfc3339(),
    };
    registry.insert(request_id.to_string(), entry);
    prune_active_session_registry(registry);
    log::info!(
        "[Bridge] active-session upsert: request_id={}, project_path={}, registry_size={}",
        request_id,
        project_path,
        registry.len()
    );
}

pub(super) fn is_inactive_session_message(message: &str) -> bool {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return true;
    }

    let lower = trimmed.to_lowercase();
    let inactive_markers = [
        "已 push 完成",
        "对话已结束",
        "任务已结束",
        "最终状态",
        "已停止分析",
    ];

    inactive_markers.iter().any(|marker| lower.contains(marker))
}

fn active_session_response_value(
    entry: Option<&ActiveSessionEntry>,
    request_id: &str,
    project_path: &str,
    title_fallback: Option<&str>,
    last_active_at: &str,
    source: &str,
    port: Option<u16>,
) -> serde_json::Value {
    let title = entry
        .and_then(|entry| {
            let title = entry.title.trim();
            if title.is_empty() {
                None
            } else {
                Some(entry.title.clone())
            }
        })
        .or_else(|| title_fallback.map(ToOwned::to_owned))
        .unwrap_or_default();
    let project_name = entry
        .map(|entry| entry.project_name.clone())
        .unwrap_or_else(|| project_name_from_path(project_path));
    let mut value = serde_json::json!({
        "request_id": request_id,
        "project_path": project_path,
        "project_name": project_name,
        "title": title,
        "last_active_at": last_active_at,
        "source": source,
    });
    if let Some(port) = port {
        value["port"] = serde_json::json!(port);
    }
    value
}

fn project_name_from_path(project_path: &str) -> String {
    std::path::Path::new(project_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(project_path)
        .to_string()
}

pub(super) fn build_active_session_summaries(
    registry: &HashMap<String, ActiveSessionEntry>,
    instances: Vec<crate::ui::window_registry::WindowInstance>,
) -> Vec<serde_json::Value> {
    build_active_session_summaries_with_focus(registry, instances, &HashMap::new())
}

pub(super) fn build_active_session_summaries_with_focus(
    registry: &HashMap<String, ActiveSessionEntry>,
    instances: Vec<crate::ui::window_registry::WindowInstance>,
    last_focused_at_by_pid: &HashMap<u32, String>,
) -> Vec<serde_json::Value> {
    struct RankedSession {
        value: serde_json::Value,
        sort_at: chrono::DateTime<chrono::Utc>,
        port: u64,
        project_path: String,
    }

    let epoch = || chrono::DateTime::<chrono::Utc>::from(std::time::UNIX_EPOCH);
    let mut seen_request_ids = HashSet::new();
    let mut sessions = instances
        .into_iter()
        .filter_map(|instance| {
            let instance_request_id = instance.request_id?.trim().to_string();
            if instance_request_id.is_empty() {
                return None;
            }
            if !seen_request_ids.insert(instance_request_id.clone()) {
                return None;
            }

            let entry = registry
                .get(&instance_request_id)
                .filter(|entry| should_keep_active_session(entry));
            let title_fallback = instance
                .request_title
                .as_deref()
                .or(Some(instance.window_title.as_str()));
            let window_registered_at = instance.registered_at.clone();
            let last_focused_at = last_focused_at_by_pid
                .get(&instance.pid)
                .unwrap_or(&instance.registered_at)
                .clone();
            let sort_at = parse_rfc3339(&window_registered_at).unwrap_or_else(epoch);
            let project_path = instance.project_path.clone();
            let port = instance.port.map(u64::from).unwrap_or(u64::MAX);

            Some(RankedSession {
                value: active_session_response_value(
                    entry,
                    &instance_request_id,
                    &project_path,
                    title_fallback,
                    &last_focused_at,
                    "window_registry",
                    instance.port,
                ),
                sort_at,
                port,
                project_path,
            })
        })
        .collect::<Vec<_>>();

    sessions.sort_by(|a, b| {
        // Registry entries can be re-touched by cached mcp_state broadcasts for older
        // requests; live-window ordering must follow the actual window binding age.
        b.sort_at
            .cmp(&a.sort_at)
            .then_with(|| a.port.cmp(&b.port))
            .then_with(|| a.project_path.cmp(&b.project_path))
    });

    sessions.into_iter().map(|session| session.value).collect()
}
