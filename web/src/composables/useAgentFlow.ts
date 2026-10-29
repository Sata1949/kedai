// 自定义 Agent 执行流程(custom 模式):流程库选择/新建/复制/删除/导出 + 步骤编辑/拖拽。
// IMPHUB 步骤 C:导入流程与「导出全部流程」收编「综合导入 / 导出」窗口——本 composable
// 只保留编辑与单流程导出;归一/报告等纯函数迁入 utils/importBundle(此处 re-export,
// 既有测试与调用方零改动)。
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

export {
  FLOW_BUNDLE_KEY,
  FLOW_BUNDLE_VERSION,
  importReplaceHint,
  importReportMessage,
  normalizeFlowFile,
} from '../utils/importBundle';

/** 工具模式选择(与后端 tools 语义对齐:null=不使用,[]=全部,list=白名单) */
export const TOOL_MODE_LABELS = {
  none: '不使用工具',
  all: '全部工具',
  list: '白名单',
} as const;

/** 有未保存修改时的导出确认文案(导出取服务端已保存的那一份,不能静默给旧版) */
export const UNSAVED_EXPORT_HINT =
  '有未保存的修改:导出的是**已保存**的版本(不含当前编辑)。要继续吗?\n(先点「保存执行流程」即导出最新版)';

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

  /** 导入流程与其冲突处理(新增副本 / 覆盖同 id)已收编「综合导入 / 导出」窗口
   *  (IMPHUB 步骤 C):形状归一与报告文案见 utils/importBundle;段级策略在窗口内选择。 */

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
  // `_id` 只为模板调用签名对齐(`v-on:drop` 侧仍在用),函数体不需要
  function onStepDragOver(e: DragEvent, _id: string): void {
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
    flowLibFlows,
    loadFlowConfig, onFlowSelect, newFlow, duplicateFlow, deleteFlowNow,
    exportFlowNow, saveFlowNow, addStep, removeStep, moveStep, onStepActionChange,
    onStepDragStart, onStepDragOver, onStepDrop, onStepDragEnd,
    // 二维依赖(二维批次 1/2):上游勾选 / 成果标注 / 并行上限
    toggleStepInput, toggleStepOutput, setFlowParallel,
    // 工具模式三态 UI 与后端字段互转(utils/agentFlowTools)
    stepToolMode, setStepToolMode, stepToolsText, setStepToolsText,
  };
}
