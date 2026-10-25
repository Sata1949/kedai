<script setup lang="ts">
// 任务成果区:成果汇总 / 交付校验 / 文件变更 / 审计 / 最终计划 / 对话记录 / 追加指令
// （C-2A，2026-10-25；原 TaskBoard 1139-1249）。
import { computed, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { splitTaskResult } from '../../taskResult';
import { renderMarkdownCached } from '../../utils/markdownCache';
import MarkdownBlock from './MarkdownBlock.vue';
import FileChangesPanel from '../FileChangesPanel.vue';
import TaskDeliveryPanel from '../TaskDeliveryPanel.vue';
import { messageKindLabel } from '../../api/labels';
import type { TaskDetail, TaskRunMode } from '../../api';

const props = defineProps<{
  detail: TaskDetail;
}>();

const store = useAppStore();
const { currentTaskId, executorById } = storeToRefs(store);

const taskMode = computed<TaskRunMode>(() => props.detail.task.task_mode ?? 'legacy');
const status = computed(() => props.detail.task.status);
/** 终态(done/partial/error/ended)才可追加指令(与后端 followup 门禁一致) */
const taskTerminal = computed(
  () => status.value === 'done' || status.value === 'partial' || status.value === 'error' || status.value === 'ended',
);
const taskPlanned = computed(() => status.value === 'planned');

/** 用户指令历史(详情 messages 字段;旧服务端无此字段,容错为空数组)。
 *  planned 态时 plan_chat 消息由批准区「与规划器对话」专属渲染(防同屏重复)。 */
const taskMessages = computed(() =>
  (props.detail.messages ?? []).filter((m) => m.kind !== 'plan_chat' || !taskPlanned.value),
);

// ----- 结果拆卡:team 模式尾部「## 审计结论」/ plan 模式尾部「## 最终计划」各自单独成卡 -----
const resultSplit = computed(() => splitTaskResult(taskMode.value, props.detail.task.result ?? ''));

/** 结果卡渲染门控:done/partial 恒渲染;error/ended 在 result 非空时渲染。
 *  pending/planning/running/planned **一律不渲染**(批次 R1):approve 后的过渡窗口
 *  result 仍持有 planned 态写入的计划清单文本,不门控会把计划清单误显示为「最终成果」。 */
const resultFinal = computed(() => {
  const s = status.value;
  if (s === 'done' || s === 'partial') return true;
  if (s === 'error' || s === 'ended') return !!(props.detail.task.result ?? '').trim();
  return false;
});

/** error/ended 态展示成果时的标注:结果只是「已完成部分的成果」,不是完整交付 */
const resultIncompleteNote = computed(() =>
  (status.value === 'error' || status.value === 'ended') && resultFinal.value
    ? '任务未完成,以下为已完成部分的成果'
    : '',
);

/** 最终成果/审计结论/最终计划的 markdown 预渲染(共享有界缓存) */
const resultMainHtml = computed(() =>
  resultFinal.value && resultSplit.value.main ? renderMarkdownCached(resultSplit.value.main) : '',
);
const resultAuditHtml = computed(() =>
  resultFinal.value && resultSplit.value.audit ? renderMarkdownCached(resultSplit.value.audit) : '',
);
const resultFinalPlanHtml = computed(() =>
  resultFinal.value && resultSplit.value.finalPlan ? renderMarkdownCached(resultSplit.value.finalPlan) : '',
);

/** 成果汇总卡展开态(默认关闭;持久化在 uiPrefs,跨会话/重启记忆) */
const summaryOpen = computed(() => store.taskResultSummaryOpen);
function toggleSummary(): void {
  store.taskResultSummaryOpen = !store.taskResultSummaryOpen;
}

/** 助手发言的署名:优先执行者库的名称;旧任务(仅 character_id)回退角色名;
 *  都无绑定(通用执行者)则「任务 Agent」。 */
const taskMessageAuthor = computed(() => {
  const task = props.detail.task;
  const eid = task.executor_id;
  if (eid) {
    const e = executorById.value(eid);
    if (e?.name) return e.name;
  }
  const cid = task.character_id;
  if (cid) {
    const c = store.characters.find((x) => x.id === cid);
    if (c?.chara_name) return c.chara_name;
  }
  return '任务 Agent';
});

/** assistant 消息的 markdown 渲染(含缓存;用户消息直接显示纯文本) */
function renderMessageHtml(m: { content: string }): string {
  return renderMarkdownCached(m.content);
}

// ----- 追加指令(批次 R2a;R2b+ 扩 mode) -----
/** 追加指令输入草稿与发送中标记 */
const followupDraft = ref('');
const followupSending = ref(false);
/** 追加模式(2026-09-10 实跑修复 F5):append=追加进成果,replace=整体重写成果。 */
const followupMode = ref<'append' | 'replace'>('append');

/** 非终态时的禁用提示(按状态给出可操作的下一步) */
const followupDisabledHint = computed(() => {
  const s = status.value;
  if (s === 'planned') return '计划待批准:请先批准或放弃(批准区可修改计划)';
  if (s === 'pending') return '任务尚未执行:请先执行,产出首轮成果后可追加';
  return '任务执行中:完成或停止后可追加指令';
});

/** 发送追加指令:成功后草稿清空(详情经 store.followupTask 内刷新带出 messages/result) */
async function sendFollowup(): Promise<void> {
  const id = currentTaskId.value;
  const content = followupDraft.value.trim();
  if (!id || !content || followupSending.value || !taskTerminal.value) return;
  followupSending.value = true;
  try {
    await store.followupTask(id, content, followupMode.value);
    followupDraft.value = '';
    followupMode.value = 'append';
  } catch (err) {
    alert(`追加失败:${(err as Error).message}`);
  } finally {
    followupSending.value = false;
  }
}

// 切换任务时清空追加草稿(与原 watch 语义一致)
watch(
  () => props.detail.task.id,
  () => {
    followupDraft.value = '';
    followupMode.value = 'append';
  },
);
</script>

<template>
  <!-- 最终结果(team 模式拆尾部「## 审计结论」、plan 模式拆尾部「## 最终计划」
       各自单独成卡;门控见 resultFinal)。实跑问题 1:逐轮对话记录区已是产出的权威视图,
       本卡默认关闭(与气泡重复),可经标题栏开关展开;审计/最终计划为独立信息,保持常显。 -->
  <div v-if="resultMainHtml" class="sv-task-section">
    <div class="sv-task-section-title">
      成果汇总
      <button
        class="sv-task-summary-toggle"
        :title="summaryOpen ? '收起成果汇总(逐轮对话记录已含全部产出)' : '展开成果汇总(合并后的完整成果)'"
        @click="toggleSummary"
      >
        {{ summaryOpen ? '收起' : '展开' }}
      </button>
    </div>
    <div v-if="resultIncompleteNote" class="sv-task-incomplete-note">
      {{ resultIncompleteNote }}
    </div>
    <MarkdownBlock v-if="summaryOpen" :html="resultMainHtml" />
  </div>
  <!-- 交付校验(TMR-6):必需/已生成/缺失/变更数 + 缺失文件清单。 -->
  <div class="sv-task-section">
    <div class="sv-task-section-title">交付校验</div>
    <TaskDeliveryPanel
      :delivery="detail.delivery ?? null"
      :workspace-bound="!!detail.task.workspace"
    />
  </div>
  <!-- 文件变更(批次 4c,PRODCAP-4):数据自取 store;空清单也不隐藏卡片。 -->
  <div class="sv-task-section">
    <div class="sv-task-section-title">文件变更</div>
    <FileChangesPanel />
  </div>
  <div v-if="resultAuditHtml" class="sv-task-section">
    <div class="sv-task-section-title">审计结论</div>
    <MarkdownBlock :html="resultAuditHtml" class="sv-task-audit" />
  </div>
  <div v-if="resultFinalPlanHtml" class="sv-task-section">
    <div class="sv-task-section-title">最终计划</div>
    <MarkdownBlock :html="resultFinalPlanHtml" class="sv-task-final-plan" />
  </div>

  <!-- 对话记录(实跑问题 1):任务目标与各轮产出按 user/assistant 逐轮气泡呈现。 -->
  <div v-if="taskMessages.length" class="sv-task-section sv-task-messages">
    <div class="sv-task-section-title">对话记录</div>
    <div v-for="m in taskMessages" :key="m.id" class="sv-task-msg-row">
      <div v-if="m.role === 'user'" class="sv-msg user">
        <div class="sv-msg-bubble">
          <span v-if="messageKindLabel(m.kind)" class="sv-tag sm sv-task-msg-kind">{{ messageKindLabel(m.kind) }}</span>{{ m.content }}
        </div>
      </div>
      <div v-else class="sv-msg assistant">
        <div class="min-w-0">
          <div class="sv-msg-name">
            {{ taskMessageAuthor }}
            <span v-if="messageKindLabel(m.kind)" class="sv-tag sm sv-task-msg-kind">{{ messageKindLabel(m.kind) }}</span>
          </div>
          <div class="sv-msg-bubble sv-msg-md" v-html="renderMessageHtml(m)" />
        </div>
      </div>
    </div>
  </div>

  <!-- 追加指令(批次 R2a;R2b+ 扩 mode):终态任务可继续下达;
       追加=在既有成果后附加新段(不改原文),重写=用新产出整体替换成果。 -->
  <div class="sv-task-section sv-task-followup">
    <div class="sv-task-section-title">
      追加指令
      <select
        v-model="followupMode"
        class="sv-task-followup-mode"
        :disabled="!taskTerminal || followupSending"
        title="追加:在成果后附加新段(保留原文);重写:用本轮产出整体替换成果"
      >
        <option value="append">追加</option>
        <option value="replace">重写</option>
      </select>
    </div>
    <div class="sv-task-followup-bar">
      <textarea
        v-model="followupDraft"
        class="sv-input"
        rows="2"
        :disabled="!taskTerminal || followupSending"
        :placeholder="taskTerminal ? (followupMode === 'replace' ? '用本轮产出整体替换成果,例如:把全文压缩到 200 字以内…' : '在既有成果基础上继续补充,例如:再润色一遍结尾…') : followupDisabledHint"
        spellcheck="false"
      />
      <button
        class="sv-btn primary sv-btn-sm"
        :disabled="!taskTerminal || followupSending || !followupDraft.trim()"
        :title="taskTerminal ? (followupMode === 'replace' ? '发送并整体替换成果' : '发送追加指令,任务将继续执行') : followupDisabledHint"
        @click="sendFollowup"
      >{{ followupSending ? '发送中…' : '发送' }}</button>
    </div>
    <div class="sv-task-followup-hint">
      <template v-if="followupMode === 'replace'">重写模式:本轮产出将<b>整体替换</b>现有成果,旧内容不再保留</template>
      <template v-else>追加模式:新产出附加在成果末尾,<b>不会修改</b>原有内容;若要压缩或改写全文请切换到「重写」</template>
    </div>
    <div v-if="!taskTerminal" class="sv-task-followup-hint">{{ followupDisabledHint }}</div>
  </div>
</template>
