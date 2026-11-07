use std::collections::HashMap;
use tauri::{AppHandle, State};

use crate::config::{save_config, AppConfig, AppState};
use crate::constants::mcp;
// use crate::mcp::tools::acemcp; // 已迁移到独立模块

/// MCP工具配置
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct MCPToolConfig {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub can_disable: bool,
    pub icon: String,
    pub icon_bg: String,
    pub dark_icon_bg: String,
    pub has_config: bool, // 是否有配置选项
}

/// 获取MCP工具配置列表
#[tauri::command]
pub async fn get_mcp_tools_config(
    state: State<'_, AppState>,
) -> Result<Vec<MCPToolConfig>, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;

    Ok(build_mcp_tools_config(&config))
}

/// Build the shared MCP tool presentation from a persisted configuration.
/// Bridge-only processes use this without constructing a synthetic Tauri state.
pub fn build_mcp_tools_config(config: &AppConfig) -> Vec<MCPToolConfig> {
    // 动态构建工具配置列表
    let mut tools = Vec::new();

    // iterate 工具 - 始终存在，无配置选项
    tools.push(MCPToolConfig {
        id: mcp::TOOL_ZHI.to_string(),
        name: "iterate".to_string(),
        description: "智能代码审查交互工具（L0 协调者）。所有对话必经，控制任务流程。支持预定义选项、自由文本输入和图片上传。".to_string(),
        enabled: config.mcp_config.tools.get(mcp::TOOL_ZHI).copied().unwrap_or(true),
        can_disable: false, // iterate 工具是必需的
        icon: "i-carbon-chat text-lg text-blue-600 dark:text-blue-400".to_string(),
        icon_bg: "bg-blue-100 dark:bg-blue-900".to_string(),
        dark_icon_bg: "dark:bg-blue-800".to_string(),
        has_config: false, // iterate 工具没有配置选项
    });

    // 代码搜索工具 - 始终存在，有配置选项
    tools.push(MCPToolConfig {
        id: mcp::TOOL_SOU.to_string(),
        name: "代码搜索".to_string(),
        description: "智能代码搜索工具。自动判断搜索类型：代码相关→语义搜索；外部知识→网络搜索。"
            .to_string(),
        enabled: config
            .mcp_config
            .tools
            .get(mcp::TOOL_SOU)
            .copied()
            .unwrap_or(false),
        can_disable: true,
        icon: "i-carbon-search text-lg text-green-600 dark:text-green-400".to_string(),
        icon_bg: "bg-green-100 dark:bg-green-900".to_string(),
        dark_icon_bg: "dark:bg-green-800".to_string(),
        has_config: true, // 代码搜索工具有配置选项
    });

    // room 编排工具 - 始终存在，无配置选项
    tools.push(MCPToolConfig {
        id: mcp::TOOL_PAI.to_string(),
        name: "Pai Room 编排".to_string(),
        description: "Pai Room 编排工具。生成 codex-room 调度草案和回包协议，不派发子代理。"
            .to_string(),
        enabled: config
            .mcp_config
            .tools
            .get(mcp::TOOL_PAI)
            .copied()
            .unwrap_or(false),
        can_disable: true,
        icon: "i-carbon-bot text-lg text-orange-600 dark:text-orange-400".to_string(),
        icon_bg: "bg-orange-100 dark:bg-orange-900".to_string(),
        dark_icon_bg: "dark:bg-orange-800".to_string(),
        has_config: false, // room 编排工具暂无配置选项
    });

    // 经验查找工具 - 始终存在，无配置选项
    tools.push(MCPToolConfig {
        id: mcp::TOOL_XI.to_string(),
        name: "经验查找".to_string(),
        description: "经验查找工具。在 .cunzhi-knowledge/ 中查找相关历史经验。".to_string(),
        enabled: config
            .mcp_config
            .tools
            .get(mcp::TOOL_XI)
            .copied()
            .unwrap_or(false),
        can_disable: true,
        icon: "i-carbon-book text-lg text-cyan-600 dark:text-cyan-400".to_string(),
        icon_bg: "bg-cyan-100 dark:bg-cyan-900".to_string(),
        dark_icon_bg: "dark:bg-cyan-800".to_string(),
        has_config: false, // 经验查找工具暂无配置选项
    });

    // 提示词库搜索工具 - 始终存在，无配置选项
    tools.push(MCPToolConfig {
        id: mcp::TOOL_CI.to_string(),
        name: "提示词库".to_string(),
        description: "提示词库搜索工具。在 .cunzhi-knowledge/prompts/ 中搜索相关模板。".to_string(),
        enabled: config
            .mcp_config
            .tools
            .get(mcp::TOOL_CI)
            .copied()
            .unwrap_or(false),
        can_disable: true,
        icon: "i-carbon-catalog text-lg text-indigo-600 dark:text-indigo-400".to_string(),
        icon_bg: "bg-indigo-100 dark:bg-indigo-900".to_string(),
        dark_icon_bg: "dark:bg-indigo-800".to_string(),
        has_config: false, // 提示词库工具暂无配置选项
    });

    // 任务系统工具 - 文件持久化任务
    tools.push(MCPToolConfig {
        id: mcp::TOOL_TASK.to_string(),
        name: "任务系统".to_string(),
        description: "文件持久化任务系统。任务存储在 .cunzhi-memory/tasks.json，跨会话持久。"
            .to_string(),
        enabled: config
            .mcp_config
            .tools
            .get(mcp::TOOL_TASK)
            .copied()
            .unwrap_or(true),
        can_disable: true,
        icon: "i-carbon-task text-lg text-teal-600 dark:text-teal-400".to_string(),
        icon_bg: "bg-teal-100 dark:bg-teal-900".to_string(),
        dark_icon_bg: "dark:bg-teal-800".to_string(),
        has_config: false,
    });

    // iPhone 合法动作路由工具
    tools.push(MCPToolConfig {
        id: mcp::TOOL_PHONE_ACTION.to_string(),
        name: "iPhone 动作".to_string(),
        description:
            "把 AI 请求路由成 iPhone 可公开执行的安全动作，如启动语音、写剪贴板、打开 URL。"
                .to_string(),
        enabled: config
            .mcp_config
            .tools
            .get(mcp::TOOL_PHONE_ACTION)
            .copied()
            .unwrap_or(true),
        can_disable: true,
        icon: "i-carbon-mobile text-lg text-sky-600 dark:text-sky-400".to_string(),
        icon_bg: "bg-sky-100 dark:bg-sky-900".to_string(),
        dark_icon_bg: "dark:bg-sky-800".to_string(),
        has_config: false,
    });

    // crontab 定时任务管理工具
    tools.push(MCPToolConfig {
        id: mcp::TOOL_CRON_MANAGE.to_string(),
        name: "定时任务".to_string(),
        description:
            "管理系统 crontab 定时任务。会写入持久 shell 命令，默认关闭且调用前需要 iterate 确认。"
                .to_string(),
        enabled: config
            .mcp_config
            .tools
            .get(mcp::TOOL_CRON_MANAGE)
            .copied()
            .unwrap_or(false),
        can_disable: true,
        icon: "i-carbon-time text-lg text-rose-600 dark:text-rose-400".to_string(),
        icon_bg: "bg-rose-100 dark:bg-rose-900".to_string(),
        dark_icon_bg: "dark:bg-rose-800".to_string(),
        has_config: false,
    });

    // 按启用状态排序，启用的在前
    tools.sort_by(|a, b| b.enabled.cmp(&a.enabled));

    tools
}

/// 设置MCP工具启用状态
#[tauri::command]
pub async fn set_mcp_tool_enabled(
    tool_id: String,
    enabled: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 检查工具是否可以禁用
        if tool_id == mcp::TOOL_ZHI && !enabled {
            return Err("iterate 工具是必需的，无法禁用".to_string());
        }

        // 更新工具状态
        config.mcp_config.tools.insert(tool_id.clone(), enabled);
    }

    // 保存配置
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 使用日志记录状态变更（在 MCP 模式下会自动输出到文件）
    log::info!("MCP工具 {} 状态已更新为: {}", tool_id, enabled);

    Ok(())
}

/// 获取所有MCP工具状态
#[tauri::command]
pub async fn get_mcp_tools_status(
    state: State<'_, AppState>,
) -> Result<HashMap<String, bool>, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.mcp_config.tools.clone())
}

/// 重置MCP工具配置为默认值
#[tauri::command]
pub async fn reset_mcp_tools_config(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        let default_config = mcp::get_default_mcp_config();
        config.mcp_config.tools.clear();
        for tool in &default_config.tools {
            config
                .mcp_config
                .tools
                .insert(tool.tool_id.clone(), tool.enabled);
        }
    }

    // 保存配置
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 使用日志记录配置重置（在 MCP 模式下会自动输出到文件）
    log::info!("MCP工具配置已重置为默认值");
    Ok(())
}

// acemcp 相关命令已迁移

// 已移除 Python Web 服务相关函数，完全使用 Rust 实现
// 如需调试配置，请直接查看本地配置文件
