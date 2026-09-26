// 对比模式(二维批次 7b)的**运行期只读口径**:被调流程的 token 汇总 + 本轮实际可调用集。
//
// 为什么单独成模块:两者都必须分别与**后端记账**与**后端判定**严格同源——口径副本散在
// 组件里,就会出现「界面说 N 个、运行期只用 M 个」这类漂移(`遗留.md` IFW-12 边界 1/2)。
// 纯函数收在这里,组件只做渲染。
import type { AgentFlowConfig, TaskLlmCall } from '../api/types';

/**
 * 该调用行是否属于**被调流程**(对比模式的动态调用层)。
 *
 * 两类形态都要认:
 *  - `call.<路径>` = 被调流程自身的节点(`task_engine/custom.rs` 的 `run_called_flow` 置 phase);
 *  - `subflow.<路径>` 且路径里含 `d<n>` 段 = 被调流程**内部**挂的静态子图节点
 *    (`phase_for_path` 只认自己那层的 path,于是记成 `subflow.d1.2`)。
 *
 * 只按 `call.` 前缀过滤会漏掉后者,「这次任务被调流程花了多少」就会少算。
 */
export function isDynamicPhase(phase: string): boolean {
  if (phase === 'call' || phase.startsWith('call.')) return true;
  if (phase === 'subflow' || phase.startsWith('subflow.')) {
    return phase.split('.').some((seg) => /^d\d+$/.test(seg));
  }
  return false;
}

/** 被调流程的调用次数与 token 合计(口径与调用面板逐行一致:prompt + completion) */
export interface DynamicCallStats {
  calls: number;
  tokens: number;
}

/** 汇总被调流程的调用次数与 token(数据源 = `GET /api/tasks/{id}/calls` 的全量行) */
export function dynamicCallStats(calls: TaskLlmCall[]): DynamicCallStats {
  let count = 0;
  let tokens = 0;
  for (const call of calls) {
    if (!isDynamicPhase(call.phase)) continue;
    count += 1;
    tokens += call.prompt_tokens + call.completion_tokens;
  }
  return { calls: count, tokens };
}

/**
 * 本轮**实际可调用集** = 名单 ∩ 冻结闭包 − 根流程。
 *
 * 口径与后端 `flow_call::callable_ids` 逐条对齐:trim → 去空 → 去重 → 排根 →
 * 必须在闭包内,**顺序恒取名单声明顺序**(后端据此生成 `run_flow` 描述,顺序与配置一致)。
 *
 * 为什么前端要重算:`tasks.flow_ids` 是**创建时勾选的名单**,而运行期还会剔除已删/停用/
 * 改坏的成员(未绑定任务取宽松口径,见 `agent_flow_service::resolve_members`)——被剔除者
 * 已不在快照闭包里。徽标若直接用名单长度,数字会大于实际可调用数(IFW-12 边界 2)。
 * 两边的输入都是「名单 + 同一份闭包」,故重算结果与运行期取用一致。
 */
export function callableFlows(
  flowIds: string[] | null | undefined,
  flows: AgentFlowConfig[],
  rootId: string | null | undefined,
): AgentFlowConfig[] {
  const out: AgentFlowConfig[] = [];
  const seen = new Set<string>();
  for (const raw of flowIds ?? []) {
    const id = raw.trim();
    if (id === '' || id === rootId || seen.has(id)) continue;
    seen.add(id);
    const flow = flows.find((f) => f.id === id);
    if (flow) out.push(flow);
  }
  return out;
}
