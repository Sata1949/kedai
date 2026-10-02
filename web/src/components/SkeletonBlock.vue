<script setup lang="ts">
// UIP-13 加载骨架(2026-10-02):统一替代「加载中…」纯文字内容占位。
// 构成主义硬边体系内:方角块面 + 纯透明度呼吸,无渐变/无扫光;reduced-motion 由
// content.css 全局兜底降级为静态块。容器带 role=status 保持「加载中」语义可达。
interface Props {
  /** 骨架行数(>1 时末行自动收窄为 62%,更接近真实列表形态) */
  lines?: number;
  /** 容器宽度(默认 100%;字段值位等内联场景传 px 定宽) */
  width?: string;
}
const props = withDefaults(defineProps<Props>(), { lines: 3, width: '100%' });
</script>

<template>
  <div class="sv-skeleton" :style="{ width: props.width }" role="status" aria-label="加载中">
    <div
      v-for="i in props.lines"
      :key="i"
      class="sv-skeleton-line"
      :style="i === props.lines && props.lines > 1 ? { width: '62%' } : undefined"
      aria-hidden="true"
    />
  </div>
</template>

<style scoped>
.sv-skeleton {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.sv-skeleton-line {
  height: 12px;
  background: var(--sv-line-strong);
  animation: sv-skeleton-pulse var(--dur-pulse) var(--ease-in-out) infinite;
}
@keyframes sv-skeleton-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.55; }
}
</style>
