// Agent 执行流程节点的**连接与轮次**纯函数(二维批次 5b)。
//
// 与后端同口径:
//   - `PlanStep.connection_id` 引用一条 `ConnectionProfile.id`(provider + 模型随该连接);
//     空白串按「未设置」处理(后端 `PlanStep::connection_ref` 同款口径)。
//   - `PlanStep.max_tool_rounds` 为 1-200 的节点级工具轮次上限,缺省沿用全局设置。
//   - 连接引用**不做保存期校验**(流程可导出/跨机导入,连接是本机设置):
//     失效引用在运行期报错,这里只做**编辑期警示**,并在选择器里保位显示。
//
// 从组件抽出为纯函数,便于单测(与 agentFlowTools / agentFlowGraph 同一组织方式)。

import type { AgentFlowStep, ConnectionProfile } from '../api/types';

/** 轮次上限取值范围(与后端 `MAX_TOOL_ROUNDS_LIMIT` / 设置项 `max_tool_rounds` 一致) */
export const MAX_TOOL_ROUNDS_MIN = 1;
export const MAX_TOOL_ROUNDS_MAX = 200;

/** 选择器里「连接」的最小信息(只读:从设置里取,不参与流程草稿的编辑) */
export interface FlowConnectionOption {
  id: string;
  name: string;
  /** 停用的连接不作为候选,但已引用的仍要能显示出来(保位) */
  enabled: boolean;
  /** 展示用:模型名(空 = 未填) */
  model: string;
  connectorType: string;
}

/** 由设置里的连接数组派生出选择器选项(启用在前,按原顺序;名称空则回退 id) */
export function connectionOptions(
  connections: ConnectionProfile[] | null | undefined,
): FlowConnectionOption[] {
  return (connections ?? []).map((c) => ({
    id: c.id,
    name: c.name?.trim() ? c.name : c.id,
    enabled: c.enabled !== false,
    model: c.model ?? '',
    connectorType: c.connector_type,
  }));
}

/** 节点引用的连接 id(空白串按未设置处理;返回 null = 用默认连接) */
export function stepConnectionId(s: Pick<AgentFlowStep, 'connection_id'>): string | null {
  const id = (s.connection_id ?? '').trim();
  return id.length > 0 ? id : null;
}

/** 写入节点连接(空串/空值 = 清除引用,回到默认连接) */
export function setStepConnectionId(s: Pick<AgentFlowStep, 'connection_id'>, id: string): void {
  const t = id.trim();
  s.connection_id = t.length > 0 ? t : null;
}

/** 节点引用的连接选项(未命中返回 null:已删除,或本机设置还没加载出来) */
export function findConnection(
  options: FlowConnectionOption[],
  id: string | null,
): FlowConnectionOption | null {
  if (!id) return null;
  return options.find((o) => o.id === id) ?? null;
}

/**
 * 选择器里「当前值不在候选里」的占位文案(无此情形返回空串)。
 * 与子流程选择器同款:原生 select 不命中任何 option 时会显示为空白,
 * 挂载状态就没有锚点了 —— 这里必须保位显示,且**不静默改绑**。
 */
export function staleConnectionLabel(
  options: FlowConnectionOption[],
  id: string | null,
): string {
  if (!id) return '';
  const hit = findConnection(options, id);
  if (!hit) return `${id}(引用已失效)`;
  return hit.enabled ? '' : `${hit.name}(已停用)`;
}

/**
 * 步骤行的连接摘要(供列表视图一眼看出「本步换了 provider」)。
 * 未指定连接返回 null —— 默认连接是常态,不给列表加噪音(与「线性流程不显示上游摘要」同口径)。
 */
export function connectionSummary(
  options: FlowConnectionOption[],
  s: Pick<AgentFlowStep, 'connection_id'>,
): string | null {
  const id = stepConnectionId(s);
  if (!id) return null;
  const hit = findConnection(options, id);
  if (!hit) return '连接:引用已失效';
  if (!hit.enabled) return `连接:${hit.name}(已停用)`;
  return `连接:${hit.name}`;
}

/**
 * 节点连接警示(空数组 = 无警示)。
 *  - 引用失效:连接已从本机设置里删除 —— 运行期该节点会明确报错(不回退默认连接),
 *    导出到别的机器再导入也会是这一条(流程不绑定本机连接,这是**有意**的);
 *  - 引用停用:连接还在但被停用,运行期同样报错;
 *  - 挂载子流程:本字段被旁路(子图各节点各自解析自己的连接),配置保留。
 */
export function stepConnectionWarnings(
  s: Pick<AgentFlowStep, 'connection_id' | 'sub_flow_id'>,
  options: FlowConnectionOption[],
): string[] {
  const id = stepConnectionId(s);
  if (!id) return [];
  const out: string[] = [];
  const mounted = (s.sub_flow_id ?? '').trim().length > 0;
  if (mounted) {
    out.push(
      '本步骤挂载了子流程:连接选择对其不生效(子图各节点用自己的连接解析),配置保留。',
    );
  }
  const hit = findConnection(options, id);
  if (!hit) {
    out.push(
      `引用的连接「${id}」不在本机设置里(可能已删除,或流程来自别的机器):该节点运行时会报错,请改选一条连接或清除引用。`,
    );
  } else if (!hit.enabled) {
    out.push(
      `引用的连接「${hit.name}」已停用:该节点运行时会报错,请启用它或改选其它连接。`,
    );
  }
  return out;
}

/**
 * 读回节点的轮次上限输入框文本(缺省 = 空串,表示沿用全局)
 */
export function stepToolRoundsText(s: Pick<AgentFlowStep, 'max_tool_rounds'>): string {
  return typeof s.max_tool_rounds === 'number' ? String(s.max_tool_rounds) : '';
}

/**
 * 写入节点的轮次上限:空/非法 → null(沿用全局);合法则夹到 1-200。
 * 夹取而不是拒绝:输入框是数字控件,越界值多半是手滑,夹取后保存期必过。
 */
export function setStepToolRounds(s: Pick<AgentFlowStep, 'max_tool_rounds'>, text: string): void {
  const t = text.trim();
  if (t === '') {
    s.max_tool_rounds = null;
    return;
  }
  const n = Number(t);
  if (!Number.isFinite(n)) {
    s.max_tool_rounds = null;
    return;
  }
  const clamped = Math.min(MAX_TOOL_ROUNDS_MAX, Math.max(MAX_TOOL_ROUNDS_MIN, Math.trunc(n)));
  s.max_tool_rounds = clamped;
}

/**
 * 轮次上限警示(空数组 = 无警示):
 *  ① 严格档下不下发工具,该配置不生效(配置保留,切回宽松档即生效);
 *  ② 未配置工具时它也不生效——只有「宽松档 + 有工具」这一种组合会真的限制循环。
 */
export function stepToolRoundsWarnings(
  s: Pick<AgentFlowStep, 'max_tool_rounds' | 'kind' | 'tools'>,
): string[] {
  if (typeof s.max_tool_rounds !== 'number') return [];
  const out: string[] = [];
  if (s.kind === 'strict') {
    out.push('严格档只有一次模型调用,工具轮次上限不会生效(配置保留,切回宽松档即生效)。');
  } else if (s.tools === null || s.tools === undefined) {
    out.push('本步不使用工具,工具轮次上限不会生效。');
  }
  return out;
}
