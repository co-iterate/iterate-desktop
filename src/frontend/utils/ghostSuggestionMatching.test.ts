/* eslint-disable test/no-import-node-test */
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import {
  filterCommandSuggestions,
  getCommandSuggestionSuffix,
  getMatchingCommandSuggestions,
  hasVisibleCommandSuggestion,
  MAX_RUNTIME_GHOST_SUGGESTIONS,
} from './ghostSuggestionMatching.ts'

const suggestions = [
  { key: 'check', description: '事实核查' },
  { key: 'che', description: '精准撤回 checkpoint' },
  { key: 'checkpoint', description: '精准撤回 checkpoint' },
  { key: 'hui1', description: '项目记忆回溯' },
  { key: 'hui', description: '项目记忆回溯' },
]

describe('ghost suggestion matching', () => {
  it('prioritizes prefix matches while preserving manual order inside each rank', () => {
    const searchableSuggestions = [
      { key: 'sync', description: 'AI 自动同步' },
      { key: 'plan', description: 'Codex 计划' },
      { key: 'active', description: '自动学习' },
      { key: 'add', description: '自动学习' },
      { key: 'Agent', description: '自动学习' },
      { key: 'Agents', description: '自动学习' },
      { key: 'ai-sidebar', description: '自动学习' },
      { key: 'airdrop', description: '自动学习' },
      { key: 'All', description: '自动学习' },
      { key: 'and', description: '自动学习' },
      { key: 'Android', description: '自动学习' },
    ]

    assert.deepEqual(
      filterCommandSuggestions(searchableSuggestions, 'a').map(suggestion => suggestion.key),
      [
        'active',
        'add',
        'Agent',
        'Agents',
        'ai-sidebar',
        'airdrop',
        'All',
        'and',
        'Android',
        'plan',
        'sync',
      ],
    )
  })

  it('ranks exact, prefix, key substring, then description matches', () => {
    const searchableSuggestions = [
      { key: 'memory', description: '调用 hui 回溯' },
      { key: 'ahui', description: '中间包含' },
      { key: 'hui', description: '完整匹配' },
      { key: 'hui0', description: '前缀匹配' },
    ]

    assert.deepEqual(
      filterCommandSuggestions(searchableSuggestions, ' HUI ').map(suggestion => suggestion.key),
      ['hui', 'hui0', 'ahui', 'memory'],
    )
  })

  it('keeps key matches ahead of description-only Chinese matches', () => {
    const searchableSuggestions = [
      { key: 'hui1', description: '项目记忆回溯' },
      { key: '事项', description: '中间包含' },
      { key: '项目', description: '首字命中' },
    ]

    assert.deepEqual(
      filterCommandSuggestions(searchableSuggestions, '项').map(suggestion => suggestion.key),
      ['项目', '事项', 'hui1'],
    )
  })

  it('shows s-prefix triggers before earlier manual-order substring matches', () => {
    const searchableSuggestions = [
      { key: 'Agents', description: '自动学习' },
      { key: 'session', description: '自动学习' },
      { key: 'sou', description: '搜索' },
      { key: 'sync', description: '自动同步' },
    ]

    assert.deepEqual(
      filterCommandSuggestions(searchableSuggestions, 's').map(suggestion => suggestion.key),
      ['session', 'sou', 'sync', 'Agents'],
    )
  })

  it('restores the original list when the search is empty', () => {
    assert.equal(filterCommandSuggestions(suggestions, ''), suggestions)
  })

  it('prioritizes an exact match before longer prefixes', () => {
    assert.deepEqual(
      getMatchingCommandSuggestions(suggestions, 'che').map(suggestion => suggestion.key),
      ['che', 'check', 'checkpoint'],
    )

    assert.deepEqual(
      getMatchingCommandSuggestions(suggestions, 'hui').map(suggestion => suggestion.key),
      ['hui', 'hui1'],
    )
  })

  it('keeps existing order when there is no exact match', () => {
    assert.deepEqual(
      getMatchingCommandSuggestions(suggestions, 'ch').map(suggestion => suggestion.key),
      ['check', 'che', 'checkpoint'],
    )
  })

  it('caps runtime matches while still finding a late exact match', () => {
    const manySuggestions = Array.from({ length: 50 }, (_, index) => ({
      key: `term${index}`,
      description: '候选',
    }))
    manySuggestions.push({ key: 'term', description: '完整匹配' })

    const matches = getMatchingCommandSuggestions(manySuggestions, 'term')
    assert.equal(matches.length, MAX_RUNTIME_GHOST_SUGGESTIONS)
    assert.equal(matches[0].key, 'term')
    assert.deepEqual(
      matches.slice(1, 4).map(suggestion => suggestion.key),
      ['term0', 'term1', 'term2'],
    )
  })

  it('hides suggestions after accepting the same token', () => {
    assert.deepEqual(getMatchingCommandSuggestions(suggestions, 'che', {
      acceptedSuggestionToken: 'che',
    }), [])
  })

  it('does not match while the user is composing text', () => {
    assert.deepEqual(getMatchingCommandSuggestions(suggestions, 'che', {
      isComposing: true,
    }), [])
  })

  it('only treats suggestions with suffixes as visible completions', () => {
    const exact = { key: 'che', description: '精准撤回 checkpoint' }
    const longer = { key: 'check', description: '事实核查' }

    assert.equal(getCommandSuggestionSuffix(exact, 'che'), '')
    assert.equal(hasVisibleCommandSuggestion(exact, 'che'), false)
    assert.equal(getCommandSuggestionSuffix(longer, 'che'), 'ck')
    assert.equal(hasVisibleCommandSuggestion(longer, 'che'), true)
  })
})
