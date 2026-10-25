<script setup lang="ts">
// 任务详情区(任务模式):当前任务的计划步骤 / 子任务执行 / 最终成果。
// 下达目标与任务历史已移入左侧 Sidebar(单列布局);任务数据持久化到后端 SQLite。
//
// C-2A(2026-10-25)拆分后本文件只做壳:顶栏 / 环境层 / 详情加载错误 / 执行中进度行 /
// pending 卡 + 子组件装配。各面板见 components/task/：
//   TaskStatusPanel(头/状态/工作区) · TaskFlowBindPanel(改绑) ·
//   TaskApprovalPanel(批准/规划对话) · TaskExecutionPanel(调用情况/步骤/子任务/流式) ·
//   TaskResultPanel(成果/交付/文件变更/对话/追加) · TaskBoardEmpty(空态)。
import { computed, defineAsyncComponent, onMounted, watch } from 'vue';
import { useAppStore } from '../store';
import { storeToRefs } from 'pinia';
import { useActiveThemeId } from '../composables/useActiveTheme';
import { useTaskActions } from '../composables/useTaskActions';
import type { TaskRunMode } from '../api';
import TaskStatusPanel from './task/TaskStatusPanel.vue';
import TaskFlowBindPanel from './task/TaskFlowBindPanel.vue';
import TaskApprovalPanel from './task/TaskApprovalPanel.vue';
import TaskExecutionPanel from './task/TaskExecutionPanel.vue';
import TaskResultPanel from './task/TaskResultPanel.vue';
import TaskBoardEmpty from './task/TaskBoardEmpty.vue';

/** 环境层懒加载:同 ChatWindow,不进首屏 index chunk。 */
const AmbientField = defineAsyncComponent(() => import('./AmbientField.vue'));

const store = useAppStore();
const activeThemeId = useActiveThemeId();
const { currentTask, currentTaskId, model, agentFlowLibrary } = storeToRefs(store);
const { runCurrent } = useTaskActions();

/** 最近一条执行进展(主/子 agent 状态简述;store 消费 agent_status 事件,不落库) */
const lastAgentStatus = computed(() => store.lastAgentStatus);

/** 当前任务是否在执行中(planning / running) */
const taskRunning = computed(() => {
  const s = currentTask.value?.task.status;
  return s === 'planning' || s === 'running';
});

/** 计划完成度(仅用于执行中的进度轨;终态不画进度)。 */
const taskStepProgress = computed(() => {
  const steps = currentTask.value?.task.plan ?? [];
  const done = steps.filter((s) => s.status === 'done').length;
  const total = steps.length;
  return { done, total, pct: total > 0 ? Math.round((done / total) * 100) : 0 };
});

const taskMode = computed<TaskRunMode>(() => currentTask.value?.task.task_mode ?? 'legacy');

/**
 * 徽标/流程名需要流程库,而任务模式此前不会加载它(只有设置区打开时才拉)。这里在
 * 遇到 custom 任务时惰性拉一次:**已有任务快照就不拉**(快照是自足的数据源,少一次请求,
 * 也不会被「当前库已被改过」误导);拉取失败保持 null(徽标不显示),不影响任务展示本身
 * ——徽标是锦上添花,不能因它让详情区报错。
 *
 * 不用 `watch(immediate: true)`:那会在 SSR 期间就发请求,而本组件的 SSR 冒烟测试
 * 依赖「服务端不发请求」这一既有约定(任务详情只在客户端看)。改为挂载时检查一次 +
 * 之后随任务/模式变化检查——两个入口都只在客户端生效。
 */
function ensureFlowLib(): void {
  if (taskMode.value !== 'custom') return;
  if (currentTask.value?.flow_snapshot) return;
  if (!agentFlowLibrary.value) void store.loadAgentFlow();
}
onMounted(ensureFlowLib);
watch([taskMode, currentTaskId], ensureFlowLib);
</script>

<template>
  <section class="flex min-h-0 flex-1 flex-col">
    <header class="sv-topbar">
      <span class="flex items-center gap-2">
        <span class="sv-supreme md" aria-hidden="true" />
        <span class="sv-topbar-title">任务工作台</span>
        <span v-if="model" class="sv-topbar-sub">{{ model }}</span>
      </span>

      <span class="sv-topbar-right">
        <!-- Agent 面板开关(面板合并后原「调用情况」独立拨杆收编:与聊天顶栏同一入口,两模式共用)
             sv-topbar-agent:移动端隐藏,该入口已由底部导航「AGENT」承载 -->
        <div class="sv-topbar-group sv-topbar-agent">
          <button
            type="button"
            class="sv-render-toggle-v2"
            :aria-pressed="store.agentPanelOpen"
            :title="store.agentPanelOpen ? 'Agent 面板已开启(含调用情况)' : 'Agent 面板已关闭(含调用情况)'"
            @click="store.toggleAgentPanel()"
          >
            <span class="toggle-track" :class="{ on: store.agentPanelOpen }">
              <span class="toggle-thumb" />
            </span>
            <span class="toggle-label">AGENT</span>
          </button>
        </div>
      </span>
    </header>

    <!-- 主体:任务详情占满(新建与历史列表在左侧 Sidebar);环境层在背后。
         彩窗主题改用 App 级整窗环境层,这里不再挂第二张 canvas。 -->
    <div class="sv-task-stage">
      <AmbientField
        v-if="activeThemeId !== 'stained-glass'"
        variant="draft"
        :intensity="currentTask ? 0.14 : 0.8"
        :paused="taskRunning || store.overlayOpen"
      />
      <div class="sv-taskboard">
      <!-- 详情区 -->
      <div class="sv-task-detail">
        <!-- 详情加载失败:切任务失败会回退到上一个任务,错误条必须可见,
             否则用户只看到「点了没反应」。放在 currentTask 判空之外,
             首次选择失败(无旧任务)时也能显示。 -->
        <div v-if="store.taskLoadError" class="sv-task-error" role="alert">
          <span>{{ store.taskLoadError }}</span>
          <button class="sv-btn ghost sv-btn-sm" @click="store.dismissTaskLoadError()">关闭</button>
        </div>
        <template v-if="currentTask">
          <TaskStatusPanel :detail="currentTask" :running="taskRunning" />
          <TaskFlowBindPanel :detail="currentTask" />

          <!-- 执行中进度行(2026-09-18):后端 agent_status 事件本就携带每轮进展简述,
               此前只用于刷详情、不展示——任务长时间执行时界面完全静止,用户无法区分
               「还在跑」与「已挂死」(实跑反馈:空转 70 秒期间界面无任何变化)。
               此处展示最近一条,给出可见的活性证据。事件不落库,故仅执行中显示。 -->
          <div
            v-if="taskRunning && lastAgentStatus"
            class="sv-task-progress-line"
            aria-live="polite"
            title="最近一次执行进展(来自后端事件,事件不落库)"
          >
            <div class="sv-task-progress-head">
              <span class="sv-task-progress-dot" />
              <span class="sv-task-progress-label">执行中</span>
              <span class="sv-task-progress-count sv-tnum">
                {{ taskStepProgress.done }}/{{ taskStepProgress.total }}
              </span>
            </div>
            <div
              class="sv-task-progress-track"
              role="progressbar"
              :aria-valuenow="taskStepProgress.done"
              :aria-valuemin="0"
              :aria-valuemax="taskStepProgress.total || 0"
              aria-label="计划步骤完成度"
            >
              <span :style="{ width: `${taskStepProgress.pct}%` }" />
            </div>
            <p class="sv-task-progress-text">{{ lastAgentStatus }}</p>
          </div>

          <TaskApprovalPanel :detail="currentTask" />
          <TaskExecutionPanel :detail="currentTask" />
          <TaskResultPanel :detail="currentTask" />

          <div v-if="currentTask.task.status === 'pending'" class="sv-task-pending">
            <div>
              <span class="sv-task-pending-kicker">已创建 · 等待执行</span>
              <p>任务已创建，点右上「执行」开始；也可以用下面的按钮立即执行。</p>
            </div>
            <button type="button" class="sv-btn ghost sv-btn-sm" @click="runCurrent">▶ 立即执行</button>
          </div>
        </template>

        <TaskBoardEmpty v-else />
      </div>
    </div>
    </div>
  </section>
</template>

<style scoped>
/* ===== UIQ-4:任务工作台终稿 =====
   环境层在 stage 背后;详情区保持透明,标题/状态矩阵用实色纸面保证可读。
   所有新增规则留在 scoped 块,不增全局 CSS 行数。 */
.sv-task-stage {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
  background-color: var(--sv-paper);
  background-image:
    linear-gradient(to right, color-mix(in srgb, var(--sv-ink) 3%, transparent) 1px, transparent 1px),
    linear-gradient(to bottom, color-mix(in srgb, var(--sv-ink) 3%, transparent) 1px, transparent 1px);
  background-size: 24px 24px;
}
.sv-taskboard { position: relative; z-index: 1; background: transparent; }
.sv-task-detail { padding-top: var(--space-3); }
.sv-task-error { margin: calc(-1 * var(--space-4)) 0 var(--space-5); }

/* 执行中:2px 进度轨 + 当前进展单行 */
.sv-task-progress-line {
  display: block;
  margin: 0 0 var(--space-5);
  padding: var(--space-3) var(--space-3-5);
  border: 1px solid var(--sv-line-strong);
  background: var(--sv-white);
}
.sv-task-progress-head { display: flex; align-items: center; gap: var(--space-2); }
.sv-task-progress-dot {
  flex: none;
  width: 8px;
  height: 8px;
  background: var(--sv-red);
  animation: sv-progress-pulse var(--dur-pulse) var(--ease-in-out) infinite;
}
@keyframes sv-progress-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.3; }
}
.sv-task-progress-label { font-size: var(--text-xs); font-weight: 800; letter-spacing: 0.08em; }
.sv-task-progress-count { margin-left: auto; font-size: var(--text-xs); color: var(--sv-ink-faint); }
.sv-task-progress-track {
  height: 2px;
  margin: var(--space-2-5) 0 var(--space-2);
  background: var(--sv-line);
  overflow: hidden;
}
.sv-task-progress-track span {
  display: block;
  height: 100%;
  background: var(--sv-red);
  transition: width var(--dur-slow) var(--ease-out);
}
.sv-task-progress-text {
  margin: 0;
  font-size: var(--text-xs);
  line-height: var(--lh-body);
  color: var(--sv-ink-dim);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* pending:不再是一句灰字 */
.sv-task-pending {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-4) var(--space-5);
  border: var(--bw) solid var(--sv-ink);
  border-left: 4px solid var(--sv-red);
  background: var(--sv-white);
  box-shadow: var(--shadow-card);
}
.sv-task-pending-kicker {
  font-size: var(--text-2xs);
  letter-spacing: 0.16em;
  color: var(--sv-ink-faint);
  text-transform: uppercase;
}
.sv-task-pending p { margin: var(--space-1) 0 0; font-size: var(--text-sm); }

@media (max-width: 767px) {
  .sv-task-detail { padding: var(--space-3); }
  .sv-task-pending { flex-direction: column; align-items: flex-start; }
}
</style>
