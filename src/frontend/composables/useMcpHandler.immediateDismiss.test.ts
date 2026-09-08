/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

const source = await readFile(new URL('./useMcpHandler.ts', import.meta.url), 'utf8')
const shortcutsSource = await readFile(new URL('./useShortcuts.ts', import.meta.url), 'utf8')
const builderSource = await readFile(new URL('../../rust/app/builder.rs', import.meta.url), 'utf8')
const commandsSource = await readFile(new URL('../../rust/ui/commands.rs', import.meta.url), 'utf8')
const appInitializationSource = await readFile(new URL('./useAppInitialization.ts', import.meta.url), 'utf8')
const windowEventsSource = await readFile(new URL('../../rust/ui/window_events.rs', import.meta.url), 'utf8')

describe('MCP response immediate dismissal', () => {
  it('dismisses the popup before route lookup and response persistence', () => {
    const responseHandler = source.match(/async function handleMcpResponse\(response: any, nativeClose = false\) \{([\s\S]*?)\n {2}\}/)?.[1]
    assert.ok(responseHandler)
    const dismissIndex = responseHandler.indexOf('await dismissMcpUiImmediately(request, nativeClose === true)')
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

  it('registers Windows native close handling before loading the cold request', () => {
    const listenerIndex = appInitializationSource.indexOf('await setupMcpEventListener()')
    const launchContextIndex = appInitializationSource.indexOf('await checkMcpMode()')
    assert.ok(listenerIndex >= 0)
    assert.ok(launchContextIndex > listenerIndex)
    assert.match(source, /listen<boolean>\('native-mcp-close-requested'/)
    assert.match(source, /await invoke\('mark_native_mcp_close_listener_ready'\)/)
    assert.match(source, /await handleMcpCloseCurrentDialog\(true\)/)
    assert.match(windowEventsSource, /has_pending_resident_mcp_request/)
    assert.match(windowEventsSource, /native-mcp-close-requested/)
    assert.match(windowEventsSource, /pub async fn close_idle_windows_window[\s\S]*?if is_standalone_mcp_interaction\(\) \|\| has_pending_resident_mcp_request/)
  })
})
