<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { computed, ref, watch } from 'vue'

interface ConversationNode {
  id: string
  parent_id: string | null
  timestamp: string
  node_type: 'user' | 'assistant'
  content: string
  is_markdown: boolean
  metadata?: {
    request_id?: string | null
    checkpoint_id?: string | null
    checkpoint_commit?: string | null
    selected_option?: string | null
    source?: string | null
  }
}

const props = defineProps<{
  treeId: string
  currentNodeId: string
  readOnly?: boolean
}>()

const emit = defineEmits<{
  nodeClick: [nodeId: string]
}>()

const nodes = ref<ConversationNode[]>([])
const loading = ref(false)
const selectedReadOnlyNodeId = ref<string | null>(null)
const selectedReadOnlyNode = computed(() => nodes.value.find(node => node.id === selectedReadOnlyNodeId.value))

const nodeCountText = computed(() => `共 ${nodes.value.length} 个节点`)

function reportTimelineDebugLog(message: string, payload?: any) {
  let suffix = ''
  if (payload !== undefined) {
    try {
      suffix = ` ${JSON.stringify(payload)}`
    }
    catch {
      suffix = ' [payload_unserializable]'
    }
  }
  void invoke('debug_log', { message: `[TimelineView] ${message}${suffix}` }).catch(() => {})
}

function formatTime(timestamp: string) {
  const date = new Date(timestamp)
  return date.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
  })
}

function buildPreview(node: ConversationNode) {
  const selectedOption = node.metadata?.selected_option?.trim()
  const normalized = (node.content.trim() || selectedOption || '').replace(/\s+/g, ' ').trim()
  if (!normalized)
    return '(空内容)'
  if (normalized.length <= 72)
    return normalized
  return `${normalized.slice(0, 72)}...`
}

function buildAnchorList(node: ConversationNode) {
  return [
    node.metadata?.checkpoint_id ? `cp ${node.metadata.checkpoint_id}` : null,
    node.metadata?.checkpoint_commit ? `git ${node.metadata.checkpoint_commit.slice(0, 7)}` : null,
    node.metadata?.request_id ? `req ${node.metadata.request_id}` : null,
    node.metadata?.source ? `src ${node.metadata.source}` : null,
  ].filter(Boolean) as string[]
}

async function loadPath() {
  if (!props.treeId || !props.currentNodeId) {
    nodes.value = []
    return
  }

  console.info('[Timeline] 开始加载对话路径', {
    treeId: props.treeId,
    currentNodeId: props.currentNodeId,
  })
  reportTimelineDebugLog('开始加载对话路径', {
    treeId: props.treeId,
    currentNodeId: props.currentNodeId,
  })
  loading.value = true
  try {
    const path = await invoke<ConversationNode[]>('get_conversation_path', {
      treeId: props.treeId,
      nodeId: props.currentNodeId,
    })
    nodes.value = path
    selectedReadOnlyNodeId.value = props.readOnly ? path.at(-1)?.id ?? null : null
    console.info('[Timeline] 对话路径加载成功', {
      treeId: props.treeId,
      currentNodeId: props.currentNodeId,
      nodeCount: path.length,
      nodeIds: path.map(item => item.id),
    })
    reportTimelineDebugLog('对话路径加载成功', {
      treeId: props.treeId,
      currentNodeId: props.currentNodeId,
      nodeCount: path.length,
    })
  }
  catch (error) {
    console.error('[Timeline] 加载对话路径失败:', error)
    reportTimelineDebugLog('加载对话路径失败')
    nodes.value = []
  }
  finally {
    loading.value = false
  }
}

function handleNodeClick(nodeId: string) {
  if (props.readOnly) {
    selectedReadOnlyNodeId.value = nodeId
    return
  }
  emit('nodeClick', nodeId)
}

watch(
  () => [props.treeId, props.currentNodeId],
  () => {
    loadPath()
  },
  { immediate: true },
)
</script>

<template>
  <div class="timeline-view">
    <div class="timeline-header">
      <div class="text-sm font-medium text-white">
        对话历史
      </div>
      <div class="text-xs text-gray-400">
        {{ nodeCountText }}
      </div>
    </div>

    <div v-if="loading" class="timeline-loading">
      <n-spin size="small" />
      <span class="text-xs text-gray-400">加载中...</span>
    </div>

    <div v-else-if="nodes.length === 0" class="timeline-empty">
      <n-empty description="暂无可回溯节点" />
    </div>

    <n-virtual-list
      v-else
      class="timeline-list"
      :items="nodes"
      :item-size="76"
      key-field="id"
    >
      <template #default="{ item, index }">
        <button
          type="button"
          class="timeline-node"
          :class="{ 'timeline-node-current': item.id === props.currentNodeId }"
          @click="handleNodeClick(item.id)"
        >
          <div class="timeline-marker">
            <div
              class="timeline-icon"
              :class="item.node_type === 'user' ? 'timeline-icon-user' : 'timeline-icon-assistant'"
            >
              <div :class="item.node_type === 'user' ? 'i-carbon-user' : 'i-carbon-chat-bot'" class="w-3.5 h-3.5" />
            </div>
            <div v-if="index !== nodes.length - 1" class="timeline-line" />
          </div>
          <div class="timeline-content">
            <div class="timeline-meta">
              <div class="timeline-meta-left">
                <span class="text-xs text-gray-300">{{ item.node_type === 'user' ? '用户' : '助手' }}</span>
                <span v-if="item.id === props.currentNodeId" class="timeline-current-badge">
                  当前锚点
                </span>
              </div>
              <span class="text-xs text-gray-500">{{ formatTime(item.timestamp) }}</span>
            </div>
            <div class="timeline-text">
              {{ buildPreview(item) }}
            </div>
            <div v-if="buildAnchorList(item).length > 0" class="timeline-anchors">
              <span
                v-for="anchor in buildAnchorList(item)"
                :key="`${item.id}-${anchor}`"
                class="timeline-anchor-chip"
                :title="anchor"
              >
                {{ anchor }}
              </span>
            </div>
          </div>
        </button>
      </template>
    </n-virtual-list>
    <div v-if="props.readOnly && selectedReadOnlyNode" class="max-h-48 overflow-y-auto whitespace-pre-wrap break-words border-t border-white/10 p-3 text-sm text-gray-100">
      {{ selectedReadOnlyNode.content || selectedReadOnlyNode.metadata?.selected_option || '(空内容)' }}
    </div>
  </div>
</template>

<style scoped>
.timeline-view {
  height: 100%;
  display: flex;
  flex-direction: column;
  background: #090909;
}

.timeline-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 14px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
}

.timeline-loading {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
}

.timeline-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

.timeline-list {
  flex: 1;
  padding: 10px 8px 12px;
}

.timeline-node {
  width: 100%;
  border: 0;
  text-align: left;
  color: inherit;
  background: transparent;
  display: flex;
  gap: 10px;
  padding: 8px 10px;
  border-radius: 8px;
  cursor: pointer;
  transition: background-color 0.15s ease;
}

.timeline-node:hover {
  background: rgba(255, 255, 255, 0.06);
}

.timeline-node-current {
  background: rgba(59, 130, 246, 0.2);
  outline: 1px solid rgba(59, 130, 246, 0.45);
}

.timeline-marker {
  width: 22px;
  position: relative;
  display: flex;
  justify-content: center;
}

.timeline-icon {
  width: 22px;
  height: 22px;
  border-radius: 999px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #fff;
  z-index: 1;
}

.timeline-icon-user {
  background: #2563eb;
}

.timeline-icon-assistant {
  background: #059669;
}

.timeline-line {
  position: absolute;
  top: 24px;
  bottom: -26px;
  width: 1px;
  background: rgba(255, 255, 255, 0.14);
}

.timeline-content {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.timeline-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.timeline-meta-left {
  display: flex;
  align-items: center;
  gap: 6px;
}

.timeline-text {
  font-size: 12px;
  line-height: 1.4;
  color: rgba(255, 255, 255, 0.88);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.timeline-current-badge {
  padding: 1px 8px;
  border-radius: 999px;
  font-size: 10px;
  line-height: 1.5;
  color: #bfdbfe;
  background: rgba(59, 130, 246, 0.22);
}

.timeline-anchors {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 6px;
}

.timeline-anchor-chip {
  max-width: 220px;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 11px;
  line-height: 1.4;
  color: rgba(255, 255, 255, 0.72);
  background: rgba(255, 255, 255, 0.08);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
