<script setup lang="ts">
// 任务附件清单(ATTACH-1,2026-11-03)。
//
// 数据自取(无 props),沿用 `FileChangesPanel.vue` 的体例:挂载点只负责位置,
// 状态与动作走 `useTaskAttachments`。这样做的直接原因见该 composable 的头注释
// ——`stores/task.ts` 行数只降不升,不该为附件再加字段。
//
// 展示口径:
// - 名称是**用户原始文件名**(可能很长/含空格),路径是**模型用的相对路径**;
// - 图像附件显示缩略标记(不真渲染缩略图:附件可达 25MB,产物里也不该出现 base64);
// - 删除走服务端 → 重拉清单(而不是本地 splice),避免与真相分叉。
import { computed, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { useTaskAttachments } from '../../composables/useTaskAttachments';
import type { TaskAttachment } from '../../api';

const store = useAppStore();
const { currentTaskId, currentTask } = storeToRefs(store);
const attachments = useTaskAttachments();

/**
 * 清单来源:优先用 composable 的缓存(刚上传完就即时可见),
 * 缓存为空时回落到详情顶层下发的 `attachments`(首屏/换设备都靠它)。
 */
const list = computed<TaskAttachment[]>(() => {
  const id = currentTaskId.value;
  if (!id) return [];
  const cached = attachments.attachmentsOf(id);
  if (cached.length > 0) return cached;
  return currentTask.value?.attachments ?? [];
});

/** 切换任务时拉一次(缓存已有则不重复请求;失败只记错误、保留旧值) */
watch(
  currentTaskId,
  (id) => {
    if (id && attachments.attachmentsOf(id).length === 0) void attachments.refresh(id);
  },
  { immediate: true },
);

function humanSize(bytes: number): string {
  const kb = 1024;
  if (bytes < kb) return `${bytes} B`;
  if (bytes < kb * kb) return `${(bytes / kb).toFixed(1)} KB`;
  if (bytes < kb * kb * kb) return `${(bytes / (kb * kb)).toFixed(1)} MB`;
  return `${(bytes / (kb * kb * kb)).toFixed(1)} GB`;
}

function isImage(a: TaskAttachment): boolean {
  return (a.mime ?? '').startsWith('image/');
}

async function remove(a: TaskAttachment): Promise<void> {
  const id = currentTaskId.value;
  if (!id) return;
  if (!confirm(`确定删除附件「${a.name}」?模型将不再看到它。`)) return;
  await attachments.remove(id, a.id);
}
</script>

<template>
  <!-- 无附件时不渲染整块(空态噪音):附件是可选输入,没有就不占版面 -->
  <div v-if="list.length > 0" class="sv-task-section">
    <div class="sv-task-section-title">任务附件</div>
    <ul class="sv-attachment-list">
      <li v-for="a in list" :key="a.id" class="sv-attachment-item">
        <span class="sv-attachment-kind" :title="isImage(a) ? '图像' : '文本/其他'">
          {{ isImage(a) ? '图' : '文' }}
        </span>
        <span class="sv-attachment-name" :title="a.name">{{ a.name }}</span>
        <code class="sv-attachment-path" :title="`模型读取用的相对路径:${a.rel_path}`">
          {{ a.rel_path }}
        </code>
        <span class="sv-attachment-size sv-tnum">{{ humanSize(a.size) }}</span>
        <button
          type="button"
          class="sv-btn ghost sv-btn-sm"
          :title="`删除附件「${a.name}」`"
          @click="remove(a)"
        >删除</button>
      </li>
    </ul>
  </div>
  <div v-else-if="attachments.lastError.value" class="sv-task-section">
    <div class="sv-task-section-title">任务附件</div>
    <p class="sv-note">{{ attachments.lastError.value }}</p>
  </div>
</template>

<style scoped>
/* 样式留 scoped,不增全局域行数(css-discipline ratchet)。 */
.sv-attachment-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-1-5);
  margin: 0;
  padding: 0;
  list-style: none;
}
.sv-attachment-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-1-5) 0;
  border-bottom: 1px solid var(--sv-line);
  font-size: var(--text-xs);
}
.sv-attachment-item:last-child { border-bottom: 0; }
/* 类型徽标:方框小字,与任务台其它 chip 同族但不抢注意力 */
.sv-attachment-kind {
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  font-size: var(--text-2xs);
  font-weight: 800;
  border: var(--bw-thin) solid var(--sv-line-strong);
  color: var(--sv-ink-soft);
}
.sv-attachment-name {
  flex: none;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 700;
}
/* 路径是可复制的事实,用等宽字体;窄屏让它吃掉剩余宽度并省略 */
.sv-attachment-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--font-mono, ui-monospace, monospace);
  font-size: var(--text-2xs);
  color: var(--sv-ink-faint);
}
.sv-attachment-size {
  flex: none;
  color: var(--sv-ink-faint);
  font-variant-numeric: tabular-nums;
}
.sv-attachment-item .sv-btn { flex: none; }
</style>
