import assert from 'node:assert/strict'
import { isExplicitConversationEndInput } from './conversationEndCommand.ts'

for (const value of [
  '结束对话',
  ' 退出对话。 ',
  '停止对话!',
  '结束本次对话！？',
  '/end',
  ' /END. ',
]) {
  assert.equal(isExplicitConversationEndInput(value), true, value)
}

for (const value of [
  '',
  '如何结束对话',
  '结束对话后会怎样',
  '请帮我结束对话',
  '/end now',
  '/end/readme',
  '/project',
]) {
  assert.equal(isExplicitConversationEndInput(value), false, value)
}
