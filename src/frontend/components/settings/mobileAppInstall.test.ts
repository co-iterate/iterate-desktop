/* eslint-disable test/no-import-node-test -- this repository executes source-contract tests with `node --test` */
import assert from 'node:assert/strict'
import test from 'node:test'
import {
  IPHONE_APP_STORE_URL,
  needsFreshPairingAfterInstall,
  readAppInstallConfirmed,
  writeAppInstallConfirmed,
} from './mobileAppInstall.ts'

test('app store link is a plain https link the system camera can open', () => {
  const url = new URL(IPHONE_APP_STORE_URL)
  assert.equal(url.protocol, 'https:')
  assert.equal(url.host, 'apps.apple.com')
  assert.match(url.pathname, /\/id6796915922$/)
})

test('install confirmation falls back to false when storage is unavailable', () => {
  // node 环境没有 localStorage，读写都不能抛错
  assert.equal(readAppInstallConfirmed(), false)
  assert.doesNotThrow(() => writeAppInstallConfirmed())
})

test('install confirmation round-trips and ignores unknown values', () => {
  const store = new Map<string, string>()
  ;(globalThis as any).localStorage = {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => store.set(key, value),
  }
  try {
    assert.equal(readAppInstallConfirmed(), false)
    writeAppInstallConfirmed()
    assert.equal(readAppInstallConfirmed(), true)
    store.set('iterate.mobileAppInstalled.v1', 'garbage')
    assert.equal(readAppInstallConfirmed(), false)
  }
  finally {
    delete (globalThis as any).localStorage
  }
})

test('a pairing code that is about to expire is replaced after installing', () => {
  assert.equal(needsFreshPairingAfterInstall(0), true)
  assert.equal(needsFreshPairingAfterInstall(59), true)
  assert.equal(needsFreshPairingAfterInstall(60), false)
  assert.equal(needsFreshPairingAfterInstall(240), false)
})
