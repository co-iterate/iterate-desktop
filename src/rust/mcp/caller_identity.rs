//! 识别调用 `call_zhi` 的客户端（P-2026-9373）。
//!
//! mcp-server 是 MCP 客户端直接拉起的 stdio 子进程。Claude Code 会为每个交互式
//! 会话写 `~/.claude/sessions/<pid>.json`（含 `sessionId`、`cwd`），所以用父进程号
//! 读这个文件就能判断调用方是不是 Claude Code。读不到时按原逻辑当作 Codex。
//!
//! Claude 会话 ID 走独立字段（`caller_client` / `caller_session_id` / `caller_cwd` /
//! `caller_pid`），不能塞进 `codex_thread_id`：后者还承担 ⌘+点击、Goal 绑定、APNs
//! 跳转等 Codex 专用语义。

use serde_json::Value;
use std::path::{Path, PathBuf};

pub const CALLER_CLIENT_CODEX: &str = "codex";
pub const CALLER_CLIENT_CLAUDE_CODE: &str = "claude_code";
/// Claude 会话时间线 route 的前缀。带冒号是故意的：`normalize_codex_thread_id`
/// 不接受冒号，route 永远不会被误当成 Codex 线程去生成 deeplink。
pub const CLAUDE_TIMELINE_ROUTE_PREFIX: &str = "claude-code:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeCaller {
    pub session_id: String,
    pub cwd: Option<String>,
    pub pid: u32,
}

impl ClaudeCodeCaller {
    pub fn timeline_route_id(&self) -> String {
        format!("{CLAUDE_TIMELINE_ROUTE_PREFIX}{}", self.session_id)
    }
}

/// Claude 会话 ID 只接受标准 UUID（8-4-4-4-12 十六进制），统一转小写。
/// 进 shell / AppleScript 前必须先过这一关。
pub fn normalize_claude_session_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.len() != 36 {
        return None;
    }
    for (index, ch) in trimmed.chars().enumerate() {
        let is_dash_slot = matches!(index, 8 | 13 | 18 | 23);
        if is_dash_slot {
            if ch != '-' {
                return None;
            }
        } else if !ch.is_ascii_hexdigit() {
            return None;
        }
    }
    Some(trimmed.to_ascii_lowercase())
}

pub fn claude_timeline_route_id(session_id: &str) -> Option<String> {
    normalize_claude_session_id(session_id)
        .map(|session_id| format!("{CLAUDE_TIMELINE_ROUTE_PREFIX}{session_id}"))
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 解析 `~/.claude/sessions/<pid>.json`。
///
/// - JSON 里的 `pid` 存在时必须等于期望的 pid；
/// - `procStart` 存在且能拿到实际进程启动时间时两者必须一致，防止 pid 被复用后
///   读到早已退出的 Claude 会话留下的旧文件（例如 Codex 恰好复用了这个 pid）。
pub fn parse_claude_session_file(
    contents: &str,
    expected_pid: u32,
    actual_proc_start: Option<&str>,
) -> Option<ClaudeCodeCaller> {
    let value: Value = serde_json::from_str(contents).ok()?;
    let object = value.as_object()?;
    if let Some(pid) = object.get("pid") {
        if pid.as_u64() != Some(u64::from(expected_pid)) {
            return None;
        }
    }
    if let (Some(recorded), Some(actual)) = (
        object.get("procStart").and_then(Value::as_str),
        actual_proc_start,
    ) {
        if normalize_whitespace(recorded) != normalize_whitespace(actual) {
            return None;
        }
    }
    let session_id = object
        .get("sessionId")
        .and_then(Value::as_str)
        .and_then(normalize_claude_session_id)?;
    let cwd = object
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty() && Path::new(cwd).is_absolute())
        .map(ToOwned::to_owned);
    Some(ClaudeCodeCaller {
        session_id,
        cwd,
        pid: expected_pid,
    })
}

pub fn read_claude_session_for_pid(
    sessions_dir: &Path,
    pid: u32,
    actual_proc_start: Option<&str>,
) -> Option<ClaudeCodeCaller> {
    if pid <= 1 {
        return None;
    }
    let contents = std::fs::read_to_string(sessions_dir.join(format!("{pid}.json"))).ok()?;
    parse_claude_session_file(&contents, pid, actual_proc_start)
}

/// Claude Code 配置目录：优先 `CLAUDE_CONFIG_DIR`，否则 `~/.claude`。
pub fn claude_sessions_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir).join("sessions"));
    }
    #[cfg(test)]
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home).join(".claude").join("sessions"));
    }
    dirs::home_dir().map(|home| home.join(".claude").join("sessions"))
}

/// 进程启动时间，格式与 Claude 写入的 `procStart` 一致（C locale + UTC 的 `lstart`）。
#[cfg(unix)]
pub fn process_start_time_utc(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = normalize_whitespace(&String::from_utf8_lossy(&output.stdout));
    (!text.is_empty()).then_some(text)
}

#[cfg(not(unix))]
pub fn process_start_time_utc(_pid: u32) -> Option<String> {
    None
}

#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    if pid <= 1 || pid > i32::MAX as u32 {
        return false;
    }
    // kill(pid, 0) 只探测，不发信号。EPERM 也说明进程存在。
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub fn is_process_alive(_pid: u32) -> bool {
    false
}

/// mcp-server 用：父进程是不是 Claude Code。非 unix 平台恒为 None。
#[cfg(unix)]
pub fn detect_claude_code_parent() -> Option<ClaudeCodeCaller> {
    let ppid = std::os::unix::process::parent_id();
    let sessions_dir = claude_sessions_dir()?;
    let proc_start = process_start_time_utc(ppid);
    read_claude_session_for_pid(&sessions_dir, ppid, proc_start.as_deref())
}

#[cfg(not(unix))]
pub fn detect_claude_code_parent() -> Option<ClaudeCodeCaller> {
    None
}

/// 某个 Claude 会话当前是否还有活着的进程。优先看调用时记录的 pid，
/// 不对（会话被 /resume 到别的进程、pid 已退出）再扫一遍 sessions 目录。
pub fn find_live_claude_session(
    sessions_dir: &Path,
    session_id: &str,
    hinted_pid: Option<u32>,
) -> Option<ClaudeCodeCaller> {
    let session_id = normalize_claude_session_id(session_id)?;
    let check = |pid: u32| -> Option<ClaudeCodeCaller> {
        if !is_process_alive(pid) {
            return None;
        }
        let proc_start = process_start_time_utc(pid);
        read_claude_session_for_pid(sessions_dir, pid, proc_start.as_deref())
            .filter(|caller| caller.session_id == session_id)
    };
    if let Some(found) = hinted_pid.and_then(|pid| check(pid)) {
        return Some(found);
    }
    let entries = std::fs::read_dir(sessions_dir).ok()?;
    entries.flatten().find_map(|entry| {
        let name = entry.file_name();
        let pid = name.to_str()?.strip_suffix(".json")?.parse::<u32>().ok()?;
        if Some(pid) == hinted_pid {
            return None;
        }
        check(pid)
    })
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// 弹窗请求 JSON 里是否声明了 Claude Code 调用方。
pub fn is_claude_code_caller_request(req: &Value) -> bool {
    req.as_object()
        .and_then(|object| string_field(object, &["caller_client", "callerClient"]))
        == Some(CALLER_CLIENT_CLAUDE_CODE)
}

/// Claude 调用方的稳定时间线 route；不是 Claude 调用方或会话 ID 不合法时为 None。
pub fn claude_timeline_route_from_request(req: &Value) -> Option<String> {
    if !is_claude_code_caller_request(req) {
        return None;
    }
    let object = req.as_object()?;
    string_field(object, &["caller_session_id", "callerSessionId"])
        .and_then(claude_timeline_route_id)
}

/// 单引号包裹的 POSIX shell 字面量。
pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Claude 会话已退出时，新 Terminal 里执行的命令。cwd 必须是绝对路径且不含控制字符。
pub fn claude_resume_shell_command(cwd: &str, session_id: &str) -> Result<String, String> {
    let session_id =
        normalize_claude_session_id(session_id).ok_or_else(|| "Claude 会话 ID 无效".to_string())?;
    let cwd = cwd.trim();
    if cwd.is_empty() || !Path::new(cwd).is_absolute() {
        return Err("Claude 会话目录无效（需要绝对路径）".to_string());
    }
    if cwd.chars().any(char::is_control) {
        return Err("Claude 会话目录包含控制字符".to_string());
    }
    Ok(format!(
        "cd {} && claude --resume {}",
        shell_single_quote(cwd),
        session_id
    ))
}

/// `ps -o tty=` 的结果，只接受 `ttys009` 这类纯字母数字设备名。
pub fn normalize_tty_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let name = trimmed.strip_prefix("/dev/").unwrap_or(trimmed);
    if name.is_empty()
        || name.len() > 32
        || name == "??"
        || !name.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(format!("/dev/{name}"))
}

#[cfg(unix)]
pub fn process_tty(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .env("LC_ALL", "C")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_tty_name(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(unix))]
pub fn process_tty(_pid: u32) -> Option<String> {
    None
}

/// 选中 Terminal.app 中 tty 匹配的标签页。tty 通过 argv 传入，不拼进脚本文本。
pub const TERMINAL_FOCUS_TTY_SCRIPT: &str = r#"on run argv
    set targetTty to item 1 of argv
    if application "Terminal" is not running then return "terminal_not_running"
    tell application "Terminal"
        repeat with w in windows
            repeat with t in tabs of w
                if (tty of t) is targetTty then
                    set selected tab of w to t
                    set index of w to 1
                    activate
                    return "focused"
                end if
            end repeat
        end repeat
    end tell
    return "tab_not_found"
end run"#;

/// 新开 Terminal 窗口执行命令。命令通过 argv 传入，不拼进脚本文本。
pub const TERMINAL_RUN_COMMAND_SCRIPT: &str = r#"on run argv
    set commandText to item 1 of argv
    tell application "Terminal"
        activate
        do script commandText
    end tell
    return "resumed"
end run"#;

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "7dbca76e-c67a-4cee-ad28-52ad4d90bd28";

    #[test]
    fn session_id_accepts_only_uuid_shape() {
        assert_eq!(
            normalize_claude_session_id(SESSION).as_deref(),
            Some(SESSION)
        );
        assert_eq!(
            normalize_claude_session_id(" 7DBCA76E-C67A-4CEE-AD28-52AD4D90BD28 ").as_deref(),
            Some(SESSION)
        );
        for bad in [
            "",
            "serve-123",
            "7dbca76e-c67a-4cee-ad28-52ad4d90bd2",
            "7dbca76e-c67a-4cee-ad28-52ad4d90bd28; rm -rf ~",
            "7dbca76e_c67a_4cee_ad28_52ad4d90bd28",
            "7dbca76e-c67a-4cee-ad28-52ad4d90bdzz",
            "'dbca76e-c67a-4cee-ad28-52ad4d90bd28",
        ] {
            assert_eq!(normalize_claude_session_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn claude_route_is_never_a_codex_thread_id() {
        let route = claude_timeline_route_id(SESSION).unwrap();
        assert_eq!(route, format!("claude-code:{SESSION}"));
        assert_eq!(
            crate::mcp::codex_deeplink::normalize_codex_thread_id(&route),
            None
        );
    }

    #[test]
    fn parses_claude_session_file_for_matching_pid() {
        let contents = format!(
            r#"{{"pid":19268,"sessionId":"{SESSION}","cwd":"/Users/example/project","procStart":"Wed Oct 21 01:57:41 2026","kind":"interactive"}}"#
        );
        let caller =
            parse_claude_session_file(&contents, 19268, Some("Wed Oct 21  01:57:41 2026")).unwrap();
        assert_eq!(caller.session_id, SESSION);
        assert_eq!(caller.cwd.as_deref(), Some("/Users/example/project"));
        assert_eq!(caller.pid, 19268);
        assert_eq!(caller.timeline_route_id(), format!("claude-code:{SESSION}"));
    }

    #[test]
    fn rejects_session_file_from_reused_pid_or_missing_session() {
        let contents = format!(
            r#"{{"pid":19268,"sessionId":"{SESSION}","cwd":"/Users/example/project","procStart":"Wed Oct 21 01:57:41 2026"}}"#
        );
        // pid 不一致
        assert_eq!(parse_claude_session_file(&contents, 19269, None), None);
        // pid 被复用：实际启动时间不同
        assert_eq!(
            parse_claude_session_file(&contents, 19268, Some("Thu Oct 22 08:00:00 2026")),
            None
        );
        // 没有 sessionId（老格式只有 peerToken）
        assert_eq!(
            parse_claude_session_file(r#"{"peerToken":"abc","pidDomain":"darwin"}"#, 19268, None),
            None
        );
        // sessionId 非 UUID
        assert_eq!(
            parse_claude_session_file(r#"{"pid":1234,"sessionId":"x; rm"}"#, 1234, None),
            None
        );
        // 不是 JSON
        assert_eq!(parse_claude_session_file("not json", 1234, None), None);
    }

    #[test]
    fn relative_cwd_is_dropped() {
        let contents = format!(r#"{{"sessionId":"{SESSION}","cwd":"relative/dir"}}"#);
        let caller = parse_claude_session_file(&contents, 4242, None).unwrap();
        assert_eq!(caller.cwd, None);
    }

    #[test]
    fn reads_session_file_from_sessions_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("4242.json"),
            format!(r#"{{"pid":4242,"sessionId":"{SESSION}","cwd":"/tmp/x"}}"#),
        )
        .unwrap();
        assert_eq!(
            read_claude_session_for_pid(dir.path(), 4242, None).map(|caller| caller.session_id),
            Some(SESSION.to_string())
        );
        assert_eq!(read_claude_session_for_pid(dir.path(), 4243, None), None);
        assert_eq!(read_claude_session_for_pid(dir.path(), 1, None), None);
    }

    #[test]
    fn request_route_requires_claude_caller_and_valid_session() {
        let claude = serde_json::json!({
            "caller_client": "claude_code",
            "caller_session_id": SESSION,
        });
        assert!(is_claude_code_caller_request(&claude));
        assert_eq!(
            claude_timeline_route_from_request(&claude),
            Some(format!("claude-code:{SESSION}"))
        );

        let codex = serde_json::json!({ "caller_client": "codex", "caller_session_id": SESSION });
        assert!(!is_claude_code_caller_request(&codex));
        assert_eq!(claude_timeline_route_from_request(&codex), None);

        let invalid =
            serde_json::json!({ "caller_client": "claude_code", "caller_session_id": "nope" });
        assert!(is_claude_code_caller_request(&invalid));
        assert_eq!(claude_timeline_route_from_request(&invalid), None);
    }

    #[test]
    fn resume_command_quotes_cwd_and_validates_session() {
        assert_eq!(
            claude_resume_shell_command("/Users/example/it's here", SESSION).unwrap(),
            format!("cd '/Users/example/it'\\''s here' && claude --resume {SESSION}")
        );
        assert!(claude_resume_shell_command("/tmp", "abc; rm -rf ~").is_err());
        assert!(claude_resume_shell_command("relative", SESSION).is_err());
        assert!(claude_resume_shell_command("/tmp/a\nb", SESSION).is_err());
    }

    #[test]
    fn tty_name_is_restricted() {
        assert_eq!(
            normalize_tty_name("ttys009 \n").as_deref(),
            Some("/dev/ttys009")
        );
        assert_eq!(
            normalize_tty_name("/dev/ttys010").as_deref(),
            Some("/dev/ttys010")
        );
        assert_eq!(normalize_tty_name("??"), None);
        assert_eq!(normalize_tty_name(""), None);
        assert_eq!(normalize_tty_name("ttys0\"; do shell script"), None);
    }

    #[test]
    fn applescripts_take_values_from_argv_only() {
        assert!(TERMINAL_FOCUS_TTY_SCRIPT.contains("item 1 of argv"));
        assert!(TERMINAL_RUN_COMMAND_SCRIPT.contains("item 1 of argv"));
    }
}
