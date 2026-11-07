<script setup lang="ts">
import type { TailscaleServeStatus } from './tailscaleServeRoute'
import { invoke } from '@tauri-apps/api/core'
import { computed, ref, watch } from 'vue'
import { useLocalQrCode } from '../../composables/useLocalQrCode'
import { bridgeFetch } from '../../services/bridgeFetch'
import {
  IPHONE_APP_STORE_URL,
  needsFreshPairingAfterInstall,
  readAppInstallConfirmed,
  writeAppInstallConfirmed,
} from './mobileAppInstall'
import { mobileConnectionFailureText } from './mobileConnectionMachine'
import MobileConnectionManager from './MobileConnectionManager.vue'
import { buildAliyunIpRouteSetupPrompt } from './mobileConnectionPrompts'
import { mobileRouteLabel, mobileRouteScanHint } from './mobileRouteOptions'
import { sendPhoneTestMessage } from './phoneTestMessage'
import TailscaleGuide from './TailscaleGuide.vue'
import {
  TAILSCALE_DNS_ADMIN_URL,
  TAILSCALE_IOS_APP_URL,
  tailscaleServeBlockerText,
  tailscaleServeChecklist,
} from './tailscaleServeRoute'
import { useMobileConnectionSetup } from './useMobileConnectionSetup'

// 设置页可以直接打开「管理连接」
const props = defineProps<{ initialPanel?: 'manage' }>()
const show = defineModel<boolean>('show', { default: false })
// 出码页只放二维码；连接方式收在「管理连接」的高级设置里，连不上时才在修复页直接给「更换」
const panel = ref<'' | 'manage' | 'choose' | 'tailscale'>('')

const {
  state,
  pairingPayload,
  pairingSession,
  formalRoute,
  qrSource,
  remainingSeconds,
  error,
  notice,
  managedAdmission,
  aiSetupPrompt,
  bootstrap,
  issueFreshPairing,
  retry,
  cancel,
} = useMobileConnectionSetup()

const { dataUrl: qrCodeUrl, error: qrCodeError } = useLocalQrCode(qrSource, { width: 320 })
const { dataUrl: appStoreQrUrl } = useLocalQrCode(IPHONE_APP_STORE_URL, { width: 240 })

// 配对码只有装了 App 才扫得开，所以先给安装入口；确认过一次就不再拦
const appInstallConfirmed = ref(readAppInstallConfirmed())
const showInstallStep = computed(() => state.value.stage === 'waiting_for_claim' && !appInstallConfirmed.value)

const stageText = computed(() => {
  if (showInstallStep.value)
    return '先在 iPhone 上安装 iterate Remote'
  switch (state.value.stage) {
    case 'bootstrap': return '正在检查桌面端连接'
    case 'preparing_bridge': return '正在准备这台 Mac'
    case 'setup_required': return '需要配置正式公网连接'
    case 'recovering_formal_route': return '正在恢复正式公网连接'
    case 'issuing_pairing': return '正在生成一次性二维码'
    case 'waiting_for_claim': return '用 iPhone 扫描二维码'
    case 'waiting_for_connection': return 'iPhone 已认领，正在建立安全连接'
    case 'complete': return 'iPhone 已连接'
    case 'repair_required': return '正式公网连接需要修复'
    case 'expired': return '二维码已过期'
    case 'cancelled': return '连接向导已取消'
    default: return '手机连接失败'
  }
})

const progressHint = computed(() => {
  switch (state.value.stage) {
    case 'bootstrap': return '正在开始检查本机 Bridge 与可用连接路线。'
    case 'preparing_bridge': return '正在准备本机 Bridge，并检查可用的安全公网通道。'
    case 'recovering_formal_route': return '正在恢复已经配置的正式公网通道。'
    case 'issuing_pairing': return '安全公网通道已验证，正在签发一次性配对信息。'
    default: return ''
  }
})

const terminalText = computed(() => mobileConnectionFailureText(state.value, error.value) || stageText.value)

function deviceSuffix(deviceId: string) {
  return deviceId ? deviceId.slice(-8) : ''
}

// 「扫不出来？」：远程桌面、反光、相机坏了时，把配对链接发到手机上
const showScanFallback = ref(false)

async function copyQrSource() {
  if (!qrSource.value)
    return
  await navigator.clipboard.writeText(qrSource.value)
  notice.value = '已复制。用 AirDrop 或「信息」发到 iPhone，在 iPhone 上复制后，到扫码页点「扫不出来？」粘贴。'
}

async function copyAppStoreUrl() {
  await navigator.clipboard.writeText(IPHONE_APP_STORE_URL)
  notice.value = 'App Store 链接已复制。'
}

function confirmAppInstalled() {
  writeAppInstallConfirmed()
  appInstallConfirmed.value = true
  // 安装期间配对码一直在倒计时，快过期就先换一张，免得刚切过来就失效
  if (needsFreshPairingAfterInstall(remainingSeconds.value))
    void issueFreshPairing()
}

function backToInstallStep() {
  appInstallConfirmed.value = false
}

async function copyAiPrompt() {
  await navigator.clipboard.writeText(aiSetupPrompt.value)
  notice.value = formalRoute.value?.configured
    ? '现有配置修复提示词已复制。'
    : '正式连接配置提示词已复制。'
}

// Tailscale Serve：只检测和登记，tailnet 配置始终由用户自己改
const tailscaleStatus = ref<TailscaleServeStatus | null>(null)
const tailscaleBusy = ref(false)
const tailscaleError = ref('')
const tailscaleChecks = computed(() => tailscaleStatus.value ? tailscaleServeChecklist(tailscaleStatus.value) : [])

function errorMessage(cause: unknown) {
  return typeof cause === 'string' ? cause : cause instanceof Error ? cause.message : ''
}

async function checkTailscale() {
  tailscaleBusy.value = true
  tailscaleError.value = ''
  try {
    tailscaleStatus.value = await invoke<TailscaleServeStatus>('get_tailscale_serve_status')
  }
  catch (cause) {
    tailscaleError.value = tailscaleServeBlockerText(errorMessage(cause) || 'tailscale_status_failed')
  }
  finally {
    tailscaleBusy.value = false
  }
}

async function adoptTailscale() {
  tailscaleBusy.value = true
  tailscaleError.value = ''
  try {
    await invoke('register_tailscale_serve_route')
    notice.value = 'Tailscale 路线已启用。iPhone 也要打开 Tailscale 才能连上。'
    await retry()
  }
  catch (cause) {
    tailscaleError.value = tailscaleServeBlockerText(errorMessage(cause) || 'tailscale_register_failed')
    void checkTailscale()
  }
  finally {
    tailscaleBusy.value = false
  }
}

function openExternal(url: string) {
  void invoke('open_external_url', { url })
}

async function copyTailscaleCommand() {
  if (!tailscaleStatus.value?.setup_command)
    return
  await navigator.clipboard.writeText(tailscaleStatus.value.setup_command)
  notice.value = '命令已复制，在终端运行后点「重新检测」。'
}

const formalRouteLabel = computed(() => mobileRouteLabel(formalRoute.value?.transport))
const scanHint = computed(() => mobileRouteScanHint(formalRoute.value?.transport))
const expiresInText = computed(() => {
  const seconds = Math.max(0, remainingSeconds.value)
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`
})

type TestState = 'idle' | 'sending' | 'foreground' | 'delivered' | 'no_phone' | 'failed' | 'timeout'
const testState = ref<TestState>('idle')
const testError = ref('')
const keepAwake = ref<boolean | null>(null)

// 引导步骤条：装 App → 扫码 → 发一条测试消息（收到手机回执才算连通）
const GUIDE_STEPS = ['装 App', '扫码配对', '测试送达']
const guideStep = computed(() => {
  if (showInstallStep.value)
    return 0
  if (state.value.stage === 'waiting_for_claim' || state.value.stage === 'waiting_for_connection')
    return 1
  if (state.value.stage === 'complete')
    return testState.value === 'delivered' ? 3 : 2
  return -1
})

async function sendTestMessage() {
  testState.value = 'sending'
  testError.value = ''
  const outcome = await sendPhoneTestMessage({
    publish: request => invoke<{ ok: boolean, id: string }>('send_phone_action_request', { request }),
    fetchResult: async (id) => {
      const response = await bridgeFetch(`http://127.0.0.1:8080/api/phone-action-result?id=${encodeURIComponent(id)}`, { cache: 'no-store' })
      const data = await response.json()
      return response.ok ? data?.result ?? null : null
    },
    sleep: ms => new Promise(resolve => setTimeout(resolve, ms)),
    now: () => Date.now(),
  }, { onWaitingForForeground: () => { testState.value = 'foreground' } })
  if (outcome.kind === 'delivered') {
    testState.value = 'delivered'
    keepAwake.value = await invoke<boolean>('get_prevent_sleep_status').catch(() => null)
  }
  else if (outcome.kind === 'failed') {
    testState.value = 'failed'
    testError.value = outcome.message
  }
  else {
    testState.value = outcome.kind
  }
}

async function toggleKeepAwake() {
  keepAwake.value = await invoke<boolean>('toggle_prevent_sleep').catch(() => keepAwake.value)
}

function handleRouteSwitched(label: string) {
  panel.value = ''
  notice.value = `已切换到 ${label}，请用手机重新扫码。`
  void retry()
}

async function copyAliyunIpPrompt() {
  await navigator.clipboard.writeText(buildAliyunIpRouteSetupPrompt())
  notice.value = '阿里云 IP 配置提示词已复制。'
}

watch(show, (visible) => {
  panel.value = visible ? props.initialPanel ?? '' : ''
  testState.value = 'idle'
  if (visible)
    void bootstrap()
  else
    cancel()
})
</script>

<template>
  <n-modal
    v-model:show="show"
    preset="card"
    title="连接 iPhone"
    class="mobile-connection-wizard"
    :bordered="false"
  >
    <n-space vertical size="large">
      <div v-if="!panel">
        <div class="text-base font-semibold">
          {{ stageText }}
        </div>
        <div class="mt-1 text-xs opacity-60">
          iterate 只会通过这台 Mac 已配置并验证的正式公网路线生成二维码。
        </div>
      </div>

      <ol v-if="!panel && guideStep >= 0" class="guide-steps" aria-label="连接步骤">
        <li
          v-for="(label, index) in GUIDE_STEPS"
          :key="label"
          class="guide-step"
          :class="{ 'guide-step--done': index < guideStep, 'guide-step--active': index === guideStep }"
          :aria-current="index === guideStep ? 'step' : undefined"
        >
          <span class="guide-step__dot">{{ index < guideStep ? '✓' : index + 1 }}</span>
          <span>{{ label }}</span>
        </li>
      </ol>

      <n-alert v-if="notice" type="success" :bordered="false" closable @close="notice = ''">
        {{ notice }}
      </n-alert>

      <TailscaleGuide
        v-if="panel === 'tailscale'"
        @done="handleRouteSwitched"
        @back="panel = 'choose'"
      />

      <MobileConnectionManager
        v-else-if="panel"
        v-model:panel="panel"
        @back="panel = ''"
        @switched="handleRouteSwitched"
      />

      <div
        v-else-if="['bootstrap', 'preparing_bridge', 'recovering_formal_route', 'issuing_pairing'].includes(state.stage)"
        class="connection-progress"
        role="status"
        :aria-label="stageText"
      >
        <div class="progress-ring progress-ring--indeterminate">
          <div class="progress-ring__center">
            <span class="text-2xl font-semibold" aria-hidden="true">…</span>
          </div>
        </div>
        <div>
          <div class="text-sm font-medium">
            {{ stageText }}
          </div>
          <div class="mt-1 text-xs opacity-60">
            {{ progressHint }}
          </div>
        </div>
      </div>

      <template v-else-if="state.stage === 'setup_required'">
        <n-alert v-if="managedAdmission" type="warning" :bordered="false">
          {{ managedAdmission === 'closed'
            ? '自动连接目前没有对新设备开放，这次没能一键开通。可以先用下面的方式配置自己的公网路线；开放后点「重新检测」会自动连接。'
            : '这台电脑不在自动连接的开放名单里，这次没能一键开通。可以先用下面的方式配置自己的公网路线；获准后点「重新检测」会自动连接。' }}
        </n-alert>
        <div class="prompt-preview">
          <div class="text-xs font-medium">
            已经在用 Tailscale？
          </div>
          <div class="mt-2 text-xs leading-5 opacity-65">
            Mac 和 iPhone 登录同一个 Tailscale 账号，iterate 只在你的 tailnet 内可达，不经过任何第三方服务器。
            iPhone 也要安装 Tailscale；tailnet 需在 DNS 设置里开启 MagicDNS 和 HTTPS 证书。
          </div>
          <div class="mt-1 flex gap-3">
            <n-button text size="tiny" type="primary" @click="openExternal(TAILSCALE_IOS_APP_URL)">
              iPhone 版 Tailscale
            </n-button>
            <n-button text size="tiny" type="primary" @click="openExternal(TAILSCALE_DNS_ADMIN_URL)">
              打开 Tailscale DNS 设置
            </n-button>
          </div>
          <ul v-if="tailscaleChecks.length" class="mt-2 text-xs leading-6">
            <li v-for="check in tailscaleChecks" :key="check.label" :class="check.ok ? 'text-success' : 'opacity-60'">
              {{ check.ok ? '✓' : '○' }} {{ check.label }}
            </li>
          </ul>
          <div v-if="tailscaleStatus?.blocker" class="mt-1 text-xs text-warning">
            {{ tailscaleServeBlockerText(tailscaleStatus.blocker) }}
          </div>
          <code v-if="tailscaleStatus?.setup_command" class="route-receipt mt-2 block text-xs">{{ tailscaleStatus.setup_command }}</code>
          <div v-if="tailscaleError" class="mt-1 text-xs text-error">
            {{ tailscaleError }}
          </div>
          <div class="mt-2 flex flex-wrap gap-2">
            <n-button size="small" type="primary" @click="panel = 'tailscale'">
              按步骤设置 Tailscale
            </n-button>
            <n-button size="small" :loading="tailscaleBusy" @click="checkTailscale">
              {{ tailscaleStatus ? '重新检测 Tailscale' : '检测 Tailscale' }}
            </n-button>
            <n-button v-if="tailscaleStatus?.setup_command" size="small" @click="copyTailscaleCommand">
              复制命令
            </n-button>
            <n-button v-if="tailscaleStatus?.base_url" type="primary" size="small" :loading="tailscaleBusy" @click="adoptTailscale">
              用 Tailscale 连接
            </n-button>
          </div>
        </div>
        <n-alert type="info" :bordered="false">
          当前电脑还没有登记正式公网路线。可以选择 Cloudflare Named Tunnel 或阿里云 IP + SSH，复制对应的安全提示词交给 AI；遇到登录、域名、DNS、管理员权限、凭据或费用相关步骤时，AI 必须停下来由你确认。
        </n-alert>
        <div class="prompt-preview">
          <div class="text-xs font-medium">
            AI 只负责引导并验证正式配置
          </div>
          <div class="mt-2 text-xs leading-5 opacity-65">
            提示词明确禁止读取凭据、创建测试通道或降低鉴权；验证完成后才会把脱敏的正式地址登记给 iterate。
          </div>
        </div>
        <div class="grid grid-cols-2 gap-2">
          <n-button size="large" class="tap-target" @click="copyAiPrompt">
            复制 AI 配置提示词（Cloudflare）
          </n-button>
          <n-button size="large" class="tap-target" @click="copyAliyunIpPrompt">
            复制阿里云 IP 配置提示词
          </n-button>
          <n-button type="primary" size="large" class="tap-target" @click="retry">
            重新检测
          </n-button>
        </div>
      </template>

      <template v-else-if="state.stage === 'repair_required'">
        <n-alert type="warning" :bordered="false">
          正式配置仍然保留，只是当前自动恢复未通过。修复提示词只允许检查和恢复现有路线，不会退回首次配置或创建测试路线。
        </n-alert>
        <div v-if="formalRoute?.base_url" class="route-receipt text-xs flex items-center gap-2">
          <span class="min-w-0 break-all">已配置{{ formalRouteLabel }}地址：{{ formalRoute.base_url }}</span>
          <n-button text size="tiny" type="primary" class="ml-auto flex-shrink-0" @click="panel = 'choose'">
            更换
          </n-button>
        </div>
        <div class="text-xs opacity-70">
          {{ terminalText }}
        </div>
        <div class="grid grid-cols-2 gap-2">
          <n-button size="large" class="tap-target" @click="copyAiPrompt">
            修复现有配置
          </n-button>
          <n-button type="primary" size="large" class="tap-target" @click="retry">
            重新检测
          </n-button>
        </div>
      </template>

      <template v-else-if="showInstallStep">
        <div class="install-card">
          <div class="qr-card qr-card--compact">
            <n-spin v-if="!appStoreQrUrl" size="small" />
            <img v-else :src="appStoreQrUrl" alt="iterate Remote 的 App Store 下载二维码" class="qr-image qr-image--compact">
          </div>
          <ol class="install-steps text-sm leading-6">
            <li>用 iPhone <b>系统相机</b>扫左边的码，在 App Store 安装 <b>iterate Remote</b>（免费，需要 iOS 16 及以上）。</li>
            <li>打开 App，点「<b>扫码连接电脑</b>」。</li>
            <li>回到这里点「已安装」，再扫描接下来出现的配对码。</li>
          </ol>
        </div>
        <n-alert type="info" :bordered="false">
          配对码只能被 iterate Remote 识别。没装 App 时，用系统相机扫它会没有反应。
        </n-alert>
        <div class="grid grid-cols-2 gap-2">
          <n-button size="large" class="tap-target" @click="copyAppStoreUrl">
            复制 App Store 链接
          </n-button>
          <n-button type="primary" size="large" class="tap-target" @click="confirmAppInstalled">
            已安装，显示配对码
          </n-button>
        </div>
      </template>

      <template v-else-if="state.stage === 'waiting_for_claim'">
        <div class="pair-qr">
          <div class="qr-card">
            <n-spin v-if="!qrCodeUrl" size="small" />
            <img v-else :src="qrCodeUrl" alt="iPhone 一次性连接二维码" class="qr-image">
          </div>
          <div class="pair-qr__tools">
            <button type="button" class="wizard-link" @click="issueFreshPairing">
              刷新
            </button>
          </div>
        </div>
        <div class="pair-steps">
          <div class="pair-steps__main">
            用 iPhone 上的 iterate Remote 扫一扫
          </div>
          <div v-if="scanHint" class="pair-hint">
            {{ scanHint }}
          </div>
          <div class="pair-meta" aria-live="polite">
            <span v-if="pairingPayload?.device_name">{{ pairingPayload.device_name }}</span>
            <span>经 {{ formalRouteLabel }}</span>
            <span>{{ expiresInText }} 后过期</span>
          </div>
        </div>
        <div v-if="qrCodeError" class="text-xs text-error">
          本地二维码生成失败：{{ qrCodeError.message }}
        </div>
        <div class="scan-fallback">
          <button
            v-if="!showScanFallback"
            type="button"
            class="wizard-link wizard-link--muted"
            :aria-expanded="showScanFallback"
            @click="showScanFallback = true"
          >
            扫不出来？
          </button>
          <template v-else>
            <n-button type="primary" size="large" class="tap-target" :disabled="!qrSource" @click="copyQrSource">
              复制配对链接
            </n-button>
            <div class="scan-fallback__hint">
              用 AirDrop 或「信息」发到 iPhone，在 iPhone 上复制这段链接，再到扫码页点「扫不出来？」→「粘贴」。链接里带着配对凭证，别发到微信等第三方聊天里；它和二维码一样只能用一次，过期后点「刷新」。
            </div>
          </template>
        </div>
        <div class="pair-links">
          <button type="button" class="wizard-link" @click="backToInstallStep">
            还没装 App？
          </button>
          <button type="button" class="wizard-link" @click="panel = 'manage'">
            管理连接
          </button>
        </div>
      </template>

      <template v-else-if="state.stage === 'waiting_for_connection'">
        <n-alert type="info" :bordered="false">
          已认领设备 {{ pairingSession?.device_name || state.deviceId }}，正在等待经过鉴权的 WebSocket 连接。
        </n-alert>
      </template>

      <template v-else-if="state.stage === 'complete'">
        <template v-if="testState !== 'delivered'">
          <div class="test-card">
            <div class="font-medium">
              iPhone 已配对，最后试一下能不能收到消息
            </div>
            <div class="text-xs opacity-65 leading-5">
              电脑会发一条测试消息到 iPhone，手机收到后自动回执。收到回执，才说明这条连接真的能用。
            </div>
            <div v-if="testState === 'sending'" class="text-xs opacity-70">
              已发送，等待 iPhone 回执…
            </div>
            <div v-else-if="testState === 'foreground'" class="pair-hint">
              iPhone 还在后台：在手机上打开 iterate Remote，就会收到这条消息
            </div>
            <div v-else-if="testState === 'no_phone'" class="text-xs text-warning">
              现在没有在线的 iPhone。打开手机上的 iterate Remote 后再试一次。
            </div>
            <div v-else-if="testState === 'timeout'" class="text-xs text-warning">
              一分钟内没有收到回执。确认手机上 iterate Remote 已打开，再试一次。
            </div>
            <div v-else-if="testState === 'failed'" class="text-xs text-error">
              发送失败：{{ testError }}
            </div>
          </div>
          <div class="grid grid-cols-2 gap-2">
            <n-button size="large" class="tap-target" @click="show = false">
              跳过
            </n-button>
            <n-button type="primary" size="large" class="tap-target" :loading="testState === 'sending' || testState === 'foreground'" @click="sendTestMessage">
              {{ testState === 'idle' ? '发送测试消息' : '再试一次' }}
            </n-button>
          </div>
        </template>
        <template v-else>
          <n-alert type="success" :bordered="false">
            iPhone 收到了测试消息，可以在手机上回复这台 Mac 的任务了。
          </n-alert>
          <div class="test-card test-card--row">
            <div class="grid">
              <span class="font-medium">合盖保持运行</span>
              <span class="text-xs opacity-60">接通电源时阻止这台 Mac 休眠。不开的话，合盖后手机就连不上了。</span>
            </div>
            <n-switch :value="keepAwake === true" :disabled="keepAwake === null" @update:value="toggleKeepAwake" />
          </div>
          <div class="text-xs opacity-55">
            设备：…{{ deviceSuffix(state.deviceId) }} · 经 {{ formalRouteLabel }}
          </div>
          <n-button type="primary" size="large" block class="tap-target" @click="show = false">
            完成
          </n-button>
        </template>
      </template>

      <template v-else-if="state.stage === 'error' || state.stage === 'expired' || state.stage === 'cancelled'">
        <n-alert type="warning" :bordered="false">
          {{ terminalText }}
        </n-alert>
        <div :class="state.stage === 'error' ? 'grid grid-cols-3 gap-2' : 'grid grid-cols-2 gap-2'">
          <n-button v-if="state.stage === 'error'" size="large" class="tap-target" @click="copyAiPrompt">
            复制给 AI
          </n-button>
          <n-button v-if="state.stage !== 'cancelled'" type="primary" size="large" class="tap-target" @click="retry">
            重试
          </n-button>
          <n-button size="large" class="tap-target" @click="show = false">
            关闭
          </n-button>
        </div>
      </template>

      <div v-if="state.stage !== 'complete' && state.stage !== 'cancelled' && !panel" class="flex justify-end">
        <button type="button" class="wizard-link wizard-link--muted" @click="show = false">
          稍后再说
        </button>
      </div>
    </n-space>
  </n-modal>
</template>

<style>
/* n-modal 渲染在 body 下，scoped 样式到不了；全部挂在弹窗自己的类名下 */
.mobile-connection-wizard {
  width: min(460px, calc(100vw - 32px));
}

.mobile-connection-wizard .connection-progress {
  display: grid;
  grid-template-columns: 88px 1fr;
  align-items: center;
  gap: 20px;
  padding: 24px 8px;
}

.mobile-connection-wizard .progress-ring {
  display: grid;
  width: 88px;
  height: 88px;
  place-items: center;
  border-radius: 999px;
}

.mobile-connection-wizard .progress-ring--indeterminate {
  background: conic-gradient(from 0deg, #18a058, rgba(24, 160, 88, 0.13) 35% 100%);
  animation: pairing-progress-spin 1.1s linear infinite;
}

@keyframes pairing-progress-spin {
  to {
    transform: rotate(1turn);
  }
}

.mobile-connection-wizard .progress-ring__center {
  display: grid;
  width: 70px;
  height: 70px;
  place-items: center;
  border-radius: 999px;
  background: var(--n-color, #fff);
}

.mobile-connection-wizard .prompt-preview,
.mobile-connection-wizard .route-receipt {
  border: 1px solid rgba(128, 128, 128, 0.2);
  border-radius: 10px;
  background: rgba(128, 128, 128, 0.05);
  padding: 14px;
}

.mobile-connection-wizard .pair-qr {
  display: grid;
  justify-items: center;
  gap: 6px;
}

.mobile-connection-wizard .qr-card {
  display: flex;
  width: 236px;
  max-width: 100%;
  aspect-ratio: 1;
  align-items: center;
  justify-content: center;
  border-radius: 14px;
  border: 1px solid rgba(128, 128, 128, 0.18);
  background: #fff;
  padding: 10px;
}

.mobile-connection-wizard .qr-image {
  width: 100%;
  height: auto;
  aspect-ratio: 1;
  image-rendering: pixelated;
}

.mobile-connection-wizard .pair-qr__tools {
  display: flex;
  gap: 16px;
  font-size: 12px;
}

.mobile-connection-wizard .pair-steps {
  display: grid;
  justify-items: center;
  gap: 6px;
  text-align: center;
}

.mobile-connection-wizard .pair-steps__main {
  font-size: 14px;
  font-weight: 500;
}

.mobile-connection-wizard .pair-hint {
  font-size: 12px;
  color: #d48806;
  background: rgba(212, 136, 6, 0.1);
  border-radius: 999px;
  padding: 2px 10px;
}

.mobile-connection-wizard .pair-meta {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 0 10px;
  font-size: 12px;
  opacity: 0.55;
  font-variant-numeric: tabular-nums;
}

.mobile-connection-wizard .pair-meta span + span::before {
  content: '·';
  margin-right: 10px;
}

.mobile-connection-wizard .scan-fallback {
  display: grid;
  justify-items: center;
  gap: 8px;
  text-align: center;
  font-size: 13px;
}

.mobile-connection-wizard .scan-fallback__hint {
  max-width: 340px;
  font-size: 12px;
  line-height: 1.6;
  opacity: 0.65;
}

.mobile-connection-wizard .pair-links {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  padding-top: 12px;
  border-top: 1px solid rgba(128, 128, 128, 0.15);
  font-size: 13px;
}

.mobile-connection-wizard .wizard-link {
  border: 0;
  background: none;
  padding: 2px 0;
  color: #18a058;
  font: inherit;
  cursor: pointer;
}

.mobile-connection-wizard .wizard-link:hover {
  text-decoration: underline;
}

.mobile-connection-wizard .wizard-link:focus-visible {
  outline: 2px solid #18a058;
  outline-offset: 2px;
  border-radius: 4px;
}

.mobile-connection-wizard .wizard-link--muted {
  color: inherit;
  opacity: 0.55;
}

.mobile-connection-wizard .guide-steps {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 8px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.mobile-connection-wizard .guide-step {
  display: flex;
  align-items: center;
  gap: 6px;
  padding-top: 8px;
  border-top: 3px solid rgba(128, 128, 128, 0.18);
  font-size: 12px;
  opacity: 0.5;
}

.mobile-connection-wizard .guide-step--active,
.mobile-connection-wizard .guide-step--done {
  border-top-color: #18a058;
  opacity: 1;
}

.mobile-connection-wizard .guide-step__dot {
  display: inline-grid;
  width: 18px;
  height: 18px;
  place-items: center;
  border-radius: 999px;
  background: rgba(128, 128, 128, 0.16);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}

.mobile-connection-wizard .guide-step--active .guide-step__dot,
.mobile-connection-wizard .guide-step--done .guide-step__dot {
  background: #18a058;
  color: #fff;
}

.mobile-connection-wizard .test-card {
  display: grid;
  gap: 8px;
  border: 1px solid rgba(128, 128, 128, 0.2);
  border-radius: 12px;
  padding: 14px 16px;
}

.mobile-connection-wizard .test-card--row {
  grid-template-columns: 1fr auto;
  align-items: center;
}

.mobile-connection-wizard .install-card {
  display: grid;
  grid-template-columns: 152px 1fr;
  align-items: center;
  gap: 16px;
}

.mobile-connection-wizard .qr-card--compact {
  width: 152px;
}

.mobile-connection-wizard .install-steps {
  margin: 0;
  padding-left: 1.2em;
}

.mobile-connection-wizard .tap-target {
  min-height: 44px;
}

@media (prefers-reduced-motion: reduce) {
  .mobile-connection-wizard .progress-ring--indeterminate {
    animation: none;
  }
}

@media (max-width: 520px) {
  .mobile-connection-wizard .install-card,
  .mobile-connection-wizard .connection-progress {
    grid-template-columns: 1fr;
    justify-items: center;
    text-align: center;
  }
}
</style>
