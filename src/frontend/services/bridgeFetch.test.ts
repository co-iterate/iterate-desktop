/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import test from 'node:test'
import { bridgeFetch, normalizeBridgeUrlForFetch } from './bridgeFetch.ts'

async function withBridgeMocks(
  platform: string,
  invoke: (command: string, args: unknown) => Promise<string>,
  fetchMock: typeof fetch,
  run: () => Promise<void>,
  native = true,
) {
  const originals = new Map(['window', 'navigator', 'fetch'].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]))
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: native ? { __TAURI_INTERNALS__: { invoke } } : {},
  })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { platform } })
  Object.defineProperty(globalThis, 'fetch', { configurable: true, value: fetchMock })
  try {
    await run()
  }
  finally {
    for (const [key, descriptor] of originals) {
      if (descriptor)
        Object.defineProperty(globalThis, key, descriptor)
      else
        Reflect.deleteProperty(globalThis, key)
    }
  }
}

test('desktop bridge fetch accepts only the exact loopback service origin', () => {
  assert.equal(
    normalizeBridgeUrlForFetch('http://127.0.0.1:8080/api/config?tab=mobile').pathname,
    '/api/config',
  )
  assert.equal(
    normalizeBridgeUrlForFetch('http://[::1]:8080/api/version').hostname,
    '[::1]',
  )

  for (const url of [
    'https://127.0.0.1:8080/api/config',
    'http://127.0.0.1:8081/api/config',
    'http://192.168.1.5:8080/api/config',
    'http://example.com:8080/api/config',
    'http://user@localhost:8080/api/config',
  ]) {
    assert.throws(() => normalizeBridgeUrlForFetch(url), /bridge_fetch_requires_loopback_8080/)
  }
})

test('Windows native pairing reads fall back only when its auth broker is unavailable', async () => {
  const paths = [
    '/api/android/pairing/status',
    '/api/android/pairing/sessions/session-1',
  ]
  for (const path of paths) {
    const calls: Array<{ url: URL, init: RequestInit }> = []
    const signal = new AbortController().signal
    await withBridgeMocks(
      'Win32',
      async (_command, args) => {
        assert.deepEqual(args, { method: 'GET', path })
        // Tauri IPC rejects Rust string errors as strings.
        // eslint-disable-next-line no-throw-literal
        throw 'bridge_auth_broker_unavailable'
      },
      async (url, init) => {
        calls.push({ url: url as URL, init: init || {} })
        return new Response('{}', { status: 200 })
      },
      async () => {
        const response = await bridgeFetch(`http://127.0.0.1:8080${path}`, {
          headers: new Headers([['Authorization', 'Bearer caller-secret'], ['X-Test', 'kept']]),
          signal,
          cache: 'no-store',
          credentials: 'include',
        })
        assert.equal(response.status, 200)
      },
    )
    assert.equal(calls.length, 1)
    assert.equal(calls[0].url.href, `http://127.0.0.1:8080${path}`)
    assert.equal(calls[0].init.method, 'GET')
    assert.equal(calls[0].init.credentials, 'omit')
    assert.equal(calls[0].init.signal, signal)
    assert.equal(calls[0].init.cache, 'no-store')
    const headers = new Headers(calls[0].init.headers)
    assert.equal(headers.has('Authorization'), false)
    assert.equal(headers.get('X-Test'), 'kept')
  }
})

test('broker fallback excludes other errors, platforms, origins, methods, and paths', async () => {
  const cases = [
    { platform: 'Win32', error: 'other_error', url: 'http://127.0.0.1:8080/api/android/pairing' },
    { platform: 'Win32', error: 'prefix_bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing' },
    { platform: 'MacIntel', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing' },
    { platform: 'Darwin', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://localhost:8080/api/android/pairing' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://[::1]:8080/api/android/pairing' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/config' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/mobile/pairing' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/mobile/pairing/status' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing/extra' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing/sessions/' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing/sessions/id/extra' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing/sessions/id%2Fextra' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing/sessions/id%5Cextra' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing?extra=1' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing', method: 'POST' },
    { platform: 'Win32', error: 'bridge_auth_broker_unavailable', url: 'http://127.0.0.1:8080/api/android/pairing', native: false },
  ]
  for (const entry of cases) {
    let fetchCalls = 0
    await withBridgeMocks(
      entry.platform,
      async () => { throw entry.error },
      async () => {
        fetchCalls++
        return new Response()
      },
      async () => {
        await assert.rejects(
          bridgeFetch(entry.url, { method: entry.method }),
          error => entry.native === false ? error instanceof TypeError : error === entry.error,
        )
      },
      entry.native !== false,
    )
    assert.equal(fetchCalls, 0, JSON.stringify(entry))
  }
})

test('successful token uses bearer and HTTP denial is not retried', async () => {
  for (const status of [401, 403]) {
    let invocations = 0
    let fetchCalls = 0
    await withBridgeMocks(
      'Win32',
      async () => {
        invocations++
        return 'desktop-token'
      },
      async (_url, init) => {
        fetchCalls++
        assert.equal(new Headers(init?.headers).get('Authorization'), 'Bearer desktop-token')
        assert.equal(init?.credentials, 'omit')
        return new Response(null, { status })
      },
      async () => {
        const response = await bridgeFetch('http://127.0.0.1:8080/api/android/pairing')
        assert.equal(response.status, status)
      },
    )
    assert.equal(invocations, 1)
    assert.equal(fetchCalls, 1)
  }
})
