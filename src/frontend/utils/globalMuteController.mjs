// Shared by the Vue adapter and behavior tests. Native events can arrive out of order.
export function createGlobalMuteController({ read, write, subscribe, onChange }) {
  let snapshot = [0, true]
  let initialization
  let subscription
  let toggles = Promise.resolve()

  function accept(next) {
    if (!Array.isArray(next) || !Number.isSafeInteger(next[0]) || next[0] < 1 || typeof next[1] !== 'boolean')
      throw new Error('Invalid global mute snapshot')
    if (next[0] > snapshot[0]) {
      snapshot = next
      onChange(next[1])
    }
    return snapshot[1]
  }

  async function initialize() {
    if (!initialization) {
      initialization = (async () => {
        subscription ??= subscribe(accept).catch((error) => {
          subscription = undefined
          throw error
        })
        await subscription
        return accept(await read())
      })().catch((error) => {
        initialization = undefined
        throw error
      })
    }
    return initialization
  }

  async function refresh() {
    await initialize()
    return accept(await read())
  }

  function toggle() {
    const operation = toggles.then(async () => {
      await refresh()
      return accept(await write(!snapshot[1]))
    })
    toggles = operation.catch(() => {})
    return operation
  }

  return { initialize, refresh, toggle }
}
