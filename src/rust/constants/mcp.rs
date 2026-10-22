// MCP 工具相关常量

/// iterate 工具标识符
pub const TOOL_ZHI: &str = "zhi";

/// 记忆管理工具标识符
pub const TOOL_JI: &str = "ji";

/// 代码搜索工具标识符
pub const TOOL_SOU: &str = "sou";

/// room 编排工具标识符
pub const TOOL_PAI: &str = "pai";

/// 经验查找工具标识符
pub const TOOL_XI: &str = "xi";

/// 提示词库搜索工具标识符
pub const TOOL_CI: &str = "ci";

/// PTY 终端执行工具标识符
pub const TOOL_EXEC_PTY: &str = "exec_pty";

/// 浏览器控制工具标识符
pub const TOOL_BROWSER: &str = "browser";

/// 文件持久化任务工具标识符
pub const TOOL_TASK: &str = "task";

/// iPhone 合法动作路由工具标识符
pub const TOOL_PHONE_ACTION: &str = "phone_action";

/// crontab 定时任务管理工具标识符
pub const TOOL_CRON_MANAGE: &str = "cron_manage";

/// 默认启用的工具列表
pub const DEFAULT_ENABLED_TOOLS: &[&str] = &[
    TOOL_ZHI,
    TOOL_JI,
    TOOL_SOU,
    TOOL_PAI,
    TOOL_XI,
    TOOL_CI,
    TOOL_BROWSER,
    TOOL_TASK,
    TOOL_PHONE_ACTION,
    TOOL_CRON_MANAGE,
];

/// 继续回复默认启用状态
pub const DEFAULT_CONTINUE_REPLY_ENABLED: bool = true;

/// 发送或继续时将当前输入备份到剪贴板，默认由用户主动开启
pub const DEFAULT_COPY_SUBMISSION_TO_CLIPBOARD: bool = false;

/// 默认自动继续阈值
pub const DEFAULT_AUTO_CONTINUE_THRESHOLD: u32 = 1000;

/// 默认继续提示词
pub const DEFAULT_CONTINUE_PROMPT: &str = "请按照最佳实践继续";

/// 默认循环提示词（结构化，含停止条件）
pub const DEFAULT_LOOP_PROMPT: &str = "进入自主循环模式。\n\n## 执行规则\n1. 基于当前上下文，按最佳实践继续执行当前任务\n2. 每轮完成后立即调用 iterate/zhi 汇报进度，不要等待用户\n3. 如果任务未完成且无需用户决策，继续自动执行下一步\n\n## 停止条件（满足任一即停止）\n- 任务已全部完成\n- 遇到必须由用户决定的问题\n- 遇到无法自动解决的错误（连续失败2次）\n- 不确定下一步该做什么\n\n## 汇报格式\n每轮简要说明：做了什么 → 结果如何 → 下一步计划";

/// Goal 提交时附加的默认执行规则。目标正文与 xi 去重检查由系统固定注入。
pub const DEFAULT_GOAL_PROMPT_TEMPLATE: &str = "1. 先把这句话整理成可执行目标；在执行任何实现动作前，必须用 Codex 的 get_goal 检查本线程正式 Goal，并完成同步：无正式 Goal 时立即 create_goal；现有 Goal 与本目标相同则继续；现有未完成 Goal 不同则先核对真实状态，只有已有证据证明它确实完成时才 update_goal 为 complete 后创建本目标，否则停止执行并通过 zhi 报告冲突，绝不能伪造完成或在未同步状态下继续。\n2. Codex 正式 Goal 是唯一状态源，iterate Live Goal 只负责展示；create_goal 成功后再开始实现，并在真正完成且验证通过后按 Goal 工具规则更新状态。\n3. 围绕目标自己选择合适的 Skill 和工具，持续执行、修复、验证；能合理推进就不要反问。\n4. 失败就继续定位和修复，直到验证通过、确实阻塞，或碰到目标外的高风险边界。\n5. 完成后再交给用户验收：说明做了什么、验证了什么、还有什么风险。\n6. 只有明显越界、破坏性操作、凭据/登录、Computer Use、提交/推送/发布，或发现需要沉淀的新问题时，才通过 zhi 询问。\n7. 这是目标提交，不是迭代循环；不要生成 [迭代 x/10] 这类轮次提示。\n8. 如果任务完成，明确写“已完成”；如果阻塞，说明原因、证据和可选下一步。";

/// MCP 请求超时时间 (ms)
pub const REQUEST_TIMEOUT_MS: u64 = 30000;

/// MCP 重试次数
pub const MAX_RETRY_COUNT: u32 = 3;

// MCP 工具配置结构体
#[derive(Debug, Clone)]
pub struct McpToolConfig {
    pub tool_id: String,
    pub enabled: bool,
    pub can_disable: bool,
}

impl McpToolConfig {
    pub fn new(tool_id: &str, enabled: bool, can_disable: bool) -> Self {
        Self {
            tool_id: tool_id.to_string(),
            enabled,
            can_disable,
        }
    }
}

// MCP 配置结构体
#[derive(Debug, Clone)]
pub struct McpConfig {
    pub tools: Vec<McpToolConfig>,
    pub continue_reply_enabled: bool,
    pub auto_continue_threshold: u32,
    pub continue_prompt: String,
    pub loop_prompt: String,
    pub goal_prompt_template: String,
    pub request_timeout_ms: u64,
    pub max_retry_count: u32,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            tools: vec![
                McpToolConfig::new(TOOL_ZHI, true, false), // iterate 工具不可禁用
                McpToolConfig::new(TOOL_JI, false, true),  // 记忆管理工具可禁用，默认关闭
                McpToolConfig::new(TOOL_SOU, false, true), // 代码搜索工具可禁用，默认关闭
                McpToolConfig::new(TOOL_PAI, false, true), // room 编排工具可禁用，默认关闭
                McpToolConfig::new(TOOL_XI, false, true),  // 经验查找工具可禁用，默认关闭
                McpToolConfig::new(TOOL_CI, false, true),  // 提示词库搜索工具可禁用，默认关闭
                McpToolConfig::new(TOOL_EXEC_PTY, false, true), // PTY终端执行工具可手动启用
                McpToolConfig::new(TOOL_BROWSER, false, true), // 浏览器控制工具可禁用，默认关闭
                McpToolConfig::new(TOOL_TASK, true, true), // 任务系统可禁用，默认开启
                McpToolConfig::new(TOOL_PHONE_ACTION, true, true), // iPhone 合法动作路由默认开启
                McpToolConfig::new(TOOL_CRON_MANAGE, false, true), // crontab 持久命令工具默认关闭
            ],
            continue_reply_enabled: DEFAULT_CONTINUE_REPLY_ENABLED,
            auto_continue_threshold: DEFAULT_AUTO_CONTINUE_THRESHOLD,
            continue_prompt: DEFAULT_CONTINUE_PROMPT.to_string(),
            loop_prompt: DEFAULT_LOOP_PROMPT.to_string(),
            goal_prompt_template: DEFAULT_GOAL_PROMPT_TEMPLATE.to_string(),
            request_timeout_ms: REQUEST_TIMEOUT_MS,
            max_retry_count: MAX_RETRY_COUNT,
        }
    }
}

impl McpConfig {
    /// 获取工具配置
    pub fn get_tool_config(&self, tool_id: &str) -> Option<&McpToolConfig> {
        self.tools.iter().find(|tool| tool.tool_id == tool_id)
    }

    /// 检查工具是否启用
    pub fn is_tool_enabled(&self, tool_id: &str) -> bool {
        self.get_tool_config(tool_id)
            .map(|tool| tool.enabled)
            .unwrap_or(false)
    }

    /// 设置工具启用状态
    pub fn set_tool_enabled(&mut self, tool_id: &str, enabled: bool) -> bool {
        if let Some(tool) = self.tools.iter_mut().find(|tool| tool.tool_id == tool_id) {
            if tool.can_disable || enabled {
                tool.enabled = enabled;
                return true;
            }
        }
        false
    }

    /// 转换为 JSON 格式
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tools": self.tools.iter().map(|tool| {
                serde_json::json!({
                    "id": tool.tool_id,
                    "enabled": tool.enabled,
                    "can_disable": tool.can_disable
                })
            }).collect::<Vec<_>>(),
            "continue_reply_enabled": self.continue_reply_enabled,
            "auto_continue_threshold": self.auto_continue_threshold,
            "continue_prompt": self.continue_prompt,
            "loop_prompt": self.loop_prompt,
            "goal_prompt_template": self.goal_prompt_template,
            "request_timeout_ms": self.request_timeout_ms,
            "max_retry_count": self.max_retry_count
        })
    }
}

// 便捷函数
/// 获取默认 MCP 配置
pub fn get_default_mcp_config() -> McpConfig {
    McpConfig::default()
}

/// 检查是否为有效的工具 ID
pub fn is_valid_tool_id(tool_id: &str) -> bool {
    matches!(
        tool_id,
        TOOL_ZHI
            | TOOL_JI
            | TOOL_SOU
            | TOOL_PAI
            | TOOL_XI
            | TOOL_CI
            | TOOL_EXEC_PTY
            | TOOL_BROWSER
            | TOOL_TASK
            | TOOL_PHONE_ACTION
            | TOOL_CRON_MANAGE
    )
}
