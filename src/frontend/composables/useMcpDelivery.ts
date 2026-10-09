import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { publishClosedMcpRequest } from '../services/bridgeRequestClosure'

export const mcpDeliveryError = ref('')
export const mcpDeliveryStatus = ref<'unknown' | 'untracked' | 'waiting' | 'returned' | 'disconnected'>('unknown')
export const MCP_DELIVERY_FAILURE = '连接已断开，回复未确认送达，请检查 AI 客户端。'

interface DeliverySource {
  id?: unknown
  project_path?: unknown
  source?: unknown
}

export function useMcpDelivery(getSource: () => DeliverySource | null) {
  let timer: ReturnType<typeof setInterval> | undefined
  let reading = false
  let warned = false
  let mounted = false
  let closed = false
  let sourceRevision = 0
  let originalSource: { id: string, projectPath: string | null } | null = null

  function readSource() {
    const request = getSource()
    const id = typeof request?.id === 'string' ? request.id.trim() : ''
    const projectPath = typeof request?.project_path === 'string' ? request.project_path.trim() : ''
    if (!id || id.startsWith('native-codex:') || request?.source === 'codex_native') {
      return null
    }
    const lowerProject = projectPath.toLowerCase()
    const closeProject = !projectPath || projectPath === '.' || lowerProject === 'unknown'
      || lowerProject.startsWith('unknown:') || lowerProject.startsWith('standalone:')
      ? null
      : projectPath
    return { id, projectPath: closeProject }
  }

  watch(() => {
    const request = getSource()
    return [request?.id, request?.project_path, request?.source] as const
  }, () => {
    sourceRevision++
    // Delivery status belongs to this process's original MCP source, not a
    // later displayed question. The optional workspace gates remote closure,
    // but must not prevent the original request's local delivery warning.
    const current = readSource()
    originalSource ??= current
    if (current && originalSource && current.id === originalSource.id
      && !originalSource.projectPath && current.projectPath) {
      originalSource = { id: originalSource.id, projectPath: current.projectPath }
    }
  }, { immediate: true, flush: 'sync' })

  async function refresh() {
    if (reading || closed || !originalSource)
      return
    const source = originalSource
    const revision = sourceRevision
    const isCurrent = () => {
      const current = readSource()
      return mounted && revision === sourceRevision
        && current?.id === source.id
    }
    const canClose = () => isCurrent() && source.projectPath !== null
      && readSource()?.projectPath === source.projectPath
    if (!isCurrent())
      return
    reading = true
    try {
      const status = await invoke<string>('get_mcp_delivery_status')
      if (!isCurrent())
        return
      mcpDeliveryStatus.value = status === 'untracked' || status === 'waiting' || status === 'returned' || status === 'disconnected'
        ? status
        : 'unknown'
      if (status === 'untracked' || status === 'returned') {
        clearInterval(timer)
        return
      }
      if (status === 'disconnected') {
        mcpDeliveryError.value = MCP_DELIVERY_FAILURE
        // Retry closure independently of the once-only window warning.
        if (!warned) {
          warned = true
          const focusOnShow = await invoke<{ focus_popup_on_show?: boolean }>('get_window_config')
            .then(config => config.focus_popup_on_show !== false)
            .catch(() => true)
          if (!isCurrent())
            return
          if (!focusOnShow) {
            await invoke('center_window')
          }
          else {
            const window = getCurrentWindow()
            await window.show()
            if (isCurrent())
              await window.setFocus()
          }
        }
        if (source.projectPath && await publishClosedMcpRequest(source.id, source.projectPath, canClose)) {
          closed = true
          clearInterval(timer)
        }
      }
    }
    catch (error) { console.error('读取调用交付状态失败:', error) }
    finally {
      reading = false
    }
  }
  onMounted(() => {
    mounted = true
    timer = setInterval(() => {
      void refresh()
    }, 500)
    void refresh()
  })
  onUnmounted(() => {
    mounted = false
    clearInterval(timer)
  })
  return { mcpDeliveryError }
}
