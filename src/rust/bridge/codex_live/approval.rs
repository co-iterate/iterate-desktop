//! One-shot approvals for the current Live execution turn. No session grants.
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(super) struct PendingApproval {
    pub token: String,
    pub rpc_id: Value,
    pub turn_id: String,
    pub action: String,
    pub accept: Value,
    pub decline: Value,
    pub expires: Instant,
}

impl PendingApproval {
    pub fn from_request(message: &Value, thread: &str, turn: &str) -> Option<Self> {
        let p = message.get("params")?;
        if p.get("threadId")?.as_str()? != thread || p.get("turnId")?.as_str()? != turn {
            return None;
        }
        let method = message.get("method")?.as_str()?;
        let (action, accept, decline) = match method {
            "item/commandExecution/requestApproval" => {
                // Network-only and persistent amendments are not single commands.
                let command = p.get("command")?.as_str()?.trim();
                if command.is_empty()
                    || !p
                        .get("networkApprovalContext")
                        .unwrap_or(&Value::Null)
                        .is_null()
                {
                    return None;
                }
                if let Some(options) = p.get("availableDecisions").and_then(Value::as_array) {
                    if !options.iter().any(|v| v == "accept") {
                        return None;
                    }
                }
                (
                    format!(
                        "{}\n\n目录：{}\n原因：{}",
                        command,
                        p.get("cwd").and_then(Value::as_str).unwrap_or("未提供"),
                        p.get("reason")
                            .and_then(Value::as_str)
                            .unwrap_or("此操作需要额外执行权限")
                    ),
                    json!({"decision":"accept"}),
                    json!({"decision":"decline"}),
                )
            }
            // Connector approvals use explicit accept/decline options. General questions
            // are not approvals and must not be answered on the user's behalf.
            "item/tool/requestUserInput" | "tool/requestUserInput" => {
                let questions = p.get("questions")?.as_array()?;
                if questions.is_empty() {
                    return None;
                }
                let mut yes = serde_json::Map::new();
                let mut no = serde_json::Map::new();
                let mut details = Vec::new();
                for q in questions {
                    let options = q.get("options")?.as_array()?;
                    let accept_label = options.iter().find_map(|o| {
                        let label = o.get("label")?.as_str()?;
                        (label.eq_ignore_ascii_case("accept") || label == "允许").then_some(label)
                    })?;
                    let decline_label = options.iter().find_map(|o| {
                        let label = o.get("label")?.as_str()?;
                        (label.eq_ignore_ascii_case("decline") || label == "拒绝").then_some(label)
                    })?;
                    let id = q.get("id")?.as_str()?.to_string();
                    yes.insert(id.clone(), json!({"answers":[accept_label]}));
                    no.insert(id, json!({"answers":[decline_label]}));
                    details.push(q.get("question")?.as_str()?.to_string());
                }
                (
                    details.join("\n\n"),
                    json!({"answers":yes}),
                    json!({"answers":no}),
                )
            }
            _ => return None,
        };
        if action.len() > 24 * 1024 {
            return None;
        }
        Some(Self {
            token: uuid::Uuid::new_v4().to_string(),
            rpc_id: message.get("id")?.clone(),
            turn_id: turn.to_owned(),
            action,
            accept,
            decline,
            expires: Instant::now() + Duration::from_secs(120),
        })
    }

    pub fn is_valid(&self, token: &str, turn: Option<&str>) -> bool {
        self.token == token && turn == Some(self.turn_id.as_str()) && Instant::now() < self.expires
    }
}

pub(super) fn unsupported_response(message: &Value) -> Value {
    let result = match message.get("method").and_then(Value::as_str).unwrap_or("") {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            json!({"decision":"decline"})
        }
        "item/permissions/requestApproval" => json!({"permissions":{},"scope":"turn"}),
        "item/tool/requestUserInput" | "tool/requestUserInput" => json!({"answers":{}}),
        "mcpServer/elicitation/request" => json!({"action":"decline","content":null}),
        _ => {
            return json!({"id":message["id"],"error":{"code":-32601,"message":"Live cannot safely answer this request; ask a concise question in the final response instead."}})
        }
    };
    json!({"id":message["id"],"result":result})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command() -> Value {
        json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"t","turnId":"u","command":"test command","cwd":"/tmp","reason":"test"}})
    }
    #[test]
    fn binds_request_turn_and_nonce() {
        let p = PendingApproval::from_request(&command(), "t", "u").unwrap();
        assert!(p.is_valid(&p.token, Some("u")));
        assert!(!p.is_valid("other", Some("u")));
        assert!(!p.is_valid(&p.token, Some("other")));
        assert_eq!(p.accept, json!({"decision":"accept"}));
        assert!(PendingApproval::from_request(&command(), "other", "u").is_none());
    }
    #[test]
    fn expiration_and_broad_grants_fail_closed() {
        let mut p = PendingApproval::from_request(&command(), "t", "u").unwrap();
        p.expires = Instant::now() - Duration::from_secs(1);
        assert!(!p.is_valid(&p.token, Some("u")));
        let mut request = command();
        request["params"]["availableDecisions"] = json!(["acceptForSession"]);
        assert!(PendingApproval::from_request(&request, "t", "u").is_none());
        request = command();
        request["params"]["networkApprovalContext"] = json!({"host":"example.com"});
        assert!(PendingApproval::from_request(&request, "t", "u").is_none());
    }
    #[test]
    fn ordinary_questions_are_never_auto_answered() {
        let r = json!({"id":3,"method":"item/tool/requestUserInput","params":{"threadId":"t","turnId":"u","questions":[{"id":"q","question":"Which?","options":[{"label":"A"},{"label":"B"}]}]}});
        assert!(PendingApproval::from_request(&r, "t", "u").is_none());
        assert_eq!(unsupported_response(&r)["result"], json!({"answers":{}}));
    }
}
