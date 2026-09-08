<script setup lang="ts">
import type { McpRequest, PopupArtifact, PopupFileAttachment, PopupInputData, PopupTextSelection, ShortcutBinding } from '../../types/popup'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useMessage } from 'naive-ui'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'

import { useShortcuts } from '../../composables/useShortcuts'
import { copySubmissionToClipboard } from '../../utils/submissionClipboard'
import { stripAutoPrompt } from '../../utils/textUtils'
import TimelineDotBar from '../conversation/TimelineDotBar.vue'
import PopupActions from './PopupActions.vue'
import PopupContent from './PopupContent.vue'
import PopupInput from './PopupInput.vue'

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

interface ConversationNode {
  id: string
  parent_id: string | null
  timestamp: string
  node_type: 'user' | 'assistant'
  content: string
  is_markdown: boolean
  metadata?: {
    request_id?: string | null
    checkpoint_id?: string | null
  }
}

type PromptContextState = Record<string, { current_state?: boolean, is_active?: boolean }>

interface Props {
  request: McpRequest | null
  appConfig: AppConfig
  mockMode?: boolean
  testMode?: boolean
  isMuted?: boolean
  contextKey?: string
  contextPromptState?: PromptContextState
  timelineTreeId?: string | null
  timelineCurrentNodeId?: string | null
  timelineMockNodes?: ConversationNode[]
}

interface Emits {
  response: [response: any]
  cancel: []
  themeChange: [theme: string]
  openMainLayout: []
  toggleAlwaysOnTop: []
  toggleMute: []
  toggleAudioNotification: []
  updateAudioUrl: [url: string]
  testAudio: []
  stopAudio: []
  testAudioError: [error: any]
  updateWindowSize: [size: { width: number, height: number, fixed: boolean }]
  timelineNodeClick: [nodeId: string]
  conditionalStateChange: [payload: { promptId: string, current_state?: boolean, is_active?: boolean }]
  openArtifact: [artifact: PopupArtifact]
}

interface TimelinePrefillPayload extends PopupInputData {
  userInput: string
  focus?: boolean
}

interface PopupInputRef {
  statusText?: string
  updateData: (data: PopupInputData) => void
  recordSubmittedInputForAutoPromotion: () => void
  handleQuoteMessage: (messageContent: string) => void
  insertSelectedTextQuote: (selection: PopupTextSelection) => Promise<boolean>
  focusInput?: () => void
}

interface PopupContentRef {
  openFileMenu?: () => void
  resolveCurrentTextSelection?: () => PopupTextSelection | null
}

const props = withDefaults(defineProps<Props>(), {
  mockMode: false,
  testMode: false,
})

const emit = defineEmits<Emits>()

// 使用消息提示
const message = useMessage()
const { loadShortcutConfig, getShortcutByAction } = useShortcuts()

// 响应式状态
const loading = ref(false)
const submitting = ref(false)
const selectedOptions = ref<string[]>([])
const userInput = ref('')
const rawUserInput = ref('')
const draggedImages = ref<string[]>([])
const attachedFiles = ref<PopupFileAttachment[]>([])
const contentRef = ref<PopupContentRef | null>(null)
const inputRef = ref<PopupInputRef | null>(null)
const pendingTimelinePrefill = ref<TimelinePrefillPayload | null>(null)
const timelineEdgeActive = ref(false)
const TIMELINE_EDGE_TRIGGER_PX = 16
const TIMELINE_EDGE_ACTIVE_PX = 42

// 处理输入框中输入 "@" 或 "爱特" 自动弹出文件菜单
function handleAtTrigger() {
  console.log('[McpPopup] 检测到 @ 触发符，尝试打开文件菜单')
  if (contentRef.value?.openFileMenu) {
    contentRef.value.openFileMenu()
  }
}

// 继续回复配置
const continueReplyEnabled = ref(true)
const copySubmissionToClipboardEnabled = ref(false)
const continuePrompt = ref('请按照最佳实践继续')
const DEFAULT_LOOP_PROMPT = `进入 GoalRun 目标模式。

## 执行规则
1. 先把用户的一句话整理成可执行目标；只有目标依赖历史、上下文被压缩，或用户明确继续上次/之前/hui1 时才恢复 hui1；需要恢复时优先当前 timeline/thread/run。
2. 围绕目标自己选择合适的 Skill 和工具，持续执行、修复、验证；能合理推进就不要反问。
3. 失败就继续定位和修复，直到验证通过、确实阻塞，或碰到目标外的高风险边界。
4. 完成后再交给用户验收：说明做了什么、验证了什么、还有什么风险。
5. 只有明显越界、破坏性操作、凭据/登录、Computer Use、提交/推送/发布，或发现需要沉淀的新问题时，才通过 zhi 询问。`
const loopPrompt = ref(DEFAULT_LOOP_PROMPT)
const DEFAULT_GOAL_PROMPT_TEMPLATE = `1. 先把这句话整理成可执行目标；在执行任何实现动作前，必须用 Codex 的 get_goal 检查本线程正式 Goal，并完成同步：无正式 Goal 时立即 create_goal；现有 Goal 与本目标相同则继续；现有未完成 Goal 不同则先核对真实状态，只有已有证据证明它确实完成时才 update_goal 为 complete 后创建本目标，否则停止执行并通过 zhi 报告冲突，绝不能伪造完成或在未同步状态下继续。
2. Codex 正式 Goal 是唯一状态源，iterate Live Goal 只负责展示；create_goal 成功后再开始实现，并在真正完成且验证通过后按 Goal 工具规则更新状态。
3. 围绕目标自己选择合适的 Skill 和工具，持续执行、修复、验证；能合理推进就不要反问。
4. 失败就继续定位和修复，直到验证通过、确实阻塞，或碰到目标外的高风险边界。
5. 完成后再交给用户验收：说明做了什么、验证了什么、还有什么风险。
6. 只有明显越界、破坏性操作、凭据/登录、Computer Use、提交/推送/发布，或发现需要沉淀的新问题时，才通过 zhi 询问。
7. 这是目标提交，不是迭代循环；不要生成 [迭代 x/10] 这类轮次提示。
8. 如果任务完成，明确写“已完成”；如果阻塞，说明原因、证据和可选下一步。`
const goalPromptTemplate = ref(DEFAULT_GOAL_PROMPT_TEMPLATE)

// 浏览器 AI 回复内容
const browserAiResponse = ref<string | null>(null)
let unlistenBrowserAi: (() => void) | null = null
const currentWindowLabel = getCurrentWindow().label || 'current-window'

type LiveGoalIntent = { action: 'start', title: string } | { action: 'complete' } | { action: 'clear' }

interface LiveGoalRunMetadata {
  run_id: string | null
  generation: number | null
  stale_of: string | null
  superseded_by: string | null
  is_stale: boolean
}

function parseLiveGoalIntentValue(value: string): LiveGoalIntent | null {
  const trimmed = value.trim()
  if (!trimmed)
    return null

  let payload: string | null = null
  const lowerTrimmed = trimmed.toLowerCase()
  const commandText = lowerTrimmed.startsWith('@/goal') || lowerTrimmed.startsWith('＠/goal')
    ? trimmed.slice(1).trim()
    : trimmed
  const lowerCommandText = commandText.toLowerCase()
  if (
    lowerCommandText === '/goal'
    || lowerCommandText.startsWith('/goal ')
    || lowerCommandText.startsWith('/goal:')
    || lowerCommandText.startsWith('/goal：')
  ) {
    const suffix = commandText.slice('/goal'.length).trim()
    payload = suffix.startsWith(':') || suffix.startsWith('：')
      ? suffix.slice(1).trim()
      : suffix
  }
  else {
    if (lowerCommandText.startsWith('goal:') || lowerCommandText.startsWith('goal：'))
      payload = commandText.slice('goal:'.length).trim()
  }

  if (payload === null)
    return null

  const command = payload.toLowerCase()
  if (['done', 'complete', 'finish', '完成', '已完成'].includes(command))
    return { action: 'complete' }

  if (['clear', 'cancel', 'stop', 'reset', '清除', '取消', '停止'].includes(command))
    return { action: 'clear' }

  const title = stripLiveGoalStartKeyword(payload)
  if (!title)
    return null

  return { action: 'start', title }
}

function stripLiveGoalStartKeyword(payload: string): string {
  const trimmed = payload.trim()
  const lowerTrimmed = trimmed.toLowerCase()

  for (const keyword of ['start', '开始', '启动']) {
    const prefix = `${keyword} `
    if (lowerTrimmed.startsWith(prefix))
      return trimmed.slice(prefix.length).trim()
  }

  return trimmed
}

function resolveLiveGoalIntent(userInput: string, options: string[] = []): LiveGoalIntent | null {
  for (const value of [userInput, ...options]) {
    const intent = parseLiveGoalIntentValue(value)
    if (intent)
      return intent
  }

  return null
}

function resolveSubmitSource(userInput: string, options: string[] = []): 'popup' | 'popup_goal_submit' {
  const intent = resolveLiveGoalIntent(userInput, options)
  return intent?.action === 'start' ? 'popup_goal_submit' : 'popup'
}

async function applyLiveGoalIntent(userInput: string, options: string[] = []): Promise<any | null> {
  const intent = resolveLiveGoalIntent(userInput, options)
  if (!intent)
    return null

  try {
    if (intent.action === 'start') {
      const goal = await invoke('start_live_goal', {
        title: intent.title,
        projectPath: props.request?.project_path,
        requestId: props.request?.id,
        codexThreadId: props.request?.codex_thread_id,
        codexDeeplink: props.request?.codex_deeplink,
      })
      message.success('目标已登记到 Live Goal，正在提交给 Codex')
      return goal
    }
    else if (intent.action === 'complete') {
      const goal = await invoke('complete_live_goal')
      message.success('目标已标记完成')
      return goal
    }
    else {
      await invoke('clear_live_goal')
      message.success('目标已从菜单栏清除')
      return null
    }
  }
  catch (error) {
    console.error('同步 Live Goal 失败:', error)
    message.warning(`目标同步失败: ${String(error)}`)
    return null
  }
}

function emptyLiveGoalRunMetadata(runId: string | null = null, generation: number | null = null): LiveGoalRunMetadata {
  return {
    run_id: runId,
    generation,
    stale_of: null,
    superseded_by: null,
    is_stale: false,
  }
}

const popupContextKey = computed(() => {
  if (props.contextKey)
    return props.contextKey

  const projectPath = props.request?.project_path?.trim()
  if (projectPath)
    return `${currentWindowLabel}:${projectPath}`

  return `${currentWindowLabel}:${props.request?.id || 'unknown-request'}`
})

function buildFileRefText(files: PopupFileAttachment[], separator = ' '): string {
  return files.map(file => `@${file.path}`).join(separator)
}

function buildImageAttachments(images: string[]) {
  return images.map(imageData => ({
    data: imageData.includes(',') ? imageData.split(',')[1] : imageData,
    media_type: 'image/png',
    filename: null,
  }))
}

function buildFilePaths(files: PopupFileAttachment[]): string[] {
  return files
    .map(file => file.path.trim())
    .filter(Boolean)
}

function buildFinalUserInput(rawInput: string, files: PopupFileAttachment[]): string | null {
  const fileRefs = buildFileRefText(files).trim()
  const trimmedInput = rawInput.trim()
  const combined = [fileRefs, trimmedInput]
    .filter(Boolean)
    .join('\n\n')
    .trim()

  return combined || null
}

function buildSelectedOptionsContext(goalText: string, options: string[]): string {
  const missingOptions = options
    .map(option => option.trim())
    .filter(option => option && !goalText.includes(option))

  if (missingOptions.length === 0)
    return ''

  return `选中的选项：\n${missingOptions.map(option => `- ${option}`).join('\n')}`
}

function buildGoalTargetText(
  rawInput: string,
  options: string[],
  files: PopupFileAttachment[],
  imageCount: number,
): string | null {
  const trimmedInput = rawInput.trim()
  const normalizedOptions = options
    .map(option => option.trim())
    .filter(Boolean)
  const selectedOptionsText = normalizedOptions
    .join('\n')
  const primaryText = trimmedInput || selectedOptionsText
  const fileRefs = buildFileRefText(files, '\n').trim()
  const contextBlocks = [
    buildSelectedOptionsContext(primaryText, normalizedOptions),
    fileRefs ? `相关文件：\n${fileRefs}` : '',
  ].filter(Boolean)

  const combined = [
    primaryText,
    ...contextBlocks,
  ].filter(Boolean).join('\n\n').trim()

  if (combined)
    return combined

  return imageCount > 0 ? '' : null
}

function buildGoalTitle(
  rawInput: string,
  options: string[],
  files: PopupFileAttachment[],
  imageCount: number,
): string {
  const trimmedInput = rawInput.trim()
  if (trimmedInput)
    return trimmedInput

  const selectedOptionsTitle = options
    .map(option => option.trim())
    .filter(Boolean)
    .join(' / ')
  if (selectedOptionsTitle)
    return selectedOptionsTitle

  const fileTitle = files
    .slice(0, 2)
    .map(file => file.name || file.path.split('/').filter(Boolean).pop() || file.path)
    .join(' / ')
  if (fileTitle)
    return `文件目标: ${fileTitle}${files.length > 2 ? ` +${files.length - 2}` : ''}`

  if (imageCount > 0)
    return `图片目标: ${imageCount} 张`

  return ''
}

// 计算属性
const isVisible = computed(() => !!props.request)
const hasOptions = computed(() => (props.request?.predefined_options?.length ?? 0) > 0)
const canSubmit = computed(() => {
  if (hasOptions.value) {
    return selectedOptions.value.length > 0
      || userInput.value.trim().length > 0
      || draggedImages.value.length > 0
      || attachedFiles.value.length > 0
  }
  return userInput.value.trim().length > 0
    || draggedImages.value.length > 0
    || attachedFiles.value.length > 0
})

// 获取输入组件的状态文本
const inputStatusText = computed(() => {
  return inputRef.value?.statusText || '等待输入...'
})

function handleOpenArtifact(artifact: PopupArtifact) {
  emit('openArtifact', artifact)
}

const DEFAULT_QUOTE_SELECTION_SHORTCUT: ShortcutBinding = {
  id: 'quote_selection',
  name: '引用选区',
  description: '将当前选区作为引用插入输入框',
  action: 'quote_selection_to_input',
  key_combination: {
    key: 'Y',
    ctrl: false,
    alt: false,
    shift: true,
    meta: true,
  },
  enabled: true,
  scope: 'popup',
}

function normalizeShortcutEventKey(key: string): string {
  const keyMap: Record<string, string> = {
    ' ': 'Space',
    'ArrowUp': 'Up',
    'ArrowDown': 'Down',
    'ArrowLeft': 'Left',
    'ArrowRight': 'Right',
    'Delete': 'Del',
    'Insert': 'Ins',
    'PageUp': 'PgUp',
    'PageDown': 'PgDn',
  }

  return keyMap[key] || (key.length === 1 ? key.toUpperCase() : key)
}

function quoteSelectionShortcutMatches(event: KeyboardEvent): boolean {
  const binding = getShortcutByAction('quote_selection_to_input') ?? DEFAULT_QUOTE_SELECTION_SHORTCUT
  if (!binding.enabled)
    return false

  const keyCombination = binding.key_combination
  return normalizeShortcutEventKey(event.key).toLowerCase() === keyCombination.key.toLowerCase()
    && event.ctrlKey === keyCombination.ctrl
    && event.altKey === keyCombination.alt
    && event.shiftKey === keyCombination.shift
    && event.metaKey === keyCombination.meta
}

async function insertSelectedTextQuote(selection: PopupTextSelection | null, showEmptyHint = false) {
  if (!selection?.text) {
    if (showEmptyHint)
      message.info('请先在当前内容区域选择文本')
    return
  }

  const inserted = await inputRef.value?.insertSelectedTextQuote(selection)
  if (!inserted && showEmptyHint)
    message.info('请先在当前内容区域选择文本')
}

function handleQuoteSelectionShortcut(event: KeyboardEvent) {
  if (!isVisible.value || submitting.value || loading.value)
    return
  if (!quoteSelectionShortcutMatches(event))
    return

  event.preventDefault()
  event.stopPropagation()

  const selection = contentRef.value?.resolveCurrentTextSelection?.() ?? null
  void insertSelectedTextQuote(selection, true)
}

let focusRetryTimer: ReturnType<typeof setTimeout> | null = null
let focusWindowListenersCleanup: (() => void) | null = null

function clearFocusRetryTimer() {
  if (focusRetryTimer) {
    clearTimeout(focusRetryTimer)
    focusRetryTimer = null
  }
}

function isEditableElement(element: Element | null): boolean {
  if (!(element instanceof HTMLElement))
    return false

  if (element.isContentEditable)
    return true

  return ['INPUT', 'TEXTAREA', 'SELECT'].includes(element.tagName)
}

function isInteractiveMiddleZoneElement(element: Element | null): boolean {
  if (!(element instanceof HTMLElement))
    return false

  return !!element.closest('input, textarea, select, button, a, label, [role="button"], [role="checkbox"], [data-guide="timeline-dot-bar"]')
}

function handleTimelineEdgeMouseMove(event: MouseEvent) {
  const container = event.currentTarget as HTMLElement
  const rect = container.getBoundingClientRect()
  const distanceFromRight = rect.right - event.clientX
  const activeDistance = timelineEdgeActive.value ? TIMELINE_EDGE_ACTIVE_PX : TIMELINE_EDGE_TRIGGER_PX

  timelineEdgeActive.value = distanceFromRight >= 0 && distanceFromRight <= activeDistance
}

function handleTimelineEdgeMouseLeave() {
  timelineEdgeActive.value = false
}

function shouldPreserveCurrentFocus(): boolean {
  const activeElement = document.activeElement
  if (!activeElement || activeElement === document.body)
    return false

  return isEditableElement(activeElement)
}

function handleMiddleZoneClick(event: MouseEvent) {
  if (!isVisible.value || loading.value || submitting.value || !inputRef.value)
    return

  const target = event.target as Element | null
  if (target instanceof HTMLElement && target.closest('[data-guide="popup-content"]'))
    return
  if (isInteractiveMiddleZoneElement(target))
    return

  void inputRef.value.focusInput?.()
}

async function scheduleInputFocus(
  reason: string,
  options: { preserveExistingFocus?: boolean, activateWindow?: boolean } = {},
) {
  if (!isVisible.value || loading.value || !inputRef.value)
    return

  if (options.preserveExistingFocus && shouldPreserveCurrentFocus()) {
    console.log(`[McpPopup] 跳过自动聚焦，保留当前焦点: ${reason}`)
    return
  }

  clearFocusRetryTimer()

  if (options.activateWindow) {
    try {
      await invoke('activate_app_window')
    }
    catch (error) {
      console.log(`[McpPopup] 激活窗口失败: ${reason}`, error)
    }
  }

  await nextTick()

  const attemptFocus = () => {
    if (!isVisible.value || loading.value || !inputRef.value)
      return false

    inputRef.value.focusInput?.()
    return true
  }

  attemptFocus()

  if (typeof window.requestAnimationFrame === 'function') {
    window.requestAnimationFrame(() => {
      void nextTick().then(() => {
        attemptFocus()
      })
    })
  }

  focusRetryTimer = setTimeout(() => {
    attemptFocus()
    focusRetryTimer = null
  }, 180)

  console.log(`[McpPopup] 已调度输入框聚焦: ${reason}`)
}

function syncInputData(data: PopupInputData) {
  if (data.userInput !== undefined) {
    userInput.value = data.userInput
    rawUserInput.value = data.rawUserInput ?? data.userInput
  }
  if (data.selectedOptions !== undefined) {
    selectedOptions.value = [...data.selectedOptions]
  }
  if (data.draggedImages !== undefined) {
    draggedImages.value = [...data.draggedImages]
  }
  if (data.attachedFiles !== undefined) {
    attachedFiles.value = [...data.attachedFiles]
  }

  if (inputRef.value) {
    const popupInputData: PopupInputData = {}
    if (data.userInput !== undefined) {
      popupInputData.userInput = data.userInput
    }
    if (data.selectedOptions !== undefined) {
      popupInputData.selectedOptions = [...data.selectedOptions]
    }
    if (data.draggedImages !== undefined) {
      popupInputData.draggedImages = [...data.draggedImages]
    }
    if (data.attachedFiles !== undefined) {
      popupInputData.attachedFiles = [...data.attachedFiles]
    }

    inputRef.value.updateData(popupInputData)
  }
}

function flushTimelinePrefill() {
  if (!pendingTimelinePrefill.value || loading.value || !inputRef.value)
    return

  const prefill = pendingTimelinePrefill.value
  pendingTimelinePrefill.value = null

  syncInputData({
    userInput: prefill.userInput,
    selectedOptions: prefill.selectedOptions ?? [],
    draggedImages: prefill.draggedImages ?? [],
    attachedFiles: prefill.attachedFiles ?? [],
  })

  if (prefill.focus !== false) {
    void scheduleInputFocus('timeline-prefill')
  }
}

function applyTimelinePrefill(payload: TimelinePrefillPayload) {
  pendingTimelinePrefill.value = {
    userInput: payload.userInput ?? '',
    selectedOptions: payload.selectedOptions ?? [],
    draggedImages: payload.draggedImages ?? [],
    attachedFiles: payload.attachedFiles ?? [],
    focus: payload.focus ?? true,
  }
  flushTimelinePrefill()
}

function getDraft(): TimelinePrefillPayload {
  return {
    userInput: rawUserInput.value,
    selectedOptions: [...selectedOptions.value],
    draggedImages: [...draggedImages.value],
    attachedFiles: attachedFiles.value.map(file => ({ ...file })),
  }
}

// 加载继续回复配置
async function loadReplyConfig() {
  try {
    const config = await invoke('get_reply_config')
    if (config) {
      const replyConfig = config as any
      continueReplyEnabled.value = replyConfig.enable_continue_reply ?? true
      copySubmissionToClipboardEnabled.value = replyConfig.copy_submission_to_clipboard ?? false
      continuePrompt.value = replyConfig.continue_prompt ?? '请按照最佳实践继续'
      loopPrompt.value = replyConfig.loop_prompt ?? DEFAULT_LOOP_PROMPT
      goalPromptTemplate.value = replyConfig.goal_prompt_template ?? DEFAULT_GOAL_PROMPT_TEMPLATE
    }
  }
  catch (error) {
    console.log('加载继续回复配置失败，使用默认值:', error)
  }
}

// 监听配置变化（当从设置页面切换回来时）
watch(() => props.appConfig.reply, (newReplyConfig) => {
  if (newReplyConfig) {
    continueReplyEnabled.value = newReplyConfig.enabled
    continuePrompt.value = newReplyConfig.prompt
    loopPrompt.value = newReplyConfig.loopPrompt
  }
}, { deep: true, immediate: true })

// Telegram事件监听器
let telegramUnlisten: (() => void) | null = null

// 监听请求变化
watch(() => props.request, async (newRequest) => {
  if (newRequest) {
    resetForm()
    loading.value = true
    // 每次显示弹窗时重新加载配置
    loadReplyConfig()

    const shouldForceShow = !!(newRequest.loop_active || newRequest.force_popup)

    // 循环检查点 / loop 完成交付：自动取消静音并显示窗口
    if (shouldForceShow && props.isMuted) {
      emit('toggleMute')
      console.log('🔔 关键 loop 弹窗：自动取消静音')
    }

    // 窗口居中到当前屏幕（静音模式下跳过，避免覆盖 minimize）
    if (!props.isMuted || shouldForceShow) {
      try {
        await invoke('center_window')
      }
      catch (e) {
        console.log('窗口居中失败:', e)
      }
    }

    void nextTick().then(() => {
      loading.value = false
      flushTimelinePrefill()
      void scheduleInputFocus('request-visible')
    })
  }
  else {
    clearFocusRetryTimer()
  }
}, { immediate: true })

watch(() => loading.value, (isLoading) => {
  if (!isLoading) {
    flushTimelinePrefill()
    void scheduleInputFocus('loading-finished')
  }
})

watch(() => inputRef.value, () => {
  flushTimelinePrefill()
  void scheduleInputFocus('input-mounted')
})

watch(isVisible, (visible) => {
  if (visible) {
    void scheduleInputFocus('popup-visible')
  }
})

function setupFocusRecoveryListeners() {
  const handleWindowFocus = () => {
    void scheduleInputFocus('window-focus', { preserveExistingFocus: true })
  }

  const handleVisibilityChange = () => {
    if (document.visibilityState === 'visible') {
      void scheduleInputFocus('visibility-visible', { preserveExistingFocus: true })
    }
  }

  window.addEventListener('focus', handleWindowFocus)
  document.addEventListener('visibilitychange', handleVisibilityChange)

  focusWindowListenersCleanup = () => {
    window.removeEventListener('focus', handleWindowFocus)
    document.removeEventListener('visibilitychange', handleVisibilityChange)
  }
}

// 设置Telegram事件监听
async function setupTelegramListener() {
  try {
    telegramUnlisten = await listen('telegram-event', (event) => {
      console.log('🎯 [McpPopup] 收到Telegram事件:', event)
      console.log('🎯 [McpPopup] 事件payload:', event.payload)
      handleTelegramEvent(event.payload as any)
    })
    console.log('🎯 [McpPopup] Telegram事件监听器已设置')
  }
  catch (error) {
    console.error('🎯 [McpPopup] 设置Telegram事件监听器失败:', error)
  }
}

// 处理Telegram事件
function handleTelegramEvent(event: any) {
  console.log('🎯 [McpPopup] 开始处理事件:', event.type)

  switch (event.type) {
    case 'option_toggled':
      console.log('🎯 [McpPopup] 处理选项切换:', event.option)
      handleOptionToggle(event.option)
      break
    case 'text_updated':
      console.log('🎯 [McpPopup] 处理文本更新:', event.text)
      handleTextUpdate(event.text)
      break
    case 'continue_pressed':
      console.log('🎯 [McpPopup] 处理继续按钮')
      handleContinue()
      break
    case 'send_pressed':
      console.log('🎯 [McpPopup] 处理发送按钮')
      handleSubmit()
      break
    default:
      console.log('🎯 [McpPopup] 未知事件类型:', event.type)
  }
}

// 处理选项切换
function handleOptionToggle(option: string) {
  const index = selectedOptions.value.indexOf(option)
  if (index > -1) {
    // 取消选择
    selectedOptions.value.splice(index, 1)
  }
  else {
    // 添加选择
    selectedOptions.value.push(option)
  }

  // 同步到PopupInput组件
  syncInputData({ selectedOptions: selectedOptions.value })
}

// 处理文本更新
function handleTextUpdate(text: string) {
  syncInputData({ userInput: text })
}

// 设置浏览器 AI 回复监听
async function setupBrowserAiListener() {
  try {
    console.log('[McpPopup] 开始设置浏览器 AI 监听...')
    unlistenBrowserAi = await listen('browser-ai-completed', (event) => {
      console.log('[McpPopup] 收到 browser-ai-completed 事件:', event)
      console.log('[McpPopup] event.payload:', event.payload)
      const payload = event.payload as { message_preview?: string }
      if (payload.message_preview) {
        console.log('[McpPopup] 收到浏览器 AI 回复，长度:', payload.message_preview.length)
        browserAiResponse.value = payload.message_preview
      }
      else {
        console.log('[McpPopup] message_preview 为空或不存在')
      }
    })
    console.log('[McpPopup] 浏览器 AI 监听设置完成')
  }
  catch (error) {
    console.error('设置浏览器 AI 监听失败:', error)
  }
}

// 获取最新的 AI 回复（弹窗打开时）
async function fetchLatestAiResponse() {
  console.log('[McpPopup] === fetchLatestAiResponse 开始 ===')
  try {
    console.log('[McpPopup] 调用 invoke get_latest_ai_response...')
    const response = await invoke<string | null>('get_latest_ai_response')
    console.log('[McpPopup] invoke 返回:', typeof response, response ? `长度${response.length}` : 'null/undefined')
    if (response) {
      browserAiResponse.value = response
      console.log('[McpPopup] browserAiResponse 已设置, 值:', browserAiResponse.value?.substring(0, 30))
    }
    else {
      console.log('[McpPopup] response 为空，不设置 browserAiResponse')
    }
  }
  catch (error) {
    console.error('[McpPopup] invoke 失败:', error)
  }
  console.log('[McpPopup] === fetchLatestAiResponse 结束 ===')
}

// 组件挂载时设置监听器和加载配置
onMounted(async () => {
  loadReplyConfig()
  setupFocusRecoveryListeners()
  setupTelegramListener()
  setupBrowserAiListener()
  window.addEventListener('keydown', handleQuoteSelectionShortcut)
  await Promise.all([
    fetchLatestAiResponse(),
    loadShortcutConfig(),
  ])
})

// 组件卸载时清理监听器
onUnmounted(() => {
  clearFocusRetryTimer()
  focusWindowListenersCleanup?.()
  focusWindowListenersCleanup = null
  if (telegramUnlisten) {
    telegramUnlisten()
  }
  if (unlistenBrowserAi) {
    unlistenBrowserAi()
  }
  window.removeEventListener('keydown', handleQuoteSelectionShortcut)
})

// 重置表单
function resetForm() {
  selectedOptions.value = []
  userInput.value = ''
  rawUserInput.value = ''
  draggedImages.value = []
  attachedFiles.value = []
  submitting.value = false
}

async function backupCurrentSubmissionToClipboard() {
  try {
    await copySubmissionToClipboard({
      enabled: copySubmissionToClipboardEnabled.value,
      userInput: rawUserInput.value,
      selectedOptions: selectedOptions.value,
      writeText: text => invoke('plugin:clipboard-manager|write_text', { text }),
    })
  }
  catch (error) {
    console.error('发送内容备份到剪贴板失败:', error)
    message.warning('未能备份到剪贴板，仍将继续发送')
  }
}

// 桌面弹窗统一回复给当前 IDE/MCP 调用方。
async function handleSubmit() {
  if (!canSubmit.value || submitting.value)
    return

  void invoke('timeline_debug_log', {
    location: 'frontend/mcp_popup/handle_submit:start',
    payload: {
      userInput: userInput.value,
      selectedOptions: selectedOptions.value,
      hasInputRef: Boolean(inputRef.value),
      hasRecordAutoPromotion: typeof inputRef.value?.recordSubmittedInputForAutoPromotion === 'function',
      sendTarget: 'ide',
    },
  }).catch(() => {})
  submitting.value = true

  try {
    await backupCurrentSubmissionToClipboard()
    const finalUserInput = buildFinalUserInput(userInput.value, attachedFiles.value)
    inputRef.value?.recordSubmittedInputForAutoPromotion()
    const response = {
      user_input: finalUserInput,
      selected_options: selectedOptions.value,
      images: buildImageAttachments(draggedImages.value),
      file_paths: buildFilePaths(attachedFiles.value),
      image_paths: [],
      metadata: {
        timestamp: new Date().toISOString(),
        request_id: props.request?.id || null,
        source: resolveSubmitSource(finalUserInput, selectedOptions.value),
      },
    }

    if (!response.user_input && response.selected_options.length === 0 && response.images.length === 0)
      response.user_input = '用户确认继续'

    const liveGoalSnapshot = await applyLiveGoalIntent(response.user_input || '', response.selected_options)
    if (liveGoalSnapshot)
      Object.assign(response.metadata, await resolveLiveGoalResponseMetadata(liveGoalSnapshot))

    if (props.mockMode) {
      await new Promise(resolve => setTimeout(resolve, 1000))
      message.success('模拟响应发送成功')
    }

    emit('response', response)
  }
  catch (error) {
    console.error('提交失败:', error)
    message.error('提交失败，请重试')
  }
  finally {
    submitting.value = false
  }
}

// 处理输入更新
function handleInputUpdate(data: PopupInputData) {
  userInput.value = data.userInput ?? ''
  rawUserInput.value = data.rawUserInput ?? data.userInput ?? ''
  selectedOptions.value = data.selectedOptions ?? []
  draggedImages.value = data.draggedImages ?? []
  attachedFiles.value = data.attachedFiles ?? []
}

// 处理图片添加 - 移除重复逻辑，避免双重添加
function handleImageAdd(_image: string) {
  // 这个函数现在只是为了保持接口兼容性，实际添加在PopupInput中完成
}

// 处理继续按钮点击
async function handleContinue() {
  if (submitting.value)
    return

  submitting.value = true

  try {
    await backupCurrentSubmissionToClipboard()
    // 使用新的结构化数据格式
    const response = {
      user_input: continuePrompt.value,
      selected_options: [],
      images: [],
      metadata: {
        timestamp: new Date().toISOString(),
        request_id: props.request?.id || null,
        source: 'popup_continue',
      },
    }

    if (props.mockMode) {
      // 模拟模式下的延迟
      await new Promise(resolve => setTimeout(resolve, 1000))
      message.success('继续请求发送成功')
    }
    else {
      // 交给父组件统一处理（主进程不退出，子进程才退出）
    }

    emit('response', response)
  }
  catch (error) {
    console.error('发送继续请求失败:', error)
    message.error('继续请求失败，请重试')
  }
  finally {
    submitting.value = false
  }
}

// 处理引用消息
function handleQuoteMessage(messageContent: string) {
  if (inputRef.value) {
    inputRef.value.handleQuoteMessage(messageContent)
  }
}

function handleTimelineNodeQuote(content: string) {
  const normalized = content.trim()
  if (!normalized)
    return
  handleQuoteMessage(normalized)
}

function handleFilesAdd(files: PopupFileAttachment[]) {
  const mergedFiles = [...attachedFiles.value]

  files.forEach((file) => {
    if (mergedFiles.some(existing => existing.path === file.path))
      return
    mergedFiles.push(file)
  })

  syncInputData({ attachedFiles: mergedFiles })
}

function buildGoalSubmitPrompt(
  goal: string,
  huiSnapshot: string | null = null,
  template: string = DEFAULT_GOAL_PROMPT_TEMPLATE,
): string {
  const huiSnapshotBlock = huiSnapshot
    ? `\n\n## Hui Snapshot（系统预取）\n${huiSnapshot}\n\n以上 snapshot 只作为恢复起点；需要使用时仍需按 hui1 规则核对当前 timeline/thread/run。`
    : '\n\n## Hui Snapshot\n未预取到 snapshot；只有目标依赖历史停点、压缩前上下文或用户明确要求时，才按 hui1 规则恢复当前 timeline/thread/run 最近任务。'

  const executionRules = template.trim() || DEFAULT_GOAL_PROMPT_TEMPLATE

  return `进入 GoalRun 目标模式。

目标：
《
${goal}
》${huiSnapshotBlock}

## XI 启动检查（正式 Goal 同步后执行）
任何实现动作前必须执行 xi，按当前项目、线程和目标关键词检查相关对话、experience 与稳定产物，判断同一目标是否已经解决或已有可复用方案。命中完成证据时先验证当前状态并复用，禁止重复实现或伪造完成；只命中相似问题时说明差异后继续。

执行规则：
${executionRules}`
}

function normalizeOptionalString(value: unknown): string | null {
  if (typeof value !== 'string')
    return null
  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

function normalizeOptionalNumber(value: unknown): number | null {
  const numeric = typeof value === 'number'
    ? value
    : typeof value === 'string'
      ? Number(value.trim())
      : Number.NaN
  return Number.isFinite(numeric) && numeric >= 0 ? Math.trunc(numeric) : null
}

async function resolveLiveGoalResponseMetadata(liveGoal: any | null): Promise<LiveGoalRunMetadata> {
  const runId = normalizeOptionalString(liveGoal?.run_id ?? liveGoal?.runId)
  const generation = normalizeOptionalNumber(liveGoal?.generation ?? liveGoal?.runGeneration)
  const fallback = emptyLiveGoalRunMetadata(runId, generation)
  if (!runId && generation === null)
    return fallback

  try {
    const metadata = await invoke<any>('resolve_live_goal_response_metadata', {
      projectPath: props.request?.project_path,
      runId,
      generation,
    })
    return {
      run_id: normalizeOptionalString(metadata?.run_id ?? metadata?.runId) ?? runId,
      generation: normalizeOptionalNumber(metadata?.generation ?? metadata?.runGeneration) ?? generation,
      stale_of: normalizeOptionalString(metadata?.stale_of ?? metadata?.staleOf),
      superseded_by: normalizeOptionalString(metadata?.superseded_by ?? metadata?.supersededBy),
      is_stale: Boolean(metadata?.is_stale ?? metadata?.isStale),
    }
  }
  catch (error) {
    console.warn('Live Goal run metadata 校验失败:', error)
    return fallback
  }
}

function resolveGoalRunTimelineRouteId(): string | null {
  const request = props.request as any
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
    request?.id,
    request?.request_id,
    request?.requestId,
    request?.metadata?.request_id,
    request?.metadata?.requestId,
  ]

  for (const candidate of candidates) {
    const normalized = normalizeOptionalString(candidate)
    if (normalized)
      return normalized
  }

  return null
}

async function getGoalRunHuiSnapshot(liveGoal: any | null = null): Promise<string | null> {
  try {
    return await invoke<string | null>('get_hui_snapshot', {
      projectPath: props.request?.project_path,
      requestId: resolveGoalRunTimelineRouteId(),
      runId: normalizeOptionalString(liveGoal?.run_id ?? liveGoal?.runId),
      generation: normalizeOptionalNumber(liveGoal?.generation ?? liveGoal?.runGeneration),
    })
  }
  catch (error) {
    console.warn('GoalRun hui snapshot 预取失败:', error)
    return null
  }
}

function shouldPrefetchGoalRunHuiSnapshot(goal: string): boolean {
  return /hui[01]?|回溯|恢复上下文|最近停点|上次|之前|继续|接着|压缩|compact/i.test(goal)
}

// 处理目标按钮点击：启动 Live Goal，并提交一次非 loop 的目标模式请求。
async function handleGoalSubmit() {
  if (submitting.value)
    return

  submitting.value = true

  try {
    // 过滤掉系统注入的上下文，只保留用户真正输入的内容
    const cleanedInput = stripAutoPrompt(userInput.value.trim()).trim()
    const selectedGoalOptions = selectedOptions.value
      .map(option => option.trim())
      .filter(Boolean)
    const goalText = buildGoalTargetText(
      cleanedInput,
      selectedGoalOptions,
      attachedFiles.value,
      draggedImages.value.length,
    )
    const goalTitle = buildGoalTitle(
      cleanedInput,
      selectedGoalOptions,
      attachedFiles.value,
      draggedImages.value.length,
    )

    if (goalText === null) {
      submitting.value = false
      message.warning('请先输入目标、选择选项、添加文件或上传图片')
      return
    }

    await backupCurrentSubmissionToClipboard()
    const liveGoalSnapshot = await applyLiveGoalIntent(`/goal ${goalTitle}`)
    const huiSnapshot = shouldPrefetchGoalRunHuiSnapshot(goalText)
      ? await getGoalRunHuiSnapshot(liveGoalSnapshot)
      : null
    const timelineRouteId = resolveGoalRunTimelineRouteId()
    const liveGoal = liveGoalSnapshot as any
    const runMetadata = await resolveLiveGoalResponseMetadata(liveGoal)

    const response = {
      user_input: buildGoalSubmitPrompt(goalText, huiSnapshot, goalPromptTemplate.value),
      selected_options: [...selectedOptions.value],
      images: buildImageAttachments(draggedImages.value),
      file_paths: buildFilePaths(attachedFiles.value),
      image_paths: [],
      metadata: {
        timestamp: new Date().toISOString(),
        request_id: props.request?.id || null,
        timeline_route_id: timelineRouteId,
        conversation_route_id: timelineRouteId,
        source: 'popup_goal_submit',
        mode: 'goalrun_takeover',
        hui_snapshot: huiSnapshot,
        ...runMetadata,
      },
    }

    if (props.mockMode) {
      await new Promise(resolve => setTimeout(resolve, 1000))
      message.success('目标请求发送成功')
    }
    else {
      // 交给父组件统一处理（主进程不退出，子进程才退出）
    }

    emit('response', response)
  }
  catch (error) {
    console.error('发送目标请求失败:', error)
    message.error('目标请求失败，请重试')
  }
  finally {
    submitting.value = false
  }
}

defineExpose({
  applyTimelinePrefill,
  getDraft,
})
</script>

<template>
  <div v-if="isVisible" class="flex flex-col flex-1 min-h-0">
    <!-- 内容区域 - 可滚动，与右侧时间线小球条并排 -->
    <div
      class="flex flex-1 min-h-0 overflow-hidden"
      data-guide="popup-timeline-row"
      @mousemove="handleTimelineEdgeMouseMove"
      @mouseleave="handleTimelineEdgeMouseLeave"
    >
      <div class="flex-1 min-h-0 overflow-y-auto scrollbar-thin" @click="handleMiddleZoneClick">
        <!-- 消息内容 - 允许选中 -->
        <div class="mx-2 mt-2 mb-1 px-4 py-3 bg-black-100 rounded-lg select-text" data-guide="popup-content">
          <PopupContent
            ref="contentRef"
            :request="request"
            :loading="loading"
            :current-theme="props.appConfig.theme"
            :browser-ai-response="browserAiResponse"
            @add-files="handleFilesAdd"
            @open-artifact="handleOpenArtifact"
            @quote-message="handleQuoteMessage"
          />
        </div>

        <!-- 输入和选项 - 允许选中 -->
        <div class="px-4 pb-3 bg-black select-text">
          <PopupInput
            ref="inputRef"
            :request="request"
            :context-key="popupContextKey"
            :context-prompt-state="props.contextPromptState"
            :enable-context-append="true"
            :loading="loading"
            :submitting="submitting"
            @update="handleInputUpdate"
            @image-add="handleImageAdd"
            @conditional-state-change="emit('conditionalStateChange', $event)"

            @at-trigger="handleAtTrigger"
          />
        </div>
      </div>

      <!-- 右侧：时间线小球条（仅在内容区，不延伸到底部操作栏） -->
      <div class="timeline-dot-column flex-shrink-0 self-stretch min-h-0 overflow-hidden">
        <TimelineDotBar
          :tree-id="props.timelineTreeId ?? null"
          :current-node-id="props.timelineCurrentNodeId ?? null"
          :mock-nodes="props.timelineMockNodes"
          compact-hover
          :compact-expanded="timelineEdgeActive"
          @node-click="emit('timelineNodeClick', $event)"
          @node-quote="handleTimelineNodeQuote"
        />
      </div>
    </div>

    <!-- 底部操作栏 - 固定在底部 -->
    <div class="mt-auto flex-shrink-0 bg-black-100 border-t-2 border-black-200" data-guide="popup-actions">
      <PopupActions
        :request="request" :loading="loading" :submitting="submitting" :can-submit="canSubmit"
        :continue-reply-enabled="continueReplyEnabled" :input-status-text="inputStatusText"
        @submit="handleSubmit" @continue="handleContinue" @goal-submit="handleGoalSubmit"
      />
    </div>
  </div>
</template>
