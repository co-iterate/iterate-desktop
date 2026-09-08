use crate::app::commands::*;
#[cfg(not(target_os = "windows"))]
use crate::app::setup::setup_application;
#[cfg(target_os = "windows")]
use crate::app::setup::start_application_setup;
use crate::config::AppState;
use crate::conversation::ConversationManager;
#[cfg(not(target_os = "windows"))]
use crate::log_important;
use crate::ui::AudioController;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::utils::assets::AssetKey;
use tauri::{Builder, Manager};

fn is_standalone_mcp_launch(args: &[String]) -> bool {
    std::env::var("ITERATE_STANDALONE_MODE").is_ok()
        || std::env::var("ITERATE_MCP_REQUEST_FILE").is_ok()
        || args.get(1).is_some_and(|arg| arg == "--mcp-request")
        || args.iter().any(|arg| arg == "--ui")
}

fn should_show_main_window_on_launch(args: &[String]) -> bool {
    if is_standalone_mcp_launch(args) {
        return true;
    }

    if args.iter().any(|arg| arg == "--show-main-window") {
        return true;
    }

    #[cfg(target_os = "windows")]
    {
        // Windows 直接双击 iterate.exe 时，默认展示主界面，
        // 避免应用已启动但只剩控制台/隐藏窗口，造成“没打开”的体验。
        args.len() == 1
    }

    #[cfg(not(target_os = "windows"))]
    {
        cfg!(debug_assertions)
            && std::env::var("ITERATE_DEV_SHOW_MAIN")
                .map(|value| value == "1")
                .unwrap_or(false)
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
fn prepare_macos_standalone_launch(args: &[String]) {
    if !is_standalone_mcp_launch(args) {
        return;
    }

    crate::ui::commands::remember_standalone_previous_frontmost_application();

    // AppKit can show a persistent-state crash recovery alert before Tauri setup
    // runs. Standalone MCP popups are short-lived, so skip window restoration.
    unsafe {
        use cocoa::base::{id, nil, NO, YES};
        use cocoa::foundation::NSString;
        use objc::{class, msg_send, sel, sel_impl};

        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if !defaults.is_null() {
            let registered_defaults: id = msg_send![class!(NSMutableDictionary), dictionary];
            if !registered_defaults.is_null() {
                let ignore_state_key = NSString::alloc(nil).init_str("ApplePersistenceIgnoreState");
                let keep_windows_key = NSString::alloc(nil).init_str("NSQuitAlwaysKeepsWindows");
                let ignore_state_value: id = msg_send![class!(NSNumber), numberWithBool: YES];
                let keep_windows_value: id = msg_send![class!(NSNumber), numberWithBool: NO];
                let _: () = msg_send![registered_defaults, setObject: ignore_state_value forKey: ignore_state_key];
                let _: () = msg_send![registered_defaults, setObject: keep_windows_value forKey: keep_windows_key];
                let _: () = msg_send![defaults, registerDefaults: registered_defaults];
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let saved_state_dir = Path::new(&home)
            .join("Library")
            .join("Saved Application State")
            .join("com.kexin94yyds.iterate.savedState");
        let _ = fs::remove_dir_all(saved_state_dir);
    }
}

#[cfg(not(target_os = "macos"))]
fn prepare_macos_standalone_launch(_args: &[String]) {}

pub fn check_frontend_assets() -> Result<(), String> {
    if tauri::is_dev() {
        return Ok(());
    }

    let context = build_tauri_context();
    check_frontend_assets_in_context(&context)
}

pub fn check_frontend_assets_for_dist(dist_dir: &Path) -> Result<usize, String> {
    let expected_assets = expected_frontend_assets_from_dist(dist_dir)?;
    let context = build_tauri_context();
    check_frontend_assets_in_context(&context)?;
    check_expected_frontend_assets_in_context(&context, &expected_assets)?;
    Ok(expected_assets.len())
}

fn build_tauri_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

fn check_frontend_assets_in_context(context: &tauri::Context<tauri::Wry>) -> Result<(), String> {
    let key = AssetKey::from("index.html");
    match context.assets.get(&key) {
        Some(asset) if !asset.is_empty() => Ok(()),
        _ => {
            let asset_count = context.assets.iter().count();
            Err(format!(
                "frontend asset missing: index.html (embedded_assets={asset_count}). \
                 Run `pnpm build` before compiling the Tauri app, then rebuild the app bundle."
            ))
        }
    }
}

fn expected_frontend_assets_from_dist(dist_dir: &Path) -> Result<Vec<String>, String> {
    let index_path = dist_dir.join("index.html");
    let index_html = fs::read_to_string(&index_path)
        .map_err(|error| format!("failed to read {}: {error}", index_path.display()))?;
    let entry_assets = extract_html_asset_refs(&index_html);

    if entry_assets.is_empty() {
        return Err(format!(
            "no frontend asset references found in {}",
            index_path.display()
        ));
    }

    let mut assets = BTreeSet::new();
    let mut pending = entry_assets;

    while let Some(asset) = pending.pop() {
        if !assets.insert(asset.clone()) {
            continue;
        }

        let asset_path = dist_dir.join(&asset);
        let metadata = fs::metadata(&asset_path).map_err(|error| {
            format!(
                "frontend dist asset missing: {} ({error})",
                asset_path.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "frontend dist asset is not a file: {}",
                asset_path.display()
            ));
        }
        if metadata.len() == 0 {
            return Err(format!(
                "frontend dist asset is empty: {}",
                asset_path.display()
            ));
        }

        if !should_scan_asset_contents(&asset) {
            continue;
        }

        let contents = fs::read_to_string(&asset_path).map_err(|error| {
            format!(
                "failed to read frontend dist asset {}: {error}",
                asset_path.display()
            )
        })?;

        for nested_asset in extract_nested_asset_refs(&contents, &asset) {
            if !assets.contains(&nested_asset) {
                pending.push(nested_asset);
            }
        }
    }

    Ok(assets.into_iter().collect())
}

fn check_expected_frontend_assets_in_context(
    context: &tauri::Context<tauri::Wry>,
    expected_assets: &[String],
) -> Result<(), String> {
    let missing_assets: Vec<_> = expected_assets
        .iter()
        .filter(|asset| {
            let key = AssetKey::from(asset.as_str());
            !matches!(context.assets.get(&key), Some(bytes) if !bytes.is_empty())
        })
        .cloned()
        .collect();

    if missing_assets.is_empty() {
        return Ok(());
    }

    Err(format!(
        "frontend embedded assets are stale or incomplete; missing from bundle: {}",
        missing_assets.join(", ")
    ))
}

fn extract_html_asset_refs(index_html: &str) -> Vec<String> {
    let mut refs = BTreeSet::new();
    collect_asset_refs_for_attr(index_html, "src", &mut refs);
    collect_asset_refs_for_attr(index_html, "href", &mut refs);
    refs.into_iter().collect()
}

fn collect_asset_refs_for_attr(index_html: &str, attr: &str, refs: &mut BTreeSet<String>) {
    let needle = format!("{attr}=");
    let mut rest = index_html;

    while let Some(index) = rest.find(&needle) {
        let after_attr = &rest[index + needle.len()..];
        let trimmed = after_attr.trim_start();
        let Some(quote) = trimmed.chars().next() else {
            break;
        };

        if quote != '"' && quote != '\'' {
            rest = &trimmed[quote.len_utf8()..];
            continue;
        }

        let value_start = quote.len_utf8();
        let value = &trimmed[value_start..];
        let Some(value_end) = value.find(quote) else {
            break;
        };

        if let Some(asset_ref) = normalize_frontend_asset_ref("", &value[..value_end]) {
            refs.insert(asset_ref);
        }

        rest = &value[value_end + quote.len_utf8()..];
    }
}

fn extract_nested_asset_refs(contents: &str, current_asset: &str) -> Vec<String> {
    let mut refs = BTreeSet::new();
    let base_dir = current_asset
        .rsplit_once('/')
        .map(|(base_dir, _)| base_dir)
        .unwrap_or("");

    collect_asset_refs_for_quoted_values(contents, base_dir, &mut refs);
    collect_asset_refs_for_css_urls(contents, base_dir, &mut refs);

    refs.into_iter().collect()
}

fn collect_asset_refs_for_quoted_values(
    contents: &str,
    base_dir: &str,
    refs: &mut BTreeSet<String>,
) {
    let mut search_start = 0;

    while search_start < contents.len() {
        let Some((relative_quote_start, quote)) = contents[search_start..]
            .char_indices()
            .find(|(_, ch)| *ch == '"' || *ch == '\'')
        else {
            break;
        };

        let quote_start = search_start + relative_quote_start;
        let value_start = quote_start + quote.len_utf8();
        let mut escaped = false;
        let mut value_end = None;

        for (index, ch) in contents[value_start..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }

            if ch == '\\' {
                escaped = true;
                continue;
            }

            if ch == quote {
                value_end = Some(value_start + index);
                break;
            }
        }

        let Some(value_end) = value_end else {
            break;
        };

        if let Some(asset_ref) =
            normalize_frontend_asset_ref(base_dir, &contents[value_start..value_end])
        {
            refs.insert(asset_ref);
        }

        search_start = value_end + quote.len_utf8();
    }
}

fn collect_asset_refs_for_css_urls(contents: &str, base_dir: &str, refs: &mut BTreeSet<String>) {
    let mut rest = contents;

    while let Some(index) = rest.find("url(") {
        let after_url = &rest[index + "url(".len()..];
        let Some(value_end) = after_url.find(')') else {
            break;
        };

        let value = after_url[..value_end]
            .trim()
            .trim_matches('"')
            .trim_matches('\'');

        if let Some(asset_ref) = normalize_frontend_asset_ref(base_dir, value) {
            refs.insert(asset_ref);
        }

        rest = &after_url[value_end + 1..];
    }
}

fn should_scan_asset_contents(asset: &str) -> bool {
    asset.ends_with(".css")
        || asset.ends_with(".html")
        || asset.ends_with(".js")
        || asset.ends_with(".mjs")
        || asset.ends_with(".svg")
}

fn normalize_frontend_asset_ref(base_dir: &str, value: &str) -> Option<String> {
    let without_fragment = value.split('#').next().unwrap_or(value);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    let value = without_query.trim();

    if value.is_empty()
        || value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("//")
        || value.starts_with("data:")
        || value.starts_with("blob:")
        || value.starts_with("javascript:")
    {
        return None;
    }

    let normalized = if let Some(asset) = value.strip_prefix("/assets/") {
        format!("assets/{asset}")
    } else if value.starts_with("assets/") {
        value.to_string()
    } else if value.starts_with("./") || value.starts_with("../") {
        normalize_relative_asset_ref(base_dir, value)?
    } else {
        return None;
    };

    if normalized.starts_with("assets/") && has_frontend_asset_extension(&normalized) {
        Some(normalized)
    } else {
        None
    }
}

fn normalize_relative_asset_ref(base_dir: &str, value: &str) -> Option<String> {
    let mut parts: Vec<&str> = base_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();

    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }

    Some(parts.join("/"))
}

fn has_frontend_asset_extension(asset: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".avif", ".css", ".gif", ".html", ".ico", ".jpeg", ".jpg", ".js", ".json", ".mjs", ".mp3",
        ".mp4", ".otf", ".png", ".svg", ".ttf", ".txt", ".wasm", ".webp", ".woff", ".woff2",
    ];

    EXTENSIONS
        .iter()
        .any(|extension| asset.ends_with(extension))
}

#[cfg(test)]
mod tests {
    use super::{expected_frontend_assets_from_dist, extract_html_asset_refs};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn extracts_vite_asset_refs_from_index_html() {
        let assets = extract_html_asset_refs(
            r#"<script type="module" src="./assets/App-C1o7Wf8b.js"></script>
               <link rel="stylesheet" href="/assets/App-x0B7Cti3.css">
               <link rel="icon" href="https://example.com/icon.png">"#,
        );

        assert_eq!(
            assets,
            vec![
                "assets/App-C1o7Wf8b.js".to_string(),
                "assets/App-x0B7Cti3.css".to_string()
            ]
        );
    }

    #[test]
    fn recursively_extracts_vite_chunk_refs_from_entry_js() {
        let test_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dist_dir = std::env::temp_dir().join(format!(
            "iterate-frontend-asset-gate-{test_id}-{}",
            std::process::id()
        ));

        fs::create_dir_all(dist_dir.join("assets")).unwrap();
        fs::write(
            dist_dir.join("index.html"),
            r#"<script type="module" crossorigin src="./assets/index-BKqn0oKf.js"></script>"#,
        )
        .unwrap();
        fs::write(
            dist_dir.join("assets/index-BKqn0oKf.js"),
            r#"const deps=["./__uno-CFTwMXKY.css","./App-C1o7Wf8b.js"];import("./App-C1o7Wf8b.js");"#,
        )
        .unwrap();
        fs::write(dist_dir.join("assets/__uno-CFTwMXKY.css"), "body{}").unwrap();
        fs::write(
            dist_dir.join("assets/App-C1o7Wf8b.js"),
            "console.log('app');",
        )
        .unwrap();

        let assets = expected_frontend_assets_from_dist(&dist_dir).unwrap();

        fs::remove_dir_all(&dist_dir).unwrap();

        assert_eq!(
            assets,
            vec![
                "assets/App-C1o7Wf8b.js".to_string(),
                "assets/__uno-CFTwMXKY.css".to_string(),
                "assets/index-BKqn0oKf.js".to_string()
            ]
        );
    }

    #[test]
    fn rejects_missing_dist_asset_referenced_by_index_html() {
        let test_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dist_dir = std::env::temp_dir().join(format!(
            "iterate-frontend-asset-gate-missing-{test_id}-{}",
            std::process::id()
        ));

        fs::create_dir_all(dist_dir.join("assets")).unwrap();
        fs::write(
            dist_dir.join("index.html"),
            r#"<script type="module" crossorigin src="./assets/index-Missing.js"></script>"#,
        )
        .unwrap();

        let error = expected_frontend_assets_from_dist(&dist_dir).unwrap_err();

        fs::remove_dir_all(&dist_dir).unwrap();

        assert!(
            error.contains("frontend dist asset missing"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn standalone_launch_defaults_are_not_persisted_with_set_bool() {
        let source = include_str!("builder.rs");
        let start = source
            .find("fn prepare_macos_standalone_launch")
            .expect("prepare_macos_standalone_launch should exist");
        let end = source[start..]
            .find("#[cfg(not(target_os = \"macos\"))]")
            .expect("non-macOS prepare_macos_standalone_launch should follow macOS implementation");
        let function_source = &source[start..start + end];

        assert!(
            function_source.contains("registerDefaults"),
            "standalone launch should use volatile registered defaults, not persistent writes"
        );
        assert!(
            !function_source.contains("setBool: YES forKey: ignore_state_key"),
            "standalone launch must not persist ApplePersistenceIgnoreState"
        );
        assert!(
            !function_source.contains("setBool: NO forKey: keep_windows_key"),
            "standalone launch must not persist NSQuitAlwaysKeepsWindows"
        );
    }

    #[test]
    fn explicit_show_main_window_launch_is_visible() {
        assert!(super::should_show_main_window_on_launch(&[
            "iterate".to_string(),
            "--show-main-window".to_string(),
        ]));
    }
}

#[cfg(target_os = "macos")]
fn forward_macos_native_text_drop(window: &tauri::Window<tauri::Wry>, event: &tauri::WindowEvent) {
    use tauri::{DragDropEvent, Emitter, WindowEvent};

    if window.label() != "main" {
        return;
    }

    let WindowEvent::DragDrop(DragDropEvent::Drop { paths, position }) = event else {
        return;
    };
    let text = crate::ui::macos_text_drop::take_main_webview_drop_text();
    if !paths.is_empty() {
        return;
    }

    let Some(text) = text else {
        return;
    };

    let payload = serde_json::json!({
        "text": text,
        "logicalPosition": {
            "x": position.x,
            "y": position.y,
        },
    });

    if let Err(error) = window.emit("popup://native-text-drop", payload) {
        log::warn!("failed to forward native text drop: {error}");
    }
}

/// 构建Tauri应用
pub fn build_tauri_app() -> Builder<tauri::Wry> {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());

    #[cfg(target_os = "macos")]
    let builder = builder.on_window_event(forward_macos_native_text_drop);

    builder
        .manage(AppState::default())
        .manage(Arc::new(ConversationManager::new()))
        .manage(crate::ui::live_goal::LiveGoalTrayState::default())
        .manage(crate::ui::quota_snapshot::QuotaSnapshotState::default())
        .manage(AudioController {
            should_stop: Arc::new(AtomicBool::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            // 基础应用命令
            crate::bridge::auth::get_bridge_desktop_token,
            get_app_info,
            requires_activation_gate,
            get_codex_quota_providers,
            get_usage_quota_providers,
            get_always_on_top,
            set_always_on_top,
            get_auto_checkpoint_enabled,
            set_auto_checkpoint_enabled,
            sync_window_state,
            reload_config,
            crate::app::setup::get_startup_status,
            crate::app::setup::retry_background_services,
            // 音频命令
            get_audio_notification_enabled,
            set_audio_notification_enabled,
            get_audio_url,
            set_audio_url,
            import_custom_audio,
            play_notification_sound,
            test_audio_sound,
            stop_audio_sound,
            get_available_audio_assets,
            refresh_audio_assets,
            // 主题和窗口命令
            get_theme,
            set_theme,
            get_window_config,
            set_window_config,
            get_reply_config,
            set_reply_config,
            get_window_settings,
            set_window_settings,
            get_window_settings_for_mode,
            get_window_constraints_cmd,
            get_current_window_size,
            apply_window_constraints,
            update_window_size,
            // 字体命令
            get_font_config,
            set_font_family,
            set_font_size,
            set_custom_font_family,
            get_font_family_options,
            get_font_size_options,
            reset_font_config,
            // MCP 命令
            get_mcp_tools_config,
            set_mcp_tool_enabled,
            get_mcp_tools_status,
            reset_mcp_tools_config,
            send_mcp_response,
            crate::conversation::commands::migrate_timeline_image_storage,
            get_cli_args,
            read_mcp_request,
            list_project_files,
            select_image_files,
            crate::ui::commands::select_files_and_folders,
            crate::ui::commands::enable_prevent_sleep,
            crate::ui::commands::disable_prevent_sleep,
            crate::ui::commands::toggle_prevent_sleep,
            crate::ui::commands::get_prevent_sleep_status,
            crate::ui::commands::read_file_base64,
            crate::ui::commands::read_clipboard_file_paths,
            crate::ui::commands::save_prompt_library_file,
            crate::ui::commands::load_prompt_library_file,
            crate::ui::commands::save_ghost_suggestions_file,
            crate::ui::commands::load_ghost_suggestions_file,
            crate::ui::commands::upsert_ghost_suggestion,
            crate::ui::commands::get_ghost_suggestion_learning_state,
            crate::ui::commands::record_ghost_suggestion_learning,
            crate::ui::commands::merge_ghost_suggestion_learning_state,
            crate::ui::commands::get_hui_snapshot,
            crate::ui::commands::get_speech_muscle_memory_entries,
            crate::ui::commands::save_speech_muscle_memory_entries,
            crate::ui::commands::get_speech_correction_memory_entries,
            crate::ui::commands::save_speech_correction_memory_entries,
            crate::ui::commands::record_speech_muscle_memory_hit,
            crate::ui::commands::record_speech_correction_memory_hit,
            crate::ui::commands::record_speech_correction_memory_feedback,
            crate::ui::commands::get_speech_vocabulary_entries,
            crate::ui::commands::record_speech_vocabulary_terms,
            crate::ui::commands::merge_speech_vocabulary_terms,
            crate::ui::commands::append_speech_history_markdown,
            crate::ui::live_goal::get_live_goal,
            crate::ui::live_goal::resolve_live_goal_response_metadata,
            crate::ui::live_goal::start_live_goal,
            crate::ui::live_goal::update_live_goal_progress,
            crate::ui::live_goal::update_live_goal_quota_status,
            crate::ui::live_goal::complete_live_goal,
            crate::ui::live_goal::clear_live_goal,
            crate::native_speech::accessibility_status,
            crate::native_speech::input_monitoring_status,
            crate::native_speech::microphone_status,
            crate::native_speech::speech_recognition_status,
            crate::native_speech::request_accessibility_permission,
            crate::native_speech::request_input_monitoring_permission,
            crate::native_speech::request_microphone_permission,
            crate::native_speech::request_speech_recognition_permission,
            crate::native_speech::remember_frontmost_app,
            crate::native_speech::get_captured_target_app_bundle_id,
            crate::native_speech::get_speech_runtime_status,
            crate::native_speech::register_popup_speech_target,
            crate::native_speech::unregister_popup_speech_target,
            crate::native_speech::get_active_popup_speech_target,
            crate::native_speech::hud_animation::animate_speech_overlay_frame,
            crate::native_speech::authorize_popup_speech_insert,
            crate::native_speech::record_popup_speech_insert_result,
            crate::native_speech::phase1::get_speech_control_snapshot,
            crate::native_speech::phase1::ack_speech_overlay_visibility,
            crate::native_speech::phase1::configure_speech_recognition,
            crate::native_speech::phase1::complete_speech_processing,
            crate::native_speech::mark_speech_overlay_ready,
            crate::native_speech::mark_speech_overlay_unready,
            crate::native_speech::reveal_speech_overlay_window,
            crate::native_speech::hide_speech_overlay_window,
            crate::native_speech::start_native_speech,
            crate::native_speech::stop_native_speech,
            crate::native_speech::set_codex_live_audio_reserved,
            crate::native_speech::commit_speech_text,
            crate::native_speech::paste_text,
            #[cfg(target_os = "windows")]
            crate::native_speech::windows::get_windows_speech_capability,
            #[cfg(target_os = "windows")]
            crate::native_speech::windows::start_windows_speech_dictation,
            #[cfg(target_os = "windows")]
            crate::native_speech::windows::commit_windows_speech_text,
            crate::ui::commands::list_prompt_files,
            crate::ui::commands::read_text_file,
            crate::ui::commands::capture_screenshot,
            open_new_windsurf_chat,
            open_new_windsurf_chat_with_content,
            open_codex_project,
            open_codex_thread,
            open_new_codex_chat_with_text,
            ack_mcp_request_ready,
            build_mcp_send_response,
            build_mcp_continue_response,
            create_test_popup,
            // 对话树命令
            create_conversation_tree,
            add_conversation_node,
            ensure_conversation_assistant_node,
            switch_conversation_node,
            get_conversation_path,
            get_current_conversation_node_id,
            clear_conversation_tree,
            // Bridge 命令
            crate::bridge::send_to_web_bridge,
            crate::bridge::send_phone_action_request,
            crate::relay::get_relay_mac_client_config,
            crate::relay::save_relay_mac_client_config,
            crate::relay::control_relay_mac_client,
            // MCP 工具命令
            crate::mcp::tools::ci::commands::execute_ci_tool,
            crate::mcp::tools::memory::commands::execute_ji_tool,
            // 自定义prompt命令
            get_hui_suggestion_terms,
            get_custom_prompt_config,
            add_custom_prompt,
            update_custom_prompt,
            delete_custom_prompt,
            set_custom_prompt_enabled,
            update_custom_prompt_order,
            update_conditional_prompt_state,
            update_conditional_prompt_active,
            // 快捷键命令
            get_shortcut_config,
            update_shortcut_binding,
            reset_shortcuts_to_default,
            get_global_shortcut_enabled,
            set_global_shortcut_enabled,
            // 窗口注册命令
            register_window_instance,
            get_default_window_registration_label,
            unregister_window_instance,
            get_all_window_instances,
            activate_window_instance,
            debug_log,
            timeline_debug_log,
            // 配置管理命令
            get_config_file_path,
            // Telegram 命令
            get_telegram_config,
            set_telegram_config,
            test_telegram_connection_cmd,
            auto_get_chat_id,
            start_telegram_sync,
            // 系统命令
            open_external_url,
            crate::ui::commands::open_local_path,
            crate::ui::commands::open_confirmed_external_file,
            // 仅打开系统终端；内嵌 PTY shell 不注册到全局 invoke
            open_terminal,
            open_in_ide,
            center_window,
            dismiss_standalone_mcp_window,
            activate_app_window,
            probe_codex_automation_permission,
            exit_app,
            handle_app_exit_request,
            force_exit_app,
            reset_exit_attempts_cmd,
            // 更新命令
            check_for_updates,
            download_and_install_update,
            get_current_version,
            restart_app,
            // 浏览器监控命令
            crate::browser::start_browser_monitoring,
            crate::browser::stop_browser_monitoring,
            crate::browser::get_browser_monitor_status,
            crate::browser::get_browser_ws_pairing_token,
            crate::browser::open_browser_url,
            crate::browser::open_html_artifact_in_browser,
            crate::browser::send_message_to_browser_ai,
            crate::browser::show_ai_completion_popup,
            crate::browser::get_latest_ai_response,
            // 寸止端口监听服务命令
            crate::server::commands::start_cunzhi_server,
            crate::server::commands::stop_cunzhi_server,
            crate::server::commands::get_cunzhi_server_status,
            crate::server::commands::cleanup_cunzhi_ports,
            crate::server::commands::check_port_available,
            // Checkpoint 命令
            crate::mcp::tools::checkpoint::commands::create_checkpoint,
            crate::mcp::tools::checkpoint::commands::list_checkpoints,
            crate::mcp::tools::checkpoint::commands::get_checkpoint_files,
            crate::mcp::tools::checkpoint::commands::restore_checkpoint,
            crate::mcp::tools::checkpoint::commands::restore_checkpoint_safe,
            crate::mcp::tools::checkpoint::commands::delete_checkpoint,
            // 远程隧道命令
            crate::tunnel::commands::start_remote_tunnel,
            crate::tunnel::commands::stop_remote_tunnel,
            crate::tunnel::commands::get_remote_tunnel_status,
            crate::tunnel::commands::start_quick_tunnel,
            crate::tunnel::commands::stop_quick_tunnel,
            crate::tunnel::commands::get_quick_tunnel_status,
            crate::tunnel::commands::check_origin_health,
            crate::tunnel::commands::recover_bridge_origin,
            crate::tunnel::commands::get_cloudflare_guided_config,
            crate::tunnel::commands::save_cloudflare_guided_config,
            crate::tunnel::commands::clear_cloudflare_guided_config,
            crate::tunnel::commands::verify_cloudflare_guided_config,
            crate::tunnel::commands::start_cloudflare_customer_tunnel,
            crate::tunnel::commands::stop_cloudflare_customer_tunnel,
            crate::tunnel::commands::get_cloudflare_customer_tunnel_status,
            crate::tunnel::commands::create_cloudflare_web_login_auto_setup,
            crate::tunnel::commands::create_cloudflare_web_login_pairing,
            crate::tunnel::commands::list_cloudflare_web_login_sessions,
            crate::tunnel::commands::revoke_cloudflare_web_login_sessions,
            // 试用期命令
            crate::license::is_licensed,
            crate::license::get_trial_status,
            crate::license::get_trial_days_remaining,
            crate::license::activate_license
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();
            crate::app::windows_lifecycle::start_tauri_shutdown_listener(app_handle.clone());
            crate::native_speech::set_app_handle(app_handle.clone());
            crate::ui::live_goal::start_live_goal_tray_timer(app_handle.clone());
            crate::ui::codex_goal_observer::start_codex_goal_observer(app_handle.clone());
            #[cfg(target_os = "windows")]
            {
                crate::ui::setup_window_event_listeners(&app_handle);
                crate::ui::window_events::start_window_registry_cleanup_task();
                if let Err(error) = crate::ui::exit_handler::setup_exit_handlers(&app_handle) {
                    log::warn!("设置退出处理器失败: {error}");
                }
            }

            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = window.with_webview(|webview| {
                    if let Err(error) =
                        crate::ui::macos_text_drop::install_main_webview_text_drop_capture(
                            webview.inner(),
                        )
                    {
                        log::warn!("failed to install native text-drop capture: {error}");
                    }
                }) {
                    log::warn!(
                        "failed to access main webview for native text-drop capture: {error}"
                    );
                }
            }

            #[cfg(not(target_os = "windows"))]
            tauri::async_runtime::block_on(async {
                if let Err(error) = setup_application(&app_handle).await {
                    log_important!(error, "应用初始化失败: {}", error);
                }
            });

            let args: Vec<String> = std::env::args().collect();

            if should_show_main_window_on_launch(&args) {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            #[cfg(target_os = "windows")]
            {
                // Windows 主窗口先显示，配置、Bridge、IPC 和浏览器监控在后台初始化，
                // 避免双击后长时间没有任何反馈。
                start_application_setup(app_handle);
            }

            Ok(())
        })
}

/// 运行Tauri应用
pub fn run_tauri_app() {
    install_android_rustls_crypto_provider();

    let args: Vec<String> = std::env::args().collect();
    let instance_role = if is_standalone_mcp_launch(&args) {
        "popup"
    } else {
        "gui"
    };
    let _instance_guard =
        match crate::app::windows_lifecycle::register_current_instance(instance_role, None) {
            Ok(guard) => Some(guard),
            Err(error) => {
                log::warn!("登记 iterate Windows 实例失败: {error}");
                None
            }
        };
    prepare_macos_standalone_launch(&args);

    let context = build_tauri_context();

    if !tauri::is_dev() {
        if let Err(error) = check_frontend_assets_in_context(&context) {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }

    build_tauri_app()
        .build(context)
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // 处理 macOS Dock 图标点击事件
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        });
}

#[cfg(target_os = "android")]
fn install_android_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(not(target_os = "android"))]
fn install_android_rustls_crypto_provider() {}
