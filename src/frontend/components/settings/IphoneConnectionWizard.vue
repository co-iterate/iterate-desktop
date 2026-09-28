<script setup lang="ts">
import { computed, watch } from 'vue'
import { useLocalQrCode } from '../../composables/useLocalQrCode'
import { mobileConnectionFailureText } from './mobileConnectionMachine'
import { useIphoneConnectionSetup } from './useIphoneConnectionSetup'

const show = defineModel<boolean>('show', { default: false })

const {
  state,
  pairingPayload,
  pairingSession,
  formalRoute,
  qrSource,
  remainingSeconds,
  error,
  notice,
  aiSetupPrompt,
  bootstrap,
  issueFreshPairing,
  retry,
  cancel,
} = useIphoneConnectionSetup()

const { dataUrl: qrCodeUrl, error: qrCodeError } = useLocalQrCode(qrSource, { width: 320 })

const stageText = computed(() => {
  switch (state.value.stage) {
    case 'bootstrap': return '正在检查桌面端连接'
    case 'preparing_bridge': return '正在准备当前电脑'
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

async function copyQrSource() {
  if (!qrSource.value)
    return
  await navigator.clipboard.writeText(qrSource.value)
  notice.value = '配对链接已复制。'
}

async function copyAiPrompt() {
  await navigator.clipboard.writeText(aiSetupPrompt.value)
  notice.value = formalRoute.value?.configured
    ? '现有配置修复提示词已复制。'
    : '正式连接配置提示词已复制。'
}

watch(show, (visible) => {
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
      <div>
        <div class="text-base font-semibold">
          {{ stageText }}
        </div>
        <div class="mt-1 text-xs opacity-60">
          iterate 只会通过当前电脑已配置并验证的正式公网路线生成二维码。
        </div>
      </div>

      <n-alert v-if="notice" type="success" :bordered="false" closable @close="notice = ''">
        {{ notice }}
      </n-alert>

      <div
        v-if="['bootstrap', 'preparing_bridge', 'recovering_formal_route', 'issuing_pairing'].includes(state.stage)"
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
        <n-alert type="info" :bordered="false">
          当前电脑还没有登记正式公网路线。复制下面的安全提示词交给 AI；遇到 Cloudflare 登录、域名、DNS、管理员权限或凭据步骤时，AI 必须停下来由你确认。
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
            复制 AI 配置提示词
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
        <div v-if="formalRoute?.base_url" class="route-receipt text-xs">
          已配置地址：{{ formalRoute.base_url }}
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

      <template v-else-if="state.stage === 'waiting_for_claim'">
        <div class="qr-card">
          <n-spin v-if="!qrCodeUrl" size="small" />
          <img v-else :src="qrCodeUrl" alt="iPhone 一次性连接二维码" class="qr-image">
        </div>
        <div class="text-center text-xs opacity-70" aria-live="polite">
          二维码将在 {{ remainingSeconds }} 秒后过期。请只在目标 iPhone 上扫描。
        </div>
        <div v-if="pairingPayload?.device_name" class="text-center text-xs opacity-55">
          当前电脑：{{ pairingPayload.device_name }}
        </div>
        <div v-if="qrCodeError" class="text-xs text-error">
          本地二维码生成失败：{{ qrCodeError.message }}
        </div>
        <div class="grid grid-cols-2 gap-2">
          <n-button size="large" class="tap-target" @click="issueFreshPairing">
            刷新二维码
          </n-button>
          <n-button size="large" class="tap-target" @click="copyQrSource">
            复制配对链接
          </n-button>
        </div>
        <div class="text-center text-xs opacity-50">
          正式路线：{{ formalRoute?.base_url || pairingPayload?.base_url }}
        </div>
      </template>

      <template v-else-if="state.stage === 'waiting_for_connection'">
        <n-alert type="info" :bordered="false">
          已认领设备 {{ pairingSession?.device_name || state.deviceId }}，正在等待经过鉴权的 WebSocket 连接。
        </n-alert>
      </template>

      <template v-else-if="state.stage === 'complete'">
        <n-alert type="success" :bordered="false">
          iPhone 已通过正式公网路线连接。
        </n-alert>
        <div class="text-xs opacity-70">
          设备 ID：…{{ deviceSuffix(state.deviceId) }}
        </div>
        <n-button type="primary" size="large" block class="tap-target" @click="show = false">
          完成
        </n-button>
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

      <div v-if="state.stage !== 'complete' && state.stage !== 'cancelled'" class="flex justify-end">
        <n-button text class="tap-target" @click="show = false">
          稍后再说
        </n-button>
      </div>
    </n-space>
  </n-modal>
</template>

<style scoped>
.mobile-connection-wizard {
  width: min(500px, calc(100vw - 32px));
}

.connection-progress {
  display: grid;
  grid-template-columns: 88px 1fr;
  align-items: center;
  gap: 20px;
  padding: 24px 8px;
}

.progress-ring {
  display: grid;
  width: 88px;
  height: 88px;
  place-items: center;
  border-radius: 999px;
}

.progress-ring--indeterminate {
  background: conic-gradient(from 0deg, #18a058, rgba(24, 160, 88, 0.13) 35% 100%);
  animation: pairing-progress-spin 1.1s linear infinite;
}

@keyframes pairing-progress-spin {
  to {
    transform: rotate(1turn);
  }
}

.progress-ring__center {
  display: grid;
  width: 70px;
  height: 70px;
  place-items: center;
  border-radius: 999px;
  background: var(--n-color, #fff);
}

.prompt-preview,
.route-receipt {
  border: 1px solid rgba(128, 128, 128, 0.2);
  border-radius: 10px;
  background: rgba(128, 128, 128, 0.05);
  padding: 14px;
}

.qr-card {
  display: flex;
  min-height: 336px;
  align-items: center;
  justify-content: center;
  border-radius: 12px;
  background: #fff;
  padding: 8px;
}

.qr-image {
  width: 320px;
  max-width: 100%;
  height: auto;
  aspect-ratio: 1;
}

.tap-target {
  min-height: 44px;
}

@media (prefers-reduced-motion: reduce) {
  .progress-ring--indeterminate {
    animation: none;
  }
}

@media (max-width: 520px) {
  .connection-progress {
    grid-template-columns: 1fr;
    justify-items: center;
    text-align: center;
  }
}
</style>
