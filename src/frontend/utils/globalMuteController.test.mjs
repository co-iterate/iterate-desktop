/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { createGlobalMuteController } from './globalMuteController.mjs'

function service(initial = [1, false]) {
  let state = initial
  const subscribers = []
  return {
    get state() { return state },
    read: async () => state,
    write: async (muted) => {
      state = [state[0] + 1, muted]
      for (const subscriber of subscribers)
        subscriber(state)
      return state
    },
    subscribe: async callback => subscribers.push(callback),
    deliver: snapshot => subscribers.forEach(callback => callback(snapshot)),
  }
}

test('toggle in either window updates all windows and newly created windows inherit', async () => {
  const backend = service()
  let a, b, c
  const first = createGlobalMuteController({ ...backend, onChange: value => a = value })
  const second = createGlobalMuteController({ ...backend, onChange: value => b = value })
  await Promise.all([first.initialize(), second.initialize()])
  await first.toggle()
  assert.equal(a, true)
  assert.equal(b, true)
  const third = createGlobalMuteController({ ...backend, onChange: value => c = value })
  await third.initialize()
  assert.equal(c, true)
  await second.toggle()
  assert.deepEqual([a, b, c], [false, false, false])
})

test('old responses cannot re-enable mute after it was disabled', async () => {
  const backend = service()
  let value
  const controller = createGlobalMuteController({ ...backend, onChange: next => value = next })
  await controller.initialize()
  await controller.toggle()
  await controller.toggle()
  backend.deliver([2, true])
  assert.equal(value, false)
})

test('rapid double click is serialized and returns to the original state', async () => {
  const backend = service()
  const controller = createGlobalMuteController({ ...backend, onChange: () => {} })
  await Promise.all([controller.toggle(), controller.toggle()])
  assert.deepEqual(backend.state, [3, false])
})

test('an event arriving during initial read wins over the stale read', async () => {
  let resolveRead
  let markReadStarted
  const readStarted = new Promise(resolve => markReadStarted = resolve)
  let accept
  let value
  const controller = createGlobalMuteController({
    read: () => new Promise((resolve) => {
      resolveRead = resolve
      markReadStarted()
    }),
    write: async () => [4, false],
    subscribe: async callback => accept = callback,
    onChange: next => value = next,
  })
  const pending = controller.initialize()
  await readStarted
  accept([3, true])
  resolveRead([1, false])
  await pending
  assert.equal(value, true)
})

test('failed writes leave all bells unchanged and allow a later retry', async () => {
  const backend = service()
  let fail = true
  let value
  const controller = createGlobalMuteController({
    ...backend,
    write: async (muted) => {
      if (fail)
        throw new Error('disk unavailable')
      return backend.write(muted)
    },
    onChange: next => value = next,
  })
  await controller.initialize()
  await assert.rejects(controller.toggle(), /disk unavailable/)
  assert.equal(value, false)
  fail = false
  await controller.toggle()
  assert.equal(value, true)
})

test('retry after a failed initial read does not add another subscription', async () => {
  let subscriptions = 0
  let reads = 0
  const controller = createGlobalMuteController({
    read: async () => {
      if (++reads === 1)
        throw new Error('temporary read error')
      return [1, false]
    },
    write: async () => [2, true],
    subscribe: async () => { subscriptions++ },
    onChange: () => {},
  })
  await assert.rejects(controller.initialize(), /temporary read error/)
  await controller.initialize()
  assert.equal(subscriptions, 1)
})

test('an unimplemented or malformed backend response fails instead of silently fixing the bell on', async () => {
  const controller = createGlobalMuteController({
    read: async () => ({}),
    write: async () => ({}),
    subscribe: async () => {},
    onChange: () => assert.fail('invalid state must not update the bell'),
  })
  await assert.rejects(controller.initialize(), /Invalid global mute snapshot/)
})
