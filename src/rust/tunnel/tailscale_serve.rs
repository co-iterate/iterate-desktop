//! Tailscale Serve 正式路线：用户自己用 `tailscale serve` 把 Bridge 挂到
//! `https://<机器名>.<tailnet>.ts.net`，只在 tailnet 内可达。
//! iterate 只读检测、登记和校验，从不替用户改 tailnet 配置；
//! 检测到 Funnel 时拒绝登记，因为那会把 Bridge 暴露到公网。

use serde::Serialize;

pub const TRANSPORT: &str = "tailscale_serve";
const TAILSCALE_APP_CLI_PATH: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";
const BRIDGE_TARGET: &str = "http://127.0.0.1:8080";
const CLI_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TailscaleServeStatus {
    pub cli_available: bool,
    pub running: bool,
    /// Tailscale 自报的状态（Running / NeedsLogin / Stopped…），引导用来区分「没登录」和「没打开」
    pub backend_state: Option<String>,
    /// tailnet 里有在线的 iPhone：引导第 4 步据此自动打勾
    pub ios_peer_online: bool,
    pub dns_name: Option<String>,
    pub https_cert_available: bool,
    pub serve_proxies_bridge: bool,
    pub funnel_enabled: bool,
    /// 全部就绪时才有值，可直接登记为正式路线
    pub base_url: Option<String>,
    /// 用户需要自己执行的命令（只在还没配好 serve 时给出）
    pub setup_command: Option<String>,
    /// 第一个未满足的条件；None 表示可以登记
    pub blocker: Option<String>,
}

/// `https://<host>.ts.net`，不带端口、路径和凭据
pub fn origin_is_valid(value: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(value.trim().trim_end_matches('/')) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.port().is_none()
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed
            .host_str()
            .is_some_and(|host| host.len() > ".ts.net".len() && host.ends_with(".ts.net"))
}

fn cli_path() -> Option<String> {
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("tailscale");
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    std::path::Path::new(TAILSCALE_APP_CLI_PATH)
        .is_file()
        .then(|| TAILSCALE_APP_CLI_PATH.to_string())
}

async fn run_json(cli: &str, args: &[&str]) -> Option<serde_json::Value> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(CLI_TIMEOUT_SECS),
        // Tailscale.app 里的二进制只有在终端（有 TERM）下才当 CLI 用；
        // 由 launchd 拉起的 Bridge 没有 TERM，会去启动 GUI 并失败（CLIError 3）
        tokio::process::Command::new(cli)
            .args(args)
            .env("TAILSCALE_BE_CLI", "1")
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn dns_name(status: &serde_json::Value) -> Option<String> {
    status
        .pointer("/Self/DNSName")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn self_tailscale_ipv4(status: &serde_json::Value) -> Option<std::net::Ipv4Addr> {
    status
        .pointer("/Self/TailscaleIPs")
        .and_then(|value| value.as_array())?
        .iter()
        .filter_map(|value| value.as_str()?.parse::<std::net::Ipv4Addr>().ok())
        .next()
}

/// 纯函数，便于测试：由 `tailscale status --json` 和 `tailscale serve status --json` 得出结论
pub fn evaluate(
    cli_available: bool,
    status: Option<&serde_json::Value>,
    serve: Option<&serde_json::Value>,
    cli_display: &str,
) -> TailscaleServeStatus {
    let backend_state = status
        .and_then(|value| value.get("BackendState"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let running = backend_state.as_deref() == Some("Running");
    let ios_peer_online = status
        .and_then(|value| value.get("Peer"))
        .and_then(|value| value.as_object())
        .is_some_and(|peers| {
            peers.values().any(|peer| {
                peer.get("OS").and_then(|os| os.as_str()) == Some("iOS")
                    && peer.get("Online").and_then(|online| online.as_bool()) == Some(true)
            })
        });
    let dns_name = status.filter(|_| running).and_then(dns_name);
    let https_cert_available = match (status, dns_name.as_deref()) {
        (Some(status), Some(host)) => status
            .get("CertDomains")
            .and_then(|value| value.as_array())
            .is_some_and(|domains| {
                domains.iter().any(|domain| {
                    domain
                        .as_str()
                        .is_some_and(|domain| domain.trim_end_matches('.').eq_ignore_ascii_case(host))
                })
            }),
        _ => false,
    };
    let web_key = dns_name.as_ref().map(|host| format!("{host}:443"));
    let serve_proxies_bridge = match (serve, web_key.as_deref()) {
        (Some(serve), Some(key)) => {
            serve.pointer("/TCP/443/HTTPS").and_then(|value| value.as_bool()) == Some(true)
                && serve
                    .get("Web")
                    .and_then(|web| web.get(key))
                    .and_then(|site| site.pointer("/Handlers/~1/Proxy"))
                    .and_then(|value| value.as_str())
                    .is_some_and(|target| {
                        let target = target.trim_end_matches('/');
                        target == BRIDGE_TARGET || target == "http://localhost:8080"
                    })
        }
        _ => false,
    };
    // 任何一个入口开着 Funnel，Bridge 就在公网上可达，这条路线的隐私前提就不成立
    let funnel_enabled = serve
        .and_then(|serve| serve.get("AllowFunnel"))
        .and_then(|value| value.as_object())
        .is_some_and(|entries| entries.values().any(|allowed| allowed.as_bool() == Some(true)));

    let blocker = if !cli_available {
        Some("tailscale_not_installed")
    } else if !running {
        Some("tailscale_not_running")
    } else if dns_name.is_none() {
        Some("tailscale_magicdns_disabled")
    } else if !https_cert_available {
        Some("tailscale_https_disabled")
    } else if funnel_enabled {
        Some("tailscale_funnel_enabled")
    } else if !serve_proxies_bridge {
        Some("tailscale_serve_not_configured")
    } else {
        None
    };
    let setup_command = (cli_available && running && !serve_proxies_bridge)
        .then(|| format!("\"{cli_display}\" serve --bg 8080"));
    let base_url = blocker
        .is_none()
        .then(|| dns_name.as_ref().map(|host| format!("https://{host}")))
        .flatten()
        .filter(|url| origin_is_valid(url));

    TailscaleServeStatus {
        cli_available,
        running,
        backend_state,
        ios_peer_online,
        dns_name,
        https_cert_available,
        serve_proxies_bridge,
        funnel_enabled,
        base_url,
        setup_command,
        blocker: blocker.map(str::to_string),
    }
}

async fn run_command(cli: &str, args: &[&str]) -> Result<(), String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::process::Command::new(cli)
            .args(args)
            .env("TAILSCALE_BE_CLI", "1")
            .output(),
    )
    .await
    .map_err(|_| "tailscale_command_timeout".to_string())?
    .map_err(|_| "tailscale_command_failed".to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        log::warn!(
            "[TailscaleServe] {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Err("tailscale_command_failed".to_string())
    }
}

/// 用户在引导里明确点了「帮我执行」才调用：只在 tailnet 内提供 8080，不开 Funnel。
/// Funnel 开着时拒绝，必须先由用户关掉 Funnel。
pub async fn start_serve() -> Result<TailscaleServeStatus, String> {
    let before = inspect().await;
    match before.blocker.as_deref() {
        None => return Ok(before),
        Some("tailscale_serve_not_configured") => {}
        Some(other) => return Err(other.to_string()),
    }
    let cli = cli_path().ok_or_else(|| "tailscale_not_installed".to_string())?;
    run_command(&cli, &["serve", "--bg", "8080"]).await?;
    Ok(inspect().await)
}

/// 用户在引导里明确点了「帮我关掉 Funnel」才调用。
/// `funnel reset` 会同时清掉 serve 配置，所以之后引导回到「挂上 iterate」那一步。
pub async fn reset_funnel() -> Result<TailscaleServeStatus, String> {
    let cli = cli_path().ok_or_else(|| "tailscale_not_installed".to_string())?;
    run_command(&cli, &["funnel", "reset"]).await?;
    Ok(inspect().await)
}

pub async fn inspect() -> TailscaleServeStatus {
    let Some(cli) = cli_path() else {
        return evaluate(false, None, None, TAILSCALE_APP_CLI_PATH);
    };
    let (status, serve) = tokio::join!(
        run_json(&cli, &["status", "--json"]),
        run_json(&cli, &["serve", "status", "--json"]),
    );
    evaluate(true, status.as_ref(), serve.as_ref(), &cli)
}

/// 校验 `*.ts.net` 端点时绕开系统代理，并把域名直接解析到本机 tailnet 地址：
/// 本机代理常常接管 DNS，把 ts.net 解析成公网的 Funnel 入口，校验就会误判失败。
/// 证书仍按域名校验，所以这不会放松身份证明。
pub async fn proof_resolve_override(endpoint: &str) -> Option<(String, std::net::SocketAddr)> {
    let host = reqwest::Url::parse(endpoint).ok()?.host_str()?.to_ascii_lowercase();
    if !host.ends_with(".ts.net") {
        return None;
    }
    let cli = cli_path()?;
    let status = run_json(&cli, &["status", "--json"]).await?;
    if dns_name(&status).as_deref() != Some(host.as_str()) {
        return None;
    }
    let ip = self_tailscale_ipv4(&status)?;
    Some((host, std::net::SocketAddr::from((ip, 443))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status() -> serde_json::Value {
        json!({
            "BackendState": "Running",
            "Self": { "DNSName": "mac.tail123.ts.net.", "TailscaleIPs": ["100.101.102.103", "fd7a::1"] },
            "CertDomains": ["mac.tail123.ts.net"],
        })
    }

    fn serve() -> serde_json::Value {
        json!({
            "TCP": { "443": { "HTTPS": true } },
            "Web": { "mac.tail123.ts.net:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:8080" } } } },
        })
    }

    #[test]
    fn ready_when_serve_proxies_bridge_inside_tailnet() {
        let result = evaluate(true, Some(&status()), Some(&serve()), "tailscale");
        assert_eq!(result.blocker, None);
        assert_eq!(result.base_url.as_deref(), Some("https://mac.tail123.ts.net"));
        assert_eq!(result.setup_command, None);
        assert_eq!(self_tailscale_ipv4(&status()), Some("100.101.102.103".parse().unwrap()));
    }

    #[test]
    fn reports_login_state_and_online_iphone() {
        let mut status = status();
        status["Peer"] = json!({
            "a": { "OS": "iOS", "Online": true },
            "b": { "OS": "macOS", "Online": true },
        });
        let result = evaluate(true, Some(&status), Some(&serve()), "t");
        assert_eq!(result.backend_state.as_deref(), Some("Running"));
        assert!(result.ios_peer_online);
        status["Peer"]["a"]["Online"] = json!(false);
        assert!(!evaluate(true, Some(&status), Some(&serve()), "t").ios_peer_online);
        let needs_login = json!({ "BackendState": "NeedsLogin" });
        let result = evaluate(true, Some(&needs_login), None, "t");
        assert_eq!(result.backend_state.as_deref(), Some("NeedsLogin"));
        assert_eq!(result.blocker.as_deref(), Some("tailscale_not_running"));
    }

    #[test]
    fn funnel_blocks_registration() {
        let mut serve = serve();
        serve["AllowFunnel"] = json!({ "mac.tail123.ts.net:443": true });
        let result = evaluate(true, Some(&status()), Some(&serve), "tailscale");
        assert_eq!(result.blocker.as_deref(), Some("tailscale_funnel_enabled"));
        assert!(result.base_url.is_none());
    }

    #[test]
    fn reports_first_missing_step_in_order() {
        assert_eq!(evaluate(false, None, None, "t").blocker.as_deref(), Some("tailscale_not_installed"));
        let stopped = json!({ "BackendState": "Stopped", "Self": { "DNSName": "" } });
        assert_eq!(evaluate(true, Some(&stopped), None, "t").blocker.as_deref(), Some("tailscale_not_running"));
        let mut no_cert = status();
        no_cert["CertDomains"] = serde_json::Value::Null;
        assert_eq!(evaluate(true, Some(&no_cert), Some(&serve()), "t").blocker.as_deref(), Some("tailscale_https_disabled"));
        let unconfigured = evaluate(true, Some(&status()), Some(&json!({})), "/bin/tailscale");
        assert_eq!(unconfigured.blocker.as_deref(), Some("tailscale_serve_not_configured"));
        assert_eq!(unconfigured.setup_command.as_deref(), Some("\"/bin/tailscale\" serve --bg 8080"));
    }

    #[test]
    fn serve_to_other_port_is_not_the_bridge() {
        let mut serve = serve();
        serve["Web"]["mac.tail123.ts.net:443"]["Handlers"]["/"]["Proxy"] = json!("http://127.0.0.1:3000");
        assert_eq!(
            evaluate(true, Some(&status()), Some(&serve), "t").blocker.as_deref(),
            Some("tailscale_serve_not_configured")
        );
    }

    #[test]
    fn origin_must_be_bare_ts_net_https() {
        assert!(origin_is_valid("https://mac.tail123.ts.net"));
        assert!(origin_is_valid("https://mac.tail123.ts.net/"));
        assert!(!origin_is_valid("http://mac.tail123.ts.net"));
        assert!(!origin_is_valid("https://mac.tail123.ts.net:8443"));
        assert!(!origin_is_valid("https://mac.tail123.ts.net/ws"));
        assert!(!origin_is_valid("https://example.com"));
        assert!(!origin_is_valid("https://ts.net.example.com"));
        assert!(!origin_is_valid("https://.ts.net"));
    }
}
