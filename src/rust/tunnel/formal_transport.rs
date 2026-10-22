/// Formal routes share the same origin-health and current-install proof gates.
/// Keep transport identifiers exact; aliases and unknown transports are rejected.
pub fn is_supported_formal_transport(transport: &str) -> bool {
    matches!(
        transport,
        "cloudflare_named_tunnel" | "self_hosted_frp" | "aliyun_ssh_reverse_tunnel"
    )
}

/// Verification timestamps are not part of a route's identity.
pub fn same_formal_route_identity(
    left: Option<&crate::config::FormalMobileRouteConfig>,
    right: Option<&crate::config::FormalMobileRouteConfig>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(a), Some(b)) => a.schema_version == b.schema_version
            && a.transport == b.transport
            && a.base_url == b.base_url
            && a.formal_route_generation == b.formal_route_generation,
        _ => false,
    }
}

/// Only route commits and provider recovery participate in this narrow lock.
/// A busy caller retries; never block the async runtime waiting for another process.
pub fn try_lock_formal_route() -> Result<std::fs::File, String> {
    let directory = crate::config::cunzhi_config_dir().map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(directory.join("formal-route.lock")).map_err(|error| error.to_string())?;
    file.try_lock().map_err(|_| "formal_route_lock_unavailable".to_string())?;
    Ok(file)
}
