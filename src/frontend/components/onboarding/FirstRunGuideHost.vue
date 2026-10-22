<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { createTauriGuideDriver, OPEN_GUIDE_EVENT, OPEN_MOBILE_PAIRING_EVENT, readGuideOutcome, writeGuideOutcome } from './firstRunGuide'
import FirstRunGuide from './FirstRunGuide.vue'

// 生产入口：只给「没走过引导、也还没有任何客户端接入」的用户自动弹出，
// 老用户不打扰；设置页可随时重新打开。
// 主窗口会内联显示 zhi 请求，所以收到测试调用后引导收起成悬浮条，
// 把窗口让给用户回复；点悬浮条回到引导，进度保留。
const driver = createTauriGuideDriver()
const visible = ref(false)
const collapsed = ref(false)
const renderKey = ref(0)

function open() {
  if (!visible.value)
    renderKey.value += 1
  visible.value = true
  collapsed.value = false
}

function close(outcome: 'done' | 'skipped') {
  writeGuideOutcome(outcome)
  visible.value = false
  collapsed.value = false
}

function openMobile() {
  close('done')
  window.dispatchEvent(new Event(OPEN_MOBILE_PAIRING_EVENT))
}

onMounted(async () => {
  window.addEventListener(OPEN_GUIDE_EVENT, open)
  if (readGuideOutcome())
    return
  try {
    const clients = await driver.listClients()
    if (clients.some(client => client.connected)) {
      writeGuideOutcome('existing-user')
      return
    }
  }
  catch (error) {
    // 读不到客户端状态时不主动打扰，用户仍可从设置页打开
    console.warn('首次引导：读取 AI 客户端状态失败', error)
    return
  }
  open()
})

onUnmounted(() => window.removeEventListener(OPEN_GUIDE_EVENT, open))
</script>

<template>
  <template v-if="visible">
    <div v-show="!collapsed" class="first-run-overlay">
      <div class="first-run-panel">
        <button type="button" class="collapse-button" @click="collapsed = true">
          先收起
        </button>
        <FirstRunGuide
          :key="renderKey"
          :driver="driver"
          @call-received="collapsed = true"
          @finish="close('done')"
          @skip="close('skipped')"
          @open-mobile="openMobile"
        />
      </div>
    </div>
    <button v-if="collapsed" type="button" class="guide-pill" @click="collapsed = false">
      <span class="pill-dot" />
      使用引导进行中 · 回复后点这里继续
    </button>
  </template>
</template>

<style scoped>
.first-run-overlay {
  position: fixed;
  inset: 0;
  z-index: 2000;
  overflow-y: auto;
  background: var(--color-surface, #fff);
}

:global(.dark) .first-run-overlay {
  background: var(--color-surface, #0a0a0a);
}

.first-run-panel {
  position: relative;
  min-height: 100%;
  padding: 12px 0 24px;
}

.collapse-button {
  position: absolute;
  top: 12px;
  left: 16px;
  padding: 4px 8px;
  border: 0;
  background: none;
  color: inherit;
  font-size: 12px;
  opacity: 0.6;
  cursor: pointer;
}

.guide-pill {
  position: fixed;
  right: 16px;
  bottom: 16px;
  z-index: 2000;
  display: inline-flex;
  gap: 8px;
  align-items: center;
  padding: 8px 14px;
  border: 0;
  border-radius: 999px;
  background: #10a37f;
  color: #fff;
  font-size: 13px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.2);
  cursor: pointer;
}

.pill-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #fff;
  animation: pill-pulse 1.2s ease-in-out infinite;
}

@keyframes pill-pulse {
  50% {
    opacity: 0.3;
  }
}

@media (prefers-reduced-motion: reduce) {
  .pill-dot {
    animation: none;
  }
}
</style>
