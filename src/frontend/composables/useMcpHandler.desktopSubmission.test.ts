/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { stripTypeScriptTypes } from 'node:module'
import { describe, it } from 'node:test'
import { runInNewContext } from 'node:vm'
import { copySubmissionToClipboard } from '../utils/submissionClipboard.ts'
import { isDesktopMcpSubmission, isDesktopSubmissionCurrent, runDesktopMcpSubmission } from './desktopMcpSubmission.ts'

const handlerSource = await readFile(new URL('./useMcpHandler.ts', import.meta.url), 'utf8')
const popupSource = (await readFile(new URL('../components/popup/McpPopup.vue', import.meta.url), 'utf8')).replace(/\r\n/g, '\n')
const popupSubmit = popupSource.match(/async function handleSubmit\(\) \{[\s\S]*?\n\}\n/)?.[0]
assert.ok(popupSubmit)
function popupFunction(name: string) {
  const source = popupSource.match(new RegExp(`(?:async )?function ${name}\\([\\s\\S]*?\\n\\}\\n`))?.[0]
  assert.ok(source, name)
  return source
}
const realPreparationFunctions = ['backupCurrentSubmissionToClipboard', 'applyLiveGoalIntent', 'resolveLiveGoalResponseMetadata', 'emptyLiveGoalRunMetadata', 'normalizeOptionalString', 'normalizeOptionalNumber']
  .map(popupFunction)
  .join('\n')

function deferred<T = void>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

async function tick() {
  for (let index = 0; index < 20; index++)
    await Promise.resolve()
}

function harness(standalone = false, tabRestoration = false) {
  const calls: string[] = []
  const delivery = deferred()
  const clipboard = deferred()
  const liveGoal = deferred<any>()
  const goalMetadata = deferred<any>()
  const cleanup = deferred()
  const references: any[] = []
  const goalCalls: { command: string, args: any }[] = []
  const messages: string[] = []
  let deliveredResponse: any
  const ref = (value: any) => {
    const result = { value }
    references.push(result)
    return result
  }
  const deliveryError = { value: '' }
  const window = {
    async show() { calls.push('show') },
    async setFocus() { calls.push('focus') },
  }
  const source = { id: 'serve-original', project_path: '/original/project', timeline_route_id: 'original-route', codex_thread_id: 'original-thread', codex_deeplink: 'original-deeplink' }
  const context: any = {
    ref,
    nextTick: () => Promise.resolve(),
    localStorage: { getItem: () => null, setItem() {} },
    window: { addEventListener() {} },
    console: { info() {}, warn() {}, error() {} },
    isDesktopMcpSubmission,
    isDesktopSubmissionCurrent,
    runDesktopMcpSubmission,
    useMcpDelivery() {},
    useNotification: () => ({}),
    mcpDeliveryError: deliveryError,
    crossDeviceSendError: { value: '' },
    MCP_DELIVERY_FAILURE: '未确认送达',
    clearActiveMcpFatalContext() {},
    setActiveMcpFatalContext() {},
    getCurrentWindow: () => window,
    publishCompletedMcpRequest: async () => {
      calls.push('cleanup')
      await cleanup.promise
    },
    invoke: async (command: string, args: any) => {
      if (command === 'dismiss_standalone_mcp_window') {
        calls.push('hide')
        return
      }
      if (command === 'send_mcp_response') {
        calls.push('deliver')
        deliveredResponse = args.response
        assert.equal(args.requestId, source.id)
        assert.equal(args.response.metadata.request_id, source.id)
        await delivery.promise
        return
      }
      if (command === 'complete_popup_tab') {
        calls.push('complete-tab')
        assert.equal(args.requestId, source.id)
        return
      }
      if (command === 'restore_popup_tab')
        return tabRestoration
      if (command === 'exit_app')
        calls.push('exit')
      if (command === 'get_window_config')
        return { focus_popup_on_show: true }
      return null
    },
  }
  const js = stripTypeScriptTypes(handlerSource.replace(/^import .*\r?\n/gm, '').replace(/^export /gm, ''))
  runInNewContext(`${js}\nglobalThis.factory = useMcpHandler`, context)
  const handler = context.factory()
  references[3].value = standalone
  handler.mcpRequest.value = source
  handler.showMcpPopup.value = true
  const draft = {
    userInput: { value: 'original text' },
    rawUserInput: { value: 'original raw text' },
    selectedOptions: { value: ['original option'] },
    draggedImages: { value: ['original-image'] },
    attachedFiles: { value: [{ path: '/original/file.md', name: 'file.md' }] },
    submitting: { value: false },
  }
  let pending: Promise<void> = Promise.resolve()
  let emitted = 0
  const popupContext: any = {
    ...draft,
    props: { request: source, mockMode: false, testMode: false },
    canSubmit: { value: true },
    isNative: { value: false },
    console: context.console,
    message: {
      error() { calls.push('popup-error') },
      success() { messages.push('success') },
      warning() { messages.push('warning') },
    },
    invoke: async (command: string, args: any) => {
      if (command === 'plugin:clipboard-manager|write_text') {
        calls.push('clipboard')
        await clipboard.promise
      }
      if (command === 'start_live_goal') {
        calls.push('goal')
        goalCalls.push({ command, args })
        return liveGoal.promise
      }
      if (command === 'resolve_live_goal_response_metadata') {
        calls.push('goal-metadata')
        goalCalls.push({ command, args })
        return goalMetadata.promise
      }
    },
    copySubmissionToClipboard,
    copySubmissionToClipboardEnabled: { value: true },
    isDesktopSubmissionCurrent,
    inputRef: { value: { recordSubmittedInputForAutoPromotion() {} } },
    buildFinalUserInput: (text: string) => text,
    buildImageAttachments: (images: string[]) => images,
    buildFilePaths: (files: any[]) => files.map(file => file.path),
    resolveSubmitSource: () => 'popup_submit',
    resolveLiveGoalIntent: (text: string) => ({ action: 'start', title: text }),
    emit: (_event: string, submission: any) => {
      emitted++
      pending = handler.handleMcpResponse(submission)
    },
  }
  runInNewContext(`${stripTypeScriptTypes(realPreparationFunctions + popupSubmit)}\nglobalThis.click = handleSubmit`, popupContext)
  return {
    calls,
    draft,
    source,
    handler,
    deliveryError,
    clipboard,
    liveGoal,
    goalMetadata,
    delivery,
    cleanup,
    goalCalls,
    messages,
    popup: popupContext,
    deliveredResponse: () => deliveredResponse,
    click: () => popupContext.click(),
    pending: () => pending,
    emitted: () => emitted,
  }
}

async function finishPreparation(test: ReturnType<typeof harness>) {
  test.clipboard.resolve()
  await tick()
  test.liveGoal.resolve({ run_id: 'original-run', generation: 1 })
  await tick()
  test.goalMetadata.resolve({ run_id: 'original-run', generation: 1 })
  await tick()
}

describe('actual desktop popup send and handler with deferred native/backend ports', () => {
  for (const standalone of [false, true]) {
    it(`hides before all preparation, retains draft/source until ACK, and guards repeat click (${standalone ? 'standalone' : 'resident'})`, async () => {
      const test = harness(standalone)
      await test.click()
      await tick()
      assert.deepEqual(test.calls, ['hide', 'clipboard'])
      assert.equal(test.handler.mcpRequest.value, test.source)
      assert.equal(test.handler.showMcpPopup.value, true)
      assert.equal(test.draft.submitting.value, true)
      await test.click()
      assert.equal(test.emitted(), 1)
      await finishPreparation(test)
      assert.deepEqual(test.calls, ['hide', 'clipboard', 'goal', 'goal-metadata', 'deliver'])
      assert.ok(!test.calls.includes('complete-tab'))
      assert.equal(test.handler.mcpRequest.value, test.source)
      assert.equal(test.draft.attachedFiles.value[0].path, '/original/file.md')
      test.delivery.resolve()
      await tick()
      assert.equal(test.handler.mcpRequest.value, null)
      assert.equal(test.handler.showMcpPopup.value, false)
      assert.equal(test.calls.includes('complete-tab'), standalone)
      if (standalone)
        assert.ok(test.calls.indexOf('complete-tab') < test.calls.indexOf('cleanup'))
      assert.ok(!test.calls.includes('show'))
      test.cleanup.resolve()
      await test.pending()
      assert.equal(test.draft.submitting.value, false)
      assert.equal(test.calls.includes('exit'), standalone)
    })
  }

  for (const failure of ['prepare', 'delivery']) {
    it(`restores the same mounted draft once and exposes the actual ${failure} failure`, async () => {
      const test = harness(true)
      await test.click()
      await tick()
      const originalDraft = JSON.stringify(test.draft)
      if (failure === 'prepare') {
        test.popup.inputRef.value.recordSubmittedInputForAutoPromotion = () => {
          throw new Error('actual prepare failure')
        }
        test.clipboard.resolve()
      }
      else {
        await finishPreparation(test)
        test.delivery.reject(new Error('actual delivery failure'))
      }
      await test.pending()
      assert.equal(test.calls.filter(call => call === 'show').length, 1)
      assert.equal(test.handler.mcpRequest.value, test.source)
      assert.equal(test.handler.showMcpPopup.value, true)
      assert.equal(JSON.stringify({ ...test.draft, submitting: { value: true } }), originalDraft)
      assert.match(test.deliveryError.value, new RegExp(`actual ${failure} failure`))
      assert.equal(test.draft.submitting.value, false)
      assert.ok(!test.calls.includes('cleanup'))
      assert.ok(!test.calls.includes('complete-tab'))
    })
  }

  it('does not reopen an accepted request if success cleanup fails', async () => {
    const test = harness(true)
    await test.click()
    await finishPreparation(test)
    test.delivery.resolve()
    await tick()
    test.cleanup.reject(new Error('cleanup failed after delivery ACK'))
    await test.pending()
    assert.equal(test.handler.mcpRequest.value, null)
    assert.equal(test.calls.filter(call => call === 'complete-tab').length, 1)
    assert.equal(test.handler.showMcpPopup.value, false)
    assert.ok(!test.calls.includes('show'))
    assert.equal(test.deliveryError.value, '')
    assert.equal(test.draft.submitting.value, false)
  })

  it('lets native tab restoration preserve the selected peer on delivery failure', async () => {
    const test = harness(true, true)
    await test.click()
    await finishPreparation(test)
    test.delivery.reject(new Error('delivery failed after tab handoff'))
    await test.pending()
    assert.equal(test.handler.mcpRequest.value, test.source)
    assert.equal(test.handler.showMcpPopup.value, true)
    assert.match(test.deliveryError.value, /delivery failed after tab handoff/)
    assert.ok(!test.calls.includes('show'))
    assert.ok(!test.calls.includes('focus'))
    assert.ok(!test.calls.includes('complete-tab'))
    assert.equal(test.draft.attachedFiles.value[0].path, '/original/file.md')
    assert.equal(test.draft.submitting.value, false)
  })

  it('does not deliver or resurrect an old source if ownership changes during preparation', async () => {
    const test = harness()
    await test.click()
    await tick()
    test.handler.mcpRequest.value = { id: 'different-live-request' }
    await finishPreparation(test)
    await test.pending()
    assert.ok(!test.calls.includes('deliver'))
    assert.ok(!test.calls.includes('show'))
    assert.equal(test.handler.mcpRequest.value.id, 'different-live-request')
  })

  it('does not clear or exit a different active request after the original delivery ACK', async () => {
    const test = harness(true)
    await test.click()
    await finishPreparation(test)
    test.handler.mcpRequest.value = { id: 'different-live-request' }
    test.delivery.resolve()
    await tick()
    test.cleanup.resolve()
    await test.pending()
    assert.equal(test.handler.mcpRequest.value.id, 'different-live-request')
    assert.ok(!test.calls.includes('show'))
    assert.ok(!test.calls.includes('exit'))
  })

  it('locks click-time text/options/attachments and full request fields through preparation', async () => {
    const test = harness()
    await test.click()
    await tick()
    test.draft.userInput.value = 'later text'
    test.draft.rawUserInput.value = 'later raw text'
    test.draft.selectedOptions.value.splice(0, 1, 'later option')
    test.draft.draggedImages.value.splice(0, 1, 'later image')
    test.draft.attachedFiles.value[0].path = '/later/file.md'
    test.source.codex_thread_id = 'later-thread'
    test.source.codex_deeplink = 'later-deeplink'
    await finishPreparation(test)
    assert.equal(test.goalCalls[0].args.title, 'original text')
    assert.equal(test.goalCalls[0].args.requestId, 'serve-original')
    assert.equal(test.goalCalls[0].args.projectPath, '/original/project')
    assert.equal(test.goalCalls[0].args.codexThreadId, 'original-thread')
    assert.equal(test.goalCalls[0].args.codexDeeplink, 'original-deeplink')
    assert.equal(test.goalCalls[1].args.projectPath, '/original/project')
    assert.deepEqual(JSON.parse(JSON.stringify(test.deliveredResponse().selected_options)), ['original option'])
    assert.deepEqual(JSON.parse(JSON.stringify(test.deliveredResponse().images)), ['original-image'])
    assert.deepEqual(JSON.parse(JSON.stringify(test.deliveredResponse().file_paths)), ['/original/file.md'])
    assert.equal(test.deliveredResponse().user_input, 'original text')
    test.delivery.resolve()
    await tick()
    test.cleanup.resolve()
    await test.pending()
  })

  for (const waitAt of ['clipboard', 'goal', 'metadata']) {
    it(`never runs new-question Goal/meta or mutates its draft when ownership changes at ${waitAt}`, async () => {
      const test = harness(true)
      await test.click()
      await tick()
      if (waitAt !== 'clipboard') {
        test.clipboard.resolve()
        await tick()
      }
      if (waitAt === 'metadata') {
        test.liveGoal.resolve({ run_id: 'original-run', generation: 1 })
        await tick()
      }
      const countBeforeSwitch = test.goalCalls.length
      const messagesBeforeSwitch = test.messages.length
      const newQuestion = { id: 'new-question', project_path: '/new/project', codex_thread_id: 'new-thread' }
      test.handler.mcpRequest.value = newQuestion
      test.popup.props.request = newQuestion
      test.draft.userInput.value = 'new-question goal'
      test.draft.selectedOptions.value = ['new option']
      test.draft.draggedImages.value = ['new image']
      test.draft.attachedFiles.value = [{ path: '/new/file.md', name: 'file.md' }]
      test.draft.submitting.value = true // The new question now owns this busy state.
      const newDraft = JSON.stringify(test.draft)
      if (waitAt === 'clipboard')
        test.clipboard.resolve()
      else if (waitAt === 'goal')
        test.liveGoal.resolve({ run_id: 'original-run', generation: 1 })
      else
        test.goalMetadata.resolve({ run_id: 'original-run', generation: 1 })
      await test.pending()
      assert.equal(test.goalCalls.length, countBeforeSwitch)
      assert.ok(test.goalCalls.every(call => call.args.projectPath === '/original/project'))
      assert.equal(test.messages.length, messagesBeforeSwitch)
      assert.equal(JSON.stringify(test.draft), newDraft)
      assert.equal(test.handler.mcpRequest.value, newQuestion)
      assert.ok(!test.calls.includes('deliver'))
      assert.ok(!test.calls.includes('show'))
      assert.ok(!test.calls.includes('exit'))
    })
  }
})
