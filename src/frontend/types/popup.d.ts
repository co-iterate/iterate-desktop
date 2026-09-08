// 弹窗系统类型定义

export type { PopupTextSelection, PopupTextSelectionSource } from '../utils/popupSelectionQuote'

export interface McpRequest {
  id: string
  message: string
  predefined_options?: string[]
  is_markdown?: boolean
  render_mode?: 'text' | 'markdown' | 'html_artifact'
  artifacts?: PopupArtifact[]
  project_path?: string
  codex_home?: string
  codex_thread_id?: string
  codex_deeplink?: string
  conversation_title?: string
  checkpoint_id?: string
  checkpoint_commit?: string
  checkpoint_message?: string
  loop_active?: boolean
  force_popup?: boolean
  link_url?: string
  link_title?: string
  browser_ai_response?: string
}

export interface PopupArtifact {
  type: 'html'
  title: string
  description?: string
  content?: string
  path?: string
}

// 自定义prompt类型定义
export interface CustomPrompt {
  id: string
  name: string
  content: string
  description?: string
  sort_order: number
  created_at: string
  updated_at: string
  type: 'normal' | 'conditional'
  // 条件性prompt专用字段
  condition_text?: string // 条件描述文本
  template_true?: string // 开关为true时的模板
  template_false?: string // 开关为false时的模板
  current_state?: boolean // 当前开关状态
  is_active?: boolean // 是否启用该项（即是否参与追加）
}

// 自定义prompt配置
export interface CustomPromptConfig {
  prompts: CustomPrompt[]
  enabled: boolean
  maxPrompts: number
}

export interface PopupState {
  visible: boolean
  loading: boolean
  submitting: boolean
  currentTheme: string
  request: McpRequest | null
}

// 快捷键相关类型定义
export interface ShortcutConfig {
  shortcuts: Record<string, ShortcutBinding>
}

export interface ShortcutBinding {
  id: string
  name: string
  description: string
  action: string // "submit", "exit", "custom"
  key_combination: ShortcutKey
  enabled: boolean
  scope: string // "global", "popup", "input"
}

export interface ShortcutKey {
  key: string // 主键，如 "Enter", "Q", "F4"
  ctrl: boolean
  alt: boolean
  shift: boolean
  meta: boolean // macOS的Cmd键
}

// 新的结构化响应格式
export interface McpResponse {
  user_input: string | null
  selected_options: string[]
  images: ImageAttachment[]
  file_paths: string[]
  image_paths: string[]
  metadata: ResponseMetadata
}

export interface PopupFileAttachment {
  path: string
  name: string
}

export interface PopupInputData {
  userInput?: string
  rawUserInput?: string
  selectedOptions?: string[]
  draggedImages?: string[]
  attachedFiles?: PopupFileAttachment[]
}

export interface ImageAttachment {
  data: string
  media_type: string
  filename: string | null
}

export interface ResponseMetadata {
  timestamp: string | null
  request_id: string | null
  source: string | null
  hui_snapshot?: string | null
  run_id?: string | null
  generation?: number | null
  stale_of?: string | null
  superseded_by?: string | null
  is_stale?: boolean | null
}

// 旧格式兼容性支持
export interface PopupResponse {
  type: 'text' | 'image' | 'option'
  text?: string
  options?: string[]
  images?: ImageData[]
}

export interface ImageData {
  type: 'image'
  source: {
    type: 'base64'
    media_type: string
    data: string
  }
}

// 事件类型
export type PopupEvent
  = | { type: 'show', payload: McpRequest }
    | { type: 'hide' }
    | { type: 'submit', payload: PopupResponse }
    | { type: 'cancel' }
    | { type: 'theme-change', payload: string }
    | { type: 'open-main-layout' }
