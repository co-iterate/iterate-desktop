<script setup lang="ts">
import type { TailscaleServeStatus } from './tailscaleServeRoute'
import { invoke } from '@tauri-apps/api/core'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useLocalQrCode } from '../../composables/useLocalQrCode'
import {
  TAILSCALE_DNS_ADMIN_URL,
  TAILSCALE_IOS_APP_URL,
  TAILSCALE_MAC_DOWNLOAD_URL,
  tailscaleGuideState,
  tailscaleServeBlockerText,
} from './tailscaleServeRoute'

// Tailscale 四步引导：每一步都由实际检测决定是否完成，不靠用户点「下一步」。
// 只有用户点了「帮我执行」「帮我关掉 Funnel」才会改 Tailscale 配置。
const emit = defineEmits<{
  done: [label: string]
  back: []
}>()

const STEPS = ['装 Mac 端', '开两个开关', '挂上 iterate', '装手机端']
const POLL_MS = 3000

const status = ref<TailscaleServeStatus | null>(null)
const busy = ref<'' | 'serve' | 'funnel' | 'finish'>('')
const error = ref('')
const copied = ref('')
let pollTimer: ReturnType<typeof setInterval> | null = null

const guide = computed(() => status.value ? tailscaleGuideState(status.value) : { step: 0, view: 'install' as const })
const { dataUrl: iosQrUrl } = useLocalQrCode(TAILSCALE_IOS_APP_URL, { width: 200 })
const serveCommand = computed(() => status.value?.setup_command || '/Applications/Tailscale.app/Contents/MacOS/Tailscale serve --bg 8080')
const funnelCommand = '/Applications/Tailscale.app/Contents/MacOS/Tailscale funnel reset'
const address = computed(() => status.value?.dns_name ? `https://${status.value.dns_name}` : 'https://你的机器名.ts.net')

function errorText(cause: unknown) {
  const code = typeof cause === 'string' ? cause : cause instanceof Error ? cause.message : ''
  if (code === 'tailscale_command_failed')
    return 'Tailscale 没有执行成功。可以改用「复制命令」在终端里运行，看看具体报错。'
  if (code === 'tailscale_command_timeout')
    return 'Tailscale 迟迟没有响应，请确认它正在运行后再试。'
  return tailscaleServeBlockerText(code || 'tailscale_status_failed')
}

async function refresh() {
  try {
    status.value = await invoke<TailscaleServeStatus>('get_tailscale_serve_status')
  }
  catch (cause) {
    error.value = errorText(cause)
  }
}

function open(url: string) {
  void invoke('open_external_url', { url })
}

async function copy(text: string, label: string) {
  await navigator.clipboard.writeText(text)
  copied.value = label
}

async function startServe() {
  busy.value = 'serve'
  error.value = ''
  try {
    status.value = await invoke<TailscaleServeStatus>('start_tailscale_serve')
  }
  catch (cause) {
    error.value = errorText(cause)
  }
  finally {
    busy.value = ''
  }
}

async function resetFunnel() {
  busy.value = 'funnel'
  error.value = ''
  try {
    status.value = await invoke<TailscaleServeStatus>('reset_tailscale_funnel')
  }
  catch (cause) {
    error.value = errorText(cause)
  }
  finally {
    busy.value = ''
  }
}

async function finish() {
  busy.value = 'finish'
  error.value = ''
  try {
    await invoke('register_tailscale_serve_route')
    emit('done', 'Tailscale')
  }
  catch (cause) {
    error.value = errorText(cause)
    void refresh()
  }
  finally {
    busy.value = ''
  }
}

onMounted(() => {
  void refresh()
  pollTimer = setInterval(() => {
    if (!busy.value)
      void refresh()
  }, POLL_MS)
})
onUnmounted(() => {
  if (pollTimer)
    clearInterval(pollTimer)
})
</script>

<template>
  <div class="ts-guide">
    <div>
      <div class="text-base font-semibold">
        用 Tailscale 连接 iPhone
      </div>
      <div class="mt-1 text-xs opacity-60">
        Mac 和 iPhone 直接相连，内容不经过任何第三方服务器。每一步检测通过会自动打勾。
      </div>
    </div>

    <ol class="guide-steps" aria-label="Tailscale 设置步骤">
      <li
        v-for="(label, index) in STEPS"
        :key="label"
        class="guide-step"
        :class="{ 'guide-step--done': index < guide.step, 'guide-step--active': index === guide.step }"
        :aria-current="index === guide.step ? 'step' : undefined"
      >
        <span class="guide-step__dot">{{ index < guide.step ? '✓' : index + 1 }}</span>
        <span>{{ label }}</span>
      </li>
    </ol>

    <!-- ① 安装 -->
    <template v-if="guide.view === 'install'">
      <div class="ts-title">
        先在这台 Mac 上安装 Tailscale
      </div>
      <div class="ts-status ts-status--wait">
        <span class="ts-spin" />还没检测到 Tailscale，装好后这里会自动打勾
      </div>
      <ol class="ts-hints">
        <li>点下面的按钮下载，安装后打开</li>
        <li>在菜单栏的 Tailscale 图标里点 <b>Log in</b>，用 Google、Apple、GitHub 或微软账号登录</li>
        <li><b>记住登的是哪个账号</b>，iPhone 上要登同一个</li>
      </ol>
      <div class="ts-actions">
        <n-button type="primary" @click="open(TAILSCALE_MAC_DOWNLOAD_URL)">
          下载 Tailscale for Mac
        </n-button>
        <n-button @click="refresh">
          重新检测
        </n-button>
      </div>
    </template>

    <template v-else-if="guide.view === 'login'">
      <div class="ts-title">
        Tailscale 已安装，但还没登录
      </div>
      <div class="ts-status ts-status--warn">
        已找到 Tailscale，状态：{{ status?.backend_state === 'Stopped' ? '已断开' : '未登录' }}
      </div>
      <ol class="ts-hints">
        <li>点菜单栏右上角的 Tailscale 图标</li>
        <li>{{ status?.backend_state === 'Stopped' ? '点 Connect 连上' : '点 Log in…，在浏览器里完成登录' }}</li>
        <li>回到这里，会自动继续</li>
      </ol>
      <div class="ts-actions">
        <n-button @click="refresh">
          重新检测
        </n-button>
      </div>
    </template>

    <!-- ② 两个开关 -->
    <template v-else-if="guide.view === 'switches'">
      <div class="ts-title">
        在 Tailscale 后台打开两个开关
      </div>
      <div class="text-xs opacity-65 leading-5">
        iterate 需要一个带证书的地址（{{ address }}），这两个开关打开才有，只需要开一次。
      </div>
      <div class="ts-admin" aria-label="Tailscale 后台 DNS 页面示意">
        <div class="ts-admin__bar">
          login.tailscale.com/admin/dns
        </div>
        <div class="ts-admin__row" :class="{ 'ts-admin__row--todo': !status?.dns_name }">
          <div>
            <b>MagicDNS</b>
            <span>给每台设备一个好记的名字</span>
          </div>
          <span class="ts-admin__state">{{ status?.dns_name ? '✓ 已开启' : '点 Enable MagicDNS' }}</span>
        </div>
        <div class="ts-admin__row" :class="{ 'ts-admin__row--todo': !status?.https_cert_available }">
          <div>
            <b>HTTPS Certificates</b>
            <span>给设备签发 HTTPS 证书</span>
          </div>
          <span class="ts-admin__state">{{ status?.https_cert_available ? '✓ 已开启' : '点 Enable HTTPS' }}</span>
        </div>
      </div>
      <div class="text-[11px] opacity-50">
        示意图：在后台页面里找到标黄的两项，各点一次按钮。
      </div>
      <div class="ts-actions">
        <n-button type="primary" @click="open(TAILSCALE_DNS_ADMIN_URL)">
          打开 Tailscale DNS 设置
        </n-button>
        <n-button @click="refresh">
          重新检测
        </n-button>
      </div>
    </template>

    <!-- ③ 挂上 iterate -->
    <template v-else-if="guide.view === 'funnel'">
      <div class="ts-title">
        先关掉 Funnel
      </div>
      <div class="ts-status ts-status--bad">
        Funnel 已开启：iterate 现在任何人都能从公网访问
      </div>
      <div class="ts-confirm">
        <div class="font-medium">
          iterate 将替你运行：
        </div>
        <code>tailscale funnel reset</code>
        <ul>
          <li>只关闭公网访问，不影响你 Tailscale 网络里的其他设备</li>
          <li>关掉之后，下一步再把 iterate 只挂到你自己的网络上</li>
        </ul>
      </div>
      <div class="ts-actions">
        <n-button type="primary" :loading="busy === 'funnel'" @click="resetFunnel">
          帮我关掉 Funnel
        </n-button>
        <n-button @click="copy(funnelCommand, 'funnel')">
          {{ copied === 'funnel' ? '已复制，粘贴到终端运行' : '复制命令，我自己来' }}
        </n-button>
      </div>
    </template>

    <template v-else-if="guide.view === 'serve'">
      <div class="ts-title">
        把 iterate 挂到你的 Tailscale 网络上
      </div>
      <div class="text-xs opacity-65 leading-5">
        这样 iPhone 才能通过 {{ address }} 找到这台 Mac 上的 iterate。
      </div>
      <div class="ts-confirm">
        <div class="font-medium">
          iterate 将替你运行：
        </div>
        <code>tailscale serve --bg 8080</code>
        <ul>
          <li>只在你自己的 Tailscale 网络里可见，<b>不会开启 Funnel，不会暴露到公网</b></li>
          <li>访问仍然需要 iterate 的配对授权</li>
          <li>随时可以撤销：运行 <code class="inline">tailscale serve reset</code></li>
        </ul>
      </div>
      <div class="ts-actions">
        <n-button type="primary" :loading="busy === 'serve'" @click="startServe">
          帮我执行
        </n-button>
        <n-button @click="copy(serveCommand, 'serve')">
          {{ copied === 'serve' ? '已复制，粘贴到终端运行' : '复制命令，我自己来' }}
        </n-button>
      </div>
    </template>

    <!-- ④ 手机 -->
    <template v-else-if="guide.view === 'phone'">
      <div class="ts-title">
        在 iPhone 上安装 Tailscale
      </div>
      <div class="ts-status ts-status--ok">
        ✓ iterate 已挂到 {{ address }}（只在你的网络里可见）
      </div>
      <div class="ts-phone">
        <div class="ts-qr">
          <n-spin v-if="!iosQrUrl" size="small" />
          <img v-else :src="iosQrUrl" alt="App Store 中 Tailscale 的下载二维码">
        </div>
        <ol class="ts-hints">
          <li>用 iPhone <b>相机</b>扫左边的码，安装 Tailscale</li>
          <li>登录<b>和 Mac 上同一个账号</b></li>
          <li>按提示添加 VPN 配置，然后<b>打开开关</b></li>
        </ol>
      </div>
      <div class="ts-status ts-status--warn">
        iPhone 同一时间只能开一个 VPN。如果你平时开着代理类 VPN，打开 Tailscale 会把它挤掉，反过来也一样。
      </div>
      <div class="ts-status ts-status--wait">
        <span class="ts-spin" />正在等待 iPhone 出现在你的 Tailscale 网络里
      </div>
    </template>

    <!-- 完成 -->
    <template v-else>
      <div class="ts-title">
        都准备好了
      </div>
      <div class="ts-status ts-status--ok">
        ✓ iPhone 已在你的 Tailscale 网络里
      </div>
      <div class="text-xs opacity-65">
        接下来生成配对码，用 iPhone 上的 iterate Remote 扫一下就能连上。
      </div>
      <n-button type="primary" block :loading="busy === 'finish'" @click="finish">
        用 Tailscale 连接，生成配对码
      </n-button>
    </template>

    <n-alert v-if="error" type="warning" :bordered="false">
      {{ error }}
    </n-alert>

    <div class="ts-footer">
      <button type="button" class="wizard-link" @click="emit('back')">
        换一种连接方式
      </button>
    </div>
  </div>
</template>

<style scoped>
.ts-guide {
  display: grid;
  gap: 14px;
}
.ts-title {
  font-size: 15px;
  font-weight: 600;
}
.ts-hints {
  margin: 0;
  padding-left: 1.2em;
  display: grid;
  gap: 4px;
  font-size: 13px;
  opacity: 0.85;
}
.ts-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
}
.ts-status {
  display: flex;
  align-items: center;
  gap: 8px;
  border-radius: 10px;
  padding: 10px 12px;
  font-size: 13px;
}
.ts-status--wait {
  background: rgba(128, 128, 128, 0.08);
  opacity: 0.85;
}
.ts-status--ok {
  color: #18a058;
  background: rgba(24, 160, 88, 0.1);
}
.ts-status--warn {
  color: #d48806;
  background: rgba(212, 136, 6, 0.1);
}
.ts-status--bad {
  color: #d03050;
  background: rgba(208, 48, 80, 0.1);
}
.ts-spin {
  width: 12px;
  height: 12px;
  flex: none;
  border-radius: 50%;
  border: 2px solid currentColor;
  border-right-color: transparent;
  animation: ts-spin 1s linear infinite;
}
@keyframes ts-spin {
  to {
    transform: rotate(1turn);
  }
}
@media (prefers-reduced-motion: reduce) {
  .ts-spin {
    animation: none;
  }
}
.ts-admin {
  border: 1px solid rgba(128, 128, 128, 0.22);
  border-radius: 10px;
  overflow: hidden;
  font-size: 12px;
}
.ts-admin__bar {
  padding: 6px 10px;
  background: rgba(128, 128, 128, 0.08);
  border-bottom: 1px solid rgba(128, 128, 128, 0.18);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 11px;
  opacity: 0.7;
}
.ts-admin__row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  padding: 10px 12px;
  border-bottom: 1px solid rgba(128, 128, 128, 0.14);
}
.ts-admin__row:last-child {
  border-bottom: 0;
}
.ts-admin__row b {
  display: block;
  font-weight: 500;
}
.ts-admin__row span {
  opacity: 0.65;
}
.ts-admin__row--todo {
  background: rgba(212, 136, 6, 0.1);
  box-shadow: inset 3px 0 0 #d48806;
}
.ts-admin__state {
  white-space: nowrap;
}
.ts-confirm {
  display: grid;
  gap: 8px;
  border: 1px solid rgba(128, 128, 128, 0.2);
  border-radius: 10px;
  padding: 12px 14px;
  background: rgba(128, 128, 128, 0.05);
  font-size: 13px;
}
.ts-confirm code {
  display: block;
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 12px;
  border: 1px solid rgba(128, 128, 128, 0.2);
  border-radius: 6px;
  padding: 6px 8px;
  overflow-x: auto;
  white-space: nowrap;
}
.ts-confirm code.inline {
  display: inline;
  padding: 1px 4px;
}
.ts-confirm ul {
  margin: 0;
  padding-left: 1.2em;
  display: grid;
  gap: 2px;
  font-size: 12.5px;
  opacity: 0.8;
}
.ts-phone {
  display: grid;
  grid-template-columns: 132px 1fr;
  gap: 16px;
  align-items: center;
}
.ts-qr {
  display: flex;
  width: 132px;
  aspect-ratio: 1;
  align-items: center;
  justify-content: center;
  border-radius: 12px;
  border: 1px solid rgba(128, 128, 128, 0.18);
  background: #fff;
  padding: 6px;
}
.ts-qr img {
  width: 100%;
  height: auto;
}
.ts-footer {
  display: flex;
  padding-top: 12px;
  border-top: 1px solid rgba(128, 128, 128, 0.15);
  font-size: 13px;
}
@media (max-width: 520px) {
  .ts-phone {
    grid-template-columns: 1fr;
    justify-items: center;
  }
}
</style>
