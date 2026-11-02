<script setup lang="ts">
// 执行过程三档切换条（TMSHOW-2 渲染 / UIFIX-8 抽为常驻表头件）。
//
// 为什么独立成组件：档位条原先埋在 TaskTranscript 内部，上面还隔着标题、状态矩阵、
// 批准区、主 Agent 分工卡——一滚就消失，用户看不到自己正在看哪一档，也点不到切换。
// UIFIX-8 把它提到 TaskStatusPanel 的 sticky 表头里，与任务身份同屏常驻。
//
// **职责边界**：这里只管「用户选了哪一档」与计数角标。降级判定（轨迹为空回退档 1）
// 仍留在 TaskTranscript 的 `mode`——那个才是「实际渲染哪一档」，两者刻意不同源：
// 回退若写回选择，持久化的档位会在空轨迹任务上被洗掉。
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import {
  countTrace,
  formatTraceCounts,
  TRANSCRIPT_MODE_LABELS,
  TRANSCRIPT_MODES,
  TRANSCRIPT_MODE_TITLES,
  type TranscriptMode,
} from '../../utils/taskTranscript';

const store = useAppStore();
const { taskTrace, taskTranscriptMode } = storeToRefs(store);

/** 用户选择的档位（分段控件高亮用；不跟着降级走） */
const requestedMode = computed<TranscriptMode>(() => taskTranscriptMode.value ?? 'full');

/** 全量角标（一行总览：思考 N · 工具 N · bash N） */
const totalBadge = computed(() => formatTraceCounts(countTrace(taskTrace.value)));
</script>

<template>
  <div class="sv-transcript-bar">
    <div class="sv-transcript-modes" role="group" aria-label="执行过程显示档位">
      <button
        v-for="m in TRANSCRIPT_MODES"
        :key="m"
        type="button"
        class="sv-transcript-mode"
        :class="{ active: requestedMode === m }"
        :aria-pressed="requestedMode === m"
        :title="TRANSCRIPT_MODE_TITLES[m]"
        @click="store.setTaskTranscriptMode(m)"
      >
        {{ TRANSCRIPT_MODE_LABELS[m] }}
      </button>
    </div>
    <span v-if="totalBadge" class="sv-transcript-badge sv-tnum">{{ totalBadge }}</span>
  </div>
</template>

<style scoped>
/* 档位条：与任务台既有控件同高（VIS 纪律：移动端 44px 触达，桌面 32px）。
   外层不再自带下边距——它住在 sticky 表头里，间距归表头的 padding。 */
.sv-transcript-bar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
}
.sv-transcript-modes {
  display: inline-flex;
  border: var(--bw-thin) solid var(--sv-line-strong);
  background: var(--sv-white);
}
.sv-transcript-mode {
  padding: var(--space-1-5) var(--space-3);
  border: 0;
  background: none;
  color: var(--sv-ink-dim);
  font-size: var(--text-xs);
  font-weight: 700;
  cursor: pointer;
  transition: background var(--dur-fast) var(--ease-standard), color var(--dur-fast) var(--ease-standard);
}
.sv-transcript-mode + .sv-transcript-mode { border-left: var(--bw-thin) solid var(--sv-line); }
.sv-transcript-mode:hover { background: var(--sv-pink-light); }
.sv-transcript-mode.active { background: var(--sv-ink); color: var(--sv-white); }
.sv-transcript-badge {
  margin-left: auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--text-xs);
  color: var(--sv-ink-faint);
}
</style>
