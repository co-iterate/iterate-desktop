use env_logger::{Builder, Target};
use log::LevelFilter;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Once;

static INIT: Once = Once::new();

/// 日志配置
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// 日志级别
    pub level: LevelFilter,
    /// 日志文件路径（None 表示不输出到文件）
    pub file_path: Option<String>,
    /// 是否为 MCP 模式（MCP 模式下不输出到 stderr）
    pub is_mcp_mode: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: LevelFilter::Warn,
            file_path: None,
            is_mcp_mode: false,
        }
    }
}

fn ensure_log_parent_dir(file_path: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(file_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn open_log_file(file_path: &str) -> std::io::Result<std::fs::File> {
    ensure_log_parent_dir(file_path)?;
    OpenOptions::new().create(true).append(true).open(file_path)
}

/// 调试日志单文件上限；超过后滚动为 `.1`，单个日志最多占用约两倍空间。
pub const DEBUG_LOG_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// 追加一行到调试日志，超过上限时先把当前文件滚动为 `<name>.1`（覆盖旧的 `.1`）。
pub fn append_capped_log_line(path: &std::path::Path, line: &[u8], max_bytes: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::metadata(path).map(|meta| meta.len() >= max_bytes).unwrap_or(false) {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        let _ = std::fs::rename(path, rotated);
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line)?;
    file.flush()
}

fn default_gui_log_file_path() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir().map(|home| {
            home.join("Library")
                .join("Logs")
                .join("iterate")
                .join("iterate.log")
                .to_string_lossy()
                .to_string()
        });
    }

    #[cfg(target_os = "windows")]
    {
        return dirs::data_local_dir().map(|dir| {
            dir.join("iterate")
                .join("logs")
                .join("iterate.log")
                .to_string_lossy()
                .to_string()
        });
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        dirs::home_dir()
            .map(|home| {
                home.join(".local")
                    .join("state")
                    .join("iterate")
                    .join("iterate.log")
                    .to_string_lossy()
                    .to_string()
            })
            .or_else(|| {
                dirs::data_local_dir().map(|dir| {
                    dir.join("iterate")
                        .join("logs")
                        .join("iterate.log")
                        .to_string_lossy()
                        .to_string()
                })
            })
    }
}

/// 初始化日志系统
pub fn init_logger(config: LogConfig) -> Result<(), Box<dyn std::error::Error>> {
    INIT.call_once(|| {
        let mut builder = Builder::new();

        // 设置日志级别
        builder.filter_level(config.level);

        // 设置日志格式
        builder.format(|buf, record| {
            let log_line = format!(
                "{} [{}] [{}] {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.module_path().unwrap_or("unknown"),
                record.args()
            );

            // 写入到原始目标（stderr 或文件）
            writeln!(buf, "{}", log_line)?;

            Ok(())
        });

        // 根据模式设置输出目标
        if config.is_mcp_mode {
            // MCP 模式：只输出到文件，不输出到 stderr
            if let Some(file_path) = &config.file_path {
                if let Ok(log_file) = open_log_file(file_path) {
                    builder.target(Target::Pipe(Box::new(log_file)));
                } else {
                    // 如果文件打开失败，禁用日志输出
                    builder.filter_level(LevelFilter::Off);
                }
            } else {
                // MCP 模式下没有指定文件路径，禁用日志输出
                builder.filter_level(LevelFilter::Off);
            }
        } else {
            // 非 MCP 模式：如果指定了文件路径，同时输出到文件和 stderr
            if let Some(file_path) = &config.file_path {
                // 尝试打开文件，如果成功则同时输出到文件和 stderr
                if let Ok(log_file) = open_log_file(file_path) {
                    // 使用自定义目标，同时写入文件和 stderr
                    use std::io::Write;
                    struct DualWriter {
                        file: std::fs::File,
                    }
                    impl Write for DualWriter {
                        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                            let written = self.file.write(buf)?;
                            let _ = std::io::stderr().write_all(buf);
                            Ok(written)
                        }
                        fn flush(&mut self) -> std::io::Result<()> {
                            self.file.flush()?;
                            std::io::stderr().flush()
                        }
                    }
                    builder.target(Target::Pipe(Box::new(DualWriter { file: log_file })));
                } else {
                    // 如果文件打开失败，只输出到 stderr
                    builder.target(Target::Stderr);
                }
            } else {
                // 没有指定文件路径，只输出到 stderr
                builder.target(Target::Stderr);
            }
        }

        builder.init();
    });

    Ok(())
}

/// 自动检测模式并初始化日志系统
pub fn auto_init_logger() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    // 检测 MCP 模式：
    // 1. 通过 --mcp-request 参数（等一下 UI 处理 MCP 请求时）
    // 2. 通过可执行文件名包含 "寸止"（MCP 服务器直接运行时）
    let is_mcp_request_mode = args.len() >= 3 && args[1] == "--mcp-request";
    let is_mcp_server = args
        .first()
        .map(|arg| arg.contains("寸止"))
        .unwrap_or(false);
    let is_mcp_mode = is_mcp_request_mode || is_mcp_server;

    let config = if is_mcp_mode {
        // MCP 模式：输出到文件，不能输出到 stderr（会破坏 JSON-RPC 通信）
        let log_file_path = env::var("MCP_LOG_FILE").unwrap_or_else(|_| {
            let temp_dir = env::temp_dir();
            temp_dir
                .join("cunzhi-mcp.log")
                .to_string_lossy()
                .to_string()
        });

        LogConfig {
            level: env::var("RUST_LOG")
                .unwrap_or_else(|_| "info".to_string())
                .parse::<LevelFilter>()
                .unwrap_or(LevelFilter::Info),
            file_path: Some(log_file_path),
            is_mcp_mode: true,
        }
    } else {
        // GUI 模式：输出到 stderr
        let gui_log_file = env::var("ITERATE_LOG_FILE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(default_gui_log_file_path);

        LogConfig {
            level: env::var("RUST_LOG")
                .unwrap_or_else(|_| "info".to_string())
                .parse::<LevelFilter>()
                .unwrap_or(LevelFilter::Info),
            file_path: gui_log_file,
            is_mcp_mode: false,
        }
    };

    init_logger(config)
}

/// 便利宏：只在重要情况下记录日志
#[macro_export]
macro_rules! log_important {
    (error, $($arg:tt)*) => {
        log::error!($($arg)*)
    };
    (warn, $($arg:tt)*) => {
        log::warn!($($arg)*)
    };
    (info, $($arg:tt)*) => {
        log::info!($($arg)*)
    };
}

/// 便利宏：调试日志（只在 debug 级别下输出）
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        log::debug!($($arg)*)
    };
}

/// 便利宏：跟踪日志（只在 trace 级别下输出）
#[macro_export]
macro_rules! log_trace {
    ($($arg:tt)*) => {
        log::trace!($($arg)*)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_config_default() {
        let config = LogConfig::default();
        assert_eq!(config.level, LevelFilter::Warn);
        assert_eq!(config.file_path, None);
        assert_eq!(config.is_mcp_mode, false);
    }

    #[test]
    fn test_append_capped_log_line_rotates_at_limit() {
        let dir = std::env::temp_dir().join(format!("iterate-capped-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("debug.log");
        let rotated = dir.join("debug.log.1");

        append_capped_log_line(&path, b"0123456789", 16).unwrap();
        append_capped_log_line(&path, b"abcdefghij", 16).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"0123456789abcdefghij");
        assert!(!rotated.exists());

        append_capped_log_line(&path, b"new", 16).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read(&rotated).unwrap(), b"0123456789abcdefghij");

        std::fs::write(&path, vec![b'x'; 16]).unwrap();
        append_capped_log_line(&path, b"latest", 16).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"latest");
        assert_eq!(std::fs::read(&rotated).unwrap(), vec![b'x'; 16]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mcp_mode_detection() {
        // 这个测试需要在实际环境中运行
        // 这里只是展示如何测试
    }
}
