<script setup lang="ts">
import type { FirstRunGuideDriver, GuideClient, GuideIncomingCall } from './firstRunGuide'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { FIRST_CALL_PROMPT, stepStates } from './firstRunGuide'
import GuideMedia from './GuideMedia.vue'
import GuideStepCard from './GuideStepCard.vue'

// 首次使用引导：接入 AI → AI 调用一次 iterate。
// iterate 自己感知：收到任意一次 zhi 调用就算接通，不要求特定内容，也不要用户手动确认。
const props = defineProps<{
  driver: FirstRunGuideDriver
}>()

const emit = defineEmits<{
  (e: 'finish'): void
  (e: 'skip'): void
  (e: 'openMobile'): void
  /** 收到测试调用：宿主据此把窗口让给用户回复 */
  (e: 'callReceived'): void
}>()

const clients = ref<GuideClient[]>([])
const loadingClients = ref(true)
const selected = ref<string[]>([])
const connecting = ref(false)
const connectMessages = ref<string[]>([])
const incomingCall = ref<GuideIncomingCall | null>(null)
const promptCopied = ref(false)
const showTroubleshoot = ref(false)
// 第 2 步等太久（30 秒）还没收到调用，自动展开排查提示
const WAIT_HINT_MS = 30_000
let waitHintTimer: ReturnType<typeof setTimeout> | null = null
let stopWatching: (() => void) | null = null

const connectedClients = computed(() => clients.value.filter(client => client.connected))
const detectedClients = computed(() => clients.value.filter(client => client.detected))
const states = computed(() => stepStates([
  connectedClients.value.length > 0,
  incomingCall.value !== null,
]))
const allDone = computed(() => states.value.every(state => state === 'done'))

onMounted(async () => {
  clients.value = await props.driver.listClients()
  selected.value = clients.value.filter(client => client.detected && !client.connected).map(client => client.id)
  loadingClients.value = false
})

// 进入第 2 步才开始监听，避免把接入之前的调用算进来
watch(() => states.value[1], (state) => {
  if (state === 'active' && !stopWatching) {
    stopWatching = props.driver.watchIncomingCall((call) => {
      if (incomingCall.value)
        return
      incomingCall.value = call
      showTroubleshoot.value = false
      emit('callReceived')
    })
    waitHintTimer = setTimeout(() => {
      if (!incomingCall.value)
        showTroubleshoot.value = true
    }, WAIT_HINT_MS)
  }
}, { immediate: true })

onUnmounted(() => {
  stopWatching?.()
  if (waitHintTimer)
    clearTimeout(waitHintTimer)
})

async function connect() {
  connecting.value = true
  try {
    const results = await props.driver.connectClients(selected.value)
    connectMessages.value = results.map(result => `${clients.value.find(c => c.id === result.id)?.name ?? result.id}：${result.message}`)
    clients.value = await props.driver.listClients()
  }
  finally {
    connecting.value = false
  }
}

async function copyPrompt() {
  try {
    await navigator.clipboard.writeText(FIRST_CALL_PROMPT)
  }
  catch {
    // 剪贴板不可用时用户仍可手动选中文本
  }
  promptCopied.value = true
}

function formatTime(at: number) {
  return new Date(at).toLocaleTimeString('zh-CN', { hour12: false })
}
</script>

<template>
  <div class="first-run-guide">
    <header class="guide-header">
      <div>
        <p class="guide-eyebrow">
          第一次使用 · 约 2 分钟
        </p>
        <h2 class="guide-title">
          AI 想结束或需要你拍板时，iterate 会先问你
        </h2>
        <p class="guide-subtitle">
          走完下面两步：接入 AI，然后让它调用一次 iterate。AI 一来叫你，就算接通了。
        </p>
      </div>
      <button type="button" class="guide-skip" @click="emit('skip')">
        稍后再说
      </button>
    </header>

    <GuideStepCard
      :index="1"
      title="接入你常用的 AI"
      :state="states[0]"
      :done-text="`已接入：${connectedClients.map(c => c.name).join('、')}`"
    >
      <GuideMedia
        label="演示：一键接入（约 8 秒）"
        storyboard="勾选 Claude Code → 点「一键接入」→ 出现「已写入配置，新开会话后生效」"
      />
      <n-spin v-if="loadingClients" size="small" />
      <template v-else-if="detectedClients.length > 0">
        <div class="client-list">
          <label v-for="client in detectedClients" :key="client.id" class="client-row">
            <n-checkbox
              :checked="selected.includes(client.id)"
              @update:checked="(value: boolean) => selected = value ? [...selected, client.id] : selected.filter(id => id !== client.id)"
            />
            <span class="client-name">{{ client.name }}</span>
            <span class="client-hint">{{ client.restartHint }}</span>
          </label>
        </div>
        <div class="row-actions">
          <n-button type="primary" :loading="connecting" :disabled="selected.length === 0" @click="connect">
            一键接入
          </n-button>
          <span class="muted">会先备份原配置，可随时在设置里移除</span>
        </div>
        <p class="muted rules-note">
          接入 Claude Code、Codex 时，会同时把 iterate 调用规则写进 ~/.claude/CLAUDE.md、~/.codex/AGENTS.md，AI 才会做完一步就回来问你。
          Claude Code 还会在 ~/.claude/settings.json 里允许 call_zhi 免确认调用，第一次叫你时不会先卡在权限确认上。
        </p>
        <ul v-if="connectMessages.length" class="result-list">
          <li v-for="line in connectMessages" :key="line">
            {{ line }}
          </li>
        </ul>
      </template>
      <n-alert v-else type="warning" :show-icon="false">
        没有检测到 Claude Code、Codex、Cursor 或 Antigravity。装好任意一个后，在设置里点「重新查看使用引导」再来一次；其他支持 MCP 的客户端可在「设置 → AI 客户端」查看手动配置方法。
      </n-alert>
    </GuideStepCard>

    <GuideStepCard
      :index="2"
      title="让 AI 叫你一次"
      :state="states[1]"
      :done-text="incomingCall ? `已接通：${formatTime(incomingCall.at)} 收到来自 ${incomingCall.clientName} 的调用。在弹出的 iterate 窗口里回复它，AI 就会接着干。` : ''"
    >
      <p class="step-text">
        先按上一步的提示<strong>重启客户端</strong>，然后在 AI 里随便交代一件事。AI 只要调用了 iterate，这里就会自动打勾，不用你确认。
      </p>
      <p class="step-text muted">
        想马上试一下，可以直接发这句：
      </p>
      <div class="copy-box">
        <code>{{ FIRST_CALL_PROMPT }}</code>
        <n-button size="small" secondary @click="copyPrompt">
          {{ promptCopied ? '已复制' : '复制' }}
        </n-button>
      </div>
      <GuideMedia
        src="./onboarding/guide-first-call.mp4"
        poster="./onboarding/guide-first-call-poster.jpg"
        label="演示：AI 用 zhi 问你一句话，你在弹窗里回复，AI 告诉你它收到了什么（约 10 秒）"
        storyboard="在 Claude Code 发送测试指令 → AI 调用 zhi 问「接通了吗？回我一句就行」→ iterate 弹窗出现 → 输入「收到，接通了」并发送 → AI 回复：收到了，你回复的是「收到，接通了」"
      />
      <p class="waiting">
        <span class="pulse" />
        正在等待 AI 调用 iterate…（收到后这里会自动打勾）
        <button v-if="!showTroubleshoot" type="button" class="link-btn" @click="showTroubleshoot = true">
          等了很久？
        </button>
      </p>
      <ul v-if="showTroubleshoot" class="result-list">
        <li>AI 说找不到 iterate 或 zhi 工具：客户端还没重启。Claude Code 要新开一个会话，Codex 要完全退出再打开</li>
        <li>Claude Code 弹出「是否允许使用 iterate」：点允许</li>
        <li>确认消息是发在刚才接入的那个客户端里</li>
        <li>还是不行：在「设置 → AI 客户端」点「修复」后再试一次</li>
      </ul>
    </GuideStepCard>

    <footer v-if="allDone" class="guide-finish">
      <div>
        <p class="finish-title">
          可以开始用了
        </p>
        <p class="muted">
          以后 AI 想提前收工、或需要你确认时，都会像刚才一样先来问你。记得在弹出的窗口里回复它。
        </p>
      </div>
      <div class="row-actions">
        <n-button type="primary" @click="emit('finish')">
          开始使用
        </n-button>
      </div>
      <div class="mobile-teaser">
        <p class="mobile-title">
          想离开电脑时用手机回复？
        </p>
        <p class="muted">
          需要先给这台 Mac 准备一条外网可达的路线（目前支持自有域名的 Cloudflare 或自有服务器）。免域名的一键路线还在开发中。
        </p>
        <n-button size="small" secondary @click="emit('openMobile')">
          查看手机连接条件
        </n-button>
      </div>
    </footer>
  </div>
</template>

<style scoped>
.first-run-guide {
  --guide-accent: #10a37f;
  --guide-border: #e4e4e7;
  --guide-card-bg: #fff;
  --guide-media-bg: #f4f4f5;
  display: flex;
  flex-direction: column;
  gap: 12px;
  max-width: 720px;
  margin: 0 auto;
  padding: 20px;
  color: #18181b;
}

:root.dark .first-run-guide {
  --guide-border: #3f3f46;
  --guide-card-bg: #18181b;
  --guide-media-bg: #27272a;
  color: #fafafa;
}

.guide-header {
  display: flex;
  gap: 16px;
  align-items: flex-start;
  justify-content: space-between;
  margin-bottom: 4px;
}

.guide-eyebrow {
  margin: 0 0 4px;
  font-size: 12px;
  font-weight: 600;
  color: var(--guide-accent);
}

.guide-title {
  margin: 0;
  font-size: 20px;
  line-height: 1.35;
}

.guide-subtitle {
  margin: 6px 0 0;
  font-size: 13px;
  opacity: 0.7;
}

.guide-skip {
  flex: none;
  padding: 4px 8px;
  border: 0;
  background: none;
  color: inherit;
  font-size: 12px;
  opacity: 0.6;
  cursor: pointer;
}

.client-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.client-row {
  display: flex;
  gap: 10px;
  align-items: center;
  font-size: 14px;
  cursor: pointer;
}

.client-name {
  font-weight: 600;
}

.client-hint,
.muted {
  font-size: 12px;
  opacity: 0.6;
}

.muted {
  margin: 0;
}

.row-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  align-items: center;
}

.rules-note {
  margin-top: 6px;
  line-height: 1.6;
}

.link-btn {
  margin-left: 8px;
  padding: 0;
  border: 0;
  background: none;
  color: var(--guide-accent);
  font: inherit;
  cursor: pointer;
}

.result-list {
  margin: 0;
  padding-left: 18px;
  font-size: 12px;
  line-height: 1.7;
  opacity: 0.8;
}

.step-text {
  margin: 0;
  font-size: 13px;
  line-height: 1.6;
}

.copy-box {
  display: flex;
  gap: 10px;
  align-items: center;
  padding: 10px 12px;
  border-radius: 10px;
  background: var(--guide-media-bg);
}

.copy-box code {
  flex: 1;
  font-size: 12px;
  line-height: 1.6;
  white-space: normal;
}

.waiting {
  display: flex;
  gap: 8px;
  align-items: center;
  margin: 0;
  font-size: 12px;
  opacity: 0.75;
}

.pulse {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--guide-accent);
  animation: pulse 1.2s ease-in-out infinite;
}

@keyframes pulse {
  50% {
    opacity: 0.25;
  }
}

@media (prefers-reduced-motion: reduce) {
  .pulse {
    animation: none;
  }
}

.guide-finish {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 16px;
  border-radius: 14px;
  background: color-mix(in srgb, var(--guide-accent) 8%, transparent);
}

.finish-title,
.mobile-title {
  margin: 0 0 2px;
  font-size: 15px;
  font-weight: 650;
}

.mobile-teaser {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
  padding-top: 10px;
  border-top: 1px solid var(--guide-border);
}

.mobile-title {
  font-size: 13px;
}
</style>
