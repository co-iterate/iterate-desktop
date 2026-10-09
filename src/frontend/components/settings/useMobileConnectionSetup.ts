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

export function buildAliyunIpRouteSetupPrompt() {
  return `请为 iterate 配置独立于 Windows 的阿里云中心，公网入口使用可信 IP 证书的 https://服务器公网IP:8443。

1. 只读检查现有 8443、443、云服务及两台源设备的有效配置；不要覆盖健康的既有路线。
2. 中心运行独立 iterate-hub 与持久 SQLite；Windows/Mac 使用各自源凭据主动连接，源端保留原注册与获胜账本。手机继承现有权限，Mac 只发布获准文字镜像。
3. HTTPS/WSS 代理指向云端 loopback hub，不转发到 Windows。保留可信 IP 证书与主机身份校验；管理员接口限运维通道。不得修改现有 443 网站或使用跳过证书校验的客户端。
4. 切换前冻结并排空，查询未知结果，取得当轮每个源的真实全入口屏障；保留 5311 主会话。验证 /health 的 service=iterate-hub、protocol=1，/ws 未授权返回 401，以及 Windows 离线时 Mac 镜像仍可用。
5. 实际部署及验收成功后运行 --mobile-route-register --transport cloud_hub --base-url "https://服务器公网IP:8443" --source ai_configured，再运行 --mobile-route-verify。未验证前不改变正式配置。

不得在输出中展示 SSH 私钥、凭据、配对链接或原始日志。任何需要新增费用、改变现有公网服务或安全边界的步骤，先说明影响并取得用户决定。`
}

export function buildFormalRouteSetupPrompt() {
  return `请帮我为当前电脑上的 iterate 配置一条长期稳定、重启后仍可用的正式 Android 公网路线，优先使用 Cloudflare Named Tunnel；不要创建临时测试路线。

平台识别：
- 执行任何安装或修改前，先识别当前系统是 macOS、Windows 还是 Linux，并定位实际可运行的 iterate 与官方 cloudflared；不要猜测安装路径。
- macOS 与 Linux 的 cloudflared 用户配置通常位于 ~/.cloudflared；Windows 通常位于 %USERPROFILE%\\.cloudflared。路径含空格时必须正确引用。
- 使用当前系统原生的持久服务方式：macOS 使用 launchd，Windows 使用 Windows Service，Linux 使用 systemd。若系统不受支持或无法安全持久化，停止并说明原因。

安全边界：
- 先只读确认 iterate 本机 Bridge 可访问，并检查是否已有正式路线或 cloudflared 服务；不得覆盖健康的现有配置。
- 如果需要登录 Cloudflare、选择账号或域名、修改 DNS、授予管理员权限或提供任何凭据，必须暂停，让我本人在官方界面或终端中完成；不要替我点击或猜测。
- 禁止向我索取 token、密钥、Cookie、二维码、配对链接或原始日志。
- 禁止读取、打印、复制或上传任何凭据文件及其内容，包括上述 cloudflared 配置目录下的证书、凭据 JSON 和配置中的敏感值。
- 禁止使用 Quick Tunnel；禁止使用 Relay；禁止关闭鉴权或扩大公网权限来绕过验证。

执行目标：
1. 检查 http://127.0.0.1:8080/api/version 与本机 Bridge；异常时先做最小恢复。
2. 在需要我登录、选域名、改 DNS 或授权前明确说明目的并暂停。
3. 建立用户选择域名下的 Named Tunnel，把 HTTPS/WSS 安全转发到 127.0.0.1:8080，并通过当前系统的原生服务设置为开机后稳定运行。
4. 验证公网 /.well-known/iterate/health、HTTPS 和 WebSocket 都属于当前电脑。
5. 通过服务状态与一次可控重启验证恢复能力；若需要重启整台电脑，必须先征得我确认。
6. 验证成功后，使用第一步定位到的 iterate 可执行文件运行：--mobile-route-register --transport cloudflare_named_tunnel --base-url "https://用户确认的域名" --source ai_configured。
7. 再运行 --mobile-route-status，只报告操作系统、脱敏后的域名、健康状态、配置时间和回滚步骤；不得报告任何凭据内容。

如果任一需要用户决定或授权的步骤尚未完成，就停在该步骤等待，不得用测试通道代替正式配置。`
}

export function buildFormalRouteRepairPrompt(baseUrl: string, code: string, transport?: string | null) {
  const safeBaseUrl = /^https:\/\/[a-z0-9.-]+(?::\d+)?$/i.test(baseUrl.trim())
    ? baseUrl.trim()
    : '已配置的正式 HTTPS 域名（请从 iterate 脱敏状态中读取）'
  const safeCode = String(code || 'formal_route_unhealthy').replace(/[^\w.:-]/g, '_').slice(0, 120)
  if (transport === 'cloud_hub') {
    return `请只检查已配置的 iterate 阿里云中心 ${safeBaseUrl}，错误码 ${safeCode}。运行 --mobile-route-status 和 --mobile-route-verify，确认 TLS 证书及主机身份、/health 的 service=iterate-hub 与 protocol=1、/ws 未授权返回 401。分别检查 Windows/Mac 源客户端的独立连接与原账本；Windows 离线不应使获准 Mac 镜像消失。不要重建 Windows 反向 SSH 隧道，不关闭 5311，不输出 token，不重放确认未知的动作。变更或受控接管前先对账现有任务。`
  }
  if (transport === 'aliyun_ssh_reverse_tunnel') {
    return `请只修复当前电脑上 iterate 已配置的阿里云 IP + SSH 正式手机路线。

已配置地址：${safeBaseUrl}
脱敏错误码：${safeCode}

先运行 --mobile-route-status 和 --mobile-route-verify；检查本机 8080、Windows 出站 SSH 自动重连、阿里云回环 18080、IP:8443 的可信 HTTPS 证书及 WSS，并验证公网端点属于当前安装。确认 nginx 保留公网 Host（含端口）、传递真实客户端 X-Forwarded-For，且无凭据访问 /api/config、/api/active-sessions 与 /ws WebSocket Upgrade 均返回 401。只恢复这条路线，保留现有 443 网站和其他接口，不得关闭 Bridge 鉴权。不要输出私钥、凭据或配对链接；需要改变安全边界或产生费用时先请用户决定。完成后再次运行 --mobile-route-verify，并报告实际恢复动作和结果。`
  }
  return `请只修复当前电脑上 iterate 已经配置的正式 Android 公网路线，不要创建或切换到另一条路线。

已配置地址：${safeBaseUrl}
脱敏错误码：${safeCode}

安全边界：
- 先识别当前系统是 macOS、Windows 还是 Linux，定位实际可运行的 iterate，再运行 --mobile-route-status 和 --mobile-route-verify。
- 只读检查本机 Bridge、现有 Named Tunnel、当前系统的持久服务、HTTPS/WSS 与端点身份；不要把一种系统的服务命令套到另一种系统。
- 如果需要登录 Cloudflare、选择账号或域名、修改 DNS、授予管理员权限或提供任何凭据，必须暂停，让我本人完成。
- 禁止索取 token、密钥、Cookie、二维码或配对链接；禁止读取、打印、复制或上传 cloudflared 用户配置目录内任何凭据内容。
- 禁止使用 Quick Tunnel；禁止使用 Relay；禁止删除现有正式配置、降低鉴权或另建临时公网地址。
- 只允许做与现有 ${safeBaseUrl} 直接相关、可回滚的本机恢复；云端或管理员操作必须先说明并等待我确认。

完成后再次运行 --mobile-route-verify，并验证当前系统的持久服务能恢复；只报告操作系统、根因、实际恢复动作、脱敏健康状态和回滚方式。若仍失败，保留现有配置并报告最少的人工步骤。`
}

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
    return await requestJson<PairingStatusResponse>(`${pairingBaseUrl}/api/android/pairing/status`)
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
        dispatch({ type: 'SETUP_REQUIRED' })
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
        `${pairingBaseUrl}/api/android/pairing`,
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
        `${pairingBaseUrl}/api/android/pairing/sessions/${encodeURIComponent(sessionId)}`,
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
