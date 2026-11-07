<script setup lang="ts">
import type { GuideStepState } from './firstRunGuide'

// 一个引导步骤：标题 + 说明 + 操作区（插槽）+ 机器检测结果。未解锁时只显示标题。
const props = defineProps<{
  index: number
  title: string
  state: GuideStepState
  /** 已完成时显示的检测结论，例如「已收到来自 Claude Code 的调用」 */
  doneText?: string
}>()
</script>

<template>
  <section class="step-card" :class="`is-${props.state}`" :aria-current="props.state === 'active' ? 'step' : undefined">
    <header class="step-header">
      <span class="step-badge">
        <span v-if="props.state === 'done'" class="i-carbon-checkmark" />
        <span v-else>{{ props.index }}</span>
      </span>
      <h3 class="step-title">
        {{ props.title }}
      </h3>
      <span v-if="props.state === 'locked'" class="step-lock">完成上一步后解锁</span>
    </header>
    <p v-if="props.state === 'done' && props.doneText" class="step-done-text">
      {{ props.doneText }}
    </p>
    <div v-if="props.state === 'active'" class="step-body">
      <slot />
    </div>
  </section>
</template>

<style scoped>
.step-card {
  padding: 14px 16px;
  border: 1px solid var(--guide-border, #e4e4e7);
  border-radius: 14px;
  background: var(--guide-card-bg, #fff);
  transition: border-color 0.2s, opacity 0.2s;
}

.step-card.is-active {
  border-color: var(--guide-accent, #10a37f);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--guide-accent, #10a37f) 14%, transparent);
}

.step-card.is-locked {
  opacity: 0.5;
}

.step-header {
  display: flex;
  align-items: center;
  gap: 10px;
}

.step-badge {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: 50%;
  background: var(--guide-border, #e4e4e7);
  font-size: 12px;
  font-weight: 700;
}

.is-active .step-badge {
  background: var(--guide-accent, #10a37f);
  color: #fff;
}

.is-done .step-badge {
  background: color-mix(in srgb, var(--guide-accent, #10a37f) 18%, transparent);
  color: var(--guide-accent, #10a37f);
}

.step-title {
  flex: 1;
  margin: 0;
  font-size: 15px;
  font-weight: 650;
}

.step-lock {
  font-size: 12px;
  opacity: 0.8;
}

.step-done-text {
  margin: 6px 0 0 34px;
  font-size: 13px;
  color: var(--guide-accent, #10a37f);
}

.step-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-top: 12px;
  padding-left: 34px;
}
</style>
