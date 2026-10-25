<script setup lang="ts">
// 任务成果区的统一 Markdown 容器（C-2A，2026-10-25）。
//
// 原 TaskBoard 的成果/审计/最终计划/待批准计划四处各写一遍
// `<div class="sv-task-result" v-html="...">`，prose 的 `:deep(...)` 规则也只写在
// TaskBoard 里。抽成组件后：
// - 调用方只传已预渲染好的 html（缓存见 utils/markdownCache.ts）；
// - hero 边框与正文排版规则随组件走，父级仍可给根节点追加 class 做局部覆盖
//   （如批准区的 .sv-task-planned-plan 去掉边框）。
defineProps<{
  /** 已渲染好的安全 HTML（调用方负责 renderMarkdown / renderMarkdownCached） */
  html: string;
}>();
</script>

<template>
  <div class="sv-task-result" v-html="html" />
</template>

<style scoped>
/* 成果:白纸 hero + 黑顶线;正文 46em,不被整栏拉宽 */
.sv-task-result {
  max-width: var(--measure-task-result);
  padding: var(--space-5);
  border: var(--bw) solid var(--sv-ink);
  border-top: 5px solid var(--sv-ink);
  font-size: var(--text-base);
  line-height: var(--lh-prose);
}
.sv-task-result :deep(h1),
.sv-task-result :deep(h2),
.sv-task-result :deep(h3) {
  font-family: var(--font-body);
  font-weight: 800;
  letter-spacing: -0.01em;
  line-height: var(--lh-tight);
}
.sv-task-result :deep(h1) { margin: 0.2em 0 0.5em; font-size: var(--text-lg); }
.sv-task-result :deep(h2) { margin: 1.1em 0 0.4em; font-size: var(--text-md); }
.sv-task-result :deep(h3) { margin: 0.9em 0 0.35em; font-size: var(--text-base); }
.sv-task-result :deep(p) { margin: 0.7em 0; }
.sv-task-result :deep(ul),
.sv-task-result :deep(ol) { margin: 0.6em 0; padding-left: 1.4em; }
.sv-task-result :deep(ul) { list-style: disc; }
.sv-task-result :deep(ol) { list-style: decimal; }
.sv-task-result :deep(li) { margin: 0.28em 0; }
.sv-task-result :deep(blockquote) {
  margin: 0.8em 0;
  padding: 0.35em 0.9em;
  border-left: 3px solid var(--sv-red);
  background: var(--sv-paper);
  color: var(--sv-ink-dim);
}
.sv-task-result :deep(table) {
  width: 100%;
  margin: 0.8em 0;
  border-collapse: collapse;
  font-size: var(--text-sm);
}
.sv-task-result :deep(th),
.sv-task-result :deep(td) {
  padding: var(--space-1-5) var(--space-2-5);
  border: 1px solid var(--sv-line-strong);
  text-align: left;
  vertical-align: top;
}
.sv-task-result :deep(th) {
  background: var(--sv-paper);
  font-weight: 800;
}
.sv-task-result :deep(a) {
  color: var(--sv-red-deep);
  text-decoration: underline;
  text-underline-offset: 3px;
}
.sv-task-result :deep(code) {
  padding: 0.1em 0.3em;
  border: 1px solid var(--sv-line);
  background: var(--sv-paper);
  font-family: var(--font-mono);
  font-size: 0.92em;
}
.sv-task-result :deep(pre code) {
  padding: 0;
  border: none;
  background: transparent;
}
.sv-task-result :deep(pre) { font-size: var(--text-xs); line-height: 1.7; }
</style>
