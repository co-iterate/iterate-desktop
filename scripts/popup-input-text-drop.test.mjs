import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import { insertDroppedText } from '../src/frontend/utils/text-drop.mjs'

const popupInputSource = readFileSync(new URL('../src/frontend/components/popup/PopupInput.vue', import.meta.url), 'utf8')
const builderSource = readFileSync(new URL('../src/rust/app/builder.rs', import.meta.url), 'utf8')
const macosTextDropSource = readFileSync(new URL('../src/rust/ui/macos_text_drop.rs', import.meta.url), 'utf8')

test('inserts dropped text into an empty input', () => {
  assert.deepEqual(insertDroppedText('', 'hello', 0, 0), { value: 'hello', cursor: 5 })
})

test('inserts dropped text at a collapsed caret', () => {
  assert.deepEqual(insertDroppedText('abcd', 'XY', 2, 2), { value: 'abXYcd', cursor: 4 })
})

test('replaces the selected range', () => {
  assert.deepEqual(insertDroppedText('abcdef', 'XY', 2, 5), { value: 'abXYf', cursor: 4 })
})

test('normalizes reversed selection bounds', () => {
  assert.deepEqual(insertDroppedText('abcdef', 'XY', 5, 2), { value: 'abXYf', cursor: 4 })
})

test('clamps selection bounds to the current text', () => {
  assert.deepEqual(insertDroppedText('abc', 'X', -10, 50), { value: 'X', cursor: 1 })
})

test('falls back to the end when selection bounds are unavailable', () => {
  assert.deepEqual(insertDroppedText('abc', 'X'), { value: 'abcX', cursor: 4 })
})

test('preserves dropped whitespace exactly', () => {
  assert.deepEqual(insertDroppedText('ab', ' \n ', 1, 1), { value: 'a \n b', cursor: 4 })
})

test('wires text dragover and drop into PopupInput', () => {
  assert.match(popupInputSource, /import \{ insertDroppedText \} from ['"]\.\.\/\.\.\/utils\/text-drop\.mjs['"]/)
  assert.match(popupInputSource, /@dragover="handleInputDragOver"/)
  assert.match(popupInputSource, /@drop="handleInputDrop"/)
  assert.match(popupInputSource, /getData\(['"]text\/plain['"]\)/)
})

test('uses textarea selection and restores the inserted caret', () => {
  assert.match(popupInputSource, /selectionStart/)
  assert.match(popupInputSource, /selectionEnd/)
  assert.match(popupInputSource, /writeUserInput\(result\.value\)/)
  assert.match(popupInputSource, /setSelectionRange\(result\.cursor, result\.cursor\)/)
})

test('captures the live macOS dragging pasteboard and bridges only empty-path drops', () => {
  assert.match(builderSource, /\.on_window_event\(/)
  assert.doesNotMatch(builderSource, /\.on_webview_event\(forward_macos_native_text_drop\)/)
  assert.match(builderSource, /WindowEvent::DragDrop\(DragDropEvent::Drop\s*\{\s*paths,\s*position\s*\}\)/)
  assert.match(builderSource, /take_main_webview_drop_text\(\)/)
  assert.match(builderSource, /paths\.is_empty\(\)/)
  assert.match(builderSource, /popup:\/\/native-text-drop/)

  const pasteboardTake = builderSource.indexOf('take_main_webview_drop_text()')
  const emptyPathGuard = builderSource.indexOf('paths.is_empty()')
  assert.ok(pasteboardTake !== -1 && pasteboardTake < emptyPathGuard)

  assert.match(builderSource, /install_main_webview_text_drop_capture/)
  assert.match(macosTextDropSource, /performDragOperation:/)
  assert.match(macosTextDropSource, /draggingPasteboard/)
  assert.match(macosTextDropSource, /NSPasteboardTypeString/)
  assert.match(macosTextDropSource, /pending_text\(\)\.replace/)
  assert.doesNotMatch(macosTextDropSource, /generalPasteboard/)
  assert.doesNotMatch(macosTextDropSource, /NSDragPboard/)
})

test('uses Wry AppKit point coordinates directly for textarea hit testing', () => {
  assert.match(popupInputSource, /popup:\/\/native-text-drop/)
  assert.match(popupInputSource, /payload\.logicalPosition\.x >= bounds\.left/)
  assert.match(popupInputSource, /payload\.logicalPosition\.y >= bounds\.top/)
  const nativeDropHandler = popupInputSource.slice(
    popupInputSource.indexOf('async function handleNativeTextDrop'),
    popupInputSource.indexOf('function resetSuggestionIndex'),
  )
  assert.doesNotMatch(nativeDropHandler, /scaleFactor\(\)/)
  assert.doesNotMatch(nativeDropHandler, /toLogical/)
  assert.match(popupInputSource, /getBoundingClientRect\(\)/)
  assert.match(popupInputSource, /applyDroppedText\(payload\.text\)/)
})

test('keeps nonempty native paths on the attachment path', () => {
  const dragDropListener = popupInputSource.slice(popupInputSource.indexOf('async function setupInputDragDropListener'))
  assert.match(dragDropListener, /payload\.type === ['"]drop['"]/)
  assert.match(dragDropListener, /addAttachmentPaths\(payload\.paths\)/)
  assert.doesNotMatch(dragDropListener, /read_text_file/)
  assert.doesNotMatch(popupInputSource, /tryInsertRelearnTextDragExport/)
})
