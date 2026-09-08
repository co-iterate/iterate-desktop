<script setup lang="ts">
import type { DesktopCodexLiveSnapshot, GlobalCodexLivePhase } from '../services/desktopCodexLiveControl'
import type { PopupArtifact, PopupInputData } from '../types/popup'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { register, unregister } from '@tauri-apps/plugin-global-shortcut'
import { useMessage } from 'naive-ui'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { setupExitWarningListener } from '../composables/useExitWarning'
import { useKeyboard } from '../composables/useKeyboard'
import { useVersionCheck } from '../composables/useVersionCheck'
import { bridgeFetch } from '../services/bridgeFetch'
import {
  getDesktopCodexLiveSnapshot,
  toggleDesktopCodexLive,
  toggleDesktopCodexLiveMicrophone,
} from '../services/desktopCodexLiveControl'
import { stripAutoPrompt } from '../utils/textUtils'
import UpdateModal from './common/UpdateModal.vue'
import TimelineView from './conversation/TimelineView.vue'
import LayoutWrapper from './layout/LayoutWrapper.vue'
import HtmlArtifactRenderer from './popup/HtmlArtifactRenderer.vue'
import McpPopup from './popup/McpPopup.vue'
import PopupHeader from './popup/PopupHeader.vue'
import MobileConnectionWizard from './settings/MobileConnectionWizard.vue'

interface AppConfig {
  theme: string
  window: {
    alwaysOnTop: boolean
    width: number
    height: number
    fixed: boolean
  }
  audio: {
    enabled: boolean
    url: string
  }
  reply: {
    enabled: boolean
    prompt: string
    loopPrompt: string
  }
}

interface Props {
  mcpRequest: any
  showMcpPopup: boolean
  appConfig: AppConfig
  isInitializing: boolean
  isMuted: boolean
}

interface Emits {
  mcpResponse: [response: any]
  mcpCancel: []
  mcpCloseCurrentDialog: []
  themeChange: [theme: string]
  toggleAlwaysOnTop: []
  toggleMute: []
  toggleAudioNotification: []
  updateAudioUrl: [url: string]
  testAudio: []
  stopAudio: []
  testAudioError: [error: any]
  updateWindowSize: [size: { width: number, height: number, fixed: boolean }]
  updateReplyConfig: [config: { enable_continue_reply?: boolean, continue_prompt?: string, loop_prompt?: string }]
  messageReady: [message: any]
  configReloaded: []
  bridgeAction: [payload: any]
}

interface TimelinePrefillPayload extends PopupInputData {
  userInput: string
  focus?: boolean
}

interface TimelineNodeSwitchResult {
  id: string
  node_type?: string
  nodeType?: string
  content?: string
}

interface TimelinePathNode extends TimelineNodeSwitchResult {
  metadata?: {
    conversation_id?: string | null
    conversationId?: string | null
    project_path?: string | null
    projectPath?: string | null
    request_id?: string | null
    requestId?: string | null
    source?: string | null
  }
}

interface EnsureConversationAssistantNodeResult {
  nodeId: string
  reused: boolean
}

interface AutomationProbeResult {
  status: string
  details: string
}

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

interface McpPopupRef {
  applyTimelinePrefill: (payload: TimelinePrefillPayload) => void
  getDraft: () => TimelinePrefillPayload
}

const props = defineProps<Props>()
const emit = defineEmits<Emits>()
const message = useMessage()

// 版本检查相关
const { versionInfo, showUpdateModal } = useVersionCheck()

// 弹窗中的设置显示控制
const showPopupSettings = ref(false)

const showIteratePairingModal = ref(false)

// 对话时间线状态
const showTimelineDrawer = ref(false)
const conversationTreeId = ref<string | null>(null)
const currentConversationNodeId = ref<string | null>(null)
const activeConversationRouteKey = ref<string | null>(null)
const mcpPopupRef = ref<McpPopupRef | null>(null)
let popupSettingsDraft: TimelinePrefillPayload | null = null
const activeArtifact = ref<PopupArtifact | null>(null)
const activeArtifactContent = computed(() => activeArtifact.value?.content || '')
let skipNextBridgePush = false
let triggerBridgePush: (() => void) | null = null
const currentWindowLabel = getCurrentWindow().label || 'current-window'
const LAST_VALID_PROJECT_PATH_KEY = 'iterate.last_valid_project_path'
const LAST_SEEN_APP_VERSION_KEY = 'iterate.last_seen_app_version'
const lastValidProjectPath = ref<string | null>(localStorage.getItem(LAST_VALID_PROJECT_PATH_KEY))
const quotaProviders = ref<UsageProvider[]>([])
const quotaStatusLabel = ref('实时')
let quotaRefreshInFlight: Promise<void> | null = null
let quotaRefreshInFlightKey: string | null = null
let quotaRefreshTimer: number | null = null

async function refreshUsageQuotaProviders(codexHome = normalizeCodexHome(props.mcpRequest)) {
  const requestKey = codexHome || ''
  if (quotaRefreshInFlight && quotaRefreshInFlightKey === requestKey)
    return quotaRefreshInFlight

  quotaRefreshInFlightKey = requestKey
  quotaRefreshInFlight = invoke('get_usage_quota_providers', codexHome ? { codexHome } : {})
    .then((providers) => {
      quotaProviders.value = Array.isArray(providers) ? providers as UsageProvider[] : []
      quotaStatusLabel.value = quotaProviders.value.length > 0 ? '实时' : '离线'
    })
    .catch((error) => {
      quotaStatusLabel.value = '离线'
      console.warn('获取 AI 额度失败:', error)
    })
    .finally(() => {
      quotaRefreshInFlight = null
      quotaRefreshInFlightKey = null
    })

  return quotaRefreshInFlight
}

function isMacPlatform(): boolean {
  return typeof navigator !== 'undefined' && /mac/i.test(navigator.userAgent)
}

function extractAppVersion(appInfo: string): string | null {
  const match = appInfo.match(/v(\d+\.\d+\.\d+)/)
  return match ? match[1] : null
}

async function showPostUpdateAutomationNotice() {
  if (!isMacPlatform()) {
    return
  }

  try {
    const appInfo = await invoke('get_app_info') as string
    const currentVersion = extractAppVersion(appInfo)
    if (!currentVersion) {
      return
    }

    const previousVersion = localStorage.getItem(LAST_SEEN_APP_VERSION_KEY)
    localStorage.setItem(LAST_SEEN_APP_VERSION_KEY, currentVersion)

    if (!previousVersion || previousVersion === currentVersion) {
      return
    }

    const probe = await invoke('probe_codex_automation_permission') as AutomationProbeResult
    if (probe.status === 'granted') {
      message.success(
        `已更新到 v${currentVersion}。我已完成一次 Codex 自动化探测，当前这台 Mac 的 AppleScript 发送链路可用。`,
        { duration: 9000, closable: true },
      )
      return
    }

    if (probe.status === 'permission_required') {
      message.warning(
        `已更新到 v${currentVersion}。我已主动触发一次自动化权限检查；如果系统弹出允许框，请点允许。若没有弹框或仍失败，请到“系统设置 > 隐私与安全性 > 自动化”检查 iterate 对 Codex / System Events 的授权。`,
        { duration: 12000, closable: true },
      )
      return
    }

    message.warning(
      `已更新到 v${currentVersion}。自动化探测未完全通过：${probe.details}`,
      { duration: 12000, closable: true },
    )
  }
  catch (error) {
    console.warn('检查版本变更提示失败:', error)
    message.info(
      '已检测到版本更新，但自动化探测未完成；如果后续 AppleScript 发送失败，请到“系统设置 > 隐私与安全性 > 自动化”检查 iterate 对 Codex / System Events 的授权。',
      { duration: 10000, closable: true },
    )
  }
}

function isDisplayableProjectPath(projectPath: string | null | undefined): projectPath is string {
  if (typeof projectPath !== 'string')
    return false

  const trimmed = projectPath.trim()
  if (!trimmed)
    return false

  const lower = trimmed.toLowerCase()
  if (trimmed === '.' || lower === 'unknown' || lower.startsWith('unknown:') || lower.startsWith('standalone:'))
    return false

  return trimmed.startsWith('/')
}

function resolveDisplayProjectPath(request: any): string | null {
  const candidate = request?.project_path ?? request?.projectPath
  if (isDisplayableProjectPath(candidate))
    return candidate.trim()
  if (isDisplayableProjectPath(lastValidProjectPath.value))
    return lastValidProjectPath.value.trim()
  return null
}

function resolveRequestProjectPath(request: any): string | null {
  const candidate = request?.project_path ?? request?.projectPath
  return isDisplayableProjectPath(candidate) ? candidate.trim() : null
}

function normalizeCodexHome(request: any): string | null {
  const candidate = request?.codex_home ?? request?.codexHome
  if (typeof candidate !== 'string')
    return null
  const trimmed = candidate.trim()
  return trimmed || null
}

const effectiveProjectPath = computed(() => resolveDisplayProjectPath(props.mcpRequest))
const reliableRequestProjectPath = computed(() => resolveRequestProjectPath(props.mcpRequest))
const globalCodexLivePhase = ref<GlobalCodexLivePhase>('idle')
const globalCodexLiveStatus = ref('启动全局 GPT-Live 主代理')
const globalCodexLiveProjectPath = ref<string | null>(null)
let desktopCodexLivePollTimer: number | null = null
let desktopCodexLivePollInFlight = false
let desktopCodexLiveToggleInFlight = false
let desktopCodexLiveMuteInFlight = false

function applyDesktopCodexLiveSnapshot(snapshot: DesktopCodexLiveSnapshot) {
  globalCodexLivePhase.value = snapshot.phase
  globalCodexLiveStatus.value = snapshot.status_text || '启动全局 GPT-Live 主代理'
  globalCodexLiveProjectPath.value = snapshot.active_project_path
}

async function pollDesktopCodexLiveControl() {
  if (desktopCodexLivePollInFlight)
    return
  desktopCodexLivePollInFlight = true
  try {
    const snapshot = await getDesktopCodexLiveSnapshot()
    applyDesktopCodexLiveSnapshot(snapshot)
  }
  catch (error) {
    console.warn('[GPT-Live] 读取全局状态失败:', error)
  }
  finally {
    desktopCodexLivePollInFlight = false
  }
}

async function initializeDesktopCodexLiveControl() {
  // AppContent is only a controller/view. The canonical App.vue root owns the
  // microphone and WebRTC transport, so changing or closing this view cannot
  // terminate the global session.
  await pollDesktopCodexLiveControl()
  desktopCodexLivePollTimer = window.setInterval(() => {
    void pollDesktopCodexLiveControl()
  }, props.showMcpPopup ? 1500 : 1000)
}
const popupContextKey = computed(() => {
  if (effectiveProjectPath.value)
    return `${currentWindowLabel}:${effectiveProjectPath.value}`
  return `${currentWindowLabel}:${normalizeRequestId(props.mcpRequest) || 'unknown-request'}`
})
const promptContextStateByKey = ref<Record<string, Record<string, { current_state?: boolean, is_active?: boolean }>>>({})
const currentPromptContextState = computed(() => promptContextStateByKey.value[popupContextKey.value] || {})

function applyPromptContextState(config: any) {
  const prompts = Array.isArray(config?.prompts) ? config.prompts : null
  if (!prompts)
    return config

  const contextState = promptContextStateByKey.value[popupContextKey.value]
  if (!contextState)
    return config

  return {
    ...config,
    prompts: prompts.map((prompt: any) => {
      const localState = contextState[prompt.id]
      if (!localState)
        return prompt
      return {
        ...prompt,
        current_state: localState.current_state ?? prompt.current_state,
        is_active: localState.is_active ?? prompt.is_active,
      }
    }),
  }
}

function handlePopupConditionalStateChange(payload: { promptId: string, current_state?: boolean, is_active?: boolean }) {
  const key = popupContextKey.value
  const nextByKey = { ...promptContextStateByKey.value }
  const nextContext = { ...(nextByKey[key] || {}) }
  nextContext[payload.promptId] = {
    ...(nextContext[payload.promptId] || {}),
    ...(payload.current_state !== undefined ? { current_state: payload.current_state } : {}),
    ...(payload.is_active !== undefined ? { is_active: payload.is_active } : {}),
  }
  nextByKey[key] = nextContext
  promptContextStateByKey.value = nextByKey
  triggerBridgePush?.()
}

function handleWindowConditionalAction(action: any): boolean {
  if (action?.action === 'update_window_conditional_state') {
    handlePopupConditionalStateChange({
      promptId: action.promptId,
      current_state: action.newState,
      is_active: action.isActive,
    })
    return true
  }
  if (action?.action === 'update_window_conditional_active') {
    handlePopupConditionalStateChange({
      promptId: action.promptId,
      is_active: action.isActive,
    })
    return true
  }
  return false
}

async function syncWindowRegistration(projectPath?: string | null, request?: any) {
  const resolvedProjectPath = projectPath || await invoke<string>('get_default_window_registration_label')
  await invoke('register_window_instance', {
    projectPath: resolvedProjectPath,
    requestId: normalizeRequestId(request),
    title: resolveRequestMessage(request),
  })
  const window = getCurrentWindow()
  const title = navigator.platform.toUpperCase().includes('WIN')
    ? 'iterate'
    : `iterate - ${resolvedProjectPath}`
  await window.setTitle(title)
}

function normalizeRequestId(request: any): string | null {
  const candidates = [
    request?.id,
    request?.request_id,
    request?.requestId,
    request?.metadata?.request_id,
    request?.metadata?.requestId,
  ]

  for (const candidate of candidates) {
    if (typeof candidate !== 'string')
      continue
    const trimmed = candidate.trim()
    if (trimmed.length > 0)
      return trimmed
  }

  return null
}

function normalizeConversationRouteId(request: any): string | null {
  const candidates = [
    request?.timeline_route_id,
    request?.timelineRouteId,
    request?.conversation_route_id,
    request?.conversationRouteId,
    request?.metadata?.timeline_route_id,
    request?.metadata?.timelineRouteId,
    request?.metadata?.conversation_route_id,
    request?.metadata?.conversationRouteId,
    request?.codex_thread_id,
    request?.codexThreadId,
    request?.metadata?.codex_thread_id,
    request?.metadata?.codexThreadId,
  ]

  for (const candidate of candidates) {
    if (typeof candidate !== 'string')
      continue
    const trimmed = candidate.trim()
    if (trimmed.length > 0)
      return trimmed
  }

  return null
}

function normalizeProjectPath(request: any): string | null {
  const projectPath = request?.project_path ?? request?.projectPath
  if (typeof projectPath !== 'string')
    return null
  const trimmed = projectPath.trim()
  if (!trimmed || trimmed === 'Unknown')
    return null
  return trimmed
}

function resolveConversationRouteKey(request: any): string | null {
  return normalizeConversationRouteId(request) ?? normalizeRequestId(request) ?? normalizeProjectPath(request)
}

function isTerminalLiveGoalStatus(status: unknown): boolean {
  if (typeof status !== 'string')
    return false
  return ['completed', 'complete', 'cleared', 'cancelled', 'canceled', 'failed'].includes(status.trim().toLowerCase())
}

function normalizeLiveGoalConversationRouteId(liveGoal: any, projectPath: string | null): string | null {
  if (!liveGoal || typeof liveGoal !== 'object' || isTerminalLiveGoalStatus(liveGoal.status))
    return null

  if (!projectPath)
    return null

  const liveGoalProjectPath = typeof liveGoal.project_path === 'string'
    ? liveGoal.project_path.trim()
    : typeof liveGoal.projectPath === 'string'
      ? liveGoal.projectPath.trim()
      : ''
  if (!liveGoalProjectPath || liveGoalProjectPath !== projectPath)
    return null

  const candidates = [
    liveGoal.codex_thread_id,
    liveGoal.codexThreadId,
    liveGoal.thread_id,
    liveGoal.threadId,
  ]
  for (const candidate of candidates) {
    if (typeof candidate !== 'string')
      continue
    const trimmed = candidate.trim()
    if (trimmed.length > 0)
      return trimmed
  }

  return null
}

async function getLiveGoalForBridge(): Promise<any | null> {
  try {
    return await invoke('get_live_goal')
  }
  catch (err) {
    console.warn('[Bridge] 获取 Live Goal 失败:', err)
    return null
  }
}

async function resolveConversationRouteKeyWithFallback(request: any): Promise<string | null> {
  const explicitRouteId = normalizeConversationRouteId(request)
  if (explicitRouteId)
    return explicitRouteId

  const projectPath = normalizeProjectPath(request)
  const liveGoal = await getLiveGoalForBridge()
  return normalizeLiveGoalConversationRouteId(liveGoal, projectPath)
    ?? normalizeRequestId(request)
    ?? projectPath
}

function resolveRequestMessage(request: any): string | null {
  const messageText = request?.message
  if (typeof messageText !== 'string')
    return null
  const trimmed = messageText.trim()
  return trimmed.length > 0 ? trimmed : null
}

function normalizePredefinedOptions(request: any): string[] | null {
  const options = request?.predefined_options ?? request?.predefinedOptions
  if (!Array.isArray(options))
    return null

  const normalized = options
    .map(item => typeof item === 'string' ? item.trim() : '')
    .filter(item => item.length > 0)

  return normalized.length > 0 ? normalized : null
}

function resolveIsMarkdown(request: any): boolean {
  if (typeof request?.is_markdown === 'boolean')
    return request.is_markdown
  if (typeof request?.isMarkdown === 'boolean')
    return request.isMarkdown
  return true
}

function reportTimelineDebugLog(message: string, payload?: any) {
  let suffix = ''
  if (payload !== undefined) {
    try {
      suffix = ` ${JSON.stringify(payload)}`
    }
    catch {
      suffix = ' [payload_unserializable]'
    }
  }
  void invoke('debug_log', { message: `[Timeline] ${message}${suffix}` }).catch(() => {})
}

function normalizeTimelineNodeType(node: TimelinePathNode): string {
  return String(node.node_type ?? node.nodeType ?? '').trim().toLowerCase()
}

function normalizeTimelineMetadataValue(value: unknown): string | null {
  if (typeof value !== 'string')
    return null

  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

function normalizeTimelineNodeRequestId(node: TimelinePathNode): string | null {
  const candidates = [
    node.metadata?.request_id,
    node.metadata?.requestId,
  ]

  for (const candidate of candidates) {
    const normalized = normalizeTimelineMetadataValue(candidate)
    if (normalized)
      return normalized
  }

  return null
}

function normalizeTimelineNodeConversationId(node: TimelinePathNode): string | null {
  const candidates = [
    node.metadata?.conversation_id,
    node.metadata?.conversationId,
  ]

  for (const candidate of candidates) {
    const normalized = normalizeTimelineMetadataValue(candidate)
    if (normalized)
      return normalized
  }

  return null
}

function normalizeTimelineNodeProjectPath(node: TimelinePathNode): string | null {
  const candidates = [
    node.metadata?.project_path,
    node.metadata?.projectPath,
  ]

  for (const candidate of candidates) {
    const normalized = normalizeTimelineMetadataValue(candidate)
    if (normalized)
      return normalized
  }

  return null
}

function timelineNodeBelongsToActiveRoute(
  node: TimelinePathNode,
  requestId: string | null,
  projectPath: string | null,
  treeId: string | null,
) {
  const nodeConversationId = normalizeTimelineNodeConversationId(node)
  if (nodeConversationId && treeId && nodeConversationId !== treeId)
    return false

  const nodeProjectPath = normalizeTimelineNodeProjectPath(node)
  if (nodeProjectPath && projectPath && nodeProjectPath !== projectPath)
    return false

  const nodeRequestId = normalizeTimelineNodeRequestId(node)
  if (requestId && nodeRequestId && nodeRequestId !== requestId)
    return false

  return true
}

function filterTimelineNodesForActiveRoute(
  nodes: TimelinePathNode[],
  request: any,
  treeId: string | null,
  routeKey: string | null = resolveConversationRouteKey(request),
) {
  const requestId = routeKey
  const projectPath = normalizeProjectPath(request)
  return nodes.filter(node => timelineNodeBelongsToActiveRoute(node, requestId, projectPath, treeId))
}

async function findExistingAssistantNodeInCurrentPath(
  treeId: string,
  routeKey: string | null,
  messageText: string,
) {
  if (!routeKey || !currentConversationNodeId.value)
    return null

  try {
    const nodes = await invoke<TimelinePathNode[]>('get_conversation_path', {
      treeId,
      nodeId: currentConversationNodeId.value,
    })
    const existingNode = [...(nodes || [])].reverse().find((node) => {
      return normalizeTimelineNodeType(node) === 'assistant'
        && normalizeTimelineNodeRequestId(node) === routeKey
        && String(node.content ?? '') === messageText
    })

    if (!existingNode)
      return null

    console.info('[Timeline] 复用已存在的 assistant 节点，跳过 frontend fallback 补录', {
      treeId,
      nodeId: existingNode.id,
      routeKey,
    })
    reportTimelineDebugLog('复用已存在的 assistant 节点，跳过 frontend fallback 补录', {
      treeId,
      nodeId: existingNode.id,
      routeKey,
    })
    return existingNode.id
  }
  catch (error) {
    console.warn('[Timeline] 检查已存在 assistant 节点失败，继续补录:', error)
    return null
  }
}

async function refreshCurrentConversationNode(reason: string) {
  if (!conversationTreeId.value)
    return

  try {
    const latestNodeId = await invoke<string | null>('get_current_conversation_node_id', {
      treeId: conversationTreeId.value,
    })
    if (latestNodeId !== currentConversationNodeId.value) {
      console.info('[Timeline] 当前节点已刷新', {
        reason,
        treeId: conversationTreeId.value,
        previousNodeId: currentConversationNodeId.value,
        nextNodeId: latestNodeId,
      })
      currentConversationNodeId.value = latestNodeId
    }
  }
  catch (error) {
    console.error('[Timeline] 刷新当前节点失败:', error)
  }
}

async function ensureConversationAssistantNode(treeId: string, request: any, routeKey: string | null) {
  const messageText = resolveRequestMessage(request)
  if (!messageText)
    return null

  const existingNodeId = await findExistingAssistantNodeInCurrentPath(treeId, routeKey, messageText)
  if (existingNodeId)
    return { nodeId: existingNodeId, reused: true }

  const metadata = {
    project_path: normalizeProjectPath(request),
    predefined_options: normalizePredefinedOptions(request),
    selected_option: null,
    images: null,
    link_url: typeof request?.link_url === 'string' ? request.link_url : null,
    link_title: typeof request?.link_title === 'string' ? request.link_title : null,
    request_id: routeKey,
    source: 'frontend_sync_fallback',
  }

  try {
    const result = await invoke<EnsureConversationAssistantNodeResult>('ensure_conversation_assistant_node', {
      treeId,
      content: messageText,
      isMarkdown: resolveIsMarkdown(request),
      metadata,
    })
    if (result.reused) {
      console.info('[Timeline] 后端复用 assistant 节点，跳过 frontend fallback 补录', {
        treeId,
        nodeId: result.nodeId,
        routeKey,
      })
      reportTimelineDebugLog('后端复用 assistant 节点，跳过 frontend fallback 补录', {
        treeId,
        nodeId: result.nodeId,
        routeKey,
      })
      return result
    }

    console.info('[Timeline] 已补录 assistant 节点', {
      treeId,
      nodeId: result.nodeId,
      routeKey,
    })
    return result
  }
  catch (error) {
    console.error('[Timeline] 补录 assistant 节点失败:', error)
    return null
  }
}

async function syncConversationRequestNode(request: any) {
  if (!request) {
    console.info('[Timeline] mcpRequest 为空，重置时间线状态')
    showTimelineDrawer.value = false
    conversationTreeId.value = null
    currentConversationNodeId.value = null
    activeConversationRouteKey.value = null
    return
  }

  const explicitConversationRouteId = normalizeConversationRouteId(request)
  const routeKey = await resolveConversationRouteKeyWithFallback(request)
  console.info('[Timeline] 开始同步请求节点', {
    routeKey,
    requestId: normalizeRequestId(request),
    conversationRouteId: explicitConversationRouteId,
    projectPath: normalizeProjectPath(request),
  })
  reportTimelineDebugLog('开始同步请求节点', {
    routeKey,
    requestId: normalizeRequestId(request),
    conversationRouteId: explicitConversationRouteId,
    projectPath: normalizeProjectPath(request),
  })
  const shouldResetTree = routeKey !== activeConversationRouteKey.value || !conversationTreeId.value
  if (shouldResetTree) {
    try {
      const treeId = await invoke<string>('create_conversation_tree', {
        requestId: routeKey,
        projectPath: normalizeProjectPath(request),
      })
      conversationTreeId.value = treeId
      activeConversationRouteKey.value = routeKey
      console.info('[Timeline] 对话树同步完成', {
        treeId,
        routeKey,
      })
    }
    catch (error) {
      console.error('[Timeline] 创建对话树失败:', error)
      return
    }
  }

  if (!conversationTreeId.value)
    return

  try {
    await refreshCurrentConversationNode('syncConversationRequestNode')

    const assistantNodeResult = await ensureConversationAssistantNode(
      conversationTreeId.value,
      request,
      routeKey,
    )
    if (assistantNodeResult && !assistantNodeResult.reused) {
      currentConversationNodeId.value = assistantNodeResult.nodeId
    }

    console.info('[Timeline] 当前节点同步结果', {
      treeId: conversationTreeId.value,
      nodeId: currentConversationNodeId.value,
      routeKey,
    })
  }
  catch (error) {
    console.error('[Timeline] 获取当前节点失败:', error)
    currentConversationNodeId.value = null
  }
}

async function handleTimelineNodeSwitch(nodeId: string) {
  if (!conversationTreeId.value)
    return

  try {
    skipNextBridgePush = true
    const node = await invoke<TimelineNodeSwitchResult>('switch_conversation_node', {
      treeId: conversationTreeId.value,
      nodeId,
    })
    currentConversationNodeId.value = node?.id ?? nodeId

    const rawContent = typeof node?.content === 'string' ? node.content : ''
    const prefillContent = stripAutoPrompt(rawContent)
    mcpPopupRef.value?.applyTimelinePrefill({
      userInput: prefillContent,
      selectedOptions: [],
      draggedImages: [],
      attachedFiles: [],
      focus: true,
    })

    message.success('已切换到历史节点')
  }
  catch (error) {
    console.error('[Timeline] 切换节点失败:', error)
    message.error('切换历史节点失败')
  }
}

function handleCloseArtifact() {
  activeArtifact.value = null
}

async function handleCopyArtifact() {
  if (!activeArtifactContent.value)
    return

  try {
    await navigator.clipboard.writeText(activeArtifactContent.value)
    message.success('HTML 已复制')
  }
  catch (error) {
    console.error('复制 HTML 失败:', error)
    message.error('复制 HTML 失败')
  }
}

async function openArtifactInBrowser(artifact: PopupArtifact | null) {
  const content = artifact?.content || ''
  if (!content)
    return false

  try {
    const filePath = await invoke<string>('open_html_artifact_in_browser', {
      content,
      title: artifact?.title || 'html-artifact.html',
    })
    message.success(`已在浏览器打开：${filePath}`)
    return true
  }
  catch (error) {
    console.error('在浏览器打开 HTML Artifact 失败:', error)
    message.error('在浏览器打开失败')
    return false
  }
}

async function handleOpenArtifact(artifact: PopupArtifact) {
  await openArtifactInBrowser(artifact)
}

async function handleOpenArtifactInBrowser() {
  await openArtifactInBrowser(activeArtifact.value)
}

// 窗口最小化功能
async function minimizeWindow() {
  try {
    const window = getCurrentWindow()
    await window.minimize()
  }
  catch (error) {
    console.error('最小化窗口失败:', error)
  }
}

// 窗口恢复功能
async function restoreWindow() {
  try {
    const window = getCurrentWindow()
    await window.unminimize()
    await window.setFocus()
  }
  catch (error) {
    console.error('恢复窗口失败:', error)
  }
}

// 用于检测单独按下 Shift 键（不与其他键组合）
let shiftKeyAlone = false

// 键盘快捷键处理
const { handleExitShortcut } = useKeyboard()

// 快捷键启用状态（窗口级别，不再是全局）
const localShortcutEnabled = ref(true)

// 400ms balances phone-action latency (<0.4s) against bridge churn:
// at 120ms a single popup generated ~8 lookups/sec with ~0% hit rate (P-2026 CPU batch).
const ACTION_POLL_INTERVAL_MS = 400
const ACTION_POLL_TIMEOUT_MS = 3000
let actionPollTimer: number | null = null
let actionPollInFlight = false
let lastActionPollErrorAt = 0
let visibilityHandler: (() => void) | null = null
let unlistenConversationNodeRecorded: (() => void) | null = null

async function pullCachedBridgeAction(reason: string) {
  const projectPath = props.mcpRequest?.project_path
  if (!projectPath || actionPollInFlight)
    return

  actionPollInFlight = true
  try {
    const requestId = normalizeRequestId(props.mcpRequest)
    let pullUrl = `http://127.0.0.1:8080/bridge/pull_action?project_path=${encodeURIComponent(projectPath)}`
    if (requestId)
      pullUrl += `&request_id=${encodeURIComponent(requestId)}`
    const res = await bridgeFetch(pullUrl, {
      method: 'POST',
      signal: AbortSignal.timeout(ACTION_POLL_TIMEOUT_MS),
    })
    if (!res.ok)
      return
    const data = await res.json()
    const action = data?.action
    if (action && !handleWindowConditionalAction(action))
      emit('bridgeAction', action)
  }
  catch (e) {
    const now = Date.now()
    if (now - lastActionPollErrorAt > 5000) {
      console.warn(`[Bridge] pull_action 拉取失败（${reason}）`, e)
      lastActionPollErrorAt = now
    }
  }
  finally {
    actionPollInFlight = false
  }
}

// 处理快捷键切换（窗口级别）
async function handleToggleShortcut(enabled: boolean) {
  localShortcutEnabled.value = enabled
  // 同时注册/注销全局快捷键
  if (enabled && props.showMcpPopup) {
    await registerShortcuts()
  }
  else {
    await unregisterShortcuts()
  }
}

async function triggerShortcutToggle() {
  const nextEnabled = !localShortcutEnabled.value
  await handleToggleShortcut(nextEnabled)
  message.success(nextEnabled ? '快捷键已启用' : '快捷键已禁用')

  if (!nextEnabled) {
    await minimizeWindow()
  }
}

// 切换弹窗设置显示
async function togglePopupSettings() {
  if (!showPopupSettings.value) {
    popupSettingsDraft = mcpPopupRef.value?.getDraft() ?? null
    showPopupSettings.value = true
    return
  }

  showPopupSettings.value = false
  await nextTick()
  if (popupSettingsDraft) {
    mcpPopupRef.value?.applyTimelinePrefill(popupSettingsDraft)
    popupSettingsDraft = null
  }
}

// 监听 MCP 请求变化，当有新请求时重置设置页面状态并更新窗口注册
watch(() => props.mcpRequest, async (newRequest) => {
  popupSettingsDraft = null
  const timelinePayload = {
    hasRequest: !!newRequest,
    requestId: normalizeRequestId(newRequest),
    projectPath: normalizeProjectPath(newRequest),
  }
  console.info('[Timeline] 监听到 mcpRequest 变化', timelinePayload)
  reportTimelineDebugLog('监听到 mcpRequest 变化', timelinePayload)
  if (newRequest && props.showMcpPopup)
    void refreshUsageQuotaProviders(normalizeCodexHome(newRequest))

  if (newRequest && showPopupSettings.value) {
    // 有新的 MCP 请求时，自动切换回消息页面
    showPopupSettings.value = false
  }

  await syncConversationRequestNode(newRequest)

  const nextProjectPath = resolveDisplayProjectPath(newRequest)
  if (nextProjectPath && nextProjectPath !== lastValidProjectPath.value) {
    lastValidProjectPath.value = nextProjectPath
    localStorage.setItem(LAST_VALID_PROJECT_PATH_KEY, nextProjectPath)
  }

  // 更新窗口注册（使用新的项目路径）
  if (nextProjectPath) {
    try {
      await syncWindowRegistration(nextProjectPath, newRequest)
    }
    catch (error) {
      console.error('更新窗口注册失败:', error)
    }
  }
}, { immediate: true, deep: true })

// 全局键盘事件处理器
function handleGlobalKeydown(event: KeyboardEvent) {
  if (
    event.key === 'Escape'
    && !event.metaKey
    && !event.ctrlKey
    && !event.altKey
    && !event.shiftKey
    && props.showMcpPopup
  ) {
    event.preventDefault()
    void triggerShortcutToggle()
    return
  }

  // 如果快捷键被禁用，不处理任何快捷键逻辑
  if (!localShortcutEnabled.value) {
    return
  }

  if (event.key.toLowerCase() === 'n' && event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey && props.showMcpPopup) {
    event.preventDefault()
    void handleNewChat()
    return
  }

  // 检测是否单独按下 Shift 键
  if (event.key === 'Shift') {
    shiftKeyAlone = true
    return
  }
  // 如果按下其他键，说明不是单独按下
  shiftKeyAlone = false

  // Shift+Tab 恢复窗口 - 仅在 MCP 弹窗显示时生效
  // 排除 Cmd/Ctrl/Alt 等系统组合键
  if (event.key === 'Tab' && event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey && props.showMcpPopup) {
    event.preventDefault()
    restoreWindow()
    return
  }

  // Tab 键最小化当前弹窗 - 仅在 MCP 弹窗显示时生效
  // 排除 Cmd/Ctrl/Alt 等系统组合键
  if (event.key === 'Tab' && !event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey && props.showMcpPopup) {
    event.preventDefault()
    minimizeWindow()
    return
  }

  handleExitShortcut(event)
}

// 键释放处理器
function handleGlobalKeyup(event: KeyboardEvent) {
  // 如果快捷键被禁用，不处理任何快捷键逻辑
  if (!localShortcutEnabled.value) {
    return
  }

  // Shift 键切换置顶
  if (event.key === 'Shift' && shiftKeyAlone && props.showMcpPopup) {
    event.preventDefault()
    emit('toggleAlwaysOnTop')
  }
  shiftKeyAlone = false
}

// 处理顶部 + 按钮：免权限打开 Codex（优先带当前项目）
async function handleNewChat() {
  const projectPath = reliableRequestProjectPath.value
  if (!projectPath) {
    message.warning('当前请求没有可靠项目路径，未打开 Codex')
    return
  }

  try {
    const result = await invoke<{
      ok: boolean
      sent: boolean
      mode: string
      message: string
    }>('open_new_codex_chat_with_text', {
      content: 'zhi',
      projectPath,
    })

    if (!result.sent) {
      message.warning(result.message || '已打开 Codex，但本次未自动发送')
    }
  }
  catch (error) {
    console.error('打开 Codex 失败:', error)
    const details = typeof error === 'string' ? error : String(error)
    message.error(details || '打开 Codex 失败')
  }
}

async function handleToggleCodexLive() {
  if (desktopCodexLiveToggleInFlight)
    return
  desktopCodexLiveToggleInFlight = true
  try {
    const projectPath = reliableRequestProjectPath.value
      || globalCodexLiveProjectPath.value
      || (isDisplayableProjectPath(lastValidProjectPath.value) ? lastValidProjectPath.value.trim() : null)
    const result = await toggleDesktopCodexLive(projectPath)
    applyDesktopCodexLiveSnapshot(result.snapshot)
    if (result.action === 'stop') {
      message.success('已请求结束全局 GPT-Live；Fn 普通听写将自动恢复')
    }
  }
  catch (error) {
    const details = error instanceof Error ? error.message : String(error)
    if (details === 'desktop_codex_live_bridge_400') {
      message.warning('请先打开一个带项目路径的 iterate 任务')
      return
    }
    message.error(details || '无法启动 GPT-Live')
  }
  finally {
    desktopCodexLiveToggleInFlight = false
  }
}

async function handleToggleCodexLiveMute() {
  if (!['preparing', 'connecting', 'active', 'reconnecting'].includes(globalCodexLivePhase.value))
    return
  if (desktopCodexLiveMuteInFlight)
    return
  desktopCodexLiveMuteInFlight = true
  try {
    applyDesktopCodexLiveSnapshot(await toggleDesktopCodexLiveMicrophone())
  }
  catch (error) {
    const details = error instanceof Error ? error.message : String(error)
    message.error(details || '无法切换 GPT-Live 静音状态')
  }
  finally {
    desktopCodexLiveMuteInFlight = false
  }
}

// 注册/注销快捷键的状态
const isShortcutsRegistered = ref(false)

// 注册全局快捷键
async function registerShortcuts() {
  if (isShortcutsRegistered.value)
    return

  try {
    const enabled = await invoke('get_global_shortcut_enabled')
    if (!enabled) {
      console.log('快捷键已全局禁用，跳过注册')
      return
    }

    await register('Shift+Tab', async () => {
      await restoreWindow()
    })
    isShortcutsRegistered.value = true
    console.log('全局快捷键已注册')
  }
  catch (error) {
    console.error('注册全局快捷键失败:', error)
  }
}

// 注销全局快捷键
async function unregisterShortcuts() {
  if (!isShortcutsRegistered.value)
    return

  try {
    await unregister('Shift+Tab')
    isShortcutsRegistered.value = false
    console.log('全局快捷键已注销')
  }
  catch (error) {
    console.error('注销全局快捷键失败:', error)
  }
}

// 监听 MCP 弹窗显示状态变化，动态注册/注销快捷键
watch(() => props.showMcpPopup, async (newValue) => {
  if (newValue) {
    await registerShortcuts()
    void refreshUsageQuotaProviders()
  }
  else {
    showTimelineDrawer.value = false
    await unregisterShortcuts()
  }
})

onMounted(async () => {
  // 将消息实例传递给父组件
  emit('messageReady', message)
  await showPostUpdateAutomationNotice()
  // 设置退出警告监听器（统一处理主界面和弹窗）
  setupExitWarningListener(message)
  await initializeDesktopCodexLiveControl()

  // 添加全局键盘事件监听器
  document.addEventListener('keydown', handleGlobalKeydown)
  document.addEventListener('keyup', handleGlobalKeyup)

  // 只在 MCP 弹窗显示时才注册快捷键（避免后台运行时干扰其他应用）
  if (props.showMcpPopup) {
    await registerShortcuts()
    void refreshUsageQuotaProviders()
  }

  quotaRefreshTimer = window.setInterval(() => {
    if (props.showMcpPopup)
      void refreshUsageQuotaProviders()
  }, 5 * 60 * 1000)

  let customPromptConfigCache: any = null
  let customPromptConfigPromise: Promise<any> | null = null

  async function getCustomPromptConfig(forceRefresh = false) {
    if (!forceRefresh && customPromptConfigCache) {
      return customPromptConfigCache
    }

    if (!forceRefresh && customPromptConfigPromise) {
      return customPromptConfigPromise
    }

    customPromptConfigPromise = invoke('get_custom_prompt_config')
      .then((config) => {
        customPromptConfigCache = config
        return config
      })
      .finally(() => {
        customPromptConfigPromise = null
      })

    return customPromptConfigPromise
  }

  const unlistenCustomPromptConfigChanged = await listen('custom-prompt-config-changed', async () => {
    customPromptConfigCache = null
    try {
      customPromptConfigCache = await getCustomPromptConfig(true)
    }
    catch (e) {
      console.error('[Bridge] 配置变更后刷新自定义模板失败:', e)
    }
  })

  function reportBridgeTiming(location: string, payload: Record<string, any>) {
    console.info(`[Bridge Timing] ${location}`, payload)
    void invoke('timeline_debug_log', {
      location: `frontend/bridge_timing:${location}`,
      payload,
    }).catch((error) => {
      console.warn('[Bridge Timing] 写入持久化日志失败:', error)
    })
  }

  // 监听来自 Web Bridge 的消息
  const unlistenBridge = await listen('bridge-message', async (event: any) => {
    const bridgeMsg = event.payload as { message_type: string, payload: any }
    console.log('[Bridge] 收到 Web 端消息:', bridgeMsg)
    const { message_type, payload } = bridgeMsg

    if (message_type === 'request_sync') {
      const currentRequestId = normalizeRequestId(props.mcpRequest)
      const targetRequestId = normalizeRequestId(payload)
      if (currentRequestId && targetRequestId && currentRequestId !== targetRequestId) {
        console.log('[Bridge] request_sync: request_id 不匹配，忽略本窗口响应', {
          targetRequestId,
          currentRequestId,
        })
        return
      }

      const targetProjectPath = normalizeProjectPath(payload)
      if (targetProjectPath) {
        const currentProjectPath = normalizeProjectPath(props.mcpRequest)
        if (currentProjectPath !== targetProjectPath) {
          console.log('[Bridge] request_sync: project_path 不匹配，忽略本窗口响应', {
            targetProjectPath,
            currentProjectPath,
          })
          return
        }
      }

      try {
        customPromptConfigCache = await getCustomPromptConfig()
      }
      catch (e) {
        console.error('[Bridge] 获取自定义模板失败:', e)
      }

      console.log('[Bridge] 正在响应同步请求, 当前状态:', {
        hasRequest: !!props.mcpRequest,
        showPopup: props.showMcpPopup,
      })

      const processedRequest = preprocessMarkdownImages(props.mcpRequest)
      const syncResponseRequestId = normalizeRequestId(processedRequest)
      const syncResponseProjectPath = normalizeProjectPath(processedRequest)
      reportBridgeTiming('respond_request_sync', {
        targetRequestId,
        currentRequestId,
        syncResponseRequestId,
        syncResponseProjectPath,
        showPopup: props.showMcpPopup,
      })
      const liveGoal = await getLiveGoalForBridge()
      invoke('send_to_web_bridge', {
        message: {
          message_type: 'mcp_state',
          payload: {
            sync_response: true,
            suppress_remote_notification: true,
            request: processedRequest,
            showMcpPopup: props.showMcpPopup,
            customPrompts: applyPromptContextState(customPromptConfigCache),
            live_goal: liveGoal,
          },
        },
      })
    }
    else if (message_type === 'mcp_action') {
      // 转发 Web 端的动作到本地处理逻辑
      if (handleWindowConditionalAction(payload))
        return
      emit('bridgeAction', payload)
    }
  })

  unlistenConversationNodeRecorded = await listen('conversation-node-recorded', async (event: any) => {
    const payload = event.payload as {
      tree_id?: string
      node_id?: string
      node_type?: string
      conversation_id?: string
      conversationId?: string
      request_key?: string
      requestKey?: string
      request_id?: string
      requestId?: string
      project_path?: string
      projectPath?: string
      source?: string
    }
    console.info('[Timeline] 收到 conversation-node-recorded 事件', payload)
    reportTimelineDebugLog('收到 conversation-node-recorded 事件', payload)

    if (!conversationTreeId.value || !payload?.tree_id || !payload?.node_id)
      return

    if (payload.tree_id !== conversationTreeId.value)
      return

    const payloadConversationId = normalizeTimelineMetadataValue(payload.conversation_id ?? payload.conversationId)
    if (payloadConversationId && payloadConversationId !== conversationTreeId.value) {
      reportTimelineDebugLog('忽略 conversation_id 不匹配的节点事件', {
        payloadConversationId,
        activeConversationId: conversationTreeId.value,
        payload,
      })
      return
    }

    const payloadRouteKey = normalizeTimelineMetadataValue(
      payload.request_key ?? payload.requestKey ?? payload.request_id ?? payload.requestId,
    )
    if (payloadRouteKey && activeConversationRouteKey.value && payloadRouteKey !== activeConversationRouteKey.value) {
      reportTimelineDebugLog('忽略 request route 不匹配的节点事件', {
        payloadRouteKey,
        activeConversationRouteKey: activeConversationRouteKey.value,
        payload,
      })
      return
    }

    const payloadProjectPath = normalizeTimelineMetadataValue(payload.project_path ?? payload.projectPath)
    const activeProjectPath = normalizeProjectPath(props.mcpRequest)
    if (payloadProjectPath && activeProjectPath && payloadProjectPath !== activeProjectPath) {
      reportTimelineDebugLog('忽略 project_path 不匹配的节点事件', {
        payloadProjectPath,
        activeProjectPath,
        payload,
      })
      return
    }

    currentConversationNodeId.value = payload.node_id
    await refreshCurrentConversationNode('conversation-node-recorded')
  })

  // 预处理 markdown 中的本地图片路径，转为 HTTP URL（让手机端通过 HTTP 下载而非 WebSocket 传输）
  function preprocessMarkdownImages(request: any): any {
    if (!request?.message)
      return request
    const msg = request.message as string
    // 匹配 ![alt](/path/to/image.ext) 格式
    const processed = msg.replace(
      /!\[([^\]]*)\]\((\/[^)]+\.(png|jpg|jpeg|gif|webp|svg))\)/g,
      (_match: string, alt: string, path: string) => {
        // 用 Bridge HTTP 端点 URL 替代本地路径，手机端按需下载
        return `![${alt}](/image?path=${encodeURIComponent(path)})`
      },
    )
    if (processed === msg)
      return request
    return { ...request, message: processed }
  }

  // 使用 debounce 合并 mcpRequest 和 showMcpPopup 变化，避免重复推送
  let bridgePushTimeout: ReturnType<typeof setTimeout> | null = null
  let bridgePushSequence = 0
  function debouncedBridgePush() {
    const scheduledAt = Date.now()
    const sequence = ++bridgePushSequence
    const requestId = normalizeRequestId(props.mcpRequest)
    const timelineRouteId = activeConversationRouteKey.value
    const projectPath = normalizeProjectPath(props.mcpRequest)
    reportBridgeTiming('schedule_mcp_state_push', {
      sequence,
      requestId,
      timelineRouteId,
      projectPath,
      showPopup: props.showMcpPopup,
      scheduledAt,
      debounceMs: 200,
    })
    if (bridgePushTimeout) {
      clearTimeout(bridgePushTimeout)
    }
    bridgePushTimeout = setTimeout(async () => {
      const firedAt = Date.now()
      reportBridgeTiming('fire_mcp_state_push', {
        sequence,
        requestId,
        timelineRouteId,
        projectPath,
        showPopup: props.showMcpPopup,
        debounceElapsedMs: firedAt - scheduledAt,
      })
      const processedRequest = preprocessMarkdownImages(props.mcpRequest)

      // 从主 Tauri app 的 ConversationManager 获取时间线节点，注入到 payload
      // bridge server 是独立进程，有自己的 ConversationManager，看不到前端创建的节点
      let timelineNodes: any[] = []
      if (conversationTreeId.value && currentConversationNodeId.value) {
        try {
          const nodes = await invoke<any[]>('get_conversation_path', {
            treeId: conversationTreeId.value,
            nodeId: currentConversationNodeId.value,
          })
          // strip heavy metadata (images etc.) to avoid WS 1009 Message Too Big
          timelineNodes = filterTimelineNodesForActiveRoute(
            nodes || [],
            processedRequest,
            conversationTreeId.value,
            activeConversationRouteKey.value,
          ).map((node: any) => {
            const stripped = { ...node }
            if (stripped.metadata) {
              stripped.metadata = { ...stripped.metadata }
              delete stripped.metadata.images
            }
            return stripped
          })
        }
        catch (err) {
          console.warn('[Bridge] 获取时间线节点失败:', err)
        }
      }

      const liveGoal = await getLiveGoalForBridge()
      const invokeAt = Date.now()
      reportBridgeTiming('invoke_send_to_web_bridge', {
        sequence,
        requestId,
        timelineRouteId,
        projectPath,
        timelineNodeCount: timelineNodes.length,
        elapsedSinceScheduleMs: invokeAt - scheduledAt,
      })
      invoke('send_to_web_bridge', {
        message: {
          message_type: 'mcp_state',
          payload: {
            request: processedRequest,
            showMcpPopup: props.showMcpPopup,
            customPrompts: applyPromptContextState(customPromptConfigCache),
            conversation_id: conversationTreeId.value,
            timeline_route_id: activeConversationRouteKey.value,
            timelineNodes,
            quotaProviders: quotaProviders.value,
            quotaStatusLabel: quotaStatusLabel.value,
            live_goal: liveGoal,
          },
        },
      })
    }, 200) // 200ms debounce，等待 syncConversationRequestNode 完成后再推送
  }
  triggerBridgePush = debouncedBridgePush

  // 监听 mcpRequest 变化
  watch(() => props.mcpRequest, () => {
    console.log('[Bridge] mcpRequest 发生变化')
    debouncedBridgePush()
  }, { deep: true })

  // 监听显示状态变化
  watch(() => props.showMcpPopup, () => {
    console.log('[Bridge] showMcpPopup 发生变化')
    debouncedBridgePush()
  })

  // 监听时间线状态变化，确保 syncConversationRequestNode 完成后重新推送（带 timelineNodes）
  watch([conversationTreeId, currentConversationNodeId], () => {
    if (conversationTreeId.value && currentConversationNodeId.value) {
      if (skipNextBridgePush) {
        skipNextBridgePush = false
        console.log('[Bridge] 跳过时间线切换触发的推送')
        return
      }
      console.log('[Bridge] 时间线状态变化，重新推送')
      debouncedBridgePush()
    }
  })

  watch([quotaProviders, quotaStatusLabel], () => {
    debouncedBridgePush()
  }, { deep: true })

  // 存储注销函数
  onUnmounted(() => {
    triggerBridgePush = null
    if (unlistenBridge) {
      unlistenBridge()
    }
    if (unlistenCustomPromptConfigChanged) {
      unlistenCustomPromptConfigChanged()
    }
    if (unlistenConversationNodeRecorded) {
      unlistenConversationNodeRecorded()
      unlistenConversationNodeRecorded = null
    }
  })

  // 注册当前窗口实例
  try {
    await syncWindowRegistration(effectiveProjectPath.value, props.mcpRequest)
  }
  catch (error) {
    console.error('注册窗口实例失败:', error)
  }

  // 窗口恢复时立即补偿拉取缓存的 action（防止最小化期间 throttle 导致延迟）
  visibilityHandler = () => {
    if (document.visibilityState === 'visible') {
      console.log('[Bridge] 窗口恢复可见，立即拉取缓存 action')
      void pullCachedBridgeAction('visibility')
    }
  }
  document.addEventListener('visibilitychange', visibilityHandler)
  void pullCachedBridgeAction('mounted')

  // 多进程弹窗模式下，手机端的 mcp_action 会先到主进程 8080，再由弹窗进程主动拉取。
  // 这里按 project_path 轮询主进程，拿到 action 后触发本进程的 bridgeAction。
  actionPollTimer = window.setInterval(() => {
    void pullCachedBridgeAction('poll')
  }, ACTION_POLL_INTERVAL_MS)
})

onUnmounted(async () => {
  if (desktopCodexLivePollTimer !== null) {
    window.clearInterval(desktopCodexLivePollTimer)
    desktopCodexLivePollTimer = null
  }

  if (quotaRefreshTimer !== null) {
    window.clearInterval(quotaRefreshTimer)
    quotaRefreshTimer = null
  }

  if (actionPollTimer !== null) {
    window.clearInterval(actionPollTimer)
    actionPollTimer = null
  }

  // 移除事件监听器
  if (visibilityHandler)
    document.removeEventListener('visibilitychange', visibilityHandler)
  document.removeEventListener('keydown', handleGlobalKeydown)
  document.removeEventListener('keyup', handleGlobalKeyup)

  // 注销全局快捷键
  await unregisterShortcuts()

  if (unlistenConversationNodeRecorded) {
    unlistenConversationNodeRecorded()
    unlistenConversationNodeRecorded = null
  }

  // 注销当前窗口实例
  try {
    await invoke('unregister_window_instance')
  }
  catch (error) {
    console.error('注销窗口实例失败:', error)
  }
})
</script>

<template>
  <div class="min-h-screen bg-black">
    <!-- MCP弹窗模式 -->
    <div
      v-if="props.showMcpPopup && props.mcpRequest"
      class="flex flex-col w-full h-screen bg-black text-white select-none"
    >
      <!-- 头部 - 固定在顶部 -->
      <div class="sticky top-0 z-50 flex-shrink-0 bg-black-100 border-b-2 border-black-200">
        <PopupHeader
          :current-theme="props.appConfig.theme"
          :loading="false"
          :show-main-layout="showPopupSettings"
          :always-on-top="props.appConfig.window.alwaysOnTop"
          :is-muted="props.isMuted"
          :shortcut-enabled="localShortcutEnabled"
          :project-path="effectiveProjectPath"
          :codex-thread-id="props.mcpRequest?.codex_thread_id"
          :conversation-title="props.mcpRequest?.conversation_title"
          :link-url="props.mcpRequest?.link_url"
          :link-title="props.mcpRequest?.link_title"
          :quota-providers="quotaProviders"
          :quota-status-label="quotaStatusLabel"
          :codex-live-phase="globalCodexLivePhase"
          :codex-live-status="globalCodexLiveStatus"
          @theme-change="$emit('themeChange', $event)"
          @open-main-layout="togglePopupSettings"
          @toggle-always-on-top="$emit('toggleAlwaysOnTop')"
          @toggle-mute="$emit('toggleMute')"
          @new-chat="handleNewChat"
          @open-iterate-pairing="showIteratePairingModal = true"
          @toggle-codex-live="handleToggleCodexLive"
          @toggle-codex-live-mute="handleToggleCodexLiveMute"
          @toggle-shortcut="handleToggleShortcut"
          @minimize-window="minimizeWindow"
          @close-current-dialog="$emit('mcpCloseCurrentDialog')"
        />
      </div>

      <MobileConnectionWizard v-model:show="showIteratePairingModal" />

      <n-drawer
        v-model:show="showTimelineDrawer"
        placement="left"
        :width="360"
      >
        <n-drawer-content
          title="对话时间线"
          closable
          body-content-class="p-0 bg-black"
        >
          <TimelineView
            v-if="conversationTreeId && currentConversationNodeId"
            :tree-id="conversationTreeId"
            :current-node-id="currentConversationNodeId"
            @node-click="handleTimelineNodeSwitch"
          />
          <div v-else class="h-full flex items-center justify-center bg-black">
            <n-empty description="暂无对话节点" />
          </div>
        </n-drawer-content>
      </n-drawer>

      <!-- 设置界面 -->
      <div
        v-if="showPopupSettings"
        class="flex-1 overflow-y-auto scrollbar-thin"
      >
        <LayoutWrapper
          :app-config="props.appConfig"
          :codex-live-phase="globalCodexLivePhase"
          :codex-live-status="globalCodexLiveStatus"
          @theme-change="$emit('themeChange', $event)"
          @toggle-always-on-top="$emit('toggleAlwaysOnTop')"
          @toggle-audio-notification="$emit('toggleAudioNotification')"
          @update-audio-url="$emit('updateAudioUrl', $event)"
          @test-audio="$emit('testAudio')"
          @stop-audio="$emit('stopAudio')"
          @test-audio-error="$emit('testAudioError', $event)"
          @update-window-size="$emit('updateWindowSize', $event)"
          @toggle-codex-live="handleToggleCodexLive"
          @toggle-codex-live-mute="handleToggleCodexLiveMute"
        />
      </div>

      <!-- 弹窗内容 -->
      <div v-else class="flex-1 flex flex-col overflow-hidden min-h-0">
        <McpPopup
          ref="mcpPopupRef"
          :request="props.mcpRequest"
          :context-key="popupContextKey"
          :context-prompt-state="currentPromptContextState"
          :app-config="props.appConfig"
          :is-muted="props.isMuted"
          :timeline-tree-id="conversationTreeId"
          :timeline-current-node-id="currentConversationNodeId"
          class="flex-1 flex flex-col overflow-hidden min-h-0"
          @response="$emit('mcpResponse', $event)"
          @cancel="$emit('mcpCancel')"
          @theme-change="$emit('themeChange', $event)"
          @toggle-mute="$emit('toggleMute')"
          @open-artifact="handleOpenArtifact"
          @timeline-node-click="handleTimelineNodeSwitch"
          @conditional-state-change="handlePopupConditionalStateChange"
        />
      </div>

      <div
        v-show="activeArtifact"
        class="fixed inset-0 z-[20000] flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
        @click.self="handleCloseArtifact"
      >
        <section
          class="flex h-[min(860px,calc(100vh-32px))] w-[min(1120px,calc(100vw-32px))] flex-col overflow-hidden rounded-lg border shadow-2xl"
          :class="props.appConfig.theme === 'light' ? 'border-gray-200 bg-white text-gray-900' : 'border-white/10 bg-[#0d0e11] text-white'"
        >
          <header
            class="flex min-h-12 items-center justify-between gap-3 border-b px-4"
            :class="props.appConfig.theme === 'light' ? 'border-gray-200' : 'border-white/10'"
          >
            <div class="flex min-w-0 items-center gap-2.5">
              <div
                class="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-md"
                :class="props.appConfig.theme === 'light' ? 'bg-gray-100 text-gray-700' : 'bg-white/10 text-gray-200'"
              >
                <div class="i-carbon-html w-4 h-4" />
              </div>
              <div class="min-w-0">
                <div class="truncate text-sm font-semibold">
                  {{ activeArtifact?.title || 'HTML Artifact' }}
                </div>
                <div
                  v-if="activeArtifact?.description || activeArtifact?.path"
                  class="truncate text-xs"
                  :class="props.appConfig.theme === 'light' ? 'text-gray-500' : 'text-gray-400'"
                >
                  {{ activeArtifact?.description || activeArtifact?.path }}
                </div>
              </div>
            </div>

            <div class="flex flex-shrink-0 items-center gap-2">
              <button
                v-if="activeArtifactContent"
                type="button"
                class="inline-flex h-8 items-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition-colors"
                :class="props.appConfig.theme === 'light'
                  ? 'border-gray-200 bg-white text-gray-700 hover:bg-gray-50'
                  : 'border-white/10 bg-white/5 text-gray-200 hover:bg-white/10'"
                @click="handleCopyArtifact"
              >
                <div class="i-carbon-copy w-3.5 h-3.5" />
                <span>复制 HTML</span>
              </button>
              <button
                v-if="activeArtifactContent"
                type="button"
                class="inline-flex h-8 items-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition-colors"
                :class="props.appConfig.theme === 'light'
                  ? 'border-gray-200 bg-white text-gray-700 hover:bg-gray-50'
                  : 'border-white/10 bg-white/5 text-gray-200 hover:bg-white/10'"
                @click="handleOpenArtifactInBrowser"
              >
                <div class="i-carbon-launch w-3.5 h-3.5" />
                <span>浏览器打开</span>
              </button>
              <button
                type="button"
                class="inline-flex h-8 w-8 items-center justify-center rounded-md border transition-colors"
                :class="props.appConfig.theme === 'light'
                  ? 'border-gray-200 bg-white text-gray-700 hover:bg-gray-50'
                  : 'border-white/10 bg-white/5 text-gray-200 hover:bg-white/10'"
                title="关闭 HTML 预览"
                @click="handleCloseArtifact"
              >
                <div class="i-carbon-close w-4 h-4" />
              </button>
            </div>
          </header>

          <div class="min-h-0 flex-1 overflow-y-auto">
            <HtmlArtifactRenderer
              v-if="activeArtifactContent"
              :content="activeArtifactContent"
              :current-theme="props.appConfig.theme"
              :title="activeArtifact?.title || 'HTML Artifact'"
              :show-chrome="false"
            />
            <div
              v-else
              class="flex h-full items-center justify-center px-6 text-sm"
              :class="props.appConfig.theme === 'light' ? 'text-gray-500' : 'text-gray-400'"
            >
              暂无可预览的 HTML 内容
            </div>
          </div>
        </section>
      </div>
    </div>

    <!-- 弹窗加载骨架屏 或 初始化骨架屏 -->
    <div
      v-else-if="props.showMcpPopup || props.isInitializing"
      class="flex flex-col w-full h-screen bg-black text-white"
    >
      <!-- 头部骨架 -->
      <div class="flex-shrink-0 bg-black-100 border-b-2 border-black-200 px-4 py-3">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <n-skeleton
              circle
              :width="12"
              :height="12"
            />
            <n-skeleton
              text
              :width="256"
            />
          </div>
          <div class="flex gap-2">
            <n-skeleton
              circle
              :width="32"
              :height="32"
            />
            <n-skeleton
              circle
              :width="32"
              :height="32"
            />
          </div>
        </div>
      </div>

      <!-- 内容骨架 -->
      <div class="flex-1 p-4">
        <div class="bg-black-100 rounded-lg p-4 mb-4">
          <n-skeleton
            text
            :repeat="3"
          />
        </div>

        <div class="space-y-3">
          <n-skeleton
            text
            :width="128"
          />
          <n-skeleton
            text
            :repeat="3"
          />
        </div>
      </div>

      <!-- 底部骨架 -->
      <div class="flex-shrink-0 bg-black-100 border-t-2 border-black-200 p-4">
        <div class="flex justify-between items-center">
          <n-skeleton
            text
            :width="96"
          />
          <div class="flex gap-2">
            <n-skeleton
              text
              :width="64"
              :height="32"
            />
            <n-skeleton
              text
              :width="64"
              :height="32"
            />
          </div>
        </div>
      </div>
    </div>

    <!-- 主界面 - 只在非弹窗模式且非初始化时显示 -->
    <LayoutWrapper
      v-else
      :app-config="props.appConfig"
      :codex-live-phase="globalCodexLivePhase"
      :codex-live-status="globalCodexLiveStatus"
      @theme-change="$emit('themeChange', $event)"
      @toggle-always-on-top="$emit('toggleAlwaysOnTop')"
      @toggle-audio-notification="$emit('toggleAudioNotification')"
      @update-audio-url="$emit('updateAudioUrl', $event)"
      @test-audio="$emit('testAudio')"
      @stop-audio="$emit('stopAudio')"
      @test-audio-error="$emit('testAudioError', $event)"
      @update-window-size="$emit('updateWindowSize', $event)"
      @toggle-codex-live="handleToggleCodexLive"
      @toggle-codex-live-mute="handleToggleCodexLiveMute"
      @config-reloaded="$emit('configReloaded')"
    />

    <!-- 更新弹窗 -->
    <UpdateModal
      v-model:show="showUpdateModal"
      :version-info="versionInfo"
    />
  </div>
</template>
