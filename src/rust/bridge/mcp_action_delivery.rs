use super::room_submit::{RoomDeliveryAttempt, RoomDeliveryResult};
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Debug, Deserialize)]
struct ServeResponseRoute {
    #[serde(default)]
    request_id: Option<String>,
    project_path: String,
    response_file: String,
    created_at: i64,
}

#[derive(Debug)]
struct ServeResponseRouteCheck {
    attempt: RoomDeliveryAttempt,
    route_file: Option<PathBuf>,
    response_file: Option<PathBuf>,
}

fn sanitize_request_id_for_filename(request_id: &str) -> String {
    request_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn serve_response_route_file(request_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "iterate_response_route_{}.json",
        sanitize_request_id_for_filename(request_id)
    ))
}

fn inspect_serve_response_route(
    request_id: Option<&str>,
    project_path: &str,
    preflight: bool,
    route_ttl_secs: i64,
    debug_log: &dyn Fn(&str),
) -> ServeResponseRouteCheck {
    let make_attempt = |project_path: &str,
                        request_id: Option<&str>,
                        delivered: bool,
                        reason: Option<&str>,
                        route_file: Option<&PathBuf>,
                        response_file: Option<&PathBuf>,
                        response_file_exists: Option<bool>,
                        route_age_secs: Option<i64>| {
        if preflight {
            RoomDeliveryAttempt::serve_response_file_preflight(
                project_path,
                request_id,
                delivered,
                reason,
                route_file,
                response_file,
                response_file_exists,
                route_age_secs,
                route_ttl_secs,
            )
        } else {
            RoomDeliveryAttempt::serve_response_file(
                project_path,
                request_id,
                delivered,
                reason,
                route_file,
                response_file,
                response_file_exists,
                route_age_secs,
                route_ttl_secs,
            )
        }
    };

    let Some(request_id) = request_id else {
        return ServeResponseRouteCheck {
            attempt: make_attempt(
                project_path,
                None,
                false,
                Some("response_route_request_id_missing"),
                None,
                None,
                None,
                None,
            ),
            route_file: None,
            response_file: None,
        };
    };

    let route_file = serve_response_route_file(request_id);
    let route = match read_serve_response_route_with_retry(&route_file) {
        Ok(route) => route,
        Err(reason) => {
            debug_log(&format!(
                "[Bridge] serve response route read failed: request_id={}, route_file={}, reason={}",
                request_id,
                route_file.display(),
                reason
            ));
            return ServeResponseRouteCheck {
                attempt: make_attempt(
                    project_path,
                    Some(request_id),
                    false,
                    Some(reason),
                    Some(&route_file),
                    None,
                    None,
                    None,
                ),
                route_file: Some(route_file),
                response_file: None,
            };
        }
    };

    if route.request_id.as_deref().is_some_and(|value| value != request_id) {
        return ServeResponseRouteCheck {
            attempt: make_attempt(project_path, Some(request_id), false,
                Some("response_route_request_id_mismatch"), Some(&route_file), None, None, None),
            route_file: Some(route_file),
            response_file: None,
        };
    }

    let route_age_secs = chrono::Utc::now()
        .timestamp()
        .saturating_sub(route.created_at);
    let response_file = PathBuf::from(&route.response_file);
    let response_file_exists = response_file.exists();

    if route_age_secs > route_ttl_secs {
        debug_log(&format!(
            "[Bridge] serve response route expired: request_id={}, age_secs={}",
            request_id, route_age_secs
        ));
        return ServeResponseRouteCheck {
            attempt: make_attempt(
                project_path,
                Some(request_id),
                false,
                Some("response_route_expired"),
                Some(&route_file),
                Some(&response_file),
                Some(response_file_exists),
                Some(route_age_secs),
            ),
            route_file: Some(route_file),
            response_file: Some(response_file),
        };
    }

    if route.project_path != project_path {
        debug_log(&format!(
            "[Bridge] serve response route project mismatch: request_id={}, route_project={}, action_project={}",
            request_id, route.project_path, project_path
        ));
        return ServeResponseRouteCheck {
            attempt: make_attempt(
                project_path,
                Some(request_id),
                false,
                Some("response_route_project_mismatch"),
                Some(&route_file),
                Some(&response_file),
                Some(response_file_exists),
                Some(route_age_secs),
            ),
            route_file: Some(route_file),
            response_file: Some(response_file),
        };
    }

    if !response_file.starts_with(std::env::temp_dir()) {
        debug_log(&format!(
            "[Bridge] serve response route rejected outside temp dir: request_id={}, response_file={}",
            request_id,
            response_file.display()
        ));
        return ServeResponseRouteCheck {
            attempt: make_attempt(
                project_path,
                Some(request_id),
                false,
                Some("response_file_outside_temp_dir"),
                Some(&route_file),
                Some(&response_file),
                Some(response_file_exists),
                Some(route_age_secs),
            ),
            route_file: Some(route_file),
            response_file: Some(response_file),
        };
    }

    if response_file_exists {
        return ServeResponseRouteCheck {
            attempt: make_attempt(
                project_path,
                Some(request_id),
                false,
                Some("response_file_already_exists"),
                Some(&route_file),
                Some(&response_file),
                Some(true),
                Some(route_age_secs),
            ),
            route_file: Some(route_file),
            response_file: Some(response_file),
        };
    }

    ServeResponseRouteCheck {
        attempt: make_attempt(
            project_path,
            Some(request_id),
            true,
            None,
            Some(&route_file),
            Some(&response_file),
            Some(response_file_exists),
            Some(route_age_secs),
        ),
        route_file: Some(route_file),
        response_file: Some(response_file),
    }
}

fn read_serve_response_route_with_retry(
    route_file: &Path,
) -> Result<ServeResponseRoute, &'static str> {
    // Renewal updates this existing file in place; a reader may catch it
    // between truncate and write. Missing files still mean a closed route.
    for attempt in 0..6 {
        match std::fs::read_to_string(route_file) {
            Ok(content) => {
                if let Ok(route) = serde_json::from_str::<ServeResponseRoute>(&content) {
                    return Ok(route);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err("response_route_missing");
            }
            Err(_) => {}
        }
        if attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    Err("response_route_invalid")
}

pub(super) fn preflight_android_serve_response_route(
    request_id: &str,
    project_path: &str,
    route_ttl_secs: i64,
    debug_log: &dyn Fn(&str),
) -> RoomDeliveryResult {
    let check = inspect_serve_response_route(
        Some(request_id), project_path, true, route_ttl_secs, debug_log,
    );
    if !check.attempt.delivered {
        return RoomDeliveryResult::rejected(check.attempt);
    }
    let route_matches_id = check.route_file.as_ref()
        .and_then(|path| read_serve_response_route_with_retry(path).ok())
        .and_then(|route| route.request_id)
        .as_deref() == Some(request_id);
    if !route_matches_id {
        let mut attempt = check.attempt;
        attempt.delivered = false;
        attempt.reason = Some("response_route_request_id_missing".to_string());
        return RoomDeliveryResult::rejected(attempt);
    }
    RoomDeliveryResult::delivered(check.attempt)
}

fn cleanup_unusable_serve_response_route(check: &ServeResponseRouteCheck) {
    if matches!(
        check.attempt.reason.as_deref(),
        Some("response_file_outside_temp_dir")
    ) {
        if let Some(route_file) = &check.route_file {
            let _ = std::fs::remove_file(route_file);
        }
    }
}

pub(super) fn try_write_serve_response_file(
    request_id: Option<&str>,
    project_path: &str,
    response_str: &str,
    route_ttl_secs: i64,
    debug_log: &dyn Fn(&str),
) -> RoomDeliveryAttempt {
    let check =
        inspect_serve_response_route(request_id, project_path, false, route_ttl_secs, debug_log);
    if !check.attempt.delivered {
        cleanup_unusable_serve_response_route(&check);
        return check.attempt;
    }

    let Some(route_file) = check.route_file.as_ref() else {
        return check.attempt;
    };
    let Some(response_file) = check.response_file.as_ref() else {
        return RoomDeliveryAttempt::serve_response_file(
            project_path,
            request_id,
            false,
            Some("response_route_invalid"),
            None,
            None,
            None,
            None,
            route_ttl_secs,
        );
    };

    // Both the local GUI and Bridge claim before recording any side effects.
    let mut claim = match crate::delivery::ResponseFileClaim::acquire(response_file) {
        Ok(claim) => claim,
        Err(error) => {
            let mut attempt = check.attempt;
            attempt.delivered = false;
            attempt.reason = Some(if error.kind() == std::io::ErrorKind::AlreadyExists {
                "response_already_claimed"
            } else {
                "response_claim_failed"
            }.to_string());
            return attempt;
        }
    };
    let marker_file = response_file.with_extension("bridge-response.sha256");
    let response_hash = hex::encode(ring::digest::digest(
        &ring::digest::SHA256,
        response_str.as_bytes(),
    ));
    let write_result = std::fs::write(&marker_file, &response_hash)
        .and_then(|_| crate::delivery::publish_claimed_response_file(response_file, response_str));
    match write_result {
        Ok(()) => {
            claim.commit();
            let _ = std::fs::remove_file(route_file);
            debug_log(&format!(
                "[Bridge] serve response file written: request_id={:?}, response_file={}",
                request_id,
                response_file.display()
            ));
            let mut attempt = check.attempt;
            attempt.delivered = true;
            attempt.reason = None;
            attempt.response_file_exists = Some(true);
            attempt
        }
        Err(err) => {
            let _ = std::fs::remove_file(&marker_file);
            debug_log(&format!(
                "[Bridge] serve response file write failed: request_id={:?}, response_file={}, err={}",
                request_id,
                response_file.display(),
                err
            ));
            let mut attempt = check.attempt;
            attempt.delivered = false;
            attempt.reason = Some(if err.kind() == std::io::ErrorKind::AlreadyExists {
                "response_file_already_exists"
            } else {
                "response_file_write_failed"
            }.to_string());
            attempt.response_file_exists = Some(response_file.exists());
            attempt
        }
    }
}

pub(super) fn preflight_mcp_action_delivery(
    app_handle: Option<&AppHandle>,
    project_path: &str,
    request_id: Option<&str>,
    route_ttl_secs: i64,
    debug_log: &dyn Fn(&str),
) -> RoomDeliveryResult {
    let mut attempts = Vec::new();
    let lookup_key = request_id.unwrap_or(project_path).to_string();

    if let Some(app_handle) = app_handle {
        if let Some(state) = app_handle.try_state::<crate::config::AppState>() {
            match state.response_channels.lock() {
                Ok(channels) => {
                    let available_count = channels.len();
                    if channels.contains_key(&lookup_key) {
                        attempts.push(RoomDeliveryAttempt::response_channel_preflight(
                            project_path,
                            request_id,
                            &lookup_key,
                            true,
                            None,
                            available_count,
                        ));
                        return RoomDeliveryResult::from_attempts(attempts);
                    }
                    attempts.push(RoomDeliveryAttempt::response_channel_preflight(
                        project_path,
                        request_id,
                        &lookup_key,
                        false,
                        Some("response_channel_missing"),
                        available_count,
                    ));
                }
                Err(_) => attempts.push(RoomDeliveryAttempt::response_channel_preflight(
                    project_path,
                    request_id,
                    &lookup_key,
                    false,
                    Some("response_channels_lock_failed"),
                    0,
                )),
            }
        } else {
            attempts.push(RoomDeliveryAttempt::internal(
                "rust_direct_preflight",
                project_path,
                request_id,
                false,
                Some("app_state_unavailable"),
            ));
        }
    }

    let serve_check =
        inspect_serve_response_route(request_id, project_path, true, route_ttl_secs, debug_log);
    if !serve_check.attempt.delivered {
        cleanup_unusable_serve_response_route(&serve_check);
    }
    attempts.push(serve_check.attempt);
    RoomDeliveryResult::from_attempts(attempts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_reader_recovers_from_renewal_truncate_window() {
        let request_id = format!("serve-renew-read-{}", uuid::Uuid::new_v4());
        let route_file = serve_response_route_file(&request_id);
        let project_path = std::env::temp_dir().display().to_string();
        let response_file = std::env::temp_dir().join(format!("iterate_response_{request_id}.json"));
        let route = serde_json::json!({
            "request_id": request_id,
            "project_path": project_path,
            "response_file": response_file,
            "created_at": chrono::Utc::now().timestamp(),
        }).to_string();
        std::fs::write(&route_file, &route).unwrap();
        let route_for_writer = route_file.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
        let writer = std::thread::spawn(move || {
            let mut file = std::fs::OpenOptions::new().write(true).open(&route_for_writer).unwrap();
            file.set_len(0).unwrap();
            ready_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(8));
            file.write_all(route.as_bytes()).unwrap();
        });
        ready_rx.recv().unwrap();
        let read = inspect_serve_response_route(
            Some(&request_id), &project_path, true, 30 * 60, &|_| {},
        );
        assert!(read.attempt.delivered);
        writer.join().unwrap();
        std::fs::remove_file(route_file).unwrap();
    }

    #[test]
    fn invalid_route_is_rejected_without_deleting_it() {
        let request_id = format!("serve-invalid-read-{}", uuid::Uuid::new_v4());
        let route_file = serve_response_route_file(&request_id);
        std::fs::write(&route_file, "{").unwrap();
        let result = try_write_serve_response_file(
            Some(&request_id),
            &std::env::temp_dir().display().to_string(),
            "reply",
            30 * 60,
            &|_| {},
        );
        assert!(!result.delivered);
        assert_eq!(result.reason.as_deref(), Some("response_route_invalid"));
        assert_eq!(std::fs::read_to_string(&route_file).unwrap(), "{");
        std::fs::remove_file(route_file).unwrap();
    }
}
