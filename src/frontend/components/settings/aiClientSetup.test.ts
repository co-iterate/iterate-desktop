/* eslint-disable test/no-import-node-test -- this repository executes source-contract tests with `node --test` */
import assert from 'node:assert/strict'
import test from 'node:test'
import * as aiClientSetup from './aiClientSetup.ts'
import {
  buildManualMcpConfig,
  CLAUDE_ZHI_PERMISSION,
  clientNeedsPermission,
  clientNeedsReconnect,
  clientNeedsRules,
  manualMcpCommand,
  MCP_SERVER_NAME,
  permissionStateText,
  RULES_FILE_HINT,
  rulesStateText,
} from './aiClientSetup.ts'

test('AI client helpers no longer carry phone-wizard step state or a VS Code-only config', () => {
  // 用户 2026-11-07：AI 连接和连接手机是两条线；VS Code 不需要单独配置
  const exported = Object.keys(aiClientSetup)
  for (const removed of ['mobileGuideStepIndex', 'readAiClientStepDone', 'writeAiClientStepDone', 'clearAiClientStepDone', 'buildVsCodeMcpConfig', 'VSCODE_MCP_CONFIG_HINT'])
    assert.ok(!exported.includes(removed), `${removed} should be removed`)
})

test('a connected client without rules can be re-selected to write them', () => {
  assert.equal(clientNeedsRules({ points_to_current: true, opted_out: false, rules_state: 'missing' }), true)
  assert.equal(clientNeedsRules({ points_to_current: true, opted_out: false, rules_state: 'installed' }), false)
  assert.equal(clientNeedsRules({ points_to_current: true, opted_out: false, rules_state: 'manual' }), false)
  assert.equal(clientNeedsRules({ points_to_current: false, opted_out: false, rules_state: 'missing' }), false)
  assert.equal(clientNeedsRules({ points_to_current: true, opted_out: false, rules_state: null }), false)
  assert.equal(rulesStateText('installed'), '已写入调用规则')
  assert.equal(rulesStateText('manual'), '已有你手写的调用规则')
  assert.equal(rulesStateText('missing'), '还没写调用规则')
})

test('a connected Claude Code without the call_zhi permission can be re-selected to add it', () => {
  // 用户 2026-11-07：一键接入时把 call_zhi 加进 Claude Code 权限白名单，首次调用不被权限确认拦住
  assert.equal(CLAUDE_ZHI_PERMISSION, 'mcp__iterate-zhi__call_zhi')
  const base = { points_to_current: true, opted_out: false, rules_state: 'installed' }
  assert.equal(clientNeedsPermission({ ...base, permission_state: 'missing' }), true)
  assert.equal(clientNeedsPermission({ ...base, permission_state: 'installed' }), false)
  assert.equal(clientNeedsPermission({ ...base, permission_state: 'manual' }), false)
  // 坏的 settings.json 不会被自动改，勾选也没用
  assert.equal(clientNeedsPermission({ ...base, permission_state: 'invalid' }), false)
  assert.equal(clientNeedsPermission({ ...base, points_to_current: false, permission_state: 'missing' }), false)
  // Codex / Cursor 没有这一项
  assert.equal(clientNeedsPermission({ ...base, permission_state: null }), false)

  assert.equal(clientNeedsReconnect({ ...base, permission_state: 'missing' }), true)
  assert.equal(clientNeedsReconnect({ ...base, rules_state: 'missing', permission_state: 'installed' }), true)
  assert.equal(clientNeedsReconnect({ ...base, permission_state: 'installed' }), false)

  assert.equal(permissionStateText('installed'), 'call_zhi 已免确认')
  assert.equal(permissionStateText('manual'), '已有你自己设置的 call_zhi 权限')
  assert.match(permissionStateText('missing'), /还没免确认/)
  assert.match(permissionStateText('invalid'), /不会自动改/)
  assert.equal(permissionStateText(null), '')
})

test('manual MCP config prefers the stable entry and falls back to the bundled server', () => {
  const location = { ok: true, bundled_server: '/Applications/iterate.app/Contents/MacOS/mcp-server', stable_server: '/Users/example/me/Library/Application Support/iterate/bin/mcp-server' }
  assert.equal(manualMcpCommand({ ...location, stable_server_ready: true }), location.stable_server)
  assert.equal(manualMcpCommand({ ...location, stable_server_ready: false }), location.bundled_server)
  assert.equal(manualMcpCommand({ ...location, ok: false }), null)
  assert.equal(manualMcpCommand(null), null)

  const parsed = JSON.parse(buildManualMcpConfig('C:\\Users\\example\\iterate\\mcp-server.exe'))
  assert.deepEqual(parsed, { mcpServers: { [MCP_SERVER_NAME]: { command: 'C:\\Users\\example\\iterate\\mcp-server.exe' } } })
  assert.equal(MCP_SERVER_NAME, 'iterate-zhi')
  assert.match(RULES_FILE_HINT, /CLAUDE\.md \/ AGENTS\.md \/ \.cursorrules/)
})
