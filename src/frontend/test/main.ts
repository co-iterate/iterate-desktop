// 导入 Naive UI 组件 (复用主应用的配置)
import {
  create,
  NAlert,
  NButton,
  NCard,
  NCheckbox,
  NCollapse,
  NCollapseItem,
  NConfigProvider,
  NDialogProvider,
  NEmpty,
  NFormItem,
  NGrid,
  NGridItem,
  NImage,
  NImageGroup,
  NInput,
  NInputGroup,
  NInputNumber,
  NMessageProvider,
  NModal,
  NNotificationProvider,
  NSelect,
  NSpace,
  NSpin,
  NSwitch,
  NTab,
  NTabPane,
  NTabs,
  NTag,
  NTooltip,
} from 'naive-ui'
import { createApp } from 'vue'

// 导入主题
import { useTheme } from '../composables/useTheme'

import TestApp from './TestApp.vue'
// 导入样式
import 'virtual:uno.css'

import '../assets/styles/style.css'

// 创建 Naive UI 实例
const naive = create({
  components: [
    NButton,
    NAlert,
    NCard,
    NCheckbox,
    NCollapse,
    NCollapseItem,
    NConfigProvider,
    NEmpty,
    NFormItem,
    NGrid,
    NGridItem,
    NImage,
    NImageGroup,
    NInput,
    NInputGroup,
    NInputNumber,
    NModal,
    NSelect,
    NSpace,
    NSpin,
    NSwitch,
    NTab,
    NTabPane,
    NTabs,
    NTag,
    NTooltip,
    NMessageProvider,
    NNotificationProvider,
    NDialogProvider,
  ],
})

// 模拟 Tauri API 用于测试环境
if (!window.__TAURI__) {
  const urlParams = new URLSearchParams(window.location.search)
  const goalRunScenario = urlParams.get('goalrun') || ''
  const goalRunInvokeLog: Array<{ command: string, args?: Record<string, unknown> }> = []
  ;(window as any).__goalrunGuiInvokeLog = goalRunInvokeLog

  let mockLiveGoal: Record<string, unknown> | null = {
    id: 'goal_preview',
    title: '测试手机 Goal MVP',
    status: 'running',
    phase: 'running',
    status_text: '执行中',
    progress_percent: 42,
    progress_source: 'test_preview',
    progress_label: '42%',
    plan_total: 5,
    plan_completed: 2,
    tokens_used: 12000,
    token_budget: 30000,
    time_used_seconds: 3540,
    started_at_ms: Date.now() - 59 * 60 * 1000,
    updated_at_ms: Date.now(),
    completed_at_ms: null,
    project_path: '/Users/test/project',
    request_id: 'preview-request',
    codex_thread_id: 'preview-codex-thread',
    codex_deeplink: 'codex://thread/preview-codex-thread',
    last_codex_event_at_ms: Date.now(),
    source: 'test_preview',
  }

  const cloudflareScenario = urlParams.get('cloudflare') || 'default'
  const quickTunnelScenario = urlParams.get('quick') || 'consent'
  let quickTunnelStatusReads = 0
  let mockQuickTunnelStatus = quickTunnelScenario === 'missing'
    ? {
        state: 'Error',
        phase: 'cloudflared_missing',
        progress: 15,
        endpoint: null,
        verified: false,
        error_code: 'cloudflared_missing',
        consent_given: true,
        enabled: true,
        endpoint_epoch: 2,
      }
    : {
        state: 'Stopped',
        phase: 'idle',
        progress: 0,
        endpoint: null as string | null,
        verified: false,
        error_code: null as string | null,
        consent_given: false,
        enabled: false,
        endpoint_epoch: 1,
      }
  const cloudflareFreshnessMs = 10 * 60 * 1000
  const mockCloudflareHostname = 'https://iterate.example.com'
  const mockWebLoginConsoleOrigin = 'https://app.iterate.example.com'
  const nowIso = () => new Date().toISOString()
  const minutesAgoIso = (minutes: number) => new Date(Date.now() - minutes * 60 * 1000).toISOString()
  const makeCloudflareVerification = (overrides: Record<string, unknown> = {}) => ({
    state: 'verified',
    public_hostname: mockCloudflareHostname,
    health_ok: true,
    pair_challenge_ok: true,
    websocket_ok: true,
    access_state: 'not_expected',
    error_code: null,
    checked_at: nowIso(),
    ...overrides,
  })
  const makeCloudflareConfig = (overrides: Record<string, unknown> = {}) => {
    const verification = makeCloudflareVerification()
    return {
      guided_setup_enabled: true,
      public_hostname: mockCloudflareHostname,
      access_expected: false,
      web_login_console_origin: mockWebLoginConsoleOrigin,
      tunnel_token_saved: true,
      last_verified_at: verification.checked_at,
      last_verification: verification,
      ...overrides,
    }
  }
  const defaultCloudflareConfig = () => ({
    guided_setup_enabled: false,
    public_hostname: '',
    access_expected: false,
    web_login_console_origin: '',
    tunnel_token_saved: false,
    last_verified_at: null,
    last_verification: null,
  })
  const cloudflareConfigForScenario = () => {
    if (cloudflareScenario === 'verified')
      return makeCloudflareConfig()
    if (cloudflareScenario === 'stale') {
      const checkedAt = minutesAgoIso(20)
      return makeCloudflareConfig({
        last_verified_at: checkedAt,
        last_verification: makeCloudflareVerification({ checked_at: checkedAt }),
      })
    }
    if (cloudflareScenario === 'mismatch') {
      return makeCloudflareConfig({
        public_hostname: 'https://iterate-new.example.com',
        last_verification: makeCloudflareVerification({
          public_hostname: 'https://iterate-old.example.com',
        }),
      })
    }
    if (cloudflareScenario === 'no-token') {
      return makeCloudflareConfig({
        tunnel_token_saved: false,
      })
    }
    if (cloudflareScenario === 'access-missing') {
      const checkedAt = nowIso()
      return makeCloudflareConfig({
        access_expected: true,
        last_verified_at: checkedAt,
        last_verification: makeCloudflareVerification({
          state: 'access_expected_missing',
          access_state: 'not_detected',
          error_code: 'access_expected_missing',
          checked_at: checkedAt,
        }),
      })
    }
    return defaultCloudflareConfig()
  }
  let mockCloudflareConfig = cloudflareConfigForScenario()
  let mockCloudflareWebLoginSessions: Array<Record<string, unknown>> = []
  const cloudflareVerificationUsable = () => {
    const verification = mockCloudflareConfig.last_verification as Record<string, unknown> | null
    if (!verification || verification.state !== 'verified')
      return false
    if (!mockCloudflareConfig.tunnel_token_saved)
      return false
    if (verification.public_hostname !== mockCloudflareConfig.public_hostname)
      return false
    const checkedAt = typeof verification.checked_at === 'string' ? Date.parse(verification.checked_at) : Number.NaN
    if (Number.isNaN(checkedAt))
      return false
    const ageMs = Date.now() - checkedAt
    return ageMs >= 0 && ageMs <= cloudflareFreshnessMs
  }

  const createMockMcpTools = () => [
    {
      id: 'zhi',
      name: 'iterate',
      description: '智能代码审查交互工具（L0 协调者）。所有对话必经，控制任务流程。',
      enabled: true,
      can_disable: false,
      icon: 'i-carbon-chat text-lg text-blue-600 dark:text-blue-400',
      icon_bg: 'bg-blue-100 dark:bg-blue-900',
      dark_icon_bg: 'dark:bg-blue-800',
      has_config: false,
    },
    {
      id: 'ji',
      name: '记忆管理',
      description: '全局记忆管理工具。支持 4 种 action：回忆/记忆/沉淀/摘要。',
      enabled: true,
      can_disable: true,
      icon: 'i-carbon-data-base text-lg text-purple-600 dark:text-purple-400',
      icon_bg: 'bg-green-100 dark:bg-green-900',
      dark_icon_bg: 'dark:bg-green-800',
      has_config: false,
    },
    {
      id: 'sou',
      name: '代码搜索',
      description: '智能代码搜索工具。自动判断搜索类型：代码相关→语义搜索；外部知识→网络搜索。',
      enabled: true,
      can_disable: true,
      icon: 'i-carbon-search text-lg text-green-600 dark:text-green-400',
      icon_bg: 'bg-green-100 dark:bg-green-900',
      dark_icon_bg: 'dark:bg-green-800',
      has_config: true,
    },
    {
      id: 'pai',
      name: 'Pai Room 编排',
      description: 'Pai Room 编排工具。生成 codex-room 调度草案和回包协议，不派发子代理。',
      enabled: true,
      can_disable: true,
      icon: 'i-carbon-bot text-lg text-orange-600 dark:text-orange-400',
      icon_bg: 'bg-orange-100 dark:bg-orange-900',
      dark_icon_bg: 'dark:bg-orange-800',
      has_config: false,
    },
    {
      id: 'xi',
      name: '经验查找',
      description: '经验查找工具。在 .cunzhi-knowledge/ 中查找相关历史经验。',
      enabled: true,
      can_disable: true,
      icon: 'i-carbon-book text-lg text-cyan-600 dark:text-cyan-400',
      icon_bg: 'bg-cyan-100 dark:bg-cyan-900',
      dark_icon_bg: 'dark:bg-cyan-800',
      has_config: false,
    },
    {
      id: 'ci',
      name: '提示词库',
      description: '提示词库搜索工具。在 .cunzhi-knowledge/prompts/ 中搜索相关模板。',
      enabled: true,
      can_disable: true,
      icon: 'i-carbon-catalog text-lg text-indigo-600 dark:text-indigo-400',
      icon_bg: 'bg-indigo-100 dark:bg-indigo-900',
      dark_icon_bg: 'dark:bg-indigo-800',
      has_config: false,
    },
    {
      id: 'task',
      name: '任务系统',
      description: '文件持久化任务系统。任务存储在 .cunzhi-memory/tasks.json，跨会话持久。',
      enabled: true,
      can_disable: true,
      icon: 'i-carbon-task text-lg text-teal-600 dark:text-teal-400',
      icon_bg: 'bg-teal-100 dark:bg-teal-900',
      dark_icon_bg: 'dark:bg-teal-800',
      has_config: false,
    },
    {
      id: 'phone_action',
      name: 'iPhone 动作',
      description: '把 AI 请求路由成 iPhone 可公开执行的安全动作，如启动语音、写剪贴板、打开 URL。',
      enabled: true,
      can_disable: true,
      icon: 'i-carbon-mobile text-lg text-sky-600 dark:text-sky-400',
      icon_bg: 'bg-sky-100 dark:bg-sky-900',
      dark_icon_bg: 'dark:bg-sky-800',
      has_config: false,
    },
    {
      id: 'cron_manage',
      name: '定时任务',
      description: '管理系统 crontab 定时任务。会写入持久 shell 命令，默认关闭且调用前需要 iterate 确认。',
      enabled: false,
      can_disable: true,
      icon: 'i-carbon-time text-lg text-rose-600 dark:text-rose-400',
      icon_bg: 'bg-rose-100 dark:bg-rose-900',
      dark_icon_bg: 'dark:bg-rose-800',
      has_config: false,
    },
  ]
  let mockMcpTools = createMockMcpTools()

  const mockInvoke = async (command: string, args?: Record<string, unknown>) => {
    console.log(`模拟 Tauri 调用: ${command}`, args)
    goalRunInvokeLog.push({ command, args })
    if (command === 'get_mcp_tools_config') {
      return mockMcpTools.map(tool => ({ ...tool }))
    }
    if (command === 'set_mcp_tool_enabled') {
      const toolId = String(args?.toolId || '')
      const tool = mockMcpTools.find(item => item.id === toolId)
      if (!tool || !tool.can_disable)
        throw new Error('工具不存在或不可禁用')
      tool.enabled = Boolean(args?.enabled)
      return null
    }
    if (command === 'reset_mcp_tools_config') {
      mockMcpTools = createMockMcpTools()
      return null
    }
    if (command === 'get_live_goal') {
      return mockLiveGoal
    }
    if (command === 'get_latest_ai_response') {
      return null
    }
    if (command === 'start_live_goal') {
      const now = Date.now()
      mockLiveGoal = {
        id: `goal_${now}`,
        title: String(args?.title || '新的 iterate 目标'),
        status: 'running',
        phase: 'running',
        status_text: '执行中',
        progress_percent: 0,
        progress_source: 'test_preview',
        progress_label: '0%',
        plan_total: null,
        plan_completed: null,
        tokens_used: null,
        token_budget: null,
        time_used_seconds: 0,
        started_at_ms: now,
        updated_at_ms: now,
        completed_at_ms: null,
        project_path: args?.projectPath || '/Users/test/project',
        request_id: args?.requestId || `preview-${now}`,
        codex_thread_id: args?.codexThreadId || 'preview-codex-thread',
        codex_deeplink: args?.codexDeeplink || 'codex://thread/preview-codex-thread',
        ...(goalRunScenario === 'late-stale'
          ? {
              run_id: 'run-current',
              generation: 200,
            }
          : {}),
        last_codex_event_at_ms: now,
        source: 'test_preview',
      }
      return mockLiveGoal
    }
    if (command === 'get_hui_snapshot') {
      if (goalRunScenario === 'late-stale') {
        return [
          '## Hui Snapshot',
          '- 最新用户输入：当前 run 回包',
          '- meta.route：`thread-current`',
          '- meta.request_id：`req-current`',
          '- meta.run_id：`run-current`',
          '- meta.generation：`200`',
          '- meta.stale_of：`无`',
          '- meta.superseded_by：`无`',
        ].join('\n')
      }
      return null
    }
    if (command === 'resolve_live_goal_response_metadata') {
      if (goalRunScenario === 'late-stale') {
        const runId = typeof args?.runId === 'string' ? args.runId : null
        const generation = typeof args?.generation === 'number'
          ? args.generation
          : typeof args?.generation === 'string'
            ? Number(args.generation)
            : null

        if (runId === 'run-old' || (generation !== null && generation < 200)) {
          return {
            run_id: runId || 'run-old',
            generation: generation ?? 100,
            stale_of: 'run-old',
            superseded_by: 'run-current',
            is_stale: true,
          }
        }

        return {
          run_id: 'run-current',
          generation: 200,
          stale_of: null,
          superseded_by: null,
          is_stale: false,
        }
      }
      return {}
    }
    if (command === 'complete_live_goal') {
      if (!mockLiveGoal)
        return null
      const now = Date.now()
      mockLiveGoal = {
        ...mockLiveGoal,
        status: 'completed',
        phase: 'completed',
        status_text: '已完成',
        progress_percent: 100,
        progress_label: '100%',
        updated_at_ms: now,
        completed_at_ms: now,
      }
      return mockLiveGoal
    }
    if (command === 'clear_live_goal') {
      mockLiveGoal = null
      return null
    }

    // 模拟主题配置返回（沙盒默认浅色，避免深色对比度过低看不清）
    if (command === 'get_theme') {
      return 'light'
    }
    if (command === 'get_theme_config') {
      return { theme: 'light' }
    }
    if (command === 'get_reply_config') {
      return {
        continue_prompt: '请按照最佳实践继续',
        loop_prompt: '进入自主循环模式。\n\n## 执行规则\n1. 基于当前上下文，按最佳实践继续执行当前任务\n2. 每轮完成后立即调用 iterate/zhi 汇报进度，不要等待用户\n3. 如果任务未完成且无需用户决策，继续自动执行下一步\n\n## 停止条件（满足任一即停止）\n- 任务已全部完成\n- 遇到必须由用户决定的问题\n- 遇到无法自动解决的错误（连续失败2次）\n- 不确定下一步该做什么\n\n## 汇报格式\n每轮简要说明：做了什么 → 结果如何 → 下一步计划',
      }
    }
    if (command === 'get_shortcut_config') {
      return {
        shortcuts: {
          quick_submit: {
            id: 'quick_submit',
            name: '快速发送',
            description: '提交当前输入',
            action: 'submit',
            key_combination: { key: 'Enter', ctrl: false, alt: false, shift: true, meta: false },
            enabled: true,
            scope: 'popup',
          },
          enhance: {
            id: 'enhance',
            name: '目标',
            description: '提交当前输入为目标',
            action: 'enhance',
            key_combination: { key: 'Enter', ctrl: true, alt: false, shift: true, meta: false },
            enabled: true,
            scope: 'popup',
          },
          continue: {
            id: 'continue',
            name: '继续',
            description: '继续当前任务',
            action: 'continue',
            key_combination: { key: 'Enter', ctrl: true, alt: false, shift: false, meta: false },
            enabled: true,
            scope: 'popup',
          },
          quote_selection: {
            id: 'quote_selection',
            name: '引用选区',
            description: '将当前选区作为引用插入输入框',
            action: 'quote_selection_to_input',
            key_combination: { key: 'Y', ctrl: false, alt: false, shift: true, meta: true },
            enabled: true,
            scope: 'popup',
          },
        },
      }
    }
    if (command === 'get_remote_tunnel_status') {
      return {
        state: 'stopped',
        domain: null,
        pid: null,
        last_error: null,
        recent_logs: ['测试环境：未启动公网兜底'],
        origin_healthy: true,
      }
    }
    if (command === 'get_bridge_desktop_token') {
      return 'desktop-test-token'
    }
    if (command === 'recover_bridge_origin') {
      return { status: 'already_healthy', healthy: true, recovered: false }
    }
    if (command === 'get_quick_tunnel_status') {
      quickTunnelStatusReads += 1
      if (mockQuickTunnelStatus.state === 'Starting' && quickTunnelStatusReads >= 2) {
        mockQuickTunnelStatus = {
          ...mockQuickTunnelStatus,
          phase: 'verifying_endpoint',
          progress: 70,
        }
      }
      if (mockQuickTunnelStatus.state === 'Starting' && quickTunnelStatusReads >= 4) {
        mockQuickTunnelStatus = {
          ...mockQuickTunnelStatus,
          state: 'Running',
          phase: 'ready',
          progress: 100,
          endpoint: 'https://preview.trycloudflare.com',
          verified: true,
        }
      }
      return mockQuickTunnelStatus
    }
    if (command === 'start_quick_tunnel') {
      quickTunnelStatusReads = 0
      mockQuickTunnelStatus = {
        ...mockQuickTunnelStatus,
        state: 'Starting',
        phase: 'starting_cloudflared',
        progress: 35,
        consent_given: true,
        enabled: true,
        error_code: null,
        endpoint_epoch: mockQuickTunnelStatus.endpoint_epoch + 1,
      }
      return mockQuickTunnelStatus
    }
    if (command === 'stop_quick_tunnel') {
      mockQuickTunnelStatus = {
        ...mockQuickTunnelStatus,
        state: 'Stopped',
        phase: 'idle',
        progress: 0,
        endpoint: null,
        verified: false,
        enabled: false,
      }
      return mockQuickTunnelStatus
    }
    if (command === 'check_origin_health') {
      return true
    }
    if (command === 'start_remote_tunnel' || command === 'stop_remote_tunnel') {
      return null
    }
    if (command === 'get_cloudflare_guided_config') {
      return { config: mockCloudflareConfig }
    }
    if (command === 'save_cloudflare_guided_config') {
      const request = args?.request as Record<string, unknown> | undefined
      const nextHostname = String(request?.public_hostname || '').trim()
      const nextAccessExpected = Boolean(request?.access_expected)
      const nextConsoleOrigin = String(request?.web_login_console_origin || '').trim()
      const nextToken = typeof request?.tunnel_token === 'string' ? request.tunnel_token.trim() : ''
      const changed = nextHostname !== mockCloudflareConfig.public_hostname
        || nextAccessExpected !== mockCloudflareConfig.access_expected
        || !!nextToken
      mockCloudflareConfig = {
        ...mockCloudflareConfig,
        guided_setup_enabled: true,
        public_hostname: nextHostname,
        access_expected: nextAccessExpected,
        web_login_console_origin: nextConsoleOrigin,
        tunnel_token_saved: mockCloudflareConfig.tunnel_token_saved || !!nextToken,
        last_verified_at: changed ? null : mockCloudflareConfig.last_verified_at,
        last_verification: changed ? null : mockCloudflareConfig.last_verification,
      }
      return { config: mockCloudflareConfig }
    }
    if (command === 'clear_cloudflare_guided_config') {
      mockCloudflareConfig = defaultCloudflareConfig()
      mockCloudflareWebLoginSessions = []
      return { config: mockCloudflareConfig }
    }
    if (command === 'create_cloudflare_web_login_auto_setup') {
      const request = args?.request as Record<string, unknown> | undefined
      const accessEmails = Array.isArray(request?.access_emails) ? request.access_emails : []
      const checkedAt = nowIso()
      const accessConfigured = accessEmails.length > 0
      const verification = makeCloudflareVerification({
        access_state: accessConfigured ? 'configured' : 'not_configured_by_iterate',
        checked_at: checkedAt,
      })
      mockCloudflareConfig = {
        ...makeCloudflareConfig({
          access_expected: accessConfigured,
          last_verified_at: checkedAt,
          last_verification: verification,
        }),
      }
      return {
        public_hostname: mockCloudflareHostname,
        tunnel_id: 'test-tunnel-id',
        tunnel_name: 'iterate-web-login-iterate-example-com-test',
        dns_record_id: 'test-dns-record-id',
        dns_action: 'created',
        access_app_id: accessConfigured ? 'test-access-app-id' : null,
        access_policy_id: accessConfigured ? 'test-access-policy-id' : null,
        access_state: accessConfigured ? 'configured' : 'not_configured_by_iterate',
        verification,
      }
    }
    if (command === 'verify_cloudflare_guided_config') {
      const checkedAt = nowIso()
      const result = mockCloudflareConfig.access_expected
        ? makeCloudflareVerification({
            public_hostname: mockCloudflareConfig.public_hostname,
            health_ok: false,
            pair_challenge_ok: false,
            websocket_ok: false,
            access_state: 'access_enabled',
            checked_at: checkedAt,
          })
        : makeCloudflareVerification({
            public_hostname: mockCloudflareConfig.public_hostname,
            checked_at: checkedAt,
          })
      mockCloudflareConfig = {
        ...mockCloudflareConfig,
        last_verified_at: checkedAt,
        last_verification: result,
      }
      return result
    }
    if (command === 'create_cloudflare_web_login_pairing') {
      if (!cloudflareVerificationUsable())
        throw new Error('测试环境：Cloudflare verification 不可用，请重新验证')
      const issuedAt = nowIso()
      const expiresAt = new Date(Date.now() + 10 * 60 * 1000).toISOString()
      mockCloudflareWebLoginSessions = [{
        session_id: 'web-test-session',
        device_id: 'test-device',
        cf_origin: mockCloudflareConfig.public_hostname,
        scopes: ['status.read', 'session.read', 'session.respond'],
        issued_at: issuedAt,
        expires_at: expiresAt,
        last_seen_at: issuedAt,
      }]
      return {
        pairing: {
          ok: true,
          device_id: 'test-device',
          cf_origin: mockCloudflareConfig.public_hostname,
          console_origin: mockCloudflareConfig.web_login_console_origin,
          pair_url: `${mockCloudflareConfig.web_login_console_origin}/pair?nonce=test-nonce`,
          nonce: 'test-nonce',
          scopes: ['web_login_pair'],
          issued_at: issuedAt,
          expires_at: expiresAt,
        },
      }
    }
    if (command === 'list_cloudflare_web_login_sessions') {
      return { sessions: mockCloudflareWebLoginSessions }
    }
    if (command === 'revoke_cloudflare_web_login_sessions') {
      const revoked = mockCloudflareWebLoginSessions.length
      mockCloudflareWebLoginSessions = []
      return { ok: true, revoked }
    }
    if (command === 'start_cloudflare_customer_tunnel' || command === 'stop_cloudflare_customer_tunnel') {
      return null
    }
    if (command === 'get_speech_runtime_status') {
      return {
        permissions: {
          microphone: true,
          speech_recognition: true,
          input_monitoring: false,
          accessibility: true,
        },
        owner: {
          fn_listener_owner: true,
          owner_pid: 5311,
          owner_path: '/Applications/iterate.app/Contents/MacOS/iterate',
          lock_path: '/tmp/iterate-fn-owner.lock',
        },
        overlay: {
          window_exists: true,
          window_visible: false,
          listener_ready: true,
          pending_toggle: false,
        },
        speech: {
          active: false,
          last_partial_length: 12,
          last_final_length: 18,
        },
        writeback: {
          last_target_bundle_id: 'com.openai.codex',
          last_paste_status: 'paste-dispatched-unverified',
          last_error: null,
        },
        diagnostics: {
          log_path: '/tmp/iterate-native-speech.log',
          last_event: 'paste-dispatched-unverified',
          last_event_at: new Date().toISOString(),
        },
      }
    }
    if ([
      'microphone_status',
      'speech_recognition_status',
      'accessibility_status',
    ].includes(command)) {
      return true
    }
    if (command === 'input_monitoring_status') {
      return false
    }
    if (command === 'get_captured_target_app_bundle_id') {
      return 'com.openai.codex'
    }
    if ([
      'request_microphone_permission',
      'request_speech_recognition_permission',
      'request_input_monitoring_permission',
      'request_accessibility_permission',
      'reveal_speech_overlay_window',
      'hide_speech_overlay_window',
      'stop_native_speech',
      'mark_speech_overlay_ready',
      'mark_speech_overlay_unready',
    ].includes(command)) {
      return true
    }
    if (command === 'activate_license') {
      await new Promise(resolve => setTimeout(resolve, 250))
      return { ok: true }
    }
    if (command === 'open_external_url') {
      const url = typeof args?.url === 'string' ? args.url : ''
      if (url)
        window.open(url, '_blank', 'noopener,noreferrer')
      return { ok: true }
    }
    if (command === 'open_local_path' || command === 'open_confirmed_external_file') {
      ;(window as any).__lastOpenLocalPath = args
      window.localStorage.setItem('__lastOpenLocalPath', JSON.stringify(args ?? null))
      document.documentElement.setAttribute(
        'data-last-open-local-path',
        JSON.stringify(args ?? null),
      )
      return { ok: true }
    }
    if (command === 'get_hui_suggestion_terms') {
      return [
        { key: 'ji', description: 'hui 高频词' },
        { key: 'hui', description: 'hui 高频词' },
        { key: 'localhost', description: 'hui 高频词' },
        { key: 'cunzhiknowledge', description: 'hui 高频词' },
        { key: 'global_rules.md', description: 'hui 高频词' },
        { key: 'index.md', description: 'hui 高频词' },
        { key: 'context.md', description: 'hui 高频词' },
        { key: 'progress.md', description: 'hui 高频词' },
        { key: 'memories', description: 'hui 高频词' },
        { key: 'skills', description: 'hui 高频词' },
        { key: 'prompts', description: 'hui 高频词' },
      ]
    }
    if (command === 'load_ghost_suggestions_file') {
      return '{"version":1,"defaultSeedVersion":0,"updatedAt":"","suggestions":[]}'
    }
    if (command === 'save_ghost_suggestions_file') {
      return null
    }
    if (command === 'get_auto_checkpoint_enabled') {
      return true
    }
    if (command === 'set_auto_checkpoint_enabled') {
      return null
    }
    return {}
  }

  window.__TAURI_INTERNALS__ = {
    ...(window.__TAURI_INTERNALS__ || {}),
    invoke: mockInvoke,
    transformCallback: (() => 0) as any,
    unregisterCallback: (() => {}) as any,
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { label: 'main' },
    },
  }

  window.__TAURI__ = {
    core: {
      invoke: mockInvoke,
    },
  }

  const originalFetch = window.fetch.bind(window)
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string'
      ? input
      : input instanceof URL
        ? input.toString()
        : input.url

    if (url.includes('/api/android/pairing/status')) {
      return new Response(JSON.stringify({
        ok: true,
        formal_route: {
          configured: true,
          transport: 'aliyun_ssh_reverse_tunnel',
          base_url: 'https://8.129.82.226:8443',
          health: 'healthy',
          endpoint_identity_ok: true,
        },
        candidates: [{
          transport_mode: 'public_tunnel',
          base_url: 'https://8.129.82.226:8443',
          ws_url: 'wss://8.129.82.226:8443/ws',
          health: 'healthy',
          disabled: false,
        }],
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    if (url.endsWith('/api/android/pairing')) {
      const issuedAt = new Date()
      const expiresAt = new Date(issuedAt.getTime() + 10 * 60 * 1000)
      return new Response(JSON.stringify({
        ok: true,
        pairing: {
          version: 2,
          pairing_session_id: 'preview-session',
          device_id: 'preview-desktop',
          device_name: 'Preview Desktop',
          transport_mode: 'public_tunnel',
          base_url: 'https://8.129.82.226:8443',
          ws_url: 'wss://8.129.82.226:8443/ws',
          candidates: [{
            transport_mode: 'public_tunnel',
            base_url: 'https://8.129.82.226:8443',
            ws_url: 'wss://8.129.82.226:8443/ws',
            health: 'healthy',
            disabled: false,
          }],
          pairing_token: 'preview-one-use-token',
          issued_at: issuedAt.toISOString(),
          expires_at: expiresAt.toISOString(),
        },
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    if (url.includes('/api/android/pairing/sessions/')) {
      return new Response(JSON.stringify({
        ok: true,
        session: {
          session_id: 'preview-session',
          state: 'pending',
          expires_at: new Date(Date.now() + 10 * 60 * 1000).toISOString(),
        },
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    if (url.includes('/api/connection-status')) {
      return new Response(JSON.stringify({
        ok: true,
        generated_at: new Date().toISOString(),
        sessions: {
          active_registry_count: 2,
          live_window_count: 1,
        },
        caches: {
          mcp_state_count: 8,
          mcp_action_count: 0,
          mcp_state_ttl_secs: 21600,
          mcp_action_ttl_secs: 1800,
          mcp_state_max_entries: 512,
          mcp_action_max_entries: 256,
          mcp_state: {
            count: 8,
            ttl_secs: 21600,
            max_entries: 512,
            metrics: {
              lookups: 12,
              hits: 9,
              misses: 3,
              hit_rate_percent: 75,
              writes: 24,
              pruned: 1,
              active_registry_fallback_hits: 2,
              routes: {
                request_id: { lookups: 9, hits: 8, misses: 1, hit_rate_percent: 88.89 },
                project_path: { lookups: 2, hits: 1, misses: 1, hit_rate_percent: 50 },
                fallback_route: { lookups: 1, hits: 0, misses: 1, hit_rate_percent: 0 },
              },
            },
          },
          mcp_action: {
            count: 0,
            ttl_secs: 1800,
            max_entries: 256,
            metrics: {
              lookups: 4,
              hits: 3,
              misses: 1,
              hit_rate_percent: 75,
              writes: 3,
              pruned: 0,
              active_registry_fallback_hits: 0,
              routes: {
                request_id: { lookups: 4, hits: 3, misses: 1, hit_rate_percent: 75 },
                project_path: { lookups: 0, hits: 0, misses: 0, hit_rate_percent: 0 },
                fallback_route: { lookups: 0, hits: 0, misses: 0, hit_rate_percent: 0 },
              },
            },
          },
        },
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    return originalFetch(input, init)
  }
}

// 创建 Vue 应用
const app = createApp(TestApp)

// 使用 Naive UI
app.use(naive)

// 挂载应用
app.mount('#app')

// 初始化主题
const { loadTheme } = useTheme()
loadTheme().catch(() => {
  console.log('使用默认主题')
})
