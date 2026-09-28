/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

const source = await readFile(new URL('./PopupHeader.vue', import.meta.url), 'utf8')

describe('PopupHeader Codex project path open', () => {
  it('prioritizes the calling Codex thread when a thread id is present', () => {
    assert.match(source, /async function openCodexTarget\(\)/)
    assert.match(source, /if \(props\.codexThreadId\)/)
    assert.match(source, /invoke\('open_codex_thread', \{ threadId: props\.codexThreadId \}\)/)
    assert.match(source, /else\s+await invoke\('open_codex_project', \{ projectPath: props\.projectPath \}\)/)
    assert.match(source, /⌘\+点击回到调用本次 MCP 的 Codex 会话/)
  })

  it('captures modified project path opens before click synthesis', () => {
    assert.match(source, /let suppressNextProjectPathClick = false/)
    assert.match(source, /async function handleProjectPathPointerDown\(event: MouseEvent \| PointerEvent\)/)
    assert.match(source, /@pointerdown="handleProjectPathPointerDown"/)
    assert.match(source, /@mousedown="handleProjectPathPointerDown"/)
    assert.match(source, /@mouseup="handleProjectPathPointerDown"/)
    assert.match(source, /openCodexTargetFromEvent\(event\)/)
    assert.match(source, /props\.codexThreadId \? '正在打开 Codex 会话' : '正在打开 Codex 项目'/)
  })

  it('renders a compact conversation title row when the protocol provides one', () => {
    assert.match(source, /conversationTitle\?: string/)
    assert.match(source, /props\.conversationTitle\?\.trim\(\) \|\| null/)
    assert.match(source, /data-guide="conversation-title"/)
    assert.match(source, /v-if="displayConversationTitle"/)
    assert.match(source, />标题<\/span>/)
    assert.match(source, /color: props\.currentTheme === 'light' \? '#374151' : '#e5e7eb'/)
    assert.match(source, /color: props\.currentTheme === 'light' \? '#111827' : '#f9fafb'/)
    assert.match(source, /textDecoration: 'underline'/)
    assert.match(source, /textUnderlineOffset: '3px'/)
  })
})
