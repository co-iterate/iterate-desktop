/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

// P-2026-9373：⌘+点击项目路径必须按调用方分支，Claude Code 调用方不得落到 Codex。
const source = await readFile(new URL('./PopupHeader.vue', import.meta.url), 'utf8')
const appContent = await readFile(new URL('../AppContent.vue', import.meta.url), 'utf8')
const mcpHandler = await readFile(new URL('../../composables/useMcpHandler.ts', import.meta.url), 'utf8')

describe('PopupHeader Claude caller open', () => {
  it('declares independent caller props instead of reusing codexThreadId', () => {
    assert.match(source, /callerClient\?: string/)
    assert.match(source, /callerSessionId\?: string/)
    assert.match(source, /callerCwd\?: string/)
    assert.match(source, /callerPid\?: number/)
    assert.match(appContent, /:caller-client="props\.mcpRequest\?\.caller_client"/)
    assert.match(appContent, /:caller-session-id="props\.mcpRequest\?\.caller_session_id"/)
    assert.match(appContent, /:caller-cwd="props\.mcpRequest\?\.caller_cwd"/)
    assert.match(appContent, /:caller-pid="props\.mcpRequest\?\.caller_pid"/)
  })

  it('routes Claude callers to open_claude_session before any Codex branch', () => {
    assert.match(source, /const isClaudeCaller = computed\(\(\) => props\.callerClient === 'claude_code' && Boolean\(props\.callerSessionId\?\.trim\(\)\)\)/)
    const branch = source.match(/async function openCallerTargetFromEvent\(event: MouseEvent \| PointerEvent\) \{([\s\S]*?)\n\}/)
    assert.ok(branch, 'openCallerTargetFromEvent must exist')
    const body = branch[1]
    const claudeIndex = body.indexOf('openClaudeSessionFromEvent(event)')
    const codexIndex = body.indexOf('openCodexTargetFromEvent(event)')
    assert.ok(claudeIndex >= 0 && codexIndex > claudeIndex, 'Claude branch must run before Codex fallback')
    assert.match(body, /if \(isClaudeCaller\.value\) \{\s*await openClaudeSessionFromEvent\(event\)\s*return\s*\}/)
    assert.match(source, /invoke<string>\('open_claude_session', \{\s*sessionId: props\.callerSessionId,/)
  })

  it('uses the caller branch from both pointerdown and click handlers', () => {
    const pointerDown = source.match(/async function handleProjectPathPointerDown[\s\S]*?\n\}/)?.[0] ?? ''
    const click = source.match(/async function handleProjectPathClick[\s\S]*?\n\}/)?.[0] ?? ''
    assert.match(pointerDown, /await openCallerTargetFromEvent\(event\)/)
    assert.match(click, /await openCallerTargetFromEvent\(event\)/)
    assert.doesNotMatch(pointerDown, /await openCodexTargetFromEvent\(event\)/)
    assert.doesNotMatch(click, /await openCodexTargetFromEvent\(event\)/)
  })

  it('shows the Claude-specific hint', () => {
    assert.match(source, /⌘\+点击回到调用本次 MCP 的 Claude 会话/)
    assert.match(source, /⌘\+点击回到调用本次 MCP 的 Codex 会话/)
    assert.match(source, /⌘\+点击在 Codex 中打开项目/)
  })

  it('never lets a Claude caller borrow the Codex live goal route', () => {
    assert.match(mcpHandler, /if \(request\?\.caller_client === 'claude_code'\)\s*return resolveRequestId\(request\)/)
    const fallback = mcpHandler.match(/async function resolveConversationRouteIdWithFallback[\s\S]*?\n\}/)?.[0] ?? ''
    assert.ok(
      fallback.indexOf('caller_client === \'claude_code\'') < fallback.indexOf('resolveLiveGoalConversationRouteId('),
      'Claude guard must run before live goal fallback',
    )
  })
})
