<script setup lang="ts">
import type { McpRequest } from '../../types/popup'
import { computed, onMounted } from 'vue'
import { useShortcuts } from '../../composables/useShortcuts'
import { callerBadgeFor } from './callerBadge'

interface Props {
  request: McpRequest | null
  loading?: boolean
  submitting?: boolean
  canSubmit?: boolean
  connectionStatus?: string
  continueReplyEnabled?: boolean
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
  connectionStatus: '已连接',
  continueReplyEnabled: true,
})

const emit = defineEmits<Emits>()

const callerBadge = computed(() => callerBadgeFor(
  props.request?.caller_client,
  props.request?.caller_model_label,
  props.request?.caller_model,
))

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
          <div class="popup-status-dot w-2 h-2 rounded-full" />
          <span class="font-medium">{{ connectionStatus }}</span>
          <span
            v-if="callerBadge"
            class="flex items-center gap-1 flex-shrink-0"
            :title="callerBadge.title"
          >
            <span class="opacity-60">·</span>
            <span
              v-if="callerBadge.pixels"
              class="agy-pixel-logo"
              aria-hidden="true"
            >
              <template v-for="(row, rowIndex) in callerBadge.pixels" :key="rowIndex">
                <i
                  v-for="(color, colIndex) in row"
                  :key="colIndex"
                  :style="color ? { background: color } : undefined"
                />
              </template>
            </span>
            <span
              v-else
              class="inline-grid place-items-center w-4 h-4 rounded"
              :style="{ background: callerBadge.tint, boxShadow: callerBadge.tint === '#ffffff' ? 'inset 0 0 0 1px rgba(15, 23, 42, 0.14)' : undefined }"
            >
              <img :src="callerBadge.icon" alt="" class="w-2.5 h-2.5">
            </span>
            <span class="font-medium">{{ callerBadge.text }}</span>
          </span>
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

<style scoped>
/* agy 像素 A：6 列 × 5 行，每格 3px，和终端里的启动图同形同色 */
.agy-pixel-logo {
  display: inline-grid;
  grid-template-columns: repeat(6, 3px);
  grid-auto-rows: 3px;
  flex-shrink: 0;
}

.agy-pixel-logo i {
  display: block;
}
</style>
