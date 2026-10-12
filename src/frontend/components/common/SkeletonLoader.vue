<script setup lang="ts">
interface Props {
  type?: 'popup' | 'main-layout' | 'content'
  lines?: number
  showHeader?: boolean
  showActions?: boolean
}

const _props = withDefaults(defineProps<Props>(), {
  type: 'popup',
  lines: 3,
  showHeader: true,
  showActions: true,
})
</script>

<template>
  <div class="w-full">
    <!-- 弹窗骨架屏 -->
    <div v-if="type === 'popup'" class="flex flex-col h-full">
      <!-- 头部骨架 -->
      <div v-if="showHeader" class="flex items-center justify-between p-4 border-b border-gray-200 dark:border-gray-700">
        <div class="skeleton-fill rounded-full w-3 h-3" />
        <div class="skeleton-fill rounded w-32 h-4" />
        <div class="skeleton-fill rounded-full w-6 h-6" />
      </div>

      <!-- 内容骨架 -->
      <div class="flex-1 p-4 space-y-2">
        <div class="skeleton-fill rounded w-full h-6 mb-4" />
        <div
          v-for="i in lines" :key="i" class="skeleton-fill rounded mb-2" :class="[
            i === lines ? 'w-3/4' : 'w-full',
            i === 1 ? 'h-5' : 'h-4',
          ]"
        />
      </div>

      <!-- 输入区域骨架 -->
      <div class="p-4 border-t border-gray-200 dark:border-gray-700">
        <div class="skeleton-fill rounded w-24 h-4 mb-2" />
        <div class="skeleton-fill rounded-md w-full h-20" />
      </div>

      <!-- 操作栏骨架 -->
      <div v-if="showActions" class="flex items-center justify-between p-4 border-t border-gray-200 dark:border-gray-700">
        <div class="skeleton-fill rounded w-16 h-4" />
        <div class="flex gap-2">
          <div class="skeleton-fill rounded-md w-16 h-8" />
          <div class="skeleton-fill rounded-md w-16 h-8" />
        </div>
      </div>
    </div>

    <!-- 主界面骨架屏 -->
    <div v-else-if="type === 'main-layout'" class="flex flex-col items-center space-y-6 p-6 pt-12">
      <!-- 标题区域 -->
      <div class="flex items-center gap-3">
        <div class="skeleton-fill rounded-full w-10 h-10" />
        <div class="skeleton-fill rounded w-32 h-8" />
      </div>

      <!-- 状态标签 -->
      <div class="flex justify-center">
        <div class="skeleton-fill rounded-full w-24 h-6" />
      </div>

      <!-- 副标题 -->
      <div class="flex justify-center">
        <div class="skeleton-fill rounded w-48 h-4" />
      </div>

      <!-- 标签页 -->
      <div class="flex gap-2 justify-center">
        <div class="skeleton-fill rounded-full w-16 h-8" />
        <div class="skeleton-fill rounded-full w-20 h-8" />
        <div class="skeleton-fill rounded-full w-18 h-8" />
        <div class="skeleton-fill rounded-full w-12 h-8" />
      </div>

      <!-- 内容区域 -->
      <div class="w-full max-w-6xl">
        <div class="grid grid-cols-2 gap-4">
          <div v-for="i in 4" :key="i" class="p-4 rounded-lg border border-gray-200 dark:border-gray-700">
            <div class="skeleton-fill rounded-full w-8 h-8 mb-3" />
            <div class="skeleton-fill rounded w-full h-5 mb-2" />
            <div class="skeleton-fill rounded w-3/4 h-4" />
          </div>
        </div>
      </div>
    </div>

    <!-- 通用内容骨架屏 -->
    <div v-else class="space-y-2">
      <div
        v-for="i in lines" :key="i" class="skeleton-fill rounded mb-3" :class="[
          i === 1 ? 'w-full h-6' : 'w-5/6 h-4',
          i === lines ? 'w-1/2' : '',
        ]"
      />
    </div>
  </div>
</template>
