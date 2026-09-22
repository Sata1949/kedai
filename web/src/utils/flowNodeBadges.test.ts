// 运行态节点徽标(遗留.md IFW-5):plan 行 → 流程节点的映射与降级。
//
// 关键事实:plan 行由**过滤后的启用步骤**构造,plan 下标 ≠ 流程数组下标——故映射只认
// `node_id`。本文件既锁「对得上时显示什么」,也锁「对不上时不显示、不猜」的降级三档。
import { describe, expect, it } from 'vitest';
import { nodeBadges, planRowBadges, findNode } from './flowNodeBadges';
import type { AgentFlowConfig, AgentFlowStep } from '../api/types';

function step(over: Partial<AgentFlowStep> & { id: string }): AgentFlowStep {
  return { name: over.id, enabled: true, goal: 'g', action: 'direct', generates: true, ...over };
}

/** 二维流程:a/b 两源 → c(严格,显式成果);d 停用 */
function flow(): AgentFlowConfig {
  return {
    id: 'f1',
    name: '流程一',
    enabled: true,
    steps: [
      step({ id: 'a' }),
      step({ id: 'b' }),
      step({ id: 'c', kind: 'strict', is_output: true, inputs: ['a', 'b'] }),
      step({ id: 'd', enabled: false }),
    ],
  };
}

describe('findNode 节点查找与降级', () => {
  it('按当前流程 id + 节点 id 命中;流程不存在或 id 为空返回 null', () => {
    const lib = [flow()];
    expect(findNode(lib, 'f1', 'c')?.id).toBe('c');
    expect(findNode(lib, 'f1', 'nope')).toBeNull();
    expect(findNode(lib, 'other', 'c')).toBeNull();
    expect(findNode(lib, 'f1', null)).toBeNull();
    expect(findNode([], 'f1', 'c')).toBeNull();
  });
});

describe('nodeBadges 单节点徽标', () => {
  it('二维节点:层级 + 档位 + 成果', () => {
    const f = flow();
    const c = f.steps.find((s) => s.id === 'c')!;
    expect(nodeBadges(f, c)).toEqual([
      { text: '第 2 层', kind: 'level' },
      { text: '严格', kind: 'strict' },
      { text: '成果', kind: 'out' },
    ]);
  });

  it('线性兼容流程也显示层级(隐式串联按执行器视角算)', () => {
    const f: AgentFlowConfig = {
      id: 'f2',
      name: '一维流程',
      enabled: true,
      steps: [step({ id: 'x' }), step({ id: 'y' }), step({ id: 'z', is_output: true })],
    };
    expect(nodeBadges(f, f.steps[0]).map((b) => b.text)).toEqual(['第 1 层']);
    expect(nodeBadges(f, f.steps[1]).map((b) => b.text)).toEqual(['第 2 层']);
    // 成果节点取显式标注的那一个
    expect(nodeBadges(f, f.steps[2])).toContainEqual({ text: '成果', kind: 'out' });
  });

  it('挂子流程的节点报「子流程」而非自己的档位(档位被旁路,报它才是误导)', () => {
    const s = step({ id: 'sub', sub_flow_id: 'other', kind: 'strict' });
    const f: AgentFlowConfig = { id: 'f1', name: 'f', enabled: true, steps: [s] };
    const badges = nodeBadges(f, s);
    expect(badges).toContainEqual({ text: '子流程', kind: 'sub' });
    expect(badges.map((b) => b.text)).not.toContain('严格');
  });

  it('停用节点标「已停用」(任务跑过后流程又被改过的情形)', () => {
    const f = flow();
    const d = f.steps.find((s) => s.id === 'd')!;
    expect(nodeBadges(f, d)).toContainEqual({ text: '已停用', kind: 'off' });
  });
});

describe('planRowBadges plan 行的三档降级', () => {
  it('node_id 命中 → 给出徽标', () => {
    expect(planRowBadges([flow()], 'f1', 'c').map((b) => b.text)).toEqual([
      '第 2 层',
      '严格',
      '成果',
    ]);
  });

  it('库为空 / 流程不在库 / 行上没有 node_id / 节点已删除 → 一律空数组(不猜)', () => {
    expect(planRowBadges([], 'f1', 'c')).toEqual([]);           // 流程库没加载
    expect(planRowBadges([flow()], null, 'c')).toEqual([]);      // 没有 current_flow_id
    expect(planRowBadges([flow()], 'f1', null)).toEqual([]);     // 旧任务的 plan 行
    expect(planRowBadges([flow()], 'f1', undefined)).toEqual([]);
    expect(planRowBadges([flow()], 'f1', 'deleted-node')).toEqual([]);
  });
});
