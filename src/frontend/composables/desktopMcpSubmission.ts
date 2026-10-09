/** Local Vue event task, never part of the MCP/Bridge wire response. */
export interface DesktopMcpSubmission {
  kind: 'desktop_mcp_submission'
  requestId: string
  request: any
  prepare: (isCurrent: () => boolean) => Promise<any>
  settled: () => void
}

export function isDesktopMcpSubmission(value: any): value is DesktopMcpSubmission {
  return value?.kind === 'desktop_mcp_submission' && typeof value.requestId === 'string'
    && value.request?.id === value.requestId && typeof value.prepare === 'function' && typeof value.settled === 'function'
}

export function isDesktopSubmissionCurrent(current: any, request: any): boolean {
  return !!request?.id && current?.id === request.id
    && current?.project_path === request.project_path && current?.source === request.source
}

interface SubmissionSteps {
  dismiss: () => Promise<void>
  isCurrent: () => boolean
  prepare: () => Promise<any>
  deliver: (response: any) => Promise<void>
  accepted: () => void
  cleanup: () => Promise<void>
  failed: (error: unknown) => Promise<void>
  cleanupFailed: (error: unknown) => void
}

/** Keep the source/draft alive through preparation and the actual delivery ACK. */
export async function runDesktopMcpSubmission(steps: SubmissionSteps): Promise<void> {
  let accepted = false
  try {
    await steps.dismiss()
    if (!steps.isCurrent())
      return
    const response = await steps.prepare()
    if (!steps.isCurrent())
      return
    await steps.deliver(response)
    accepted = true
    steps.accepted()
    await steps.cleanup()
  }
  catch (error) {
    if (accepted)
      steps.cleanupFailed(error)
    else if (steps.isCurrent())
      await steps.failed(error)
  }
}
