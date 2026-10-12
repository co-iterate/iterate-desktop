/**
 * 事件处理器封装
 * 将复杂的事件传递简化为可复用的处理器
 */
export function useEventHandlers(actions: any, _mcpRequest?: any) {
  return {
    // MCP 事件
    onMcpResponse: actions.mcp.handleResponse,
    onMcpCancel: actions.mcp.handleCancel,
    onMcpCloseCurrentDialog: actions.mcp.handleCloseCurrentDialog,

    // 主题事件
    onThemeChange: actions.theme.setTheme,

    // 设置事件
    onToggleAlwaysOnTop: actions.settings.toggleAlwaysOnTop,
    onToggleMute: actions.mcp.toggleMute,
    onToggleAudioNotification: actions.settings.toggleAudioNotification,
    onUpdateAudioUrl: actions.settings.updateAudioUrl,
    onTestAudio: actions.settings.testAudio,
    onStopAudio: actions.settings.stopAudio,
    onUpdateWindowSize: actions.settings.updateWindowSize,
    onUpdateReplyConfig: actions.settings.updateReplyConfig,
    onMessageReady: actions.settings.setMessageInstance,

    // 音频事件
    onTestAudioError: actions.audio.handleTestError,

    // 配置事件
    onConfigReloaded: actions.settings.reloadAllSettings,

    // Bridge 事件
    onBridgeAction: async (payload: any) => {
      console.log('🎯 [useEventHandlers] 收到 Bridge 动作:', payload)
      const {
        action,
        user_input,
        selected_options,
        images,
        file_paths,
        project_path,
        request_id,
        requestId,
        timeline_route_id,
        timelineRouteId: timelineRouteIdPayload,
        conversation_route_id,
        conversationRouteId: conversationRouteIdPayload,
      } = payload
      const requestRouteId = request_id || requestId || null
      const timelineRouteId = timeline_route_id
        || timelineRouteIdPayload
        || conversation_route_id
        || conversationRouteIdPayload
        || requestRouteId

      if (action === 'submit') {
        const normalizedImages = Array.isArray(images)
          ? images
              .map((img: any) => {
                const dataUrl = img?.data
                if (typeof dataUrl !== 'string')
                  return null

                const base64 = dataUrl.includes(',') ? dataUrl.split(',')[1] : dataUrl
                return {
                  data: base64,
                  media_type: img?.media_type || 'image/png',
                  filename: img?.filename ?? null,
                }
              })
              .filter(Boolean)
          : []
        const normalizedFilePaths = Array.isArray(file_paths)
          ? file_paths.filter((path: unknown): path is string => typeof path === 'string' && path.length > 0)
          : []

        const response = {
          user_input: user_input || null,
          selected_options: selected_options || [],
          images: normalizedImages,
          file_paths: normalizedFilePaths,
          image_paths: [],
          project_path: project_path || null,
          metadata: {
            timestamp: new Date().toISOString(),
            request_id: requestRouteId,
            timeline_route_id: timelineRouteId,
            conversation_route_id: timelineRouteId,
            source: 'web_bridge',
          },
        }
        await actions.mcp.handleResponse(response)
      }
      else if (action === 'continue') {
        // 模拟点击“继续”按钮
        await actions.mcp.handleMcpContinue()
      }
      else if (action === 'loop' || action === 'loop_start') {
        // 模拟点击“循环/接管”按钮
        await actions.mcp.handleMcpLoopReply('web_bridge_loop_start', user_input)
      }
      else if (action === 'enhance') {
        // 模拟点击“增强”按钮
        await actions.mcp.handleMcpEnhance(user_input)
      }
      else if (action === 'cancel') {
        await actions.mcp.handleCancel()
      }
      else if (action === 'update_conditional_state') {
        await actions.mcp.handleUpdateConditionalState(payload.promptId, payload.newState)
      }
      else if (action === 'update_conditional_active') {
        await actions.mcp.handleUpdateConditionalActive(payload.promptId, payload.isActive)
      }
      else if (action === 'update_custom_prompt_order') {
        await actions.mcp.handleUpdateCustomPromptOrder(payload.promptIds)
      }
    },
  }
}
