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
import { buildAliyunIpRouteSetupPrompt, buildFormalRouteRepairPrompt, buildFormalRouteSetupPrompt } from './mobileConnectionPrompts.ts'

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

test('compact QR payload keeps only fields required by iPhone import', () => {
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
  const prompt = await readFile(new URL('./mobileConnectionPrompts.ts', import.meta.url), 'utf8')
  const wizard = await readFile(new URL('./MobileConnectionWizard.vue', import.meta.url), 'utf8')

  assert.doesNotMatch(setup, /cloudflareApiToken|zoneName|subdomain|create_cloudflare_web_login_auto_setup/)
  assert.doesNotMatch(setup, /start_quick_tunnel|stop_quick_tunnel|get_quick_tunnel_status/)
  assert.match(setup, /from '\.\/mobileConnectionPrompts\.ts'/)
  assert.match(setup, /buildFormalRouteSetupPrompt/)
  assert.match(setup, /buildFormalRouteRepairPrompt/)
  assert.match(prompt, /登录 Cloudflare[\s\S]*等待我明确确认/)
  assert.match(prompt, /禁止[\s\S]*token/)
  assert.match(prompt, /禁止使用 Quick Tunnel、随机 trycloudflare\.com、Relay、LAN/)
  assert.match(wizard, /复制 AI 配置提示词/)
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
  const prompt = await readFile(new URL('./mobileConnectionPrompts.ts', import.meta.url), 'utf8')
  const setupPrompt = buildFormalRouteSetupPrompt()
  const repairPrompt = buildFormalRouteRepairPrompt('https://user-owned.example.com', 'probe timeout / raw detail')
  const aliyunSetup = buildAliyunIpRouteSetupPrompt()

  assert.match(aliyunSetup, /公网 Host（含端口）/)
  assert.match(aliyunSetup, /X-Forwarded-For/)
  assert.match(aliyunSetup, /X-Forwarded-Proto/)
  assert.match(aliyunSetup, /\/api\/config、\/api\/active-sessions 及 \/ws WebSocket Upgrade 都必须返回 401/)
  assert.match(aliyunSetup, /不得关闭 Bridge 鉴权/)
  assert.match(prompt, /buildFormalRouteSetupPrompt/)
  assert.match(prompt, /buildFormalRouteRepairPrompt/)
  assert.match(prompt, /\$\{safeBaseUrl\}/)
  assert.doesNotMatch(prompt, /iterate\.tobooks\.xin/)
  assert.match(setupPrompt, /--mobile-route-status[\s\S]*--mobile-route-verify/)
  assert.match(setupPrompt, /configured=true 且 healthy/)
  assert.match(setupPrompt, /configured=true 但不健康/)
  assert.match(setupPrompt, /configured=false/)
  assert.match(setupPrompt, /正式路线只支持 cloudflare_named_tunnel/)
  assert.match(setupPrompt, /当前版本无法完成长期稳定的正式二维码公网路线/)
  assert.match(setupPrompt, /禁止读取、打印、复制或上传凭据内容/)
  assert.match(setupPrompt, /禁止使用 Quick Tunnel、随机 trycloudflare\.com、Relay、LAN/)
  assert.doesNotMatch(setupPrompt, /iterate\.tobooks\.xin/)
  assert.match(repairPrompt, /已配置地址：https:\/\/user-owned\.example\.com/)
  assert.match(repairPrompt, /脱敏错误码：probe_timeout___raw_detail/)
  assert.doesNotMatch(repairPrompt, /probe timeout \/ raw detail/)
})

test('the handbook keeps the approved tab name, card order, and shared QR prompt', async () => {
  const layout = await readFile(new URL('../layout/MainLayout.vue', import.meta.url), 'utf8')
  const source = await readFile(new URL('../tabs/PromptsTab.vue', import.meta.url), 'utf8')

  assert.match(layout, /<n-tab-pane name="prompts" tab="使用说明书">/)
  assert.doesNotMatch(layout, /<n-tab-pane name="prompts" tab="参考提示词">/)
  assert.match(source, /import \{ buildFormalRouteSetupPrompt \} from '\.\.\/settings\/mobileConnectionPrompts'/)
  assert.match(source, /const mobilePromptContent = buildFormalRouteSetupPrompt\(\)/)
  const handbookIndex = source.indexOf('<!-- 使用说明书卡片 -->')
  const installIndex = source.indexOf('<!-- iterate 安装卡片 -->')
  const iphoneIndex = source.indexOf('<!-- iPhone 二维码配置提示词卡片 -->')
  const systemPromptIndex = source.indexOf('<!-- 核心系统提示词卡片 -->')
  assert.ok(handbookIndex >= 0)
  assert.ok(installIndex > handbookIndex)
  assert.ok(iphoneIndex > installIndex)
  assert.ok(systemPromptIndex > iphoneIndex)
  assert.equal((source.match(/>\s*使用说明书\s*</g) || []).length, 1)
  assert.equal((source.match(/>\s*iterate 安装\s*</g) || []).length, 1)
  assert.equal((source.match(/>\s*启用 iPhone 二维码\s*</g) || []).length, 1)
  assert.equal((source.match(/>\s*核心系统提示词\s*</g) || []).length, 1)
  assert.match(source, /启用 iPhone 二维码/)
  assert.match(source, /iPhone 远程连接/)
  assert.match(source, /复制配置提示词/)
  assert.match(source, /查看完整提示词/)
  assert.match(source, /n-collapse-transition :show="mobilePromptExpanded"/)
  assert.match(source, /配置提示词已复制/)
  assert.match(source, /其中 \{\{ enabledPromptToolCount \}\} 个模块已纳入下方核心系统提示词/)
  assert.doesNotMatch(source, />\s*安装提示词\s*</)
  assert.doesNotMatch(source, />\s*使用说明\s*</)
  assert.doesNotMatch(source, /--mobile-route-register/)
  assert.doesNotMatch(source, /重新检测/)
})

test('AI client setup lives on the AI connection path, not in the iPhone wizard', async () => {
  const wizard = await readFile(new URL('./MobileConnectionWizard.vue', import.meta.url), 'utf8')
  const clients = await readFile(new URL('./McpClientSettings.vue', import.meta.url), 'utf8')
  const settingsTab = await readFile(new URL('../tabs/SettingsTab.vue', import.meta.url), 'utf8')
  const onboarding = await readFile(new URL('../common/OnboardingWelcome.vue', import.meta.url), 'utf8')
  const firstRunGuide = await readFile(new URL('../onboarding/FirstRunGuide.vue', import.meta.url), 'utf8')
  const handbook = await readFile(new URL('../tabs/PromptsTab.vue', import.meta.url), 'utf8')
  const corePrompt = await readFile(new URL('../../composables/useCoreSystemPrompt.ts', import.meta.url), 'utf8')

  // 用户 2026-11-07：「AI 连接和连接手机是分开的」——手机向导回到 3 步，不再出现 AI 客户端配置
  assert.match(wizard, /const GUIDE_STEPS = \['装 App', '扫码配对', '测试送达'\]/)
  assert.match(wizard, /grid-template-columns: repeat\(3, 1fr\)/)
  assert.doesNotMatch(wizard, /配 AI 客户端|McpClientSettings|aiClientSetup|showClientStep|backToClientStep/)
  assert.match(wizard, /iterate 只会通过这台 Mac 已配置并验证的正式公网路线生成二维码。/)

  // AI 连接这条线：首启卡片、设置页用同一个组件，都能看到 CLAUDE.md / AGENTS.md 规则状态
  assert.match(onboarding, /<McpClientSettings compact \/>/)
  assert.match(settingsTab, /<McpClientSettings show-manual \/>/)
  assert.match(clients, /rules_path/)
  assert.match(clients, /rulesStateText\(client\.rules_state\)/)
  assert.match(clients, /<div v-if="rulesClients\.length > 0"/)
  assert.match(clients, /'connect_mcp_clients'/)
  assert.match(clients, /'get_mcp_client_overview'/)
  // 首次使用引导第 1 步用自己的 driver，至少说明会写调用规则
  assert.match(firstRunGuide, /~\/\.claude\/CLAUDE\.md、~\/\.codex\/AGENTS\.md/)

  // 列表外的客户端：两个复制按钮，不再单独提 VS Code
  assert.match(clients, /复制核心系统提示词/)
  assert.match(clients, />\s*复制 MCP 配置\s*</)
  assert.match(clients, /RULES_FILE_HINT/)
  assert.doesNotMatch(clients, /VS Code|buildVsCodeMcpConfig|copilot-instructions/)

  // 核心系统提示词只有一份来源
  assert.match(corePrompt, /generateFullPrompt\(promptTools\.value\)/)
  assert.match(clients, /useCoreSystemPrompt\(\)/)
  assert.match(handbook, /useCoreSystemPrompt\(\)/)
  assert.doesNotMatch(clients, /CORE_RULES|generateFullPrompt/)
  assert.equal((handbook.match(/>\s*核心系统提示词\s*</g) || []).length, 1)
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
})

test('verified Aliyun IP route permits a matching secure pairing candidate', () => {
  assert.equal(resolveMobileConnectionBootstrap({
    formal_route: {
      ...healthyFormalRoute,
      transport: 'aliyun_ssh_reverse_tunnel',
      base_url: 'https://203.0.113.50:8443',
    },
    candidates: [{
      ...healthyPublic,
      base_url: 'https://203.0.113.50:8443',
      ws_url: 'wss://203.0.113.50:8443/ws',
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
