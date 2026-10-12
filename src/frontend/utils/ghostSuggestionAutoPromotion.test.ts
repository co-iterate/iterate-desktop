/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import {
  createEmptyGhostSuggestionAutoPromotionState,
  directUserLearningChunkFromCompositionEvent,
  directUserLearningChunkFromInputEvent,
  extractDirectUserAuthoredPromotionTerms,
  extractGhostSuggestionAutoPromotionTerms,
  getGhostSuggestionAutoPromotionCandidates,
  markGhostSuggestionAutoPromotionPromoted,
  parseGhostSuggestionAutoPromotionState,
  recordGhostSuggestionAutoPromotionAcceptance,
  recordGhostSuggestionAutoPromotionTyping,
  shouldTrackGhostSuggestionAutoPromotion,
} from './ghostSuggestionAutoPromotion.ts'

describe('ghost suggestion auto promotion', () => {
  it('promotes after the acceptance threshold', () => {
    const first = recordGhostSuggestionAutoPromotionAcceptance(
      createEmptyGhostSuggestionAutoPromotionState(),
      'activity',
      'hui 高频词',
      '2026-05-22T00:00:00.000Z',
    )
    assert.equal(first.entry.accepted_count, 1)
    assert.equal(first.shouldPromote, false)

    const second = recordGhostSuggestionAutoPromotionAcceptance(
      first.state,
      'activity',
      'hui 高频词',
      '2026-05-22T00:01:00.000Z',
    )
    assert.equal(second.entry.accepted_count, 2)
    assert.equal(second.shouldPromote, true)
  })

  it('does not promote an entry after it has been marked promoted', () => {
    const accepted = recordGhostSuggestionAutoPromotionAcceptance(
      createEmptyGhostSuggestionAutoPromotionState(),
      'activity',
      'hui 高频词',
    )
    const promotedState = markGhostSuggestionAutoPromotionPromoted(accepted.state, 'activity')
    const repeated = recordGhostSuggestionAutoPromotionAcceptance(
      promotedState,
      'activity',
      'hui 高频词',
    )

    assert.equal(repeated.entry.accepted_count, 2)
    assert.equal(repeated.entry.promoted, true)
    assert.equal(repeated.shouldPromote, false)
  })

  it('filters existing suggestions and noisy values', () => {
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('activity', ['activity']), false)
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('/Users/example/cunzhi'), false)
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('global_rules.md'), false)
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('.env'), false)
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('auth_token'), false)
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('commit_hash'), false)
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('9f86d081884c'), false)
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('123456'), false)
    assert.equal(shouldTrackGhostSuggestionAutoPromotion('activity'), true)
  })

  it('turns repeated typed terms into runtime candidates without promoting them', () => {
    let state = createEmptyGhostSuggestionAutoPromotionState()
    state = recordGhostSuggestionAutoPromotionTyping(state, 'activity', undefined, '2026-05-22T00:00:00.000Z')
    state = recordGhostSuggestionAutoPromotionTyping(state, 'activity', undefined, '2026-05-22T00:01:00.000Z')

    assert.deepEqual(getGhostSuggestionAutoPromotionCandidates(state), [])

    state = recordGhostSuggestionAutoPromotionTyping(state, 'activity', undefined, '2026-05-22T00:02:00.000Z')
    assert.deepEqual(getGhostSuggestionAutoPromotionCandidates(state), [
      {
        key: 'activity',
        description: '自动学习 / 手动输入高频候选',
      },
    ])
    assert.equal(state.entries.activity.promoted, false)
  })

  it('extracts only trackable terms from submitted input', () => {
    assert.deepEqual(
      extractGhostSuggestionAutoPromotionTerms('activity global_rules.md auth_token /Users/example/cunzhi activity', []),
      ['activity'],
    )
  })

  it('learns only native text insertion and committed IME text event types', () => {
    assert.equal(directUserLearningChunkFromInputEvent({
      isTrusted: true,
      inputType: 'insertText',
      data: 'activity',
      isComposing: false,
    }), 'activity')
    assert.equal(directUserLearningChunkFromInputEvent({
      isTrusted: true,
      inputType: 'insertFromPaste',
      data: 'AI response',
      isComposing: false,
    }), '')
    assert.equal(directUserLearningChunkFromInputEvent({
      isTrusted: false,
      inputType: 'insertText',
      data: 'webview-text',
      isComposing: false,
    }), 'webview-text')
    assert.equal(directUserLearningChunkFromInputEvent({
      isTrusted: true,
      inputType: 'insertCompositionText',
      data: '上',
      isComposing: true,
    }), '')
    assert.equal(directUserLearningChunkFromCompositionEvent({
      isTrusted: false,
      data: '上下文',
    }), '上下文')
  })

  it('excludes programmatic or deleted text from submitted learning terms', () => {
    assert.deepEqual(
      extractDirectUserAuthoredPromotionTerms(
        'activity deletedWord',
        'AI generated context gpt-5.6-terra activity',
        [],
      ),
      ['activity'],
    )
  })

  it('parses persisted state defensively', () => {
    assert.deepEqual(parseGhostSuggestionAutoPromotionState('not json'), createEmptyGhostSuggestionAutoPromotionState())

    const parsed = parseGhostSuggestionAutoPromotionState(JSON.stringify({
      version: 1,
      entries: {
        activity: {
          key: 'activity',
          description: 'hui 高频词',
          accepted_count: 2.8,
          first_accepted_at: '2026-05-22T00:00:00.000Z',
          last_accepted_at: '2026-05-22T00:01:00.000Z',
          promoted: true,
        },
        invalid: {
          key: '',
          accepted_count: 2,
        },
      },
    }))

    assert.equal(parsed.entries.activity.accepted_count, 2)
    assert.equal(parsed.entries.activity.promoted, true)
    assert.equal(parsed.entries.invalid, undefined)
  })
})
