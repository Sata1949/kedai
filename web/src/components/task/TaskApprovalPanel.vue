<script setup lang="ts">
// plan 模式批准区 + 规划对话（C-2A，2026-10-25；原 TaskBoard 919-1031）。
//
// 计划待批准时的单一交互面：执行方式、直接批准 / 修改后批准 / 放弃、与规划器对话。
// 状态仍由 store 驱动；切换任务时本地编辑态与草稿随任务 id 复位（原 watch 语义）。
import { computed, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { useTaskActions } from '../../composables/useTaskActions';
import { renderMarkdownCached } from '../../utils/markdownCache';
import MarkdownBlock from './MarkdownBlock.vue';
import { APPROVE_EXEC_MODE_LABELS, APPROVE_EXEC_MODE_ORDER } from '../../api/labels';
import type { TaskApproveExecMode, TaskDetail, TaskRunMode, TaskStep } from '../../api';

const props = defineProps<{
  detail: TaskDetail;
}>();

const store = useAppStore();
const { currentTaskId } = storeToRefs(store);
const { stopCurrent } = useTaskActions();

const taskMode = computed<TaskRunMode>(() => props.detail.task.task_mode ?? 'legacy');
/** 当前任务是否处于计划待批准(plan 模式 run 后的暂停态) */
const taskPlanned = computed(() => props.detail.task.status === 'planned');

/** 计划编辑器开关(「修改后批准」展开) */
const planEditing = ref(false);
/** 编辑中的计划副本(name/goal 可改;status/result 保留原值随提交回传) */
const editedPlan = ref<TaskStep[]>([]);
const approving = ref(false);
/** 本次批准的执行方式(2026-09-17):默认「按计划逐步执行」= 改造前行为。 */
const approveExecMode = ref<TaskApproveExecMode>('approved_plan');

/** 规划对话输入草稿与修订中标记(批次 R2b) */
const planChatDraft = ref('');
const planChatting = ref(false);

/** planned 态:批准区内渲染 result 的计划清单文本(批次 R1:planned 态 result 语义 =
 *  待批准的计划清单,markdown 渲染供批准前审阅)。非空时下方「计划步骤」区隐藏。 */
const plannedPlanHtml = computed(() => {
  if (!taskPlanned.value || taskMode.value !== 'plan') return '';
  const result = props.detail.task.result ?? '';
  return result ? renderMarkdownCached(result) : '';
});

/** 批准环节规划对话记录(plan_chat 消息;批准区内渲染,批次 R2b) */
const planChatMessages = computed(() =>
  (props.detail.messages ?? []).filter((m) => m.kind === 'plan_chat'),
);

/** assistant 消息的 markdown 渲染(含缓存;用户消息直接显示纯文本) */
function renderMessageHtml(m: { content: string }): string {
  return renderMarkdownCached(m.content);
}

// 切换任务时收起编辑态与 pending 操作;规划对话草稿一并清空(批次 R2)
watch(
  () => props.detail.task.id,
  () => {
    planEditing.value = false;
    editedPlan.value = [];
    approveExecMode.value = 'approved_plan';
    planChatDraft.value = '';
  },
);

/** 进入「修改后批准」:复制当前计划为可编辑副本 */
function startEditPlan(): void {
  const plan = props.detail.task.plan ?? [];
  editedPlan.value = plan.map((s) => ({ ...s }));
  planEditing.value = true;
}

/** 批准执行:plan 为空表示按原计划批准;给了 plan 则替换后逐步执行。 */
async function approveCurrent(plan?: TaskStep[]): Promise<void> {
  const id = currentTaskId.value;
  if (!id || approving.value) return;
  approving.value = true;
  try {
    await store.approveTask(id, plan, approveExecMode.value);
    planEditing.value = false;
    editedPlan.value = [];
  } catch (err) {
    alert(`批准失败:${(err as Error).message}`);
  } finally {
    approving.value = false;
  }
}

/** 放弃计划:复用既有停止接口(任务进入已停止终态) */
async function discardPlan(): Promise<void> {
  if (!confirm('确定放弃该计划?任务将停止,不再执行。')) return;
  await stopCurrent();
}

/** 发送规划对话反馈:同步等待规划器修订完成,计划区随详情刷新(R2b) */
async function sendPlanChat(): Promise<void> {
  const id = currentTaskId.value;
  const content = planChatDraft.value.trim();
  if (!id || !content || planChatting.value) return;
  planChatting.value = true;
  try {
    await store.planChatTask(id, content);
    planChatDraft.value = '';
  } catch (err) {
    alert(`修订失败:${(err as Error).message}`);
  } finally {
    planChatting.value = false;
  }
}
</script>

<template>
  <div v-if="taskPlanned" class="sv-task-approve">
    <div class="sv-task-section-title">计划待批准</div>
    <div class="sv-task-approve-head">
      <!-- 执行方式选择(2026-09-17):默认按计划逐步执行;选其它模式则由该模式
           自行组织执行(team/custom 会用自身规划/流程重写计划步骤显示)。
           编辑态下同样可见——「修改后批准」也走这个选择。 -->
      <div class="sv-task-approve-exec">
        <label class="sv-inp-tag">执行方式</label>
        <select v-model="approveExecMode" class="sv-select" title="选择批准后用什么模式执行这份计划">
          <option v-for="m in APPROVE_EXEC_MODE_ORDER" :key="m" :value="m">
            {{ APPROVE_EXEC_MODE_LABELS[m] }}
          </option>
        </select>
      </div>
      <p v-if="approveExecMode !== 'approved_plan'" class="sv-note approve-exec-hint">
        非默认方式将以该模式自行组织执行:已批准计划作为目标上下文下发;
        团队协作与自定义流程会用自己的规划/流程重写计划步骤显示。
      </p>
      <template v-if="!planEditing">
        <div class="sv-task-approve-actions">
          <button
            class="sv-btn primary sv-btn-sm"
            :disabled="approving"
            title="按当前计划开始执行"
            @click="approveCurrent()"
          >✓ 批准执行</button>
          <button
            class="sv-btn ghost sv-btn-sm"
            :disabled="approving || detail.task.plan.length === 0"
            title="修改计划步骤后批准执行"
            @click="startEditPlan"
          >修改后批准</button>
          <button
            class="sv-btn ghost sv-btn-sm"
            :disabled="approving"
            title="放弃该计划,任务停止不再执行"
            @click="discardPlan"
          >放弃</button>
        </div>
      </template>
    </div>
    <!-- 批次 R1:planned 态 result = 待批准的计划清单,批准区内渲染供批准前审阅 -->
    <MarkdownBlock v-if="plannedPlanHtml" :html="plannedPlanHtml" class="sv-task-planned-plan" />
    <template v-if="planEditing">
      <!-- 计划简易编辑:每步 名称/目标 两个输入框(增删步不做,保持最小) -->
      <div v-for="(s, i) in editedPlan" :key="i" class="sv-task-plan-edit">
        <span class="sv-task-step-idx">{{ i + 1 }}</span>
        <div class="sv-task-plan-edit-body">
          <input
            v-model="s.name"
            type="text"
            class="sv-input"
            placeholder="步骤名称"
            spellcheck="false"
          />
          <input
            v-model="s.goal"
            type="text"
            class="sv-input"
            placeholder="步骤目标"
            spellcheck="false"
          />
        </div>
      </div>
      <div class="sv-task-approve-actions">
        <button
          class="sv-btn primary sv-btn-sm"
          :disabled="approving"
          @click="approveCurrent(editedPlan)"
        >✓ 确认并批准执行</button>
        <button
          class="sv-btn ghost sv-btn-sm"
          :disabled="approving"
          @click="planEditing = false"
        >取消</button>
      </div>
    </template>

    <!-- 与规划器对话(批次 R2b):plan_chat 对话记录 + 反馈输入框;
         发送后规划器按反馈修订计划(任务保持 planned,需重新批准) -->
    <div class="sv-task-plan-chat">
      <div class="sv-task-plan-chat-title">与规划器对话(按反馈修订计划)</div>
      <div v-if="planChatMessages.length" class="sv-task-plan-chat-log">
        <div v-for="m in planChatMessages" :key="m.id" class="sv-task-msg-row">
          <div v-if="m.role === 'user'" class="sv-msg user">
            <div class="sv-msg-bubble">{{ m.content }}</div>
          </div>
          <div v-else class="sv-msg assistant">
            <div class="min-w-0">
              <div class="sv-msg-bubble sv-msg-md" v-html="renderMessageHtml(m)" />
            </div>
          </div>
        </div>
      </div>
      <div class="sv-task-followup-bar">
        <textarea
          v-model="planChatDraft"
          class="sv-input"
          rows="2"
          :disabled="planChatting || approving"
          placeholder="对计划提出修改意见,例如:把第二步换成先做竞品调研…"
          spellcheck="false"
        />
        <button
          class="sv-btn primary sv-btn-sm"
          :disabled="planChatting || approving || !planChatDraft.trim()"
          title="发送反馈,规划器将修订计划"
          @click="sendPlanChat"
        >{{ planChatting ? '修订中…' : '发送' }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 批准区「执行方式」选择行(2026-09-17)。样式纪律(MAINTENANCE D-5):新增组件样式
   一律写在 scoped 内,不进 style.css;只复用既有 :root 令牌与 .sv-* 基础类。 */
.sv-task-approve-exec {
  display: flex;
  align-items: center;
  gap: var(--space-2-5);
  margin: var(--space-3) 0 0;
}
.sv-task-approve-exec .sv-select { flex: 1; min-width: 0; }
.approve-exec-hint { margin: var(--space-1-5) 0 0; }

/* 待批准:hero 卡(2px 外框 + 4px 红顶线 + 实黑硬影) */
.sv-task-approve {
  padding: var(--space-5);
  border: var(--bw) solid var(--sv-ink);
  border-top: 4px solid var(--sv-red);
  background: var(--sv-white);
  box-shadow: var(--shadow-hard);
}
.sv-task-approve .sv-task-section-title {
  padding-bottom: 0;
  margin-bottom: var(--space-3);
  border-bottom: none;
  font-size: var(--text-md);
}
.sv-task-approve-head {
  margin-bottom: var(--space-4);
  padding-bottom: var(--space-3);
  border-bottom: 1px solid var(--sv-line);
}
.sv-task-approve-head .sv-task-approve-actions {
  margin-top: var(--space-3);
  padding-top: 0;
  border-top: none;
}
.sv-task-approve .sv-task-planned-plan {
  max-width: none;
  padding: 0;
  border: none;
  box-shadow: none;
  background: transparent;
}
.sv-task-approve .sv-task-result :deep(p) { margin: 0.6em 0; }
.sv-task-approve-actions {
  margin-top: var(--space-4);
  padding-top: var(--space-3-5);
  border-top: 1px solid var(--sv-line);
}

@media (max-width: 767px) {
  .sv-task-approve { padding: var(--space-4); }
}
</style>
