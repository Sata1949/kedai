<script setup lang="ts">
// 任务执行过程三档显示(TMSHOW-2)。
//
// 三档:
//   档 1 `legacy`    —— 计划步骤时间线精修(状态色带 + 长结果折叠),不显示中间过程;
//   档 2 `full`      —— 扁平全消息时间线,每条轨迹独立成卡,按 seq 排序;
//   档 3 `condensed` —— 按 phase + step_index 分组,每组一张卡,组内折叠 + 分类计数角标。
//
// 数据来自 TMSHOW-1 的 `task_trace`(经 composables/useTaskTrace 加载,SSE 增量并入)。
// **降级路径**:轨迹为空(旧任务 / 无工具循环的模式)时档 2 / 档 3 自动回退档 1 布局
// 并显示一行说明——绝不出现一片空白的时间线(那会被误读成「加载失败」)。
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';
import { renderMarkdownCached } from '../../utils/markdownCache';
import { badgeFlowSource, planRowBadges, type NodeBadge } from '../../utils/flowNodeBadges';
import { taskStatusClass as statusClass, taskStatusLabel as statusLabel } from '../../taskStatus';
import {
  flattenTrace,
  formatTraceCounts,
  groupTrace,
  type TranscriptMode,
} from '../../utils/taskTranscript';
import { bufferLabel } from '../../utils/phaseLabel';
import { createVirtualListState } from '../../composables/useVirtualMessages';
import TaskTraceItem from './TaskTraceItem.vue';
import type { TaskDetail, TaskRunMode, TaskStep } from '../../api';

const props = defineProps<{
  detail: TaskDetail;
}>();

const store = useAppStore();
const { agentFlowLibrary, taskTrace, taskTraceTruncated, taskTraceLoading, taskTraceError, taskTranscriptMode } =
  storeToRefs(store);

const taskMode = computed<TaskRunMode>(() => props.detail.task.task_mode ?? 'legacy');
const taskId = computed(() => props.detail.task.id);

// ===== 档位 =====
/** 当前档位:轨迹为空时档 2/3 自动回退档 1(降级),但**用户选择本身不改**。
 *  选择控件已迁到表头(TaskTranscriptModes),这里只读不写。 */
const requestedMode = computed<TranscriptMode>(() => taskTranscriptMode.value ?? 'full');
const hasTrace = computed(() => taskTrace.value.length > 0);
const degraded = computed(() => !hasTrace.value && requestedMode.value !== 'legacy');
const mode = computed<TranscriptMode>(() => (degraded.value ? 'legacy' : requestedMode.value));

// ===== 拉取 =====
/**
 * 首次挂载时若 store 里还没有本任务的轨迹,拉一次。
 * selectTask 也会拉(那才是主要入口);这里兜底「组件晚挂载 / 状态被清过」的情形。
 * 已有轨迹时不重拉——重复拉全量在长任务上是几 MB 的浪费。
 */
function ensureTrace(): void {
  const id = taskId.value;
  if (!id) return;
  if (taskTrace.value.length > 0) return;
  void store.loadTaskTrace(id);
}
onMounted(ensureTrace);
watch(taskId, ensureTrace);

function retryLoad(): void {
  const id = taskId.value;
  if (id) void store.loadTaskTrace(id);
}

// ===== 档 1:计划步骤(原 TaskExecutionPanel 的步骤区语义原样保留)=====
interface RenderedStep {
  step: TaskStep;
  html: string;
  /** 结果是否超过折叠阈值(超过则默认收起) */
  collapsible: boolean;
}

/** 长结果折叠阈值(字符):超过即默认收起,由用户点开 */
const STEP_RESULT_COLLAPSE = 800;

const planRendered = computed<RenderedStep[]>(() =>
  (props.detail.task.plan ?? []).map((s) => ({
    step: s,
    html: s.result ? renderMarkdownCached(s.result) : '',
    collapsible: (s.result ?? '').length > STEP_RESULT_COLLAPSE,
  })),
);

/** 已展开的步骤下标(仅对那些超过阈值的步骤有意义) */
const expandedSteps = ref<Set<number>>(new Set());
function toggleStep(index: number): void {
  const next = new Set(expandedSteps.value);
  if (next.has(index)) next.delete(index);
  else next.add(index);
  expandedSteps.value = next;
}

/** 逐行编排徽标(下标与 plan 对齐;空数组 = 该行不显示),逻辑与原 TaskExecutionPanel 一致 */
const planBadges = computed<NodeBadge[][]>(() => {
  const rows = props.detail.task.plan ?? [];
  const source = badgeFlowSource(props.detail.flow_snapshot, agentFlowLibrary.value);
  if (taskMode.value !== 'custom' || !source) {
    return rows.map(() => []);
  }
  return rows.map((s) => planRowBadges(source.flows, source.rootId, s.node_id));
});

/** planned 态计划清单已渲染于批准区时隐藏步骤区(视觉去重;与批准区同口径) */
const plannedPlanHtml = computed(() => {
  if (props.detail.task.status !== 'planned' || taskMode.value !== 'plan') return '';
  const result = props.detail.task.result ?? '';
  return result ? renderMarkdownCached(result) : '';
});

/**
 * 档 1 是否渲染计划步骤区。
 *
 * **team 模式排除**:team 的步骤由 TaskExecutionPanel 的「主 Agent 分工」卡呈现
 * (按【主Agent-N】前缀分组),两处都渲染同一批 plan 行就是同一份内容出现两次
 * ——这是原 C-2A 拆分时用 `v-if`/`v-else-if` 保证的互斥关系,档 1 必须原样继承。
 * 档 2/3 没有这个问题(它们渲染的是轨迹,不是 plan)。
 */
const showPlan = computed(
  () =>
    (props.detail.task.plan ?? []).length > 0 &&
    !plannedPlanHtml.value &&
    taskMode.value !== 'team',
);
const planTitle = computed(() => (taskMode.value === 'custom' ? '流程步骤进度' : '计划步骤'));

// ===== 档 2:扁平时间线(>阈值时窗口化)=====
/**
 * 虚拟化阈值:超过这么多条才窗口化。
 *
 * 取值理由:一屏大约能显示 5~8 条轨迹卡;120 条是「用户已经开始滚很久」的量,
 * 低于这个数真实挂载的开销可以忽略,而窗口化本身有占位抖动。
 */
const VIRTUAL_THRESHOLD = 120;
/** 尾部常驻挂载条数(最新轨迹总在尾部,与聊天侧同口径) */
const VIRTUAL_TAIL_KEEP = 30;

const scrollArea = ref<HTMLElement | null>(null);

/**
 * 档 2 的窗口化状态。
 *
 * 行 id 用 `trace.seq`(数字):轨迹在任务内唯一且稳定,**不能用数组下标**——
 * 增量并入会让下标整体位移,窗口会跟着乱跳。
 */
/**
 * 档 2 的行序列。
 *
 * **必须过 `flattenTrace`**:store 里的数组通常已有序,但组件不能依赖上游的排序承诺
 * ——补拉帧与实时帧拼接、以及任何一次外部写入都可能让顺序失真,而「时间线顺序错乱」
 * 是用户第一眼就能看出、却极难自查的缺陷。`flattenTrace` 同时负责按 seq 去重。
 */
const fullRows = computed(() => flattenTrace(taskTrace.value));
const vmRows = computed(() => fullRows.value.map((t) => ({ id: t.seq, role: 'assistant' as const })));
const vmState = createVirtualListState({
  threshold: VIRTUAL_THRESHOLD,
  tailKeep: VIRTUAL_TAIL_KEEP,
  overscanPx: 900,
});

const vmEnabled = computed(() => vmState.isEnabled(vmRows.value.length));
/** 行 id → 下标映射:窗口判定要用(按数组引用记忆化,避免逐行重建) */
let orderOwner: unknown = null;
function syncOrderIfNeeded(): void {
  if (orderOwner === vmRows.value) return;
  orderOwner = vmRows.value;
  vmState.syncOrder(vmRows.value.map((r) => r.id));
}

const observedRows = new Map<number, HTMLElement>();
let observer: IntersectionObserver | null = null;

/** 行是否真实挂载:窗口外降级为等高占位 */
function rowActive(seq: number, index: number): boolean {
  syncOrderIfNeeded();
  return vmState.isActive(seq, index, vmRows.value.length, null);
}

/**
 * 行间距(px),必须与 `TaskTraceItem.vue` 的 `.sv-trace-item { margin-bottom }` 同值。
 *
 * **为什么占位高度要含它**:占位 div 是真实行的兄弟节点,真实行的高度里有 8px 下间距,
 * 占位却没有。每换一行就丢 8px,几百行的滚动位置会累积漂移——表现就是「滚到中段时
 * 列表突然跳一下」(UIFIX-8 实测)。这是等高占位的必要条件,不是保险。
 */
const ROW_GAP_PX = 8;

/** 占位高度:实测缓存(未渲染过则按角色估算)+ 行间距 */
function placeholderStyle(seq: number): { height: string } {
  return { height: `${vmState.placeholderHeight(seq, 'assistant') + ROW_GAP_PX}px` };
}

/**
 * 实测一行的高度(含行间距)。
 *
 * `offsetHeight` 只算元素自身盒高,不含子元素的 margin——真实行外层是个裸 div,
 * 里面的 `.sv-trace-item` 带 8px 下间距,直接取 `offsetHeight` 会系统性偏小。
 * 故量到子元素的实际外边距并补回;取不到就退回纯 offsetHeight(不虚构数值)。
 */
function measureRow(el: HTMLElement): number {
  const own = el.offsetHeight;
  if (own <= 0) return 0;
  const child = el.firstElementChild;
  if (!child) return own + ROW_GAP_PX;
  const mb = parseFloat(getComputedStyle(child).marginBottom);
  return own + (Number.isFinite(mb) ? mb : ROW_GAP_PX);
}

/** 行元素登记(占位与真实行都登记;IO 据此上报进出缓冲区) */
function bindRow(seq: number, el: unknown): void {
  const prev = observedRows.get(seq);
  if (prev) {
    observer?.unobserve(prev);
    observedRows.delete(seq);
  }
  const target = el as HTMLElement | null;
  if (!target) return;
  target.dataset.kdVmId = `n:${seq}`;
  observedRows.set(seq, target);
  observer?.observe(target);
}

/**
 * IO 的 root 必须是**真正在滚的那个元素**。
 *
 * 任务详情区的滚动发生在 `.sv-task-detail`(全局 CSS `overflow-y: auto`),
 * 而 `.sv-transcript-scroll` 只是本组件的包裹 div、自己并不滚。若把 root 绑在包裹 div 上,
 * IntersectionObserver 会以「该 div 的可见区域」为观察窗口——它不随页面滚动变化,
 * 于是占位行永远等不到「进入视口」的上报,窗口化退化成「尾部 30 条 + 291 个死占位」。
 * 这里从包裹 div 向上找第一个可滚动的祖先,找不到才退回视口(root=null)。
 */
function resolveScrollRoot(el: HTMLElement | null): HTMLElement | null {
  let node: HTMLElement | null = el?.parentElement ?? null;
  while (node) {
    const style = getComputedStyle(node);
    if (/(auto|scroll)/.test(style.overflowY) && node.scrollHeight > node.clientHeight) return node;
    node = node.parentElement;
  }
  return null; // 退回视口:整页滚动时 IO 默认行为正确
}

function setupObserver(): void {
  if (typeof IntersectionObserver === 'undefined') return;
  observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const el = entry.target as HTMLElement;
        const seq = Number((el.dataset.kdVmId ?? '').replace('n:', ''));
        if (!Number.isFinite(seq)) continue;
        if (entry.isIntersecting) {
          vmState.markVisible(seq);
        } else if (vmState.visible.has(seq)) {
          // 用 measured 而非 offsetHeight:后者不含行间距,占位会比真实行矮(见 measureRow)
          vmState.markHidden(seq, measureRow(el) || Math.round(entry.boundingClientRect.height));
        }
      }
    },
    { root: resolveScrollRoot(scrollArea.value), rootMargin: '900px 0px' },
  );
  for (const el of observedRows.values()) observer.observe(el);
}

/**
 * 挂载后再建观察器:此时 `.sv-transcript-scroll` 已入 DOM,向上找滚动祖先才拿得到
 * 真实的 `scrollHeight/clientHeight`(挂载前两者都是 0,`resolveScrollRoot` 会误判为「不可滚」)。
 */
onMounted(async () => {
  await nextTick();
  setupObserver();
  // 行元素可能在首帧后才落地(轨迹是异步加载的),补挂一次
  for (const el of observedRows.values()) observer?.observe(el);
});

/** 切任务时整体重置窗口(旧任务的高度缓存对新任务无意义) */
watch(taskId, async () => {
  vmState.reset();
  observedRows.clear();
  observer?.disconnect();
  observer = null;
  await nextTick();
  setupObserver();
});

// ===== 档 3:分组折叠 =====
/** 步骤名解析器(把 step 类阶段补上可读步骤名) */
function stepNameAt(index: number): string | null {
  return props.detail.task.plan?.[index]?.name ?? null;
}

const groups = computed(() => groupTrace(taskTrace.value, stepNameAt));

/** 已展开的组(默认全收:档 3 的卖点就是「先看结构」) */
const expandedGroups = ref<Set<string>>(new Set());
function toggleGroup(key: string): void {
  const next = new Set(expandedGroups.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  expandedGroups.value = next;
}

/** 全量角标已随档位条迁到 TaskTranscriptModes(UIFIX-8);本组件不再需要计数 */

// ===== 「正在生成」活动指示(沿用 store 的流式缓冲)=====
const liveRaw = computed(() => {
  const entries = [...store.liveBuffers.entries()];
  if (entries.length === 1) return entries[0][1];
  const plan = props.detail.task.plan ?? [];
  const stepName = (i: number) => plan[i]?.name || null;
  return entries.map(([k, t]) => `【${bufferLabel(k, stepName)}】\n${t}`).join('\n\n');
});

const liveText = ref(liveRaw.value);
let liveTimer: ReturnType<typeof setTimeout> | null = null;
watch(liveRaw, (v) => {
  if (liveTimer) return;
  liveText.value = v;
  liveTimer = setTimeout(() => {
    liveTimer = null;
    if (liveRaw.value !== liveText.value) liveText.value = liveRaw.value;
  }, 100);
});

onUnmounted(() => {
  if (liveTimer) clearTimeout(liveTimer);
  observer?.disconnect();
  observer = null;
});
</script>
<template>
  <div class="sv-transcript" :data-mode="mode">
    <!-- 档位切换条已迁出(UIFIX-8):住在 TaskStatusPanel 的 sticky 表头里
         (TaskTranscriptModes.vue),滚到任何位置都能看到当前档并直接切换。 -->

    <!-- 降级说明:轨迹为空时档 2/3 回退档 1,不让用户看到一片空白时间线 -->
    <div v-if="degraded" class="sv-transcript-note" role="status">
      本任务没有可展示的执行轨迹（旧任务或本次运行未产生工具调用），已回退为步骤视图。
    </div>

    <!-- 加载失败:与「没有轨迹」严格区分,给重试入口 -->
    <div v-if="taskTraceError" class="sv-transcript-error" role="alert">
      <span>执行轨迹加载失败：{{ taskTraceError }}</span>
      <button type="button" class="sv-btn ghost sv-btn-sm" @click="retryLoad">重试</button>
    </div>

    <!-- 窗口截断:更早的轨迹已被清理,如实告知 -->
    <div v-if="taskTraceTruncated" class="sv-transcript-note">
      轨迹保留窗口已清理更早的条目，以下为仍可回看的部分。
    </div>

    <!-- ===== 档 2:扁平全消息时间线 ===== -->
    <div v-if="mode === 'full'" class="sv-transcript-full">
      <div v-if="taskTraceLoading && !hasTrace" class="sv-transcript-note">正在加载执行轨迹…</div>
      <div ref="scrollArea" class="sv-transcript-scroll">
        <template v-for="(row, i) in fullRows" :key="row.seq">
          <!-- 占位与真实行**都必须登记** ref:占位 div 也要能被 IO 观察到,
               否则滚进占位区时没有任何元素上报「进入视口」,占位永远换不回真实条目
               (首轮实现漏了这一条,实测滚动后 items 恒为 30)。 -->
          <div
            v-if="vmEnabled && !rowActive(row.seq, i)"
            :ref="(el) => bindRow(row.seq, el)"
            :style="placeholderStyle(row.seq)"
            class="sv-transcript-placeholder"
          />
          <div v-else :ref="(el) => bindRow(row.seq, el)">
            <TaskTraceItem :trace="row" :step-name="stepNameAt" />
          </div>
        </template>
      </div>
    </div>

    <!-- ===== 档 3:按 phase + step_index 分组折叠 ===== -->
    <div v-else-if="mode === 'condensed'" class="sv-transcript-condensed">
      <div v-for="g in groups" :key="g.key" class="sv-transcript-group">
        <button
          type="button"
          class="sv-transcript-group-head"
          :aria-expanded="expandedGroups.has(g.key)"
          @click="toggleGroup(g.key)"
        >
          <span class="sv-transcript-caret" :class="{ open: expandedGroups.has(g.key) }" aria-hidden="true">▸</span>
          <span class="sv-transcript-group-label">{{ g.label }}</span>
          <span class="sv-transcript-group-count sv-tnum">{{ formatTraceCounts(g.counts) }}</span>
        </button>
        <div v-if="expandedGroups.has(g.key)" class="sv-transcript-group-body">
          <TaskTraceItem
            v-for="row in g.rows"
            :key="row.seq"
            :trace="row"
            :step-name="stepNameAt"
            :default-open="true"
          />
        </div>
      </div>
    </div>

    <!-- ===== 档 1:计划步骤(精修:状态色带 + 长结果折叠) ===== -->
    <div v-if="mode === 'legacy'" class="sv-transcript-steps">
      <template v-if="showPlan">
        <div class="sv-task-section-title">{{ planTitle }}</div>
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
              <span
                v-for="(badge, bi) in planBadges[i]"
                :key="bi"
                class="sv-tag sm flow-node-tag"
                :class="badge.kind"
              >{{ badge.text }}</span>
            </div>
            <div v-if="rs.step.goal" class="sv-task-step-goal">{{ rs.step.goal }}</div>
            <template v-if="rs.step.result">
              <div
                v-if="!rs.collapsible || expandedSteps.has(i)"
                class="sv-task-step-result"
                v-html="rs.html"
              />
              <button
                v-else
                type="button"
                class="sv-transcript-expand"
                @click="toggleStep(i)"
              >展开步骤结果（{{ rs.step.result.length }} 字）</button>
              <button
                v-if="rs.collapsible && expandedSteps.has(i)"
                type="button"
                class="sv-transcript-expand"
                @click="toggleStep(i)"
              >收起步骤结果</button>
            </template>
          </div>
        </div>
      </template>
    </div>

    <!-- 正在生成:活动指示,三档都保留(它是「还在动」的证据,不是中间过程) -->
    <div v-if="liveText" class="sv-task-section sv-task-live">
      <div class="sv-task-section-title">正在生成</div>
      <div class="sv-task-live-text sv-stream-cursor">{{ liveText }}</div>
    </div>
  </div>
</template>

<style scoped>
.sv-transcript { display: block; }

/* 档位条的样式随组件迁到 TaskTranscriptModes.vue(UIFIX-8),此处不再保留。 */

.sv-transcript-note {
  margin-bottom: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border-left: 2px solid var(--sv-line-strong);
  background: var(--sv-surface-elevated);
  font-size: var(--text-xs);
  line-height: var(--lh-body);
  color: var(--sv-ink-dim);
}
.sv-transcript-error {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border-left: 2px solid var(--sv-red);
  background: var(--sv-white);
  font-size: var(--text-xs);
  color: var(--sv-ink);
}

.sv-transcript-scroll { max-height: none; }

/* 档 3 分组卡 */
.sv-transcript-group {
  border: var(--bw-thin) solid var(--sv-line-strong);
  background: var(--sv-white);
  margin-bottom: var(--space-2);
}
.sv-transcript-group-head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  padding: var(--space-2-5) var(--space-3);
  border: 0;
  background: none;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
}
.sv-transcript-group-label { flex: 1; min-width: 0; font-size: var(--text-sm); font-weight: 700; }
.sv-transcript-group-count { flex: none; font-size: var(--text-xs); color: var(--sv-ink-faint); }
.sv-transcript-caret {
  flex: none;
  font-size: var(--text-2xs);
  color: var(--sv-ink-faint);
  transition: transform var(--dur-fast) var(--ease-standard);
}
.sv-transcript-caret.open { transform: rotate(90deg); }
.sv-transcript-group-body { padding: 0 var(--space-3) var(--space-2); }

.sv-transcript-expand {
  padding: 0;
  border: 0;
  background: none;
  color: var(--sv-pink-dark);
  font-size: var(--text-xs);
  cursor: pointer;
}
.sv-transcript-expand:hover { text-decoration: underline; }

/* 档 1 精修:左侧 3px 状态色带。既有全局 .sv-task-step::before 是**竖向连线**,
   两者作用在不同侧(连线在左、色带更靠左),故此处只覆盖 border-left 不改连线。 */
.sv-transcript-steps .sv-task-step { border-left-width: 3px; }
.sv-transcript-steps .sv-task-step.done { border-left-color: var(--sv-green); }
.sv-transcript-steps .sv-task-step.active { border-left-color: var(--sv-yellow); }
.sv-transcript-steps .sv-task-step.partial { border-left-color: var(--sv-orange); }
.sv-transcript-steps .sv-task-step.error { border-left-color: var(--sv-red); }
.sv-transcript-steps .sv-task-step.pending { border-left-color: var(--sv-line-strong); }

/* 自定义流程的运行态节点徽标(遗留.md IFW-5;C-2A 时在 TaskExecutionPanel,C-2A→TMSHOW-2
   随计划步骤模板一并迁入本文件 = CSS 单一所有者)。与画布节点卡片的 .flow-tag 同一套
   配色语义(层级灰、严格虚线、子流程实底粉、成果粉线、停用淡),但类名独立——
   那边 scoped 到 AgentFlowNodeCard,跨组件复用会被样式隔离挡掉。 */
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
