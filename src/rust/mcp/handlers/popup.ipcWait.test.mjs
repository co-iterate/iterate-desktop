import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('running-app popup IPC waits for the user without spawning a timed fallback', async () => {
  const source = await readFile(new URL('./popup.rs', import.meta.url), 'utf8')

  assert.match(source, /stream\.set_read_timeout\(None\)\?/)
  assert.doesNotMatch(source, /set_read_timeout\(Some\(/)
  assert.match(source, /IPC_CONNECT_TIMEOUT_MS/)
})
