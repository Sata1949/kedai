// 任务运行态的**节点徽标**:把 plan 行(`TaskStep.node_id`)对回当前流程节点,
// 显示这一步在编排里的位置与性质(第 N 层 / 严格档 / 子流程 / 成果节点 / 已停用)。
//
// 为什么需要它:任务侧「流程步骤进度」此前只有步骤名 + 状态徽标,用户看不出
// 「这一步在流程里是第几层、是不是严格档、挂没挂子流程」。而 plan 行**不能按下标**
// 对回流程数组——plan 由过滤后的启用步骤构造(见 `遗留.md` IFW-5),故映射只认 `node_id`。
//
// 降级原则(宁可少显示,不可错显示):
//  - 流程库**未加载**(null)→ 不显示任何徽标(设置区没打开过时库就是空的);
//  - 行上没有 `node_id`(旧任务 / 非 custom 模式)→ 不显示;
//  - `node_id` 在库里找不到(节点已删、流程已换)→ 不显示,由调用方按需另给提示。
import {
  effectiveLevels,
  outputStepId,
  stepKind,
  subFlowId,
} from './agentFlowGraph';
import type { AgentFlowConfig, AgentFlowStep } from '../api/types';

/** 一个节点的运行态徽标(空数组 = 该行不显示徽标) */
export interface NodeBadge {
  /** 徽标文案 */
  text: string;
  /** 样式类(渲染层据此上色;沿用画布节点卡片的 flow-tag 语义) */
  kind: 'level' | 'strict' | 'sub' | 'out' | 'off';
}

/**
 * 在当前流程里按 id 找节点(找不到返回 null)。
 * 与 `flowName` 同一口径:流程库为空/流程不在库里都返回 null,由调用方决定怎么降级。
 */
export function findNode(
  flows: AgentFlowConfig[],
  flowId: string | null | undefined,
  nodeId: string | null | undefined,
): AgentFlowStep | null {
  if (!nodeId) return null;
  const flow = flows.find((f) => f.id === flowId);
  if (!flow) return null;
  return flow.steps.find((s) => s.id === nodeId) ?? null;
}

/**
 * 一个 plan 行的徽标(空数组 = 不显示)。调用方只给「当前流程库 + 当前流程 id + 行的
 * `node_id`」,三档降级都收在这里:库/流程不在、行上没有 id、id 指向的节点已不存在。
 * 把判定集中在一处,是为了让「什么时候该显示」只有一种说法(两个消费方也会一致)。
 */
export function planRowBadges(
  flows: AgentFlowConfig[],
  flowId: string | null | undefined,
  nodeId: string | null | undefined,
): NodeBadge[] {
  const node = findNode(flows, flowId, nodeId);
  if (!node) return [];
  const flow = flows.find((f) => f.id === flowId);
  return flow ? nodeBadges(flow, node) : [];
}

/**
 * 该节点的运行态徽标。`node` 见 {@link findNode}(取不到就不该调用本函数)。
 *
 * 层级用 `effectiveLevels`(执行器视角:线性兼容流程的隐式串联也算进去),
 * 于是存量一维流程的行也能显示「第 N 层」而不互相重叠。
 */
export function nodeBadges(flow: AgentFlowConfig, node: AgentFlowStep): NodeBadge[] {
  const out: NodeBadge[] = [];
  const levels = effectiveLevels(flow.steps);
  const idx = flow.steps.findIndex((s) => s.id === node.id);
  if (levels && idx >= 0) out.push({ text: `第 ${levels[idx] + 1} 层`, kind: 'level' });
  // 档位与子流程互斥(挂子流程的节点不走自己的档位):只报实际生效的那个
  const sub = subFlowId(node);
  if (sub) out.push({ text: '子流程', kind: 'sub' });
  else if (stepKind(node) === 'strict') out.push({ text: '严格', kind: 'strict' });
  if (outputStepId(flow.steps) === node.id) out.push({ text: '成果', kind: 'out' });
  // 停用只可能出现在「任务跑过之后流程又被改过」的情形(plan 只装启用步骤):
  // 此时这张卡是**当下**的流程状态,如实标出来比假装一致更有用(库是实时读的,
  // 任务与流程的绑定要等批次 5a 的 flow_id + 快照)。
  if (!node.enabled) out.push({ text: '已停用', kind: 'off' });
  return out;
}
