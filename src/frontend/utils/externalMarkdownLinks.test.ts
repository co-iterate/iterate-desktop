import assert from 'node:assert/strict'
import {
  decorateExternalMarkdownLink,
  handleExternalMarkdownLinkClick,
} from './externalMarkdownLinks.ts'

class MockToken {
  private readonly attrs = new Map<string, string>()

  constructor(href: string) {
    this.attrs.set('href', href)
  }

  attrGet(name: string) {
    return this.attrs.get(name) ?? null
  }

  attrSet(name: string, value: string) {
    this.attrs.set(name, value)
  }
}

const token = new MockToken('https://example.com/docs')
assert.equal(decorateExternalMarkdownLink(token), true)
assert.equal(token.attrGet('href'), 'https://example.com/docs')
assert.equal(token.attrGet('target'), null)
assert.equal(token.attrGet('rel'), 'noopener noreferrer')
assert.equal(token.attrGet('data-external-url'), 'https://example.com/docs')

let prevented = 0
let stopped = 0
let opened = 0
const handled = handleExternalMarkdownLinkClick(
  'https://example.com/docs',
  {
    preventDefault: () => prevented++,
    stopPropagation: () => stopped++,
  },
  () => opened++,
)

assert.equal(handled, true)
assert.equal(prevented, 1)
assert.equal(stopped, 1)
assert.equal(opened, 1)

assert.equal(handleExternalMarkdownLinkClick(
  'javascript:alert(1)',
  {
    preventDefault: () => prevented++,
    stopPropagation: () => stopped++,
  },
  () => opened++,
), false)
assert.equal(prevented, 1)
assert.equal(stopped, 1)
assert.equal(opened, 1)
