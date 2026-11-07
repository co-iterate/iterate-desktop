// UI 常量配置
// 这些常量与后端配置保持同步，通过 get_window_constraints 命令动态获取

// 默认窗口约束（后备值）
export const DEFAULT_WINDOW_CONSTRAINTS = {
  min_width: 600,
  min_height: 400,
  max_width: 1500,
  max_height: 1000,
  resize_step: 50,
  resize_throttle_ms: 1000,
  size_update_delay_ms: 500,
  size_check_delay_ms: 100,
} as const

// 默认窗口尺寸
export const DEFAULT_WINDOW_SIZE = {
  width: 600,
  height: 900,
} as const

// 过渡动画常量
export const TRANSITION_DURATIONS = {
  quick: 200,
  normal: 300,
  smooth: 500,
  slow: 800,
} as const

// 过渡缓动函数
export const TRANSITION_EASINGS = {
  ease: 'ease',
  easeIn: 'ease-in',
  easeOut: 'ease-out',
  easeInOut: 'ease-in-out',
  cubic: 'cubic-bezier(0.25, 0.8, 0.25, 1)',
  bounce: 'cubic-bezier(0.34, 1.56, 0.64, 1)',
} as const

// 主题常量
export const THEMES = {
  light: 'light',
  dark: 'dark',
} as const

// 默认主题
export const DEFAULT_THEME = THEMES.dark

// 音频配置常量
export const AUDIO_CONFIG = {
  defaultUrl: '',
  defaultEnabled: false,
} as const

// 继续回复配置常量
export const REPLY_CONFIG = {
  defaultEnabled: true,
  defaultThreshold: 1000,
  defaultPrompt: '请按照最佳实践继续',
} as const

// MCP 工具常量
export const MCP_TOOLS = {
  zhi: 'zhi',
} as const

// 字体大小常量
export const FONT_SIZES = {
  'xs': '0.75rem',
  'sm': '0.875rem',
  'base': '0.875rem',
  'lg': '1rem',
  'xl': '1.125rem',
  '2xl': '1.25rem',
} as const

// 间距常量
export const SPACING = {
  'xs': '0.25rem',
  'sm': '0.5rem',
  'md': '1rem',
  'lg': '1.5rem',
  'xl': '2rem',
  '2xl': '3rem',
} as const

// 圆角常量
export const BORDER_RADIUS = {
  'none': '0',
  'sm': '0.125rem',
  'md': '0.375rem',
  'lg': '0.5rem',
  'xl': '0.75rem',
  '2xl': '1rem',
  'full': '9999px',
} as const

// 阴影常量
export const SHADOWS = {
  sm: '0 1px 2px 0 rgb(0 0 0 / 0.05)',
  md: '0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1)',
  lg: '0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)',
  xl: '0 20px 25px -5px rgb(0 0 0 / 0.1), 0 8px 10px -6px rgb(0 0 0 / 0.1)',
} as const

// Z-index 层级常量
export const Z_INDEX = {
  dropdown: 1000,
  sticky: 1020,
  fixed: 1030,
  modal: 1040,
  popover: 1050,
  tooltip: 1060,
  toast: 1070,
} as const

// 断点常量
export const BREAKPOINTS = {
  'sm': '640px',
  'md': '768px',
  'lg': '1024px',
  'xl': '1280px',
  '2xl': '1536px',
} as const

// 类型定义
export type WindowConstraints = typeof DEFAULT_WINDOW_CONSTRAINTS
export type WindowSize = typeof DEFAULT_WINDOW_SIZE
export type TransitionDuration = keyof typeof TRANSITION_DURATIONS
export type TransitionEasing = keyof typeof TRANSITION_EASINGS
export type Theme = keyof typeof THEMES
export type FontSize = keyof typeof FONT_SIZES
export type Spacing = keyof typeof SPACING
export type BorderRadius = keyof typeof BORDER_RADIUS
export type Shadow = keyof typeof SHADOWS
export type ZIndex = keyof typeof Z_INDEX
export type Breakpoint = keyof typeof BREAKPOINTS
