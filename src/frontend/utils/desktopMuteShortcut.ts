type EscapeEvent = Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'repeat' | 'isComposing' | 'defaultPrevented'>

// Called only by the focused document, never registered as a system-wide Escape key.
export function isDesktopMuteToggle(event: EscapeEvent, popupVisible: boolean): boolean {
  return popupVisible && event.key === 'Escape'
    && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey
    && !event.repeat && !event.isComposing && !event.defaultPrevented
}
