<script setup lang="ts">
import { computed, ref } from 'vue'
import { createMockGuideDriver } from '../../components/onboarding/firstRunGuide'
import FirstRunGuide from '../../components/onboarding/FirstRunGuide.vue'

// 场景通过 ?guide= 切换：normal（默认）/ none（未检测到客户端）/ silent（AI 一直不来调用）
const scenario = ref(new URLSearchParams(window.location.search).get('guide') || 'normal')
const renderKey = ref(0)
const driver = computed(() => {
  void renderKey.value
  return createMockGuideDriver({
    noClients: scenario.value === 'none',
    callDelayMs: scenario.value === 'silent' ? null : 2500,
  })
})
const lastEvent = ref('')
</script>

<template>
  <div class="guide-preview">
    <div class="preview-bar">
      <span>模拟场景：</span>
      <n-button
        v-for="item in [['normal', '正常'], ['none', '没装客户端'], ['silent', 'AI 不来调用']]"
        :key="item[0]"
        size="tiny"
        :type="scenario === item[0] ? 'primary' : 'default'"
        @click="scenario = item[0]; renderKey += 1"
      >
        {{ item[1] }}
      </n-button>
      <span v-if="lastEvent" class="preview-event">事件：{{ lastEvent }}</span>
    </div>
    <FirstRunGuide
      :key="`${scenario}-${renderKey}`"
      :driver="driver"
      @finish="lastEvent = 'finish'"
      @skip="lastEvent = 'skip'"
      @open-mobile="lastEvent = 'openMobile'"
    />
  </div>
</template>

<style scoped>
.preview-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  padding: 8px 20px;
  font-size: 12px;
  opacity: 0.85;
}

.preview-event {
  margin-left: auto;
}
</style>
