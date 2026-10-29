<script setup lang="ts">
// 侧栏任务历史（C-2A，2026-10-25；原 Sidebar 659-684）。
// 两个根节点（标题 + 列表）保持为 .sv-task-scroll 的直接子项，避免包一层 div 破坏
// UIP-4 的单滚动区 flex 布局；删除走 useTaskActions 的单一出口。
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { useTaskActions } from '../../composables/useTaskActions';
import { taskStatusClass as statusClass, taskStatusLabel as statusLabel } from '../../taskStatus';

const store = useAppStore();
const { tasks, currentTaskId } = storeToRefs(store);
const { removeTask } = useTaskActions();
</script>

<template>
  <div class="sv-task-list-head">任务历史</div>
  <div class="sv-task-list">
    <button
      v-for="t in tasks"
      :key="t.id"
      class="sv-task-item"
      :class="{ active: t.id === currentTaskId }"
      @click="store.selectTask(t.id)"
    >
      <span :key="t.status" class="sv-supreme xs sv-dot-flash" :class="statusClass(t.status)" />
      <span class="sv-task-item-title">{{ t.title }}</span>
      <span class="sv-task-item-meta">{{ statusLabel(t.status) }}</span>
      <span class="sv-task-item-del" title="删除任务" @click.stop="removeTask(t)">✕</span>
    </button>
    <div v-if="tasks.length === 0" class="sv-empty-geo sm mb10">
      <span class="sq black" />
      <span class="diag" />
    </div>
    <p v-if="tasks.length === 0" class="sv-task-list-empty">
      暂无任务,输入目标创建第一个任务
    </p>
    <p v-if="tasks.length === 0" class="sv-task-list-empty-hint">
      01 写目标 · 02 展开执行设置 · 03 创建并执行
    </p>
  </div>
</template>

<style scoped>
.sv-task-list-empty,
.sv-task-list-empty-hint {
  text-align: center;
}
.sv-task-list-empty-hint {
  margin: var(--space-1) 0 0;
  font-size: var(--text-xs);
  line-height: var(--lh-body);
  color: var(--sv-ink-faint);
}
.sv-task-list .sv-empty-geo.sm {
  justify-content: center;
  margin-left: auto;
  margin-right: auto;
}
</style>
