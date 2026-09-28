<script setup lang="ts">
import type { PromptLibraryItem } from '../../composables/usePromptLibrary'
import { useMessage } from 'naive-ui'
import { ref } from 'vue'
import { usePromptLibrary } from '../../composables/usePromptLibrary'

const message = useMessage()
const promptLibrary = usePromptLibrary()

// UI 状态
const showAddDialog = ref(false)
const showEditDialog = ref(false)
const showImportDirDialog = ref(false)
const fileInputRef = ref<HTMLInputElement | null>(null)
const importDirPath = ref('')

// 新建表单
const newItem = ref({ name: '', content: '', category: '' })

// 编辑表单
const editingItem = ref<{ id: string, name: string, content: string, category: string } | null>(null)

// 添加提示词
function addItem() {
  if (!newItem.value.name.trim() || !newItem.value.content.trim()) {
    message.warning('名称和内容不能为空')
    return
  }
  const item = promptLibrary.addItem(
    newItem.value.name,
    newItem.value.content,
    newItem.value.category,
  )
  if (item) {
    message.success('提示词已添加')
    newItem.value = { name: '', content: '', category: '' }
    showAddDialog.value = false
  }
  else {
    message.warning('添加失败（可能已存在相同提示词）')
  }
}

// 编辑提示词
function startEdit(item: PromptLibraryItem) {
  editingItem.value = { id: item.id, name: item.name, content: item.content, category: item.category }
  showEditDialog.value = true
}

function saveEdit() {
  if (!editingItem.value)
    return
  const ok = promptLibrary.updateItem(editingItem.value.id, {
    name: editingItem.value.name,
    content: editingItem.value.content,
    category: editingItem.value.category,
  })
  if (ok) {
    message.success('已更新')
    editingItem.value = null
    showEditDialog.value = false
  }
}

// 删除提示词
function deleteItem(id: string) {
  if (promptLibrary.deleteItem(id)) {
    message.success('已删除')
  }
}

// 文件导入
async function handleFileImport(event: Event) {
  const input = event.target as HTMLInputElement
  if (!input.files?.length)
    return
  const result = await promptLibrary.importFiles(input.files)
  message.success(`导入 ${result.imported} 条，跳过 ${result.skipped} 条${result.failedFiles.length ? `，失败: ${result.failedFiles.join(', ')}` : ''}`)
  input.value = ''
}

// 目录导入
async function handleDirImport() {
  if (!importDirPath.value.trim()) {
    message.warning('请输入目录路径')
    return
  }
  const result = await promptLibrary.importFromDirectory(importDirPath.value)
  if (result.imported > 0 || result.skipped > 0) {
    message.success(`导入 ${result.imported} 条，跳过 ${result.skipped} 条${result.failedFiles.length ? `，失败: ${result.failedFiles.join(', ')}` : ''}`)
  }
  else {
    message.warning('未找到可导入的提示词文件')
  }
  showImportDirDialog.value = false
}

// 清空
function clearAll() {
  promptLibrary.clearLibrary()
  message.success('提示词库已清空')
}
</script>

<template>
  <div v-if="promptLibrary.saveError.value" role="alert" class="mb-3 text-red-500">
    {{ promptLibrary.saveError.value }}。未保存的编辑仍保留在当前窗口。
    <n-button size="small" @click="promptLibrary.load()">放弃未保存编辑并重新加载</n-button>
  </div>
  <div class="p-4">
    <!-- 统计信息 -->
    <div class="flex items-center justify-between mb-4">
      <div class="text-sm opacity-60">
        共 {{ promptLibrary.items.value.length }} 条提示词，{{ promptLibrary.categories.value.length }} 个分类
      </div>
      <div class="flex items-center gap-2">
        <n-button size="small" @click="showImportDirDialog = true">
          <template #icon>
            <div class="i-carbon-folder w-4 h-4" />
          </template>
          目录导入
        </n-button>
        <n-button size="small" @click="fileInputRef?.click()">
          <template #icon>
            <div class="i-carbon-upload w-4 h-4" />
          </template>
          文件导入
        </n-button>
        <n-button type="primary" size="small" @click="showAddDialog = true">
          <template #icon>
            <div class="i-carbon-add w-4 h-4" />
          </template>
          添加
        </n-button>
      </div>
    </div>

    <input
      ref="fileInputRef"
      type="file"
      accept=".txt"
      multiple
      class="hidden"
      @change="handleFileImport"
    >

    <!-- 分类展示 -->
    <div v-if="promptLibrary.items.value.length === 0" class="text-center py-8 opacity-60">
      <div class="i-carbon-catalog text-4xl mb-2" />
      <div>提示词库为空</div>
      <div class="text-xs mt-1">
        点击"添加"新建，或从目录导入手机端提示词
      </div>
    </div>

    <div v-else class="space-y-4">
      <!-- 按分类分组显示 -->
      <div v-for="category in promptLibrary.categories.value" :key="category">
        <div class="flex items-center justify-between mb-2">
          <div class="text-sm font-medium text-on-surface flex items-center gap-2">
            <span class="px-2 py-0.5 rounded bg-primary-500/15 text-primary-400 text-xs">{{ category }}</span>
            <span class="text-xs opacity-50">{{ promptLibrary.items.value.filter(i => i.category === category).length }} 条</span>
          </div>
        </div>
        <div class="space-y-2">
          <div
            v-for="item in promptLibrary.items.value.filter(i => i.category === category)"
            :key="item.id"
            class="bg-black-50 rounded-lg p-3 border border-black-200 shadow-sm hover:border-black-300 transition-colors"
          >
            <div class="flex justify-between items-start">
              <div class="flex-1 min-w-0">
                <div class="font-medium text-white text-sm mb-1">
                  {{ item.name }}
                </div>
                <div class="text-xs opacity-60 truncate" :title="item.content">
                  {{ item.content }}
                </div>
              </div>
              <div class="flex gap-1 ml-3 flex-shrink-0">
                <n-button size="tiny" quaternary @click="startEdit(item)">
                  <template #icon>
                    <div class="i-carbon-edit w-3.5 h-3.5" />
                  </template>
                </n-button>
                <n-button size="tiny" quaternary type="error" @click="deleteItem(item.id)">
                  <template #icon>
                    <div class="i-carbon-trash-can w-3.5 h-3.5" />
                  </template>
                </n-button>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 清空按钮 -->
      <div class="flex justify-end pt-2">
        <n-popconfirm @positive-click="clearAll">
          <template #trigger>
            <n-button size="small" type="error" quaternary>
              清空提示词库
            </n-button>
          </template>
          确定要清空所有提示词吗？此操作不可撤销。
        </n-popconfirm>
      </div>
    </div>

    <!-- 添加对话框 -->
    <n-modal v-model:show="showAddDialog" preset="card" title="添加提示词" style="width: 500px">
      <n-form :model="newItem" label-placement="top">
        <n-form-item label="名称" required>
          <n-input v-model:value="newItem.name" placeholder="提示词名称（如：Debug）" />
        </n-form-item>
        <n-form-item label="分类">
          <n-input v-model:value="newItem.category" placeholder="分类名称（如：编程、看书）" />
        </n-form-item>
        <n-form-item label="内容" required>
          <n-input
            v-model:value="newItem.content"
            type="textarea"
            placeholder="提示词内容..."
            :autosize="{ minRows: 4, maxRows: 10 }"
          />
        </n-form-item>
      </n-form>
      <template #footer>
        <div class="flex justify-end gap-2">
          <n-button @click="showAddDialog = false">
            取消
          </n-button>
          <n-button type="primary" @click="addItem">
            添加
          </n-button>
        </div>
      </template>
    </n-modal>

    <!-- 编辑对话框 -->
    <n-modal v-model:show="showEditDialog" preset="card" title="编辑提示词" style="width: 500px">
      <n-form v-if="editingItem" :model="editingItem" label-placement="top">
        <n-form-item label="名称" required>
          <n-input v-model:value="editingItem.name" placeholder="提示词名称" />
        </n-form-item>
        <n-form-item label="分类">
          <n-input v-model:value="editingItem.category" placeholder="分类名称" />
        </n-form-item>
        <n-form-item label="内容" required>
          <n-input
            v-model:value="editingItem.content"
            type="textarea"
            placeholder="提示词内容..."
            :autosize="{ minRows: 4, maxRows: 10 }"
          />
        </n-form-item>
      </n-form>
      <template #footer>
        <div class="flex justify-end gap-2">
          <n-button @click="showEditDialog = false">
            取消
          </n-button>
          <n-button type="primary" @click="saveEdit">
            保存
          </n-button>
        </div>
      </template>
    </n-modal>

    <!-- 目录导入对话框 -->
    <n-modal v-model:show="showImportDirDialog" preset="card" title="从目录导入提示词" style="width: 500px">
      <n-form label-placement="top">
        <n-form-item label="目录路径">
          <n-input v-model:value="importDirPath" placeholder="提示词文件所在目录路径" />
        </n-form-item>
        <div class="text-xs opacity-60 mb-2">
          将读取目录下所有 .txt 文件，支持 prompts_*_export.txt 格式（每条提示词：标题\n内容\n\n）
        </div>
      </n-form>
      <template #footer>
        <div class="flex justify-end gap-2">
          <n-button @click="showImportDirDialog = false">
            取消
          </n-button>
          <n-button type="primary" :loading="promptLibrary.isImporting.value" @click="handleDirImport">
            导入
          </n-button>
        </div>
      </template>
    </n-modal>
  </div>
</template>
