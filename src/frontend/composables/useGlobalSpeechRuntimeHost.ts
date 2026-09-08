import type { SpeechLayerIdentity, SpeechSnapshot } from '../services/globalSpeechSession'
import type {
  SpeechCorrectionMemoryEntry,
  SpeechMemoryEntry,
  SpeechVocabularyEntry,
} from '../services/speechContext'
import type { SpeechPostprocessResult } from '../services/speechPostprocess'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getDesktopSpeechRecognitionMode } from '../services/desktopSpeechRecognitionMode'
import { GlobalSpeechSessionGuard } from '../services/globalSpeechSession'
import { buildSpeechContextualStrings, extractSafeSpeechVocabularyTerms } from '../services/speechContext'
import { applySpeechPostprocess } from '../services/speechPostprocess'

interface ProcessTranscriptPayload {
  identity: SpeechLayerIdentity
  text: string
}

interface WindowsTranscriptPayload {
  targetToken: string
  text: string
}

export function useGlobalSpeechRuntimeHost() {
  const guard = new GlobalSpeechSessionGuard()
  let muscleMemoryEntries: SpeechMemoryEntry[] = []
  let correctionMemoryEntries: SpeechCorrectionMemoryEntry[] = []
  let vocabularyEntries: SpeechVocabularyEntry[] = []
  let contextualStrings: string[] = []
  let unlistenSnapshot: (() => void) | null = null
  let unlistenTranscript: (() => void) | null = null
  let unlistenWindowsTranscript: (() => void) | null = null
  let unlistenWindowsError: (() => void) | null = null
  let resourcesReady: Promise<void> = Promise.resolve()
  let initialized = false

  function rebuildContextualStrings() {
    contextualStrings = buildSpeechContextualStrings({
      muscleMemoryEntries,
      correctionMemoryEntries,
      rememberedTerms: vocabularyEntries.map(entry => entry.term),
    })
  }

  // 纠错/语义门禁的上下文与识别 hints 必须分离：hints 里包含纠错词条自身的 intendedText、
  // 静态领域词表和自动沉淀的词汇记忆，回流进 postprocess 会让上下文门禁自我满足，
  // 在任意话题下放行替换（P-2026-2125）。门禁只允许使用真实环境上下文；
  // 转写文本自身的词项由 applySpeechPostprocess 内部补充。当前请求/用户输入的
  // 跨进程上下文通道接通后，应从这里注入，而不是复用 contextualStrings。
  function buildCorrectionEligibilityTerms(): string[] {
    return []
  }

  async function preloadPermissions() {
    const checks = [
      ['microphone_status', 'request_microphone_permission'],
      ['speech_recognition_status', 'request_speech_recognition_permission'],
      ['accessibility_status', 'request_accessibility_permission'],
      ['input_monitoring_status', 'request_input_monitoring_permission'],
    ] as const
    for (const [statusCommand, requestCommand] of checks) {
      const granted = await invoke<boolean>(statusCommand).catch(() => false)
      if (!granted)
        await invoke(requestCommand).catch(() => undefined)
    }
  }

  async function refreshRecognitionResources() {
    const [muscleResult, correctionResult, vocabularyResult] = await Promise.all([
      invoke<unknown>('get_speech_muscle_memory_entries').catch(() => []),
      invoke<unknown>('get_speech_correction_memory_entries').catch(() => []),
      invoke<unknown>('get_speech_vocabulary_entries').catch(() => []),
    ])
    muscleMemoryEntries = Array.isArray(muscleResult) ? muscleResult as SpeechMemoryEntry[] : []
    correctionMemoryEntries = Array.isArray(correctionResult) ? correctionResult as SpeechCorrectionMemoryEntry[] : []
    vocabularyEntries = Array.isArray(vocabularyResult) ? vocabularyResult as SpeechVocabularyEntry[] : []
    rebuildContextualStrings()
  }

  async function configure(snapshot: SpeechSnapshot) {
    if (snapshot.phase !== 'Arming' || !snapshot.identity || !guard.claimDirective('configure', snapshot.identity))
      return
    const identity = snapshot.identity
    // 每次 Arming 重读记忆库：设置页、iPhone bridge 或其他进程的更新必须在下一次 Fn 立即生效，
    // 不能停留在初始化时的一次性快照上。
    resourcesReady = refreshRecognitionResources()
    await resourcesReady
    if (!guard.isCurrent(identity))
      return
    await invoke('configure_speech_recognition', {
      identity,
      contextualStrings,
      recognitionMode: getDesktopSpeechRecognitionMode(),
    }).catch(() => undefined)
  }

  function acceptSnapshot(snapshot: SpeechSnapshot) {
    if (!guard.applySnapshot(snapshot))
      return
    void configure(snapshot)
  }

  async function persistMemoryWriteback(processed: SpeechPostprocessResult) {
    const writes: Promise<unknown>[] = [
      invoke('append_speech_history_markdown', { text: processed.text }).catch(() => undefined),
    ]
    // 命中计数走 Rust 锁内原子自增，并用返回的最新表刷新缓存；
    // 禁止用本地缓存整表覆盖写回，否则会吞掉设置页 / bridge 的并发编辑。
    if (processed.correctionEntry) {
      writes.push(
        invoke<unknown>('record_speech_correction_memory_hit', {
          id: processed.correctionEntry.id ?? null,
          observedText: processed.correctionEntry.observedText ?? null,
          intendedText: processed.correctionEntry.intendedText ?? null,
        })
          .then((result) => {
            if (Array.isArray(result))
              correctionMemoryEntries = result as SpeechCorrectionMemoryEntry[]
          })
          .catch(() => undefined),
      )
    }
    if (processed.muscleEntry) {
      writes.push(
        invoke<unknown>('record_speech_muscle_memory_hit', {
          id: processed.muscleEntry.id ?? null,
          spokenPhrase: processed.muscleEntry.spokenPhrase ?? null,
        })
          .then((result) => {
            if (Array.isArray(result))
              muscleMemoryEntries = result as SpeechMemoryEntry[]
          })
          .catch(() => undefined),
      )
    }
    const vocabularyTerms = extractSafeSpeechVocabularyTerms(processed.text)
    if (vocabularyTerms.length > 0) {
      writes.push(
        invoke<unknown>('record_speech_vocabulary_terms', { terms: vocabularyTerms })
          .then((result) => {
            if (Array.isArray(result))
              vocabularyEntries = result as SpeechVocabularyEntry[]
          })
          .catch(() => undefined),
      )
    }
    await Promise.all(writes)
    rebuildContextualStrings()
  }

  async function processTranscript(payload: ProcessTranscriptPayload) {
    if (!guard.claimDirective('process', payload.identity))
      return
    const rawText = payload.text.trim()
    let finalText = rawText
    try {
      await resourcesReady
      if (!guard.isCurrent(payload.identity))
        return
      const processed = applySpeechPostprocess({
        text: rawText,
        muscleMemoryEntries,
        correctionMemoryEntries,
        contextTerms: buildCorrectionEligibilityTerms(),
      })
      finalText = processed.text
      await persistMemoryWriteback(processed)
    }
    catch {
      // 后处理或持久化失败不允许吞掉这段话：退回原始转写完成会话，
      // 否则 reducer 会停在 Processing 直到看门狗超时、用户的话直接丢失。
      finalText = rawText
    }
    if (!guard.isCurrent(payload.identity))
      return
    await invoke('complete_speech_processing', {
      identity: payload.identity,
      text: finalText,
    }).catch(() => undefined)
  }

  async function processWindowsTranscript(payload: WindowsTranscriptPayload) {
    const rawText = payload.text.trim()
    if (!rawText || !payload.targetToken)
      return

    let finalText = rawText
    try {
      await resourcesReady
      const processed = applySpeechPostprocess({
        text: rawText,
        muscleMemoryEntries,
        correctionMemoryEntries,
        contextTerms: buildCorrectionEligibilityTerms(),
      })
      finalText = processed.text
      await persistMemoryWriteback(processed)
    }
    catch {
      finalText = rawText
    }

    await invoke('commit_windows_speech_text', {
      targetToken: payload.targetToken,
      text: finalText,
    }).catch(error => console.error('Windows 语音写回失败:', error))
  }

  async function initialize() {
    if (initialized)
      return
    initialized = true
    const windowsPlatform = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('WIN')
    resourcesReady = refreshRecognitionResources()

    if (windowsPlatform) {
      unlistenWindowsTranscript = await listen<WindowsTranscriptPayload>('speech://windows-transcript', event => void processWindowsTranscript(event.payload))
      unlistenWindowsError = await listen<string>('speech://windows-error', event => console.warn('Windows 语音识别失败:', event.payload))
      return
    }
    unlistenSnapshot = await listen<SpeechSnapshot>('speech://session-snapshot', event => acceptSnapshot(event.payload))
    unlistenTranscript = await listen<ProcessTranscriptPayload>('speech://process-transcript', event => void processTranscript(event.payload))
    const snapshot = await invoke<SpeechSnapshot>('get_speech_control_snapshot')
    acceptSnapshot(snapshot)
    void preloadPermissions()
  }

  function dispose() {
    unlistenSnapshot?.()
    unlistenTranscript?.()
    unlistenWindowsTranscript?.()
    unlistenWindowsError?.()
    unlistenSnapshot = null
    unlistenTranscript = null
    unlistenWindowsTranscript = null
    unlistenWindowsError = null
    initialized = false
  }

  return { initialize, dispose }
}
