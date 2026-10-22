/* eslint-disable test/no-import-node-test -- this repository executes source-contract tests with `node --test` */
import assert from 'node:assert/strict'
import test from 'node:test'
import { createMockGuideDriver, readGuideOutcome, stepStates, writeGuideOutcome } from './firstRunGuide.ts'

test('steps unlock strictly in order', () => {
  assert.deepEqual(stepStates([false, false, false]), ['active', 'locked', 'locked'])
  assert.deepEqual(stepStates([true, false, false]), ['done', 'active', 'locked'])
  assert.deepEqual(stepStates([true, true, true]), ['done', 'done', 'done'])
})

test('an incoming call only counts after a client is connected', async () => {
  const driver = createMockGuideDriver({ callDelayMs: 10 })
  const calls: string[] = []
  const stop = driver.watchIncomingCall(call => calls.push(call.clientName))
  await new Promise(resolve => setTimeout(resolve, 30))
  assert.deepEqual(calls, [], 'no call before connecting')
  stop()

  await driver.connectClients(['codex'])
  const clients = await driver.listClients()
  assert.equal(clients.find(client => client.id === 'codex')?.connected, true)

  const received = await new Promise<string>((resolve) => {
    const stopAgain = driver.watchIncomingCall((call) => {
      stopAgain()
      resolve(call.clientName)
    })
  })
  assert.equal(received, 'Codex')
})

test('guide outcome falls back to null when storage is unavailable', () => {
  // node 环境没有 localStorage，读写都不能抛错
  assert.equal(readGuideOutcome(), null)
  assert.doesNotThrow(() => writeGuideOutcome('done'))
})

test('guide outcome round-trips and ignores unknown values', () => {
  const store = new Map<string, string>()
  ;(globalThis as any).localStorage = {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => store.set(key, value),
  }
  try {
    writeGuideOutcome('skipped')
    assert.equal(readGuideOutcome(), 'skipped')
    store.set('iterate.firstRunGuide.v1', 'garbage')
    assert.equal(readGuideOutcome(), null)
  }
  finally {
    delete (globalThis as any).localStorage
  }
})
