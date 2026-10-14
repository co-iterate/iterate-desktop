import type { ClickModifierState } from './clickModifiers.ts'
import { hasOpenModifier } from './clickModifiers.ts'

export interface LocalMarkdownLinkTarget {
  path: string
}

export interface OpenLocalPathInvokeArgs {
  path: string
  projectPath: string
  preferEditor: boolean
}

function hasScheme(value: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(value)
}

function normalizeWindowsDrivePath(value: string): string {
  // Markdown/file URLs may spell an absolute drive path as /E:/folder/file.
  const path = value.replace(/^[/\\]([a-z]:[/\\])/i, '$1')
  return /^[a-z]:[/\\]/i.test(path) ? path.replace(/\\/g, '/') : path
}

function isWindowsDrivePath(value: string): boolean {
  return /^[a-z]:\//i.test(normalizeWindowsDrivePath(value))
}

function decodeRepeatedly(value: string): string {
  let decoded = value

  for (let i = 0; i < 4; i++) {
    try {
      const next = decodeURIComponent(decoded)
      if (next === decoded)
        break
      decoded = next
    }
    catch {
      break
    }
  }

  return decoded
}

function stripQueryAndHash(value: string): string {
  const hashIndex = value.indexOf('#')
  const withoutHash = hashIndex >= 0 ? value.slice(0, hashIndex) : value
  const queryIndex = withoutHash.indexOf('?')
  return queryIndex >= 0 ? withoutHash.slice(0, queryIndex) : withoutHash
}

function stripWrappingQuotes(value: string): string {
  return value.trim().replace(/^['"]|['"]$/g, '')
}

export function isPotentialLocalMarkdownHref(href: string): boolean {
  const trimmed = stripWrappingQuotes(href)
  if (!trimmed || trimmed.startsWith('#') || trimmed.startsWith('//'))
    return false

  if (isWindowsDrivePath(trimmed))
    return true

  if (hasScheme(trimmed))
    return trimmed.toLowerCase().startsWith('file://')

  return true
}

export function resolveLocalMarkdownHref(
  href: string,
  projectPath: string | null | undefined,
): LocalMarkdownLinkTarget | null {
  const trimmed = stripWrappingQuotes(href)
  if (!isPotentialLocalMarkdownHref(trimmed))
    return null

  if (!projectPath?.trim())
    return null

  let path = stripQueryAndHash(trimmed)

  if (path.toLowerCase().startsWith('file://')) {
    try {
      const url = new URL(path)
      path = url.hostname && url.hostname !== 'localhost' ? `//${url.hostname}${url.pathname}` : url.pathname
    }
    catch {
      path = path.replace(/^file:\/\//i, '')
    }
  }

  path = normalizeWindowsDrivePath(decodeRepeatedly(path))
  if (!path)
    return null

  if (path.startsWith('/') || isWindowsDrivePath(path))
    return { path }

  const normalizedProjectPath = normalizeWindowsDrivePath(projectPath.trim()).replace(/\/+$/, '')
  const normalizedRelativePath = (isWindowsDrivePath(normalizedProjectPath) ? path.replace(/\\/g, '/') : path).replace(/^\.\/+/, '')
  return { path: `${normalizedProjectPath}/${normalizedRelativePath}` }
}

function normalizePathForProjectComparison(path: string): string {
  const withoutEditorLocation = normalizeWindowsDrivePath(path.replace(/:(\d+)(?::\d+)?$/, ''))
  const windows = isWindowsDrivePath(withoutEditorLocation)

  try {
    const normalized = decodeRepeatedly(new URL(`file://${windows ? '/' : ''}${withoutEditorLocation}`).pathname).replace(/\/+$/, '') || '/'
    return windows ? normalized.toLowerCase() : normalized
  }
  catch {
    return withoutEditorLocation.replace(/\/+$/, '') || '/'
  }
}

/**
 * This is only a UI routing decision. The Rust command canonicalizes the path
 * again before opening it, so symlinks cannot bypass the security boundary.
 */
export function isOutsideCurrentProject(
  target: LocalMarkdownLinkTarget,
  projectPath: string,
): boolean {
  const normalizedProjectPath = normalizePathForProjectComparison(projectPath)
  const normalizedTargetPath = normalizePathForProjectComparison(target.path)

  return normalizedTargetPath !== normalizedProjectPath
    && !normalizedTargetPath.startsWith(`${normalizedProjectPath}/`)
}

export function buildOpenLocalPathInvokeArgs(
  target: LocalMarkdownLinkTarget,
  projectPath: string,
  modifiers: ClickModifierState,
): OpenLocalPathInvokeArgs {
  return {
    path: target.path,
    projectPath,
    preferEditor: hasOpenModifier(modifiers),
  }
}
