//! Observability only. This module never interprets permission content or grants access.
use once_cell::sync::Lazy;
use ring::hmac;
use serde_json::{json, Value};

const METHOD: &str = "mcpServer/elicitation/request";
// Ephemeral keyed identifiers allow correlation within a process without logging raw IDs.
static KEY: Lazy<hmac::Key> =
    Lazy::new(|| hmac::Key::new(hmac::HMAC_SHA256, uuid::Uuid::new_v4().as_bytes()));

fn kind(v: Option<&Value>) -> &'static str {
    match v {
        None => "missing",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn identifier(v: Option<&Value>) -> Value {
    let encoded = match v {
        Some(Value::String(s)) if s.len() <= 4096 => format!("s:{s}"),
        Some(Value::Number(n)) => format!("n:{n}"),
        _ => return Value::Null,
    };
    let digest = hmac::sign(&KEY, encoded.as_bytes());
    Value::String(digest.as_ref().iter().map(|b| format!("{b:02x}")).collect())
}

pub(super) struct Diagnostic(Value);

impl Diagnostic {
    pub(super) fn from_request(message: &Value, thread: &str, turn: Option<&str>) -> Option<Self> {
        if message.get("method").and_then(Value::as_str) != Some(METHOD) {
            return None;
        }
        let p = &message["params"];
        let mode = match p.get("mode").and_then(Value::as_str) {
            Some("form") => "form",
            Some("openai/form") => "openai/form",
            Some("openaiForm") => "openaiForm",
            Some("url") => "url",
            Some("openai/userVerification") => "openai/userVerification",
            _ => "unknown",
        };
        let schema = p.get("requestedSchema");
        Some(Self(json!({
            "diagnostic_version": 1,
            "source_baseline": "ac7c305e30af713e7d3bf7dc85e8ad082f1be3fd",
            "event_id": uuid::Uuid::new_v4().to_string(),
            "method": METHOD,
            "rpc_id_ref": identifier(message.get("id")),
            "rpc_id_type": kind(message.get("id")),
            "thread_ref": identifier(p.get("threadId")),
            "turn_ref": identifier(p.get("turnId")),
            "thread_matches": p.get("threadId").and_then(Value::as_str) == Some(thread),
            "turn_matches": turn.is_some() && p.get("turnId").and_then(Value::as_str) == turn,
            "turn_id_type": kind(p.get("turnId")),
            "server_ref": identifier(p.get("serverName")),
            "params_type": kind(message.get("params")),
            "mode": mode,
            "schema_type": kind(schema),
            "schema_property_count": schema.and_then(|s| s.get("properties")).and_then(Value::as_object).map(|p| p.len()),
            "schema_required_count": schema.and_then(|s| s.get("required")).and_then(Value::as_array).map(|p| p.len()),
            "metadata_type": kind(p.get("_meta")),
            "target_app": "unavailable_without_verified_contract",
            "tool_call_correlation": "unavailable_without_verified_contract"
        })))
    }

    fn event(&self, phase: &str, result: &str) -> Value {
        let mut event = self.0.clone();
        event["phase"] = json!(phase);
        event["result"] = json!(result);
        event
    }

    pub(super) fn received(&self) {
        self.emit("received", "undecided");
    }

    pub(super) fn duplicate_ignored(&self) {
        self.emit("routed", "pending_rpc_collision_ignored");
    }

    pub(super) fn declined(&self) {
        self.emit("routed", "unsupported_decline");
    }

    pub(super) fn written(&self, success: bool) {
        self.emit(
            "response_write",
            if success {
                "decline_flushed"
            } else {
                "write_failed_delivery_unknown"
            },
        );
    }

    fn emit(&self, phase: &str, result: &str) {
        // Local flush is not evidence that the remote endpoint consumed the response.
        eprintln!("[codex-live-elicitation] {}", self.event(phase, result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_values_keys_identifiers_and_unknown_modes() {
        let secret = "PRIVATE_TOKEN_PROMPT_URL_APP_ID";
        let r = json!({"id":secret,"method":METHOD,"params":{
            "threadId":secret,"turnId":secret,"serverName":secret,"mode":secret,
            "message":secret,"challenge":secret,"url":secret,"_meta":{secret:secret},
            "requestedSchema":{"properties":{secret:{"description":secret}},"required":[secret]}}});
        let d = Diagnostic::from_request(&r, secret, Some(secret)).unwrap();
        let received = d.event("received", "undecided");
        let written = d.event("response_write", "decline_flushed");
        assert!(!received.to_string().contains(secret));
        assert_eq!(received["rpc_id_ref"], written["rpc_id_ref"]);
        assert_eq!(received["event_id"], written["event_id"]);
        assert_eq!(received["mode"], "unknown");
        assert_eq!(received["schema_property_count"], 1);
        assert_eq!(received["thread_matches"], true);
        assert_eq!(received["turn_matches"], true);
    }

    #[test]
    fn malformed_and_null_context_are_bounded_and_not_matched() {
        for params in [Value::Null, json!([]), json!(false), json!({"turnId":null})] {
            let d = Diagnostic::from_request(
                &json!({"id":{},"method":METHOD,"params":params}),
                "t",
                None,
            )
            .unwrap();
            assert_eq!(d.0["turn_matches"], false);
            assert_eq!(d.0["rpc_id_ref"], Value::Null);
            assert!(d.0.to_string().len() < 1800);
        }
        assert_eq!(identifier(Some(&json!("x".repeat(5000)))), Value::Null);
        assert_ne!(identifier(Some(&json!(7))), identifier(Some(&json!("7"))));
        assert!(Diagnostic::from_request(
            &json!({"method":"item/commandExecution/requestApproval"}),
            "t",
            Some("u")
        )
        .is_none());
    }
}
