/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

const source = await readFile(new URL('./useMcpHandler.ts', import.meta.url), 'utf8')
const shortcutsSource = await readFile(new URL('./useShortcuts.ts', import.meta.url), 'utf8')
const builderSource = await readFile(new URL('../../rust/app/builder.rs', import.meta.url), 'utf8')
const commandsSource = await readFile(new URL('../../rust/ui/commands.rs', import.meta.url), 'utf8')

describe('MCP response immediate dismissal', () => {
  it('dismisses the popup before route lookup and response persistence', () => {
    const responseHandler = source.match(/async function handleMcpResponse\(response: any\) \{([\s\S]*?)\n {2}\}/)?.[1]
    assert.ok(responseHandler)
    const dismissIndex = responseHandler.indexOf('await dismissMcpUiImmediately(request)')
    const routeIndex = responseHandler.indexOf('await resolveConversationRouteIdWithFallback')
    const sendIndex = responseHandler.indexOf('send_mcp_response')
    assert.ok(dismissIndex >= 0)
    assert.ok(routeIndex > dismissIndex)
    assert.ok(sendIndex > routeIndex)
  })

  it('restores both inline and standalone UI after a send failure', () => {
    assert.match(source, /showMcpPopup\.value = false\s+mcpRequest\.value = null/)
    assert.match(source, /await invoke\('dismiss_standalone_mcp_window'\)/)
    assert.match(source, /mcpRequest\.value = dismissal\.request\s+showMcpPopup\.value = true/)
    assert.match(source, /await window\.show\(\)\s+await window\.setFocus\(\)/)
    assert.match(source, /MCP响应处理失败:[\s\S]*await restoreMcpUiAfterFailure\(dismissal\)/)
    assert.match(source, /MCP取消处理失败:[\s\S]*await restoreMcpUiAfterFailure\(dismissal\)/)
  })

  it('consumes configured popup shortcuts at keydown before AppKit sees them', () => {
    assert.doesNotMatch(shortcutsSource, /const keys = useMagicKeys\(\)/)
    assert.match(shortcutsSource, /window\.addEventListener\('keydown', handleKeydown, true\)/)
    assert.match(shortcutsSource, /event\.preventDefault\(\)\s+event\.stopPropagation\(\)/)
    assert.match(shortcutsSource, /if \(event\.repeat\) \{\s+return\s+\}\s+callback\(\)/)
  })

  it('restores the exact pre-popup macOS application after hiding the standalone window', () => {
    assert.match(builderSource, /remember_standalone_previous_frontmost_application\(\);/)
    assert.match(commandsSource, /capture_frontmost_application\(\)/)
    assert.match(commandsSource, /window\s+\.hide\(\)[\s\S]*restore_standalone_previous_frontmost_application\(\)/)
    assert.match(commandsSource, /activate_application\(&application\)/)
  })
})
