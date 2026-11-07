// 「clauding · Opus 5.5」：哪个 AI、用哪个模型在调这次弹窗。读不到模型时只显示客户端。
// 桌面放在底部状态栏（顶栏太挤）；手机端由 iOS 自己在头部显示。
// 颜色按各家真实 App 图标核对（2026-10-29）：Claude 橙底白标；Codex 桌面 App 白底深色花形；
// Antigravity 白底彩色渐变 A（底部蓝、顶部绿→黄→红）。白底方块加细边，免得在浅色状态栏里看不出边缘。
export const CALLER_BADGES: Record<string, { verb: string, name: string, icon: string, tint: string }> = {
  claude_code: { verb: 'clauding', name: 'Claude Code', icon: '/icons/ai-providers/claude.svg', tint: '#d97757' },
  codex: { verb: 'codexing', name: 'Codex', icon: '/icons/ai-providers/codex-dark.svg', tint: '#ffffff' },
  antigravity: { verb: 'agying', name: 'Antigravity CLI', icon: '/icons/ai-providers/antigravity-color.svg', tint: '#ffffff' },
}

// agy 命令行启动时显示的像素 A（6×5），颜色取自真实终端截图；空字符串是透明格。
export const AGY_PIXEL_LOGO: string[][] = [
  ['', '', '#e59646', '#e07946', '', ''],
  ['', '#a6c259', '#d89c4f', '#e68146', '#de5f4c', ''],
  ['', '#8bc160', '#6fa78f', '#736fbd', '#9060a3', ''],
  ['', '#7ac084', '', '', '#577fe3', ''],
  ['#7bb7ef', '#77b4f0', '', '', '#4d81f1', '#5083f4'],
]

export interface CallerBadge {
  text: string
  title: string
  icon: string
  tint: string
  /** 有值时用代码画像素图，不用图片图标 */
  pixels?: string[][]
}

export function callerBadgeFor(client?: string, modelLabel?: string, model?: string): CallerBadge | null {
  const badge = client ? CALLER_BADGES[client] : undefined
  if (!badge)
    return null
  const label = modelLabel?.trim()
  return {
    text: label ? `${badge.verb} · ${label}` : badge.verb,
    title: [badge.name, model?.trim()].filter(Boolean).join(' · '),
    icon: badge.icon,
    tint: badge.tint,
    pixels: client === 'antigravity' ? AGY_PIXEL_LOGO : undefined,
  }
}
