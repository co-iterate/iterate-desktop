<script setup lang="ts">
import type { KnownFormalMobileRoute, MobileRouteOptions } from './mobileRouteOptions'
import { invoke } from '@tauri-apps/api/core'
import { computed, onMounted, ref, watch } from 'vue'
import { bridgeFetch } from '../../services/bridgeFetch'
import { buildAliyunIpRouteSetupPrompt, buildFormalRouteSetupPrompt } from './mobileConnectionPrompts'
import {
  mobileRouteDescription,
  mobileRouteLabel,
  mobileRouteSwitchErrorText,
} from './mobileRouteOptions'
import { TAILSCALE_SERVE_TRANSPORT, tailscaleServeBlockerText, tailscaleServeChecklist } from './tailscaleServeRoute'

// 「连接 iPhone → 管理连接」：已连接的 iPhone、合盖保持运行、连接方式（高级）
const props = defineProps<{ panel: 'manage' | 'choose' }>()
const emit = defineEmits<{
  'update:panel': [panel: 'manage' | 'choose' | 'tailscale']
  'back': []
  'switched': [label: string]
}>()

interface PhoneClient {
  client_id: string
  client_kind?: string
  device_id?: string
  selected_transport_mode?: string
  last_seen_at?: string
}

const options = ref<MobileRouteOptions | null>(null)
const loading = ref(false)
const loadError = ref('')
const phones = ref<PhoneClient[]>([])
const keepAwake = ref<boolean | null>(null)
const keepAwakePending = ref(false)
const choice = ref('')
const switching = ref(false)
const switchError = ref('')
const copied = ref('')

const currentTransport = computed(() => options.value?.current.configured ? options.value.current.transport ?? null : null)
const currentHealthy = computed(() => options.value?.current.health === 'healthy')
const tailscaleChecks = computed(() => options.value ? tailscaleServeChecklist(options.value.tailscale) : [])

interface RouteChoice {
  key: string
  transport: string
  base_url: string
  title: string
  description: string
  tag?: string
  ready: boolean
  blocker?: string
}

// 当前路线 + Tailscale（本机检测）+ 之前验证过的路线
const choices = computed<RouteChoice[]>(() => {
  const data = options.value
  if (!data)
    return []
  const list: RouteChoice[] = []
  const seen = new Set<string>()
  const push = (item: RouteChoice) => {
    if (seen.has(item.key))
      return
    seen.add(item.key)
    list.push(item)
  }
  if (data.current.configured && data.current.transport && data.current.base_url) {
    push({
      key: `${data.current.transport}|${data.current.base_url}`,
      transport: data.current.transport,
      base_url: data.current.base_url,
      title: mobileRouteLabel(data.current.transport),
      description: mobileRouteDescription(data.current.transport),
      tag: '当前',
      ready: true,
    })
  }
  const tailscaleUrl = data.tailscale.base_url || (data.tailscale.dns_name ? `https://${data.tailscale.dns_name}` : '')
  push({
    key: `${TAILSCALE_SERVE_TRANSPORT}|${tailscaleUrl}`,
    transport: TAILSCALE_SERVE_TRANSPORT,
    base_url: tailscaleUrl,
    title: 'Tailscale',
    description: mobileRouteDescription(TAILSCALE_SERVE_TRANSPORT),
    tag: '推荐',
    ready: !data.tailscale.blocker,
    blocker: data.tailscale.blocker ? tailscaleServeBlockerText(data.tailscale.blocker) : undefined,
  })
  data.known.forEach((route: KnownFormalMobileRoute) => push({
    key: `${route.transport}|${route.base_url}`,
    transport: route.transport,
    base_url: route.base_url,
    title: mobileRouteLabel(route.transport),
    description: mobileRouteDescription(route.transport),
    tag: '之前用过',
    ready: true,
  }))
  return list
})

const selected = computed(() => choices.value.find(item => item.key === choice.value) || null)
const selectedIsCurrent = computed(() => selected.value?.tag === '当前')
const canSwitch = computed(() => Boolean(selected.value?.ready && !selectedIsCurrent.value && !switching.value))

async function loadOptions() {
  loading.value = true
  loadError.value = ''
  try {
    options.value = await invoke<MobileRouteOptions>('get_mobile_route_options')
    // 打开时默认选中当前路线，展开它的详情
    if (!choice.value && choices.value[0])
      choice.value = choices.value[0].key
  }
  catch (cause) {
    loadError.value = String(cause || '读取连接方式失败')
  }
  finally {
    loading.value = false
  }
}

async function loadPhones() {
  try {
    const response = await bridgeFetch('http://127.0.0.1:8080/api/connection-status', { cache: 'no-store' })
    const data = await response.json()
    const clients: PhoneClient[] = data?.websocket?.clients || []
    phones.value = clients.filter(client => (client.client_kind || '').toLowerCase().includes('ios'))
  }
  catch {
    phones.value = []
  }
}

async function loadKeepAwake() {
  try {
    keepAwake.value = await invoke<boolean>('get_prevent_sleep_status')
  }
  catch {
    keepAwake.value = null
  }
}

async function toggleKeepAwake() {
  if (keepAwakePending.value)
    return
  keepAwakePending.value = true
  try {
    keepAwake.value = await invoke<boolean>('toggle_prevent_sleep')
  }
  finally {
    keepAwakePending.value = false
  }
}

function phoneMeta(client: PhoneClient) {
  const seen = client.last_seen_at ? new Date(client.last_seen_at) : null
  const ago = seen && !Number.isNaN(seen.getTime())
    ? `${Math.max(0, Math.round((Date.now() - seen.getTime()) / 60000))} 分钟前活跃`
    : '在线'
  const route = client.selected_transport_mode === 'tailscale' || client.selected_transport_mode === 'public_tunnel'
    ? `经 ${mobileRouteLabel(currentTransport.value)}`
    : ''
  return [ago, route].filter(Boolean).join(' · ')
}

function openChooser() {
  choice.value = ''
  switchError.value = ''
  copied.value = ''
  emit('update:panel', 'choose')
  void loadOptions()
}

async function switchRoute() {
  const target = selected.value
  if (!target || !canSwitch.value)
    return
  switching.value = true
  switchError.value = ''
  try {
    await invoke('switch_formal_mobile_route', { transport: target.transport, baseUrl: target.base_url })
    emit('switched', target.title)
  }
  catch (cause) {
    const code = typeof cause === 'string' ? cause : cause instanceof Error ? cause.message : 'formal_route_switch_failed'
    switchError.value = mobileRouteSwitchErrorText(code, tailscaleServeBlockerText)
    void loadOptions()
  }
  finally {
    switching.value = false
  }
}

async function copyPrompt(kind: 'cloudflare' | 'aliyun') {
  await navigator.clipboard.writeText(kind === 'aliyun' ? buildAliyunIpRouteSetupPrompt() : buildFormalRouteSetupPrompt())
  copied.value = kind === 'aliyun' ? '阿里云配置提示词已复制，交给 AI 配置完成后回到这里切换。' : 'Cloudflare 配置提示词已复制，交给 AI 配置完成后回到这里切换。'
}

let manageLoaded = false
function loadManageDetails() {
  if (manageLoaded)
    return
  manageLoaded = true
  void loadPhones()
  void loadKeepAwake()
}

watch(() => props.panel, (panel) => {
  if (panel === 'manage')
    loadManageDetails()
})

onMounted(() => {
  void loadOptions()
  if (props.panel === 'manage')
    loadManageDetails()
})
</script>

<template>
  <div class="manager">
    <template v-if="panel === 'manage'">
      <div>
        <div class="text-base font-semibold">
          管理连接
        </div>
        <div class="mt-1 text-xs opacity-60">
          能控制这台 Mac 的 iPhone，以及它们怎么连过来。
        </div>
      </div>

      <section class="block">
        <div class="block-label">
          已连接的 iPhone
        </div>
        <div v-if="phones.length" class="grid gap-2">
          <div v-for="(phone, index) in phones" :key="phone.client_id" class="row">
            <div class="grid">
              <span class="font-medium">iterate Remote · iPhone {{ phones.length > 1 ? index + 1 : '' }}</span>
              <span class="text-xs opacity-60">{{ phoneMeta(phone) }}</span>
            </div>
          </div>
        </div>
        <div v-else class="text-xs opacity-60">
          现在没有在线的 iPhone。扫码连接后会出现在这里。
        </div>
      </section>

      <section class="block">
        <div class="row">
          <div class="grid">
            <span class="font-medium">合盖保持运行</span>
            <span class="text-xs opacity-60">接通电源时阻止这台 Mac 休眠，手机才能一直连上。</span>
          </div>
          <n-switch
            :value="keepAwake === true"
            :loading="keepAwakePending"
            :disabled="keepAwake === null"
            @update:value="toggleKeepAwake"
          />
        </div>
      </section>

      <section class="block">
        <div class="block-label">
          高级 · 连接方式
        </div>
        <div class="row">
          <div class="grid min-w-0">
            <span class="font-medium">
              {{ mobileRouteLabel(currentTransport) }}
              <span v-if="options" class="state-pill" :class="currentHealthy ? 'state-pill--ok' : 'state-pill--warn'">
                {{ currentHealthy ? '正常' : '连不上' }}
              </span>
            </span>
            <span class="text-xs opacity-60 break-all">{{ options?.current.base_url || '还没有配置' }}</span>
          </div>
          <button type="button" class="wizard-link flex-shrink-0" @click="openChooser">
            更换连接方式
          </button>
        </div>
      </section>

      <div class="flex justify-end">
        <n-button class="tap-target" @click="emit('back')">
          返回二维码
        </n-button>
      </div>
    </template>

    <template v-else>
      <div>
        <div class="text-base font-semibold">
          选择连接方式
        </div>
        <div class="mt-1 text-xs opacity-60">
          每种方式都会先验证确实连到这台 Mac，通过后才切换；失败时保留当前连接方式。
        </div>
      </div>

      <n-spin :show="loading && !options">
        <n-radio-group v-model:value="choice" class="w-full" name="mobile-route">
          <div class="grid gap-2">
            <label
              v-for="item in choices"
              :key="item.key"
              class="choice"
              :class="{ 'choice--selected': choice === item.key }"
            >
              <n-radio :value="item.key" />
              <div class="grid gap-1 min-w-0">
                <span class="font-medium flex flex-wrap items-center gap-2">
                  {{ item.title }}
                  <span v-if="item.tag" class="tag" :class="{ 'tag--current': item.tag === '当前' }">{{ item.tag }}</span>
                </span>
                <span class="text-xs opacity-65">{{ item.description }}</span>
                <!-- 只展开选中的那一项，其余只留名字和一句说明 -->
                <template v-if="choice === item.key">
                  <span v-if="item.base_url" class="text-xs opacity-50 break-all">{{ item.base_url }}</span>
                  <div v-if="item.transport === TAILSCALE_SERVE_TRANSPORT" class="checks">
                    <span v-for="check in tailscaleChecks" :key="check.label" :class="check.ok ? 'check--ok' : 'check--todo'">
                      {{ check.ok ? '✓' : '○' }} {{ check.label }}
                    </span>
                  </div>
                  <span v-if="item.blocker" class="text-xs text-warning">{{ item.blocker }}</span>
                  <button
                    v-if="item.transport === TAILSCALE_SERVE_TRANSPORT && !item.ready"
                    type="button"
                    class="wizard-link justify-self-start"
                    @click.prevent="emit('update:panel', 'tailscale')"
                  >
                    按步骤设置 Tailscale →
                  </button>
                  <code v-if="item.transport === TAILSCALE_SERVE_TRANSPORT && options?.tailscale.setup_command" class="command">{{ options.tailscale.setup_command }}</code>
                </template>
              </div>
            </label>
          </div>
        </n-radio-group>
      </n-spin>

      <div class="text-xs opacity-65">
        还没配置的方式：
        <button type="button" class="wizard-link" @click="copyPrompt('cloudflare')">
          复制 Cloudflare 配置提示词
        </button>
        ·
        <button type="button" class="wizard-link" @click="copyPrompt('aliyun')">
          复制阿里云配置提示词
        </button>
      </div>
      <n-alert v-if="copied" type="success" :bordered="false">
        {{ copied }}
      </n-alert>
      <n-alert v-if="loadError" type="error" :bordered="false">
        {{ loadError }}
      </n-alert>
      <n-alert v-if="switchError" type="warning" :bordered="false">
        {{ switchError }}
      </n-alert>

      <div class="text-xs text-warning">
        {{ selectedIsCurrent ? '这就是当前的连接方式。' : '切换后旧配对失效，手机需要重新扫一次码。' }}
      </div>
      <div class="flex justify-end gap-2 flex-wrap">
        <n-button class="tap-target" @click="emit('update:panel', 'manage')">
          取消
        </n-button>
        <n-button type="primary" class="tap-target" :disabled="!canSwitch" :loading="switching" @click="switchRoute">
          切换并生成新码
        </n-button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.manager {
  display: grid;
  gap: 14px;
}
.block {
  display: grid;
  gap: 10px;
  border: 1px solid var(--color-border, rgba(127, 127, 127, 0.22));
  border-radius: 10px;
  padding: 12px 14px;
}
.block-label {
  font-size: 12px;
  opacity: 0.6;
}
.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.state-pill {
  margin-left: 6px;
  font-size: 11px;
  font-weight: 400;
  border-radius: 999px;
  padding: 1px 8px;
}
.state-pill--ok {
  color: #18a058;
  background: rgba(24, 160, 88, 0.12);
}
.state-pill--warn {
  color: #d48806;
  background: rgba(212, 136, 6, 0.12);
}
.choice {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 10px;
  align-items: start;
  border: 1px solid var(--color-border, rgba(127, 127, 127, 0.22));
  border-radius: 10px;
  padding: 12px 14px;
  cursor: pointer;
}
.choice--selected {
  border-color: currentColor;
}
.tag {
  font-size: 11px;
  font-weight: 400;
  border-radius: 4px;
  padding: 0 6px;
  background: rgba(127, 127, 127, 0.14);
}
.tag--current {
  color: #18a058;
  background: rgba(24, 160, 88, 0.12);
}
.checks {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 12px;
  font-size: 12px;
}
.check--ok {
  color: #18a058;
}
.check--todo {
  opacity: 0.6;
}
.command {
  font-size: 12px;
  word-break: break-all;
  opacity: 0.8;
}
</style>
