/* eslint-disable test/no-import-node-test -- this repository executes source-contract tests with `node --test` */
import type { PhoneTestDeps } from './phoneTestMessage.ts'
import assert from 'node:assert/strict'
import test from 'node:test'
import { PHONE_TEST_MESSAGE_TEXT, sendPhoneTestMessage } from './phoneTestMessage.ts'

function fakeDeps(statuses: Array<string | null>, publishOk = true) {
  let clock = 0
  const requests: Array<Record<string, unknown>> = []
  const deps: PhoneTestDeps = {
    publish: async (request) => {
      requests.push(request)
      return { ok: publishOk, id: String(request.id) }
    },
    fetchResult: async () => {
      const status = statuses.length > 1 ? statuses.shift()! : statuses[0]
      return status === null ? null : { status, received_at: '2026-10-23T10:00:00Z' }
    },
    sleep: async (ms) => {
      clock += ms
    },
    now: () => clock,
  }
  return { deps, requests }
}

test('sends a show_message action with the test text and reports delivery', async () => {
  const { deps, requests } = fakeDeps([null, 'pending', 'success'])
  const outcome = await sendPhoneTestMessage(deps, { deviceId: 'phone-1' })
  assert.deepEqual(outcome, { kind: 'delivered', at: '2026-10-23T10:00:00Z' })
  assert.equal(requests[0].action, 'show_message')
  assert.equal(requests[0].text, PHONE_TEST_MESSAGE_TEXT)
  assert.equal(requests[0].targetDeviceId, 'phone-1')
})

test('no online phone is reported without polling', async () => {
  const { deps } = fakeDeps(['success'], false)
  assert.deepEqual(await sendPhoneTestMessage(deps), { kind: 'no_phone' })
})

test('a backgrounded phone is flagged once, then waits until delivered', async () => {
  const { deps } = fakeDeps(['waiting_for_foreground', 'waiting_for_foreground', 'ok'])
  let flagged = 0
  const outcome = await sendPhoneTestMessage(deps, {
    onWaitingForForeground: () => {
      flagged += 1
    },
  })
  assert.equal(outcome.kind, 'delivered')
  assert.equal(flagged, 1)
})

test('times out instead of waiting forever', async () => {
  const { deps } = fakeDeps(['pending'])
  assert.deepEqual(await sendPhoneTestMessage(deps, { timeoutMs: 2000 }), { kind: 'timeout' })
  const { deps: background } = fakeDeps(['waiting_for_foreground'])
  assert.deepEqual(await sendPhoneTestMessage(background, { timeoutMs: 2000 }), { kind: 'waiting_for_foreground' })
})

test('a failed receipt carries its message', async () => {
  const { deps } = fakeDeps(['error'])
  assert.equal((await sendPhoneTestMessage(deps)).kind, 'failed')
})
