<script setup lang="ts">
// 任务模式「工具调用待确认」卡片（2026-11-06 三档授权）。
//
// 为什么必须有它：后端在「逐步」档对写/执行类工具 `begin_wait` 并等
// `tool_authorization_timeout_secs`（默认 300 秒）。没有这张卡，用户选了「逐步」
// 之后只会看到任务静默卡满超时，然后被自动拒绝——档位等于不可用。
//
// 挂载位置：`TaskBoard.vue` 的进度行之后、`TaskApprovalPanel` 之前。
// **不要放进 TaskStatusPanel 的 sticky 表头**——`taskStickyHead.test.ts` 锁死了
// 表头高度上限，多一条队列行会把它顶破。
//
// 归属：队列只服务当前任务（`useTaskToolAuth` 的 ownerTaskId），本组件只负责渲染与
// 提交决议；切任务 / 终态 / 删除后的清空由 store 统一调用 `clearTaskToolAuth`。
import { computed, onMounted, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { TASK_AUTHORIZATION_TIER_LABELS, TOOL_RISK_LABELS } from '../../api/labels';
import {
  decideTaskToolAuth,
  loadTaskToolAuthSnapshot,
  taskToolAuthKey,
  useTaskToolAuth,
  type TaskToolAuthScope,
} from '../../composables/useTaskToolAuth';
import type { TaskToolAuthorization, ToolRisk } from '../../api';

const props = defineProps<{
  /** 当前任务 id（父级已判空） */
  taskId: string;
}>();

const store = useAppStore();
const { toolAuthorizationTimeoutSecs } = storeToRefs(store);
const { queue, ownerTaskId, busy, feedback, loading, error } = useTaskToolAuth();

/** 风险等级 → 徽标配色（沿用任务区 .sv-badge 的语义色：绿=安全 / 黄=敏感 / 红=危险） */
const RISK_CLASS: Record<ToolRisk, string> = {
  safe: 'done',
  sensitive: 'pending',
  dangerous: 'fail',
};

/** 等待时限（后端 30..=1800，缺省 300；未加载设置时按后端缺省显示） */
const timeoutSecs = computed(() => toolAuthorizationTimeoutSecs.value ?? 300);
/** 当前任务的授权档位（决定这张卡为什么出现：逐步 / 越界转确认） */
const tierHint = computed(() => {
  const tier = store.currentTask?.task.tool_authorization;
  return tier ? `本任务授权档位：${TASK_AUTHORIZATION_TIER_LABELS[tier]}` : '';
});

/** 面板是否出现：有待确认、正在拉快照、有错误、有回执——四者任一都要让用户看见 */
const visible = computed(
  () => queue.value.length > 0 || loading.value || error.value !== null || feedback.value !== null,
);

/**
 * 面板标题按内容切换：有卡片时是「待确认」，只剩回执时是「结果」。
 * 队列空着还挂「工具调用待确认」会让用户以为有东西没处理。
 */
const title = computed(() => (queue.value.length > 0 ? '工具调用待确认' : '工具授权结果'));

function riskLabel(risk: ToolRisk): string {
  return TOOL_RISK_LABELS[risk];
}

function riskClass(risk: ToolRisk): string {
  return RISK_CLASS[risk];
}

/** 该条是否正在提交（同一条的另一次点击会被 disabled 挡住） */
function busyScope(item: TaskToolAuthorization): TaskToolAuthScope | undefined {
  return busy.value[taskToolAuthKey(item)];
}

/** 原因码 → 短标注（机器码只做辅助说明，正文 reason 才是权威解释） */
function codeHint(code: string | undefined): string {
  if (code === 'step_confirm') return '逐步档确认';
  if (code === 'workspace_escape') return '越出工作区';
  return '';
}

async function decide(item: TaskToolAuthorization, scope: TaskToolAuthScope): Promise<void> {
  await decideTaskToolAuth(item, scope);
}

function refresh(): void {
  void loadTaskToolAuthSnapshot(props.taskId);
}

// 兜底恢复：面板重新挂载（档位切换重渲染 / 从后台切回）而 store 尚未拉快照时补一次。
// 正常路径由 store.selectTask 拉，ownerTaskId 已就位，这里不会重复请求。
onMounted(() => {
  if (ownerTaskId.value !== props.taskId) void loadTaskToolAuthSnapshot(props.taskId);
});
watch(
  () => props.taskId,
  (id) => {
    if (id && ownerTaskId.value !== id) void loadTaskToolAuthSnapshot(id);
  },
);
</script>

<template>
  <section v-if="visible" class="sv-task-toolauth" aria-live="polite">
    <div class="sv-task-section-title">
      <span>{{ title }}</span>
      <span v-if="queue.length" class="sv-task-toolauth-count">{{ queue.length }} 项</span>
      <button
        v-if="queue.length || error"
        class="sv-btn ghost sv-btn-sm sv-task-toolauth-refresh"
        :disabled="loading"
        title="重新读取本任务当前待确认的调用(断线或超时后可用)"
        @click="refresh"
      >{{ loading ? '刷新中…' : '刷新' }}</button>
    </div>

    <!-- 操作说明只在确实有待确认项时给;只剩回执时这段话是噪音 -->
    <p v-if="queue.length" class="sv-note sv-task-toolauth-hint">
      写文件、改文件与执行命令前的确认，点「仅允许本次」后模型才继续，点「拒绝」会把拒绝结果交回模型。
      每次确认只对这一次调用生效，等待超过 {{ timeoutSecs }} 秒会自动拒绝。
      <template v-if="tierHint">{{ tierHint }}。</template>
    </p>

    <article
      v-for="item in queue"
      :key="taskToolAuthKey(item)"
      class="sv-task-toolauth-card"
    >
      <div class="sv-task-toolauth-head">
        <span class="sv-badge" :class="riskClass(item.risk)">{{ riskLabel(item.risk) }}</span>
        <b class="sv-task-toolauth-name">{{ item.name }}</b>
        <span v-if="codeHint(item.code)" class="sv-tag sv-tag-muted">{{ codeHint(item.code) }}</span>
      </div>
      <p class="sv-task-toolauth-reason">{{ item.reason }}</p>
      <details v-if="item.input_preview" class="sv-task-toolauth-input">
        <summary>查看调用参数</summary>
        <pre>{{ item.input_preview }}</pre>
      </details>
      <div class="sv-task-toolauth-actions">
        <button
          class="sv-btn primary sv-btn-sm"
          :disabled="!!busyScope(item)"
          title="只允许这一次调用，其他同类调用仍会逐条询问"
          @click="decide(item, 'once')"
        >{{ busyScope(item) === 'once' ? '提交中…' : '仅允许本次' }}</button>
        <button
          class="sv-btn ghost sv-btn-sm"
          :disabled="!!busyScope(item)"
          title="拒绝这一次调用，模型会收到拒绝结果并自行调整"
          @click="decide(item, 'deny')"
        >{{ busyScope(item) === 'deny' ? '提交中…' : '拒绝' }}</button>
      </div>
    </article>

    <p v-if="feedback" class="sv-feedback" :class="feedback.kind">{{ feedback.text }}</p>
    <p v-if="error" class="sv-feedback err" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
/* 样式纪律（MAINTENANCE D-5）：新增组件样式一律 scoped，只复用既有令牌与 .sv-* 基础类。
   间距全部走 --space-*（css-discipline 的裸值 ratchet）。 */
.sv-task-toolauth {
  margin: 0 0 var(--space-5);
  padding: var(--space-4) var(--space-5);
  border: var(--bw) solid var(--sv-ink);
  border-left: 4px solid var(--sv-yellow);
  background: var(--sv-white);
  box-shadow: var(--shadow-card);
}
.sv-task-toolauth .sv-task-section-title {
  margin-bottom: var(--space-2);
}
.sv-task-toolauth-count {
  font-size: var(--text-2xs);
  font-weight: 700;
  color: var(--sv-yellow-deep);
}
.sv-task-toolauth-refresh { margin-left: auto; }
.sv-task-toolauth-hint { margin-bottom: var(--space-3); }

.sv-task-toolauth-card {
  margin-top: var(--space-3);
  padding: var(--space-3) var(--space-3-5);
  border: 1px solid var(--sv-line-strong);
  background: var(--sv-surface);
}
.sv-task-toolauth-head {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-2);
}
.sv-task-toolauth-name {
  font-size: var(--text-sm);
  font-family: var(--font-mono);
  overflow-wrap: anywhere;
}
.sv-task-toolauth-reason {
  margin: var(--space-2) 0 0;
  font-size: var(--text-sm);
  line-height: var(--lh-body);
}
.sv-task-toolauth-input { margin-top: var(--space-2); }
.sv-task-toolauth-input summary {
  cursor: pointer;
  font-size: var(--text-xs);
  color: var(--sv-ink-faint);
}
.sv-task-toolauth-input pre {
  max-height: 220px;
  margin: var(--space-2) 0 0;
  padding: var(--space-2-5) var(--space-3);
  overflow: auto;
  border: 1px solid var(--sv-line);
  background: var(--sv-white);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  white-space: pre-wrap;
  word-break: break-word;
}
.sv-task-toolauth-actions {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: var(--space-3);
}

@media (max-width: 767px) {
  .sv-task-toolauth { padding: var(--space-3); }
}
</style>
