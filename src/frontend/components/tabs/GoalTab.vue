<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, onUnmounted, ref } from 'vue'

interface LiveGoalSnapshot {
  id: string
  title: string
  status: string
  phase?: string | null
  status_text?: string | null
  progress_percent?: number | null
  progress_source?: string | null
  progress_label?: string | null
  plan_total?: number | null
  plan_completed?: number | null
  tokens_used?: number | null
  token_budget?: number | null
  time_used_seconds?: number | null
  started_at_ms: number
  updated_at_ms?: number | null
  completed_at_ms?: number | null
  project_path?: string | null
  request_id?: string | null
  codex_thread_id?: string | null
  codex_deeplink?: string | null
  run_id?: string | null
  generation?: number | null
  stale_of?: string | null
  superseded_by?: string | null
  last_codex_event_at_ms?: number | null
  source?: string | null
}

const message = useMessage()
const goalTitle = ref('')
const currentGoal = ref<LiveGoalSnapshot | null>(null)
const nowMs = ref(Date.now())
const isSubmitting = ref(false)
let clockTimer: number | null = null
let pollTimer: number | null = null

const activeGoal = computed(() => normalizeGoal(currentGoal.value))
const canStart = computed(() => goalTitle.value.trim().length > 0 && !isSubmitting.value)
const elapsedMs = computed(() => {
  if (!activeGoal.value)
    return 0
  const end = activeGoal.value.completed_at_ms ?? nowMs.value
  return Math.max(0, end - activeGoal.value.started_at_ms)
})

const elapsedLabel = computed(() => formatElapsed(elapsedMs.value))
const displayTitle = computed(() => activeGoal.value?.title || '等待设定目标')
const projectLabel = computed(() => projectName(activeGoal.value?.project_path))
const statusLabel = computed(() => {
  if (!activeGoal.value)
    return '未启动'
  switch (activeGoal.value.status) {
    case 'completed':
      return '已完成'
    case 'cleared':
    case 'cancelled':
    case 'canceled':
      return '已结束'
    default:
      return activeGoal.value.status_text || '进行中'
  }
})

const stageLabel = computed(() => {
  if (!activeGoal.value)
    return '等待目标'
  if (activeGoal.value.status === 'completed')
    return '完成归档'
  if (activeGoal.value.phase === 'waiting_for_user')
    return '等待输入'
  if (activeGoal.value.phase === 'waiting_for_approval')
    return '等待审批'
  if (activeGoal.value.phase)
    return activeGoal.value.status_text || activeGoal.value.phase
  return '实时执行'
})

const progressValue = computed(() => {
  if (!activeGoal.value)
    return 0
  if (activeGoal.value.status === 'completed')
    return 100
  const explicitProgress = normalizePercent(activeGoal.value.progress_percent)
  if (explicitProgress !== null)
    return explicitProgress

  const minutes = Math.floor(elapsedMs.value / 60000)
  return Math.min(88, 34 + minutes * 3)
})

const progressStyle = computed(() => ({
  width: `${progressValue.value}%`,
}))

const statusClass = computed(() => {
  if (!activeGoal.value)
    return 'is-idle'
  return activeGoal.value.status === 'completed' ? 'is-complete' : 'is-running'
})

async function loadGoal() {
  try {
    const nextGoal = await invoke<LiveGoalSnapshot | null>('get_live_goal')
    currentGoal.value = normalizeGoal(nextGoal)
  }
  catch (error) {
    console.error('读取 Live Goal 失败:', error)
  }
}

async function startGoal() {
  if (!canStart.value)
    return

  isSubmitting.value = true
  try {
    const nextGoal = await invoke<LiveGoalSnapshot>('start_live_goal', {
      title: goalTitle.value.trim(),
    })
    currentGoal.value = normalizeGoal(nextGoal)
    goalTitle.value = ''
    message.success('Goal 已同步')
  }
  catch (error) {
    console.error('启动 Live Goal 失败:', error)
    message.error(String(error || '启动 Live Goal 失败'))
  }
  finally {
    isSubmitting.value = false
  }
}

async function completeGoal() {
  isSubmitting.value = true
  try {
    const nextGoal = await invoke<LiveGoalSnapshot | null>('complete_live_goal')
    currentGoal.value = normalizeGoal(nextGoal)
    message.success('Goal 已完成')
  }
  catch (error) {
    console.error('完成 Live Goal 失败:', error)
    message.error(String(error || '完成 Live Goal 失败'))
  }
  finally {
    isSubmitting.value = false
  }
}

async function clearGoal() {
  isSubmitting.value = true
  try {
    await invoke('clear_live_goal')
    currentGoal.value = null
    message.success('Goal 已清除')
  }
  catch (error) {
    console.error('清除 Live Goal 失败:', error)
    message.error(String(error || '清除 Live Goal 失败'))
  }
  finally {
    isSubmitting.value = false
  }
}

function normalizeGoal(goal: LiveGoalSnapshot | null | undefined): LiveGoalSnapshot | null {
  if (!goal || typeof goal !== 'object')
    return null
  if (!goal.id || !goal.title || !Number.isFinite(goal.started_at_ms))
    return null

  return {
    ...goal,
    status: goal.status || 'running',
  }
}

function projectName(path: string | null | undefined) {
  const value = path?.trim()
  if (!value)
    return 'iterate'
  return value.split('/').filter(Boolean).at(-1) || 'iterate'
}

function formatElapsed(ms: number) {
  const totalSeconds = Math.floor(ms / 1000)
  const hours = Math.floor(totalSeconds / 3600)
  const minutes = Math.floor((totalSeconds % 3600) / 60)
  const seconds = totalSeconds % 60

  if (hours > 0)
    return `${hours}h ${minutes.toString().padStart(2, '0')}m`
  if (minutes > 0)
    return `${minutes}m ${seconds.toString().padStart(2, '0')}s`
  return `${seconds}s`
}

function normalizePercent(value: number | null | undefined) {
  if (value === null || value === undefined)
    return null
  const numericValue = Number(value)
  if (!Number.isFinite(numericValue))
    return null
  return Math.min(100, Math.max(0, numericValue))
}

onMounted(() => {
  void loadGoal()
  clockTimer = window.setInterval(() => {
    nowMs.value = Date.now()
  }, 1000)
  pollTimer = window.setInterval(() => {
    void loadGoal()
  }, 2000)
})

onUnmounted(() => {
  if (clockTimer !== null)
    window.clearInterval(clockTimer)
  if (pollTimer !== null)
    window.clearInterval(pollTimer)
})
</script>

<template>
  <div class="goal-tab">
    <section class="goal-panel">
      <div class="goal-panel__header">
        <div class="goal-brand">
          <img src="/icons/icon-128.png" alt="iterate" class="goal-brand__icon">
          <div>
            <div class="goal-brand__title">
              Live Goal
            </div>
            <div class="goal-brand__meta">
              {{ projectLabel }} · {{ stageLabel }}
            </div>
          </div>
        </div>

        <div class="goal-state" :class="statusClass">
          <span class="goal-state__dot" />
          {{ statusLabel }}
        </div>
      </div>

      <div class="goal-compose">
        <n-input
          v-model:value="goalTitle"
          size="medium"
          placeholder="设定一个当前目标"
          maxlength="80"
          clearable
          @keyup.enter="startGoal"
        />
        <n-button
          type="primary"
          :disabled="!canStart"
          :loading="isSubmitting"
          @click="startGoal"
        >
          <template #icon>
            <div class="i-carbon-play-filled-alt" />
          </template>
          启动
        </n-button>
      </div>

      <div class="goal-live-card">
        <div class="goal-live-card__icon">
          <img src="/icons/icon-128.png" alt="iterate">
        </div>
        <div class="goal-live-card__body">
          <div class="goal-live-card__top">
            <div class="goal-live-card__title">
              {{ displayTitle }}
            </div>
            <div class="goal-live-card__time">
              {{ elapsedLabel }}
            </div>
          </div>

          <div class="goal-progress" aria-hidden="true">
            <span :style="progressStyle" />
          </div>

          <div class="goal-live-card__bottom">
            <span>iterate</span>
            <span>{{ progressValue }}%</span>
            <span>{{ stageLabel }}</span>
          </div>
        </div>
      </div>

      <div class="goal-details">
        <div class="goal-detail">
          <span>状态</span>
          <strong>{{ statusLabel }}</strong>
        </div>
        <div class="goal-detail">
          <span>耗时</span>
          <strong>{{ elapsedLabel }}</strong>
        </div>
        <div class="goal-detail">
          <span>同步</span>
          <strong>{{ activeGoal ? '实时' : '等待' }}</strong>
        </div>
      </div>

      <div class="goal-actions">
        <n-button
          v-if="activeGoal && activeGoal.status !== 'completed'"
          size="small"
          secondary
          :loading="isSubmitting"
          @click="completeGoal"
        >
          <template #icon>
            <div class="i-carbon-checkmark" />
          </template>
          完成
        </n-button>
        <n-button
          size="small"
          tertiary
          :disabled="!activeGoal"
          :loading="isSubmitting"
          @click="clearGoal"
        >
          <template #icon>
            <div class="i-carbon-close" />
          </template>
          清除
        </n-button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.goal-tab {
  width: min(100%, 720px);
  margin: 0 auto;
}

.goal-panel {
  border: 1px solid var(--color-border, rgba(31, 35, 40, 0.12));
  border-radius: 8px;
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.78), rgba(255, 255, 255, 0.52)),
    var(--card-color, #fff);
  box-shadow: 0 16px 40px rgba(15, 23, 42, 0.08);
  padding: 16px;
}

.goal-panel__header,
.goal-compose,
.goal-live-card__top,
.goal-live-card__bottom,
.goal-actions {
  display: flex;
  align-items: center;
}

.goal-panel__header {
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 14px;
}

.goal-brand {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 10px;
}

.goal-brand__icon,
.goal-live-card__icon img {
  display: block;
  object-fit: cover;
}

.goal-brand__icon {
  width: 34px;
  height: 34px;
  border-radius: 8px;
}

.goal-brand__title {
  color: var(--text-color, #171717);
  font-size: 17px;
  font-weight: 650;
  line-height: 1.2;
}

.goal-brand__meta {
  max-width: 320px;
  overflow: hidden;
  color: var(--color-on-surface-secondary, rgba(23, 23, 23, 0.58));
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.goal-state {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 6px;
  border: 1px solid rgba(107, 114, 128, 0.2);
  border-radius: 999px;
  padding: 4px 9px;
  color: rgba(75, 85, 99, 0.9);
  font-size: 12px;
  font-weight: 650;
}

.goal-state__dot {
  width: 6px;
  height: 6px;
  border-radius: 999px;
  background: currentColor;
}

.goal-state.is-running {
  border-color: rgba(8, 145, 178, 0.22);
  color: #0891b2;
}

.goal-state.is-complete {
  border-color: rgba(22, 163, 74, 0.22);
  color: #16a34a;
}

.goal-compose {
  gap: 8px;
  margin-bottom: 14px;
}

.goal-live-card {
  display: grid;
  grid-template-columns: 40px minmax(0, 1fr);
  gap: 12px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 8px;
  background:
    radial-gradient(circle at 22% 16%, rgba(34, 211, 238, 0.2), transparent 26%),
    linear-gradient(135deg, #111827, #05070a 62%, #0b1220);
  padding: 13px;
  color: #fff;
}

.goal-live-card__icon {
  display: flex;
  width: 40px;
  height: 40px;
  align-items: center;
  justify-content: center;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.05);
}

.goal-live-card__icon img {
  width: 30px;
  height: 30px;
  border-radius: 7px;
}

.goal-live-card__body {
  min-width: 0;
}

.goal-live-card__top {
  gap: 10px;
  justify-content: space-between;
  margin-bottom: 9px;
}

.goal-live-card__title {
  min-width: 0;
  overflow: hidden;
  font-size: 16px;
  font-weight: 700;
  letter-spacing: 0;
  line-height: 1.25;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.goal-live-card__time {
  flex: 0 0 auto;
  color: rgba(255, 255, 255, 0.64);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}

.goal-progress {
  height: 6px;
  overflow: hidden;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.12);
}

.goal-progress span {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, #22d3ee, #a3e635);
  transition: width 0.25s ease;
}

.goal-live-card__bottom {
  justify-content: space-between;
  gap: 8px;
  margin-top: 8px;
  color: rgba(255, 255, 255, 0.62);
  font-size: 12px;
}

.goal-details {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px;
  margin-top: 12px;
}

.goal-detail {
  border: 1px solid var(--color-border, rgba(31, 35, 40, 0.1));
  border-radius: 8px;
  padding: 9px 10px;
}

.goal-detail span {
  display: block;
  color: var(--color-on-surface-secondary, rgba(23, 23, 23, 0.56));
  font-size: 12px;
}

.goal-detail strong {
  display: block;
  margin-top: 3px;
  overflow: hidden;
  color: var(--text-color, #171717);
  font-size: 14px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.goal-actions {
  justify-content: flex-end;
  gap: 8px;
  margin-top: 12px;
}

:root.dark .goal-panel {
  background:
    linear-gradient(180deg, rgba(31, 41, 55, 0.78), rgba(17, 24, 39, 0.62)),
    var(--card-color, #111827);
}

@media (max-width: 640px) {
  .goal-compose {
    align-items: stretch;
    flex-direction: column;
  }

  .goal-details {
    grid-template-columns: 1fr;
  }
}
</style>
