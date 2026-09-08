<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useMessage } from 'naive-ui'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { hasOpenModifier } from '../../utils/clickModifiers'
import ThemeIcon from '../common/ThemeIcon.vue'
import UsageQuotaPopover from '../common/UsageQuotaPopover.vue'

interface QuotaMetric {
  label: string
  remaining: number
  resetLabel?: string
  resetAtMs?: number
}

interface UsageProvider {
  id: string
  name: string
  accountLabel?: string
  color: string
  iconUrl?: string
  summary: string
  updatedAt?: string
  metrics: QuotaMetric[]
}

interface Props {
  currentTheme?: string
  loading?: boolean
  showMainLayout?: boolean
  alwaysOnTop?: boolean
  isMuted?: boolean
  shortcutEnabled?: boolean
  projectPath?: string
  codexThreadId?: string
  conversationTitle?: string
  linkUrl?: string
  linkTitle?: string
  quotaProviders?: UsageProvider[]
  quotaStatusLabel?: string
  codexLivePhase?: 'idle' | 'preparing' | 'connecting' | 'active' | 'reconnecting' | 'failed'
  codexLiveStatus?: string
}

interface Emits {
  themeChange: [theme: string]
  openMainLayout: []
  toggleAlwaysOnTop: []
  toggleMute: []
  newChat: []
  openIteratePairing: []
  toggleShortcut: [enabled: boolean]
  minimizeWindow: []
  toggleCodexLive: []
  toggleCodexLiveMute: []
  closeCurrentDialog: []
}

const props = withDefaults(defineProps<Props>(), {
  currentTheme: 'dark',
  loading: false,
  showMainLayout: false,
  alwaysOnTop: false,
  isMuted: false,
  shortcutEnabled: true,
  projectPath: undefined,
  codexLivePhase: 'idle',
  codexLiveStatus: '启动全局 GPT-Live 主代理',
})

const emit = defineEmits<Emits>()
const message = useMessage()
let suppressNextProjectPathClick = false
const preventSleepEnabled = ref(false)
const preventSleepPending = ref(false)
let unlistenPreventSleepStatus: UnlistenFn | null = null
let preventSleepPollTimer: number | null = null

async function refreshPreventSleepStatus(logFailure = false) {
  if (preventSleepPending.value)
    return

  try {
    preventSleepEnabled.value = await invoke<boolean>('get_prevent_sleep_status')
  }
  catch (error) {
    if (logFailure)
      console.error('读取合盖运行状态失败:', error)
  }
}

async function handleTogglePreventSleep() {
  if (preventSleepPending.value)
    return

  preventSleepPending.value = true
  try {
    const enabled = await invoke<boolean>('toggle_prevent_sleep')
    preventSleepEnabled.value = enabled
    message.success(enabled ? '合盖运行已开启（仅接电时有效）' : '合盖运行已关闭')
  }
  catch (error) {
    console.error('切换合盖运行失败:', error)
    message.error('切换合盖运行失败')
  }
  finally {
    preventSleepPending.value = false
  }
}

onMounted(async () => {
  try {
    unlistenPreventSleepStatus = await listen<{ enabled?: boolean }>(
      'prevent_sleep_status',
      (event) => {
        if (typeof event.payload?.enabled === 'boolean')
          preventSleepEnabled.value = event.payload.enabled
      },
    )
  }
  catch (error) {
    console.error('监听合盖运行状态失败:', error)
  }

  await refreshPreventSleepStatus(true)
  preventSleepPollTimer = window.setInterval(() => {
    void refreshPreventSleepStatus()
  }, 3000)
})

onUnmounted(() => {
  cancelCodexLiveHold()
  unlistenPreventSleepStatus?.()
  unlistenPreventSleepStatus = null
  if (preventSleepPollTimer !== null)
    window.clearInterval(preventSleepPollTimer)
  preventSleepPollTimer = null
})

// 显示相对路径：用 ~ 替换 home 目录前缀
const displayProjectPath = computed(() => {
  if (!props.projectPath)
    return null
  const home = '/Users/'
  const idx = props.projectPath.indexOf(home)
  if (idx === 0) {
    const afterUsers = props.projectPath.slice(home.length)
    const slashIdx = afterUsers.indexOf('/')
    if (slashIdx !== -1)
      return `~/${afterUsers.slice(slashIdx + 1)}`
  }
  return props.projectPath
})

const projectPathTitle = computed(() => {
  if (!props.projectPath)
    return ''

  const cmdAction = props.codexThreadId
    ? '⌘+点击回到调用本次 MCP 的 Codex 会话'
    : '⌘+点击在 Codex 中打开项目'

  return `${props.projectPath}\n(点击复制完整路径)\n(${cmdAction})`
})

const displayConversationTitle = computed(() => props.conversationTitle?.trim() || null)

async function openCodexTarget() {
  if (props.codexThreadId)
    await invoke('open_codex_thread', { threadId: props.codexThreadId })
  else
    await invoke('open_codex_project', { projectPath: props.projectPath })
}

async function openCodexTargetFromEvent(event: MouseEvent | PointerEvent) {
  event.preventDefault()
  event.stopPropagation()
  message.info(props.codexThreadId ? '正在打开 Codex 会话' : '正在打开 Codex 项目')

  try {
    await openCodexTarget()
    message.success(props.codexThreadId ? '已请求 Codex 打开会话' : '已请求 Codex 打开项目')
  }
  catch (error) {
    console.error('打开 Codex 失败:', error)
    message.error(props.codexThreadId ? '打开 Codex 会话失败' : '打开 Codex 项目失败')
  }
}

// Cmd+按下项目路径时提前打开，避免 WebKit 吞掉修饰键 click
async function handleProjectPathPointerDown(event: MouseEvent | PointerEvent) {
  if (!props.projectPath || !hasOpenModifier(event))
    return
  if (suppressNextProjectPathClick) {
    event.preventDefault()
    event.stopPropagation()
    return
  }

  suppressNextProjectPathClick = true
  await openCodexTargetFromEvent(event)
}

// 点击项目路径：普通点击复制；Cmd/Ctrl 点击作为 pointerdown 未触发时的回退
async function handleProjectPathClick(event: MouseEvent) {
  if (!props.projectPath)
    return

  if (suppressNextProjectPathClick) {
    suppressNextProjectPathClick = false
    event.preventDefault()
    event.stopPropagation()
    return
  }

  if (hasOpenModifier(event)) {
    await openCodexTargetFromEvent(event)
    return
  }

  try {
    await navigator.clipboard.writeText(props.projectPath)
    message.success('已复制路径')
  }
  catch (error) {
    console.error('复制项目路径失败:', error)
    message.error('复制路径失败')
  }
}

async function handleHeaderLinkClick(event: MouseEvent) {
  if (!props.linkUrl)
    return

  event.preventDefault()

  if (hasOpenModifier(event)) {
    try {
      await invoke('open_external_url', { url: props.linkUrl })
    }
    catch (error) {
      console.error('打开链接失败:', error)
      message.error('打开链接失败')
    }
    return
  }

  try {
    await navigator.clipboard.writeText(props.linkUrl)
    message.success('已复制链接')
  }
  catch (error) {
    console.error('复制链接失败:', error)
    message.error('复制链接失败')
  }
}

function handleThemeChange() {
  // 切换到下一个主题
  const nextTheme = props.currentTheme === 'light' ? 'dark' : 'light'
  emit('themeChange', nextTheme)
}

function handleOpenMainLayout() {
  emit('openMainLayout')
}

function handleToggleAlwaysOnTop() {
  emit('toggleAlwaysOnTop')
}

function handleToggleMute() {
  emit('toggleMute')
}

function handleCloseCurrentDialog() {
  emit('closeCurrentDialog')
}

function handleNewChat() {
  emit('newChat')
}

function handleOpenIteratePairing() {
  emit('openIteratePairing')
}

async function handleOpenTerminal() {
  try {
    await invoke('open_terminal', { cwd: props.projectPath })
  }
  catch (error) {
    console.error('打开终端失败:', error)
    message.error('打开终端失败')
  }
}

function handleToggleShortcut() {
  const nextEnabled = !props.shortcutEnabled
  emit('toggleShortcut', nextEnabled)
  message.success(nextEnabled ? '快捷键已启用' : '快捷键已禁用')
  // 禁用时最小化窗口到 dock
  if (!nextEnabled) {
    emit('minimizeWindow')
  }
}

const codexLiveActive = computed(() => ['preparing', 'connecting', 'active', 'reconnecting'].includes(props.codexLivePhase))
const codexLiveTitle = computed(() => {
  const action = codexLiveActive.value ? '短按静音，长按 5 秒结束' : '长按 5 秒启动'
  return `${props.codexLiveStatus || '全局 GPT-Live 主代理'}（${action}）`
})

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
</script>

<template>
  <div class="popup-header-root px-4 py-3 select-none">
    <div class="flex items-center justify-between">
      <!-- 左侧：标题和项目路径/链接 -->
      <div class="flex items-center gap-3 min-w-0 flex-1">
        <UsageQuotaPopover
          v-if="props.quotaProviders?.length"
          :providers="props.quotaProviders"
          title="额度"
          subtitle="本机用量"
          :status-label="props.quotaStatusLabel || '实时'"
          class="popup-header-quota"
        >
          <template #trigger>
            <span class="text-xl font-bold text-white flex-shrink-0 leading-none">∞</span>
            <h1 class="text-base font-medium text-white flex-shrink-0">
              iterate
            </h1>
          </template>
        </UsageQuotaPopover>
        <template v-else>
          <span class="text-xl font-bold text-white flex-shrink-0 leading-none">∞</span>
          <h1 class="text-base font-medium text-white flex-shrink-0">
            iterate
          </h1>
        </template>
        <!-- 链接标题 (cmd+点击打开) -->
        <a
          v-if="props.linkUrl"
          :href="props.linkUrl"
          target="_blank"
          class="text-sm text-primary-400 hover:text-primary-300 truncate cursor-pointer"
          :title="`${props.linkTitle || props.linkUrl}\n(点击复制链接)\n(Cmd+点击打开)`"
          @click="handleHeaderLinkClick"
        >
          {{ props.linkTitle || props.linkUrl }}
        </a>
        <span
          v-else-if="displayProjectPath"
          class="text-sm text-gray-400 truncate cursor-pointer hover:text-primary-400 transition-colors"
          :title="projectPathTitle"
          @pointerdown="handleProjectPathPointerDown"
          @mousedown="handleProjectPathPointerDown"
          @mouseup="handleProjectPathPointerDown"
          @click="handleProjectPathClick"
        >
          / {{ displayProjectPath }}
        </span>
      </div>

      <!-- 右侧：操作按钮 -->
      <n-space size="small">
        <n-button
          size="small"
          quaternary
          circle
          class="codex-live-button"
          :class="`codex-live-button--${props.codexLivePhase}`"
          :title="codexLiveTitle"
          :aria-label="codexLiveTitle"
          :aria-pressed="codexLiveActive"
          :data-codex-live-phase="props.codexLivePhase"
          @pointerdown="handleCodexLivePointerDown"
          @pointercancel="handleCodexLivePointerCancel"
          @click="handleCodexLiveClick"
        >
          <template #icon>
            <span class="codex-live-glyph" aria-hidden="true">
              <span class="i-carbon-voice-activate h-4 w-4" />
              <span class="codex-live-spark">✦</span>
            </span>
          </template>
        </n-button>
        <n-button
          size="small"
          quaternary
          circle
          title="在当前项目打开终端"
          aria-label="在当前项目打开终端"
          @click="handleOpenTerminal"
        >
          <template #icon>
            <div class="i-carbon-terminal w-4 h-4" style="color: #111827;" />
          </template>
        </n-button>
        <n-button
          size="small"
          quaternary
          circle
          title="显示手机连接二维码"
          @click="handleOpenIteratePairing"
        >
          <template #icon>
            <div class="i-carbon-qr-code w-4 h-4" style="color: #111827;" />
          </template>
        </n-button>
        <n-button
          size="small"
          quaternary
          circle
          :title="props.shortcutEnabled ? '快捷键已启用 (点击禁用)' : '快捷键已禁用 (点击启用)'"
          @click="handleToggleShortcut"
        >
          <template #icon>
            <div
              :class="props.shortcutEnabled ? 'i-carbon-flash-filled' : 'i-carbon-flash-off'"
              class="w-4 h-4"
              style="color: #111827;"
            />
          </template>
        </n-button>
        <!-- 静音按钮 -->
        <n-button
          size="small"
          quaternary
          circle
          :title="props.isMuted ? '通知已静音 (点击开启)' : '通知已开启 (点击静音)'"
          @click="handleToggleMute"
        >
          <template #icon>
            <div
              :class="props.isMuted ? 'i-carbon-notification-off' : 'i-carbon-notification'"
              class="w-4 h-4"
              style="color: #111827;"
            />
          </template>
        </n-button>
        <n-button
          size="small"
          quaternary
          circle
          :loading="preventSleepPending"
          :aria-pressed="preventSleepEnabled"
          :aria-label="preventSleepEnabled ? '合盖运行已开启，点击关闭' : '合盖运行已关闭，点击开启'"
          :title="preventSleepEnabled ? '合盖运行已开启（仅接电时有效，点击关闭）' : '合盖运行已关闭（点击开启）'"
          @click="handleTogglePreventSleep"
        >
          <template #icon>
            <div
              class="i-carbon-cafe w-4 h-4"
              :style="{ color: preventSleepEnabled ? '#d97706' : '#111827' }"
            />
          </template>
        </n-button>
        <!-- 新聊天按钮 -->
        <n-button
          size="small"
          quaternary
          circle
          title="在 Codex 中打开当前项目"
          @click="handleNewChat"
        >
          <template #icon>
            <div class="i-carbon-add w-4 h-4" style="color: #111827;" />
          </template>
        </n-button>
        <!-- 置顶按钮 -->
        <n-button
          size="small"
          quaternary
          circle
          :title="props.alwaysOnTop ? '取消置顶' : '窗口置顶'"
          @click="handleToggleAlwaysOnTop"
        >
          <template #icon>
            <div
              :class="props.alwaysOnTop ? 'i-carbon-pin-filled' : 'i-carbon-pin'"
              class="w-4 h-4"
              style="color: #111827;"
            />
          </template>
        </n-button>
        <n-button
          size="small"
          quaternary
          circle
          :title="props.showMainLayout ? '返回聊天' : '打开设置'"
          @click="handleOpenMainLayout"
        >
          <template #icon>
            <div
              :class="props.showMainLayout ? 'i-carbon-chat' : 'i-carbon-settings'"
              class="w-4 h-4"
              style="color: #111827;"
            />
          </template>
        </n-button>
        <!-- 主题切换按钮 -->
        <n-button
          size="small"
          quaternary
          circle
          :title="`切换到${props.currentTheme === 'light' ? '深色' : '浅色'}主题`"
          @click="handleThemeChange"
        >
          <template #icon>
            <ThemeIcon :theme="props.currentTheme" class="w-4 h-4" style="color: #111827;" />
          </template>
        </n-button>
        <n-button
          size="small"
          quaternary
          circle
          class="close-current-dialog-button"
          title="结束当前对话（iterate 继续运行）"
          aria-label="结束当前对话（iterate 继续运行）"
          data-guide="close-current-dialog"
          @click="handleCloseCurrentDialog"
        >
          <template #icon>
            <div class="i-carbon-close w-4 h-4" />
          </template>
        </n-button>
      </n-space>
    </div>
    <div
      v-if="displayConversationTitle"
      class="conversation-title-row mt-2 flex min-w-0 items-center gap-2 border-t pt-2"
      :class="props.currentTheme === 'light' ? 'border-gray-200' : 'border-white/10'"
      :title="displayConversationTitle"
      data-guide="conversation-title"
    >
      <span
        class="flex-shrink-0 text-xs"
        :style="{ color: props.currentTheme === 'light' ? '#374151' : '#e5e7eb' }"
      >标题</span>
      <span
        class="truncate text-sm font-medium"
        :style="{
          color: props.currentTheme === 'light' ? '#111827' : '#f9fafb',
          textDecoration: 'underline',
          textUnderlineOffset: '3px',
        }"
      >{{ displayConversationTitle }}</span>
    </div>
  </div>
</template>

<style scoped>
.popup-header-quota :deep(.usage-trigger) {
  min-width: 0;
  height: auto;
  gap: 10px;
  padding: 2px 6px 2px 4px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: inherit;
  box-shadow: none;
  line-height: 1;
}

.popup-header-quota :deep(.usage-trigger:hover),
.popup-header-quota :deep(.usage-trigger:focus-visible),
.popup-header-quota :deep(.usage-trigger[aria-expanded='true']) {
  background: rgba(255, 255, 255, 0.08);
  box-shadow: none;
  transform: none;
}

.popup-header-quota :deep(.usage-panel) {
  top: calc(100% + 16px);
  left: -4px;
}

.popup-header-quota h1 {
  margin: 0;
  line-height: 1.15;
}

.popup-header-root {
  position: relative;
  z-index: 5000;
  overflow: visible;
  isolation: isolate;
}

.close-current-dialog-button {
  color: #dc2626;
}

.close-current-dialog-button:hover,
.close-current-dialog-button:focus-visible {
  background: rgba(239, 68, 68, 0.14);
  color: #ef4444;
}

.codex-live-button {
  position: relative;
  color: #111827;
  transition:
    color 160ms cubic-bezier(0.16, 1, 0.3, 1),
    background-color 160ms cubic-bezier(0.16, 1, 0.3, 1),
    transform 120ms cubic-bezier(0.16, 1, 0.3, 1);
}

.codex-live-button:active {
  transform: scale(0.94);
}

.codex-live-glyph {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.codex-live-spark {
  position: absolute;
  top: -6px;
  right: -7px;
  font-size: 8px;
  line-height: 1;
}

.codex-live-button--preparing,
.codex-live-button--connecting,
.codex-live-button--reconnecting {
  color: #d97706;
  background: rgba(245, 158, 11, 0.12);
}

.codex-live-button--active {
  color: #0284c7;
  background: rgba(14, 165, 233, 0.14);
}

.codex-live-button--active::after {
  position: absolute;
  inset: 2px;
  border: 1px solid rgba(14, 165, 233, 0.55);
  border-radius: 9999px;
  content: '';
  pointer-events: none;
  animation: codex-live-ring 1.8s cubic-bezier(0.16, 1, 0.3, 1) infinite;
}

.codex-live-button--failed {
  color: #dc2626;
  background: rgba(239, 68, 68, 0.12);
}

.codex-live-button--preparing .codex-live-glyph,
.codex-live-button--connecting .codex-live-glyph,
.codex-live-button--reconnecting .codex-live-glyph {
  animation: codex-live-breathe 1.1s ease-in-out infinite;
}

@keyframes codex-live-ring {
  0% {
    opacity: 0.7;
    transform: scale(0.78);
  }
  70%,
  100% {
    opacity: 0;
    transform: scale(1.24);
  }
}

@keyframes codex-live-breathe {
  0%,
  100% {
    opacity: 1;
    transform: scale(1);
  }
  50% {
    opacity: 0.58;
    transform: scale(0.9);
  }
}

@media (prefers-reduced-motion: reduce) {
  .codex-live-button,
  .codex-live-button::after,
  .codex-live-glyph {
    animation: none !important;
    transition-duration: 0.01ms !important;
  }
}
</style>
