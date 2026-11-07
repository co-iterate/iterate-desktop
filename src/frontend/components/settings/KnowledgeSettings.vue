<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'
import { buildKnowledgeSyncPrompt } from './knowledgeSyncPrompt'

interface SkillLinkStatus {
  client: string
  name: string
  path: string
  present: boolean
  available: boolean
}

interface KnowledgeOverview {
  root: string
  state: 'starter' | 'user' | 'missing'
  skills: SkillLinkStatus[]
  conversation_logging: boolean
}

const message = useMessage()
const overview = ref<KnowledgeOverview | null>(null)
const working = ref(false)

const stateLabel = computed(() => {
  switch (overview.value?.state) {
    case 'starter':
      return { text: 'iterate 已创建', type: 'success' as const }
    case 'user':
      return { text: '你自建的知识库', type: 'info' as const }
    default:
      return { text: '未创建', type: 'default' as const }
  }
})

const skillsByClient = computed(() => {
  const groups = new Map<string, SkillLinkStatus[]>()
  for (const skill of overview.value?.skills ?? []) {
    const list = groups.get(skill.client) ?? []
    list.push(skill)
    groups.set(skill.client, list)
  }
  return [...groups.entries()]
})

const linkableMissing = computed(() =>
  (overview.value?.skills ?? []).some(skill => !skill.present && skill.available),
)

const canInstall = computed(() =>
  overview.value !== null
  && (overview.value.state === 'missing' || linkableMissing.value),
)

const installLabel = computed(() => {
  if (overview.value?.state === 'missing')
    return '创建知识库'
  return overview.value?.state === 'user' ? '挂载已有 Skill' : '补挂缺少的 Skill'
})

function skillTagText(skill: SkillLinkStatus) {
  if (skill.present)
    return skill.name
  return skill.available ? `${skill.name}（未挂）` : `${skill.name}（知识库里没有）`
}

async function load() {
  try {
    overview.value = await invoke<KnowledgeOverview>('get_knowledge_overview')
  }
  catch (error) {
    console.error('读取知识库状态失败:', error)
  }
}

async function install() {
  working.value = true
  try {
    overview.value = await invoke<KnowledgeOverview>('install_knowledge_starter')
    message.success('已挂载，重启 Claude Code / Codex 后可用 /hui、/xi、/ji')
  }
  catch (error) {
    message.error(String(error))
  }
  finally {
    working.value = false
  }
}

async function setLogging(enabled: boolean) {
  working.value = true
  try {
    overview.value = await invoke<KnowledgeOverview>('set_conversation_logging', { enabled })
    message.success(enabled ? '已开启对话记录' : '已关闭对话记录，已有记录不会删除')
  }
  catch (error) {
    message.error(String(error))
  }
  finally {
    working.value = false
  }
}

async function openDir() {
  try {
    await invoke('open_knowledge_dir')
  }
  catch (error) {
    message.error(String(error))
  }
}

async function copySyncPrompt() {
  if (!overview.value)
    return
  try {
    await navigator.clipboard.writeText(buildKnowledgeSyncPrompt(overview.value.root))
    message.success('已复制，粘贴给 Claude Code 或 Codex 即可')
  }
  catch {
    message.error('复制失败')
  }
}

onMounted(load)
</script>

<template>
  <n-space vertical size="medium">
    <div class="flex items-center justify-between">
      <div class="min-w-0">
        <div class="text-sm font-medium leading-relaxed">
          位置
        </div>
        <div class="text-xs opacity-60 truncate" :title="overview?.root">
          {{ overview?.root ?? '读取中…' }}
        </div>
      </div>
      <div class="flex items-center gap-2 flex-shrink-0 ml-3">
        <n-tag size="small" :type="stateLabel.type" :bordered="false">
          {{ stateLabel.text }}
        </n-tag>
        <n-button size="tiny" quaternary :disabled="overview?.state === 'missing'" @click="openDir">
          打开
        </n-button>
      </div>
    </div>

    <div class="text-xs opacity-70 leading-relaxed">
      在 Claude Code / Codex 里说 <code>hui</code> 恢复上次进度，<code>xi</code> 查以前踩过的坑，<code>ji</code> 把刚解决的问题记下来。
      知识库创建后归你所有，iterate 不会覆盖你的修改。
    </div>

    <div v-if="overview && overview.state !== 'missing'" class="flex items-center justify-between">
      <div class="min-w-0">
        <div class="text-sm font-medium leading-relaxed">
          记录弹窗对话
        </div>
        <div class="text-xs opacity-60">
          写入知识库的 conversations/，自动脱敏密钥；hui 靠它回溯。只存在本机。
        </div>
      </div>
      <n-switch
        :value="overview.conversation_logging"
        :loading="working"
        @update:value="setLogging"
      />
    </div>

    <div v-for="[client, skills] in skillsByClient" :key="client" class="flex items-center justify-between py-1">
      <div class="text-sm">
        {{ client }}
      </div>
      <div class="flex items-center gap-1">
        <n-tag
          v-for="skill in skills"
          :key="skill.name"
          size="small"
          :bordered="false"
          :type="skill.present ? 'success' : 'default'"
          :title="skill.path"
        >
          {{ skillTagText(skill) }}
        </n-tag>
      </div>
    </div>

    <div v-if="overview?.state === 'user'" class="text-xs opacity-60">
      这是你自建的知识库，iterate 不会往里面写文件。「挂载已有 Skill」只会在客户端的 skills 目录里建链接，指向你知识库里已有的 Skill。
    </div>
    <div v-if="linkableMissing" class="text-xs opacity-60">
      「未挂」不影响使用：如果你在 CLAUDE.md / AGENTS.md 里写了路由，照样能用。挂上后可以直接用 /hui 调用。
    </div>

    <div class="flex items-center justify-between gap-2">
      <n-button size="small" secondary :disabled="overview?.state === 'missing'" @click="copySyncPrompt">
        复制「在 GitHub 创建知识库」提示词
      </n-button>
      <n-button
        v-if="canInstall"
        type="primary"
        size="small"
        :loading="working"
        @click="install"
      >
        {{ installLabel }}
      </n-button>
    </div>
  </n-space>
</template>
