<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { computed, ref, watch } from 'vue'
import { selectionTextInsideElement } from '../../utils/popupSelectionQuote'

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

const props = withDefaults(defineProps<{
  treeId: string | null
  currentNodeId: string | null
  mockNodes?: ConversationNode[]
  compactHover?: boolean
  compactExpanded?: boolean
}>(), {
  compactHover: false,
  compactExpanded: false,
})

const emit = defineEmits<{
  nodeClick: [node: ConversationNode | null]
  nodeQuote: [content: string]
}>()

const nodes = ref<ConversationNode[]>([])
const hoveredIndex = ref<number | null>(null)
const pinnedNodeId = ref<string | null>(null)
const tooltipStyle = ref<Record<string, string>>({})
const rootRef = ref<HTMLElement | null>(null)
const tooltipRef = ref<HTMLElement | null>(null)
const tooltipContentRef = ref<HTMLElement | null>(null)

const nodeCount = computed(() => nodes.value.length)

function buildPreview(node: ConversationNode) {
  const selectedOption = node.metadata?.selected_option?.trim()
  const normalized = (node.content.trim() || selectedOption || '').replace(/\s+/g, ' ').trim()
  if (!normalized)
    return '(空内容)'
  if (normalized.length <= 40)
    return normalized
  return `${normalized.slice(0, 40)}...`
}

function formatTime(timestamp: string) {
  const date = new Date(timestamp)
  return date.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
  })
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
  if (props.mockNodes) {
    nodes.value = props.mockNodes
    return
  }
  if (!props.treeId || !props.currentNodeId) {
    nodes.value = []
    return
  }
  try {
    const path = await invoke<ConversationNode[]>('get_conversation_path', {
      treeId: props.treeId,
      nodeId: props.currentNodeId,
    })
    nodes.value = path
  }
  catch (error) {
    console.error('[TimelineDotBar] 加载路径失败:', error)
    nodes.value = []
  }
}

function updateTooltipPosition(dot: HTMLElement) {
  const rect = dot.getBoundingClientRect()
  tooltipStyle.value = {
    top: `${rect.top + rect.height / 2}px`,
    right: `${window.innerWidth - rect.left + 8}px`,
  }
}

function handleDotHover(index: number, event: MouseEvent) {
  hoveredIndex.value = index
  if (pinnedNodeId.value)
    return
  updateTooltipPosition(event.currentTarget as HTMLElement)
}

function handleDotLeave() {
  hoveredIndex.value = null
}

const pinnedIndex = computed(() => {
  if (!pinnedNodeId.value)
    return null
  const idx = nodes.value.findIndex(n => n.id === pinnedNodeId.value)
  return idx >= 0 ? idx : null
})

const activeTooltipIndex = computed(() => hoveredIndex.value)
const isPinnedTooltip = computed(() => false)

function buildTooltipContent(node: ConversationNode) {
  const selectedOption = node.metadata?.selected_option?.trim()
  const content = node.content.trim() ? node.content : selectedOption || ''
  return isPinnedTooltip.value ? content : buildPreview(node)
}

function buildQuoteContent(node: ConversationNode) {
  const selectedText = selectionTextInsideElement(tooltipContentRef.value)
  return (selectedText || node.content.trim() || node.metadata?.selected_option?.trim() || '').trim()
}

function clearTextSelection() {
  if (typeof window === 'undefined')
    return
  window.getSelection()?.removeAllRanges()
}

function closePinnedTooltip() {
  pinnedNodeId.value = null
  emit('nodeClick', null)
  clearTextSelection()
}

function handleQuoteClick(node: ConversationNode) {
  const content = buildQuoteContent(node)
  if (!content)
    return
  emit('nodeQuote', content)
  closePinnedTooltip()
}

function handleDotClick(index: number, nodeId: string) {
  if (pinnedNodeId.value === nodeId) {
    closePinnedTooltip()
  }
  else {
    pinnedNodeId.value = nodeId
    hoveredIndex.value = null
    emit('nodeClick', nodes.value[index] ?? null)
  }
}

watch(
  () => [props.treeId, props.currentNodeId, props.mockNodes],
  () => {
    if (pinnedNodeId.value && !props.treeId && !props.mockNodes)
      closePinnedTooltip()
    loadPath()
  },
  { immediate: true },
)
</script>

<template>
  <div
    v-if="nodeCount > 0"
    ref="rootRef"
    class="timeline-dot-bar"
    :class="{
      'timeline-dot-bar-compact-hover': props.compactHover,
      'timeline-dot-bar-compact-expanded': props.compactExpanded,
    }"
    data-guide="timeline-dot-bar"
  >
    <!-- 小球轨道 -->
    <div class="dot-track">
      <button
        v-for="(node, index) in nodes"
        :key="node.id"
        type="button"
        class="dot-item"
        :class="{
          'dot-user': node.node_type === 'user',
          'dot-assistant': node.node_type === 'assistant',
          'dot-current': node.id === props.currentNodeId,
          'dot-hovered': hoveredIndex === index || pinnedIndex === index,
        }"
        @click="handleDotClick(index, node.id)"
        @mouseenter="handleDotHover(index, $event)"
        @mouseleave="handleDotLeave"
      >
        <div class="dot-circle" />
      </button>
    </div>

    <!-- 悬浮提示 -->
    <Transition name="tooltip-fade">
      <div
        v-if="activeTooltipIndex !== null && nodes[activeTooltipIndex]"
        ref="tooltipRef"
        class="dot-tooltip"
        :class="{ 'dot-tooltip-pinned': isPinnedTooltip }"
        :style="tooltipStyle"
      >
        <div class="tooltip-header">
          <div class="tooltip-header-left">
            <span class="tooltip-type" :class="nodes[activeTooltipIndex].node_type === 'user' ? 'text-gray-300' : 'text-gray-500'">
              {{ nodes[activeTooltipIndex].node_type === 'user' ? '用户' : '助手' }}
            </span>
            <span
              v-if="nodes[activeTooltipIndex].id === props.currentNodeId"
              class="tooltip-current-badge"
            >
              当前锚点
            </span>
          </div>
          <div class="tooltip-header-right">
            <button
              v-if="isPinnedTooltip"
              type="button"
              class="tooltip-quote-button"
              title="引用选中文字；未选中时引用这条历史内容"
              @pointerdown.prevent
              @click.stop="handleQuoteClick(nodes[activeTooltipIndex])"
            >
              引用
            </button>
            <span class="tooltip-time">{{ formatTime(nodes[activeTooltipIndex].timestamp) }}</span>
          </div>
        </div>
        <div ref="tooltipContentRef" class="tooltip-content">
          {{ buildTooltipContent(nodes[activeTooltipIndex]) }}
        </div>
        <div
          v-if="buildAnchorList(nodes[activeTooltipIndex]).length > 0"
          class="tooltip-anchors"
        >
          <span
            v-for="anchor in buildAnchorList(nodes[activeTooltipIndex])"
            :key="`${nodes[activeTooltipIndex].id}-${anchor}`"
            class="tooltip-anchor-chip"
          >
            {{ anchor }}
          </span>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.timeline-dot-bar {
  position: relative;
  width: 32px;
  height: 100%;
  max-height: none;
  flex-shrink: 0;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 12px 0;
  background: #ffffff;
  overflow-y: auto;
  overflow-x: hidden;
  scrollbar-width: none;
  border: 1px solid rgba(0, 0, 0, 0.12);
  border-radius: 8px;
  margin: 4px;
}

.timeline-dot-bar::-webkit-scrollbar {
  display: none;
}

.timeline-dot-bar-compact-hover {
  width: 0;
  min-width: 0;
  padding: 0;
  border-width: 0;
  border-color: transparent;
  background: transparent;
  box-shadow: none;
  cursor: pointer;
  margin: 4px 0;
  transition: width 0.16s ease, min-width 0.16s ease, padding 0.16s ease, border-color 0.16s ease, background 0.16s ease, box-shadow 0.16s ease;
}

.timeline-dot-bar-compact-hover .dot-track {
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.12s ease;
}

.timeline-dot-bar-compact-hover:hover,
.timeline-dot-bar-compact-hover:focus-within,
.timeline-dot-bar-compact-expanded {
  width: 34px;
  min-width: 34px;
  padding: 12px 0;
  border-width: 0;
  background: transparent;
  border-color: transparent;
  box-shadow: none;
}

.timeline-dot-bar-compact-hover:hover .dot-track,
.timeline-dot-bar-compact-hover:focus-within .dot-track,
.timeline-dot-bar-compact-expanded .dot-track {
  opacity: 1;
  pointer-events: auto;
}

.dot-track {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  flex: 1;
}

/* 连续竖线贯穿所有小球 */
.dot-track::before {
  content: '';
  position: absolute;
  top: 12px;
  bottom: 12px;
  left: 50%;
  transform: translateX(-50%);
  width: 1.5px;
  background: #d1d5db;
  z-index: 0;
}

.dot-item {
  position: relative;
  width: 32px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  cursor: pointer;
  padding: 0;
  transition: transform 0.15s ease;
}

.dot-item:hover {
  transform: scale(1.3);
}

.dot-circle {
  position: relative;
  z-index: 1;
  width: 12px;
  height: 12px;
  border-radius: 50%;
  transition: all 0.15s ease;
}

.dot-user .dot-circle {
  background: #9ca3af;
}

.dot-assistant .dot-circle {
  background: #374151;
}

.dot-current .dot-circle {
  width: 14px;
  height: 14px;
  box-shadow: 0 0 0 2px rgba(0, 0, 0, 0.2);
}

.dot-hovered .dot-circle {
  width: 14px;
  height: 14px;
}

/* 单独的短连接线已被 dot-track::before 的连续线替代 */

/* 悬浮提示 */
.dot-tooltip {
  position: fixed;
  z-index: 9999;
  transform: translateY(-50%);
  background: #1a1a1a;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 8px;
  padding: 8px 12px;
  min-width: 160px;
  max-width: 240px;
  pointer-events: none;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  white-space: normal;
  overflow: hidden;
}

.dot-tooltip-pinned {
  width: min(520px, calc(100vw - 48px));
  max-width: min(520px, calc(100vw - 48px));
  max-height: min(60vh, 520px);
  pointer-events: auto;
  overflow-y: auto;
  overscroll-behavior: contain;
  user-select: text;
  -webkit-user-select: text;
}

.tooltip-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}

.tooltip-header-left {
  display: flex;
  align-items: center;
  gap: 6px;
}

.tooltip-header-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.tooltip-quote-button {
  height: 22px;
  padding: 0 9px;
  border: 1px solid rgba(255, 255, 255, 0.16);
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.08);
  color: rgba(255, 255, 255, 0.84);
  cursor: pointer;
  font-size: 11px;
  font-weight: 600;
  line-height: 20px;
}

.tooltip-quote-button:hover {
  background: rgba(255, 255, 255, 0.14);
  color: #fff;
}

.tooltip-type {
  font-size: 11px;
  font-weight: 600;
}

.tooltip-time {
  font-size: 10px;
  color: rgba(255, 255, 255, 0.4);
}

.tooltip-current-badge {
  padding: 1px 8px;
  border-radius: 999px;
  font-size: 10px;
  line-height: 1.5;
  color: #bfdbfe;
  background: rgba(59, 130, 246, 0.22);
}

.tooltip-content {
  font-size: 11px;
  line-height: 1.4;
  color: rgba(255, 255, 255, 0.75);
  white-space: pre-wrap;
  word-break: break-word;
  cursor: text;
  user-select: text;
  -webkit-user-select: text;
}

.tooltip-content::selection {
  color: #0f172a;
  background: rgba(147, 197, 253, 0.82);
}

.tooltip-content::-moz-selection {
  color: #0f172a;
  background: rgba(147, 197, 253, 0.82);
}

.tooltip-anchors {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: 8px;
}

.tooltip-anchor-chip {
  max-width: 220px;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 10px;
  line-height: 1.4;
  color: rgba(255, 255, 255, 0.74);
  background: rgba(255, 255, 255, 0.08);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 动画 */
.tooltip-fade-enter-active,
.tooltip-fade-leave-active {
  transition: opacity 0.12s ease;
}

.tooltip-fade-enter-from,
.tooltip-fade-leave-to {
  opacity: 0;
}
</style>
