<script setup lang="ts">
import { onMounted, ref } from 'vue'

// 步骤演示：静音循环的短视频。没有素材或加载失败时整块不显示，系统开启「减少动态效果」时不自动播放。
const props = defineProps<{
  src?: string
  poster?: string
  /** 素材的分镜说明，供录制或替换素材时参考，不在界面上显示 */
  storyboard?: string
  label: string
}>()

const video = ref<HTMLVideoElement | null>(null)
const paused = ref(false)
const failed = ref(false)
const reduceMotion = ref(false)

onMounted(() => {
  reduceMotion.value = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
  paused.value = reduceMotion.value
})

function toggle() {
  const el = video.value
  if (!el)
    return
  if (el.paused) {
    void el.play()
    paused.value = false
  }
  else {
    el.pause()
    paused.value = true
  }
}
</script>

<template>
  <figure v-if="props.src && !failed" class="guide-media">
    <video
      ref="video"
      :src="props.src"
      :poster="props.poster"
      :autoplay="!reduceMotion"
      loop
      muted
      playsinline
      :aria-label="props.label"
      @click="toggle"
      @error="failed = true"
    />
    <button type="button" class="media-toggle" @click="toggle">
      {{ paused ? '播放演示' : '暂停' }}
    </button>
  </figure>
</template>

<style scoped>
.guide-media {
  position: relative;
  margin: 0;
  border-radius: 12px;
  overflow: hidden;
  background: var(--guide-media-bg, #f4f4f5);
  aspect-ratio: 16 / 9;
}

.guide-media video {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
  cursor: pointer;
}

.media-toggle {
  position: absolute;
  right: 10px;
  bottom: 10px;
  padding: 4px 10px;
  border: 0;
  border-radius: 999px;
  background: rgba(0, 0, 0, 0.6);
  color: #fff;
  font-size: 12px;
  cursor: pointer;
}
</style>
