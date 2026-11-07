// 「AI 连接」这条线（首启接入卡片、设置 → AI 客户端）共用的纯函数；和连接 iPhone 的向导无关。
// 一键接入走 Rust 的 get_mcp_client_overview / connect_mcp_clients；
// 列表外的客户端给「复制核心系统提示词」「复制 MCP 配置」，由用户贴进自己的客户端。

export const MCP_SERVER_NAME = 'iterate-zhi'

export const RULES_FILE_HINT = '贴到你客户端的系统提示词文件（如 CLAUDE.md / AGENTS.md / .cursorrules）'

export type RulesState = 'installed' | 'manual' | 'missing'

/** 一键接入 Claude Code 时加进 ~/.claude/settings.json → permissions.allow 的条目，只放行 call_zhi。 */
export const CLAUDE_ZHI_PERMISSION = `mcp__${MCP_SERVER_NAME}__call_zhi`

export type PermissionState = 'installed' | 'manual' | 'missing' | 'invalid'

export interface RulesAwareClient {
  points_to_current: boolean
  opted_out: boolean
  rules_path?: string | null
  rules_state?: RulesState | string | null
  permission_path?: string | null
  permission_state?: PermissionState | string | null
}

export interface ServerLocation {
  ok: boolean
  bundled_server: string | null
  stable_server: string
  stable_server_ready?: boolean
}

/** MCP 已指向当前 App，但系统提示词文件里还没有 iterate 交互协议：勾选后重新接入即可补上。 */
export function clientNeedsRules(client: RulesAwareClient) {
  return client.points_to_current && client.rules_state === 'missing'
}

/** MCP 已指向当前 App，但 Claude Code 首次调用 call_zhi 仍会弹权限确认：勾选后重新接入即可补上白名单。 */
export function clientNeedsPermission(client: RulesAwareClient) {
  return client.points_to_current && client.permission_state === 'missing'
}

/** 已接入但缺调用规则或缺免确认，都可以再勾选一次补齐。 */
export function clientNeedsReconnect(client: RulesAwareClient) {
  return clientNeedsRules(client) || clientNeedsPermission(client)
}

export function permissionStateText(state: RulesAwareClient['permission_state']) {
  switch (state) {
    case 'installed': return 'call_zhi 已免确认'
    case 'manual': return '已有你自己设置的 call_zhi 权限'
    case 'missing': return 'call_zhi 还没免确认，首次调用会弹确认'
    case 'invalid': return '文件不是合法 JSON，不会自动改'
    default: return ''
  }
}

export function rulesStateText(state: RulesAwareClient['rules_state']) {
  switch (state) {
    case 'installed': return '已写入调用规则'
    case 'manual': return '已有你手写的调用规则'
    case 'missing': return '还没写调用规则'
    default: return ''
  }
}

/** 手动配置用的 command：稳定入口已就绪就用它（App 升级、挪动后不用改），否则用 App 包内路径。 */
export function manualMcpCommand(location: ServerLocation | null | undefined) {
  if (!location?.ok)
    return null
  if (location.stable_server_ready)
    return location.stable_server
  return location.bundled_server
}

/** 通用的 mcpServers 写法，Cursor、Windsurf、Cline 等都认。 */
export function buildManualMcpConfig(command: string) {
  return JSON.stringify({ mcpServers: { [MCP_SERVER_NAME]: { command } } }, null, 2)
}
