pub mod commands;
pub mod end_command;
pub mod manager;
pub mod types;

pub use end_command::{
    is_explicit_conversation_end, is_explicit_conversation_end_response,
    is_popup_closed_response_source, EXPLICIT_CONVERSATION_END_SOURCE, POPUP_CLOSED_SOURCE,
};

pub use manager::ConversationManager;
pub use types::{
    ConversationNode, ConversationTree, NodeMetadata, NodeType, TimelineImageAttachment,
};

fn normalize_route_part(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|part| !part.is_empty() && *part != "Unknown")
        .map(ToOwned::to_owned)
}

pub fn resolve_tree_route_key(
    request_id: Option<&str>,
    project_path: Option<&str>,
) -> Option<String> {
    normalize_route_part(request_id).or_else(|| normalize_route_part(project_path))
}
