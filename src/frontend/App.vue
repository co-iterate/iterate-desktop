<script setup lang="ts">
import type { McpLaunchContext } from './composables/useMcpHandler'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import AppContent from './components/AppContent.vue'
import TrialExpiredOverlay from './components/common/TrialExpiredOverlay.vue'
import InfinitySpeechAnchor from './components/speech/InfinitySpeechAnchor.vue'
import { useAppManager } from './composables/useAppManager'
import { useEventHandlers } from './composables/useEventHandlers'
import { useGlobalSpeechRuntimeHost } from './composables/useGlobalSpeechRuntimeHost'
import { resolveMcpLaunchContext } from './composables/useMcpHandler'

// 使用封装的应用管理器
const {
  naiveTheme,
  mcpRequest,
  showMcpPopup,
  isMuted,
  appConfig,
  isInitializing,
  actions,
} = useAppManager()

// 创建事件处理器
const handlers = useEventHandlers(actions, () => mcpRequest.value)
const speechRuntimeHost = useGlobalSpeechRuntimeHost()

// 试用期状态
const trialStatus = ref<any>(null)
const trialChecked = ref(false)
const licenseChecked = ref(false)
const isLicensed = ref(false)
const onboardingChecked = ref(false)
const appStarted = ref(false)
const mcpLaunchContext = ref<McpLaunchContext | null>(null)
const trialCheckMessage = ref('正在检查授权状态...')
const TRIAL_STATUS_TIMEOUT_MS = 5000
const speechOverlayMode = new URLSearchParams(window.location.search).get('view') === 'speech-overlay'
const windowsPlatform = navigator.platform.toUpperCase().includes('WIN')
interface StartupStatus {
  phase: 'starting' | 'ready' | 'degraded'
  message: string
}
const startupStatus = ref<StartupStatus>({ phase: 'starting', message: '正在启动后台服务' })
const retryingBackgroundServices = ref(false)
let unlistenStartupStatus: (() => void) | null = null
const trialExpired = computed(() => Boolean(trialStatus.value?.is_expired))
const mcpShellMode = computed(() => Boolean(mcpLaunchContext.value?.isMcp))
const appReadyToRender = computed(() => {
  return mcpShellMode.value || (trialChecked.value && licenseChecked.value && onboardingChecked.value)
})
const shouldShowActivation = computed(() => {
  if (mcpShellMode.value)
    return false

  if (!trialChecked.value || !licenseChecked.value || !trialStatus.value)
    return false

  return trialExpired.value || !isLicensed.value
})

// 浏览器监控弹窗监听器
let unlistenBrowserPopup: (() => void) | null = null

interface InitializeApplicationOptions {
  mcpShell?: boolean
}

async function initializeApplication(options: InitializeApplicationOptions = {}) {
  if (appStarted.value)
    return

  const { mcpShell = false } = options
  await actions.app.initialize({ mcpShell })
  appStarted.value = true

  if (mcpShell)
    return

  unlistenBrowserPopup = await listen<{ site_name: string, url: string, title: string }>('show-browser-popup', async (event) => {
    const { site_name, url, title } = event.payload
    // 静音模式下跳过声音和通知
    if (isMuted.value) {
      console.log('🔕 静音模式：跳过浏览器弹窗通知')
      return
    }
    // 播放提示音
    try {
      await invoke('test_audio_sound')
    }
    catch (e) {
      console.log('播放提示音失败:', e)
    }
    // 显示系统通知
    if (Notification.permission === 'granted') {
      const notification = new Notification(`${site_name} AI 完成`, {
        body: title || '点击跳转到聊天页面',
        icon: '/icon.png',
      })
      notification.onclick = () => {
        invoke('open_browser_url', { url })
      }
    }
  })
}

async function retryBackgroundServices() {
  if (retryingBackgroundServices.value)
    return
  retryingBackgroundServices.value = true
  try {
    startupStatus.value = await invoke<StartupStatus>('retry_background_services')
  }
  catch (error) {
    startupStatus.value = { phase: 'degraded', message: `重试失败：${String(error)}` }
  }
  finally {
    retryingBackgroundServices.value = false
  }
}

async function reportTrialDebug(message: string) {
  try {
    await invoke('debug_log', { message: `[License] ${message}` })
  }
  catch {
    // 忽略日志上报失败，避免影响启动流程
  }
}

async function requiresActivationGate() {
  try {
    return await invoke<boolean>('requires_activation_gate')
  }
  catch (e) {
    console.warn('读取启动授权策略失败:', e)
    await reportTrialDebug(`requires_activation_gate:failed ${String(e)}`)
    return true // fail closed
  }
}

async function handleTrialActivated() {
  await reportTrialDebug('handleTrialActivated:start')
  try {
    const status = await getTrialStatusWithTimeout()
    trialStatus.value = status
    isLicensed.value = await invoke<boolean>('is_licensed')
    licenseChecked.value = true
    await reportTrialDebug(`handleTrialActivated:resolved expired=${trialExpired.value}`)
  }
  catch (e) {
    console.warn('激活后重新检查试用状态失败:', e)
    await reportTrialDebug(`handleTrialActivated:failed ${String(e)}`)
    return
  }

  if (!trialExpired.value) {
    onboardingChecked.value = true
    await initializeApplication()
  }
}

function getTrialStatusWithTimeout() {
  void reportTrialDebug(`get_trial_status:invoke timeout=${TRIAL_STATUS_TIMEOUT_MS}`)
  return Promise.race([
    invoke('get_trial_status'),
    new Promise((_, reject) => {
      window.setTimeout(() => {
        reject(new Error(`get_trial_status timeout after ${TRIAL_STATUS_TIMEOUT_MS}ms`))
      }, TRIAL_STATUS_TIMEOUT_MS)
    }),
  ])
}

// 初始化
onMounted(async () => {
  if (speechOverlayMode)
    return

  if (windowsPlatform) {
    unlistenStartupStatus = await listen<StartupStatus>('startup-status-changed', (event) => {
      startupStatus.value = event.payload
    })
    try {
      startupStatus.value = await invoke<StartupStatus>('get_startup_status')
    }
    catch (error) {
      console.warn('读取后台启动状态失败:', error)
    }
  }

  await reportTrialDebug('onMounted:start')
  try {
    mcpLaunchContext.value = await resolveMcpLaunchContext()
    if (mcpLaunchContext.value.isMcp) {
      await reportTrialDebug(`onMounted:mcpShellLaunch kind=${mcpLaunchContext.value.kind}`)
      trialCheckMessage.value = '正在初始化弹窗...'
      await initializeApplication({ mcpShell: true })
      await reportTrialDebug('onMounted:mcpShellInitializeApplication:done')
      return
    }

    if (windowsPlatform) {
      try {
        await speechRuntimeHost.initialize()
      }
      catch (error) {
        console.warn('语音运行时初始化失败，主界面将继续启动:', error)
        await reportTrialDebug(`onMounted:speechRuntimeDegraded ${String(error)}`)
      }
    }
    else {
      await speechRuntimeHost.initialize()
    }

    if (mcpLaunchContext.value.kind === 'invalid')
      await reportTrialDebug(`onMounted:mcpLaunchInvalid ${mcpLaunchContext.value.error ?? 'unknown'}`)

    const activationGateRequired = await requiresActivationGate()
    await reportTrialDebug(`onMounted:activationGateRequired=${activationGateRequired}`)
    if (!activationGateRequired) {
      trialChecked.value = true
      licenseChecked.value = true
      onboardingChecked.value = true
      trialCheckMessage.value = '正在初始化应用...'
      await initializeApplication()
      await reportTrialDebug('onMounted:activationGateBypassed:done')
      return
    }

    // 检查试用期状态
    try {
      trialCheckMessage.value = '正在检查授权状态...'
      const status = await getTrialStatusWithTimeout()
      trialStatus.value = status
      trialChecked.value = true
      isLicensed.value = await invoke<boolean>('is_licensed')
      licenseChecked.value = true
      await reportTrialDebug(`onMounted:trialResolved expired=${trialExpired.value}`)
    }
    catch (e) {
      console.warn('试用期检查失败:', e)
      await reportTrialDebug(`onMounted:trialFailed ${String(e)}`)
      trialStatus.value = {
        is_active: false,
        is_expired: true,
        days_remaining: 0,
        trial_days: 0,
        days_used: 0,
        first_launch_at: '',
        expires_at: '',
        contact_url: 'https://iterate.xin/iterate/',
        expired_message: '暂时无法读取授权状态',
        expired_subtitle: '请重启应用，或直接去官网购买激活码。',
        time_anomaly: false,
      }
      trialChecked.value = true
      licenseChecked.value = true
      return
    }

    if (shouldShowActivation.value) {
      return
    }

    onboardingChecked.value = true
    trialCheckMessage.value = '授权状态正常，正在初始化应用...'
    await initializeApplication()
    await reportTrialDebug('onMounted:initializeApplication:done')
  }
  catch (error) {
    console.error('应用初始化失败:', error)
    await reportTrialDebug(`onMounted:initializeFailed ${String(error)}`)
  }
})

// 清理
onUnmounted(() => {
  unlistenStartupStatus?.()
  unlistenStartupStatus = null
  speechRuntimeHost.dispose()
  if (!speechOverlayMode)
    actions.app.cleanup()
  if (unlistenBrowserPopup) {
    unlistenBrowserPopup()
  }
})
</script>

<template>
  <InfinitySpeechAnchor v-if="speechOverlayMode" />

  <div v-else class="min-h-screen bg-surface transition-colors duration-200">
    <div
      v-if="windowsPlatform && startupStatus.phase !== 'ready'"
      class="fixed left-1/2 top-3 z-[1000] flex max-w-[calc(100%-24px)] -translate-x-1/2 items-center gap-3 rounded-lg border px-3 py-2 text-sm shadow-lg"
      :class="startupStatus.phase === 'degraded'
        ? 'border-amber-400/40 bg-amber-950/95 text-amber-100'
        : 'border-blue-400/30 bg-slate-950/95 text-slate-100'"
    >
      <div
        class="h-2.5 w-2.5 flex-none rounded-full"
        :class="startupStatus.phase === 'starting' ? 'animate-pulse bg-blue-400' : 'bg-amber-400'"
      />
      <span class="min-w-0 truncate">{{ startupStatus.message }}</span>
      <button
        v-if="startupStatus.phase === 'degraded'"
        type="button"
        class="flex-none rounded-md bg-white/10 px-2 py-1 text-xs hover:bg-white/20 disabled:opacity-50"
        :disabled="retryingBackgroundServices"
        @click="retryBackgroundServices"
      >
        {{ retryingBackgroundServices ? '重试中…' : '重试' }}
      </button>
    </div>
    <!-- 试用期到期遮罩 -->
    <TrialExpiredOverlay
      v-if="shouldShowActivation"
      :trial-status="trialStatus"
      @activated="handleTrialActivated"
    />

    <n-config-provider v-else-if="appReadyToRender" :theme="naiveTheme">
      <n-message-provider>
        <n-notification-provider>
          <n-dialog-provider>
            <AppContent
              :mcp-request="mcpRequest" :show-mcp-popup="showMcpPopup" :app-config="appConfig"
              :is-initializing="isInitializing" :is-muted="isMuted"
              @mcp-response="handlers.onMcpResponse" @mcp-cancel="handlers.onMcpCancel"
              @mcp-close-current-dialog="handlers.onMcpCloseCurrentDialog"
              @theme-change="handlers.onThemeChange" @toggle-always-on-top="handlers.onToggleAlwaysOnTop"
              @toggle-mute="handlers.onToggleMute"
              @toggle-audio-notification="handlers.onToggleAudioNotification"
              @update-audio-url="handlers.onUpdateAudioUrl" @test-audio="handlers.onTestAudio"
              @stop-audio="handlers.onStopAudio" @test-audio-error="handlers.onTestAudioError"
              @update-window-size="handlers.onUpdateWindowSize"
              @update-reply-config="handlers.onUpdateReplyConfig" @message-ready="handlers.onMessageReady"
              @config-reloaded="handlers.onConfigReloaded" @bridge-action="handlers.onBridgeAction"
            />
          </n-dialog-provider>
        </n-notification-provider>
      </n-message-provider>
    </n-config-provider>

    <div v-else class="flex min-h-screen items-center justify-center bg-surface px-6 text-center">
      <div class="max-w-sm space-y-3 text-on-surface-secondary">
        <div class="text-lg font-semibold text-on-surface">
          {{ trialCheckMessage }}
        </div>
        <div class="text-sm leading-6">
          如果等待超过几秒仍无变化，应用会自动回落到错误态，避免白屏无反馈。
        </div>
      </div>
    </div>
  </div>
</template>
