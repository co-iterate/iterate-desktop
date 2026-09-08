/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { nativeWindowCloseAction } from './nativeWindowClose.ts'

test('native X cancels an active request and never turns a resolving Send into exit', () => {
  assert.equal(nativeWindowCloseAction(true, false, true), 'cancel')
  assert.equal(nativeWindowCloseAction(true, false, false), 'cancel')
  assert.equal(nativeWindowCloseAction(false, true, false), 'ignore')
  assert.equal(nativeWindowCloseAction(true, true, true), 'ignore')
})

test('a queued interaction close cannot become an idle global exit', () => {
  assert.equal(nativeWindowCloseAction(false, false, true), 'ignore')
  assert.equal(nativeWindowCloseAction(false, false, false), 'exit')
})
