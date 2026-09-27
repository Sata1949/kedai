// 自定义流程的二维依赖图工具函数(二维批次 1/2 前端)。
//
// 与后端 `services/agent_flow_service/` 的图原语**同口径**:线性兼容模式(全部步骤
// 不设上游)、上游合法性、层级/就绪规则、成果节点选拔(D4)。这里只做编辑器侧的即时
// 提示与候选过滤,**权威校验仍在后端**(保存被拒时返回中文错误,由 flowMsg 展示)。
// 纯函数便于单测(与 agentFlowTools 同一约定;画布/像素不在断言范围内)。

import type { AgentFlowConfig, AgentFlowStep } from '../api/types';
import { stepToolMode } from './agentFlowTools';

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

/** 显式上游 id → 步骤下标(缺失/脏数据跳过) */
function parentIndices(steps: AgentFlowStep[]): number[][] {
  const index = new Map(steps.map((s, i) => [s.id, i]));
  return steps.map((s) =>
    stepInputs(s)
      .map((id) => index.get(id))
      .filter((i): i is number => i !== undefined),
  );
}

/** Kahn 分层(源节点 = 第 0 层);有环时返回 null */
function levelsFrom(steps: AgentFlowStep[], parents: number[][]): number[] | null {
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

/**
 * 层级(源节点 = 第 1 层):同层节点彼此无依赖,可并行执行(受并行上限约束)。
 * 存在环时返回 null(编辑器禁止保存,后端亦会拒绝)。
 *
 * 注意:这里只认**显式**上游,于是线性兼容流程的每一步都会落在第 1 层——列表视图
 * 据此提示「线性串联」即可。要拿执行器实际使用的层级请用 `effectiveLevels`。
 */
export function computeLevels(steps: AgentFlowStep[]): number[] | null {
  return levelsFrom(steps, parentIndices(steps));
}

/**
 * 有效上游 id(与后端 `effective_inputs` 同口径):线性兼容流程里第 i 步的隐式上游是
 * 第 i-1 步——执行器就是这么串起来的,画布的连线与自动布局必须按同一张图来画,
 * 否则存量一维流程进画布会显示成「一堆互不相干的节点」,与 WF-10 的纵向链预期相悖。
 */
export function effectiveInputIds(steps: AgentFlowStep[]): string[][] {
  const linear = isLinearCompat(steps);
  return steps.map((s, i) => (linear ? (i === 0 ? [] : [steps[i - 1].id]) : stepInputs(s)));
}

/** 有效层级(执行器视角;源节点 = 第 0 层)。有环时返回 null */
export function effectiveLevels(steps: AgentFlowStep[]): number[] | null {
  const linear = isLinearCompat(steps);
  const parents = linear
    ? steps.map((_, i) => (i === 0 ? [] : [i - 1]))
    : parentIndices(steps);
  return levelsFrom(steps, parents);
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

/**
 * 按流程数组下标排序上游 id(未知 id 排在末尾、相对顺序稳定——脏数据由后端校验与
 * `upstreamWarnings` 兜底)。与后端多父合并顺序同一口径。
 */
export function orderUpstreams(steps: AgentFlowStep[], ids: string[]): string[] {
  const index = new Map(steps.map((s, i) => [s.id, i]));
  return [...ids].sort(
    (a, b) =>
      (index.get(a) ?? Number.MAX_SAFE_INTEGER) - (index.get(b) ?? Number.MAX_SAFE_INTEGER),
  );
}

/**
 * 切换上游勾选(就地写入步骤的 inputs)。
 *
 * **归一化:写入后按流程数组下标升序排序** —— 后端多父合并按「父节点在数组中的下标升序」
 * 拼接上游产出,勾选顺序与它无关;若这里不排序,列表摘要显示的顺序会与实际执行的拼接顺序
 * 不一致(遗留 IFW-6,2026-09-21 修)。故需要 `steps` 才能定位下标。
 */
export function toggleStepInput(steps: AgentFlowStep[], step: AgentFlowStep, upstreamId: string): void {
  const now = stepInputs(step);
  const next = now.includes(upstreamId)
    ? now.filter((id) => id !== upstreamId)
    : [...now, upstreamId];
  step.inputs = orderUpstreams(steps, next);
}

/** 切换「最终成果」标注(取消时写回 null,序列化时省略该字段) */
export function toggleStepOutput(step: AgentFlowStep): void {
  step.is_output = step.is_output === true ? null : true;
}

// ---------- 节点档位(二维批次 6a:严格/宽松) ----------

/** 节点档位:loose = 工具自循环(缺省语义);strict = 单次模型调用、不下发任何工具 */
export type StepKind = 'loose' | 'strict';

/** 读回档位:缺省/脏数据一律按 loose(与后端 `PlanStep::is_strict` 同口径) */
export function stepKind(s: Pick<AgentFlowStep, 'kind'>): StepKind {
  return s.kind === 'strict' ? 'strict' : 'loose';
}

/** 切换档位(loose 写回 null,序列化时省略该字段——与后端缺省即宽松的语义一致) */
export function setStepKind(s: Pick<AgentFlowStep, 'kind'>, kind: StepKind): void {
  s.kind = kind === 'strict' ? 'strict' : null;
}

/**
 * 档位警示(空数组 = 无警示)。严格档**优先于**工具配置(后端同一口径:
 * `PlanStep::is_strict` 在任务侧与聊天侧都短路掉工具下发),故这里只提示、**不清空**配置——
 * 用户把档位切回宽松后原配置立即恢复生效。
 */
export function stepKindWarnings(
  s: Pick<AgentFlowStep, 'kind' | 'tools' | 'tool_choice' | 'tool_choice_function'>,
): string[] {
  if (stepKind(s) !== 'strict') return [];
  const out: string[] = [];
  const mode = stepToolMode(s);
  if (mode === 'all') {
    out.push(
      '严格档:本步配的是「全部工具」,但严格档不下发任何工具(配置保留,切回宽松档即生效)。',
    );
  } else if (mode === 'list') {
    const count = (s.tools ?? []).filter((t) => typeof t === 'string' && t.trim().length > 0).length;
    out.push(
      `严格档:已配置的 ${count} 个工具不会下发(配置保留,切回宽松档即生效);严格档只有一次模型调用,靠工具完成的步骤请改回宽松档。`,
    );
  }
  const choice = s.tool_choice ?? 'auto';
  if (choice !== 'auto') {
    out.push(`严格档:工具策略「${choice}」不下发工具,不会生效。`);
  }
  return out;
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
 * 成果节点(D4 规则镜像):显式 `is_output` 优先;未标注取无后继汇点;
 * 候选内取最后一个「生成正文」步骤,候选内没有生成步时回退全流程最后一个生成步。
 */
export function outputStep(steps: AgentFlowStep[]): AgentFlowStep | null {
  const explicit = steps.filter((s) => s.is_output === true);
  const referenced = new Set(steps.flatMap((s) => stepInputs(s)));
  const candidates = explicit.length
    ? explicit
    : steps.filter((s) => !referenced.has(s.id));
  const generating = (list: AgentFlowStep[]) => list.filter(isGeneratingStep);
  return generating(candidates).at(-1) ?? generating(steps).at(-1) ?? null;
}

/** 成果节点 id(画布按 id 标徽标;按名字比对会在重名步骤上出错) */
export function outputStepId(steps: AgentFlowStep[]): string | null {
  return outputStep(steps)?.id ?? null;
}

/**
 * 成果节点名。供编辑器提示「这次任务会以哪一步的产出为最终结果」;
 * 名字可能重复,仅用于展示。
 */
export function outputStepName(steps: AgentFlowStep[]): string | null {
  const pick = outputStep(steps);
  return pick ? pick.name || pick.id : null;
}

/**
 * 二维流程的整体提示(线性流程返回 null,不给用户增加噪音)。
 *
 * B 批 B2:并行不只是「快」——它是**成本**变化。同一层最多 N 个节点同时跑,
 * 每个节点各自发起自己的模型调用,故 token 消耗随之成倍增加(嵌套子流程时还会按
 * 层数相乘)。此前只写「最多 N 个同时跑」,用户读到的是性能提示而非成本提示;
 * 这里把成本口径**显式**写进同一句(AgentFlowSection 的 note 是另一处文案,不改)。
 */
export function graphHint(steps: AgentFlowStep[], maxParallel?: number | null): string | null {
  if (isLinearCompat(steps)) return null;
  if (computeLevels(steps) === null) {
    return '当前流程存在环,无法确定执行顺序:保存会被拒绝。';
  }
  const cap = typeof maxParallel === 'number' && maxParallel > 0 ? maxParallel : 2;
  const output = outputStepName(steps);
  const tail = output ? `,成果取「${output}」的产出` : '';
  return (
    `二维流程:同一层的节点可并行执行(最多 ${cap} 个同时跑)${tail};` +
    `并行会成倍消耗 token——同一层最多 ${cap} 个节点同时跑,每个节点各发起自己的模型调用,` +
    'token 消耗随之成倍增加。'
  );
}

// ---------- 静态子图(二维批次 6b:节点挂载子流程) ----------

/** 子流程嵌套深度上限(与后端 `agent_flow_service::MAX_SUB_FLOW_DEPTH` 同值) */
export const MAX_SUB_FLOW_DEPTH = 3;

/** 读回节点挂载的子流程 id(空串/脏数据 = 未挂载;与后端 `PlanStep::sub_flow_ref` 同口径) */
export function subFlowId(s: Pick<AgentFlowStep, 'sub_flow_id'>): string | null {
  const id = typeof s.sub_flow_id === 'string' ? s.sub_flow_id.trim() : '';
  return id.length > 0 ? id : null;
}

/** 挂载/解除子流程(解除写回 null,序列化时省略该字段) */
export function setSubFlowId(s: Pick<AgentFlowStep, 'sub_flow_id'>, id: string | null): void {
  const trimmed = typeof id === 'string' ? id.trim() : '';
  s.sub_flow_id = trimmed.length > 0 ? trimmed : null;
}

/** 流程的启用步骤(子流程引用只看启用步骤,与后端「停用步骤不参与引用链」同口径) */
function activeSteps(flow: AgentFlowConfig): AgentFlowStep[] {
  return (flow.steps ?? []).filter((s) => s.enabled);
}

/** 流程名(命名为空回退 id;展示与警示共用) */
export function flowName(flows: AgentFlowConfig[], id: string): string {
  const flow = flows.find((f) => f.id === id);
  return flow?.name?.trim() || id;
}

/**
 * 把 candidateId 挂为「本流程」的子流程是否会成环(与后端跨流程环校验同口径):
 * 沿 candidateId 的引用链向下走,遇到 currentFlowId 即闭合成环(A→B→A)。
 * candidateId === currentFlowId(自引用)恒为真。
 */
export function wouldCreateFlowCycle(
  flows: AgentFlowConfig[],
  currentFlowId: string,
  candidateId: string,
): boolean {
  const byId = new Map(flows.map((f) => [f.id, f]));
  const seen = new Set<string>();
  const stack = [candidateId];
  while (stack.length) {
    const cur = stack.pop() as string;
    if (cur === currentFlowId) return true;
    if (seen.has(cur)) continue;
    seen.add(cur);
    const flow = byId.get(cur);
    if (!flow) continue;
    for (const s of activeSteps(flow)) {
      const ref = subFlowId(s);
      if (ref) stack.push(ref);
    }
  }
  return false;
}

/**
 * 可选子流程:排除自身与「引用它会成环」的流程(与 `upstreamCandidates` 同一取舍——
 * 只滤掉**必然错**的候选,深度过深等「保存会被拒」的情况保留在候选里并给警示,
 * 不静默丢弃用户的选择)。
 */
export function subFlowCandidates(
  flows: AgentFlowConfig[],
  currentFlowId: string,
): AgentFlowConfig[] {
  return flows.filter((f) => !!f.id && !wouldCreateFlowCycle(flows, currentFlowId, f.id));
}

/**
 * 流程的嵌套层数(入口流程算 0 层;= 1 + 各子流程节点里最深的那条链)。
 * 成环或引用悬空时返回 null(层数无法确定,由警示文案兜底)。
 */
export function flowNestingDepth(flows: AgentFlowConfig[], flowId: string): number | null {
  const byId = new Map(flows.map((f) => [f.id, f]));
  const visiting = new Set<string>();
  const walk = (id: string): number | null => {
    if (visiting.has(id)) return null;
    const flow = byId.get(id);
    if (!flow) return null;
    visiting.add(id);
    let depth = 0;
    for (const s of activeSteps(flow)) {
      const ref = subFlowId(s);
      if (!ref) continue;
      const child = walk(ref);
      if (child === null) {
        visiting.delete(id);
        return null;
      }
      depth = Math.max(depth, 1 + child);
    }
    visiting.delete(id);
    return depth;
  };
  return walk(flowId);
}

/**
 * 库内**到达**该流程的最长挂载跳数(0 = 没人挂它;`null` = 成环/无法确定)。
 *
 * 与 {@link flowNestingDepth}(向下)相反,本函数沿**反向边**向上走:一个流程自己
 * 可以是根,也可以是被别人挂的子流程——两者都要算进「这条链路有多长」。后端的全库
 * 校验正是从**库里每个流程**起链(`validate_sub_flows`),只看向下的层数会漏掉
 * 「本流程自身已被祖先挂了两层」的情形(前端一条警示都不给,保存却被拒)。
 */
export function flowAncestorDepth(flows: AgentFlowConfig[], flowId: string): number | null {
  const byId = new Map(flows.map((f) => [f.id, f]));
  if (!byId.has(flowId)) return null;
  // 反向边:被引用方 → 引用它的流程(未保存、还没有 id 的流程不入图)
  const parents = new Map<string, string[]>();
  for (const flow of flows) {
    if (!flow.id) continue;
    const parentId = flow.id;
    for (const s of activeSteps(flow)) {
      const ref = subFlowId(s);
      if (!ref) continue;
      const list = parents.get(ref);
      if (list) list.push(parentId);
      else parents.set(ref, [parentId]);
    }
  }
  const visiting = new Set<string>();
  const memo = new Map<string, number>();
  const walk = (id: string): number | null => {
    const cached = memo.get(id);
    if (cached !== undefined) return cached;
    if (visiting.has(id)) return null; // 祖先侧成环:层数无法确定
    visiting.add(id);
    let depth = 0;
    for (const p of parents.get(id) ?? []) {
      const up = walk(p);
      if (up === null) {
        visiting.delete(id);
        return null;
      }
      depth = Math.max(depth, 1 + up);
    }
    visiting.delete(id);
    memo.set(id, depth);
    return depth;
  };
  return walk(flowId);
}

/**
 * 挂载子流程后的**执行旁路说明**(挂载时恒有;它是语义说明而非警示,故与
 * `stepSubFlowWarnings` 分开)。与 6a 的档位同一纪律:配置保留、只是不参与执行。
 * 未挂载(含空串)返回空串,便于模板直接渲染。
 */
export function subFlowMountNote(flows: AgentFlowConfig[], id: string | null): string {
  if (!id) return '';
  return (
    `本节点已挂载子流程「${flowName(flows, id)}」:` +
    '本节点的目标/动作/档位/工具/提示词/温度都不参与执行,子流程成果即本节点产出' +
    '(配置保留,清空子流程即恢复生效)。'
  );
}

/**
 * 子流程引用警示(空数组 = 无警示)。覆盖「保存会被后端拒绝」的五类情形:
 * 悬空引用、被引用流程结构不合法、跨流程环、嵌套超过 {@link MAX_SUB_FLOW_DEPTH} 层、
 * 反思步骤挂子流程。
 *
 * 两个开关与后端同口径:
 *  - **步骤停用** → 整组警示为空(后端 `sub_flow_edges` 只看启用步骤,停用步骤的引用
 *    既不参与执行也不参与校验;编辑器另给一句「启用后才会被校验」的说明);
 *  - **流程停用** → 只有「反思步骤挂子流程」这条不报(它由 `validate_flow` 判,而
 *    `validate_flow` 对未启用流程直接放行);其余几类由 `validate_sub_flows` 判,
 *    那条链路**遍历全库、不跳过未启用流程**,故照报。
 */
export function stepSubFlowWarnings(
  flows: AgentFlowConfig[],
  currentFlowId: string,
  s: Pick<AgentFlowStep, 'sub_flow_id' | 'action' | 'enabled'>,
  flowEnabled = true,
): string[] {
  if (!s.enabled) return [];
  const id = subFlowId(s);
  if (!id) return [];
  const out: string[] = [];
  if (flowEnabled && s.action === 'reflect') {
    out.push('反思步骤不支持挂载子流程(保存会被拒绝):反思步骤不产出正文。');
  }
  const target = flows.find((f) => f.id === id);
  if (!target) {
    out.push(`引用的子流程不存在(保存会被拒绝):${id}`);
    return out;
  }
  // 被引用流程按「启用态」复用同一套结构规则(后端 validate_sub_flows → validate_flow)
  const active = activeSteps(target);
  if (active.length === 0) {
    out.push(
      `被引用的流程「${flowName(flows, id)}」没有启用的步骤(保存会被拒绝):` +
        '子流程至少要有一个启用步骤。',
    );
  } else if (!active.some(isGeneratingStep)) {
    out.push(
      `被引用的流程「${flowName(flows, id)}」缺少生成正文的步骤(保存会被拒绝):` +
        '子流程需要至少一个「生成正文」的执行步骤。',
    );
  }
  if (wouldCreateFlowCycle(flows, currentFlowId, id)) {
    out.push(
      `子流程引用成环(保存会被拒绝):「${flowName(flows, id)}」的引用链上已有本流程` +
        '(嵌套调用会无限递归)。',
    );
  }
  // 本条链路的层数 = 祖先层数 + 本流程到子流程这 1 跳 + 子流程自身的层数。
  // 后端允许链上最多 MAX_SUB_FLOW_DEPTH + 1 个流程,等价于「边数 ≤ MAX_SUB_FLOW_DEPTH」;
  // 任一段算不出(成环/悬空)就跳过深度警示——那是别的警示要覆盖的情形。
  const child = flowNestingDepth(flows, id);
  const ancestor = currentFlowId ? flowAncestorDepth(flows, currentFlowId) : null;
  if (child !== null && ancestor !== null && ancestor + 1 + child > MAX_SUB_FLOW_DEPTH) {
    const total = ancestor + 1 + child + 1;
    out.push(
      `子流程嵌套超过 ${MAX_SUB_FLOW_DEPTH} 层(保存会被拒绝):` +
        `挂上「${flowName(flows, id)}」后本条链路共 ${total} 个流程。`,
    );
  }
  return out;
}

/**
 * 子流程节点的行内摘要(列表视图;未挂载返回空串)。
 * 引用失效时点名「引用已失效」——与画布徽标、编辑器下拉三处同文案,
 * 免得同一个状态在三处各说各话。
 */
export function subFlowSummary(
  flows: AgentFlowConfig[],
  s: Pick<AgentFlowStep, 'sub_flow_id'>,
): string {
  const id = subFlowId(s);
  if (!id) return '';
  const known = flows.some((f) => f.id === id);
  return known ? `子流程:${flowName(flows, id)}` : `子流程:${id}(引用已失效)`;
}
