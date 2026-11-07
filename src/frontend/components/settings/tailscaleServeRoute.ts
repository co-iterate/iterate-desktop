// Tailscale Serve 路线：用户自己把 Bridge 挂到 tailnet 内的 https://<机器名>.ts.net，
// iterate 只检测和登记。与 src/rust/tunnel/tailscale_serve.rs 的字段一一对应。

export const TAILSCALE_SERVE_TRANSPORT = 'tailscale_serve'
export const TAILSCALE_DNS_ADMIN_URL = 'https://login.tailscale.com/admin/dns'
export const TAILSCALE_IOS_APP_URL = 'https://apps.apple.com/app/tailscale/id1470499037'

export interface TailscaleServeStatus {
  cli_available: boolean
  running: boolean
  backend_state?: string | null
  ios_peer_online?: boolean
  dns_name: string | null
  https_cert_available: boolean
  serve_proxies_bridge: boolean
  funnel_enabled: boolean
  base_url: string | null
  setup_command: string | null
  blocker: string | null
}

export interface TailscaleServeCheck {
  label: string
  ok: boolean
}

export function tailscaleServeChecklist(status: TailscaleServeStatus): TailscaleServeCheck[] {
  return [
    { label: 'Tailscale 已安装并登录', ok: status.cli_available && status.running },
    { label: 'MagicDNS 与 HTTPS 证书已开启', ok: Boolean(status.dns_name) && status.https_cert_available },
    { label: 'Funnel 已关闭（不暴露到公网）', ok: status.running && !status.funnel_enabled },
    { label: 'serve 已指向本机 iterate（8080）', ok: status.serve_proxies_bridge },
  ]
}

const BLOCKER_TEXT: Record<string, string> = {
  tailscale_not_installed: '这台 Mac 还没安装 Tailscale。',
  tailscale_not_running: 'Tailscale 没有运行或还没登录，请在菜单栏连接后再检测。',
  tailscale_magicdns_disabled: 'tailnet 没有开启 MagicDNS，请在 Tailscale 后台的 DNS 页面开启。',
  tailscale_https_disabled: 'tailnet 没有开启 HTTPS 证书，请在 Tailscale 后台的 DNS 页面点 Enable。',
  tailscale_funnel_enabled: 'Funnel 正开着，iterate 会暴露到公网。请先运行 tailscale funnel reset 再检测。',
  tailscale_serve_not_configured: '还没把 iterate 挂到 tailnet 上，请在终端运行下面这条命令。',
  bridge_unhealthy: '本机 iterate Bridge 没有就绪，请稍后重试。',
  endpoint_identity_mismatch: '能访问这个地址，但它不是当前这台 Mac 上的 iterate。请确认 serve 指向 8080。',
  formal_route_lock_unavailable: '连接设置正在被修改，请稍后重试。',
}

export function tailscaleServeBlockerText(code: string | null | undefined) {
  if (!code)
    return ''
  return BLOCKER_TEXT[code] || `Tailscale 路线暂时无法启用（${code}）。`
}

export const TAILSCALE_MAC_DOWNLOAD_URL = 'https://tailscale.com/download/mac'

export type TailscaleGuideView = 'install' | 'login' | 'switches' | 'funnel' | 'serve' | 'phone' | 'ready'

/**
 * Tailscale 引导现在该停在哪一步：完全由实际检测结果决定，不靠用户点「下一步」。
 * step 是 0..3 的步骤条序号，4 表示全部完成。
 */
export function tailscaleGuideState(status: TailscaleServeStatus): { step: number, view: TailscaleGuideView } {
  if (!status.cli_available)
    return { step: 0, view: 'install' }
  if (!status.running)
    return { step: 0, view: 'login' }
  if (!status.dns_name || !status.https_cert_available)
    return { step: 1, view: 'switches' }
  if (status.funnel_enabled)
    return { step: 2, view: 'funnel' }
  if (!status.serve_proxies_bridge)
    return { step: 2, view: 'serve' }
  if (!status.ios_peer_online)
    return { step: 3, view: 'phone' }
  return { step: 4, view: 'ready' }
}
