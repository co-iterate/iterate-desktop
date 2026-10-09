import { invoke } from '@tauri-apps/api/core'
import { computed, ref } from 'vue'

// ============================================================
// Types
// ============================================================

export interface PromptLibraryItem {
  id: string
  name: string
  content: string
  category: string
}

interface PromptLibraryStore {
  version: 1
  updatedAt: string
  items: PromptLibraryItem[]
}

interface ImportResult {
  imported: number
  skipped: number
  failedFiles: string[]
}

interface SearchResult extends PromptLibraryItem {
  score: number
}

// ============================================================
// Constants
// ============================================================

const STORAGE_KEY = 'iterate:prompt-library:v1'

// ============================================================
// Composable
// ============================================================

// ============================================================
// Singleton shared state (all callers share the same data)
// ============================================================

const items = ref<PromptLibraryItem[]>([])
const searchQuery = ref('')
const isSearchOpen = ref(false)
const isImporting = ref(false)
let _loaded = false

const saveError = ref('')
let savedItems: PromptLibraryItem[] = []
let pendingSave = Promise.resolve()

async function _load() {
  await pendingSave
  try {
    const content = await invoke<string>('load_prompt_library_file')
    const store: PromptLibraryStore = JSON.parse(content)
    items.value = store.items ?? []
    savedItems = structuredClone(store.items ?? [])
    localStorage.setItem(STORAGE_KEY, content)
    saveError.value = ''
  }
  catch (cause) { saveError.value = String(cause) }
}

function _save() {
  const snapshot = JSON.parse(JSON.stringify(items.value)) as PromptLibraryItem[]
  pendingSave = pendingSave.then(async () => {
    if (saveError.value)
      return
    const store: PromptLibraryStore = { version: 1, updatedAt: new Date().toISOString(), items: snapshot }
    await invoke('save_prompt_library_file', { content: JSON.stringify(store), expectedItems: savedItems })
    savedItems = snapshot
    localStorage.setItem(STORAGE_KEY, JSON.stringify(store))
  }).catch((cause: unknown) => { saveError.value = String(cause) })
}

export function usePromptLibrary() {
  // Alias internal functions for public API
  const load = _load
  const save = _save

  // ----------------------------------------------------------
  // Dedup key
  // ----------------------------------------------------------

  function dedupKey(item: Pick<PromptLibraryItem, 'category' | 'name' | 'content'>): string {
    return `${item.category.trim().toLowerCase()}|${item.name.trim().toLowerCase()}|${item.content.trim().toLowerCase()}`
  }

  // ----------------------------------------------------------
  // Import parser
  // ----------------------------------------------------------

  function extractCategory(filename: string): string {
    // prompts_编程_export (14).txt → 编程
    const match = filename.match(/^prompts_(.+?)_export/)
    if (match)
      return match[1]
    // fallback: use filename without extension
    return filename.replace(/\.[^.]+$/, '').replace(/prompts_?/, '').replace(/_?export.*/, '') || '未分类'
  }

  function parsePromptFile(text: string, category: string): PromptLibraryItem[] {
    // Normalize line endings, remove BOM
    const normalized = text.replace(/\uFEFF/, '').replace(/\r\n/g, '\n').trim()
    if (!normalized)
      return []

    const results: PromptLibraryItem[] = []

    // Strategy 1: split by blank lines
    const blocks = normalized.split(/\n\s*\n/)
    if (blocks.length > 1) {
      for (const block of blocks) {
        const lines = block.trim().split('\n')
        if (lines.length === 0)
          continue
        const name = lines[0].trim()
        const content = lines.slice(1).join('\n').trim()
        if (name) {
          results.push({
            id: crypto.randomUUID(),
            name,
            content: content || name,
            category,
          })
        }
      }
    }

    // Strategy 2 fallback: title/content pairs (every 2 lines)
    if (results.length === 0) {
      const lines = normalized.split('\n').filter(l => l.trim())
      for (let i = 0; i < lines.length; i += 2) {
        const name = lines[i]?.trim()
        const content = lines[i + 1]?.trim() || name || ''
        if (name) {
          results.push({
            id: crypto.randomUUID(),
            name,
            content,
            category,
          })
        }
      }
    }

    return results.filter(r => r.name && r.content)
  }

  async function importFiles(files: FileList): Promise<ImportResult> {
    isImporting.value = true
    const result: ImportResult = { imported: 0, skipped: 0, failedFiles: [] }

    const existingKeys = new Set(items.value.map(dedupKey))

    try {
      for (const file of Array.from(files)) {
        try {
          const text = await file.text()
          const category = extractCategory(file.name)
          const parsed = parsePromptFile(text, category)

          for (const item of parsed) {
            const key = dedupKey(item)
            if (existingKeys.has(key)) {
              result.skipped++
            }
            else {
              items.value.push(item)
              existingKeys.add(key)
              result.imported++
            }
          }
        }
        catch {
          result.failedFiles.push(file.name)
        }
      }

      if (result.imported > 0)
        save()
    }
    finally {
      isImporting.value = false
    }

    return result
  }

  // ----------------------------------------------------------
  // Search
  // ----------------------------------------------------------

  function scoreItem(item: PromptLibraryItem, query: string): number {
    const q = query.trim().toLowerCase()
    if (!q)
      return 0

    const keywords = q.split(/\s+/)
    let total = 0

    for (const kw of keywords) {
      let kwScore = 0
      const nameLower = item.name.toLowerCase()
      const contentLower = item.content.toLowerCase()
      const categoryLower = item.category.toLowerCase()

      if (nameLower.startsWith(kw))
        kwScore += 100
      else if (nameLower.includes(kw))
        kwScore += 60

      if (categoryLower.includes(kw))
        kwScore += 40

      if (contentLower.includes(kw))
        kwScore += 20

      total += kwScore
    }

    // Bonus for all keywords matching
    const allMatch = keywords.every(kw =>
      item.name.toLowerCase().includes(kw)
      || item.content.toLowerCase().includes(kw)
      || item.category.toLowerCase().includes(kw),
    )
    if (allMatch && keywords.length > 1)
      total += 50

    return total
  }

  const searchResults = computed<SearchResult[]>(() => {
    const q = searchQuery.value.trim()
    if (!q)
      return items.value.slice(0, 30).map(i => ({ ...i, score: 0 }))

    return items.value
      .map(item => ({ ...item, score: scoreItem(item, q) }))
      .filter(r => r.score > 0)
      .sort((a, b) => b.score - a.score || a.name.length - b.name.length)
      .slice(0, 30)
  })

  // ----------------------------------------------------------
  // Actions
  // ----------------------------------------------------------

  function toggleSearch() {
    isSearchOpen.value = !isSearchOpen.value
    if (!isSearchOpen.value)
      searchQuery.value = ''
  }

  function clearLibrary() {
    items.value = []
    save()
  }

  // ----------------------------------------------------------
  // CRUD
  // ----------------------------------------------------------

  function addItem(name: string, content: string, category: string): PromptLibraryItem | null {
    if (!name.trim() || !content.trim())
      return null
    const item: PromptLibraryItem = {
      id: crypto.randomUUID(),
      name: name.trim(),
      content: content.trim(),
      category: category.trim() || '未分类',
    }
    const key = dedupKey(item)
    if (items.value.some(i => dedupKey(i) === key))
      return null
    items.value.push(item)
    save()
    return item
  }

  function updateItem(id: string, updates: Partial<Pick<PromptLibraryItem, 'name' | 'content' | 'category'>>): boolean {
    const idx = items.value.findIndex(i => i.id === id)
    if (idx === -1)
      return false
    if (updates.name !== undefined)
      items.value[idx].name = updates.name.trim()
    if (updates.content !== undefined)
      items.value[idx].content = updates.content.trim()
    if (updates.category !== undefined)
      items.value[idx].category = updates.category.trim() || '未分类'
    save()
    return true
  }

  function deleteItem(id: string): boolean {
    const idx = items.value.findIndex(i => i.id === id)
    if (idx === -1)
      return false
    items.value.splice(idx, 1)
    save()
    return true
  }

  // ----------------------------------------------------------
  // Directory import (via Tauri read_dir + read_file)
  // ----------------------------------------------------------

  async function importFromDirectory(dirPath: string): Promise<ImportResult> {
    isImporting.value = true
    const result: ImportResult = { imported: 0, skipped: 0, failedFiles: [] }
    const existingKeys = new Set(items.value.map(dedupKey))

    try {
      const files: string[] = await invoke('list_prompt_files', { dirPath })
      for (const filePath of files) {
        try {
          const text: string = await invoke('read_text_file', { filePath })
          const filename = filePath.split('/').pop() || ''
          const category = extractCategory(filename)
          const parsed = parsePromptFile(text, category)

          for (const item of parsed) {
            const key = dedupKey(item)
            if (existingKeys.has(key)) {
              result.skipped++
            }
            else {
              items.value.push(item)
              existingKeys.add(key)
              result.imported++
            }
          }
        }
        catch {
          const filename = filePath.split('/').pop() || filePath
          result.failedFiles.push(filename)
        }
      }

      if (result.imported > 0)
        save()
    }
    catch (e) {
      console.error('[PromptLibrary] importFromDirectory failed:', e)
    }
    finally {
      isImporting.value = false
    }

    return result
  }

  // ----------------------------------------------------------
  // Categories
  // ----------------------------------------------------------

  const categories = computed(() => {
    const cats = new Set(items.value.map(i => i.category))
    return Array.from(cats).sort()
  })

  // Init: only load from localStorage once, then sync to file
  if (!_loaded) {
    _loaded = true // 防止重复调用
    void _load()
    window.addEventListener('iterate:settings-synced', () => {
      if (!saveError.value)
        void _load()
    })
  }

  return {
    saveError,
    items,
    searchQuery,
    isSearchOpen,
    isImporting,
    searchResults,
    categories,
    importFiles,
    importFromDirectory,
    toggleSearch,
    clearLibrary,
    addItem,
    updateItem,
    deleteItem,
    load,
    save,
  }
}
