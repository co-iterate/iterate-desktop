<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useNotification } from '../../composables/useNotification'
import { SETUP_PROMPT_CONTENT } from '../../constants/setupPrompt'
import McpClientSettings from '../settings/McpClientSettings.vue'

const props = defineProps<{
  trialDays?: number
}>()

const emit = defineEmits<{
  (e: 'complete'): void
  (e: 'skip'): void
}>()

const totalSteps = 4
const currentStep = ref(1)
const setupCopyText = ref('复制安装提示词')

const notification = useNotification()

const stepMeta = [
  {
    eyebrow: 'Trial',
    title: '试用已开启',
    kicker: '先感受到价值，再决定是否激活。',
  },
  {
    eyebrow: 'Core Path',
    title: '先掌握两个入口',
    kicker: '第一天只学核心路径，不会被功能淹没。',
  },
  {
    eyebrow: 'Keep Flowing',
    title: '通知负责把流程接回来',
    kicker: '开启后，AI 有结果时你不会错过继续入口。',
  },
  {
    eyebrow: 'Activation',
    title: '试用结束后再完成激活',
    kicker: '先开始，付费动作放到真正需要的时候。',
  },
] as const

const stepDots = computed(() => Array.from({ length: totalSteps }, (_, index) => index + 1))
const currentStepMeta = computed(() => stepMeta[currentStep.value - 1])
const notificationStatusLabel = computed(() => {
  switch (notification.permissionStatus.value) {
    case 'granted':
      return '已开启'
    case 'denied':
      return '已拒绝'
    default:
      return '未设置'
  }
})
const trialDaysLabel = computed(() => props.trialDays || 7)
const canGoPrev = computed(() => currentStep.value > 1)
const isLastStep = computed(() => currentStep.value === totalSteps)
const notificationActionLabel = computed(() => {
  if (notification.permissionStatus.value === 'granted')
    return '通知已开启'
  if (notification.permissionStatus.value === 'denied')
    return '系统已拒绝通知'
  return '开启通知'
})
const notificationActionDisabled = computed(() => notification.permissionStatus.value !== 'default')

function nextStep() {
  if (currentStep.value < totalSteps)
    currentStep.value += 1
}

function prevStep() {
  if (currentStep.value > 1)
    currentStep.value -= 1
}

function handleSkip() {
  emit('skip')
}

function handleComplete() {
  emit('complete')
}

async function requestNotificationPermission() {
  await notification.requestPermission()
}

async function copySetupPrompt() {
  try {
    await navigator.clipboard.writeText(SETUP_PROMPT_CONTENT)
    setupCopyText.value = '已复制'
    setTimeout(() => {
      setupCopyText.value = '复制安装提示词'
    }, 2000)
  }
  catch (error) {
    setupCopyText.value = '复制失败'
    setTimeout(() => {
      setupCopyText.value = '复制安装提示词'
    }, 2000)
    console.error('复制安装提示词失败:', error)
  }
}

onMounted(() => {
  notification.init()
})
</script>

<template>
  <div class="onboarding-overlay">
    <div class="onboarding-card">
      <div class="card-background" />

      <div class="onboarding-topbar">
        <div class="brand-lockup">
          <img src="/icons/icon-128.png" alt="iterate" class="onboarding-logo">
          <div>
            <p class="onboarding-badge">
              Iterate Welcome
            </p>
            <p class="onboarding-badge-subtitle">
              新用户最短上手路径
            </p>
          </div>
        </div>

        <div class="step-counter">
          <span class="step-counter-label">Step</span>
          <span class="step-counter-value">{{ currentStep }}/{{ totalSteps }}</span>
        </div>
      </div>

      <div class="steps-indicator">
        <div
          v-for="step in stepDots"
          :key="step"
          class="step-dot"
          :class="{ active: step === currentStep }"
        />
      </div>

      <div class="onboarding-content">
        <div class="content-frame">
          <p class="frame-eyebrow">
            {{ currentStepMeta.eyebrow }}
          </p>
          <h1 class="onboarding-title">
            {{ currentStepMeta.title }}
          </h1>
          <p class="onboarding-subtitle">
            {{ currentStepMeta.kicker }}
          </p>
        </div>

        <div v-if="currentStep === 1" class="step-panel">
          <h2 class="step-title">
            你已自动开始 {{ trialDaysLabel }} 天试用
          </h2>
          <p class="step-description">
            现在不需要购买，也不需要先输入激活码。你可以先直接体验 Iterate 的核心流程，试用结束后再决定是否继续激活使用。
          </p>

          <div class="hero-stage">
            <div class="hero-note">
              <p>先试用，后付费。先感受到价值，再进入激活流程。</p>
            </div>

            <div class="metric-strip">
              <div class="metric-card">
                <span class="metric-label">当前状态</span>
                <span class="metric-value">已进入试用</span>
              </div>
              <div class="metric-card accent">
                <span class="metric-label">建议动作</span>
                <span class="metric-value">直接开始使用</span>
              </div>
            </div>

            <div class="setup-copy-card setup-connect-card">
              <div class="setup-copy-copy">
                <p class="setup-copy-title">
                  把 iterate 接入你正在用的 AI 客户端
                </p>
                <p class="setup-copy-description">
                  已勾选本机检测到的客户端，确认后点一次即可，之后 iterate 会自动维护这份配置。
                </p>
              </div>

              <McpClientSettings compact />
            </div>

            <div class="setup-copy-card">
              <div class="setup-copy-copy">
                <p class="setup-copy-title">
                  用的是其他 IDE 或 CLI？
                </p>
                <p class="setup-copy-description">
                  把安装提示词发给你当前正在使用的 AI。它会继续帮你配置 MCP、刷新当前 IDE 或 CLI，并做最小验证。
                </p>
              </div>

              <button class="setup-copy-btn" @click="copySetupPrompt">
                {{ setupCopyText }}
              </button>
            </div>
          </div>
        </div>

        <div v-else-if="currentStep === 2" class="step-panel">
          <h2 class="step-title">
            先从两个核心入口开始
          </h2>
          <p class="step-description">
            你不需要先学会所有功能。第一天先掌握最常用的两个入口，就足够开始使用。
          </p>

          <div class="path-ribbon">
            <div class="path-chip active">
              <span class="path-index">01</span>
              <span>主对话</span>
            </div>
            <div class="path-connector" />
            <div class="path-chip">
              <span class="path-index">02</span>
              <span>弹窗继续</span>
            </div>
          </div>

          <div class="feature-grid">
            <div class="feature-card">
              <p class="feature-eyebrow">
                Core Path
              </p>
              <h3 class="feature-title">
                主对话入口
              </h3>
              <p class="feature-description">
                大多数任务、问题和连续对话，都从这里开始。
              </p>
            </div>

            <div class="feature-card">
              <p class="feature-eyebrow">
                Continue
              </p>
              <h3 class="feature-title">
                弹窗中断后继续
              </h3>
              <p class="feature-description">
                当流程被打断时，你可以直接从弹窗回到当前上下文继续，而不用重新解释一遍。
              </p>
            </div>
          </div>
        </div>

        <div v-else-if="currentStep === 3" class="step-panel">
          <h2 class="step-title">
            打开通知，别错过继续入口
          </h2>
          <p class="step-description">
            当 AI 返回结果或需要你继续时，Iterate 会通过弹窗和系统通知提醒你。开启通知后，继续流程会更顺。
          </p>

          <div class="permission-stage">
            <div class="permission-row">
              <div class="permission-label">
                <p>通知权限</p>
              </div>
              <div class="permission-value" :class="notification.permissionStatus.value">
                <p>{{ notificationStatusLabel }}</p>
              </div>
            </div>

            <div class="permission-note">
              <p>你也可以稍后在应用设置里调整静音和提示音，不会影响现在开始使用。</p>
            </div>

            <button
              class="permission-btn"
              :disabled="notificationActionDisabled"
              @click="requestNotificationPermission"
            >
              {{ notificationActionLabel }}
            </button>
          </div>
        </div>

        <div v-else class="step-panel">
          <h2 class="step-title">
            试用结束后，再激活继续使用
          </h2>
          <p class="step-description">
            现在你可以直接开始正常使用；试用结束后，系统会提示你购买并输入激活码，激活成功后即可继续使用。
          </p>

          <div class="activation-stage">
            <div class="flow-row">
              <div class="flow-card">
                <span class="flow-index">01</span>
                <p>开始试用</p>
              </div>
              <div class="flow-arrow" />
              <div class="flow-card">
                <span class="flow-index">02</span>
                <p>试用到期</p>
              </div>
              <div class="flow-arrow" />
              <div class="flow-card">
                <span class="flow-index">03</span>
                <p>输入激活码</p>
              </div>
            </div>

            <div class="hero-note compact">
              <p>这一步不是现在要完成的任务，只是提前告诉你后续路径，不让到期时突然懵掉。</p>
            </div>
          </div>
        </div>
      </div>

      <div class="footer-actions">
        <div class="footer-meta">
          <p class="footer-label">
            推荐用时
          </p>
          <p class="footer-value">
            30 秒走完开始路径
          </p>
        </div>

        <div class="footer-buttons">
          <button
            class="action-btn secondary"
            :disabled="!canGoPrev"
            @click="prevStep"
          >
            上一步
          </button>

          <button
            v-if="!isLastStep"
            class="action-btn ghost"
            @click="handleSkip"
          >
            跳过
          </button>

          <button
            v-if="!isLastStep"
            class="action-btn primary"
            @click="nextStep"
          >
            下一步
          </button>

          <button
            v-else
            class="action-btn primary"
            @click="handleComplete"
          >
            开始使用
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.onboarding-overlay {
  position: fixed;
  inset: 0;
  z-index: 99998;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background:
    radial-gradient(circle at top left, rgba(255, 255, 255, 0.08), transparent 32%),
    radial-gradient(circle at bottom right, rgba(255, 255, 255, 0.04), transparent 28%),
    rgba(8, 8, 8, 0.92);
  backdrop-filter: blur(14px);
}

.onboarding-card {
  position: relative;
  overflow: hidden;
  width: min(92vw, 560px);
  min-height: 560px;
  display: flex;
  flex-direction: column;
  padding: 28px;
  border-radius: 24px;
  border: 1px solid rgba(255, 255, 255, 0.09);
  background:
    linear-gradient(180deg, rgba(18, 18, 18, 0.98), rgba(10, 10, 10, 0.98));
  box-shadow:
    0 28px 80px rgba(0, 0, 0, 0.48),
    inset 0 1px 0 rgba(255, 255, 255, 0.06);
}

.card-background {
  position: absolute;
  inset: 0;
  background:
    radial-gradient(circle at top center, rgba(255, 255, 255, 0.05), transparent 36%),
    linear-gradient(135deg, rgba(255, 255, 255, 0.04), transparent 55%);
  pointer-events: none;
}

.onboarding-topbar,
.steps-indicator,
.onboarding-content,
.footer-actions {
  position: relative;
  z-index: 1;
}

.onboarding-topbar {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}

.brand-lockup {
  display: flex;
  align-items: center;
  gap: 12px;
}

.onboarding-logo {
  width: 54px;
  height: 54px;
  border-radius: 14px;
  box-shadow: 0 10px 24px rgba(0, 0, 0, 0.32);
}

.onboarding-badge {
  margin: 0;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: rgba(255, 255, 255, 0.62);
}

.onboarding-badge-subtitle {
  margin: 4px 0 0;
  font-size: 12px;
  line-height: 1.5;
  color: rgba(255, 255, 255, 0.42);
}

.step-counter {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 10px 14px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.1);
}

.step-counter-label {
  font-size: 11px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: rgba(255, 255, 255, 0.46);
}

.step-counter-value {
  font-size: 18px;
  font-weight: 700;
  color: #ffffff;
}

.steps-indicator {
  display: flex;
  justify-content: center;
  gap: 8px;
  margin: 18px 0 20px;
}

.step-dot {
  width: 8px;
  height: 8px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.12);
  transition: all 0.2s ease;
}

.step-dot.active {
  width: 26px;
  background: #ffffff;
}

.onboarding-content {
  flex: 1;
  display: flex;
  flex-direction: column;
}

.content-frame {
  margin-bottom: 20px;
  padding: 18px 20px;
  border-radius: 20px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  background: rgba(255, 255, 255, 0.04);
}

.frame-eyebrow {
  margin: 0 0 10px;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: rgba(255, 255, 255, 0.48);
}

.onboarding-title {
  margin: 0;
  font-size: 30px;
  line-height: 1.2;
  color: #ffffff;
}

.onboarding-subtitle {
  margin: 10px 0 0;
  font-size: 14px;
  line-height: 1.65;
  color: rgba(255, 255, 255, 0.62);
}

.step-panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
  animation: panel-enter 220ms ease;
}

.step-title {
  margin: 0;
  font-size: 22px;
  line-height: 1.3;
  color: #ffffff;
}

.step-description {
  margin: 0;
  font-size: 14px;
  line-height: 1.75;
  color: rgba(255, 255, 255, 0.64);
}

.hero-stage,
.permission-stage,
.activation-stage {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.hero-note,
.permission-note {
  padding: 16px 18px;
  border-radius: 18px;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.08);
}

.hero-note p,
.permission-note p {
  margin: 0;
  font-size: 13px;
  line-height: 1.65;
  color: rgba(255, 255, 255, 0.68);
}

.hero-note.compact p {
  font-size: 12px;
}

.metric-strip {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}

.setup-copy-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 18px;
  border-radius: 18px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(255, 255, 255, 0.04);
}

.setup-connect-card {
  flex-direction: column;
  align-items: stretch;
}

.setup-copy-copy {
  min-width: 0;
}

.setup-copy-title {
  margin: 0 0 6px;
  font-size: 14px;
  line-height: 1.45;
  color: #ffffff;
  font-weight: 700;
}

.setup-copy-description {
  margin: 0;
  font-size: 13px;
  line-height: 1.7;
  color: rgba(255, 255, 255, 0.62);
}

.setup-copy-btn {
  flex-shrink: 0;
  padding: 10px 16px;
  border: 1px solid rgba(255, 255, 255, 0.16);
  border-radius: 12px;
  background: #ffffff;
  color: #090909;
  font-size: 13px;
  font-weight: 700;
  cursor: pointer;
  transition: background 0.2s ease, transform 0.2s ease;
}

.setup-copy-btn:hover {
  background: #e9e9e9;
  transform: translateY(-1px);
}

.metric-card {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 16px 18px;
  border-radius: 18px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(255, 255, 255, 0.03);
}

.metric-card.accent {
  background: #ffffff;
  border-color: rgba(255, 255, 255, 0.18);
}

.metric-label {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: rgba(255, 255, 255, 0.46);
}

.metric-value {
  font-size: 16px;
  line-height: 1.4;
  color: #ffffff;
  font-weight: 700;
}

.metric-card.accent .metric-label,
.metric-card.accent .metric-value {
  color: #0a0a0a;
}

.path-ribbon {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 14px;
  border-radius: 18px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(255, 255, 255, 0.03);
}

.path-chip {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 14px;
  border-radius: 14px;
  background: rgba(255, 255, 255, 0.04);
  color: rgba(255, 255, 255, 0.72);
  font-size: 13px;
  font-weight: 700;
}

.path-chip.active {
  background: #ffffff;
  color: #090909;
}

.path-index {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.24);
  font-size: 11px;
  letter-spacing: 0.08em;
}

.path-chip:not(.active) .path-index {
  background: rgba(255, 255, 255, 0.08);
}

.path-connector {
  width: 18px;
  height: 2px;
  background: linear-gradient(90deg, #ffffff, rgba(255, 255, 255, 0.12));
  border-radius: 999px;
}

.feature-grid {
  display: grid;
  grid-template-columns: 1fr;
  gap: 14px;
}

.feature-card {
  padding: 18px;
  border-radius: 18px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(255, 255, 255, 0.03);
  box-shadow: none;
}

.feature-eyebrow {
  margin: 0 0 8px;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: rgba(255, 255, 255, 0.44);
}

.feature-title {
  margin: 0 0 8px;
  font-size: 17px;
  line-height: 1.35;
  color: #ffffff;
}

.feature-description {
  margin: 0;
  font-size: 13px;
  line-height: 1.7;
  color: rgba(255, 255, 255, 0.62);
}

.permission-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 18px;
  border-radius: 18px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(255, 255, 255, 0.03);
}

.permission-label p,
.permission-value p {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}

.permission-value {
  padding: 7px 12px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.06);
}

.permission-value.granted {
  background: rgba(255, 255, 255, 0.92);
}

.permission-value.granted p {
  color: #080808;
}

.permission-value.denied {
  background: rgba(255, 255, 255, 0.12);
}

.permission-value.denied p {
  color: #ffffff;
}

.permission-value.default p {
  color: rgba(255, 255, 255, 0.66);
}

.permission-btn {
  align-self: flex-start;
  padding: 10px 16px;
  border: 1px solid rgba(255, 255, 255, 0.16);
  border-radius: 12px;
  background: #ffffff;
  color: #090909;
  font-size: 14px;
  font-weight: 600;
  cursor: pointer;
  transition: background 0.2s ease, transform 0.2s ease, opacity 0.2s ease;
}

.permission-btn:hover {
  background: #e9e9e9;
  transform: translateY(-1px);
}

.permission-btn:disabled {
  opacity: 0.6;
  cursor: default;
  transform: none;
}

.flow-row {
  display: grid;
  grid-template-columns: 1fr 16px 1fr 16px 1fr;
  align-items: center;
  gap: 8px;
}

.flow-card {
  min-height: 78px;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 12px;
  text-align: center;
  border-radius: 18px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(255, 255, 255, 0.03);
}

.flow-card p {
  margin: 0;
  font-size: 13px;
  line-height: 1.5;
  color: rgba(255, 255, 255, 0.76);
  font-weight: 600;
}

.flow-index {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  margin-bottom: 10px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.08);
  color: #ffffff;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
}

.flow-arrow {
  width: 12px;
  height: 12px;
  justify-self: center;
  border-top: 2px solid rgba(255, 255, 255, 0.36);
  border-right: 2px solid rgba(255, 255, 255, 0.36);
  transform: rotate(45deg);
}

.footer-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 22px;
  padding-top: 18px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
}

.footer-meta {
  min-width: 0;
}

.footer-label,
.footer-value {
  margin: 0;
}

.footer-label {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: rgba(255, 255, 255, 0.42);
}

.footer-value {
  margin-top: 6px;
  font-size: 14px;
  line-height: 1.4;
  color: rgba(255, 255, 255, 0.78);
}

.footer-buttons {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
  flex-wrap: wrap;
}

.action-btn {
  min-width: 104px;
  padding: 10px 18px;
  border-radius: 12px;
  border: 1px solid rgba(255, 255, 255, 0.12);
  font-size: 14px;
  font-weight: 600;
  cursor: pointer;
  transition: transform 0.2s ease, background 0.2s ease, border-color 0.2s ease;
}

.action-btn.primary {
  border-color: #ffffff;
  background: #ffffff;
  color: #090909;
}

.action-btn:not(:disabled):hover {
  transform: translateY(-1px);
}

.action-btn.secondary,
.action-btn.ghost {
  background: rgba(255, 255, 255, 0.04);
  color: #ffffff;
}

.action-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

@media (max-width: 640px) {
  .onboarding-card {
    width: min(94vw, 560px);
    min-height: 520px;
    padding: 22px;
  }

  .onboarding-topbar,
  .footer-actions {
    flex-direction: column;
    align-items: stretch;
  }

  .step-counter {
    align-self: flex-start;
  }

  .metric-strip {
    grid-template-columns: 1fr;
  }

  .path-ribbon {
    flex-direction: column;
    align-items: stretch;
  }

  .path-connector {
    width: 2px;
    height: 16px;
    margin: 0 auto;
    background: linear-gradient(180deg, #ffffff, rgba(255, 255, 255, 0.12));
  }

  .setup-copy-card {
    flex-direction: column;
    align-items: stretch;
  }

  .setup-copy-btn {
    width: 100%;
  }

  .flow-row {
    grid-template-columns: 1fr;
  }

  .flow-arrow {
    transform: rotate(135deg);
  }

  .footer-buttons {
    justify-content: stretch;
  }

  .action-btn {
    width: 100%;
  }
}

@keyframes panel-enter {
  from {
    opacity: 0;
    transform: translateY(6px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>
