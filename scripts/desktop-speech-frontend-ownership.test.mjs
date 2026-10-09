import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'
import { stripTypeScriptTypes } from 'node:module'
import { runInNewContext } from 'node:vm'

const root = new URL('../', import.meta.url)

test('starting and restarting a speech host never requests system permissions', async () => {
  const source = await readFile(new URL('src/frontend/composables/useGlobalSpeechRuntimeHost.ts', root), 'utf8')
  const script = stripTypeScriptTypes(source.replace(/\r\n/g, '\n'))
    .replace(/^import .*\n/gm, '')
    .replace('export function useGlobalSpeechRuntimeHost', 'function useGlobalSpeechRuntimeHost')
  const commands = []
  const host = runInNewContext(`${script}\nuseGlobalSpeechRuntimeHost()`, {
    invoke: async (command) => {
      commands.push(command)
      if (command.endsWith('_status')) return false
      if (command === 'get_speech_control_snapshot') return { phase: 'Idle' }
      return []
    },
    listen: async () => () => {},
    GlobalSpeechSessionGuard: class { applySnapshot() { return false } },
    buildSpeechContextualStrings: () => [],
  })
  await host.initialize()
  host.dispose()
  await host.initialize()
  await new Promise(resolve => setImmediate(resolve))
  assert.equal(commands.filter(command => command === 'get_speech_control_snapshot').length, 2)
  assert.deepEqual(commands.filter(command => command.startsWith('request_')), [])
  host.dispose()
})

test('overlay composable is a Rust snapshot projection without a second controller', async () => {
  const source = await readFile(new URL('src/frontend/composables/useGlobalSpeechInput.ts', root), 'utf8')

  assert.match(source, /speech:\/\/session-snapshot/)
  assert.match(source, /get_speech_control_snapshot/)
  assert.match(source, /ack_speech_overlay_visibility/)
  assert.doesNotMatch(source, /speech:\/\/(?:toggle|native-)/)
  assert.doesNotMatch(source, /\b(?:1200|1800|2600)\b/)
  assert.doesNotMatch(source, /(?:start_native_speech|stop_native_speech|commit_speech_text)/)
  assert.doesNotMatch(source, /phase\.value\s*=(?!=)/)
})

test('canonical runtime host owns configuration and tagged transcript completion', async () => {
  const source = await readFile(new URL('src/frontend/composables/useGlobalSpeechRuntimeHost.ts', root), 'utf8')
  assert.match(source, /configure_speech_recognition/)
  assert.match(source, /speech:\/\/process-transcript/)
  assert.match(source, /complete_speech_processing/)
  assert.match(source, /applySpeechPostprocess/)
  assert.doesNotMatch(source, /(?:start_native_speech|stop_native_speech|commit_speech_text)/)
})

test('popup accepts remote speech identity only after backend IPC authorization', async () => {
  const source = await readFile(new URL('src/frontend/components/popup/PopupInput.vue', root), 'utf8')

  assert.match(source, /authorize_popup_speech_insert/)
  assert.match(source, /authenticated-ipc/)
  assert.match(source, /speechInsertGuard\.classify\(payload, authority\)/)
})
