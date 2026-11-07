import { invoke } from '@tauri-apps/api/core'
import { computed, ref } from 'vue'
import { applyThemeVariables, getTheme } from '../theme'

type ThemeName = 'light' | 'dark'

// 新用户默认浅色；已选过深色的用户由本地缓存 + 后端配置还原
const DEFAULT_THEME: ThemeName = 'light'
// 仅用于启动瞬间同步上色，避免深色用户在读取配置前闪一下白；以后端配置为准
const THEME_CACHE_KEY = 'iterate.theme.last'

function normalizeTheme(value: unknown): ThemeName {
  return value === 'dark' || value === 'light' ? value : DEFAULT_THEME
}

function readCachedTheme(): ThemeName {
  try {
    return normalizeTheme(localStorage.getItem(THEME_CACHE_KEY))
  }
  catch {
    return DEFAULT_THEME
  }
}

function writeCachedTheme(theme: ThemeName) {
  try {
    localStorage.setItem(THEME_CACHE_KEY, theme)
  }
  catch {
    // 存不下只影响启动瞬间的颜色，不影响最终主题
  }
}

export function useTheme() {
  // 先同步应用上次的主题（首次打开为浅色），再异步读取后端配置校正
  const initialTheme = readCachedTheme()
  applyThemeVariables(initialTheme)
  const currentTheme = ref<string>(initialTheme)

  // 计算 Naive UI 主题
  const naiveTheme = computed(() => {
    const theme = currentTheme.value || DEFAULT_THEME
    return getTheme(theme)
  })

  // 应用主题
  function applyTheme(theme: string) {
    const validTheme = normalizeTheme(theme)
    applyThemeVariables(validTheme)
    currentTheme.value = validTheme
    writeCachedTheme(validTheme)
  }

  // 切换主题
  async function setTheme(theme: string) {
    try {
      // 先保存到后端
      await invoke('set_theme', { theme })
      // 保存成功后应用前端主题
      applyTheme(theme)
    }
    catch (error) {
      console.error('保存主题设置失败:', error)
      // 保存失败时不应用主题，保持一致性
    }
  }

  // 加载主题设置
  async function loadTheme() {
    try {
      const theme = await invoke('get_theme')
      // 应用后端主题（无论是否与当前主题相同，确保状态同步）
      applyTheme(normalizeTheme(theme))
    }
    catch (error) {
      console.error('加载主题失败:', error)
      // 加载失败时保持启动时已应用的主题
      applyTheme(initialTheme)
    }
  }

  // 立即尝试加载主题，避免延迟
  void loadTheme()

  return {
    currentTheme,
    naiveTheme,
    setTheme,
    loadTheme,
  }
}
