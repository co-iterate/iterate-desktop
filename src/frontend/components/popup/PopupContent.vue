<script setup lang="ts">
import type { McpRequest, PopupArtifact, PopupFileAttachment, PopupTextSelection, PopupTextSelectionSource } from '../../types/popup'
import { invoke } from '@tauri-apps/api/core'
import hljs from 'highlight.js'
import githubDarkHighlightStyleUrl from 'highlight.js/styles/github-dark.css?url'
import githubHighlightStyleUrl from 'highlight.js/styles/github.css?url'
import MarkdownIt from 'markdown-it'
import mermaid from 'mermaid'
import { NImagePreview, useDialog, useMessage } from 'naive-ui'
import { computed, nextTick, onMounted, onUnmounted, onUpdated, ref, shallowRef, watch } from 'vue'
import { buildOpenLocalPathInvokeArgs, isOutsideCurrentProject, isPotentialLocalMarkdownHref, resolveLocalMarkdownHref } from '../../utils/localMarkdownLinks'
import { normalizePopupSelectionFragments, normalizePopupSelectionText } from '../../utils/popupSelectionQuote'
import HtmlArtifactRenderer from './HtmlArtifactRenderer.vue'

const props = withDefaults(defineProps<Props>(), {
  loading: false,
  currentTheme: 'dark',
})

const emit = defineEmits<Emits>()

// 预处理引用内容，移除增强prompt格式标记
function preprocessQuoteContent(content: string): string {
  let processedContent = content

  // 定义需要移除的格式标记
  const markersToRemove = [
    /### BEGIN RESPONSE ###\s*/gi,
    /Here is an enhanced version of the original instruction that is more specific and clear:\s*/gi,
    /<augment-enhanced-prompt>\s*/gi,
    /<\/augment-enhanced-prompt>\s*/gi,
    /### END RESPONSE ###\s*/gi,
  ]

  // 逐个移除格式标记
  markersToRemove.forEach((marker) => {
    processedContent = processedContent.replace(marker, '')
  })

  // 清理多余的空行和首尾空白
  processedContent = processedContent
    .replace(/\n\s*\n\s*\n/g, '\n\n') // 将多个连续空行合并为两个
    .trim() // 移除首尾空白

  return processedContent
}

// 动态导入代码高亮样式，根据主题切换

// 动态加载代码高亮样式
function loadHighlightStyle(theme: string) {
  // 移除现有的highlight.js样式
  const existingStyle = document.querySelector('link[data-highlight-theme]')
  if (existingStyle) {
    existingStyle.remove()
  }

  // 动态创建样式链接
  const link = document.createElement('link')
  link.rel = 'stylesheet'
  link.href = theme === 'light' ? githubHighlightStyleUrl : githubDarkHighlightStyleUrl
  link.setAttribute('data-highlight-theme', theme)
  document.head.appendChild(link)
}

interface Props {
  request: McpRequest | null
  loading?: boolean
  currentTheme?: string
  browserAiResponse?: string | null
}

interface Emits {
  quoteMessage: [message: string]
  addFiles: [files: PopupFileAttachment[]]
  openArtifact: [artifact: PopupArtifact]
  textSelection: [selection: PopupTextSelection | null]
}

const selectionSurfaceRef = ref<HTMLElement | null>(null)
const requestContentRef = ref<HTMLElement | null>(null)
const browserResponseRef = ref<HTMLElement | null>(null)
const previewImageSrc = ref<string | null>(null)
const dialog = useDialog()
const platform = navigator.platform.toUpperCase()
const isWindows = platform.includes('WIN')
const fileManager = isWindows ? '资源管理器' : platform.includes('MAC') ? 'Finder' : '文件管理器'

interface ManagedSelectionRange {
  range: Range
  source: PopupTextSelectionSource
}

const managedSelectionRanges = shallowRef<ManagedSelectionRange[]>([])
const hasManagedSelection = computed(() => managedSelectionRanges.value.length > 0)
const selectedFragmentCount = computed(() => managedSelectionRanges.value.length)
const copyActionLabel = computed(() => hasManagedSelection.value ? `复制选中 (${selectedFragmentCount.value})` : '复制原文')
const quoteActionLabel = computed(() => hasManagedSelection.value ? `引用选中 (${selectedFragmentCount.value})` : '引用原文')
let appendSelectionOnPointerUp = false

// 本地的浏览器 AI 回复状态（直接从 props.request 获取）
const EMPTY_MESSAGE_FALLBACK = '已暂停，等待你的下一步指令。'
const localBrowserAiResponse = computed(() => props.request?.browser_ai_response || null)
const rawRequestMessage = computed(() => props.request?.message ?? '')
const messageForDisplay = computed(() => {
  return rawRequestMessage.value.trim() ? rawRequestMessage.value : EMPTY_MESSAGE_FALLBACK
})
const parsedArtifactPayload = computed(() => parseHtmlArtifactBlocks(rawRequestMessage.value))
const displayMessage = computed(() => {
  if (!props.request)
    return ''

  if (!parsedArtifactPayload.value.artifacts.length)
    return messageForDisplay.value

  return parsedArtifactPayload.value.cleanedMessage.trim() || 'HTML Artifact 已附加，点击下方打开。'
})
const htmlArtifacts = computed(() => {
  const requestArtifacts = props.request?.artifacts?.filter(artifact => artifact.type === 'html') ?? []
  return [...requestArtifacts, ...parsedArtifactPayload.value.artifacts]
})

function artifactCardKey(artifact: PopupArtifact, index: number) {
  return `${artifact.type}:${artifact.title || 'untitled'}:${index}`
}

function artifactCardLabel(artifact: PopupArtifact) {
  return `打开 HTML Artifact：${artifact.title || '未命名'}`
}

function parseArtifactAttributes(source: string): Record<string, string> {
  const attrs: Record<string, string> = {}
  const attrPattern = /(\w+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s}]+))/g
  let match: RegExpExecArray | null

  while (true) {
    match = attrPattern.exec(source)
    if (!match)
      break

    attrs[match[1]] = match[2] ?? match[3] ?? match[4] ?? ''
  }

  return attrs
}

function parseHtmlArtifactBlocks(message: string): { cleanedMessage: string, artifacts: PopupArtifact[] } {
  const artifacts: PopupArtifact[] = []
  const cleanedParts: string[] = []
  let cursor = 0

  while (cursor < message.length) {
    const start = message.indexOf('::::artifact{', cursor)
    if (start < 0) {
      cleanedParts.push(message.slice(cursor))
      break
    }

    const attrsStart = start + '::::artifact{'.length
    const attrsEnd = message.indexOf('}', attrsStart)
    const contentStart = attrsEnd >= 0 ? message.indexOf('\n', attrsEnd) : -1
    if (attrsEnd < 0 || contentStart < 0) {
      cleanedParts.push(message.slice(cursor))
      break
    }

    const close = message.indexOf('\n::::', contentStart + 1)
    if (close < 0) {
      cleanedParts.push(message.slice(cursor))
      break
    }

    cleanedParts.push(message.slice(cursor, start))

    const rawAttrs = message.slice(attrsStart, attrsEnd)
    const rawContent = message.slice(contentStart + 1, close)
    const attrs = parseArtifactAttributes(rawAttrs)
    if (attrs.type === 'html') {
      artifacts.push({
        type: 'html',
        title: attrs.title || `HTML Artifact ${artifacts.length + 1}`,
        description: attrs.description,
        content: rawContent.trim(),
      })
    }

    const closeLineEnd = message.indexOf('\n', close + 1)
    cursor = closeLineEnd < 0 ? message.length : closeLineEnd + 1
  }

  return { cleanedMessage: cleanedParts.join(''), artifacts }
}

const message = useMessage()

function selectionTextInside(container: HTMLElement | null): string {
  if (!container || typeof window === 'undefined')
    return ''

  const selection = window.getSelection()
  if (!selection || selection.isCollapsed || selection.rangeCount === 0)
    return ''

  const range = selection.getRangeAt(0)
  const commonAncestor = range.commonAncestorContainer
  if (!container.contains(commonAncestor.nodeType === Node.ELEMENT_NODE ? commonAncestor as Element : commonAncestor.parentElement))
    return ''

  return normalizePopupSelectionText(selection.toString())
}

function sourceContainer(source: PopupTextSelectionSource): HTMLElement | null {
  return source === 'request' ? requestContentRef.value : browserResponseRef.value
}

function selectedElementForRange(range: Range): Element | null {
  const commonAncestor = range.commonAncestorContainer
  return commonAncestor.nodeType === Node.ELEMENT_NODE
    ? commonAncestor as Element
    : commonAncestor.parentElement
}

function rangeIsInsideSource(range: Range, source: PopupTextSelectionSource): boolean {
  const container = sourceContainer(source)
  const selectedElement = selectedElementForRange(range)
  return !!container && !!selectedElement && container.contains(selectedElement)
}

function rangesAreEqual(left: Range, right: Range): boolean {
  return left.startContainer === right.startContainer
    && left.startOffset === right.startOffset
    && left.endContainer === right.endContainer
    && left.endOffset === right.endOffset
}

function sortManagedRanges(ranges: ManagedSelectionRange[]): ManagedSelectionRange[] {
  return [...ranges].sort((left, right) => {
    if (left.source !== right.source)
      return left.source === 'request' ? -1 : 1

    try {
      return left.range.compareBoundaryPoints(0, right.range)
    }
    catch {
      return 0
    }
  })
}

function resolveManagedTextSelection(): PopupTextSelection | null {
  if (!managedSelectionRanges.value.length)
    return null

  const sortedRanges = sortManagedRanges(managedSelectionRanges.value)
  const text = normalizePopupSelectionFragments(sortedRanges.map(item => item.range.toString()))
  if (!text)
    return null

  return { text, source: sortedRanges[0].source }
}

function clearManagedSelection(removeNativeSelection = true) {
  managedSelectionRanges.value = []
  appendSelectionOnPointerUp = false
  emit('textSelection', null)

  if (removeNativeSelection)
    window.getSelection()?.removeAllRanges()
}

function handleSelectionStart(event: MouseEvent, source: PopupTextSelectionSource) {
  if (event.button !== 0)
    return

  const target = event.target as Element | null
  if (target?.closest('img')) {
    event.preventDefault()
    return
  }

  if (event.currentTarget instanceof HTMLElement)
    event.currentTarget.focus({ preventScroll: true })

  const sourceChanged = managedSelectionRanges.value.some(item => item.source !== source)
  if (!event.metaKey || sourceChanged)
    clearManagedSelection()
  appendSelectionOnPointerUp = event.metaKey && !sourceChanged
}

function captureTextSelection(source: PopupTextSelectionSource) {
  const selection = window.getSelection()
  const capturedRanges: ManagedSelectionRange[] = []

  if (selection && !selection.isCollapsed) {
    for (let index = 0; index < selection.rangeCount; index += 1) {
      const range = selection.getRangeAt(index)
      if (!range.collapsed && rangeIsInsideSource(range, source)) {
        capturedRanges.push({ range: range.cloneRange(), source })
      }
    }
  }

  const nextRanges = appendSelectionOnPointerUp
    ? managedSelectionRanges.value.filter(item => item.source === source)
    : []

  for (const item of capturedRanges) {
    if (!nextRanges.some(existing => rangesAreEqual(existing.range, item.range)))
      nextRanges.push(item)
  }

  managedSelectionRanges.value = sortManagedRanges(nextRanges)
  appendSelectionOnPointerUp = false

  emit('textSelection', resolveManagedTextSelection())
}

function handleRequestTextSelection() {
  captureTextSelection('request')
}

function handleBrowserTextSelection() {
  captureTextSelection('browser_ai')
}

function resolveCurrentTextSelection(): PopupTextSelection | null {
  const managedSelection = resolveManagedTextSelection()
  if (managedSelection)
    return managedSelection

  const requestText = selectionTextInside(requestContentRef.value)
  if (requestText)
    return { text: requestText, source: 'request' }

  const browserText = selectionTextInside(browserResponseRef.value)
  if (browserText)
    return { text: browserText, source: 'browser_ai' }

  return null
}

function activeElementIsEditable(): boolean {
  const activeElement = document.activeElement
  return activeElement instanceof HTMLElement
    && (activeElement.matches('input, textarea, select') || activeElement.isContentEditable)
}

function handleDocumentCopy(event: ClipboardEvent) {
  const selection = resolveManagedTextSelection()
  if (!selection || activeElementIsEditable() || !event.clipboardData)
    return

  event.preventDefault()
  event.clipboardData.setData('text/plain', selection.text)
  message.success(`已复制 ${selectedFragmentCount.value} 个选中片段`)
}

function handleDocumentMouseDown(event: MouseEvent) {
  const target = event.target
  if (!(target instanceof Node) || selectionSurfaceRef.value?.contains(target))
    return

  clearManagedSelection()
}

// 复制原文或当前受控多选内容到剪贴板
async function copyMessage() {
  const managedSelection = resolveManagedTextSelection()
  if (managedSelection) {
    try {
      await navigator.clipboard.writeText(managedSelection.text)
      message.success(`已复制 ${selectedFragmentCount.value} 个选中片段`)
    }
    catch {
      message.error('复制失败')
    }
    return
  }

  if (props.request) {
    try {
      const processedContent = preprocessQuoteContent(messageForDisplay.value)
      await navigator.clipboard.writeText(processedContent)
      message.success('原文已复制到剪贴板')
    }
    catch {
      message.error('复制失败')
    }
  }
}

// 引用当前受控多选内容；无选区时保持原来的整篇引用行为
function quoteMessage() {
  const managedSelection = resolveManagedTextSelection()
  if (managedSelection) {
    emit('quoteMessage', managedSelection.text)
    return
  }

  if (props.request) {
    const processedContent = preprocessQuoteContent(messageForDisplay.value)
    emit('quoteMessage', processedContent)
  }
}

// 使用原生 Finder 打开路径选择器，支持文件和文件夹
async function openNativeFileSelector() {
  const projectPath = props.request?.project_path

  try {
    const selected = await invoke('select_files_and_folders', {
      defaultPath: projectPath || null,
    }) as string[]

    if (selected && selected.length > 0) {
      emit('addFiles', selected.map(path => ({
        path,
        name: path.split('/').filter(Boolean).pop() || path,
      })))
      message.success(`已添加 ${selected.length} 个路径`)
    }
  }
  catch (error) {
    console.error('打开文件选择器失败:', error)
    message.error('打开文件选择器失败')
  }
}

// Mermaid 图表计数器（用于生成唯一 ID）
let mermaidCounter = 0

function escapeHtml(value: string) {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

function isLocalImagePath(src: string): boolean {
  return src.startsWith('/') || src.startsWith('file://')
}

function normalizeLocalImagePath(src: string): string {
  let path = src.startsWith('file://') ? src.replace(/^file:\/\//, '') : src

  for (let i = 0; i < 4; i++) {
    try {
      const decoded = decodeURIComponent(path)
      if (decoded === path)
        break
      path = decoded
    }
    catch {
      break
    }
  }

  return path
}

function isAllowedExternalUrl(url: string): boolean {
  try {
    const parsedUrl = new URL(url)
    return ['http:', 'https:', 'mailto:'].includes(parsedUrl.protocol)
  }
  catch {
    return false
  }
}

async function openExternalUrl(url: string) {
  try {
    await invoke('open_external_url', { url })
  }
  catch (error) {
    console.error('打开外部链接失败:', error)
    message.error('打开链接失败')
  }
}

async function openLocalMarkdownHref(href: string, event: MouseEvent) {
  const projectPath = props.request?.project_path
  const target = resolveLocalMarkdownHref(href, projectPath)

  if (!projectPath?.trim()) {
    message.warning('当前请求缺少项目路径，无法打开本地文件')
    return
  }

  if (!target) {
    message.warning('不支持打开此本地链接')
    return
  }

  if (isOutsideCurrentProject(target, projectPath)) {
    if (isWindows) {
      await openConfirmedExternalLocalFile(target.path)
      return
    }
    dialog.warning({
      title: '打开跨项目文件或文件夹？',
      content: `将在${fileManager}中打开文件夹或定位文件，不会执行文件或交给编辑器：\n${target.path}`,
      positiveText: `在${fileManager}中打开`,
      negativeText: '取消',
      onPositiveClick: () => {
        void openConfirmedExternalLocalFile(target.path)
      },
    })
    return
  }

  try {
    await invoke('open_local_path', buildOpenLocalPathInvokeArgs(target, projectPath, event))
  }
  catch (error) {
    console.error('打开本地文件失败:', error)
    message.error(`打开本地文件失败: ${error}`)
  }
}

async function openConfirmedExternalLocalFile(path: string) {
  try {
    await invoke('open_confirmed_external_file', { path })
  }
  catch (error) {
    console.error('打开跨项目文件失败:', error)
    message.error(`打开跨项目文件失败: ${error}`)
  }
}

function closeImagePreview() {
  previewImageSrc.value = null
}

function handleImagePreviewShowChange(show: boolean) {
  if (!show)
    closeImagePreview()
}

function handleMarkdownContentClick(event: MouseEvent) {
  const target = event.target as Element | null
  const image = target?.closest('img') as HTMLImageElement | null
  if (image) {
    event.preventDefault()
    previewImageSrc.value = image.currentSrc || image.src
    return
  }

  const anchor = target?.closest('a[href]') as HTMLAnchorElement | null
  if (!anchor)
    return

  const href = anchor.getAttribute('href') || ''
  if (!href)
    return

  if (isPotentialLocalMarkdownHref(href)) {
    event.preventDefault()
    void openLocalMarkdownHref(href, event)
    return
  }

  if (!isAllowedExternalUrl(href)) {
    if (!href.startsWith('#')) {
      event.preventDefault()
      message.warning('不支持打开此链接')
    }
    return
  }

  event.preventDefault()
  void openExternalUrl(href)
}

// 创建 Markdown 实例 - 保持代码高亮功能
const md = new MarkdownIt({
  html: false,
  xhtmlOut: false,
  breaks: true,
  langPrefix: 'language-',
  linkify: true,
  typographer: true,
  quotes: '""\'\'',
  highlight(str: string, lang: string) {
    // Mermaid 图表特殊处理
    if (lang === 'mermaid') {
      const id = `mermaid-${Date.now()}-${mermaidCounter++}`
      return `<div class="mermaid-container"><pre class="mermaid" id="${id}">${escapeHtml(str)}</pre></div>`
    }
    if (lang && hljs.getLanguage(lang)) {
      try {
        return hljs.highlight(str, { language: lang }).value
      }
      catch {
        // 忽略错误
      }
    }
    return ''
  },
})

const defaultValidateLink = md.validateLink.bind(md)
md.validateLink = (url: string) => {
  return isPotentialLocalMarkdownHref(url) || defaultValidateLink(url)
}

// 自定义图片渲染器 - 本地路径使用占位符，异步加载 base64
md.renderer.rules.image = function (tokens, idx) {
  const token = tokens[idx]
  const src = token.attrGet('src') || ''
  const alt = token.content || ''
  const title = token.attrGet('title') || ''
  const escapedSrc = escapeHtml(src)
  const escapedAlt = escapeHtml(alt)
  const titleAttr = title ? ` title="${escapeHtml(title)}"` : ''
  const imgStyle = 'display: block; max-width: 100%; height: auto; border-radius: 8px; margin: 8px 0; cursor: zoom-in;'

  if (isLocalImagePath(src)) {
    const localPath = normalizeLocalImagePath(src)
    const escapedLocalPath = escapeHtml(localPath)
    const uid = `local-img-${Date.now()}-${idx}`
    // 异步加载本地文件为 base64
    invoke('read_file_base64', { path: localPath }).then((result: unknown) => {
      const dataUrl = result as string
      const el = document.getElementById(uid)
      if (el) {
        el.textContent = ''
        const img = document.createElement('img')
        img.src = dataUrl
        img.alt = alt
        img.draggable = false
        img.style.display = 'block'
        img.style.maxWidth = '100%'
        img.style.height = 'auto'
        img.style.borderRadius = '8px'
        img.style.cursor = 'zoom-in'
        el.appendChild(img)
      }
    }).catch((err: unknown) => {
      console.warn('加载本地图片失败:', localPath, err)
      const el = document.getElementById(uid)
      if (el) {
        el.textContent = `❌ 加载失败: ${err}`
      }
    })
    return `<div id="${uid}" style="background:#333;color:#aaa;padding:12px;border-radius:8px;margin:8px 0;font-size:12px;">📷 加载本地图片: ${escapedLocalPath}</div>`
  }

  return `<img src="${escapedSrc}" alt="${escapedAlt}"${titleAttr} style="${imgStyle}" />`
}

interface MarkdownTokenWithAttrs {
  attrGet: (name: string) => string | null
  attrSet: (name: string, value: string) => void
}

function addExternalLinkAttrs(token: MarkdownTokenWithAttrs) {
  const href = token.attrGet('href')
  if (!href || !isAllowedExternalUrl(href))
    return

  token.attrSet('target', '_blank')
  token.attrSet('rel', 'noopener noreferrer')
  token.attrSet('data-external-url', href)
  token.attrSet('title', `打开链接: ${href}`)
}

function addLocalLinkAttrs(token: MarkdownTokenWithAttrs) {
  const href = token.attrGet('href')
  if (!href || !isPotentialLocalMarkdownHref(href))
    return

  token.attrSet('data-local-file-link', 'true')
  token.attrSet('title', `打开文件: ${href}`)
}

// 自定义链接渲染器 - 外链保留 href，由外层点击处理交给系统浏览器打开
md.renderer.rules.link_open = function (tokens, idx, options, env, renderer) {
  const token = tokens[idx]
  addExternalLinkAttrs(token)
  addLocalLinkAttrs(token)
  return renderer.renderToken(tokens, idx, options)
}

// 自定义自动链接渲染器 - 处理 linkify 生成的链接
md.renderer.rules.autolink_open = function (tokens, idx, options, env, renderer) {
  const token = tokens[idx]
  addExternalLinkAttrs(token)
  addLocalLinkAttrs(token)
  return renderer.renderToken(tokens, idx, options)
}

// Markdown 渲染函数
function renderMarkdown(content: string) {
  try {
    // 将字面量 \n 转换为实际换行符（AI 有时会发送转义的换行符）
    const normalizedContent = content.replace(/\\n/g, '\n')
    return md.render(normalizedContent)
  }
  catch (error) {
    console.error('Markdown 渲染失败:', error)
    return escapeHtml(content)
  }
}

const renderedDisplayMessage = computed(() => renderMarkdown(displayMessage.value))
const renderedBrowserAiResponse = computed(() => {
  return localBrowserAiResponse.value ? renderMarkdown(localBrowserAiResponse.value) : ''
})

// 创建复制按钮
function createCopyButton(preEl: Element) {
  // 检查是否已经有复制按钮
  if (preEl.querySelector('.copy-button'))
    return

  const copyButton = document.createElement('div')
  copyButton.className = 'copy-button'
  // 极简设计：无背景，无边框
  copyButton.style.cssText = `
    position: absolute;
    top: 8px;
    right: 8px;
    z-index: 1000;
    opacity: 1;
    transition: opacity 0.2s ease;
    pointer-events: auto;
    height: 20px;
    width: 20px;
    display: flex;
    align-items: center;
    justify-content: center;
  `

  copyButton.innerHTML = `
    <button style="
      display: flex;
      align-items: center;
      justify-content: center;
      width: 100%;
      height: 100%;
      color: #9ca3af;
      transition: color 0.2s ease;
      border: none;
      background: none;
      cursor: pointer;
      padding: 0;
      margin: 0;
    " onmouseover="this.style.color='#14b8a6'" onmouseout="this.style.color='#9ca3af'">
      <div class="i-carbon-copy" style="width: 16px; height: 16px; display: block;"></div>
    </button>
  `

  const button = copyButton.querySelector('button')!
  button.addEventListener('click', async (e) => {
    e.stopPropagation()
    e.preventDefault()
    try {
      const codeEl = preEl.querySelector('code')
      const textContent = codeEl?.textContent || preEl.textContent || ''
      await navigator.clipboard.writeText(textContent)

      // 更新为成功状态
      const icon = button.querySelector('div')!
      icon.className = 'i-carbon-checkmark'
      icon.style.cssText = 'width: 16px; height: 16px; color: #22c55e; display: block;'

      setTimeout(() => {
        icon.className = 'i-carbon-copy'
        icon.style.cssText = 'width: 16px; height: 16px; display: block;'
      }, 2000)
      message.success('代码已复制到剪贴板')
    }
    catch {
      message.error('复制失败')
    }
  })

  // 确保父元素有相对定位和足够的层级
  const preElement = preEl as HTMLElement
  preElement.style.position = 'relative'
  preElement.style.zIndex = '1'

  // 按钮始终显示，不需要悬停事件

  preElement.appendChild(copyButton)
}

// 设置内联代码复制
function setupInlineCodeCopy() {
  const inlineCodeElements = document.querySelectorAll('.markdown-content p code, .markdown-content li code')
  inlineCodeElements.forEach((codeEl) => {
    codeEl.addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(codeEl.textContent || '')
        message.success('代码已复制到剪贴板')
      }
      catch {
        message.error('复制失败')
      }
    })
  })
}

// 设置代码复制功能
let setupCodeCopyTimer: number | null = null
function setupCodeCopy() {
  if (setupCodeCopyTimer) {
    clearTimeout(setupCodeCopyTimer)
  }

  // 增加延迟时间，确保DOM完全渲染
  setupCodeCopyTimer = window.setTimeout(() => {
    nextTick(() => {
      // 确保选择正确的 pre 元素
      const preElements = document.querySelectorAll('.markdown-content pre')
      console.log('设置代码复制按钮，找到', preElements.length, '个代码块')
      preElements.forEach((preEl) => {
        createCopyButton(preEl)
      })
      setupInlineCodeCopy()

      // 如果没有找到代码块，再次尝试
      if (preElements.length === 0) {
        setTimeout(() => {
          const retryElements = document.querySelectorAll('.markdown-content pre')
          console.log('重试设置代码复制按钮，找到', retryElements.length, '个代码块')
          retryElements.forEach((preEl) => {
            createCopyButton(preEl)
          })
        }, 200)
      }
    })
  }, 300)
}

// 监听request变化，重新设置代码复制和 Mermaid 渲染
watch(() => props.request, () => {
  clearManagedSelection()
  previewImageSrc.value = null
  if (props.request) {
    setupCodeCopy()
    renderMermaidDiagrams()
  }
}, { deep: true })

// 监听loading状态变化
watch(() => props.loading, (newLoading) => {
  if (newLoading)
    clearManagedSelection()
  if (!newLoading && props.request) {
    setupCodeCopy()
  }
})

onMounted(() => {
  // 初始化代码高亮样式
  loadHighlightStyle(props.currentTheme)
  document.addEventListener('copy', handleDocumentCopy)
  document.addEventListener('mousedown', handleDocumentMouseDown)
  if (props.request) {
    setupCodeCopy()
    renderMermaidDiagrams()
  }
})

onUnmounted(() => {
  document.removeEventListener('copy', handleDocumentCopy)
  document.removeEventListener('mousedown', handleDocumentMouseDown)
  clearManagedSelection()
  if (setupCodeCopyTimer)
    clearTimeout(setupCodeCopyTimer)
})

// 监听主题变化
watch(() => props.currentTheme, (newTheme) => {
  loadHighlightStyle(newTheme)
}, { immediate: false })

// 渲染 Mermaid 图表
async function renderMermaidDiagrams() {
  await nextTick()
  const mermaidElements = document.querySelectorAll('.mermaid:not([data-processed])')
  if (mermaidElements.length > 0) {
    try {
      // 根据当前主题更新 Mermaid 配置
      mermaid.initialize({
        startOnLoad: false,
        theme: props.currentTheme === 'light' ? 'default' : 'dark',
        securityLevel: 'strict',
        flowchart: {
          htmlLabels: false,
        },
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
      })
      await mermaid.run({ nodes: mermaidElements as NodeListOf<HTMLElement> })
    }
    catch (error) {
      console.error('Mermaid 渲染失败:', error)
    }
  }
}

// 在DOM更新后也尝试设置
onUpdated(() => {
  if (props.request && !props.loading) {
    setupCodeCopy()
    renderMermaidDiagrams()
  }
})

defineExpose({
  openFileMenu: openNativeFileSelector,
  resolveCurrentTextSelection,
})
</script>

<template>
  <div :class="currentTheme === 'light' ? 'text-gray-900' : 'text-white'">
    <!-- 加载状态 -->
    <div v-if="loading" class="flex flex-col items-center justify-center py-8">
      <n-spin size="medium" />
      <p
        class="text-sm mt-3"
        :class="currentTheme === 'light' ? 'text-gray-600' : 'text-white opacity-60'"
      >
        加载中...
      </p>
    </div>

    <!-- 消息显示区域 -->
    <div v-else-if="request" ref="selectionSurfaceRef" class="selection-surface relative">
      <!-- 主要内容 -->
      <HtmlArtifactRenderer
        v-if="request.render_mode === 'html_artifact'"
        :content="displayMessage"
        :current-theme="currentTheme"
      />
      <div
        v-else-if="request.is_markdown"
        ref="requestContentRef"
        class="markdown-content prose prose-sm max-w-none prose-headings:font-semibold prose-headings:leading-tight prose-h1:!mt-4 prose-h1:!mb-2 prose-h1:!text-lg prose-h1:!font-bold prose-h1:!leading-tight prose-h2:!mt-3 prose-h2:!mb-1.5 prose-h2:!text-base prose-h2:!font-semibold prose-h2:!leading-tight prose-h3:!mt-2.5 prose-h3:!mb-1 prose-h3:!text-sm prose-h3:!font-medium prose-h3:!leading-tight prose-h4:!mt-2 prose-h4:!mb-1 prose-h4:!text-sm prose-h4:!font-medium prose-h4:!leading-tight prose-p:my-1 prose-p:leading-relaxed prose-p:text-sm prose-ul:my-1 prose-ul:text-sm prose-ul:pl-4 prose-ol:my-1 prose-ol:text-sm prose-ol:pl-4 prose-li:my-1 prose-li:text-sm prose-li:leading-relaxed prose-blockquote:my-2 prose-blockquote:text-sm prose-blockquote:pl-4 prose-blockquote:ml-0 prose-blockquote:italic prose-blockquote:border-l-4 prose-blockquote:border-primary-500 prose-pre:relative prose-pre:border prose-pre:rounded-lg prose-pre:p-4 prose-pre:my-3 prose-pre:overflow-x-auto scrollbar-code prose-code:px-1 prose-code:py-0.5 prose-code:text-xs prose-code:cursor-pointer prose-code:font-mono prose-a:text-primary-500 prose-a:no-underline prose-a:cursor-pointer hover:prose-a:underline hover:prose-a:underline-offset-2" :class="[
          currentTheme === 'light' ? 'markdown-content--light' : 'markdown-content--dark',
          { 'cross-device-note': request.id.startsWith('cross-') && displayMessage.endsWith('注:跨设备来源，仅支持文本和选项') },
          currentTheme === 'light' ? 'prose-slate' : 'prose-invert',
          currentTheme === 'light' ? 'prose-headings:text-gray-900' : 'prose-headings:text-white',
          currentTheme === 'light' ? 'prose-p:text-gray-700' : 'prose-p:text-white prose-p:opacity-85',
          currentTheme === 'light' ? 'prose-ul:text-gray-700 prose-ol:text-gray-700 prose-li:text-gray-700' : 'prose-ul:text-white prose-ul:opacity-85 prose-ol:text-white prose-ol:opacity-85 prose-li:text-white prose-li:opacity-85',
          currentTheme === 'light' ? 'prose-blockquote:text-gray-600' : 'prose-blockquote:text-gray-300 prose-blockquote:opacity-90',
          currentTheme === 'light' ? 'prose-pre:bg-gray-50 prose-pre:border-gray-200 prose-pre:text-gray-900 prose-code:text-gray-900' : 'prose-pre:bg-black prose-pre:border-gray-700',
          currentTheme === 'light' ? 'prose-strong:text-gray-900 prose-strong:font-semibold' : 'prose-strong:text-white prose-strong:font-semibold',
          currentTheme === 'light' ? 'prose-em:text-gray-600 prose-em:italic' : 'prose-em:text-gray-300 prose-em:italic',
        ]"
        @click="handleMarkdownContentClick"
        @mousedown="handleSelectionStart($event, 'request')"
        @mouseup="handleRequestTextSelection"
        v-html="renderedDisplayMessage"
      />
      <div
        v-else
        ref="requestContentRef"
        class="whitespace-pre-wrap leading-relaxed"
        :class="currentTheme === 'light' ? 'text-gray-800' : 'text-white'"
        @mousedown="handleSelectionStart($event, 'request')"
        @mouseup="handleRequestTextSelection"
      >
        {{ displayMessage }}
      </div>

      <div v-if="htmlArtifacts.length" class="mt-3 grid gap-2">
        <button
          v-for="(artifact, index) in htmlArtifacts"
          :key="artifactCardKey(artifact, index)"
          type="button"
          :aria-label="artifactCardLabel(artifact)"
          class="group w-full rounded-lg border px-3 py-2.5 text-left transition-colors"
          :class="currentTheme === 'light'
            ? 'border-gray-200 bg-gray-50 hover:border-primary-400 hover:bg-white'
            : 'border-white/10 bg-white/5 hover:border-primary-400/70 hover:bg-white/10'"
          @click="emit('openArtifact', artifact)"
        >
          <div class="flex items-center justify-between gap-3">
            <div class="flex min-w-0 items-center gap-2.5">
              <div
                class="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-md"
                :class="currentTheme === 'light' ? 'bg-gray-200 text-gray-700' : 'bg-black/30 text-gray-200'"
              >
                <div class="i-carbon-html w-4 h-4" />
              </div>
              <div class="min-w-0">
                <div class="truncate text-sm font-semibold">
                  {{ artifact.title }}
                </div>
                <div
                  v-if="artifact.description || artifact.path"
                  class="mt-0.5 truncate text-xs"
                  :class="currentTheme === 'light' ? 'text-gray-500' : 'text-gray-400'"
                >
                  {{ artifact.description || artifact.path }}
                </div>
              </div>
            </div>
            <div class="inline-flex flex-shrink-0 items-center gap-1.5 text-xs font-medium text-primary-400">
              <span>打开</span>
              <div class="i-carbon-launch w-3.5 h-3.5 transition-transform group-hover:translate-x-0.5" />
            </div>
          </div>
        </button>
      </div>

      <!-- 浏览器 AI 回复（Web 模式下显示） -->
      <div v-if="localBrowserAiResponse" class="mt-4 pt-4 border-t border-gray-600/30">
        <div
          ref="browserResponseRef"
          class="markdown-content prose prose-sm max-w-none prose-headings:font-semibold prose-headings:leading-tight prose-h1:!mt-4 prose-h1:!mb-2 prose-h1:!text-lg prose-h1:!font-bold prose-h1:!leading-tight prose-h2:!mt-3 prose-h2:!mb-1.5 prose-h2:!text-base prose-h2:!font-semibold prose-h2:!leading-tight prose-h3:!mt-2.5 prose-h3:!mb-1 prose-h3:!text-sm prose-h3:!font-medium prose-h3:!leading-tight prose-h4:!mt-2 prose-h4:!mb-1 prose-h4:!text-sm prose-h4:!font-medium prose-h4:!leading-tight prose-p:my-1 prose-p:leading-relaxed prose-p:text-sm prose-ul:my-1 prose-ul:text-sm prose-ul:pl-4 prose-ol:my-1 prose-ol:text-sm prose-ol:pl-4 prose-li:my-1 prose-li:text-sm prose-li:leading-relaxed prose-blockquote:my-2 prose-blockquote:text-sm prose-blockquote:pl-4 prose-blockquote:ml-0 prose-blockquote:italic prose-blockquote:border-l-4 prose-blockquote:border-primary-500 prose-pre:relative prose-pre:border prose-pre:rounded-lg prose-pre:p-4 prose-pre:my-3 prose-pre:overflow-x-auto scrollbar-code prose-code:px-1 prose-code:py-0.5 prose-code:text-xs prose-code:cursor-pointer prose-code:font-mono prose-a:text-primary-500 prose-a:no-underline prose-a:cursor-pointer hover:prose-a:underline hover:prose-a:underline-offset-2" :class="[
            currentTheme === 'light' ? 'markdown-content--light' : 'markdown-content--dark',
            currentTheme === 'light' ? 'prose-slate' : 'prose-invert',
            currentTheme === 'light' ? 'prose-headings:text-gray-900' : 'prose-headings:text-white',
            currentTheme === 'light' ? 'prose-p:text-gray-700' : 'prose-p:text-white prose-p:opacity-85',
            currentTheme === 'light' ? 'prose-ul:text-gray-700 prose-ol:text-gray-700 prose-li:text-gray-700' : 'prose-ul:text-white prose-ul:opacity-85 prose-ol:text-white prose-ol:opacity-85 prose-li:text-white prose-li:opacity-85',
            currentTheme === 'light' ? 'prose-blockquote:text-gray-600' : 'prose-blockquote:text-gray-300 prose-blockquote:opacity-90',
            currentTheme === 'light' ? 'prose-pre:bg-gray-50 prose-pre:border-gray-200 prose-pre:text-gray-900 prose-code:text-gray-900' : 'prose-pre:bg-black prose-pre:border-gray-700',
            currentTheme === 'light' ? 'prose-strong:text-gray-900 prose-strong:font-semibold' : 'prose-strong:text-white prose-strong:font-semibold',
            currentTheme === 'light' ? 'prose-em:text-gray-600 prose-em:italic' : 'prose-em:text-gray-300 prose-em:italic',
          ]"
          @click="handleMarkdownContentClick"
          @mousedown="handleSelectionStart($event, 'browser_ai')"
          @mouseup="handleBrowserTextSelection"
          v-html="renderedBrowserAiResponse"
        />
      </div>

      <!-- 操作按钮区域 -->
      <div class="flex justify-end items-center mt-4 pt-3 border-t border-gray-600/30" data-guide="quote-message">
        <!-- 右侧：@路径、复制和引用按钮 -->
        <div class="flex gap-2 relative">
          <!-- @路径按钮 - 使用原生 Finder 选择文件或文件夹 -->
          <div
            :title="`打开${fileManager}选择文件或文件夹路径`"
            class="popup-message-action-button inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md cursor-pointer transition-colors duration-100"
            @click="openNativeFileSelector"
          >
            <span>➕ 路径</span>
          </div>
          <div
            :title="hasManagedSelection ? '复制当前不连续选区' : '点击复制 AI 的完整消息内容到剪贴板'"
            class="popup-message-action-button inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md cursor-pointer transition-colors duration-100"
            @click="copyMessage"
          >
            <div class="i-carbon-copy w-3.5 h-3.5" />
            <span>{{ copyActionLabel }}</span>
          </div>
          <div
            :title="hasManagedSelection ? '引用当前不连续选区到输入框' : '点击将 AI 的完整消息内容引用到输入框中'"
            class="popup-message-action-button inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md cursor-pointer transition-colors duration-100"
            @click="quoteMessage"
          >
            <div class="i-carbon-quotes w-3.5 h-3.5" />
            <span>{{ quoteActionLabel }}</span>
          </div>
        </div>
      </div>

      <NImagePreview
        v-if="previewImageSrc"
        :src="previewImageSrc"
        :show="true"
        @close="closeImagePreview"
        @update:show="handleImagePreviewShowChange"
      />
    </div>

    <!-- 错误状态 -->
    <n-alert v-else type="error" title="数据加载错误">
      <div :class="currentTheme === 'light' ? 'text-gray-900' : 'text-white'">
        Request对象: {{ JSON.stringify(request) }}
      </div>
    </n-alert>
  </div>
</template>

<style scoped>
.cross-device-note :deep(> hr:nth-last-child(2)) {
  margin-block: 4px;
}

.cross-device-note :deep(> hr:nth-last-child(2) + p:last-child) {
  margin-block: 0;
  line-height: 1.625;
}

.popup-message-action-button {
  background-color: #ffffff !important;
  border: 1px solid #e5e7eb !important;
  color: #374151 !important;
  box-shadow: 0 1px 2px rgba(15, 23, 42, 0.06);
}

.popup-message-action-button:hover {
  background-color: #f9fafb !important;
  border-color: #d1d5db !important;
  color: #111827 !important;
}

.popup-message-action-button:active {
  background-color: #f3f4f6 !important;
  transform: translateY(1px);
  box-shadow: inset 0 1px 2px rgba(15, 23, 42, 0.08);
}

.markdown-content--light :deep(pre),
.markdown-content--light :deep(pre code) {
  color: #111827 !important;
}

.markdown-content--light :deep(pre) {
  background-color: #f8fafc !important;
}

.markdown-content--light :deep(pre code) {
  background-color: transparent !important;
}

.markdown-content--light :deep(:not(pre) > code) {
  background-color: #e5e7eb !important;
  color: #111827 !important;
}
</style>
