// 连接 iPhone 的最后一步：从电脑发一条测试消息，收到手机回执才算真正连通。
// 复用已有的手机动作（show_message）和回执接口，不新增后端。

export const PHONE_TEST_MESSAGE_TEXT = 'iterate 测试：这台 Mac 已经能把消息送到你的 iPhone 了。'

export type PhoneTestOutcome
  = | { kind: 'delivered', at: string }
    | { kind: 'waiting_for_foreground' }
    | { kind: 'no_phone' }
    | { kind: 'failed', message: string }
    | { kind: 'timeout' }

export interface PhoneTestDeps {
  publish: (request: Record<string, unknown>) => Promise<{ ok: boolean, id: string }>
  fetchResult: (id: string) => Promise<{ status: string, message?: string | null, received_at?: string } | null>
  sleep: (ms: number) => Promise<void>
  now: () => number
}

const DONE = new Set(['success', 'ok', 'completed'])
const FAILED = new Set(['failed', 'failure', 'error', 'expired', 'cancelled'])

/**
 * 发送并等待回执。手机在后台时回执是 waiting_for_foreground：先回报一次，
 * 让界面提示「在 iPhone 上打开 iterate Remote」，然后继续等到超时。
 */
export async function sendPhoneTestMessage(
  deps: PhoneTestDeps,
  options: { deviceId?: string, timeoutMs?: number, intervalMs?: number, onWaitingForForeground?: () => void } = {},
): Promise<PhoneTestOutcome> {
  const timeoutMs = options.timeoutMs ?? 60_000
  const intervalMs = options.intervalMs ?? 500
  const request: Record<string, unknown> = {
    id: `desktop-pairing-test-${deps.now().toString(36)}`,
    action: 'show_message',
    text: PHONE_TEST_MESSAGE_TEXT,
    source: 'desktop_pairing_guide',
  }
  if (options.deviceId)
    request.targetDeviceId = options.deviceId

  let published: { ok: boolean, id: string }
  try {
    published = await deps.publish(request)
  }
  catch (error) {
    return { kind: 'failed', message: String(error instanceof Error ? error.message : error) }
  }
  if (!published.ok)
    return { kind: 'no_phone' }

  const deadline = deps.now() + timeoutMs
  let toldForeground = false
  while (deps.now() < deadline) {
    const result = await deps.fetchResult(published.id).catch(() => null)
    const status = result?.status?.toLowerCase() ?? ''
    if (DONE.has(status))
      return { kind: 'delivered', at: result?.received_at ?? '' }
    if (FAILED.has(status))
      return { kind: 'failed', message: result?.message || status }
    if (status === 'waiting_for_foreground' && !toldForeground) {
      toldForeground = true
      options.onWaitingForForeground?.()
    }
    await deps.sleep(intervalMs)
  }
  return toldForeground ? { kind: 'waiting_for_foreground' } : { kind: 'timeout' }
}
