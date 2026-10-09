<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'

interface Tab { pid: number, request_id: string, title: string, project_path: string, unread: boolean }
interface Snapshot { mode: 'windows' | 'tabs', current_pid: number, active_pid: number | null, tabs: Tab[] }
const props = defineProps<{ requestId?: string }>()
const emit = defineEmits<{ visibilityChange: [visible: boolean] }>()
const snapshot = ref<Snapshot | null>(null)
const error = ref('')
const visible = computed(() => snapshot.value?.mode === 'tabs'
  && snapshot.value.tabs.some(tab => tab.pid === snapshot.value?.current_pid && tab.request_id === props.requestId))
watch(visible, value => emit('visibilityChange', value), { immediate: true, flush: 'sync' })
let unlisten: (() => void) | undefined
let disposed = false
onMounted(async () => {
  window.addEventListener('keydown', handleTabSwitchKeydown, true)
  const stop = await listen<Snapshot>('popup-tabs-changed', (event) => {
    snapshot.value = event.payload
  })
  if (disposed) {
    stop()
    return
  }
  unlisten = stop
  try {
    snapshot.value = await invoke<Snapshot>('get_popup_tabs')
  }
  catch (e) {
    console.warn('读取标签列表失败:', e)
  }
})
onUnmounted(() => {
  disposed = true
  emit('visibilityChange', false)
  window.removeEventListener('keydown', handleTabSwitchKeydown, true)
  unlisten?.()
})
function handleTabSwitchKeydown(event: KeyboardEvent) {
  if (!visible.value || !document.hasFocus() || event.isComposing
    || event.key !== 'Tab' || !event.ctrlKey || event.shiftKey || event.altKey || event.metaKey) {
    return
  }
  event.preventDefault()
  event.stopPropagation()
  const state = snapshot.value
  if (event.repeat || !state || state.active_pid !== state.current_pid || state.tabs.length < 2)
    return
  const current = state.tabs.findIndex(tab => tab.pid === state.current_pid)
  if (current >= 0)
    void select(state.tabs[(current + 1) % state.tabs.length])
}
async function select(tab: Tab) {
  if (tab.pid === snapshot.value?.current_pid)
    return
  error.value = ''
  try {
    snapshot.value = await invoke<Snapshot>('select_popup_tab', { pid: tab.pid })
  }
  catch (e) {
    error.value = String(e)
  }
}
</script>

<template>
  <div v-if="visible" class="flex-shrink-0 border-b border-black-200 bg-black-100">
    <div role="tablist" aria-label="会话标签" class="flex gap-1 overflow-x-auto scrollbar-thin p-1">
      <button
        v-for="tab in snapshot?.tabs" :key="`${tab.pid}:${tab.request_id}`"
        type="button" role="tab" :aria-selected="tab.pid === snapshot?.current_pid"
        :title="`${tab.title}\n${tab.project_path}`"
        class="min-h-10 max-w-48 flex-shrink-0 truncate rounded border border-solid px-3 py-2 text-sm"
        :class="tab.pid === snapshot?.current_pid ? 'border-white bg-black-300 text-white font-medium' : 'border-[var(--color-on-surface-muted)] bg-black-50 text-[var(--color-on-surface-secondary)] hover:bg-black-200 hover:text-white'"
        @click="select(tab)"
      >
        <span v-if="tab.unread" aria-label="未读">● </span>{{ tab.title || tab.project_path || '会话' }}
      </button>
    </div>
    <div v-if="error" role="alert" class="px-3 py-1 text-xs text-red-500">
      {{ error }}
    </div>
  </div>
</template>
