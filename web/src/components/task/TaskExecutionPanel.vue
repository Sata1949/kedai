<script setup lang="ts">
// 任务执行区:调用情况入口 / 分工卡 / 执行过程(TaskTranscript) / 子任务。
// （C-2A，2026-10-25 原 TaskBoard 拆分；TMSHOW-2 起步骤区与三档显示交给 TaskTranscript）
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { renderMarkdownCached } from '../../utils/markdownCache';
import { taskStatusClass as statusClass, taskStatusLabel as statusLabel } from '../../taskStatus';
import { MODE_LABELS } from '../../api/labels';
import TaskTranscript from './TaskTranscript.vue';
import type { TaskDetail, TaskRunMode, TaskStep } from '../../api';

const props = defineProps<{
  detail: TaskDetail;
}>();

const store = useAppStore();
const { taskTranscriptMode, taskTrace } = storeToRefs(store);

const taskMode = computed<TaskRunMode>(() => props.detail.task.task_mode ?? 'legacy');
const taskModeLabel = computed(() => MODE_LABELS[taskMode.value]);

/**
 * 是否处于「步骤视图」(档 1)。
 *
 * **必须同时看轨迹是否为空**:默认档是 `full`,但旧任务(或本次运行没有工具循环)
 * 的轨迹是空的——那时 `TaskTranscript` 会降级成档 1 布局,分工卡也必须跟着回来,
 * 否则 team 旧任务会既没有时间线也没有分工卡,只剩一片「没有可展示的执行轨迹」。
 * 判定与 `TaskTranscript` 内的 `degraded` 同式(两处必须一致)。
 */
const stepViewActive = computed(() => {
  const requested = taskTranscriptMode.value ?? 'full';
  if (requested === 'legacy') return true;
  return taskTrace.value.length === 0;
});

// ----- team 模式:分工卡(按步骤名「【主Agent-N】」前缀分组;契约见批次 4) -----
/** 分工前缀捕获:【主Agent-N】子目标名 */
const TEAM_PREFIX_RE = /^【(主Agent-\d+)】\s*/;

interface TeamGroup {
  /** 分工标签(如「主Agent-1」;空前缀步骤归为 '' 未分工组) */
  agent: string;
  steps: Array<{ step: TaskStep; index: number }>;
}

const teamGroups = computed<TeamGroup[]>(() => {
  const plan = props.detail.task.plan ?? [];
  const groups: TeamGroup[] = [];
  const byAgent = new Map<string, TeamGroup>();
  const unassigned: TeamGroup = { agent: '', steps: [] };
  plan.forEach((step, index) => {
    const m = TEAM_PREFIX_RE.exec(step.name);
    if (!m) {
      unassigned.steps.push({ step, index });
      return;
    }
    let g = byAgent.get(m[1]);
    if (!g) {
      g = { agent: m[1], steps: [] };
      byAgent.set(m[1], g);
      groups.push(g);
    }
    g.steps.push({ step, index });
  });
  if (unassigned.steps.length) groups.push(unassigned);
  return groups;
});

/** 分工卡的 markdown 预渲染(同 planRendered 的缓存口径) */
interface RenderedTeamGroup {
  agent: string;
  steps: Array<{ step: TaskStep; index: number; html: string }>;
}
const teamGroupsRendered = computed<RenderedTeamGroup[]>(() =>
  teamGroups.value.map((g) => ({
    agent: g.agent,
    steps: g.steps.map((it) => ({
      step: it.step,
      index: it.index,
      html: it.step.result ? renderMarkdownCached(it.step.result) : '',
    })),
  })),
);

/** 去掉分工前缀后的步骤名(卡内不再重复显示前缀) */
function teamStepName(name: string): string {
  return name.replace(TEAM_PREFIX_RE, '');
}

/**
 * 「查看调用情况」入口的说明文案(收口批 2026-09-24:custom 此前没有这个入口)。
 * 面板本身与模式无关(通往 Agent 面板的「调用情况」tab);差别只在**细节是什么**。
 */
const callTraceHint = computed(() =>
  taskMode.value === 'custom'
    ? `${taskModeLabel.value}模式的执行细节(各流程节点调用,对比模式下含被调流程)请查看 Agent 面板「调用情况」`
    : `${taskModeLabel.value}模式的执行细节(主/子 Agent 调用)请查看 Agent 面板「调用情况」`,
);

/** solo/multi/custom 模式入口:打开 Agent 合并面板并落在「调用情况」tab */
function openCallTrace(): void {
  store.callTraceOpen = true; // tab 记忆指向「调用情况」
  store.openAgentPanel();     // 用户显式展开:清除自动展开抑制
}
</script>

<template>
  <!-- solo / multi / custom 模式:执行细节在 Agent 面板「调用情况」tab -->
  <div
    v-if="taskMode === 'solo' || taskMode === 'multi' || taskMode === 'custom'"
    class="sv-task-hint"
  >
    <span class="sv-task-hint-text">
      {{ callTraceHint }}
    </span>
    <button class="sv-btn ghost sv-btn-sm" @click="openCallTrace">查看调用情况</button>
  </div>

  <!-- team 模式分工卡:只在步骤视图(档 1)显示。
       档 2/3 下主 Agent 的分工过程由轨迹时间线按 phase=agent 呈现,再叠一张卡是重复。 -->
  <div
    v-if="taskMode === 'team' && detail.task.plan.length && stepViewActive"
    class="sv-task-section"
  >
    <div class="sv-task-section-title">主 Agent 分工</div>
    <div
      v-for="g in teamGroupsRendered"
      :key="g.agent || 'unassigned'"
      class="sv-task-team-card"
    >
      <div class="sv-task-team-agent">
        <span class="sv-supreme xs" />
        {{ g.agent ? `【${g.agent}】` : '未分工' }}
      </div>
      <div
        v-for="item in g.steps"
        :key="item.index"
        class="sv-task-step"
        :class="statusClass(item.step.status)"
      >
        <span class="sv-task-step-idx" :class="statusClass(item.step.status)">{{ item.index + 1 }}</span>
        <div class="sv-task-step-body">
          <div class="sv-task-step-name">
            {{ teamStepName(item.step.name) }}
            <span class="sv-tag sm" :class="statusClass(item.step.status)">{{ statusLabel(item.step.status) }}</span>
          </div>
          <div v-if="item.step.result" class="sv-task-step-result" v-html="item.html" />
        </div>
      </div>
    </div>
  </div>

  <!-- 执行过程:三档显示(档 1 = 原「计划步骤」/「流程步骤进度」区;档 2/3 = 轨迹时间线)。
       原此处的内联步骤模板已整体迁入 TaskTranscript(含 planned 态视觉去重与编排徽标)。 -->
  <TaskTranscript :detail="detail" />

  <!-- 子任务(multi 子 agent 记录)。legacy 不渲染:与「计划步骤」同源重复 -->
  <div
    v-if="detail.subtasks.length && taskMode !== 'legacy'"
    class="sv-task-section"
  >
    <div class="sv-task-section-title">子任务执行</div>
    <div
      v-for="st in detail.subtasks"
      :key="st.id"
      class="sv-task-subtask"
    >
      <span :key="st.status" class="sv-supreme xs sv-dot-flash" :class="statusClass(st.status)" />
      <div class="sv-task-subtask-body">
        <div class="sv-task-subtask-name">
          {{ st.name }}
          <span class="sv-tag sm" :class="statusClass(st.status)">{{ statusLabel(st.status) }}</span>
        </div>
        <div v-if="st.error" class="sv-task-error">{{ st.error }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 子任务错误沿用原 TaskBoard 的紧凑间距（与状态区错误同一口径）。
   步骤卡样式与编排徽标随模板迁入 TaskTranscript 的 scoped 块(CSS 单一所有者);
   全局 styles/task.css 的 .sv-task-step 规则仍在,本组件的分工卡继续复用它。 */
.sv-task-subtask {
  padding: var(--space-2-5) var(--space-3);
  border-left: 2px solid var(--sv-line-strong);
  background: var(--sv-surface-elevated);
}
.sv-task-error { margin: calc(-1 * var(--space-4)) 0 var(--space-5); }
</style>
