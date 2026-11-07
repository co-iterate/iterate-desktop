/* eslint-disable test/no-import-node-test -- this repository executes source-contract tests with `node --test` */
import assert from 'node:assert/strict'
import test from 'node:test'
import { mobileRouteLabel, mobileRouteScanHint, mobileRouteSwitchErrorText } from './mobileRouteOptions.ts'

test('every formal transport has a readable name', () => {
  assert.equal(mobileRouteLabel('tailscale_serve'), 'Tailscale')
  assert.equal(mobileRouteLabel('cloudflare_named_tunnel'), 'Cloudflare')
  assert.equal(mobileRouteLabel('aliyun_ssh_reverse_tunnel'), '阿里云 IP + SSH')
  assert.equal(mobileRouteLabel(null), '未配置')
})

test('only tailscale asks the user to prepare the phone before scanning', () => {
  assert.match(mobileRouteScanHint('tailscale_serve'), /Tailscale/)
  assert.equal(mobileRouteScanHint('cloudflare_named_tunnel'), '')
})

test('switch errors fall back to the tailscale texts for unknown codes', () => {
  assert.match(mobileRouteSwitchErrorText('endpoint_identity_mismatch', () => 'x'), /保留当前连接方式/)
  assert.equal(mobileRouteSwitchErrorText('tailscale_funnel_enabled', code => `fallback:${code}`), 'fallback:tailscale_funnel_enabled')
})
