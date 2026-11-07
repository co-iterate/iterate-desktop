/* eslint-disable test/no-import-node-test -- this repository executes source-contract tests with `node --test` */
import type { TailscaleServeStatus } from './tailscaleServeRoute.ts'
import assert from 'node:assert/strict'
import test from 'node:test'
import { tailscaleGuideState, tailscaleServeBlockerText, tailscaleServeChecklist } from './tailscaleServeRoute.ts'

const ready: TailscaleServeStatus = {
  cli_available: true,
  running: true,
  dns_name: 'mac.tail123.ts.net',
  https_cert_available: true,
  serve_proxies_bridge: true,
  funnel_enabled: false,
  base_url: 'https://mac.tail123.ts.net',
  setup_command: null,
  blocker: null,
}

test('a ready tailnet passes every check', () => {
  assert.ok(tailscaleServeChecklist(ready).every(check => check.ok))
  assert.equal(tailscaleServeBlockerText(ready.blocker), '')
})

test('funnel fails the privacy check even when serve points at the bridge', () => {
  const checks = tailscaleServeChecklist({ ...ready, funnel_enabled: true, blocker: 'tailscale_funnel_enabled' })
  assert.equal(checks.find(check => check.label.includes('Funnel'))?.ok, false)
  assert.match(tailscaleServeBlockerText('tailscale_funnel_enabled'), /funnel reset/)
})

test('a stopped tailscale does not claim funnel is off', () => {
  const stopped = { ...ready, running: false, dns_name: null, https_cert_available: false, serve_proxies_bridge: false }
  assert.ok(tailscaleServeChecklist(stopped).every(check => !check.ok))
})

test('unknown blocker codes still produce readable text', () => {
  assert.match(tailscaleServeBlockerText('something_new'), /something_new/)
})

test('the guide follows real detection, step by step', () => {
  const none = { ...ready, cli_available: false, running: false, dns_name: null, https_cert_available: false, serve_proxies_bridge: false }
  assert.deepEqual(tailscaleGuideState(none), { step: 0, view: 'install' })
  assert.deepEqual(tailscaleGuideState({ ...none, cli_available: true, backend_state: 'NeedsLogin' }), { step: 0, view: 'login' })
  assert.deepEqual(tailscaleGuideState({ ...ready, https_cert_available: false }), { step: 1, view: 'switches' })
  assert.deepEqual(tailscaleGuideState({ ...ready, funnel_enabled: true }), { step: 2, view: 'funnel' })
  assert.deepEqual(tailscaleGuideState({ ...ready, serve_proxies_bridge: false }), { step: 2, view: 'serve' })
  assert.deepEqual(tailscaleGuideState({ ...ready, ios_peer_online: false }), { step: 3, view: 'phone' })
  assert.deepEqual(tailscaleGuideState({ ...ready, ios_peer_online: true }), { step: 4, view: 'ready' })
})

test('funnel is handled before serve, so iterate is never left public', () => {
  assert.equal(tailscaleGuideState({ ...ready, funnel_enabled: true, serve_proxies_bridge: false }).view, 'funnel')
})
