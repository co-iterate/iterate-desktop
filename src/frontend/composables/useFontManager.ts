import { invoke } from '@tauri-apps/api/core'
import { computed, onScopeDispose, ref, watch } from 'vue'

export interface FontInfo {
  font_family: string
  font_size: string
  custom_font_family: string
}

export interface FontFamilyOption {
  id: string
  name: string
  css_value: string
}

export interface FontSizeOption {
  id: string
  name: string
  scale: number
}

/**
 * 字体管理组合式函数
 */
export function useFontManager() {
  const refreshSyncedFont = () => { void loadFontConfig() }
  window.addEventListener('iterate:settings-synced', refreshSyncedFont)
  onScopeDispose(() => window.removeEventListener('iterate:settings-synced', refreshSyncedFont))
  // 响应式状态
  const fontConfig = ref<FontInfo>({
    font_family: 'inter',
    font_size: 'medium',
    custom_font_family: 'Inter, -apple-system, BlinkMacSystemFont, \'Segoe UI\', Roboto, sans-serif',
  })

  const fontFamilyOptions = ref<FontFamilyOption[]>([])
  const fontSizeOptions = ref<FontSizeOption[]>([])

  // 计算当前字体CSS值
  const currentFontFamily = computed(() => {
    if (fontConfig.value.font_family === 'custom') {
      return fontConfig.value.custom_font_family
    }

    const option = fontFamilyOptions.value.find(opt => opt.id === fontConfig.value.font_family)
    return option?.css_value || fontFamilyOptions.value[0]?.css_value || 'Inter, sans-serif'
  })

  // 计算当前字体大小比例
  const currentFontScale = computed(() => {
    const option = fontSizeOptions.value.find(opt => opt.id === fontConfig.value.font_size)
    return option?.scale || 1.0
  })

  /**
   * 应用字体变量到DOM
   */
  function applyFontVariables() {
    const root = document.documentElement

    // 设置字体系列CSS变量
    root.style.setProperty('--font-family', currentFontFamily.value)

    // 设置字体大小比例CSS变量
    root.style.setProperty('--font-size-scale', currentFontScale.value.toString())

    // 应用字体大小到各个尺寸级别
    const baseSizes = [
      { name: 'xs', base: 0.75 },
      { name: 'sm', base: 0.875 },
      { name: 'base', base: 0.875 },
      { name: 'lg', base: 1.0 },
      { name: 'xl', base: 1.125 },
      { name: '2xl', base: 1.25 },
      { name: '3xl', base: 1.5 },
    ]

    baseSizes.forEach(({ name, base }) => {
      const scaledSize = base * currentFontScale.value
      root.style.setProperty(`--font-size-${name}`, `${scaledSize}rem`)
    })

    // 设置body字体
    root.style.setProperty('--body-font-family', currentFontFamily.value)

    // 强制更新body元素的字体
    document.body.style.fontFamily = currentFontFamily.value
  }

  /**
   * 加载字体配置
   */
  async function loadFontConfig() {
    try {
      const config = await invoke<FontInfo>('get_font_config')
      fontConfig.value = config
      applyFontVariables()
    }
    catch (error) {
      console.error('加载字体配置失败:', error)
    }
  }

  /**
   * 加载字体选项
   */
  async function loadFontOptions() {
    try {
      const [familyOptions, sizeOptions] = await Promise.all([
        invoke<FontFamilyOption[]>('get_font_family_options'),
        invoke<FontSizeOption[]>('get_font_size_options'),
      ])

      fontFamilyOptions.value = familyOptions
      fontSizeOptions.value = sizeOptions
    }
    catch (error) {
      console.error('加载字体选项失败:', error)
    }
  }

  /**
   * 设置字体系列
   */
  async function setFontFamily(fontFamily: string) {
    try {
      await invoke('set_font_family', { fontFamily })
      fontConfig.value.font_family = fontFamily
      applyFontVariables()
    }
    catch (error) {
      console.error('设置字体系列失败:', error)
      throw error
    }
  }

  /**
   * 设置字体大小
   */
  async function setFontSize(fontSize: string) {
    try {
      await invoke('set_font_size', { fontSize })
      fontConfig.value.font_size = fontSize
      applyFontVariables()
    }
    catch (error) {
      console.error('设置字体大小失败:', error)
      throw error
    }
  }

  /**
   * 设置自定义字体系列
   */
  async function setCustomFontFamily(customFontFamily: string) {
    try {
      await invoke('set_custom_font_family', { customFontFamily })
      fontConfig.value.custom_font_family = customFontFamily
      applyFontVariables()
    }
    catch (error) {
      console.error('设置自定义字体系列失败:', error)
      throw error
    }
  }

  /**
   * 重置字体配置
   */
  async function resetFontConfig() {
    try {
      await invoke('reset_font_config')
      await loadFontConfig()
    }
    catch (error) {
      console.error('重置字体配置失败:', error)
      throw error
    }
  }

  // 监听字体配置变化，自动应用
  watch([currentFontFamily, currentFontScale], () => {
    applyFontVariables()
  }, { immediate: true })

  // 监听字体配置对象变化
  watch(fontConfig, () => {
    applyFontVariables()
  }, { deep: true })

  return {
    // 状态
    fontConfig,
    fontFamilyOptions,
    fontSizeOptions,
    currentFontFamily,
    currentFontScale,

    // 方法
    loadFontConfig,
    loadFontOptions,
    setFontFamily,
    setFontSize,
    setCustomFontFamily,
    resetFontConfig,
    applyFontVariables,
  }
}
