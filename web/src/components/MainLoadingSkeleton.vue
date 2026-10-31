<script setup lang="ts">
// 主区懒加载骨架(UIVIS-3/UI-C2):仅在 ChatWindow / TaskBoard 的 chunk 下载期间可见。
//
// 形态与 SkeletonBlock 的区别:后者是字段级细条(12px 高、字段宽度),铺满主区会读成
// 「加载失败」;这里给的是「一屏内容即将出现」的轮廓——标题行 + 段落行 + 两块正文块,
// 与消息区/任务台的真实排布同一节奏,切换时视觉重心不跳。
//
// 纯展示、零 props、零依赖:它会在 chunk 到位前被挂载,必须保持极小的初始化成本。
</script>

<template>
  <div class="sv-main-skeleton" role="status" aria-label="正在加载">
    <div class="sv-main-skeleton-head" aria-hidden="true" />
    <div class="sv-main-skeleton-lines" aria-hidden="true">
      <div class="sv-main-skeleton-line" />
      <div class="sv-main-skeleton-line" />
      <div class="sv-main-skeleton-line short" />
    </div>
    <div class="sv-main-skeleton-block" aria-hidden="true" />
    <div class="sv-main-skeleton-lines" aria-hidden="true">
      <div class="sv-main-skeleton-line" />
      <div class="sv-main-skeleton-line short" />
    </div>
  </div>
</template>

<style scoped>
/* 骨架沿用 UIP-13 的呼吸语言(纯透明度,无渐变/无扫光);reduced-motion 由
   content.css 的全局兜底降级为静态块(与其他 sv-skeleton 同一套)。 */
.sv-main-skeleton {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  padding: var(--space-6) clamp(16px, 3vw, 36px);
  max-width: var(--measure-chat);
  margin: 0 auto;
  width: 100%;
}
.sv-main-skeleton-head {
  height: 28px;
  width: 42%;
  background: var(--sv-line-strong);
  animation: sv-main-skeleton-pulse var(--dur-pulse) var(--ease-in-out) infinite;
}
.sv-main-skeleton-lines {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.sv-main-skeleton-line {
  height: 12px;
  background: var(--sv-line);
  animation: sv-main-skeleton-pulse var(--dur-pulse) var(--ease-in-out) infinite;
}
.sv-main-skeleton-line.short { width: 62%; }
/* 正文块:模拟消息气泡/成果卡的高度与边框节奏 */
.sv-main-skeleton-block {
  height: 132px;
  border: var(--bw) solid var(--sv-line-strong);
  background: var(--sv-white);
  animation: sv-main-skeleton-pulse var(--dur-pulse) var(--ease-in-out) infinite;
}
@keyframes sv-main-skeleton-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.55; }
}
</style>
