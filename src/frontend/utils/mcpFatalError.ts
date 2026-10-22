import type { App as VueApp } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { publishCompletedMcpRequest } from '../services/bridgeRequestClosure'

interface ActiveMcpFatalContext {
  isMcpProcess: boolean
  projectPath: string | null
  request: any | null
  requestId: string | null
}

let activeContext: ActiveMcpFatalContext | null = null
let registered = false
let fatalReportInFlight = false

const IGNORED_WINDOW_ERROR_MESSAGES = [
  'ResizeObserver loop completed with undelivered notifications',
  'ResizeObserver loop limit exceeded',
]

function normalizeNonEmpty(value: unknown): string | null {
  if (typeof value !== 'string')
    return null

  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

function resolveRequestId(request: any): string | null {
  return normalizeNonEmpty(request?.id)
    ?? normalizeNonEmpty(request?.request_id)
    ?? normalizeNonEmpty(request?.metadata?.request_id)
    ?? normalizeNonEmpty(request?.metadata?.requestId)
}

function resolveProjectPath(request: any): string | null {
  return normalizeNonEmpty(request?.project_path)
    ?? normalizeNonEmpty(request?.projectPath)
}

function errorName(error: unknown): string {
  return error instanceof Error ? error.name : typeof error
}

function errorMessage(error: unknown): string {
  if (error instanceof Error)
    return error.message || error.name

  if (typeof error === 'string')
    return error

  try {
    return JSON.stringify(error)
  }
  catch {
    return String(error)
  }
}

function errorStack(error: unknown): string | null {
  if (error instanceof Error && typeof error.stack === 'string')
    return error.stack

  return null
}

export function isIgnorableMcpFatalError(error: unknown, fatalSource: string): boolean {
  if (fatalSource !== 'window_error')
    return false

  const message = errorMessage(error).trim()
  return IGNORED_WINDOW_ERROR_MESSAGES.some(ignoredMessage => message.includes(ignoredMessage))
}

export function setActiveMcpFatalContext(request: any, isMcpProcess: boolean) {
  activeContext = {
    request,
    isMcpProcess,
    requestId: resolveRequestId(request),
    projectPath: resolveProjectPath(request),
  }
  fatalReportInFlight = false
}

export function clearActiveMcpFatalContext() {
  activeContext = null
  fatalReportInFlight = false
}

async function reportMcpFatalError(error: unknown, fatalSource: string, info?: string) {
  const context = activeContext
  if (!context?.requestId || fatalReportInFlight)
    return

  if (isIgnorableMcpFatalError(error, fatalSource))
    return

  fatalReportInFlight = true
  const message = errorMessage(error)
  const stack = errorStack(error)
  const userInput = [
    'MCP popup frontend fatal error.',
    '',
    `source: ${fatalSource}`,
    info ? `info: ${info}` : '',
    `error: ${message}`,
    stack ? `stack:\n${stack}` : '',
  ].filter(Boolean).join('\n')

  const response = {
    user_input: userInput,
    selected_options: [],
    images: [],
    metadata: {
      timestamp: new Date().toISOString(),
      request_id: context.requestId,
      source: 'frontend_fatal_error',
      fatal_source: fatalSource,
      fatal_info: info ?? null,
      error_name: errorName(error),
      project_path: context.projectPath,
    },
  }

  try {
    await invoke('send_mcp_response', {
      response,
      projectPath: context.projectPath,
      requestId: context.requestId,
    })
    await publishCompletedMcpRequest(context.requestId, context.projectPath)

    if (context.isMcpProcess) {
      window.setTimeout(() => {
        invoke('exit_app').catch((exitError) => {
          console.error('[MCP] fatal error exit_app failed:', exitError)
        })
      }, 100)
    }
  }
  catch (sendError) {
    console.error('[MCP] failed to report frontend fatal error:', sendError)
  }
}

export function registerMcpFatalErrorHandler(app: VueApp) {
  const previousErrorHandler = app.config.errorHandler
  app.config.errorHandler = (error, instance, info) => {
    void reportMcpFatalError(error, 'vue_error', info)
    previousErrorHandler?.(error, instance, info)
  }

  if (registered || typeof window === 'undefined')
    return

  registered = true
  window.addEventListener('error', (event) => {
    const error = event.error ?? event.message
    if (isIgnorableMcpFatalError(error, 'window_error')) {
      event.preventDefault()
      return
    }

    void reportMcpFatalError(error, 'window_error')
  })
  window.addEventListener('unhandledrejection', (event) => {
    void reportMcpFatalError(event.reason, 'unhandledrejection')
  })
}
