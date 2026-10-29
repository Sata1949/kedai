<script setup lang="ts">
// 任务执行区:调用情况入口 / 分工卡 / 计划步骤 / 子任务 / 流式输出
// （C-2A，2026-10-25；原 TaskBoard 1033-1137）。
import { computed, onUnmounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { renderMarkdownCached } from '../../utils/markdownCache';
import { bufferLabel } from '../../utils/phaseLabel';
import { badgeFlowSource, planRowBadges, type NodeBadge } from '../../utils/flowNodeBadges';
import { taskStatusClass as statusClass, taskStatusLabel as statusLabel } from '../../taskStatus';
import { MODE_LABELS } from '../../api/labels';
import type { TaskDetail, TaskRunMode, TaskStep } from '../../api';

const props = defineProps<{
  detail: TaskDetail;
}>();

const store = useAppStore();
const { agentFlowLibrary } = storeToRefs(store);

const taskMode = computed<TaskRunMode>(() => props.detail.task.task_mode ?? 'legacy');
const taskModeLabel = computed(() => MODE_LABELS[taskMode.value]);

/**
 * 计划步骤的 markdown 预渲染(定向响应式,multi/team 卡顿修复):
 * 依赖精确到 plan 引用——store 侧签名去重保证数据未变时引用不换,此处零重算。
 */
interface RenderedStep {
  step: TaskStep;
  html: string;
}
const planRendered = computed<RenderedStep[]>(() =>
  (props.detail.task.plan ?? []).map((s) => ({
    step: s,
    html: s.result ? renderMarkdownCached(s.result) : '',
  })),
);

// ----- 自定义流程的运行态节点徽标(遗留.md IFW-5;二维批次 5a 起读任务快照)-----
/**
 * 逐行徽标(下标与 plan 对齐;空数组 = 该行不显示)。
 * 映射只认 `node_id`:plan 行由**过滤后的启用步骤**构造,plan 下标 ≠ 流程数组下标。
 */
const planBadges = computed<NodeBadge[][]>(() => {
  const rows = props.detail.task.plan ?? [];
  const source = badgeFlowSource(props.detail.flow_snapshot, agentFlowLibrary.value);
  if (taskMode.value !== 'custom' || !source) {
    return rows.map(() => []);
  }
  return rows.map((s) => planRowBadges(source.flows, source.rootId, s.node_id));
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

/** planned 态计划清单已渲染于批准区时隐藏步骤区(视觉去重;与批准区同口径) */
const plannedPlanHtml = computed(() => {
  if (props.detail.task.status !== 'planned' || taskMode.value !== 'plan') return '';
  const result = props.detail.task.result ?? '';
  return result ? renderMarkdownCached(result) : '';
});

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

// ----- 批次 R4 流式输出:「正在生成」块 -----
/**
 * 流式缓冲原文(全部活跃调用的攒批增量;单缓冲纯文本,多缓冲(team 并行/子图)带 key 前缀区分)。
 * 多缓冲的 key 是**内部键**,统一走 utils/phaseLabel 的中文标签;step 阶段能对上 plan 时补步骤名。
 */
const liveRaw = computed(() => {
  const entries = [...store.liveBuffers.entries()];
  if (entries.length === 1) return entries[0][1];
  const plan = props.detail.task.plan ?? [];
  const stepName = (i: number) => plan[i]?.name || null;
  return entries.map(([k, t]) => `【${bufferLabel(k, stepName)}】\n${t}`).join('\n\n');
});

/** 节流展示文本(100ms):流式期间绝不跑 renderMarkdown(防每 token 重跑)。 */
const liveText = ref(liveRaw.value);
let liveTimer: ReturnType<typeof setTimeout> | null = null;
watch(liveRaw, (v) => {
  if (liveTimer) return; // 冷却中:到点后统一对齐最新值
  liveText.value = v; // 首帧/冷却外立即同步
  liveTimer = setTimeout(() => {
    liveTimer = null;
    if (liveRaw.value !== liveText.value) liveText.value = liveRaw.value;
  }, 100);
});
onUnmounted(() => {
  if (liveTimer) clearTimeout(liveTimer);
});
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

  <!-- team 模式:分工卡(步骤名「【主Agent-N】」前缀分组;契约见批次 4) -->
  <div v-if="taskMode === 'team' && detail.task.plan.length" class="sv-task-section">
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

  <!-- 计划步骤(legacy / solo / multi / plan;custom 为流程步骤进度,步骤 + 状态徽标)。
       批次 R1:planned 态且 result 计划清单已渲染于批准区时隐藏本区(视觉去重) -->
  <div v-else-if="detail.task.plan.length && !plannedPlanHtml" class="sv-task-section">
    <div class="sv-task-section-title">{{ taskMode === 'custom' ? '流程步骤进度' : '计划步骤' }}</div>
    <div
      v-for="(rs, i) in planRendered"
      :key="i"
      class="sv-task-step"
      :class="statusClass(rs.step.status)"
    >
      <span class="sv-task-step-idx" :class="statusClass(rs.step.status)">{{ i + 1 }}</span>
      <div class="sv-task-step-body">
        <div class="sv-task-step-name">
          {{ rs.step.name }}
          <span class="sv-tag sm" :class="statusClass(rs.step.status)">{{ statusLabel(rs.step.status) }}</span>
          <!-- 编排徽标(遗留.md IFW-5):按 node_id 对回流程节点,显示层级/档位/子流程/成果。
               对不上就不显示(旧任务、节点已删、流程库没加载) -->
          <span
            v-for="(badge, bi) in planBadges[i]"
            :key="bi"
            class="sv-tag sm flow-node-tag"
            :class="badge.kind"
          >{{ badge.text }}</span>
        </div>
        <!-- 步骤目标(plan 模式待批准清单的完整内容:名称+目标;
             批准后/完成后同区持续显示,状态徽标与 result 随 SSE 刷新实时反映) -->
        <div v-if="rs.step.goal" class="sv-task-step-goal">{{ rs.step.goal }}</div>
        <div v-if="rs.step.result" class="sv-task-step-result" v-html="rs.html" />
      </div>
    </div>
  </div>

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

  <!-- 正在生成(批次 R4 流式输出):流式期间纯文本 + 光标(不跑 renderMarkdown);
       调用落库后 store 清缓冲、本块消失,权威文本由最终成果/步骤区 markdown 接管 -->
  <div v-if="liveText" class="sv-task-section sv-task-live">
    <div class="sv-task-section-title">正在生成</div>
    <div class="sv-task-live-text sv-stream-cursor">{{ liveText }}</div>
  </div>
</template>

<style scoped>
/* 步骤时间线:更松的行距、更清楚的数字锚点 */
.sv-task-step {
  padding: var(--space-3-5) var(--space-4);
  margin-bottom: var(--space-2);
  border: 1px solid var(--sv-line-strong);
  background: var(--sv-white);
}
.sv-task-step-name { font-size: var(--text-base); }
.sv-task-step-goal { font-size: var(--text-xs); line-height: var(--lh-body); color: var(--sv-ink-dim); }
.sv-task-step-result { font-size: var(--text-sm); line-height: var(--lh-prose); }
.sv-task-step-idx { font-size: var(--text-xs); }
.sv-task-step-idx.active { background: var(--sv-red); color: var(--sv-white); }
.sv-task-subtask {
  padding: var(--space-2-5) var(--space-3);
  border-left: 2px solid var(--sv-line-strong);
  background: var(--sv-surface-elevated);
}
/* 子任务错误沿用原 TaskBoard 的紧凑间距（与状态区错误同一口径） */
.sv-task-error { margin: calc(-1 * var(--space-4)) 0 var(--space-5); }

/* 自定义流程的运行态节点徽标(遗留.md IFW-5):与画布节点卡片的 .flow-tag 同一套
   配色语义(层级灰、严格虚线、子流程实底粉、成果粉线、停用淡),但类名独立——
   那边是 scoped 到 AgentFlowNodeCard 的,跨组件复用会被样式隔离挡掉。 */
.sv-task-step-name .flow-node-tag {
  margin-left: 2px;
}
.flow-node-tag.level {
  border-color: transparent;
  color: var(--sv-ink-faint);
}
.flow-node-tag.strict {
  border-style: dashed;
  color: var(--sv-ink-dim);
}
.flow-node-tag.sub {
  border-color: var(--sv-pink-dark);
  background: var(--sv-pink-light);
  color: var(--sv-ink);
}
.flow-node-tag.out {
  border-color: var(--sv-pink-dark);
  color: var(--sv-pink-dark);
  font-weight: 700;
}
.flow-node-tag.off {
  border-color: var(--sv-ink-faint);
  color: var(--sv-ink-faint);
}
</style>
