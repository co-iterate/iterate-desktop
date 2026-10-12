use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use super::code::{
    license_days, license_type_label, normalize_license_type, parse_and_verify_license_code,
};
use crate::config::cunzhi_config_dir;

pub const TRIAL_CONTACT_URL: &str = "https://iterate.xin/iterate/";
pub const TRIAL_EXPIRED_MESSAGE: &str = "试用期已结束";
pub const TRIAL_EXPIRED_SUBTITLE: &str =
    "请前往官网购买，或输入新的激活码继续使用。优惠码「无限迭代」可用于永久版。";
pub const ACTIVATION_REQUIRED_MESSAGE: &str = "欢迎使用 Iterate";
pub const ACTIVATION_REQUIRED_SUBTITLE: &str = "输入激活码开始使用";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseData {
    pub license_key: String,
    pub activated_at: String,
    #[serde(default = "default_last_validated_at")]
    pub last_validated_at: String,
    pub device_id: String,
    #[serde(default = "default_license_type")]
    pub license_type: String,
}

fn default_license_type() -> String {
    "permanent".to_string()
}

fn default_last_validated_at() -> String {
    String::new()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialData {
    pub first_launch_at: String,
    pub last_launch_at: String,
    pub device_id: String,
    pub installed_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialStatus {
    pub is_active: bool,
    pub is_expired: bool,
    pub days_remaining: u64,
    pub trial_days: u64,
    pub days_used: u64,
    pub first_launch_at: String,
    pub expires_at: String,
    pub contact_url: String,
    pub expired_message: String,
    pub expired_subtitle: String,
    pub time_anomaly: bool,
}

enum PersistedState<T> {
    Missing,
    Valid(T),
    Corrupted(String),
}

fn get_cunzhi_config_dir() -> Result<PathBuf> {
    cunzhi_config_dir()
}

fn get_license_data_path() -> Result<PathBuf> {
    Ok(get_cunzhi_config_dir()?.join(".license"))
}

fn load_license_state() -> Result<PersistedState<LicenseData>> {
    let path = get_license_data_path()?;
    if !path.exists() {
        return Ok(PersistedState::Missing);
    }
    let content = fs::read_to_string(&path)?;
    let decoded = match base64_decode(&content) {
        Ok(value) => value,
        Err(err) => return Ok(PersistedState::Corrupted(err.to_string())),
    };
    let data: LicenseData = match serde_json::from_str(&decoded) {
        Ok(value) => value,
        Err(err) => return Ok(PersistedState::Corrupted(err.to_string())),
    };
    Ok(PersistedState::Valid(data))
}

fn save_license_data(data: &LicenseData) -> Result<()> {
    let path = get_license_data_path()?;
    let json = serde_json::to_string(data)?;
    let encoded = base64_encode(&json);
    write_file_atomically(path, encoded.as_bytes())
}

fn write_file_atomically(path: PathBuf, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("无法获取授权文件目录"))?;
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow::anyhow!("授权文件名无效"))?;
    let temp_path = parent.join(format!(".{}.{}.tmp", file_name, std::process::id()));

    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
    }

    fs::rename(&temp_path, &path)?;
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn blocked_status(
    now: DateTime<Utc>,
    message: impl Into<String>,
    subtitle: impl Into<String>,
) -> TrialStatus {
    TrialStatus {
        is_active: false,
        is_expired: true,
        days_remaining: 0,
        trial_days: 0,
        days_used: 0,
        first_launch_at: now.to_rfc3339(),
        expires_at: String::new(),
        contact_url: TRIAL_CONTACT_URL.to_string(),
        expired_message: message.into(),
        expired_subtitle: subtitle.into(),
        time_anomaly: false,
    }
}

fn time_anomaly_status(
    now: DateTime<Utc>,
    title: impl Into<String>,
    subtitle: impl Into<String>,
) -> TrialStatus {
    TrialStatus {
        is_active: false,
        is_expired: true,
        days_remaining: 0,
        trial_days: 0,
        days_used: 0,
        first_launch_at: now.to_rfc3339(),
        expires_at: String::new(),
        contact_url: TRIAL_CONTACT_URL.to_string(),
        expired_message: title.into(),
        expired_subtitle: subtitle.into(),
        time_anomaly: true,
    }
}

fn parse_timestamp(value: &str, field_name: &str) -> Result<DateTime<Utc>> {
    value
        .parse::<DateTime<Utc>>()
        .map_err(|e| anyhow::anyhow!("解析{}失败: {}", field_name, e))
}

fn detect_time_anomaly(now: DateTime<Utc>, last_seen_at: DateTime<Utc>) -> bool {
    now < last_seen_at - chrono::Duration::hours(1)
}

fn trial_status_from_license(now: DateTime<Utc>, license: &mut LicenseData) -> Result<TrialStatus> {
    let payload = parse_and_verify_license_code(&license.license_key)?;
    let activated_at = parse_timestamp(&license.activated_at, "激活时间")?;
    let last_validated_at = if license.last_validated_at.trim().is_empty() {
        activated_at
    } else {
        parse_timestamp(&license.last_validated_at, "上次授权校验时间")?
    };

    if detect_time_anomaly(now, last_validated_at) {
        return Ok(time_anomaly_status(
            now,
            "授权状态异常",
            "检测到本地时间回退，已暂停授权，请校准系统时间后重新激活。",
        ));
    }

    let normalized_type = normalize_license_type(&payload.license_type)?;
    if license.license_type != normalized_type {
        license.license_type = normalized_type.to_string();
    }
    license.last_validated_at = now.to_rfc3339();
    save_license_data(license)?;

    if let Some(days) = license_days(normalized_type) {
        let expires_at = activated_at + chrono::Duration::days(days as i64);
        let elapsed = now.signed_duration_since(activated_at);
        let days_used = elapsed.num_days().max(0) as u64;
        let is_expired = now >= expires_at;

        return Ok(TrialStatus {
            is_active: !is_expired,
            is_expired,
            days_remaining: if is_expired {
                0
            } else {
                days.saturating_sub(days_used)
            },
            trial_days: days,
            days_used: days_used.min(days),
            first_launch_at: activated_at.to_rfc3339(),
            expires_at: expires_at.to_rfc3339(),
            contact_url: TRIAL_CONTACT_URL.to_string(),
            expired_message: if is_expired {
                TRIAL_EXPIRED_MESSAGE.to_string()
            } else {
                String::new()
            },
            expired_subtitle: if is_expired {
                TRIAL_EXPIRED_SUBTITLE.to_string()
            } else {
                String::new()
            },
            time_anomaly: false,
        });
    }

    Ok(TrialStatus {
        is_active: true,
        is_expired: false,
        days_remaining: 0,
        trial_days: 0,
        days_used: 0,
        first_launch_at: activated_at.to_rfc3339(),
        expires_at: String::new(),
        contact_url: TRIAL_CONTACT_URL.to_string(),
        expired_message: String::new(),
        expired_subtitle: String::new(),
        time_anomaly: false,
    })
}

fn current_license_status(now: DateTime<Utc>) -> Result<Option<TrialStatus>> {
    match load_license_state()? {
        PersistedState::Missing => Ok(None),
        PersistedState::Corrupted(reason) => Ok(Some(blocked_status(
            now,
            "本地授权状态损坏",
            format!("请修复或删除授权文件后重试。{}", reason),
        ))),
        PersistedState::Valid(mut license) => match trial_status_from_license(now, &mut license) {
            Ok(status) => Ok(Some(status)),
            Err(err) => Ok(Some(blocked_status(
                now,
                "激活码无效",
                format!("请检查激活码或重新获取。{}", err),
            ))),
        },
    }
}

fn license_priority(license_type: &str) -> u8 {
    match normalize_license_type(license_type).ok() {
        Some("day1") => 1,
        Some("day7") => 2,
        Some("permanent") => 3,
        _ => 0,
    }
}

fn resolve_existing_license_type(license: &LicenseData) -> Result<&'static str> {
    if let Ok(kind) = normalize_license_type(&license.license_type) {
        return Ok(kind);
    }

    let payload = parse_and_verify_license_code(&license.license_key)?;
    normalize_license_type(&payload.license_type)
}

#[tauri::command]
pub fn is_licensed() -> bool {
    current_license_status(Utc::now())
        .ok()
        .flatten()
        .map(|status| status.is_active && !status.is_expired)
        .unwrap_or(false)
}

pub fn activate(key: &str) -> Result<()> {
    let key = key.trim();
    let payload = parse_and_verify_license_code(key)?;
    let license_type = normalize_license_type(&payload.license_type)?;
    let device_id = match load_trial_state()? {
        PersistedState::Valid(trial) => trial.device_id,
        _ => generate_device_id(),
    };
    let now = Utc::now().to_rfc3339();
    let mut activated_at = now.clone();

    if let PersistedState::Valid(existing) = load_license_state()? {
        if existing.license_key == key {
            activated_at = existing.activated_at;
        } else if let Ok(existing_type) = resolve_existing_license_type(&existing) {
            if license_priority(license_type) < license_priority(existing_type) {
                anyhow::bail!(
                    "当前已存在更高等级授权（{}），不能降级覆盖",
                    license_type_label(existing_type)
                );
            }
        }
    }

    let license = LicenseData {
        license_key: key.to_string(),
        activated_at,
        last_validated_at: now,
        device_id,
        license_type: license_type.to_string(),
    };
    save_license_data(&license)?;

    log::info!(
        "[License] 激活成功: type={}, key={}",
        license_type_label(license_type),
        &key[..8]
    );
    Ok(())
}

fn get_trial_data_path() -> Result<PathBuf> {
    Ok(get_cunzhi_config_dir()?.join(".trial"))
}

fn generate_device_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn load_trial_state() -> Result<PersistedState<TrialData>> {
    let path = get_trial_data_path()?;
    if !path.exists() {
        return Ok(PersistedState::Missing);
    }
    let content = fs::read_to_string(&path)?;
    let decoded = match base64_decode(&content) {
        Ok(value) => value,
        Err(err) => return Ok(PersistedState::Corrupted(err.to_string())),
    };
    let data: TrialData = match serde_json::from_str(&decoded) {
        Ok(value) => value,
        Err(err) => return Ok(PersistedState::Corrupted(err.to_string())),
    };
    Ok(PersistedState::Valid(data))
}

fn save_trial_data(data: &TrialData) -> Result<()> {
    let path = get_trial_data_path()?;
    let json = serde_json::to_string(data)?;
    let encoded = base64_encode(&json);
    write_file_atomically(path, encoded.as_bytes())
}

fn base64_encode(input: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(input.as_bytes())
}

fn base64_decode(input: &str) -> Result<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(input.trim())
        .map_err(|e| anyhow::anyhow!("base64 解码失败: {}", e))?;
    String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("UTF-8 解码失败: {}", e))
}

pub fn check_trial_status() -> Result<TrialStatus> {
    let now = Utc::now();

    if let Some(status) = current_license_status(now)? {
        return Ok(status);
    }

    match load_trial_state()? {
        PersistedState::Valid(_) | PersistedState::Missing | PersistedState::Corrupted(_) => {
            Ok(blocked_status(
                now,
                ACTIVATION_REQUIRED_MESSAGE,
                ACTIVATION_REQUIRED_SUBTITLE,
            ))
        }
    }
}

#[tauri::command]
pub fn get_trial_status() -> Result<TrialStatus, String> {
    log::info!("[License] get_trial_status called");
    match check_trial_status() {
        Ok(status) => {
            log::info!(
                "[License] get_trial_status resolved: active={}, expired={}, remaining_days={}, first_launch_at={}, expires_at={}",
                status.is_active,
                status.is_expired,
                status.days_remaining,
                status.first_launch_at,
                status.expires_at
            );
            Ok(status)
        }
        Err(err) => {
            log::warn!("[License] get_trial_status failed: {}", err);
            Err(format!("检查试用期状态失败: {}", err))
        }
    }
}

#[tauri::command]
pub fn get_trial_days_remaining() -> Result<u64, String> {
    let status = check_trial_status().map_err(|e| format!("检查试用期状态失败: {}", e))?;
    Ok(status.days_remaining)
}

#[tauri::command]
pub fn activate_license(key: String) -> Result<(), String> {
    activate(&key).map_err(|e| format!("{}", e))
}

#[cfg(test)]
mod tests {
    use super::{
        activate, check_trial_status, is_licensed, load_license_state, save_license_data,
        LicenseData, PersistedState, ACTIVATION_REQUIRED_MESSAGE, ACTIVATION_REQUIRED_SUBTITLE,
    };
    use crate::license::code::{
        generate_license_code, generate_signing_keypair, LICENSE_TEST_ENV_LOCK,
    };
    use tempfile::tempdir;

    struct ConfigGuard {
        previous: Option<String>,
        _config_env: std::sync::MutexGuard<'static, ()>,
    }

    impl ConfigGuard {
        fn set(path: &std::path::Path) -> Self {
            let config_env = crate::config::paths::lock_config_env_for_test();
            let previous = std::env::var("ITERATE_CONFIG_DIR").ok();
            std::env::set_var("ITERATE_CONFIG_DIR", path);
            Self {
                previous,
                _config_env: config_env,
            }
        }
    }

    impl Drop for ConfigGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.as_ref() {
                std::env::set_var("ITERATE_CONFIG_DIR", previous);
            } else {
                std::env::remove_var("ITERATE_CONFIG_DIR");
            }
        }
    }

    struct PublicKeyGuard {
        previous: Option<String>,
    }

    impl PublicKeyGuard {
        fn set(value: &str) -> Self {
            let previous = std::env::var("ITERATE_LICENSE_PUBLIC_KEY_B64").ok();
            std::env::set_var("ITERATE_LICENSE_PUBLIC_KEY_B64", value);
            Self { previous }
        }
    }

    impl Drop for PublicKeyGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.as_ref() {
                std::env::set_var("ITERATE_LICENSE_PUBLIC_KEY_B64", previous);
            } else {
                std::env::remove_var("ITERATE_LICENSE_PUBLIC_KEY_B64");
            }
        }
    }

    #[test]
    fn check_trial_status_requires_activation_on_first_launch() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());

        let status = check_trial_status().unwrap();

        assert!(!status.is_active);
        assert!(status.is_expired);
        assert_eq!(status.days_remaining, 0);
        assert_eq!(status.trial_days, 0);
        assert!(!temp.path().join(".trial").exists());
    }

    #[test]
    fn check_trial_status_blocks_corrupted_trial_file() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        std::fs::write(temp.path().join(".trial"), "not-base64").unwrap();

        let status = check_trial_status().unwrap();

        assert!(status.is_expired);
        assert_eq!(status.expired_message, ACTIVATION_REQUIRED_MESSAGE);
        assert_eq!(status.expired_subtitle, ACTIVATION_REQUIRED_SUBTITLE);
    }

    #[test]
    fn check_trial_status_uses_real_license_duration() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let key = generate_license_code("day1", &private_key_b64).unwrap();
        let now = chrono::Utc::now().to_rfc3339();

        save_license_data(&LicenseData {
            license_key: key,
            activated_at: now.clone(),
            last_validated_at: now,
            device_id: "device-1".to_string(),
            license_type: "day1".to_string(),
        })
        .unwrap();

        let status = check_trial_status().unwrap();

        assert!(status.is_active);
        assert_eq!(status.trial_days, 1);
        assert_eq!(status.days_remaining, 1);
    }

    #[test]
    fn activate_day1_license_updates_status_in_temp_config() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let key = generate_license_code("day1", &private_key_b64).unwrap();

        let _ = check_trial_status().unwrap();
        activate(&key).unwrap();

        let status = check_trial_status().unwrap();

        assert!(is_licensed());
        assert!(status.is_active);
        assert_eq!(status.trial_days, 1);
        assert_eq!(status.days_remaining, 1);
        assert!(temp.path().join(".license").exists());
    }

    #[test]
    fn day1_license_expires_after_one_day() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let key = generate_license_code("day1", &private_key_b64).unwrap();
        let activated_at = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();

        save_license_data(&LicenseData {
            license_key: key,
            activated_at: activated_at.clone(),
            last_validated_at: activated_at,
            device_id: "device-1".to_string(),
            license_type: "day1".to_string(),
        })
        .unwrap();

        let status = check_trial_status().unwrap();

        assert!(status.is_expired);
        assert!(!status.is_active);
        assert_eq!(status.trial_days, 1);
        assert_eq!(status.days_remaining, 0);
        assert_eq!(status.days_used, 1);
    }

    #[test]
    fn day7_license_stays_active_before_seventh_day() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let key = generate_license_code("day7", &private_key_b64).unwrap();
        let activated_at = (chrono::Utc::now() - chrono::Duration::days(6)).to_rfc3339();

        save_license_data(&LicenseData {
            license_key: key,
            activated_at: activated_at.clone(),
            last_validated_at: activated_at,
            device_id: "device-1".to_string(),
            license_type: "day7".to_string(),
        })
        .unwrap();

        let status = check_trial_status().unwrap();

        assert!(status.is_active);
        assert!(!status.is_expired);
        assert_eq!(status.trial_days, 7);
        assert_eq!(status.days_remaining, 1);
        assert_eq!(status.days_used, 6);
    }

    #[test]
    fn day7_license_expires_on_seventh_day_boundary() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let key = generate_license_code("day7", &private_key_b64).unwrap();
        let activated_at = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();

        save_license_data(&LicenseData {
            license_key: key,
            activated_at: activated_at.clone(),
            last_validated_at: activated_at,
            device_id: "device-1".to_string(),
            license_type: "day7".to_string(),
        })
        .unwrap();

        let status = check_trial_status().unwrap();

        assert!(status.is_expired);
        assert!(!status.is_active);
        assert_eq!(status.trial_days, 7);
        assert_eq!(status.days_remaining, 0);
        assert_eq!(status.days_used, 7);
    }

    #[test]
    fn permanent_license_does_not_expire() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let key = generate_license_code("permanent", &private_key_b64).unwrap();
        let activated_at = (chrono::Utc::now() - chrono::Duration::days(90)).to_rfc3339();

        save_license_data(&LicenseData {
            license_key: key,
            activated_at: activated_at.clone(),
            last_validated_at: activated_at,
            device_id: "device-1".to_string(),
            license_type: "permanent".to_string(),
        })
        .unwrap();

        let status = check_trial_status().unwrap();

        assert!(status.is_active);
        assert!(!status.is_expired);
        assert_eq!(status.trial_days, 0);
        assert_eq!(status.days_remaining, 0);
        assert_eq!(status.days_used, 0);
        assert!(status.expires_at.is_empty());
    }

    #[test]
    fn reactivating_same_day1_license_does_not_extend_duration() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let key = generate_license_code("day1", &private_key_b64).unwrap();
        let activated_at = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();

        save_license_data(&LicenseData {
            license_key: key.clone(),
            activated_at: activated_at.clone(),
            last_validated_at: activated_at.clone(),
            device_id: "device-1".to_string(),
            license_type: "day1".to_string(),
        })
        .unwrap();

        activate(&key).unwrap();
        let status = check_trial_status().unwrap();

        assert!(status.is_expired);
        assert_eq!(status.days_remaining, 0);

        let stored = match load_license_state().unwrap() {
            PersistedState::Valid(license) => license,
            _ => panic!("expected valid license state"),
        };
        assert_eq!(stored.activated_at, activated_at);
    }

    #[test]
    fn higher_tier_license_can_upgrade_existing_license() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let day1_key = generate_license_code("day1", &private_key_b64).unwrap();
        let permanent_key = generate_license_code("permanent", &private_key_b64).unwrap();
        let activated_at = (chrono::Utc::now() - chrono::Duration::hours(12)).to_rfc3339();

        save_license_data(&LicenseData {
            license_key: day1_key,
            activated_at: activated_at.clone(),
            last_validated_at: activated_at,
            device_id: "device-1".to_string(),
            license_type: "day1".to_string(),
        })
        .unwrap();

        activate(&permanent_key).unwrap();
        let status = check_trial_status().unwrap();

        assert!(status.is_active);
        assert!(!status.is_expired);
        assert_eq!(status.trial_days, 0);

        let stored = match load_license_state().unwrap() {
            PersistedState::Valid(license) => license,
            _ => panic!("expected valid license state"),
        };
        assert_eq!(stored.license_key, permanent_key);
        assert_eq!(stored.license_type, "permanent");
    }

    #[test]
    fn lower_tier_license_cannot_downgrade_permanent_license() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let (private_key_b64, public_key_b64) = generate_signing_keypair().unwrap();
        let _public_key_guard = PublicKeyGuard::set(&public_key_b64);
        let permanent_key = generate_license_code("permanent", &private_key_b64).unwrap();
        let day7_key = generate_license_code("day7", &private_key_b64).unwrap();
        let activated_at = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();

        save_license_data(&LicenseData {
            license_key: permanent_key.clone(),
            activated_at: activated_at.clone(),
            last_validated_at: activated_at,
            device_id: "device-1".to_string(),
            license_type: "permanent".to_string(),
        })
        .unwrap();

        let err = activate(&day7_key).unwrap_err();
        assert!(err.to_string().contains("不能降级覆盖"));

        let stored = match load_license_state().unwrap() {
            PersistedState::Valid(license) => license,
            _ => panic!("expected valid license state"),
        };
        assert_eq!(stored.license_key, permanent_key);
        assert_eq!(stored.license_type, "permanent");
    }

    #[test]
    fn check_trial_status_blocks_invalid_signature_license() {
        let _lock = LICENSE_TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let _guard = ConfigGuard::set(temp.path());
        let now = chrono::Utc::now().to_rfc3339();

        save_license_data(&LicenseData {
            license_key: "ITL1.invalid.signature".to_string(),
            activated_at: now.clone(),
            last_validated_at: now,
            device_id: "device-1".to_string(),
            license_type: "day7".to_string(),
        })
        .unwrap();

        let status = check_trial_status().unwrap();

        assert!(status.is_expired);
        assert!(status.expired_message.contains("无效"));
    }
}
