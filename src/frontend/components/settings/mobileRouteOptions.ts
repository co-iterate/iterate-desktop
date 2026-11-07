// 「管理连接 → 更换连接方式」的展示规则。与 tunnel/commands.rs 的 get_mobile_route_options 对应。
import type { FormalMobileRouteStatus } from './mobileConnectionMachine.ts'
import type { TailscaleServeStatus } from './tailscaleServeRoute.ts'

export interface KnownFormalMobileRoute {
  transport: string
  base_url: string
  last_verified_at?: string | null
}

export interface MobileRouteOptions {
  current: FormalMobileRouteStatus
  known: KnownFormalMobileRoute[]
  tailscale: TailscaleServeStatus
}

export function mobileRouteLabel(transport?: string | null) {
  switch (transport) {
    case 'tailscale_serve': return 'Tailscale'
    case 'cloudflare_named_tunnel': return 'Cloudflare'
    case 'aliyun_ssh_reverse_tunnel': return '阿里云 IP + SSH'
    case 'self_hosted_frp': return '自动连接'
    default: return '未配置'
  }
}

export function mobileRouteDescription(transport?: string | null) {
  switch (transport) {
    case 'tailscale_serve': return '只在你自己的设备之间传输，不经过第三方服务器。iPhone 需要打开 Tailscale。'
    case 'cloudflare_named_tunnel': return '经 Cloudflare 转发，手机不用装别的；晚高峰可能变慢。'
    case 'aliyun_ssh_reverse_tunnel': return '经你自己的阿里云服务器转发。'
    case 'self_hosted_frp': return '由 iterate 提供的托管通道。'
    default: return ''
  }
}

// 扫码前要提醒用户的事，没有就返回空串
export function mobileRouteScanHint(transport?: string | null) {
  return transport === 'tailscale_serve' ? '扫码前请先在 iPhone 上打开 Tailscale。' : ''
}

const SWITCH_ERROR_TEXT: Record<string, string> = {
  endpoint_identity_mismatch: '这条路线现在连不上这台 Mac，已保留当前连接方式。请先确认它还在运行。',
  bridge_unhealthy: '本机 iterate Bridge 没有就绪，请稍后重试。',
  formal_route_not_known: '这条路线还没有在这台 Mac 上验证过。',
  formal_route_lock_unavailable: '连接设置正在被修改，请稍后重试。',
  formal_route_changed: '连接设置刚被其他地方改过，请重新打开再试。',
}

export function mobileRouteSwitchErrorText(code: string, fallback: (code: string) => string) {
  return SWITCH_ERROR_TEXT[code] || fallback(code)
}
