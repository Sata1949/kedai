// 自定义流程的二维依赖图工具函数(二维批次 1/2 前端)。
//
// 与后端 `services/agent_flow_service.rs` 的图原语**同口径**:线性兼容模式(全部步骤
// 不设上游)、上游合法性、层级/就绪规则、成果节点选拔(D4)。这里只做编辑器侧的即时
// 提示与候选过滤,**权威校验仍在后端**(保存被拒时返回中文错误,由 flowMsg 展示)。
// 纯函数便于单测(与 agentFlowTools 同一约定;画布/像素不在断言范围内)。

import type { AgentFlowStep } from '../api/types';

/** 步骤的上游 id 列表(缺省/脏数据 = 空数组) */
export function stepInputs(s: Pick<AgentFlowStep, 'inputs'>): string[] {
  if (!Array.isArray(s.inputs)) return [];
  return s.inputs.filter((id): id is string => typeof id === 'string' && id.length > 0);
}

/**
 * 线性兼容模式:全部步骤都不设上游 → 执行按列表顺序逐步串联(等价一维流程)。
 * 该模式下「上一步产出」单上游格式与旧版逐字节一致;任一步设了上游即为二维流程。
 */
export function isLinearCompat(steps: AgentFlowStep[]): boolean {
  return steps.every((s) => stepInputs(s).length === 0);
}

/** 步骤是否为「生成正文」的 direct 步骤(成果候选;与后端 is_generating 同口径) */
export function isGeneratingStep(s: Pick<AgentFlowStep, 'action' | 'generates'>): boolean {
  return s.action === 'direct' && s.generates === true;
}

/** 把 upstreamId 设为 targetId 的上游是否会成环:targetId 若已在 upstreamId 的下游链上即成环 */
export function wouldCreateCycle(
  steps: AgentFlowStep[],
  targetId: string,
  upstreamId: string,
): boolean {
  if (targetId === upstreamId) return true;
  const byId = new Map(steps.map((s) => [s.id, s]));
  // 从 upstreamId 出发沿「上游」向上游走:若能到达 targetId,说明 target 是它的祖先,
  // 再加 target→(upstream 为上游) 这条边就闭合成环
  const seen = new Set<string>();
  const stack = [upstreamId];
  while (stack.length) {
    const cur = stack.pop() as string;
    if (cur === targetId) return true;
    if (seen.has(cur)) continue;
    seen.add(cur);
    const node = byId.get(cur);
    if (!node) continue;
    for (const parent of stepInputs(node)) stack.push(parent);
  }
  return false;
}

/** 可选上游:排除自身与会成环的节点;停用/缺失节点保留在候选里(避免用户的选择被静默丢弃) */
export function upstreamCandidates(
  steps: AgentFlowStep[],
  targetId: string,
): AgentFlowStep[] {
  return steps.filter((s) => s.id !== targetId && !wouldCreateCycle(steps, targetId, s.id));
}

/**
 * 层级(源节点 = 第 1 层):同层节点彼此无依赖,可并行执行(受并行上限约束)。
 * 存在环时返回 null(编辑器禁止保存,后端亦会拒绝)。
 */
export function computeLevels(steps: AgentFlowStep[]): number[] | null {
  const index = new Map(steps.map((s, i) => [s.id, i]));
  const parents: number[][] = steps.map((s) =>
    stepInputs(s)
      .map((id) => index.get(id))
      .filter((i): i is number => i !== undefined),
  );
  const level = steps.map(() => 0);
  const pending = parents.map((p) => p.length);
  const queue: number[] = [];
  for (let i = 0; i < steps.length; i++) if (pending[i] === 0) queue.push(i);
  let done = 0;
  let head = 0;
  while (head < queue.length) {
    const i = queue[head++];
    done++;
    for (let j = 0; j < steps.length; j++) {
      if (!parents[j].includes(i)) continue;
      level[j] = Math.max(level[j], level[i] + 1);
      if (--pending[j] === 0) queue.push(j);
    }
  }
  return done === steps.length ? level : null;
}

/** 某步骤的层级(第 N 层,从 1 起);不存在或成环时返回 null */
export function levelOf(steps: AgentFlowStep[], id: string): number | null {
  const levels = computeLevels(steps);
  if (!levels) return null;
  const i = steps.findIndex((s) => s.id === id);
  return i < 0 ? null : levels[i] + 1;
}

/** 删除步骤后就地清理其它步骤对它的上游引用(悬空引用会被后端拒绝保存) */
export function removeStepReferences(steps: AgentFlowStep[], removedId: string): void {
  for (const s of steps) {
    const kept = stepInputs(s).filter((id) => id !== removedId);
    if (kept.length !== stepInputs(s).length) s.inputs = kept;
  }
}

/** 切换上游勾选(就地写入步骤的 inputs) */
export function toggleStepInput(step: AgentFlowStep, upstreamId: string): void {
  const now = stepInputs(step);
  const next = now.includes(upstreamId)
    ? now.filter((id) => id !== upstreamId)
    : [...now, upstreamId];
  // 归一化:保持与流程数组同序(后端按下标升序合并,与勾选顺序无关,此处只是稳定展示)
  step.inputs = next;
}

/** 步骤行的上游摘要(供列表视图一眼看出依赖关系) */
export function upstreamSummary(steps: AgentFlowStep[], s: AgentFlowStep): string {
  const level = levelOf(steps, s.id);
  const levelText = level ? `第 ${level} 层` : '层级未定(存在环)';
  const inputs = stepInputs(s);
  if (inputs.length === 0) {
    const linear = isLinearCompat(steps);
    return linear
      ? `${levelText} · 线性串联(上一步产出)`
      : `${levelText} · 源节点(只用任务目标)`;
  }
  const names = inputs.map((id) => steps.find((x) => x.id === id)?.name || id).join('、');
  return `${levelText} · 上游:${names}`;
}

/** 步骤级警示(为空数组 = 无警示) */
export function upstreamWarnings(steps: AgentFlowStep[], s: AgentFlowStep): string[] {
  const out: string[] = [];
  const byId = new Map(steps.map((x) => [x.id, x]));
  for (const id of stepInputs(s)) {
    const node = byId.get(id);
    if (!node) {
      out.push(`上游节点已不存在(保存会被拒绝):${id}`);
    } else if (!node.enabled) {
      out.push(`上游「${node.name || id}」已停用,保存会被拒绝:请先启用或移除该上游`);
    }
  }
  if (computeLevels(steps) === null) {
    out.push('流程存在环,保存会被拒绝:请检查各步骤的上游设置');
  }
  return out;
}

/**
 * 成果节点名(D4 规则镜像):显式 `is_output` 优先;未标注取无后继汇点;
 * 候选内取最后一个「生成正文」步骤,候选内没有生成步时回退全流程最后一个生成步。
 * 供编辑器提示「这次任务会以哪一步的产出为最终结果」。
 */
export function outputStepName(steps: AgentFlowStep[]): string | null {
  const explicit = steps.filter((s) => s.is_output === true);
  const referenced = new Set(steps.flatMap((s) => stepInputs(s)));
  const candidates = explicit.length
    ? explicit
    : steps.filter((s) => !referenced.has(s.id));
  const generating = (list: AgentFlowStep[]) => list.filter(isGeneratingStep);
  const pick = generating(candidates).at(-1) ?? generating(steps).at(-1) ?? null;
  return pick ? pick.name || pick.id : null;
}

/** 二维流程的整体提示(线性流程返回 null,不给用户增加噪音) */
export function graphHint(steps: AgentFlowStep[], maxParallel?: number | null): string | null {
  if (isLinearCompat(steps)) return null;
  if (computeLevels(steps) === null) {
    return '当前流程存在环,无法确定执行顺序:保存会被拒绝。';
  }
  const cap = typeof maxParallel === 'number' && maxParallel > 0 ? maxParallel : 2;
  const output = outputStepName(steps);
  const tail = output ? `,成果取「${output}」的产出` : '';
  return `二维流程:同一层的节点可并行执行(最多 ${cap} 个同时跑)${tail}。`;
}
