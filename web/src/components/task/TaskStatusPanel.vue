<script setup lang="ts">
// 任务头 / 状态矩阵 / 工作区与画像（C-2A，2026-10-25；原 TaskBoard 733-819）。
//
// 只做展示与"当前任务"级操作（执行/停止/删除）；数据由父组件传入详情，
// 执行动作统一走 useTaskActions（与 Sidebar 任务历史共用）。
import { computed, ref } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { useTaskActions } from '../../composables/useTaskActions';
import { taskStatusClass as statusClass, taskStatusLabel as statusLabel } from '../../taskStatus';
import { badgeFlowSource } from '../../utils/flowNodeBadges';
import { callableFlows } from '../../utils/flowCallStats';
import { projectHintTitle, projectKindLabel } from '../../utils/workspaceProfile';
import { FLOW_MODE_LABELS, MODE_LABELS } from '../../api/labels';
import type { TaskDetail, TaskRunMode } from '../../api';

const props = defineProps<{
  /** 当前任务详情（父级已做 v-if 判空） */
  detail: TaskDetail;
  /** planning / running */
  running: boolean;
}>();

const store = useAppStore();
const { currentTaskUsage, agentFlowLibrary } = storeToRefs(store);
const { runCurrent, stopCurrent, removeTask } = useTaskActions();

const task = computed(() => props.detail.task);
const taskMode = computed<TaskRunMode>(() => task.value.task_mode ?? 'legacy');
const taskModeLabel = computed(() => MODE_LABELS[taskMode.value]);

/** 当前任务累计 token(prompt + completion;由任务详情 usage_total 带出) */
const taskTotalTokens = computed(() => {
  const u = currentTaskUsage.value;
  return u ? u.prompt_tokens + u.completion_tokens : 0;
});

/**
 * 当前任务用的流程名(二维批次 5a):绑定流程后显示其名称,未绑定显示「跟随当前流程」。
 * 流程名从**快照**里查(与徽标同一份编排),快照缺失时才用当前库/流程 id 兜底。
 */
const taskFlowLabel = computed<string | null>(() => {
  if (taskMode.value !== 'custom') return null;
  const boundId = task.value.flow_id;
  if (!boundId) return '跟随当前流程';
  const source = badgeFlowSource(props.detail.flow_snapshot, agentFlowLibrary.value);
  const name = source?.flows.find((f) => f.id === boundId)?.name;
  return name && name.trim() ? name : boundId;
});

/**
 * 对比模式的实际可调用集(二维批次 7b 收口):名单 ∩ 冻结闭包 − 根流程,**与后端运行期
 * 取用同一口径**(`utils/flowCallStats::callableFlows`,对齐 `flow_call::callable_ids`)。
 */
const compareCallable = computed(() => {
  if (taskMode.value !== 'custom') return null;
  const ids = task.value.flow_ids ?? [];
  if (ids.length === 0) return null;
  const source = badgeFlowSource(props.detail.flow_snapshot, agentFlowLibrary.value);
  return source ? callableFlows(ids, source.flows, source.rootId) : null;
});

/** 对比模式徽标:有快照按实际可调用集显示，否则回退名单条数。 */
const taskCompareLabel = computed<string | null>(() => {
  if (taskMode.value !== 'custom') return null;
  const ids = task.value.flow_ids ?? [];
  if (ids.length === 0) return null;
  const declared = new Set(ids.map((s) => s.trim()).filter((s) => s !== '')).size;
  const callable = compareCallable.value;
  const head = FLOW_MODE_LABELS.compare;
  if (callable === null) return `${head} · 可调用 ${declared} 个流程`;
  const base = `${head} · 可调用 ${callable.length} 个流程`;
  const missing = declared - callable.length;
  return missing > 0 ? `${base}(名单 ${declared} 个,${missing} 个本轮不可用)` : base;
});

/** 对比模式徽标的悬停说明:列出本轮**实际**可调用流程名(取不到名则退回 id) */
const taskCompareTitle = computed<string>(() => {
  const head =
    `${FLOW_MODE_LABELS.compare}模式:根流程照常执行,名单内流程作为工具释放给流程节点,由模型在工具循环里自主调用并取回成果`;
  const callable = compareCallable.value;
  if (!callable || callable.length === 0) return head;
  const names = callable
    .slice(0, 6)
    .map((f) => f.name?.trim() || f.id || '')
    .filter((n) => n !== '');
  const more = callable.length > names.length ? ` 等 ${callable.length} 个` : '';
  return `${head}。本轮可调用:${names.join('、')}${more}`;
});

/** 任务工作区(CODE-1):创建期冻结的 canonical 绝对路径;未绑定为 null/空 → 该行不渲染 */
const taskWorkspace = computed(() => task.value.workspace ?? '');
/** 工作区画像(CODE-4):后端在详情读取时实时探测;未绑定/旧服务端为 null → 该行不渲染 */
const taskWorkspaceProfile = computed(() => props.detail.workspace_profile ?? null);
/** 复制成功的短暂回执(1.5s 自动复位;失败保持原文案,不虚报「已复制」) */
const workspaceCopied = ref(false);
async function copyWorkspacePath(): Promise<void> {
  const p = taskWorkspace.value;
  if (!p) return;
  try {
    await navigator.clipboard.writeText(p);
    workspaceCopied.value = true;
    setTimeout(() => (workspaceCopied.value = false), 1500);
  } catch {
    // 剪贴板不可用(权限/非安全上下文):不做任何声称成功的改文案
  }
}
</script>

<template>
  <div class="sv-task-head">
    <div class="sv-task-head-copy">
      <span class="sv-task-head-kicker">任务 / TASK</span>
      <div class="sv-task-head-title">
        <span class="sv-supreme pink-deep" />
        {{ task.title }}
      </div>
    </div>
    <div class="sv-task-head-actions">
      <button
        v-if="!running"
        class="sv-btn primary sv-btn-sm"
        title="执行(重新)此任务"
        @click="runCurrent"
      >▶ 执行</button>
      <button
        v-else
        class="sv-btn sv-btn-sm stop"
        title="停止执行"
        @click="stopCurrent"
      >■ 停止</button>
      <button
        class="sv-btn ghost sv-btn-sm"
        title="删除任务"
        @click="removeTask(task)"
      >删除</button>
    </div>
  </div>

  <!-- 状态矩阵:状态 / 模式 / 流程 / 累计 token;错误单独成行,不混进矩阵 -->
  <div class="sv-task-status-line">
    <span class="sv-tag sv-task-status-tag" :class="statusClass(task.status)">
      状态:{{ statusLabel(task.status) }}
    </span>
    <span class="sv-tag sv-task-status-tag" title="任务执行模式(批次 4 六模式)">模式:{{ taskModeLabel }}</span>
    <!-- 本任务用的流程(二维批次 5a):绑定流程显示其名,未绑定显示「跟随当前流程」。
         名字取自任务快照(与徽标同一份编排),故跑完任务后再改流程这里也不会变 -->
    <span
      v-if="taskFlowLabel"
      class="sv-tag sv-task-status-tag"
      title="本任务绑定/使用的流程(绑定即冻结:创建时的编排;未绑定 = 跟随当前流程)"
    >流程:{{ taskFlowLabel }}</span>
    <span
      v-else
      class="sv-tag sv-task-status-tag"
      title="非自定义流程任务按执行模式自身的编排运行"
    >流程:按模式编排</span>
    <!-- 对比模式(二维批次 7b):根流程照常执行,名单内流程额外作为 run_flow 工具
         释放给宽松节点,由模型自主调用、取回成果。数字按**本轮实际**可调用集显示 -->
    <span
      v-if="taskCompareLabel"
      class="sv-tag sv-task-status-tag"
      :title="taskCompareTitle"
    >{{ taskCompareLabel }}</span>
    <span class="sv-task-usage sv-task-status-tag">
      累计 token: <b class="sv-tnum">{{ taskTotalTokens.toLocaleString() }}</b>
    </span>
  </div>
  <div v-if="task.error" class="sv-task-error" role="alert">{{ task.error }}</div>

  <!-- 工作区(CODE-1):创建期冻结;未绑定不显示该行(不把「没有」渲染成一行噪音) -->
  <div v-if="taskWorkspace" class="sv-task-workspace">
    <span class="sv-note">工作区:</span>
    <span class="sv-task-workspace-path" :title="taskWorkspace">{{ taskWorkspace }}</span>
    <button
      class="sv-btn ghost sv-btn-sm"
      title="复制工作区路径"
      @click="copyWorkspacePath"
    >{{ workspaceCopied ? '已复制' : '复制' }}</button>
  </div>
  <!-- 工作区画像(CODE-4):只读展示,后端实时探测;未命中不渲染 -->
  <div
    v-if="taskWorkspaceProfile && taskWorkspaceProfile.detected.length > 0"
    class="sv-task-workspace-profile"
  >
    <span
      v-for="h in taskWorkspaceProfile.detected"
      :key="`${h.kind}:${h.marker}`"
      class="sv-tag sv-tag-muted"
      :title="projectHintTitle(h.kind, h.marker, h.suggested_command)"
    >{{ projectKindLabel(h.kind) }} · {{ h.suggested_command }}</span>
    <span
      v-if="taskWorkspaceProfile.truncated"
      class="sv-note"
      title="子目录过多,仅扫描了前若干个子目录——结果可能不全"
    >(未扫全)</span>
  </div>
</template>

<style scoped>
/* 工作区行(CODE-1):状态行下方一行;长路径单行省略(完整值在 title 与复制里),
   复制按钮固定宽度不参与压缩。样式纪律同下:只写 scoped、只复用既有令牌。 */
.sv-task-workspace {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin: 0 0 var(--space-5);
}
.sv-task-workspace .sv-btn {
  flex: none;
}
.sv-task-workspace-path {
  flex: 1; /* UIP-10:与「复制」键同行时吃掉余量,长路径走省略号而不顶出按钮 */
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  color: var(--sv-ink-soft, #666);
}

/* 工作区画像行(CODE-4):与工作区行同段、更轻;chips 由全局 .sv-tag 提供外观 */
.sv-task-workspace-profile {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-1-5);
  margin: calc(-1 * var(--space-3-5)) 0 var(--space-5);
}

/* 标题:sticky,滚动时仍能看到任务身份与主操作 */
.sv-task-head {
  position: sticky;
  top: 0;
  z-index: 2;
  align-items: flex-start;
  margin-bottom: var(--space-2);
  padding: var(--space-2) 0 var(--space-3);
  border-bottom: 1px solid var(--sv-line);
  background: var(--sv-paper);
}
.sv-task-head-copy { min-width: 0; }
.sv-task-head-kicker {
  display: block;
  font-size: var(--text-2xs);
  letter-spacing: 0.18em;
  color: var(--sv-ink-faint);
  text-transform: uppercase;
}
.sv-task-head-title {
  align-items: flex-start;
  margin-top: var(--space-1);
  font-size: clamp(22px, 2.4vw, 28px);
  line-height: 1.2;
  letter-spacing: -0.01em;
}
.sv-task-head-title .sv-supreme { flex: none; margin-top: var(--space-1-5); }
.sv-task-head-actions .sv-btn { min-height: var(--control-h); }

/* 状态矩阵:一格一项,发丝分隔;状态色只改文字/左侧记号,不画彩色盒子 */
.sv-task-status-line {
  display: flex;
  flex-wrap: wrap;
  align-items: stretch;
  gap: 0;
  margin-bottom: var(--space-6);
  border-top: 1px solid var(--sv-line-strong);
  border-bottom: 1px solid var(--sv-line-strong);
}
.sv-task-status-tag {
  display: flex;
  align-items: center;
  min-height: 38px;
  margin: 0;
  padding: 0 var(--space-3);
  border: none;
  border-right: 1px solid var(--sv-line);
  background: transparent;
  font-size: var(--text-xs);
  font-weight: 700;
  letter-spacing: 0;
}
.sv-task-status-tag:last-child { border-right: none; }
.sv-task-status-tag:first-child { box-shadow: inset 3px 0 0 currentColor; }
.sv-task-usage b { font-weight: 800; color: var(--sv-ink); }
.sv-task-error { margin: calc(-1 * var(--space-4)) 0 var(--space-5); }

@media (max-width: 767px) {
  .sv-task-head { padding-top: var(--space-1); }
  .sv-task-status-tag { min-height: 34px; padding: 0 var(--space-2); }
}
</style>
