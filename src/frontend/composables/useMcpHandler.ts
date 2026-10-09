import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow, UserAttentionType } from '@tauri-apps/api/window'
import { nextTick, ref } from 'vue'
import { publishCompletedMcpRequest } from '../services/bridgeRequestClosure'
import { clearActiveMcpFatalContext, setActiveMcpFatalContext } from '../utils/mcpFatalError'
import { isDesktopMcpSubmission, isDesktopSubmissionCurrent, runDesktopMcpSubmission } from './desktopMcpSubmission'
import { crossDeviceSendError } from './useCrossDevice'
import { MCP_DELIVERY_FAILURE, mcpDeliveryError, useMcpDelivery } from './useMcpDelivery'
import { useNotification } from './useNotification'

const MUTE_STORAGE_KEY = 'iterate.muted'
const LAST_VALID_PROJECT_PATH_KEY = 'iterate.last_valid_project_path'
async function shouldFocusPopupOnShow(): Promise<boolean> {
  try {
    const config = await invoke<{ focus_popup_on_show?: boolean }>('get_window_config')
    return config.focus_popup_on_show !== false
  }
  catch (error) {
    console.warn('加载弹窗聚焦设置失败，使用默认行为:', error)
    return true
  }
}

const DEFAULT_CONTINUE_PROMPT = '请按照最佳实践继续'
const DEFAULT_LOOP_PROMPT = '进入自主循环模式。\n\n## 执行规则\n1. 基于当前上下文，按最佳实践继续执行当前任务\n2. 每轮完成后立即调用 iterate/zhi 汇报进度，不要等待用户\n3. 如果任务未完成且无需用户决策，继续自动执行下一步\n\n## 停止条件（满足任一即停止）\n- 任务已全部完成\n- 遇到必须由用户决定的问题\n- 遇到无法自动解决的错误（连续失败2次）\n- 不确定下一步该做什么\n\n## 汇报格式\n每轮简要说明：做了什么 → 结果如何 → 下一步计划'

function buildEnhancePrompt(input: string) {
  return `Use the following prompt to optimize and enhance the context of the content in 《》, and return the enhanced result by calling the tool 'iterate' after completion. Here is an instruction that I'd like to give you, but it needs to be improved. Rewrite and enhance this instruction to make it clearer, more specific, less ambiguous, and correct any mistakes. Reply immediately with your answer, even if you're not sure. Consider the context of our conversation history when enhancing the prompt. Reply with the following format:
### BEGIN RESPONSE ###
Here is an enhanced version of the original instruction that is more specific and clear:
<augment-enhanced-prompt>enhanced prompt goes here</augment-enhanced-prompt>

### END RESPONSE ###

Here is my original instruction:

《${input}》`
}

export type McpLaunchKind = 'normal' | 'inline' | 'file' | 'invalid'

export interface McpLaunchContext {
  kind: McpLaunchKind
  isMcp: boolean
  isStandaloneMode: boolean
  request: any | null
  requestFile?: string
  error?: string
}

function isDisplayableProjectPath(projectPath: unknown): projectPath is string {
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

function isRecord(value: unknown): value is Record<string, any> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function resolveRequestId(request: any): string | null {
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

function resolveExplicitConversationRouteId(request: any): string | null {
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

async function resolveLiveGoalConversationRouteId(projectPath: string | null): Promise<string | null> {
  try {
    const liveGoal = await invoke('get_live_goal')
    return normalizeLiveGoalConversationRouteId(liveGoal, projectPath)
  }
  catch {
    return null
  }
}

async function resolveConversationRouteIdWithFallback(
  request: any,
  projectPath: string | null,
): Promise<string | null> {
  const explicitRouteId = resolveExplicitConversationRouteId(request)
  if (explicitRouteId)
    return explicitRouteId

  const liveGoalRouteId = await resolveLiveGoalConversationRouteId(projectPath)
  return liveGoalRouteId ?? resolveRequestId(request)
}

async function freezeConversationRouteId(request: any, projectPath: string | null) {
  if (!isRecord(request) || resolveExplicitConversationRouteId(request))
    return request

  const timelineRouteId = await resolveConversationRouteIdWithFallback(request, projectPath)
  if (!timelineRouteId)
    return request

  const metadata = isRecord(request.metadata) ? request.metadata : {}
  return {
    ...request,
    timeline_route_id: timelineRouteId,
    metadata: {
      ...metadata,
      timeline_route_id: timelineRouteId,
    },
  }
}

function hasValidMcpRequest(request: unknown): request is Record<string, any> {
  return isRecord(request) && resolveRequestId(request) !== null
}

async function loadMcpLaunchContext(): Promise<McpLaunchContext> {
  let args: any = null
  try {
    args = await invoke('get_cli_args')
  }
  catch (error) {
    return {
      kind: 'normal',
      isMcp: false,
      isStandaloneMode: false,
      request: null,
      error: `get_cli_args failed: ${String(error)}`,
    }
  }

  const isStandaloneMode = Boolean(args?.standalone_mode)
  const inlineRequest = args?.mcp_request_inline
  const requestFile = typeof args?.mcp_request === 'string' ? args.mcp_request.trim() : ''
  const hasRequestSignal = inlineRequest != null || requestFile.length > 0

  if (!isStandaloneMode) {
    if (hasRequestSignal) {
      return {
        kind: 'invalid',
        isMcp: false,
        isStandaloneMode,
        request: null,
        requestFile: requestFile || undefined,
        error: 'MCP request signal without standalone_mode',
      }
    }

    return {
      kind: 'normal',
      isMcp: false,
      isStandaloneMode,
      request: null,
    }
  }

  if (inlineRequest != null) {
    if (hasValidMcpRequest(inlineRequest)) {
      return {
        kind: 'inline',
        isMcp: true,
        isStandaloneMode,
        request: inlineRequest,
      }
    }

    return {
      kind: 'invalid',
      isMcp: false,
      isStandaloneMode,
      request: null,
      error: 'inline MCP request is missing a valid request id',
    }
  }

  if (requestFile) {
    try {
      const content = await invoke('read_mcp_request', { filePath: requestFile })
      if (hasValidMcpRequest(content)) {
        return {
          kind: 'file',
          isMcp: true,
          isStandaloneMode,
          request: content,
          requestFile,
        }
      }

      return {
        kind: 'invalid',
        isMcp: false,
        isStandaloneMode,
        request: null,
        requestFile,
        error: 'file MCP request is missing a valid request id',
      }
    }
    catch (error) {
      return {
        kind: 'invalid',
        isMcp: false,
        isStandaloneMode,
        request: null,
        requestFile,
        error: `read_mcp_request failed: ${String(error)}`,
      }
    }
  }

  return {
    kind: 'invalid',
    isMcp: false,
    isStandaloneMode,
    request: null,
    error: 'standalone_mode without MCP request content',
  }
}

let cachedMcpLaunchContext: Promise<McpLaunchContext> | null = null

export function resolveMcpLaunchContext(): Promise<McpLaunchContext> {
  if (!cachedMcpLaunchContext)
    cachedMcpLaunchContext = loadMcpLaunchContext()

  return cachedMcpLaunchContext
}

// 静音状态（模块级单例，确保所有调用者共享同一个状态）
const isMuted = ref(localStorage.getItem(MUTE_STORAGE_KEY) === 'true')
window.addEventListener('storage', (event) => {
  if (event.key === MUTE_STORAGE_KEY)
    isMuted.value = event.newValue === 'true'
})

/**
 * 切换静音状态
 */
function toggleMute() {
  isMuted.value = !isMuted.value
  localStorage.setItem(MUTE_STORAGE_KEY, String(isMuted.value))
}

/**
 * MCP处理组合式函数
 */
export function useMcpHandler() {
  const mcpRequest = ref(null)
  const showMcpPopup = ref(false)
  const isMcpProcess = ref(false)
  useMcpDelivery(() => {
    const request = mcpRequest.value as any
    return isMcpProcess.value && request
      ? { id: resolveRequestId(request), project_path: request.project_path ?? request.projectPath, source: request.source }
      : null
  })
  const resolvingRequestIds = new Set<string>()
  const pendingDialogs: any[] = []
  const nativeDrafts = new Map<string, { userInput: string, selectedOptions: string[] }>()
  const locallyHiddenNative = new Set<string>()
  let nativeQuestions: any[] = []
  let nativeRevision = 0
  let openingDialog = false
  let nativeSendingId: string | null = null

  function isNative(request: any) {
    return request?.source === 'codex_native' && request?.native && String(request?.id).startsWith('native-codex:')
  }

  function nativeDisplay(request: any) {
    const group = nativeQuestions.filter(q => q.native.group_id === request.native.group_id && !locallyHiddenNative.has(q.id))
    return { ...request, native: { ...request.native, remaining: group.filter(q => q.id !== request.id && !nativeDrafts.has(q.id)).length, draft: nativeDrafts.get(request.id) } }
  }

  async function pumpDialogs() {
    if (openingDialog || mcpRequest.value || isMcpProcess.value)
      return
    let request = pendingDialogs.shift()
    if (!request) {
      // If another client answered the current item, preserve keyed drafts of remaining items.
      request = nativeQuestions.find(q => !locallyHiddenNative.has(q.id))
    }
    if (!request)
      return
    openingDialog = true
    try {
      await showMcpDialog(isNative(request) ? nativeDisplay(request) : request)
    }
    finally {
      openingDialog = false
      if (!mcpRequest.value)
        void pumpDialogs()
    }
  }

  function reconcileNative(snapshot: any) {
    if (!snapshot || snapshot.revision <= nativeRevision || !Array.isArray(snapshot.questions))
      return
    nativeRevision = snapshot.revision
    nativeQuestions = snapshot.questions.filter(isNative)
    const liveIds = new Set(nativeQuestions.map(q => q.id))
    for (const id of nativeDrafts.keys()) {
      if (!liveIds.has(id) && id !== nativeSendingId)
        nativeDrafts.delete(id)
    }
    for (let i = pendingDialogs.length - 1; i >= 0; i--) {
      if (isNative(pendingDialogs[i]) && !liveIds.has(pendingDialogs[i].id))
        pendingDialogs.splice(i, 1)
    }
    const current = mcpRequest.value as any
    if (isNative(current) && Array.isArray(snapshot.invalidated) && snapshot.invalidated.includes(current.id)) {
      showMcpPopup.value = false
      mcpRequest.value = null
    }
    else if (isNative(current) && current.native.status !== 'received') {
      const live = nativeQuestions.find(q => q.id === current.id)
      if (live) {
        mcpRequest.value = { ...nativeDisplay(live), native: { ...nativeDisplay(live).native, error: current.native.error } }
      }
      else if (current.id !== nativeSendingId) {
        showMcpPopup.value = false
        mcpRequest.value = null
      }
    }
    for (const q of nativeQuestions) {
      if (q.id !== resolveRequestId(mcpRequest.value) && !nativeDrafts.has(q.id)
        && !locallyHiddenNative.has(q.id) && !pendingDialogs.some(p => p.id === q.id)) {
        pendingDialogs.push(q)
      }
    }
    void pumpDialogs()
  }

  async function hideNative(request: any) {
    locallyHiddenNative.add(request.id)
    nativeDrafts.delete(request.id)
    for (let i = pendingDialogs.length - 1; i >= 0; i--) {
      if (pendingDialogs[i].id === request.id)
        pendingDialogs.splice(i, 1)
    }
    // Native cancellation hides one question; it never sends an empty answer or cancels CLI.
    await invoke('send_mcp_response', { response: 'CANCELLED', requestId: request.id })
    if (resolveRequestId(mcpRequest.value) === request.id) {
      showMcpPopup.value = false
      mcpRequest.value = null
    }
    void pumpDialogs()
  }

  async function handleNativeResponse(request: any, response: any) {
    if (response?.metadata?.source !== 'codex_native')
      return
    const key = beginRequestResolution(request)
    if (!key)
      return
    try {
      const draft = { userInput: String(response.user_input || ''), selectedOptions: response.selected_options || [] }
      nativeDrafts.set(request.id, draft)
      const group = nativeQuestions.filter(q => q.native.group_id === request.native.group_id && !locallyHiddenNative.has(q.id))
      if (!group.some(q => q.id === request.id))
        return
      const remaining = group.find(q => !nativeDrafts.has(q.id))
      if (remaining) {
        showMcpPopup.value = false
        mcpRequest.value = null
        await pumpDialogs()
        return
      }
      const answers = group.map((q) => {
        const d = nativeDrafts.get(q.id)!
        return { id: q.id, answer: [...d.selectedOptions, d.userInput].filter(Boolean).join('\n') }
      })
      nativeSendingId = request.id
      mcpRequest.value = { ...request, native: { ...request.native, status: 'sending', draft, error: '' } }
      // Rust returns only after remote turn/steer ACK, not after local oneshot delivery.
      await invoke('send_mcp_response', { requestId: request.id, response: { ...response, native_answers: answers, native_retry: Boolean(response.native_retry) } })
      for (const q of group)
        nativeDrafts.delete(q.id)
      if (resolveRequestId(mcpRequest.value) === request.id)
        mcpRequest.value = { ...request, native: { ...request.native, status: 'received', draft, error: '' } }
    }
    catch (error) {
      const live = nativeQuestions.find(q => q.id === request.id)
      if (live && !locallyHiddenNative.has(request.id)) {
        mcpRequest.value = { ...nativeDisplay(live), native: { ...nativeDisplay(live).native, status: String(error).includes('结果未知') ? 'unknown' : 'waiting', error: String(error) } }
      }
      else if (resolveRequestId(mcpRequest.value) === request.id) {
        mcpRequest.value = null
        showMcpPopup.value = false
      }
    }
    finally {
      nativeSendingId = null
      finishRequestResolution(key)
      void pumpDialogs()
    }
  }

  function beginRequestResolution(request: any, response?: any): string | null {
    const key = resolveRequestId(request) ?? resolveRequestId(response) ?? '__unrouted_mcp_request__'
    if (resolvingRequestIds.has(key)) {
      console.info('[MCP] 忽略重复响应', { requestId: key })
      return null
    }
    resolvingRequestIds.add(key)
    return key
  }

  function finishRequestResolution(key: string) {
    resolvingRequestIds.delete(key)
  }

  interface ImmediateMcpDismissal {
    request: any
    closedInlinePopup: boolean
    hidStandaloneWindow: boolean
  }

  async function dismissMcpUiImmediately(request: any): Promise<ImmediateMcpDismissal> {
    const dismissal: ImmediateMcpDismissal = {
      request,
      closedInlinePopup: !isMcpProcess.value,
      hidStandaloneWindow: isMcpProcess.value,
    }

    if (dismissal.closedInlinePopup) {
      showMcpPopup.value = false
      mcpRequest.value = null
    }

    if (dismissal.hidStandaloneWindow) {
      try {
        await invoke('dismiss_standalone_mcp_window')
      }
      catch (error) {
        dismissal.hidStandaloneWindow = false
        console.error('提前隐藏MCP窗口失败:', error)
      }
    }

    return dismissal
  }

  async function restoreMcpUiAfterFailure(dismissal: ImmediateMcpDismissal) {
    if (dismissal.closedInlinePopup) {
      mcpRequest.value = dismissal.request
      showMcpPopup.value = true
    }

    if (dismissal.hidStandaloneWindow) {
      try {
        if (await invoke<boolean>('restore_popup_tab'))
          return
        if (await shouldFocusPopupOnShow()) {
          const window = getCurrentWindow()
          await window.show()
          await window.setFocus()
        }
        else {
          await invoke('center_window')
        }
      }
      catch (error) {
        console.error('恢复MCP窗口失败:', error)
      }
    }
  }

  function resolveProjectPath(request: any): string | null {
    if (isNative(request))
      return request.project_path || null
    if (String(request?.id || '').startsWith('cross-'))
      return null
    const candidate = request?.project_path ?? request?.projectPath
    if (isDisplayableProjectPath(candidate)) {
      const normalized = candidate.trim()
      localStorage.setItem(LAST_VALID_PROJECT_PATH_KEY, normalized)
      return normalized
    }

    const cached = localStorage.getItem(LAST_VALID_PROJECT_PATH_KEY)
    if (isDisplayableProjectPath(cached))
      return cached.trim()

    return null
  }

  async function completeDeliveredPopupTab(requestId: string | null) {
    if (!isMcpProcess.value || !requestId)
      return
    try {
      await invoke('complete_popup_tab', { requestId })
    }
    catch (error) {
      // Delivery is already accepted. Presentation cleanup must not reopen or
      // resubmit an acknowledged request; process exit remains the fallback.
      console.warn('已发送标签切换失败:', error)
    }
  }

  /**
   * 统一的MCP响应处理
   */
  async function handleMcpResponse(response: any) {
    const submission = isDesktopMcpSubmission(response) ? response : null
    if (submission && !isDesktopSubmissionCurrent(mcpRequest.value, submission.request)) {
      submission.settled()
      return
    }
    if (isNative(mcpRequest.value))
      return handleNativeResponse(mcpRequest.value, response)
    crossDeviceSendError.value = ''
    mcpDeliveryError.value = ''
    const request = submission ? submission.request : mcpRequest.value as any
    const resolutionKey = beginRequestResolution(request, response)
    if (!resolutionKey) {
      submission?.settled()
      return
    }
    const projectPath = resolveProjectPath(request)
    const requestId = resolveRequestId(request)
    if (submission) {
      const isCurrent = () => isDesktopSubmissionCurrent(mcpRequest.value, request)
      try {
        await runDesktopMcpSubmission({
          // Hide this same native window in both resident and standalone modes.
          // Retain its request and mounted form until the real send ACK.
          dismiss: () => invoke('dismiss_standalone_mcp_window'),
          isCurrent,
          prepare: () => submission.prepare(isCurrent),
          deliver: async (preparedResponse) => {
            const timelineRouteId = await resolveConversationRouteIdWithFallback(request, projectPath)
            if (!isCurrent() || resolveRequestId(preparedResponse) !== requestId)
              throw new Error('回复请求归属已变化，未发送')
            await invoke('send_mcp_response', { response: preparedResponse, projectPath, requestId, timelineRouteId })
            if (isCurrent())
              await completeDeliveredPopupTab(requestId)
          },
          accepted: () => {
            if (isCurrent()) {
              showMcpPopup.value = false
              mcpRequest.value = null
            }
          },
          cleanup: async () => {
            await publishCompletedMcpRequest(requestId, request?.project_path ?? request?.projectPath)
            if (!mcpRequest.value) {
              clearActiveMcpFatalContext()
              if (isMcpProcess.value)
                await invoke('exit_app')
            }
          },
          failed: async (error) => {
            console.error('MCP响应处理失败:', error)
            mcpDeliveryError.value = `发送未确认成功：${String(error)}`
            if (String(requestId || '').startsWith('cross-'))
              crossDeviceSendError.value = String(error)
            // No remount/resetForm: text, options and attachments stay in place.
            if (await invoke<boolean>('restore_popup_tab'))
              return
            const window = getCurrentWindow()
            await window.show()
            const shouldFocus = await shouldFocusPopupOnShow()
            if (isCurrent() && shouldFocus)
              await window.setFocus()
          },
          cleanupFailed: error => console.error('MCP回复已确认，收尾失败:', error),
        })
      }
      finally {
        submission.settled()
        finishRequestResolution(resolutionKey)
        void pumpDialogs()
      }
      return
    }
    let dismissal: ImmediateMcpDismissal | null = null
    let submitted = false
    try {
      dismissal = await dismissMcpUiImmediately(request)
      // 通过Tauri命令发送响应并退出应用
      const timelineRouteId = await resolveConversationRouteIdWithFallback(request, projectPath)
      console.info('[MCP] 发送响应', {
        requestId,
        timelineRouteId,
        projectPath,
        hasResponse: response != null,
      })
      await invoke('send_mcp_response', { response, projectPath, requestId, timelineRouteId })
      submitted = true
      await completeDeliveredPopupTab(requestId)
      await publishCompletedMcpRequest(requestId, request?.project_path ?? request?.projectPath)
      clearActiveMcpFatalContext()
      if (isMcpProcess.value) {
        await invoke('exit_app')
      }
    }
    catch (error) {
      console.error('MCP响应处理失败:', error)
      if (!submitted) {
        mcpDeliveryError.value = MCP_DELIVERY_FAILURE
        if (String(requestId || '').startsWith('cross-'))
          crossDeviceSendError.value = String(error)
        if (dismissal)
          await restoreMcpUiAfterFailure(dismissal)
      }
    }
    finally {
      finishRequestResolution(resolutionKey)
      void pumpDialogs()
    }
  }

  /**
   * 结束当前 zhi/call_zhi，但保留 iterate 主程序和其他请求。
   */
  async function handleMcpCloseCurrentDialog() {
    const request = mcpRequest.value as any
    if (isNative(request))
      return hideNative(request)
    const requestId = resolveRequestId(request)
    if (!request || !requestId)
      return

    await handleMcpResponse({
      user_input: '',
      selected_options: [],
      images: [],
      file_paths: [],
      image_paths: [],
      metadata: {
        timestamp: new Date().toISOString(),
        request_id: requestId,
        source: 'popup_closed',
      },
    })
  }

  /**
   * 统一的MCP取消处理
   */
  async function handleMcpCancel() {
    const request = mcpRequest.value as any
    if (isNative(request))
      return hideNative(request)
    const resolutionKey = beginRequestResolution(request)
    if (!resolutionKey)
      return
    const projectPath = resolveProjectPath(request)
    const requestId = resolveRequestId(request)
    let dismissal: ImmediateMcpDismissal | null = null
    try {
      dismissal = await dismissMcpUiImmediately(request)
      // 发送取消信息并退出应用
      const timelineRouteId = await resolveConversationRouteIdWithFallback(request, projectPath)
      console.info('[MCP] 发送取消响应', {
        requestId,
        timelineRouteId,
        projectPath,
      })
      await invoke('send_mcp_response', { response: 'CANCELLED', projectPath, requestId, timelineRouteId })
      await publishCompletedMcpRequest(requestId, request?.project_path ?? request?.projectPath)
      clearActiveMcpFatalContext()
      if (isMcpProcess.value) {
        await invoke('exit_app')
      }
    }
    catch (error) {
      // 静默处理MCP取消错误
      console.error('MCP取消处理失败:', error)
      if (dismissal)
        await restoreMcpUiAfterFailure(dismissal)
    }
    finally {
      finishRequestResolution(resolutionKey)
      void pumpDialogs()
    }
  }

  /**
   * 显示MCP弹窗
   */
  async function showMcpDialog(request: any) {
    mcpDeliveryError.value = ''
    const projectPath = resolveProjectPath(request)
    const routedRequest = await freezeConversationRouteId(request, projectPath)
    const requestId = resolveRequestId(routedRequest)
    const timelineRouteId = resolveExplicitConversationRouteId(routedRequest)
    setActiveMcpFatalContext(routedRequest, isMcpProcess.value)
    console.info('[MCP] showMcpDialog', {
      requestId,
      timelineRouteId,
      projectPath,
      hasMessage: typeof routedRequest?.message === 'string' && routedRequest.message.length > 0,
    })

    try {
      await invoke('ack_mcp_request_ready', { requestId, projectPath })
    }
    catch (error) {
      console.warn('[MCP] ack_mcp_request_ready 失败:', error)
    }

    // 设置窗口标题为项目路径
    const displayProjectPath = projectPath
    if (displayProjectPath) {
      try {
        const window = getCurrentWindow()
        await window.setTitle(`iterate - ${displayProjectPath}`)
      }
      catch (error) {
        console.error('设置窗口标题失败:', error)
      }
    }

    // 获取Telegram配置，检查是否需要隐藏前端弹窗
    let shouldShowFrontendPopup = true
    try {
      const telegramConfig = isNative(request) ? null : await invoke('get_telegram_config')
      // 如果Telegram启用且配置了隐藏前端弹窗，则不显示前端弹窗
      if (telegramConfig && (telegramConfig as any).enabled && (telegramConfig as any).hide_frontend_popup) {
        shouldShowFrontendPopup = false
        console.log('🔕 根据Telegram配置，隐藏前端弹窗')
      }
    }
    catch (error) {
      console.error('获取Telegram配置失败:', error)
      // 配置获取失败时，保持默认行为（显示弹窗）
    }

    // 根据配置决定是否显示前端弹窗
    if (isNative(request) && (!nativeQuestions.some(q => q.id === request.id) || locallyHiddenNative.has(request.id)))
      return
    if (shouldShowFrontendPopup) {
      let tabIsBackground = false
      if (isMcpProcess.value && requestId) {
        try {
          const tabs = await invoke<{ mode: string, current_pid: number, active_pid: number }>('register_popup_tab', {
            requestId,
            title: routedRequest?.conversation_title || routedRequest?.project_path || '会话',
            projectPath: routedRequest?.project_path || '',
          })
          tabIsBackground = tabs.mode === 'tabs' && tabs.active_pid !== tabs.current_pid
        }
        catch (error) { console.warn('注册会话标签失败，保留原弹窗:', error) }
      }
      const shouldForceShow = !!(routedRequest?.loop_active || routedRequest?.force_popup)
      // 循环检查点 / loop 完成交付：强制取消静音并显示窗口
      if (shouldForceShow && isMuted.value) {
        console.log('🔔 关键 loop 弹窗：自动取消静音，显示窗口')
        toggleMute()
        mcpRequest.value = routedRequest
        showMcpPopup.value = true
      }
      // 静音模式：跳过弹窗显示，窗口最小化到 Dock
      else if (isMuted.value) {
        console.log('🔕 静音模式：跳过弹窗显示，窗口最小化')
        // 仍然设置请求数据，但不显示弹窗
        mcpRequest.value = routedRequest
        showMcpPopup.value = true
        if (isMcpProcess.value) {
          await nextTick()
          try {
            await invoke('position_window_left')
          }
          catch (error) {
            console.error('定位静音MCP窗口失败:', error)
          }
        }
        // 最小化窗口到 Dock
        try {
          const window = getCurrentWindow()
          await window.minimize()
          // Windows needs show() for a taskbar entry; macOS show() restores
          // the miniaturized window, so leave its Dock presentation alone.
          if (isMcpProcess.value && !tabIsBackground && !navigator.platform.toUpperCase().includes('MAC'))
            await window.show()
          if (navigator.platform.toUpperCase().includes('WIN'))
            await window.requestUserAttention(UserAttentionType.Informational)
        }
        catch (error) {
          console.error('最小化窗口失败:', error)
        }
      }
      else {
        // 正常模式：设置请求数据和显示状态
        mcpRequest.value = routedRequest
        showMcpPopup.value = true
      }
      // Standalone windows start hidden: decide their first visible state only
      // after the shared notification preference and force-popup rules resolve.
      if (isMcpProcess.value && !isMuted.value) {
        await nextTick()
        try {
          await invoke('center_window')
        }
        catch (error) {
          console.error('定位并显示MCP窗口失败:', error)
          if (await shouldFocusPopupOnShow()) {
            const window = getCurrentWindow()
            await window.show()
            await window.setFocus()
          }
        }
      }
    }
    else {
      console.log('🔕 跳过前端弹窗显示，仅使用Telegram交互')
      // Keep the old request's route occupied while Telegram handles its hidden dialog.
      mcpRequest.value = routedRequest
      showMcpPopup.value = false
    }

    // 播放音频通知（静音时跳过）
    if (!isMuted.value) {
      try {
        await invoke('play_notification_sound')
      }
      catch (error) {
        console.error('播放音频通知失败:', error)
      }

      // 发送系统通知（Web Notification API）
      try {
        const notification = useNotification()
        // 确保通知状态已初始化（从 localStorage 加载）
        notification.init()
        const title = 'iterate'
        const body = request?.message?.substring(0, 100) || '有新的 MCP 请求'
        notification.sendNotification(title, body)
      }
      catch (error) {
        console.error('发送系统通知失败:', error)
      }
    }

    // 启动Telegram同步（无论是否显示弹窗都启动）
    try {
      if (request?.message && !isNative(request)) {
        await invoke('start_telegram_sync', {
          message: request.message,
          predefinedOptions: request.predefined_options || [],
          isMarkdown: request.is_markdown || false,
        })
        console.log('✅ Telegram同步启动成功')
      }
    }
    catch (error) {
      console.error('启动Telegram同步失败:', error)
    }
  }

  /**
   * 检查MCP模式
   */
  async function checkMcpMode() {
    try {
      const launchContext = await resolveMcpLaunchContext()
      console.log('[checkMcpMode] launch context:', launchContext)

      if (launchContext.isMcp && launchContext.request) {
        isMcpProcess.value = launchContext.isStandaloneMode
        await showMcpDialog(launchContext.request)
        return { isMcp: true, mcpContent: launchContext.request }
      }

      if (launchContext.kind === 'invalid')
        console.warn('[checkMcpMode] 无效 MCP 启动参数:', launchContext.error)
    }
    catch (error) {
      console.error('检查MCP模式失败:', error)
    }
    isMcpProcess.value = false
    return { isMcp: false, mcpContent: null }
  }

  /**
   * 设置MCP事件监听器
   */
  async function setupMcpEventListener() {
    try {
      const context = await resolveMcpLaunchContext()
      const resident = !context.isStandaloneMode && context.kind === 'normal'
      if (resident && getCurrentWindow().label === 'main') {
        await listen('codex-native-questions', event => reconcileNative(event.payload))
        await invoke('ack_mcp_request_ready', { requestId: 'iterate-codex-native-ready-v1' })
      }
      await listen('mcp-request', (event) => {
        const payload = event.payload as any
        if (resident) {
          // ACK immediately even while another source occupies the popup; the IPC caller waits.
          void invoke('ack_mcp_request_ready', { requestId: resolveRequestId(payload), projectPath: payload?.project_path })
          if (resolveRequestId(mcpRequest.value) !== resolveRequestId(payload)
            && !pendingDialogs.some(p => resolveRequestId(p) === resolveRequestId(payload))) {
            pendingDialogs.push(payload)
            void pumpDialogs()
          }
          return
        }
        const currentRequest = mcpRequest.value as any
        const currentWindowRequestId = resolveRequestId(currentRequest)
        const targetRequestId = resolveRequestId(payload)
        if (currentWindowRequestId && targetRequestId && currentWindowRequestId !== targetRequestId) {
          console.log('[MCP] 忽略不匹配的 request_id 请求:', {
            current: currentWindowRequestId,
            target: targetRequestId,
          })
          return
        }

        const currentWindowProjectPath = resolveProjectPath(currentRequest)
        const targetProjectPath = resolveProjectPath(payload)
        if (currentWindowProjectPath && targetProjectPath && currentWindowProjectPath !== targetProjectPath) {
          console.log('[MCP] 忽略不匹配的项目请求:', { current: currentWindowProjectPath, target: targetProjectPath })
          return
        }

        showMcpDialog(payload)
      })
    }
    catch (error) {
      console.error('设置MCP事件监听器失败:', error)
    }
  }

  async function getReplyPrompts() {
    const config = await invoke('get_reply_config')
    return {
      continuePrompt: (config as any)?.continue_prompt ?? DEFAULT_CONTINUE_PROMPT,
      loopPrompt: (config as any)?.loop_prompt ?? DEFAULT_LOOP_PROMPT,
    }
  }

  /**
   * 模拟点击“继续”按钮
   */
  async function handleMcpContinue(source = 'web_bridge_continue') {
    if (isNative(mcpRequest.value))
      return
    try {
      const { continuePrompt } = await getReplyPrompts()

      const response = {
        user_input: continuePrompt,
        selected_options: [],
        images: [],
        metadata: {
          timestamp: new Date().toISOString(),
          source,
        },
      }
      await handleMcpResponse(response)
    }
    catch (error) {
      console.error('MCP继续处理失败:', error)
    }
  }

  /**
   * 模拟点击“循环”按钮
   */
  async function handleMcpLoopReply(source = 'web_bridge_loop_start', userInput: string = '') {
    if (isNative(mcpRequest.value))
      return
    try {
      const { loopPrompt } = await getReplyPrompts()
      const trimmedUserInput = userInput.trim()

      const response = {
        user_input: trimmedUserInput || loopPrompt,
        selected_options: [],
        images: [],
        metadata: {
          timestamp: new Date().toISOString(),
          source,
        },
      }
      await handleMcpResponse(response)
    }
    catch (error) {
      console.error('MCP循环处理失败:', error)
    }
  }

  async function handleMcpEnhance(userInput: string = '') {
    if (isNative(mcpRequest.value))
      return
    try {
      const trimmedUserInput = userInput.trim()
      if (!trimmedUserInput)
        return

      const response = {
        user_input: buildEnhancePrompt(trimmedUserInput),
        selected_options: [],
        images: [],
        metadata: {
          timestamp: new Date().toISOString(),
          source: 'web_bridge_enhance',
        },
      }
      await handleMcpResponse(response)
    }
    catch (error) {
      console.error('MCP增强处理失败:', error)
    }
  }

  /**
   * 更新条件性提示词状态
   */
  async function handleUpdateConditionalState(promptId: string, newState: boolean) {
    try {
      await invoke('update_conditional_prompt_state', { promptId, newState })
    }
    catch (error) {
      console.error('更新条件性提示词状态失败:', error)
    }
  }

  /**
   * 更新条件性提示词激活状态
   */
  async function handleUpdateConditionalActive(promptId: string, isActive: boolean) {
    try {
      await invoke('update_conditional_prompt_active', { promptId, isActive })
    }
    catch (error) {
      console.error('更新条件性提示词激活状态失败:', error)
    }
  }

  /**
   * 更新普通提示词排序
   */
  async function handleUpdateCustomPromptOrder(promptIds: string[]) {
    if (!Array.isArray(promptIds) || promptIds.length === 0)
      return

    try {
      await invoke('update_custom_prompt_order', { promptIds })
    }
    catch (error) {
      console.error('更新提示词排序失败:', error)
    }
  }

  return {
    mcpRequest,
    showMcpPopup,
    isMuted,
    toggleMute,
    handleMcpResponse,
    handleMcpCancel,
    handleMcpCloseCurrentDialog,
    handleMcpContinue,
    handleMcpLoopReply,
    handleMcpEnhance,
    handleUpdateConditionalState,
    handleUpdateConditionalActive,
    handleUpdateCustomPromptOrder,
    showMcpDialog,
    checkMcpMode,
    setupMcpEventListener,
  }
}
