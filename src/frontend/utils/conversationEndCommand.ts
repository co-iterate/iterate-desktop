const EXPLICIT_CONVERSATION_END_COMMANDS = new Set([
  '结束对话',
  '退出对话',
  '停止对话',
  '结束本次对话',
  '/end',
])

const TERMINAL_PUNCTUATION_PATTERN
  = /[\s!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~。，；：！？、…～—”’」』】）]+$/u

/**
 * Mirrors the Rust response-boundary check closely enough to prevent an
 * explicit `/end` paste from being mistaken for a Unix file attachment.
 * The backend remains the authoritative place that ends the interaction.
 */
export function isExplicitConversationEndInput(value: string): boolean {
  const normalized = value
    .trim()
    .replace(TERMINAL_PUNCTUATION_PATTERN, '')
    .toLocaleLowerCase('en-US')

  return EXPLICIT_CONVERSATION_END_COMMANDS.has(normalized)
}
