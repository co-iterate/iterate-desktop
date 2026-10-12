import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { spawnSync } from 'node:child_process'
import test from 'node:test'

const root = path.resolve(new URL('.', import.meta.url).pathname, '..')
const helper = path.join(root, 'scripts', 'merge-codex-mcp-config.py')

function runMerge(initial, command) {
  const dir = mkdtempSync(path.join(tmpdir(), 'cunzhi-config-'))
  const configPath = path.join(dir, 'config.toml')
  try {
    writeFileSync(configPath, initial)
    const result = spawnSync('python3', [helper, configPath, command], { encoding: 'utf8' })
    assert.equal(result.status, 0, result.stderr || result.stdout)
    return readFileSync(configPath, 'utf8')
  }
  finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

test('preserves the existing Codex MCP options while updating the binary path', () => {
  const output = runMerge(`[mcp_servers."iterate-zhi"]
command = "/old/mcp-server"
args = ["5411"]
tool_timeout_sec = 315360000.0
disabled = false

[mcp_servers."iterate-zhi".tools.call_zhi]
approval_mode = "approve"

[mcp_servers.other]
command = "/other"
`, '/new/mcp-server')

  assert.match(output, /command = "\/new\/mcp-server"/)
  assert.match(output, /args = \["5411"\]/)
  assert.match(output, /tool_timeout_sec = 315360000/)
  assert.match(output, /disabled = false/)
  assert.match(output, /\[mcp_servers\."iterate-zhi"\.tools\.call_zhi\]/)
  assert.match(output, /approval_mode = "approve"/)
  assert.match(output, /\[mcp_servers\.other\]/)
})

test('adds the ten-year timeout without removing an existing server block', () => {
  const output = runMerge(`[mcp_servers."iterate-zhi"]
command = "/old/mcp-server"

[mcp_servers.other]
command = "/other"
`, '/new/mcp-server')

  assert.match(output, /command = "\/new\/mcp-server"/)
  assert.match(output, /args = \[\]/)
  assert.match(output, /tool_timeout_sec = 315360000/)
  assert.match(output, /\[mcp_servers\."iterate-zhi"\.tools\.call_zhi\]/)
  assert.match(output, /approval_mode = "approve"/)
  assert.match(output, /\[mcp_servers\.other\]/)
})

test('fresh Codex config auto-approves call_zhi while keeping the long timeout', () => {
  const output = runMerge('', '/new/mcp-server')

  assert.match(output, /\[mcp_servers\."iterate-zhi"\]/)
  assert.match(output, /command = "\/new\/mcp-server"/)
  assert.match(output, /tool_timeout_sec = 315360000/)
  assert.match(output, /\[mcp_servers\."iterate-zhi"\.tools\.call_zhi\]/)
  assert.match(output, /approval_mode = "approve"/)
})

test('adds default approval to an existing call_zhi tool block when unset', () => {
  const output = runMerge(`[mcp_servers."iterate-zhi"]
command = "/old/mcp-server"

[mcp_servers."iterate-zhi".tools.call_zhi]
enabled = true
`, '/new/mcp-server')

  assert.match(output, /enabled = true/)
  assert.match(output, /approval_mode = "approve"/)
})

test('preserves an explicit user approval mode instead of silently loosening it', () => {
  const output = runMerge(`[mcp_servers."iterate-zhi"]
command = "/old/mcp-server"

[mcp_servers."iterate-zhi".tools.call_zhi]
approval_mode = "prompt"
`, '/new/mcp-server')

  assert.match(output, /approval_mode = "prompt"/)
  assert.doesNotMatch(output, /approval_mode = "approve"/)
})

test('both installers delegate Codex config merging to the shared helper', () => {
  for (const installer of ['install.sh', 'release-package/install.sh']) {
    const source = readFileSync(path.join(root, installer), 'utf8')
    assert.match(source, /merge-codex-mcp-config\.py/)
  }
})
