<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onUnmounted, ref } from 'vue'
import CrossDeviceToggle from '../common/CrossDeviceToggle.vue'
import IntroTab from '../tabs/IntroTab.vue'
import McpToolsTab from '../tabs/McpToolsTab.vue'
import PromptsTab from '../tabs/PromptsTab.vue'
import SettingsTab from '../tabs/SettingsTab.vue'

const props = defineProps<Props>()

const emit = defineEmits<Emits>()

interface Props {
  currentTheme: string
  alwaysOnTop: boolean
  isMuted: boolean
  audioNotificationEnabled: boolean
  audioUrl: string
  windowWidth: number
  windowHeight: number
  fixedWindowSize: boolean
  codexLivePhase: 'idle' | 'preparing' | 'connecting' | 'active' | 'reconnecting' | 'failed'
  codexLiveStatus: string
}

interface Emits {
  themeChange: [theme: string]
  toggleAlwaysOnTop: []
  toggleMute: []
  toggleAudioNotification: []
  updateAudioUrl: [url: string]
  testAudio: []
  stopAudio: []
  testAudioError: [error: any]
  updateWindowSize: [size: { width: number, height: number, fixed: boolean }]
  configReloaded: []
  toggleCodexLive: []
  toggleCodexLiveMute: []
}

const codexLiveActive = computed(() => ['preparing', 'connecting', 'active', 'reconnecting'].includes(props.codexLivePhase))
const codexLiveTitle = computed(() => `${props.codexLiveStatus}（${codexLiveActive.value ? '短按静音，长按 5 秒结束' : '长按 5 秒启动'}）`)
const CODEX_LIVE_LONG_PRESS_MS = 5_000
let codexLiveHoldTimer: number | null = null
let codexLiveHoldTriggered = false

function cancelCodexLiveHold() {
  if (codexLiveHoldTimer !== null)
    window.clearTimeout(codexLiveHoldTimer)
  codexLiveHoldTimer = null
}

function handleCodexLivePointerDown(event: PointerEvent) {
  if (event.pointerType === 'mouse' && event.button !== 0)
    return
  codexLiveHoldTriggered = false
  cancelCodexLiveHold()
  ;(event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId)
  codexLiveHoldTimer = window.setTimeout(() => {
    codexLiveHoldTimer = null
    codexLiveHoldTriggered = true
    emit('toggleCodexLive')
  }, CODEX_LIVE_LONG_PRESS_MS)
}

function handleCodexLivePointerCancel() {
  cancelCodexLiveHold()
  codexLiveHoldTriggered = false
}

function handleCodexLiveClick() {
  cancelCodexLiveHold()
  if (codexLiveHoldTriggered) {
    codexLiveHoldTriggered = false
    return
  }
  if (codexLiveActive.value)
    emit('toggleCodexLiveMute')
}

onUnmounted(cancelCodexLiveHold)

// 处理配置重新加载事件
function handleConfigReloaded() {
  emit('configReloaded')
}

const activeTab = ref('intro')
const message = useMessage()

// 图标加载错误处理
function handleImageError(event: Event) {
  const img = event.target as HTMLImageElement
  // 如果图标加载失败，隐藏图片元素
  img.style.display = 'none'
  console.warn('LOGO图标加载失败，已隐藏')
}

// 测试popup功能 - 创建独立的popup窗口
async function showTestMcpPopup() {
  try {
    // 创建测试请求数据
    const testRequest = {
      id: `test-${Date.now()}`,
      message: `# 🧪 测试弹窗功能

这是一个**测试弹窗**，用于验证MCP popup组件的功能。

## 功能特性
- ✅ 支持 Markdown 格式显示
- ✅ 支持预定义选项选择
- ✅ 支持自由文本输入
- ✅ 支持图片粘贴上传

## 代码示例
\`\`\`javascript
// 这是一个代码示例
function testPopup() {
  console.log('测试弹窗功能')
  return '成功'
}
\`\`\`

请选择您要测试的功能，或者在下方输入框中添加您的反馈。`,
      predefined_options: ['测试选项功能', '测试文本输入', '测试图片上传', '测试Markdown渲染'],
      is_markdown: true,
    }

    // 调用Tauri命令创建popup窗口
    await invoke('create_test_popup', { request: testRequest })
    message.success('测试popup窗口已创建')
  }
  catch (error) {
    console.error('创建测试popup失败:', error)
    message.error(`创建测试popup失败: ${error}`)
  }
}
</script>

<template>
  <div class="flex flex-col min-h-screen">
    <!-- 主要内容区域 -->
    <div class="flex-1 flex items-start justify-center p-6 pt-12">
      <div class="max-w-6xl w-full">
        <!-- 标题区域 -->
        <div class="text-center mb-8">
          <!-- 主标题 -->
          <div class="flex items-center justify-center gap-3 mb-3" data-guide="app-logo">
            <img
              src="/icons/icon-128.png"
              alt="iterate Logo"
              class="w-10 h-10 rounded-xl shadow-lg"
              @error="handleImageError"
            >
            <h1 class="text-4xl font-medium text-white">
              iterate
            </h1>
            <CrossDeviceToggle />
            <n-button
              size="small"
              type="tertiary"
              circle
              :title="props.isMuted ? '免打扰已开启（点击恢复弹窗通知）' : '免打扰已关闭（点击暂停弹窗通知）'"
              :aria-label="props.isMuted ? '免打扰已开启（点击恢复弹窗通知）' : '免打扰已关闭（点击暂停弹窗通知）'"
              :aria-pressed="props.isMuted"
              data-guide="notification-mute"
              @click="$emit('toggleMute')"
            >
              <template #icon>
                <div :class="props.isMuted ? 'i-carbon-notification-off' : 'i-carbon-notification'" class="w-4 h-4" />
              </template>
            </n-button>
            <n-button
              size="small"
              type="tertiary"
              circle
              :title="codexLiveTitle"
              :aria-label="codexLiveTitle"
              :aria-pressed="codexLiveActive"
              class="global-codex-live ml-2"
              :class="`global-codex-live--${props.codexLivePhase}`"
              data-guide="global-codex-live"
              :data-codex-live-phase="props.codexLivePhase"
              @pointerdown="handleCodexLivePointerDown"
              @pointercancel="handleCodexLivePointerCancel"
              @click="handleCodexLiveClick"
            >
              <template #icon>
                <span class="global-codex-live__glyph" aria-hidden="true">
                  <span class="i-carbon-microphone w-4 h-4" />
                  <span class="global-codex-live__spark">✦</span>
                </span>
              </template>
            </n-button>
            <!-- 测试按钮 -->
            <n-button
              size="small"
              type="tertiary"
              circle
              title="测试 Popup 功能"
              class="ml-2"
              data-guide="test-button"
              @click="showTestMcpPopup"
            >
              <template #icon>
                <div class="i-carbon-test-tool w-4 h-4" />
              </template>
            </n-button>
          </div>

          <!-- 服务器状态 -->
          <div class="mb-4">
            <n-tag type="success" size="small" round class="px-3 py-1">
              <template #icon>
                <div class="w-2 h-2 bg-success rounded-full animate-pulse" />
              </template>
              MCP 服务已启动
            </n-tag>
          </div>

          <!-- 副标题 -->
          <p class="text-base opacity-50 font-normal text-white">
            智能代码审查工具
          </p>
        </div>

        <!-- Tab组件 -->
        <n-tabs v-model:value="activeTab" type="segment" size="small" justify-content="center" data-guide="tabs">
          <n-tab-pane name="intro" tab="介绍">
            <IntroTab />
          </n-tab-pane>
          <n-tab-pane name="mcp-tools" tab="MCP 工具">
            <McpToolsTab />
          </n-tab-pane>
          <n-tab-pane name="prompts" tab="使用说明书">
            <PromptsTab />
          </n-tab-pane>
          <n-tab-pane name="settings" tab="设置" data-guide="settings-tab">
            <SettingsTab
              :current-theme="currentTheme"
              :always-on-top="alwaysOnTop"
              :audio-notification-enabled="audioNotificationEnabled"
              :audio-url="audioUrl"
              :window-width="windowWidth"
              :window-height="windowHeight"
              :fixed-window-size="fixedWindowSize"
              @theme-change="$emit('themeChange', $event)"
              @toggle-always-on-top="$emit('toggleAlwaysOnTop')"
              @toggle-audio-notification="$emit('toggleAudioNotification')"
              @update-audio-url="$emit('updateAudioUrl', $event)"
              @test-audio="$emit('testAudio')"
              @stop-audio="$emit('stopAudio')"
              @test-audio-error="$emit('testAudioError', $event)"
              @update-window-size="$emit('updateWindowSize', $event)"
              @config-reloaded="handleConfigReloaded"
            />
          </n-tab-pane>
        </n-tabs>
      </div>
    </div>
  </div>
</template>

<style scoped>
.global-codex-live {
  position: relative;
  transition: transform 140ms ease, color 180ms ease, box-shadow 180ms ease;
}

.global-codex-live:active {
  transform: translateY(1px) scale(0.94);
}

.global-codex-live__glyph {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.global-codex-live__spark {
  position: absolute;
  top: -7px;
  right: -8px;
  font-size: 9px;
  line-height: 1;
}

.global-codex-live--active {
  color: #34d399;
  box-shadow: 0 0 0 1px rgb(52 211 153 / 45%), 0 0 18px rgb(52 211 153 / 25%);
}

.global-codex-live--preparing,
.global-codex-live--connecting,
.global-codex-live--reconnecting {
  color: #60a5fa;
}

.global-codex-live--preparing .global-codex-live__glyph,
.global-codex-live--connecting .global-codex-live__glyph,
.global-codex-live--reconnecting .global-codex-live__glyph {
  animation: global-codex-live-breathe 1.1s ease-in-out infinite;
}

.global-codex-live--failed {
  color: #fb7185;
}

@keyframes global-codex-live-breathe {
  0%, 100% { opacity: 0.55; transform: scale(0.92); }
  50% { opacity: 1; transform: scale(1.08); }
}

@media (prefers-reduced-motion: reduce) {
  .global-codex-live,
  .global-codex-live__glyph {
    animation: none !important;
    transition: none !important;
  }
}
</style>
