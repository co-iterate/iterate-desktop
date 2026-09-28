import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { computed, ref } from 'vue'
import { normalizeGhostSuggestionOrder } from '../utils/ghostSuggestionOrdering'
import { shouldApplyIncomingStore, shouldPreventCacheRollback, storeTimestamp } from '../utils/ghostSuggestionStoreSync'

export interface GhostSuggestion {
  id: string
  key: string
  description: string
  enabled: boolean
  sort_order: number
  created_at: string
  updated_at: string
}

interface GhostSuggestionStore {
  version: 1
  defaultSeedVersion: number
  updatedAt: string
  suggestions: GhostSuggestion[]
}

interface GhostSuggestionInput {
  key: string
  description?: string
  enabled?: boolean
}

export interface DefaultGhostSuggestion {
  key: string
  description: string
}

const GHOST_SUGGESTION_KEY_COLLATOR = new Intl.Collator('en', {
  numeric: true,
  sensitivity: 'base',
})

export type GhostSuggestionSaveResult
  = | { ok: true, item: GhostSuggestion }
    | { ok: false, reason: string }

export const GHOST_SUGGESTION_KEY_PATTERN = /^(?:[\p{Letter}\p{Number}]|\.[\p{Letter}\p{Number}])[\p{Letter}\p{Number}_.:-]*$/u
export const GHOST_SUGGESTION_TOKEN_PATTERN = /((?:[\p{Letter}\p{Number}]|\.[\p{Letter}\p{Number}])[\p{Letter}\p{Number}_.:-]*)$/u

function ghostSuggestions(description: string, keys: string[]): DefaultGhostSuggestion[] {
  return keys.map(key => ({ key, description }))
}

// Keep this runtime seed aligned with ~/.cunzhi-knowledge/prompts/skills/INDEX.md.
// Defaults stay deliberately small; less common skill triggers can be added manually.
export const DEFAULT_GHOST_SUGGESTIONS: DefaultGhostSuggestion[] = [
  ...ghostSuggestions('沉淀/记忆', ['ji']),
  ...ghostSuggestions('代码审查', ['cha', 'check', 'review']),
  ...ghostSuggestions('多终端并发编排', ['pai']),
  ...ghostSuggestions('咨询建议', ['qiu']),
  ...ghostSuggestions('多模型执行', ['copilot']),
  ...ghostSuggestions('网络搜索', ['sou']),
  ...ghostSuggestions('查询历史经验', ['xi']),
  ...ghostSuggestions('同步知识库', ['sync']),
  ...ghostSuggestions('并行调研', ['yan']),
  ...ghostSuggestions('Codex 计划', ['plan']),
  ...ghostSuggestions('GoalRun 目标循环', ['GoalRun', 'goalrun', 'goal-loop', '目标循环']),
  ...ghostSuggestions('项目记忆回溯', ['hui', 'hui1', 'hui0', '回']),
  ...ghostSuggestions('系统化调试', ['debug']),
  ...ghostSuggestions('软件自动化工作流', ['auto']),
  ...ghostSuggestions('精准撤回 checkpoint', [
    'che',
    'checkpoint',
    'checkpoint_id',
    'commit_hash',
    'iterate-checkpoint',
    'conversation-id',
  ]),
  ...ghostSuggestions('常用构建/测试', ['build', 'test', 'lint', 'commit']),
]

const STORAGE_KEY = 'iterate:ghost-suggestions:v1'
const STORAGE_EVENT_NAME = 'iterate:ghost-suggestions-updated'
export const GHOST_SUGGESTIONS_ENABLED_STORAGE_KEY = 'iterate:ghost-suggestions:enabled'
const GHOST_SUGGESTIONS_ENABLED_EVENT_NAME = 'iterate:ghost-suggestions-enabled-changed'
const DEFAULT_SEED_VERSION = 6
const MAX_KEY_LENGTH = 32
const SHARED_FILE_SYNC_INTERVAL_MS = 2500

const suggestions = ref<GhostSuggestion[]>([])
const storeUpdatedAt = ref('')
const ghostSuggestionsEnabled = ref(true)
let loaded = false
let syncListenerReady = false
let tauriSyncListenerReady = false
let sharedFileSyncTimer: number | null = null
let sharedFileSyncInFlight = false

export function normalizeGhostSuggestionKey(key: string): string {
  return key.trim()
}

export function isValidGhostSuggestionKey(key: string): boolean {
  const normalizedKey = normalizeGhostSuggestionKey(key)
  return normalizedKey.length > 0
    && normalizedKey.length <= MAX_KEY_LENGTH
    && GHOST_SUGGESTION_KEY_PATTERN.test(normalizedKey)
}

export function compareGhostSuggestionKeys(left: Pick<GhostSuggestion, 'key'>, right: Pick<GhostSuggestion, 'key'>): number {
  const keyComparison = GHOST_SUGGESTION_KEY_COLLATOR.compare(left.key, right.key)
  if (keyComparison !== 0)
    return keyComparison

  return left.key.localeCompare(right.key)
}

function createId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function')
    return crypto.randomUUID()

  return `ghost_${Date.now()}_${Math.random().toString(36).slice(2, 10)}`
}

function createDefaultId(key: string): string {
  return `default_${encodeURIComponent(key)}`
}

function sanitizeSuggestion(raw: Partial<GhostSuggestion> | null | undefined, index: number): GhostSuggestion | null {
  if (!raw || typeof raw.key !== 'string')
    return null

  const key = normalizeGhostSuggestionKey(raw.key)
  if (!isValidGhostSuggestionKey(key))
    return null

  const now = new Date().toISOString()
  return {
    id: typeof raw.id === 'string' && raw.id.trim() ? raw.id : createId(),
    key,
    description: typeof raw.description === 'string' ? raw.description.trim() : '',
    enabled: raw.enabled !== false,
    sort_order: typeof raw.sort_order === 'number' ? raw.sort_order : index + 1,
    created_at: typeof raw.created_at === 'string' ? raw.created_at : now,
    updated_at: typeof raw.updated_at === 'string' ? raw.updated_at : now,
  }
}

function normalizeSort(items: GhostSuggestion[]): GhostSuggestion[] {
  return normalizeGhostSuggestionOrder(items)
}

function getDefaultKeySet(): Set<string> {
  return new Set(DEFAULT_GHOST_SUGGESTIONS.map(item => item.key.toLowerCase()))
}

function getDefaultKeys(): string[] {
  return DEFAULT_GHOST_SUGGESTIONS.map(item => item.key)
}

function shouldProtectSharedStore(candidateStore: GhostSuggestionStore | null, baselineStore: GhostSuggestionStore | null): boolean {
  return shouldPreventCacheRollback(candidateStore, baselineStore, {
    defaultKeys: getDefaultKeys(),
  })
}

function isDefaultSuggestionId(id: string): boolean {
  return id.startsWith('default_')
}

function pruneRetiredDefaultSuggestions(items: GhostSuggestion[]): GhostSuggestion[] {
  const defaultKeys = getDefaultKeySet()
  return items.filter(item => !isDefaultSuggestionId(item.id) || defaultKeys.has(item.key.toLowerCase()))
}

function mergeDefaultSuggestions(items: GhostSuggestion[]): GhostSuggestion[] {
  const nextSuggestions = normalizeSort(items)
  const existingKeys = new Set(nextSuggestions.map(item => item.key.toLowerCase()))
  const now = new Date().toISOString()

  DEFAULT_GHOST_SUGGESTIONS.forEach((defaultSuggestion) => {
    const key = normalizeGhostSuggestionKey(defaultSuggestion.key)
    if (!key || existingKeys.has(key.toLowerCase()))
      return

    nextSuggestions.push({
      id: createDefaultId(key),
      key,
      description: defaultSuggestion.description,
      enabled: true,
      sort_order: nextSuggestions.length + 1,
      created_at: now,
      updated_at: now,
    })
    existingKeys.add(key.toLowerCase())
  })

  return normalizeSort(nextSuggestions)
}

function normalizeStore(raw: Partial<GhostSuggestionStore> | null | undefined): GhostSuggestionStore | null {
  if (!raw)
    return null

  const nextSuggestions = Array.isArray(raw.suggestions)
    ? raw.suggestions
        .map((suggestion, index) => sanitizeSuggestion(suggestion, index))
        .filter((suggestion): suggestion is GhostSuggestion => !!suggestion)
    : []

  const suggestions = raw.defaultSeedVersion !== DEFAULT_SEED_VERSION
    ? mergeDefaultSuggestions(pruneRetiredDefaultSuggestions(nextSuggestions))
    : normalizeSort(nextSuggestions)

  return {
    version: 1,
    defaultSeedVersion: DEFAULT_SEED_VERSION,
    updatedAt: typeof raw.updatedAt === 'string' ? raw.updatedAt : '',
    suggestions,
  }
}

function parseStore(raw: string): GhostSuggestionStore | null {
  try {
    return normalizeStore(JSON.parse(raw) as Partial<GhostSuggestionStore>)
  }
  catch (error) {
    console.error('[GhostSuggestions] parse failed:', error)
    return null
  }
}

function createStore(items: GhostSuggestion[], updatedAt = new Date().toISOString()): GhostSuggestionStore {
  return {
    version: 1,
    defaultSeedVersion: DEFAULT_SEED_VERSION,
    updatedAt,
    suggestions: normalizeSort(items),
  }
}

function readStoreFromStorage(): GhostSuggestionStore | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? parseStore(raw) : null
  }
  catch (error) {
    console.error('[GhostSuggestions] load failed:', error)
    return null
  }
}

function writeStoreToStorage(store: GhostSuggestionStore) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(store))
  emitStorageUpdate()
}

function applySharedStore(store: GhostSuggestionStore) {
  suggestions.value = store.suggestions
  storeUpdatedAt.value = store.updatedAt
  writeStoreToStorage(store)
}

async function syncFromSharedFile(options: { writeBackIfLocalNewer?: boolean } = {}) {
  if (sharedFileSyncInFlight)
    return

  sharedFileSyncInFlight = true
  try {
    const content = await invoke<string>('load_ghost_suggestions_file')
    if (!content)
      return

    const sharedStore = parseStore(content)
    if (!sharedStore)
      return

    if (sharedStore.suggestions.length === 0) {
      if (options.writeBackIfLocalNewer && suggestions.value.length > 0)
        save()
      return
    }

    const localStore = readStoreFromStorage()

    if (shouldApplyIncomingStore(sharedStore, localStore)) {
      if (shouldProtectSharedStore(sharedStore, localStore)) {
        console.warn('[GhostSuggestions] ignored incoming store because it would drop multiple user suggestions')
        return
      }

      applySharedStore(sharedStore)
      return
    }

    if (options.writeBackIfLocalNewer && storeTimestamp(localStore) > storeTimestamp(sharedStore)) {
      if (shouldProtectSharedStore(localStore, sharedStore)) {
        console.warn('[GhostSuggestions] restored shared file over smaller local cache to prevent suggestion rollback')
        applySharedStore(sharedStore)
        return
      }

      save()
    }
  }
  catch (error) {
    console.warn('[GhostSuggestions] shared file fallback failed:', error)
  }
  finally {
    sharedFileSyncInFlight = false
  }
}

function emitStorageUpdate() {
  if (typeof window === 'undefined')
    return

  window.dispatchEvent(new CustomEvent(STORAGE_EVENT_NAME))
}

function save() {
  const store = createStore(suggestions.value)

  suggestions.value = store.suggestions
  storeUpdatedAt.value = store.updatedAt
  const content = JSON.stringify(store)
  localStorage.setItem(STORAGE_KEY, content)
  invoke('save_ghost_suggestions_file', { content }).catch((error: unknown) => {
    console.warn('[GhostSuggestions] shared file sync failed:', error)
  })
  emitStorageUpdate()
}

function reloadFromStorage() {
  const store = readStoreFromStorage()
  suggestions.value = store?.suggestions ?? mergeDefaultSuggestions([])
  storeUpdatedAt.value = store?.updatedAt ?? ''
}

function reloadGlobalEnabledFromStorage() {
  if (typeof window === 'undefined') {
    ghostSuggestionsEnabled.value = true
    return
  }

  ghostSuggestionsEnabled.value = localStorage.getItem(GHOST_SUGGESTIONS_ENABLED_STORAGE_KEY) !== 'false'
}

function setGhostSuggestionsEnabled(enabled: boolean) {
  ghostSuggestionsEnabled.value = enabled
  if (typeof window === 'undefined')
    return

  localStorage.setItem(GHOST_SUGGESTIONS_ENABLED_STORAGE_KEY, String(enabled))
  window.dispatchEvent(new CustomEvent(GHOST_SUGGESTIONS_ENABLED_EVENT_NAME, {
    detail: { enabled },
  }))
}

function load() {
  if (loaded)
    return

  loaded = true
  setupSyncListener()
  setupTauriSyncListener()
  setupSharedFileSyncTimer()

  reloadFromStorage()
  reloadGlobalEnabledFromStorage()
  void syncFromSharedFile({ writeBackIfLocalNewer: true })
}

function setupSyncListener() {
  if (syncListenerReady || typeof window === 'undefined')
    return

  syncListenerReady = true
  window.addEventListener('iterate:settings-synced', () => { void syncFromSharedFile() })
  window.addEventListener('storage', (event) => {
    if (event.key === STORAGE_KEY)
      reloadFromStorage()
    else if (event.key === GHOST_SUGGESTIONS_ENABLED_STORAGE_KEY)
      reloadGlobalEnabledFromStorage()
  })
  window.addEventListener(STORAGE_EVENT_NAME, () => {
    reloadFromStorage()
  })
  window.addEventListener(GHOST_SUGGESTIONS_ENABLED_EVENT_NAME, (event) => {
    const enabled = (event as CustomEvent<{ enabled?: boolean }>).detail?.enabled
    if (typeof enabled === 'boolean')
      ghostSuggestionsEnabled.value = enabled
    else
      reloadGlobalEnabledFromStorage()
  })
}

function setupTauriSyncListener() {
  if (tauriSyncListenerReady)
    return

  tauriSyncListenerReady = true
  listen<Partial<GhostSuggestionStore>>('ghost-suggestions-changed', (event) => {
    const sharedStore = normalizeStore(event.payload)
    if (!sharedStore)
      return

    const localStore = readStoreFromStorage()
    if (shouldApplyIncomingStore(sharedStore, localStore)) {
      if (shouldProtectSharedStore(sharedStore, localStore)) {
        console.warn('[GhostSuggestions] ignored backend event because it would drop multiple user suggestions')
        return
      }

      applySharedStore(sharedStore)
    }
  }).catch((error: unknown) => {
    console.warn('[GhostSuggestions] backend sync listener failed:', error)
  })
}

function setupSharedFileSyncTimer() {
  if (sharedFileSyncTimer !== null || typeof window === 'undefined')
    return

  sharedFileSyncTimer = window.setInterval(() => {
    void syncFromSharedFile()
  }, SHARED_FILE_SYNC_INTERVAL_MS)

  window.addEventListener('focus', () => {
    void syncFromSharedFile()
  })
}

function ensureLoaded() {
  if (!loaded)
    load()
}

function validateInput(input: GhostSuggestionInput, ignoredId?: string): { ok: true, key: string, description: string } | { ok: false, reason: string } {
  const key = normalizeGhostSuggestionKey(input.key)
  const description = input.description?.trim() ?? ''

  if (!key)
    return { ok: false, reason: '请填写触发词' }

  if (key.length > MAX_KEY_LENGTH)
    return { ok: false, reason: `触发词不能超过 ${MAX_KEY_LENGTH} 个字符` }

  if (!isValidGhostSuggestionKey(key))
    return { ok: false, reason: '触发词需以文字、数字或点号文件扩展名开头，仅支持文字、数字、下划线、点、冒号和短横线' }

  const lowerKey = key.toLowerCase()
  const duplicate = suggestions.value.some(item => item.id !== ignoredId && item.key.toLowerCase() === lowerKey)
  if (duplicate)
    return { ok: false, reason: '触发词已存在' }

  return { ok: true, key, description }
}

export function useGhostSuggestions() {
  ensureLoaded()

  const enabledSuggestions = computed(() => normalizeSort(suggestions.value.filter(suggestion => suggestion.enabled)))

  function addSuggestion(input: GhostSuggestionInput): GhostSuggestionSaveResult {
    ensureLoaded()
    const validation = validateInput(input)
    if (!validation.ok)
      return validation

    const now = new Date().toISOString()
    const item: GhostSuggestion = {
      id: createId(),
      key: validation.key,
      description: validation.description,
      enabled: input.enabled !== false,
      sort_order: suggestions.value.length + 1,
      created_at: now,
      updated_at: now,
    }

    suggestions.value = [...suggestions.value, item]
    save()
    return { ok: true, item }
  }

  function updateSuggestion(id: string, input: GhostSuggestionInput): GhostSuggestionSaveResult {
    ensureLoaded()
    const index = suggestions.value.findIndex(item => item.id === id)
    if (index === -1)
      return { ok: false, reason: '词条不存在' }

    const validation = validateInput(input, id)
    if (!validation.ok)
      return validation

    const current = suggestions.value[index]
    const item: GhostSuggestion = {
      ...current,
      key: validation.key,
      description: validation.description,
      enabled: input.enabled ?? current.enabled,
      updated_at: new Date().toISOString(),
    }

    suggestions.value = suggestions.value.map(existing => existing.id === id ? item : existing)
    save()
    return { ok: true, item }
  }

  function removeSuggestion(id: string): boolean {
    ensureLoaded()
    const beforeCount = suggestions.value.length
    suggestions.value = suggestions.value.filter(item => item.id !== id)
    if (suggestions.value.length === beforeCount)
      return false

    save()
    return true
  }

  function toggleSuggestion(id: string, enabled: boolean): boolean {
    ensureLoaded()
    const item = suggestions.value.find(suggestion => suggestion.id === id)
    if (!item)
      return false

    item.enabled = enabled
    item.updated_at = new Date().toISOString()
    save()
    return true
  }

  function replaceSuggestions(nextSuggestions: GhostSuggestion[]): boolean {
    ensureLoaded()
    const ids = new Set<string>()
    const keys = new Set<string>()

    for (const item of nextSuggestions) {
      const normalizedKey = normalizeGhostSuggestionKey(item.key)
      const lowerKey = normalizedKey.toLowerCase()
      if (!item.id || ids.has(item.id) || keys.has(lowerKey) || !isValidGhostSuggestionKey(normalizedKey))
        return false
      ids.add(item.id)
      keys.add(lowerKey)
    }

    suggestions.value = normalizeSort(nextSuggestions.map(item => ({ ...item })))
    save()
    return true
  }

  function reorderSuggestions(ids: string[]): boolean {
    ensureLoaded()
    if (ids.length !== suggestions.value.length || new Set(ids).size !== ids.length)
      return false

    const itemById = new Map(suggestions.value.map(item => [item.id, item]))
    const reordered = ids.map(id => itemById.get(id))
    if (reordered.some(item => !item))
      return false

    return replaceSuggestions(reordered as GhostSuggestion[])
  }

  return {
    suggestions,
    storeUpdatedAt,
    enabledSuggestions,
    ghostSuggestionsEnabled,
    load,
    save,
    setGhostSuggestionsEnabled,
    addSuggestion,
    updateSuggestion,
    removeSuggestion,
    toggleSuggestion,
    replaceSuggestions,
    reorderSuggestions,
  }
}
