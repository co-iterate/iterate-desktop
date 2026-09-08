<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useMessage } from 'naive-ui'
import { onMounted, onUnmounted, ref } from 'vue'

interface AiCompletionEvent {
  url: string
  title: string
  site_name: string
  message_preview: string
  timestamp: string
}

interface BrowserMonitorStatus {
  connected: boolean
  monitoring: boolean
}

const message = useMessage()
const isMonitoring = ref(false)
const isConnecting = ref(false)
const browserWsToken = ref('')
const browserWsTokenLoading = ref(false)
const browserWsCopied = ref(false)
const completionEvents = ref<AiCompletionEvent[]>([])

let unlistenCompletion: (() => void) | null = null

async function loadBrowserWsPairingToken() {
  browserWsTokenLoading.value = true
  try {
    browserWsToken.value = await invoke<string>('get_browser_ws_pairing_token')
  }
  catch (error: any) {
    message.error(`读取连接密钥失败: ${error}`)
  }
  finally {
    browserWsTokenLoading.value = false
  }
}

async function copyBrowserWsToken() {
  if (!browserWsToken.value) {
    await loadBrowserWsPairingToken()
  }
  if (!browserWsToken.value)
    return

  try {
    await navigator.clipboard.writeText(browserWsToken.value)
    browserWsCopied.value = true
    message.success('连接密钥已复制')
    window.setTimeout(() => {
      browserWsCopied.value = false
    }, 1600)
  }
  catch (error: any) {
    message.error(`复制失败: ${error}`)
  }
}

function browserWsTokenPreview() {
  if (!browserWsToken.value)
    return '未生成'
  return `${browserWsToken.value.slice(0, 9)}...${browserWsToken.value.slice(-6)}`
}

async function syncMonitoringStatus() {
  try {
    const status = await invoke<BrowserMonitorStatus>('get_browser_monitor_status')
    isMonitoring.value = status.monitoring
  }
  catch (error: any) {
    message.error(`读取监控状态失败: ${error}`)
  }
}

async function startMonitoring() {
  isConnecting.value = true
  try {
    const result = await invoke('start_browser_monitoring', {})
    await syncMonitoringStatus()
    message.success(result as string)
  }
  catch (error: any) {
    message.error(`启动失败: ${error}`)
  }
  finally {
    isConnecting.value = false
  }
}

async function stopMonitoring() {
  try {
    await invoke('stop_browser_monitoring')
    await syncMonitoringStatus()
    message.info('浏览器监控已停止')
  }
  catch (error: any) {
    message.error(`停止失败: ${error}`)
  }
}

async function openUrl(url: string) {
  try {
    await invoke('open_browser_url', { url })
  }
  catch (error: any) {
    message.error(`打开 URL 失败: ${error}`)
  }
}

function clearEvents() {
  completionEvents.value = []
}

function formatTime(timestamp: string) {
  return new Date(timestamp).toLocaleTimeString()
}

onMounted(async () => {
  await loadBrowserWsPairingToken()
  await syncMonitoringStatus()
  unlistenCompletion = await listen<AiCompletionEvent>('browser-ai-completed', (event) => {
    completionEvents.value.unshift(event.payload)
    if (completionEvents.value.length > 20) {
      completionEvents.value = completionEvents.value.slice(0, 20)
    }
    message.info(`${event.payload.site_name} AI 完成！`)
  })
})

onUnmounted(() => {
  if (unlistenCompletion) {
    unlistenCompletion()
  }
})
</script>

<template>
  <n-space vertical size="large">
    <!-- 说明 -->
    <div class="flex items-start">
      <div class="w-1.5 h-1.5 bg-info rounded-full mr-3 flex-shrink-0 mt-2" />
      <div>
        <div class="text-sm font-medium leading-relaxed mb-1">
          使用方式
        </div>
        <div class="text-xs opacity-60 leading-relaxed">
          1. 点击「开始监控」启动 WebSocket 服务器（端口 9333）<br>
          2. 浏览器扩展由官方单独分发，不包含在本 desktop-source 仓库<br>
          3. 已取得官方扩展时，在扩展设置中填入下方连接密钥<br>
          4. 打开 ChatGPT/Gemini 等网页，AI 完成后会收到通知
        </div>
      </div>
    </div>

    <div class="flex items-center justify-between gap-3">
      <div class="min-w-0">
        <div class="text-sm font-medium leading-relaxed">
          浏览器扩展连接密钥
        </div>
        <div class="text-xs opacity-60 leading-relaxed truncate">
          {{ browserWsTokenPreview() }}
        </div>
      </div>
      <n-space>
        <n-button
          size="small"
          :loading="browserWsTokenLoading"
          @click="loadBrowserWsPairingToken"
        >
          重新读取
        </n-button>
        <n-button
          size="small"
          :type="browserWsCopied ? 'success' : 'primary'"
          :loading="browserWsTokenLoading"
          @click="copyBrowserWsToken"
        >
          {{ browserWsCopied ? '已复制' : '复制到扩展' }}
        </n-button>
      </n-space>
    </div>

    <!-- 控制按钮 -->
    <div class="flex items-center justify-between">
      <div class="flex items-center">
        <div class="w-1.5 h-1.5 rounded-full mr-3 flex-shrink-0" :class="isMonitoring ? 'bg-success' : 'bg-gray-400'" />
        <div>
          <div class="text-sm font-medium leading-relaxed">
            监控状态
          </div>
          <div class="text-xs opacity-60">
            {{ isMonitoring ? 'WebSocket 服务器运行中 (端口 9333)' : '未启动' }}
          </div>
        </div>
      </div>
      <n-space>
        <n-button
          v-if="!isMonitoring"
          size="small"
          type="primary"
          :loading="isConnecting"
          @click="startMonitoring"
        >
          开始监控
        </n-button>
        <n-button
          v-else
          size="small"
          type="error"
          @click="stopMonitoring"
        >
          停止监控
        </n-button>
      </n-space>
    </div>

    <!-- AI 完成通知列表 -->
    <div v-if="completionEvents.length > 0">
      <div class="flex items-center justify-between mb-2">
        <div class="text-sm font-medium">
          完成通知 ({{ completionEvents.length }})
        </div>
        <n-button size="tiny" text @click="clearEvents">
          清空
        </n-button>
      </div>
      <div class="space-y-2 max-h-48 overflow-y-auto">
        <div
          v-for="(event, index) in completionEvents"
          :key="index"
          class="p-2 bg-black-100 rounded cursor-pointer hover:bg-black-200"
          @click="openUrl(event.url)"
        >
          <div class="flex items-center justify-between mb-1">
            <span class="text-sm font-medium text-primary">
              {{ event.site_name }}
            </span>
            <span class="text-xs opacity-50">
              {{ formatTime(event.timestamp) }}
            </span>
          </div>
          <div class="text-xs opacity-60 truncate">
            {{ event.title }}
          </div>
          <div class="text-xs text-primary truncate mt-1">
            点击跳转 →
          </div>
        </div>
      </div>
    </div>
  </n-space>
</template>
