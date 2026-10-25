<script setup lang="ts">
// 任务改绑流程（C-2A，2026-10-25；原 TaskBoard 147-313 + 821-887）。
//
// 入口门禁与后端同口径：仅 custom，且不在 planning/running/planned。
// 展开是内联入选区，不开模态；保存走 store.bindTask（全量下发根流程与名单）。
import { computed, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { flowCandidates, staleMembers, type FlowCandidate } from '../../utils/flowCandidates';
import type { TaskDetail, TaskRunMode } from '../../api';

const props = defineProps<{
  detail: TaskDetail;
}>();

const store = useAppStore();
const { currentTaskId, agentFlowLibrary } = storeToRefs(store);

const taskMode = computed<TaskRunMode>(() => props.detail.task.task_mode ?? 'legacy');

/**
 * 「改绑流程」入口的可见性:仅 custom 模式,且状态**不在** `planning` / `running` /
 * `planned`(与后端门禁同一口径)。前端不该给出一个必然 400 的入口——进行中改绑会撕裂
 * 快照,待批准态改绑会让批准后跑的那份编排对不上已批准的计划。
 */
const canBindFlow = computed(() => {
  if (taskMode.value !== 'custom') return false;
  const s = props.detail.task.status;
  return s !== 'planning' && s !== 'running' && s !== 'planned';
});

/** 入选区展开态(内联展开,不开模态) */
const bindOpen = ref(false);
/** 入选草稿:根流程空串 = 跟随当前流程(下发 null);名单空 = 强制模式 */
const bindRoot = ref('');
const bindIds = ref<string[]>([]);
const bindSaving = ref(false);
/** 保存失败的后端原文(400 文案点名原因,如「任务已在执行中」) */
const bindError = ref('');

/** 候选数据源(与创建选择器同一份库) */
const bindFlows = computed(() => agentFlowLibrary.value?.flows ?? []);

/** 根流程 id(与 TaskFlowSelect 同口径:显式绑定优先,否则「跟随当前流程」= 库的当前流程) */
const bindRootId = computed(
  () => bindRoot.value || agentFlowLibrary.value?.current_flow_id || '',
);

/** 绑定的流程库里已不存在 → 保位选项(与选择器同款,避免静默改绑到别的流程) */
const bindStaleRoot = computed(() => {
  const id = bindRoot.value;
  if (!id) return '';
  return bindFlows.value.some((f) => f.id === id) ? '' : id;
});

/** 勾选行(根流程不可调用 / 停用不可勾 / 已失效成员保位):判定与创建选择器共用同一实现 */
const bindCandidates = computed<FlowCandidate[]>(() =>
  flowCandidates(bindFlows.value, bindRootId.value, bindIds.value),
);

/** 名单里库中已不存在的成员:后端必 400,提前说清(与选择器同款警示) */
const bindStaleSelected = computed(() => staleMembers(bindCandidates.value, bindIds.value));

// 切换任务时收起入选区:草稿属于上一个任务,不跨任务沿用(原 TaskBoard watch 语义)
watch(
  () => props.detail.task.id,
  () => {
    bindOpen.value = false;
    bindError.value = '';
  },
);

/** 展开入选区:草稿从当前绑定起步(取消即丢弃,不写回任何状态) */
function openBind(): void {
  const task = props.detail.task;
  bindRoot.value = task.flow_id ?? '';
  bindIds.value = [...(task.flow_ids ?? [])];
  bindError.value = '';
  bindOpen.value = true;
  // 候选来自流程库:有快照时父级不会拉库(徽标自足),而改绑必须有库
  if (!agentFlowLibrary.value) void store.loadAgentFlow();
}

function cancelBind(): void {
  bindOpen.value = false;
  bindError.value = '';
}

/** 勾选/取消一个可调用流程(数组顺序 = 用户勾选顺序,后端工具描述按此列举) */
function toggleBindId(id: string, ev: Event): void {
  const checked = (ev.target as HTMLInputElement).checked;
  const list = bindIds.value.filter((x) => x !== id);
  if (checked) list.push(id);
  bindIds.value = list;
}

/** 改绑确认文案:说清**快照会被替换**(历史徽标可能不再匹配)与名单形态 */
function bindConfirmText(): string {
  const id = bindRoot.value;
  const rootName = id
    ? bindFlows.value.find((f) => f.id === id)?.name?.trim() || id
    : '跟随当前流程';
  const list =
    bindIds.value.length > 0
      ? `可调用名单 ${bindIds.value.length} 个流程`
      : '强制模式(清空可调用名单)';
  return (
    `将把本任务改绑到「${rootName}」(${list})。` +
    '改绑会替换流程快照:历史步骤的编排徽标按新编排重新解析,对不上的不再显示;' +
    '已跑过的步骤结果与调用记录不受影响。确定改绑吗?'
  );
}

/** 保存改绑:全量替换(根流程与名单都显式下发;名单空 = 强制模式) */
async function saveBind(): Promise<void> {
  const id = currentTaskId.value;
  if (!id || bindSaving.value) return;
  if (!confirm(bindConfirmText())) return;
  bindSaving.value = true;
  bindError.value = '';
  try {
    await store.bindTask(id, bindRoot.value || null, [...bindIds.value]);
    bindOpen.value = false;
  } catch (err) {
    // 后端 400 文案(点名原因)原样透出,不吞成「保存失败」
    bindError.value = (err as Error).message;
  } finally {
    bindSaving.value = false;
  }
}
</script>

<template>
  <div v-if="canBindFlow" class="sv-task-flow-bind">
    <button
      v-if="!bindOpen"
      class="sv-btn ghost sv-btn-sm"
      title="改绑本任务执行的流程:保存后按新流程重新冻结快照"
      @click="openBind"
    >改绑流程</button>
    <div v-else class="sv-task-flow-bind-panel">
      <div class="sv-task-flow-bind-title">改绑流程(保存后按新编排重新冻结快照)</div>
      <div class="sv-inp-row">
        <label class="sv-inp-tag">根流程</label>
        <select
          v-model="bindRoot"
          class="sv-select sv-task-flow-bind-select"
          title="绑定要执行的流程;绑定后该任务即冻结在这份编排上(改流程/换当前流程都不影响它)。「跟随当前流程」= 执行时按当时的当前流程跑"
        >
          <option value="">流程:跟随当前流程</option>
          <option v-if="bindStaleRoot" :value="bindStaleRoot">流程:(已失效) {{ bindStaleRoot }}</option>
          <option v-for="f in bindFlows" :key="f.id" :value="f.id" :disabled="!f.enabled">
            流程:{{ f.name || f.id }}{{ f.enabled ? '' : '(已停用)' }}
          </option>
        </select>
      </div>
      <!-- 可调用名单(与创建选择器同款文案与判定):空名单 = 强制模式,是合法选择 -->
      <p v-if="!bindFlows.length" class="sv-note flow-tool-warn">
        流程库为空(或尚未加载):请先在设置里添加并启用流程,再来改绑。
      </p>
      <p class="sv-note">
        可调用流程(模型在工具循环里自主调用,成果回灌给根流程;一个都不勾 = 强制模式,只跑根流程):
      </p>
      <label
        v-for="c in bindCandidates"
        :key="c.id"
        class="flow-id-item"
        :class="{ disabled: c.disabled }"
      >
        <input
          type="checkbox"
          class="flow-id-box"
          :checked="bindIds.includes(c.id)"
          :disabled="c.disabled"
          @change="toggleBindId(c.id, $event)"
        />
        <span>{{ c.label }}</span>
      </label>
      <p v-if="bindStaleSelected.length" class="sv-note flow-tool-warn">
        名单里有已失效的流程,请取消勾选后再保存(否则后端会拒绝并保持原绑定)。
      </p>
      <p v-if="bindError" class="sv-note sv-task-flow-bind-err">改绑失败:{{ bindError }}</p>
      <div class="sv-task-flow-bind-actions">
        <button
          class="sv-btn primary sv-btn-sm"
          :disabled="bindSaving"
          title="保存后按新流程重新冻结快照;历史步骤的编排徽标按新编排解析"
          @click="saveBind"
        >{{ bindSaving ? '保存中…' : '保存' }}</button>
        <button
          class="sv-btn ghost sv-btn-sm"
          :disabled="bindSaving"
          title="放弃本次改绑(当前绑定不受影响)"
          @click="cancelBind"
        >取消</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 改绑流程(B 批 B3):内联入选区。样式纪律(MAINTENANCE D-5)同下:只写 scoped、
   只复用既有 :root 令牌与 .sv-* 基础类(直角体系,不引入圆角)。 */
.sv-task-flow-bind {
  margin: var(--space-2-5) 0 18px;
}
.sv-task-flow-bind-panel {
  display: flex;
  flex-direction: column;
  gap: var(--space-1-5);
  padding: var(--space-2-5) var(--space-3);
  border: var(--bw-thin) solid var(--sv-line-strong);
  background: var(--sv-surface-elevated);
}
.sv-task-flow-bind-title {
  font-weight: 700;
  font-size: 12px;
}
.sv-task-flow-bind-select {
  flex: 1;
  min-width: 0;
}
.sv-task-flow-bind-panel .flow-id-item {
  display: flex;
  align-items: center;
  gap: var(--space-1-5);
  font-size: 13px;
  cursor: pointer;
}
.sv-task-flow-bind-panel .flow-id-item.disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.sv-task-flow-bind-panel .sv-note {
  margin: 0;
}
.sv-task-flow-bind-err {
  color: var(--sv-red-deep);
}
.sv-task-flow-bind-actions {
  display: flex;
  gap: var(--space-2);
  margin-top: var(--space-1);
}
</style>
