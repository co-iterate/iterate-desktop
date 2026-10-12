import type { ShortcutBinding, ShortcutConfig, ShortcutKey } from '../types/popup'
import { invoke } from '@tauri-apps/api/core'
import { computed, onScopeDispose, ref } from 'vue'

/**
 * 自定义快捷键管理
 */
export function useShortcuts() {
  const shortcutConfig = ref<ShortcutConfig>({
    shortcuts: {},
  })

  // 检测操作系统
  const isMac = computed(() => {
    if (typeof navigator !== 'undefined') {
      return navigator.platform.toUpperCase().includes('MAC')
    }
    return false
  })

  let compositionActive = false
  const handleCompositionStart = () => {
    compositionActive = true
  }
  const handleCompositionEnd = () => {
    compositionActive = false
  }

  if (typeof window !== 'undefined') {
    window.addEventListener('compositionstart', handleCompositionStart)
    window.addEventListener('compositionend', handleCompositionEnd)
    onScopeDispose(() => {
      window.removeEventListener('compositionstart', handleCompositionStart)
      window.removeEventListener('compositionend', handleCompositionEnd)
    })
  }

  // 加载快捷键配置
  async function loadShortcutConfig() {
    try {
      const config = await invoke<ShortcutConfig>('get_shortcut_config')
      shortcutConfig.value = config
    }
    catch (error) {
      console.error('加载快捷键配置失败:', error)
    }
  }

  // 保存快捷键配置
  async function saveShortcutBinding(shortcutId: string, binding: ShortcutBinding) {
    try {
      await invoke('update_shortcut_binding', {
        shortcutId,
        binding,
      })
      shortcutConfig.value.shortcuts[shortcutId] = binding
    }
    catch (error) {
      console.error('保存快捷键配置失败:', error)
      throw error
    }
  }

  // 重置快捷键为默认值
  async function resetShortcutsToDefault() {
    try {
      await invoke('reset_shortcuts_to_default')
      await loadShortcutConfig()
    }
    catch (error) {
      console.error('重置快捷键失败:', error)
      throw error
    }
  }

  // 将快捷键组合转换为字符串表示
  function shortcutKeyToString(shortcutKey: ShortcutKey): string {
    const parts: string[] = []

    if (isMac.value) {
      if (shortcutKey.meta)
        parts.push('⌘')
      if (shortcutKey.ctrl)
        parts.push('⌃')
      if (shortcutKey.alt)
        parts.push('⌥')
      if (shortcutKey.shift)
        parts.push('⇧')
    }
    else {
      if (shortcutKey.ctrl)
        parts.push('Ctrl')
      if (shortcutKey.alt)
        parts.push('Alt')
      if (shortcutKey.shift)
        parts.push('Shift')
      if (shortcutKey.meta)
        parts.push('Meta')
    }

    parts.push(shortcutKey.key)
    return parts.join(isMac.value ? '' : '+')
  }

  // 将快捷键组合转换为useMagicKeys格式
  function shortcutKeyToMagicKey(shortcutKey: ShortcutKey): string {
    const parts: string[] = []

    if (shortcutKey.ctrl)
      parts.push('Ctrl')
    if (shortcutKey.alt)
      parts.push('Alt')
    if (shortcutKey.shift)
      parts.push('Shift')
    if (shortcutKey.meta)
      parts.push('Meta')

    parts.push(shortcutKey.key)
    return parts.join('+')
  }

  // 检查快捷键是否冲突（全局唯一，不区分作用域）
  function checkShortcutConflict(newBinding: ShortcutBinding, excludeId?: string): string | null {
    const newKeyStr = shortcutKeyToMagicKey(newBinding.key_combination)

    for (const [id, binding] of Object.entries(shortcutConfig.value.shortcuts)) {
      if (id === excludeId)
        continue

      const existingKeyStr = shortcutKeyToMagicKey(binding.key_combination)
      if (existingKeyStr === newKeyStr) {
        return binding.name
      }
    }

    return null
  }

  // 获取指定动作的快捷键
  function getShortcutByAction(action: string): ShortcutBinding | null {
    for (const binding of Object.values(shortcutConfig.value.shortcuts)) {
      if (binding.action === action) {
        return binding
      }
    }
    return null
  }

  function shortcutMatchesEvent(event: KeyboardEvent, shortcutKey: ShortcutKey): boolean {
    return event.key.toLowerCase() === shortcutKey.key.toLowerCase()
      && event.ctrlKey === shortcutKey.ctrl
      && event.altKey === shortcutKey.alt
      && event.shiftKey === shortcutKey.shift
      && event.metaKey === shortcutKey.meta
  }

  function fallbackShortcutKey(action: 'submit' | 'enhance' | 'continue'): ShortcutKey {
    if (isMac.value) {
      if (action === 'submit')
        return { key: 'Enter', ctrl: false, alt: false, shift: false, meta: true }
      if (action === 'enhance')
        return { key: 'Enter', ctrl: false, alt: true, shift: false, meta: false }
      return { key: 'Enter', ctrl: false, alt: false, shift: true, meta: false }
    }

    if (action === 'submit')
      return { key: 'Enter', ctrl: false, alt: false, shift: true, meta: false }
    if (action === 'enhance')
      return { key: 'Enter', ctrl: true, alt: false, shift: true, meta: false }
    return { key: 'Enter', ctrl: true, alt: false, shift: false, meta: false }
  }

  function isActiveShortcutContext(): boolean {
    return typeof document === 'undefined'
      || (document.visibilityState !== 'hidden' && document.hasFocus())
  }

  function useShortcutKeydown(action: string, callback: () => void) {
    const handleKeydown = (event: KeyboardEvent) => {
      const binding = getShortcutByAction(action)
      if (
        !binding?.enabled
        || compositionActive
        || event.isComposing
        || event.keyCode === 229
        || !isActiveShortcutContext()
        || !shortcutMatchesEvent(event, binding.key_combination)
      ) {
        return
      }

      // Consume the native key event before it reaches WebKit/AppKit. Watching
      // useMagicKeys state can trigger the callback, but cannot prevent the
      // same Cmd+Enter from falling through to macOS and producing a beep.
      event.preventDefault()
      event.stopPropagation()

      if (event.repeat) {
        return
      }

      callback()
    }

    window.addEventListener('keydown', handleKeydown, true)
    onScopeDispose(() => window.removeEventListener('keydown', handleKeydown, true))
  }

  // 获取快速发送快捷键的显示文本
  const quickSubmitShortcutText = computed(() => {
    const binding = getShortcutByAction('submit')
    const key = binding?.key_combination ?? fallbackShortcutKey('submit')
    return `${shortcutKeyToString(key)} ${binding?.name ?? '快速发送'}`
  })

  // 获取增强快捷键的显示文本
  const enhanceShortcutText = computed(() => {
    const binding = getShortcutByAction('enhance')
    const key = binding?.key_combination ?? fallbackShortcutKey('enhance')
    return `${shortcutKeyToString(key)} 增强`
  })

  // 获取继续快捷键的显示文本
  const continueShortcutText = computed(() => {
    const binding = getShortcutByAction('continue')
    const key = binding?.key_combination ?? fallbackShortcutKey('continue')
    return `${shortcutKeyToString(key)} ${binding?.name ?? '继续'}`
  })

  // 监听快速发送快捷键
  function useQuickSubmitShortcut(callback: () => void) {
    useShortcutKeydown('submit', callback)
  }

  // 监听增强快捷键
  function useEnhanceShortcut(callback: () => void) {
    useShortcutKeydown('enhance', callback)
  }

  // 监听继续快捷键
  function useContinueShortcut(callback: () => void) {
    useShortcutKeydown('continue', callback)
  }

  return {
    shortcutConfig,
    isMac,
    loadShortcutConfig,
    saveShortcutBinding,
    resetShortcutsToDefault,
    shortcutKeyToString,
    shortcutKeyToMagicKey,
    checkShortcutConflict,
    getShortcutByAction,
    quickSubmitShortcutText,
    enhanceShortcutText,
    continueShortcutText,
    useQuickSubmitShortcut,
    useEnhanceShortcut,
    useContinueShortcut,
  }
}
