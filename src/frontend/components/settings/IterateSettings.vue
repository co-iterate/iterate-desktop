<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useLocalQrCode } from '../../composables/useLocalQrCode'
import { bridgeFetch } from '../../services/bridgeFetch'
import ConnectionRouteStatusPanel from './ConnectionRouteStatusPanel.vue'
import MobileConnectionWizard from './MobileConnectionWizard.vue'
import { buildConnectionRouteView } from './useConnectionRouteStatus'

interface TunnelStatus {
  state: 'stopped' | 'starting' | 'running' | 'error'
  mode?: string
  domain: string | null
  pid: number | null
  last_error: string | null
  recent_logs: string[]
  origin_healthy: boolean
}

interface PairedDeviceFileRoots {
  device_id: string
  device_name: string
  client_kind: string
  created_at: string
  last_seen_at: string
  file_browser_roots: string[]
}

interface PairedDeviceFileRootsResponse {
  ok: boolean
  devices: PairedDeviceFileRoots[]
}

interface CloudflareVerificationResult {
  state: string
  public_hostname: string
  health_ok: boolean
  pair_challenge_ok: boolean
  websocket_ok: boolean
  access_state: string
  error_code: string | null
  checked_at: string
}

interface CloudflareGuidedConfig {
  guided_setup_enabled: boolean
  public_hostname: string
  access_expected: boolean
  web_login_console_origin: string
  tunnel_token_saved: boolean
  last_verified_at: string | null
  last_verification: CloudflareVerificationResult | null
}

interface CloudflareGuidedConfigResponse {
  config: CloudflareGuidedConfig
}

interface CloudflareAutoSetupResponse {
  public_hostname: string
  tunnel_id: string
  tunnel_name: string
  dns_record_id: string | null
  dns_action: string
  access_app_id: string | null
  access_policy_id: string | null
  access_state: string
  verification: CloudflareVerificationResult
}

interface CloudflareWebLoginPairing {
  ok: boolean
  device_id: string
  cf_origin: string
  console_origin: string
  pair_url: string
  nonce: string
  scopes: string[]
  issued_at: string
  expires_at: string
}

interface CloudflareWebLoginPairingResponse {
  pairing: CloudflareWebLoginPairing
}

interface CloudflareWebLoginSession {
  session_id: string
  device_id: string
  cf_origin: string
  scopes: string[]
  issued_at: string
  expires_at: string
  last_seen_at: string
}

interface CloudflareWebLoginSessionsResponse {
  sessions: CloudflareWebLoginSession[]
}

interface CloudflareWebLoginRevokeSessionsResponse {
  ok: boolean
  revoked: number
}

interface RelayMacClientConfig {
  relay_url: string
  device_id: string
  local_base_url: string
  heartbeat_secs: number
  allow_recover: boolean
  relay_token: string
  clear_relay_token: boolean
  token_present: boolean
  config_path: string
  plist_path: string
  runner_path: string
}

interface RelayMacClientControlResult {
  action: string
  ok: boolean
  message: string
  configured: boolean
  runner_present: boolean
  plist_present: boolean
  launchctl_loaded: boolean
  process_running: boolean
  pid?: number | null
  config_path: string
  plist_path: string
  runner_path: string
  stdout: string
  stderr: string
}

interface CacheRouteMetrics {
  lookups?: number
  hits?: number
  misses?: number
  hit_rate_percent?: number
}

interface CacheMetrics {
  lookups?: number
  hits?: number
  misses?: number
  hit_rate_percent?: number
  writes?: number
  pruned?: number
  active_registry_fallback_hits?: number
  routes?: {
    request_id?: CacheRouteMetrics
    project_path?: CacheRouteMetrics
    fallback_route?: CacheRouteMetrics
  }
}

interface CacheStatus {
  count?: number
  ttl_secs?: number
  max_entries?: number
  metrics?: CacheMetrics
}

interface BridgeWebSocketClient {
  client_id: string
  connected_at?: string
  last_seen_at?: string
  last_message_type?: string | null
  client_kind?: string
  device_id?: string | null
  selected_transport_mode?: string | null
  selected_ws_url?: string | null
}

interface ConnectionStatusSnapshot {
  diagnosis?: { code?: string }
  local_origin?: { healthy?: boolean }
  public_tunnel?: { healthy?: boolean, health_source?: string }
  root_tunnel?: BridgeDiagnostics['root_tunnel']
}

interface BridgeDiagnostics {
  generated_at?: string
  diagnosis?: { code?: string }
  local_origin?: { healthy?: boolean }
  public_tunnel?: { healthy?: boolean, health_source?: string }
  root_tunnel?: {
    derived?: {
      ha_degraded?: boolean
      ha_ready?: boolean
      backoff_active?: boolean
      ha_active?: boolean
      backoff_remaining_secs?: number
    }
    status?: {
      diagnosis_code?: string
      structural_block?: boolean
      ha_connection_count?: number
      expected_ha_connections?: number
      backoff_remaining_secs?: number
    }
  }
  caches?: {
    mcp_state?: CacheStatus
    mcp_action?: CacheStatus
    mcp_state_count?: number
    mcp_action_count?: number
    mcp_state_ttl_secs?: number
    mcp_action_ttl_secs?: number
    mcp_state_max_entries?: number
    mcp_action_max_entries?: number
  }
  websocket?: {
    client_count?: number
    subscriber_count?: number
    registry_count?: number
    clients?: BridgeWebSocketClient[]
  }
}

const cloudflareVerificationFreshnessMs = 10 * 60 * 1000

const defaultCloudflareConfig: CloudflareGuidedConfig = {
  guided_setup_enabled: false,
  public_hostname: '',
  access_expected: false,
  web_login_console_origin: '',
  tunnel_token_saved: false,
  last_verified_at: null,
  last_verification: null,
}

const defaultRelayMacClientConfig: RelayMacClientConfig = {
  relay_url: '',
  device_id: 'local-mac',
  local_base_url: 'http://127.0.0.1:8080',
  heartbeat_secs: 15,
  allow_recover: false,
  relay_token: '',
  clear_relay_token: false,
  token_present: false,
  config_path: '',
  plist_path: '',
  runner_path: '',
}

const message = useMessage()
const status = ref<TunnelStatus>({
  state: 'stopped',
  mode: 'quick_tunnel',
  domain: null,
  pid: null,
  last_error: null,
  recent_logs: [],
  origin_healthy: false,
})
const cloudflareConfig = ref<CloudflareGuidedConfig>({ ...defaultCloudflareConfig })
const cloudflareHostname = ref('')
const cloudflareWebLoginConsoleOrigin = ref('')
const cloudflareTunnelToken = ref('')
const cloudflareAccessExpected = ref(false)
const cloudflareApiToken = ref('')
const cloudflareZoneName = ref('')
const cloudflareSubdomain = ref('login')
const cloudflareOverwriteDns = ref(false)
const cloudflareAccessEmails = ref('')
const relayMacClientConfig = ref<RelayMacClientConfig>({ ...defaultRelayMacClientConfig })
const relayMacClientTokenInput = ref('')
const isRelayMacClientSaving = ref(false)
const isRelayMacClientControlling = ref(false)
const relayMacClientError = ref('')
const relayMacClientRuntime = ref<RelayMacClientControlResult | null>(null)
const isCloudflareSaving = ref(false)
const isCloudflareVerifying = ref(false)
const isCloudflareAutoSetupLoading = ref(false)
const isCustomerTunnelLoading = ref(false)
const cloudflareError = ref('')
const isWebLoginPairingLoading = ref(false)
const webLoginPairing = ref<CloudflareWebLoginPairing | null>(null)
const isWebLoginSessionsLoading = ref(false)
const webLoginSessions = ref<CloudflareWebLoginSession[]>([])
const isLoading = ref(false)
const showMobileConnectionWizard = ref(false)
const showQuickTunnelDeveloperControls = import.meta.env.DEV
  && import.meta.env.VITE_ENABLE_QUICK_TUNNEL_TEST === '1'
const showLogs = ref(false)
const showQrCode = ref(false)
const copySuccess = ref(false)
const pairedFileRootDevices = ref<PairedDeviceFileRoots[]>([])
const selectedFileRootDeviceId = ref('')
const isFileRootLoading = ref(false)
const fileRootError = ref('')
const bridgeDiagnostics = ref<BridgeDiagnostics | null>(null)
const diagnosticsError = ref('')
const isDiagnosticsLoading = ref(false)
let pollTimer: ReturnType<typeof setInterval> | null = null
let diagnosticsPollTimer: ReturnType<typeof setInterval> | null = null
let mobileQrImageLoadStartedAt = 0

function elapsedMs(startedAt: number) {
  return Math.round(performance.now() - startedAt)
}

const statusColor = computed(() => {
  switch (status.value.state) {
    case 'running':
      return 'bg-success'
    case 'starting':
      return 'bg-warning animate-pulse'
    case 'error':
      return 'bg-error'
    default:
      return 'bg-gray-400'
  }
})

const statusText = computed(() => {
  switch (status.value.state) {
    case 'running':
      return '运行中'
    case 'starting':
      return '启动中...'
    case 'error':
      return '错误'
    default:
      return '未启动'
  }
})

function buildMobileUrl(baseUrl: string | null) {
  if (!baseUrl)
    return ''

  try {
    return new URL('/mobile', baseUrl).toString()
  }
  catch {
    const normalized = baseUrl.endsWith('/') ? baseUrl.slice(0, -1) : baseUrl
    return `${normalized}/mobile`
  }
}

const mobileEntryBaseUrl = computed(() => status.value.domain)
const mobileUrl = computed(() => buildMobileUrl(mobileEntryBaseUrl.value))

const cloudflareVerification = computed(() => cloudflareConfig.value.last_verification)
const cloudflareVerificationUsable = computed(() => {
  const verification = cloudflareVerification.value
  if (!verification || verification.state !== 'verified')
    return false
  if (!cloudflareConfig.value.tunnel_token_saved)
    return false
  if (verification.public_hostname !== cloudflareConfig.value.public_hostname)
    return false
  const checkedAt = Date.parse(verification.checked_at)
  if (!Number.isFinite(checkedAt))
    return false
  const ageMs = Date.now() - checkedAt
  return ageMs >= 0 && ageMs <= cloudflareVerificationFreshnessMs
})
const cloudflareVerified = computed(() => cloudflareVerificationUsable.value)
const cloudflareWebLoginConsoleConfigured = computed(() => Boolean(cloudflareWebLoginConsoleOrigin.value.trim()))
const customerTunnelRunning = computed(() => status.value.state === 'running' && status.value.mode === 'customer_tunnel')
const cloudflareTokenStatusText = computed(() => cloudflareConfig.value.tunnel_token_saved ? 'token 已保存' : 'token 未保存')
const relayMacTokenStatusText = computed(() =>
  relayMacClientConfig.value.token_present ? 'token 已保存' : 'token 未保存',
)
const relayMacRuntimeStatusText = computed(() => {
  const runtime = relayMacClientRuntime.value
  if (!runtime)
    return '未检查'
  if (runtime.process_running)
    return `运行中${runtime.pid ? ` · pid ${runtime.pid}` : ''}`
  if (runtime.launchctl_loaded)
    return 'LaunchAgent 已加载'
  if (runtime.plist_present)
    return '已安装未运行'
  if (runtime.configured)
    return '已配置未安装'
  return '未配置'
})
const relayMacRuntimeStatusClass = computed(() => {
  const runtime = relayMacClientRuntime.value
  if (runtime?.process_running)
    return 'bg-success/15 text-success'
  if (runtime?.launchctl_loaded || runtime?.plist_present || runtime?.configured)
    return 'bg-warning/15 text-warning'
  return 'bg-black-200'
})
const cloudflareVerificationText = computed(() => {
  const verification = cloudflareVerification.value
  if (!verification)
    return '尚未验证'
  if (verification.state === 'verified' && !cloudflareVerificationUsable.value)
    return '需重新验证'
  if (verification.state === 'verified')
    return `Verified · ${new Date(verification.checked_at).toLocaleTimeString()}`
  return `${verification.state}${verification.error_code ? ` · ${verification.error_code}` : ''}`
})

const fileRootDeviceOptions = computed(() => pairedFileRootDevices.value.map(device => ({
  label: `${device.device_name || 'Android'} · ${shortDeviceId(device.device_id)} · ${formatClientSeenAt(device.last_seen_at)}`,
  value: device.device_id,
})))

const selectedFileRootDevice = computed(() => pairedFileRootDevices.value.find(
  device => device.device_id === selectedFileRootDeviceId.value,
) || null)

const mobileEntrySourceText = computed(() => {
  if (status.value.domain)
    return '来自公网兜底域名'
  return ''
})

const pairingRouteDescription = computed(() => '使用安全连接向导时，iterate 会先复用或验证 HTTPS/WSS 公网通道，再生成一次性二维码。')

function fallbackCacheStatus(kind: 'mcp_state' | 'mcp_action'): CacheStatus {
  const caches = bridgeDiagnostics.value?.caches
  if (!caches)
    return {}

  if (kind === 'mcp_state') {
    return {
      count: caches.mcp_state_count,
      ttl_secs: caches.mcp_state_ttl_secs,
      max_entries: caches.mcp_state_max_entries,
    }
  }

  return {
    count: caches.mcp_action_count,
    ttl_secs: caches.mcp_action_ttl_secs,
    max_entries: caches.mcp_action_max_entries,
  }
}

const cacheMetricRows = computed(() => {
  const caches = bridgeDiagnostics.value?.caches
  const stateCache = caches?.mcp_state || fallbackCacheStatus('mcp_state')
  const actionCache = caches?.mcp_action || fallbackCacheStatus('mcp_action')

  return [
    {
      key: 'mcp_state',
      label: 'MCP State',
      description: 'request_sync 状态缓存',
      status: stateCache,
    },
    {
      key: 'mcp_action',
      label: 'MCP Action',
      description: 'pull_action 消费缓存',
      status: actionCache,
    },
  ]
})

const connectedAndroidClients = computed(() => {
  const clients = bridgeDiagnostics.value?.websocket?.clients || []
  return clients.filter((client) => {
    const kind = (client.client_kind || '').toLowerCase()
    return kind === 'android' || kind.includes('android')
  })
})

const connectionStatusSnapshot = computed<ConnectionStatusSnapshot | null>(() => {
  if (!bridgeDiagnostics.value)
    return null
  return {
    diagnosis: bridgeDiagnostics.value.diagnosis,
    local_origin: bridgeDiagnostics.value.local_origin,
    public_tunnel: bridgeDiagnostics.value.public_tunnel,
    root_tunnel: bridgeDiagnostics.value.root_tunnel,
  }
})

const tailscaleClientOnline = computed(() =>
  connectedAndroidClients.value.some(client => client.selected_transport_mode === 'tailscale'),
)

const tailscaleCandidateAvailable = computed(() => false)

const connectionRouteView = computed(() => buildConnectionRouteView({
  connectionStatus: connectionStatusSnapshot.value,
  localBridgeHealthy: status.value.origin_healthy,
  tailscaleClientOnline: tailscaleClientOnline.value,
  tailscaleCandidateAvailable: tailscaleCandidateAvailable.value,
}))

const {
  dataUrl: qrCodeUrl,
  error: mobileQrCodeError,
} = useLocalQrCode(mobileUrl, { width: 200 })

async function refreshStatus() {
  try {
    const result = await invoke('get_remote_tunnel_status') as TunnelStatus
    status.value = result
  }
  catch (error) {
    console.error('获取状态失败:', error)
  }
}

async function refreshCloudflareConfig() {
  cloudflareError.value = ''
  try {
    const response = await invoke('get_cloudflare_guided_config') as CloudflareGuidedConfigResponse
    cloudflareConfig.value = response.config || { ...defaultCloudflareConfig }
    cloudflareHostname.value = cloudflareConfig.value.public_hostname || ''
    cloudflareWebLoginConsoleOrigin.value = cloudflareConfig.value.web_login_console_origin || ''
    cloudflareAccessExpected.value = Boolean(cloudflareConfig.value.access_expected)
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || 'Cloudflare 配置读取失败')
  }
}

async function refreshRelayMacClientConfig() {
  relayMacClientError.value = ''
  try {
    const config = await invoke('get_relay_mac_client_config') as RelayMacClientConfig
    relayMacClientConfig.value = {
      ...defaultRelayMacClientConfig,
      ...config,
      relay_token: '',
      clear_relay_token: false,
      heartbeat_secs: Math.max(5, Number(config.heartbeat_secs || 15)),
    }
    relayMacClientTokenInput.value = ''
    await refreshRelayMacClientRuntimeStatus()
  }
  catch (error: any) {
    relayMacClientError.value = String(error?.message || error || 'Relay Mac 配置读取失败')
  }
}

async function refreshRelayMacClientRuntimeStatus() {
  try {
    relayMacClientRuntime.value = await invoke('control_relay_mac_client', { action: 'status' }) as RelayMacClientControlResult
  }
  catch (error: any) {
    relayMacClientError.value = String(error?.message || error || 'Relay Mac 状态读取失败')
  }
}

async function refreshRelayMacClientPanel() {
  await refreshRelayMacClientConfig()
}

async function saveRelayMacClientConfig() {
  isRelayMacClientSaving.value = true
  relayMacClientError.value = ''
  try {
    const relayToken = relayMacClientTokenInput.value.trim()
    const request = {
      ...relayMacClientConfig.value,
      relay_url: relayMacClientConfig.value.relay_url.trim(),
      device_id: relayMacClientConfig.value.device_id.trim() || 'local-mac',
      local_base_url: relayMacClientConfig.value.local_base_url.trim() || 'http://127.0.0.1:8080',
      heartbeat_secs: Math.max(5, Number(relayMacClientConfig.value.heartbeat_secs || 15)),
      relay_token: relayToken,
      clear_relay_token: !relayToken && relayMacClientConfig.value.clear_relay_token,
    }
    const saved = await invoke('save_relay_mac_client_config', { request }) as RelayMacClientConfig
    relayMacClientConfig.value = {
      ...defaultRelayMacClientConfig,
      ...saved,
      relay_token: '',
      clear_relay_token: false,
      heartbeat_secs: Math.max(5, Number(saved.heartbeat_secs || 15)),
    }
    relayMacClientTokenInput.value = ''
    await refreshRelayMacClientRuntimeStatus()
    message.success('Relay Mac 配置已保存')
  }
  catch (error: any) {
    relayMacClientError.value = String(error?.message || error || 'Relay Mac 配置保存失败')
    message.error(relayMacClientError.value)
  }
  finally {
    isRelayMacClientSaving.value = false
  }
}

async function controlRelayMacClient(action: 'install' | 'start' | 'stop' | 'restart' | 'health') {
  isRelayMacClientControlling.value = true
  relayMacClientError.value = ''
  try {
    const result = await invoke('control_relay_mac_client', { action }) as RelayMacClientControlResult
    relayMacClientRuntime.value = result
    if (result.ok)
      message.success(result.message)
    else
      message.error(result.message)
  }
  catch (error: any) {
    relayMacClientError.value = String(error?.message || error || 'Relay Mac 操作失败')
    message.error(relayMacClientError.value)
  }
  finally {
    isRelayMacClientControlling.value = false
  }
}

async function saveAndRestartRelayMacClient() {
  await saveRelayMacClientConfig()
  if (relayMacClientError.value)
    return
  await controlRelayMacClient('restart')
}

async function saveCloudflareConfig() {
  isCloudflareSaving.value = true
  cloudflareError.value = ''
  try {
    const response = await invoke('save_cloudflare_guided_config', {
      request: {
        public_hostname: cloudflareHostname.value.trim(),
        access_expected: cloudflareAccessExpected.value,
        web_login_console_origin: cloudflareWebLoginConsoleOrigin.value.trim(),
        tunnel_token: cloudflareTunnelToken.value.trim() || null,
      },
    }) as CloudflareGuidedConfigResponse
    cloudflareConfig.value = response.config
    cloudflareHostname.value = cloudflareConfig.value.public_hostname || ''
    cloudflareWebLoginConsoleOrigin.value = cloudflareConfig.value.web_login_console_origin || ''
    cloudflareTunnelToken.value = ''
    webLoginPairing.value = null
    webLoginSessions.value = []
    message.success('Cloudflare 配置已保存')
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || 'Cloudflare 配置保存失败')
    message.error(cloudflareError.value)
  }
  finally {
    isCloudflareSaving.value = false
  }
}

async function clearCloudflareConfig() {
  isCloudflareSaving.value = true
  cloudflareError.value = ''
  try {
    const response = await invoke('clear_cloudflare_guided_config') as CloudflareGuidedConfigResponse
    cloudflareConfig.value = response.config || { ...defaultCloudflareConfig }
    cloudflareHostname.value = ''
    cloudflareWebLoginConsoleOrigin.value = ''
    cloudflareTunnelToken.value = ''
    cloudflareAccessExpected.value = false
    webLoginPairing.value = null
    webLoginSessions.value = []
    message.success('Cloudflare 配置已清空')
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || 'Cloudflare 配置清空失败')
    message.error(cloudflareError.value)
  }
  finally {
    isCloudflareSaving.value = false
  }
}

async function autoSetupCloudflare() {
  isCloudflareAutoSetupLoading.value = true
  cloudflareError.value = ''
  webLoginPairing.value = null
  webLoginSessions.value = []
  try {
    const result = await invoke('create_cloudflare_web_login_auto_setup', {
      request: {
        api_token: cloudflareApiToken.value.trim(),
        zone_name: cloudflareZoneName.value.trim(),
        subdomain: cloudflareSubdomain.value.trim(),
        overwrite_dns: cloudflareOverwriteDns.value,
        access_emails: cloudflareAccessEmails.value
          .split(',')
          .map(email => email.trim())
          .filter(Boolean),
      },
    }) as CloudflareAutoSetupResponse
    cloudflareApiToken.value = ''
    cloudflareHostname.value = result.public_hostname
    await refreshCloudflareConfig()
    await refreshStatus()
    if (result.verification.state === 'verified') {
      const accessText = result.access_state === 'configured' ? '，Access 已创建' : ''
      message.success(`Cloudflare Tunnel 已配置${accessText}：${result.public_hostname}`)
    }
    else {
      message.warning(`Cloudflare Tunnel 已创建，验证未通过：${result.verification.error_code || result.verification.state}`)
    }
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || 'Cloudflare Tunnel 配置失败')
    message.error(cloudflareError.value)
  }
  finally {
    isCloudflareAutoSetupLoading.value = false
  }
}

async function verifyCloudflareConfig() {
  isCloudflareVerifying.value = true
  cloudflareError.value = ''
  try {
    const result = await invoke('verify_cloudflare_guided_config') as CloudflareVerificationResult
    await refreshCloudflareConfig()
    if (result.state === 'verified')
      message.success('Cloudflare Web Login 已验证')
    else
      message.warning(`Cloudflare 验证未通过：${result.error_code || result.state}`)
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || 'Cloudflare 验证失败')
    message.error(cloudflareError.value)
  }
  finally {
    isCloudflareVerifying.value = false
  }
}

async function createCloudflareWebLoginPairing() {
  isWebLoginPairingLoading.value = true
  cloudflareError.value = ''
  try {
    const response = await invoke('create_cloudflare_web_login_pairing') as CloudflareWebLoginPairingResponse
    webLoginPairing.value = response.pairing
    await refreshCloudflareWebLoginSessions()
    message.success('Web Login 链接已生成')
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || 'Web Login 链接生成失败')
    message.error(cloudflareError.value)
  }
  finally {
    isWebLoginPairingLoading.value = false
  }
}

async function refreshCloudflareWebLoginSessions() {
  isWebLoginSessionsLoading.value = true
  cloudflareError.value = ''
  try {
    const response = await invoke('list_cloudflare_web_login_sessions') as CloudflareWebLoginSessionsResponse
    webLoginSessions.value = response.sessions || []
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || 'Web Login 会话读取失败')
    message.error(cloudflareError.value)
  }
  finally {
    isWebLoginSessionsLoading.value = false
  }
}

async function revokeCloudflareWebLoginSessions() {
  isWebLoginSessionsLoading.value = true
  cloudflareError.value = ''
  try {
    const response = await invoke('revoke_cloudflare_web_login_sessions') as CloudflareWebLoginRevokeSessionsResponse
    webLoginSessions.value = []
    message.success(`已撤销 ${response.revoked || 0} 个 Web Login 会话`)
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || 'Web Login 会话撤销失败')
    message.error(cloudflareError.value)
  }
  finally {
    isWebLoginSessionsLoading.value = false
  }
}

async function startCustomerTunnel() {
  isCustomerTunnelLoading.value = true
  cloudflareError.value = ''
  try {
    await invoke('start_cloudflare_customer_tunnel')
    message.info('客户 Cloudflare Tunnel 启动中')
    startPolling()
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || '客户 Cloudflare Tunnel 启动失败')
    message.error(cloudflareError.value)
  }
  finally {
    isCustomerTunnelLoading.value = false
  }
}

async function stopCustomerTunnel() {
  isCustomerTunnelLoading.value = true
  cloudflareError.value = ''
  try {
    await invoke('stop_cloudflare_customer_tunnel')
    message.success('客户 Cloudflare Tunnel 已停止')
    stopPolling()
    await refreshStatus()
  }
  catch (error: any) {
    cloudflareError.value = String(error?.message || error || '客户 Cloudflare Tunnel 停止失败')
    message.error(cloudflareError.value)
  }
  finally {
    isCustomerTunnelLoading.value = false
  }
}

async function refreshBridgeDiagnostics() {
  isDiagnosticsLoading.value = true
  diagnosticsError.value = ''
  try {
    const response = await bridgeFetch('http://127.0.0.1:8080/api/connection-status', {
      cache: 'no-store',
    })
    const data = await response.json()
    if (!response.ok)
      throw new Error(data?.error || `HTTP ${response.status}`)
    bridgeDiagnostics.value = data as BridgeDiagnostics
  }
  catch (error: any) {
    diagnosticsError.value = String(error?.message || error || '连接诊断获取失败')
  }
  finally {
    isDiagnosticsLoading.value = false
  }
}

async function checkHealth() {
  try {
    const response = await fetch('http://127.0.0.1:8080/api/version', {
      cache: 'no-store',
      signal: AbortSignal.timeout(3000),
    })
    if (response.ok) {
      status.value.origin_healthy = true
      return
    }
  }
  catch {
    // Fall through to the Tauri command. Some app contexts can still reach 8080 through Rust.
  }

  try {
    const healthy = await invoke('check_origin_health') as boolean
    status.value.origin_healthy = healthy
    if (!healthy) {
      message.warning('8080 端口不可达，请确保 iterate 服务已启动')
    }
  }
  catch (error) {
    console.error('健康检查失败:', error)
  }
}

function startTunnel() {
  showMobileConnectionWizard.value = true
}

async function refreshPairedDeviceFileRoots() {
  isFileRootLoading.value = true
  fileRootError.value = ''
  try {
    const response = await bridgeFetch('http://127.0.0.1:8080/api/mobile/paired-device-file-roots', {
      cache: 'no-store',
    })
    const data = await response.json() as PairedDeviceFileRootsResponse & { error?: string }
    if (!response.ok || !data.ok)
      throw new Error(data.error || '设备目录授权读取失败')

    pairedFileRootDevices.value = (data.devices || []).filter(
      device => device.client_kind.toLowerCase() === 'android',
    )
    if (!pairedFileRootDevices.value.some(device => device.device_id === selectedFileRootDeviceId.value))
      selectedFileRootDeviceId.value = pairedFileRootDevices.value[0]?.device_id || ''
  }
  catch (error: any) {
    pairedFileRootDevices.value = []
    selectedFileRootDeviceId.value = ''
    fileRootError.value = String(error?.message || error || '设备目录授权读取失败')
  }
  finally {
    isFileRootLoading.value = false
  }
}

async function saveSelectedDeviceFileRoots(roots: string[]) {
  const deviceId = selectedFileRootDeviceId.value
  if (!deviceId) {
    fileRootError.value = '请先选择要授权的 Android 设备'
    return
  }

  isFileRootLoading.value = true
  fileRootError.value = ''
  try {
    const response = await bridgeFetch('http://127.0.0.1:8080/api/mobile/paired-device-file-roots', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        device_id: deviceId,
        roots,
      }),
    })
    const data = await response.json()
    if (!response.ok || !data.ok)
      throw new Error(data.error || '设备目录授权保存失败')
    await refreshPairedDeviceFileRoots()
    message.success(roots.length ? '已更新这台 Android 设备的目录授权' : '已撤销这台 Android 设备的目录授权')
  }
  catch (error: any) {
    fileRootError.value = String(error?.message || error || '设备目录授权保存失败')
    message.error(fileRootError.value)
  }
  finally {
    isFileRootLoading.value = false
  }
}

async function chooseSelectedDeviceFileRoot() {
  fileRootError.value = ''
  try {
    const selected = await invoke('select_files_and_folders', {
      defaultPath: selectedFileRootDevice.value?.file_browser_roots[0] || null,
      directoriesOnly: true,
    }) as string[]
    const root = selected[0]
    if (!root)
      return
    await saveSelectedDeviceFileRoots([root])
  }
  catch (error: any) {
    fileRootError.value = String(error?.message || error || '打开目录选择器失败')
    message.error(fileRootError.value)
  }
}

async function clearSelectedDeviceFileRoots() {
  await saveSelectedDeviceFileRoots([])
}

async function stopTunnel() {
  isLoading.value = true
  try {
    await invoke('stop_quick_tunnel')
    message.success('免费临时通道已停止；已配对设备仍然保留')
    stopPolling()
    await refreshStatus()
  }
  catch (error: any) {
    message.error(`停止失败: ${error}`)
  }
  finally {
    isLoading.value = false
  }
}

function startPolling() {
  if (pollTimer)
    return
  pollTimer = setInterval(async () => {
    await refreshStatus()
    if (status.value.state === 'running' && status.value.domain) {
      message.success('公网兜底已就绪！')
      stopPolling()
    }
    else if (status.value.state === 'error') {
      stopPolling()
    }
  }, 1000)
}

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

function startDiagnosticsPolling() {
  if (diagnosticsPollTimer)
    return
  diagnosticsPollTimer = setInterval(refreshBridgeDiagnostics, 10000)
}

function stopDiagnosticsPolling() {
  if (diagnosticsPollTimer) {
    clearInterval(diagnosticsPollTimer)
    diagnosticsPollTimer = null
  }
}

function formatMetricNumber(value?: number) {
  return typeof value === 'number' && Number.isFinite(value) ? String(value) : '0'
}

function formatHitRate(value?: number) {
  if (typeof value !== 'number' || !Number.isFinite(value))
    return '0.00%'
  return `${Number(value).toFixed(2)}%`
}

function formatTtl(seconds?: number) {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || !seconds)
    return '0s'
  if (seconds >= 3600)
    return `${Math.round(seconds / 3600)}h`
  if (seconds >= 60)
    return `${Math.round(seconds / 60)}m`
  return `${seconds}s`
}

function shortDeviceId(deviceId?: string | null) {
  if (!deviceId)
    return 'unknown'
  return deviceId.length > 8 ? deviceId.slice(0, 8) : deviceId
}

function formatClientSeenAt(value?: string) {
  if (!value)
    return '刚连接'
  const date = new Date(value)
  if (Number.isNaN(date.getTime()))
    return '刚连接'
  return date.toLocaleTimeString()
}

async function copyDomain() {
  if (!mobileUrl.value)
    return
  try {
    await navigator.clipboard.writeText(mobileUrl.value)
    copySuccess.value = true
    message.success('链接已复制')
    setTimeout(() => {
      copySuccess.value = false
    }, 2000)
  }
  catch {
    message.error('复制失败')
  }
}

function openDomain() {
  if (mobileUrl.value) {
    window.open(mobileUrl.value, '_blank')
  }
}

function onMobileQrImageLoad() {
  console.info('[IterateSettings][MobilePairing] mobile_qr_image_load', {
    srcLength: qrCodeUrl.value.length,
    elapsedMs: elapsedMs(mobileQrImageLoadStartedAt),
  })
}

function onMobileQrImageError() {
  console.warn('[IterateSettings][MobilePairing] mobile_qr_image_error', {
    srcLength: qrCodeUrl.value.length,
    elapsedMs: elapsedMs(mobileQrImageLoadStartedAt),
  })
}

watch(qrCodeUrl, (url) => {
  if (!url)
    return
  mobileQrImageLoadStartedAt = performance.now()
  console.info('[IterateSettings][MobilePairing] mobile_qr_image_start', {
    srcLength: url.length,
  })
})

watch(mobileQrCodeError, (error) => {
  if (!error)
    return
  console.warn('[IterateSettings][MobilePairing] mobile_qr_generation_error', {
    error: error.message,
  })
})

onMounted(async () => {
  await refreshStatus()
  await refreshCloudflareConfig()
  await refreshRelayMacClientConfig()
  await refreshCloudflareWebLoginSessions()
  await checkHealth()
  if (status.value.origin_healthy)
    await refreshPairedDeviceFileRoots()
  await refreshBridgeDiagnostics()
  startDiagnosticsPolling()

  // 如果已经在运行，开始轮询
  if (status.value.state === 'starting') {
    startPolling()
  }
})

onUnmounted(() => {
  stopPolling()
  stopDiagnosticsPolling()
})
</script>

<template>
  <n-space vertical size="large">
    <!-- 说明 -->
    <div class="flex items-start">
      <div class="w-1.5 h-1.5 bg-info rounded-full mr-3 flex-shrink-0 mt-2" />
      <div>
        <div class="text-sm font-medium leading-relaxed mb-1">
          使用方式
        </div>
        <div class="text-xs opacity-60 leading-relaxed">
          {{ pairingRouteDescription }}<br>
          Tailscale、Funnel、Cloudflare 或 LAN 会按当前候选列表排序展示。
        </div>
      </div>
    </div>

    <!-- 连接通道三分法 -->
    <div class="flex items-center justify-between mb-1">
      <div class="text-sm font-medium leading-relaxed">
        手机连接通道
      </div>
      <n-button size="tiny" text :loading="isDiagnosticsLoading" @click="async () => { await checkHealth(); await refreshBridgeDiagnostics() }">
        刷新
      </n-button>
    </div>
    <ConnectionRouteStatusPanel :route-view="connectionRouteView" />

    <!-- Mac Relay Client -->
    <div class="p-3 bg-black-100 rounded-lg">
      <div class="flex items-start justify-between gap-3 mb-3">
        <div>
          <div class="text-sm font-medium text-primary">
            Mac Relay Client
          </div>
          <div class="text-xs opacity-60">
            常驻 Mac 出站 WebSocket，用于已授权的移动设备通过 Relay 下发受限恢复命令。
          </div>
        </div>
        <div class="flex flex-col items-end gap-1">
          <span
            class="text-[11px] leading-relaxed px-2 py-0.5 rounded"
            :class="relayMacRuntimeStatusClass"
          >
            {{ relayMacRuntimeStatusText }}
          </span>
          <span
            class="text-[11px] leading-relaxed px-2 py-0.5 rounded"
            :class="relayMacClientConfig.token_present ? 'bg-success/15 text-success' : 'bg-black-200'"
          >
            {{ relayMacTokenStatusText }}
          </span>
        </div>
      </div>

      <n-space vertical size="small">
        <n-input
          v-model:value="relayMacClientConfig.relay_url"
          size="small"
          placeholder="wss://relay.example.com/mac/ws"
          :disabled="isRelayMacClientSaving"
        />
        <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
          <n-input
            v-model:value="relayMacClientConfig.device_id"
            size="small"
            placeholder="local-mac"
            :disabled="isRelayMacClientSaving"
          />
          <n-input
            v-model:value="relayMacClientConfig.local_base_url"
            size="small"
            placeholder="http://127.0.0.1:8080"
            :disabled="isRelayMacClientSaving"
          />
          <n-input-number
            v-model:value="relayMacClientConfig.heartbeat_secs"
            size="small"
            :min="5"
            :max="300"
            :disabled="isRelayMacClientSaving"
          />
        </div>
        <n-input
          v-model:value="relayMacClientTokenInput"
          size="small"
          type="password"
          show-password-on="click"
          placeholder="Relay token；留空则保留已保存 token"
          :disabled="isRelayMacClientSaving"
        />
        <n-checkbox
          v-model:checked="relayMacClientConfig.clear_relay_token"
          size="small"
          :disabled="isRelayMacClientSaving || Boolean(relayMacClientTokenInput.trim()) || !relayMacClientConfig.token_present"
        >
          清空已保存 token
        </n-checkbox>
        <div class="flex items-center justify-between gap-3">
          <n-switch
            v-model:value="relayMacClientConfig.allow_recover"
            size="small"
            :disabled="isRelayMacClientSaving"
          >
            <template #checked>
              真实恢复
            </template>
            <template #unchecked>
              模拟恢复
            </template>
          </n-switch>
          <div class="flex flex-wrap gap-2">
            <n-button
              size="tiny"
              text
              :loading="isRelayMacClientSaving || isRelayMacClientControlling"
              @click="refreshRelayMacClientPanel"
            >
              刷新
            </n-button>
            <n-button
              size="tiny"
              type="primary"
              :loading="isRelayMacClientSaving"
              :disabled="!relayMacClientConfig.relay_url.trim()"
              @click="saveRelayMacClientConfig"
            >
              保存
            </n-button>
            <n-button
              size="tiny"
              type="primary"
              ghost
              :loading="isRelayMacClientSaving || isRelayMacClientControlling"
              :disabled="!relayMacClientConfig.relay_url.trim()"
              @click="saveAndRestartRelayMacClient"
            >
              安装/重启
            </n-button>
            <n-button
              size="tiny"
              text
              :loading="isRelayMacClientControlling"
              @click="controlRelayMacClient('health')"
            >
              体检
            </n-button>
            <n-button
              size="tiny"
              text
              type="warning"
              :loading="isRelayMacClientControlling"
              @click="controlRelayMacClient('stop')"
            >
              停止
            </n-button>
          </div>
        </div>
        <div class="grid grid-cols-1 gap-1 text-[11px] opacity-60 leading-relaxed">
          <div>config: {{ relayMacClientConfig.config_path || '未初始化' }}</div>
          <div>plist: {{ relayMacClientConfig.plist_path || '未初始化' }}</div>
          <div>runner: {{ relayMacClientConfig.runner_path || '未初始化' }}</div>
        </div>
        <div v-if="relayMacClientRuntime?.message" class="text-xs leading-relaxed opacity-75">
          {{ relayMacClientRuntime.message }}
        </div>
        <pre
          v-if="relayMacClientRuntime?.stdout || relayMacClientRuntime?.stderr"
          class="max-h-32 overflow-auto rounded bg-black-200/60 p-2 text-[11px] leading-relaxed whitespace-pre-wrap"
        >{{ [relayMacClientRuntime.stdout, relayMacClientRuntime.stderr].filter(Boolean).join('\n') }}</pre>
        <div v-if="relayMacClientError" class="text-xs text-error leading-relaxed">
          {{ relayMacClientError }}
        </div>
      </n-space>
    </div>

    <!-- 客户 Cloudflare Web Login -->
    <div class="p-3 bg-black-100 rounded-lg">
      <div class="flex items-start justify-between gap-3 mb-3">
        <div>
          <div class="text-sm font-medium text-primary">
            Cloudflare Web Login
          </div>
          <div class="text-xs opacity-60">
            客户自有 hostname 的持久通道；填写 Access 邮箱可自动配置 allow-email 策略，留空则只配置 Tunnel。
          </div>
        </div>
        <span
          class="text-[11px] leading-relaxed px-2 py-0.5 rounded"
          :class="cloudflareVerified ? 'bg-success/15 text-success' : 'bg-black-200'"
        >
          {{ cloudflareVerificationText }}
        </span>
      </div>

      <n-space vertical size="small">
        <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
          <n-input
            v-model:value="cloudflareApiToken"
            size="small"
            type="password"
            show-password-on="click"
            placeholder="Cloudflare API Token"
            :disabled="isCloudflareAutoSetupLoading"
          />
          <n-input
            v-model:value="cloudflareZoneName"
            size="small"
            placeholder="域名：example.com"
            :disabled="isCloudflareAutoSetupLoading"
          />
          <n-input
            v-model:value="cloudflareSubdomain"
            size="small"
            placeholder="子域名：login"
            :disabled="isCloudflareAutoSetupLoading"
          />
        </div>
        <n-input
          v-model:value="cloudflareAccessEmails"
          size="small"
          placeholder="可选 Access 允许邮箱，多个用逗号分隔；留空则只配置 Tunnel"
          :disabled="isCloudflareAutoSetupLoading"
        />
        <div class="flex items-center justify-between gap-3">
          <n-switch
            v-model:value="cloudflareOverwriteDns"
            size="small"
            :disabled="isCloudflareAutoSetupLoading"
          >
            <template #checked>
              覆盖 DNS
            </template>
            <template #unchecked>
              保留 DNS
            </template>
          </n-switch>
          <n-button
            size="tiny"
            type="primary"
            :loading="isCloudflareAutoSetupLoading"
            :disabled="!cloudflareApiToken.trim() || !cloudflareZoneName.trim() || !cloudflareSubdomain.trim()"
            @click="autoSetupCloudflare"
          >
            {{ cloudflareAccessEmails.trim() ? '配置 Tunnel + Access' : '配置 Tunnel' }}
          </n-button>
        </div>
        <div class="h-px bg-black-200" />
        <n-input
          v-model:value="cloudflareHostname"
          size="small"
          placeholder="https://iterate.example.com"
          :disabled="isCloudflareSaving || isCloudflareVerifying"
        />
        <n-input
          v-model:value="cloudflareWebLoginConsoleOrigin"
          size="small"
          placeholder="Web Console origin，例如 https://app.example.com"
          :disabled="isCloudflareSaving || isCloudflareVerifying"
        />
        <n-input
          v-model:value="cloudflareTunnelToken"
          size="small"
          type="password"
          show-password-on="click"
          placeholder="粘贴 Cloudflare tunnel token；保存后清空"
          :disabled="isCloudflareSaving || isCloudflareVerifying"
        />
        <div class="flex items-center justify-between gap-3">
          <div class="text-xs opacity-60">
            {{ cloudflareTokenStatusText }}
          </div>
          <n-switch
            v-model:value="cloudflareAccessExpected"
            size="small"
            :disabled="isCloudflareSaving || isCloudflareVerifying"
          >
            <template #checked>
              Access
            </template>
            <template #unchecked>
              No Access
            </template>
          </n-switch>
        </div>

        <div class="flex flex-wrap gap-2">
          <n-button
            size="tiny"
            type="primary"
            :loading="isCloudflareSaving"
            @click="saveCloudflareConfig"
          >
            保存
          </n-button>
          <n-button
            size="tiny"
            :loading="isCloudflareVerifying"
            :disabled="!cloudflareConfig.tunnel_token_saved"
            @click="verifyCloudflareConfig"
          >
            验证
          </n-button>
          <n-button
            size="tiny"
            :loading="isWebLoginPairingLoading"
            :disabled="!cloudflareVerified || !cloudflareWebLoginConsoleConfigured"
            @click="createCloudflareWebLoginPairing"
          >
            生成登录链接
          </n-button>
          <n-button
            v-if="!customerTunnelRunning"
            size="tiny"
            :loading="isCustomerTunnelLoading"
            :disabled="!cloudflareConfig.tunnel_token_saved || !cloudflareConfig.public_hostname"
            @click="startCustomerTunnel"
          >
            启动通道
          </n-button>
          <n-button
            v-else
            size="tiny"
            type="error"
            :loading="isCustomerTunnelLoading"
            @click="stopCustomerTunnel"
          >
            停止通道
          </n-button>
          <n-button
            size="tiny"
            text
            :loading="isCloudflareSaving"
            @click="clearCloudflareConfig"
          >
            清空
          </n-button>
        </div>

        <div v-if="cloudflareVerification" class="grid grid-cols-3 gap-2 text-[11px]">
          <div>
            <div class="opacity-50">
              Health
            </div>
            <div class="font-medium" :class="cloudflareVerification.health_ok ? 'text-success' : 'text-error'">
              {{ cloudflareVerification.health_ok ? 'ok' : 'fail' }}
            </div>
          </div>
          <div>
            <div class="opacity-50">
              Pair
            </div>
            <div class="font-medium" :class="cloudflareVerification.pair_challenge_ok ? 'text-success' : 'text-error'">
              {{ cloudflareVerification.pair_challenge_ok ? 'ok' : 'fail' }}
            </div>
          </div>
          <div>
            <div class="opacity-50">
              WS
            </div>
            <div class="font-medium" :class="cloudflareVerification.websocket_ok ? 'text-success' : 'text-error'">
              {{ cloudflareVerification.websocket_ok ? 'ok' : 'fail' }}
            </div>
          </div>
        </div>

        <div v-if="webLoginPairing" class="space-y-1 text-[11px] leading-relaxed">
          <div class="opacity-50">
            Web Login Pair Page
          </div>
          <n-input
            :value="webLoginPairing.pair_url"
            size="tiny"
            readonly
          />
          <div class="opacity-60">
            {{ webLoginPairing.device_id }} · {{ new Date(webLoginPairing.expires_at).toLocaleTimeString() }}
          </div>
        </div>

        <div class="space-y-2 text-[11px] leading-relaxed">
          <div class="flex items-center justify-between gap-2">
            <div>
              <div class="opacity-50">
                Web Login Sessions
              </div>
              <div class="opacity-60">
                {{ webLoginSessions.length }} active
              </div>
            </div>
            <div class="flex gap-2">
              <n-button
                size="tiny"
                text
                :loading="isWebLoginSessionsLoading"
                @click="refreshCloudflareWebLoginSessions"
              >
                刷新
              </n-button>
              <n-button
                size="tiny"
                text
                type="error"
                :loading="isWebLoginSessionsLoading"
                :disabled="webLoginSessions.length === 0"
                @click="revokeCloudflareWebLoginSessions"
              >
                撤销全部
              </n-button>
            </div>
          </div>
          <div v-if="webLoginSessions.length > 0" class="space-y-1">
            <div
              v-for="session in webLoginSessions"
              :key="session.session_id"
              class="rounded bg-black-200/60 px-2 py-1"
            >
              <div class="font-medium">
                {{ session.device_id }}
              </div>
              <div class="opacity-60">
                {{ session.cf_origin }} · expires {{ new Date(session.expires_at).toLocaleTimeString() }}
              </div>
            </div>
          </div>
        </div>

        <div v-if="cloudflareError" class="text-xs text-error leading-relaxed">
          {{ cloudflareError }}
        </div>
      </n-space>
    </div>

    <!-- Bridge 缓存命中率 -->
    <div class="p-3 bg-black-100 rounded-lg">
      <div class="flex items-start justify-between gap-3 mb-3">
        <div>
          <div class="text-sm font-medium leading-relaxed">
            Bridge 缓存命中率
          </div>
          <div class="text-xs opacity-60">
            {{ bridgeDiagnostics?.generated_at ? `更新于 ${new Date(bridgeDiagnostics.generated_at).toLocaleTimeString()}` : '等待诊断数据' }}
          </div>
        </div>
        <n-button size="tiny" text :loading="isDiagnosticsLoading" @click="refreshBridgeDiagnostics">
          刷新
        </n-button>
      </div>

      <div v-if="diagnosticsError" class="text-xs text-error leading-relaxed">
        {{ diagnosticsError }}
      </div>

      <div v-else class="divide-y divide-black-200/70">
        <div
          v-for="row in cacheMetricRows"
          :key="row.key"
          class="py-2 first:pt-0 last:pb-0"
        >
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <div class="text-xs font-medium leading-relaxed">
                {{ row.label }}
              </div>
              <div class="text-[11px] opacity-55 leading-relaxed">
                {{ row.description }} · {{ formatMetricNumber(row.status.count) }}/{{ formatMetricNumber(row.status.max_entries) }} · TTL {{ formatTtl(row.status.ttl_secs) }}
              </div>
            </div>
            <div class="text-right">
              <div class="text-base font-semibold tabular-nums leading-tight">
                {{ formatHitRate(row.status.metrics?.hit_rate_percent) }}
              </div>
              <div class="text-[11px] opacity-55">
                hit rate
              </div>
            </div>
          </div>

          <div class="grid grid-cols-4 gap-2 mt-2 text-[11px]">
            <div>
              <div class="opacity-50">
                查询
              </div>
              <div class="font-medium tabular-nums">
                {{ formatMetricNumber(row.status.metrics?.lookups) }}
              </div>
            </div>
            <div>
              <div class="opacity-50">
                命中
              </div>
              <div class="font-medium tabular-nums">
                {{ formatMetricNumber(row.status.metrics?.hits) }}
              </div>
            </div>
            <div>
              <div class="opacity-50">
                写入
              </div>
              <div class="font-medium tabular-nums">
                {{ formatMetricNumber(row.status.metrics?.writes) }}
              </div>
            </div>
            <div>
              <div class="opacity-50">
                清理
              </div>
              <div class="font-medium tabular-nums">
                {{ formatMetricNumber(row.status.metrics?.pruned) }}
              </div>
            </div>
          </div>

          <div class="flex flex-wrap gap-1.5 mt-2">
            <span class="text-[11px] leading-relaxed bg-black-200 px-2 py-0.5 rounded">
              request_id {{ formatHitRate(row.status.metrics?.routes?.request_id?.hit_rate_percent) }}
            </span>
            <span class="text-[11px] leading-relaxed bg-black-200 px-2 py-0.5 rounded">
              project_path {{ formatHitRate(row.status.metrics?.routes?.project_path?.hit_rate_percent) }}
            </span>
            <span class="text-[11px] leading-relaxed bg-black-200 px-2 py-0.5 rounded">
              fallback {{ formatHitRate(row.status.metrics?.routes?.fallback_route?.hit_rate_percent) }}
            </span>
            <span
              v-if="row.key === 'mcp_state'"
              class="text-[11px] leading-relaxed bg-black-200 px-2 py-0.5 rounded"
            >
              registry fallback {{ formatMetricNumber(row.status.metrics?.active_registry_fallback_hits) }}
            </span>
          </div>
        </div>
      </div>
    </div>

    <template v-if="showQuickTunnelDeveloperControls">
      <!-- 开发测试通道状态与控制 -->
      <div class="flex items-center justify-between">
        <div class="flex items-center">
          <div class="w-1.5 h-1.5 rounded-full mr-3 flex-shrink-0" :class="statusColor" />
          <div>
            <div class="text-sm font-medium leading-relaxed">
              公网兜底状态
            </div>
            <div class="text-xs opacity-60">
              {{ statusText }}
              <template v-if="status.pid">
                (PID: {{ status.pid }})
              </template>
            </div>
          </div>
        </div>
        <n-space>
          <n-button
            v-if="status.state === 'stopped' || status.state === 'error'"
            size="small"
            type="primary"
            :loading="isLoading"
            @click="startTunnel"
          >
            安全连接 Android
          </n-button>
          <n-button
            v-else
            size="small"
            type="error"
            :loading="isLoading"
            @click="stopTunnel"
          >
            停止
          </n-button>
        </n-space>
      </div>

      <!-- 手机浏览器入口展示 -->
      <div v-if="mobileUrl" class="p-3 bg-black-100 rounded-lg">
        <div class="flex items-center justify-between mb-2">
          <div>
            <div class="text-sm font-medium text-primary">
              手机浏览器入口
            </div>
            <div v-if="mobileEntrySourceText" class="text-xs opacity-60">
              {{ mobileEntrySourceText }}
            </div>
          </div>
          <n-space size="small">
            <n-button size="tiny" text @click="showQrCode = !showQrCode">
              {{ showQrCode ? '隐藏二维码' : '显示二维码' }}
            </n-button>
          </n-space>
        </div>

        <div class="flex items-center gap-2 mb-2">
          <code class="text-xs bg-black-200 px-2 py-1 rounded flex-1 truncate">
            {{ mobileUrl }}
          </code>
          <n-button size="tiny" :type="copySuccess ? 'success' : 'default'" @click="copyDomain">
            {{ copySuccess ? '已复制' : '复制' }}
          </n-button>
          <n-button size="tiny" type="primary" @click="openDomain">
            打开
          </n-button>
        </div>

        <!-- 二维码 -->
        <div v-if="showQrCode" class="flex justify-center pt-2">
          <img
            :src="qrCodeUrl"
            alt="QR Code"
            class="w-32 h-32 rounded bg-white p-1"
            @load="onMobileQrImageLoad"
            @error="onMobileQrImageError"
          >
        </div>
      </div>
    </template>

    <div
      v-if="status.origin_healthy"
      class="p-3 bg-black-100 rounded-lg"
    >
      <div class="flex items-center justify-between mb-2">
        <div>
          <div class="text-sm font-medium text-primary">
            Android 安全连接
          </div>
          <div class="text-xs opacity-60">
            使用已登记并验证的正式公网路线；尚未配置时会提供安全的 AI 配置提示词。
          </div>
        </div>
        <n-button size="small" type="primary" @click="showMobileConnectionWizard = true">
          连接 Android
        </n-button>
      </div>
      <div class="text-[11px] opacity-60 leading-relaxed bg-black-200/70 px-2 py-1.5 rounded">
        正式配置与瞬时健康分开保存；暂时故障会先自动恢复，不会退回测试通道。
      </div>
      <div class="grid gap-2 mt-3 pt-3 border-t border-black-200/70">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <div class="text-xs font-medium leading-relaxed">
              Android 目录浏览授权
            </div>
            <div class="text-[11px] opacity-60 leading-relaxed">
              只授权选中的设备；文件系统根 / 永不允许。
            </div>
          </div>
          <n-button size="tiny" text :loading="isFileRootLoading" @click="refreshPairedDeviceFileRoots">
            刷新
          </n-button>
        </div>

        <n-select
          v-if="fileRootDeviceOptions.length"
          v-model:value="selectedFileRootDeviceId"
          size="small"
          :options="fileRootDeviceOptions"
          placeholder="选择 Android 设备"
        />
        <div v-else class="text-[11px] opacity-60 leading-relaxed bg-black-200/70 px-2 py-1.5 rounded">
          暂无可授权的 Android 配对记录。
        </div>

        <div
          v-if="selectedFileRootDevice"
          class="text-[11px] leading-relaxed bg-black-200/70 px-2 py-1.5 rounded"
        >
          <template v-if="selectedFileRootDevice.file_browser_roots.length">
            <div class="opacity-60 mb-0.5">
              当前授权根
            </div>
            <code class="break-all">{{ selectedFileRootDevice.file_browser_roots.join(' · ') }}</code>
          </template>
          <template v-else>
            <span class="opacity-60">当前只有活跃项目根可浏览。</span>
          </template>
        </div>

        <div class="flex justify-end gap-2">
          <n-button
            size="tiny"
            :disabled="!selectedFileRootDeviceId"
            :loading="isFileRootLoading"
            @click="chooseSelectedDeviceFileRoot"
          >
            选择授权目录
          </n-button>
          <n-button
            size="tiny"
            type="error"
            ghost
            :disabled="!selectedFileRootDevice?.file_browser_roots.length"
            :loading="isFileRootLoading"
            @click="clearSelectedDeviceFileRoots"
          >
            撤销目录授权
          </n-button>
        </div>
      </div>
      <div v-if="fileRootError" class="text-xs text-error leading-relaxed mt-2">
        {{ fileRootError }}
      </div>
    </div>

    <!-- 等待公网兜底域名 -->
    <div v-else-if="status.state === 'starting'" class="p-3 bg-black-100 rounded-lg">
      <div class="flex items-center gap-2">
        <div class="w-4 h-4 border-2 border-primary border-t-transparent rounded-full animate-spin" />
        <span class="text-sm opacity-60">正在启动公网兜底，请稍候...</span>
      </div>
    </div>

    <!-- 错误信息 -->
    <div v-if="status.last_error && status.state === 'error'" class="p-3 bg-error-100 rounded-lg">
      <div class="text-sm font-medium text-error mb-1">
        错误信息
      </div>
      <div class="text-xs opacity-80 break-all">
        {{ status.last_error }}
      </div>
    </div>

    <!-- 日志展示 -->
    <div v-if="status.recent_logs.length > 0">
      <n-button size="tiny" text @click="showLogs = !showLogs">
        {{ showLogs ? '隐藏日志' : '显示日志' }} ({{ status.recent_logs.length }})
      </n-button>
      <div v-if="showLogs" class="mt-2 p-2 bg-black-100 rounded text-xs font-mono max-h-32 overflow-y-auto">
        <div v-for="(log, i) in status.recent_logs.slice(-10)" :key="i" class="opacity-60 truncate">
          {{ log }}
        </div>
      </div>
    </div>

    <MobileConnectionWizard v-model:show="showMobileConnectionWizard" />
  </n-space>
</template>
