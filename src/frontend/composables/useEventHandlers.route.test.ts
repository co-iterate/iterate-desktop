/* eslint-disable test/no-import-node-test -- this repository executes source tests with `node --test` */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { isBridgeActionForWindow, useEventHandlers } from './useEventHandlers.ts'

const currentRequest = {
  id: 'request-a',
  project_path: 'E:\\Github\\iterate-desktop',
}

test('Bridge action requires both route parts to match the current request', () => {
  const matching = {
    action: 'submit',
    request_id: 'request-a',
    project_path: currentRequest.project_path,
  }
  assert.equal(isBridgeActionForWindow(matching, currentRequest), true)
  assert.equal(isBridgeActionForWindow({ ...matching, request_id: 'request-b' }, currentRequest), false)
  assert.equal(isBridgeActionForWindow({ ...matching, project_path: 'E:\\Github\\other' }, currentRequest), false)
  assert.equal(isBridgeActionForWindow({ ...matching, request_id: undefined }, currentRequest), false)
  assert.equal(isBridgeActionForWindow({ ...matching, project_path: undefined }, currentRequest), false)
  assert.equal(isBridgeActionForWindow(matching, null), false)
  assert.equal(isBridgeActionForWindow(matching, { ...currentRequest, id: undefined }), false)
  assert.equal(isBridgeActionForWindow(matching, { ...currentRequest, project_path: undefined }), false)
  assert.equal(isBridgeActionForWindow({ ...matching, request_id: ' request-a ' }, currentRequest), true)
})

test('Bridge action accepts established route aliases', () => {
  assert.equal(isBridgeActionForWindow({
    requestId: 'request-a',
    projectPath: currentRequest.project_path,
  }, {
    metadata: { request_id: 'request-a' },
    projectPath: currentRequest.project_path,
  }), true)
})

test('legacy project-only action reaches only an unbound window for that project', () => {
  const projectOnly = { action: 'continue', project_path: currentRequest.project_path }
  assert.equal(isBridgeActionForWindow(projectOnly, { project_path: currentRequest.project_path }), true)
  assert.equal(isBridgeActionForWindow(projectOnly, currentRequest), false)
  assert.equal(isBridgeActionForWindow(projectOnly, { project_path: 'E:\\Github\\other' }), false)
  assert.equal(isBridgeActionForWindow({ action: 'continue' }, { project_path: currentRequest.project_path }), false)
})

test('mismatched or incomplete Bridge action never invokes a window closing handler', async () => {
  const calls: string[] = []
  const noop = () => {}
  const actions = {
    mcp: {
      handleResponse: () => calls.push('submit'),
      handleCancel: () => calls.push('cancel'),
      handleCloseCurrentDialog: noop,
      handleMcpContinue: () => calls.push('continue'),
      handleMcpLoopReply: noop,
      handleMcpEnhance: noop,
      handleUpdateConditionalState: noop,
      handleUpdateConditionalActive: noop,
      handleUpdateCustomPromptOrder: noop,
      toggleMute: noop,
    },
    theme: { setTheme: noop },
    settings: {
      toggleAlwaysOnTop: noop,
      toggleAudioNotification: noop,
      updateAudioUrl: noop,
      testAudio: noop,
      stopAudio: noop,
      updateWindowSize: noop,
      updateReplyConfig: noop,
      setMessageInstance: noop,
      reloadAllSettings: noop,
    },
    audio: { handleTestError: noop },
  }
  let activeRequest: typeof currentRequest | null = currentRequest
  const { onBridgeAction } = useEventHandlers(actions, () => activeRequest)
  const route = { request_id: 'request-a', project_path: currentRequest.project_path }

  await onBridgeAction({ action: 'submit', ...route, request_id: 'request-b' })
  await onBridgeAction({ action: 'cancel', ...route, project_path: 'E:\\Github\\other' })
  await onBridgeAction({ action: 'continue', project_path: route.project_path })
  activeRequest = null
  await onBridgeAction({ action: 'submit', ...route })
  assert.deepEqual(calls, [])

  activeRequest = currentRequest
  await onBridgeAction({ action: 'continue', ...route })
  assert.deepEqual(calls, ['continue'])
})

test('both AppContent Bridge entrances filter before conditional changes or forwarding', async () => {
  const source = await readFile(new URL('../components/AppContent.vue', import.meta.url), 'utf8')
  assert.match(source, /if \(action && isBridgeActionForWindow\(action, props\.mcpRequest\)\) \{\s+if \(!handleWindowConditionalAction\(action\)\)/)
  assert.match(source, /else if \(message_type === 'mcp_action'\) \{[\s\S]*?if \(!isBridgeActionForWindow\(payload, props\.mcpRequest\)\)\s+return\s+if \(handleWindowConditionalAction\(payload\)\)/)
})
