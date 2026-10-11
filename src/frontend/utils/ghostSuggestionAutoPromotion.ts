export const GHOST_SUGGESTION_AUTO_PROMOTION_STORAGE_KEY = 'iterate:ghost-suggestions:auto-promotion:v1'
export const GHOST_SUGGESTION_AUTO_PROMOTION_ACCEPT_THRESHOLD = 2
export const GHOST_SUGGESTION_AUTO_PROMOTION_TYPED_THRESHOLD = 3
export const GHOST_SUGGESTION_AUTO_PROMOTION_DESCRIPTION = '自动学习 / 当前项目高频'
export const GHOST_SUGGESTION_AUTO_PROMOTION_TYPED_DESCRIPTION = '自动学习 / 手动输入高频候选'

const MAX_KEY_LENGTH = 32
const KEY_PATTERN = /^(?:[\p{Letter}\p{Number}]|\.[\p{Letter}\p{Number}])[\p{Letter}\p{Number}_.:-]*$/u
const TOKEN_PATTERN = /(?:[\p{Letter}\p{Number}]|\.[\p{Letter}\p{Number}])[\p{Letter}\p{Number}_.:-]*/gu
const PATH_OR_URL_PATTERN = /[\\/@?=]/
const FILE_NAME_PATTERN = /^[\p{Letter}\p{Number}_-]+\.[A-Za-z0-9]{1,8}$/u
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
const LONG_HEX_PATTERN = /^[0-9a-f]{12,}$/i
const ISSUE_OR_RUNTIME_ID_PATTERN = /^(?:p-\d{4}-\d+|t\d+|serve-\d+|sample-\d+|run-\d+)$/i
const SECRET_PATTERN = /(?:^|[_.:-])(?:token|secret|password|passwd|apikey|api-key|api_key|private|credential|auth)(?:$|[_.:-])/i
const ENV_PATTERN = /(?:^|[_.:-])env(?:$|[_.:-])/i
const ID_LIKE_PATTERN = /(?:^|[_.:-])(?:id|uuid|hash|sha|commit)(?:$|[_.:-])/i

export interface GhostSuggestionAutoPromotionEntry {
  key: string
  description: string
  accepted_count: number
  typed_count: number
  first_accepted_at: string
  last_accepted_at: string
  first_typed_at: string
  last_typed_at: string
  promoted: boolean
}

export interface GhostSuggestionAutoPromotionState {
  version: 2
  entries: Record<string, GhostSuggestionAutoPromotionEntry>
}

export interface GhostSuggestionAutoPromotionResult {
  state: GhostSuggestionAutoPromotionState
  entry: GhostSuggestionAutoPromotionEntry
  shouldPromote: boolean
}

export interface GhostSuggestionAutoPromotionCandidate {
  key: string
  description: string
}

interface DirectUserInputEventLike {
  isTrusted: boolean
  inputType: string
  data: string | null
  isComposing: boolean
}

interface DirectUserCompositionEventLike {
  isTrusted: boolean
  data: string
}

export function createEmptyGhostSuggestionAutoPromotionState(): GhostSuggestionAutoPromotionState {
  return {
    version: 2,
    entries: {},
  }
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function normalizeGhostSuggestionAutoPromotionState(raw: unknown): GhostSuggestionAutoPromotionState {
  if (!isPlainObject(raw) || !isPlainObject(raw.entries))
    return createEmptyGhostSuggestionAutoPromotionState()

  const entries: Record<string, GhostSuggestionAutoPromotionEntry> = {}
  const now = new Date().toISOString()

  Object.values(raw.entries).forEach((value) => {
    if (!isPlainObject(value) || typeof value.key !== 'string')
      return

    const key = normalizeAutoPromotionKey(value.key)
    const acceptedCount = typeof value.accepted_count === 'number' && Number.isFinite(value.accepted_count)
      ? Math.max(0, Math.floor(value.accepted_count))
      : 0
    const typedCount = typeof value.typed_count === 'number' && Number.isFinite(value.typed_count)
      ? Math.max(0, Math.floor(value.typed_count))
      : 0
    if (!key || (acceptedCount <= 0 && typedCount <= 0))
      return

    entries[key.toLowerCase()] = {
      key,
      description: typeof value.description === 'string' ? value.description : '',
      accepted_count: acceptedCount,
      typed_count: typedCount,
      first_accepted_at: typeof value.first_accepted_at === 'string' ? value.first_accepted_at : now,
      last_accepted_at: typeof value.last_accepted_at === 'string' ? value.last_accepted_at : now,
      first_typed_at: typeof value.first_typed_at === 'string' ? value.first_typed_at : now,
      last_typed_at: typeof value.last_typed_at === 'string' ? value.last_typed_at : now,
      promoted: value.promoted === true,
    }
  })

  return {
    version: 2,
    entries,
  }
}

export function parseGhostSuggestionAutoPromotionState(raw: string | null): GhostSuggestionAutoPromotionState {
  if (!raw)
    return createEmptyGhostSuggestionAutoPromotionState()

  try {
    return normalizeGhostSuggestionAutoPromotionState(JSON.parse(raw))
  }
  catch {
    return createEmptyGhostSuggestionAutoPromotionState()
  }
}

export function normalizeAutoPromotionKey(key: string): string {
  return key.trim()
}

function digitRatio(key: string): number {
  if (!key)
    return 0

  const digitCount = Array.from(key).filter(character => /\d/.test(character)).length
  return digitCount / Array.from(key).length
}

function isPathAdjacent(input: string, index: number, length: number): boolean {
  const before = index > 0 ? input[index - 1] : ''
  const after = input[index + length] ?? ''
  return PATH_OR_URL_PATTERN.test(before) || PATH_OR_URL_PATTERN.test(after)
}

export function shouldTrackGhostSuggestionAutoPromotion(
  rawKey: string,
  existingKeys: string[] = [],
): boolean {
  const key = normalizeAutoPromotionKey(rawKey)
  if (!key || key.length < 3 || key.length > MAX_KEY_LENGTH)
    return false

  if (!KEY_PATTERN.test(key))
    return false

  const lowerKey = key.toLowerCase()
  if (existingKeys.some(existingKey => existingKey.trim().toLowerCase() === lowerKey))
    return false

  if (PATH_OR_URL_PATTERN.test(key))
    return false

  if (FILE_NAME_PATTERN.test(key) || ENV_PATTERN.test(lowerKey))
    return false

  if (/^\d+$/.test(key) || digitRatio(key) > 0.6)
    return false

  if (UUID_PATTERN.test(key) || LONG_HEX_PATTERN.test(key) || ISSUE_OR_RUNTIME_ID_PATTERN.test(key))
    return false

  if (SECRET_PATTERN.test(key) || ID_LIKE_PATTERN.test(key))
    return false

  return true
}

export function extractGhostSuggestionAutoPromotionTerms(
  input: string,
  existingKeys: string[] = [],
): string[] {
  const terms = new Set<string>()

  for (const match of input.matchAll(TOKEN_PATTERN)) {
    if (isPathAdjacent(input, match.index ?? 0, match[0].length))
      continue

    const key = normalizeAutoPromotionKey(match[0])
    if (!shouldTrackGhostSuggestionAutoPromotion(key, existingKeys))
      continue
    terms.add(key)
  }

  return Array.from(terms)
}

export function directUserLearningChunkFromInputEvent(
  event: DirectUserInputEventLike,
): string {
  if (event.isComposing)
    return ''

  if (event.inputType === 'insertText')
    return event.data ?? ''

  if (event.inputType === 'insertLineBreak' || event.inputType === 'insertParagraph')
    return '\n'

  return ''
}

export function directUserLearningChunkFromCompositionEvent(
  event: DirectUserCompositionEventLike,
): string {
  return event.data
}

export function extractDirectUserAuthoredPromotionTerms(
  authoredText: string,
  submittedInput: string,
  existingKeys: string[] = [],
): string[] {
  const submittedTerms = new Set(
    extractGhostSuggestionAutoPromotionTerms(submittedInput, existingKeys)
      .map(term => term.toLocaleLowerCase()),
  )

  return extractGhostSuggestionAutoPromotionTerms(authoredText, existingKeys)
    .filter(term => submittedTerms.has(term.toLocaleLowerCase()))
}

export function recordGhostSuggestionAutoPromotionAcceptance(
  state: GhostSuggestionAutoPromotionState,
  rawKey: string,
  rawDescription: string,
  now = new Date().toISOString(),
): GhostSuggestionAutoPromotionResult {
  const key = normalizeAutoPromotionKey(rawKey)
  const normalizedLookupKey = key.toLowerCase()
  const current = state.entries[normalizedLookupKey]
  const entry: GhostSuggestionAutoPromotionEntry = current
    ? {
        ...current,
        key,
        description: rawDescription || current.description,
        accepted_count: current.accepted_count + 1,
        typed_count: current.typed_count ?? 0,
        first_accepted_at: current.first_accepted_at || now,
        last_accepted_at: now,
      }
    : {
        key,
        description: rawDescription,
        accepted_count: 1,
        typed_count: 0,
        first_accepted_at: now,
        last_accepted_at: now,
        first_typed_at: '',
        last_typed_at: '',
        promoted: false,
      }

  const nextState: GhostSuggestionAutoPromotionState = {
    version: 2,
    entries: {
      ...state.entries,
      [normalizedLookupKey]: entry,
    },
  }

  return {
    state: nextState,
    entry,
    shouldPromote: !entry.promoted
      && entry.accepted_count >= GHOST_SUGGESTION_AUTO_PROMOTION_ACCEPT_THRESHOLD,
  }
}

export function markGhostSuggestionAutoPromotionPromoted(
  state: GhostSuggestionAutoPromotionState,
  rawKey: string,
): GhostSuggestionAutoPromotionState {
  const key = normalizeAutoPromotionKey(rawKey).toLowerCase()
  const entry = state.entries[key]
  if (!entry)
    return state

  return {
    version: 2,
    entries: {
      ...state.entries,
      [key]: {
        ...entry,
        promoted: true,
      },
    },
  }
}

export function recordGhostSuggestionAutoPromotionTyping(
  state: GhostSuggestionAutoPromotionState,
  rawKey: string,
  rawDescription = GHOST_SUGGESTION_AUTO_PROMOTION_TYPED_DESCRIPTION,
  now = new Date().toISOString(),
): GhostSuggestionAutoPromotionState {
  const key = normalizeAutoPromotionKey(rawKey)
  const normalizedLookupKey = key.toLowerCase()
  const current = state.entries[normalizedLookupKey]
  const entry: GhostSuggestionAutoPromotionEntry = current
    ? {
        ...current,
        key,
        description: current.description || rawDescription,
        typed_count: (current.typed_count ?? 0) + 1,
        last_typed_at: now,
      }
    : {
        key,
        description: rawDescription,
        accepted_count: 0,
        typed_count: 1,
        first_accepted_at: '',
        last_accepted_at: '',
        first_typed_at: now,
        last_typed_at: now,
        promoted: false,
      }

  return {
    version: 2,
    entries: {
      ...state.entries,
      [normalizedLookupKey]: entry,
    },
  }
}

export function getGhostSuggestionAutoPromotionCandidates(
  state: GhostSuggestionAutoPromotionState,
  existingKeys: string[] = [],
): GhostSuggestionAutoPromotionCandidate[] {
  return Object.values(state.entries)
    .filter(entry => !entry.promoted)
    .filter(entry => entry.typed_count >= GHOST_SUGGESTION_AUTO_PROMOTION_TYPED_THRESHOLD)
    .filter(entry => shouldTrackGhostSuggestionAutoPromotion(entry.key, existingKeys))
    .sort((left, right) => {
      if (left.typed_count !== right.typed_count)
        return right.typed_count - left.typed_count
      return right.last_typed_at.localeCompare(left.last_typed_at)
    })
    .map(entry => ({
      key: entry.key,
      description: entry.description || GHOST_SUGGESTION_AUTO_PROMOTION_TYPED_DESCRIPTION,
    }))
}
