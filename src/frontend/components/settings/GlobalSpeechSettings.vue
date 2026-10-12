<script setup lang="ts">
import type { DesktopSpeechRecognitionMode } from '../../services/desktopSpeechRecognitionMode'
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'
import {
  desktopSpeechRecognitionModeOptions,
  getDesktopSpeechRecognitionMode,
  setDesktopSpeechRecognitionMode,
} from '../../services/desktopSpeechRecognitionMode'

type PermissionKey = 'microphone' | 'speechRecognition' | 'inputMonitoring' | 'accessibility'
type PermissionValue = boolean | null

interface PermissionRow {
  key: PermissionKey
  label: string
  description: string
  command: string
  actionCommand: string
  actionLabel: string
  restartHint?: string
}

interface WindowsSpeechCapability {
  available: boolean
  recognizerName: string | null
  culture: string | null
  shortcut: string
  details: string
}

interface SpeechRuntimeStatus {
  permissions: {
    microphone: boolean
    speech_recognition: boolean
    input_monitoring: boolean
    accessibility: boolean
  }
  owner: {
    fn_listener_owner: boolean
    owner_pid: number | null
    owner_bundle_id?: string | null
    owner_path: string | null
    owner_team_id?: string | null
    owner_cdhash?: string | null
    owner_exe_mtime?: string | null
    owner_acquired_at?: string | null
    owner_is_current_process?: boolean
    owner_matches_current_binary?: boolean | null
    current_pid?: number
    current_path?: string | null
    current_team_id?: string | null
    current_cdhash?: string | null
    current_exe_mtime?: string | null
    lock_path: string | null
  }
  overlay: {
    window_exists: boolean
    window_visible: boolean
    listener_ready: boolean
    pending_toggle: boolean
  }
  speech: {
    active: boolean
    recognition_mode?: string | null
    last_partial_length: number | null
    last_final_length: number | null
  }
  writeback: {
    last_target_kind?: string | null
    last_target_bundle_id: string | null
    last_target_pid?: number | null
    last_target_window_label?: string | null
    last_target_request_id?: string | null
    active_popup_window_label?: string | null
    active_popup_request_id?: string | null
    registered_popup_target_count?: number
    latest_registered_popup_pid?: number | null
    latest_registered_popup_window_label?: string | null
    latest_registered_popup_request_id?: string | null
    last_paste_status: string | null
    last_error: string | null
  }
  diagnostics: {
    log_path: string
    last_event: string | null
    last_event_at: string | null
  }
}

const OWN_BUNDLE_ID = 'com.kexin94yyds.iterate'
const LOG_PATH = '/tmp/iterate-native-speech.log'

const message = useMessage()
const windowsPlatform = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('WIN')
const windowsCapability = ref<WindowsSpeechCapability | null>(null)
const loading = ref(false)
const actionLoading = ref<string | null>(null)
const overlayLoading = ref<string | null>(null)
const targetBundleId = ref<string | null>(null)
const runtimeStatus = ref<SpeechRuntimeStatus | null>(null)
const recognitionMode = ref<DesktopSpeechRecognitionMode>(getDesktopSpeechRecognitionMode())
const lastRefreshedAt = ref('')
const lastError = ref('')
const permissions = ref<Record<PermissionKey, PermissionValue>>({
  microphone: null,
  speechRecognition: null,
  inputMonitoring: null,
  accessibility: null,
})

const permissionRows: PermissionRow[] = [
  {
    key: 'microphone',
    label: '麦克风',
    description: '用于采集 Fn 听写音频。',
    command: 'microphone_status',
    actionCommand: 'request_microphone_permission',
    actionLabel: '请求权限',
  },
  {
    key: 'speechRecognition',
    label: '语音识别',
    description: '用于调用 macOS Speech 识别文本。',
    command: 'speech_recognition_status',
    actionCommand: 'request_speech_recognition_permission',
    actionLabel: '请求权限',
  },
  {
    key: 'inputMonitoring',
    label: '输入监控',
    description: '用于监听全局 Fn 按键。',
    command: 'input_monitoring_status',
    actionCommand: 'request_input_monitoring_permission',
    actionLabel: '打开设置',
    restartHint: '授权或重签后通常需要重启 iterate。',
  },
  {
    key: 'accessibility',
    label: '辅助功能',
    description: '用于激活目标 App 并执行写回。',
    command: 'accessibility_status',
    actionCommand: 'request_accessibility_permission',
    actionLabel: '打开设置',
    restartHint: '授权后如仍无法写回，请重启 iterate。',
  },
]

const grantedCount = computed(() =>
  permissionRows.filter(row => permissions.value[row.key] === true).length,
)

const allGranted = computed(() => grantedCount.value === permissionRows.length)
const capturedOwnApp = computed(() => targetBundleId.value === OWN_BUNDLE_ID)
const hasCapturedTarget = computed(() => Boolean(targetBundleId.value))
const displayedLogPath = computed(() => runtimeStatus.value?.diagnostics.log_path || LOG_PATH)
const recognitionModeDescription = computed(() => {
  const option = desktopSpeechRecognitionModeOptions.find(item => item.value === recognitionMode.value)
  return option?.description || ''
})
const runtimeRecognitionMode = computed(() =>
  runtimeStatus.value?.speech.recognition_mode || '未启动',
)
const ownerIsCurrentProcess = computed(() => {
  const owner = runtimeStatus.value?.owner
  if (!owner)
    return false
  return owner.owner_is_current_process ?? owner.fn_listener_owner
})
const ownerTagText = computed(() => {
  const owner = runtimeStatus.value?.owner
  if (!owner)
    return '未知'
  if (ownerIsCurrentProcess.value)
    return `当前进程${owner.current_pid ? ` PID ${owner.current_pid}` : ''}`
  if (owner.owner_pid)
    return `PID ${owner.owner_pid}`
  return '未持有'
})
const ownerDiagnosticRows = computed(() => {
  const owner = runtimeStatus.value?.owner
  if (!owner)
    return []

  return [
    ['当前 PID', owner.current_pid ? String(owner.current_pid) : 'unknown'],
    ['当前路径', owner.current_path || 'unknown'],
    ['当前 Team', owner.current_team_id || 'unknown'],
    ['当前 CDHash', owner.current_cdhash || 'unknown'],
    ['Owner PID', owner.owner_pid ? String(owner.owner_pid) : 'unknown'],
    ['Owner 路径', owner.owner_path || 'unknown'],
    ['Owner Team', owner.owner_team_id || 'unknown'],
    ['Owner CDHash', owner.owner_cdhash || 'unknown'],
    ['Owner 获取时间', owner.owner_acquired_at || 'unknown'],
    ['Lock 文件', owner.lock_path || 'unknown'],
  ]
})
const showOwnerWarning = computed(() => {
  const owner = runtimeStatus.value?.owner
  if (!owner)
    return false
  return Boolean(owner.owner_pid && !ownerIsCurrentProcess.value)
})
const runtimeRows = computed(() => {
  const status = runtimeStatus.value
  if (!status)
    return []

  return [
    {
      label: 'Fn owner',
      value: ownerIsCurrentProcess.value,
      detail: ownerTagText.value,
      warning: showOwnerWarning.value,
    },
    {
      label: 'owner binary',
      value: status.owner.owner_matches_current_binary === true,
      detail: status.owner.owner_matches_current_binary === true ? '匹配当前包' : status.owner.owner_matches_current_binary === false ? '不匹配当前包' : '未知',
      warning: status.owner.owner_matches_current_binary === false,
      neutralWhenFalse: status.owner.owner_matches_current_binary == null,
    },
    {
      label: 'overlay window',
      value: status.overlay.window_exists,
      detail: status.overlay.window_visible ? '可见' : '隐藏',
    },
    {
      label: 'overlay ready',
      value: status.overlay.listener_ready,
      detail: status.overlay.pending_toggle ? '有 pending toggle' : '无 pending',
      warning: status.overlay.pending_toggle,
    },
    {
      label: 'speech active',
      value: status.speech.active,
      detail: status.speech.active ? '识别中' : '空闲',
      neutralWhenFalse: true,
    },
    {
      label: 'recognition mode',
      value: Boolean(status.speech.recognition_mode),
      detail: runtimeRecognitionMode.value,
      neutralWhenFalse: true,
    },
    {
      label: 'popup registry',
      value: (status.writeback.registered_popup_target_count || 0) > 0,
      detail: status.writeback.latest_registered_popup_request_id
        ? `PID ${status.writeback.latest_registered_popup_pid || 'unknown'} / ${status.writeback.latest_registered_popup_request_id}`
        : `${status.writeback.registered_popup_target_count || 0} targets`,
      neutralWhenFalse: true,
    },
    {
      label: 'last external dispatch',
      value: status.writeback.last_paste_status === 'paste-posted',
      detail: status.writeback.last_paste_status || 'none',
      warning: status.writeback.last_paste_status === 'error',
      neutralWhenFalse: !status.writeback.last_paste_status
        || status.writeback.last_paste_status === 'paste-dispatched-unverified',
    },
    {
      label: 'last event',
      value: Boolean(status.diagnostics.last_event),
      detail: status.diagnostics.last_event || 'none',
      neutralWhenFalse: true,
    },
  ]
})

function permissionStatusText(value: PermissionValue) {
  if (value === true)
    return '已授权'
  if (value === false)
    return '未授权'
  return '未知'
}

function permissionStatusType(value: PermissionValue) {
  if (value === true)
    return 'success'
  if (value === false)
    return 'warning'
  return 'default'
}

function statusDotClass(value: PermissionValue) {
  if (value === true)
    return 'bg-emerald-500'
  if (value === false)
    return 'bg-amber-500'
  return 'bg-gray-400'
}

function runtimeStatusType(row: { value: boolean, warning?: boolean, neutralWhenFalse?: boolean }) {
  if (row.warning)
    return 'warning'
  if (row.value)
    return 'success'
  if (row.neutralWhenFalse)
    return 'default'
  return 'warning'
}

function formatInvokeError(error: any) {
  if (typeof error === 'string')
    return error
  if (error?.message)
    return String(error.message)
  return String(error)
}

function updateRecognitionMode(value: string | number) {
  recognitionMode.value = setDesktopSpeechRecognitionMode(value)
  message.success(`语音识别模式已切换为${recognitionMode.value === 'quality' ? '质量优先' : '隐私优先'}`)
}

async function readPermission(row: PermissionRow) {
  try {
    return await invoke<boolean>(row.command)
  }
  catch (error) {
    console.error(`读取${row.label}权限失败:`, error)
    return null
  }
}

async function readCapturedTarget() {
  try {
    return await invoke<string | null>('get_captured_target_app_bundle_id')
  }
  catch (error) {
    console.error('读取语音写回目标失败:', error)
    return null
  }
}

async function refreshStatus(showSuccess = false) {
  if (loading.value)
    return

  loading.value = true
  lastError.value = ''
  try {
    if (windowsPlatform) {
      windowsCapability.value = await invoke<WindowsSpeechCapability>('get_windows_speech_capability')
      runtimeStatus.value = null
      targetBundleId.value = null
      lastRefreshedAt.value = new Date().toLocaleTimeString()
      if (showSuccess)
        message.success('Windows 语音状态已刷新')
      return
    }

    const runtime = await invoke<SpeechRuntimeStatus>('get_speech_runtime_status')
    runtimeStatus.value = runtime

    permissions.value = {
      microphone: runtime.permissions.microphone,
      speechRecognition: runtime.permissions.speech_recognition,
      inputMonitoring: runtime.permissions.input_monitoring,
      accessibility: runtime.permissions.accessibility,
    }
    targetBundleId.value = runtime.writeback.last_target_bundle_id
    lastRefreshedAt.value = new Date().toLocaleTimeString()

    if (showSuccess)
      message.success('语音状态已刷新')
  }
  catch (error: any) {
    console.error('读取语音运行态失败，降级读取权限状态:', error)
    const [microphone, speechRecognition, inputMonitoring, accessibility, target] = await Promise.all([
      readPermission(permissionRows[0]),
      readPermission(permissionRows[1]),
      readPermission(permissionRows[2]),
      readPermission(permissionRows[3]),
      readCapturedTarget(),
    ])

    runtimeStatus.value = null
    permissions.value = {
      microphone,
      speechRecognition,
      inputMonitoring,
      accessibility,
    }
    targetBundleId.value = target
    lastRefreshedAt.value = new Date().toLocaleTimeString()
    lastError.value = String(error)
    message.warning('语音运行态不可用，已降级读取权限状态')
  }
  finally {
    loading.value = false
  }
}

async function requestPermission(row: PermissionRow) {
  if (actionLoading.value)
    return

  actionLoading.value = row.key
  try {
    await invoke(row.actionCommand)
    await refreshStatus(false)
    if (permissions.value[row.key] === true)
      message.success(`${row.label}已授权`)
    else
      message.info(`${row.label}需要在系统设置中确认`)
  }
  catch (error: any) {
    console.error(`${row.label}权限请求失败:`, error)
    message.error(`${row.label}权限请求失败：${formatInvokeError(error)}`)
  }
  finally {
    actionLoading.value = null
  }
}

async function runOverlayAction(command: string, loadingKey: string, successMessage: string) {
  if (overlayLoading.value)
    return

  overlayLoading.value = loadingKey
  try {
    await invoke(command)
    message.success(successMessage)
    await refreshStatus(false)
  }
  catch (error: any) {
    console.error(`${successMessage}失败:`, error)
    message.error(`${successMessage}失败`)
  }
  finally {
    overlayLoading.value = null
  }
}

onMounted(() => {
  refreshStatus(false)
})
</script>

<template>
  <div class="space-y-4">
    <template v-if="windowsPlatform">
      <div class="rounded-lg border border-[var(--n-border-color)] bg-[var(--n-card-color)] p-4">
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div>
            <div class="text-base font-medium">
              Windows 全局语音输入
            </div>
            <div class="text-sm opacity-70 mt-1">
              使用 Windows 本地 System.Speech 识别，识别结果继续经过 iterate 的纠错、肌肉记忆与词汇记忆后处理。
            </div>
          </div>
          <n-button size="small" :loading="loading" @click="refreshStatus(true)">
            <template #icon>
              <div class="i-carbon-renew w-4 h-4" />
            </template>
            刷新
          </n-button>
        </div>

        <div class="mt-4 grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div class="rounded-lg border border-dashed border-[var(--n-border-color)] p-3">
            <div class="text-xs opacity-60">
              本地识别器
            </div>
            <div class="mt-1 text-sm font-medium">
              {{ windowsCapability?.recognizerName || '未检测到' }}
            </div>
            <div class="mt-1 text-xs opacity-60">
              {{ windowsCapability?.culture || 'unknown' }}
            </div>
          </div>
          <div class="rounded-lg border border-dashed border-[var(--n-border-color)] p-3">
            <div class="text-xs opacity-60">
              全局快捷键
            </div>
            <div class="mt-1 text-sm font-medium">
              {{ windowsCapability?.shortcut || 'Shift+Ctrl+Space' }}
            </div>
            <div class="mt-1 text-xs opacity-60">
              在任意输入窗口按下后开始一次听写；识别结束后写回原窗口。
            </div>
          </div>
        </div>

        <n-alert
          class="mt-4"
          :type="windowsCapability?.available ? 'success' : 'warning'"
          :bordered="false"
        >
          {{ windowsCapability?.details || '正在检测 Windows 本地语音识别能力…' }}
        </n-alert>
        <div v-if="lastRefreshedAt" class="text-xs opacity-60 mt-3">
          最近刷新：{{ lastRefreshedAt }}
        </div>
      </div>

      <n-alert type="info" :bordered="false">
        Windows 不使用 macOS 的 Fn / Speech / Accessibility 权限模型，因此这里不会伪装显示“4/4 已授权”。全局快捷键总开关仍沿用 iterate 的“全局快捷键”设置。
      </n-alert>
      <n-alert v-if="lastError" type="error" :bordered="false">
        {{ lastError }}
      </n-alert>
    </template>

    <div v-if="!windowsPlatform" class="rounded-lg border border-[var(--n-border-color)] bg-[var(--n-card-color)] p-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <div class="text-base font-medium">
            全局语音输入
          </div>
          <div class="text-sm opacity-70 mt-1">
            Fn 听写、macOS 权限与写回诊断集中检查。
          </div>
        </div>
        <div class="flex items-center gap-2">
          <n-tag :type="allGranted ? 'success' : 'warning'" :bordered="false" size="small">
            权限 {{ grantedCount }}/{{ permissionRows.length }}
          </n-tag>
          <n-tag
            v-if="runtimeStatus"
            :type="ownerIsCurrentProcess ? 'success' : 'warning'"
            :bordered="false"
            size="small"
          >
            Fn owner {{ ownerTagText }}
          </n-tag>
          <n-tag
            v-if="runtimeStatus"
            :type="runtimeStatus.overlay.pending_toggle ? 'warning' : runtimeStatus.overlay.listener_ready ? 'success' : 'default'"
            :bordered="false"
            size="small"
          >
            {{ runtimeStatus.overlay.pending_toggle ? 'pending toggle' : runtimeStatus.overlay.listener_ready ? 'overlay ready' : 'overlay 未 ready' }}
          </n-tag>
          <n-button size="small" :loading="loading" @click="refreshStatus(true)">
            <template #icon>
              <div class="i-carbon-renew w-4 h-4" />
            </template>
            刷新
          </n-button>
        </div>
      </div>
      <div v-if="lastRefreshedAt" class="text-xs opacity-60 mt-3">
        最近刷新：{{ lastRefreshedAt }}
      </div>
      <n-alert v-if="showOwnerWarning" class="mt-3" type="warning" :bordered="false">
        Fn 监听不在当前窗口进程，当前窗口只用于查看状态；实际监听进程是 {{ ownerTagText }}。
      </n-alert>
    </div>

    <div v-if="!windowsPlatform" class="rounded-lg border border-[var(--n-border-color)] bg-[var(--n-card-color)] p-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div class="min-w-0">
          <div class="text-sm font-medium">
            识别模式
          </div>
          <div class="mt-1 text-xs opacity-60">
            {{ recognitionModeDescription }}
          </div>
        </div>
        <n-segmented
          :value="recognitionMode"
          :options="desktopSpeechRecognitionModeOptions"
          size="small"
          @update:value="updateRecognitionMode"
        />
      </div>
      <n-alert v-if="recognitionMode === 'quality'" class="mt-3" type="info" :bordered="false">
        质量优先不会强制本机识别，是否使用在线识别由 macOS Speech 决定。
      </n-alert>
    </div>

    <div v-if="!windowsPlatform" class="grid grid-cols-1 md:grid-cols-2 gap-3">
      <div
        v-for="row in permissionRows"
        :key="row.key"
        class="rounded-lg border border-[var(--n-border-color)] bg-[var(--n-card-color)] p-4"
      >
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <div class="w-2 h-2 rounded-full shrink-0" :class="statusDotClass(permissions[row.key])" />
              <div class="text-sm font-medium">
                {{ row.label }}
              </div>
            </div>
            <div class="text-xs opacity-60 mt-1 leading-relaxed">
              {{ row.description }}
            </div>
          </div>
          <n-tag :type="permissionStatusType(permissions[row.key])" :bordered="false" size="small">
            {{ permissionStatusText(permissions[row.key]) }}
          </n-tag>
        </div>
        <div class="mt-3 flex flex-wrap items-center gap-2">
          <n-button
            v-if="permissions[row.key] !== true"
            size="tiny"
            type="primary"
            secondary
            :loading="actionLoading === row.key"
            @click="requestPermission(row)"
          >
            {{ row.actionLabel }}
          </n-button>
          <span v-if="row.restartHint" class="text-xs opacity-60">
            {{ row.restartHint }}
          </span>
        </div>
      </div>
    </div>

    <div v-if="!windowsPlatform" class="rounded-lg border border-[var(--n-border-color)] bg-[var(--n-card-color)] p-4">
      <div class="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div class="text-sm font-medium">
            Fn 浮层与写回目标
          </div>
          <div class="text-xs opacity-60 mt-1">
            当前 Fn 为固定全局触发键；这里仅做检查和清理。
          </div>
        </div>
        <div class="flex flex-wrap gap-2">
          <n-button
            size="tiny"
            :loading="overlayLoading === 'show'"
            @click="runOverlayAction('reveal_speech_overlay_window', 'show', '语音浮层已显示')"
          >
            显示浮层
          </n-button>
          <n-button
            size="tiny"
            :loading="overlayLoading === 'hide'"
            @click="runOverlayAction('hide_speech_overlay_window', 'hide', '语音浮层已隐藏')"
          >
            隐藏浮层
          </n-button>
          <n-button
            size="tiny"
            type="warning"
            secondary
            :loading="overlayLoading === 'stop'"
            @click="runOverlayAction('stop_native_speech', 'stop', '语音识别已停止')"
          >
            停止识别
          </n-button>
        </div>
      </div>

      <div class="mt-4 rounded-lg border border-dashed border-[var(--n-border-color)] p-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="text-xs opacity-60">
            最近写回目标
          </div>
          <n-tag
            :type="capturedOwnApp ? 'warning' : hasCapturedTarget ? 'success' : 'default'"
            :bordered="false"
            size="small"
          >
            {{ targetBundleId || '未捕获' }}
          </n-tag>
        </div>
        <n-alert v-if="capturedOwnApp" class="mt-3" type="warning" :bordered="false">
          最近目标是 iterate 自身，可能表示显示浮层前没有保留真实输入 App。
        </n-alert>
      </div>

      <div v-if="runtimeRows.length" class="mt-3 grid grid-cols-1 sm:grid-cols-2 gap-2">
        <div
          v-for="row in runtimeRows"
          :key="row.label"
          class="rounded-lg bg-black-100 px-3 py-2"
        >
          <div class="flex items-center justify-between gap-2">
            <div class="text-xs opacity-60">
              {{ row.label }}
            </div>
            <n-tag :type="runtimeStatusType(row)" :bordered="false" size="small">
              {{ row.detail }}
            </n-tag>
          </div>
        </div>
      </div>

      <div v-if="ownerDiagnosticRows.length" class="mt-3 rounded-lg border border-dashed border-[var(--n-border-color)] p-3">
        <div class="text-xs opacity-60 mb-2">
          Fn owner 诊断
        </div>
        <div class="grid grid-cols-1 gap-1">
          <div
            v-for="[label, value] in ownerDiagnosticRows"
            :key="label"
            class="flex min-w-0 items-start justify-between gap-3 text-xs"
          >
            <span class="shrink-0 opacity-60">{{ label }}</span>
            <code class="min-w-0 break-all text-right">{{ value }}</code>
          </div>
        </div>
      </div>

      <n-alert
        v-if="runtimeStatus?.writeback.last_error"
        class="mt-3"
        type="warning"
        :bordered="false"
      >
        最近错误：{{ runtimeStatus.writeback.last_error }}
      </n-alert>
    </div>

    <n-alert v-if="!windowsPlatform && lastError" type="error" :bordered="false">
      {{ lastError }}
    </n-alert>

    <n-alert v-if="!windowsPlatform" type="info" :bordered="false">
      运行日志：<code>{{ displayedLogPath }}</code>。runtime status 会显示 Fn owner、overlay ready、pending toggle、speech active 与最近写回状态。
    </n-alert>
  </div>
</template>
