// 画布布局纯函数测试(二维批次 3 前端)。
//
// 口径(计划 §四 批次 3 验收断言「测试不断言像素/布局坐标」):这里**不写死任何坐标数值**,
// 只断言节点之间的相对关系与确定性——层号递增则 y 递增、同层 x 按下标递增、宽度居中后
// 仍保持相对次序。这样调整节点尺寸/间距常量不会打爆测试,而布局语义回归一定被抓到。
import { describe, expect, it } from 'vitest';
import type { AgentFlowStep } from '../api/types';
import {
  applyAutoLayout,
  autoLayout,
  displayPositions,
  hasCustomPositions,
  savedPosition,
  setNodePosition,
} from './agentFlowLayout';

/** 步骤工厂(与 agentFlowGraph.test.ts 同形) */
function step(
  id: string,
  inputs: string[] = [],
  overrides: Partial<AgentFlowStep> = {},
): AgentFlowStep {
  return {
    id,
    name: id,
    enabled: true,
    goal: `${id} 目标`,
    action: 'direct',
    generates: true,
    inputs,
    ...overrides,
  };
}

/** 菱形:a、b 源节点 → c 合并 → d 收口 */
function diamond(): AgentFlowStep[] {
  return [step('a'), step('b'), step('c', ['a', 'b']), step('d', ['c'])];
}

describe('agentFlowLayout 自动布局', () => {
  it('布局覆盖全部步骤,且不产生多余条目', () => {
    const steps = diamond();
    const pos = autoLayout(steps);
    expect(Object.keys(pos).sort()).toEqual(['a', 'b', 'c', 'd']);
  });

  it('一维线性流程排成一列:y 随下标递增,x 全部相同', () => {
    const pos = autoLayout([step('a'), step('b'), step('c')]);
    expect(pos.b.y).toBeGreaterThan(pos.a.y);
    expect(pos.c.y).toBeGreaterThan(pos.b.y);
    expect(pos.b.x).toBe(pos.a.x);
    expect(pos.c.x).toBe(pos.a.x);
  });

  it('同层节点 y 相同且 x 按下标递增;下游层整体在更下方', () => {
    const steps = diamond();
    const pos = autoLayout(steps);
    // a、b 同层(源节点)
    expect(pos.b.y).toBe(pos.a.y);
    expect(pos.b.x).toBeGreaterThan(pos.a.x);
    // c 在第 2 层、d 在第 3 层
    expect(pos.c.y).toBeGreaterThan(pos.a.y);
    expect(pos.d.y).toBeGreaterThan(pos.c.y);
  });

  it('同层只有一个节点时被居中,但仍严格位于其上游层之下', () => {
    const steps = [step('a'), step('b'), step('c', ['a', 'b'])];
    const pos = autoLayout(steps);
    const mid = (pos.a.x + pos.b.x) / 2;
    expect(pos.c.x).toBe(mid);
    expect(pos.c.y).toBeGreaterThan(pos.a.y);
  });

  it('存在环时退化为单列而不是抛错(画布仍要能显示问题节点)', () => {
    const steps = [step('a', ['b']), step('b', ['a'])];
    const pos = autoLayout(steps);
    expect(Object.keys(pos).sort()).toEqual(['a', 'b']);
    expect(pos.b.y).toBeGreaterThan(pos.a.y);
  });

  it('空流程返回空布局', () => {
    expect(autoLayout([])).toEqual({});
  });

  it('同一输入两次调用结果一致(布局必须可复现)', () => {
    const steps = diamond();
    expect(autoLayout(steps)).toEqual(autoLayout(diamond()));
  });
});

describe('agentFlowLayout 显示坐标与用户坐标', () => {
  it('用户拖拽过的节点以 x/y 为准,未拖拽的走自动布局', () => {
    const steps = diamond();
    steps[3].x = 999;
    steps[3].y = 888;
    const pos = displayPositions(steps);
    expect(pos.d).toEqual({ x: 999, y: 888 });
    // 其余节点仍按层级给位(不等同于用户坐标)
    expect(pos.a.x).not.toBe(999);
    expect(pos.c.y).toBeLessThan(pos.d.y);
  });

  it('只有一个分量有值时不算已布局(视为未拖拽)', () => {
    const onlyX = step('a');
    onlyX.x = 10;
    expect(savedPosition(onlyX)).toBeNull();

    const both = step('b');
    both.x = 10;
    both.y = 20;
    expect(savedPosition(both)).toEqual({ x: 10, y: 20 });
  });

  it('hasCustomPositions 只在存在完整坐标时为真', () => {
    const steps = diamond();
    expect(hasCustomPositions(steps)).toBe(false);
    steps[0].x = 1;
    expect(hasCustomPositions(steps)).toBe(false);
    steps[0].y = 2;
    expect(hasCustomPositions(steps)).toBe(true);
  });
});

describe('agentFlowLayout 坐标写入', () => {
  it('applyAutoLayout(重新布局)给每个步骤写回坐标', () => {
    const steps = diamond();
    applyAutoLayout(steps);
    for (const s of steps) {
      expect(typeof s.x).toBe('number');
      expect(typeof s.y).toBe('number');
    }
    expect(displayPositions(steps)).toEqual(autoLayout(steps));
  });

  it('setNodePosition 取整写回,未知 id 静默忽略', () => {
    const steps = diamond();
    setNodePosition(steps, 'c', { x: 12.6, y: -3.2 });
    expect(savedPosition(steps[2])).toEqual({ x: 13, y: -3 });
    expect(() => setNodePosition(steps, 'ghost', { x: 1, y: 1 })).not.toThrow();
    expect(steps.every((s) => s.id === 'c' || s.x === undefined)).toBe(true);
  });
});
