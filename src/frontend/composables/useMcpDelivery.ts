import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { onMounted, onUnmounted, ref } from 'vue'

export const mcpDeliveryError = ref('')
export const MCP_DELIVERY_FAILURE = '连接已断开，回复未确认送达，请检查 AI 客户端。'

export function useMcpDelivery() {
  let timer: ReturnType<typeof setInterval> | undefined
  let reading = false
  let warned = false
  async function refresh() {
    if (reading || warned)
      return
    reading = true
    try {
      const status = await invoke<string>('get_mcp_delivery_status')
      if (status === 'untracked' || status === 'returned') {
        clearInterval(timer)
        return
      }
      if (status === 'disconnected') {
        warned = true
        mcpDeliveryError.value = MCP_DELIVERY_FAILURE
        const window = getCurrentWindow()
        await window.show()
        await window.setFocus()
      }
    }
    catch (error) { console.error('读取调用交付状态失败:', error) }
    finally {
      reading = false
    }
  }
  onMounted(() => {
    timer = setInterval(() => {
      void refresh()
    }, 500)
    void refresh()
  })
  onUnmounted(() => clearInterval(timer))
  return { mcpDeliveryError }
}
