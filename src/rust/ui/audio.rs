use anyhow::Result;
use rodio::{Decoder, OutputStream, Sink};
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

use super::audio_assets::{get_audio_asset_manager, AudioSource};
use crate::config::{save_config, AppState};
use crate::log_important;

// 音频播放控制器 - 只存储控制信号，不存储音频流
pub struct AudioController {
    pub should_stop: Arc<AtomicBool>,
}

#[tauri::command]
pub async fn get_audio_notification_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.audio_config.notification_enabled)
}

#[tauri::command]
pub async fn set_audio_notification_enabled(
    enabled: bool,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // 如果是首次启用音频通知，先复制音频文件
    if enabled {
        if let Err(e) = ensure_audio_file_exists(&app).await {
            return Err(format!("准备音频文件失败: {}", e));
        }
    }

    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.audio_config.notification_enabled = enabled;
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn get_audio_url(state: State<'_, AppState>) -> Result<String, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.audio_config.custom_url.clone())
}

#[tauri::command]
pub async fn set_audio_url(
    url: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.audio_config.custom_url = url;
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn play_notification_sound(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if super::global_mute::is_globally_muted()? {
        return Ok(());
    }
    // 检查是否启用音频通知
    let (enabled, audio_url) = {
        let config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        (
            config.audio_config.notification_enabled,
            config.audio_config.custom_url.clone(),
        )
    };

    if !enabled {
        return Ok(());
    }

    // 异步播放音频，避免阻塞主线程
    tokio::spawn(async move {
        if super::global_mute::is_globally_muted().unwrap_or(true) {
            return;
        }
        if let Err(e) = play_audio_file(&app, &audio_url).await {
            log_important!(warn, "播放音频失败: {}", e);
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn test_audio_sound(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // 获取当前配置的音效URL
    let audio_url = {
        let config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.audio_config.custom_url.clone()
    };

    // 同步测试音频播放，确保能捕获错误
    match play_audio_file(&app, &audio_url).await {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("音效测试失败: {}", e)),
    }
}

#[tauri::command]
pub async fn stop_audio_sound(app: tauri::AppHandle) -> Result<(), String> {
    // 设置停止信号
    if let Some(audio_controller) = app.try_state::<AudioController>() {
        audio_controller.should_stop.store(true, Ordering::Relaxed);
    }
    Ok(())
}

pub async fn play_audio_file(app: &AppHandle, audio_url: &str) -> Result<()> {
    // 重置停止信号
    if let Some(audio_controller) = app.try_state::<AudioController>() {
        audio_controller.should_stop.store(false, Ordering::Relaxed);
    }

    let audio_source = {
        let manager = get_audio_asset_manager();
        let manager = manager
            .lock()
            .map_err(|e| anyhow::anyhow!("获取管理器锁失败: {}", e))?;
        manager.parse_audio_url(app, audio_url)?
    };

    match audio_source {
        AudioSource::Url(url) => {
            // 网络音频文件
            play_audio_from_url(app, &url).await
        }
        AudioSource::File(path) => {
            // 本地文件路径
            if path.exists() {
                let app_handle = app.clone();
                tokio::task::spawn_blocking(move || {
                    play_audio_sync_with_controller(&path, &app_handle)
                })
                .await
                .map_err(|e| anyhow::anyhow!("音频播放任务失败: {}", e))?
            } else {
                Err(anyhow::anyhow!("音频文件不存在: {:?}", path))
            }
        }
        AudioSource::Asset(asset_id) => {
            // 内置音频资源
            let audio_path = {
                let manager = get_audio_asset_manager();
                let manager = manager
                    .lock()
                    .map_err(|e| anyhow::anyhow!("获取管理器锁失败: {}", e))?;
                manager.ensure_audio_exists(app, &asset_id)?
            };
            let app_handle = app.clone();
            tokio::task::spawn_blocking(move || {
                play_audio_sync_with_controller(&audio_path, &app_handle)
            })
            .await
            .map_err(|e| anyhow::anyhow!("音频播放任务失败: {}", e))?
        }
    }
}

async fn play_audio_from_url(app: &AppHandle, url: &str) -> Result<()> {
    // 下载音频文件到临时目录
    let response = reqwest::get(url)
        .await
        .map_err(|e| anyhow::anyhow!("下载音频文件失败: {}", e))?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "下载音频文件失败，状态码: {}",
            response.status()
        ));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("读取音频数据失败: {}", e))?;

    // 将bytes转换为Vec<u8>以便移动到线程中
    let bytes_vec = bytes.to_vec();
    let app_handle = app.clone();

    // 使用 tokio::task::spawn_blocking 来等待音频播放完成
    tokio::task::spawn_blocking(move || {
        play_audio_from_bytes_with_controller(bytes_vec, &app_handle)
    })
    .await
    .map_err(|e| anyhow::anyhow!("音频播放任务失败: {}", e))?
}

fn play_audio_from_bytes_with_controller(bytes: Vec<u8>, app: &AppHandle) -> Result<()> {
    // 创建音频输出流
    let (_stream, stream_handle) =
        OutputStream::try_default().map_err(|e| anyhow::anyhow!("创建音频输出流失败: {}", e))?;

    // 创建音频播放器
    let sink =
        Sink::try_new(&stream_handle).map_err(|e| anyhow::anyhow!("创建音频播放器失败: {}", e))?;

    // 从字节数据解码音频
    let cursor = std::io::Cursor::new(bytes);
    let source = Decoder::new(cursor).map_err(|e| anyhow::anyhow!("解码音频数据失败: {}", e))?;

    // 播放音频
    sink.append(source);

    // 检查停止信号并播放
    if let Some(audio_controller) = app.try_state::<AudioController>() {
        while !sink.empty() {
            if audio_controller.should_stop.load(Ordering::Relaxed) {
                sink.stop();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    } else {
        sink.sleep_until_end();
    }

    Ok(())
}

fn play_audio_sync_with_controller(audio_path: &PathBuf, app: &AppHandle) -> Result<()> {
    // 创建音频输出流
    let (_stream, stream_handle) =
        OutputStream::try_default().map_err(|e| anyhow::anyhow!("创建音频输出流失败: {}", e))?;

    // 创建音频播放器
    let sink =
        Sink::try_new(&stream_handle).map_err(|e| anyhow::anyhow!("创建音频播放器失败: {}", e))?;

    // 读取音频文件
    let file =
        std::fs::File::open(audio_path).map_err(|e| anyhow::anyhow!("打开音频文件失败: {}", e))?;
    let buf_reader = BufReader::new(file);

    // 解码音频
    let source =
        Decoder::new(buf_reader).map_err(|e| anyhow::anyhow!("解码音频文件失败: {}", e))?;

    // 播放音频
    sink.append(source);

    // 检查停止信号并播放
    if let Some(audio_controller) = app.try_state::<AudioController>() {
        while !sink.empty() {
            if audio_controller.should_stop.load(Ordering::Relaxed) {
                sink.stop();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    } else {
        // 如果没有控制器，使用原来的方式
        sink.sleep_until_end();
    }

    Ok(())
}

/// 确保默认音频文件存在，如果不存在则从资源目录复制
pub async fn ensure_audio_file_exists(app: &AppHandle) -> Result<()> {
    let manager = get_audio_asset_manager();
    let manager = manager
        .lock()
        .map_err(|e| anyhow::anyhow!("获取管理器锁失败: {}", e))?;

    // 确保第一个可用的音频资源存在
    let all_assets = manager.get_all_assets();
    if let Some(first_asset) = all_assets.first() {
        manager.ensure_audio_exists(app, &first_asset.id)?;
    } else {
        return Err(anyhow::anyhow!("没有可用的音频资源"));
    }

    Ok(())
}
