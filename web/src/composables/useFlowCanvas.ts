// 自定义流程画布的数据层(二维批次 3 前端)。
//
// 与画布库解耦:本文件**不 import `@vue-flow/*`**,只产出「节点 / 连线 / 交互结果」,
// 于是能在 node 环境直接单测(jsdom 下 Vue Flow 需要 ResizeObserver/DOMMatrix,挂载即受限),
// 渲染层(AgentFlowCanvas.vue)只负责把它映射成画布库的容器、节点与连线。
//
// 权威校验仍在后端:这里只做「连了会成环就当场拒绝并给中文原因」的即时拦截,
// 与列表视图的勾选口径一致(共用 utils/agentFlowGraph 的图原语)。

import { computed, ref, type ComputedRef, type Ref } from 'vue';
import type { AgentFlowStep } from '../api/types';
import {
  effectiveInputIds,
  effectiveLevels,
  isLinearCompat,
  outputStepId,
  stepInputs,
  wouldCreateCycle,
} from '../utils/agentFlowGraph';
import {
  applyAutoLayout,
  displayPositions,
  hasCustomPositions,
  setNodePosition,
  type FlowPoint,
} from '../utils/agentFlowLayout';

/** 节点卡片数据(渲染层据此画节点,不读 store) */
export interface FlowCanvasNodeData {
  step: AgentFlowStep;
  /** 在流程数组里的下标:线性串联顺序与多父合并顺序都以数组下标为准 */
  index: number;
  /** 有效层级(执行器视角,从 1 起);流程成环时为 null */
  level: number | null;
  /** 是否为本次流程的最终成果节点(D4 选拔结果) */
  isOutput: boolean;
}

export interface FlowCanvasNode {
  id: string;
  /** 画布节点类型名:渲染层据此把节点挂到自定义卡片(AgentFlowNodeCard)上 */
  type: 'flowStep';
  position: FlowPoint;
  data: FlowCanvasNodeData;
}

export interface FlowCanvasEdge {
  id: string;
  source: string;
  target: string;
}

export interface UseFlowCanvas {
  /** 画布内的即时提示(连线被拒 / 已连接 / 已重排);空串 = 无提示 */
  canvasMsg: Ref<string>;
  nodes: ComputedRef<FlowCanvasNode[]>;
  edges: ComputedRef<FlowCanvasEdge[]>;
  /** 是否存在用户拖拽坐标(决定「重新布局」要不要二次确认) */
  hasLayout: ComputedRef<boolean>;
  /** 当前打开 Inspector 的节点 id(null = 未打开) */
  inspectingId: Ref<string | null>;
  /**
   * 建立上游连接(source 成为 target 的上游)。
   * 返回是否已连接:自环 / 重复 / 成环一律拒绝并写 `canvasMsg`,不抛错。
   */
  connect: (source: string, target: string) => boolean;
  /** 断开上游连接(把 source 从 target.inputs 里移除) */
  disconnect: (source: string, target: string) => void;
  /** 节点拖拽结束:把坐标写回步骤(保存流程时才落盘) */
  onNodeDragStop: (id: string, at: FlowPoint) => void;
  /** 按依赖层级重新布局并写回坐标(会覆盖已有坐标,调用方负责二次确认) */
  relayout: () => void;
  inspect: (id: string | null) => void;
}

function label(s: AgentFlowStep): string {
  return s.name || s.id;
}

/**
 * @param steps 取当前流程步骤数组的 getter(通常是 `() => flowDraft.value?.steps ?? []`)。
 *   传 getter 而不是数组:步骤数组会被整体替换(切换流程、重载草稿),getter 才能拿到最新那份。
 */
export function useFlowCanvas(steps: () => AgentFlowStep[]): UseFlowCanvas {
  const canvasMsg = ref('');
  const inspectingId = ref<string | null>(null);

  const nodes = computed<FlowCanvasNode[]>(() => {
    const list = steps();
    const pos = displayPositions(list);
    const levels = effectiveLevels(list);
    const outId = outputStepId(list);
    return list.map((s, i) => ({
      id: s.id,
      type: 'flowStep' as const,
      position: pos[s.id] ?? { x: 0, y: 0 },
      data: {
        step: s,
        index: i,
        level: levels ? levels[i] + 1 : null,
        isOutput: outId !== null && s.id === outId,
      },
    }));
  });

  /**
   * 连线取**有效**上游(`effectiveInputIds`):线性兼容流程会补出隐式串联边,
   * 于是存量一维流程进画布也显示成一条链,与执行器实际跑的图一致。
   * 上游已不存在(悬空引用,保存会被后端拒)的边不画:画到不存在的节点上会让人误以为能跑。
   */
  const edges = computed<FlowCanvasEdge[]>(() => {
    const list = steps();
    const known = new Set(list.map((s) => s.id));
    const out: FlowCanvasEdge[] = [];
    effectiveInputIds(list).forEach((ups, i) => {
      for (const up of ups) {
        if (!known.has(up)) continue;
        out.push({ id: `${up}->${list[i].id}`, source: up, target: list[i].id });
      }
    });
    return out;
  });

  const hasLayout = computed(() => hasCustomPositions(steps()));

  function connect(source: string, target: string): boolean {
    const list = steps();
    const from = list.find((s) => s.id === source);
    const to = list.find((s) => s.id === target);
    if (!from || !to) return false;
    if (source === target) {
      canvasMsg.value = '不能把步骤连到自己:自环会被后端拒绝保存。';
      return false;
    }
    if (stepInputs(to).includes(source)) {
      canvasMsg.value = `「${label(to)}」的上游里已经有「${label(from)}」了。`;
      return false;
    }
    if (wouldCreateCycle(list, target, source)) {
      canvasMsg.value = `「${label(from)}」已经是「${label(to)}」的上游(直接或间接),反向连接会形成环,保存会被拒绝。`;
      return false;
    }
    const wasLinear = isLinearCompat(list);
    to.inputs = [...stepInputs(to), source];
    // 一维流程连上第一条边 = 语义分叉点:此后没勾上游的步骤不再是「上一步」而是源节点。
    // 不说清楚的话,用户会以为只是补了一条边。
    canvasMsg.value = wasLinear
      ? `已连接「${label(from)}」→「${label(to)}」。注意:流程已从「线性串联」变为二维流程,未设上游的步骤会变成源节点(只拿任务目标)。`
      : `已连接「${label(from)}」→「${label(to)}」。`;
    return true;
  }

  function disconnect(source: string, target: string): void {
    const to = steps().find((s) => s.id === target);
    if (!to) return;
    const kept = stepInputs(to).filter((id) => id !== source);
    if (kept.length === stepInputs(to).length) return;
    to.inputs = kept;
    canvasMsg.value = `已断开「${label(to)}」与上游的连接。`;
  }

  function onNodeDragStop(id: string, at: FlowPoint): void {
    setNodePosition(steps(), id, at);
    canvasMsg.value = '已记录节点位置,点「保存执行流程」后生效。';
  }

  function relayout(): void {
    applyAutoLayout(steps());
    canvasMsg.value = '已按依赖层级重新布局,点「保存执行流程」后生效。';
  }

  function inspect(id: string | null): void {
    inspectingId.value = id;
  }

  return {
    canvasMsg,
    nodes,
    edges,
    hasLayout,
    inspectingId,
    connect,
    disconnect,
    onNodeDragStop,
    relayout,
    inspect,
  };
}
