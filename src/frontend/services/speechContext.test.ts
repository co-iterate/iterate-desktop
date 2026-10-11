/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import {
  buildSpeechContextualStrings,
  extractSafeSpeechVocabularyTerms,
  extractSpeechTerms,
  normalizeSpeechText,
} from './speechContext.ts'

describe('speech context', () => {
  it('ranks correction and live context terms before command fallbacks', () => {
    const hints = buildSpeechContextualStrings({
      requestMessage: '修复 Swift 语音输入',
      userInput: '现在处理 style 对齐',
      correctionMemoryEntries: [
        {
          observedText: 'sell',
          intendedText: 'style',
          contextTerms: ['CSS', '前端'],
          confirmCount: 4,
          hitCount: 2,
          isEnabled: true,
        },
      ],
      muscleMemoryEntries: [
        {
          spokenPhrase: '指令',
          outputText: 'zhi',
          trainingCount: 8,
          isEnabled: true,
        },
      ],
      limit: 24,
    })

    assert.deepEqual(hints.slice(0, 3), ['style', 'CSS', '前端'])
    assert.ok(hints.includes('style'))
    assert.ok(hints.includes('Swift'))
    assert.equal(hints.indexOf('style') < hints.indexOf('Swift'), true)
    assert.equal(hints.includes('zhi'), true)
    assert.equal(hints.indexOf('zhi') > hints.indexOf('Swift'), true)
  })

  it('deduplicates by normalized speech text', () => {
    const hints = buildSpeechContextualStrings({
      correctionMemoryEntries: [
        { intendedText: 'call_zhi', contextTerms: ['call zhi'], confirmCount: 3, isEnabled: true },
      ],
      limit: 10,
    })

    assert.equal(hints.filter(term => normalizeSpeechText(term) === 'callzhi').length, 1)
  })

  it('extracts command-like English terms and multi-character Chinese terms', () => {
    assert.deepEqual(
      extractSpeechTerms('打开 call_zhi，然后继续调试，回到项目'),
      ['打开', 'call_zhi', '然后继续调试', '回到项目', '回'],
    )
  })

  it('keeps reusable vocabulary while rejecting paths, secrets, ids, and duplicates', () => {
    assert.deepEqual(
      extractSafeSpeechVocabularyTerms('Codex codex /Users/example/docs auth_token 019f51bab1657453 迭代语音'),
      ['Codex', '迭代语音'],
    )
  })

  it('ranks remembered vocabulary before fallback domain terms', () => {
    const hints = buildSpeechContextualStrings({ rememberedTerms: ['个人词典'], limit: 20 })
    assert.equal(hints.indexOf('个人词典') < hints.indexOf('MCP'), true)
  })
})
