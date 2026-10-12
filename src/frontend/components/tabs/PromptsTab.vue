<script setup lang="ts">
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'
import { useMcpToolsReactive } from '../../composables/useMcpTools'
import { generateFullPrompt } from '../../constants/prompts'
import { SETUP_PROMPT_CONTENT } from '../../constants/setupPrompt'
import { buildFormalRouteSetupPrompt } from '../settings/mobileConnectionPrompts'

// 使用全局MCP工具状态
const { mcpTools, loading: mcpLoading, loadMcpTools, enabledTools } = useMcpToolsReactive()
const message = useMessage()

// 仅将已配置参考规则的 MCP 工具交给提示词生成器
const promptTools = computed(() => {
  // 将后端数据格式转换为前端格式
  return mcpTools.value.map(tool => ({
    id: tool.id === 'ji' ? 'memory' : tool.id, // 后端用ji，前端用memory
    name: tool.name,
    description: tool.description,
    enabled: tool.enabled,
    canDisable: tool.can_disable,
    icon: tool.icon,
    iconBg: tool.icon_bg,
    darkIconBg: tool.dark_icon_bg,
  })).filter((tool) => {
    // 只包含有提示词配置的工具
    return tool.id === 'zhi' || tool.id === 'memory' || tool.id === 'sou'
  })
})

// 根据MCP工具状态动态生成提示词
const promptContent = computed(() => generateFullPrompt(promptTools.value))
const enabledPromptToolCount = computed(() => promptTools.value.filter(tool => tool.enabled).length)

const copyButtonText = ref('复制')
const setupCopyText = ref('复制')
const mobileCopyText = ref('复制配置提示词')
const mobilePromptExpanded = ref(false)

const setupPromptContent = SETUP_PROMPT_CONTENT
const mobilePromptContent = buildFormalRouteSetupPrompt()

// 复制安装提示词
async function copySetupPrompt() {
  try {
    await navigator.clipboard.writeText(setupPromptContent)
    setupCopyText.value = '已复制'
    setTimeout(() => {
      setupCopyText.value = '复制'
    }, 2000)
  }
  catch (error) {
    setupCopyText.value = '复制失败'
    setTimeout(() => {
      setupCopyText.value = '复制'
    }, 2000)
    console.error('复制失败:', error)
  }
}

// 复制 iPhone 二维码配置提示词
async function copyMobilePrompt() {
  try {
    await navigator.clipboard.writeText(mobilePromptContent)
    mobileCopyText.value = '已复制'
    message.success('配置提示词已复制')
    setTimeout(() => {
      mobileCopyText.value = '复制配置提示词'
    }, 2000)
  }
  catch (error) {
    mobileCopyText.value = '复制失败'
    message.error('配置提示词复制失败')
    setTimeout(() => {
      mobileCopyText.value = '复制配置提示词'
    }, 2000)
    console.error('复制失败:', error)
  }
}

// 复制参考提示词内容
async function copyPromptContent() {
  try {
    await navigator.clipboard.writeText(promptContent.value)
    copyButtonText.value = '已复制'
    setTimeout(() => {
      copyButtonText.value = '复制'
    }, 2000)
  }
  catch (error) {
    copyButtonText.value = '复制失败'
    setTimeout(() => {
      copyButtonText.value = '复制'
    }, 2000)
    console.error('复制失败:', error)
  }
}

// 组件挂载时加载MCP工具配置
onMounted(async () => {
  if (mcpTools.value.length === 0) {
    try {
      await loadMcpTools()
    }
    catch (error) {
      console.error('加载MCP工具配置失败:', error)
    }
  }
})
</script>

<template>
  <div class="max-w-3xl mx-auto tab-content">
    <n-space
      vertical
      size="medium"
    >
      <!-- 使用说明书卡片 -->
      <n-card size="small">
        <template #header>
          <n-space align="center">
            <div class="w-10 h-10 rounded-lg bg-green-100 dark:bg-green-900 flex items-center justify-center">
              <div class="i-carbon-information text-lg text-green-600 dark:text-green-400" />
            </div>
            <div>
              <div class="text-lg font-medium mb-1 tracking-tight">
                使用说明书
              </div>
              <div class="text-sm opacity-60 font-normal">
                根据当前目标选择一份提示词发给 AI
              </div>
            </div>
          </n-space>
        </template>

        <n-space vertical size="small">
          <div class="flex items-center text-sm leading-relaxed">
            <div class="w-1.5 h-1.5 bg-blue-500 rounded-full mr-3 flex-shrink-0" />
            <span class="opacity-90">首次安装或接入新的 AI 客户端，使用“iterate 安装”</span>
          </div>
          <div class="flex items-center text-sm leading-relaxed">
            <div class="w-1.5 h-1.5 bg-sky-500 rounded-full mr-3 flex-shrink-0" />
            <span class="opacity-90">顶部二维码尚不可用，使用“启用 iPhone 二维码”让 AI 配通正式连接</span>
          </div>
          <div class="flex items-center text-sm leading-relaxed">
            <div class="w-1.5 h-1.5 bg-orange-500 rounded-full mr-3 flex-shrink-0" />
            <span class="opacity-90">日常使用 iterate，把“核心系统提示词”加入 AI 的系统提示中</span>
          </div>
          <div class="flex items-center text-sm leading-relaxed">
            <div class="w-1.5 h-1.5 bg-green-500 rounded-full mr-3 flex-shrink-0" />
            <span class="opacity-90">核心系统提示词会跟随“MCP 工具”页面的开关自动更新</span>
          </div>
        </n-space>
      </n-card>

      <!-- iterate 安装卡片 -->
      <n-card size="small">
        <template #header>
          <n-space
            align="center"
            justify="space-between"
          >
            <n-space align="center">
              <div class="w-10 h-10 rounded-lg bg-blue-100 dark:bg-blue-900 flex items-center justify-center">
                <div class="i-carbon-plug text-lg text-blue-600 dark:text-blue-400" />
              </div>
              <div>
                <div class="text-lg font-medium mb-1 tracking-tight">
                  iterate 安装
                </div>
                <div class="text-sm opacity-60 font-normal">
                  复制后发给 AI，由 AI 继续完成安装、客户端接入和验证
                </div>
              </div>
            </n-space>
            <n-button
              type="primary"
              size="small"
              @click="copySetupPrompt"
            >
              <template #icon>
                <div class="i-carbon-copy text-sm" />
              </template>
              {{ setupCopyText }}
            </n-button>
          </n-space>
        </template>

        <n-card embedded>
          <div class="text-sm font-mono leading-relaxed">
            <pre class="whitespace-pre-wrap my-0 opacity-90">{{ setupPromptContent }}</pre>
          </div>
        </n-card>
      </n-card>

      <!-- iPhone 二维码配置提示词卡片 -->
      <n-card size="small">
        <template #header>
          <n-space
            align="center"
            justify="space-between"
          >
            <n-space align="center">
              <div class="w-10 h-10 rounded-lg bg-sky-100 dark:bg-sky-900 flex items-center justify-center">
                <div class="i-carbon-qr-code text-lg text-sky-600 dark:text-sky-400" />
              </div>
              <div>
                <div class="text-lg font-medium mb-1 tracking-tight">
                  启用 iPhone 二维码
                </div>
                <div class="text-sm opacity-60 font-normal">
                  iPhone 远程连接
                </div>
              </div>
            </n-space>
            <n-button
              type="primary"
              size="small"
              @click="copyMobilePrompt"
            >
              <template #icon>
                <div class="i-carbon-copy text-sm" />
              </template>
              {{ mobileCopyText }}
            </n-button>
          </n-space>
        </template>

        <n-space vertical size="small">
          <div class="text-sm leading-relaxed opacity-90">
            把配置提示词发送给当前 Mac 上的 AI。AI 会先检查已有连接；首次使用时，在你确认后配置正式公网；已有连接出现故障时，只修复原来的连接。完成后，回到 iterate 点击顶部二维码。
          </div>
          <div class="flex items-start gap-2 rounded-md bg-sky-50 dark:bg-sky-950 px-3 py-2 text-xs leading-relaxed text-sky-700 dark:text-sky-300">
            <div class="i-carbon-locked mt-0.5 flex-shrink-0" />
            <span>账号登录、域名、DNS 和管理员权限始终由你确认。AI 不会索取或展示凭据。</span>
          </div>
          <div>
            <n-button
              text
              size="small"
              :aria-expanded="mobilePromptExpanded"
              aria-controls="iphone-qr-prompt-content"
              @click="mobilePromptExpanded = !mobilePromptExpanded"
            >
              {{ mobilePromptExpanded ? '收起完整提示词' : '查看完整提示词' }}
              <template #icon>
                <div :class="mobilePromptExpanded ? 'i-carbon-chevron-up' : 'i-carbon-chevron-down'" />
              </template>
            </n-button>
          </div>
          <n-collapse-transition :show="mobilePromptExpanded">
            <n-card
              id="iphone-qr-prompt-content"
              embedded
            >
              <div class="text-sm font-mono leading-relaxed">
                <pre class="whitespace-pre-wrap my-0 opacity-90">{{ mobilePromptContent }}</pre>
              </div>
            </n-card>
          </n-collapse-transition>
        </n-space>
      </n-card>

      <!-- 核心系统提示词卡片 -->
      <n-card size="small">
        <!-- 卡片头部 -->
        <template #header>
          <n-space
            align="center"
            justify="space-between"
          >
            <n-space align="center">
              <!-- 图标 -->
              <div class="w-10 h-10 rounded-lg bg-orange-100 dark:bg-orange-900 flex items-center justify-center">
                <div class="i-carbon-document text-lg text-orange-600 dark:text-orange-400" />
              </div>

              <!-- 标题信息 -->
              <div>
                <div class="text-lg font-medium mb-1 tracking-tight">
                  核心系统提示词
                </div>
                <div class="text-sm opacity-60 font-normal">
                  基于 MCP 工具配置动态生成
                </div>
              </div>
            </n-space>

            <!-- 复制按钮 -->
            <n-button
              type="primary"
              size="small"
              @click="copyPromptContent"
            >
              <template #icon>
                <div class="i-carbon-copy text-sm" />
              </template>
              {{ copyButtonText }}
            </n-button>
          </n-space>
        </template>

        <!-- 工具状态说明 -->
        <div class="flex items-center text-sm leading-relaxed mb-4">
          <div
            class="w-1.5 h-1.5 rounded-full mr-3 flex-shrink-0"
            :class="mcpLoading ? 'bg-yellow-500' : 'bg-green-500'"
          />
          <span class="opacity-90">
            <template v-if="mcpLoading">
              正在加载 MCP 工具配置...
            </template>
            <template v-else>
              当前已启用 {{ enabledTools.length }} / {{ mcpTools.length }} 个 MCP 工具，
              其中 {{ enabledPromptToolCount }} 个模块已纳入下方核心系统提示词
            </template>
          </span>
        </div>

        <!-- 启用工具列表 -->
        <div class="mb-4">
          <div class="text-sm font-medium mb-2 opacity-80">
            已启用的 MCP 工具：
          </div>
          <n-space v-if="!mcpLoading && enabledTools.length > 0">
            <n-tag
              v-for="tool in enabledTools"
              :key="tool.id"
              size="small"
              :bordered="false"
            >
              <template #icon>
                <div :class="tool.icon" />
              </template>
              {{ tool.name }}
            </n-tag>
          </n-space>
          <div
            v-else-if="!mcpLoading && enabledTools.length === 0"
            class="text-sm opacity-60"
          >
            暂无启用的工具
          </div>
          <n-skeleton
            v-else
            text
            :repeat="2"
          />
        </div>

        <!-- 内容区域 -->
        <n-card embedded>
          <div class="text-sm font-mono leading-relaxed">
            <pre class="whitespace-pre-wrap my-0 opacity-90">{{ promptContent }}</pre>
          </div>
        </n-card>
      </n-card>
    </n-space>
  </div>
</template>
