<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'

interface ClientStatus {
  id: string
  name: string
  detected: boolean
  configured: boolean
  points_to_current: boolean
  needs_repair: boolean
  opted_out: boolean
  config_path: string
  configured_command: string | null
}

interface AppLocationStatus {
  ok: boolean
  bundled_server: string | null
  stable_server: string
  problem: string | null
}

interface ClientOverview {
  app_location: AppLocationStatus
  clients: ClientStatus[]
}

interface ClientActionResult {
  id: string
  name: string
  ok: boolean
  message: string
  backup_path: string | null
}

const props = withDefaults(defineProps<{
  /** 新手引导里使用：隐藏未检测到的客户端和逐项移除按钮。 */
  compact?: boolean
}>(), {
  compact: false,
})

const emit = defineEmits<{
  (e: 'connected', results: ClientActionResult[]): void
}>()

const message = useMessage()

const overview = ref<ClientOverview | null>(null)
const selected = ref<string[]>([])
const loading = ref(true)
const working = ref(false)
const results = ref<ClientActionResult[]>([])

const visibleClients = computed(() => {
  const clients = overview.value?.clients ?? []
  return props.compact ? clients.filter(client => client.detected) : clients
})

const pendingClients = computed(() =>
  visibleClients.value.filter(client => !client.points_to_current),
)

const canConnect = computed(() =>
  !working.value
  && overview.value?.app_location.ok === true
  && selected.value.length > 0,
)

/** 远程（URL）方式的已有条目不自动迁移，避免产生 command / url 混合配置。 */
function isRemote(client: ClientStatus) {
  return /^(?:https?|wss?):\/\//.test(client.configured_command ?? '')
}

function statusLabel(client: ClientStatus) {
  if (client.points_to_current)
    return { text: '已接入', type: 'success' as const }
  if (client.needs_repair)
    return { text: '需修复', type: 'error' as const }
  if (client.configured)
    return { text: '指向其他位置', type: 'warning' as const }
  if (client.opted_out)
    return { text: '已移除', type: 'default' as const }
  if (client.detected)
    return { text: '未接入', type: 'info' as const }
  return { text: '未检测到', type: 'default' as const }
}

async function loadOverview() {
  loading.value = true
  try {
    overview.value = await invoke<ClientOverview>('get_mcp_client_overview')
    // 默认勾选：检测到、尚未配置、且用户没有主动移除过的客户端；以及 iterate 自己失效待修复的条目。
    // 用户已有的自定义入口（wrapper、其他版本）不默认勾选，需用户看到当前命令后自己决定是否迁移。
    selected.value = pendingClients.value
      .filter(client => client.needs_repair
        || (client.detected && !client.configured && !client.opted_out))
      .map(client => client.id)
  }
  catch (error) {
    console.error('读取 AI 客户端状态失败:', error)
    message.error('读取 AI 客户端状态失败')
  }
  finally {
    loading.value = false
  }
}

async function connectSelected() {
  if (!canConnect.value)
    return
  working.value = true
  try {
    const output = await invoke<ClientActionResult[]>('connect_mcp_clients', { clients: selected.value })
    results.value = output
    const failed = output.filter(result => !result.ok)
    if (failed.length === 0)
      message.success('已接入，按提示重启对应客户端后即可使用')
    else
      message.warning(`${failed.length} 个客户端接入失败，详见下方说明`)
    emit('connected', output)
  }
  catch (error) {
    console.error('接入 AI 客户端失败:', error)
    message.error('接入 AI 客户端失败')
  }
  finally {
    working.value = false
    await loadOverview()
  }
}

async function disconnect(client: ClientStatus) {
  working.value = true
  try {
    const result = await invoke<ClientActionResult>('disconnect_mcp_client', { client: client.id })
    results.value = [result]
    if (result.ok)
      message.success(`已从 ${client.name} 移除`)
    else
      message.error(result.message)
  }
  catch (error) {
    console.error('移除 AI 客户端失败:', error)
    message.error('移除失败')
  }
  finally {
    working.value = false
    await loadOverview()
  }
}

onMounted(() => {
  loadOverview()
})
</script>

<template>
  <n-space vertical size="medium">
    <n-alert v-if="overview && !overview.app_location.ok" type="warning" :show-icon="true">
      {{ overview.app_location.problem }}
    </n-alert>

    <div v-if="loading && !overview" class="text-xs opacity-60">
      正在检测本机的 AI 客户端…
    </div>

    <div v-else-if="visibleClients.length === 0" class="text-xs opacity-60">
      没有检测到 Claude Code、Codex 或 Cursor。其他客户端可以使用“复制安装提示词”手动接入。
    </div>

    <n-checkbox-group v-else v-model:value="selected">
      <div
        v-for="client in visibleClients"
        :key="client.id"
        class="flex items-center justify-between py-2"
      >
        <div class="flex items-center min-w-0">
          <n-checkbox
            :value="client.id"
            :disabled="client.points_to_current || isRemote(client) || working"
            class="mr-3"
          />
          <div class="min-w-0">
            <div class="text-sm font-medium leading-relaxed">
              {{ client.name }}
            </div>
            <div class="text-xs opacity-60 truncate" :title="client.config_path">
              {{ client.config_path }}
            </div>
            <div
              v-if="isRemote(client)"
              class="text-xs text-warning truncate"
              :title="client.configured_command ?? ''"
            >
              已有远程配置：{{ client.configured_command }}；需先手动移除后才能接入
            </div>
            <div
              v-else-if="client.configured && !client.points_to_current && !client.needs_repair"
              class="text-xs text-warning truncate"
              :title="client.configured_command ?? ''"
            >
              当前指向：{{ client.configured_command }}；勾选后将改为 iterate 自带的版本
            </div>
            <div v-else-if="client.needs_repair" class="text-xs text-error">
              iterate 的入口已失效（App 可能被移动过），勾选后重新接入即可修复
            </div>
          </div>
        </div>
        <div class="flex items-center gap-2 flex-shrink-0 ml-3">
          <n-tag size="small" :type="statusLabel(client).type" :bordered="false">
            {{ statusLabel(client).text }}
          </n-tag>
          <n-button
            v-if="!props.compact && client.configured"
            size="tiny"
            quaternary
            :disabled="working"
            @click="disconnect(client)"
          >
            移除
          </n-button>
        </div>
      </div>
    </n-checkbox-group>

    <div class="flex items-center justify-between">
      <div class="text-xs opacity-60">
        写入前会自动备份原配置，只改动 iterate-zhi 这一项。
      </div>
      <n-button
        type="primary"
        size="small"
        :loading="working"
        :disabled="!canConnect"
        @click="connectSelected"
      >
        接入所选客户端
      </n-button>
    </div>

    <div v-if="results.length > 0" class="space-y-1">
      <div
        v-for="result in results"
        :key="result.id"
        class="text-xs"
        :class="result.ok ? 'text-success' : 'text-error'"
      >
        {{ result.name }}：{{ result.message }}
      </div>
    </div>
  </n-space>
</template>
