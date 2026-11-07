// 打字卡顿探针：默认关闭，只在消息里带 TYPING_PROBE_MARKER 的测试弹窗里启用。
// 记录超过阈值的卡帧、输入事件排队延迟，以及卡顿前刚跑过的定时任务名；不记录任何正文。
import { invoke } from '@tauri-apps/api/core'

export const TYPING_PROBE_MARKER = '<!-- typing-probe -->'
const STALL_MS = 50
const INPUT_DELAY_MS = 30
const MARK_WINDOW_MS = 250
const FLUSH_INTERVAL_MS = 5000

interface ProbeMark { at: number, name: string, ms?: number }

let active = false
let frameHandle: number | null = null
let flushTimer: ReturnType<typeof setInterval> | null = null
let lastFrameAt = 0
let marks: ProbeMark[] = []
let events: Array<Record<string, unknown>> = []
let inputCount = 0

function recentMarks(now: number) {
  return marks
    .filter(mark => now - mark.at <= MARK_WINDOW_MS)
    .map(mark => mark.ms === undefined ? mark.name : `${mark.name}:${Math.round(mark.ms)}ms`)
}

function onFrame(now: number) {
  if (!active)
    return
  if (lastFrameAt && now - lastFrameAt > STALL_MS)
    events.push({ kind: 'stall', gap_ms: Math.round(now - lastFrameAt), before: recentMarks(now) })
  lastFrameAt = now
  marks = marks.filter(mark => now - mark.at <= MARK_WINDOW_MS * 4)
  frameHandle = window.requestAnimationFrame(onFrame)
}

function flush() {
  if (events.length === 0 && inputCount === 0)
    return
  const payload = { inputs: inputCount, events: events.slice(0, 60) }
  events = []
  inputCount = 0
  void invoke('timeline_debug_log', { location: 'frontend/typing_probe', payload }).catch(() => {})
}

export function isTypingProbeActive() {
  return active
}

export function startTypingProbe() {
  if (active || typeof window === 'undefined')
    return
  active = true
  lastFrameAt = 0
  frameHandle = window.requestAnimationFrame(onFrame)
  flushTimer = setInterval(flush, FLUSH_INTERVAL_MS)
  void invoke('timeline_debug_log', { location: 'frontend/typing_probe', payload: { started: true } }).catch(() => {})
}

export function stopTypingProbe() {
  if (!active)
    return
  flush()
  active = false
  if (frameHandle !== null)
    window.cancelAnimationFrame(frameHandle)
  if (flushTimer)
    clearInterval(flushTimer)
  frameHandle = null
  flushTimer = null
}

// 定时任务、同步、测高等在开始时打点；ms 给出这段工作自身的同步耗时
export function markTypingProbe(name: string, ms?: number) {
  if (active)
    marks.push({ at: performance.now(), name, ms })
}

// 包一层异步任务：记录开始和整体耗时（含 await），便于和卡帧对齐
export async function probeTask<T>(name: string, task: () => Promise<T>): Promise<T> {
  if (!active)
    return task()
  const startedAt = performance.now()
  markTypingProbe(`${name}>`)
  try {
    return await task()
  }
  finally {
    markTypingProbe(`${name}<`, performance.now() - startedAt)
  }
}

// 输入事件从发生到被处理的排队时间：主线程忙时会变大
export function recordTypingProbeInput(event: Event) {
  if (!active)
    return
  inputCount += 1
  const delay = performance.now() - event.timeStamp
  if (delay > INPUT_DELAY_MS)
    events.push({ kind: 'input-delay', type: event.type, delay_ms: Math.round(delay), before: recentMarks(performance.now()) })
}
