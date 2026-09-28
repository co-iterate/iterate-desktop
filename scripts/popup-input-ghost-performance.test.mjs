import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
// This source-contract test intentionally runs with the repository's Node test scripts.
// eslint-disable-next-line test/no-import-node-test
import test from 'node:test'

const popupInputSource = readFileSync(
  new URL('../src/frontend/components/popup/PopupInput.vue', import.meta.url),
  'utf8',
).replace(/\r\n/g, '\n')
const popupStylesSource = readFileSync(
  new URL('../src/frontend/assets/styles/style.css', import.meta.url),
  'utf8',
)
const popupContentSource = readFileSync(
  new URL('../src/frontend/components/popup/PopupContent.vue', import.meta.url),
  'utf8',
)
const ghostSuggestionsSource = readFileSync(
  new URL('../src/frontend/composables/useGhostSuggestions.ts', import.meta.url),
  'utf8',
)
const popupHeaderSource = readFileSync(
  new URL('../src/frontend/components/popup/PopupHeader.vue', import.meta.url),
  'utf8',
)
const mcpPopupSource = readFileSync(
  new URL('../src/frontend/components/popup/McpPopup.vue', import.meta.url),
  'utf8',
)

test('ordinary CRUD input performs no DOM measurement or ghost scheduling', () => {
  const watcher = popupInputSource.match(/watch\(userInput,[\s\S]*?\n\}\)\n/)?.[0] ?? ''

  assert.doesNotMatch(watcher, /scheduleGhostMetricsSync\(/)
  assert.doesNotMatch(watcher, /syncGhostMetrics\(\)/)
  assert.doesNotMatch(watcher, /scrollHeight|clientHeight|getComputedStyle|getBoundingClientRect/)
})

test('main editor is a native textarea without Naive UI or field-sizing', () => {
  const mainInputClassIndex = popupInputSource.indexOf('class="popup-main-input"')
  const mainInputStart = popupInputSource.lastIndexOf('<textarea', mainInputClassIndex)
  const mainInputEnd = popupInputSource.indexOf('/>', mainInputClassIndex)
  const mainInput = popupInputSource.slice(mainInputStart, mainInputEnd + 2)

  assert.match(mainInput, /^<textarea/)
  assert.doesNotMatch(mainInput, /<n-input|:autosize=|field-sizing/)
  assert.match(mainInput, /rows="3"/)
  assert.doesNotMatch(popupStylesSource, /field-sizing\s*:/)
  assert.match(popupStylesSource, /\.popup-main-input\s*\{[\s\S]*?min-height:/)
  assert.match(popupStylesSource, /max-height:\s*calc\(6 \* 1\.6em \+ 18px\)/)
})

test('Insert Backspace Delete replace and clear share the native input path', () => {
  const keydownHandler = popupInputSource.match(/function handleInputKeydown\(event: KeyboardEvent\) \{[\s\S]*?\n\}/)?.[0] ?? ''
  const inputWatcher = popupInputSource.match(/watch\(userInput,[\s\S]*?\n\}\)\n/)?.[0] ?? ''

  assert.doesNotMatch(keydownHandler, /Backspace|Delete|event\.key\.length === 1/)
  assert.doesNotMatch(inputWatcher, /scrollHeight|clientHeight/)
})

test('scroll tracking reads only scroll position and does not measure geometry', () => {
  const scrollHandler = popupInputSource.match(/function handleTextareaScroll\(event: Event\) \{[\s\S]*?\n\}/)?.[0] ?? ''

  assert.match(scrollHandler, /scrollTop/)
  assert.doesNotMatch(scrollHandler, /scrollHeight|clientHeight|getComputedStyle|getBoundingClientRect/)
  assert.match(popupInputSource, /@scroll="handleTextareaScroll"/)
})

test('ghost metric scheduling coalesces work into one animation frame', () => {
  assert.match(popupInputSource, /if \(ghostMetricsFrame !== null\)\s*return/)
  assert.match(popupInputSource, /ghostMetricsFrame = window\.requestAnimationFrame\(run\)/)
  assert.match(popupInputSource, /cancelGhostMetricsSync\(\)/)
})

test('focus animation frame is cancelled during teardown', () => {
  assert.match(popupInputSource, /let localFocusFrame: number \| null = null/)
  assert.match(popupInputSource, /window\.cancelAnimationFrame\(localFocusFrame\)/)
  assert.match(popupInputSource, /localFocusFrame = window\.requestAnimationFrame/)
})

test('full layout measurements only write changed ghost styles', () => {
  assert.match(popupInputSource, /const ghostMetricsStyle = ref\(''\)/)
  assert.match(
    popupInputSource,
    /if \(ghostMetricsStyle\.value !== nextMetricsStyle\)\s*ghostMetricsStyle\.value = nextMetricsStyle/,
  )
})

test('resize still requests a coalesced ghost layout refresh', () => {
  assert.match(
    popupInputSource,
    /function handleGhostMetricsResize\(\) \{\s*scheduleGhostMetricsSync\(\)\s*scheduleTextareaAutosize\(0\)\s*\}/,
  )
  assert.match(popupInputSource, /window\.addEventListener\('resize', handleGhostMetricsResize\)/)
  assert.match(popupInputSource, /window\.removeEventListener\('resize', handleGhostMetricsResize\)/)
})

test('IME composition suppresses ghost suggestions without a measurement path', () => {
  assert.match(popupInputSource, /if \(!ghostSuggestionsEnabled\.value\)\s*return \[\]/)
  assert.match(popupInputSource, /if \(!token \|\| isComposing\.value\)\s*return \[\]/)
  assert.match(popupInputSource, /@compositionstart="handleCompositionStart"/)
  assert.match(popupInputSource, /@compositionend="handleCompositionEnd"/)
  const compositionStart = popupInputSource.match(/function handleCompositionStart\(\) \{[\s\S]*?\n\}/)?.[0] ?? ''
  assert.match(compositionStart, /cancelTextareaAutosize\(\)/)
  assert.doesNotMatch(compositionStart, /scrollHeight|clientHeight|getComputedStyle|getBoundingClientRect/)
})

test('textarea autosize is deferred until typing is idle or composition ends', () => {
  const watcher = popupInputSource.match(/watch\(userInput,[\s\S]*?\n\}\)\n/)?.[0] ?? ''
  const scheduler = popupInputSource.match(/function scheduleTextareaAutosize\([\s\S]*?\n\}/)?.[0] ?? ''

  assert.match(watcher, /scheduleTextareaAutosize\(\)/)
  assert.doesNotMatch(watcher, /resizeTextareaNow\(\)|scrollHeight|clientHeight/)
  assert.match(scheduler, /setTimeout\(/)
  assert.match(scheduler, /requestAnimationFrame/)
  assert.match(popupInputSource, /const TEXTAREA_AUTOSIZE_IDLE_MS = 120/)
  assert.match(popupInputSource, /scheduleTextareaAutosize\(0\)/)
})

test('ghost completion uses solitary left Option to enable and right Option to disable without breaking selection shortcuts', () => {
  assert.match(ghostSuggestionsSource, /iterate:ghost-suggestions:enabled/)
  assert.match(ghostSuggestionsSource, /function setGhostSuggestionsEnabled\(enabled: boolean\)/)
  assert.match(popupInputSource, /function handleOptionGhostControlKeydown\(event: KeyboardEvent\)/)
  assert.match(popupInputSource, /function handleOptionGhostControlKeyup\(event: KeyboardEvent\)/)
  assert.match(popupInputSource, /event\.code === 'AltLeft' \|\| event\.code === 'AltRight'/)
  assert.match(popupInputSource, /const enabled = optionCode === 'AltLeft'/)
  assert.match(popupInputSource, /if \(event\.altKey \|\| pressedGhostControlOptionCodes\.size > 0\)\s*optionGhostControlCandidate = null/)
  assert.match(popupInputSource, /pressedGhostControlOptionCodes\.size === 1/)
  assert.match(popupInputSource, /setGhostSuggestionsEnabled\(enabled\)/)
  assert.match(popupInputSource, /window\.addEventListener\('keydown', handleOptionGhostControlKeydown\)/)
  assert.match(popupInputSource, /window\.addEventListener\('keyup', handleOptionGhostControlKeyup\)/)
  assert.match(popupInputSource, /window\.removeEventListener\('keydown', handleOptionGhostControlKeydown\)/)
  assert.match(popupInputSource, /window\.removeEventListener\('keyup', handleOptionGhostControlKeyup\)/)
  assert.match(popupInputSource, /function handlePromptShortcutModifierKeydown\(event: KeyboardEvent\)/)
  assert.match(popupInputSource, /function handlePromptShortcut\(event: KeyboardEvent\)/)
  assert.doesNotMatch(popupInputSource, /handleCtrlGhostToggle|ctrlGhostToggleCandidate/)
  assert.doesNotMatch(popupContentSource, /幽灵补全全局开关|<span>补全开<\/span>|<span>补全关<\/span>/)
  assert.doesNotMatch(popupContentSource, />IDE<|>Web<|setSendTarget/)
  assert.doesNotMatch(mcpPopupSource, /SEND_TARGET_KEY|sendTarget\.value/)
  assert.match(popupHeaderSource, /invoke\('open_terminal', \{ cwd: props\.projectPath \}\)/)
  assert.match(popupHeaderSource, /title="在当前项目打开终端"/)
})
