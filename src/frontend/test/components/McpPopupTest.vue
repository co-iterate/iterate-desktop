<script setup lang="ts">
import type { McpRequest, PopupArtifact } from '../../types/popup'
import { invoke } from '@tauri-apps/api/core'
import { computed, ref, watch } from 'vue'
import HtmlArtifactRenderer from '../../components/popup/HtmlArtifactRenderer.vue'
import McpPopup from '../../components/popup/McpPopup.vue'
import PopupHeader from '../../components/popup/PopupHeader.vue'
import { applyThemeVariables } from '../../theme'

// Props
const props = defineProps<{
  showControls?: boolean
}>()

// 默认显示控制面板
const showControls = ref(props.showControls !== false)

const currentTheme = ref('dark')
const showPopup = ref(true)
const isMuted = ref(false)
const shortcutEnabled = ref(true)

const mockAppConfig = computed(() => ({
  theme: currentTheme.value,
  window: {
    alwaysOnTop: false,
    width: 900,
    height: 640,
    fixed: false,
  },
  audio: {
    enabled: true,
    url: '',
  },
  reply: {
    enabled: true,
    prompt: '请按照最佳实践继续',
    loopPrompt: '进入循环模式',
  },
}))

const htmlArtifactDemo = `
<main class="artifact-shell">
  <section class="hero">
    <div>
      <p class="eyebrow">iterate artifact lab</p>
      <h1>HTML 输出物渲染验证</h1>
      <p class="lede">同一份 agent 输出可以同时承载规格、代码片段、流程图和轻量交互，用来判断弹窗是否适合作为 HTML Artifact 的阅读面板。</p>
    </div>
    <svg class="hero-map" viewBox="0 0 360 180" role="img" aria-label="Artifact flow">
      <defs>
        <linearGradient id="flowLine" x1="0" x2="1" y1="0" y2="1">
          <stop offset="0%" stop-color="#35c481" />
          <stop offset="54%" stop-color="#5ba7f7" />
          <stop offset="100%" stop-color="#f97316" />
        </linearGradient>
      </defs>
      <rect x="12" y="22" width="92" height="48" rx="8" class="node" />
      <rect x="134" y="92" width="92" height="48" rx="8" class="node node-hot" />
      <rect x="256" y="42" width="92" height="48" rx="8" class="node" />
      <path d="M104 46 C138 46 118 116 134 116" class="flow" />
      <path d="M226 116 C258 116 238 66 256 66" class="flow" />
      <text x="58" y="52" text-anchor="middle">Agent</text>
      <text x="180" y="122" text-anchor="middle">HTML</text>
      <text x="302" y="72" text-anchor="middle">Review</text>
    </svg>
  </section>

  <section class="metric-grid">
    <article>
      <span class="metric">3x</span>
      <p>同屏信息密度，高于纯 Markdown 长文。</p>
    </article>
    <article>
      <span class="metric">1</span>
      <p>单文件即可分享、归档、回放和审阅。</p>
    </article>
    <article>
      <span class="metric">0</span>
      <p>原型阶段不要求改 MCP 后端协议。</p>
    </article>
  </section>

  <section class="split">
    <div>
      <h2>适合先测试的内容</h2>
      <table>
        <thead>
          <tr>
            <th>类型</th>
            <th>验证点</th>
            <th>状态</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>规格说明</td>
            <td>层级、目录、重点块</td>
            <td><span class="pill green">ready</span></td>
          </tr>
          <tr>
            <td>代码审查</td>
            <td>diff、批注、严重等级</td>
            <td><span class="pill blue">next</span></td>
          </tr>
          <tr>
            <td>交互调参</td>
            <td>滑块、开关、导出结果</td>
            <td><span class="pill orange">probe</span></td>
          </tr>
        </tbody>
      </table>
    </div>

    <div class="code-card">
      <div class="code-card__bar">
        <span></span><span></span><span></span>
      </div>
      <pre><code>interface McpRequest {
  message: string
  is_markdown?: boolean
  render_mode?: 'html_artifact'
}</code></pre>
    </div>
  </section>

  <section class="playground">
    <div>
      <h2>交互原型</h2>
      <p>拖动密度滑块，验证 iframe 内部 JS、布局重排和父级自动高度同步。</p>
    </div>
    <label>
      信息密度
      <input id="density" type="range" min="1" max="4" value="2" />
    </label>
    <div id="density-output" class="density-output">当前：平衡阅读</div>
  </section>
</main>

<style>
  .artifact-shell {
    display: grid;
    gap: 16px;
  }

  .hero {
    display: grid;
    grid-template-columns: minmax(0, 1.1fr) minmax(220px, 0.9fr);
    gap: 18px;
    align-items: center;
    padding: 18px;
    border: 1px solid var(--artifact-border);
    border-radius: 8px;
    background:
      linear-gradient(135deg, rgba(53, 196, 129, 0.12), transparent 34%),
      linear-gradient(315deg, rgba(249, 115, 22, 0.14), transparent 40%),
      var(--artifact-panel);
  }

  .eyebrow {
    margin-bottom: 8px;
    color: var(--artifact-green);
    font-size: 11px;
    font-weight: 800;
    letter-spacing: 0;
    text-transform: uppercase;
  }

  .lede {
    max-width: 46em;
    margin-bottom: 0;
  }

  .hero-map {
    width: 100%;
    min-height: 160px;
  }

  .node {
    fill: rgba(91, 167, 247, 0.12);
    stroke: var(--artifact-blue);
    stroke-width: 2;
  }

  .node-hot {
    fill: rgba(53, 196, 129, 0.16);
    stroke: var(--artifact-green);
  }

  .flow {
    fill: none;
    stroke: url(#flowLine);
    stroke-width: 4;
    stroke-linecap: round;
  }

  .hero-map text {
    fill: var(--artifact-fg);
    font: 700 13px ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  .metric-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 10px;
  }

  .metric-grid article,
  .code-card,
  .playground {
    padding: 14px;
    border: 1px solid var(--artifact-border);
    border-radius: 8px;
    background: var(--artifact-panel);
  }

  .metric {
    display: block;
    margin-bottom: 5px;
    color: var(--artifact-orange);
    font-size: 28px;
    font-weight: 850;
    line-height: 1;
  }

  .metric-grid p {
    margin: 0;
  }

  .split {
    display: grid;
    grid-template-columns: minmax(0, 1.15fr) minmax(210px, 0.85fr);
    gap: 14px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    overflow: hidden;
    border: 1px solid var(--artifact-border);
    border-radius: 8px;
    background: var(--artifact-panel);
  }

  th,
  td {
    padding: 10px;
    border-bottom: 1px solid var(--artifact-border);
    text-align: left;
  }

  th {
    color: var(--artifact-muted);
    font-size: 12px;
  }

  tr:last-child td {
    border-bottom: 0;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    min-height: 22px;
    padding: 0 8px;
    border-radius: 999px;
    color: #101114;
    font-size: 11px;
    font-weight: 800;
  }

  .green {
    background: var(--artifact-green);
  }

  .blue {
    background: var(--artifact-blue);
  }

  .orange {
    background: var(--artifact-orange);
  }

  .code-card {
    align-self: start;
    padding: 0;
    overflow: hidden;
  }

  .code-card__bar {
    display: flex;
    gap: 6px;
    padding: 10px;
    border-bottom: 1px solid var(--artifact-border);
  }

  .code-card__bar span {
    width: 9px;
    height: 9px;
    border-radius: 999px;
    background: var(--artifact-muted);
  }

  pre {
    margin: 0;
    padding: 14px;
    overflow-x: auto;
  }

  .playground {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(180px, 0.7fr);
    gap: 14px;
    align-items: center;
  }

  .playground h2,
  .playground p {
    margin-bottom: 4px;
  }

  label {
    display: grid;
    gap: 8px;
    color: var(--artifact-muted);
    font-weight: 700;
  }

  input[type="range"] {
    width: 100%;
    accent-color: var(--artifact-green);
  }

  .density-output {
    grid-column: 1 / -1;
    padding: 10px;
    border: 1px dashed var(--artifact-border);
    border-radius: 8px;
    color: var(--artifact-fg);
  }

  @media (max-width: 560px) {
    .hero,
    .split,
    .playground {
      grid-template-columns: 1fr;
    }

    .metric-grid {
      grid-template-columns: 1fr;
    }
  }
</style>

<script>
  (() => {
    const density = document.getElementById('density');
    const output = document.getElementById('density-output');
    const labels = ['宽松阅读', '平衡阅读', '密集审查', '会议速览'];

    density.addEventListener('input', () => {
      const value = Number(density.value);
      output.textContent = '当前：' + labels[value - 1];
      document.querySelector('.artifact-shell').style.gap = (22 - value * 3) + 'px';
      document.querySelectorAll('.metric-grid article, .playground, .code-card').forEach((el) => {
        el.style.padding = Math.max(8, 18 - value * 2) + 'px';
      });
    });
  })();
<\/script>
`

// 模拟不同类型的 MCP 请求
const requestTemplates: Array<{ name: string, request: McpRequest }> = [
  {
    name: '基础文本请求',
    request: {
      id: 'test-basic',
      message: '这是一个基础的模拟请求，用于测试弹窗功能。请确认是否继续执行操作。',
      is_markdown: false,
    },
  },
  {
    name: '预定义选项请求',
    request: {
      id: 'test-options',
      message: '请选择您需要的操作类型：',
      predefined_options: ['创建新文件', '修改现有文件', '删除文件', '查看文件内容'],
      is_markdown: false,
    },
  },
  {
    name: 'Markdown + 代码块',
    request: {
      id: 'test-markdown-code',
      message: `# 代码审查请求

我需要对以下代码进行审查和优化：

## 当前代码

\`\`\`typescript
interface User {
  id: string
  name: string
  email: string
}

function createUser(data: Partial<User>): User {
  return {
    id: Math.random().toString(36),
    name: data.name || 'Unknown',
    email: data.email || 'unknown@example.com'
  }
}
\`\`\`

## 发现的问题

1. **ID生成不安全** - 使用 \`Math.random()\` 可能产生重复ID
2. **类型安全性** - 缺少必要的验证
3. **错误处理** - 没有处理无效输入

## 建议的改进

\`\`\`typescript

interface User {
  id: string
  name: string
  email: string
}

interface CreateUserData {
  name: string
  email: string
}

function createUser(data: CreateUserData): User {
  if (!data.name || !data.email) {
    throw new Error('Name and email are required')
  }

  if (!isValidEmail(data.email)) {
    throw new Error('Invalid email format')
  }

  return {
    id: uuidv4(),
    name: data.name.trim(),
    email: data.email.toLowerCase().trim()
  }
}

function isValidEmail(email: string): boolean {
  const emailRegex = /^[^\\s@]+@[^\\s@]+\\.[^\\s@]+$/
  return emailRegex.test(email)
}
\`\`\`

请选择您希望的操作：`,
      predefined_options: ['应用建议的改进', '需要进一步讨论', '查看更多示例', '拒绝修改'],
      is_markdown: true,
    },
  },
  {
    name: '自定义请求',
    request: {
      id: 'test-custom',
      message: `# 🎨 新弹窗系统测试

欢迎使用重构后的弹窗系统！

## ✨ 新特性
- 🧩 **模块化组件**：头部、内容、输入、操作栏独立组件
- 🎭 **过渡动画**：流畅的切换效果和骨架屏
- 🏠 **主界面切换**：点击头部按钮可切换到主界面
- 🎯 **状态管理**：完整的应用状态管理系统
- 🧪 **模拟数据**：支持完全脱离MCP服务运行

## 🔧 测试功能
请尝试以下操作：
1. 切换主题
2. 选择预定义选项
3. 输入文本内容
4. 拖拽或粘贴图片
5. 点击主界面按钮

\`\`\`typescript
// 新的弹窗系统架构
interface PopupSystem {
  manager: PopupManager
  components: ModularComponents
  transitions: SmoothAnimations
  state: ReactiveState
}
\`\`\`

请选择您要测试的功能：`,
      predefined_options: [
        '🎨 测试主题切换',
        '🏠 切换到主界面',
        '📝 测试文本输入',
        '🖼️ 测试图片上传',
        '⚡ 测试快捷键',
        '🔄 测试状态管理',
      ],
      is_markdown: true,
    },
  },
  {
    name: 'Markdown 可点击链接日报',
    request: {
      id: 'test-markdown-links',
      message: `# Daily Digest 链接点击测试

## Discussion Brief

### AI agents and coding

1. [OpenAI 官方站点](https://openai.com/)

**Source**: OpenAI  
**Published**: 2026-06-02  
**What**: 这条用来验证 Markdown 标题链接是否可以直接点击打开。  
**Why it matters**: 日报讨论版摘要需要把来源链接放进标题里，读者不应该再手动复制 URL。  
**Discussion point**: 如果链接能直接打开，后续 digest 可以把每条内容都做成标题链接。

2. [YouTube channel link](https://www.youtube.com/@OpenAI)

**Source**: YouTube  
**Published**: 2026-06-02  
**What**: 这条用来验证 YouTube 链接在 iterate 弹窗里是否可点击。  
**Why it matters**: ContentDash 的生产日报会混合 YouTube 和 X 来源。  
**Discussion point**: 摘要里可以保留频道、视频或 tweet 的原始落点。

3. 自动链接测试：https://example.com/iterate-link-test

**Source**: linkify  
**Published**: 2026-06-02  
**What**: 这条不用 Markdown 方括号，专门测试 \`markdown-it\` 的自动链接。  
**Why it matters**: 有些 agent 输出会直接粘贴裸 URL。  
**Discussion point**: 裸 URL 也应该能被安全打开。`,
      predefined_options: ['链接可以直接点', '链接样式再明显点', '继续验证真实弹窗'],
      is_markdown: true,
    },
  },
  {
    name: 'Markdown + HTML 入口',
    request: {
      id: 'test-html-artifact',
      message: `# HTML Artifact 人机交互测试

这条消息保留 Markdown 作为快速决策摘要，完整 HTML 放在下方 artifact 入口里打开。

## 为什么这样做

- 弹窗负责快速判断和回复，不承载长报告。
- HTML 负责深度阅读、视觉化和交互探索。
- 用户可以先看摘要，再决定是否打开完整页面。

## 这次要验证

1. Markdown 摘要是否仍然清晰。
2. HTML 入口是否足够轻，不打断主流程。
3. 打开后的 Viewer 是否比内嵌 iframe 更像给人看的页面。`,
      predefined_options: ['这个方向可以', '入口再轻一点', 'Viewer 再优化', '先不接入正式协议'],
      is_markdown: true,
      artifacts: [
        {
          type: 'html',
          title: 'HTML 输出物渲染验证.html',
          description: '可视化规格、流程图、表格和交互滑块',
          content: htmlArtifactDemo,
        },
      ],
    },
  },
  {
    name: 'Markdown + HTML Artifact 块',
    request: {
      id: 'test-html-artifact-block',
      message: `# zhi HTML Artifact 块测试

这条模板模拟 \`call_zhi.message\` 里的真实 artifact 块。

主弹窗应只显示这段摘要和一个 HTML 卡片；点击卡片后，Viewer 应渲染完整 HTML，并且 iframe 内的滑块交互可用。

::::artifact{type="html" title="zhi-html-artifact-block-smoke.html" description="模拟 zhi message 内嵌 artifact 块的正式解析路径"}
${htmlArtifactDemo}
::::`,
      predefined_options: ['主弹窗正常', '源码泄漏', '卡片打不开', 'Viewer 交互异常'],
      is_markdown: true,
    },
  },
  {
    name: 'Markdown 本地文件链接',
    request: {
      id: 'test-local-file-links',
      project_path: '/Users/test/project',
      message: `# 本地文件链接点击测试

请点击下面几个项目内文件链接，验证弹窗会调用本地打开命令：

1. [README.md](README.md)
2. [后端命令实现](src/rust/ui/commands.rs:1610)
3. [package.json](file:///Users/test/project/package.json)

外部链接仍应走系统浏览器：[OpenAI](https://openai.com/)`,
      predefined_options: ['本地链接正常', '项目外路径被拦截', '继续真实弹窗验证'],
      is_markdown: true,
    },
  },
]

const initialTemplateId = new URLSearchParams(window.location.search).get('template')
const initialTemplateIndex = requestTemplates.findIndex((template) => {
  return template.request.id === initialTemplateId || template.name === initialTemplateId
})
const defaultTemplateIndex = initialTemplateIndex >= 0 ? initialTemplateIndex : 6

const currentTemplate = ref(defaultTemplateIndex) // 默认显示真实 zhi HTML Artifact 块模板
const currentRequest = ref(requestTemplates[defaultTemplateIndex].request)
const activeArtifact = ref<PopupArtifact | null>(null)
const previewArtifact = computed(() => activeArtifact.value ?? currentRequest.value.artifacts?.[0] ?? null)
const previewArtifactContent = computed(() => previewArtifact.value?.content || '')

watch(
  currentTheme,
  theme => applyThemeVariables(theme),
  { immediate: true },
)

const mockTimelineNodes = computed(() => [
  {
    id: 'test-node-1',
    parent_id: null,
    timestamp: new Date(Date.now() - 8 * 60 * 1000).toISOString(),
    node_type: 'user' as const,
    content: currentRequest.value.message,
    is_markdown: currentRequest.value.is_markdown ?? false,
    metadata: {
      request_id: currentRequest.value.id,
      checkpoint_id: null,
    },
  },
  {
    id: 'test-node-2',
    parent_id: 'test-node-1',
    timestamp: new Date(Date.now() - 4 * 60 * 1000).toISOString(),
    node_type: 'assistant' as const,
    content: '可以按它的方向做，但不能原样一口气照搬。我对照了真实仓库，结论是需要保留原型 app 的真实时间线链路。',
    is_markdown: false,
    metadata: {
      request_id: 'req serve-1776687029989',
      checkpoint_id: null,
    },
  },
  {
    id: 'test-node-3',
    parent_id: 'test-node-2',
    timestamp: new Date().toISOString(),
    node_type: 'user' as const,
    content: '但是我们的节点在我们的原型上还在吧',
    is_markdown: false,
    metadata: {
      request_id: 'req-preview-current',
      checkpoint_id: null,
    },
  },
])

const usageProviders = [
  {
    id: 'codex',
    name: 'Codex',
    color: '#2563eb',
    iconUrl: './icons/ai-providers/codex.svg',
    summary: 'Pro OAuth',
    updatedAt: '刚刚',
    metrics: [
      { label: 'Session', remaining: 69, resetLabel: '12:27 重置' },
      { label: 'Weekly', remaining: 61, resetLabel: '周三 06:34' },
    ],
  },
  {
    id: 'cursor',
    name: 'Cursor',
    color: '#7c3aed',
    iconUrl: './icons/ai-providers/cursor.svg',
    summary: 'Usage dashboard',
    updatedAt: '1m',
    metrics: [
      { label: 'Fast', remaining: 82, resetLabel: '明天 09:00' },
      { label: 'Slow', remaining: 77, resetLabel: '按月刷新' },
    ],
  },
  {
    id: 'claude',
    name: 'Claude',
    color: '#d97706',
    iconUrl: './icons/ai-providers/claude.svg',
    summary: 'Team workspace',
    updatedAt: '3m',
    metrics: [
      { label: 'Opus', remaining: 41, resetLabel: '2h 后刷新' },
      { label: 'Sonnet', remaining: 90, resetLabel: '5h 后刷新' },
    ],
  },
  {
    id: 'gemini',
    name: 'Gemini',
    color: '#0f766e',
    iconUrl: './icons/ai-providers/gemini.svg',
    summary: 'AI Studio',
    updatedAt: '8m',
    metrics: [
      { label: 'Daily', remaining: 73, resetLabel: '今天 23:59' },
      { label: 'Tokens', remaining: 58, resetLabel: '滚动窗口' },
    ],
  },
]

function switchTemplate(index: number) {
  currentTemplate.value = index
  currentRequest.value = requestTemplates[index].request
  activeArtifact.value = null
}

function handleResponse(response: any) {
  console.log('MCP 响应:', response)
  ;(window as any).__lastMcpPopupResponse = response
  try {
    window.localStorage.setItem('__lastMcpPopupResponse', JSON.stringify(response))
  }
  catch (error) {
    console.warn('记录 MCP 测试响应失败:', error)
  }
  document.documentElement.setAttribute(
    'data-last-popup-response-source',
    response?.metadata?.source || '',
  )
  document.documentElement.setAttribute(
    'data-last-popup-run-id',
    response?.metadata?.run_id || '',
  )
  document.documentElement.setAttribute(
    'data-last-popup-generation',
    String(response?.metadata?.generation ?? ''),
  )
}

function handleCancel() {
  console.log('MCP 取消')
}

function handleCloseArtifact() {
  activeArtifact.value = null
}

async function handleCopyArtifact() {
  if (!previewArtifactContent.value)
    return

  await navigator.clipboard.writeText(previewArtifactContent.value)
}

async function openArtifactInBrowser(artifact: PopupArtifact | null) {
  const content = artifact?.content || ''
  if (!content)
    return false

  try {
    const filePath = await invoke<string>('open_html_artifact_in_browser', {
      content,
      title: artifact?.title || 'html-artifact.html',
    })
    console.log('HTML Artifact 已在浏览器打开:', filePath)
    return true
  }
  catch (error) {
    console.error('在浏览器打开 HTML Artifact 失败:', error)
    return false
  }
}

async function handleOpenArtifact(artifact: PopupArtifact) {
  const opened = await openArtifactInBrowser(artifact)
  if (!opened)
    activeArtifact.value = artifact
}

async function handleOpenArtifactInBrowser() {
  await openArtifactInBrowser(previewArtifact.value)
}

function handleThemeChange(theme: string) {
  currentTheme.value = theme
  console.log('主题切换:', theme)
}

function handleOpenMainLayout() {
  console.log('打开主界面')
}

function togglePopup() {
  showPopup.value = !showPopup.value
}

function handleToggleAlwaysOnTop() {
  console.log('切换置顶')
}

function handleToggleMute() {
  isMuted.value = !isMuted.value
}

function handleNewChat() {
  console.log('新聊天')
}

function handleToggleTerminal() {
  console.log('切换终端')
}

function handleToggleShortcut(enabled: boolean) {
  shortcutEnabled.value = enabled
  console.log('切换快捷键:', enabled)
}

function handleMinimizeWindow() {
  console.log('最小化窗口')
}
</script>

<template>
  <div class="mcp-popup-test">
    <!-- 控制面板模式 -->
    <div v-if="showControls">
      <n-card title="MCP 弹窗测试 - 新弹窗系统">
        <template #header-extra>
          <n-space>
            <n-tag size="small" type="info">
              测试模式
            </n-tag>
            <n-button size="small" @click="togglePopup">
              {{ showPopup ? '隐藏弹窗' : '显示弹窗' }}
            </n-button>
          </n-space>
        </template>

        <!-- 控制面板 -->
        <div class="control-panel">
          <n-card title="测试控制" size="small">
            <n-space vertical>
              <div class="control-section">
                <h4>请求模板:</h4>
                <n-space>
                  <n-button
                    v-for="(template, index) in requestTemplates" :key="index"
                    :type="currentTemplate === index ? 'primary' : 'default'" size="small"
                    @click="switchTemplate(index)"
                  >
                    {{ template.name }}
                  </n-button>
                </n-space>
              </div>

              <div class="control-section">
                <h4>当前状态:</h4>
                <n-space vertical size="small">
                  <n-space align="center" justify="space-between">
                    <span>主题:</span>
                    <n-tag size="small" :type="currentTheme === 'dark' ? 'warning' : 'info'">
                      {{ currentTheme }}
                    </n-tag>
                  </n-space>

                  <n-space align="center" justify="space-between">
                    <span>弹窗:</span>
                    <n-tag size="small" :type="showPopup ? 'success' : 'default'">
                      {{ showPopup ? '显示' : '隐藏' }}
                    </n-tag>
                  </n-space>

                  <n-space align="center" justify="space-between">
                    <span>选项数量:</span>
                    <n-tag size="small" type="info">
                      {{ currentRequest.predefined_options?.length || 0 }}
                    </n-tag>
                  </n-space>
                </n-space>
              </div>
            </n-space>
          </n-card>
        </div>

        <!-- 弹窗组件显示区域 -->
        <div class="popup-container">
          <!-- 弹窗组件 -->
          <div v-if="showPopup" class="popup-mode">
            <div
              class="popup-overlay"
              :class="currentTheme === 'light' ? 'popup-overlay--light' : 'popup-overlay--dark'"
            >
              <div
                class="popup-shell"
                :class="currentTheme === 'light' ? 'popup-shell--light' : 'popup-shell--dark'"
              >
                <div class="popup-shell-header">
                  <PopupHeader
                    :current-theme="currentTheme"
                    :loading="false"
                    :show-main-layout="false"
                    :always-on-top="mockAppConfig.window.alwaysOnTop"
                    :is-muted="isMuted"
                    :shortcut-enabled="shortcutEnabled"
                    :quota-providers="usageProviders"
                    project-path="/Users/test/project"
                    conversation-title="为 MCP 弹窗显示当前对话标题"
                    @theme-change="handleThemeChange"
                    @open-main-layout="handleOpenMainLayout"
                    @toggle-always-on-top="handleToggleAlwaysOnTop"
                    @toggle-mute="handleToggleMute"
                    @new-chat="handleNewChat"
                    @toggle-terminal="handleToggleTerminal"
                    @toggle-shortcut="handleToggleShortcut"
                    @minimize-window="handleMinimizeWindow"
                  />
                </div>
                <McpPopup
                  :request="currentRequest" :app-config="mockAppConfig" :mock-mode="true" :is-muted="isMuted"
                  :timeline-mock-nodes="mockTimelineNodes"
                  timeline-current-node-id="test-node-3"
                  @response="handleResponse" @cancel="handleCancel" @theme-change="handleThemeChange"
                  @open-artifact="handleOpenArtifact"
                  @open-main-layout="handleOpenMainLayout"
                />
              </div>
            </div>
          </div>

          <!-- 隐藏状态提示 -->
          <div v-else class="hidden-state">
            <div class="hidden-message">
              <h3>弹窗已隐藏</h3>
              <p>点击"显示弹窗"按钮来查看弹窗组件</p>
            </div>
          </div>
        </div>

        <!-- 说明信息 -->
        <div class="info-panel">
          <n-card title="测试说明" size="small">
            <n-space vertical size="small">
              <div class="flex items-center text-sm">
                <div class="w-1.5 h-1.5 bg-green-500 rounded-full mr-3 flex-shrink-0" />
                全新的模块化弹窗系统，支持完整的状态管理和过渡动画
              </div>
              <div class="flex items-center text-sm">
                <div class="w-1.5 h-1.5 bg-green-500 rounded-full mr-3 flex-shrink-0" />
                模块化组件：头部、内容、输入、操作栏独立组件
              </div>
              <div class="flex items-center text-sm">
                <div class="w-1.5 h-1.5 bg-green-500 rounded-full mr-3 flex-shrink-0" />
                支持模拟数据，无需依赖MCP服务
              </div>
              <div class="flex items-center text-sm">
                <div class="w-1.5 h-1.5 bg-green-500 rounded-full mr-3 flex-shrink-0" />
                符合代码规范，使用UnoCSS和Naive UI组件
              </div>
              <div class="flex items-center text-sm">
                <div class="w-1.5 h-1.5 bg-blue-500 rounded-full mr-3 flex-shrink-0" />
                <span class="opacity-70">src/frontend/components/popup/</span>
              </div>
            </n-space>
          </n-card>
        </div>
      </n-card>
    </div>

    <!-- 纯净模式 - 只显示弹窗 -->
    <div v-else class="pure-mode">
      <div
        class="popup-shell"
        :class="currentTheme === 'light' ? 'popup-shell--light' : 'popup-shell--dark'"
      >
        <div class="popup-shell-header">
          <PopupHeader
            :current-theme="currentTheme"
            :loading="false"
            :show-main-layout="false"
            :always-on-top="mockAppConfig.window.alwaysOnTop"
            :is-muted="isMuted"
            :shortcut-enabled="shortcutEnabled"
            :quota-providers="usageProviders"
            project-path="/Users/test/project"
            conversation-title="为 MCP 弹窗显示当前对话标题"
            @theme-change="handleThemeChange"
            @open-main-layout="handleOpenMainLayout"
            @toggle-always-on-top="handleToggleAlwaysOnTop"
            @toggle-mute="handleToggleMute"
            @new-chat="handleNewChat"
            @toggle-terminal="handleToggleTerminal"
            @toggle-shortcut="handleToggleShortcut"
            @minimize-window="handleMinimizeWindow"
          />
        </div>
        <McpPopup
          :request="currentRequest" :app-config="mockAppConfig" :mock-mode="true" :is-muted="isMuted"
          :timeline-mock-nodes="mockTimelineNodes"
          timeline-current-node-id="test-node-3"
          @response="handleResponse"
          @cancel="handleCancel" @theme-change="handleThemeChange" @open-artifact="handleOpenArtifact"
          @open-main-layout="handleOpenMainLayout"
        />
      </div>
    </div>

    <div
      v-show="activeArtifact"
      class="fixed inset-0 z-[20000] flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
      @click.self="handleCloseArtifact"
    >
      <section
        class="flex h-[min(860px,calc(100vh-32px))] w-[min(1120px,calc(100vw-32px))] flex-col overflow-hidden rounded-lg border shadow-2xl"
        :class="currentTheme === 'light' ? 'border-gray-200 bg-white text-gray-900' : 'border-white/10 bg-[#0d0e11] text-white'"
      >
        <header
          class="flex min-h-12 items-center justify-between gap-3 border-b px-4"
          :class="currentTheme === 'light' ? 'border-gray-200' : 'border-white/10'"
        >
          <div class="flex min-w-0 items-center gap-2.5">
            <div
              class="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-md"
              :class="currentTheme === 'light' ? 'bg-gray-100 text-gray-700' : 'bg-white/10 text-gray-200'"
            >
              <div class="i-carbon-html w-4 h-4" />
            </div>
            <div class="min-w-0">
              <div class="truncate text-sm font-semibold">
                {{ previewArtifact?.title || 'HTML Artifact' }}
              </div>
              <div
                v-if="previewArtifact?.description || previewArtifact?.path"
                class="truncate text-xs"
                :class="currentTheme === 'light' ? 'text-gray-500' : 'text-gray-400'"
              >
                {{ previewArtifact?.description || previewArtifact?.path }}
              </div>
            </div>
          </div>

          <div class="flex flex-shrink-0 items-center gap-2">
            <button
              v-if="previewArtifactContent"
              type="button"
              class="inline-flex h-8 items-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition-colors"
              :class="currentTheme === 'light'
                ? 'border-gray-200 bg-white text-gray-700 hover:bg-gray-50'
                : 'border-white/10 bg-white/5 text-gray-200 hover:bg-white/10'"
              @click="handleCopyArtifact"
            >
              <div class="i-carbon-copy w-3.5 h-3.5" />
              <span>复制 HTML</span>
            </button>
            <button
              v-if="previewArtifactContent"
              type="button"
              class="inline-flex h-8 items-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition-colors"
              :class="currentTheme === 'light'
                ? 'border-gray-200 bg-white text-gray-700 hover:bg-gray-50'
                : 'border-white/10 bg-white/5 text-gray-200 hover:bg-white/10'"
              @click="handleOpenArtifactInBrowser"
            >
              <div class="i-carbon-launch w-3.5 h-3.5" />
              <span>浏览器打开</span>
            </button>
            <button
              type="button"
              class="inline-flex h-8 w-8 items-center justify-center rounded-md border transition-colors"
              :class="currentTheme === 'light'
                ? 'border-gray-200 bg-white text-gray-700 hover:bg-gray-50'
                : 'border-white/10 bg-white/5 text-gray-200 hover:bg-white/10'"
              title="关闭 HTML 预览"
              @click="handleCloseArtifact"
            >
              <div class="i-carbon-close w-4 h-4" />
            </button>
          </div>
        </header>

        <div class="min-h-0 flex-1 overflow-y-auto">
          <HtmlArtifactRenderer
            v-if="previewArtifactContent"
            :content="previewArtifactContent"
            :current-theme="currentTheme"
            :title="previewArtifact?.title || 'HTML Artifact'"
            :show-chrome="false"
          />
          <div
            v-else
            class="flex h-full items-center justify-center px-6 text-sm"
            :class="currentTheme === 'light' ? 'text-gray-500' : 'text-gray-400'"
          >
            暂无可预览的 HTML 内容
          </div>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
.mcp-popup-test {
  max-width: 1200px;
  margin: 0 auto;
}

.control-panel {
  margin-bottom: 20px;
}

.control-section {
  margin-bottom: 15px;
}

.control-section h4 {
  margin: 0 0 8px 0;
  color: var(--text-color);
  font-size: 0.9rem;
  font-weight: 500;
}

.popup-container {
  margin: 20px 0;
  border: 2px dashed var(--border-color);
  border-radius: 8px;
  padding: 0;
  background: var(--card-color);
  position: relative;
  min-height: 400px;
  overflow: hidden;
}

.popup-container::before {
  content: '新弹窗系统预览 - 支持模块化组件和状态管理';
  position: absolute;
  top: -10px;
  left: 20px;
  background: var(--card-color);
  padding: 0 10px;
  font-size: 0.8rem;
  color: var(--text-color);
  opacity: 0.6;
  z-index: 10;
}

.popup-overlay {
  position: relative;
  width: 100%;
  height: 100%;
  min-height: 400px;
  padding: 12px;
}

.popup-overlay--light {
  background: #eef2f7;
}

.popup-overlay--dark {
  background: rgba(0, 0, 0, 0.1);
}

.popup-shell {
  min-height: 400px;
  background: var(--body-color);
  border-radius: 12px;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--color-border);
  color: var(--text-color);
}

.popup-shell--dark {
  background: #0b0b10;
  border-color: rgba(255, 255, 255, 0.08);
  color: #e5e7eb;
}

.popup-shell--light {
  background: #ffffff;
  border-color: #d1d5db;
  color: #111827;
}

.popup-shell-header {
  flex-shrink: 0;
  border-bottom: 1px solid var(--color-divider);
}

.popup-shell--dark .popup-shell-header {
  border-bottom-color: rgba(255, 255, 255, 0.08);
}

.popup-shell--light .popup-shell-header {
  border-bottom-color: #e5e7eb;
}

.popup-overlay :deep(.popup-container) {
  position: relative !important;
  width: 100% !important;
  height: 100% !important;
  max-width: none !important;
  max-height: none !important;
  border-radius: 0 !important;
}

.info-panel {
  margin-top: 20px;
}

/* 纯净模式 */
.pure-mode {
  width: 100%;
  height: 100%;
}

.pure-mode :deep(.popup-container) {
  position: relative !important;
  inset: 0 !important;
  width: 100% !important;
  height: 100% !important;
}

/* 增强模式样式 */
.enhanced-mode {
  @apply w-full h-full min-h-[500px];
}

/* 基础模式样式 */
.basic-mode {
  @apply w-full h-full min-h-[500px];
}

/* 隐藏状态样式 */
.hidden-state {
  @apply flex items-center justify-center w-full h-full min-h-[300px];
  @apply bg-gray-50 dark:bg-gray-800 rounded-lg;
}

.hidden-message {
  @apply text-center space-y-2;
}

.hidden-message h3 {
  @apply text-lg font-medium text-gray-700 dark:text-gray-300;
}

.hidden-message p {
  @apply text-sm text-gray-500 dark:text-gray-400;
}
</style>
