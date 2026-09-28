pub const EXPLICIT_CONVERSATION_END_SOURCE: &str = "explicit_conversation_end";
pub const POPUP_CLOSED_SOURCE: &str = "popup_closed";

const END_COMMANDS: &[&str] = &["结束对话", "退出对话", "停止对话", "结束本次对话", "/end"];

fn trim_terminal_punctuation(value: &str) -> &str {
    value.trim_end_matches(|character: char| {
        character.is_whitespace()
            || character.is_ascii_punctuation()
            || matches!(
                character,
                '。' | '，'
                    | '；'
                    | '：'
                    | '！'
                    | '？'
                    | '、'
                    | '…'
                    | '～'
                    | '—'
                    | '”'
                    | '’'
                    | '」'
                    | '』'
                    | '】'
                    | '）'
            )
    })
}

/// Only a complete, standalone command ends the current zhi/call_zhi interaction.
/// Sentences that merely mention an end command remain ordinary user input.
pub fn is_explicit_conversation_end(value: &str) -> bool {
    let normalized = trim_terminal_punctuation(value.trim());
    END_COMMANDS
        .iter()
        .any(|command| normalized.eq_ignore_ascii_case(command))
}

/// Treat an exact end command submitted either as free text or as a predefined
/// option as the same explicit user decision.
pub fn is_explicit_conversation_end_response(
    user_input: &str,
    selected_options: &[String],
) -> bool {
    is_explicit_conversation_end(user_input)
        || selected_options
            .iter()
            .any(|option| is_explicit_conversation_end(option))
}

pub fn is_popup_closed_response_source(source: &str) -> bool {
    source.trim().eq_ignore_ascii_case(POPUP_CLOSED_SOURCE)
}

#[cfg(test)]
mod tests {
    use super::{
        is_explicit_conversation_end, is_explicit_conversation_end_response,
        is_popup_closed_response_source,
    };

    #[test]
    fn accepts_complete_end_commands_with_whitespace_and_terminal_punctuation() {
        for value in [
            "结束对话",
            " 退出对话。 ",
            "停止对话!",
            "结束本次对话！？",
            "结束对话，",
            "结束对话……",
            "/end",
            " /END. ",
        ] {
            assert!(
                is_explicit_conversation_end(value),
                "expected match: {value:?}"
            );
        }
    }

    #[test]
    fn rejects_mentions_and_unrelated_content() {
        for value in [
            "",
            "如何结束对话",
            "结束对话后会怎样",
            "请帮我结束对话",
            "/end now",
            "结束循环",
        ] {
            assert!(
                !is_explicit_conversation_end(value),
                "expected ordinary input: {value:?}"
            );
        }
    }

    #[test]
    fn accepts_end_commands_from_predefined_options() {
        assert!(is_explicit_conversation_end_response(
            "",
            &["结束对话".to_string()]
        ));
        assert!(is_explicit_conversation_end_response(
            "补充说明",
            &["继续".to_string(), "/end".to_string()]
        ));
        assert!(!is_explicit_conversation_end_response(
            "如何结束对话",
            &["稍后处理".to_string()]
        ));
    }

    #[test]
    fn recognizes_only_the_popup_closed_response_source() {
        assert!(is_popup_closed_response_source("popup_closed"));
        assert!(is_popup_closed_response_source(" POPUP_CLOSED "));
        assert!(!is_popup_closed_response_source("popup_cancelled"));
    }
}
