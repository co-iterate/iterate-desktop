export type MobileConnectionStage
  = | 'bootstrap'
    | 'preparing_bridge'
    | 'setup_required'
    | 'recovering_formal_route'
    | 'issuing_pairing'
    | 'waiting_for_claim'
    | 'waiting_for_connection'
    | 'complete'
    | 'repair_required'
    | 'expired'
    | 'error'
    | 'cancelled'

export interface MobileConnectionState {
  stage: MobileConnectionStage
  pairingSessionId: string
  expiresAt: string
  deviceId: string
  transportMode: string
  error: null | {
    stage: MobileConnectionStage
    code: string
    message: string
    retryable: boolean
  }
}

export type MobileConnectionEvent
  = | { type: 'BRIDGE_PREPARING' }
    | { type: 'SETUP_REQUIRED' }
    | { type: 'FORMAL_ROUTE_RECOVERING' }
    | { type: 'FORMAL_ROUTE_REPAIR_REQUIRED', code: string, message: string }
    | { type: 'ROUTE_VERIFIED' }
    | { type: 'PAIRING_REFRESHING' }
    | { type: 'PAIRING_ISSUED', sessionId: string, expiresAt: string }
    | { type: 'SESSION_CLAIMED', sessionId: string, deviceId: string }
    | { type: 'SESSION_CONNECTED', sessionId: string, deviceId: string, transportMode: string }
    | { type: 'SESSION_EXPIRED', sessionId: string }
    | { type: 'SESSION_FAILED', sessionId: string, code: string, message: string }
    | {
      type: 'FAILED'
      stage: MobileConnectionStage
      code: string
      message: string
      retryable: boolean
    }
    | { type: 'RETRY' }
    | { type: 'CANCEL' }

export interface PairingPayloadExpiry {
  expires_at?: string
  expiresAt?: string
}

export interface PairingStatusCandidate {
  transport_mode?: string
  base_url?: string
  ws_url?: string
  health?: string | null
  disabled?: boolean | null
}

export interface FormalMobileRouteStatus {
  configured: boolean
  transport?: string | null
  base_url?: string | null
  configured_at?: string | null
  formal_route_generation?: number | null
  health: string
  health_checked_at?: string | null
  last_verified_at?: string | null
  endpoint_identity_ok?: boolean
  repair_reason?: string | null
}

export interface PairingStatusResponse {
  ok?: boolean
  transport_mode?: string
  base_url?: string
  ws_url?: string
  candidates?: PairingStatusCandidate[]
  formal_route?: FormalMobileRouteStatus
  capabilities?: {
    quick_tunnel_test?: boolean
  }
  error?: string
}

export type MobileConnectionBootstrapAction
  = 'issue_pairing'
    | 'setup_required'
    | 'recover_formal_route'

export interface PairingSessionConnectionSnapshot {
  session_id: string
  state: string
  device_id?: string | null
  selected_transport_mode?: string | null
}

export interface CompactMobilePairingSource {
  version: number
  device_id: string
  issued_at: string
  expires_at: string
  transport_mode: string
  base_url: string
  ws_url: string
  candidates?: unknown[]
  pairing_token: string
}

export function normalizePairingStatusCandidates(status: PairingStatusResponse): PairingStatusCandidate[] {
  if (Array.isArray(status.candidates))
    return status.candidates
  if (status.transport_mode || status.base_url || status.ws_url)
    return [status]
  return []
}

export function isHealthySecurePairingCandidate(candidate: PairingStatusCandidate): boolean {
  if (candidate.disabled)
    return false
  const mode = String(candidate.transport_mode || '').toLowerCase()
  if (!['public_tunnel', 'cloudflare_tunnel', 'relay', 'cloud_hub'].includes(mode))
    return false
  const health = String(candidate.health || '').toLowerCase()
  if (!['healthy', 'ok'].includes(health))
    return false
  try {
    const url = new URL(String(candidate.base_url || ''))
    const wsUrl = new URL(String(candidate.ws_url || ''))
    return url.protocol === 'https:' && wsUrl.protocol === 'wss:'
  }
  catch {
    return false
  }
}

function sameOrigin(left: string, right: string) {
  try {
    return new URL(left).origin === new URL(right).origin
  }
  catch {
    return false
  }
}

export function resolveMobileConnectionBootstrap(
  pairingStatus: PairingStatusResponse,
): MobileConnectionBootstrapAction {
  const formalRoute = pairingStatus.formal_route
  if (!formalRoute?.configured)
    return 'setup_required'

  const configuredBaseUrl = String(formalRoute.base_url || '')
  const routeIsHealthy = String(formalRoute.health || '').toLowerCase() === 'healthy'
    && formalRoute.endpoint_identity_ok === true
  const matchingCandidate = normalizePairingStatusCandidates(pairingStatus).some(candidate => (
    (candidate.transport_mode === 'public_tunnel' || (formalRoute.transport === 'cloud_hub' && candidate.transport_mode === 'cloud_hub'))
    && isHealthySecurePairingCandidate(candidate)
    && sameOrigin(String(candidate.base_url || ''), configuredBaseUrl)
  ))
  return routeIsHealthy && matchingCandidate ? 'issue_pairing' : 'recover_formal_route'
}

export function buildCompactMobilePairingPayload(payload: CompactMobilePairingSource) {
  return {
    version: payload.version,
    device_id: payload.device_id,
    issued_at: payload.issued_at,
    expires_at: payload.expires_at,
    transport_mode: payload.transport_mode,
    base_url: payload.base_url,
    ws_url: payload.ws_url,
    candidates: payload.candidates || [],
    pairing_token: payload.pairing_token,
  }
}

export function connectedPairingSessionEvent(
  session: PairingSessionConnectionSnapshot,
  expectedSessionId: string,
): Extract<MobileConnectionEvent, { type: 'SESSION_CONNECTED' }> | null {
  const deviceId = session.device_id?.trim() || ''
  const transportMode = session.selected_transport_mode?.trim() || ''
  if (
    session.state !== 'connected'
    || session.session_id !== expectedSessionId
    || !deviceId
    || !transportMode
  ) {
    return null
  }
  return {
    type: 'SESSION_CONNECTED',
    sessionId: session.session_id,
    deviceId,
    transportMode,
  }
}

export function singleFlight<T>(operation: () => Promise<T>): () => Promise<T> {
  let active: Promise<T> | null = null
  return () => {
    if (!active) {
      active = operation().finally(() => {
        active = null
      })
    }
    return active
  }
}

export function initialMobileConnectionState(): MobileConnectionState {
  return {
    stage: 'bootstrap',
    pairingSessionId: '',
    expiresAt: '',
    deviceId: '',
    transportMode: '',
    error: null,
  }
}

export function mobileConnectionFailureText(state: MobileConnectionState, transientError: string): string {
  if (state.error)
    return `[${state.error.code}] ${state.error.message}`
  return transientError.trim()
}

export function secondsUntilExpiry(expiresAt: string, now = Date.now()): number {
  const expiry = Date.parse(expiresAt)
  if (!Number.isFinite(expiry))
    return 0
  return Math.max(0, Math.ceil((expiry - now) / 1000))
}

export function pairingPayloadIsFresh(
  payload: PairingPayloadExpiry | null | undefined,
  now = Date.now(),
  minimumRemainingSeconds = 30,
): boolean {
  const expiresAt = payload?.expires_at ?? payload?.expiresAt
  return Boolean(expiresAt && secondsUntilExpiry(expiresAt, now) > minimumRemainingSeconds)
}

function withStage(state: MobileConnectionState, stage: MobileConnectionStage): MobileConnectionState {
  return { ...state, stage, error: null }
}

export function reduceMobileConnection(
  state: MobileConnectionState,
  event: MobileConnectionEvent,
): MobileConnectionState {
  switch (event.type) {
    case 'BRIDGE_PREPARING':
      return state.stage === 'bootstrap' ? withStage(state, 'preparing_bridge') : state
    case 'SETUP_REQUIRED':
      return withStage(state, 'setup_required')
    case 'FORMAL_ROUTE_RECOVERING':
      return withStage(state, 'recovering_formal_route')
    case 'FORMAL_ROUTE_REPAIR_REQUIRED':
      return {
        ...state,
        stage: 'repair_required',
        error: {
          stage: 'recovering_formal_route',
          code: event.code,
          message: event.message,
          retryable: true,
        },
      }
    case 'ROUTE_VERIFIED':
      return withStage(state, 'issuing_pairing')
    case 'PAIRING_REFRESHING':
      return ['waiting_for_claim', 'waiting_for_connection'].includes(state.stage)
        ? withStage(state, 'issuing_pairing')
        : state
    case 'PAIRING_ISSUED':
      if (state.stage !== 'issuing_pairing' || !event.sessionId || !event.expiresAt)
        return state
      return {
        ...state,
        stage: 'waiting_for_claim',
        pairingSessionId: event.sessionId,
        expiresAt: event.expiresAt,
        deviceId: '',
        transportMode: '',
        error: null,
      }
    case 'SESSION_CLAIMED':
      if (state.stage !== 'waiting_for_claim' || event.sessionId !== state.pairingSessionId || !event.deviceId)
        return state
      return { ...state, stage: 'waiting_for_connection', deviceId: event.deviceId, error: null }
    case 'SESSION_CONNECTED':
      if (
        !['waiting_for_claim', 'waiting_for_connection'].includes(state.stage)
        || event.sessionId !== state.pairingSessionId
        || !event.deviceId
        || !event.transportMode
      ) {
        return state
      }
      return {
        ...state,
        stage: 'complete',
        deviceId: event.deviceId,
        transportMode: event.transportMode,
        error: null,
      }
    case 'SESSION_EXPIRED':
      if (!['waiting_for_claim', 'waiting_for_connection'].includes(state.stage) || event.sessionId !== state.pairingSessionId)
        return state
      return withStage(state, 'expired')
    case 'SESSION_FAILED':
      if (!['waiting_for_claim', 'waiting_for_connection'].includes(state.stage) || event.sessionId !== state.pairingSessionId)
        return state
      return {
        ...state,
        stage: 'error',
        error: {
          stage: state.stage,
          code: event.code,
          message: event.message,
          retryable: true,
        },
      }
    case 'FAILED':
      if (state.stage === 'cancelled')
        return state
      return {
        ...state,
        stage: 'error',
        error: {
          stage: event.stage,
          code: event.code,
          message: event.message,
          retryable: event.retryable,
        },
      }
    case 'RETRY':
      if (state.stage === 'expired' || state.error?.stage === 'issuing_pairing')
        return withStage(state, 'issuing_pairing')
      return initialMobileConnectionState()
    case 'CANCEL':
      return withStage(state, 'cancelled')
    default:
      return state
  }
}
