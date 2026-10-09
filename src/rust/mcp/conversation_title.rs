use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub fn session_title(home: &Path, thread_id: &str) -> Option<String> {
    let file = std::fs::File::open(home.join("session_index.jsonl")).ok()?;
    let mut title = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        if entry.get("id").and_then(Value::as_str) != Some(thread_id) { continue; }
        if let Some(name) = entry.get("thread_name").and_then(Value::as_str)
            .map(str::trim).filter(|name| !name.is_empty()) {
            title = Some(name.to_owned());
        }
    }
    title
}

/// Older MCP servers may still supply a per-message title. Resolve display only;
/// never change the caller's request, thread, or reply routing identity.
pub fn normalize_local_request(request: &mut Value) {
    if request.get("codex_thread_provenance").and_then(Value::as_str) != Some("caller_meta")
        || request.get("id").and_then(Value::as_str).is_some_and(|id| id.starts_with("cross-")) {
        return;
    }
    let Some(thread) = request.get("codex_thread_id").and_then(Value::as_str) else { return };
    let home = request.get("codex_home").and_then(Value::as_str)
        .filter(|home| !home.trim().is_empty()).map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    if let Some(title) = home.filter(|home| home.is_absolute())
        .and_then(|home| session_title(&home, thread)) {
        request["conversation_title"] = Value::String(title);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn real_name_wins_over_each_progress_title_without_changing_route() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("session_index.jsonl"), concat!(
            "not-json\n", "{\"id\":\"a\",\"thread_name\":\"Original\"}\n",
            "{\"id\":\"other\",\"thread_name\":\"Other task\"}\n",
            "{\"id\":\"a\",\"thread_name\":\"Renamed session\"}\n",
            "{\"id\":\"a\",\"thread_name\":\"  \"}\n",
        )).unwrap();
        for progress in ["Testing", "Finished"] {
            let mut request = json!({"id":"serve-a","codex_thread_id":"a",
                "codex_thread_provenance":"caller_meta","codex_home":home.path(),
                "conversation_title":progress,"message":"unchanged","timeline_route_id":"route-a"});
            let mut expected = request.clone();
            expected["conversation_title"] = json!("Renamed session");
            normalize_local_request(&mut request);
            assert_eq!(request, expected);
        }
    }

    #[test]
    fn missing_name_untrusted_caller_and_mirror_keep_supplied_title() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("session_index.jsonl"),
            "{\"id\":\"a\",\"thread_name\":\"Local title\"}\n").unwrap();
        for (id, thread, provenance) in [("serve-a", "missing", "caller_meta"),
            ("serve-a", "a", "project_fallback"), ("cross-a", "a", "caller_meta")] {
            let mut request = json!({"id":id,"codex_thread_id":thread,
                "codex_thread_provenance":provenance,"codex_home":home.path(),"conversation_title":"Supplied"});
            let original = request.clone();
            normalize_local_request(&mut request);
            assert_eq!(request, original);
        }
    }
}
