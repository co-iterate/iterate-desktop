<script setup lang="ts">
import type { McpRequest } from '../../types/popup'
import { computed, onMounted } from 'vue'
import { useShortcuts } from '../../composables/useShortcuts'

interface Props {
  request: McpRequest | null
  loading?: boolean
  submitting?: boolean
  canSubmit?: boolean
  connectionStatus?: string
  continueReplyEnabled?: boolean
  inputStatusText?: string
}

interface Emits {
  submit: []
  continue: []
  goalSubmit: []
}

const props = withDefaults(defineProps<Props>(), {
  loading: false,
  submitting: false,
  canSubmit: false,
  connectionStatus: '状态待确认',
  continueReplyEnabled: true,
  inputStatusText: '',
})

const emit = defineEmits<Emits>()

// 使用自定义快捷键系统
const {
  quickSubmitShortcutText,
  enhanceShortcutText,
  continueShortcutText,
  useQuickSubmitShortcut,
  useEnhanceShortcut,
  useContinueShortcut,
  loadShortcutConfig,
} = useShortcuts()

const shortcutText = quickSubmitShortcutText
const goalShortcut = computed(() => enhanceShortcutText.value.replace('增强', '目标'))

const statusText = computed(() => {
  // 如果可以提交，直接显示快捷键提示
  if (props.canSubmit) {
    return shortcutText.value
  }

  // 如果有输入状态文本且不是默认状态，显示输入状态
  if (props.inputStatusText && props.inputStatusText !== '等待输入...') {
    return props.inputStatusText
  }

  // 根据请求类型显示不同的提示
  if (props.request?.predefined_options) {
    return '选择选项或输入文本'
  }
  return '请输入内容'
})

// 处理快捷键
useQuickSubmitShortcut(() => {
  if (props.canSubmit && !props.submitting) {
    handleSubmit()
  }
})

useEnhanceShortcut(() => {
  if (props.canSubmit && !props.submitting) {
    handleGoalSubmit()
  }
})

useContinueShortcut(() => {
  if (!props.submitting) {
    handleContinue()
  }
})

function handleSubmit() {
  if (props.canSubmit && !props.submitting) {
    emit('submit')
  }
}

function handleContinue() {
  if (!props.submitting) {
    emit('continue')
  }
}

function handleGoalSubmit() {
  if (props.canSubmit && !props.submitting) {
    emit('goalSubmit')
  }
}

// 组件挂载时加载快捷键配置
onMounted(() => {
  loadShortcutConfig()
})
</script>

<template>
  <div class="px-4 py-3 bg-black-100 min-h-[60px] select-none">
    <div v-if="!loading" class="flex justify-between items-center gap-3 flex-wrap">
      <!-- 左侧状态信息 -->
      <div class="flex items-center min-w-0 flex-1">
        <div class="flex items-center gap-2 text-xs text-gray-600 min-w-0">
          <div v-if="connectionStatus === '等待调用方接收'" class="popup-status-dot w-2 h-2 rounded-full" />
          <span class="font-medium">{{ connectionStatus }}</span>
          <span class="opacity-60">|</span>
          <span class="opacity-60 truncate">{{ statusText }}</span>
        </div>
      </div>

      <!-- 右侧操作按钮 -->
      <div class="flex items-center ml-auto" data-guide="popup-actions">
        <div class="flex items-center gap-2 flex-wrap justify-end">
          <!-- 目标按钮：启动 GoalRun 目标模式 -->
          <n-tooltip trigger="hover" placement="top">
            <template #trigger>
              <n-button
                class="popup-footer-btn popup-footer-btn--goal-submit"
                :disabled="!canSubmit || submitting"
                size="medium"
                type="info"
                data-guide="goal-submit-button"
                @click="handleGoalSubmit"
              >
                <template #icon>
                  <div class="i-carbon-target w-4 h-4" />
                </template>
                目标
              </n-button>
            </template>
            {{ goalShortcut }}
          </n-tooltip>

          <!-- 继续按钮 -->
          <n-tooltip v-if="continueReplyEnabled" trigger="hover" placement="top">
            <template #trigger>
              <n-button
                class="popup-footer-btn popup-footer-btn--continue"
                :disabled="submitting"
                :loading="submitting"
                size="medium"
                type="default"
                data-guide="continue-button"
                @click="handleContinue"
              >
                <template #icon>
                  <div class="i-carbon-play w-4 h-4" />
                </template>
                继续
              </n-button>
            </template>
            {{ continueShortcutText }}
          </n-tooltip>

          <!-- 发送按钮 -->
          <n-tooltip trigger="hover" placement="top">
            <template #trigger>
              <n-button
                class="popup-footer-btn popup-footer-btn--submit"
                type="primary"
                :disabled="!canSubmit || submitting"
                :loading="submitting"
                size="medium"
                data-guide="submit-button"
                @click="handleSubmit"
              >
                <template #icon>
                  <div v-if="!submitting" class="i-carbon-send w-4 h-4" />
                </template>
                {{ submitting ? '发送中...' : '发送' }}
              </n-button>
            </template>
            {{ shortcutText }}
          </n-tooltip>
        </div>
      </div>
    </div>
  </div>
</template>
