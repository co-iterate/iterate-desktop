import { invoke } from '@tauri-apps/api/core'

const LOOPBACK_HOSTS = new Set(['127.0.0.1', 'localhost', '[::1]'])
const PAIRING_SESSION_PREFIXES = ['/api/android/pairing/sessions/', '/api/mobile/pairing/sessions/']

function hasOnePairingSessionSegment(pathname: string): boolean {
  const prefix = PAIRING_SESSION_PREFIXES.find(value => pathname.startsWith(value))
  if (!prefix)
    return false

  const segment = pathname.slice(prefix.length)
  if (!segment || segment.includes('/'))
    return false

  try {
    const decoded = decodeURIComponent(segment)
    return !!decoded && !decoded.includes('/') && !decoded.includes('\\')
  }
  catch {
    return false
  }
}

function canUseWindowsPairingLoopbackFallback(url: URL, method: string): boolean {
  if (
    typeof window === 'undefined'
    || typeof window.__TAURI_INTERNALS__?.invoke !== 'function'
    || typeof navigator === 'undefined'
    || !navigator.platform.toUpperCase().startsWith('WIN')
    || method !== 'GET'
    || url.origin !== 'http://127.0.0.1:8080'
    || url.search !== ''
    || url.hash !== ''
  ) {
    return false
  }

  return url.pathname === '/api/android/pairing/status'
    || url.pathname === '/api/mobile/pairing/status'
    || hasOnePairingSessionSegment(url.pathname)
}

function isBrokerUnavailable(error: unknown): boolean {
  return error === 'bridge_auth_broker_unavailable'
    || (error instanceof Error && error.message === 'bridge_auth_broker_unavailable')
}

export function normalizeBridgeUrlForFetch(input: string | URL): URL {
  const url = input instanceof URL ? new URL(input.href) : new URL(input)
  if (
    url.protocol !== 'http:'
    || !LOOPBACK_HOSTS.has(url.hostname)
    || url.port !== '8080'
    || url.username !== ''
    || url.password !== ''
  ) {
    throw new Error('bridge_fetch_requires_loopback_8080')
  }
  return url
}

/**
 * Authenticated fetch for the native desktop renderer.
 *
 * The bearer is minted through Tauri IPC, lasts only 20 seconds, and is bound
 * to this exact method and URL path. It is never reused for another request or
 * forwarded to a non-loopback origin.
 */
export async function bridgeFetch(input: string | URL, init: RequestInit = {}): Promise<Response> {
  const url = normalizeBridgeUrlForFetch(input)
  const method = (init.method || 'GET').trim().toUpperCase()
  let token: string
  try {
    token = await invoke<string>('get_bridge_desktop_token', {
      method,
      path: url.pathname,
    })
  }
  catch (error) {
    if (!isBrokerUnavailable(error) || !canUseWindowsPairingLoopbackFallback(url, method))
      throw error

    const headers = new Headers(init.headers)
    headers.delete('Authorization')
    return await fetch(url, {
      ...init,
      method,
      headers,
      credentials: 'omit',
    })
  }
  if (!token)
    throw new Error('bridge_desktop_token_unavailable')

  const headers = new Headers(init.headers)
  headers.set('Authorization', `Bearer ${token}`)
  return await fetch(url, {
    ...init,
    method,
    headers,
    credentials: 'omit',
  })
}
