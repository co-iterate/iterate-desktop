// 首次使用引导的数据层。组件只依赖 FirstRunGuideDriver：
// 正式环境用 createTauriGuideDriver，测试页用 createMockGuideDriver。
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export interface GuideClient {
  id: string
  name: string
  detected: boolean
  /** 配置已指向当前 iterate（不代表宿主已重启加载） */
  connected: boolean
  restartHint: string
}

export interface GuideConnectResult {
  id: string
  ok: boolean
  message: string
}

export interface GuideIncomingCall {
  clientName: string
  at: number
}

export interface FirstRunGuideDriver {
  listClients: () => Promise<GuideClient[]>
  connectClients: (ids: string[]) => Promise<GuideConnectResult[]>
  /** 订阅引导开始后 iterate 收到的第一个 zhi 调用；返回取消函数 */
  watchIncomingCall: (onCall: (call: GuideIncomingCall) => void) => () => void
}

const RESTART_HINTS: Record<string, string> = {
  'claude-code': '新开一个 Claude Code 会话后生效',
  'codex': '完全退出 Codex 再打开后生效',
  'cursor': '重启 Cursor 后生效',
}

const CALLER_NAMES: Record<string, string> = {
  claude_code: 'Claude Code',
  codex: 'Codex',
  cursor: 'Cursor',
}

interface ClientOverviewPayload {
  clients: Array<{ id: string, name: string, detected: boolean, points_to_current: boolean }>
}

/** 正式环境：复用 client_install 的命令，并监听广播给所有窗口的 mcp-request */
export function createTauriGuideDriver(): FirstRunGuideDriver {
  return {
    async listClients() {
      const overview = await invoke<ClientOverviewPayload>('get_mcp_client_overview')
      return overview.clients.map(client => ({
        id: client.id,
        name: client.name,
        detected: client.detected,
        connected: client.points_to_current,
        restartHint: RESTART_HINTS[client.id] ?? '重启该客户端后生效',
      }))
    },
    async connectClients(ids) {
      const results = await invoke<GuideConnectResult[]>('connect_mcp_clients', { clients: ids })
      return results.map(result => ({ id: result.id, ok: result.ok, message: result.message }))
    },
    watchIncomingCall(onCall) {
      let disposed = false
      let unlisten: (() => void) | null = null
      void listen<{ caller_client?: string }>('mcp-request', (event) => {
        const caller = event.payload?.caller_client ?? ''
        onCall({ clientName: CALLER_NAMES[caller] ?? 'AI 客户端', at: Date.now() })
      }).then((stop) => {
        if (disposed)
          stop()
        else
          unlisten = stop
      })
      return () => {
        disposed = true
        unlisten?.()
      }
    },
  }
}

// 引导进度只存在本机；读写失败时按「未完成」处理，不影响正常使用
const GUIDE_STORAGE_KEY = 'iterate.firstRunGuide.v1'
export type GuideOutcome = 'done' | 'skipped' | 'existing-user'

export function readGuideOutcome(): GuideOutcome | null {
  try {
    const value = localStorage.getItem(GUIDE_STORAGE_KEY)
    return value === 'done' || value === 'skipped' || value === 'existing-user' ? value : null
  }
  catch {
    return null
  }
}

export function writeGuideOutcome(outcome: GuideOutcome) {
  try {
    localStorage.setItem(GUIDE_STORAGE_KEY, outcome)
  }
  catch {
    // 存不下时下次启动会再判断一次是否已有客户端接入
  }
}

/** 设置页「重新查看使用引导」通过这个窗口事件唤起 */
export const OPEN_GUIDE_EVENT = 'iterate:open-first-run-guide'
/** 引导结束页「查看手机连接条件」通知 AppContent 打开已有的手机连接向导 */
export const OPEN_MOBILE_PAIRING_EVENT = 'iterate:open-mobile-pairing'

export type GuideStepState = 'locked' | 'active' | 'done'

/** 给用户照抄到 AI 对话里的测试指令 */
export const FIRST_CALL_PROMPT
  = '请调用 iterate 的 zhi 工具，message 写「iterate 接入测试」，然后等待我的回复，并把我的回复原样告诉我。'

export function stepStates(doneFlags: boolean[]): GuideStepState[] {
  const firstOpen = doneFlags.findIndex(done => !done)
  return doneFlags.map((done, index) => {
    if (done)
      return 'done'
    return index === firstOpen ? 'active' : 'locked'
  })
}

export interface MockGuideScenario {
  /** 没有检测到任何受支持客户端 */
  noClients?: boolean
  /** 接入后多久模拟收到 AI 调用（毫秒）；null 表示永远不来 */
  callDelayMs?: number | null
}

export function createMockGuideDriver(scenario: MockGuideScenario = {}): FirstRunGuideDriver {
  const clients: GuideClient[] = scenario.noClients
    ? []
    : [
        { id: 'claude-code', name: 'Claude Code', detected: true, connected: false, restartHint: '新开一个 Claude Code 会话后生效' },
        { id: 'codex', name: 'Codex', detected: true, connected: false, restartHint: '完全退出 Codex 再打开后生效' },
        { id: 'cursor', name: 'Cursor', detected: false, connected: false, restartHint: '重启 Cursor 后生效' },
      ]
  let connectedName: string | null = null
  let pendingCall: ((call: GuideIncomingCall) => void) | null = null
  let callTimer: ReturnType<typeof setTimeout> | null = null

  function scheduleCall() {
    const delay = scenario.callDelayMs === undefined ? 2500 : scenario.callDelayMs
    if (delay === null || !pendingCall || !connectedName || callTimer)
      return
    callTimer = setTimeout(() => {
      pendingCall?.({ clientName: connectedName!, at: Date.now() })
    }, delay)
  }

  return {
    async listClients() {
      await wait(300)
      return clients.map(client => ({ ...client }))
    },
    async connectClients(ids) {
      await wait(700)
      return ids.map((id) => {
        const client = clients.find(item => item.id === id)
        if (!client)
          return { id, ok: false, message: '未找到该客户端' }
        client.connected = true
        connectedName ??= client.name
        return { id, ok: true, message: `已写入配置，${client.restartHint}` }
      })
    },
    watchIncomingCall(onCall) {
      pendingCall = onCall
      scheduleCall()
      return () => {
        pendingCall = null
        if (callTimer)
          clearTimeout(callTimer)
        callTimer = null
      }
    },
  }
}

function wait(ms: number) {
  return new Promise(resolve => setTimeout(resolve, ms))
}
