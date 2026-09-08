export function nativeWindowCloseAction(hasRequest: boolean, resolving: boolean, interactionOnly: boolean): 'cancel' | 'ignore' | 'exit' {
  if (resolving)
    return 'ignore'
  if (hasRequest)
    return 'cancel'
  return interactionOnly ? 'ignore' : 'exit'
}
