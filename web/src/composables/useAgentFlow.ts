// 自定义 Agent 执行流程(custom 模式):流程库选择/新建/复制/删除/导入/导出 + 步骤编辑/拖拽。
import { computed, ref } from 'vue';
import { useAppStore } from '../store';
import * as api from '../api';
import { downloadBlob } from '../exportFile';
import {
  cleanStepsTools,
  normalizeStepAction,
  setStepToolMode,
  setStepToolsText,
  stepToolMode,
  stepToolsText,
} from '../utils/agentFlowTools';
import {
  removeStepReferences,
  toggleStepInput as toggleStepInputUtil,
  toggleStepOutput as toggleStepOutputUtil,
} from '../utils/agentFlowGraph';

/** 工具模式选择(与后端 tools 语义对齐:null=不使用,[]=全部,list=白名单) */
export const TOOL_MODE_LABELS = {
  none: '不使用工具',
  all: '全部工具',
  list: '白名单',
} as const;

/** 搬运包版本键与版本号(与后端 `agent_flow_service::FLOW_BUNDLE_*` 对齐) */
export const FLOW_BUNDLE_KEY = 'kedai_flow_bundle';
export const FLOW_BUNDLE_VERSION = 1;

/** 有未保存修改时的导出确认文案(导出取服务端已保存的那一份,不能静默给旧版) */
export const UNSAVED_EXPORT_HINT =
  '有未保存的修改:导出的是**已保存**的版本(不含当前编辑)。要继续吗?\n(先点「保存执行流程」即导出最新版)';

/**
 * **覆盖模式**导入的二次确认文案(B 批 B4;覆盖不可逆,故每次导入前都问一遍)。
 * 说清三件事:只动同 id 的那几份、id 不变引用不破、无法撤销——「本机其它流程不动」
 * 一并写明,否则用户会以为整库被替换。
 */
export function importReplaceHint(count: number): string {
  return (
    `本次将导入 ${count} 个流程,并开启「覆盖同名流程」。\n` +
    '与本机同 id 且内容不同的流程会被替换(id 不变,引用不会断),该操作无法撤销;\n' +
    '本机其它流程不受影响。确定继续吗?'
  );
}

/**
 * 归一流程文件 → 待导入的 flows[](+ 可选入口 root_id)。
 *
 * 支持四种形状(旧文件必须继续能导,新增的搬运包才有版本键):
 *  - 搬运包 `{kedai_flow_bundle:1, root_id, flows:[…]}`:整包导入,root_id 一并带上;
 *  - 库格式 `{flows:[…], current_flow_id}`:**整库**导入(此前只取当前/第一个,会静默丢流程);
 *  - `{config:{…}}` 包装:取其中的单流程;
 *  - 单流程 `{name,enabled,steps:[…]}`:一份。
 *
 * 版本键存在但不是 1 → 明确拒绝(新版字段旧端读不懂,静默丢字段比报错更糟)。
 */
export function normalizeFlowFile(raw: unknown): {
  flows: api.AgentFlowConfig[];
  rootId?: string;
} {
  if (!raw || typeof raw !== 'object') {
    throw new Error('无法识别的流程文件:内容不是 JSON 对象');
  }
  const obj = raw as Record<string, unknown>;
  if (obj[FLOW_BUNDLE_KEY] !== undefined && obj[FLOW_BUNDLE_KEY] !== FLOW_BUNDLE_VERSION) {
    throw new Error(
      `流程包版本不支持(${String(obj[FLOW_BUNDLE_KEY])}),请升级 Kedai 后再导入`,
    );
  }
  const asFlow = (v: unknown): api.AgentFlowConfig | null =>
    v && typeof v === 'object' && Array.isArray((v as api.AgentFlowConfig).steps)
      ? (v as api.AgentFlowConfig)
      : null;
  if (Array.isArray(obj.flows)) {
    const flows = (obj.flows as unknown[]).map(asFlow);
    if (flows.length === 0 || flows.some((f) => f === null)) {
      throw new Error('流程文件里的 flows 不含合法流程(每项都需要 steps 数组)');
    }
    // 入口:搬运包用 root_id;库格式用 current_flow_id(否则导入后只选中「第一个」)
    const root =
      typeof obj.root_id === 'string'
        ? obj.root_id
        : typeof obj.current_flow_id === 'string'
          ? obj.current_flow_id
          : undefined;
    return { flows: flows as api.AgentFlowConfig[], rootId: root };
  }
  const single = asFlow(obj.config) ?? asFlow(obj);
  if (!single) {
    throw new Error('无法识别的流程文件:需包含 steps 数组(单流程 / {config} 包装 / 流程库 / 搬运包)');
  }
  return { flows: [single] };
}

/** 导入报告 → 用户文案(导入 N / 跳过 M / 其中 K 个分配了新 id / 覆盖 J 个) */
export function importReportMessage(report: api.FlowImportReport): string {
  const parts = [`已导入 ${report.imported} 个流程`];
  if (report.skipped > 0) parts.push(`跳过 ${report.skipped} 个(内容已存在)`);
  if (report.renamed.length > 0) {
    const names = report.renamed.map((r) => `「${r.name || r.old_id}」`).join('、');
    parts.push(`${report.renamed.length} 个因 id 冲突分配了新 id:${names}`);
  }
  // 覆盖段(B 批 B4):仅 `on_conflict=replace` 才可能非空。容缺读取(replaced 缺失 =
  // 旧服务端的报告,不给用户报「导入失败」),空数组则一字不出现(缺省的 rename 路径不变)
  const replaced = report.replaced ?? [];
  if (replaced.length > 0) {
    const names = replaced.map((r) => `「${r.name || r.id}」`).join('、');
    parts.push(`覆盖 ${replaced.length} 个同 id 流程(本机原有那份已被替换):${names}`);
  }
  return parts.join(';');
}

/** 下载 JSON(文件名净化非法字符后交给 downloadBlob;helper 只管落盘) */
function downloadJson(base: string, payload: unknown): void {
  downloadBlob(
    `${base.replace(/[\\/:*?"<>|]/g, '_')}.json`,
    new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' }),
  );
}

export function useAgentFlow() {
  const store = useAppStore();

  const flowDraft = ref<api.AgentFlowConfig | null>(null);
  const flowSaving = ref(false);
  const flowMsg = ref('');
  const editingStepId = ref<string | null>(null);
  const dragStepId = ref<string | null>(null);
  /** 当前正在编辑的流程 id(库中已存在或新建待保存) */
  const flowId = ref<string | null>(null);
  const flowName = ref('');
  const flowDesc = ref('');
  const flowImportInput = ref<HTMLInputElement | null>(null);
  const flowImporting = ref(false);
  /**
   * 导入的**冲突处理**(B 批 B4):false(缺省)= 现状「新增副本 + 分配新 id」;
   * true = 覆盖本机同 id 那份(不可逆,故 onFlowImport 里每次导入前二次确认)。
   * 默认**不勾选**:覆盖会替换本机既有流程,绝不能是一个「顺手就带上」的默认项。
   */
  const flowImportReplace = ref(false);

  /** 流程库全部流程(选择器选项) */
  const flowLibFlows = computed(() => store.agentFlowLibrary?.flows ?? []);

  /** 从服务端加载流程库并克隆当前选中流程为可编辑草稿(保存时才提交) */
  async function loadFlowConfig(): Promise<void> {
    await store.loadAgentFlow();
    const lib = store.agentFlowLibrary;
    if (!lib) {
      flowDraft.value = null;
      return;
    }
    const cur = lib.flows.find((f) => f.id === lib.current_flow_id) ?? lib.flows[0] ?? null;
    flowDraft.value = cur ? (JSON.parse(JSON.stringify(cur)) as api.AgentFlowConfig) : null;
    flowId.value = cur?.id ?? null;
    flowName.value = cur?.name ?? '';
    flowDesc.value = cur?.description ?? '';
  }

  /** 切换流程:确认后改选中并重载草稿(丢弃未保存修改) */
  async function onFlowSelect(): Promise<void> {
    if (!flowId.value) return;
    if (flowDraft.value && !window.confirm('切换流程将丢弃当前未保存的修改,确定继续?')) {
      flowId.value = flowDraft.value.id ?? null;
      return;
    }
    try {
      await store.selectAgentFlow(flowId.value);
      await loadFlowConfig();
      flowMsg.value = `已切换到「${flowName.value || '未命名流程'}」`;
      setTimeout(() => (flowMsg.value = ''), 2500);
    } catch (e) {
      flowMsg.value = `切换失败:${(e as Error).message}`;
    }
  }

  function newFlowId(): string {
    return (crypto.randomUUID?.() ?? `flow-${Date.now()}`) as string;
  }

  /** 新建流程:空流程立即入库并选中(未启用,校验放行),随后可编辑步骤 */
  async function newFlow(): Promise<void> {
    const count = flowLibFlows.value.length + 1;
    const draft: api.AgentFlowConfig = {
      id: newFlowId(),
      name: `新流程 ${count}`,
      description: '',
      enabled: false,
      steps: [],
    };
    try {
      await store.saveAgentFlowConfig(draft);
      await loadFlowConfig();
      editingStepId.value = null;
      flowMsg.value = '已新建流程,点击「＋ 新增步骤」开始编辑';
      setTimeout(() => (flowMsg.value = ''), 3000);
    } catch (e) {
      flowMsg.value = `新建失败:${(e as Error).message}`;
    }
  }

  /** 复制当前流程(新 id,名称加「副本」,立即入库并选中) */
  async function duplicateFlow(): Promise<void> {
    const cur = flowDraft.value;
    if (!cur || !flowId.value) return;
    const copy: api.AgentFlowConfig = JSON.parse(JSON.stringify(cur)) as api.AgentFlowConfig;
    copy.id = newFlowId();
    copy.name = `${cur.name || '未命名流程'} 副本`;
    try {
      await store.saveAgentFlowConfig(copy);
      await loadFlowConfig();
      flowMsg.value = '已复制当前流程';
      setTimeout(() => (flowMsg.value = ''), 2500);
    } catch (e) {
      flowMsg.value = `复制失败:${(e as Error).message}`;
    }
  }

  /** 删除当前流程(服务端删除当前流程后回退到第一个) */
  async function deleteFlowNow(): Promise<void> {
    if (!flowId.value) return;
    if (!window.confirm(`确定删除流程「${flowName.value || '未命名流程'}」?该操作不可撤销。`)) return;
    try {
      await store.deleteAgentFlow(flowId.value);
      await loadFlowConfig();
      flowMsg.value = '流程已删除';
      setTimeout(() => (flowMsg.value = ''), 2500);
    } catch (e) {
      flowMsg.value = `删除失败:${(e as Error).message}`;
    }
  }

  /** 导入流程 JSON:支持单流程 {name,enabled,steps}、{config:{…}} 包装、库格式 {flows:[…]}
   *  与搬运包 {kedai_flow_bundle:1,…};归一为 flows[] 后**一次**请求导入(失败库不变)。
   *  搬运包版本键不是 1 时明确拒绝(提示升级),不静默丢字段。
   *  B 批 B4:勾选「覆盖同名流程」时以 `on_conflict=replace` 导入——覆盖**不可逆**,
   *  故发请求前二次确认(取消 = 什么都不做,本库一字不动);未勾选则不下发该键(现状)。 */
  async function onFlowImport(e: Event): Promise<void> {
    const input = e.target as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file || flowImporting.value) return;
    flowImporting.value = true;
    flowMsg.value = '';
    try {
      const raw = JSON.parse(await file.text()) as unknown;
      const { flows, rootId } = normalizeFlowFile(raw);
      const onConflict: api.FlowImportConflict | undefined = flowImportReplace.value
        ? 'replace'
        : undefined;
      if (onConflict && !window.confirm(importReplaceHint(flows.length))) return;
      const report = await store.importAgentFlows(flows, rootId, onConflict);
      await loadFlowConfig();
      flowMsg.value = importReportMessage(report);
      setTimeout(() => (flowMsg.value = ''), 4000);
    } catch (err) {
      flowMsg.value = `导入失败:${(err as Error).message}`;
    } finally {
      flowImporting.value = false;
    }
  }

  /** 导出当前流程 + 其**可达子流程闭包**为搬运包(含 id,故子流程引用跨机仍可达) */
  async function exportFlowNow(): Promise<void> {
    const id = flowId.value;
    if (!id) return;
    if (draftDirty() && !window.confirm(UNSAVED_EXPORT_HINT)) return;
    // 导出取的是**服务端已保存**的那一份,故文件名与提示都用库里的名字(草稿名可能还没保存)
    const stored = store.agentFlowLibrary?.flows.find((f) => f.id === id);
    const storedName = stored?.name || flowName.value || '未命名';
    try {
      const bundle = await api.exportAgentFlows(id);
      downloadJson(`kedai-flow-${storedName}`, bundle);
      // 闭包可能不止一个:如实告知,避免用户以为只导了一个
      const extra = bundle.flows.length - 1;
      flowMsg.value =
        extra > 0 ? `已导出「${storedName}」及其 ${extra} 个子流程` : `已导出「${storedName}」`;
      setTimeout(() => (flowMsg.value = ''), 3000);
    } catch (e) {
      flowMsg.value = `导出失败:${(e as Error).message}`;
    }
  }

  /** 导出**全部流程**(库整体搬运:目标机器导入后即为同一套库) */
  async function exportAllFlows(): Promise<void> {
    if (draftDirty() && !window.confirm(UNSAVED_EXPORT_HINT)) return;
    try {
      const bundle = await api.exportAgentFlows();
      downloadJson('kedai-flows-all', bundle);
      flowMsg.value = `已导出全部 ${bundle.flows.length} 个流程`;
      setTimeout(() => (flowMsg.value = ''), 3000);
    } catch (e) {
      flowMsg.value = `导出失败:${(e as Error).message}`;
    }
  }

  /**
   * 草稿与库中那份是否有差异。
   *
   * 为什么需要它:导出取的是**服务端已保存**的那一份(端点只认 id),而用户可能刚改完步骤
   * 还没点保存——不提示就等于**静默**给出旧版本,而导出的用途恰恰是搬到别的机器。
   * 归一化后比较(忽略 id / 键序;名字与说明来自各自的输入框),避免「看起来一样却判为脏」。
   */
  function draftDirty(): boolean {
    const stored = store.agentFlowLibrary?.flows.find((f) => f.id === flowId.value);
    const draft = flowDraft.value;
    if (!stored || !draft) return false;
    const norm = (f: api.AgentFlowConfig, name: string, desc: string): string =>
      JSON.stringify({
        name,
        description: desc || null,
        enabled: f.enabled,
        steps: f.steps,
        max_parallel_nodes: f.max_parallel_nodes ?? null,
      });
    return (
      norm(stored, stored.name ?? '', stored.description ?? '') !==
      norm(draft, flowName.value, flowDesc.value)
    );
  }

  async function saveFlowNow(): Promise<void> {
    if (!flowDraft.value || flowSaving.value) return;
    flowSaving.value = true;
    flowMsg.value = '';
    try {
      flowDraft.value.id = flowId.value ?? undefined;
      flowDraft.value.name = flowName.value;
      flowDraft.value.description = flowDesc.value || null;
      // 保存前清洗各步骤 tools:去空白项;白名单仅剩占位空串 → 不使用;显式 [] (全部)保留
      cleanStepsTools(flowDraft.value.steps);
      await store.saveAgentFlowConfig(flowDraft.value);
      await loadFlowConfig();
      flowMsg.value = '执行流程已保存';
      setTimeout(() => (flowMsg.value = ''), 2500);
    } catch (e) {
      flowMsg.value = `保存失败:${(e as Error).message}`;
    } finally {
      flowSaving.value = false;
    }
  }

  function newStepId(): string {
    return (crypto.randomUUID?.() ?? `s-${Date.now()}`) as string;
  }

  function addStep(): void {
    if (!flowDraft.value) return;
    const id = newStepId();
    flowDraft.value.steps.push({
      id,
      name: '新步骤',
      enabled: true,
      goal: '',
      action: 'direct',
      generates: true,
      system_prompt: null,
      temperature: null,
      max_tokens: null,
      tools: null,
      tool_choice: 'auto',
      tool_choice_function: null,
      parallel_tool_calls: null,
      // 二维字段缺省 = 一维语义(不设上游 = 按列表顺序串联);上游由用户在编辑区勾选
      inputs: [],
      is_output: null,
      // 静态子图(二维批次 6b):缺省不挂载 = 本节点自己生成;挂载由用户在编辑区选择
      sub_flow_id: null,
    });
    editingStepId.value = id;
  }

  function removeStep(id: string): void {
    if (!flowDraft.value) return;
    flowDraft.value.steps = flowDraft.value.steps.filter((s) => s.id !== id);
    // 同步清理其它步骤对它的上游引用:悬空引用会被后端拒绝保存(中文错误)
    removeStepReferences(flowDraft.value.steps, id);
    if (editingStepId.value === id) editingStepId.value = null;
  }

  /** 勾选/取消上游(就地写入步骤 inputs;候选过滤见 utils/agentFlowGraph)。
   *  归一化(按流程数组下标排序)需要全量步骤,故从草稿取 */
  function toggleStepInput(step: api.AgentFlowStep, upstreamId: string): void {
    toggleStepInputUtil(flowDraft.value?.steps ?? [], step, upstreamId);
  }

  /** 切换「最终成果」标注(未勾选写回 null,序列化时省略该字段) */
  function toggleStepOutput(step: api.AgentFlowStep): void {
    toggleStepOutputUtil(step);
  }

  /** 并行节点上限(1-8;空 = 用后端默认 2);越界由后端 400 拦下,前端限定输入范围 */
  function setFlowParallel(value: string): void {
    if (!flowDraft.value) return;
    const n = Number(value);
    flowDraft.value.max_parallel_nodes =
      value.trim() === '' || !Number.isFinite(n) ? null : Math.min(8, Math.max(1, Math.trunc(n)));
  }

  /** ↑/↓ 移动(拖拽的兜底操作;数组顺序即执行顺序) */
  function moveStep(id: string, dir: -1 | 1): void {
    const steps = flowDraft.value?.steps;
    if (!steps) return;
    const idx = steps.findIndex((s) => s.id === id);
    const to = idx + dir;
    if (idx < 0 || to < 0 || to >= steps.length) return;
    [steps[idx], steps[to]] = [steps[to], steps[idx]];
  }

  /** 切换步骤动作:反思步骤不允许生成/系统提示词;直接步骤默认生成 */
  function onStepActionChange(s: api.AgentFlowStep): void {
    normalizeStepAction(s);
  }

  // ===== 步骤拖拽(HTML5 DnD;数组顺序即执行顺序) =====
  function onStepDragStart(e: DragEvent, id: string): void {
    dragStepId.value = id;
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }
  function onStepDragOver(e: DragEvent, id: string): void {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
  }
  function onStepDrop(e: DragEvent, targetId: string): void {
    e.preventDefault();
    const fromId = dragStepId.value;
    dragStepId.value = null;
    if (!fromId || fromId === targetId) return;
    const steps = flowDraft.value?.steps;
    if (!steps) return;
    const from = steps.findIndex((s) => s.id === fromId);
    const to = steps.findIndex((s) => s.id === targetId);
    if (from < 0 || to < 0) return;
    const [item] = steps.splice(from, 1);
    steps.splice(to, 0, item);
  }
  function onStepDragEnd(): void {
    dragStepId.value = null;
  }

  return {
    flowDraft, flowSaving, flowMsg, editingStepId, dragStepId, flowId, flowName, flowDesc,
    flowImportInput, flowImporting, flowImportReplace, flowLibFlows,
    loadFlowConfig, onFlowSelect, newFlow, duplicateFlow, deleteFlowNow, onFlowImport,
    exportFlowNow, exportAllFlows, saveFlowNow, addStep, removeStep, moveStep, onStepActionChange,
    onStepDragStart, onStepDragOver, onStepDrop, onStepDragEnd,
    // 二维依赖(二维批次 1/2):上游勾选 / 成果标注 / 并行上限
    toggleStepInput, toggleStepOutput, setFlowParallel,
    // 工具模式三态 UI 与后端字段互转(utils/agentFlowTools)
    stepToolMode, setStepToolMode, stepToolsText, setStepToolsText,
  };
}
