use crate::app::builder::{check_frontend_assets, check_frontend_assets_for_dist, run_tauri_app};
use crate::config::load_standalone_telegram_config;
use crate::log_important;
use crate::mcp::tools::checkpoint;
use crate::server::{
    self, DialogRequest, DialogResponse, InteractionLifecycleEvent, InteractionPhase,
};
use crate::telegram::handle_telegram_only_mcp_request;
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

fn instance_debug_log(tag: &str, message: impl AsRef<str>) {
    let line = format!(
        "{} [iterate-cli:{}] {} {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        std::process::id(),
        tag,
        message.as_ref()
    );
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/iterate-instance-debug.log")
    {
        let _ = file.write_all(line.as_bytes());
    }
}

const DEFAULT_LOCAL_BRIDGE_BASE_URL: &str = "http://127.0.0.1:8080";

static LOCAL_BRIDGE_NOTIFY_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("local bridge notification client should build")
});

fn local_bridge_base_url_from_override(value: Option<String>) -> String {
    let normalized = value.map(|value| value.trim().trim_end_matches('/').to_string());
    let is_loopback_http = normalized
        .as_deref()
        .and_then(|value| reqwest::Url::parse(value).ok())
        .is_some_and(|url| {
            url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
        });
    if is_loopback_http {
        normalized.unwrap_or_else(|| DEFAULT_LOCAL_BRIDGE_BASE_URL.to_string())
    } else {
        DEFAULT_LOCAL_BRIDGE_BASE_URL.to_string()
    }
}

fn local_bridge_base_url() -> String {
    local_bridge_base_url_from_override(std::env::var("ITERATE_LOCAL_BRIDGE_BASE_URL").ok())
}

fn emit_interaction_phase(
    request: &DialogRequest,
    phase: InteractionPhase,
    serve_request_id: &str,
) {
    if let Some(tx) = &request.lifecycle_tx {
        let _ = tx.send(InteractionLifecycleEvent::new(
            phase,
            Some(serve_request_id.to_string()),
        ));
    }
}

fn is_macos_app_bundle_executable(path: &Path) -> bool {
    path.to_string_lossy().contains(".app/Contents/MacOS/")
}

fn installed_macos_iterate_executable() -> PathBuf {
    PathBuf::from("/Applications/iterate.app/Contents/MacOS/iterate")
}

fn resolve_dialog_gui_executable_path(
    current_exe: PathBuf,
    override_path: Option<PathBuf>,
    installed_exe: PathBuf,
) -> PathBuf {
    if let Some(path) = override_path {
        if path.is_file() {
            return path;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if !is_macos_app_bundle_executable(&current_exe) {
            if installed_exe.is_file() {
                return installed_exe;
            }
        }
    }

    current_exe
}

fn dialog_gui_executable_path() -> PathBuf {
    resolve_dialog_gui_executable_path(
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("iterate")),
        std::env::var_os("ITERATE_DIALOG_GUI_EXECUTABLE").map(PathBuf::from),
        installed_macos_iterate_executable(),
    )
}

fn dialog_gui_ready_timeout() -> std::time::Duration {
    const DEFAULT_READY_TIMEOUT_MS: u64 = 60_000;

    std::env::var("ITERATE_DIALOG_GUI_READY_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(std::time::Duration::from_millis)
        .unwrap_or_else(|| std::time::Duration::from_millis(DEFAULT_READY_TIMEOUT_MS))
}

fn sanitize_request_id_for_filename(request_id: &str) -> String {
    request_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn serve_response_route_file(request_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "iterate_response_route_{}.json",
        sanitize_request_id_for_filename(request_id)
    ))
}

fn register_serve_response_route(
    request_id: &str,
    request: &DialogRequest,
    response_file: &Path,
) -> PathBuf {
    let route_file = serve_response_route_file(request_id);
    let payload = serde_json::json!({
        "request_id": request_id,
        "project_path": request.workspace,
        "response_file": response_file.to_string_lossy(),
        "created_at": chrono::Utc::now().timestamp(),
    });

    match std::fs::write(
        &route_file,
        serde_json::to_string(&payload).unwrap_or_default(),
    ) {
        Ok(()) => instance_debug_log(
            "[serve-response-route-registered]",
            format!(
                "request_id={}, route_file={}, response_file={}",
                request_id,
                route_file.display(),
                response_file.display()
            ),
        ),
        Err(err) => instance_debug_log(
            "[serve-response-route-register-failed]",
            format!(
                "request_id={}, route_file={}, error={}",
                request_id,
                route_file.display(),
                err
            ),
        ),
    }

    route_file
}

fn notify_bridge_apns_on_popup_ready(request_id: String, request: &DialogRequest) {
    let payload = serde_json::json!({
        "title": "iterate",
        "body": request.message.clone(),
        "project_path": if request.workspace.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(request.workspace.clone())
        },
        "request_id": request_id,
        "predefined_options": request.options.clone(),
        "is_markdown": request.is_markdown,
        "codex_thread_id": request.codex_thread_id,
        "codex_deeplink": request.codex_deeplink,
        "conversation_title": request.conversation_title,
        "loop_active": request.loop_active,
        "force_popup": request.force_popup,
        "source": "desktop_popup_ready",
    });

    tokio::spawn(async move {
        let notify_url = format!("{}/api/apns/notify", local_bridge_base_url());
        let notify_started_at = std::time::Instant::now();
        let request_id_for_log = payload
            .get("request_id")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string();
        let retry_delays = [
            Duration::from_millis(0),
            Duration::from_millis(200),
            Duration::from_millis(600),
            Duration::from_millis(1200),
        ];

        for (attempt_index, delay) in retry_delays.iter().enumerate() {
            if !delay.is_zero() {
                sleep(*delay).await;
            }

            let attempt = attempt_index + 1;
            let request = LOCAL_BRIDGE_NOTIFY_CLIENT.post(&notify_url).json(&payload);
            let request = match crate::bridge::auth::authorize_internal_bridge_request(
                request,
                "POST",
                &notify_url,
            ) {
                Ok(request) => request,
                Err(error) => {
                    log::warn!("[APNs] Bridge 内部鉴权签发失败: {}", error);
                    instance_debug_log(
                        "[serve-request-ready-apns-retry]",
                        format!(
                            "request_id={}, attempt={}, auth_error={}, elapsed_ms={}",
                            request_id_for_log,
                            attempt,
                            error,
                            notify_started_at.elapsed().as_millis()
                        ),
                    );
                    continue;
                }
            };
            let result = request.send().await;

            match result {
                Ok(response) if response.status().is_success() => {
                    instance_debug_log(
                        "[serve-request-ready-apns]",
                        format!(
                            "request_id={}, attempt={}, status={}, elapsed_ms={}",
                            request_id_for_log,
                            attempt,
                            response.status(),
                            notify_started_at.elapsed().as_millis()
                        ),
                    );
                    return;
                }
                Ok(response) => {
                    instance_debug_log(
                        "[serve-request-ready-apns-retry]",
                        format!(
                            "request_id={}, attempt={}, status={}, elapsed_ms={}",
                            request_id_for_log,
                            attempt,
                            response.status(),
                            notify_started_at.elapsed().as_millis()
                        ),
                    );
                }
                Err(err) => {
                    instance_debug_log(
                        "[serve-request-ready-apns-retry]",
                        format!(
                            "request_id={}, attempt={}, error={}, elapsed_ms={}",
                            request_id_for_log,
                            attempt,
                            err,
                            notify_started_at.elapsed().as_millis()
                        ),
                    );
                }
            }
        }

        instance_debug_log(
            "[serve-request-ready-apns-failed]",
            format!(
                "request_id={}, attempts={}, elapsed_ms={}",
                request_id_for_log,
                retry_delays.len(),
                notify_started_at.elapsed().as_millis()
            ),
        );
    });
}

async fn wait_child_exit_with_timeout(
    child: &mut std::process::Child,
    request_id: &str,
    child_pid: u32,
    reason: &str,
) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                instance_debug_log(
                    "[serve-request-child-reaped]",
                    format!(
                        "request_id={}, child_pid={}, reason={}, status={:?}",
                        request_id, child_pid, reason, status
                    ),
                );
                return Some(status);
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                sleep(Duration::from_millis(50)).await;
            }
            Ok(None) => {
                instance_debug_log(
                    "[serve-request-child-reap-timeout]",
                    format!(
                        "request_id={}, child_pid={}, reason={}, timeout_ms=2000",
                        request_id, child_pid, reason
                    ),
                );
                return None;
            }
            Err(err) => {
                instance_debug_log(
                    "[serve-request-child-reap-failed]",
                    format!(
                        "request_id={}, child_pid={}, reason={}, error={}",
                        request_id, child_pid, reason, err
                    ),
                );
                return None;
            }
        }
    }
}

async fn kill_child_best_effort(
    child: &mut std::process::Child,
    request_id: &str,
    reason: &str,
) -> Option<std::process::ExitStatus> {
    let child_pid = child.id();
    match child.kill() {
        Ok(()) => instance_debug_log(
            "[serve-request-child-kill-ok]",
            format!(
                "request_id={}, child_pid={}, reason={}",
                request_id, child_pid, reason
            ),
        ),
        Err(err) => instance_debug_log(
            "[serve-request-child-kill-skipped]",
            format!(
                "request_id={}, child_pid={}, reason={}, error={}",
                request_id, child_pid, reason, err
            ),
        ),
    }

    wait_child_exit_with_timeout(child, request_id, child_pid, reason).await
}

/// 将 base64 图片保存为文件，返回文件路径列表
fn save_images_to_files(images: &[serde_json::Value]) -> Vec<String> {
    let images_dir = dirs::home_dir()
        .map(|h| h.join(".cunzhi").join("images"))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/cunzhi_images"));

    if std::fs::create_dir_all(&images_dir).is_err() {
        return vec![];
    }

    let mut saved_paths = Vec::new();
    let timestamp = chrono::Utc::now().timestamp_millis();

    for (i, img) in images.iter().enumerate() {
        let data = img.get("data").and_then(|v| v.as_str()).unwrap_or("");
        let media_type = img
            .get("media_type")
            .and_then(|v| v.as_str())
            .unwrap_or("image/png");

        // 确定文件扩展名
        let ext = if media_type.contains("jpeg") || media_type.contains("jpg") {
            "jpg"
        } else if media_type.contains("gif") {
            "gif"
        } else if media_type.contains("webp") {
            "webp"
        } else {
            "png"
        };

        // 处理 data URL 格式
        let base64_data = if data.starts_with("data:") {
            data.split(',').nth(1).unwrap_or(data)
        } else {
            data
        };

        // 解码并保存
        if let Ok(bytes) = BASE64.decode(base64_data) {
            let filename = format!("image_{}_{}.{}", timestamp, i, ext);
            let filepath = images_dir.join(&filename);
            if std::fs::write(&filepath, bytes).is_ok() {
                saved_paths.push(filepath.to_string_lossy().to_string());
            }
        }
    }

    saved_paths
}

fn is_goal_response_source(source: &str) -> bool {
    let normalized = source.trim().to_ascii_lowercase();
    normalized.contains("goal_submit")
        || normalized.contains("goal_start")
        || normalized.contains("goalrun")
}

fn format_attachment_path_block(label: &str, paths: &[String]) -> Option<String> {
    let lines: Vec<&str> = paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .collect();

    if lines.is_empty() {
        return None;
    }

    Some(format!(
        "{}：\n{}",
        label,
        lines
            .into_iter()
            .map(|path| format!("- {}", path))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn paths_missing_from_input(user_input: &str, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty() && !user_input.contains(*path))
        .map(ToString::to_string)
        .collect()
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

fn strip_goal_image_reference_context(user_input: &str) -> String {
    let lines: Vec<&str> = user_input.lines().collect();
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

fn inject_goal_attachment_paths(
    user_input: &str,
    file_paths: &[String],
    image_paths: &[String],
) -> String {
    let cleaned_user_input = strip_goal_image_reference_context(user_input);
    let mut blocks = Vec::new();

    let missing_file_paths = paths_missing_from_input(&cleaned_user_input, file_paths);
    if let Some(file_block) = format_attachment_path_block("附加文件路径", &missing_file_paths)
    {
        blocks.push(file_block);
    }

    let missing_image_paths = paths_missing_from_input(&cleaned_user_input, image_paths);
    if let Some(image_block) = format_attachment_path_block("附加图片路径", &missing_image_paths)
    {
        blocks.push(image_block);
    }

    if blocks.is_empty() {
        return cleaned_user_input;
    }

    let attachment_text = blocks.join("\n\n");
    if let Some(close_index) = cleaned_user_input.rfind('》') {
        let (before, after) = cleaned_user_input.split_at(close_index);
        format!("{}\n\n{}{}", before.trim_end(), attachment_text, after)
    } else {
        format!("{}\n\n{}", cleaned_user_input.trim_end(), attachment_text)
    }
}

fn prepend_selected_options_to_user_input(user_input: &str, selected_options: &[String]) -> String {
    let missing_options = selected_options
        .iter()
        .map(|option| option.trim())
        .filter(|option| !option.is_empty() && !user_input.contains(option))
        .collect::<Vec<_>>();

    if missing_options.is_empty() {
        return user_input.to_string();
    }

    let prefix = format!("选中的选项: {}", missing_options.join(" / "));
    if user_input.trim().is_empty() {
        prefix
    } else {
        format!("{}\n\n{}", prefix, user_input)
    }
}

fn enrich_goal_user_input_with_attachment_paths(
    user_input: String,
    response_source: &str,
    file_paths: &[String],
    image_paths: &[String],
) -> String {
    if user_input.trim().is_empty()
        || !is_goal_response_source(response_source)
        || (file_paths.is_empty() && image_paths.is_empty())
    {
        return user_input;
    }

    inject_goal_attachment_paths(&user_input, file_paths, image_paths)
}

/// 解析命令行参数为键值对
fn parse_args() -> (Vec<String>, HashMap<String, String>) {
    let args: Vec<String> = std::env::args().collect();
    let mut flags = Vec::new();
    let mut options = HashMap::new();

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with("--") {
            let key = arg.clone();
            // 检查是否是带值的参数
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                options.insert(key, args[i + 1].clone());
                i += 2;
            } else {
                flags.push(key);
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    (flags, options)
}

/// 处理命令行参数
pub fn handle_early_cli_args() -> bool {
    let args: Vec<String> = std::env::args().collect();

    if args.len() != 2 {
        return false;
    }

    match args[1].as_str() {
        "--help" | "-h" => {
            print_help();
            true
        }
        "--version" | "-v" => {
            print_version();
            true
        }
        "--activation-gate-status" => {
            println!(
                "activation_gate_required={}",
                crate::ui::commands::activation_gate_required_for_current_build(false)
            );
            true
        }
        _ => false,
    }
}

/// 处理命令行参数
pub fn handle_cli_args() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // A no-argument launch is the only Windows entry point allowed to clear the
    // manual-stop marker.  Keep the guard here as well as in mcp-server so an
    // older host process cannot bypass the marker by spawning a newer iterate
    // executable directly.
    #[cfg(target_os = "windows")]
    if blocks_manual_stop_cli_mode(&args) && crate::app::windows_lifecycle::is_manually_stopped() {
        anyhow::bail!(crate::app::windows_lifecycle::MANUALLY_STOPPED_MESSAGE);
    }

    if args
        .get(1)
        .is_some_and(|arg| arg == "--bridge-room-submit-token")
    {
        if args.len() != 2 {
            anyhow::bail!("room submit token helper accepts the body digest on stdin only");
        }
        let mut input = String::new();
        std::io::stdin().take(129).read_to_string(&mut input)?;
        let digest = input.trim();
        if digest.len() != 64 {
            anyhow::bail!("invalid room submit body digest");
        }
        let token = crate::bridge::auth::issue_internal_room_submit_token(digest)
            .map_err(anyhow::Error::msg)?;
        write_stdout_lossy(&format!("{token}\n"));
        return Ok(());
    }

    if args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--speech-paste-helper" | "--speech-paste-helper-dry-run"
        )
    }) {
        if args.len() != 2 {
            anyhow::bail!("speech paste helper accepts no payload arguments");
        }
        let dry_run = args[1] == "--speech-paste-helper-dry-run";
        return crate::native_speech::external_sender::run_paste_helper_stdio(dry_run)
            .map_err(anyhow::Error::msg);
    }

    let (flags, options) = parse_args();

    // 检查是否是 --ui 模式（兼容旧版 --ui）
    if flags.contains(&"--ui".to_string()) {
        return handle_ui_mode(&options);
    }

    // 检查生产包是否包含 Tauri 前端入口资源，不打开窗口。
    if flags.contains(&"--check-frontend-assets".to_string()) {
        if let Some(dist_dir) = options.get("--frontend-dist") {
            let verified_count =
                check_frontend_assets_for_dist(Path::new(dist_dir)).map_err(anyhow::Error::msg)?;
            println!("frontend assets ok ({verified_count} dist assets verified)");
        } else {
            check_frontend_assets().map_err(anyhow::Error::msg)?;
            println!("frontend assets ok");
        }
        return Ok(());
    }

    // Bridge daemon 通过 LaunchServices 启动一个明确的主 GUI 实例。
    // 该参数与 standalone MCP 弹窗分离，避免同 bundle 的 zhi 窗口被误激活。
    if flags.contains(&"--show-main-window".to_string()) {
        run_tauri_app();
        return Ok(());
    }

    if flags.contains(&"--cloudflare-auto-setup-smoke".to_string()) {
        return handle_cloudflare_auto_setup_smoke(&flags, &options);
    }

    if flags.contains(&"--mobile-route-status".to_string()) {
        return handle_mobile_route_status(false);
    }

    if flags.contains(&"--mobile-route-verify".to_string()) {
        return handle_mobile_route_status(true);
    }

    if flags.contains(&"--mobile-route-register".to_string()) {
        return handle_mobile_route_register(&flags, &options);
    }

    if flags.contains(&"--relay-server".to_string()) {
        return handle_relay_server_mode(&options);
    }

    if flags.contains(&"--relay-mac-client".to_string()) {
        return handle_relay_mac_client_mode(&flags, &options);
    }

    // 检查是否是 --serve 模式（HTTP 服务器模式，类似 Infinite WF）
    if flags.contains(&"--serve".to_string()) {
        let port = options
            .get("--port")
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or_else(|| server::find_available_port(5310));
        let workspace = options.get("--workspace").cloned();
        return handle_serve_mode(port, workspace);
    }

    // 检查是否是 --bridge-only 模式（无 GUI 的 8080 mobile bridge origin）
    if flags.contains(&"--bridge-only".to_string()) {
        let port = options
            .get("--port")
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(8080);
        return handle_bridge_only_mode(port);
    }

    // 检查是否是 --bridge 模式（替代 cunzhi.py，直接与 --serve 服务器通信）
    if flags.contains(&"--bridge".to_string()) {
        let port = options.get("--port").and_then(|p| p.parse::<u16>().ok());
        let workspace = options.get("--workspace").cloned();
        let message_file = options.get("--message-file").cloned();
        return handle_bridge_mode(port, workspace, message_file);
    }

    match args.len() {
        // 无参数：正常启动GUI
        1 => {
            run_tauri_app();
        }
        // 单参数：帮助或版本
        2 => match args[1].as_str() {
            "--help" | "-h" => print_help(),
            "--version" | "-v" => print_version(),
            _ => {
                eprintln!("未知参数: {}", args[1]);
                print_help();
                std::process::exit(1);
            }
        },
        // 多参数：MCP请求模式
        _ => {
            if args[1] == "--mcp-request" && args.len() >= 3 {
                handle_mcp_request(&args[2])?;
            } else {
                eprintln!("无效的命令行参数");
                print_help();
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn blocks_manual_stop_cli_mode(args: &[String]) -> bool {
    args.iter().skip(1).any(|arg| {
        matches!(
            arg.as_str(),
            "--ui" | "--show-main-window" | "--serve" | "--bridge-only" | "--mcp-request"
        )
    })
}

fn required_option(options: &HashMap<String, String>, key: &str) -> Result<String> {
    options
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("缺少必需参数: {}", key))
}

fn parse_bool_cli_value(value: &str, key: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" => Ok(true),
        "0" | "false" | "no" | "n" => Ok(false),
        _ => Err(anyhow::anyhow!("参数 {} 只接受 true/false/1/0/yes/no", key)),
    }
}

fn bool_option_or_flag(
    flags: &[String],
    options: &HashMap<String, String>,
    key: &str,
    default_value: bool,
) -> Result<bool> {
    if let Some(value) = options.get(key) {
        return parse_bool_cli_value(value, key);
    }
    if flags.iter().any(|flag| flag == key) {
        return Ok(true);
    }
    Ok(default_value)
}

fn optional_token_from_env(options: &HashMap<String, String>, key: &str) -> Result<Option<String>> {
    let Some(env_name) = options.get(key) else {
        return Ok(None);
    };
    let env_name = env_name.trim();
    if env_name.is_empty() {
        return Err(anyhow::anyhow!("参数 {} 不能为空", key));
    }
    let token = std::env::var(env_name)
        .map_err(|_| anyhow::anyhow!("环境变量 {} 未设置", env_name))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(anyhow::anyhow!("环境变量 {} 为空", env_name));
    }
    Ok(Some(token))
}

fn relay_audit_log_path(options: &HashMap<String, String>) -> Option<PathBuf> {
    let configured = options
        .get("--relay-audit-log")
        .cloned()
        .or_else(|| std::env::var("ITERATE_RELAY_AUDIT_LOG").ok());
    if let Some(value) = configured {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("off")
            || trimmed.eq_ignore_ascii_case("none")
        {
            return None;
        }
        return Some(PathBuf::from(trimmed));
    }

    dirs::home_dir().map(|home| {
        home.join("Library")
            .join("Logs")
            .join("iterate")
            .join("relay-audit.jsonl")
    })
}

fn handle_relay_server_mode(options: &HashMap<String, String>) -> Result<()> {
    let host = options
        .get("--host")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = options
        .get("--port")
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8790);
    let token = optional_token_from_env(options, "--relay-token-env")?;
    let audit_log_path = relay_audit_log_path(options);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(crate::relay::start_relay_server(
        crate::relay::RelayServerConfig {
            host,
            port,
            token,
            audit_log_path,
        },
    ))
}

fn handle_relay_mac_client_mode(flags: &[String], options: &HashMap<String, String>) -> Result<()> {
    let relay_url = required_option(options, "--relay-url")?;
    let device_id = options
        .get("--device-id")
        .cloned()
        .unwrap_or_else(|| "local-mac".to_string());
    let local_base_url = options
        .get("--local-base-url")
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());
    let heartbeat_secs = options
        .get("--heartbeat-secs")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(15);
    let allow_recover = bool_option_or_flag(flags, options, "--allow-recover", false)?;
    let token = optional_token_from_env(options, "--relay-token-env")?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(crate::relay::start_relay_mac_client(
        crate::relay::RelayMacClientConfig {
            relay_url,
            device_id,
            token,
            local_base_url,
            heartbeat_secs,
            allow_recover,
        },
    ))
}

fn handle_cloudflare_auto_setup_smoke(
    flags: &[String],
    options: &HashMap<String, String>,
) -> Result<()> {
    if options.contains_key("--api-token") || flags.iter().any(|flag| flag == "--api-token") {
        return Err(anyhow::anyhow!(
            "出于安全原因不支持 --api-token 明文参数，请使用 --api-token-env"
        ));
    }

    let api_token_env = required_option(options, "--api-token-env")?;
    let api_token = std::env::var(&api_token_env)
        .map(|value| value.trim().to_string())
        .map_err(|_| anyhow::anyhow!("环境变量 {} 未设置", api_token_env))?;
    if api_token.is_empty() {
        return Err(anyhow::anyhow!("环境变量 {} 为空", api_token_env));
    }

    let zone_name = required_option(options, "--zone")?;
    let subdomain = required_option(options, "--subdomain")?;
    let overwrite_dns = bool_option_or_flag(flags, options, "--overwrite-dns", false)?;
    let access_emails = options
        .get("--access-email")
        .map(|value| {
            value
                .split(',')
                .map(|email| email.trim().to_string())
                .filter(|email| !email.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let request = crate::tunnel::commands::CloudflareWebLoginAutoSetupCoreRequest {
        api_token,
        zone_name,
        subdomain,
        overwrite_dns,
        access_emails,
    };

    let rt = tokio::runtime::Runtime::new()?;
    let response = rt
        .block_on(crate::tunnel::commands::create_cloudflare_web_login_auto_setup_headless(request))
        .map_err(anyhow::Error::msg)?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    if !response.ok {
        std::process::exit(2);
    }
    Ok(())
}

fn handle_mobile_route_status(verify: bool) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let status = if verify {
        rt.block_on(crate::tunnel::commands::verify_formal_mobile_route())
            .map_err(anyhow::Error::msg)?
    } else {
        rt.block_on(crate::tunnel::commands::get_formal_mobile_route_status())
    };
    write_stdout_lossy(&format!("{}\n", serde_json::to_string_pretty(&status)?));
    if status.configured && status.health != "healthy" {
        std::process::exit(2);
    }
    Ok(())
}

fn handle_mobile_route_register(flags: &[String], options: &HashMap<String, String>) -> Result<()> {
    const ALLOWED_FLAGS: [&str; 1] = ["--mobile-route-register"];
    const ALLOWED_OPTIONS: [&str; 3] = ["--base-url", "--transport", "--source"];
    if let Some(flag) = flags
        .iter()
        .find(|flag| !ALLOWED_FLAGS.contains(&flag.as_str()))
    {
        anyhow::bail!("mobile route register rejects unexpected flag: {flag}");
    }
    if let Some(option) = options
        .keys()
        .find(|option| !ALLOWED_OPTIONS.contains(&option.as_str()))
    {
        anyhow::bail!("mobile route register rejects unexpected option: {option}");
    }
    let transport = options
        .get("--transport")
        .map(String::as_str)
        .unwrap_or("cloudflare_named_tunnel");
    let base_url = required_option(options, "--base-url")?;
    let source = options
        .get("--source")
        .map(String::as_str)
        .unwrap_or("ai_configured");
    let rt = tokio::runtime::Runtime::new()?;
    let status = rt
        .block_on(crate::tunnel::commands::register_formal_mobile_route(
            transport, &base_url, source,
        ))
        .map_err(anyhow::Error::msg)?;
    write_stdout_lossy(&format!("{}\n", serde_json::to_string_pretty(&status)?));
    Ok(())
}

/// 处理 --ui 模式（兼容旧版 --ui）
/// 始终启动独立弹窗，不转发到主进程（主进程显示的是主页，不是弹窗）
fn handle_ui_mode(options: &HashMap<String, String>) -> Result<()> {
    let message = options
        .get("--message")
        .cloned()
        .unwrap_or_else(|| "请确认是否继续？".to_string());
    let options_str = options.get("--options").cloned().unwrap_or_default();
    let workspace = options.get("--workspace").cloned();
    // 转换为绝对路径，过滤无效值
    let resolved_workspace: Option<String> = workspace
        .as_deref()
        .filter(|s| !s.is_empty() && *s != ".")
        .and_then(|s| std::fs::canonicalize(s).ok())
        .and_then(|p| p.to_str().map(String::from));

    // 解析选项
    let predefined_options: Vec<String> = if options_str.is_empty() {
        vec![]
    } else {
        options_str
            .split(',')
            .map(|s| s.trim().to_string())
            .collect()
    };

    // 创建 MCP 请求
    let request_id = format!("standalone-{}", chrono::Utc::now().timestamp_millis());
    let request = serde_json::json!({
        "id": request_id,
        "message": message,
        "predefined_options": predefined_options,
        "is_markdown": true,
        "project_path": resolved_workspace
    });

    // 写入临时文件
    let temp_dir = std::env::temp_dir();
    let request_file = temp_dir.join(format!("iterate_request_{}.json", request_id));
    std::fs::write(&request_file, serde_json::to_string_pretty(&request)?)?;

    // 设置环境变量，让 Tauri 应用读取
    std::env::set_var(
        "ITERATE_MCP_REQUEST_FILE",
        request_file.to_string_lossy().to_string(),
    );
    std::env::set_var("ITERATE_STANDALONE_MODE", "1");

    // 启动独立弹窗 GUI
    run_tauri_app();

    Ok(())
}

/// 处理MCP请求
fn handle_mcp_request(request_file: &str) -> Result<()> {
    std::env::set_var("ITERATE_MCP_REQUEST_FILE", request_file);
    std::env::set_var("ITERATE_STANDALONE_MODE", "1");

    // 检查Telegram配置，决定是否启用纯Telegram模式
    match load_standalone_telegram_config() {
        Ok(telegram_config) => {
            if telegram_config.enabled && telegram_config.hide_frontend_popup {
                // 纯Telegram模式：不启动GUI，直接处理
                if let Err(e) = tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(handle_telegram_only_mcp_request(request_file))
                {
                    log_important!(error, "处理Telegram请求失败: {}", e);
                    std::process::exit(1);
                }
            } else {
                // 正常模式：启动GUI处理弹窗
                run_tauri_app();
            }
        }
        Err(e) => {
            log_important!(warn, "加载Telegram配置失败: {}，使用默认GUI模式", e);
            // 配置加载失败时，使用默认行为（启动GUI）
            run_tauri_app();
        }
    }
    Ok(())
}

/// 处理 --serve 模式（HTTP 服务器模式）
fn handle_serve_mode(port: u16, workspace: Option<String>) -> Result<()> {
    #[cfg(target_os = "windows")]
    if crate::app::windows_lifecycle::is_manually_stopped() {
        anyhow::bail!(crate::app::windows_lifecycle::MANUALLY_STOPPED_MESSAGE);
    }

    // 禁用日志输出到终端（--serve 模式下静默运行）
    log::set_max_level(log::LevelFilter::Off);
    let _instance_guard =
        crate::app::windows_lifecycle::register_current_instance("serve", Some(port))?;

    instance_debug_log(
        "[serve-mode-start]",
        format!(
            "port={}, workspace={:?}, pid={}, current_exe={:?}, cwd={:?}",
            port,
            workspace,
            std::process::id(),
            std::env::current_exe().ok(),
            std::env::current_dir().ok()
        ),
    );
    eprintln!("Starting cunzhi HTTP server on port {}...", port);

    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(async {
        let (request_tx, mut request_rx) = mpsc::channel::<DialogRequest>(32);

        // 启动 HTTP 服务器
        let server_port = port;
        let server_workspace = workspace.clone();
        let mut server_handle = Some(tokio::spawn(async move {
            if let Err(e) = server::start_server(server_port, request_tx, server_workspace).await {
                instance_debug_log(
                    "[serve-server-task-error]",
                    format!("port={}, error={}", server_port, e),
                );
                eprintln!("Server error: {}", e);
            }
        }));

        // 等待服务器真正可用，而不是只等固定时长就宣告 ready。
        let startup_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        let startup_started = tokio::time::Instant::now();
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            if check_server_health(port) {
                instance_debug_log(
                    "[serve-health-ready]",
                    format!(
                        "port={}, workspace={:?}, attempts={}, elapsed_ms={}",
                        port,
                        workspace,
                        attempts,
                        startup_started.elapsed().as_millis()
                    ),
                );
                break;
            }

            if server_handle
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            {
                let handle = server_handle.take().expect("server handle available");
                let _ = handle.await;
                instance_debug_log(
                    "[serve-task-exited-before-health]",
                    format!(
                        "port={}, workspace={:?}, attempts={}, elapsed_ms={}",
                        port,
                        workspace,
                        attempts,
                        startup_started.elapsed().as_millis()
                    ),
                );
                Err(anyhow::anyhow!(
                    "HTTP server on port {} exited before becoming healthy",
                    port
                ))?;
            }

            if tokio::time::Instant::now() >= startup_deadline {
                if let Some(handle) = server_handle.take() {
                    handle.abort();
                    let _ = handle.await;
                }
                instance_debug_log(
                    "[serve-health-timeout]",
                    format!(
                        "port={}, workspace={:?}, attempts={}, elapsed_ms={}",
                        port,
                        workspace,
                        attempts,
                        startup_started.elapsed().as_millis()
                    ),
                );
                Err(anyhow::anyhow!(
                    "HTTP server on port {} did not become healthy within 10s",
                    port
                ))?;
            }

            if attempts == 1 || attempts % 5 == 0 {
                instance_debug_log(
                    "[serve-health-wait]",
                    format!(
                        "port={}, workspace={:?}, attempts={}, elapsed_ms={}",
                        port,
                        workspace,
                        attempts,
                        startup_started.elapsed().as_millis()
                    ),
                );
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }

        println!("Server ready! Listening on http://127.0.0.1:{}", port);
        println!("Use: python3 cunzhi.py {} --message \"Your message\"", port);

        let shutdown_wait = crate::app::windows_lifecycle::wait_for_global_shutdown();
        tokio::pin!(shutdown_wait);

        // 处理请求队列
        loop {
            tokio::select! {
                Some(mut request) = request_rx.recv() => {
                    if let Some(response_tx) = request.response_tx.take() {
                        // 启动 GUI 处理这个请求
                        let response = handle_dialog_request(&request).await;
                        let _ = response_tx.send(response);
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    instance_debug_log(
                        "[serve-ctrl-c]",
                        format!("port={}, workspace={:?}", port, workspace),
                    );
                    println!("\nShutting down server...");
                    break;
                }
                _ = &mut shutdown_wait => {
                    instance_debug_log(
                        "[serve-global-shutdown]",
                        format!("port={}, workspace={:?}", port, workspace),
                    );
                    break;
                }
            }
        }

        if let Some(handle) = server_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        instance_debug_log(
            "[serve-mode-stop]",
            format!("port={}, workspace={:?}", port, workspace),
        );
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}

/// 处理 --bridge-only 模式（无 GUI 的 mobile bridge origin）
fn handle_bridge_only_mode(port: u16) -> Result<()> {
    #[cfg(target_os = "windows")]
    if crate::app::windows_lifecycle::is_manually_stopped() {
        anyhow::bail!(crate::app::windows_lifecycle::MANUALLY_STOPPED_MESSAGE);
    }

    log::set_max_level(log::LevelFilter::Off);
    let _instance_guard =
        crate::app::windows_lifecycle::register_current_instance("bridge-only", Some(port))?;
    instance_debug_log(
        "[bridge-only-mode-start]",
        format!(
            "port={}, pid={}, current_exe={:?}, cwd={:?}",
            port,
            std::process::id(),
            std::env::current_exe().ok(),
            std::env::current_dir().ok()
        ),
    );
    eprintln!("Starting iterate bridge-only daemon on port {}...", port);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        tokio::select! {
            result = crate::bridge::start_bridge_daemon(port) => {
                result.map_err(|error| anyhow::anyhow!("{}", error))
            }
            _ = crate::app::windows_lifecycle::wait_for_global_shutdown() => Ok(()),
        }
    })?;
    Ok(())
}

fn clean_dialog_dismissal_response(
    popup_ready: bool,
    response_present: bool,
    child_exited_successfully: bool,
) -> Option<DialogResponse> {
    (popup_ready && !response_present && child_exited_successfully).then(|| DialogResponse {
        keep_going: false,
        response_source: crate::conversation::POPUP_CLOSED_SOURCE.to_string(),
        error: None,
        ..Default::default()
    })
}

/// 处理单个对话请求（启动 GUI）
async fn handle_dialog_request(request: &DialogRequest) -> DialogResponse {
    // 创建临时请求文件
    let request_id = format!("serve-{}", chrono::Utc::now().timestamp_millis());
    let parent_request_id = request.request_id.clone();
    emit_interaction_phase(request, InteractionPhase::StartingGui, &request_id);
    instance_debug_log(
        "[serve-request-begin]",
        format!(
            "request_id={}, parent_request_id={}, workspace={:?}, options_len={}, loop_active={}, force_popup={}, message_len={}",
            request_id,
            parent_request_id,
            request.workspace,
            request.options.len(),
            request.loop_active,
            request.force_popup,
            request.message.len()
        ),
    );
    let mcp_request = serde_json::json!({
        "id": request_id,
        "parent_request_id": if parent_request_id.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(parent_request_id.clone()) },
        "message": request.message,
        "predefined_options": request.options,
        "is_markdown": request.is_markdown,
        "project_path": if request.workspace.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(request.workspace.clone()) },
        "codex_home": request.codex_home,
        "codex_thread_id": request.codex_thread_id,
        "codex_deeplink": request.codex_deeplink,
        "conversation_title": request.conversation_title,
        "checkpoint_id": request.checkpoint_id,
        "checkpoint_commit": request.checkpoint_commit,
        "checkpoint_message": request.checkpoint_message,
        "loop_active": request.loop_active,
        "force_popup": request.force_popup
    });

    let temp_dir = std::env::temp_dir();
    let request_file = temp_dir.join(format!("iterate_request_{}.json", request_id));
    let response_file = temp_dir.join(format!("iterate_response_{}.json", request_id));
    let ready_file = temp_dir.join(format!("iterate_ready_{}.json", request_id));

    if let Err(e) = std::fs::write(
        &request_file,
        serde_json::to_string_pretty(&mcp_request).unwrap_or_default(),
    ) {
        instance_debug_log(
            "[serve-request-write-failed]",
            format!("request_id={}, error={}", request_id, e),
        );
        return DialogResponse {
            keep_going: false,
            error: Some(format!("Failed to write request file: {}", e)),
            ..Default::default()
        };
    }
    let response_route_file = register_serve_response_route(&request_id, request, &response_file);

    // 设置环境变量
    std::env::set_var(
        "ITERATE_MCP_REQUEST_FILE",
        request_file.to_string_lossy().to_string(),
    );
    std::env::set_var(
        "ITERATE_RESPONSE_FILE",
        response_file.to_string_lossy().to_string(),
    );
    std::env::set_var(
        "ITERATE_READY_FILE",
        ready_file.to_string_lossy().to_string(),
    );
    std::env::set_var("ITERATE_STANDALONE_MODE", "1");

    let cleanup_temp_files = || {
        let _ = std::fs::remove_file(&ready_file);
        let _ = std::fs::remove_file(&response_file);
        let _ = std::fs::remove_file(&request_file);
        let _ = std::fs::remove_file(&response_route_file);
    };

    // 启动 GUI 进程（子进程），重定向 stdout/stderr 避免日志污染终端
    let exe_path = dialog_gui_executable_path();
    instance_debug_log(
        "[serve-request-spawn-begin]",
        format!(
            "request_id={}, parent_request_id={}, exe_path={}, current_exe={:?}, request_file={}, response_file={}",
            request_id,
            parent_request_id,
            exe_path.display(),
            std::env::current_exe().ok(),
            request_file.display(),
            response_file.display()
        ),
    );
    let child = std::process::Command::new(&exe_path)
        .env(
            "ITERATE_MCP_REQUEST_FILE",
            request_file.to_string_lossy().to_string(),
        )
        .env(
            "ITERATE_RESPONSE_FILE",
            response_file.to_string_lossy().to_string(),
        )
        .env(
            "ITERATE_READY_FILE",
            ready_file.to_string_lossy().to_string(),
        )
        .env("ITERATE_STANDALONE_MODE", "1")
        .env("RUST_LOG", "off") // 禁用子进程日志
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match child {
        Ok(mut child) => {
            instance_debug_log(
                "[serve-request-spawn-success]",
                format!(
                    "request_id={}, parent_request_id={}, child_pid={}",
                    request_id,
                    parent_request_id,
                    child.id()
                ),
            );
            let ready_timeout = dialog_gui_ready_timeout();
            let startup_deadline = std::time::Instant::now() + ready_timeout;
            let mut wait_result = None;

            loop {
                if ready_file.exists() || response_file.exists() {
                    let ready_file_exists = ready_file.exists();
                    let response_file_exists = response_file.exists();
                    instance_debug_log(
                        "[serve-request-ready-observed]",
                        format!(
                            "request_id={}, child_pid={}, ready_file_exists={}, response_file_exists={}",
                            request_id,
                            child.id(),
                            ready_file_exists,
                            response_file_exists
                        ),
                    );
                    if ready_file_exists && !response_file_exists {
                        emit_interaction_phase(request, InteractionPhase::WaitingUser, &request_id);
                        notify_bridge_apns_on_popup_ready(request_id.clone(), request);
                    } else if response_file_exists {
                        emit_interaction_phase(request, InteractionPhase::Responded, &request_id);
                    }
                    break;
                }

                match child.try_wait() {
                    Ok(Some(status)) => {
                        wait_result = Some(Ok(status));
                        instance_debug_log(
                            "[serve-request-child-exit-before-ready]",
                            format!(
                                "request_id={}, child_pid={}, status={:?}",
                                request_id,
                                child.id(),
                                status
                            ),
                        );
                        break;
                    }
                    Ok(None) => {
                        if std::time::Instant::now() >= startup_deadline {
                            instance_debug_log(
                                "[serve-request-ready-timeout]",
                                format!(
                                    "request_id={}, child_pid={}, timeout_ms={}, ready_file={}, response_file={}",
                                    request_id,
                                    child.id(),
                                    ready_timeout.as_millis(),
                                    ready_file.display(),
                                    response_file.display()
                                ),
                            );
                            let child_pid = child.id();
                            emit_interaction_phase(request, InteractionPhase::Failed, &request_id);
                            let reap_result =
                                kill_child_best_effort(&mut child, &request_id, "ready_timeout")
                                    .await;
                            instance_debug_log(
                                "[serve-request-ready-timeout-reaped]",
                                format!(
                                    "request_id={}, child_pid={}, reap_result={:?}",
                                    request_id, child_pid, reap_result
                                ),
                            );
                            cleanup_temp_files();
                            return DialogResponse {
                                keep_going: false,
                                error: Some("GUI failed to become ready".to_string()),
                                ..Default::default()
                            };
                        }
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                    Err(err) => {
                        instance_debug_log(
                            "[serve-request-try-wait-failed]",
                            format!(
                                "request_id={}, child_pid={}, error={}",
                                request_id,
                                child.id(),
                                err
                            ),
                        );
                        cleanup_temp_files();
                        emit_interaction_phase(request, InteractionPhase::Failed, &request_id);
                        return DialogResponse {
                            keep_going: false,
                            error: Some(format!("Failed to monitor GUI process: {}", err)),
                            ..Default::default()
                        };
                    }
                }
            }

            let wait_result = match wait_result {
                Some(result) => result,
                None => loop {
                    if response_file.exists() {
                        emit_interaction_phase(request, InteractionPhase::Responded, &request_id);
                        instance_debug_log(
                            "[serve-request-response-file-observed]",
                            format!("request_id={}, child_pid={}", request_id, child.id()),
                        );
                        break match kill_child_best_effort(
                            &mut child,
                            &request_id,
                            "response_file_observed",
                        )
                        .await
                        {
                            Some(status) => Ok(status),
                            None => Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "GUI child did not exit after kill",
                            )),
                        };
                    }

                    match child.try_wait() {
                        Ok(Some(status)) => break Ok(status),
                        Ok(None) => {
                            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                        }
                        Err(err) => break Err(err),
                    }
                },
            };
            instance_debug_log(
                "[serve-request-child-exit]",
                format!("request_id={}, wait_result={:?}", request_id, wait_result),
            );

            // 读取响应文件
            let response_file_exists = response_file.exists();
            let ready_file_exists = ready_file.exists();
            if response_file_exists {
                emit_interaction_phase(request, InteractionPhase::Responded, &request_id);
                if let Ok(content) = std::fs::read_to_string(&response_file) {
                    if let Ok(response) = serde_json::from_str::<serde_json::Value>(&content) {
                        instance_debug_log(
                            "[serve-request-response-loaded]",
                            format!(
                                "request_id={}, parent_request_id={}, content_len={}, response_source={:?}",
                                request_id,
                                parent_request_id,
                                content.len(),
                                response
                                    .get("metadata")
                                    .and_then(|v| v.get("source"))
                                    .and_then(|v| v.as_str())
                            ),
                        );
                        cleanup_temp_files();
                        emit_interaction_phase(request, InteractionPhase::Cleaning, &request_id);

                        let raw_user_input = response
                            .get("user_input")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let submitted_options: Vec<String> = response
                            .get("selected_options")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let submitted_source = response
                            .get("metadata")
                            .and_then(|v| v.get("source"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let explicit_end =
                            crate::conversation::is_explicit_conversation_end_response(
                                &raw_user_input,
                                &submitted_options,
                            );
                        let popup_closed =
                            crate::conversation::is_popup_closed_response_source(&submitted_source);
                        let interaction_ended = explicit_end || popup_closed;

                        // 结束当前交互时不携带选项或附件进入业务响应。
                        let image_paths = if interaction_ended {
                            vec![]
                        } else if let Some(images) =
                            response.get("images").and_then(|v| v.as_array())
                        {
                            save_images_to_files(images)
                        } else {
                            vec![]
                        };

                        let response_source = if explicit_end {
                            crate::conversation::EXPLICIT_CONVERSATION_END_SOURCE.to_string()
                        } else if popup_closed {
                            crate::conversation::POPUP_CLOSED_SOURCE.to_string()
                        } else {
                            submitted_source
                        };
                        let file_paths: Vec<String> = if interaction_ended {
                            vec![]
                        } else {
                            response
                                .get("file_paths")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default()
                        };
                        let mut metadata = response
                            .get("metadata")
                            .cloned()
                            .and_then(|value| {
                                serde_json::from_value::<crate::mcp::ResponseMetadata>(value).ok()
                            })
                            .unwrap_or_default();
                        if interaction_ended {
                            metadata.source = Some(response_source.clone());
                        }
                        let user_input = enrich_goal_user_input_with_attachment_paths(
                            raw_user_input,
                            &response_source,
                            &file_paths,
                            &image_paths,
                        );

                        return DialogResponse {
                            keep_going: !interaction_ended,
                            user_input,
                            response_source,
                            selected_options: if interaction_ended {
                                vec![]
                            } else {
                                submitted_options
                            },
                            file_paths,
                            image_paths,
                            metadata,
                            error: None,
                        };
                    }
                    instance_debug_log(
                        "[serve-request-response-parse-failed]",
                        format!(
                            "request_id={}, parent_request_id={}, content_len={}",
                            request_id,
                            parent_request_id,
                            content.len()
                        ),
                    );
                } else {
                    instance_debug_log(
                        "[serve-request-response-read-failed]",
                        format!(
                            "request_id={}, parent_request_id={}, response_file={}",
                            request_id,
                            parent_request_id,
                            response_file.display()
                        ),
                    );
                }
            } else {
                instance_debug_log(
                    "[serve-request-response-missing]",
                    format!(
                        "request_id={}, parent_request_id={}, response_file={}, ready_file_exists={}, response_route_file_exists={}",
                        request_id,
                        parent_request_id,
                        response_file.display(),
                        ready_file_exists,
                        response_route_file.exists()
                    ),
                );
            }

            let child_exited_successfully =
                wait_result.as_ref().is_ok_and(|status| status.success());
            if let Some(dismissal_response) = clean_dialog_dismissal_response(
                ready_file_exists,
                response_file_exists,
                child_exited_successfully,
            ) {
                cleanup_temp_files();
                emit_interaction_phase(request, InteractionPhase::Cleaning, &request_id);
                instance_debug_log(
                    "[serve-request-finish-dismissed]",
                    format!(
                        "request_id={}, parent_request_id={}, returning_keep_going_false",
                        request_id, parent_request_id
                    ),
                );

                return dismissal_response;
            }

            // 清理临时文件
            cleanup_temp_files();
            emit_interaction_phase(request, InteractionPhase::Failed, &request_id);
            instance_debug_log(
                "[serve-request-finish-no-response]",
                format!(
                    "request_id={}, parent_request_id={}, returning_no_response_from_gui",
                    request_id, parent_request_id
                ),
            );

            DialogResponse {
                keep_going: false,
                error: Some("No response from GUI".to_string()),
                ..Default::default()
            }
        }
        Err(e) => {
            instance_debug_log(
                "[serve-request-spawn-failed]",
                format!("request_id={}, error={}", request_id, e),
            );
            emit_interaction_phase(request, InteractionPhase::Failed, &request_id);
            DialogResponse {
                keep_going: false,
                error: Some(format!("Failed to start GUI: {}", e)),
                ..Default::default()
            }
        }
    }
}

/// 处理 --bridge 模式（替代 cunzhi.py）
/// 读取 message_file 或 output.md，发送请求到 --serve 服务器，写入 input.md
fn handle_bridge_mode(
    port: Option<u16>,
    workspace: Option<String>,
    message_file: Option<String>,
) -> Result<()> {
    use std::path::PathBuf;

    let workspace_path = workspace.unwrap_or_else(|| ".".to_string());

    // 如果没有指定端口，尝试自动检测
    // bridge 模式优先选有非空 output.md 的端口（AI 降级时一定先写了 output.md）
    let port = match port {
        Some(p) => p,
        None => {
            // 优先：按 workspace 匹配 + 有 output.md 的端口
            let discovered = find_port_for_workspace_ex(&workspace_path, true)
                // 次之：所有活跃端口中找有 output.md 的（端口可能没注册 workspace）
                .or_else(|| find_port_with_output())
                // 兜底：普通 workspace 匹配
                .or_else(|| auto_discover_port(Some(&workspace_path)));
            discovered.unwrap_or_else(|| {
                println!("KeepGoing=false");
                println!("Error: Cannot find running iterate server");
                println!("Please start the server first: iterate --serve");
                std::process::exit(1);
            })
        }
    };

    // 验证端口是否可用；不可用则自动尝试拉起一次服务器
    if !ensure_server_running(port) {
        println!("KeepGoing=false");
        println!("Error: Port {} is not available", port);
        println!("Please start the server: iterate --serve --port {}", port);
        std::process::exit(1);
    }

    // 获取数据目录
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let data_dir = home.join(".cunzhi").join(port.to_string());
    let input_file = data_dir.join("input.md");

    // 读取消息：优先从 message_file，其次从 output.md
    let message = if let Some(ref msg_file) = message_file {
        std::fs::read_to_string(msg_file).unwrap_or_else(|_| {
            eprintln!("Warning: Cannot read message file: {}", msg_file);
            "任务完成，请确认是否继续？".to_string()
        })
    } else {
        let output_file = data_dir.join("output.md");
        if output_file.exists() {
            let content = std::fs::read_to_string(&output_file).unwrap_or_default();
            if content.trim().is_empty() {
                "任务完成，请确认是否继续？".to_string()
            } else {
                content
            }
        } else {
            "任务完成，请确认是否继续？".to_string()
        }
    };

    // 自动创建 git 检查点（如果有未提交的更改）
    auto_create_checkpoint(&workspace_path);

    // 发送请求到服务器
    println!("[Waiting for user response...]");

    let request_body = serde_json::json!({
        "message": message,
        "options": [],
        "workspace": workspace_path,
        "is_markdown": true
    });

    // 发送 HTTP 请求
    match send_http_request(port, "/api/dialog", &request_body.to_string()) {
        Ok(response_str) => {
            match serde_json::from_str::<serde_json::Value>(&response_str) {
                Ok(response) => {
                    if let Some(error) = response.get("error").and_then(|v| v.as_str()) {
                        println!("KeepGoing=false");
                        println!("Error: {}", error);
                        std::process::exit(1);
                    }

                    let keep_going = response
                        .get("keep_going")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    if keep_going {
                        // 清空 output.md（如果使用了 output.md）
                        if message_file.is_none() {
                            let output_file = data_dir.join("output.md");
                            let _ = std::fs::write(&output_file, "");
                        }

                        // 获取用户输入
                        let user_input = response
                            .get("user_input")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let response_source = response
                            .get("response_source")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let selected_options: Vec<String> = response
                            .get("selected_options")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let file_paths: Vec<String> = response
                            .get("file_paths")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let image_paths: Vec<String> = response
                            .get("image_paths")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let project_path = response
                            .get("project_path")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        // 写入 input.md（与 cunzhi.py 格式一致）
                        let mut input_content = String::new();

                        // 图片路径置顶
                        if !image_paths.is_empty() {
                            input_content
                                .push_str(&format!("image_paths: {}\n\n", image_paths.join(",")));
                        }

                        let display_user_input = if is_goal_response_source(response_source) {
                            user_input.to_string()
                        } else {
                            prepend_selected_options_to_user_input(user_input, &selected_options)
                        };

                        // 用户输入
                        input_content.push_str(&display_user_input);

                        // 文件路径
                        if !file_paths.is_empty() {
                            input_content
                                .push_str(&format!("\n\nfile_paths: {}", file_paths.join(",")));
                        }

                        std::fs::create_dir_all(&data_dir)?;
                        std::fs::write(&input_file, input_content)?;

                        println!("KeepGoing=true");
                        println!("input_file: {}", input_file.display());
                        println!("project_path: {}", project_path);
                        if !response_source.is_empty() {
                            println!("response_source: {}", response_source);
                        }
                    } else {
                        println!("KeepGoing=false");
                    }
                }
                Err(e) => {
                    println!("KeepGoing=false");
                    println!("Error: Failed to parse response: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            println!("KeepGoing=false");
            println!("Error: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// 检查服务器健康状态
fn check_server_health(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("127.0.0.1:{}", port);
    match TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(1)) {
        Ok(mut stream) => {
            let request = "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
            if stream.write_all(request.as_bytes()).is_err() {
                return false;
            }
            let mut response = String::new();
            if stream.read_to_string(&mut response).is_err() {
                return false;
            }
            response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200")
        }
        Err(_) => false,
    }
}

/// 读取已注册的端口及其项目路径映射
fn registered_ports_with_workspace() -> Vec<(u16, String)> {
    let mut ports = Vec::new();
    if let Some(home) = dirs::home_dir() {
        let dir = home.join(".cunzhi_ports");
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Ok(p) = name.parse::<u16>() {
                        // 读取文件内容获取项目路径
                        let workspace = fs::read_to_string(entry.path())
                            .unwrap_or_default()
                            .trim()
                            .to_string();
                        ports.push((p, workspace));
                    }
                }
            }
        }
    }
    // 排序确保稳定性
    ports.sort_by_key(|(p, _)| *p);
    ports
}

/// 读取已注册的端口（由 --serve 启动时写入 ~/.cunzhi_ports/<port>）
fn registered_ports() -> Vec<u16> {
    registered_ports_with_workspace()
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

use crate::utils::workspace::{normalize_workspace_path, workspace_depth};

/// 根据 workspace 查找对应端口
fn find_port_for_workspace(workspace: &str) -> Option<u16> {
    find_port_for_workspace_ex(workspace, false)
}

/// 根据 workspace 查找对应端口（扩展版）
/// prefer_with_output=true 时，在同等 depth 下优先选 output.md 非空的端口
fn find_port_for_workspace_ex(workspace: &str, prefer_with_output: bool) -> Option<u16> {
    let workspace_path = normalize_workspace_path(workspace)?;
    let ports = registered_ports_with_workspace();
    // (depth, has_output, port)
    let mut best_match: Option<(usize, bool, u16)> = None;

    let home = dirs::home_dir();

    for (port, ws) in ports {
        let Some(candidate_path) = normalize_workspace_path(&ws) else {
            continue;
        };

        if !workspace_path.starts_with(&candidate_path) {
            continue;
        }

        if !check_server_health(port) {
            continue;
        }

        let depth = workspace_depth(&candidate_path);
        let has_output = if prefer_with_output {
            home.as_ref()
                .map(|h| {
                    let output_file = h.join(".cunzhi").join(port.to_string()).join("output.md");
                    output_file.exists()
                        && std::fs::read_to_string(&output_file)
                            .map(|c| !c.trim().is_empty())
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        } else {
            false
        };

        let is_better = match &best_match {
            None => true,
            Some((best_depth, best_has_output, _)) => {
                if prefer_with_output && has_output != *best_has_output {
                    // 有 output.md 的优先
                    has_output
                } else {
                    // 否则按 depth 排序
                    depth > *best_depth
                }
            }
        };

        if is_better {
            best_match = Some((depth, has_output, port));
        }
    }

    best_match.map(|(_, _, port)| port)
}

/// 在所有已注册端口中查找有非空 output.md 且服务健康的端口
/// 用于 bridge 模式降级：AI 写了 output.md 但端口可能没注册 workspace
fn find_port_with_output() -> Option<u16> {
    let home = dirs::home_dir()?;
    let ports = registered_ports();
    for port in ports {
        if !check_server_health(port) {
            continue;
        }
        let output_file = home
            .join(".cunzhi")
            .join(port.to_string())
            .join("output.md");
        if output_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&output_file) {
                if !content.trim().is_empty() {
                    return Some(port);
                }
            }
        }
    }
    None
}

/// 自动发现可用端口：优先匹配 workspace，次之 ~/.cunzhi_ports，最后 5310-5350 扫描
fn auto_discover_port(workspace: Option<&str>) -> Option<u16> {
    // 1. 优先根据 workspace 查找对应端口
    if let Some(ws) = workspace {
        if let Some(port) = find_port_for_workspace(ws) {
            return Some(port);
        }
    }

    // 2. 次之从已注册端口中查找可用的
    let mut candidates = registered_ports();
    if candidates.is_empty() {
        candidates = (5310u16..5350u16).collect();
    }

    candidates.into_iter().find(|p| check_server_health(*p))
}

/// 确保指定端口的服务器在线；若离线则尝试一次自启动
fn ensure_server_running(port: u16) -> bool {
    if check_server_health(port) {
        instance_debug_log(
            "[ensure-server-skip]",
            format!("port={} already healthy", port),
        );
        return true;
    }

    // 尝试自动拉起一次服务器
    let exe_path = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("iterate"));
    instance_debug_log(
        "[ensure-server-spawn-begin]",
        format!("port={}, exe_path={}", port, exe_path.display()),
    );
    let spawn_result = std::process::Command::new(exe_path)
        .arg("--serve")
        .arg("--port")
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    if let Ok(child) = &spawn_result {
        instance_debug_log(
            "[ensure-server-spawn-success]",
            format!("port={}, child_pid={}", port, child.id()),
        );
    }

    if spawn_result.is_err() {
        instance_debug_log(
            "[ensure-server-spawn-failed]",
            format!("port={}, error={:?}", port, spawn_result.err()),
        );
        return false;
    }

    // 等待 1.5s 让服务起来
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let healthy = check_server_health(port);
    instance_debug_log(
        "[ensure-server-health-after-spawn]",
        format!("port={}, healthy={}", port, healthy),
    );
    healthy
}

/// 发送 HTTP POST 请求
fn send_http_request(port: u16, path: &str, body: &str) -> Result<String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let addr = format!("127.0.0.1:{}", port);
    let mut stream = TcpStream::connect(&addr)?;

    let request = format!(
        "POST {} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path,
        body.len(),
        body
    );

    stream.write_all(request.as_bytes())?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;

    // 解析 HTTP 响应，提取 body
    if let Some(body_start) = response.find("\r\n\r\n") {
        Ok(response[body_start + 4..].to_string())
    } else {
        Err(anyhow::anyhow!("Invalid HTTP response"))
    }
}

/// 自动创建 git 检查点
fn auto_create_checkpoint(workspace: &str) {
    let _ = checkpoint::maybe_auto_checkpoint(workspace, None);
}

/// 显示帮助信息
fn print_help() {
    write_stdout_lossy(concat!(
        "iterate - 智能代码审查工具\n",
        "\n",
        "用法:\n",
        "  iterate                       启动设置界面\n",
        "  iterate --ui [options]        弹窗模式（兼容旧版 --ui）\n",
        "  iterate --serve [--port N]    HTTP 服务器模式（类似 Infinite WF）\n",
        "  iterate --bridge-only [--port N]  无 GUI mobile bridge origin\n",
        "  iterate --bridge [options]    桥接模式（替代 cunzhi.py）\n",
        "  iterate --relay-server [--host H] [--port N] [--relay-token-env ENV] [--relay-audit-log PATH|off]\n",
        "  iterate --relay-mac-client --relay-url URL [--device-id ID] [--relay-token-env ENV]\n",
        "  iterate --cloudflare-auto-setup-smoke --zone Z --subdomain S --api-token-env ENV\n",
        "  iterate --mobile-route-status 读取脱敏的正式手机公网配置与健康状态\n",
        "  iterate --mobile-route-register --base-url URL [--transport cloudflare_named_tunnel]\n",
        "  iterate --mobile-route-verify  重新验证已登记的正式手机公网路线\n",
        "  iterate --mcp-request <文件>  处理 MCP 请求\n",
        "  iterate --check-frontend-assets [--frontend-dist dist]  检查生产包前端资源\n",
        "  iterate --activation-gate-status  输出当前构建的主界面激活门禁状态\n",
        "  iterate --help                显示此帮助信息\n",
        "  iterate --version             显示版本信息\n",
        "\n",
        "--ui 模式参数:\n",
        "  --message \"消息\"    显示的消息内容\n",
        "  --options \"A,B,C\"   预定义选项\n",
        "  --workspace \"路径\"  工作区路径\n",
        "\n",
        "--serve 模式:\n",
        "  --port N            监听端口（默认自动分配）\n",
        "\n",
        "--bridge-only 模式:\n",
        "  --port N            mobile bridge 端口（默认 8080）\n",
        "\n",
        "--cloudflare-auto-setup-smoke 模式（Tunnel + DNS，可选创建 Access allow-email policy）:\n",
        "  --zone Z            Cloudflare zone，例如 tobooks.xin\n",
        "  --subdomain S       要创建或验证的子域名，例如 iterate-test\n",
        "  --api-token-env ENV 从环境变量读取 Cloudflare API Token，不支持明文参数\n",
        "  --overwrite-dns B   是否覆盖冲突 DNS，默认 false\n",
        "  --access-email E    可选，创建 Access allow policy；多个邮箱用逗号分隔\n",
        "\n",
        "--mobile-route-register 模式（只保存脱敏回执；登记前强制验证本机归属）:\n",
        "  --base-url U        当前 Mac 自己的稳定 HTTPS origin\n",
        "  --transport T       当前仅支持 cloudflare_named_tunnel\n",
        "  --source S          ai_configured / manual_adopt / legacy_migration\n",
        "\n",
        "--bridge 模式（替代 cunzhi.py）:\n",
        "  --port N            服务器端口（默认自动检测）\n",
        "  --workspace \"路径\"  工作区路径\n",
        "\n",
        "--relay-server 模式:\n",
        "  --host H            监听地址（默认 127.0.0.1；非 loopback 必须设置 --relay-token-env）\n",
        "  --port N            relay 端口（默认 8790）\n",
        "  --relay-token-env E 从环境变量读取 relay token，不支持明文参数\n",
        "  --relay-audit-log P audit JSONL 路径；默认 ~/Library/Logs/iterate/relay-audit.jsonl，off 关闭\n",
        "\n",
        "--relay-mac-client 模式:\n",
        "  --relay-url URL     relay 地址，例如 ws://127.0.0.1:8790/mac/ws\n",
        "  --device-id ID      Mac 设备 ID（默认 local-mac）\n",
        "  --local-base-url U  本机 bridge URL（默认 http://127.0.0.1:8080）\n",
        "  --heartbeat-secs N  心跳间隔（默认 15，最小 5）\n",
        "  --allow-recover     允许执行 recover_bridge_origin / recover_public_tunnel；默认只模拟\n",
    ));
}

/// 显示版本信息
fn print_version() {
    write_stdout_lossy(&format!("iterate v{}\n", env!("CARGO_PKG_VERSION")));
}

fn write_stdout_lossy(text: &str) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[cfg(target_os = "windows")]
    #[test]
    fn manual_stop_blocks_only_ui_and_owned_background_entry_points() {
        for blocked in [
            "--ui",
            "--show-main-window",
            "--serve",
            "--bridge-only",
            "--mcp-request",
        ] {
            assert!(blocks_manual_stop_cli_mode(&[
                "iterate.exe".to_string(),
                blocked.to_string(),
            ]));
        }

        for allowed in ["--help", "--version", "--check-frontend-assets"] {
            assert!(!blocks_manual_stop_cli_mode(&[
                "iterate.exe".to_string(),
                allowed.to_string(),
            ]));
        }
        assert!(!blocks_manual_stop_cli_mode(&["iterate.exe".to_string()]));
    }

    #[test]
    fn clean_dialog_dismissal_returns_false_without_an_error() {
        let response = clean_dialog_dismissal_response(true, false, true)
            .expect("ready popup with a clean exit and no response should be a dismissal");

        assert!(!response.keep_going);
        assert_eq!(
            response.response_source,
            crate::conversation::POPUP_CLOSED_SOURCE
        );
        assert!(response.error.is_none());
    }

    #[test]
    fn clean_dialog_dismissal_rejects_ambiguous_or_failed_exits() {
        assert!(clean_dialog_dismissal_response(false, false, true).is_none());
        assert!(clean_dialog_dismissal_response(true, false, false).is_none());
        assert!(clean_dialog_dismissal_response(true, true, true).is_none());
    }

    #[test]
    fn popup_ready_apns_always_targets_loopback_bridge() {
        assert_eq!(
            local_bridge_base_url_from_override(Some("https://iterate.example.com".to_string())),
            DEFAULT_LOCAL_BRIDGE_BASE_URL
        );
        assert_eq!(
            local_bridge_base_url_from_override(Some(" http://localhost:18080/ ".to_string())),
            "http://localhost:18080"
        );
        assert_eq!(
            local_bridge_base_url_from_override(Some("http://localhost.example:18080".to_string())),
            DEFAULT_LOCAL_BRIDGE_BASE_URL
        );
    }

    #[cfg(target_os = "macos")]
    fn temp_executable(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create temp executable parent");
        }
        File::create(path).expect("create temp executable");
    }

    #[cfg(target_os = "macos")]
    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iterate-cli-test-{}-{}-{}",
            name,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dialog_gui_path_prefers_explicit_override() {
        let dir = temp_test_dir("dialog-override");
        let current_exe = dir.join("target/release/iterate");
        let override_exe = dir.join("custom/iterate");
        let installed_exe = dir.join("Applications/iterate.app/Contents/MacOS/iterate");
        temp_executable(&current_exe);
        temp_executable(&override_exe);
        temp_executable(&installed_exe);

        assert_eq!(
            resolve_dialog_gui_executable_path(
                current_exe,
                Some(override_exe.clone()),
                installed_exe
            ),
            override_exe
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dialog_gui_path_uses_installed_app_for_local_release_cli() {
        let dir = temp_test_dir("dialog-local-release");
        let current_exe = dir.join("target/release/iterate");
        let installed_exe = dir.join("Applications/iterate.app/Contents/MacOS/iterate");
        temp_executable(&current_exe);
        temp_executable(&installed_exe);

        assert_eq!(
            resolve_dialog_gui_executable_path(current_exe, None, installed_exe.clone()),
            installed_exe
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dialog_gui_path_uses_installed_app_for_non_release_cli() {
        let dir = temp_test_dir("dialog-installed");
        let current_exe = dir.join("target/debug/iterate");
        let installed_exe = dir.join("Applications/iterate.app/Contents/MacOS/iterate");
        temp_executable(&current_exe);
        temp_executable(&installed_exe);

        assert_eq!(
            resolve_dialog_gui_executable_path(current_exe, None, installed_exe.clone()),
            installed_exe
        );
    }

    #[test]
    fn enrich_goal_user_input_adds_saved_image_paths_inside_target() {
        let user_input = "进入 GoalRun 目标模式。\n\n目标：\n《看这张图\n\n附加图片：1 张\n附件地址：\n- images[0]\n（见 images 附件）》\n\n执行规则：继续"
            .to_string();
        let enriched = enrich_goal_user_input_with_attachment_paths(
            user_input,
            "web_bridge_goal_submit",
            &[],
            &["/Users/test/.cunzhi/images/image_123_0.jpg".to_string()],
        );

        assert!(!enriched.contains("附件地址："));
        assert!(!enriched.contains("images[0]"));
        assert!(!enriched.contains("见 images 附件"));
        assert!(enriched.contains("附加图片路径：\n- /Users/test/.cunzhi/images/image_123_0.jpg》"));
        assert!(enriched.contains("》\n\n执行规则"));
    }

    #[test]
    fn enrich_goal_user_input_adds_missing_image_path_when_file_path_already_present() {
        let user_input = "进入 GoalRun 目标模式。\n\n目标：\n《修复目标提交\n\n相关文件：\n@/tmp/spec.md》\n\n执行规则：继续"
            .to_string();
        let enriched = enrich_goal_user_input_with_attachment_paths(
            user_input,
            "web_bridge_goal_submit",
            &["/tmp/spec.md".to_string()],
            &["/Users/test/.cunzhi/images/image_123_0.jpg".to_string()],
        );

        assert_eq!(enriched.matches("/tmp/spec.md").count(), 1);
        assert!(!enriched.contains("images[0]"));
        assert!(!enriched.contains("见 images 附件"));
        assert!(enriched.contains("附加图片路径：\n- /Users/test/.cunzhi/images/image_123_0.jpg》"));
    }

    #[test]
    fn strip_goal_image_reference_context_keeps_goal_closing_marker() {
        let stripped = strip_goal_image_reference_context(
            "进入 GoalRun 目标模式。\n\n目标：\n《看图\n\n附加图片：1 张（见 images 附件）》\n\n执行规则：继续",
        );

        assert!(!stripped.contains("见 images 附件"));
        assert!(stripped.contains("目标：\n《看图》"));
        assert!(stripped.contains("》\n\n执行规则"));
    }

    #[test]
    fn prepend_selected_options_to_user_input_places_options_before_text() {
        let display = prepend_selected_options_to_user_input(
            "✔️不明白的地方反问我，先不着急编码",
            &["先做 T7".to_string()],
        );

        assert_eq!(
            display,
            "选中的选项: 先做 T7\n\n✔️不明白的地方反问我，先不着急编码"
        );
    }

    #[test]
    fn enrich_goal_user_input_skips_non_goal_sources() {
        let user_input = "普通回复".to_string();
        let enriched = enrich_goal_user_input_with_attachment_paths(
            user_input.clone(),
            "popup_submit",
            &[],
            &["/Users/test/.cunzhi/images/image_123_0.jpg".to_string()],
        );

        assert_eq!(enriched, user_input);
    }

    #[test]
    fn mobile_route_register_rejects_unexpected_secret_bearing_options() {
        let flags = vec!["--mobile-route-register".to_string()];
        let options = HashMap::from([
            (
                "--base-url".to_string(),
                "https://iterate.example.com".to_string(),
            ),
            (
                "--api-token".to_string(),
                "must-not-be-accepted".to_string(),
            ),
        ]);

        let error = handle_mobile_route_register(&flags, &options)
            .expect_err("unexpected options must be rejected before route verification");
        assert!(error
            .to_string()
            .contains("mobile route register rejects unexpected option: --api-token"));
    }
}
