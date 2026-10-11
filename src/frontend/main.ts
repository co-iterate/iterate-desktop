type TauriInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>

let invokePromise: Promise<TauriInvoke> | null = null

function loadInvoke(): Promise<TauriInvoke> {
  if (!invokePromise) {
    invokePromise = import('@tauri-apps/api/core')
      .then(module => module.invoke as TauriInvoke)
  }

  return invokePromise
}

function formatError(error: unknown): string {
  if (error instanceof Error)
    return error.stack || `${error.name}: ${error.message}`

  try {
    return JSON.stringify(error, null, 2)
  }
  catch {
    return String(error)
  }
}

function escapeHtml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
}

async function reportBoot(stage: string, error?: unknown) {
  const details = error ? ` ${formatError(error)}` : ''
  const message = `[FrontendBoot] ${stage}${details}`

  console.error(message)
  try {
    const invoke = await loadInvoke()
    await invoke('debug_log', { message })
  }
  catch {
    // 启动失败时不能再依赖后端日志成功，避免二次报错遮住真实问题。
  }
}

function renderBootFailure(title: string, error: unknown) {
  const root = document.querySelector('#app')
  if (!root)
    return

  const detail = escapeHtml(formatError(error))

  root.innerHTML = `
    <div style="min-height:100vh;padding:24px;background:#f6f7f9;color:#111827;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;box-sizing:border-box;">
      <div style="max-width:840px;margin:0 auto;background:#ffffff;border:1px solid #d1d5db;border-radius:8px;padding:20px;box-shadow:0 8px 24px rgba(0,0,0,.08);">
        <div style="font-size:20px;font-weight:700;margin-bottom:8px;">${escapeHtml(title)}</div>
        <div style="font-size:14px;line-height:1.7;color:#374151;margin-bottom:12px;">
          前端没有正常完成启动，所以这次不再保持纯白窗口。请把下面这段错误留给我继续定位。
        </div>
        <pre style="margin:0;white-space:pre-wrap;word-break:break-word;background:#111827;color:#f9fafb;border-radius:8px;padding:14px;font-size:12px;line-height:1.6;overflow:auto;">${detail}</pre>
      </div>
    </div>
  `
}

function renderBootFailureIfEmpty(title: string, error: unknown) {
  const root = document.querySelector('#app')
  if (root && root.childElementCount === 0)
    renderBootFailure(title, error)
}

window.addEventListener('error', (event) => {
  const error = event.error ?? event.message
  void reportBoot('window.error', error)
  renderBootFailureIfEmpty('前端启动失败', error)
})

window.addEventListener('unhandledrejection', (event) => {
  void reportBoot('window.unhandledrejection', event.reason)
  renderBootFailureIfEmpty('前端启动失败', event.reason)
})

async function bootstrap() {
  const [
    ,
    ,
    vue,
    naiveUi,
    appModule,
    fatalErrorModule,
  ] = await Promise.all([
    import('virtual:uno.css'),
    import('./assets/styles/style.css'),
    import('vue'),
    import('naive-ui'),
    import('./App.vue'),
    import('./utils/mcpFatalError'),
  ])

  const naive = naiveUi.create({
    components: [
      naiveUi.NAlert,
      naiveUi.NButton,
      naiveUi.NCard,
      naiveUi.NCheckbox,
      naiveUi.NCheckboxGroup,
      naiveUi.NCollapse,
      naiveUi.NCollapseItem,
      naiveUi.NCollapseTransition,
      naiveUi.NConfigProvider,
      naiveUi.NDrawer,
      naiveUi.NDrawerContent,
      naiveUi.NDialogProvider,
      naiveUi.NForm,
      naiveUi.NFormItem,
      naiveUi.NDynamicInput,
      naiveUi.NMessageProvider,
      naiveUi.NModal,
      naiveUi.NNotificationProvider,
      naiveUi.NSpace,
      naiveUi.NSpin,
      naiveUi.NStep,
      naiveUi.NSteps,
      naiveUi.NSwitch,
      naiveUi.NTab,
      naiveUi.NTabPane,
      naiveUi.NTabs,
      naiveUi.NInput,
      naiveUi.NInputGroup,
      naiveUi.NInputNumber,
      naiveUi.NSelect,
      naiveUi.NEmpty,
      naiveUi.NTooltip,
      naiveUi.NIcon,
      naiveUi.NImage,
      naiveUi.NImageGroup,
      naiveUi.NGrid,
      naiveUi.NGridItem,
      naiveUi.NScrollbar,
      naiveUi.NCode,
      naiveUi.NTag,
      naiveUi.NSkeleton,
      naiveUi.NProgress,
      naiveUi.NVirtualList,
      naiveUi.NRadio,
      naiveUi.NRadioGroup,
    ],
  })

  const app = vue.createApp(appModule.default)
  fatalErrorModule.registerMcpFatalErrorHandler(app)
  const previousErrorHandler = app.config.errorHandler
  app.config.errorHandler = (error: unknown, instance: any, info: string) => {
    const owner = instance?.$options?.name || 'anonymous'
    void reportBoot(`vue.errorHandler ${info} @ ${owner}`, error)
    previousErrorHandler?.(error, instance, info)
  }

  app.use(naive)
  app.mount('#app')
  void reportBoot('bootstrap.ok')
}

void bootstrap().catch((error) => {
  void reportBoot('bootstrap.failed', error)
  renderBootFailure('前端启动失败', error)
})
