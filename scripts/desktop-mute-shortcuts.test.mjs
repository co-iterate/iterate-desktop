import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'
import { test } from 'node:test'

const source = readFileSync(new URL('../src/frontend/components/AppContent.vue', import.meta.url), 'utf8')
const start = source.indexOf('const isShortcutsRegistered =')
const end = source.indexOf('watch(() => props.isMuted', start)
assert.ok(start >= 0 && end > start)
const registrationCode = stripTypeScriptTypes(source.slice(start, end))

function harness(enableHook = async () => {}) {
  const props = { showMcpPopup: true, isMuted: true }
  const calls = []
  const api = new Function('ref', 'props', 'invoke', 'console', `${registrationCode}
    return {
      sync: syncWindowShortcuts,
      registered: () => isShortcutsRegistered.value,
      dispose: () => { shortcutLifecycleDisposed = true; return syncWindowShortcuts() }
    }
  `)(value => ({ value }), props, async (command, args) => {
    if (command === 'get_global_shortcut_enabled') return true
    assert.equal(command, 'set_popup_restore_enabled')
    calls.push(args.enabled)
    if (args.enabled) await enableHook()
    return args.enabled
  }, { error: error => assert.fail(String(error)) })
  return { props, calls, api }
}

test('the shared bell enables native restoration only for an active popup', async () => {
  const h = harness()
  await h.api.sync()
  assert.ok(h.calls.every(enabled => enabled === false))
  h.props.isMuted = false
  await h.api.sync()
  assert.equal(h.api.registered(), true)
  h.props.isMuted = true
  await h.api.sync()
  assert.equal(h.api.registered(), false)
  assert.deepEqual(h.calls, [false, true, false])
})

test('mute during an in-flight native enable finishes disabled', async () => {
  let release, started
  const enablingStarted = new Promise(resolve => { started = resolve })
  const h = harness(() => new Promise(resolve => { release = resolve; started() }))
  h.props.isMuted = false
  const enabling = h.api.sync()
  await enablingStarted
  h.props.isMuted = true
  const disabling = h.api.sync()
  release()
  await Promise.all([enabling, disabling])
  assert.equal(h.api.registered(), false)
  assert.deepEqual(h.calls, [true, false])
})

test('disposing a popup prevents later queued work from enabling restoration', async () => {
  const h = harness()
  h.props.isMuted = false
  await h.api.dispose()
  await h.api.sync()
  assert.ok(h.calls.every(enabled => enabled === false))
})

test('closing a popup disables its native restore target', async () => {
  const h = harness()
  h.props.isMuted = false
  await h.api.sync()
  h.props.showMcpPopup = false
  await h.api.sync()
  assert.deepEqual(h.calls, [true, false])
})
