// 真实 App 原型页：先装测试假后端，再原样启动正式入口 ../main.ts（正式的 App.vue）。
// 这样页面上每个按钮都是正式版的按钮；正式版改了，这里自动跟着变。
// 后端命令由 mockTauri.ts 假装返回，没覆盖到的命令会出现在右下角的“未模拟命令”面板里。
import { installMockTauri } from './mockTauri'

const params = new URLSearchParams(window.location.search)

// ?fresh=1：模拟刚下载的新用户（清掉本地缓存的引导进度、主题等）
if (params.get('fresh') === '1') {
  try {
    localStorage.clear()
  }
  catch {}
}

// ?guide=skip：跳过首次引导，直接看主界面 / 弹窗
if (params.get('guide') === 'skip') {
  try {
    localStorage.setItem('iterate.firstRunGuide.v1', 'skipped')
  }
  catch {}
}

installMockTauri()

// ---------- 原型专用：事件系统 + 常用命令的合理默认值 ----------
type Invoke = (command: string, args?: Record<string, any>, options?: unknown) => Promise<unknown>
const internals = (window as any).__TAURI_INTERNALS__
const baseInvoke: Invoke = internals.invoke

// Tauri v2 的 listen/emit 依赖 transformCallback；测试 mock 里它是空实现，事件永远收不到。
const callbacks = new Map<number, (payload: unknown) => void>()
const listeners = new Map<string, Map<number, number>>() // event -> (eventId -> callbackId)
let nextId = 1
internals.transformCallback = (cb: (payload: unknown) => void) => {
  const id = nextId++
  callbacks.set(id, cb)
  return id
}
internals.unregisterCallback = (id: number) => callbacks.delete(id)

// @tauri-apps/api 的 unlisten 会调用这个对象
;(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: (event: string, eventId: number) => {
    listeners.get(event)?.delete(eventId)
  },
}

export function emitMock(event: string, payload: unknown) {
  for (const [eventId, cbId] of listeners.get(event) || [])
    callbacks.get(cbId)?.({ event, id: eventId, payload })
}
;(window as any).__iterateEmit = emitMock

const state = {
  clients: [
    { id: 'claude-code', name: 'Claude Code', detected: true, points_to_current: false, rules_path: '~/.claude/CLAUDE.md', rules_state: 'missing' },
    { id: 'codex', name: 'Codex', detected: true, points_to_current: false, rules_path: '~/.codex/AGENTS.md', rules_state: 'missing' },
    { id: 'cursor', name: 'Cursor', detected: false, points_to_current: false, rules_path: null, rules_state: null },
  ],
  lastResponse: '' as string,
}

const overrides: Record<string, (args: Record<string, any>) => unknown> = {
  'plugin:event|listen': ({ event, handler }) => {
    const eventId = nextId++
    if (!listeners.has(event))
      listeners.set(event, new Map())
    listeners.get(event)!.set(eventId, handler)
    return eventId
  },
  'plugin:event|unlisten': ({ event, eventId }) => {
    listeners.get(event)?.delete(eventId)
    return null
  },
  'plugin:event|emit': ({ event, payload }) => {
    emitMock(event, payload)
    return null
  },
  'get_theme': () => params.get('theme') || 'light',
  'get_cli_args': () => ({ standalone_mode: false }),
  'requires_activation_gate': () => false,
  'is_licensed': () => true,
  'get_trial_status': () => ({
    is_active: true,
    is_expired: false,
    days_remaining: 7,
    trial_days: 7,
    days_used: 0,
    first_launch_at: new Date().toISOString(),
    expires_at: new Date(Date.now() + 7 * 86400e3).toISOString(),
    contact_url: '',
    expired_message: '',
    expired_subtitle: '',
  }),
  'get_app_info': () => 'iterate v0.7.2',
  'get_mcp_client_overview': () => ({ clients: state.clients.map(c => ({ ...c })) }),
  'connect_mcp_clients': ({ clients }) => (clients as string[]).map((id) => {
    const client = state.clients.find(c => c.id === id)
    if (client) {
      client.points_to_current = true
      if (client.rules_path)
        client.rules_state = 'installed'
    }
    return { id, ok: Boolean(client), message: client ? '已写入配置（原型模拟）' : '未找到客户端' }
  }),
  'get_font_config': () => ({ font_family: 'system', font_size: 'medium', custom_font_family: '' }),
  'get_font_family_options': () => [
    { id: 'system', name: '系统默认', css_value: '-apple-system, BlinkMacSystemFont, "PingFang SC", sans-serif' },
  ],
  'get_font_size_options': () => [
    { id: 'small', name: '小', scale: 0.9 },
    { id: 'medium', name: '中', scale: 1 },
    { id: 'large', name: '大', scale: 1.1 },
  ],
  'get_window_constraints_cmd': () => null,
  'get_window_settings': () => ({ current_width: 600, current_height: 900, fixed: false }),
  'get_window_config': () => ({ popup_placement: 'center' }),
  'get_always_on_top': () => false,
  'get_audio_notification_enabled': () => false,
  'get_audio_url': () => '',
  'get_default_window_registration_label': () => 'main',
  'get_speech_muscle_memory_entries': () => [],
  'get_speech_correction_memory_entries': () => [],
  'get_speech_vocabulary_entries': () => [],
  'send_mcp_response': ({ response }) => {
    state.lastResponse = typeof response === 'string' ? response : JSON.stringify(response)
    window.dispatchEvent(new CustomEvent('iterate-mock-response', { detail: state.lastResponse }))
    return null
  },
}

const silentPrefixes = ['plugin:window|', 'plugin:webview|', 'plugin:app|', 'set_', 'debug_log', 'mark_', 'sync_', 'register_']

const protoInvoke: Invoke = async (command, args = {}, options) => {
  const override = overrides[command]
  if (override)
    return override(args)
  if (silentPrefixes.some(prefix => command.startsWith(prefix)))
    return null
  const missesBefore = window.__iterateMockMisses?.length ?? 0
  const result = await baseInvoke(command, args, options)
  // 基础 mock 对没覆盖的命令一律返回 {}，正式组件拿到 {} 常会类型错乱（.map / .find 报错）。
  // 原型页按命令名猜一个更安全的默认值；命令仍会列在“未模拟命令”面板里。
  if ((window.__iterateMockMisses?.length ?? 0) > missesBefore)
    return guessDefault(command)
  return result
}

function guessDefault(command: string): unknown {
  if (/(?:_entries|_instances|_list|_providers|_path|_history|_files|_items)$/.test(command) || command.startsWith('list_'))
    return []
  if (/(?:_enabled|_status_bool)$/.test(command) || command.startsWith('is_') || command.startsWith('has_'))
    return false
  if (command.endsWith('_pid'))
    return 1
  if (command.startsWith('create_') || command.startsWith('ensure_'))
    return `proto-${command}-${Date.now()}`
  // 猜不出时沿用基础 mock 的 {}：有些启动路径会直接读字段，返回 null 反而会让整个 App 启动失败
  return {}
}
internals.invoke = protoInvoke
;(window as any).__TAURI__.core.invoke = protoInvoke

function simulateZhiCall() {
  const id = `proto-${Date.now()}`
  emitMock('mcp-request', {
    id,
    request_id: id,
    message: '## 原型模拟：AI 叫你一次\n\n这是在浏览器原型里模拟的一次 `zhi` 调用。你在这里回复后，回复内容会显示在右下角面板。\n\n::::artifact{type="html" title="demo-preview.html" description="原型里的 HTML 卡片，用来检查浅色/深色下的颜色"}\n<h1 style="font-family:sans-serif">Hello iterate</h1>\n::::',
    predefined_options: ['收到，接入成功', '先别改'],
    is_markdown: true,
    project_path: '/Users/example/project',
    caller_client: 'claude_code',
  })
}

function mountMissPanel() {
  const panel = document.createElement('div')
  panel.id = 'mock-miss-panel'
  panel.innerHTML = `
    <style>
      #mock-miss-panel{position:fixed;right:12px;bottom:120px;z-index:2147483647;font:12px/1.5 -apple-system,sans-serif;color:#111;max-width:320px}
      #mock-miss-panel summary{cursor:pointer;background:#fff;border:1px solid #d4d4d8;border-radius:999px;padding:4px 12px;box-shadow:0 2px 8px rgba(0,0,0,.12);list-style:none}
      #mock-miss-panel[data-count="0"] summary{opacity:.55}
      #mock-miss-panel ul{margin:6px 0 0;padding:8px 12px 8px 24px;background:#fff;border:1px solid #d4d4d8;border-radius:10px;max-height:260px;overflow:auto;box-shadow:0 6px 20px rgba(0,0,0,.15)}
      #mock-miss-panel code{font-size:11px}
    </style>
    <div style="display:flex;flex-direction:column;align-items:flex-end;gap:6px">
      <button data-zhi style="cursor:pointer;background:#111;color:#fff;border:1px solid rgba(255,255,255,.5);border-radius:999px;padding:6px 14px;font:inherit;box-shadow:0 2px 8px rgba(0,0,0,.2)">模拟 AI 调用一次 zhi</button>
      <div data-reply hidden style="background:#ecfdf5;border:1px solid #6ee7b7;border-radius:10px;padding:6px 10px;max-width:320px;white-space:pre-wrap"></div>
      <details>
        <summary>原型模式 · 未模拟命令 <b data-n>0</b></summary>
        <ul data-list><li>暂无。点到没有假数据的功能时，这里会列出命令名。</li></ul>
      </details>
    </div>`
  document.body.appendChild(panel)
  panel.querySelector('[data-zhi]')!.addEventListener('click', simulateZhiCall)
  const reply = panel.querySelector<HTMLElement>('[data-reply]')!
  window.addEventListener('iterate-mock-response', (event) => {
    reply.hidden = false
    reply.textContent = `AI 收到你的回复：\n${(event as CustomEvent).detail}`.slice(0, 400)
  })
  const n = panel.querySelector('[data-n]')!
  const list = panel.querySelector('[data-list]')!
  const render = () => {
    const counts = new Map<string, number>()
    for (const miss of window.__iterateMockMisses || [])
      counts.set(miss.command, (counts.get(miss.command) || 0) + 1)
    panel.dataset.count = String(counts.size)
    n.textContent = String(counts.size)
    if (counts.size)
      list.innerHTML = [...counts].map(([c, k]) => `<li><code>${c}</code> ×${k}</li>`).join('')
  }
  window.addEventListener('iterate-mock-miss', render)
  render()
}

if (document.readyState === 'loading')
  document.addEventListener('DOMContentLoaded', mountMissPanel)
else
  mountMissPanel()

void import('../main')
