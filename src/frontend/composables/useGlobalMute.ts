import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { ref } from 'vue'
import { createGlobalMuteController } from '../utils/globalMuteController.mjs'

const MUTE_STORAGE_KEY = 'iterate.muted'
// Fail closed until the process-shared preference has been read.
export const isMuted = ref(true)

const controller = createGlobalMuteController({
  read: () => invoke<[number, boolean]>('get_global_mute', {
    legacyMuted: localStorage.getItem(MUTE_STORAGE_KEY) === 'true',
  }),
  write: (muted: boolean) => invoke<[number, boolean]>('set_global_mute', { muted }),
  subscribe: (accept: (snapshot: [number, boolean]) => void) =>
    listen<[number, boolean]>('global-mute-changed', event => accept(event.payload)),
  onChange: (muted: boolean) => {
    isMuted.value = muted
    localStorage.setItem(MUTE_STORAGE_KEY, String(muted))
  },
})

export const refreshGlobalMute = controller.refresh

export async function toggleMute() {
  try {
    await controller.toggle()
  }
  catch (error) {
    console.error('同步全局静音失败:', error)
  }
}
