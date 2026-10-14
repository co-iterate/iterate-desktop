<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { onMounted, ref } from 'vue'

interface ReplyConfig {
  enable_continue_reply: boolean
  copy_submission_to_clipboard: boolean
  auto_continue_threshold: number
  continue_prompt: string
  loop_prompt: string
  goal_prompt_template: string
}

const DEFAULT_GOAL_PROMPT_TEMPLATE = `1. 先把这句话整理成可执行目标；在执行任何实现动作前，必须用 Codex 的 get_goal 检查本线程正式 Goal，并完成同步：无正式 Goal 时立即 create_goal；现有 Goal 与本目标相同则继续；现有未完成 Goal 不同则先核对真实状态，只有已有证据证明它确实完成时才 update_goal 为 complete 后创建本目标，否则停止执行并通过 zhi 报告冲突，绝不能伪造完成或在未同步状态下继续。
2. Codex 正式 Goal 是唯一状态源，iterate Live Goal 只负责展示；create_goal 成功后再开始实现，并在真正完成且验证通过后按 Goal 工具规则更新状态。
3. 围绕目标自己选择合适的 Skill 和工具，持续执行、修复、验证；能合理推进就不要反问。
4. 失败就继续定位和修复，直到验证通过、确实阻塞，或碰到目标外的高风险边界。
5. 完成后再交给用户验收：说明做了什么、验证了什么、还有什么风险。
6. 只有明显越界、破坏性操作、凭据/登录、Computer Use、提交/推送/发布，或发现需要沉淀的新问题时，才通过 zhi 询问。
7. 这是目标提交，不是迭代循环；不要生成 [迭代 x/10] 这类轮次提示。
8. 如果任务完成，明确写“已完成”；如果阻塞，说明原因、证据和可选下一步。`

const localConfig = ref<ReplyConfig>({
  enable_continue_reply: true,
  copy_submission_to_clipboard: false,
  auto_continue_threshold: 1000,
  continue_prompt: '请按照最佳实践继续',
  loop_prompt: '进入自主循环模式。\n\n## 执行规则\n1. 基于当前上下文，按最佳实践继续执行当前任务\n2. 每轮完成后立即调用 iterate/zhi 汇报进度，不要等待用户\n3. 如果任务未完成且无需用户决策，继续自动执行下一步\n\n## 停止条件（满足任一即停止）\n- 任务已全部完成\n- 遇到必须由用户决定的问题\n- 遇到无法自动解决的错误（连续失败2次）\n- 不确定下一步该做什么\n\n## 汇报格式\n每轮简要说明：做了什么 → 结果如何 → 下一步计划',
  goal_prompt_template: DEFAULT_GOAL_PROMPT_TEMPLATE,
})

// 加载配置
async function loadConfig() {
  try {
    const config = await invoke('get_reply_config')
    localConfig.value = {
      ...localConfig.value,
      ...(config as Partial<ReplyConfig>),
    }
  }
  catch (error) {
    console.error('加载继续回复配置失败:', error)
  }
}

// 更新配置
async function updateConfig() {
  try {
    await invoke('set_reply_config', { replyConfig: localConfig.value })
  }
  catch (error) {
    console.error('保存继续回复配置失败:', error)
  }
}

onMounted(() => {
  loadConfig()
})
</script>

<template>
  <!-- 设置内容 -->
  <n-space vertical size="large">
    <!-- 启用继续回复 -->
    <div class="flex items-center justify-between">
      <div class="flex items-center">
        <div class="w-1.5 h-1.5 bg-info rounded-full mr-3 flex-shrink-0" />
        <div>
          <div class="text-sm font-medium leading-relaxed">
            启用继续回复
          </div>
          <div class="text-xs opacity-60">
            启用后将显示继续按钮
          </div>
        </div>
      </div>
      <n-switch
        v-model:value="localConfig.enable_continue_reply"
        size="small"
        @update:value="updateConfig"
      />
    </div>

    <!-- 发送前备份到剪贴板 -->
    <div class="flex items-center justify-between">
      <div class="flex items-center">
        <div class="w-1.5 h-1.5 bg-info rounded-full mr-3 flex-shrink-0" />
        <div>
          <div class="text-sm font-medium leading-relaxed">
            发送时备份到剪贴板
          </div>
          <div class="text-xs opacity-60">
            点击发送或继续时复制当前选项与输入，便于 MCP 失联后手动粘贴
          </div>
        </div>
      </div>
      <n-switch
        v-model:value="localConfig.copy_submission_to_clipboard"
        size="small"
        @update:value="updateConfig"
      />
    </div>

    <!-- Goal 模板 -->
    <div>
      <div class="flex items-center mb-3">
        <div class="w-1.5 h-1.5 bg-info rounded-full mr-3 flex-shrink-0" />
        <div>
          <div class="text-sm font-medium leading-relaxed">
            Goal 模板
          </div>
          <div class="text-xs opacity-60">
            点击目标按钮时附加的执行规则；目标内容与 xi 去重检查由系统自动加入
          </div>
        </div>
      </div>
      <n-input
        v-model:value="localConfig.goal_prompt_template"
        type="textarea"
        size="small"
        :autosize="{ minRows: 5, maxRows: 10 }"
        :placeholder="DEFAULT_GOAL_PROMPT_TEMPLATE"
        @input="updateConfig"
      />
    </div>
  </n-space>
</template>
