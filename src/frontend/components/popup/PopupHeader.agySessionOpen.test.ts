/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

// agy 调用方：⌘+点击项目路径回到 agy 会话（agy --conversation <id>），和 Claude 同一套交互。
const source = await readFile(new URL('./PopupHeader.vue', import.meta.url), 'utf8')

describe('PopupHeader agy session open', () => {
  it('treats antigravity callers with a conversation id as resumable', () => {
    assert.match(source, /const isAgyCaller = computed\(\(\) => props\.callerClient === 'antigravity' && Boolean\(props\.callerSessionId\?\.trim\(\)\)\)/)
  })

  it('routes cmd+click to open_agy_session with the conversation, cwd and pid', () => {
    assert.match(source, /invoke<string>\('open_agy_session', \{\s*conversationId: props\.callerSessionId,\s*cwd: props\.callerCwd \?\? null,/)
    assert.match(source, /if \(isAgyCaller\.value\) \{\s*await openAgySessionFromEvent\(event\)/)
  })
})
