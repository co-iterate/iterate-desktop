/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

// 「clauding · Opus 5.5」：显示是哪个 AI、哪个模型在调这次弹窗。桌面放底部状态栏，顶栏不放。
const badge = await readFile(new URL('./callerBadge.ts', import.meta.url), 'utf8')
const actions = await readFile(new URL('./PopupActions.vue', import.meta.url), 'utf8')
const header = await readFile(new URL('./PopupHeader.vue', import.meta.url), 'utf8')

describe('popup caller badge', () => {
  it('maps every known caller client to its -ing verb and logo', () => {
    assert.match(badge, /claude_code: \{ verb: 'clauding', name: 'Claude Code', icon: '\/icons\/ai-providers\/claude\.svg', tint: '#[0-9a-f]{6}' \}/)
    assert.match(badge, /codex: \{ verb: 'codexing', name: 'Codex', icon: '\/icons\/ai-providers\/codex-dark\.svg', tint: '#[0-9a-f]{6}' \}/)
    assert.match(badge, /antigravity: \{ verb: 'agying', name: 'Antigravity CLI', icon: '\/icons\/ai-providers\/antigravity-color\.svg', tint: '#[0-9a-f]{6}' \}/)
  })

  it('shows the model label only when one was read, never a guess', () => {
    assert.match(badge, /label \? `\$\{badge\.verb\} · \$\{label\}` : badge\.verb/)
    assert.match(badge, /if \(!badge\)\s*return null/)
  })

  it('draws agy as the CLI pixel A instead of an image', () => {
    assert.match(badge, /AGY_PIXEL_LOGO: string\[\]\[\] = \[/)
    assert.match(badge, /\['#7bb7ef', '#77b4f0', '', '', '#4d81f1', '#5083f4'\]/)
    assert.match(badge, /pixels: client === 'antigravity' \? AGY_PIXEL_LOGO : undefined/)
    assert.match(actions, /v-if="callerBadge\.pixels"/)
    assert.match(actions, /grid-template-columns: repeat\(6, 3px\)/)
  })

  it('lives in the desktop status bar, not the crowded header', () => {
    assert.match(actions, /callerBadgeFor\(\s*props\.request\?\.caller_client,\s*props\.request\?\.caller_model_label,\s*props\.request\?\.caller_model,/)
    assert.match(actions, /:style="\{ background: callerBadge\.tint,/)
    assert.match(actions, /<img :src="callerBadge\.icon"/)
    assert.doesNotMatch(header, /callerBadge/)
  })
})
