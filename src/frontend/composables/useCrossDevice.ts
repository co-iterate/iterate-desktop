import { invoke } from '@tauri-apps/api/core'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { publishCompletedMcpRequest } from '../services/bridgeRequestClosure'

interface CrossDeviceStatus {
  enabled: boolean
  connected: boolean
  connected_ip?: string
  using_backup?: boolean
  peer_enabled?: boolean
  mirror: boolean
  origin_name?: string
  resolved?: boolean
  local_only?: boolean
  source_pending?: { response: unknown, request_id: string, project_path: string | null, timeline_route_id?: string | null }
  error?: string
}

const state = ref<CrossDeviceStatus>({ enabled: false, connected: false, mirror: false })
const busy = ref(false)
export const crossDeviceSendError = ref('')
let reading = false
let subscribers = 0
let timer: ReturnType<typeof setInterval> | undefined

export async function refreshCrossDevice() {
  if (reading)
    return
  reading = true
  try {
    state.value = await invoke<CrossDeviceStatus>('get_cross_device_status')
    if (state.value.source_pending) {
      const pending = state.value.source_pending
      await invoke('send_mcp_response', { response: pending.response, requestId: pending.request_id, projectPath: pending.project_path, timelineRouteId: pending.timeline_route_id ?? null })
      await publishCompletedMcpRequest(pending.request_id, pending.project_path)
      // Deferred sources retain their GUI until the HTTP handoff completes.
      // Only close after success; a failed handoff must keep its error window.
      await invoke('exit_app')
    }
    if (state.value.mirror && state.value.resolved)
      await invoke('exit_app')
  }
  catch (error) {
    state.value = { ...state.value, connected: false, error: String(error) }
  }
  finally { reading = false }
}

export function useCrossDevice() {
  onMounted(() => {
    if (subscribers++ === 0) {
      void refreshCrossDevice()
      timer = setInterval(() => {
        void refreshCrossDevice()
      }, 2000)
    }
  })
  onUnmounted(() => {
    if (--subscribers === 0)
      clearInterval(timer)
  })
  const title = computed(() => {
    if (!state.value.enabled)
      return `跨设备已关闭，点击开启${state.value.error ? `（${state.value.error}）` : ''}`
    if (!state.value.connected)
      return '跨设备已开启 · 连接异常，点击关闭'
    if (!state.value.peer_enabled)
      return '跨设备已开启 · 等待对端开启，点击关闭'
    return state.value.local_only ? '跨设备已开启 · 本次请求仅在本机处理（新请求生效；附件不镜像）' : '跨设备已开启 · 两端已连接，点击关闭'
  })
  const color = computed(() => !state.value.enabled ? '#4b5563' : state.value.connected && state.value.peer_enabled ? '#15803d' : '#b45309')
  async function toggle() {
    if (busy.value)
      return
    busy.value = true
    try {
      state.value = await invoke<CrossDeviceStatus>('set_cross_device_enabled', { enabled: !state.value.enabled })
    }
    catch (error) {
      state.value = { ...state.value, error: String(error) }
    }
    finally { busy.value = false }
  }
  return { crossState: state, crossBusy: busy, crossTitle: title, crossColor: color, toggleCrossDevice: toggle }
}
