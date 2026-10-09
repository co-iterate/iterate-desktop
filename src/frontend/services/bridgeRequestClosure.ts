import { invoke } from '@tauri-apps/api/core'

/** Tell Bridge that a delivered MCP request has finished before its window exits. */
export async function publishCompletedMcpRequest(requestId: unknown, rawProjectPath: unknown): Promise<boolean> {
  return publishClosedMcpRequest(requestId, rawProjectPath)
}

/** Close one exact MCP request; a close notification is not a delivery receipt. */
export async function publishClosedMcpRequest(
  requestId: unknown,
  rawProjectPath: unknown,
  isCurrent: () => boolean = () => true,
): Promise<boolean> {
  const request_id = typeof requestId === 'string' ? requestId.trim() : ''
  const project_path = typeof rawProjectPath === 'string' ? rawProjectPath.trim() : ''
  if (!request_id || !project_path || project_path === '.' || project_path.toLowerCase() === 'unknown')
    return false

  for (let attempt = 0; attempt < 3; attempt++) {
    if (!isCurrent())
      return false
    try {
      await invoke('send_to_web_bridge', {
        message: {
          message_type: 'mcp_state',
          payload: {
            request: null,
            showMcpPopup: false,
            request_id,
            project_path,
          },
        },
      })
      return true
    }
    catch (error) {
      if (attempt === 2) {
        console.warn('[MCP] 发布请求的关闭状态失败:', error)
        return false
      }
      await new Promise(resolve => setTimeout(resolve, 500 * (attempt + 1)))
    }
  }
  return false
}
