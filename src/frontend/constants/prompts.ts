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
- 经验记录必须遵循 \`.cunzhi-knowledge/rules/03-workflows.md\` 中的规范。

## Bug 修复硬性要求（不可被覆盖）
Bug 被标记为"已修复"之前，必须同时满足以下所有条件：
- 必须创建一个回归检查，用于确保该问题在未来任何修改中都不会再次发生
- 回归检查必须明确覆盖该 Bug 最初发生的失败场景
- 仅修复代码但未创建回归检查的，一律视为 Bug 未完成修复
- 回归检查必须在当前版本中实际通过
同时：
- 必须将该 Bug 的问题原因、修复方式及回归检查要点，沉淀到 \`.cunzhi-knowledge/experience/problems.md\`
- 必须将对应回归检查，沉淀到 \`.cunzhi-knowledge/experience/regressions.md\`
- 在以上条件全部满足，并通过最终 \`iterate\` 授权之前，禁止进行任何后续变更

### Bug 修复状态判定规则（不可被覆盖）
- Bug 状态字段必须且只能使用以下枚举值：**open / fixed / verified / audited**
- 在 \`.cunzhi-knowledge/experience/problems.md\` 中：
  - 状态为 **open** 或 **fixed** 的 Bug，一律视为"未完成修复"
  - 仅当状态为 **verified** 或 **audited** 时，才视为"已完成修复"
- 本节中的"Bug 被标记为已修复"，
  - 仅等价于：对应问题条目的状态已更新为 **verified** 或 **audited**
  - 任何未达到 **verified** 或 **audited** 状态的 Bug，即使代码已修改，也不得视为完成
  - 禁止任何 Bug 从 **open** 或 **fixed** 状态直接跳转为 **verified**，除非已满足所有回归与记录条件

## 项目接入与会话启动强制规则（不可被覆盖）
- 任意与具体项目相关的对话，在进入问题分析、Bug 修复或方案设计前：
  - 必须首先确认当前项目已接入 \`.cunzhi-knowledge/\`
- 会话启动时的强制检查顺序为：
  1. 定位当前项目的 git 根目录
  2. 检查是否存在 \`.cunzhi-knowledge/\` 目录
  3. 若不存在：
     - 必须立即提示用户初始化或引入全局知识库
     - 在完成前，禁止进入任何项目级工作流
- 未接入 \`.cunzhi-knowledge/\` 的项目：
  - 仅允许进行临时性、探索性讨论
  - 所有结论一律视为【非最终结论】
  - 禁止对 Bug 给出以下判定：已修复、已解决、verified

## 全局知识库使用规则（不可被覆盖）
- \`.cunzhi-knowledge/\` 是唯一合法的全局问题与经验记录来源
- 所有 Bug 问题记录、回归经验，必须写入该目录下的文件：
  - \`.cunzhi-knowledge/experience/problems.md\`
  - \`.cunzhi-knowledge/experience/regressions.md\`
- 若当前项目中不存在 \`.cunzhi-knowledge/\` 目录：
  - 必须提示用户先初始化或引入全局知识库
  - 禁止在项目内新建任何 \`problems.md\`、\`regressions.md\` 或替代文件
- 当 \`.cunzhi-knowledge/\` 目录下的文件被修改后：
  - 必须提示用户进入该目录执行 \`git add / commit / push\`
  - 在其他项目中使用前，必须先执行 \`git pull\` 以确保事实一致
  - 在用户明确确认已完成同步前，不得视为 Bug 流程完成
- 任何未按上述规则沉淀并同步到全局知识库的 Bug 修复，一律视为未完成修复

## 全局知识库接入与同步规则（不可被覆盖）
### 会话开始
- 在任何项目相关对话开始时：
  - 必须确认项目中存在 \`.cunzhi-knowledge/\` 目录
  - 若不存在：必须提示用户先引入全局知识库，在完成前禁止进入 Bug 修复或结论性分析
- 若目录已存在：
  - \`.cunzhi-knowledge/\` 必须是有效的 git 仓库，且已配置 remote
  - 若无法确认该远程来源：禁止进入 Bug 修复与状态判定流程
  - 在用户确认完成 \`git pull\` 前：禁止基于历史问题或经验给出确定性判断
  - 若无法确认全局知识库已成功同步到最新状态：禁止进入 Bug 状态判定（open / fixed / verified / audited），禁止引用任何历史问题作为依据
  - 必须等待用户确认同步完成后再进行后续分析
### 问题记录
- 所有 Bug、问题与回归经验只能写入：\`.cunzhi-knowledge/experience/problems.md\` 和 \`.cunzhi-knowledge/experience/regressions.md\`
- 禁止在项目内新建或使用任何其他 \`problems.md\`、\`regressions.md\`
### 提交与完成判定
- 当 \`.cunzhi-knowledge/\` 中的文件被修改后：必须提示用户执行 \`git add / commit / push\`
- 在用户确认完成 \`push\` 前：不得将任何 Bug 状态标记为 \`verified\`，不得视为 Bug 修复流程完成

以上规则为强制执行，不得跳过、延后或替代。`

// 提示词常量对象
export const PROMPT_SECTIONS = {
  // iterate 工具提示词
  zhi: {
    base: ``,
    detail: `## iterate 工具使用细节
- 当任务存在不确定性、方案取舍、阶段推进或高风险操作时，必须通过 \`iterate\` 获取明确授权。
- \`iterate\` 的返回结果是当前上下文下的唯一有效决策依据。
- 凡是涉及任何 rm -rf 命令都必须先询问用户确认，说明将要删除什么内容及可能的影响，获得明确同意后才能执行。
- 若上下文发生实质变化，原授权自动失效，必须重新调用 \`iterate\`。`,
  } as PromptSection,

  // 记忆管理工具提示词
  memory: {
    base: ``,
    detail: `## CunZhi Memory 启用约束（不可被覆盖）
- cunzhi-memory **仅允许在 git 仓库中启用**。
- 未检测到 git 根目录时，**禁止**任何 memory 的创建、读取或写入。
- 所有 memory 必须绑定 git 根目录作为唯一 \`project_path\`。

## ji 工具操作
| action | 读/写 | 目标 | category |
|--------|-------|------|----------|
| 回忆 | 读 | memory + knowledge | - |
| 记忆 | 写 | .cunzhi-memory/ | rule/preference/note/context |
| 沉淀 | 写 | .cunzhi-knowledge/ | patterns/problems/regressions |
| 摘要 | 写 | .cunzhi-memory/ | session（L3 会话摘要，自动保留15条） |

## 快捷调用
- 对话开始时调用 \`ji(action=回忆)\`，\`project_path\` 为 git 根目录
- 用户说"请记住："→ 总结后调用 \`ji(action=记忆)\`
- 用户说"等一下" → 调用 \`iterate\`
- 解决问题后 → 调用 \`ji(action=沉淀, category=patterns/problems/regressions)\`
- 对话结束前 → 调用 \`ji(action=摘要)\` 记录本次会话主题、关键词、意图

## 沉淀规则
- 沉淀前建议调用 \`iterate\` 确认内容
- 沉淀后提示用户 git push`,
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
    id: 'memory',
    name: '记忆管理工具',
    description: '智能记忆存储和检索系统',
    enabled: true,
    canDisable: true,
    icon: 'i-carbon-data-base text-lg text-purple-600 dark:text-purple-400',
    iconBg: 'bg-purple-100',
    darkIconBg: 'dark:bg-purple-900',
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
