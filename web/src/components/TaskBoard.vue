<script setup lang="ts">
// 任务详情区(任务模式):当前任务的计划步骤 / 子任务执行 / 最终成果。
// 下达目标与任务历史已移入左侧 Sidebar(单列布局);任务数据持久化到后端 SQLite。
// 批次 4 六模式:模式徽标、plan 模式批准区(批准/放弃/修改后批准)、
// team 模式分工卡与审计结论卡、solo/multi 调用情况面板入口、custom 流程步骤进度。
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useAppStore } from '../store';
import { storeToRefs } from 'pinia';
import { renderMarkdown } from '../markdown';
import { splitTaskResult } from '../taskResult';
import { taskStatusClass as statusClass, taskStatusLabel as statusLabel } from '../taskStatus';
import { bufferLabel } from '../utils/phaseLabel';
import { badgeFlowSource, planRowBadges, type NodeBadge } from '../utils/flowNodeBadges';
import { projectHintTitle, projectKindLabel } from '../utils/workspaceProfile';
import { callableFlows } from '../utils/flowCallStats';
import { flowCandidates, staleMembers, type FlowCandidate } from '../utils/flowCandidates';
import { APPROVE_EXEC_MODE_LABELS, APPROVE_EXEC_MODE_ORDER, FLOW_MODE_LABELS, MODE_LABELS, messageKindLabel } from '../api/labels';
import type { TaskApproveExecMode, TaskRecord, TaskRunMode, TaskStep } from '../api';
import FileChangesPanel from './FileChangesPanel.vue';

const store = useAppStore();
const { currentTask, currentTaskId, model, currentTaskUsage, executorById } = storeToRefs(store);
/** 流程库(自定义流程的节点徽标要按 node_id 对回节点;可能未加载 = null) */
const { agentFlowLibrary } = storeToRefs(store);

/** 最近一条执行进展(主/子 agent 状态简述;store 消费 agent_status 事件,不落库) */
const lastAgentStatus = computed(() => store.lastAgentStatus);

/** 当前任务是否在执行中(planning / running) */
const taskRunning = computed(() => {
  const s = currentTask.value?.task.status;
  return s === 'planning' || s === 'running';
});

/** 当前任务累计 token(prompt + completion;由任务详情 usage_total 带出) */
const taskTotalTokens = computed(() => {
  const u = currentTaskUsage.value;
  return u ? u.prompt_tokens + u.completion_tokens : 0;
});

// ===== 批次 4:六模式呈现 =====

// 模式中文文案统一取自 api/labels.ts(唯一源;穷尽校验见该文件)。
// 原此处手写 MODE_LABELS 与 TaskModeSelect.vue 内联 option 是两份,已收敛。

/** 当前任务执行模式(旧服务端不带 task_mode 时按 legacy 处理) */
const taskMode = computed<TaskRunMode>(() => currentTask.value?.task.task_mode ?? 'legacy');
const taskModeLabel = computed(() => MODE_LABELS[taskMode.value]);

/**
 * 计划步骤的 markdown 预渲染(定向响应式,multi/team 卡顿修复):
 * 原写法在模板里直接 v-html="renderMarkdown(s.result)",每次组件重渲染(事件驱动的
 * 详情刷新)都对全部步骤重跑 markdown + 重设 innerHTML。改为 computed 缓存:
 * 依赖精确到 plan 引用——store 侧签名去重保证数据未变时引用不换,此处零重算;
 * 即便重算后 html 值相同,Vue patch 对 v-html 值不变也跳过 DOM 更新。
 */
interface RenderedStep {
  /** 原始步骤(名称/状态展示用) */
  step: TaskStep;
  /** 预渲染的 result HTML(空结果为空串) */
  html: string;
}
const planRendered = computed<RenderedStep[]>(() =>
  (currentTask.value?.task.plan ?? []).map((s) => ({
    step: s,
    html: s.result ? renderMarkdown(s.result) : '',
  })),
);

// ----- 自定义流程的运行态节点徽标(遗留.md IFW-5;二维批次 5a 起读任务快照)-----
/**
 * 逐行徽标(下标与 plan 对齐;空数组 = 该行不显示)。
 *
 * 映射只认 `node_id`:plan 行由**过滤后的启用步骤**构造,plan 下标 ≠ 流程数组下标,
 * 按下标对齐会在停了中间步骤时整条串位。
 *
 * 数据源择一(二维批次 5a):**本任务的流程快照优先**,没有才回退当前流程库——
 * 于是「跑完任务后改流程 / 换当前流程」不会再让徽标缺失或与当时编排不符。
 * 降级规则(两个数据源都没有/流程不在其中/旧任务没带 node_id/节点已删除 → 不显示)
 * 收在 utils/flowNodeBadges 的 `badgeFlowSource` + `planRowBadges`。
 */
const planBadges = computed<NodeBadge[][]>(() => {
  const rows = currentTask.value?.task.plan ?? [];
  const source = badgeFlowSource(currentTask.value?.flow_snapshot, agentFlowLibrary.value);
  if (taskMode.value !== 'custom' || !source) {
    return rows.map(() => []);
  }
  return rows.map((s) => planRowBadges(source.flows, source.rootId, s.node_id));
});

/**
 * 当前任务用的流程名(二维批次 5a):绑定流程后显示其名称,未绑定显示「跟随当前流程」。
 * 流程名从**快照**里查(与徽标同一份编排),快照缺失时才用当前库/流程 id 兜底。
 */
const taskFlowLabel = computed<string | null>(() => {
  if (taskMode.value !== 'custom') return null;
  const task = currentTask.value?.task;
  if (!task) return null;
  const boundId = task.flow_id;
  if (!boundId) return '跟随当前流程';
  const source = badgeFlowSource(currentTask.value?.flow_snapshot, agentFlowLibrary.value);
  const name = source?.flows.find((f) => f.id === boundId)?.name;
  return name && name.trim() ? name : boundId;
});

/**
 * 对比模式的实际可调用集(二维批次 7b 收口):名单 ∩ 冻结闭包 − 根流程,**与后端运行期
 * 取用同一口径**(`utils/flowCallStats::callableFlows`,对齐 `flow_call::callable_ids`)。
 *
 * 数据源与节点徽标同一份(快照优先、当前库兜底):未绑定任务的失效成员在快照构造时
 * 就已被剔除,故按快照重算即「本轮实际可调用」。两个数据源都取不到(库未加载/为空)
 * 时返回 null,由展示层回退到名单条数,不猜。
 */
const compareCallable = computed(() => {
  if (taskMode.value !== 'custom') return null;
  const ids = currentTask.value?.task.flow_ids ?? [];
  if (ids.length === 0) return null;
  const source = badgeFlowSource(currentTask.value?.flow_snapshot, agentFlowLibrary.value);
  return source ? callableFlows(ids, source.flows, source.rootId) : null;
});

/**
 * 对比模式徽标(二维批次 7b;2026-09-24 改按**实际**可调用集显示)。
 *
 * 旧口径 N 取创建时勾选的名单长度(`task.flow_ids`),而运行期还会剔除根流程与已不可用的
 * 成员——极端情形下数字会大于实际可调用数(遗留.md IFW-12 边界 2)。现在:有快照就以重算
 * 结果为准,与名单条数不等时把差额一并说明;两个数据源都取不到时才回退名单条数。
 */
const taskCompareLabel = computed<string | null>(() => {
  if (taskMode.value !== 'custom') return null;
  const ids = currentTask.value?.task.flow_ids ?? [];
  if (ids.length === 0) return null;
  // 与后端同口径地去重去空后再数「名单几个」:重复/空白项不该被算成两个成员
  const declared = new Set(ids.map((s) => s.trim()).filter((s) => s !== '')).size;
  const callable = compareCallable.value;
  // 「对比」二字取自 api/labels.ts 的 FLOW_MODE_LABELS(单一出处;此前硬拼在展示串里)
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

// ----- B 批 B3:改绑流程(custom 模式;进行中不可改)-----
/**
 * 「改绑流程」入口的可见性:仅 custom 模式,且状态**不在** `planning` / `running` /
 * `planned`(与后端门禁同一口径)。前端不该给出一个必然 400 的入口——进行中改绑会撕裂
 * 快照,待批准态改绑会让批准后跑的那份编排对不上已批准的计划。
 */
const canBindFlow = computed(() => {
  if (taskMode.value !== 'custom') return false;
  const s = currentTask.value?.task.status;
  return s !== 'planning' && s !== 'running' && s !== 'planned';
});

/** 入选区展开态(内联展开,不开模态);切换任务时收起 */
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

/** 展开入选区:草稿从当前绑定起步(取消即丢弃,不写回任何状态) */
function openBind(): void {
  const task = currentTask.value?.task;
  if (!task) return;
  bindRoot.value = task.flow_id ?? '';
  bindIds.value = [...(task.flow_ids ?? [])];
  bindError.value = '';
  bindOpen.value = true;
  // 候选来自流程库:有快照时 ensureFlowLib 不会拉库(徽标自足),而改绑必须有库
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

/** 当前任务是否处于计划待批准(plan 模式 run 后的暂停态) */
const taskPlanned = computed(() => currentTask.value?.task.status === 'planned');

// ----- 批次 R2:多轮用户输入(追加指令 / 用户指令历史) -----
/** 终态(done/partial/error/ended)才可追加指令(与后端 followup 门禁一致) */
const taskTerminal = computed(() => {
  const s = currentTask.value?.task.status;
  return s === 'done' || s === 'partial' || s === 'error' || s === 'ended';
});

/** 用户指令历史(详情 messages 字段;旧服务端无此字段,容错为空数组)。
 *  planned 态时 plan_chat 消息由批准区「与规划器对话」专属渲染(防同屏重复);
 *  其他状态(含完成后的回顾)底部历史区全量显示。 */
const taskMessages = computed(() =>
  (currentTask.value?.messages ?? []).filter((m) => m.kind !== 'plan_chat' || !taskPlanned.value),
);

/** 批准环节规划对话记录(plan_chat 消息;批准区内渲染,批次 R2b) */
const planChatMessages = computed(() =>
  (currentTask.value?.messages ?? []).filter((m) => m.kind === 'plan_chat'),
);

/** 规划对话输入草稿与修订中标记(批次 R2b) */
const planChatDraft = ref('');
const planChatting = ref(false);

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

/** 追加指令输入草稿与发送中标记 */
const followupDraft = ref('');
const followupSending = ref(false);
/** 追加模式(2026-09-10 实跑修复 F5):append=追加进成果,replace=整体重写成果。
 *  append 无法表达「压缩 / 重写 / 改前面」类指令(原文仍在),故提供重写模式。 */
const followupMode = ref<'append' | 'replace'>('append');

/** 非终态时的禁用提示(按状态给出可操作的下一步) */
const followupDisabledHint = computed(() => {
  const s = currentTask.value?.task.status;
  if (s === 'planned') return '计划待批准:请先批准或放弃(批准区可修改计划)';
  if (s === 'pending') return '任务尚未执行:请先执行,产出首轮成果后可追加';
  return '任务执行中:完成或停止后可追加指令';
});

/** 消息种类小标签(文案源见 api/labels.ts;未登记种类返回空串、不显示) */
// 原此处手写 MESSAGE_KIND_LABELS,已收敛到 api/labels.ts 的 messageKindLabel()

/** 助手发言的署名:优先执行者库的名称;旧任务(仅 character_id)回退角色名;
 *  都无绑定(通用执行者)则「任务 Agent」。
 *  执行者优先是本次解耦的要求:任务模式不应再以角色扮演角色卡为执行者署名。 */
const taskMessageAuthor = computed(() => {
  const task = currentTask.value?.task;
  const eid = task?.executor_id;
  if (eid) {
    const e = executorById.value(eid);
    if (e?.name) return e.name;
  }
  const cid = task?.character_id;
  if (cid) {
    const c = store.characters.find((x) => x.id === cid);
    if (c?.chara_name) return c.chara_name;
  }
  return '任务 Agent';
});

/**
 * 对话记录里 assistant 消息的 markdown 预渲染(与 planRendered 同款缓存口径):
 * 依赖精确到 messages 数组引用——store 侧按内容签名去重保证数据未变时引用不换,
 * 事件风暴中这里零重算;按消息 id 缓存避免同一列表反复渲染时重复跑 markdown。
 */
const messageHtmlCache = computed(() => {
  const map = new Map<string, string>();
  for (const m of currentTask.value?.messages ?? []) {
    if (m.role === 'assistant') map.set(m.id, renderMarkdown(m.content));
  }
  return map;
});
function renderMessageHtml(m: { id: string; content: string }): string {
  return messageHtmlCache.value.get(m.id) ?? '';
}

/** 成果汇总卡展开态(默认关闭;持久化在 uiPrefs,跨会话/重启记忆) */
const summaryOpen = computed(() => store.taskResultSummaryOpen);
function toggleSummary(): void {
  store.taskResultSummaryOpen = !store.taskResultSummaryOpen;
}

/** 任务工作区(CODE-1):创建期冻结的 canonical 绝对路径;未绑定为 null/空 → 该行不渲染 */
const taskWorkspace = computed(() => currentTask.value?.task.workspace ?? '');
/** 工作区画像(CODE-4):后端在详情读取时实时探测;未绑定/旧服务端为 null → 该行不渲染 */
const taskWorkspaceProfile = computed(() => currentTask.value?.workspace_profile ?? null);
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

// ----- plan 模式批准区 -----
/** 计划编辑器开关(「修改后批准」展开) */
const planEditing = ref(false);
/** 编辑中的计划副本(name/goal 可改;status/result 保留原值随提交回传) */
const editedPlan = ref<TaskStep[]>([]);
const approving = ref(false);
/** 本次批准的执行方式(2026-09-17):默认「按计划逐步执行」= 改造前行为。
 *  切换任务时随编辑态一并复位,避免把上个任务的选择带到下个任务。 */
const approveExecMode = ref<TaskApproveExecMode>('approved_plan');

// 切换任务时收起编辑态与 pending 操作;追加指令/规划对话草稿一并清空(批次 R2);
// 执行方式选择同时复位为默认(不跨任务沿用);改绑入选区同样收起(草稿属于上一个任务)
watch(currentTaskId, () => {
  planEditing.value = false;
  editedPlan.value = [];
  approveExecMode.value = 'approved_plan';
  followupDraft.value = '';
  planChatDraft.value = '';
  bindOpen.value = false;
  bindError.value = '';
});

/** 进入「修改后批准」:复制当前计划为可编辑副本 */
function startEditPlan(): void {
  const plan = currentTask.value?.task.plan ?? [];
  editedPlan.value = plan.map((s) => ({ ...s }));
  planEditing.value = true;
}

/** 批准执行:plan 为空表示按原计划批准;给了 plan 则替换后逐步执行。
 *  execMode 为本次执行方式(缺省「按计划逐步执行」),后端据此选执行器。 */
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

// ----- team 模式:分工卡(按步骤名「【主Agent-N】」前缀分组;契约见批次 4) -----
/** 分工前缀捕获:【主Agent-N】子目标名 */
const TEAM_PREFIX_RE = /^【(主Agent-\d+)】\s*/;

interface TeamGroup {
  /** 分工标签(如「主Agent-1」;空前缀步骤归为 '' 未分工组) */
  agent: string;
  steps: Array<{ step: TaskStep; index: number }>;
}

const teamGroups = computed<TeamGroup[]>(() => {
  const plan = currentTask.value?.task.plan ?? [];
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

/** 分工卡的 markdown 预渲染(同 planRendered:缓存到 plan 引用维度,事件风暴中不重跑) */
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
      html: it.step.result ? renderMarkdown(it.step.result) : '',
    })),
  })),
);

/** 去掉分工前缀后的步骤名(卡内不再重复显示前缀) */
function teamStepName(name: string): string {
  return name.replace(TEAM_PREFIX_RE, '');
}

// ----- 结果拆卡:team 模式尾部「## 审计结论」/ plan 模式尾部「## 最终计划」各自单独成卡 -----
// (拆段纯函数抽至 ../taskResult,契约与 server-rs task_engine 对齐,单测锁定)
const resultSplit = computed(() => splitTaskResult(taskMode.value, currentTask.value?.task.result ?? ''));

/** 结果卡渲染门控:done/partial 恒渲染;error/ended 在 result 非空时渲染
 * (提交 2 部分成果兜底:汇总失败/取消/重启中断的任务也会带着已完成步骤的产出落库,
 *  不展示等于把已完成的工作藏起来)。
 *  pending/planning/running/planned **一律不渲染**(批次 R1):approve 后的过渡窗口
 *  result 仍持有 planned 态写入的计划清单文本,不门控会把计划清单误显示为「最终成果」;
 *  planned 态的计划清单由批准区内的专属卡渲染(见 plannedPlanHtml)。 */
const resultFinal = computed(() => {
  const s = currentTask.value?.task.status;
  if (s === 'done' || s === 'partial') return true;
  if (s === 'error' || s === 'ended') return !!(currentTask.value?.task.result ?? '').trim();
  return false;
});

/** error/ended 态展示成果时的标注:结果只是「已完成部分的成果」,不是完整交付 */
const resultIncompleteNote = computed(() => {
  const s = currentTask.value?.task.status;
  return (s === 'error' || s === 'ended') && resultFinal.value
    ? '任务未完成,以下为已完成部分的成果'
    : '';
});

/** 最终成果/审计结论/最终计划的 markdown 预渲染(同 planRendered 的缓存口径) */
const resultMainHtml = computed(() =>
  resultFinal.value && resultSplit.value.main ? renderMarkdown(resultSplit.value.main) : '',
);
const resultAuditHtml = computed(() =>
  resultFinal.value && resultSplit.value.audit ? renderMarkdown(resultSplit.value.audit) : '',
);
const resultFinalPlanHtml = computed(() =>
  resultFinal.value && resultSplit.value.finalPlan ? renderMarkdown(resultSplit.value.finalPlan) : '',
);

/** planned 态:批准区内渲染 result 的计划清单文本(批次 R1:planned 态 result 语义 =
 *  待批准的计划清单,markdown 渲染供批准前审阅)。非空时下方「计划步骤」区隐藏(视觉去重:
 *  planned 态步骤全 pending,徽标无信息量;result 为空的旧数据回退显示步骤区) */
const plannedPlanHtml = computed(() => {
  if (!taskPlanned.value || taskMode.value !== 'plan') return '';
  const result = currentTask.value?.task.result ?? '';
  return result ? renderMarkdown(result) : '';
});

/**
 * 「查看调用情况」入口的说明文案(收口批 2026-09-24:custom 此前没有这个入口)。
 * 面板本身与模式无关(通往 Agent 面板的「调用情况」tab);差别只在**细节是什么**:
 * solo/multi 是主/子 Agent 时间线,custom 是逐个流程节点的调用
 * (对比模式下还含被调流程的 `call.` 行,成本数据已在前端单列)。
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
/** 流式缓冲原文(全部活跃调用的攒批增量;单缓冲纯文本,多缓冲(team 并行/子图)带 key 前缀区分)。
 *  多缓冲的 key 是**内部键**(`${phase}:${step_index ?? ''}`),不能直接插值给用户看
 *  (会露出 `【subflow.1:0】` 这类裸 key,见 `遗留.md` IFW-7③)——统一走 utils/phaseLabel 的
 *  中文标签;step 阶段能对上 plan 时补一个步骤名,便于在多缓冲里认出是哪一步。 */
const liveRaw = computed(() => {
  const entries = [...store.liveBuffers.entries()];
  if (entries.length === 1) return entries[0][1];
  const plan = currentTask.value?.task.plan ?? [];
  const stepName = (i: number) => plan[i]?.name || null;
  return entries.map(([k, t]) => `【${bufferLabel(k, stepName)}】\n${t}`).join('\n\n');
});

/**
 * 节流展示文本(100ms):delta 攒批后仍可能每 200ms 一条,渲染只做纯文本插值,
 * 流式期间绝不跑 renderMarkdown(防每 token 重跑);落库后缓冲清空、本块消失,
 * 权威文本由计划步骤/最终成果区的 markdown 渲染接管(即「结束一次 markdown」)。
 * 初始值直接取 liveRaw:SSR/挂载瞬间缓冲已有数据时首帧即含文本。
 */
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

    <!-- 主体:任务详情占满(新建与历史列表在左侧 Sidebar) -->
    <div class="sv-taskboard">
      <!-- 详情区 -->
      <div class="sv-task-detail">
        <template v-if="currentTask">
          <div class="sv-task-head">
            <div class="sv-task-head-title">
              <span class="sv-supreme pink-deep" />
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
                class="sv-btn sv-btn-sm stop"
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

          <!-- 状态 + 模式徽标 + 累计 token + 错误 -->
          <div class="sv-task-status-line">
            <span class="sv-tag" :class="statusClass(currentTask.task.status)">
              {{ statusLabel(currentTask.task.status) }}
            </span>
            <span class="sv-tag sm" title="任务执行模式(批次 4 六模式)">模式:{{ taskModeLabel }}</span>
            <!-- 本任务用的流程(二维批次 5a):绑定流程显示其名,未绑定显示「跟随当前流程」。
                 名字取自任务快照(与徽标同一份编排),故跑完任务后再改流程这里也不会变 -->
            <span
              v-if="taskFlowLabel"
              class="sv-tag sm"
              title="本任务绑定/使用的流程(绑定即冻结:创建时的编排;未绑定 = 跟随当前流程)"
            >流程:{{ taskFlowLabel }}</span>
            <!-- 对比模式(二维批次 7b):根流程照常执行,名单内流程额外作为 run_flow 工具
                 释放给宽松节点,由模型自主调用、取回成果。数字按**本轮实际**可调用集显示 -->
            <span
              v-if="taskCompareLabel"
              class="sv-tag sm"
              :title="taskCompareTitle"
            >{{ taskCompareLabel }}</span>
            <span v-if="taskTotalTokens > 0" class="sv-task-usage">累计 token {{ taskTotalTokens.toLocaleString() }}</span>
            <span v-if="currentTask.task.error" class="sv-task-error">{{ currentTask.task.error }}</span>
          </div>

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

          <!-- 改绑流程(B 批 B3):仅 custom 模式且不在进行中/待批准时给入口
               (后端也会拒:进行中改绑会撕裂快照)。展开是**内联**入选区,不开模态 -->
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

          <!-- 执行中进度行(2026-09-18):后端 agent_status 事件本就携带每轮进展简述,
               此前只用于刷详情、不展示——任务长时间执行时界面完全静止,用户无法区分
               「还在跑」与「已挂死」(实跑反馈:空转 70 秒期间界面无任何变化)。
               此处展示最近一条,给出可见的活性证据。事件不落库,故仅执行中显示。 -->
          <div
            v-if="taskRunning && lastAgentStatus"
            class="sv-task-progress-line"
            title="最近一次执行进展(来自后端事件,事件不落库)"
          >
            <span class="sv-task-progress-dot" />
            <span class="sv-task-progress-text">{{ lastAgentStatus }}</span>
          </div>

          <!-- plan 模式批准区:计划已生成待批准(批准 / 修改后批准 / 放弃) -->
          <div v-if="taskPlanned" class="sv-task-approve">
            <div class="sv-task-section-title">计划待批准</div>
            <!-- 批次 R1:planned 态 result = 待批准的计划清单,批准区内渲染供批准前审阅 -->
            <div v-if="plannedPlanHtml" class="sv-task-result sv-task-planned-plan" v-html="plannedPlanHtml" />
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
                  :disabled="approving || currentTask.task.plan.length === 0"
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
            <template v-else>
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

          <!-- solo / multi / custom 模式:执行细节在 Agent 面板「调用情况」tab
               (custom 收口批 2026-09-24 纳入:对比模式有被调流程成本数据,却没有入口指引) -->
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
          <div v-if="taskMode === 'team' && currentTask.task.plan.length" class="sv-task-section">
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
          <div v-else-if="currentTask.task.plan.length && !plannedPlanHtml" class="sv-task-section">
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

          <!-- 子任务(multi 子 agent 记录)。legacy 不渲染:legacy 的 subtasks 与上方
               「计划步骤」同源重复(每个步骤名会各出现一次),信息以计划步骤区为准
               (2026-09-10 实跑修复 F7) -->
          <div
            v-if="currentTask.subtasks.length && taskMode !== 'legacy'"
            class="sv-task-section"
          >
            <div class="sv-task-section-title">子任务执行</div>
            <div
              v-for="st in currentTask.subtasks"
              :key="st.id"
              class="sv-task-subtask"
            >
              <span class="sv-supreme xs sv-dot-flash" :class="statusClass(st.status)" :key="st.status" />
              <div class="sv-task-subtask-body">
                <div class="sv-task-subtask-name">
                  {{ st.name }}
                  <span class="sv-tag sm" :class="statusClass(st.status)">{{ statusLabel(st.status) }}</span>
                </div>
                <div v-if="st.error" class="sv-task-error">{{ st.error }}</div>
              </div>
            </div>
          </div>

          <!-- 正在生成(批次 R4 流式输出):计划步骤区与最终成果区之间的流式块。
               流式期间纯文本 + 光标(不跑 renderMarkdown,防每 token 重跑);
               调用落库后 store 清缓冲、本块消失,权威文本由最终成果/步骤区 markdown 接管 -->
          <div v-if="liveText" class="sv-task-section sv-task-live">
            <div class="sv-task-section-title">正在生成</div>
            <div class="sv-task-live-text sv-stream-cursor">{{ liveText }}</div>
          </div>

          <!-- 最终结果(team 模式拆尾部「## 审计结论」、plan 模式拆尾部「## 最终计划」
               各自单独成卡;门控见 resultFinal:done/partial 恒渲染,error/ended 仅在
               result 非空时渲染并加「未完成」标注——提交 2 起失败/取消/中断的任务也会
               带着已完成步骤的产出落库,不展示等于把已完成的工作藏起来)。
               实跑问题 1:逐轮对话记录区已是产出的权威视图,本卡默认关闭(与气泡重复),
               可经标题栏开关展开;team 审计结论 / plan 最终计划为独立信息,保持常显。 -->
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
            <div v-if="summaryOpen" class="sv-task-result" v-html="resultMainHtml" />
          </div>
          <!-- 文件变更(批次 4c,PRODCAP-4「交付可审计」):任务结束后「改了什么」的结构化答案。
               数据自取 store(与 CallTracePanel 同款:本组件只负责位置);空清单也**不隐藏卡片**——
               「本轮未改动文件」「基线不可用」「扫描缺项」是三种必须说得出来的不同答案,
               四态渲染纪律见 FileChangesPanel.vue 头注释与 docs/契约.md 台账小节。 -->
          <div class="sv-task-section">
            <div class="sv-task-section-title">文件变更</div>
            <FileChangesPanel />
          </div>
          <div v-if="resultAuditHtml" class="sv-task-section">
            <div class="sv-task-section-title">审计结论</div>
            <div class="sv-task-result sv-task-audit" v-html="resultAuditHtml" />
          </div>
          <div v-if="resultFinalPlanHtml" class="sv-task-section">
            <div class="sv-task-section-title">最终计划</div>
            <div class="sv-task-result sv-task-final-plan" v-html="resultFinalPlanHtml" />
          </div>

          <!-- 对话记录(实跑问题 1):任务目标与各轮产出按 user/assistant 逐轮气泡呈现,
               与 chat 模式同款视觉(用户气泡右对齐、模型气泡左对齐完整 markdown)。
               数据源 = 详情 messages 字段(kind: goal/result/followup/plan_chat);
               plan_chat 在 planned 态由批准区专属渲染,此处跳过防同屏重复。 -->
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
               追加=在既有成果后附加新段(不改原文),重写=用新产出整体替换成果
               (支持「压缩/重写/改前面」类指令);执行中/待批准/未执行时禁用 -->
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

          <div v-if="currentTask.task.status === 'pending'" class="sv-task-empty sv-mt16">
            <p class="sub">任务已创建,点击右上「执行」开始</p>
          </div>
        </template>

        <div v-else class="sv-task-empty fill">
          <div class="sv-empty-geo lg mb14">
            <span class="sq black" />
            <span class="sq pink" />
            <span class="sq deep" />
          <i class="diag" />
          </div>
          <p class="lead">选择或创建一个任务</p>
          <p class="sub">在左侧栏输入目标,系统会拆解计划、派子智能体执行并汇总结果</p>
        </div>
      </div>
    </div>
  </section>
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

/* 执行中进度行(2026-09-18):展示最近一条 agent_status 简述,长循环期间可见活性。
   脉冲圆点用既有动画节奏;文字单行省略,避免长工具名把状态行撑开。 */
.sv-task-progress-line {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin: var(--space-2) 0 0;
  font-size: 0.85em;
  color: var(--sv-ink-dim);
}
.sv-task-progress-text {
  min-width: 0; /* UIP-10:flex 行内省略号前置条件 */
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sv-task-progress-dot {
  flex: none;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--sv-pink-deep);
  animation: sv-progress-pulse var(--dur-pulse) var(--ease-in-out) infinite;
}
@keyframes sv-progress-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.3; }
}

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
