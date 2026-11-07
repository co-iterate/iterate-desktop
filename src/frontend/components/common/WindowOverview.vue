<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'

export interface WindowInstance {
  pid: number
  project_path: string
  window_title: string
  registered_at: string
  request_title?: string | null
  /** 消息全文（后端有上限），只用于搜索 */
  request_text?: string | null
}

interface OverviewItem {
  instance: WindowInstance
  title: string
  waitedMs: number
}

interface OverviewGroup {
  project: string
  path: string
  items: OverviewItem[]
}

interface QuotaProvider {
  id: string
  name: string
  metrics: Array<{ label: string, remaining: number, resetLabel?: string }>
}

const props = defineProps<{
  visible: boolean
  instances: WindowInstance[]
  currentPid: number
  quotaProviders?: QuotaProvider[]
  // 点击或快捷键打开时才抢焦点、接管方向键；悬停打开只展示，不打断正在输入的回复。
  interactive?: boolean
  anchor?: { top: number, left: number }
}>()

const emit = defineEmits<{
  close: []
  hoverChange: [hovering: boolean]
}>()

const LONG_WAIT_MS = 3 * 60 * 60 * 1000

const query = ref('')
const longWaitOnly = ref(false)
const selectedPid = ref(0)
const now = ref(Date.now())
const searchInput = ref<HTMLInputElement | null>(null)

function projectName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean)
  return parts[parts.length - 1] || path
}

// request_title 存的是消息正文开头，取第一行有内容的文字并去掉 Markdown 记号。
function headline(instance: WindowInstance): string {
  const source = instance.request_title || ''
  for (const raw of source.split('\n')) {
    const line = raw
      .replace(/^\s*(?:#{1,6}\s+|[-*+]\s+|>\s*|\d+\.\s+)/, '')
      .replace(/[*_`~]/g, '')
      .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
      .trim()
    if (line && !/^[-|:\s]+$/.test(line))
      return line.replace(/[:：]$/, '')
  }
  return ''
}

// 列表是单行菜单，等待时间用紧凑写法，如 7m、1h48m、2d3h。
function formatWait(ms: number): string {
  const minutes = Math.max(0, Math.floor(ms / 60000))
  if (minutes < 1)
    return '刚刚'
  if (minutes < 60)
    return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24)
    return minutes % 60 ? `${hours}h${minutes % 60}m` : `${hours}h`
  return hours % 24 ? `${Math.floor(hours / 24)}d${hours % 24}h` : `${Math.floor(hours / 24)}d`
}

const items = computed<OverviewItem[]>(() => props.instances.map(instance => ({
  instance,
  title: headline(instance),
  waitedMs: now.value - new Date(instance.registered_at).getTime(),
})))

const filteredItems = computed(() => {
  const keyword = query.value.trim().toLowerCase()
  return items.value.filter((item) => {
    if (longWaitOnly.value && item.waitedMs < LONG_WAIT_MS)
      return false
    if (!keyword)
      return true
    // 标题只是正文第一行，搜索要覆盖整条消息。
    const text = item.instance.request_text || item.instance.request_title || ''
    return `${item.title} ${item.instance.project_path} ${text}`.toLowerCase().includes(keyword)
  })
})

// 当前窗口单独置顶，不参与分组。
const currentItem = computed(() => filteredItems.value.find(item => item.instance.pid === props.currentPid) ?? null)

// 按项目分组；组内和组间都让等得最久的排在前面。
const groups = computed<OverviewGroup[]>(() => {
  const byPath = new Map<string, OverviewGroup>()
  const others = filteredItems.value.filter(item => item.instance.pid !== props.currentPid)
  for (const item of others.sort((a, b) => b.waitedMs - a.waitedMs)) {
    const path = item.instance.project_path
    if (!byPath.has(path))
      byPath.set(path, { project: projectName(path), path, items: [] })
    byPath.get(path)!.items.push(item)
  }
  return [...byPath.values()]
})

const orderedPids = computed(() => [
  ...(currentItem.value ? [currentItem.value.instance.pid] : []),
  ...groups.value.flatMap(group => group.items.map(item => item.instance.pid)),
])
const longWaitCount = computed(() => items.value.filter(item => item.waitedMs >= LONG_WAIT_MS).length)

// 默认选中等得最久的其他窗口，Enter 直接跳过去。
function defaultSelection(pids: number[]): number {
  return pids.find(pid => pid !== props.currentPid) ?? pids[0] ?? 0
}

watch(orderedPids, (pids) => {
  if (!pids.includes(selectedPid.value))
    selectedPid.value = defaultSelection(pids)
})

watch(() => props.visible, async (visible) => {
  if (!visible)
    return
  now.value = Date.now()
  query.value = ''
  longWaitOnly.value = false
  selectedPid.value = defaultSelection(orderedPids.value)
  if (!props.interactive)
    return
  await nextTick()
  searchInput.value?.focus()
})

watch(() => props.interactive, async (interactive) => {
  if (!interactive || !props.visible)
    return
  await nextTick()
  searchInput.value?.focus()
})

async function activate(pid: number) {
  emit('close')
  if (pid === props.currentPid)
    return
  try {
    await invoke('activate_window_instance', { pid })
  }
  catch (error) {
    console.error('切换窗口失败:', error)
  }
}

function moveSelection(step: number) {
  const pids = orderedPids.value
  if (pids.length === 0)
    return
  const index = pids.indexOf(selectedPid.value)
  selectedPid.value = pids[(index + step + pids.length) % pids.length]
  nextTick(() => {
    document.querySelector(`[data-overview-pid="${selectedPid.value}"]`)?.scrollIntoView({ block: 'nearest' })
  })
}

// 捕获阶段拦截，避免 Esc 触发全局静音、Tab 触发最小化。
function handleKeydown(event: KeyboardEvent) {
  if (!props.visible || !props.interactive || event.isComposing)
    return
  const handled = ['Escape', 'Enter', 'ArrowDown', 'ArrowUp', 'ArrowRight', 'ArrowLeft', 'Tab']
  if (!handled.includes(event.key))
    return
  event.preventDefault()
  event.stopImmediatePropagation()
  if (event.key === 'Escape')
    emit('close')
  else if (event.key === 'Enter' && selectedPid.value)
    void activate(selectedPid.value)
  else if (event.key === 'ArrowDown' || event.key === 'ArrowRight' || (event.key === 'Tab' && !event.shiftKey))
    moveSelection(1)
  else if (event.key === 'ArrowUp' || event.key === 'ArrowLeft' || event.key === 'Tab')
    moveSelection(-1)
}

onMounted(() => {
  window.addEventListener('keydown', handleKeydown, true)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeydown, true)
})
</script>

<template>
  <Teleport to="body">
    <Transition name="overview-fade">
      <div
        v-if="visible"
        class="overview"
        :style="{ top: `${anchor?.top ?? 52}px`, left: `${anchor?.left ?? 12}px` }"
        role="dialog"
        aria-label="全部窗口"
      >
        <div
          class="overview-panel"
          @mouseenter="emit('hoverChange', true)"
          @mouseleave="emit('hoverChange', false)"
          @mousedown="emit('hoverChange', true)"
        >
          <div class="overview-head">
            <label class="overview-search">
              <span class="i-carbon-search overview-search-icon" aria-hidden="true" />
              <input
                id="window-overview-search"
                ref="searchInput"
                v-model="query"
                type="search"
                placeholder="搜索窗口"
                aria-label="搜索窗口"
              >
              <kbd>⌘⇧A</kbd>
            </label>
            <button
              v-if="longWaitCount > 0"
              type="button"
              class="overview-chip"
              :aria-pressed="longWaitOnly"
              title="只看等了 3 小时以上的窗口"
              @click="longWaitOnly = !longWaitOnly"
            >
              3h+ · {{ longWaitCount }}
            </button>
          </div>

          <div class="overview-body">
            <p v-if="instances.length === 0" class="overview-empty">
              没有其他打开的窗口
            </p>
            <p v-else-if="orderedPids.length === 0" class="overview-empty">
              没有匹配的窗口
            </p>
            <button
              v-if="currentItem"
              type="button"
              class="overview-row is-current"
              :class="{ 'is-selected': currentItem.instance.pid === selectedPid }"
              :data-overview-pid="currentItem.instance.pid"
              @click="activate(currentItem.instance.pid)"
              @mouseenter="selectedPid = currentItem.instance.pid"
            >
              <span class="overview-dot" aria-hidden="true" />
              <span class="overview-row-title" :class="{ 'is-blank': !currentItem.title }">{{ currentItem.title || '空白会话' }}</span>
              <span class="overview-wait">当前</span>
            </button>
            <section v-for="group in groups" :key="group.path" class="overview-group">
              <div class="overview-group-head" :title="group.path">
                <span>{{ group.project }}</span>
                <span>{{ group.items.length }}</span>
              </div>
              <button
                v-for="item in group.items"
                :key="item.instance.pid"
                type="button"
                class="overview-row"
                :class="{ 'is-selected': item.instance.pid === selectedPid }"
                :data-overview-pid="item.instance.pid"
                :title="item.title"
                @click="activate(item.instance.pid)"
                @mouseenter="selectedPid = item.instance.pid"
              >
                <span class="overview-dot" aria-hidden="true" />
                <span class="overview-row-title" :class="{ 'is-blank': !item.title }">{{ item.title || '空白会话' }}</span>
                <span class="overview-wait" :class="{ 'is-long': item.waitedMs >= LONG_WAIT_MS }">
                  {{ formatWait(item.waitedMs) }}
                </span>
              </button>
            </section>
          </div>

          <div v-if="quotaProviders?.length" class="overview-quota">
            <template v-for="provider in quotaProviders" :key="provider.id">
              <div
                v-for="metric in provider.metrics"
                :key="`${provider.id}-${metric.label}`"
                class="overview-meter"
                :title="metric.resetLabel"
              >
                <div class="overview-meter-line">
                  <span>{{ provider.name }} {{ metric.label }}</span>
                  <b>{{ Math.round(metric.remaining) }}%</b>
                </div>
                <div class="overview-meter-track">
                  <span :style="{ width: `${Math.max(0, Math.min(100, metric.remaining))}%` }" />
                </div>
              </div>
            </template>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
/* 跟随主题变量；深色模式另外抬高一层，免得和弹窗底色 #101014 糊成一片。 */
.overview {
  position: fixed;
  z-index: 9999;
  pointer-events: none;
}

.overview-panel {
  width: min(360px, calc(100vw - 24px));
  max-height: min(520px, calc(100vh - 80px));
  display: flex;
  flex-direction: column;
  background: var(--color-surface);
  color: var(--color-on-surface);
  border: 1px solid rgb(15 23 42 / 0.1);
  border-radius: 12px;
  box-shadow:
    0 18px 48px -16px rgb(15 23 42 / 0.28),
    0 2px 6px rgb(15 23 42 / 0.05);
  overflow: hidden;
  pointer-events: auto;
}

.overview-head {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px;
}

.overview-search {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  padding: 0 10px;
  height: 32px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: var(--color-surface-100);
  transition: border-color 0.12s;
}

.overview-search:focus-within {
  border-color: var(--color-surface-300);
}

.overview-search-icon {
  flex: none;
  width: 14px;
  height: 14px;
  color: var(--color-on-surface-muted);
}

.overview-search input {
  flex: 1;
  min-width: 0;
  border: 0;
  background: transparent;
  color: var(--color-on-surface);
  font-size: 13px;
  outline: none;
}

.overview-search input::placeholder {
  color: var(--color-on-surface-muted);
}

.overview-search input::-webkit-search-cancel-button {
  display: none;
}

.overview-search kbd {
  flex: none;
  color: var(--color-on-surface-muted);
  font: 11px var(--font-family-mono);
}

.overview-chip {
  flex: none;
  height: 32px;
  padding: 0 10px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: var(--color-surface-100);
  color: var(--color-on-surface-secondary);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  cursor: pointer;
}

.overview-chip[aria-pressed='true'] {
  background: var(--color-on-surface);
  color: var(--color-surface);
}

.overview-body {
  display: flex;
  flex-direction: column;
  padding: 0 6px 6px;
  overflow-y: auto;
}

.overview-empty {
  margin: 20px 0;
  text-align: center;
  font-size: 13px;
  color: var(--color-on-surface-muted);
}

.overview-group {
  display: flex;
  flex-direction: column;
}

.overview-group-head {
  display: flex;
  justify-content: space-between;
  padding: 10px 8px 4px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.02em;
  color: var(--color-on-surface-muted);
}

.overview-row {
  display: flex;
  align-items: center;
  gap: 10px;
  height: 34px;
  padding: 0 8px;
  border: 0;
  border-radius: 7px;
  background: transparent;
  color: var(--color-on-surface);
  font-size: 13px;
  text-align: left;
  cursor: pointer;
}

.overview-row.is-selected {
  background: var(--color-surface-100);
}

.overview-row:focus-visible {
  outline: 2px solid var(--color-on-surface);
  outline-offset: -2px;
}

.overview-dot {
  flex: none;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-surface-300);
}

.overview-row.is-current {
  color: var(--color-on-surface-muted);
}

.overview-row.is-current .overview-dot {
  background: var(--color-on-surface);
}

.overview-row-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.overview-row-title.is-blank {
  color: var(--color-on-surface-muted);
  font-style: italic;
}

.overview-wait {
  flex: none;
  font-size: 12px;
  color: var(--color-on-surface-muted);
  font-variant-numeric: tabular-nums;
}

.overview-wait.is-long {
  color: var(--color-on-surface);
  font-weight: 600;
}

.overview-quota {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px 14px;
  padding: 10px 14px 12px;
  border-top: 1px solid var(--color-surface-100);
}

.overview-meter {
  min-width: 0;
  font-size: 11px;
  color: var(--color-on-surface-secondary);
}

.overview-meter-line {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 4px;
}

.overview-meter-line span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.overview-meter-line b {
  color: var(--color-on-surface);
  font-variant-numeric: tabular-nums;
}

.overview-meter-track {
  height: 4px;
  overflow: hidden;
  border-radius: 999px;
  background: var(--color-surface-100);
}

.overview-meter-track span {
  display: block;
  height: 100%;
  min-width: 4px;
  border-radius: inherit;
  background: var(--color-on-surface);
}

:root.dark .overview-panel {
  background: var(--color-surface-50);
  border-color: rgb(255 255 255 / 0.08);
  box-shadow:
    0 22px 60px -12px rgb(0 0 0 / 0.7),
    inset 0 1px 0 rgb(255 255 255 / 0.04);
}

:root.dark .overview-search,
:root.dark .overview-chip {
  background: rgb(255 255 255 / 0.05);
}

:root.dark .overview-search:focus-within {
  border-color: rgb(255 255 255 / 0.14);
}

:root.dark .overview-row.is-selected {
  background: rgb(255 255 255 / 0.07);
}

:root.dark .overview-dot {
  background: rgb(255 255 255 / 0.18);
}

:root.dark .overview-row.is-current .overview-dot {
  background: var(--color-on-surface);
}

:root.dark .overview-wait.is-long {
  color: #f8fafc;
}

:root.dark .overview-quota {
  border-top-color: rgb(255 255 255 / 0.06);
}

:root.dark .overview-meter-track {
  background: rgb(255 255 255 / 0.1);
}

:root.dark .overview-meter-track span {
  background: linear-gradient(90deg, #f8fafc, #a1a1aa);
}

.overview-fade-enter-active,
.overview-fade-leave-active {
  transition: opacity 0.14s ease, transform 0.14s ease;
}

.overview-fade-enter-from,
.overview-fade-leave-to {
  opacity: 0;
  transform: translateY(-4px) scale(0.985);
}

@media (prefers-reduced-motion: reduce) {
  .overview-fade-enter-active,
  .overview-fade-leave-active {
    transition: none;
  }
}
</style>
