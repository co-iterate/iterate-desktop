import assert from 'node:assert/strict'
import {
  buildSubmissionClipboardText,
  copySubmissionToClipboard,
} from './submissionClipboard.ts'

assert.equal(
  buildSubmissionClipboardText('输入框原文', ['选项甲', '选项乙']),
  '选中的选项: 选项甲 / 选项乙\n\n输入框原文',
)

assert.equal(
  buildSubmissionClipboardText('  保留输入原文  ', []),
  '  保留输入原文  ',
)

assert.equal(
  buildSubmissionClipboardText('', ['仅选项']),
  '选中的选项: 仅选项',
)

{
  const writes: string[] = []
  const result = await copySubmissionToClipboard({
    enabled: true,
    userInput: '正文',
    selectedOptions: ['选择 A'],
    writeText: async (text) => {
      writes.push(text)
    },
  })

  assert.equal(result, 'copied')
  assert.deepEqual(writes, ['选中的选项: 选择 A\n\n正文'])
}

{
  const writes: string[] = []
  const result = await copySubmissionToClipboard({
    enabled: false,
    userInput: '不会覆盖剪贴板',
    selectedOptions: ['关闭'],
    writeText: async (text) => {
      writes.push(text)
    },
  })

  assert.equal(result, 'disabled')
  assert.deepEqual(writes, [])
}

{
  const writes: string[] = []
  const result = await copySubmissionToClipboard({
    enabled: true,
    userInput: '',
    selectedOptions: [],
    writeText: async (text) => {
      writes.push(text)
    },
  })

  assert.equal(result, 'empty')
  assert.deepEqual(writes, [])
}
