/* eslint-disable test/no-import-node-test -- behavioral tests run with node --test */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'
import test from 'node:test'
import * as vue from 'vue'
import * as machine from './mobileConnectionMachine.ts'
import * as prompts from './mobileConnectionPrompts.ts'

// Load the actual composable, replacing only the native and HTTP boundaries.
const source = stripTypeScriptTypes(readFileSync(new URL('./useMobileConnectionSetup.ts', import.meta.url), 'utf8'), { mode: 'transform' })
  .replace(/^import\s+\{([^}]+)\}\s+from\s+(['"])([^'"]+)\2;?/gm, (_, names, _quote, path) => `const {${names}} = dependencies[${JSON.stringify(path)}];`)
  .replace('export function useMobileConnectionSetup', 'function useMobileConnectionSetup')
// Only trusted repository source is evaluated; network/native calls are replaced below.
// eslint-disable-next-line no-new-func
const load = new Function('dependencies', `${source}\nreturn useMobileConnectionSetup;`)
const route = { configured: true, transport: 'self_hosted_frp', base_url: 'https://device.example.test', health: 'healthy', endpoint_identity_ok: true }
const candidate = { transport_mode: 'public_tunnel', base_url: route.base_url, ws_url: 'wss://device.example.test/ws', health: 'healthy', disabled: false }
const empty = { formal_route: { configured: false, health: 'unknown' }, candidates: [] }
const healthy = { formal_route: route, candidates: [candidate] }

function deferred() {
  let resolve!: (value: any) => void
  let reject!: (cause: Error) => void
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

function fixture(t: any, status: () => any = () => empty, native: (name: string) => any = () => route) {
  const calls: string[] = []
  const useSetup = load({
    '@tauri-apps/api/core': { invoke: async (name: string) => {
      calls.push(name)
      return name === 'recover_bridge_origin' ? { healthy: true } : native(name)
    } },
    'vue': { ...vue, onUnmounted: () => {} },
    '../../services/bridgeFetch.ts': { bridgeFetch: async (url: string) => {
      calls.push(url)
      const data = url.endsWith('/status')
        ? status()
        : {
            ok: true,
            pairing: { version: 2, pairing_session_id: 'fixture', device_id: 'fixture-mac', device_name: 'Fixture', ...candidate, pairing_token: 'synthetic-no-authority', issued_at: new Date().toISOString(), expires_at: new Date(Date.now() + 120000).toISOString(), candidates: [candidate] },
          }
      return { ok: true, json: async () => data }
    } },
    './mobileConnectionMachine.ts': machine,
    './mobileConnectionPrompts.ts': prompts,
  })
  const scope = vue.effectScope()
  const setup = scope.run(() => useSetup({ pollIntervalMs: 60000, recoveryAttempts: 1, recoveryDelayMs: 0 }))
  t.after(() => {
    setup.cancel()
    scope.stop()
  })
  return { setup, calls }
}

const tick = () => new Promise(resolve => setImmediate(resolve))

test('empty install provisions before issuing a QR code', async (t) => {
  const { setup, calls } = fixture(t)
  await setup.bootstrap()
  assert.equal(setup.state.value.stage, 'waiting_for_claim')
  assert.ok(setup.qrSource.value)
  assert.ok(calls.indexOf('ensure_managed_mobile_route') < calls.findIndex(x => x.endsWith('/pairing')))
})

test('existing healthy route is reused without enrollment', async (t) => {
  const { setup, calls } = fixture(t, () => healthy)
  await setup.bootstrap()
  assert.equal(setup.state.value.stage, 'waiting_for_claim')
  assert.ok(!calls.includes('ensure_managed_mobile_route'))
})

test('unverified native result cannot issue a QR code', async (t) => {
  const { setup } = fixture(t, () => empty, () => ({ ...route, endpoint_identity_ok: false }))
  await setup.bootstrap()
  assert.equal(setup.state.value.stage, 'error')
  assert.equal(setup.qrSource.value, '')
})

test('unsupported platforms retain the existing setup handoff', async (t) => {
  const { setup } = fixture(t, () => empty, () => {
    throw new Error('managed_platform_unsupported')
  })
  await setup.bootstrap()
  assert.equal(setup.state.value.stage, 'setup_required')
})

test('builds without a managed service fall back to self-hosted setup', async (t) => {
  const { setup } = fixture(t, () => empty, () => {
    throw new Error('managed_not_configured')
  })
  await setup.bootstrap()
  assert.equal(setup.state.value.stage, 'setup_required')
  assert.equal(setup.error.value, '')
})

test('late enrollment cannot replace the reopened route or QR', async (t) => {
  const pending = deferred()
  let status = empty as any
  const { setup } = fixture(t, () => status, () => pending.promise)
  const old = setup.bootstrap()
  await tick()
  setup.cancel()
  status = healthy
  await setup.bootstrap()
  const qr = setup.qrSource.value
  pending.resolve({ ...route, base_url: 'https://obsolete.example.test' })
  await old
  assert.equal(setup.formalRoute.value.base_url, route.base_url)
  assert.equal(setup.qrSource.value, qr)
})

test('late recovery error cannot change a reopened healthy view', async (t) => {
  const pending = deferred()
  let status = { formal_route: { ...route, health: 'degraded', endpoint_identity_ok: false }, candidates: [] } as any
  const { setup } = fixture(t, () => status, () => pending.promise)
  const old = setup.bootstrap()
  await tick()
  setup.cancel()
  status = healthy
  await setup.bootstrap()
  pending.reject(new Error('managed_service_unavailable'))
  await old
  assert.equal(setup.state.value.stage, 'waiting_for_claim')
  assert.equal(setup.error.value, '')
})

test('temporary enrollment failure can be retried', async (t) => {
  let attempts = 0
  const { setup } = fixture(t, () => empty, () => {
    if (++attempts === 1)
      throw new Error('managed_service_busy')
    return route
  })
  await setup.bootstrap()
  assert.equal(setup.qrSource.value, '')
  await setup.retry()
  assert.equal(setup.state.value.stage, 'waiting_for_claim')
  assert.equal(attempts, 2)
})

test('existing manual frp route can recover without managed enrollment', async (t) => {
  let reads = 0
  const { setup } = fixture(t, () => ++reads === 1 ? { formal_route: { ...route, health: 'degraded', endpoint_identity_ok: false }, candidates: [] } : healthy, () => {
    throw new Error('managed_route_not_owned')
  })
  await setup.bootstrap()
  assert.equal(setup.state.value.stage, 'waiting_for_claim')
  assert.equal(setup.formalRoute.value.base_url, route.base_url)
})
