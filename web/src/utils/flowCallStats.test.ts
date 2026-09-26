// 对比模式的运行期只读口径(遗留.md IFW-12 边界 1/2):被调流程 token 汇总 + 实际可调用集。
//
// 两条口径都必须与后端同源:前者认两类 phase 前缀(`call.` 与「含 d 段的 subflow.」),
// 后者与 `flow_call::callable_ids` 逐条对齐。本文件把「同源」写成可执行的断言。
import { describe, expect, it } from 'vitest';
import { callableFlows, dynamicCallStats, isDynamicPhase } from './flowCallStats';
import type { AgentFlowConfig, TaskLlmCall } from '../api/types';

function call(phase: string, prompt: number, completion: number): TaskLlmCall {
  return {
    id: `${phase}@${prompt}`,
    task_id: 't1',
    phase,
    step_index: null,
    model: 'mock',
    prompt_summary: '',
    response_summary: '',
    prompt_tokens: prompt,
    completion_tokens: completion,
    reasoning_tokens: 0,
    elapsed_ms: 1,
    status: 'ok',
    finish_reason: '',
    created_at: '',
  };
}

function flow(id: string, name?: string): AgentFlowConfig {
  return { id, name: name ?? id, enabled: true, steps: [] };
}

describe('isDynamicPhase 被调流程调用行的两种形态', () => {
  it('`call.<路径>`(被调流程自身节点)命中,含嵌套与子图内发起的调用', () => {
    expect(isDynamicPhase('call')).toBe(true);
    expect(isDynamicPhase('call.d1')).toBe(true);
    expect(isDynamicPhase('call.d1.d2')).toBe(true);
    // 宿主在静态子图内发起:路径前缀是当时的父下标链
    expect(isDynamicPhase('call.3.d1')).toBe(true);
  });

  it('`subflow.<含 d 段路径>`(被调流程内部的静态子图)同样命中——只按 call. 过滤会漏算', () => {
    expect(isDynamicPhase('subflow.d1')).toBe(true);
    expect(isDynamicPhase('subflow.d1.2')).toBe(true);
  });

  it('入口层与本层静态子图不算被调流程', () => {
    expect(isDynamicPhase('step')).toBe(false);
    expect(isDynamicPhase('subflow')).toBe(false);
    expect(isDynamicPhase('subflow.1')).toBe(false);
    expect(isDynamicPhase('subflow.1.2')).toBe(false);
    expect(isDynamicPhase('planner')).toBe(false);
    expect(isDynamicPhase('agent')).toBe(false);
    expect(isDynamicPhase('caller')).toBe(false);
  });
});

describe('dynamicCallStats 被调流程 token 汇总', () => {
  it('只统计动态层,口径与面板逐行一致(prompt + completion)', () => {
    const stats = dynamicCallStats([
      call('step', 10, 5),
      call('call.d1', 100, 40),
      call('subflow.d1.2', 7, 3),
      call('subflow.1', 999, 999),
      call('call.d1.d2', 1, 1),
    ]);
    expect(stats.calls).toBe(3);
    expect(stats.tokens).toBe(152);
  });

  it('没有动态调用时为零(徽标/小结据此不显示)', () => {
    expect(dynamicCallStats([call('step', 10, 5)])).toEqual({ calls: 0, tokens: 0 });
    expect(dynamicCallStats([])).toEqual({ calls: 0, tokens: 0 });
  });
});

describe('callableFlows 实际可调用集(与后端 flow_call::callable_ids 同口径)', () => {
  it('按名单顺序取交集:去空、去重、排根、闭包外剔除', () => {
    const closure = [flow('root'), flow('b', '乙'), flow('c', '丙')];
    const got = callableFlows(['c', ' b ', 'root', 'b', '', 'missing'], closure, 'root');
    expect(got.map((f) => f.id)).toEqual(['c', 'b']);
  });

  it('名单为空/缺省 → 空集(强制模式)', () => {
    expect(callableFlows([], [flow('a')], 'root')).toEqual([]);
    expect(callableFlows(null, [flow('a')], 'root')).toEqual([]);
    expect(callableFlows(undefined, [flow('a')], 'root')).toEqual([]);
  });

  it('成员已从快照闭包消失(运行期剔除)→ 不计入,数字随之下降', () => {
    // 未绑定任务:成员在创建后被删/停用/改坏,resolve_members 已把它剔出闭包
    const closure = [flow('root'), flow('a', '甲')];
    expect(callableFlows(['a', 'gone'], closure, 'root').map((f) => f.id)).toEqual(['a']);
  });

  it('根流程未知(null,旧数据/未绑定且无当前流程)→ 只做去重与闭包过滤', () => {
    expect(callableFlows(['a', 'a'], [flow('a')], null).map((f) => f.id)).toEqual(['a']);
  });
});
