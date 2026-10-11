<script setup lang="ts">
import { useMessage } from 'naive-ui'
import { computed } from 'vue'
import { useVersionCheck } from '../../composables/useVersionCheck'
import { functionalColors } from '../../theme/colors'

interface Props {
  show: boolean
  versionInfo: {
    current: string
    latest: string
    hasUpdate: boolean
    releaseUrl: string
    releaseNotes: string
  } | null
}

const props = defineProps<Props>()
const emit = defineEmits<{
  'update:show': [value: boolean]
}>()

const message = useMessage()
const {
  isUpdating,
  updateStatus,
  updateProgress,
  performOneClickUpdate,
  restartApp,
  dismissUpdate,
} = useVersionCheck()

// 简单的文本格式化（将换行转换为HTML）
const formattedReleaseNotes = computed(() => {
  if (!props.versionInfo?.releaseNotes)
    return ''
  return props.versionInfo.releaseNotes
    .replace(/\n/g, '<br>')
    .replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*(.*?)\*/g, '<em>$1</em>')
    .replace(/`(.*?)`/g, '<code>$1</code>')
})

const isVisible = computed({
  get: () => props.show,
  set: value => emit('update:show', value),
})

// 确认更新
async function handleConfirmUpdate() {
  try {
    message.info('正在准备更新...')
    await performOneClickUpdate()

    if (updateStatus.value === 'completed') {
      message.success('更新完成！')
    }
  }
  catch (error) {
    const errorMsg = error instanceof Error ? error.message : String(error)
    console.error('❌ 更新失败:', errorMsg)

    if (errorMsg.includes('已下载并打开 DMG') || errorMsg.includes('手动覆盖安装') || errorMsg.includes('自动替换应用内部二进制')) {
      message.success(errorMsg)
      setTimeout(() => {
        isVisible.value = false
      }, 2000)
      return
    }

    // 如果是需要手动下载的错误，引导用户手动下载
    if (errorMsg.includes('手动下载') || errorMsg.includes('网络请求受限') || errorMsg.includes('403')) {
      let warningMsg = '自动更新不可用，将为您打开下载页面'

      if (errorMsg.includes('网络请求受限') || errorMsg.includes('403')) {
        warningMsg = '网络请求受限，将为您打开下载页面'
      }

      message.warning(warningMsg)

      // 打开下载页面
      if (props.versionInfo?.releaseUrl) {
        try {
          window.open(props.versionInfo.releaseUrl, '_blank')
        }
        catch (openError) {
          console.error('❌ 打开下载页面失败:', openError)
          message.error('无法打开下载页面，请手动访问 GitHub 下载最新版本')
        }
      }
      else {
        message.error('无法获取下载链接，请手动访问 GitHub 下载最新版本')
      }

      // 延迟关闭弹窗，让用户看到提示
      setTimeout(() => {
        isVisible.value = false
      }, 2000)
    }
    else {
      // 其他错误显示具体错误信息
      let displayMsg = errorMsg || '更新失败，请稍后重试'

      // 检查是否是网络相关错误
      if (errorMsg.includes('网络') || errorMsg.includes('连接') || errorMsg.includes('请求失败')
        || errorMsg.includes('timeout') || errorMsg.includes('ENOTFOUND') || errorMsg.includes('ECONNREFUSED')) {
        displayMsg = '网络连接异常，请检查网络后重试'
      }

      message.error(`更新失败: ${displayMsg}`)
    }
  }
}

// 关闭弹窗（不再提醒）
function handleDismiss() {
  dismissUpdate()
  message.info('已关闭更新提醒')
}

// 重启应用
async function handleRestart() {
  try {
    await restartApp()
  }
  catch (error) {
    console.error('重启失败:', error)
    message.error('重启失败，请手动重启应用')
  }
}
</script>

<template>
  <n-modal
    v-model:show="isVisible"
    :mask-closable="false"
    :close-on-esc="false"
    preset="dialog"
    class="max-w-lg"
    :style="{ maxHeight: '80vh' }"
  >
    <template #header>
      <div class="flex items-center gap-3">
        <div class="i-carbon-upgrade text-xl text-blue-500" />
        <span class="font-medium text-lg">🚀 发现新版本</span>
      </div>
    </template>

    <div class="space-y-4">
      <!-- 版本信息 -->
      <div v-if="versionInfo" class="space-y-3">
        <div class="p-4 bg-surface-100 dark:bg-surface-800 rounded-lg border border-surface-200 dark:border-surface-700">
          <div class="flex items-center justify-between mb-3">
            <span class="text-sm text-on-surface-secondary">当前版本:</span>
            <n-tag size="small" type="info">
              v{{ versionInfo.current }}
            </n-tag>
          </div>
          <div class="flex items-center justify-between">
            <span class="text-sm text-on-surface-secondary">最新版本:</span>
            <n-tag size="small" type="success">
              v{{ versionInfo.latest }}
            </n-tag>
          </div>
        </div>

        <!-- 更新进度 -->
        <div v-if="isUpdating" class="p-4 bg-blue-50 dark:bg-blue-900/30 rounded-lg border border-blue-200 dark:border-blue-700">
          <div class="space-y-3">
            <div class="flex items-center gap-2">
              <n-spin size="small" />
              <span class="text-sm font-medium text-on-surface dark:text-on-surface">
                {{ updateStatus === 'checking' ? '检查更新中...'
                  : updateStatus === 'downloading' ? '下载更新中...'
                    : updateStatus === 'installing' ? '安装更新中...'
                      : updateStatus === 'completed' ? '更新完成！'
                        : '更新中...' }}
              </span>
            </div>

            <!-- 下载进度条 -->
            <div v-if="updateProgress && updateStatus === 'downloading'" class="space-y-2">
              <n-progress
                type="line"
                :percentage="Math.round(updateProgress.percentage)"
                :show-indicator="false"
                :height="8"
                :color="functionalColors.info"
              />
              <div class="flex justify-between text-xs text-on-surface-secondary dark:text-on-surface-secondary">
                <span>{{ Math.round(updateProgress.downloaded / 1024 / 1024 * 100) / 100 }}MB</span>
                <span v-if="updateProgress.content_length">
                  / {{ Math.round(updateProgress.content_length / 1024 / 1024 * 100) / 100 }}MB
                </span>
                <span>{{ Math.round(updateProgress.percentage) }}%</span>
              </div>
            </div>
          </div>
        </div>

        <!-- 更新说明 -->
        <div v-if="versionInfo.releaseNotes && !isUpdating" class="space-y-3">
          <div class="flex items-center gap-2">
            <div class="i-carbon-document text-blue-500" />
            <h4 class="text-sm font-medium text-on-surface">
              更新内容
            </h4>
          </div>
          <div class="max-h-40 overflow-y-auto">
            <div class="text-sm p-4 rounded-lg border bg-surface-50 dark:bg-surface-900 border-surface-200 dark:border-surface-700 text-on-surface-secondary">
              <div
                class="release-notes-content space-y-2"
                v-html="formattedReleaseNotes"
              />
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #action>
      <div class="flex justify-end gap-3">
        <!-- 关闭按钮 -->
        <n-button
          v-if="updateStatus !== 'completed'"
          :disabled="isUpdating"
          @click="handleDismiss"
        >
          关闭
        </n-button>

        <!-- 立即更新按钮 -->
        <n-button
          v-if="updateStatus !== 'completed'"
          type="primary"
          :loading="isUpdating"
          @click="handleConfirmUpdate"
        >
          <template #icon>
            <div class="i-carbon-upgrade" />
          </template>
          立即更新
        </n-button>

        <!-- 重启按钮 -->
        <n-button
          v-if="updateStatus === 'completed'"
          type="success"
          @click="handleRestart"
        >
          <template #icon>
            <div class="i-carbon-restart" />
          </template>
          重启应用
        </n-button>
      </div>
    </template>
  </n-modal>
</template>

<style scoped>
.release-notes-content :deep(h1),
.release-notes-content :deep(h2),
.release-notes-content :deep(h3),
.release-notes-content :deep(h4) {
  font-weight: 600;
  margin: 0.75rem 0 0.5rem 0;
  color: var(--text-color-1);
}

.release-notes-content :deep(h2) {
  font-size: 1.1em;
  border-bottom: 1px solid var(--border-color);
  padding-bottom: 0.25rem;
}

.release-notes-content :deep(h3) {
  font-size: 1em;
}

.release-notes-content :deep(p) {
  margin: 0.5rem 0;
  line-height: 1.5;
}

.release-notes-content :deep(ul),
.release-notes-content :deep(ol) {
  margin: 0.5rem 0;
  padding-left: 1.5rem;
}

.release-notes-content :deep(li) {
  margin: 0.25rem 0;
  line-height: 1.4;
}

.release-notes-content :deep(strong) {
  font-weight: 600;
  color: var(--text-color-1);
}

.release-notes-content :deep(em) {
  font-style: italic;
}

.release-notes-content :deep(code) {
  padding: 0.125rem 0.375rem;
  font-size: 0.875em;
  border-radius: 0.25rem;
  font-family: ui-monospace, SFMono-Regular, 'SF Mono', monospace;
  background-color: var(--code-color);
  color: var(--text-color-1);
  border: 1px solid var(--border-color);
}

.release-notes-content :deep(blockquote) {
  margin: 0.75rem 0;
  padding: 0.5rem 1rem;
  border-left: 3px solid var(--primary-color);
  background-color: var(--code-color);
  border-radius: 0 0.25rem 0.25rem 0;
}
</style>
