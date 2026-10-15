/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

const source = await readFile(new URL('./useMcpHandler.ts', import.meta.url), 'utf8')
const shortcutsSource = await readFile(new URL('./useShortcuts.ts', import.meta.url), 'utf8')
const builderSource = await readFile(new URL('../../rust/app/builder.rs', import.meta.url), 'utf8')
const commandsSource = await readFile(new URL('../../rust/ui/commands.rs', import.meta.url), 'utf8')
const actionHandlerSource = await readFile(new URL('../../rust/bridge/mcp_action_handler.rs', import.meta.url), 'utf8')
const actionDeliverySource = await readFile(new URL('../../rust/bridge/mcp_action_delivery.rs', import.meta.url), 'utf8')
const cliSource = await readFile(new URL('../../rust/app/cli.rs', import.meta.url), 'utf8')

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
    assert.match(source, /await refreshGlobalMute\(\)\s+await invoke\('present_mcp_window'\)/)
    assert.match(source, /MCP响应处理失败:[\s\S]*await restoreMcpUiAfterFailure\(dismissal\)/)
    assert.match(source, /MCP取消处理失败:[\s\S]*await restoreMcpUiAfterFailure\(dismissal\)/)
  })

  it('closes only the matching resident popup as soon as a remote reply is delivered', () => {
    assert.match(source, /await listen\('bridge-mcp-action-handled',[\s\S]*handleRemoteActionHandled\(event\.payload\)/)
    assert.match(source, /const currentRequestId = resolveRequestId\(mcpRequest\.value\)/)
    assert.match(source, /!currentRequestId \|\| !targetRequestId \|\| currentRequestId !== targetRequestId/)
    assert.match(source, /if \(isMcpProcess\.value\)[\s\S]*return[\s\S]*showMcpPopup\.value = false\s+mcpRequest\.value = null/)

    const responseChannelBranch = actionHandlerSource.match(/if let Some\(sender\) = sender \{([\s\S]*?)\n {8}\} else \{/)?.[1]
    assert.ok(responseChannelBranch)
    const deliveryIndex = responseChannelBranch.indexOf('sender.send(response_str)')
    const ackIndex = responseChannelBranch.indexOf('"bridge-mcp-action-handled"')
    const timelineIndex = responseChannelBranch.indexOf('record_mcp_action_response_node(')
    assert.ok(deliveryIndex >= 0)
    assert.ok(ackIndex > deliveryIndex)
    assert.ok(timelineIndex > ackIndex)
    assert.match(responseChannelBranch, /"request_id": request_id/)
  })

  it('closes a standalone popup from its exact response file before timeline persistence', () => {
    assert.match(actionDeliverySource, /let Some\(request_id\) = request_id else \{[\s\S]*response_route_request_id_missing/)
    assert.match(actionDeliverySource, /let route_file = serve_response_route_file\(request_id\)/)
    assert.match(actionDeliverySource, /iterate_response_route_\{\}\.json/)

    const serveBranchStart = actionHandlerSource.indexOf('let serve_attempt = try_write_serve_response_file(')
    assert.ok(serveBranchStart >= 0)
    const serveBranch = actionHandlerSource.slice(serveBranchStart, actionHandlerSource.indexOf('return RoomDeliveryResult::from_attempts', serveBranchStart))
    const writeIndex = serveBranch.indexOf('try_write_serve_response_file(')
    const ackIndex = serveBranch.indexOf('"bridge-mcp-action-handled"')
    const timelineIndex = serveBranch.indexOf('record_mcp_action_response_node(')
    assert.ok(writeIndex >= 0)
    assert.ok(ackIndex > writeIndex)
    assert.ok(timelineIndex > ackIndex)

    assert.match(cliSource, /ITERATE_RESPONSE_FILE/)
    assert.match(cliSource, /if response_ready \{[\s\S]*kill_child_best_effort[\s\S]*"response_file_observed"/)
    assert.match(cliSource, /tokio::time::sleep\(tokio::time::Duration::from_millis\(50\)\)\.await/)
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
