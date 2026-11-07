// 导入 Naive UI 组件 (复用主应用的配置)
import {
  create,
  NAlert,
  NButton,
  NCard,
  NCheckbox,
  NCollapse,
  NCollapseItem,
  NCollapseTransition,
  NConfigProvider,
  NDialogProvider,
  NEmpty,
  NFormItem,
  NGrid,
  NGridItem,
  NImage,
  NImageGroup,
  NInput,
  NInputGroup,
  NInputNumber,
  NMessageProvider,
  NModal,
  NNotificationProvider,
  NSelect,
  NSkeleton,
  NSpace,
  NSpin,
  NSwitch,
  NTab,
  NTabPane,
  NTabs,
  NTag,
  NTooltip,
} from 'naive-ui'
import { createApp } from 'vue'

// 导入主题
import { useTheme } from '../composables/useTheme'

import { installMockTauri } from './mockTauri'
import TestApp from './TestApp.vue'
// 导入样式
import 'virtual:uno.css'

import '../assets/styles/style.css'

// 创建 Naive UI 实例
const naive = create({
  components: [
    NButton,
    NAlert,
    NCard,
    NCheckbox,
    NCollapse,
    NCollapseItem,
    NCollapseTransition,
    NConfigProvider,
    NEmpty,
    NFormItem,
    NGrid,
    NGridItem,
    NImage,
    NImageGroup,
    NInput,
    NInputGroup,
    NInputNumber,
    NModal,
    NSelect,
    NSkeleton,
    NSpace,
    NSpin,
    NSwitch,
    NTab,
    NTabPane,
    NTabs,
    NTag,
    NTooltip,
    NMessageProvider,
    NNotificationProvider,
    NDialogProvider,
  ],
})

// 模拟 Tauri API 用于测试环境
installMockTauri()

// 创建 Vue 应用
// ?popupCompact=0|1 用于对比简洁弹窗开关的两种状态
const popupCompactParam = new URLSearchParams(window.location.search).get('popupCompact')
if (popupCompactParam === '0' || popupCompactParam === '1') {
  try {
    localStorage.setItem('iterate:popup:compact-mode', popupCompactParam === '1' ? 'true' : 'false')
  }
  catch {}
}

const app = createApp(TestApp)

// 使用 Naive UI
app.use(naive)

// 挂载应用
app.mount('#app')

// 初始化主题
const { loadTheme } = useTheme()
loadTheme().catch(() => {
  console.log('使用默认主题')
})
