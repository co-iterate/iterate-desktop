pub(super) fn normalize_mcp_action_images(payload: &serde_json::Value) -> Vec<serde_json::Value> {
    let images_raw = payload
        .get("images")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));

    if let Some(arr) = images_raw.as_array() {
        arr.iter()
            .filter_map(|img| {
                let data_url = img.get("data")?.as_str()?;
                let base64 = if let Some(pos) = data_url.find(',') {
                    &data_url[pos + 1..]
                } else {
                    data_url
                };
                Some(serde_json::json!({
                    "data": base64,
                    "media_type": img
                        .get("media_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("image/png"),
                    "filename": img.get("filename").cloned().unwrap_or(serde_json::Value::Null),
                }))
            })
            .collect()
    } else {
        vec![]
    }
}

fn collapse_extra_blank_lines(text: &str) -> String {
    let mut lines = Vec::new();
    let mut blank_count = 0;

    for line in text.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                lines.push(String::new());
            }
        } else {
            blank_count = 0;
            lines.push(line.to_string());
        }
    }

    lines.join("\n").trim().to_string()
}

fn normalize_goal_closing_spacing(text: &str) -> String {
    let mut normalized = text.to_string();
    while normalized.contains("\n\n》") {
        normalized = normalized.replace("\n\n》", "》");
    }
    while normalized.contains("\n》") {
        normalized = normalized.replace("\n》", "》");
    }
    normalized
}

fn strip_goal_image_reference_context(goal_text: &str) -> String {
    let lines: Vec<&str> = goal_text.lines().collect();
    let mut kept = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        let starts_legacy_image_block = trimmed.starts_with("附加图片：")
            && (trimmed.contains("images 附件")
                || lines
                    .get(index + 1)
                    .map(|next| next.trim() == "附件地址：")
                    .unwrap_or(false));

        if starts_legacy_image_block {
            let mut preserve_goal_close = trimmed.ends_with('》');
            index += 1;

            if lines
                .get(index)
                .map(|next| next.trim() == "附件地址：")
                .unwrap_or(false)
            {
                index += 1;
            }

            while index < lines.len() {
                let nested = lines[index].trim();
                if nested.starts_with("- images[")
                    || nested == "（见 images 附件）"
                    || nested == "（见 images 附件）》"
                {
                    preserve_goal_close = preserve_goal_close || nested.ends_with('》');
                    index += 1;
                    continue;
                }
                break;
            }

            if preserve_goal_close {
                kept.push("》");
            }
            continue;
        }

        kept.push(line);
        index += 1;
    }

    normalize_goal_closing_spacing(&collapse_extra_blank_lines(&kept.join("\n")))
}

fn append_goal_selected_options_context(goal_text: &str, selected_options: &[String]) -> String {
    let missing_options: Vec<&str> = selected_options
        .iter()
        .map(|option| option.trim())
        .filter(|option| !option.is_empty() && !goal_text.contains(option))
        .collect();

    if missing_options.is_empty() {
        return goal_text.to_string();
    }

    let option_text = format!(
        "选中的选项：\n{}",
        missing_options
            .iter()
            .map(|option| format!("- {}", option))
            .collect::<Vec<_>>()
            .join("\n")
    );

    if goal_text.trim().is_empty() {
        option_text
    } else {
        format!("{}\n\n{}", goal_text, option_text)
    }
}

pub(crate) fn build_goal_payload_parts(
    payload: &serde_json::Value,
) -> (String, String, serde_json::Value) {
    let cleaned_input = strip_goal_image_reference_context(
        payload
            .get("user_input")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim(),
    );
    let selected_options: Vec<String> = payload
        .get("selected_options")
        .and_then(|value| value.as_array())
        .map(|options| {
            options
                .iter()
                .filter_map(|option| option.as_str())
                .map(str::trim)
                .filter(|option| !option.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let selected_options_value = payload
        .get("selected_options")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let image_count = normalize_mcp_action_images(payload).len();

    let base_goal_text = if cleaned_input.is_empty() {
        selected_options.join("\n")
    } else {
        cleaned_input.clone()
    };
    let goal_text = append_goal_selected_options_context(&base_goal_text, &selected_options);
    let goal_title = if !cleaned_input.is_empty() {
        cleaned_input
    } else if !selected_options.is_empty() {
        selected_options.join(" / ")
    } else if image_count > 0 {
        format!("图片目标: {} 张", image_count)
    } else {
        String::new()
    };

    (goal_text, goal_title, selected_options_value)
}

const GOAL_XI_PREFLIGHT: &str = "任何实现动作前必须执行 xi，按当前项目、线程和目标关键词检查相关对话、experience 与稳定产物，判断同一目标是否已经解决或已有可复用方案。命中完成证据时先验证当前状态并复用，禁止重复实现或伪造完成；只命中相似问题时说明差异后继续。";

pub(super) fn render_goal_submit_prompt(goal: &str, template: &str) -> String {
    let execution_rules = if template.trim().is_empty() {
        crate::constants::mcp::DEFAULT_GOAL_PROMPT_TEMPLATE
    } else {
        template.trim()
    };

    format!(
        "进入 GoalRun 目标模式。\n\n目标：\n《\n{}\n》\n\n## XI 启动检查（正式 Goal 同步后执行）\n{}\n\n执行规则：\n{}",
        goal, GOAL_XI_PREFLIGHT, execution_rules
    )
}

pub(crate) fn build_goal_submit_prompt(goal: &str) -> String {
    let template = crate::config::load_standalone_config()
        .map(|config| config.reply_config.goal_prompt_template)
        .unwrap_or_else(|_| crate::constants::mcp::DEFAULT_GOAL_PROMPT_TEMPLATE.to_string());
    render_goal_submit_prompt(goal, &template)
}

#[cfg(test)]
mod tests {
    use super::render_goal_submit_prompt;

    #[test]
    fn goal_prompt_uses_the_configured_template_and_always_injects_xi_preflight() {
        let prompt = render_goal_submit_prompt("修复 Goal", "1. 自定义执行规则");

        assert!(prompt.contains("目标：\n《\n修复 Goal\n》"));
        assert!(prompt.contains("## XI 启动检查（正式 Goal 同步后执行）"));
        assert!(prompt.contains("必须执行 xi"));
        assert!(prompt.contains("禁止重复实现"));
        assert!(prompt.ends_with("1. 自定义执行规则"));
    }
}
