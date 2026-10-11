import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'
import test from 'node:test'

const source = readFileSync(new URL('../src/frontend/components/AppContent.vue', import.meta.url), 'utf8')

test('Tab delegates minimization and its exact restore identity to native code', async () => {
  const start = source.indexOf('async function minimizeWindow()')
  const end = source.indexOf('// 用于检测单独按下 Shift', start)
  assert.ok(start >= 0 && end > start)
  const calls = []
  const code = stripTypeScriptTypes(source.slice(start, end))
  const minimize = new Function('invoke', 'getCurrentWindow', 'message', 'console', 'syncWindowShortcuts', `${code}; return minimizeWindow`)(
    async (command, args) => calls.push({ command, args }),
    () => ({ minimize: async () => calls.push({ command: 'webview-minimize-only' }) }),
    { error: error => assert.fail(String(error)) },
    { error: error => assert.fail(String(error)) },
    async () => {},
  )
  await minimize()
  assert.equal(calls[0]?.command, 'minimize_popup_window')
})
