import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('browser popup replies do not append MCP conversation rules', async () => {
  const popup = await readFile(new URL('./McpPopup.vue', import.meta.url), 'utf8')
  const types = await readFile(new URL('../../types/popup.d.ts', import.meta.url), 'utf8')

  assert.match(types, /browser_tab_id\?: number/)
  assert.match(
    popup,
    /props\.request\?\.browser_ai_response == null && props\.request\?\.browser_tab_id == null/,
  )
  assert.match(popup, /:enable-context-append="shouldAppendConversationContext"/)
})
