// MCP工具注册模块
// 工具实现按各自的模块目录组织

pub mod browser;
pub mod checkpoint;
pub mod ci;
pub mod cron_manage;
pub mod dispatch;
pub mod interaction;
pub mod phone_action;
pub mod pty_exec;
pub mod smart;
pub mod task;
pub mod web_fetch;

// 重新导出工具以便访问
pub use ci::CiTool;
pub use dispatch::DispatchTool;
pub use interaction::InteractionTool;
pub use phone_action::PhoneActionTool;
pub use pty_exec::PtyExecTool;
pub use smart::SmartTool;
