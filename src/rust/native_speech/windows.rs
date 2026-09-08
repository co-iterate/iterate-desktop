use serde::Serialize;
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_CONTROL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, IsWindow, SetForegroundWindow,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const WINDOWS_SPEECH_SHORTCUT: &str = "Shift+Ctrl+Space";
const WINDOWS_SPEECH_TRANSCRIPT_EVENT: &str = "speech://windows-transcript";
const WINDOWS_SPEECH_ERROR_EVENT: &str = "speech://windows-error";
const WINDOWS_SPEECH_STATE_EVENT: &str = "speech://windows-state";

static WINDOWS_SPEECH_ACTIVE: AtomicBool = AtomicBool::new(false);
static WINDOWS_SPEECH_TARGETS: OnceLock<Mutex<HashMap<String, isize>>> = OnceLock::new();

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsSpeechTranscriptPayload {
    pub target_token: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsSpeechCapability {
    pub available: bool,
    pub recognizer_name: Option<String>,
    pub culture: Option<String>,
    pub shortcut: &'static str,
    pub details: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowsSpeechStatePayload {
    active: bool,
    phase: &'static str,
    message: &'static str,
}

fn emit_state(app: &AppHandle, active: bool, phase: &'static str, message: &'static str) {
    let _ = app.emit(
        WINDOWS_SPEECH_STATE_EVENT,
        WindowsSpeechStatePayload {
            active,
            phase,
            message,
        },
    );
}

fn targets() -> &'static Mutex<HashMap<String, isize>> {
    WINDOWS_SPEECH_TARGETS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn powershell_encoded_command(script: &str) -> String {
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    base64_013::encode(&bytes)
}

fn run_powershell(script: &str) -> Result<Output, String> {
    let encoded = powershell_encoded_command(script);
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-EncodedCommand",
            encoded.as_str(),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("启动 Windows PowerShell 失败: {error}"))
}

fn speech_capability() -> WindowsSpeechCapability {
    let script = r#"
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Speech
$recognizers=[System.Speech.Recognition.SpeechRecognitionEngine]::InstalledRecognizers()
$preferred=$recognizers | Where-Object { $_.Culture.Name -eq [System.Globalization.CultureInfo]::CurrentUICulture.Name } | Select-Object -First 1
if(-not $preferred){ $preferred=$recognizers | Where-Object { $_.Culture.Name -eq 'zh-CN' } | Select-Object -First 1 }
if(-not $preferred){ $preferred=$recognizers | Select-Object -First 1 }
if($preferred){
  [Console]::OutputEncoding=New-Object System.Text.UTF8Encoding($false)
  [Console]::Write($preferred.Name + "`t" + $preferred.Culture.Name)
}
"#;

    match run_powershell(script) {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let mut parts = text.splitn(2, '\t');
            let name = parts
                .next()
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let culture = parts
                .next()
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let available = name.is_some();
            WindowsSpeechCapability {
                available,
                recognizer_name: name,
                culture,
                shortcut: WINDOWS_SPEECH_SHORTCUT,
                details: if available {
                    "Windows 本地 System.Speech 识别器可用；识别文本会继续经过 iterate 的纠错与肌肉记忆后处理。".to_string()
                } else {
                    "Windows 未安装可用的 System.Speech 识别器。".to_string()
                },
            }
        }
        Ok(output) => WindowsSpeechCapability {
            available: false,
            recognizer_name: None,
            culture: None,
            shortcut: WINDOWS_SPEECH_SHORTCUT,
            details: format!(
                "Windows 语音识别器探测失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(error) => WindowsSpeechCapability {
            available: false,
            recognizer_name: None,
            culture: None,
            shortcut: WINDOWS_SPEECH_SHORTCUT,
            details: error,
        },
    }
}

fn recognize_once() -> Result<Option<String>, String> {
    let script = r#"
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Speech
$recognizers=[System.Speech.Recognition.SpeechRecognitionEngine]::InstalledRecognizers()
$preferred=$recognizers | Where-Object { $_.Culture.Name -eq [System.Globalization.CultureInfo]::CurrentUICulture.Name } | Select-Object -First 1
if(-not $preferred){ $preferred=$recognizers | Where-Object { $_.Culture.Name -eq 'zh-CN' } | Select-Object -First 1 }
if(-not $preferred){ $preferred=$recognizers | Select-Object -First 1 }
if(-not $preferred){ throw 'no Windows System.Speech recognizer is installed' }
$engine=New-Object System.Speech.Recognition.SpeechRecognitionEngine($preferred)
try {
  $engine.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar))
  $engine.InitialSilenceTimeout=[TimeSpan]::FromSeconds(10)
  $engine.BabbleTimeout=[TimeSpan]::FromSeconds(5)
  $engine.EndSilenceTimeout=[TimeSpan]::FromMilliseconds(800)
  $engine.EndSilenceTimeoutAmbiguous=[TimeSpan]::FromMilliseconds(1200)
  $engine.SetInputToDefaultAudioDevice()
  $result=$engine.Recognize([TimeSpan]::FromSeconds(60))
  if($result){
    [Console]::OutputEncoding=New-Object System.Text.UTF8Encoding($false)
    [Console]::Write($result.Text)
  }
} finally {
  $engine.Dispose()
}
"#;

    let output = run_powershell(script)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "Windows 语音识别失败".to_string()
        } else {
            format!("Windows 语音识别失败: {stderr}")
        });
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!text.is_empty()).then_some(text))
}

fn read_clipboard_text() -> Option<String> {
    let script = r#"
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=New-Object System.Text.UTF8Encoding($false)
$value=Get-Clipboard -Raw -Format Text
if($null -ne $value){ [Console]::Write([string]$value) }
"#;
    let output = run_powershell(script).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

fn write_clipboard_text(text: &str) -> Result<(), String> {
    let escaped = text.replace('\'', "''");
    let script = format!("$ErrorActionPreference='Stop'; Set-Clipboard -Value '{escaped}'");
    let output = run_powershell(&script)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "写入 Windows 剪贴板失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn paste_into_target(target: isize, text: &str) -> Result<(), String> {
    let hwnd = target as *mut core::ffi::c_void;
    if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return Err("原语音写回窗口已经不存在".to_string());
    }

    let previous = read_clipboard_text();
    write_clipboard_text(text)?;

    if unsafe { SetForegroundWindow(hwnd) } == 0 {
        if let Some(previous) = previous.as_deref() {
            let _ = write_clipboard_text(previous);
        }
        return Err("无法重新激活原语音写回窗口，已取消粘贴".to_string());
    }

    std::thread::sleep(Duration::from_millis(90));
    unsafe {
        keybd_event(VK_CONTROL as u8, 0, 0, 0);
        keybd_event(b'V', 0, 0, 0);
        keybd_event(b'V', 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_CONTROL as u8, 0, KEYEVENTF_KEYUP, 0);
    }
    std::thread::sleep(Duration::from_millis(100));

    if let Some(previous) = previous.as_deref() {
        let _ = write_clipboard_text(previous);
    }
    Ok(())
}

pub fn start_windows_dictation(app: AppHandle) -> Result<bool, String> {
    if WINDOWS_SPEECH_ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        WINDOWS_SPEECH_ACTIVE.store(false, Ordering::SeqCst);
        return Err("没有可用的前台窗口，无法确定语音写回目标".to_string());
    }
    let target = hwnd as isize;
    if let Err(error) = super::reveal_overlay(&app) {
        WINDOWS_SPEECH_ACTIVE.store(false, Ordering::SeqCst);
        return Err(format!("显示 Windows 语音浮层失败: {error}"));
    }
    emit_state(&app, true, "listening", "正在聆听");
    let failure_app = app.clone();

    std::thread::Builder::new()
        .name("iterate-windows-speech".to_string())
        .spawn(move || {
            let result = recognize_once();
            match result {
                Ok(Some(text)) => {
                    emit_state(&app, true, "processing", "正在处理");
                    let target_token = Uuid::new_v4().to_string();
                    let stored = targets()
                        .lock()
                        .map(|mut targets| {
                            targets.insert(target_token.clone(), target);
                        })
                        .is_ok();
                    if stored {
                        let payload = WindowsSpeechTranscriptPayload {
                            target_token: target_token.clone(),
                            text,
                        };
                        if app.emit(WINDOWS_SPEECH_TRANSCRIPT_EVENT, payload).is_err() {
                            if let Ok(mut targets) = targets().lock() {
                                targets.remove(&target_token);
                            }
                            emit_state(&app, false, "error", "写回通道不可用");
                            let _ = super::hide_overlay(&app);
                        }
                    } else {
                        emit_state(&app, false, "error", "无法保存写回目标");
                        let _ = super::hide_overlay(&app);
                    }
                }
                Ok(None) => {
                    emit_state(&app, false, "idle", "未识别到内容");
                    let _ = super::hide_overlay(&app);
                }
                Err(error) => {
                    let _ = app.emit(WINDOWS_SPEECH_ERROR_EVENT, error);
                    emit_state(&app, false, "error", "语音识别失败");
                    let _ = super::hide_overlay(&app);
                }
            }
            WINDOWS_SPEECH_ACTIVE.store(false, Ordering::SeqCst);
        })
        .map_err(|error| {
            WINDOWS_SPEECH_ACTIVE.store(false, Ordering::SeqCst);
            emit_state(&failure_app, false, "error", "语音线程启动失败");
            let _ = super::hide_overlay(&failure_app);
            format!("启动 Windows 语音线程失败: {error}")
        })?;

    Ok(true)
}

#[tauri::command]
pub fn get_windows_speech_capability() -> WindowsSpeechCapability {
    speech_capability()
}

#[tauri::command]
pub fn start_windows_speech_dictation(app: AppHandle) -> Result<bool, String> {
    start_windows_dictation(app)
}

#[tauri::command]
pub fn commit_windows_speech_text(
    app: AppHandle,
    target_token: String,
    text: String,
) -> Result<(), String> {
    let target_token = target_token.trim();
    let text = text.trim();
    if target_token.is_empty() || text.is_empty() {
        return Err("Windows 语音写回参数为空".to_string());
    }

    let target = targets()
        .lock()
        .map_err(|_| "Windows 语音目标表锁失败".to_string())?
        .remove(target_token)
        .ok_or_else(|| "Windows 语音写回目标已失效".to_string())?;

    match paste_into_target(target, text) {
        Ok(()) => {
            emit_state(&app, false, "success", "已写回");
            let _ = super::hide_overlay(&app);
            Ok(())
        }
        Err(error) => {
            emit_state(&app, false, "error", "写回失败");
            let _ = super::hide_overlay(&app);
            Err(error)
        }
    }
}
