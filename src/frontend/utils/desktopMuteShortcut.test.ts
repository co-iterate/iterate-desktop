/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { isDesktopMuteToggle } from './desktopMuteShortcut.ts'

const escape = {
  key: 'Escape',
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  repeat: false,
  isComposing: false,
  defaultPrevented: false,
}

test('plain Escape toggles only an active popup document', () => {
  assert.equal(isDesktopMuteToggle(escape, true), true)
  assert.equal(isDesktopMuteToggle(escape, false), false)
  assert.equal(isDesktopMuteToggle({ ...escape, key: 'Tab' }, true), false)
})

test('repeat, IME cancellation, handled dialogs and modified Escape do not toggle mute', () => {
  for (const key of ['repeat', 'isComposing', 'defaultPrevented', 'metaKey', 'ctrlKey', 'altKey', 'shiftKey'])
    assert.equal(isDesktopMuteToggle({ ...escape, [key]: true }, true), false, key)
})
