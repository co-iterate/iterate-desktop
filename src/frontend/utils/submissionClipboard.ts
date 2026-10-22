export type SubmissionClipboardResult = 'disabled' | 'empty' | 'copied'

interface CopySubmissionToClipboardOptions {
  enabled: boolean
  userInput: string
  selectedOptions: readonly string[]
  writeText: (text: string) => Promise<void>
}

export function buildSubmissionClipboardText(
  userInput: string,
  selectedOptions: readonly string[],
): string {
  const readableOptions = selectedOptions.filter(option => option.trim().length > 0)
  if (readableOptions.length === 0)
    return userInput

  const optionText = `选中的选项: ${readableOptions.join(' / ')}`
  return userInput.length > 0 ? `${optionText}\n\n${userInput}` : optionText
}

export async function copySubmissionToClipboard({
  enabled,
  userInput,
  selectedOptions,
  writeText,
}: CopySubmissionToClipboardOptions): Promise<SubmissionClipboardResult> {
  if (!enabled)
    return 'disabled'

  const text = buildSubmissionClipboardText(userInput, selectedOptions)
  if (text.length === 0)
    return 'empty'

  await writeText(text)
  return 'copied'
}
