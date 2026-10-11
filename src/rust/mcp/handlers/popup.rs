use anyhow::Result;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use crate::log_important;
use crate::mcp::types::PopupRequest;

const IPC_ADDR: &str = "127.0.0.1:37101";
const IPC_CONNECT_TIMEOUT_MS: u64 = 180;
const IPC_WRITE_TIMEOUT_MS: u64 = 30_000;

/// 创建 Tauri 弹窗
///
/// 优先调用与 MCP 服务器同目录的 UI 命令，找不到时使用全局版本
pub fn create_tauri_popup(request: &PopupRequest) -> Result<String> {
    // 优先走常驻桌面应用 IPC，避免每次弹窗都冷启动 iterate 进程。
    if let Ok(response) = forward_request_to_running_app_sync(request) {
        return Ok(response);
    }

    // 创建临时请求文件 - 跨平台适配
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("mcp_request_{}.json", request.id));
    let request_json = serde_json::to_string_pretty(request)?;
    fs::write(&temp_file, request_json)?;

    // 尝试找到 iterate 命令的路径
    let command_path = find_ui_command()?;

    // 记录弹窗开始时间
    let start = Instant::now();
    let pid = std::process::id();
    log_important!(
        info,
        "[POPUP] 开始等待弹窗 PID={} request_id={}",
        pid,
        request.id
    );

    // 调用 iterate 命令
    let output = Command::new(&command_path)
        .arg("--mcp-request")
        .arg(temp_file.to_string_lossy().to_string())
        .output()?;

    // 记录弹窗结束时间
    let elapsed = start.elapsed();
    log_important!(
        info,
        "[POPUP] 弹窗完成 PID={} request_id={} 耗时={:.2}s",
        pid,
        request.id,
        elapsed.as_secs_f64()
    );

    // 清理临时文件
    let _ = fs::remove_file(&temp_file);

    if output.status.success() {
        let response = String::from_utf8_lossy(&output.stdout);
        let response = response.trim();
        if response.is_empty() {
            Ok("用户取消了操作".to_string())
        } else {
            Ok(response.to_string())
        }
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("UI进程失败: {}", error);
    }
}

fn forward_request_to_running_app_sync(request: &PopupRequest) -> Result<String> {
    let addr: SocketAddr = IPC_ADDR.parse()?;
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_millis(IPC_CONNECT_TIMEOUT_MS))?;
    // A visible popup is allowed to wait for the user indefinitely. A read
    // timeout would mistake a slow reply for IPC failure and open a duplicate
    // fallback popup while the original is still active.
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(Some(Duration::from_millis(IPC_WRITE_TIMEOUT_MS)))?;

    let request_bytes = serde_json::to_vec(request)?;
    write_frame_sync(&mut stream, &request_bytes)?;

    let response_bytes = read_frame_sync(&mut stream)?;
    let response = String::from_utf8(response_bytes)?;

    log_important!(
        info,
        "[POPUP] 通过常驻 IPC 处理请求 request_id={}",
        request.id
    );

    Ok(response)
}

fn read_frame_sync(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;

    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        anyhow::bail!("空响应帧");
    }

    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_frame_sync(stream: &mut TcpStream, bytes: &[u8]) -> Result<()> {
    let len: u32 = bytes.len().try_into()?;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()?;
    Ok(())
}

/// 查找 iterate UI 命令的路径
///
/// 按优先级查找：同目录 -> 全局版本 -> 开发环境
fn find_ui_command() -> Result<String> {
    // 1. 优先尝试与当前 MCP 服务器同目录的 iterate 命令
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            let local_ui_path = exe_dir.join("iterate");
            if local_ui_path.exists() && is_executable(&local_ui_path) {
                return Ok(local_ui_path.to_string_lossy().to_string());
            }
        }
    }

    // 2. 尝试全局命令（最常见的部署方式）
    if test_command_available("iterate") {
        return Ok("iterate".to_string());
    }

    // 3. 如果都找不到，返回详细错误信息
    anyhow::bail!(
        "找不到 iterate UI 命令。请确保：\n\
         1. 已编译项目：cargo build --release\n\
         2. 或已全局安装：./install.sh\n\
         3. 或 iterate 命令在同目录下"
    )
}

/// 测试命令是否可用
fn test_command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 检查文件是否可执行
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        // Windows 上检查文件扩展名
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("exe"))
            .unwrap_or(false)
    }
}
