// Measure outside the popup's flex layout. Temporarily shrinking the focused
// textarea can reset its scroll position and forces the whole popup to lay out.
// The caller schedules this only after typing is idle, never during composition.
export function createTextareaAutosizer(onResize: () => void) {
  let measure: HTMLTextAreaElement | null = null
  let observedInput: HTMLTextAreaElement | null = null
  let observer: ResizeObserver | null = null
  let lastStyle = ''
  let lastValue: string | null = null
  let contentHeight = 0

  function resize(input: HTMLTextAreaElement) {
    if (observedInput !== input) {
      observer?.disconnect()
      observer = new ResizeObserver(onResize)
      observer.observe(input)
      observedInput = input
    }
    const styles = window.getComputedStyle(input)
    const lineHeight = Number.parseFloat(styles.lineHeight) || 24
    const padding = Number.parseFloat(styles.paddingTop) + Number.parseFloat(styles.paddingBottom)
    const border = Number.parseFloat(styles.borderTopWidth) + Number.parseFloat(styles.borderBottomWidth)
    const width = input.clientWidth
    if (!width)
      return

    // In a roomy popup the flex layout already provides more than the maximum
    // content floor. No text measurement can change that layout. Observe size
    // changes so opening extras or narrowing the window rechecks the floor.
    if (input.clientHeight > lineHeight * 6 + padding)
      return

    if (!measure) {
      measure = document.createElement('textarea')
      measure.tabIndex = -1
      measure.readOnly = true
      measure.setAttribute('aria-hidden', 'true')
      measure.setAttribute('inert', '')
      document.body.appendChild(measure)
    }

    const measureStyle = [
      'position:fixed;left:-10000px;top:0;height:0;min-height:0;max-height:none',
      'visibility:hidden;pointer-events:none;overflow:hidden;resize:none',
      'contain:strict;box-sizing:border-box;border:0;margin:0',
      `width:${width}px`,
      `padding:${styles.paddingTop} ${styles.paddingRight} ${styles.paddingBottom} ${styles.paddingLeft}`,
      `font:${styles.font}`,
      `line-height:${styles.lineHeight}`,
      `letter-spacing:${styles.letterSpacing}`,
      `word-spacing:${styles.wordSpacing}`,
      `tab-size:${styles.tabSize}`,
      `white-space:${styles.whiteSpace}`,
      `overflow-wrap:${styles.overflowWrap}`,
      `word-break:${styles.wordBreak}`,
    ].join(';')

    if (lastStyle !== measureStyle || lastValue !== input.value) {
      if (lastStyle !== measureStyle)
        measure.style.cssText = measureStyle
      measure.value = input.value
      contentHeight = measure.scrollHeight
      lastStyle = measureStyle
      lastValue = input.value
    }

    const height = Math.max(lineHeight * 3 + padding, Math.min(contentHeight + border, lineHeight * 6 + padding))
    // Match CSSOM's serialized precision so fractional line heights don't cause
    // the same min-height to be written again on every idle callback.
    const minHeight = `${Math.round(height * 1000) / 1000}px`
    if (input.style.minHeight !== minHeight)
      input.style.minHeight = minHeight
  }

  function dispose() {
    observer?.disconnect()
    observer = null
    observedInput = null
    measure?.remove()
    measure = null
    lastStyle = ''
    lastValue = null
  }

  return { resize, dispose }
}
