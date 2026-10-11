import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(__dirname, '..')

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8')
}

test('component test fixture mocks MCP tools as an array of tool configs', () => {
  const source = read('src/frontend/test/main.ts')

  assert.match(source, /command === 'get_mcp_tools_config'/)
  assert.match(source, /id:\s*'ji'/)
  assert.match(source, /name:\s*'记忆管理'/)
  assert.match(source, /can_disable:\s*true/)
  assert.match(source, /icon_bg:\s*'bg-/)
})

test('component test fixture registers Naive UI components used by McpToolsTab', () => {
  const source = read('src/frontend/test/main.ts')

  for (const component of [
    'NCollapse',
    'NCollapseItem',
    'NEmpty',
    'NFormItem',
    'NModal',
    'NSpin',
  ]) {
    assert.match(source, new RegExp(`\\b${component}\\b`), `${component} is not registered`)
  }
})
