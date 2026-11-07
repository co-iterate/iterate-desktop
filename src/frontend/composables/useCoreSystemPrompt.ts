import { computed } from 'vue'
import { generateFullPrompt } from '../constants/prompts'
import { useMcpToolsReactive } from './useMcpTools'

/**
 * 「核心系统提示词」：使用说明书里的卡片、连接向导和「设置 → AI 客户端」都从这里取，
 * 跟随「MCP 工具」页面的开关生成，不另存一份文案。
 */
export function useCoreSystemPrompt() {
  const { mcpTools, loading, loadMcpTools, enabledTools } = useMcpToolsReactive()

  // 仅将已配置参考规则的 MCP 工具交给提示词生成器
  const promptTools = computed(() => mcpTools.value.map(tool => ({
    id: tool.id,
    name: tool.name,
    description: tool.description,
    enabled: tool.enabled,
    canDisable: tool.can_disable,
    icon: tool.icon,
    iconBg: tool.icon_bg,
    darkIconBg: tool.dark_icon_bg,
  })).filter(tool => tool.id === 'zhi' || tool.id === 'memory' || tool.id === 'sou'))

  const promptContent = computed(() => generateFullPrompt(promptTools.value))
  const enabledPromptToolCount = computed(() => promptTools.value.filter(tool => tool.enabled).length)

  /** 工具配置还没加载过时加载一次；失败时仍可用默认生成结果。 */
  async function ensureMcpToolsLoaded() {
    if (mcpTools.value.length > 0)
      return
    try {
      await loadMcpTools()
    }
    catch (error) {
      console.error('加载MCP工具配置失败:', error)
    }
  }

  return { mcpTools, loading, enabledTools, promptTools, promptContent, enabledPromptToolCount, ensureMcpToolsLoaded }
}
