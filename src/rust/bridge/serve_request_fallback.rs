use crate::ui::window_registry::{WindowInstance, WindowRegistry};
use serde::Deserialize;
use std::fs::{File, OpenOptions};
use std::io::{Read, Take};
use std::path::{Path, PathBuf};

const SERVE_REQUEST_MAX_BYTES: u64 = 1024 * 1024;
const SERVE_ROUTE_MAX_BYTES: u64 = 16 * 1024;
const CLOCK_SKEW_ALLOWANCE_SECS: i64 = 5 * 60;

// Only separator spelling in ordinary Windows drive-absolute paths may differ.
// Keep case, components, prefixes and relative paths exact; Unix '/' and '\\'
// are different characters. This comparison does not resolve filesystem aliases.
pub(super) fn local_project_paths_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        let drive_absolute = |path: &str| {
            let bytes = path.as_bytes();
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\')
        };
        let separator = |byte| if byte == b'/' { b'\\' } else { byte };
        return drive_absolute(left)
            && drive_absolute(right)
            && left.bytes().map(separator).eq(right.bytes().map(separator));
    }
    #[cfg(not(target_os = "windows"))]
    false
}

#[derive(Debug)]
pub(super) struct ServeRequestFallback {
    pub(super) payload: serde_json::Value,
    pub(super) age_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServeRequestFallbackMiss {
    InvalidRoute,
    InsecureTempDirectory,
    NoLiveWindowBinding,
    InvalidRouteFile,
    InvalidRequestFile,
    StaleRoute,
    RouteMismatch,
    RequestMismatch,
}

impl ServeRequestFallbackMiss {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRoute => "invalid_route",
            Self::InsecureTempDirectory => "insecure_temp_directory",
            Self::NoLiveWindowBinding => "no_live_window_binding",
            Self::InvalidRouteFile => "invalid_route_file",
            Self::InvalidRequestFile => "invalid_request_file",
            Self::StaleRoute => "stale_route",
            Self::RouteMismatch => "route_mismatch",
            Self::RequestMismatch => "request_mismatch",
        }
    }
}

#[derive(Debug, Deserialize)]
struct ServeResponseRoute {
    request_id: String,
    project_path: String,
    response_file: PathBuf,
    created_at: i64,
    #[serde(default)]
    original_created_at: Option<i64>,
}

pub(super) fn load_live_serve_request_fallback(
    request_id: &str,
    project_path: &str,
) -> Result<ServeRequestFallback, ServeRequestFallbackMiss> {
    let mut registry = WindowRegistry::load();
    let instances = registry.get_all_instances();
    load_live_serve_request_fallback_from_dir(
        &std::env::temp_dir(),
        request_id,
        project_path,
        &instances,
        chrono::Utc::now().timestamp(),
    )
}

fn load_live_serve_request_fallback_from_dir(
    temp_dir: &Path,
    request_id: &str,
    project_path: &str,
    instances: &[WindowInstance],
    now_unix_secs: i64,
) -> Result<ServeRequestFallback, ServeRequestFallbackMiss> {
    let request_id = request_id.trim();
    let project_path = project_path.trim();
    if !is_serve_request_id(request_id) || project_path.is_empty() || project_path == "." {
        return Err(ServeRequestFallbackMiss::InvalidRoute);
    }
    validate_secure_temp_directory(temp_dir)?;

    let live_binding = instances.iter().find(|instance| {
        instance.request_id.as_deref().map(str::trim) == Some(request_id)
            && local_project_paths_match(instance.project_path.trim(), project_path)
    });
    let Some(live_binding) = live_binding else {
        return Err(ServeRequestFallbackMiss::NoLiveWindowBinding);
    };

    let route_path = temp_dir.join(format!("iterate_response_route_{request_id}.json"));
    let route_bytes = read_guarded_file(
        &route_path,
        SERVE_ROUTE_MAX_BYTES,
        ServeRequestFallbackMiss::InvalidRouteFile,
    )?;
    let route: ServeResponseRoute = serde_json::from_slice(&route_bytes)
        .map_err(|_| ServeRequestFallbackMiss::InvalidRouteFile)?;

    let route_age_secs = now_unix_secs.saturating_sub(route.created_at);
    let original_created_at = route.original_created_at.unwrap_or(route.created_at);
    if route.created_at > now_unix_secs.saturating_add(CLOCK_SKEW_ALLOWANCE_SECS) {
        return Err(ServeRequestFallbackMiss::StaleRoute);
    }
    if original_created_at > route.created_at {
        return Err(ServeRequestFallbackMiss::StaleRoute);
    }

    let expected_response_path = temp_dir.join(format!("iterate_response_{request_id}.json"));
    if route.request_id.trim() != request_id
        || !local_project_paths_match(route.project_path.trim(), project_path)
        || route.response_file != expected_response_path
    {
        return Err(ServeRequestFallbackMiss::RouteMismatch);
    }

    let registered_at = chrono::DateTime::parse_from_rfc3339(&live_binding.registered_at)
        .map_err(|_| ServeRequestFallbackMiss::NoLiveWindowBinding)?
        .timestamp();
    if registered_at.saturating_sub(original_created_at).abs() > CLOCK_SKEW_ALLOWANCE_SECS {
        return Err(ServeRequestFallbackMiss::NoLiveWindowBinding);
    }

    let request_path = temp_dir.join(format!("iterate_request_{request_id}.json"));
    let request_bytes = read_guarded_file(
        &request_path,
        SERVE_REQUEST_MAX_BYTES,
        ServeRequestFallbackMiss::InvalidRequestFile,
    )?;
    let request: serde_json::Value = serde_json::from_slice(&request_bytes)
        .map_err(|_| ServeRequestFallbackMiss::InvalidRequestFile)?;
    let request_matches = request.get("id").and_then(|value| value.as_str()) == Some(request_id)
        && request
            .get("project_path")
            .and_then(|value| value.as_str())
            .is_some_and(|path| local_project_paths_match(path.trim(), project_path))
        && request
            .get("message")
            .and_then(|value| value.as_str())
            .is_some_and(|message| !message.trim().is_empty());
    if !request_matches {
        return Err(ServeRequestFallbackMiss::RequestMismatch);
    }

    Ok(ServeRequestFallback {
        payload: serde_json::json!({
            "request": request,
            "showMcpPopup": true,
            "timelineNodes": [],
        }),
        age_ms: route_age_secs.max(0).saturating_mul(1000),
    })
}

fn is_serve_request_id(request_id: &str) -> bool {
    let Some(suffix) = request_id.strip_prefix("serve-") else {
        return false;
    };
    if !suffix.is_empty() && suffix.len() <= 20 && suffix.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    let Some((timestamp, uuid)) = suffix.split_once('-') else {
        return false;
    };
    timestamp.len() == 13
        && timestamp.bytes().all(|b| b.is_ascii_digit())
        && uuid::Uuid::parse_str(uuid).is_ok()
}

fn validate_secure_temp_directory(temp_dir: &Path) -> Result<(), ServeRequestFallbackMiss> {
    let metadata = std::fs::symlink_metadata(temp_dir)
        .map_err(|_| ServeRequestFallbackMiss::InsecureTempDirectory)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(ServeRequestFallbackMiss::InsecureTempDirectory);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let effective_uid = unsafe { libc::geteuid() };
        let mode = metadata.mode();
        let private_user_directory = metadata.uid() == effective_uid && mode & 0o077 == 0;
        let sticky_shared_directory = mode & 0o1000 != 0 && mode & 0o002 != 0;
        if !private_user_directory && !sticky_shared_directory {
            return Err(ServeRequestFallbackMiss::InsecureTempDirectory);
        }
    }

    Ok(())
}

fn read_guarded_file(
    path: &Path,
    max_bytes: u64,
    miss: ServeRequestFallbackMiss,
) -> Result<Vec<u8>, ServeRequestFallbackMiss> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| miss)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > max_bytes
    {
        return Err(miss);
    }

    let file = open_read_only_no_follow(path).map_err(|_| miss)?;
    let opened_metadata = file.metadata().map_err(|_| miss)?;
    if !opened_metadata.is_file() || opened_metadata.len() > max_bytes {
        return Err(miss);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened_metadata.uid() != unsafe { libc::geteuid() } {
            return Err(miss);
        }
    }

    read_bounded(file.take(max_bytes.saturating_add(1)), max_bytes).map_err(|_| miss)
}

fn open_read_only_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn read_bounded(mut reader: Take<File>, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "serve request file exceeds size limit",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const NOW: i64 = 1_786_277_363;

    fn private_temp_dir() -> tempfile::TempDir {
        let temp_dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(temp_dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        temp_dir
    }

    fn window_instance(request_id: &str, project_path: &str) -> WindowInstance {
        WindowInstance {
            pid: std::process::id(),
            project_path: project_path.to_string(),
            window_title: format!("iterate — {project_path}"),
            registered_at: chrono::DateTime::from_timestamp(NOW, 0)
                .unwrap()
                .to_rfc3339(),
            port: Some(5311),
            request_id: Some(request_id.to_string()),
            request_title: Some("preview".to_string()),
        }
    }

    fn write_fixture(temp_dir: &Path, request_id: &str, project_path: &str, created_at: i64) {
        let response_file = temp_dir.join(format!("iterate_response_{request_id}.json"));
        let route = serde_json::json!({
            "request_id": request_id,
            "project_path": project_path,
            "response_file": response_file,
            "created_at": created_at,
        });
        fs::write(
            temp_dir.join(format!("iterate_response_route_{request_id}.json")),
            serde_json::to_vec(&route).unwrap(),
        )
        .unwrap();
        let request = serde_json::json!({
            "id": request_id,
            "message": "完整请求正文",
            "predefined_options": ["继续"],
            "is_markdown": true,
            "project_path": project_path,
            "loop_active": false,
            "force_popup": false,
        });
        fs::write(
            temp_dir.join(format!("iterate_request_{request_id}.json")),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn loads_request_only_when_live_route_files_agree() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        write_fixture(temp_dir.path(), request_id, project_path, NOW - 2);

        let fallback = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[window_instance(request_id, project_path)],
            NOW,
        )
        .unwrap();

        assert_eq!(fallback.payload["request"]["id"], request_id);
        assert_eq!(fallback.payload["request"]["message"], "完整请求正文");
        assert_eq!(fallback.payload["showMcpPopup"], true);
        assert_eq!(fallback.age_ms, 2_000);
    }

    #[test]
    fn loads_current_serve_request_id_with_uuid() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1790436233047-8218c42c-7a73-452d-a8a2-5a8ec969e763";
        let project_path = "/tmp/iterate-desktop";
        write_fixture(temp_dir.path(), request_id, project_path, NOW);
        let fallback = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[window_instance(request_id, project_path)],
            NOW,
        )
        .unwrap();
        assert_eq!(fallback.payload["request"]["id"], request_id);
    }

    #[test]
    fn project_identity_does_not_merge_case_components_prefixes_or_relative_paths() {
        assert!(local_project_paths_match("E:/Github/iterate-desktop", "E:/Github/iterate-desktop"));
        for other in [
            "E:/Github/other", "E:/Github/iterate-desktop-old", "e:/Github/iterate-desktop",
            "E:/github/iterate-desktop", "E:/Github/./iterate-desktop",
            "E:/Github/child/../iterate-desktop", "E:/Github/iterate-desktop/",
            r"\\?\E:\Github\iterate-desktop", "Github/iterate-desktop",
        ] {
            assert!(!local_project_paths_match("E:/Github/iterate-desktop", other), "{other}");
        }
        assert!(!local_project_paths_match("Github/iterate-desktop", r"Github\iterate-desktop"));
        assert!(!local_project_paths_match("E:Github/iterate-desktop", r"E:Github\iterate-desktop"));
        #[cfg(not(target_os = "windows"))]
        {
            assert!(!local_project_paths_match("E:/Github/iterate-desktop", r"E:\Github\iterate-desktop"));
            assert!(!local_project_paths_match("/tmp/project", r"\tmp\project"));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn loads_windows_route_with_mixed_separators_at_each_identity_check() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1790913331998-e8e52f66-620c-4589-a59f-4bc6fd6820f3";
        let forward = "E:/Github/iterate-desktop";
        let backward = r"E:\Github\iterate-desktop";
        for (target, live, route, request) in [
            (backward, forward, forward, forward),
            (forward, backward, forward, forward),
            (forward, forward, backward, forward),
            (forward, forward, forward, backward),
        ] {
            write_fixture(temp_dir.path(), request_id, route, NOW);
            let request_path = temp_dir.path().join(format!("iterate_request_{request_id}.json"));
            let mut payload: serde_json::Value = serde_json::from_slice(&fs::read(&request_path).unwrap()).unwrap();
            payload["project_path"] = serde_json::json!(request);
            fs::write(request_path, serde_json::to_vec(&payload).unwrap()).unwrap();
            let loaded = load_live_serve_request_fallback_from_dir(
                temp_dir.path(), request_id, target, &[window_instance(request_id, live)], NOW,
            ).unwrap();
            assert_eq!(loaded.payload["request"], payload);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn mixed_separators_still_require_exact_request_and_project_at_each_source() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1790913331998-e8e52f66-620c-4589-a59f-4bc6fd6820f3";
        let forward = "E:/Github/iterate-desktop";
        let backward = r"E:\Github\iterate-desktop";
        write_fixture(temp_dir.path(), request_id, forward, NOW);
        for live in [window_instance("serve-1790913331999", backward), window_instance(request_id, r"E:\Github\other")] {
            assert_eq!(load_live_serve_request_fallback_from_dir(
                temp_dir.path(), request_id, backward, &[live], NOW,
            ).unwrap_err(), ServeRequestFallbackMiss::NoLiveWindowBinding);
        }
        let route_path = temp_dir.path().join(format!("iterate_response_route_{request_id}.json"));
        for (field, value) in [("request_id", "serve-1790913331999"), ("project_path", "E:/Github/other")] {
            write_fixture(temp_dir.path(), request_id, forward, NOW);
            let mut route: serde_json::Value = serde_json::from_slice(&fs::read(&route_path).unwrap()).unwrap();
            route[field] = serde_json::json!(value);
            fs::write(&route_path, serde_json::to_vec(&route).unwrap()).unwrap();
            assert_eq!(load_live_serve_request_fallback_from_dir(
                temp_dir.path(), request_id, backward, &[window_instance(request_id, backward)], NOW,
            ).unwrap_err(), ServeRequestFallbackMiss::RouteMismatch);
        }
        let request_path = temp_dir.path().join(format!("iterate_request_{request_id}.json"));
        for (field, value) in [("id", "serve-1790913331999"), ("project_path", "E:/Github/other")] {
            write_fixture(temp_dir.path(), request_id, forward, NOW);
            let mut request: serde_json::Value = serde_json::from_slice(&fs::read(&request_path).unwrap()).unwrap();
            request[field] = serde_json::json!(value);
            fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
            assert_eq!(load_live_serve_request_fallback_from_dir(
                temp_dir.path(), request_id, backward, &[window_instance(request_id, backward)], NOW,
            ).unwrap_err(), ServeRequestFallbackMiss::RequestMismatch);
        }
    }

    #[test]
    fn rejects_non_serve_request_ids() {
        let temp_dir = private_temp_dir();
        let result = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            "../../secret",
            "/tmp/cunzhi",
            &[],
            NOW,
        );
        assert_eq!(result.unwrap_err(), ServeRequestFallbackMiss::InvalidRoute);
        for request_id in [
            "serve-1790436233047-../../secret",
            "serve-1790436233047-not-a-uuid",
            "serve-1790436233047-8218c42c-7a73-452d-a8a2-5a8ec969e763/evil",
        ] {
            let result = load_live_serve_request_fallback_from_dir(
                temp_dir.path(), request_id, "/tmp/cunzhi", &[], NOW,
            );
            assert_eq!(result.unwrap_err(), ServeRequestFallbackMiss::InvalidRoute);
        }
    }

    #[test]
    fn rejects_missing_live_window_binding() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        write_fixture(temp_dir.path(), request_id, project_path, NOW);

        let result = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[],
            NOW,
        );
        assert_eq!(
            result.unwrap_err(),
            ServeRequestFallbackMiss::NoLiveWindowBinding
        );
    }

    #[test]
    fn rejects_route_project_mismatch() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        write_fixture(temp_dir.path(), request_id, "/tmp/other", NOW);

        let result = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            "/tmp/cunzhi",
            &[window_instance(request_id, "/tmp/cunzhi")],
            NOW,
        );
        assert_eq!(result.unwrap_err(), ServeRequestFallbackMiss::RouteMismatch);
    }

    #[test]
    fn rejects_request_body_route_mismatch() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        write_fixture(temp_dir.path(), request_id, project_path, NOW);
        let mismatched_request = serde_json::json!({
            "id": "serve-1786277363181",
            "message": "不属于当前 route 的正文",
            "project_path": project_path,
        });
        fs::write(
            temp_dir
                .path()
                .join(format!("iterate_request_{request_id}.json")),
            serde_json::to_vec(&mismatched_request).unwrap(),
        )
        .unwrap();

        let result = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[window_instance(request_id, project_path)],
            NOW,
        );
        assert_eq!(
            result.unwrap_err(),
            ServeRequestFallbackMiss::RequestMismatch
        );
    }

    #[test]
    fn accepts_old_route_while_live_window_binding_remains() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        let created_at = NOW - 24 * 60 * 60;
        write_fixture(temp_dir.path(), request_id, project_path, created_at);
        let mut live_window = window_instance(request_id, project_path);
        live_window.registered_at = chrono::DateTime::from_timestamp(created_at, 0)
            .unwrap()
            .to_rfc3339();

        let fallback = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[live_window],
            NOW,
        )
        .unwrap();

        assert_eq!(fallback.payload["request"]["id"], request_id);
        assert_eq!(fallback.age_ms, 24 * 60 * 60 * 1000);
    }

    #[test]
    fn renewed_route_still_matches_original_window_creation_time() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        let original_created_at = NOW - 24 * 60 * 60;
        write_fixture(temp_dir.path(), request_id, project_path, NOW);
        let route_path = temp_dir.path().join(format!("iterate_response_route_{request_id}.json"));
        let mut route: serde_json::Value = serde_json::from_slice(&fs::read(&route_path).unwrap()).unwrap();
        route["original_created_at"] = serde_json::json!(original_created_at);
        fs::write(&route_path, serde_json::to_vec(&route).unwrap()).unwrap();
        let mut live_window = window_instance(request_id, project_path);
        live_window.registered_at = chrono::DateTime::from_timestamp(original_created_at, 0)
            .unwrap().to_rfc3339();

        let fallback = load_live_serve_request_fallback_from_dir(
            temp_dir.path(), request_id, project_path, &[live_window.clone()], NOW,
        ).unwrap();
        assert_eq!(fallback.payload["request"]["id"], request_id);
        assert_eq!(fallback.age_ms, 0);

        live_window.registered_at = chrono::DateTime::from_timestamp(NOW, 0)
            .unwrap().to_rfc3339();
        assert_eq!(
            load_live_serve_request_fallback_from_dir(
                temp_dir.path(), request_id, project_path, &[live_window], NOW,
            ).unwrap_err(),
            ServeRequestFallbackMiss::NoLiveWindowBinding,
        );
    }

    #[test]
    fn rejects_route_created_too_far_in_the_future() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        write_fixture(
            temp_dir.path(),
            request_id,
            project_path,
            NOW + CLOCK_SKEW_ALLOWANCE_SECS + 1,
        );

        let result = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[window_instance(request_id, project_path)],
            NOW,
        );
        assert_eq!(result.unwrap_err(), ServeRequestFallbackMiss::StaleRoute);
    }

    #[test]
    fn rejects_oversized_request_file() {
        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        write_fixture(temp_dir.path(), request_id, project_path, NOW);
        fs::write(
            temp_dir
                .path()
                .join(format!("iterate_request_{request_id}.json")),
            vec![b'x'; SERVE_REQUEST_MAX_BYTES as usize + 1],
        )
        .unwrap();

        let result = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[window_instance(request_id, project_path)],
            NOW,
        );
        assert_eq!(
            result.unwrap_err(),
            ServeRequestFallbackMiss::InvalidRequestFile
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_request_file() {
        use std::os::unix::fs::symlink;

        let temp_dir = private_temp_dir();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        write_fixture(temp_dir.path(), request_id, project_path, NOW);
        let request_path = temp_dir
            .path()
            .join(format!("iterate_request_{request_id}.json"));
        let real_path = temp_dir.path().join("real-request.json");
        fs::rename(&request_path, &real_path).unwrap();
        symlink(&real_path, &request_path).unwrap();

        let result = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[window_instance(request_id, project_path)],
            NOW,
        );
        assert_eq!(
            result.unwrap_err(),
            ServeRequestFallbackMiss::InvalidRequestFile
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_private_temp_directory() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = private_temp_dir();
        fs::set_permissions(temp_dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

        let result = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            "serve-1786277363180",
            "/tmp/cunzhi",
            &[],
            NOW,
        );
        assert_eq!(
            result.unwrap_err(),
            ServeRequestFallbackMiss::InsecureTempDirectory
        );
    }

    #[cfg(unix)]
    #[test]
    fn accepts_sticky_shared_temp_directory_with_owned_files() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = private_temp_dir();
        fs::set_permissions(temp_dir.path(), fs::Permissions::from_mode(0o1777)).unwrap();
        let request_id = "serve-1786277363180";
        let project_path = "/tmp/cunzhi";
        write_fixture(temp_dir.path(), request_id, project_path, NOW);

        let fallback = load_live_serve_request_fallback_from_dir(
            temp_dir.path(),
            request_id,
            project_path,
            &[window_instance(request_id, project_path)],
            NOW,
        )
        .unwrap();
        assert_eq!(fallback.payload["request"]["id"], request_id);
    }
}
