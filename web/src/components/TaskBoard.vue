<script setup lang="ts">
// 任务详情区(任务模式):当前任务的计划步骤 / 子任务执行 / 最终成果。
// 下达目标与任务历史已移入左侧 Sidebar(单列布局);任务数据持久化到后端 SQLite。
import { computed } from 'vue';
import { useAppStore } from '../store';
import { storeToRefs } from 'pinia';
import { renderMarkdown } from '../markdown';
import { taskStatusClass as statusClass, taskStatusLabel as statusLabel } from '../taskStatus';
import type { TaskRecord } from '../api';

const store = useAppStore();
const { currentTask, currentTaskId, model } = storeToRefs(store);

/** 当前任务是否在执行中(planning / running) */
const taskRunning = computed(() => {
  const s = currentTask.value?.task.status;
  return s === 'planning' || s === 'running';
});

/** 执行当前任务 */
async function runCurrent(): Promise<void> {
  const id = currentTaskId.value;
  if (!id) return;
  try {
    await store.runTask(id);
  } catch (err) {
    alert(`执行失败:${(err as Error).message}`);
  }
}

/** 停止当前任务 */
async function stopCurrent(): Promise<void> {
  const id = currentTaskId.value;
  if (!id) return;
  try {
    await store.stopTask(id);
  } catch (err) {
    alert(`停止失败:${(err as Error).message}`);
  }
}

/** 删除任务 */
async function removeTask(task: TaskRecord): Promise<void> {
  if (!confirm(`确定删除任务「${task.title}」?其子任务将一并删除。`)) return;
  try {
    await store.deleteTask(task.id);
  } catch (err) {
    alert(`删除失败:${(err as Error).message}`);
  }
}
</script>

<template>
  <section class="flex min-h-0 min-w-0 flex-1 flex-col">
    <!-- 顶栏 -->
    <header class="sv-topbar">
      <span class="flex items-center gap-2">
        <span class="sv-supreme" style="width: 12px; height: 12px" aria-hidden="true" />
        <span class="sv-topbar-title">任务工作台</span>
        <span v-if="model" class="sv-topbar-sub">{{ model }}</span>
      </span>
    </header>

    <!-- 主体:任务详情占满(新建与历史列表在左侧 Sidebar) -->
    <div class="sv-taskboard">
      <!-- 详情区 -->
      <div class="sv-task-detail">
        <template v-if="currentTask">
          <div class="sv-task-head">
            <div class="sv-task-head-title">
              <span class="sv-supreme pink-deep" style="width: 14px; height: 14px" />
              {{ currentTask.task.title }}
            </div>
            <div class="sv-task-head-actions">
              <button
                v-if="!taskRunning"
                class="sv-btn primary sv-btn-sm"
                title="执行(重新)此任务"
                @click="runCurrent"
              >▶ 执行</button>
              <button
                v-else
                class="sv-btn sv-btn-sm"
                style="background: var(--sv-red); border-color: var(--sv-red); color: #fff"
                title="停止执行"
                @click="stopCurrent"
              >■ 停止</button>
              <button
                class="sv-btn ghost sv-btn-sm"
                title="删除任务"
                @click="removeTask(currentTask.task)"
              >删除</button>
            </div>
          </div>

          <!-- 状态 + 错误 -->
          <div class="sv-task-status-line">
            <span class="sv-tag" :class="statusClass(currentTask.task.status)">
              {{ statusLabel(currentTask.task.status) }}
            </span>
            <span v-if="currentTask.task.error" class="sv-task-error">{{ currentTask.task.error }}</span>
          </div>

          <!-- 计划步骤 -->
          <div v-if="currentTask.task.plan.length" class="sv-task-section">
            <div class="sv-task-section-title">计划步骤</div>
            <div
              v-for="(s, i) in currentTask.task.plan"
              :key="i"
              class="sv-task-step"
              :class="statusClass(s.status)"
            >
              <span class="sv-task-step-idx" :class="statusClass(s.status)">{{ i + 1 }}</span>
              <div class="sv-task-step-body">
                <div class="sv-task-step-name">
                  {{ s.name }}
                  <span class="sv-tag sm" :class="statusClass(s.status)">{{ statusLabel(s.status) }}</span>
                </div>
                <div v-if="s.result" class="sv-task-step-result" v-html="renderMarkdown(s.result)" />
              </div>
            </div>
          </div>

          <!-- 子任务 -->
          <div v-if="currentTask.subtasks.length" class="sv-task-section">
            <div class="sv-task-section-title">子任务执行</div>
            <div
              v-for="st in currentTask.subtasks"
              :key="st.id"
              class="sv-task-subtask"
            >
              <span class="sv-supreme" :class="statusClass(st.status)" style="width: 9px; height: 9px" />
              <div class="sv-task-subtask-body">
                <div class="sv-task-subtask-name">
                  {{ st.name }}
                  <span class="sv-tag sm" :class="statusClass(st.status)">{{ statusLabel(st.status) }}</span>
                </div>
                <div v-if="st.error" class="sv-task-error">{{ st.error }}</div>
              </div>
            </div>
          </div>

          <!-- 最终结果 -->
          <div v-if="currentTask.task.result" class="sv-task-section">
            <div class="sv-task-section-title">最终成果</div>
            <div class="sv-task-result" v-html="renderMarkdown(currentTask.task.result)" />
          </div>

          <div v-if="currentTask.task.status === 'pending'" class="sv-task-empty" style="margin-top: 16px">
            <p style="font-size: 12px">任务已创建,点击右上「执行」开始</p>
          </div>
        </template>

        <div v-else class="sv-task-empty" style="height: 100%">
          <div class="sv-empty-geo mb14">
            <span class="sq black" style="width: 22px; height: 22px" />
            <span class="sq pink" />
            <span class="sq deep" />
          <i class="diag" />
          </div>
          <p style="font-size: 14px">选择或创建一个任务</p>
          <p style="font-size: 12px">在左侧栏输入目标,系统会拆解计划、派子智能体执行并汇总结果</p>
        </div>
      </div>
    </div>
  </section>
</template>
