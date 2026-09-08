<script setup lang="ts">
import type { SpeechSnapshot } from '../../services/globalSpeechSession'
import type { SpeechInsertAuthority, SpeechInsertPayload } from '../../services/speechInsertGuard'
import type { CustomPrompt, McpRequest, PopupFileAttachment, PopupInputData, PopupTextSelection } from '../../types/popup'
import type { GhostSuggestionAutoPromotionState } from '../../utils/ghostSuggestionAutoPromotion'
import type { CommandSuggestion } from '../../utils/ghostSuggestionMatching'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { useSortable } from '@vueuse/integrations/useSortable'
import { useMessage } from 'naive-ui'
import { computed, nextTick, onMounted, onUnmounted, ref, shallowRef, watch } from 'vue'
import { GHOST_SUGGESTION_TOKEN_PATTERN, useGhostSuggestions } from '../../composables/useGhostSuggestions'
import { useKeyboard } from '../../composables/useKeyboard'
import { usePromptLibrary } from '../../composables/usePromptLibrary'
import { SpeechInsertGuard } from '../../services/speechInsertGuard'
import { isExplicitConversationEndInput } from '../../utils/conversationEndCommand'
import {
  extractGhostSuggestionAutoPromotionTerms,
  getGhostSuggestionAutoPromotionCandidates,
  GHOST_SUGGESTION_AUTO_PROMOTION_STORAGE_KEY,
  parseGhostSuggestionAutoPromotionState,
  shouldTrackGhostSuggestionAutoPromotion,
} from '../../utils/ghostSuggestionAutoPromotion'
import {
  getCommandSuggestionSuffix,
  getMatchingCommandSuggestions,
  hasVisibleCommandSuggestion,
} from '../../utils/ghostSuggestionMatching'
import { buildSelectedTextQuoteBlock } from '../../utils/popupSelectionQuote'
import { getPromptShortcutIndex, PROMPT_SHORTCUT_LIMIT } from '../../utils/prompt-shortcut.mjs'
import { insertDroppedText } from '../../utils/text-drop.mjs'

interface Props {
  request: McpRequest | null
  contextKey?: string
  contextPromptState?: Record<string, Pick<CustomPrompt, 'current_state' | 'is_active'>>
  enableContextAppend?: boolean
  loading?: boolean
  submitting?: boolean
}

interface Emits {
  update: [data: PopupInputData]
  imageAdd: [image: string]
  atTrigger: []
  conditionalStateChange: [payload: { promptId: string, current_state?: boolean, is_active?: boolean }]
}

interface CustomPromptConfigSnapshot {
  prompts?: CustomPrompt[]
  enabled?: boolean
}

interface ActiveCommandContext {
  prefix: string
  token: string
}

interface NativeTextDropPayload {
  text: string
  logicalPosition: {
    x: number
    y: number
  }
}

const props = withDefaults(defineProps<Props>(), {
  enableContextAppend: true,
  loading: false,
  submitting: false,
})

const emit = defineEmits<Emits>()
const CONTEXT_INJECTION_START = '<!-- CONTEXT_INJECTION_START -->'

let customPromptConfigCache: CustomPromptConfigSnapshot | null = null
let customPromptConfigPromise: Promise<CustomPromptConfigSnapshot | null> | null = null
let unlistenDragDrop: (() => void) | null = null
let unlistenNativeTextDrop: (() => void) | null = null
let unlistenSpeechInsert: (() => void) | null = null
let unlistenSpeechSnapshot: (() => void) | null = null
let registeredSpeechTargetRequestId: string | null = null
const speechInsertGuard = new SpeechInsertGuard()
let suppressSpeechTargetFocusUntil = 0
const contextPromptStateCache = new Map<string, Record<string, Pick<CustomPrompt, 'current_state' | 'is_active'>>>()

// 响应式数据
const userInput = ref('')
const selectedOptions = ref<string[]>([])
const uploadedImages = ref<string[]>([])
const attachedFiles = ref<PopupFileAttachment[]>([])
const textareaRef = ref<HTMLTextAreaElement | null>(null)
const activeSuggestionIndex = ref(0)
const acceptedSuggestionToken = ref('')
const isComposing = ref(false)
const ghostMetricsStyle = ref('')
const textareaIsScrolled = ref(false)
const historyCommandSuggestions = ref<CommandSuggestion[]>([])
const isInputDragOver = ref(false)

// 自定义prompt相关状态
const customPrompts = ref<CustomPrompt[]>([])
const customPromptEnabled = ref(true)
const showInsertDialog = ref(false)
const pendingPromptContent = ref('')
const activePromptShortcutIndex = ref<number | null>(null)
const isPromptShortcutModifierHeld = ref(false)

// 提示词库
const promptLibrary = usePromptLibrary()
const {
  suggestions: allGhostSuggestions,
  enabledSuggestions: customGhostSuggestions,
  ghostSuggestionsEnabled,
  setGhostSuggestionsEnabled,
  load: loadGhostSuggestions,
} = useGhostSuggestions()
const fileInputRef = ref<HTMLInputElement | null>(null)

// 提示词库管理状态
const showAddPromptForm = ref(false)
const editingLibraryItem = ref<{ id: string, name: string, content: string, category: string } | null>(null)
const newLibraryItem = ref({ name: '', content: '', category: '' })
const selectedCategory = ref('')

// 移除条件性prompt状态管理，直接使用prompt的current_state

// 分离普通prompt和条件性prompt
const normalPrompts = computed(() =>
  customPrompts.value.filter(prompt => prompt.type === 'normal' || !prompt.type),
)

const conditionalPrompts = computed(() =>
  customPrompts.value.filter(prompt => prompt.type === 'conditional'),
)

// 拖拽排序相关状态
const promptContainer = ref<HTMLElement | null>(null)
const sortablePrompts = shallowRef<CustomPrompt[]>([])
const { start, stop } = useSortable(promptContainer, sortablePrompts, {
  animation: 200,
  ghostClass: 'sortable-ghost',
  chosenClass: 'sortable-chosen',
  dragClass: 'sortable-drag',
  handle: '.drag-handle',
  forceFallback: true,
  fallbackTolerance: 3,
  onStart: (evt) => {
    console.log('PopupInput: 拖拽开始:', evt)
    console.log('PopupInput: 拖拽开始时的容器:', evt.from)
    console.log('PopupInput: 拖拽开始时的元素:', evt.item)
  },
  onEnd: (evt) => {
    console.log('PopupInput: 拖拽排序完成:', evt)
    console.log('PopupInput: 从索引', evt.oldIndex, '移动到索引', evt.newIndex)
    console.log('PopupInput: 拖拽后的sortablePrompts:', sortablePrompts.value.map(p => ({ id: p.id, name: p.name })))

    // 检查是否真的发生了位置变化
    if (evt.oldIndex !== evt.newIndex && evt.oldIndex !== undefined && evt.newIndex !== undefined) {
      // 手动重新排列数组
      const newList = [...sortablePrompts.value]
      const [movedItem] = newList.splice(evt.oldIndex, 1)
      newList.splice(evt.newIndex, 0, movedItem)

      // 更新sortablePrompts
      sortablePrompts.value = newList
      console.log('PopupInput: 手动更新后的sortablePrompts:', sortablePrompts.value.map(p => ({ id: p.id, name: p.name })))

      // 立即更新 customPrompts 的顺序，确保数据同步
      // 保留条件性prompt，只更新普通prompt的顺序
      const conditionalPromptsList = customPrompts.value.filter(prompt => prompt.type === 'conditional')
      customPrompts.value = [...sortablePrompts.value, ...conditionalPromptsList]
      console.log('PopupInput: 位置发生变化，保存新排序')

      // 立即保存排序
      savePromptOrder()
    }
    else {
      console.log('PopupInput: 位置未发生变化，无需保存')
    }
  },
  onMove: (evt) => {
    console.log('PopupInput: 拖拽移动中:', evt)
    return true // 允许移动
  },
  onChoose: (evt) => {
    console.log('PopupInput: 选择拖拽元素:', evt)
  },
  onUnchoose: (evt) => {
    console.log('PopupInput: 取消选择拖拽元素:', evt)
  },
})

// 使用键盘快捷键 composable
const { pasteShortcut } = useKeyboard()

const message = useMessage()
let localFocusTimer: ReturnType<typeof setTimeout> | null = null
let localFocusFrame: number | null = null
let promptShortcutFeedbackTimer: ReturnType<typeof setTimeout> | null = null
let ghostMetricsFrame: number | null = null
let textareaAutosizeTimer: ReturnType<typeof setTimeout> | null = null
let textareaAutosizeFrame: number | null = null
let optionGhostControlCandidate: 'AltLeft' | 'AltRight' | null = null
const pressedGhostControlOptionCodes = new Set<'AltLeft' | 'AltRight'>()
const TEXTAREA_AUTOSIZE_IDLE_MS = 120

// 计算属性
const hasOptions = computed(() => (props.request?.predefined_options?.length ?? 0) > 0)
const canSubmit = computed(() => {
  const hasOptionsSelected = selectedOptions.value.length > 0
  const hasInputText = userInput.value.trim().length > 0
  const hasImages = uploadedImages.value.length > 0
  const hasFiles = attachedFiles.value.length > 0

  if (hasOptions.value) {
    return hasOptionsSelected || hasInputText || hasImages || hasFiles
  }
  return hasInputText || hasImages || hasFiles
})

const mergedCommandSuggestions = computed(() => {
  const merged = new Map<string, CommandSuggestion>()
  const userSuggestions = customGhostSuggestions.value.map(suggestion => ({
    key: suggestion.key,
    description: suggestion.description || '自定义幽灵补全',
  }))

  ;[...userSuggestions, ...historyCommandSuggestions.value].forEach((suggestion) => {
    const normalizedKey = suggestion.key.trim()
    if (!normalizedKey)
      return

    if (!merged.has(normalizedKey)) {
      merged.set(normalizedKey, {
        key: normalizedKey,
        description: suggestion.description,
      })
    }
  })

  return Array.from(merged.values())
})

const activeCommandContext = computed<ActiveCommandContext | null>(() => {
  const rawInput = userInput.value
  if (!rawInput)
    return null

  const match = rawInput.match(GHOST_SUGGESTION_TOKEN_PATTERN)
  if (!match)
    return null

  const token = match[0]
  const prefix = rawInput.slice(0, rawInput.length - token.length)
  if (prefix.endsWith('@') || prefix.endsWith('爱特'))
    return null

  return {
    prefix,
    token,
  }
})

const activeCommandToken = computed(() => {
  return activeCommandContext.value?.token ?? ''
})

const ghostPrefixText = computed(() => {
  return activeCommandContext.value?.prefix ?? ''
})

const commandSuggestions = computed(() => {
  if (!ghostSuggestionsEnabled.value)
    return []

  const token = activeCommandToken.value.toLowerCase()
  if (!token || isComposing.value)
    return []

  if (acceptedSuggestionToken.value && token === acceptedSuggestionToken.value.toLowerCase())
    return []

  return getMatchingCommandSuggestions(mergedCommandSuggestions.value, token, {
    acceptedSuggestionToken: acceptedSuggestionToken.value,
    isComposing: isComposing.value,
  })
})

const hasCommandSuggestions = computed(() => commandSuggestions.value.length > 0)

const activeSuggestion = computed(() => {
  if (!hasCommandSuggestions.value)
    return null

  const safeIndex = Math.min(activeSuggestionIndex.value, commandSuggestions.value.length - 1)
  return commandSuggestions.value[safeIndex] ?? null
})

const previewSuffix = computed(() => {
  const suggestion = activeSuggestion.value
  const token = activeCommandToken.value
  return getCommandSuggestionSuffix(suggestion, token)
})

const shouldShowGhostSuggestion = computed(() => {
  return !!activeSuggestion.value && previewSuffix.value.length > 0 && !textareaIsScrolled.value
})

const canApplyActiveSuggestion = computed(() => {
  return hasVisibleCommandSuggestion(activeSuggestion.value, activeCommandToken.value)
})

// 工具栏状态文本
const statusText = computed(() => {
  // 检查是否有任何输入内容
  const hasInput = selectedOptions.value.length > 0
    || uploadedImages.value.length > 0
    || attachedFiles.value.length > 0
    || userInput.value.trim().length > 0

  // 如果有任何输入内容，返回空字符串让 PopupActions 显示快捷键
  if (hasInput) {
    return ''
  }

  return '等待输入...'
})

// 发送更新事件
function emitUpdate() {
  // 获取条件性prompt的追加内容
  // 结束指令必须保持为完整原文，不能被上下文模板扩展后再交给
  // Rust 响应边界判断。其他输入仍沿用原有上下文追加行为。
  const conditionalContent = props.enableContextAppend
    && !isExplicitConversationEndInput(userInput.value)
    ? generateConditionalContent()
    : ''

  // 将条件性内容追加到用户输入
  const finalUserInput = userInput.value + conditionalContent

  emit('update', {
    userInput: finalUserInput,
    rawUserInput: userInput.value,
    selectedOptions: selectedOptions.value,
    draggedImages: uploadedImages.value,
    attachedFiles: attachedFiles.value,
  })
}

// 处理选项变化
function handleOptionChange(option: string, checked: boolean) {
  if (checked) {
    selectedOptions.value.push(option)
  }
  else {
    const idx = selectedOptions.value.indexOf(option)
    if (idx > -1)
      selectedOptions.value.splice(idx, 1)
  }
  emitUpdate()
}

// 处理选项切换（整行点击）
function handleOptionToggle(option: string) {
  const idx = selectedOptions.value.indexOf(option)
  if (idx > -1) {
    selectedOptions.value.splice(idx, 1)
  }
  else {
    selectedOptions.value.push(option)
  }
  emitUpdate()
}

const IMAGE_PATH_PATTERN = /\.(?:png|jpe?g|gif|webp|svg|bmp|heic|heif|tiff?)$/i

function getDisplayNameFromPath(path: string): string {
  return path.split('/').filter(Boolean).pop() || path
}

function isImagePath(path: string): boolean {
  return IMAGE_PATH_PATTERN.test(path)
}

function normalizeClipboardPath(rawValue: string): string | null {
  const trimmed = rawValue.trim().replace(/^['"]|['"]$/g, '')
  if (!trimmed)
    return null

  if (trimmed.startsWith('file://')) {
    try {
      return decodeURIComponent(new URL(trimmed).pathname)
    }
    catch {
      return decodeURIComponent(trimmed.replace(/^file:\/\//, ''))
    }
  }

  if (trimmed.startsWith('//') || trimmed.startsWith('/*')) {
    return null
  }

  if (trimmed.startsWith('/')) {
    return trimmed
  }

  return null
}

function extractClipboardPaths(rawText: string): string[] {
  const uniquePaths = new Set<string>()

  rawText
    .split(/\r?\n/)
    .map(value => normalizeClipboardPath(value))
    .filter((value): value is string => !!value)
    .forEach((value) => {
      uniquePaths.add(value)
    })

  return Array.from(uniquePaths)
}

function addFileAttachments(files: PopupFileAttachment[]): void {
  let addedCount = 0

  files.forEach((file) => {
    if (attachedFiles.value.some(existing => existing.path === file.path))
      return

    attachedFiles.value.push(file)
    addedCount += 1
  })

  if (addedCount > 0) {
    emitUpdate()
    message.success(`已添加 ${addedCount} 个文件`)
  }
}

async function addImagePath(path: string): Promise<boolean> {
  try {
    const dataUrl = await invoke('read_file_base64', { path }) as string
    if (!uploadedImages.value.includes(dataUrl)) {
      uploadedImages.value.push(dataUrl)
      emitUpdate()
      message.success(`图片 ${getDisplayNameFromPath(path)} 已添加`)
    }
    return true
  }
  catch (error) {
    console.error('从路径加载图片失败:', error)
    return false
  }
}

async function addAttachmentPaths(paths: string[]): Promise<void> {
  if (paths.length === 0)
    return

  const filesToAdd: PopupFileAttachment[] = []

  for (const path of paths) {
    if (isImagePath(path)) {
      const addedAsImage = await addImagePath(path)
      if (addedAsImage)
        continue
    }

    filesToAdd.push({
      path,
      name: getDisplayNameFromPath(path),
    })
  }

  addFileAttachments(filesToAdd)
}

async function tryAddNativeClipboardFiles(): Promise<void> {
  try {
    const clipboardPaths = await invoke('read_clipboard_file_paths') as string[]
    const normalizedPaths = clipboardPaths
      .map(path => normalizeClipboardPath(path))
      .filter((path): path is string => Boolean(path))

    await addAttachmentPaths(normalizedPaths)
  }
  catch (error) {
    console.error('读取原生剪贴板文件失败:', error)
  }
}

function handleInputPaste(event: ClipboardEvent) {
  const items = event.clipboardData?.items
  let hasImage = false

  if (items) {
    for (const item of items) {
      if (item.type.includes('image')) {
        hasImage = true
        const file = item.getAsFile()
        if (file) {
          handleImageFiles([file])
        }
      }
    }
  }

  const clipboardText = event.clipboardData?.getData('text') ?? ''

  // `/end` starts with a slash but is a conversation command, not a file path.
  // Let the browser paste it normally; the Rust response boundary decides
  // whether it ends this interaction.
  if (isExplicitConversationEndInput(clipboardText))
    return

  const pastedPaths = extractClipboardPaths(clipboardText)

  if (pastedPaths.length > 0) {
    event.preventDefault()
    void addAttachmentPaths(pastedPaths)
    return
  }

  if (hasImage) {
    event.preventDefault()
    return
  }

  if (clipboardText.trim().length === 0)
    void tryAddNativeClipboardFiles()
}

async function handleImageFiles(files: FileList | File[]): Promise<void> {
  console.log('=== 处理图片文件 ===')
  console.log('文件数量:', files.length)

  for (const file of files) {
    console.log('处理文件:', file.name, '类型:', file.type, '大小:', file.size)

    if (file.type.startsWith('image/')) {
      try {
        console.log('开始转换为 Base64...')
        const base64 = await fileToBase64(file)
        console.log('Base64转换成功，长度:', base64.length)

        // 检查是否已存在相同图片，避免重复添加
        if (!uploadedImages.value.includes(base64)) {
          uploadedImages.value.push(base64)
          console.log('图片已添加到数组，当前数量:', uploadedImages.value.length)
          message.success(`图片 ${file.name} 已添加`)
          emitUpdate()
        }
        else {
          console.log('图片已存在，跳过:', file.name)
          message.warning(`图片 ${file.name} 已存在`)
        }
      }
      catch (error) {
        console.error('图片处理失败:', error)
        message.error(`图片 ${file.name} 处理失败`)
        throw error
      }
    }
    else {
      console.log('跳过非图片文件:', file.type)
    }
  }

  console.log('=== 图片文件处理完成 ===')
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result as string)
    reader.onerror = reject
    reader.readAsDataURL(file)
  })
}

function removeImage(index: number) {
  uploadedImages.value.splice(index, 1)
  emitUpdate()
}

function removeFile(index: number) {
  attachedFiles.value.splice(index, 1)
  emitUpdate()
}

function handleInputDragOver(event: DragEvent) {
  event.preventDefault()
  isInputDragOver.value = true
}

function handleInputDragLeave(event: DragEvent) {
  const currentTarget = event.currentTarget as HTMLElement | null
  const relatedTarget = event.relatedTarget as Node | null
  if (currentTarget && relatedTarget && currentTarget.contains(relatedTarget))
    return

  isInputDragOver.value = false
}

function applyDroppedText(droppedText: string) {
  if (droppedText.length === 0)
    return

  const inputElement = getTextareaElement()
  const result = insertDroppedText(
    userInput.value,
    droppedText,
    inputElement?.selectionStart,
    inputElement?.selectionEnd,
  )

  userInput.value = result.value

  void nextTick(() => {
    inputElement?.focus()
    inputElement?.setSelectionRange(result.cursor, result.cursor)
  })
}

function handleInputDrop(event: DragEvent) {
  event.preventDefault()
  isInputDragOver.value = false

  const droppedText = event.dataTransfer?.getData('text/plain') ?? ''
  applyDroppedText(droppedText)
}

async function handleNativeTextDrop(payload: NativeTextDropPayload) {
  const inputElement = getTextareaElement()
  if (
    !inputElement
    || props.loading
    || props.submitting
    || payload.text.length === 0
    || !Number.isFinite(payload.logicalPosition?.x)
    || !Number.isFinite(payload.logicalPosition?.y)
  ) {
    return
  }

  try {
    const bounds = inputElement.getBoundingClientRect()
    const hitsTextarea = payload.logicalPosition.x >= bounds.left
      && payload.logicalPosition.x <= bounds.right
      && payload.logicalPosition.y >= bounds.top
      && payload.logicalPosition.y <= bounds.bottom

    if (!hitsTextarea)
      return

    applyDroppedText(payload.text)
  }
  catch (error) {
    console.error('处理原生文字拖放失败:', error)
  }
}

function resetSuggestionIndex() {
  activeSuggestionIndex.value = 0
}

function moveSuggestion(delta: number) {
  if (!hasCommandSuggestions.value)
    return

  const total = commandSuggestions.value.length
  activeSuggestionIndex.value = (activeSuggestionIndex.value + delta + total) % total
}

function normalizeSuggestionLookupKey(key: string): string {
  return key.trim().toLowerCase()
}

let canonicalAutoPromotionState: GhostSuggestionAutoPromotionState | null = null

function readAutoPromotionState(): GhostSuggestionAutoPromotionState {
  if (canonicalAutoPromotionState)
    return canonicalAutoPromotionState
  if (typeof localStorage === 'undefined')
    return parseGhostSuggestionAutoPromotionState(null)

  return parseGhostSuggestionAutoPromotionState(
    localStorage.getItem(GHOST_SUGGESTION_AUTO_PROMOTION_STORAGE_KEY),
  )
}

function writeAutoPromotionState(state: GhostSuggestionAutoPromotionState) {
  canonicalAutoPromotionState = state
  if (typeof localStorage === 'undefined')
    return

  localStorage.setItem(GHOST_SUGGESTION_AUTO_PROMOTION_STORAGE_KEY, JSON.stringify(state))
}

function autoPromotionStateFromUnknown(value: unknown) {
  try {
    return parseGhostSuggestionAutoPromotionState(JSON.stringify(value))
  }
  catch {
    return parseGhostSuggestionAutoPromotionState(null)
  }
}

async function syncAutoPromotionStateFromDisk() {
  const legacyState = readAutoPromotionState()
  const hasLegacyEntries = Object.keys(legacyState.entries).length > 0
  const result = hasLegacyEntries
    ? await invoke<unknown>('merge_ghost_suggestion_learning_state', { state: legacyState })
    : await invoke<unknown>('get_ghost_suggestion_learning_state')
  const payload = result && typeof result === 'object' && 'state' in result
    ? (result as { state: unknown }).state
    : result
  const state = autoPromotionStateFromUnknown(payload)
  writeAutoPromotionState(state)
  return state
}

async function recordAutoPromotionEvent(
  event: 'accepted' | 'typed',
  terms: Array<{ key: string, description?: string }>,
) {
  const result = await invoke<unknown>('record_ghost_suggestion_learning', {
    request: {
      event,
      terms,
    },
  })
  if (result && typeof result === 'object' && 'state' in result) {
    const state = autoPromotionStateFromUnknown((result as { state: unknown }).state)
    writeAutoPromotionState(state)
    return state
  }
  return readAutoPromotionState()
}

function existingGhostSuggestionKeys(): string[] {
  return allGhostSuggestions.value.map(existingSuggestion => existingSuggestion.key)
}

function mergeRuntimeCommandSuggestions(suggestions: CommandSuggestion[]): CommandSuggestion[] {
  const merged = new Map<string, CommandSuggestion>()

  suggestions.forEach((suggestion) => {
    const key = suggestion.key.trim()
    if (!key)
      return

    const lookupKey = normalizeSuggestionLookupKey(key)
    if (merged.has(lookupKey))
      return

    merged.set(lookupKey, {
      key,
      description: suggestion.description,
    })
  })

  return Array.from(merged.values())
}

function isHistoryCommandSuggestion(suggestion: CommandSuggestion): boolean {
  const lookupKey = normalizeSuggestionLookupKey(suggestion.key)
  return historyCommandSuggestions.value.some(historySuggestion =>
    normalizeSuggestionLookupKey(historySuggestion.key) === lookupKey,
  )
}

async function recordAcceptedHistoryCommandSuggestion(suggestion: CommandSuggestion) {
  if (!isHistoryCommandSuggestion(suggestion))
    return

  if (!shouldTrackGhostSuggestionAutoPromotion(suggestion.key, existingGhostSuggestionKeys()))
    return

  try {
    await recordAutoPromotionEvent('accepted', [{
      key: suggestion.key,
      description: suggestion.description,
    }])
  }
  catch (error) {
    console.warn('PopupInput: 幽灵补全接受次数落盘失败:', error)
  }
}

function recordSubmittedInputForAutoPromotion(input = userInput.value) {
  const terms = extractGhostSuggestionAutoPromotionTerms(input, existingGhostSuggestionKeys())
  if (terms.length === 0)
    return

  void invoke('timeline_debug_log', {
    location: 'frontend/ghost_auto_promotion/submitted_terms',
    payload: { term_count: terms.length },
  }).catch(() => {})
  void recordAutoPromotionEvent(
    'typed',
    terms.map(key => ({ key })),
  ).catch((error) => {
    console.warn('PopupInput: 幽灵补全输入次数落盘失败:', error)
  })
}

function applyCommandSuggestion(index = activeSuggestionIndex.value) {
  const suggestion = commandSuggestions.value[index]
  const context = activeCommandContext.value
  if (!suggestion || !context)
    return

  userInput.value = `${context.prefix}${suggestion.key}`
  acceptedSuggestionToken.value = suggestion.key
  activeSuggestionIndex.value = index
  void recordAcceptedHistoryCommandSuggestion(suggestion)
  emitUpdate()

  nextTick(() => {
    void focusInput()
  })
}

function handleInputKeydown(event: KeyboardEvent) {
  if (isComposing.value || event.isComposing || event.keyCode === 229 || event.repeat)
    return

  if (!hasCommandSuggestions.value)
    return

  if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
    event.preventDefault()
    event.stopPropagation()
    moveSuggestion(-1)
    return
  }

  if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
    event.preventDefault()
    event.stopPropagation()
    moveSuggestion(1)
    return
  }

  if (event.key === 'Enter') {
    if (!canApplyActiveSuggestion.value)
      return

    event.preventDefault()
    event.stopPropagation()
    applyCommandSuggestion()
    return
  }

  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    resetSuggestionIndex()
  }
}

function getGhostControlOptionCode(event: KeyboardEvent): 'AltLeft' | 'AltRight' | null {
  if (event.code === 'AltLeft' || event.code === 'AltRight')
    return event.code
  return null
}

function handleOptionGhostControlKeydown(event: KeyboardEvent) {
  const optionCode = getGhostControlOptionCode(event)
  if (optionCode) {
    pressedGhostControlOptionCodes.add(optionCode)
    if (
      !event.repeat
      && !event.ctrlKey
      && !event.metaKey
      && !event.shiftKey
      && pressedGhostControlOptionCodes.size === 1
    ) {
      optionGhostControlCandidate = optionCode
    }
    else {
      optionGhostControlCandidate = null
    }
    return
  }

  if (event.altKey || pressedGhostControlOptionCodes.size > 0)
    optionGhostControlCandidate = null
}

function handleOptionGhostControlKeyup(event: KeyboardEvent) {
  const optionCode = getGhostControlOptionCode(event)
  if (!optionCode)
    return

  const shouldApply = optionGhostControlCandidate === optionCode
    && pressedGhostControlOptionCodes.size === 1
    && pressedGhostControlOptionCodes.has(optionCode)
  optionGhostControlCandidate = null
  pressedGhostControlOptionCodes.delete(optionCode)
  if (!shouldApply || event.ctrlKey || event.metaKey || event.shiftKey)
    return

  const enabled = optionCode === 'AltLeft'
  setGhostSuggestionsEnabled(enabled)
  resetSuggestionIndex()
  message.success(enabled ? '幽灵补全已全局开启' : '幽灵补全已全局关闭')
}

function cancelOptionGhostControl() {
  optionGhostControlCandidate = null
  pressedGhostControlOptionCodes.clear()
}

// 移除自定义图片预览功能，改用 Naive UI 的内置预览

// 加载自定义prompt配置
async function getCachedCustomPromptConfig(forceRefresh = false): Promise<CustomPromptConfigSnapshot | null> {
  if (!forceRefresh && customPromptConfigCache) {
    return customPromptConfigCache
  }

  if (!forceRefresh && customPromptConfigPromise) {
    return customPromptConfigPromise
  }

  customPromptConfigPromise = invoke('get_custom_prompt_config')
    .then((config) => {
      customPromptConfigCache = (config as CustomPromptConfigSnapshot) || null
      return customPromptConfigCache
    })
    .finally(() => {
      customPromptConfigPromise = null
    })

  return customPromptConfigPromise
}

async function loadCustomPrompts(options: { forceRefresh?: boolean } = {}) {
  try {
    console.log('PopupInput: 开始加载自定义prompt配置')
    const config = await getCachedCustomPromptConfig(options.forceRefresh)
    if (config) {
      const promptConfig = config

      // 按sort_order排序
      customPrompts.value = applyContextPromptState(
        (promptConfig.prompts || []).sort((a: CustomPrompt, b: CustomPrompt) => a.sort_order - b.sort_order),
      )
      customPromptEnabled.value = promptConfig.enabled ?? true
      console.log('PopupInput: 加载到的prompt数量:', customPrompts.value.length)
      console.log('PopupInput: 条件性prompt列表:', customPrompts.value.filter(p => p.type === 'conditional'))

      // 同步到拖拽列表（只包含普通prompt）
      sortablePrompts.value = [...normalPrompts.value]
      console.log('PopupInput: 同步到sortablePrompts:', sortablePrompts.value.length)

      // 延迟初始化拖拽功能，等待组件完全挂载
      if (customPrompts.value.length > 0) {
        console.log('PopupInput: 准备启动拖拽功能')
        initializeDragSort()
      }
      else {
        console.log('PopupInput: 没有prompt，跳过拖拽初始化')
      }
    }
  }
  catch (error) {
    console.error('PopupInput: 加载自定义prompt失败:', error)
  }
}

function currentContextPromptState() {
  const key = props.contextKey
  if (!key)
    return null

  let state = contextPromptStateCache.get(key)
  if (!state) {
    state = {}
    contextPromptStateCache.set(key, state)
  }
  return state
}

function applyContextPromptState(prompts: CustomPrompt[]) {
  const contextState = currentContextPromptState()

  return prompts.map((prompt) => {
    const nextPrompt = { ...prompt }
    const applyPromptState = (state?: Pick<CustomPrompt, 'current_state' | 'is_active'>) => {
      if (!state)
        return
      if (state.current_state !== undefined)
        nextPrompt.current_state = state.current_state
      if (state.is_active !== undefined)
        nextPrompt.is_active = state.is_active
    }
    applyPromptState(contextState?.[prompt.id])
    applyPromptState(props.contextPromptState?.[prompt.id])
    return nextPrompt
  })
}

function rememberContextPromptState(prompt: CustomPrompt) {
  const contextState = currentContextPromptState()
  if (!contextState)
    return

  contextState[prompt.id] = {
    current_state: prompt.current_state,
    is_active: prompt.is_active,
  }
}

async function loadHistoryCommandSuggestions() {
  try {
    const [suggestions, autoPromotionState] = await Promise.all([
      invoke('get_hui_suggestion_terms', {
        projectPath: props.request?.project_path ?? null,
      }) as Promise<CommandSuggestion[]>,
      syncAutoPromotionStateFromDisk().catch(() => readAutoPromotionState()),
    ])

    if (!Array.isArray(suggestions)) {
      historyCommandSuggestions.value = []
      return
    }

    const huiSuggestions = suggestions
      .filter(suggestion => typeof suggestion?.key === 'string' && suggestion.key.trim().length > 0)
      .map(suggestion => ({
        key: suggestion.key.trim(),
        description: suggestion.description || 'hui 高频词',
      }))
    const autoPromotionSuggestions = getGhostSuggestionAutoPromotionCandidates(
      autoPromotionState,
      existingGhostSuggestionKeys(),
    )

    historyCommandSuggestions.value = mergeRuntimeCommandSuggestions([
      ...huiSuggestions,
      ...autoPromotionSuggestions,
    ])
  }
  catch (error) {
    console.error('PopupInput: 加载 hui 高频词失败:', error)
    historyCommandSuggestions.value = []
  }
}

// 处理自定义prompt点击
function handlePromptClick(prompt: CustomPrompt) {
  // 如果prompt内容为空或只有空格，直接清空输入框
  if (!prompt.content || prompt.content.trim() === '') {
    userInput.value = ''
    emitUpdate()
    return
  }

  insertPromptContent(prompt.content, 'append')
}

// 处理提示词库结果点击
function handleLibraryPromptClick(content: string) {
  if (userInput.value.trim()) {
    pendingPromptContent.value = content
    showInsertDialog.value = true
  }
  else {
    insertPromptContent(content)
  }
  promptLibrary.isSearchOpen.value = false
  promptLibrary.searchQuery.value = ''
}

// 处理提示词库导入
async function handleLibraryImport(event: Event) {
  const input = event.target as HTMLInputElement
  if (!input.files?.length)
    return
  const result = await promptLibrary.importFiles(input.files)
  message.success(`导入 ${result.imported} 条，跳过 ${result.skipped} 条${result.failedFiles.length ? `，失败: ${result.failedFiles.join(', ')}` : ''}`)
  input.value = ''
}

// 添加新提示词到库
function handleAddLibraryItem() {
  if (!newLibraryItem.value.name.trim() || !newLibraryItem.value.content.trim()) {
    message.warning('名称和内容不能为空')
    return
  }
  const item = promptLibrary.addItem(
    newLibraryItem.value.name,
    newLibraryItem.value.content,
    newLibraryItem.value.category || selectedCategory.value,
  )
  if (item) {
    message.success('提示词已添加')
    newLibraryItem.value = { name: '', content: '', category: '' }
    showAddPromptForm.value = false
  }
  else {
    message.warning('添加失败（可能重复）')
  }
}

// 保存编辑的提示词
function handleSaveLibraryEdit() {
  if (!editingLibraryItem.value)
    return
  const ok = promptLibrary.updateItem(editingLibraryItem.value.id, {
    name: editingLibraryItem.value.name,
    content: editingLibraryItem.value.content,
    category: editingLibraryItem.value.category,
  })
  if (ok) {
    message.success('已更新')
    editingLibraryItem.value = null
  }
}

// 删除提示词
function handleDeleteLibraryItem(id: string) {
  if (promptLibrary.deleteItem(id)) {
    message.success('已删除')
  }
}

// 处理引用消息内容
function handleQuoteMessage(messageContent: string) {
  if (userInput.value.trim()) {
    // 输入框有内容，显示插入选择对话框
    pendingPromptContent.value = messageContent
    showInsertDialog.value = true
  }
  else {
    // 输入框为空，直接插入
    insertPromptContent(messageContent)
    message.success('原文内容已引用到输入框')
  }
}

function currentRequestId() {
  return props.request?.id?.trim() || ''
}

function currentProjectPath() {
  return props.request?.project_path?.trim() || undefined
}

function suppressProgrammaticSpeechTargetFocus() {
  suppressSpeechTargetFocusUntil = Date.now() + 350
}

function shouldSuppressSpeechTargetFocus() {
  return Date.now() < suppressSpeechTargetFocusUntil
}

function handleSpeechTargetFocus() {
  if (shouldSuppressSpeechTargetFocus())
    return
  void registerPopupSpeechTarget('focus')
}

function handleSpeechTargetClick() {
  suppressSpeechTargetFocusUntil = 0
  void registerPopupSpeechTarget('click')
}

async function registerPopupSpeechTarget(reason: string) {
  const requestId = currentRequestId()
  if (!requestId || props.loading || props.submitting)
    return

  try {
    const webview = getCurrentWebviewWindow()
    const windowLabel = webview.label
    await invoke('register_popup_speech_target', {
      windowLabel,
      requestId,
      reason,
      projectPath: currentProjectPath(),
    })
    registeredSpeechTargetRequestId = requestId
    speechInsertGuard.activateLease(requestId, windowLabel)
    console.log(`[PopupInput] 已注册语音输入目标: ${reason}`)
  }
  catch (error) {
    console.error('[PopupInput] 注册语音输入目标失败:', error)
  }
}

async function unregisterPopupSpeechTarget(requestId = registeredSpeechTargetRequestId) {
  if (!requestId)
    return

  try {
    await invoke('unregister_popup_speech_target', { requestId })
    if (registeredSpeechTargetRequestId === requestId)
      registeredSpeechTargetRequestId = null
    if (!registeredSpeechTargetRequestId)
      speechInsertGuard.invalidateLease()
    console.log('[PopupInput] 已注销语音输入目标')
  }
  catch (error) {
    console.error('[PopupInput] 注销语音输入目标失败:', error)
  }
}

async function insertTextAtCursor(text: string): Promise<boolean> {
  if (!text)
    return false

  const inputElement = getTextareaElement()
  const currentValue = userInput.value
  const start = inputElement?.selectionStart ?? currentValue.length
  const end = inputElement?.selectionEnd ?? start
  userInput.value = `${currentValue.slice(0, start)}${text}${currentValue.slice(end)}`
  emitUpdate()

  await nextTick()
  const nextCursor = start + text.length
  const refreshedInput = getTextareaElement()
  if (refreshedInput) {
    refreshedInput.focus()
    refreshedInput.setSelectionRange(nextCursor, nextCursor)
  }

  return true
}

async function insertSelectedTextQuote(selection: PopupTextSelection | null | undefined): Promise<boolean> {
  const quoteBlock = buildSelectedTextQuoteBlock(selection)
  if (!quoteBlock)
    return false

  return insertTextAtCursor(`${quoteBlock}\n\n`)
}

async function acknowledgeSpeechInsert(payload: SpeechInsertPayload) {
  await invoke('record_popup_speech_insert_result', {
    identity: payload.identity,
    requestId: payload.request_id,
    windowLabel: payload.window_label,
    insertId: payload.insert_id,
    textLen: Array.from(payload.text).length,
  }).catch((error) => {
    console.error('[PopupInput] 记录语音插入结果失败:', error)
  })
}

async function hasAuthenticatedPopupIpcAuthority(payload: SpeechInsertPayload): Promise<boolean> {
  return invoke<boolean>('authorize_popup_speech_insert', {
    identity: payload.identity,
    requestId: payload.request_id,
    windowLabel: payload.window_label,
    insertId: payload.insert_id,
    textLen: Array.from(payload.text).length,
  }).catch((error) => {
    console.error('[PopupInput] 验证跨进程语音插入授权失败:', error)
    return false
  })
}

async function applySpeechInsertText(payload: SpeechInsertPayload) {
  let authority: SpeechInsertAuthority = 'local-session'
  let decision = speechInsertGuard.classify(payload, authority)
  if (decision === 'reject'
    && speechInsertGuard.rejectionReason(payload, authority) === 'identity-mismatch'
    && await hasAuthenticatedPopupIpcAuthority(payload)) {
    authority = 'authenticated-ipc'
    decision = speechInsertGuard.classify(payload, authority)
  }
  if (decision === 'reject') {
    console.warn('[PopupInput] 拒绝语音插入:', {
      reason: speechInsertGuard.rejectionReason(payload, authority),
      authority,
      requestId: payload.request_id,
      windowLabel: payload.window_label,
      insertId: payload.insert_id,
    })
    return
  }
  if (decision === 'ignore')
    return
  if (decision === 'acknowledge') {
    await acknowledgeSpeechInsert(payload)
    return
  }

  const text = String(payload.text || '')
  const inserted = await insertTextAtCursor(text)
  if (!inserted) {
    speechInsertGuard.release(payload.insert_id)
    return
  }

  speechInsertGuard.markApplied(payload.insert_id)
  await acknowledgeSpeechInsert(payload)
}

async function focusInput(options: { registerSpeechTarget?: boolean } = {}) {
  const registerSpeechTarget = options.registerSpeechTarget ?? true
  if (!registerSpeechTarget)
    suppressProgrammaticSpeechTargetFocus()

  await nextTick()

  if (!textareaRef.value)
    return

  try {
    const webview = getCurrentWebviewWindow()
    await webview.setFocus()

    const inputElement = getTextareaElement()

    if (!inputElement)
      return

    const scrollableAncestors: Array<{ element: HTMLElement, top: number, left: number }> = []
    let currentParent = inputElement.parentElement

    while (currentParent) {
      const hasScrollableContent = currentParent.scrollHeight > currentParent.clientHeight
        || currentParent.scrollWidth > currentParent.clientWidth
      if (hasScrollableContent) {
        scrollableAncestors.push({
          element: currentParent,
          top: currentParent.scrollTop,
          left: currentParent.scrollLeft,
        })
      }
      currentParent = currentParent.parentElement
    }

    if (typeof inputElement.focus === 'function') {
      try {
        inputElement.focus({ preventScroll: true })
      }
      catch {
        inputElement.focus()
      }
    }

    if (typeof inputElement.setSelectionRange === 'function') {
      const cursorPosition = typeof inputElement.value === 'string' ? inputElement.value.length : 0
      inputElement.setSelectionRange(cursorPosition, cursorPosition)
    }

    if (scrollableAncestors.length > 0) {
      const restoreScrollPosition = () => {
        scrollableAncestors.forEach(({ element, top, left }) => {
          if (element.scrollTop !== top)
            element.scrollTop = top
          if (element.scrollLeft !== left)
            element.scrollLeft = left
        })
      }

      restoreScrollPosition()

      if (typeof window.requestAnimationFrame === 'function') {
        window.requestAnimationFrame(() => {
          restoreScrollPosition()
        })
      }

      setTimeout(() => {
        restoreScrollPosition()
      }, 0)
    }

    if (registerSpeechTarget)
      void registerPopupSpeechTarget('focus-input')
  }
  catch (error) {
    console.log('设置光标位置失败:', error)
  }
}

function getTextareaElement(): HTMLTextAreaElement | null {
  return textareaRef.value
}

function handleTextareaScroll(event: Event) {
  const inputElement = event.currentTarget as HTMLTextAreaElement | null
  textareaIsScrolled.value = (inputElement?.scrollTop ?? 0) > 0
}

function syncGhostMetrics() {
  const inputElement = getTextareaElement()
  if (!inputElement || typeof window === 'undefined')
    return

  const styles = window.getComputedStyle(inputElement)
  const nextMetricsStyle = [
    `--ghost-padding-top: ${styles.paddingTop}`,
    `--ghost-padding-right: ${styles.paddingRight}`,
    `--ghost-padding-bottom: ${styles.paddingBottom}`,
    `--ghost-padding-left: ${styles.paddingLeft}`,
    `--ghost-font-size: ${styles.fontSize}`,
    `--ghost-line-height: ${styles.lineHeight}`,
    `--ghost-font-family: ${styles.fontFamily}`,
    `--ghost-letter-spacing: ${styles.letterSpacing}`,
  ].join('; ')

  if (ghostMetricsStyle.value !== nextMetricsStyle)
    ghostMetricsStyle.value = nextMetricsStyle
}

function scheduleGhostMetricsSync() {
  if (typeof window === 'undefined')
    return

  if (ghostMetricsFrame !== null)
    return

  const run = () => {
    ghostMetricsFrame = null
    syncGhostMetrics()
  }

  if (typeof window.requestAnimationFrame !== 'function') {
    run()
    return
  }

  ghostMetricsFrame = window.requestAnimationFrame(run)
}

function cancelGhostMetricsSync() {
  if (ghostMetricsFrame !== null && typeof window !== 'undefined' && typeof window.cancelAnimationFrame === 'function')
    window.cancelAnimationFrame(ghostMetricsFrame)

  ghostMetricsFrame = null
}

function handleGhostMetricsResize() {
  scheduleGhostMetricsSync()
  scheduleTextareaAutosize(0)
}

function resizeTextareaNow() {
  const inputElement = getTextareaElement()
  if (!inputElement)
    return

  inputElement.style.height = 'auto'
  inputElement.style.height = `${Math.ceil(inputElement.scrollHeight + 2)}px`
  textareaIsScrolled.value = inputElement.scrollHeight > inputElement.clientHeight + 1
}

function scheduleTextareaAutosize(delay = TEXTAREA_AUTOSIZE_IDLE_MS) {
  if (isComposing.value || typeof window === 'undefined')
    return

  if (textareaAutosizeTimer)
    clearTimeout(textareaAutosizeTimer)
  if (textareaAutosizeFrame !== null && typeof window.cancelAnimationFrame === 'function')
    window.cancelAnimationFrame(textareaAutosizeFrame)

  textareaAutosizeFrame = null
  textareaAutosizeTimer = setTimeout(() => {
    textareaAutosizeTimer = null
    const run = () => {
      textareaAutosizeFrame = null
      resizeTextareaNow()
    }

    if (typeof window.requestAnimationFrame === 'function')
      textareaAutosizeFrame = window.requestAnimationFrame(run)
    else
      run()
  }, delay)
}

function cancelTextareaAutosize() {
  if (textareaAutosizeTimer)
    clearTimeout(textareaAutosizeTimer)
  if (textareaAutosizeFrame !== null && typeof window !== 'undefined' && typeof window.cancelAnimationFrame === 'function')
    window.cancelAnimationFrame(textareaAutosizeFrame)

  textareaAutosizeTimer = null
  textareaAutosizeFrame = null
}

function handleCompositionStart() {
  isComposing.value = true
  cancelTextareaAutosize()
}

function handleCompositionEnd() {
  isComposing.value = false
  void nextTick(() => {
    scheduleTextareaAutosize(0)
  })
}

function clearLocalFocusTimer() {
  if (localFocusTimer) {
    clearTimeout(localFocusTimer)
    localFocusTimer = null
  }
  if (localFocusFrame !== null && typeof window !== 'undefined' && typeof window.cancelAnimationFrame === 'function') {
    window.cancelAnimationFrame(localFocusFrame)
    localFocusFrame = null
  }
}

function scheduleTextareaFocus(reason: string) {
  if (props.loading || props.submitting)
    return

  clearLocalFocusTimer()

  void focusInput({ registerSpeechTarget: false })
  scheduleGhostMetricsSync()

  if (typeof window.requestAnimationFrame === 'function') {
    localFocusFrame = window.requestAnimationFrame(() => {
      localFocusFrame = null
      void focusInput({ registerSpeechTarget: false })
      scheduleGhostMetricsSync()
    })
  }

  localFocusTimer = setTimeout(() => {
    void focusInput({ registerSpeechTarget: false })
    scheduleGhostMetricsSync()
    localFocusTimer = null
  }, 180)

  console.log(`[PopupInput] 已调度 textarea 聚焦: ${reason}`)
}

// 插入prompt内容
function insertPromptContent(content: string, mode: 'replace' | 'append' = 'replace') {
  if (mode === 'replace') {
    userInput.value = content
  }
  else {
    const separator = !userInput.value
      || userInput.value.endsWith('\n')
      || userInput.value.endsWith(' ')
      || content.startsWith('\n')
      || content.startsWith(' ')
      ? ''
      : '\n'
    userInput.value = `${userInput.value}${separator}${content}`
  }

  // 聚焦到输入框
  setTimeout(() => {
    void focusInput()
  }, 100)

  emitUpdate()
}

// 处理插入模式选择
function handleInsertMode(mode: 'replace' | 'append') {
  insertPromptContent(pendingPromptContent.value, mode)
  showInsertDialog.value = false
  pendingPromptContent.value = ''
}

// 插入对话框 Enter 键快捷确认（追加内容）
function handleInsertDialogKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter') {
    event.preventDefault()
    event.stopPropagation()
    handleInsertMode('append')
  }
}

watch(showInsertDialog, (visible) => {
  if (visible) {
    window.addEventListener('keydown', handleInsertDialogKeydown)
  }
  else {
    window.removeEventListener('keydown', handleInsertDialogKeydown)
  }
})

// 处理条件性prompt激活状态变化
function handleConditionalActiveToggle(promptId: string, value: boolean) {
  const prompt = customPrompts.value.find(p => p.id === promptId)
  if (prompt) {
    prompt.is_active = value
    rememberContextPromptState(prompt)
    emit('conditionalStateChange', { promptId, is_active: value })
    emitUpdate()
  }
}

// 处理条件性prompt开关变化
function handleConditionalToggle(promptId: string, value: boolean) {
  // 先更新本地状态
  const prompt = customPrompts.value.find(p => p.id === promptId)
  if (prompt) {
    prompt.current_state = value
    // 如果当前未激活，自动激活当前窗口上下文，不写全局配置
    if (prompt.is_active === false) {
      prompt.is_active = true
    }
    rememberContextPromptState(prompt)
    emit('conditionalStateChange', {
      promptId,
      current_state: value,
      is_active: prompt.is_active,
    })
    emitUpdate()
  }
}

// 生成条件性prompt的追加内容
function generateConditionalContent(): string {
  const conditionalTexts: string[] = []

  conditionalPrompts.value.forEach((prompt) => {
    // 只有在 is_active 为 true 时才追加内容
    if (prompt.is_active !== false) {
      const isEnabled = prompt.current_state ?? false
      const template = isEnabled ? prompt.template_true : prompt.template_false

      if (template && template.trim()) {
        conditionalTexts.push(template.trim())
      }
    }
  })

  return conditionalTexts.length > 0
    ? `\n\n${CONTEXT_INJECTION_START}\n${conditionalTexts.join('\n')}`
    : ''
}

// 获取条件性prompt的自适应描述
function getConditionalDescription(prompt: CustomPrompt): string {
  const isEnabled = prompt.current_state ?? false
  const template = isEnabled ? prompt.template_true : prompt.template_false

  // 如果有对应状态的模板，显示模板内容，否则显示原始描述
  if (template && template.trim()) {
    return template.trim()
  }

  return prompt.description || ''
}

// 初始化拖拽排序功能
async function initializeDragSort() {
  console.log('PopupInput: initializeDragSort 被调用')

  // 等待 DOM 渲染
  await nextTick()
  await nextTick()

  setTimeout(() => {
    // 先销毁旧实例，再重新创建（解决 useSortable 的 start() 幂等保护问题）
    stop()

    if (promptContainer.value) {
      console.log('PopupInput: 容器已就绪，启动拖拽')
      start()
    }
    else {
      console.log('PopupInput: 容器未找到，DOM 可能还没渲染')
    }
  }, 300)
}

// 保存prompt排序
async function savePromptOrder() {
  try {
    console.log('savePromptOrder被调用')
    console.log('当前sortablePrompts:', sortablePrompts.value.map(p => ({ id: p.id, name: p.name })))
    const promptIds = sortablePrompts.value.map(p => p.id)
    console.log('开始保存排序，prompt IDs:', promptIds)

    const startTime = Date.now()
    await invoke('update_custom_prompt_order', { promptIds })
    const endTime = Date.now()

    console.log(`排序已保存，耗时: ${endTime - startTime}ms`)
    message.success('排序已保存')
  }
  catch (error) {
    console.error('保存排序失败:', error)
    message.error('保存排序失败')
    // 重新加载以恢复原始顺序
    loadCustomPrompts()
  }
}

// 监听用户输入变化
watch(userInput, (newVal) => {
  if (activeCommandToken.value !== acceptedSuggestionToken.value) {
    acceptedSuggestionToken.value = ''
  }

  resetSuggestionIndex()
  emitUpdate()
  if (!isComposing.value)
    scheduleTextareaAutosize()
  if (newVal.endsWith('@')) {
    userInput.value = newVal.slice(0, -1)
    emit('atTrigger')
  }
  else if (newVal.endsWith('爱特')) {
    userInput.value = newVal.slice(0, -2)
    emit('atTrigger')
  }
})

// 移除拖拽相关的监听器

// 事件监听器引用
let unlistenCustomPromptUpdate: (() => void) | null = null
let unlistenWindowMove: (() => void) | null = null

// 修复输入法候选框位置的函数
function fixIMEPosition() {
  const inputElement = getTextareaElement()
  if (inputElement) {
    try {
      if (document.activeElement === inputElement) {
        // 先失焦再聚焦，让输入法重新计算位置
        inputElement.blur()
        setTimeout(() => {
          inputElement.focus()
        }, 10)
      }
    }
    catch (error) {
      console.debug('修复IME位置失败:', error)
    }
  }
}

// 设置窗口移动监听器
async function setupWindowMoveListener() {
  try {
    const webview = getCurrentWebviewWindow()

    // 监听窗口移动事件
    unlistenWindowMove = await webview.onMoved(() => {
      // 窗口移动后修复输入法位置
      fixIMEPosition()
    })

    console.log('窗口移动监听器已设置')
  }
  catch (error) {
    console.error('设置窗口移动监听器失败:', error)
  }
}

// 截图事件监听器
let unlistenScreenshot: (() => void) | null = null

// Cmd+1~9 快捷键选择预定义选项
function handleOptionShortcut(event: KeyboardEvent) {
  if (!(event.metaKey || event.ctrlKey) || event.shiftKey || event.altKey)
    return
  const key = event.key
  if (key >= '1' && key <= '9') {
    const options = props.request?.predefined_options
    if (!options || options.length === 0)
      return
    const index = Number.parseInt(key) - 1
    if (index < options.length) {
      event.preventDefault()
      event.stopPropagation()
      handleOptionToggle(options[index])
    }
  }
}

function clearPromptShortcutFeedback() {
  if (promptShortcutFeedbackTimer) {
    clearTimeout(promptShortcutFeedbackTimer)
    promptShortcutFeedbackTimer = null
  }
  activePromptShortcutIndex.value = null
}

function handlePromptShortcutModifierKeydown(event: KeyboardEvent) {
  if (event.key === 'Alt' || event.altKey)
    isPromptShortcutModifierHeld.value = true
}

function handlePromptShortcutModifierKeyup(event: KeyboardEvent) {
  if (event.key === 'Alt' || !event.altKey)
    isPromptShortcutModifierHeld.value = false
}

function clearPromptShortcutModifier() {
  isPromptShortcutModifierHeld.value = false
}

function handlePromptShortcutVisibilityChange() {
  if (document.hidden)
    clearPromptShortcutModifier()
}

// Option+1~9 选择当前排序中的快捷模板
function handlePromptShortcut(event: KeyboardEvent) {
  const index = getPromptShortcutIndex(event)
  if (
    index < 0
    || props.loading
    || props.submitting
    || showInsertDialog.value
    || !customPromptEnabled.value
  ) {
    return
  }

  const prompt = sortablePrompts.value[index]
  if (!prompt)
    return

  event.preventDefault()
  event.stopPropagation()

  clearPromptShortcutFeedback()
  activePromptShortcutIndex.value = index
  promptShortcutFeedbackTimer = setTimeout(() => {
    activePromptShortcutIndex.value = null
    promptShortcutFeedbackTimer = null
  }, 120)

  handlePromptClick(prompt)
}

async function initializeAsyncListeners() {
  try {
    const [customPromptUpdateResult, configChangedResult, screenshotResult, speechInsertResult, speechSnapshotResult] = await Promise.allSettled([
      listen('custom-prompt-updated', () => {
        console.log('收到自定义prompt更新事件（前端），重新加载数据')
        customPromptConfigCache = null
        void loadCustomPrompts({ forceRefresh: true })
      }),
      listen('custom-prompt-config-changed', () => {
        console.log('收到后端配置变更事件，重新加载数据')
        customPromptConfigCache = null
        void loadCustomPrompts({ forceRefresh: true })
      }),
      listen<string>('screenshot-captured', (event) => {
        console.log('收到截图事件，图片数据长度:', event.payload.length)
        if (event.payload && !uploadedImages.value.includes(event.payload)) {
          uploadedImages.value.push(event.payload)
          message.success('截图已添加')
          emitUpdate()
          nextTick(() => {
            textareaRef.value?.focus()
          })
        }
      }),
      listen<SpeechInsertPayload>('speech://insert-text', (event) => {
        void applySpeechInsertText(event.payload)
      }),
      listen<SpeechSnapshot>('speech://session-snapshot', (event) => {
        speechInsertGuard.updateSnapshot(event.payload)
      }),
    ])

    if (customPromptUpdateResult.status === 'fulfilled') {
      const frontendUnlisten = customPromptUpdateResult.value
      const configUnlisten = configChangedResult.status === 'fulfilled'
        ? configChangedResult.value
        : null

      unlistenCustomPromptUpdate = () => {
        frontendUnlisten()
        configUnlisten?.()
      }
    }
    else {
      console.error('注册 custom-prompt-updated 监听失败:', customPromptUpdateResult.reason)
    }

    if (configChangedResult.status === 'rejected') {
      console.error('注册 custom-prompt-config-changed 监听失败:', configChangedResult.reason)
    }

    if (screenshotResult.status === 'fulfilled') {
      unlistenScreenshot = screenshotResult.value
    }
    else {
      console.error('注册 screenshot-captured 监听失败:', screenshotResult.reason)
    }

    if (speechInsertResult.status === 'fulfilled') {
      unlistenSpeechInsert = speechInsertResult.value
    }
    else {
      console.error('注册 speech://insert-text 监听失败:', speechInsertResult.reason)
    }
    if (speechSnapshotResult.status === 'fulfilled') {
      unlistenSpeechSnapshot = speechSnapshotResult.value
      const snapshot = await invoke<SpeechSnapshot>('get_speech_control_snapshot').catch(() => null)
      if (snapshot)
        speechInsertGuard.updateSnapshot(snapshot)
    }
    else {
      console.error('注册 speech://session-snapshot 监听失败:', speechSnapshotResult.reason)
    }
  }
  catch (error) {
    console.error('初始化异步监听器失败:', error)
  }
}

async function setupInputDragDropListener() {
  try {
    const webview = getCurrentWebview()
    unlistenDragDrop = await webview.onDragDropEvent(async (event) => {
      const payload = event.payload

      if (payload.type === 'enter' || payload.type === 'over') {
        isInputDragOver.value = true
        return
      }

      if (payload.type === 'leave') {
        isInputDragOver.value = false
        return
      }

      if (payload.type === 'drop') {
        isInputDragOver.value = false
        await addAttachmentPaths(payload.paths)
      }
    })
  }
  catch (error) {
    console.error('注册文件拖拽监听失败:', error)
  }
}

async function setupNativeTextDropListener() {
  try {
    unlistenNativeTextDrop = await listen<NativeTextDropPayload>('popup://native-text-drop', (event) => {
      void handleNativeTextDrop(event.payload)
    })
  }
  catch (error) {
    console.error('注册原生文字拖放监听失败:', error)
  }
}

// 组件挂载时加载自定义prompt
onMounted(() => {
  console.log('组件挂载，开始加载prompt')
  scheduleTextareaFocus('mounted')
  nextTick(() => {
    scheduleGhostMetricsSync()
    scheduleTextareaAutosize(0)
  })
  void loadCustomPrompts()
  loadGhostSuggestions()
  void loadHistoryCommandSuggestions()
  void initializeAsyncListeners()
  void setupInputDragDropListener()
  void setupNativeTextDropListener()

  // 设置窗口移动监听器
  void setupWindowMoveListener()

  // 注册 Cmd+1~9 快捷键监听
  window.addEventListener('keydown', handleOptionShortcut)
  window.addEventListener('keydown', handleOptionGhostControlKeydown)
  window.addEventListener('keydown', handlePromptShortcutModifierKeydown)
  window.addEventListener('keydown', handlePromptShortcut)
  window.addEventListener('keyup', handleOptionGhostControlKeyup)
  window.addEventListener('keyup', handlePromptShortcutModifierKeyup)
  window.addEventListener('pointerdown', cancelOptionGhostControl)
  window.addEventListener('blur', clearPromptShortcutModifier)
  window.addEventListener('blur', cancelOptionGhostControl)
  document.addEventListener('visibilitychange', handlePromptShortcutVisibilityChange)
  window.addEventListener('resize', handleGhostMetricsResize)
})

onUnmounted(() => {
  clearLocalFocusTimer()
  clearPromptShortcutFeedback()
  cancelGhostMetricsSync()
  cancelTextareaAutosize()
  void unregisterPopupSpeechTarget()
  // 清理事件监听器
  if (unlistenCustomPromptUpdate) {
    unlistenCustomPromptUpdate()
  }

  // 清理截图事件监听器
  if (unlistenScreenshot) {
    unlistenScreenshot()
  }

  if (unlistenSpeechInsert) {
    unlistenSpeechInsert()
  }
  if (unlistenSpeechSnapshot) {
    unlistenSpeechSnapshot()
  }
  speechInsertGuard.invalidateLease()

  // 清理窗口移动监听器
  if (unlistenWindowMove) {
    unlistenWindowMove()
  }
  if (unlistenDragDrop) {
    unlistenDragDrop()
  }
  if (unlistenNativeTextDrop) {
    unlistenNativeTextDrop()
  }

  // 清理 Cmd+1~9 快捷键监听
  window.removeEventListener('keydown', handleOptionShortcut)
  window.removeEventListener('keydown', handleOptionGhostControlKeydown)
  window.removeEventListener('keydown', handlePromptShortcutModifierKeydown)
  window.removeEventListener('keydown', handlePromptShortcut)
  window.removeEventListener('keyup', handleOptionGhostControlKeyup)
  window.removeEventListener('keyup', handlePromptShortcutModifierKeyup)
  window.removeEventListener('pointerdown', cancelOptionGhostControl)
  window.removeEventListener('blur', clearPromptShortcutModifier)
  window.removeEventListener('blur', cancelOptionGhostControl)
  document.removeEventListener('visibilitychange', handlePromptShortcutVisibilityChange)
  window.removeEventListener('resize', handleGhostMetricsResize)

  // 停止拖拽功能
  stop()
})

watch(() => props.loading, (loading) => {
  if (!loading) {
    scheduleTextareaFocus('loading-finished')
  }
})

watch(() => props.request?.id, (requestId, previousRequestId) => {
  if (previousRequestId)
    void unregisterPopupSpeechTarget(previousRequestId)
  if (requestId) {
    scheduleTextareaFocus(`request-changed:${requestId}`)
  }
})

watch(() => props.submitting, (submitting) => {
  if (submitting)
    void unregisterPopupSpeechTarget()
})

watch(() => props.contextKey, () => {
  void loadCustomPrompts({ forceRefresh: true })
})

watch(() => props.contextPromptState, () => {
  void loadCustomPrompts()
}, { deep: true })

watch(() => props.enableContextAppend, () => {
  emitUpdate()
})

watch(() => props.request?.project_path, () => {
  void loadHistoryCommandSuggestions()
})

watch(commandSuggestions, (suggestions) => {
  if (suggestions.length === 0) {
    activeSuggestionIndex.value = 0
    return
  }

  if (activeSuggestionIndex.value >= suggestions.length) {
    activeSuggestionIndex.value = 0
  }
})

// 重置数据
function reset() {
  userInput.value = ''
  selectedOptions.value = []
  uploadedImages.value = []
  attachedFiles.value = []
  emitUpdate()
}

// 更新数据（用于外部同步）
function updateData(data: PopupInputData) {
  if (data.userInput !== undefined) {
    userInput.value = data.userInput
  }
  if (data.selectedOptions !== undefined) {
    selectedOptions.value = data.selectedOptions
  }
  if (data.draggedImages !== undefined) {
    uploadedImages.value = data.draggedImages
  }
  if (data.attachedFiles !== undefined) {
    attachedFiles.value = data.attachedFiles
  }

  emitUpdate()
}

// 移除了文件选择和测试图片功能

// 暴露方法给父组件
defineExpose({
  reset,
  canSubmit,
  statusText,
  updateData,
  recordSubmittedInputForAutoPromotion,
  focusInput,
  handleQuoteMessage,
  insertSelectedTextQuote,
  textareaRef, // 暴露引用以便父组件访问
})
</script>

<template>
  <div class="space-y-3">
    <!-- 文本输入框 - 放在最上面，方便直接输入 -->
    <div v-if="!loading" class="space-y-2">
      <div
        class="popup-main-input-shell"
        :class="{ 'popup-main-input-shell--dragover': isInputDragOver }"
        :style="ghostMetricsStyle"
        @dragover="handleInputDragOver"
        @dragleave="handleInputDragLeave"
        @drop="handleInputDrop"
      >
        <div
          v-if="shouldShowGhostSuggestion"
          class="popup-main-input-ghost"
          aria-hidden="true"
        >
          <span v-if="ghostPrefixText" class="popup-main-input-ghost__typed">{{ ghostPrefixText }}</span><span class="popup-main-input-ghost__typed">{{ activeCommandToken }}</span><span class="popup-main-input-ghost__suffix">{{ previewSuffix }}</span>
        </div>
        <textarea
          ref="textareaRef"
          v-model="userInput"
          class="popup-main-input"
          :class="{ 'popup-main-input--ghosting': shouldShowGhostSuggestion }"
          :placeholder="hasOptions ? `补充说明；输入“结束对话”或 /end 可结束本次交互（支持粘贴 ${pasteShortcut}）` : `请输入回复；输入“结束对话”或 /end 可结束本次交互（支持粘贴 ${pasteShortcut}）`"
          :disabled="submitting"
          rows="3"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
          data-guide="popup-input"
          @paste="handleInputPaste"
          @focus="handleSpeechTargetFocus"
          @click="handleSpeechTargetClick"
          @keydown="handleInputKeydown"
          @scroll="handleTextareaScroll"
          @compositionstart="handleCompositionStart"
          @compositionend="handleCompositionEnd"
        />
      </div>
    </div>

    <!-- 预定义选项 - 在输入框下面 -->
    <div v-if="!loading && hasOptions" class="space-y-3" data-guide="predefined-options">
      <h4 class="text-sm font-medium text-white">
        请选择选项
      </h4>
      <n-space vertical size="small">
        <div
          v-for="(option, index) in request!.predefined_options"
          :key="`option-${index}`"
          class="rounded-lg p-3 border border-gray-600 bg-container-secondary cursor-pointer transition-all duration-200"
          @click="handleOptionToggle(option)"
        >
          <div class="flex items-center justify-between w-full">
            <n-checkbox
              :value="option"
              :checked="selectedOptions.includes(option)"
              :disabled="submitting"
              size="medium"
              @update:checked="(checked: boolean) => handleOptionChange(option, checked)"
              @click.stop
            >
              {{ option }}
            </n-checkbox>
            <span v-if="index < 9" class="text-[10px] text-gray-500 ml-2 flex-shrink-0">⌘{{ index + 1 }}</span>
          </div>
        </div>
      </n-space>
    </div>

    <!-- 附件预览区域 -->
    <div v-if="!loading && (uploadedImages.length > 0 || attachedFiles.length > 0)" class="space-y-3">
      <h4 class="text-sm font-medium text-white">
        已添加的内容 ({{ uploadedImages.length + attachedFiles.length }})
      </h4>

      <!-- 使用 Naive UI 的图片组件，支持预览和放大 -->
      <n-image-group v-if="uploadedImages.length > 0">
        <div class="flex flex-wrap gap-3">
          <div
            v-for="(image, index) in uploadedImages"
            :key="`image-${index}`"
            class="relative"
          >
            <!-- 使用 n-image 组件，启用预览功能 -->
            <n-image
              :src="image"
              width="100"
              height="100"
              object-fit="cover"
              class="rounded-lg border-2 border-gray-300 hover:border-primary-400 transition-all duration-200 cursor-pointer"
            />

            <!-- 删除按钮 -->
            <n-button
              class="absolute -top-2 -right-2 z-10"
              size="tiny"
              type="error"
              circle
              @click.stop="removeImage(index)"
            >
              <template #icon>
                <div class="i-carbon-close w-3 h-3" />
              </template>
            </n-button>

            <!-- 序号 -->
            <div class="absolute bottom-1 left-1 w-5 h-5 bg-primary-500 text-white text-xs rounded-full flex items-center justify-center font-bold shadow-sm z-5">
              {{ index + 1 }}
            </div>
          </div>
        </div>
      </n-image-group>

      <div v-if="attachedFiles.length > 0" class="space-y-2">
        <div
          v-for="(file, index) in attachedFiles"
          :key="`${file.path}-${index}`"
          class="flex items-center gap-3 rounded-lg border border-gray-600 bg-container-secondary px-3 py-2"
        >
          <div class="flex h-9 w-9 items-center justify-center rounded-md bg-black/20 text-blue-300">
            <div class="i-carbon-document w-4 h-4" />
          </div>
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm text-white">
              {{ file.name }}
            </div>
            <div class="truncate text-[11px] text-on-surface-secondary">
              {{ file.path }}
            </div>
          </div>
          <n-button
            size="tiny"
            type="error"
            quaternary
            circle
            @click.stop="removeFile(index)"
          >
            <template #icon>
              <div class="i-carbon-close w-3 h-3" />
            </template>
          </n-button>
        </div>
      </div>
    </div>

    <!-- 补充说明区域 - 放在最后一排 -->
    <div v-if="!loading" class="space-y-3">
      <h4 v-if="hasOptions" class="text-sm font-medium text-white">
        补充说明 (可选)
      </h4>

      <!-- 自定义prompt按钮区域 -->
      <div v-if="customPromptEnabled && customPrompts.length > 0" class="space-y-2" data-guide="custom-prompts">
        <div class="text-xs text-on-surface-secondary flex items-center gap-2">
          <div class="flex items-center gap-1.5 flex-shrink-0">
            <div class="i-carbon-bookmark w-3 h-3 text-primary-500" />
            <span>快捷模板:</span>
          </div>
          <input
            v-model="promptLibrary.searchQuery.value"
            type="text"
            placeholder="搜索提示词..."
            class="flex-1 min-w-0 px-2 py-1 text-xs bg-container-primary rounded border border-gray-600 text-on-surface outline-none focus:border-primary-500 transition-colors"
          >
          <button
            class="p-1 rounded hover:bg-container-tertiary transition-colors flex-shrink-0"
            :class="{ 'bg-primary-500/20 text-primary-400': promptLibrary.isSearchOpen.value }"
            title="浏览提示词库"
            @click="promptLibrary.toggleSearch()"
          >
            <div class="i-carbon-search w-3.5 h-3.5" />
          </button>
          <input
            ref="fileInputRef"
            type="file"
            accept=".txt"
            multiple
            class="hidden"
            @change="handleLibraryImport"
          >
        </div>

        <!-- 提示词库搜索结果面板 -->
        <div v-if="promptLibrary.searchQuery.value.trim() || promptLibrary.isSearchOpen.value" class="space-y-2 p-2 bg-container-secondary rounded border border-gray-600">
          <!-- 工具栏 -->
          <div class="flex items-center justify-between gap-2">
            <div class="flex items-center gap-2">
              <div class="text-[10px] text-on-surface-secondary">
                {{ promptLibrary.items.value.length > 0 ? `共 ${promptLibrary.items.value.length} 条` : '' }}
              </div>
              <!-- 分类筛选 -->
              <select
                v-if="promptLibrary.categories.value.length > 1"
                v-model="selectedCategory"
                class="text-[10px] px-1.5 py-0.5 bg-container-primary rounded border border-gray-600 text-on-surface outline-none"
              >
                <option value="">
                  全部分类
                </option>
                <option v-for="cat in promptLibrary.categories.value" :key="cat" :value="cat">
                  {{ cat }}
                </option>
              </select>
            </div>
            <div class="flex items-center gap-1.5">
              <button
                class="px-2 py-1 text-xs bg-green-500/20 text-green-400 rounded hover:bg-green-500/30 transition-colors whitespace-nowrap"
                title="新建提示词"
                @click="showAddPromptForm = !showAddPromptForm"
              >
                {{ showAddPromptForm ? '取消' : '+ 新建' }}
              </button>
              <button
                class="px-2 py-1 text-xs bg-primary-500/20 text-primary-400 rounded hover:bg-primary-500/30 transition-colors whitespace-nowrap"
                :disabled="promptLibrary.isImporting.value"
                @click="fileInputRef?.click()"
              >
                {{ promptLibrary.isImporting.value ? '导入中...' : '导入' }}
              </button>
              <button
                v-if="promptLibrary.items.value.length > 0"
                class="px-2 py-1 text-xs text-red-400 hover:text-red-300 rounded hover:bg-red-500/10 transition-colors whitespace-nowrap"
                @click="promptLibrary.clearLibrary()"
              >
                清空
              </button>
            </div>
          </div>

          <!-- 新建提示词表单 -->
          <div v-if="showAddPromptForm" class="space-y-1.5 p-2 bg-container-tertiary rounded border border-gray-500">
            <input
              v-model="newLibraryItem.name"
              type="text"
              placeholder="名称（如：Debug）"
              class="w-full px-2 py-1 text-xs bg-container-primary rounded border border-gray-600 text-on-surface outline-none focus:border-primary-500"
            >
            <textarea
              v-model="newLibraryItem.content"
              placeholder="提示词内容..."
              rows="3"
              class="w-full px-2 py-1 text-xs bg-container-primary rounded border border-gray-600 text-on-surface outline-none focus:border-primary-500 resize-y"
            />
            <div class="flex items-center gap-2">
              <input
                v-model="newLibraryItem.category"
                type="text"
                placeholder="分类（如：编程）"
                class="flex-1 px-2 py-1 text-xs bg-container-primary rounded border border-gray-600 text-on-surface outline-none focus:border-primary-500"
              >
              <button
                class="px-3 py-1 text-xs bg-primary-500 text-white rounded hover:bg-primary-600 transition-colors whitespace-nowrap"
                @click="handleAddLibraryItem"
              >
                添加
              </button>
            </div>
          </div>

          <!-- 编辑提示词表单 -->
          <div v-if="editingLibraryItem" class="space-y-1.5 p-2 bg-container-tertiary rounded border border-yellow-500/50">
            <div class="text-[10px] text-yellow-400 mb-1">
              编辑提示词
            </div>
            <input
              v-model="editingLibraryItem.name"
              type="text"
              placeholder="名称"
              class="w-full px-2 py-1 text-xs bg-container-primary rounded border border-gray-600 text-on-surface outline-none focus:border-primary-500"
            >
            <textarea
              v-model="editingLibraryItem.content"
              placeholder="内容"
              rows="3"
              class="w-full px-2 py-1 text-xs bg-container-primary rounded border border-gray-600 text-on-surface outline-none focus:border-primary-500 resize-y"
            />
            <div class="flex items-center gap-2">
              <input
                v-model="editingLibraryItem.category"
                type="text"
                placeholder="分类"
                class="flex-1 px-2 py-1 text-xs bg-container-primary rounded border border-gray-600 text-on-surface outline-none focus:border-primary-500"
              >
              <button
                class="px-2 py-1 text-xs text-on-surface-secondary hover:text-on-surface rounded hover:bg-container-tertiary transition-colors"
                @click="editingLibraryItem = null"
              >
                取消
              </button>
              <button
                class="px-3 py-1 text-xs bg-primary-500 text-white rounded hover:bg-primary-600 transition-colors whitespace-nowrap"
                @click="handleSaveLibraryEdit"
              >
                保存
              </button>
            </div>
          </div>

          <!-- 结果列表 -->
          <div v-if="promptLibrary.items.value.length === 0" class="text-xs text-on-surface-secondary text-center py-2">
            提示词库为空，点击"+ 新建"添加或"📱导入"从手机导入
          </div>
          <div v-else-if="promptLibrary.searchResults.value.length === 0 && promptLibrary.searchQuery.value" class="text-xs text-on-surface-secondary text-center py-2">
            未找到匹配提示词
          </div>
          <div v-else class="max-h-48 overflow-y-auto space-y-1">
            <div
              v-for="result in (selectedCategory ? promptLibrary.searchResults.value.filter(r => r.category === selectedCategory) : promptLibrary.searchResults.value)"
              :key="result.id"
              class="flex items-start gap-2 p-1.5 rounded cursor-pointer hover:bg-container-tertiary transition-colors group"
              @click="handleLibraryPromptClick(result.content)"
            >
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-1.5">
                  <span class="text-xs font-medium text-on-surface truncate">{{ result.name }}</span>
                  <span class="text-[10px] px-1 py-0.5 rounded bg-primary-500/15 text-primary-400 whitespace-nowrap">{{ result.category }}</span>
                </div>
                <div class="text-[11px] text-on-surface-secondary truncate mt-0.5">
                  {{ result.content }}
                </div>
              </div>
              <!-- 编辑/删除按钮 -->
              <div class="hidden group-hover:flex items-center gap-1 flex-shrink-0">
                <button
                  class="p-0.5 rounded hover:bg-container-primary transition-colors"
                  title="编辑"
                  @click.stop="editingLibraryItem = { id: result.id, name: result.name, content: result.content, category: result.category }"
                >
                  <div class="i-carbon-edit w-3 h-3 text-on-surface-secondary" />
                </button>
                <button
                  class="p-0.5 rounded hover:bg-red-500/20 transition-colors"
                  title="删除"
                  @click.stop="handleDeleteLibraryItem(result.id)"
                >
                  <div class="i-carbon-trash-can w-3 h-3 text-red-400" />
                </button>
              </div>
            </div>
          </div>
        </div>
        <div
          ref="promptContainer"
          data-prompt-container
          class="flex flex-wrap gap-2"
        >
          <div
            v-for="(prompt, index) in sortablePrompts"
            :key="prompt.id"
            :title="prompt.description || (prompt.content.trim() ? prompt.content : '清空输入框')"
            class="inline-flex items-center gap-1 px-2 py-1 text-xs bg-container-secondary hover:bg-container-tertiary rounded transition-all duration-200 select-none border border-gray-600 text-on-surface sortable-item"
            :class="{ 'prompt-shortcut-active': activePromptShortcutIndex === index }"
          >
            <!-- 拖拽手柄 -->
            <div class="drag-handle cursor-move p-0.5 rounded hover:bg-container-tertiary transition-colors">
              <div class="i-carbon-drag-horizontal w-3 h-3 text-on-surface-secondary" />
            </div>

            <!-- 按钮内容 -->
            <div
              class="inline-flex items-center cursor-pointer"
              @click="handlePromptClick(prompt)"
            >
              <span>{{ prompt.name }}</span>
              <span
                v-if="isPromptShortcutModifierHeld && index < PROMPT_SHORTCUT_LIMIT"
                class="ml-1 text-[10px] text-on-surface-secondary opacity-70"
                aria-hidden="true"
              >⌥{{ index + 1 }}</span>
            </div>
          </div>
        </div>
      </div>

      <!-- 上下文追加区域 -->
      <div v-if="props.enableContextAppend && customPromptEnabled && conditionalPrompts.length > 0" class="space-y-2" data-guide="context-append">
        <div class="text-xs text-on-surface-secondary flex items-center gap-2">
          <div class="i-carbon-settings-adjust w-3 h-3 text-primary-500" />
          <span>上下文追加:</span>
        </div>
        <div class="grid grid-cols-2 gap-2">
          <div
            v-for="prompt in conditionalPrompts"
            :key="prompt.id"
            class="flex items-center justify-between p-2 bg-container-secondary rounded border border-gray-600 hover:bg-container-tertiary transition-colors text-xs"
            :class="{ 'opacity-50': prompt.is_active === false }"
          >
            <div class="flex items-center flex-1 min-w-0 mr-2">
              <n-checkbox
                :checked="prompt.is_active !== false"
                size="small"
                class="mr-2"
                @update:checked="(value: boolean) => handleConditionalActiveToggle(prompt.id, value)"
              />
              <div class="flex-1 min-w-0">
                <div class="text-xs text-on-surface truncate font-medium" :title="prompt.condition_text || prompt.name">
                  {{ prompt.condition_text || prompt.name }}
                </div>
                <div v-if="getConditionalDescription(prompt)" class="text-xs text-primary-600 dark:text-primary-400 opacity-50 dark:opacity-60 mt-0.5 truncate leading-tight" :title="getConditionalDescription(prompt)">
                  {{ getConditionalDescription(prompt) }}
                </div>
              </div>
            </div>
            <n-switch
              :value="prompt.current_state ?? false"
              size="small"
              @update:value="(value: boolean) => handleConditionalToggle(prompt.id, value)"
            />
          </div>
        </div>
      </div>

      <!-- 图片提示区域 -->
      <div v-if="uploadedImages.length === 0 && attachedFiles.length === 0" class="text-center">
        <div class="text-xs text-on-surface-secondary">
          💡 提示：可以在输入框中粘贴图片、Finder 复制的文件或绝对路径，也可以把文件拖进来 ({{ pasteShortcut }})
        </div>
      </div>
    </div>

    <!-- 插入模式选择对话框 -->
    <n-modal v-model:show="showInsertDialog" preset="dialog" title="插入模式选择">
      <template #header>
        <div class="flex items-center gap-2">
          <div class="i-carbon-text-creation w-4 h-4" />
          <span>插入Prompt</span>
        </div>
      </template>
      <div class="space-y-4">
        <p class="text-sm text-on-surface-secondary">
          输入框中已有内容，请选择插入模式：
        </p>
        <div class="bg-container-secondary p-3 rounded text-sm">
          {{ pendingPromptContent }}
        </div>
      </div>
      <template #action>
        <div class="flex gap-2">
          <n-button @click="showInsertDialog = false">
            取消
          </n-button>
          <n-button type="warning" @click="handleInsertMode('replace')">
            替换内容
          </n-button>
          <n-button type="primary" @click="handleInsertMode('append')">
            追加内容
          </n-button>
        </div>
      </template>
    </n-modal>
  </div>
</template>

<style scoped>
/* Sortable.js 拖拽样式 */
.sortable-ghost {
  opacity: 0.5;
  transform: scale(0.95);
}

.sortable-chosen {
  cursor: grabbing !important;
}

.sortable-drag {
  opacity: 0.8;
  transform: rotate(5deg);
}

.prompt-shortcut-active {
  transform: translateY(1px) scale(0.98) !important;
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.2) !important;
}

@media (prefers-reduced-motion: reduce) {
  .sortable-item {
    transition-duration: 0.01ms !important;
  }
}
</style>
