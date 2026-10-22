import { ref } from 'vue'

/**
 * 简洁弹窗：开启后，弹窗里的「快捷模板」和「上下文追加」收进一行状态条，点开才展开。
 * 只影响显示，不影响上下文追加的注入，也不影响 ⌥1-9 快捷模板。
 */
export const POPUP_COMPACT_MODE_STORAGE_KEY = 'iterate:popup:compact-mode'
const POPUP_COMPACT_MODE_EVENT_NAME = 'iterate:popup-compact-mode-changed'

const popupCompactMode = ref(true)
let listenerReady = false

function readStoredCompactMode(): boolean {
  try {
    return localStorage.getItem(POPUP_COMPACT_MODE_STORAGE_KEY) !== 'false'
  }
  catch {
    return true
  }
}

function setupListener() {
  if (listenerReady || typeof window === 'undefined')
    return

  listenerReady = true
  popupCompactMode.value = readStoredCompactMode()
  // 设置窗口和弹窗窗口共享 localStorage，用 storage 事件跨窗口同步
  window.addEventListener('storage', (event) => {
    if (event.key === POPUP_COMPACT_MODE_STORAGE_KEY)
      popupCompactMode.value = readStoredCompactMode()
  })
  window.addEventListener(POPUP_COMPACT_MODE_EVENT_NAME, () => {
    popupCompactMode.value = readStoredCompactMode()
  })
}

function setPopupCompactMode(enabled: boolean) {
  popupCompactMode.value = enabled
  if (typeof window === 'undefined')
    return

  try {
    localStorage.setItem(POPUP_COMPACT_MODE_STORAGE_KEY, String(enabled))
  }
  catch {}
  window.dispatchEvent(new CustomEvent(POPUP_COMPACT_MODE_EVENT_NAME))
}

export function usePopupCompactMode() {
  setupListener()
  return {
    popupCompactMode,
    setPopupCompactMode,
  }
}
