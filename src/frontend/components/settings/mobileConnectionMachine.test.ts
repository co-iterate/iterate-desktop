/* eslint-disable test/no-import-node-test -- this repository executes source-contract tests with `node --test` */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  buildCompactMobilePairingPayload,
  connectedPairingSessionEvent,
  initialMobileConnectionState,
  isHealthySecurePairingCandidate,
  pairingPayloadIsFresh,
  reduceMobileConnection,
  resolveMobileConnectionBootstrap,
  singleFlight,
} from './mobileConnectionMachine.ts'

const healthyPublic = {
  transport_mode: 'public_tunnel',
  base_url: 'https://iterate.example.com',
  ws_url: 'wss://iterate.example.com/ws',
  health: 'healthy',
  disabled: false,
}

const healthyFormalRoute = {
  configured: true,
  transport: 'cloudflare_named_tunnel',
  base_url: 'https://iterate.example.com',
  health: 'healthy',
  last_verified_at: '2026-08-16T00:00:00Z',
  endpoint_identity_ok: true,
  repair_reason: null,
}

test('configured and healthy formal route issues pairing directly', () => {
  assert.equal(
    resolveMobileConnectionBootstrap({
      formal_route: healthyFormalRoute,
      candidates: [healthyPublic],
    }),
    'issue_pairing',
  )
})

test('unconfigured production profile requires the AI setup handoff', () => {
  assert.equal(
    resolveMobileConnectionBootstrap({
      formal_route: {
        configured: false,
        health: 'unknown',
      },
      candidates: [],
    }),
    'setup_required',
  )
})

test('a reachable candidate cannot make an unconfigured install trust a developer default', () => {
  assert.equal(
    resolveMobileConnectionBootstrap({
      formal_route: {
        configured: false,
        health: 'unknown',
      },
      candidates: [healthyPublic],
    }),
    'setup_required',
  )
})

test('configured but unhealthy formal route recovers instead of returning to setup', () => {
  assert.equal(
    resolveMobileConnectionBootstrap({
      formal_route: {
        ...healthyFormalRoute,
        health: 'degraded',
        endpoint_identity_ok: false,
        repair_reason: 'probe_timeout',
      },
      candidates: [],
    }),
    'recover_formal_route',
  )
})

test('configured route still requires a matching healthy secure candidate before issuing', () => {
  assert.equal(
    resolveMobileConnectionBootstrap({
      formal_route: healthyFormalRoute,
      candidates: [{ ...healthyPublic, base_url: 'https://other.example.com' }],
    }),
    'recover_formal_route',
  )
})

test('pairing candidate gate requires healthy HTTPS and WSS', () => {
  assert.equal(isHealthySecurePairingCandidate(healthyPublic), true)
  assert.equal(isHealthySecurePairingCandidate({ ...healthyPublic, ws_url: 'ws://iterate.example.com/ws' }), false)
  assert.equal(isHealthySecurePairingCandidate({ ...healthyPublic, health: 'degraded' }), false)
  assert.equal(isHealthySecurePairingCandidate({ ...healthyPublic, disabled: true }), false)
  assert.equal(isHealthySecurePairingCandidate({ ...healthyPublic, transport_mode: 'lan_fallback' }), false)
})

test('state machine cannot display a QR before route verification', () => {
  let state = initialMobileConnectionState()
  state = reduceMobileConnection(state, { type: 'BRIDGE_PREPARING' })
  state = reduceMobileConnection(state, { type: 'FORMAL_ROUTE_RECOVERING' })
  const premature = reduceMobileConnection(state, {
    type: 'PAIRING_ISSUED',
    sessionId: 'session',
    expiresAt: new Date(Date.now() + 60_000).toISOString(),
  })
  assert.equal(premature.stage, 'recovering_formal_route')

  state = reduceMobileConnection(state, { type: 'ROUTE_VERIFIED' })
  state = reduceMobileConnection(state, {
    type: 'PAIRING_ISSUED',
    sessionId: 'session',
    expiresAt: new Date(Date.now() + 60_000).toISOString(),
  })
  assert.equal(state.stage, 'waiting_for_claim')
})

test('formal setup and repair are distinct stable states', () => {
  const setup = reduceMobileConnection(initialMobileConnectionState(), { type: 'SETUP_REQUIRED' })
  assert.equal(setup.stage, 'setup_required')

  const recovering = reduceMobileConnection(initialMobileConnectionState(), { type: 'FORMAL_ROUTE_RECOVERING' })
  const repair = reduceMobileConnection(recovering, {
    type: 'FORMAL_ROUTE_REPAIR_REQUIRED',
    code: 'probe_timeout',
    message: '正式公网暂时不可用。',
  })
  assert.equal(repair.stage, 'repair_required')
  assert.equal(repair.error?.code, 'probe_timeout')
})

test('an expired pairing session can return to the guarded issuing stage', () => {
  let state = reduceMobileConnection(initialMobileConnectionState(), { type: 'ROUTE_VERIFIED' })
  state = reduceMobileConnection(state, {
    type: 'PAIRING_ISSUED',
    sessionId: 'session-expiring',
    expiresAt: new Date(Date.now() + 60_000).toISOString(),
  })
  state = reduceMobileConnection(state, {
    type: 'SESSION_EXPIRED',
    sessionId: 'session-expiring',
  })
  assert.equal(state.stage, 'expired')
  state = reduceMobileConnection(state, { type: 'RETRY' })
  assert.equal(state.stage, 'issuing_pairing')
})

test('connected session event rejects stale or incomplete snapshots', () => {
  assert.deepEqual(
    connectedPairingSessionEvent({
      session_id: 's1',
      state: 'connected',
      device_id: 'iphone',
      selected_transport_mode: 'cloudflare_tunnel',
    }, 's1'),
    {
      type: 'SESSION_CONNECTED',
      sessionId: 's1',
      deviceId: 'iphone',
      transportMode: 'cloudflare_tunnel',
    },
  )
  assert.equal(connectedPairingSessionEvent({
    session_id: 'stale',
    state: 'connected',
    device_id: 'iphone',
    selected_transport_mode: 'cloudflare_tunnel',
  }, 's1'), null)
})

test('pairing freshness enforces a minimum lifetime', () => {
  const now = Date.now()
  assert.equal(pairingPayloadIsFresh({ expires_at: new Date(now + 31_000).toISOString() }, now, 30), true)
  assert.equal(pairingPayloadIsFresh({ expires_at: new Date(now + 30_000).toISOString() }, now, 30), false)
})

test('compact QR payload keeps only fields required by Android import', () => {
  const payload = buildCompactMobilePairingPayload({
    version: 2,
    device_id: 'mac',
    issued_at: '2026-08-14T00:00:00Z',
    expires_at: '2026-08-14T00:10:00Z',
    transport_mode: 'cloudflare_tunnel',
    base_url: 'https://quick.trycloudflare.com',
    ws_url: 'wss://quick.trycloudflare.com/ws',
    pairing_token: 'one-use-token',
  })
  assert.deepEqual(Object.keys(payload), [
    'version',
    'device_id',
    'issued_at',
    'expires_at',
    'transport_mode',
    'base_url',
    'ws_url',
    'candidates',
    'pairing_token',
  ])
})

test('singleFlight coalesces duplicate pairing issue requests', async () => {
  let calls = 0
  let release!: () => void
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  const operation = singleFlight(async () => {
    calls += 1
    await gate
    return calls
  })
  const first = operation()
  const second = operation()
  assert.equal(first, second)
  assert.equal(calls, 1)
  release()
  await first
})

test('production wizard exposes formal setup and repair prompts without Quick or Relay fallback', async () => {
  const setup = await readFile(new URL('./useMobileConnectionSetup.ts', import.meta.url), 'utf8')
  const wizard = await readFile(new URL('./MobileConnectionWizard.vue', import.meta.url), 'utf8')

  assert.doesNotMatch(setup, /cloudflareApiToken|zoneName|subdomain|create_cloudflare_web_login_auto_setup/)
  assert.doesNotMatch(setup, /start_quick_tunnel|stop_quick_tunnel|get_quick_tunnel_status/)
  assert.doesNotMatch(setup, /trycloudflare\.com|免费临时通道/)
  assert.match(setup, /buildFormalRouteSetupPrompt/)
  assert.match(setup, /buildFormalRouteRepairPrompt/)
  assert.match(setup, /登录 Cloudflare[\s\S]*必须暂停/)
  assert.match(setup, /禁止[\s\S]*token/)
  assert.match(wizard, /复制 AI 配置提示词/)
  assert.match(wizard, /title="连接 Android"/)
  assert.match(wizard, /Android 一次性连接二维码/)
  assert.doesNotMatch(wizard, /iPhone/)
  assert.match(wizard, /修复现有配置/)
  assert.match(wizard, /state\.stage === 'setup_required'/)
  assert.match(wizard, /state\.stage === 'repair_required'/)
  assert.doesNotMatch(wizard, /免费临时通道|trycloudflare\.com|开启免费临时通道/)
  assert.match(wizard, /progress-ring/)
  assert.match(wizard, /case 'bootstrap': return '正在开始检查本机 Bridge 与可用连接路线。'/)
  assert.match(wizard, /case 'preparing_bridge': return '正在准备本机 Bridge，并检查可用的安全公网通道。'/)
  assert.match(wizard, /case 'recovering_formal_route': return '正在恢复已经配置的正式公网通道。'/)
  assert.match(wizard, /case 'issuing_pairing': return '安全公网通道已验证，正在签发一次性配对信息。'/)
  assert.match(wizard, /progress-ring--indeterminate/)
  assert.match(wizard, /background: #fff/)
  assert.match(wizard, /min-height: 44px/)
})

test('formal AI prompts are redacted, route-scoped, and contain no developer hostname', async () => {
  const prompt = await readFile(new URL('./useMobileConnectionSetup.ts', import.meta.url), 'utf8')
  const aliyunSetup = prompt.slice(prompt.indexOf('export function buildAliyunIpRouteSetupPrompt()'), prompt.indexOf('export function buildFormalRouteSetupPrompt()'))

  assert.match(aliyunSetup, /独立于 Windows/)
  assert.match(aliyunSetup, /--transport cloud_hub/)
  assert.match(aliyunSetup, /service=iterate-hub/)
  assert.match(aliyunSetup, /保留 5311/)
  assert.doesNotMatch(aliyunSetup, /--transport aliyun_ssh_reverse_tunnel/)
  assert.match(prompt, /buildFormalRouteSetupPrompt/)
  assert.match(prompt, /buildFormalRouteRepairPrompt/)
  assert.match(prompt, /\$\{safeBaseUrl\}/)
  assert.doesNotMatch(prompt, /iterate\.tobooks\.xin/)
  assert.match(prompt, /禁止[\s\S]*读取、打印、复制或上传/)
  assert.match(prompt, /禁止[\s\S]*Quick Tunnel/)
  assert.match(prompt, /禁止[\s\S]*Relay/)
})

test('bootstrap branches only on persisted formal route status', async () => {
  const setup = await readFile(new URL('./useMobileConnectionSetup.ts', import.meta.url), 'utf8')
  const bootstrapStart = setup.indexOf('async function bootstrap()')
  const pairingRead = setup.indexOf('const pairingStatus = await readPairingStatus()', bootstrapStart)
  const formalDecision = setup.indexOf('resolveMobileConnectionBootstrap(pairingStatus)', pairingRead)

  assert.ok(bootstrapStart >= 0)
  assert.ok(pairingRead > bootstrapStart)
  assert.ok(formalDecision > pairingRead)
  assert.doesNotMatch(setup.slice(bootstrapStart), /readQuickTunnelStatus/)
  assert.match(setup, /\/api\/android\/pairing\/status/)
  assert.match(setup, /\/api\/android\/pairing\/sessions\//)
  assert.match(setup, /\/api\/android\/pairing`/)
  assert.doesNotMatch(setup, /\/api\/mobile\/pairing/)
})

test('verified Aliyun IP route permits a matching secure Android pairing candidate', () => {
  assert.equal(resolveMobileConnectionBootstrap({
    formal_route: {
      ...healthyFormalRoute,
      transport: 'aliyun_ssh_reverse_tunnel',
      base_url: 'https://8.129.82.226:8443',
    },
    candidates: [{
      ...healthyPublic,
      base_url: 'https://8.129.82.226:8443',
      ws_url: 'wss://8.129.82.226:8443/ws',
    }],
  }), 'issue_pairing')
})

test('wizard reopens only an unclaimed fresh QR without preparing the bridge again', async () => {
  const setup = await readFile(new URL('./useMobileConnectionSetup.ts', import.meta.url), 'utf8')
  const resumeStart = setup.indexOf('function pairingViewCanResumeWithoutBootstrap()')
  const resetStart = setup.indexOf('function resetForReopen()', resumeStart)
  const bootstrapStart = setup.indexOf('async function bootstrap()')
  const issuePairingStart = setup.indexOf('async function issueFreshPairingOperation', bootstrapStart)
  const cancelStart = setup.indexOf('function cancel()')
  const disposeStart = setup.indexOf('function dispose()', cancelStart)

  assert.ok(resumeStart >= 0)
  assert.ok(resetStart > resumeStart)
  assert.ok(bootstrapStart >= 0)
  assert.ok(issuePairingStart > bootstrapStart)
  assert.ok(cancelStart >= 0)
  assert.ok(disposeStart > cancelStart)

  const resumeSource = setup.slice(resumeStart, resetStart)
  assert.match(resumeSource, /state\.value\.stage === 'waiting_for_claim'[\s\S]*pairingPayloadIsFresh\(pairingPayload\.value, now\.value, 0\)/)
  assert.doesNotMatch(resumeSource, /stage === 'complete'/)
  assert.doesNotMatch(resumeSource, /stage === 'waiting_for_connection'/)

  const bootstrapSource = setup.slice(bootstrapStart, issuePairingStart)
  assert.match(bootstrapSource, /if \(pairingViewCanResumeWithoutBootstrap\(\)\)[\s\S]*startClock\(\)[\s\S]*schedulePairingPoll\(\)[\s\S]*return/)

  const cancelSource = setup.slice(cancelStart, disposeStart)
  const preserveGuard = cancelSource.indexOf('if (preservePairingView)')
  const payloadClear = cancelSource.indexOf('pairingPayload.value = null')
  assert.ok(preserveGuard >= 0)
  assert.ok(payloadClear > preserveGuard)
})

test('popup phone entry opens the formal mobile connection wizard', async () => {
  const source = await readFile(new URL('../AppContent.vue', import.meta.url), 'utf8')
  assert.match(source, /import MobileConnectionWizard from ['"]\.\/settings\/MobileConnectionWizard\.vue['"]/)
  assert.match(source, /@open-iterate-pairing="showIteratePairingModal = true"/)
  assert.match(source, /<MobileConnectionWizard v-model:show="showIteratePairingModal"/)
  assert.doesNotMatch(source, /import IteratePairingModal/)
})
