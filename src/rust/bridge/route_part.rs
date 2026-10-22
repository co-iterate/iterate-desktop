pub(super) fn normalize_route_part(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Compare internal project keys without changing the path sent back to clients.
pub(super) fn project_paths_match(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.replace('\\', "/") == right.replace('\\', "/")
    } else {
        // On Unix a backslash may be a literal filename character.
        left == right
    }
}
