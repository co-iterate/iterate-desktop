import type {
  FormalMobileRouteStatus,
  MobileConnectionState,
  PairingStatusResponse,
} from './mobileConnectionMachine.ts'
import { invoke } from '@tauri-apps/api/core'
import { computed, onUnmounted, ref, watch } from 'vue'
import { bridgeFetch } from '../../services/bridgeFetch.ts'
import {
  buildCompactMobilePairingPayload,
  connectedPairingSessionEvent,
  initialMobileConnectionState,
  isHealthySecurePairingCandidate,
  pairingPayloadIsFresh,
  reduceMobileConnection,
  resolveMobileConnectionBootstrap,
  secondsUntilExpiry,
  singleFlight,
} from './mobileConnectionMachine.ts'
import { buildFormalRouteRepairPrompt, buildFormalRouteSetupPrompt } from './mobileConnectionPrompts.ts'

export interface MobilePairingCandidate {
  transport_mode: string
  base_url: string
  ws_url: string
  health?: string | null
  disabled?: boolean | null
  warning?: string | null
}

export interface MobilePairingPayload {
  version: number
  pairing_session_id: string
  device_id: string
  device_name: string
  transport_mode: string
  base_url: string
  ws_url: string
  candidates?: MobilePairingCandidate[]
  pairing_token: string
  issued_at: string
  expires_at: string
  warning?: string | null
}

export interface MobilePairingSession {
  session_id: string
  state: 'pending' | 'claimed' | 'connected' | 'expired' | 'failed' | string
  expires_at: string
  device_id?: string | null
  device_name?: string | null
  client_kind?: string | null
  claimed_at?: string | null
  connected_at?: string | null
  selected_transport_mode?: string | null
}

interface SetupOptions {
  pairingBaseUrl?: string
  requestTimeoutMs?: number
  pollIntervalMs?: number
  recoveryAttempts?: number
  recoveryDelayMs?: number
}

const DEFAULT_PAIRING_BASE_URL = 'http://127.0.0.1:8080'

function trimBaseUrl(value: string) {
  return value.trim().replace(/\/$/, '') || DEFAULT_PAIRING_BASE_URL
}

function encodeBase64Url(value: string) {
  const bytes = new TextEncoder().encode(value)
  let binary = ''
  for (const byte of bytes)
    binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
}

function errorCode(error: unknown, fallback: string) {
  if (error instanceof DOMException && error.name === 'AbortError')
    return 'request_timeout'
  const value = String((error as { message?: unknown } | null)?.message || error || fallback).trim()
  if (/fetch is aborted|aborterror|aborted/i.test(value))
    return 'request_timeout'
  return value.replace(/^\[|\]$/g, '') || fallback
}

function delay(ms: number) {
  return new Promise<void>(resolve => setTimeout(resolve, ms))
}

function sameOrigin(left: string, right: string) {
  try {
    return new URL(left).origin === new URL(right).origin
  }
  catch {
    return false
  }
}

export function useMobileConnectionSetup(options: SetupOptions = {}) {
  const pairingBaseUrl = trimBaseUrl(options.pairingBaseUrl || DEFAULT_PAIRING_BASE_URL)
  const requestTimeoutMs = options.requestTimeoutMs ?? 12_000
  const pollIntervalMs = options.pollIntervalMs ?? 1500
  const recoveryAttempts = Math.max(1, options.recoveryAttempts ?? 3)
  const recoveryDelayMs = options.recoveryDelayMs ?? 800

  const state = ref<MobileConnectionState>(initialMobileConnectionState())
  const pairingPayload = ref<MobilePairingPayload | null>(null)
  const pairingSession = ref<MobilePairingSession | null>(null)
  const formalRoute = ref<FormalMobileRouteStatus | null>(null)
  const error = ref('')
  const notice = ref('')
  const bridgeOriginHealthy = ref(false)
  const now = ref(Date.now())

  let activeController: AbortController | null = null
  let pollInFlight = false
  let pollTimer: ReturnType<typeof setTimeout> | null = null
  let clockTimer: ReturnType<typeof setInterval> | null = null
  let disposed = false
  let lifecycleGeneration = 0
  let issueFreshPairing: () => Promise<void>

  const remainingSeconds = computed(() => secondsUntilExpiry(state.value.expiresAt, now.value))
  const pairingImportUrl = computed(() => {
    if (!pairingPayload.value)
      return ''
    const compact = buildCompactMobilePairingPayload(pairingPayload.value)
    return `iterate://pairing?payload=${encodeURIComponent(encodeBase64Url(JSON.stringify(compact)))}`
  })
  const qrSource = computed(() => pairingImportUrl.value)

  function dispatch(event: Parameters<typeof reduceMobileConnection>[1]) {
    state.value = reduceMobileConnection(state.value, event)
  }

  function lifecycleIsActive(generation: number) {
    return !disposed && generation === lifecycleGeneration && state.value.stage !== 'cancelled'
  }

  function clearPollTimer() {
    if (pollTimer) {
      clearTimeout(pollTimer)
      pollTimer = null
    }
  }

  function abortActiveRequest() {
    activeController?.abort()
    activeController = null
  }

  function stopTimers() {
    clearPollTimer()
    if (clockTimer) {
      clearInterval(clockTimer)
      clockTimer = null
    }
  }

  function pairingViewCanResumeWithoutBootstrap() {
    return state.value.stage === 'waiting_for_claim'
      && pairingPayloadIsFresh(pairingPayload.value, now.value, 0)
  }

  function resetForReopen() {
    lifecycleGeneration += 1
    abortActiveRequest()
    stopTimers()
    pairingPayload.value = null
    pairingSession.value = null
    formalRoute.value = null
    error.value = ''
    notice.value = ''
    now.value = Date.now()
    state.value = initialMobileConnectionState()
  }

  async function requestJson<T>(url: string): Promise<T> {
    abortActiveRequest()
    const controller = new AbortController()
    activeController = controller
    const timeout = setTimeout(() => controller.abort(), requestTimeoutMs)
    try {
      const response = await bridgeFetch(url, {
        signal: controller.signal,
        cache: 'no-store',
      })
      const data = await response.json() as T & { error?: string }
      if (!response.ok)
        throw new Error(data?.error || `HTTP ${response.status}`)
      return data
    }
    finally {
      clearTimeout(timeout)
      if (activeController === controller)
        activeController = null
    }
  }

  async function readPairingStatus() {
    return await requestJson<PairingStatusResponse>(`${pairingBaseUrl}/api/mobile/pairing/status`)
  }

  function startClock() {
    if (!clockTimer)
      clockTimer = setInterval(() => { now.value = Date.now() }, 1000)
  }

  function schedulePairingPoll() {
    clearPollTimer()
    if (disposed || !['waiting_for_claim', 'waiting_for_connection'].includes(state.value.stage))
      return
    pollTimer = setTimeout(() => void pollPairingSession(), pollIntervalMs)
  }

  async function recoverFormalRoute(generation: number) {
    dispatch({ type: 'FORMAL_ROUTE_RECOVERING' })
    for (let attempt = 0; attempt < recoveryAttempts; attempt += 1) {
      if (attempt > 0)
        await delay(recoveryDelayMs * attempt)
      if (!lifecycleIsActive(generation))
        return
      try {
        await invoke('recover_bridge_origin')
        if (!lifecycleIsActive(generation))
          return
        if (formalRoute.value?.transport === 'self_hosted_frp') {
          try {
            await invoke('ensure_managed_mobile_route')
          }
          catch (cause) {
            // A manually configured frp route keeps its existing lifecycle.
            if (errorCode(cause, '') !== 'managed_route_not_owned')
              throw cause
          }
        }
        if (!lifecycleIsActive(generation))
          return
        const pairingStatus = await readPairingStatus()
        if (!lifecycleIsActive(generation))
          return
        if (pairingStatus.formal_route?.configured)
          formalRoute.value = pairingStatus.formal_route
        if (resolveMobileConnectionBootstrap(pairingStatus) === 'issue_pairing') {
          dispatch({ type: 'ROUTE_VERIFIED' })
          await issueFreshPairing()
          return
        }
      }
      catch (cause) {
        if (!lifecycleIsActive(generation))
          return
        error.value = errorCode(cause, 'formal_route_recovery_failed')
      }
    }
    const code = formalRoute.value?.repair_reason || error.value || 'formal_route_unhealthy'
    error.value = code
    dispatch({
      type: 'FORMAL_ROUTE_REPAIR_REQUIRED',
      code,
      message: '已保留正式公网配置，但自动恢复后仍暂时不可用。',
    })
  }

  async function bootstrap() {
    if (disposed)
      return
    now.value = Date.now()
    if (pairingViewCanResumeWithoutBootstrap()) {
      error.value = ''
      notice.value = ''
      startClock()
      schedulePairingPoll()
      return
    }
    if (state.value.stage !== 'bootstrap')
      resetForReopen()
    const generation = lifecycleGeneration
    error.value = ''
    notice.value = ''
    dispatch({ type: 'BRIDGE_PREPARING' })
    try {
      const recovery = await invoke<{ healthy?: boolean }>('recover_bridge_origin')
      if (!lifecycleIsActive(generation))
        return
      bridgeOriginHealthy.value = recovery.healthy === true
      const pairingStatus = await readPairingStatus()
      if (!lifecycleIsActive(generation))
        return
      formalRoute.value = pairingStatus.formal_route || null
      const action = resolveMobileConnectionBootstrap(pairingStatus)
      if (action === 'setup_required') {
        notice.value = '正在为这台电脑开通连接…'
        try {
          const managedRoute = await invoke<FormalMobileRouteStatus>('ensure_managed_mobile_route')
          if (!lifecycleIsActive(generation))
            return
          formalRoute.value = managedRoute
          if (!formalRoute.value.configured || formalRoute.value.health !== 'healthy' || !formalRoute.value.endpoint_identity_ok)
            throw new Error('managed_route_not_ready')
          notice.value = ''
          dispatch({ type: 'ROUTE_VERIFIED' })
          await issueFreshPairing()
        }
        catch (cause) {
          if (!lifecycleIsActive(generation))
            return
          const code = errorCode(cause, 'managed_service_unavailable')
          notice.value = ''
          // 不支持托管的平台，或未配置托管服务的构建（如社区构建）：直接进入自建连接设置
          if (code === 'managed_platform_unsupported' || code === 'managed_not_configured') {
            dispatch({ type: 'SETUP_REQUIRED' })
            return
          }
          error.value = code
          dispatch({ type: 'FAILED', stage: 'preparing_bridge', code, message: code === 'managed_service_busy' || code === 'managed_connection_busy'
            ? '连接服务正在忙，请稍后重试。'
            : code.startsWith('managed_proxy')
              ? '当前系统代理暂不支持自动连接，请检查网络设置。'
              : code.startsWith('managed_keychain') || code === 'managed_credentials_invalid'
                ? '无法读取本机连接凭据，请解锁 Mac 并检查钥匙串授权。'
                : code === 'formal_route_changed' || code === 'managed_route_not_owned'
                  ? '连接设置已发生变化，已保留当前路线，请重新检测。'
                  : '暂时未能准备好连接，请检查网络后重试。', retryable: true })
        }
        return
      }
      if (action === 'recover_formal_route') {
        await recoverFormalRoute(generation)
        return
      }
      dispatch({ type: 'ROUTE_VERIFIED' })
      await issueFreshPairing()
    }
    catch (cause) {
      if (!lifecycleIsActive(generation))
        return
      const code = errorCode(cause, 'bridge_or_mcp_not_ready')
      error.value = code
      if (formalRoute.value?.configured) {
        dispatch({
          type: 'FORMAL_ROUTE_REPAIR_REQUIRED',
          code,
          message: '正式公网配置仍保留，但本机检查暂时失败。',
        })
      }
      else {
        dispatch({
          type: 'FAILED',
          stage: 'preparing_bridge',
          code,
          message: code === 'request_timeout' ? '本机连接检查超时，请重新检测。' : '本机 Bridge 或 MCP 尚未就绪。',
          retryable: true,
        })
      }
    }
  }

  async function issueFreshPairingOperation() {
    const previousPayload = pairingPayload.value
    const previousSession = pairingSession.value
    const canRestorePrevious = state.value.stage === 'waiting_for_claim'
      && pairingPayloadIsFresh(previousPayload, Date.now(), 0)
    if (['waiting_for_claim', 'waiting_for_connection'].includes(state.value.stage))
      dispatch({ type: 'PAIRING_REFRESHING' })
    if (state.value.stage === 'expired')
      dispatch({ type: 'RETRY' })
    if (state.value.stage !== 'issuing_pairing')
      return
    const generation = lifecycleGeneration
    clearPollTimer()
    try {
      const data = await requestJson<{ ok?: boolean, pairing?: MobilePairingPayload, error?: string }>(
        `${pairingBaseUrl}/api/mobile/pairing`,
      )
      if (!lifecycleIsActive(generation))
        return
      const payload = data.pairing
      if (!payload || payload.version !== 2 || !payload.pairing_session_id)
        throw new Error(data.error || 'invalid_pairing_payload')
      if (!pairingPayloadIsFresh(payload, Date.now(), 30))
        throw new Error('pairing_payload_expiring')
      const configuredBaseUrl = formalRoute.value?.base_url || ''
      const selectedCandidate = payload.candidates?.find(candidate => (
        candidate.transport_mode === payload.transport_mode
        && candidate.base_url === payload.base_url
        && candidate.ws_url === payload.ws_url
      ))
      if (
        payload.transport_mode !== 'public_tunnel'
        || !sameOrigin(payload.base_url, configuredBaseUrl)
        || !selectedCandidate
        || !isHealthySecurePairingCandidate(selectedCandidate)
      ) {
        throw new Error('endpoint_proof_failed')
      }

      pairingPayload.value = payload
      pairingSession.value = null
      now.value = Date.now()
      error.value = ''
      dispatch({
        type: 'PAIRING_ISSUED',
        sessionId: payload.pairing_session_id,
        expiresAt: payload.expires_at,
      })
      startClock()
      schedulePairingPoll()
    }
    catch (cause) {
      if (!lifecycleIsActive(generation))
        return
      const code = errorCode(cause, 'pairing_issue_failed')
      if (canRestorePrevious && previousPayload) {
        pairingPayload.value = previousPayload
        pairingSession.value = previousSession
        error.value = ''
        notice.value = '新二维码获取失败，已保留仍有效的原二维码。'
        dispatch({
          type: 'PAIRING_ISSUED',
          sessionId: previousPayload.pairing_session_id,
          expiresAt: previousPayload.expires_at,
        })
        startClock()
        schedulePairingPoll()
        return
      }
      pairingPayload.value = null
      pairingSession.value = null
      error.value = code
      if (formalRoute.value?.configured) {
        dispatch({
          type: 'FORMAL_ROUTE_REPAIR_REQUIRED',
          code,
          message: '正式公网配置仍保留，但当前无法安全签发二维码。',
        })
      }
      else {
        dispatch({
          type: 'FAILED',
          stage: 'issuing_pairing',
          code,
          message: '配对信息获取失败。',
          retryable: true,
        })
      }
    }
  }

  issueFreshPairing = singleFlight(issueFreshPairingOperation)

  async function pollPairingSession() {
    if (disposed || pollInFlight || !state.value.pairingSessionId)
      return
    if (remainingSeconds.value <= 0) {
      dispatch({ type: 'SESSION_EXPIRED', sessionId: state.value.pairingSessionId })
      await issueFreshPairing()
      return
    }
    const generation = lifecycleGeneration
    const sessionId = state.value.pairingSessionId
    pollInFlight = true
    try {
      const data = await requestJson<{ ok?: boolean, session?: MobilePairingSession }>(
        `${pairingBaseUrl}/api/mobile/pairing/sessions/${encodeURIComponent(sessionId)}`,
      )
      if (!lifecycleIsActive(generation) || data.session?.session_id !== sessionId)
        return
      const session = data.session
      pairingSession.value = session
      const connectedEvent = connectedPairingSessionEvent(session, sessionId)
      if (connectedEvent) {
        dispatch(connectedEvent)
        stopTimers()
      }
      else if (session.state === 'claimed' && session.device_id && state.value.stage === 'waiting_for_claim') {
        dispatch({ type: 'SESSION_CLAIMED', sessionId, deviceId: session.device_id })
      }
      else if (session.state === 'expired') {
        dispatch({ type: 'SESSION_EXPIRED', sessionId })
        await issueFreshPairing()
      }
      else if (session.state === 'failed') {
        dispatch({
          type: 'SESSION_FAILED',
          sessionId,
          code: 'pairing_session_failed',
          message: '手机授权未能保存，请重新扫描新的二维码。',
        })
        stopTimers()
      }
    }
    catch (cause) {
      if (!(cause instanceof DOMException && cause.name === 'AbortError'))
        error.value = errorCode(cause, 'pairing_status_failed')
    }
    finally {
      pollInFlight = false
      schedulePairingPoll()
    }
  }

  async function retry() {
    abortActiveRequest()
    clearPollTimer()
    error.value = ''
    const failedStage = state.value.error?.stage
    dispatch({ type: 'RETRY' })
    if (state.value.stage === 'issuing_pairing' || failedStage === 'issuing_pairing')
      await issueFreshPairing()
    else
      await bootstrap()
  }

  function cancel() {
    now.value = Date.now()
    const preservePairingView = pairingViewCanResumeWithoutBootstrap()
    lifecycleGeneration += 1
    abortActiveRequest()
    stopTimers()
    error.value = ''
    notice.value = ''
    if (preservePairingView)
      return
    pairingPayload.value = null
    pairingSession.value = null
    dispatch({ type: 'CANCEL' })
  }

  function dispose() {
    if (disposed)
      return
    disposed = true
    lifecycleGeneration += 1
    abortActiveRequest()
    stopTimers()
  }

  watch(remainingSeconds, (seconds) => {
    if (
      state.value.expiresAt
      && seconds <= 0
      && ['waiting_for_claim', 'waiting_for_connection'].includes(state.value.stage)
    ) {
      void issueFreshPairing()
    }
  })

  onUnmounted(dispose)

  return {
    state,
    pairingPayload,
    pairingSession,
    formalRoute,
    pairingImportUrl,
    qrSource,
    remainingSeconds,
    error,
    notice,
    bridgeOriginHealthy,
    aiSetupPrompt: computed(() => (
      formalRoute.value?.configured
        ? buildFormalRouteRepairPrompt(formalRoute.value.base_url || '', state.value.error?.code || error.value, formalRoute.value.transport)
        : buildFormalRouteSetupPrompt()
    )),
    bootstrap,
    issueFreshPairing,
    pollPairingSession,
    retry,
    cancel,
    dispose,
  }
}
