use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::signature::{self, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};

const PUBLIC_RELEASES_API_URL: &str =
    "https://api.github.com/repos/kexin94yyds/iterate-releases/releases/latest";
const PUBLIC_RELEASES_LATEST_PAGE_URL: &str =
    "https://github.com/kexin94yyds/iterate-releases/releases/latest";
const TRUSTED_RELEASE_DOWNLOAD_HOST: &str = "github.com";
const TRUSTED_RELEASE_DOWNLOAD_PATH_PREFIX: &str =
    "/kexin94yyds/iterate-releases/releases/download/";
const EXPECTED_MACOS_TEAM_IDENTIFIER: &str = "UM3Z9G5DNH";
const RELEASE_PUBLIC_KEY_ENV: &str = "ITERATE_RELEASE_PUBLIC_KEY_B64";
const EMBEDDED_RELEASE_PUBLIC_KEY_B64: Option<&str> = option_env!("ITERATE_RELEASE_PUBLIC_KEY_B64");

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub download_url: String,
    pub sha256_url: String,
    pub signature_url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpdateProgress {
    pub chunk_length: usize,
    pub content_length: Option<u64>,
    pub downloaded: u64,
    pub percentage: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MacOsCodeSignatureDetails {
    team_identifier: Option<String>,
    bundle_identifier: Option<String>,
}

fn release_download_url_is_trusted(download_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(download_url) else {
        return false;
    };

    url.scheme() == "https"
        && url.host_str() == Some(TRUSTED_RELEASE_DOWNLOAD_HOST)
        && url.path().starts_with(TRUSTED_RELEASE_DOWNLOAD_PATH_PREFIX)
}

fn macos_update_signature_details_trusted(
    details: &MacOsCodeSignatureDetails,
) -> Result<(), String> {
    if details.team_identifier.as_deref() != Some(EXPECTED_MACOS_TEAM_IDENTIFIER) {
        return Err(format!(
            "更新包签名 Team ID 不匹配: expected={}, actual={}",
            EXPECTED_MACOS_TEAM_IDENTIFIER,
            details.team_identifier.as_deref().unwrap_or("<missing>")
        ));
    }

    if details.bundle_identifier.as_deref() != Some(crate::constants::app::APP_IDENTIFIER) {
        return Err(format!(
            "更新包 Bundle ID 不匹配: expected={}, actual={}",
            crate::constants::app::APP_IDENTIFIER,
            details.bundle_identifier.as_deref().unwrap_or("<missing>")
        ));
    }

    Ok(())
}

/// 检查是否有可用更新
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<UpdateInfo, String> {
    log::info!("🔍 开始检查更新");

    // 由于Tauri更新器无法处理中文tag，这里直接使用GitHub API检查
    let client = reqwest::Client::new();
    let release_api_url = release_api_url();
    log::info!("📡 发送 GitHub API 请求");

    let response = client
        .get(release_api_url)
        .header("User-Agent", "cunzhi-app/1.0")
        .header("Accept", "application/vnd.github.v3+json")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            log::error!("❌ 网络请求失败: {}", e);
            format!("网络请求失败: {}", e)
        })?;

    log::info!("📊 GitHub API 响应状态: {}", response.status());

    let release: serde_json::Value = if response.status().is_success() {
        response.json().await.map_err(|e| {
            log::error!("❌ 解析响应失败: {}", e);
            format!("解析响应失败: {}", e)
        })?
    } else {
        let status = response.status();
        // 匿名 API 按出口 IP 限额（共享代理节点很容易用完），网页跳转不受此限制（P-2026-9358）
        if status == 403 || status == 429 {
            log::warn!("⚠️ GitHub API 受限 ({status})，改用发布页跳转获取最新版本");
            match fetch_latest_release_via_redirect().await {
                Ok(release) => release,
                Err(fallback_error) => {
                    let error_msg = format!(
                        "网络请求受限，请手动下载最新版本: {} ({fallback_error})",
                        PUBLIC_RELEASES_LATEST_PAGE_URL
                    );
                    log::error!("❌ {}", error_msg);
                    return Err(error_msg);
                }
            }
        } else {
            let error_msg = if status == 404 {
                format!("发布源不可用: {}", release_api_url)
            } else {
                format!("网络请求失败: {}", status)
            };
            log::error!("❌ {}", error_msg);
            return Err(error_msg);
        }
    };

    log::info!("📋 成功获取 release 数据");

    let current_version = app.package_info().version.to_string();
    log::info!("📦 当前版本: {}", current_version);

    // 提取最新版本号，处理中文tag
    let tag_name = release["tag_name"].as_str().unwrap_or("").to_string();

    log::info!("🏷️ GitHub tag: {}", tag_name);

    // 移除前缀v和中文字符，只保留数字和点
    let latest_version = tag_name
        .replace("v", "")
        .chars()
        .filter(|c| c.is_numeric() || *c == '.')
        .collect::<String>();

    log::info!("🆕 解析后的最新版本: {}", latest_version);

    if latest_version.is_empty() {
        let error_msg = "无法解析版本号".to_string();
        log::error!("❌ {}", error_msg);
        return Err(error_msg);
    }

    // 比较版本号
    let has_update = compare_versions(&latest_version, &current_version);
    log::info!("🔄 版本比较结果 - 有更新: {}", has_update);

    // 获取实际的下载URL（从assets中找到对应平台的文件）
    let selected_asset = match get_platform_download_asset(&release) {
        Ok(asset) => asset,
        Err(err) if has_update => return Err(err),
        Err(_) => release_page_download_asset(&release),
    };

    let update_info = UpdateInfo {
        available: has_update,
        current_version,
        latest_version,
        release_notes: release["body"].as_str().unwrap_or("").to_string(),
        download_url: selected_asset.download_url,
        sha256_url: selected_asset.sha256_url,
        signature_url: selected_asset.signature_url,
    };

    log::info!("✅ 更新检查完成: {:?}", update_info);
    Ok(update_info)
}

fn release_api_url() -> &'static str {
    PUBLIC_RELEASES_API_URL
}

/// CI 每个版本都会发布的稳定文件名（见 release.yml 的 stable aliases），各自带 .sha256 / .sig。
const STABLE_RELEASE_ASSET_NAMES: &[&str] = &[
    "iterate_aarch64.dmg",
    "iterate-macos.zip",
    "iterate-windows-x64.zip",
];

/// 不经 API：请求发布页的 latest 地址但不跟随跳转，从 Location 里取 tag。
async fn fetch_latest_release_via_redirect() -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建请求失败: {e}"))?;
    let response = client
        .get(PUBLIC_RELEASES_LATEST_PAGE_URL)
        .header("User-Agent", "cunzhi-app/1.0")
        .send()
        .await
        .map_err(|e| format!("请求发布页失败: {e}"))?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| format!("发布页没有返回跳转地址 ({})", response.status()))?;
    let tag = latest_tag_from_redirect_location(location)
        .ok_or_else(|| format!("无法从跳转地址解析版本: {location}"))?;
    log::info!("🏷️ 发布页跳转得到 tag: {}", tag);
    Ok(synthetic_release_for_tag(&tag))
}

fn latest_tag_from_redirect_location(location: &str) -> Option<String> {
    let path = location.split(['?', '#']).next()?;
    let tag = path.rsplit_once("/releases/tag/")?.1.trim_end_matches('/');
    let valid = !tag.is_empty()
        && tag.len() <= 64
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    valid.then(|| tag.to_string())
}

/// 用稳定文件名拼出与 API 结构一致的 release，后续的资产选择、可信来源与签名校验保持不变。
fn synthetic_release_for_tag(tag: &str) -> serde_json::Value {
    let download_base = format!("https://github.com{TRUSTED_RELEASE_DOWNLOAD_PATH_PREFIX}{tag}");
    let assets: Vec<serde_json::Value> = STABLE_RELEASE_ASSET_NAMES
        .iter()
        .flat_map(|name| {
            [
                name.to_string(),
                format!("{name}.sha256"),
                format!("{name}.sig"),
            ]
        })
        .map(|name| {
            serde_json::json!({
                "name": name,
                "browser_download_url": format!("{download_base}/{name}"),
            })
        })
        .collect();
    serde_json::json!({
        "tag_name": tag,
        "html_url": format!("https://github.com/kexin94yyds/iterate-releases/releases/tag/{tag}"),
        "body": "GitHub API 暂时受限，未能获取更新说明，请在发布页查看。",
        "assets": assets,
    })
}

/// 简单的版本比较函数
fn compare_versions(v1: &str, v2: &str) -> bool {
    let v1_parts: Vec<u32> = v1.split('.').filter_map(|s| s.parse().ok()).collect();
    let v2_parts: Vec<u32> = v2.split('.').filter_map(|s| s.parse().ok()).collect();

    let max_len = v1_parts.len().max(v2_parts.len());

    for i in 0..max_len {
        let v1_part = v1_parts.get(i).unwrap_or(&0);
        let v2_part = v2_parts.get(i).unwrap_or(&0);

        if v1_part > v2_part {
            return true;
        } else if v1_part < v2_part {
            return false;
        }
    }

    false
}

/// 下载并安装更新
#[tauri::command]
pub async fn download_and_install_update(app: AppHandle) -> Result<(), String> {
    log::info!("🚀 开始下载和安装更新");

    // 首先检查更新信息
    log::info!("🔍 重新检查更新信息");
    let update_info = check_for_updates(app.clone()).await?;

    log::info!("📊 更新信息: {:?}", update_info);

    if !update_info.available {
        let error_msg = "没有可用的更新".to_string();
        log::warn!("⚠️ {}", error_msg);
        return Err(error_msg);
    }

    log::info!("✅ 确认有可用更新，准备下载");

    // 发送下载开始事件
    log::info!("📢 发送下载开始事件");
    let _ = app.emit("update_download_started", ());

    // 实现真正的下载和安装逻辑
    match download_and_install_update_impl(&app, &update_info).await {
        Ok(_) => {
            log::info!("✅ 更新下载和安装成功");
            let _ = app.emit("update_install_finished", ());
            Ok(())
        }
        Err(e) => {
            log::error!("❌ 更新失败: {}", e);

            // 如果自动更新失败，提供手动下载选项
            log::info!("🔗 发送手动下载事件，URL: {}", update_info.download_url);
            let _ = app.emit("update_manual_download_required", &update_info.download_url);

            // 返回更友好的错误消息
            if e.contains("手动下载") {
                Err("请手动下载最新版本".to_string())
            } else {
                Err(format!("自动更新失败，请手动下载最新版本: {}", e))
            }
        }
    }
}

/// 获取当前应用版本
#[tauri::command]
pub async fn get_current_version(app: AppHandle) -> Result<String, String> {
    Ok(app.package_info().version.to_string())
}

/// 重启应用以完成更新
#[tauri::command]
pub async fn restart_app(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if macos_pending_update_marker_path().exists() {
        return crate::ui::exit::force_exit_app(app).await;
    }

    app.restart();
}

#[cfg(target_os = "macos")]
enum MacOsUpdateCleanup {
    DetachDmg(PathBuf),
    RemoveDir(PathBuf),
}

#[cfg(target_os = "macos")]
fn macos_pending_update_marker_path() -> PathBuf {
    std::env::temp_dir().join("cunzhi_macos_pending_update")
}

#[cfg(target_os = "macos")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "macos")]
fn current_app_bundle_path() -> Result<PathBuf, String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("无法获取当前可执行文件路径: {}", e))?;
    let app_path = current_exe
        .ancestors()
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("app"))
        .ok_or_else(|| "当前进程不在 .app bundle 内，无法执行整包替换".to_string())?;
    Ok(app_path.to_path_buf())
}

#[cfg(target_os = "macos")]
fn mount_dmg(file_path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(file_path)
        .output()
        .map_err(|e| format!("挂载 DMG 失败: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "挂载 DMG 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().rev() {
        if let Some(mount_point) = line.split('\t').next_back() {
            let trimmed = mount_point.trim();
            if trimmed.starts_with("/Volumes/") {
                return Ok(PathBuf::from(trimmed));
            }
        }
    }

    Err(format!("挂载 DMG 成功，但无法解析挂载点: {}", stdout))
}

#[cfg(target_os = "macos")]
fn find_app_bundle_in_dir(dir: &Path) -> Result<PathBuf, String> {
    fn find(dir: &Path, apps: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| format!("读取目录失败: {}", e))? {
            let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("app") {
                apps.push(path);
                continue;
            }
            if path.is_dir() {
                find(&path, apps)?;
            }
        }
        Ok(())
    }

    let mut apps = Vec::new();
    find(dir, &mut apps)?;
    apps.into_iter()
        .next()
        .ok_or_else(|| format!("未在 {} 中找到 .app 安装包", dir.display()))
}

#[cfg(target_os = "macos")]
fn macos_codesign_details(app_path: &Path) -> Result<MacOsCodeSignatureDetails, String> {
    let output = Command::new("codesign")
        .args(["-dv", "--verbose=4"])
        .arg(app_path)
        .output()
        .map_err(|e| format!("读取更新包签名详情失败: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "读取更新包签名详情失败: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut details = MacOsCodeSignatureDetails {
        team_identifier: None,
        bundle_identifier: None,
    };

    for line in stderr.lines() {
        if let Some(value) = line.strip_prefix("TeamIdentifier=") {
            details.team_identifier = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Identifier=") {
            details.bundle_identifier = Some(value.trim().to_string());
        }
    }

    Ok(details)
}

#[cfg(target_os = "macos")]
fn verify_macos_update_app_bundle(app_path: &Path) -> Result<(), String> {
    let codesign_output = Command::new("codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=2"])
        .arg(app_path)
        .output()
        .map_err(|e| format!("执行更新包代码签名校验失败: {}", e))?;

    if !codesign_output.status.success() {
        return Err(format!(
            "更新包代码签名校验失败: {}",
            String::from_utf8_lossy(&codesign_output.stderr)
        ));
    }

    let details = macos_codesign_details(app_path)?;
    macos_update_signature_details_trusted(&details)?;

    let gatekeeper_output = Command::new("spctl")
        .args(["--assess", "--type", "execute", "--verbose=4"])
        .arg(app_path)
        .output()
        .map_err(|e| format!("执行 Gatekeeper 校验失败: {}", e))?;

    if !gatekeeper_output.status.success() {
        return Err(format!(
            "更新包 Gatekeeper 校验失败: {}",
            String::from_utf8_lossy(&gatekeeper_output.stderr)
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn stage_macos_bundle_replace(
    new_app_path: &Path,
    cleanup: Option<MacOsUpdateCleanup>,
) -> Result<(), String> {
    let current_app_path = current_app_bundle_path()?;
    let marker_path = macos_pending_update_marker_path();

    if let Some(parent) = marker_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建更新标记目录失败: {}", e))?;
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("获取时间戳失败: {}", e))?
        .as_secs();
    let script_dir = std::env::temp_dir().join(format!("cunzhi_macos_bundle_update_{stamp}"));
    fs::create_dir_all(&script_dir).map_err(|e| format!("创建更新脚本目录失败: {}", e))?;

    let script_path = script_dir.join("apply_update.sh");
    let log_path = script_dir.join("apply_update.log");
    let cleanup_cmd = match cleanup {
        Some(MacOsUpdateCleanup::DetachDmg(mount_point)) => format!(
            "hdiutil detach {} -quiet >/dev/null 2>&1 || true",
            shell_quote(&mount_point.to_string_lossy())
        ),
        Some(MacOsUpdateCleanup::RemoveDir(dir)) => format!(
            "rm -rf {} >/dev/null 2>&1 || true",
            shell_quote(&dir.to_string_lossy())
        ),
        None => "true".to_string(),
    };

    let script = format!(
        r#"#!/bin/sh
set -eu

APP_PATH={app_path}
APP_BIN={app_bin}
SOURCE_APP={source_app}
MARKER_PATH={marker_path}
SELF_PATH={self_path}
LOG_PATH={log_path}

exec >>"$LOG_PATH" 2>&1
trap 'rm -f "$MARKER_PATH" "$SELF_PATH"' EXIT

for _ in $(seq 1 120); do
  if ! pgrep -f -- "$APP_BIN" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

TMP_APP="${{APP_PATH}}.updating"
rm -rf "$TMP_APP"
ditto "$SOURCE_APP" "$TMP_APP"
rm -rf "$APP_PATH"
mv "$TMP_APP" "$APP_PATH"
{cleanup_cmd}
open "$APP_PATH" >/dev/null 2>&1 || true
"#,
        app_path = shell_quote(&current_app_path.to_string_lossy()),
        app_bin = shell_quote(
            &current_app_path
                .join("Contents/MacOS/iterate")
                .to_string_lossy()
        ),
        source_app = shell_quote(&new_app_path.to_string_lossy()),
        marker_path = shell_quote(&marker_path.to_string_lossy()),
        self_path = shell_quote(&script_path.to_string_lossy()),
        log_path = shell_quote(&log_path.to_string_lossy()),
        cleanup_cmd = cleanup_cmd,
    );

    fs::write(&script_path, script).map_err(|e| format!("写入更新脚本失败: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)
            .map_err(|e| format!("读取更新脚本权限失败: {}", e))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)
            .map_err(|e| format!("设置更新脚本权限失败: {}", e))?;
    }

    fs::write(&marker_path, script_path.to_string_lossy().to_string())
        .map_err(|e| format!("写入更新标记失败: {}", e))?;

    Command::new("sh")
        .arg(&script_path)
        .spawn()
        .map_err(|e| format!("启动 macOS 更新脚本失败: {}", e))?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlatformDownloadAsset {
    name: String,
    download_url: String,
    sha256_url: String,
    signature_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseAsset {
    name: String,
    download_url: String,
}

fn release_asset_from_json(asset: &serde_json::Value) -> Option<ReleaseAsset> {
    let name = asset["name"].as_str()?.to_string();
    let download_url = asset["browser_download_url"].as_str()?.to_string();
    if !release_download_url_is_trusted(&download_url) {
        log::warn!(
            "忽略非信任来源的更新资产 URL: asset={} url={}",
            name,
            download_url
        );
        return None;
    }
    Some(ReleaseAsset { name, download_url })
}

fn release_page_download_asset(release: &serde_json::Value) -> PlatformDownloadAsset {
    PlatformDownloadAsset {
        name: String::new(),
        download_url: release["html_url"].as_str().unwrap_or("").to_string(),
        sha256_url: String::new(),
        signature_url: String::new(),
    }
}

fn checksum_asset_url_for(assets: &[serde_json::Value], asset_name: &str) -> Option<String> {
    let exact_names = [
        format!("{asset_name}.sha256"),
        format!("{asset_name}.sha256sum"),
        format!("{asset_name}.sha256.txt"),
    ];

    for candidate_name in &exact_names {
        if let Some(asset) = assets
            .iter()
            .filter_map(release_asset_from_json)
            .find(|asset| asset.name.eq_ignore_ascii_case(candidate_name))
        {
            return Some(asset.download_url);
        }
    }

    const GENERAL_CHECKSUM_FILES: &[&str] = &[
        "SHA256SUMS",
        "SHA256SUMS.txt",
        "checksums.txt",
        "checksums.sha256",
    ];
    assets
        .iter()
        .filter_map(release_asset_from_json)
        .find(|asset| {
            GENERAL_CHECKSUM_FILES
                .iter()
                .any(|candidate| asset.name.eq_ignore_ascii_case(candidate))
        })
        .map(|asset| asset.download_url)
}

fn signature_asset_url_for(assets: &[serde_json::Value], asset_name: &str) -> Option<String> {
    let exact_names = [
        format!("{asset_name}.sig"),
        format!("{asset_name}.ed25519"),
        format!("{asset_name}.sig.txt"),
    ];

    exact_names.iter().find_map(|candidate_name| {
        assets
            .iter()
            .filter_map(release_asset_from_json)
            .find(|asset| asset.name.eq_ignore_ascii_case(candidate_name))
            .map(|asset| asset.download_url)
    })
}

fn requires_detached_update_signature_for_current_platform() -> bool {
    cfg!(target_os = "windows") || cfg!(target_os = "linux")
}

fn platform_download_asset_with_checksum(
    assets: &[serde_json::Value],
    selected: ReleaseAsset,
    require_signature: bool,
) -> Result<PlatformDownloadAsset, String> {
    let sha256_url = checksum_asset_url_for(assets, &selected.name)
        .ok_or_else(|| format!("更新包缺少 SHA-256 校验文件: {}", selected.name))?;
    let signature_url = signature_asset_url_for(assets, &selected.name);
    if require_signature && signature_url.is_none() {
        return Err(format!("更新包缺少 Ed25519 签名文件: {}", selected.name));
    }

    Ok(PlatformDownloadAsset {
        name: selected.name,
        download_url: selected.download_url,
        sha256_url,
        signature_url: signature_url.unwrap_or_default(),
    })
}

/// 获取当前平台对应的下载资源
fn get_platform_download_asset(
    release: &serde_json::Value,
) -> Result<PlatformDownloadAsset, String> {
    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| "无法获取release assets".to_string())?;

    log::info!("📦 Release assets 总数: {}", assets.len());

    if cfg!(target_os = "macos") {
        let arch_markers: &[&str] = if cfg!(target_arch = "aarch64") {
            &["aarch64", "arm64", "macos-aarch64"]
        } else {
            &["x86_64", "x64", "macos-x86_64"]
        };

        for asset in assets {
            if let Some(name) = asset["name"].as_str() {
                let is_matching_dmg = name.ends_with(".dmg")
                    && arch_markers.iter().any(|marker| name.contains(marker));
                if is_matching_dmg {
                    if let Some(selected) = release_asset_from_json(asset) {
                        log::info!("✅ macOS 优先选择 DMG 安装包: {}", name);
                        log::info!("🔗 下载URL: {}", selected.download_url);
                        return platform_download_asset_with_checksum(
                            assets,
                            selected,
                            requires_detached_update_signature_for_current_platform(),
                        );
                    }
                }
            }
        }
    }

    let (platform, exact_matches, substring_matches): (&str, &[&str], &[&str]) =
        if cfg!(target_os = "macos") {
            if cfg!(target_arch = "aarch64") {
                ("macos-aarch64", &[], &["macos-aarch64"])
            } else {
                ("macos-x86_64", &[], &["macos-x86_64"])
            }
        } else if cfg!(target_os = "windows") {
            if cfg!(target_arch = "aarch64") {
                (
                    "windows-aarch64",
                    &["iterate-windows-arm64.zip"],
                    &["windows-aarch64", "windows-arm64"],
                )
            } else {
                (
                    "windows-x86_64",
                    &["iterate-windows-x64.zip"],
                    &["windows-x86_64", "windows-x64"],
                )
            }
        } else if cfg!(target_os = "linux") {
            if cfg!(target_arch = "aarch64") {
                ("linux-aarch64", &[], &["linux-aarch64"])
            } else {
                ("linux-x86_64", &[], &["linux-x86_64"])
            }
        } else {
            return Err("不支持的平台".to_string());
        };

    log::info!("🔍 查找平台 {} 的下载文件", platform);

    // 列出所有可用的 assets
    for (i, asset) in assets.iter().enumerate() {
        if let Some(name) = asset["name"].as_str() {
            log::info!("📄 Asset {}: {}", i + 1, name);
        }
    }

    for asset in assets {
        if let Some(name) = asset["name"].as_str() {
            if exact_matches.iter().any(|candidate| name == *candidate) {
                if let Some(selected) = release_asset_from_json(asset) {
                    log::info!("✅ 找到平台 {} 的稳定下载文件: {}", platform, name);
                    log::info!("🔗 下载URL: {}", selected.download_url);
                    return platform_download_asset_with_checksum(
                        assets,
                        selected,
                        requires_detached_update_signature_for_current_platform(),
                    );
                }
            }
        }
    }

    // 查找对应平台的文件
    for asset in assets {
        if let Some(name) = asset["name"].as_str() {
            log::info!("🔍 检查文件: {} (候选匹配 {:?})", name, substring_matches);
            if substring_matches
                .iter()
                .any(|candidate| name.contains(candidate))
            {
                if let Some(selected) = release_asset_from_json(asset) {
                    log::info!("✅ 找到匹配的下载文件: {}", name);
                    log::info!("🔗 下载URL: {}", selected.download_url);
                    return platform_download_asset_with_checksum(
                        assets,
                        selected,
                        requires_detached_update_signature_for_current_platform(),
                    );
                }
            }
        }
    }

    // 如果找不到对应平台的文件，返回release页面URL作为fallback
    log::warn!("⚠️ 未找到平台 {} 的下载文件，使用release页面", platform);
    log::warn!("💡 可能的原因：1. 该平台没有预编译版本 2. 文件名格式不匹配");
    Ok(release_page_download_asset(release))
}

/// 实际的下载和安装实现
async fn download_and_install_update_impl(
    app: &AppHandle,
    update_info: &UpdateInfo,
) -> Result<(), String> {
    log::info!("🚀 开始自动更新实现");
    log::info!("📋 更新信息: {:?}", update_info);

    // 如果下载URL是GitHub页面而不是直接下载链接，引导用户手动下载
    if update_info.download_url.contains("/releases/tag/") {
        log::info!(
            "🔗 下载URL是release页面，需要手动下载: {}",
            update_info.download_url
        );
        log::info!("💡 这通常意味着没有找到当前平台的预编译版本");
        return Err("请手动下载最新版本".to_string());
    }

    log::info!("📥 开始下载文件: {}", update_info.download_url);
    if update_info.sha256_url.trim().is_empty() {
        return Err("更新包缺少 SHA-256 校验文件".to_string());
    }

    // 创建临时目录
    let temp_dir = std::env::temp_dir().join("cunzhi_update");
    fs::create_dir_all(&temp_dir).map_err(|e| format!("创建临时目录失败: {}", e))?;

    // 确定文件名
    let file_name = update_info
        .download_url
        .split('/')
        .next_back()
        .unwrap_or("update_file")
        .to_string();

    let file_path = temp_dir.join(&file_name);

    // 下载文件
    let client = reqwest::Client::new();
    let mut response = client
        .get(&update_info.download_url)
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("下载失败: HTTP {}", response.status()));
    }

    let total_size = response.content_length();
    let mut downloaded = 0u64;
    let mut file = fs::File::create(&file_path).map_err(|e| format!("创建文件失败: {}", e))?;

    // 下载并报告进度
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("下载数据失败: {}", e))?
    {
        file.write_all(&chunk)
            .map_err(|e| format!("写入文件失败: {}", e))?;

        downloaded += chunk.len() as u64;

        let percentage = if let Some(total) = total_size {
            (downloaded as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        let progress = UpdateProgress {
            chunk_length: chunk.len(),
            content_length: total_size,
            downloaded,
            percentage,
        };

        let _ = app.emit("update_download_progress", &progress);
    }

    log::info!("✅ 文件下载完成: {}", file_path.display());

    let expected_sha256 =
        download_expected_sha256(&client, &update_info.sha256_url, &file_name).await?;
    verify_file_sha256(&file_path, &expected_sha256)?;
    log::info!("✅ 更新包 SHA-256 校验通过: {}", file_name);
    if requires_detached_update_signature_for_current_platform() {
        if update_info.signature_url.trim().is_empty() {
            return Err("非 macOS 更新包缺少 Ed25519 签名文件".to_string());
        }
        download_and_verify_update_signature(&client, &update_info.signature_url, &file_path)
            .await?;
        log::info!("✅ 更新包 Ed25519 签名校验通过: {}", file_name);
    }

    // 开始安装
    let _ = app.emit("update_install_started", ());

    // 根据平台执行不同的安装逻辑
    install_update(&file_path).await?;

    Ok(())
}

async fn download_expected_sha256(
    client: &reqwest::Client,
    sha256_url: &str,
    asset_name: &str,
) -> Result<String, String> {
    log::info!("📥 下载 SHA-256 校验文件: {}", sha256_url);
    let response = client
        .get(sha256_url)
        .send()
        .await
        .map_err(|e| format!("下载 SHA-256 校验文件失败: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "下载 SHA-256 校验文件失败: HTTP {}",
            response.status()
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 SHA-256 校验文件失败: {}", e))?;
    parse_sha256_checksum(&body, asset_name)
}

fn normalize_sha256_hex(value: &str) -> Result<String, String> {
    let value = value.trim().strip_prefix("sha256:").unwrap_or(value.trim());
    if value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err("无效的 SHA-256 校验值".to_string())
    }
}

fn parse_sha256_checksum(content: &str, asset_name: &str) -> Result<String, String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(first) = parts.next() else {
            continue;
        };
        let Ok(hash) = normalize_sha256_hex(first) else {
            continue;
        };

        let names: Vec<&str> = parts.collect();
        if names.is_empty() {
            return Ok(hash);
        }

        if names.iter().any(|name| {
            name.trim_start_matches('*')
                .rsplit(['/', '\\'])
                .next()
                .is_some_and(|candidate| candidate == asset_name)
        }) {
            return Ok(hash);
        }
    }

    Err(format!(
        "SHA-256 校验文件中未找到安装包条目: {}",
        asset_name
    ))
}

fn verify_file_sha256(file_path: &Path, expected_sha256: &str) -> Result<(), String> {
    let expected_sha256 = normalize_sha256_hex(expected_sha256)?;
    let bytes = fs::read(file_path).map_err(|e| format!("读取更新包失败: {}", e))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let actual_sha256 = hex::encode(digest.as_ref());
    if actual_sha256 != expected_sha256 {
        return Err(format!(
            "更新包 SHA-256 校验失败: expected={}, actual={}",
            expected_sha256, actual_sha256
        ));
    }

    Ok(())
}

async fn download_and_verify_update_signature(
    client: &reqwest::Client,
    signature_url: &str,
    file_path: &Path,
) -> Result<(), String> {
    log::info!("📥 下载 Ed25519 签名文件: {}", signature_url);
    let response = client
        .get(signature_url)
        .send()
        .await
        .map_err(|e| format!("下载 Ed25519 签名文件失败: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "下载 Ed25519 签名文件失败: HTTP {}",
            response.status()
        ));
    }

    let signature_text = response
        .text()
        .await
        .map_err(|e| format!("读取 Ed25519 签名文件失败: {}", e))?;
    let payload = fs::read(file_path).map_err(|e| format!("读取更新包失败: {}", e))?;
    verify_update_signature_bytes(&payload, &signature_text)
}

fn configured_release_public_key_bytes() -> Result<Vec<u8>, String> {
    let runtime_public_key = std::env::var(RELEASE_PUBLIC_KEY_ENV).ok();
    configured_release_public_key_bytes_from_sources(
        runtime_public_key.as_deref(),
        EMBEDDED_RELEASE_PUBLIC_KEY_B64,
    )
}

fn configured_release_public_key_bytes_from_sources(
    runtime_public_key: Option<&str>,
    embedded_public_key: Option<&str>,
) -> Result<Vec<u8>, String> {
    let public_key = runtime_public_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            embedded_public_key
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| {
            format!(
                "缺少 {}（运行时或编译期），无法验证非 macOS 更新包签名",
                RELEASE_PUBLIC_KEY_ENV
            )
        })?;

    decode_update_signature_material(public_key, "Ed25519 公钥")
}

fn parse_update_signature_bytes(signature_text: &str) -> Result<Vec<u8>, String> {
    for line in signature_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some(first) = line.split_whitespace().next() else {
            continue;
        };
        return decode_update_signature_material(first, "Ed25519 签名");
    }

    Err("Ed25519 签名文件为空".to_string())
}

fn decode_update_signature_material(value: &str, label: &str) -> Result<Vec<u8>, String> {
    let value = value
        .trim()
        .strip_prefix("ed25519:")
        .or_else(|| value.trim().strip_prefix("signature:"))
        .unwrap_or(value.trim());

    if let Ok(bytes) = STANDARD.decode(value) {
        return Ok(bytes);
    }
    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(value) {
        return Ok(bytes);
    }
    if value.len() % 2 == 0 && value.chars().all(|c| c.is_ascii_hexdigit()) {
        return hex::decode(value).map_err(|e| format!("{} hex 解码失败: {}", label, e));
    }

    Err(format!("{} 不是有效的 base64 或 hex 编码", label))
}

fn verify_update_signature_bytes(payload: &[u8], signature_text: &str) -> Result<(), String> {
    let public_key = configured_release_public_key_bytes()?;
    verify_update_signature_bytes_with_public_key(payload, signature_text, public_key)
}

#[cfg(test)]
fn verify_update_signature_bytes_with_public_key_sources(
    payload: &[u8],
    signature_text: &str,
    runtime_public_key: Option<&str>,
    embedded_public_key: Option<&str>,
) -> Result<(), String> {
    let public_key =
        configured_release_public_key_bytes_from_sources(runtime_public_key, embedded_public_key)?;
    verify_update_signature_bytes_with_public_key(payload, signature_text, public_key)
}

fn verify_update_signature_bytes_with_public_key(
    payload: &[u8],
    signature_text: &str,
    public_key: Vec<u8>,
) -> Result<(), String> {
    let signature_bytes = parse_update_signature_bytes(signature_text)?;
    let verifier = UnparsedPublicKey::new(&signature::ED25519, public_key);
    verifier
        .verify(payload, &signature_bytes)
        .map_err(|_| "更新包 Ed25519 签名无效".to_string())
}

/// 根据平台安装更新
async fn install_update(file_path: &PathBuf) -> Result<(), String> {
    log::info!("🔧 开始安装更新: {}", file_path.display());

    if cfg!(target_os = "macos") {
        install_macos_update(file_path).await
    } else {
        Err(
            "当前平台已完成更新包完整性与签名校验，但自动安装流程尚未实现，请手动安装最新版本"
                .to_string(),
        )
    }
}

/// macOS 安装逻辑
#[cfg(target_os = "macos")]
async fn install_macos_update(file_path: &PathBuf) -> Result<(), String> {
    let file_name = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if file_name.ends_with(".tar.gz") || file_name.ends_with(".zip") {
        log::info!("📦 处理 macOS 压缩包文件，改为整包替换 .app");
        let temp_dir = std::env::temp_dir().join("cunzhi_extract");
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir).map_err(|e| format!("清理临时目录失败: {}", e))?;
        }
        fs::create_dir_all(&temp_dir).map_err(|e| format!("创建临时解压目录失败: {}", e))?;

        if file_name.ends_with(".tar.gz") {
            extract_tar_gz(file_path, &temp_dir)?;
        } else {
            extract_zip(file_path, &temp_dir)?;
        }

        let new_app = find_app_bundle_in_dir(&temp_dir)?;
        verify_macos_update_app_bundle(&new_app)?;
        stage_macos_bundle_replace(&new_app, Some(MacOsUpdateCleanup::RemoveDir(temp_dir)))?;
        Ok(())
    } else if file_name.ends_with(".dmg") {
        log::info!("📦 处理 DMG 文件，准备在应用退出后整体替换 .app bundle");
        let mount_point = mount_dmg(file_path)?;
        let new_app = find_app_bundle_in_dir(&mount_point)?;
        verify_macos_update_app_bundle(&new_app)?;
        stage_macos_bundle_replace(&new_app, Some(MacOsUpdateCleanup::DetachDmg(mount_point)))?;
        Ok(())
    } else {
        Err("未知的文件格式，请手动下载最新版本".to_string())
    }
}

#[cfg(not(target_os = "macos"))]
async fn install_macos_update(_file_path: &PathBuf) -> Result<(), String> {
    Err("macOS 更新安装只支持 macOS".to_string())
}

/// 解压 tar.gz 文件
fn extract_tar_gz(archive_path: &Path, extract_to: &Path) -> Result<(), String> {
    log::info!("📦 解压 tar.gz 文件");

    let output = Command::new("tar")
        .args([
            "-xzf",
            archive_path.to_str().unwrap(),
            "-C",
            extract_to.to_str().unwrap(),
        ])
        .output()
        .map_err(|e| format!("执行 tar 命令失败: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "tar 解压失败: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    log::info!("✅ tar.gz 解压完成");
    Ok(())
}

/// 解压 zip 文件
fn extract_zip(archive_path: &Path, extract_to: &Path) -> Result<(), String> {
    log::info!("📦 解压 zip 文件");

    // Windows 使用 PowerShell 解压
    if cfg!(target_os = "windows") {
        let ps_command = format!(
            "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
            archive_path.display(),
            extract_to.display()
        );

        let output = Command::new("powershell")
            .args(["-Command", &ps_command])
            .output()
            .map_err(|e| format!("执行 PowerShell 命令失败: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "PowerShell 解压失败: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    } else {
        // Unix 系统使用 unzip
        let output = Command::new("unzip")
            .args([
                "-o",
                archive_path.to_str().unwrap(),
                "-d",
                extract_to.to_str().unwrap(),
            ])
            .output()
            .map_err(|e| format!("执行 unzip 命令失败: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "unzip 解压失败: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }

    log::info!("✅ zip 解压完成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        get_platform_download_asset, latest_tag_from_redirect_location,
        macos_update_signature_details_trusted, parse_sha256_checksum,
        platform_download_asset_with_checksum, release_download_url_is_trusted,
        synthetic_release_for_tag, verify_file_sha256, verify_update_signature_bytes,
        verify_update_signature_bytes_with_public_key_sources, MacOsCodeSignatureDetails,
        ReleaseAsset, RELEASE_PUBLIC_KEY_ENV,
    };
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    #[test]
    fn latest_redirect_location_yields_tag() {
        assert_eq!(
            latest_tag_from_redirect_location(
                "https://github.com/kexin94yyds/iterate-releases/releases/tag/v0.6.6"
            )
            .as_deref(),
            Some("v0.6.6")
        );
        assert_eq!(
            latest_tag_from_redirect_location(
                "/kexin94yyds/iterate-releases/releases/tag/v1.2.3/?x=1#y"
            )
            .as_deref(),
            Some("v1.2.3")
        );
        assert_eq!(
            latest_tag_from_redirect_location("https://github.com/login"),
            None
        );
        assert_eq!(
            latest_tag_from_redirect_location(
                "https://github.com/kexin94yyds/iterate-releases/releases/tag/v1/../../evil"
            ),
            None
        );
        assert_eq!(
            latest_tag_from_redirect_location(
                "https://github.com/kexin94yyds/iterate-releases/releases/tag/"
            ),
            None
        );
    }

    #[test]
    fn synthetic_release_selects_signed_trusted_platform_asset() {
        let release = synthetic_release_for_tag("v0.6.6");
        assert_eq!(release["tag_name"], "v0.6.6");
        let asset = get_platform_download_asset(&release).expect("platform asset");
        let prefix = "https://github.com/kexin94yyds/iterate-releases/releases/download/v0.6.6/";
        assert!(
            asset.download_url.starts_with(prefix),
            "{}",
            asset.download_url
        );
        assert!(release_download_url_is_trusted(&asset.download_url));
        assert_eq!(asset.sha256_url, format!("{}.sha256", asset.download_url));
        assert_eq!(asset.signature_url, format!("{}.sig", asset.download_url));
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(asset.name, "iterate_aarch64.dmg");
        }
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            assert_eq!(asset.name, "iterate-windows-x64.zip");
        }
    }
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::json;
    use std::sync::Mutex;

    static UPDATE_SIGNATURE_TEST_ENV_LOCK: once_cell::sync::Lazy<Mutex<()>> =
        once_cell::sync::Lazy::new(|| Mutex::new(()));

    fn current_platform_asset_name() -> &'static str {
        if cfg!(target_os = "macos") {
            if cfg!(target_arch = "aarch64") {
                "iterate-macos-aarch64.dmg"
            } else {
                "iterate-macos-x86_64.dmg"
            }
        } else if cfg!(target_os = "windows") {
            if cfg!(target_arch = "aarch64") {
                "iterate-windows-arm64.zip"
            } else {
                "iterate-windows-x64.zip"
            }
        } else if cfg!(target_os = "linux") {
            if cfg!(target_arch = "aarch64") {
                "iterate-linux-aarch64.tar.gz"
            } else {
                "iterate-linux-x86_64.tar.gz"
            }
        } else {
            "iterate-unsupported.zip"
        }
    }

    fn trusted_release_asset_url(asset_name: &str) -> String {
        format!(
            "https://github.com/kexin94yyds/iterate-releases/releases/download/v9.9.9/{asset_name}"
        )
    }

    struct ReleasePublicKeyGuard {
        previous: Option<String>,
    }

    impl ReleasePublicKeyGuard {
        fn set(value: &str) -> Self {
            let previous = std::env::var(RELEASE_PUBLIC_KEY_ENV).ok();
            std::env::set_var(RELEASE_PUBLIC_KEY_ENV, value);
            Self { previous }
        }

        fn clear() -> Self {
            let previous = std::env::var(RELEASE_PUBLIC_KEY_ENV).ok();
            std::env::remove_var(RELEASE_PUBLIC_KEY_ENV);
            Self { previous }
        }
    }

    impl Drop for ReleasePublicKeyGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(RELEASE_PUBLIC_KEY_ENV, previous);
            } else {
                std::env::remove_var(RELEASE_PUBLIC_KEY_ENV);
            }
        }
    }

    #[test]
    fn platform_download_asset_rejects_missing_sha256_asset() {
        let asset_name = current_platform_asset_name();
        let release = json!({
            "html_url": "https://github.com/kexin94yyds/iterate-releases/releases/tag/v9.9.9",
            "assets": [
                {
                    "name": asset_name,
                    "browser_download_url": trusted_release_asset_url(asset_name)
                }
            ]
        });

        let error = get_platform_download_asset(&release)
            .expect_err("installer assets without checksum metadata must be rejected");
        assert!(error.contains("SHA-256"));
    }

    #[test]
    fn platform_download_asset_carries_sha256_companion_url() {
        let asset_name = current_platform_asset_name();
        let checksum_name = format!("{asset_name}.sha256");
        let signature_name = format!("{asset_name}.sig");
        let release = json!({
            "html_url": "https://github.com/kexin94yyds/iterate-releases/releases/tag/v9.9.9",
            "assets": [
                {
                    "name": asset_name,
                    "browser_download_url": trusted_release_asset_url(asset_name)
                },
                {
                    "name": checksum_name,
                    "browser_download_url": trusted_release_asset_url(&checksum_name)
                },
                {
                    "name": signature_name,
                    "browser_download_url": trusted_release_asset_url(&signature_name)
                }
            ]
        });

        let selected = get_platform_download_asset(&release).expect("checksum asset should match");
        assert_eq!(selected.name, asset_name);
        assert_eq!(selected.download_url, trusted_release_asset_url(asset_name));
        assert_eq!(
            selected.sha256_url,
            trusted_release_asset_url(&checksum_name)
        );
        assert_eq!(
            selected.signature_url,
            trusted_release_asset_url(&signature_name)
        );
    }

    #[test]
    fn non_macos_asset_policy_rejects_missing_signature_when_required() {
        let asset_name = "iterate-linux-x86_64.tar.gz";
        let checksum_name = format!("{asset_name}.sha256");
        let assets = vec![
            json!({
                "name": asset_name,
                "browser_download_url": trusted_release_asset_url(asset_name)
            }),
            json!({
                "name": checksum_name,
                "browser_download_url": trusted_release_asset_url(&checksum_name)
            }),
        ];
        let selected = ReleaseAsset {
            name: asset_name.to_string(),
            download_url: trusted_release_asset_url(asset_name),
        };

        let error = platform_download_asset_with_checksum(&assets, selected, true)
            .expect_err("non-macOS installer assets without detached signatures must be rejected");
        assert!(error.contains("签名"));
    }

    #[test]
    fn update_signature_verification_requires_configured_public_key_and_matching_signature() {
        let _lock = UPDATE_SIGNATURE_TEST_ENV_LOCK.lock().unwrap();
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate release signing key");
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("parse signing key");
        let payload = b"verified update payload";
        let signature = key_pair.sign(payload);
        let signature_b64 = STANDARD.encode(signature.as_ref());

        let _missing_key = ReleasePublicKeyGuard::clear();
        assert!(verify_update_signature_bytes(payload, &signature_b64).is_err());
        drop(_missing_key);

        let _public_key = ReleasePublicKeyGuard::set(&STANDARD.encode(key_pair.public_key()));
        assert!(verify_update_signature_bytes(payload, &signature_b64).is_ok());
        assert!(verify_update_signature_bytes(b"tampered update payload", &signature_b64).is_err());
    }

    #[test]
    fn update_signature_verification_accepts_embedded_public_key_without_runtime_env() {
        let _lock = UPDATE_SIGNATURE_TEST_ENV_LOCK.lock().unwrap();
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate release signing key");
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("parse signing key");
        let payload = b"verified update payload";
        let signature = key_pair.sign(payload);
        let signature_b64 = STANDARD.encode(signature.as_ref());
        let embedded_public_key = STANDARD.encode(key_pair.public_key());

        let _missing_runtime_key = ReleasePublicKeyGuard::clear();
        assert!(verify_update_signature_bytes_with_public_key_sources(
            payload,
            &signature_b64,
            None,
            Some(&embedded_public_key),
        )
        .is_ok());
    }

    #[test]
    fn runtime_public_key_overrides_embedded_public_key_for_development() {
        let _lock = UPDATE_SIGNATURE_TEST_ENV_LOCK.lock().unwrap();
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate release signing key");
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("parse signing key");
        let payload = b"verified update payload";
        let signature = key_pair.sign(payload);
        let signature_b64 = STANDARD.encode(signature.as_ref());
        let runtime_public_key = STANDARD.encode(key_pair.public_key());
        let wrong_embedded_public_key = STANDARD.encode([7_u8; 32]);

        assert!(verify_update_signature_bytes_with_public_key_sources(
            payload,
            &signature_b64,
            Some(&runtime_public_key),
            Some(&wrong_embedded_public_key),
        )
        .is_ok());
    }

    #[test]
    fn release_download_url_policy_rejects_untrusted_asset_hosts() {
        assert!(release_download_url_is_trusted(
            "https://github.com/kexin94yyds/iterate-releases/releases/download/v9.9.9/iterate-macos-aarch64.dmg",
        ));
        assert!(!release_download_url_is_trusted(
            "https://example.test/iterate-macos-aarch64.dmg",
        ));
        assert!(!release_download_url_is_trusted(
            "https://github.com/other/repo/releases/download/v9.9.9/iterate-macos-aarch64.dmg",
        ));
        assert!(!release_download_url_is_trusted(
            "http://github.com/kexin94yyds/iterate-releases/releases/download/v9.9.9/iterate-macos-aarch64.dmg",
        ));
    }

    #[test]
    fn macos_update_signature_policy_requires_expected_team_and_bundle_id() {
        let trusted = MacOsCodeSignatureDetails {
            team_identifier: Some("UM3Z9G5DNH".to_string()),
            bundle_identifier: Some("com.kexin94yyds.iterate".to_string()),
        };
        assert!(macos_update_signature_details_trusted(&trusted).is_ok());

        let wrong_team = MacOsCodeSignatureDetails {
            team_identifier: Some("OTHERTEAMID".to_string()),
            bundle_identifier: Some("com.kexin94yyds.iterate".to_string()),
        };
        assert!(macos_update_signature_details_trusted(&wrong_team).is_err());

        let wrong_bundle = MacOsCodeSignatureDetails {
            team_identifier: Some("UM3Z9G5DNH".to_string()),
            bundle_identifier: Some("com.attacker.iterate".to_string()),
        };
        assert!(macos_update_signature_details_trusted(&wrong_bundle).is_err());

        let missing_identity = MacOsCodeSignatureDetails {
            team_identifier: None,
            bundle_identifier: None,
        };
        assert!(macos_update_signature_details_trusted(&missing_identity).is_err());
    }

    #[test]
    fn sha256_checksum_parser_accepts_single_hash_or_named_sum() {
        let expected = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_sha256_checksum(expected, "iterate-macos-aarch64.dmg").unwrap(),
            expected
        );
        assert_eq!(
            parse_sha256_checksum(
                &format!("{expected}  *iterate-macos-aarch64.dmg"),
                "iterate-macos-aarch64.dmg",
            )
            .unwrap(),
            expected
        );
    }

    #[test]
    fn file_sha256_verification_rejects_mismatched_update_payloads() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let update_path = temp_dir.path().join("iterate-macos-aarch64.dmg");
        std::fs::write(&update_path, b"update payload").expect("write update payload");

        assert!(verify_file_sha256(
            &update_path,
            "2dc876f11bc35fccb277bb8d60e2898a21ad55094d0f66197ee9632a427a7245",
        )
        .is_ok());
        assert!(verify_file_sha256(
            &update_path,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .is_err());
    }
}
