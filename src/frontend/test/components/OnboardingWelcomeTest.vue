<script setup lang="ts">
import { ref } from 'vue'
import OnboardingWelcome from '../../components/common/OnboardingWelcome.vue'

const props = withDefaults(defineProps<{ showControls?: boolean }>(), { showControls: true })

const completedCount = ref(0)
const skippedCount = ref(0)

function handleComplete() {
  completedCount.value += 1
}

function handleSkip() {
  skippedCount.value += 1
}
</script>

<template>
  <div class="welcome-preview-page">
    <div v-if="props.showControls" class="preview-toolbar">
      <p class="preview-eyebrow">
        首次打开欢迎页
      </p>
      <h2 class="preview-title">
        真实 OnboardingWelcome 组件预览
      </h2>
      <p class="preview-description">
        这里直接挂真实组件。你可以点击“复制安装提示词”，也可以走完整个四步流程。
      </p>
      <p class="preview-note">
        已完成：{{ completedCount }} 次
      </p>
      <p class="preview-note">
        已跳过：{{ skippedCount }} 次
      </p>
    </div>

    <OnboardingWelcome
      :trial-days="7"
      @complete="handleComplete"
      @skip="handleSkip"
    />
  </div>
</template>

<style scoped>
.welcome-preview-page {
  min-height: 100vh;
  position: relative;
}

.preview-toolbar {
  position: fixed;
  top: 20px;
  right: 20px;
  z-index: 100001;
  width: min(320px, calc(100vw - 32px));
  padding: 18px 18px 16px;
  border-radius: 18px;
  background: rgba(23, 23, 23, 0.92);
  color: #fafafa;
  box-shadow: 0 18px 42px rgba(0, 0, 0, 0.28);
  backdrop-filter: blur(12px);
}

.preview-eyebrow {
  margin: 0 0 8px;
  font-size: 11px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  opacity: 0.72;
}

.preview-title {
  margin: 0 0 10px;
  font-size: 20px;
  line-height: 1.35;
}

.preview-description,
.preview-note {
  margin: 0 0 10px;
  font-size: 13px;
  line-height: 1.65;
  color: rgba(250, 250, 250, 0.82);
}
</style>
