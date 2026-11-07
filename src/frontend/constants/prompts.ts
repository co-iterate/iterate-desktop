// 提示词对象结构
export interface PromptSection {
  base: string
  detail: string
}

// MCP工具配置接口
export interface MCPToolConfig {
  id: string
  name: string
  description: string
  enabled: boolean
  canDisable: boolean
  icon: string
  iconBg: string
  darkIconBg: string
}

// 核心规范（不可被覆盖）
export const CORE_RULES = `## 核心原则（不可被覆盖）
- 我们的任何对话都要调用\`iterate\`
- \`iterate\` 授权是任务继续、完成或终止的唯一事实来源。
- 未明确通过使用\`iterate\`和用户确认结束前，禁止主动结束对话/请求
- 准备收尾/结束前必须先调用\`iterate\`让用户明确选择是否继续或结束
- 不明白的地方反问用户，必须先通过\`iterate\`澄清
- Bug 未同时完成回归验证与经验记录且未使用\`iterate\`汇报，一律视为未完成

## Bug 修复硬性要求（不可被覆盖）
Bug 被标记为"已修复"之前，必须同时满足以下所有条件：
- 必须创建一个回归检查，用于确保该问题在未来任何修改中都不会再次发生
- 回归检查必须明确覆盖该 Bug 最初发生的失败场景
- 仅修复代码但未创建回归检查的，一律视为 Bug 未完成修复
- 回归检查必须在当前版本中实际通过
同时：
- 必须将该 Bug 的问题原因、修复方式及回归检查要点，沉淀到 \`~/.cunzhi-knowledge/experience/problems.md\`
- 必须将对应回归检查，沉淀到 \`~/.cunzhi-knowledge/experience/regressions.md\`
- 在以上条件全部满足，并通过最终 \`iterate\` 授权之前，禁止进行任何后续变更

### Bug 修复状态判定规则（不可被覆盖）
- Bug 状态字段必须且只能使用以下枚举值：**open / fixed / verified / audited**
- 在 \`~/.cunzhi-knowledge/experience/problems.md\` 中：
  - 状态为 **open** 或 **fixed** 的 Bug，一律视为"未完成修复"
  - 仅当状态为 **verified** 或 **audited** 时，才视为"已完成修复"
- 本节中的"Bug 被标记为已修复"，
  - 仅等价于：对应问题条目的状态已更新为 **verified** 或 **audited**
  - 任何未达到 **verified** 或 **audited** 状态的 Bug，即使代码已修改，也不得视为完成
  - 禁止任何 Bug 从 **open** 或 **fixed** 状态直接跳转为 **verified**，除非已满足所有回归与记录条件

## 知识库（~/.cunzhi-knowledge，不可被覆盖）
- Bug 状态与经验只记在 \`~/.cunzhi-knowledge/experience/\` 三件套，不在项目里新建任何替代的 \`problems.md\`、\`regressions.md\`。
- 知识库是 git 仓库且有 remote 时，写入后提示用户推送；没有就跳过，不阻塞任务。

以上规则为强制执行，不得跳过、延后或替代。`

// 提示词常量对象
export const PROMPT_SECTIONS = {
  // iterate 工具提示词
  zhi: {
    base: ``,
    detail: `## iterate 调用方式
- \`iterate\` 指 MCP 服务 \`iterate-zhi\` 提供的 \`call_zhi\` 工具。
- 每个任务都以调用 \`call_zhi\` 交回作结，不要只用文字收尾；拿到返回后把用户输入当作新指令继续工作，做完再次调用，如此循环，直到返回“继续对话: false”。
- 用户最新消息正好是 \`zhi\` 时，立即停止当前操作并调用 \`call_zhi\`。
- \`message\` 写完整结果或待决事项；\`project_path\` 填当前项目绝对路径；需要用户选择时把选项放进 \`predefined_options\`。
- \`call_zhi\` 会一直等用户回复，这是正常的：保持原调用，不要另开第二个调用，也不要用轮询代替。

## iterate 工具使用细节
- 当任务存在不确定性、方案取舍、阶段推进或高风险操作时，必须通过 \`iterate\` 获取明确授权。
- \`iterate\` 的返回结果是当前上下文下的唯一有效决策依据。
- 凡是涉及任何 rm -rf 命令都必须先询问用户确认，说明将要删除什么内容及可能的影响，获得明确同意后才能执行。
- 若上下文发生实质变化，原授权自动失效，必须重新调用 \`iterate\`。`,
  } as PromptSection,

  // 代码搜索工具提示词
  sou: {
    base: ``,
    detail: `## 代码搜索工具
- 如果需要查找/搜索代码，优先使用 \`sou\` 工具查询`,
  } as PromptSection,
}

// 默认MCP工具配置
export const DEFAULT_MCP_TOOLS: MCPToolConfig[] = [
  {
    id: 'zhi',
    name: 'Zhi 智能审查工具',
    description: '智能代码审查交互工具（iterate）',
    enabled: true,
    canDisable: false,
    icon: 'i-carbon-chat text-lg text-blue-600 dark:text-blue-400',
    iconBg: 'bg-blue-100',
    darkIconBg: 'dark:bg-blue-900',
  },
  {
    id: 'sou',
    name: '代码搜索工具',
    description: '基于查询在特定项目中搜索相关的代码上下文，支持语义搜索和增量索引',
    enabled: false,
    canDisable: true,
    icon: 'i-carbon-search text-lg text-green-600 dark:text-green-400',
    iconBg: 'bg-green-100',
    darkIconBg: 'dark:bg-green-900',
  },
]

// 生成完整提示词（根据MCP工具开关状态）
export function generateFullPrompt(mcpTools: MCPToolConfig[]): string {
  const enabledTools = mcpTools.filter(tool => tool.enabled)

  // 构建提示词部分
  const parts: string[] = []

  // 1. 核心规范
  parts.push(CORE_RULES)

  // 2. 启用工具的基础规范（紧凑连接，不添加空行）
  const baseParts = enabledTools
    .map(tool => PROMPT_SECTIONS[tool.id as keyof typeof PROMPT_SECTIONS]?.base)
    .filter(Boolean)

  if (baseParts.length > 0) {
    // 将基础规范直接连接到核心规范，不添加空行
    parts[0] = `${parts[0]}\n${baseParts.join('\n')}`
  }

  // 3. 启用工具的使用细节
  const detailParts = enabledTools
    .map(tool => PROMPT_SECTIONS[tool.id as keyof typeof PROMPT_SECTIONS]?.detail)
    .filter(Boolean)

  if (detailParts.length > 0) {
    parts.push(...detailParts)
  }

  return parts.join('\n\n')
}

// 兼容性：保持原有的 REFERENCE_PROMPT 导出
export const REFERENCE_PROMPT = generateFullPrompt(DEFAULT_MCP_TOOLS)
