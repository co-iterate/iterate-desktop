<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'

interface Entry {
  id: string
  spokenPhrase: string
  outputText: string
  trainingCount: number
  isEnabled: boolean
}

const message = useMessage()
const loading = ref(false)
const saving = ref(false)
const entries = ref<Entry[]>([])
let savedEntries: unknown = []
const showModal = ref(false)
const editingId = ref<string | null>(null)
const form = ref<Entry>({
  id: '',
  spokenPhrase: '',
  outputText: '',
  trainingCount: 0,
  isEnabled: true,
})

function normalizeEntries(value: unknown): Entry[] {
  return (Array.isArray(value) ? value : []).map((item: any) => ({
    id: String(item?.id || `speech-memory-${Date.now()}-${Math.random().toString(36).slice(2)}`),
    spokenPhrase: String(item?.spokenPhrase || '').trim(),
    outputText: String(item?.outputText || '').trim(),
    trainingCount: Number(item?.trainingCount || 0),
    isEnabled: item?.isEnabled !== false,
  }))
}

const activeEntries = computed(() =>
  entries.value.filter(entry => entry.isEnabled && entry.trainingCount >= 4),
)

async function loadEntries() {
  loading.value = true
  try {
    const result = await invoke('get_speech_muscle_memory_entries')
    savedEntries = structuredClone(result)
    entries.value = normalizeEntries(result)
  }
  catch (error) {
    console.error('加载肌肉记忆库失败:', error)
    message.error('加载肌肉记忆库失败')
  }
  finally {
    loading.value = false
  }
}

async function persistEntries(nextEntries: Entry[], successMessage?: string) {
  saving.value = true
  try {
    const result = await invoke('save_speech_muscle_memory_entries', {
      entries: nextEntries,
      expectedEntries: savedEntries,
    })
    savedEntries = structuredClone(result)
    entries.value = normalizeEntries(result)
    if (successMessage)
      message.success(successMessage)
  }
  catch (error) {
    console.error('保存肌肉记忆库失败:', error)
    message.error(String(error))
  }
  finally {
    saving.value = false
  }
}

function resetForm() {
  editingId.value = null
  form.value = {
    id: '',
    spokenPhrase: '',
    outputText: '',
    trainingCount: 0,
    isEnabled: true,
  }
}

function openCreateModal() {
  resetForm()
  showModal.value = true
}

function openEditModal(entry: Entry) {
  editingId.value = entry.id
  form.value = { ...entry }
  showModal.value = true
}

async function submitForm() {
  const spokenPhrase = form.value.spokenPhrase.trim()
  const outputText = form.value.outputText.trim()
  if (!spokenPhrase || !outputText) {
    message.warning('短语和输出都不能为空')
    return
  }

  const nextEntry: Entry = {
    ...form.value,
    id: editingId.value || `speech-memory-${Date.now()}-${Math.random().toString(36).slice(2)}`,
    spokenPhrase,
    outputText,
  }

  const nextEntries = editingId.value
    ? entries.value.map(entry => entry.id === editingId.value ? nextEntry : entry)
    : [nextEntry, ...entries.value]

  await persistEntries(nextEntries, editingId.value ? '已更新词条' : '已新增词条')
  showModal.value = false
  resetForm()
}

async function toggleEntry(id: string) {
  await persistEntries(
    entries.value.map(entry =>
      entry.id === id ? { ...entry, isEnabled: !entry.isEnabled } : entry,
    ),
    '状态已更新',
  )
}

async function deleteEntry(id: string) {
  await persistEntries(
    entries.value.filter(entry => entry.id !== id),
    '已删除词条',
  )
}

onMounted(() => {
  loadEntries()
})
</script>

<template>
  <div class="space-y-4">
    <div class="rounded-xl border border-[var(--n-border-color)] bg-[var(--n-card-color)] p-4">
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="text-base font-medium">
            语音肌肉记忆库
          </div>
          <div class="text-sm opacity-70 mt-1">
            桌面端现在可以直接管理词条，iPhone 端下次打开 `/mobile` 时会同步到本机缓存。
          </div>
        </div>
        <n-button type="primary" :loading="saving" @click="openCreateModal">
          新增词条
        </n-button>
      </div>
      <div class="mt-3 text-sm opacity-70">
        已激活 {{ activeEntries.length }} 条，共 {{ entries.length }} 条。训练满 4 次后进入激活状态。
      </div>
    </div>

    <n-spin :show="loading">
      <div
        v-if="entries.length"
        class="space-y-3"
      >
        <div
          v-for="entry in entries"
          :key="entry.id"
          class="rounded-xl border border-[var(--n-border-color)] bg-[var(--n-card-color)] p-4"
        >
          <div class="flex items-start justify-between gap-4">
            <div class="min-w-0 flex-1">
              <div class="text-base font-medium break-words">
                {{ entry.spokenPhrase }}
              </div>
              <div class="text-sm opacity-70 mt-2 break-words">
                输出：{{ entry.outputText }}
              </div>
              <div class="text-xs mt-2" :class="entry.trainingCount >= 4 ? 'text-emerald-500' : 'opacity-60'">
                训练 {{ entry.trainingCount }}/4
                <span v-if="!entry.isEnabled"> · 已禁用</span>
              </div>
            </div>
            <div class="flex flex-col gap-2 shrink-0">
              <n-button size="small" @click="openEditModal(entry)">
                编辑
              </n-button>
              <n-button size="small" @click="toggleEntry(entry.id)">
                {{ entry.isEnabled ? '停用' : '启用' }}
              </n-button>
              <n-popconfirm @positive-click="deleteEntry(entry.id)">
                <template #trigger>
                  <n-button size="small" type="error" secondary>
                    删除
                  </n-button>
                </template>
                删除这条语音词条？
              </n-popconfirm>
            </div>
          </div>
        </div>
      </div>
      <div
        v-else
        class="rounded-xl border border-dashed border-[var(--n-border-color)] p-8 text-center opacity-70"
      >
        还没有词条，先新增一条常说短语和固定输出。
      </div>
    </n-spin>

    <n-modal
      v-model:show="showModal"
      preset="card"
      :title="editingId ? '编辑词条' : '新增词条'"
      style="max-width: 560px;"
    >
      <n-form label-placement="top">
        <n-form-item label="你会说的话" required>
          <n-input
            v-model:value="form.spokenPhrase"
            placeholder="例如：把这个发给张三"
          />
        </n-form-item>
        <n-form-item label="固定输出文本" required>
          <n-input
            v-model:value="form.outputText"
            type="textarea"
            :autosize="{ minRows: 3, maxRows: 8 }"
            placeholder="例如：请把这份材料发送给张三，并抄送我。"
          />
        </n-form-item>
        <n-form-item label="当前训练次数">
          <n-input-number v-model:value="form.trainingCount" :min="0" />
        </n-form-item>
        <n-form-item label="启用状态">
          <n-switch v-model:value="form.isEnabled" />
        </n-form-item>
      </n-form>
      <template #footer>
        <div class="flex justify-end gap-2">
          <n-button @click="showModal = false">
            取消
          </n-button>
          <n-button type="primary" :loading="saving" @click="submitForm">
            保存
          </n-button>
        </div>
      </template>
    </n-modal>
  </div>
</template>
