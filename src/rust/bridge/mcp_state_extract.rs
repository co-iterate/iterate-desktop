use super::room_submit::payload_string_field;

pub(super) fn extract_project_path_from_mcp_state(payload: &serde_json::Value) -> Option<String> {
    let request = payload.get("request")?;
    ["project_path", "projectPath"].iter().find_map(|key| {
        request
            .get(*key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && *s != ".")
            .map(|s| s.to_string())
    })
}

pub(super) fn extract_request_id_from_mcp_state(payload: &serde_json::Value) -> Option<String> {
    let request = payload.get("request")?;
    ["id", "request_id", "requestId"].iter().find_map(|key| {
        request
            .get(*key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

/// Whether the payload carries a body the iOS client will render; the client
/// drops sync replies whose `request.message` is blank.
pub(super) fn mcp_state_has_request_message(payload: &serde_json::Value) -> bool {
    payload
        .get("request")
        .and_then(|request| request.get("message"))
        .and_then(|value| value.as_str())
        .is_some_and(|message| !message.trim().is_empty())
}

pub(super) fn extract_timeline_route_id_from_mcp_state(
    payload: &serde_json::Value,
) -> Option<String> {
    [
        "timeline_route_id",
        "timelineRouteId",
        "conversation_route_id",
        "conversationRouteId",
    ]
    .iter()
    .find_map(|key| {
        payload
            .get(*key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
    .or_else(|| {
        payload.get("request").and_then(|request| {
            [
                "codex_thread_id",
                "codexThreadId",
                "conversation_id",
                "conversationId",
                "timeline_route_id",
                "timelineRouteId",
            ]
            .iter()
            .find_map(|key| {
                request
                    .get(*key)
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            })
        })
    })
}

pub(super) fn extract_timeline_route_id_from_mcp_action(
    payload: &serde_json::Value,
) -> Option<String> {
    payload_string_field(
        payload,
        &[
            "timeline_route_id",
            "timelineRouteId",
            "conversation_route_id",
            "conversationRouteId",
            "codex_thread_id",
            "codexThreadId",
        ],
    )
    .or_else(|| {
        payload.get("metadata").and_then(|metadata| {
            payload_string_field(
                metadata,
                &[
                    "timeline_route_id",
                    "timelineRouteId",
                    "conversation_route_id",
                    "conversationRouteId",
                    "codex_thread_id",
                    "codexThreadId",
                ],
            )
        })
    })
}

pub(super) fn extract_conversation_id_from_mcp_state(
    payload: &serde_json::Value,
) -> Option<String> {
    ["conversation_id", "conversationId", "timelineTreeId"]
        .iter()
        .find_map(|key| {
            payload
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

#[cfg(test)]
mod tests {
    use super::mcp_state_has_request_message;

    #[test]
    fn request_message_presence_ignores_blank_bodies() {
        assert!(mcp_state_has_request_message(
            &serde_json::json!({ "request": { "message": "hi" } })
        ));
        assert!(!mcp_state_has_request_message(
            &serde_json::json!({ "request": { "message": "  \n" } })
        ));
        assert!(!mcp_state_has_request_message(
            &serde_json::json!({ "request": { "request_id": "r1" } })
        ));
        assert!(!mcp_state_has_request_message(&serde_json::json!({})));
    }
}
