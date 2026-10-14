<script setup lang="ts">
import { computed } from 'vue'
import { useCrossDevice } from '../../composables/useCrossDevice'

const { crossState, crossBusy, crossTitle, crossColor, toggleCrossDevice } = useCrossDevice()
const phase = computed(() => !crossState.value.enabled ? 'off' : crossState.value.connected && crossState.value.peer_enabled ? 'connected' : 'enabled')
</script>

<template>
  <n-button
    size="small" circle :loading="crossBusy"
    :title="crossTitle" :aria-label="crossTitle" :aria-pressed="crossState.enabled"
    :style="`background-color: ${crossColor} !important; color: #fff !important;`"
    :data-state="phase" data-testid="cross-device-toggle" @click="toggleCrossDevice"
  >
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <rect x="1" y="3" width="9" height="8" rx="1.2" />
      <path d="M5.5 11v3M2.5 14h6M10 7h4" />
      <rect x="14" y="8" width="9" height="8" rx="1.2" />
      <path d="M18.5 16v3M15.5 19h6" />
    </svg>
  </n-button>
</template>
