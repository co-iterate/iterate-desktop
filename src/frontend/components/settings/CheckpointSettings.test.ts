/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

const source = (await readFile(new URL('./CheckpointSettings.vue', import.meta.url), 'utf8'))
  .replace(/\r\n/g, '\n')

describe('CheckpointSettings auto checkpoint switch', () => {
  it('keeps the switch busy until initial config has loaded', () => {
    assert.match(source, /const loading = ref\(true\)/)
    assert.match(
      source,
      /async function loadConfig\(\) \{\n\s+loading\.value = true[\s\S]*finally \{\n\s+loading\.value = false/,
    )
    assert.match(source, /if \(loading\.value\)\n\s+return/)
  })

  it('describes disabling future automatic checkpoint work precisely', () => {
    assert.match(source, /关闭后将停止新的自动提交和后台监控触发/)
    assert.doesNotMatch(source, /关闭后将完全停止自动提交和后台监控/)
  })
})
