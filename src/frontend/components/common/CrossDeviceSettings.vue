<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref, watch } from 'vue'
import { refreshCrossDevice, useCrossDevice } from '../../composables/useCrossDevice'
import SyncSettingsPreview from './SyncSettingsPreview.vue'

interface ConnectionConfig {
  device_name: string
  listen_ip: string
  listen_port: number
  peer_host: string
  peer_port: number
}
interface ConfigResult { config: ConnectionConfig, peer_name: string | null }
const message = useMessage()
const busy = ref(false)
const syncingWindows = ref(false)
const syncError = ref('')
const syncNotice = ref('')
const loaded = ref(false)
const error = ref('')
const notice = ref('')
const peerName = ref<string | null>(null)
const pairingCode = ref('')
const ownCode = ref('')
const savedConfig = ref('')
const config = ref<ConnectionConfig>({ device_name: '', listen_ip: '', listen_port: 5540, peer_host: '', peer_port: 5540 })
const { crossState } = useCrossDevice()
const hasChanges = computed(() => JSON.stringify(config.value) !== savedConfig.value)
const incomingPeerName = computed(() => {
  try {
    const code = pairingCode.value.trim()
    if (!code.startsWith('iterate-pair-v2:'))
      return ''
    const bytes = Uint8Array.from(atob(code.slice('iterate-pair-v2:'.length)), char => char.charCodeAt(0))
    const pair = JSON.parse(new TextDecoder().decode(bytes))
    return typeof pair.name === 'string' ? pair.name.slice(0, 64) : ''
  }
  catch { return '' }
})
watch(config, () => {
  ownCode.value = ''
}, { deep: true })
const status = computed(() => crossState.value.connected
  ? (crossState.value.peer_enabled ? '加密通道已连接，对端已开启' : '加密通道已连接，等待对端开启')
  : '对端未连接')

async function loadSettings() {
  busy.value = true
  loaded.value = false
  error.value = ''
  notice.value = ''
  pairingCode.value = ''
  ownCode.value = ''
  try {
    const result = await invoke<ConfigResult>('get_cross_device_config')
    config.value = result.config
    peerName.value = result.peer_name
    savedConfig.value = JSON.stringify(result.config)
    loaded.value = true
  }
  catch (cause) { error.value = String(cause) }
  finally { busy.value = false }
}

async function runAction(action: 'save' | 'test' | 'generate' | 'pair' | 'paste') {
  if (busy.value || !loaded.value)
    return
  if (![config.value.listen_port, config.value.peer_port].every(port => Number.isInteger(port) && port >= 1 && port <= 65535)) {
    error.value = '端口须为 1–65535'
    return
  }
  busy.value = true
  error.value = ''
  notice.value = ''
  const importing = action === 'pair' || action === 'paste'
  let imported = false
  try {
    if (action === 'paste') {
      const clipboard = await invoke<string | null>('plugin:clipboard-manager|read_text')
      pairingCode.value = clipboard?.trim() || ''
      if (!pairingCode.value)
        throw new Error('剪贴板为空，请先在另一端复制本机配对码。')
    }
    if (importing && !pairingCode.value.trim().startsWith('iterate-pair-v2:'))
      throw new Error('剪贴板内容不是 iterate 配对码，请在另一端点击“复制本机配对码”。')
    if (action !== 'test') {
      const result = await invoke<ConfigResult>('save_cross_device_config', {
        config: importing ? { ...config.value, peer_host: '', backup_peer_host: '' } : config.value,
        pairingCode: importing ? pairingCode.value.trim() : null,
      })
      config.value = result.config
      peerName.value = result.peer_name
      savedConfig.value = JSON.stringify(result.config)
      ownCode.value = ''
      if (importing) {
        imported = true
        pairingCode.value = ''
        notice.value = `已添加对端 ${peerName.value || ''}，正在检查连接。`
      }
      else {
        notice.value = '本机地址已保存，接收服务已就绪。'
      }
    }
    if (action === 'generate') {
      ownCode.value = await invoke<string>('generate_cross_device_pairing')
      await copyCode()
    }
    else if (action === 'test' || importing) {
      const result = await invoke<{ message: string }>('test_cross_device_connection', { config: config.value, pairingCode: null })
      notice.value = result.message
      if (importing)
        message.success(`已写入对端配置并连接：${peerName.value || '对端'}`)
    }
  }
  catch (cause) {
    error.value = String(cause)
    if (imported) {
      notice.value = '对端资料已保存，连接检查未通过。请确认另一端也已导入本机配对码，且地址可达。'
      message.warning('对端配置已写入，连接尚未成功')
    }
    else { message.error(String(cause)) }
  }
  finally {
    busy.value = false
    void refreshCrossDevice().catch(() => {})
  }
}

async function copyCode() {
  try {
    await invoke('plugin:clipboard-manager|write_text', { text: ownCode.value })
    notice.value = ''
    message.success('本机配对码已复制，在另一端点击“一键粘贴并写入”')
  }
  catch {
    error.value = '复制失败，请选中配对码手动复制。'
    message.error(error.value)
  }
}

async function syncWindows() {
  if (busy.value || syncingWindows.value)
    return
  syncingWindows.value = true
  syncError.value = ''
  syncNotice.value = ''
  try {
    const result = await invoke<{ count: number }>('sync_cross_device_windows')
    syncNotice.value = result.count
      ? `已同步对端 ${result.count} 个窗口，已有窗口不会重复打开。`
      : '对端没有可同步的待回复窗口。'
    message.success(syncNotice.value)
  }
  catch (cause) {
    syncError.value = String(cause)
    message.error(syncError.value)
  }
  finally {
    syncingWindows.value = false
  }
}

onMounted(loadSettings)
</script>

<template>
  <section aria-label="跨设备配置">
    <div class="mb-4">
      <n-button type="primary" :loading="syncingWindows" :disabled="syncingWindows || busy || !loaded || !peerName || hasChanges || !!pairingCode.trim()" @click="syncWindows">
        同步对端已打开窗口
      </n-button>
      <p class="mt-2 text-xs opacity-75">
        断线或未连接期间打开的窗口，可在重新连接并开启两端跨设备开关后，点击上方按钮补同步一次。
      </p>
      <p v-if="syncError" role="alert" class="mt-2 text-red-500 whitespace-pre-wrap break-all">
        {{ syncError }}
      </p>
      <p v-if="syncNotice" role="status" class="mt-2 text-green-600">
        {{ syncNotice }}
      </p>
    </div>
    <p class="mb-3 text-sm opacity-75">
      这边复制本机配对码，另一端一键粘贴并写入，再反向操作一次。复制时自动保存本机地址，粘贴时自动写入对端地址并检查连接。
    </p>
    <n-form label-placement="top" size="small" :disabled="busy || !loaded" :show-feedback="false">
      <n-form-item label="本机名称" class="mb-3">
        <n-input v-model:value="config.device_name" aria-label="本机名称" placeholder="例如：办公电脑" :maxlength="64" />
      </n-form-item>
      <div class="flex gap-3 mb-3">
        <n-form-item label="本机 IP（主 / 备用 / …）" class="flex-1">
          <n-input v-model:value="config.listen_ip" aria-label="本机 IP" placeholder="192.168.1.10/10.8.0.4/公网 IP" />
        </n-form-item>
        <n-form-item label="接收端口" style="width: 110px">
          <n-input-number v-model:value="config.listen_port" aria-label="接收端口" :min="1" :max="65535" :precision="0" :show-button="false" />
        </n-form-item>
      </div>
      <p class="mb-3 text-xs opacity-75">
        本机列表包含各工作环境的网卡、VPN 或公网 IP，须包含当前网卡地址。公网地址需在路由器映射到本机接收端口；切换网络后自动使用当前可用的本机地址。
      </p>
      <div class="flex gap-3 mb-3">
        <n-form-item label="对端 IP（主 / 备用 / …）" class="flex-1">
          <n-input v-model:value="config.peer_host" aria-label="对端 IP" placeholder="192.168.1.20/10.8.0.3/公网 IP；也可导入配对码" />
        </n-form-item>
        <n-form-item label="对端端口" style="width: 110px">
          <n-input-number v-model:value="config.peer_port" aria-label="对端端口" :min="1" :max="65535" :precision="0" :show-button="false" />
        </n-form-item>
      </div>
      <p class="mb-3 text-xs opacity-75">
        按填写顺序连接，当前地址不通时尝试下一项；各地址使用同一端口，均须通向同一台已配对设备。
      </p>
      <n-form-item label="对端配对码" class="mb-3">
        <n-input v-model:value="pairingCode" type="password" show-password-on="click" aria-label="对端配对码" :placeholder="peerName ? `已添加：${peerName}；更换时粘贴新码` : '粘贴另一台设备生成的配对码'" />
      </n-form-item>
    </n-form>
    <p v-if="incomingPeerName" class="text-sm mb-3">
      即将添加对端：{{ incomingPeerName }}
    </p>
    <div class="flex flex-wrap gap-2 mb-3">
      <n-button size="small" :disabled="busy || !loaded" @click="ownCode && !hasChanges ? copyCode() : runAction('generate')">
        复制本机配对码
      </n-button>
      <n-button type="primary" size="small" :loading="busy" :disabled="!loaded" @click="runAction('paste')">
        一键粘贴并写入
      </n-button>
    </div>
    <n-input v-if="ownCode" :value="ownCode" type="password" show-password-on="click" readonly aria-label="本机配对码" class="mb-3" />
    <p class="text-xs opacity-75 mb-3">
      配对信息保存在本机 JSON 文件中，不参与设置同步。
    </p>
    <p role="status" class="mb-3">
      {{ status }}<span v-if="crossState.connected && crossState.connected_ip"> · {{ crossState.using_backup ? '备用 IP' : '主 IP' }} {{ crossState.connected_ip }}</span><span v-if="peerName"> · 已添加对端 {{ peerName }}</span><span> · 本机提醒{{ crossState.enabled ? '已开启' : '未开启' }}</span>
    </p>
    <p v-if="error" role="alert" class="mb-3 text-red-500 whitespace-pre-wrap break-all">
      {{ error }}
    </p>
    <p v-if="notice" role="status" class="mb-3 text-green-600">
      {{ notice }}
    </p>
    <div class="flex flex-wrap items-center justify-end gap-2">
      <SyncSettingsPreview :disabled="busy || !loaded || !peerName || hasChanges || !!pairingCode.trim()" :peer-name="peerName" />
      <n-button v-if="peerName && !pairingCode.trim()" :loading="busy" :disabled="!loaded || hasChanges" @click="runAction('test')">
        重新检查连接
      </n-button>
      <n-button v-if="peerName && hasChanges && !pairingCode.trim()" :loading="busy" :disabled="!loaded" @click="runAction('save')">
        保存修改
      </n-button>
      <n-button v-if="pairingCode.trim()" type="primary" :loading="busy" :disabled="!loaded" @click="runAction('pair')">
        写入并连接
      </n-button>
    </div>
  </section>
</template>
