export interface MarkdownLinkToken {
  attrGet: (name: string) => string | null
  attrSet: (name: string, value: string) => void
}

export interface ConsumableLinkClick {
  preventDefault: () => void
  stopPropagation: () => void
}

export function isAllowedExternalUrl(url: string): boolean {
  try {
    const parsedUrl = new URL(url)
    return ['http:', 'https:', 'mailto:'].includes(parsedUrl.protocol)
  }
  catch {
    return false
  }
}

export function decorateExternalMarkdownLink(token: MarkdownLinkToken): boolean {
  const href = token.attrGet('href')
  if (!href || !isAllowedExternalUrl(href))
    return false

  // The delegated click handler is the only owner of navigation. Keeping
  // target="_blank" here lets WebKit open a second copy beside the Tauri command.
  token.attrSet('rel', 'noopener noreferrer')
  token.attrSet('data-external-url', href)
  token.attrSet('title', `打开链接: ${href}`)
  return true
}

export function handleExternalMarkdownLinkClick(
  href: string,
  event: ConsumableLinkClick,
  open: (url: string) => unknown,
): boolean {
  if (!isAllowedExternalUrl(href))
    return false

  event.preventDefault()
  event.stopPropagation()
  void open(href)
  return true
}
