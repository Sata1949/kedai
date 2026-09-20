// 二维依赖图工具函数测试(二维批次 1/2 前端)。
// 与后端 agent_flow_service 的图原语同口径:线性兼容、上游合法性、层级、成果选拔。
// 只断言数据层(与「画布不做像素断言」同一原则),不涉及 DOM。
import { describe, expect, it } from 'vitest';
import type { AgentFlowStep } from '../api/types';
import {
  computeLevels,
  effectiveInputIds,
  effectiveLevels,
  graphHint,
  isLinearCompat,
  levelOf,
  outputStepName,
  removeStepReferences,
  stepInputs,
  toggleStepInput,
  upstreamCandidates,
  upstreamSummary,
  upstreamWarnings,
  wouldCreateCycle,
} from './agentFlowGraph';

/** 步骤工厂:id/上游/是否生成可定制,其余取一维默认值 */
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

describe('agentFlowGraph 依赖解析', () => {
  it('全部步骤不设上游 = 线性兼容(一维流程)', () => {
    expect(isLinearCompat([step('a'), step('b'), step('c')])).toBe(true);
    expect(isLinearCompat(diamond())).toBe(false);
  });

  it('stepInputs 归一化缺省与脏数据', () => {
    expect(stepInputs({ inputs: undefined })).toEqual([]);
    expect(stepInputs({ inputs: ['a', ''] })).toEqual(['a']);
  });

  it('层级按依赖深度计算,源节点为第 1 层', () => {
    const steps = diamond();
    expect(computeLevels(steps)).toEqual([0, 0, 1, 2]);
    expect(levelOf(steps, 'a')).toBe(1);
    expect(levelOf(steps, 'c')).toBe(2);
    expect(levelOf(steps, 'd')).toBe(3);
  });

  it('成环时层级为 null(编辑器提示,后端亦拒绝保存)', () => {
    const steps = [step('a', ['b']), step('b', ['a'])];
    expect(computeLevels(steps)).toBeNull();
    expect(levelOf(steps, 'a')).toBeNull();
    expect(graphHint(steps)).toContain('环');
  });
});

describe('agentFlowGraph 有效图(执行器视角,画布连线与布局共用)', () => {
  it('线性兼容流程的隐式上游 = 前一步(第 i 步挂第 i-1 步)', () => {
    const steps = [step('a'), step('b'), step('c')];
    expect(effectiveInputIds(steps)).toEqual([[], ['a'], ['b']]);
    // 显式 computeLevels 会把它们全放在第 1 层(线性提示用),有效层级则是链
    expect(computeLevels(steps)).toEqual([0, 0, 0]);
    expect(effectiveLevels(steps)).toEqual([0, 1, 2]);
  });

  it('二维流程的有效上游就是显式上游,不再追加隐式串联边', () => {
    const steps = diamond();
    expect(effectiveInputIds(steps)).toEqual([[], [], ['a', 'b'], ['c']]);
    expect(effectiveLevels(steps)).toEqual([0, 0, 1, 2]);
  });

  it('只要任一步设了上游,整个流程就不再按线性串联补边', () => {
    // 用户把 c 的上游显式设为 a:此时 a、b 都是源节点,b 不再隐式依赖 a
    const steps = [step('a'), step('b'), step('c', ['a'])];
    expect(effectiveInputIds(steps)).toEqual([[], [], ['a']]);
    expect(effectiveLevels(steps)).toEqual([0, 0, 1]);
  });

  it('有效层级在成环时为 null', () => {
    expect(effectiveLevels([step('a', ['b']), step('b', ['a'])])).toBeNull();
  });
});

describe('agentFlowGraph 上游候选与成环拦截', () => {
  it('候选排除自身与下游节点(选中即会成环)', () => {
    const steps = diamond();
    // 给 a 选上游:自身与 a 的全部下游(c、d)都会成环 → 只剩 b 可选
    const ids = upstreamCandidates(steps, 'a').map((s) => s.id);
    expect(ids).toEqual(['b']);
  });

  it('wouldCreateCycle 自环与间接环都判真', () => {
    const steps = diamond();
    expect(wouldCreateCycle(steps, 'a', 'a')).toBe(true);
    expect(wouldCreateCycle(steps, 'a', 'c')).toBe(true); // c 依赖 a → 再加 a→c 成环
    expect(wouldCreateCycle(steps, 'c', 'b')).toBe(false); // b 与 c 无路径
  });

  it('勾选上游就地写入 inputs,可反复切换', () => {
    const c = step('c', ['a']);
    toggleStepInput(c, 'a');
    expect(stepInputs(c)).toEqual([]);
    toggleStepInput(c, 'b');
    expect(stepInputs(c)).toEqual(['b']);
  });

  it('删除步骤后清理其它步骤对它的悬空引用', () => {
    const steps = [step('a'), step('b'), step('c', ['a', 'b']), step('d', ['a'])];
    removeStepReferences(
      steps.filter((s) => s.id !== 'a'),
      'a',
    );
    const c = steps.find((s) => s.id === 'c') as AgentFlowStep;
    const d = steps.find((s) => s.id === 'd') as AgentFlowStep;
    expect(stepInputs(c)).toEqual(['b']);
    expect(stepInputs(d)).toEqual([]);
  });
});

describe('agentFlowGraph 提示与成果节点', () => {
  it('上游摘要区分线性串联 / 源节点 / 多上游', () => {
    const linear = [step('a'), step('b')];
    expect(upstreamSummary(linear, linear[1])).toContain('线性串联');

    const steps = diamond();
    expect(upstreamSummary(steps, steps[0])).toContain('源节点');
    expect(upstreamSummary(steps, steps[2])).toContain('上游:a、b');
  });

  it('上游缺失或停用时给出警示(后端会拒绝保存)', () => {
    const missing = [step('a', ['ghost'])];
    expect(upstreamWarnings(missing, missing[0])[0]).toContain('上游节点已不存在');

    const disabled = [step('a', [], { enabled: false }), step('b', ['a'])];
    expect(upstreamWarnings(disabled, disabled[1])[0]).toContain('已停用');
    expect(upstreamWarnings(disabled, disabled[1])[0]).toContain('保存会被拒绝');
  });

  it('成果节点:显式标注优先,未标注取无后继汇点,汇点不生成时回退末个生成步', () => {
    // 未标注:汇点 = d(生成步)
    expect(outputStepName(diamond())).toBe('d');
    // 显式标注优先于汇点
    const marked = diamond();
    marked[1].is_output = true;
    expect(outputStepName(marked)).toBe('b');
    // 汇点是反思步(不生成)→ 回退末个生成步(与后端 [生成, 反思] 的旧语义一致)
    const reflectTail = [step('gen'), step('check', ['gen'], { action: 'reflect', generates: undefined })];
    expect(outputStepName(reflectTail)).toBe('gen');
  });

  it('一维流程不给二维提示(不增加噪音)', () => {
    expect(graphHint([step('a'), step('b')])).toBeNull();
  });

  it('二维提示带上并行上限与成果节点', () => {
    const text = graphHint(diamond(), 3);
    expect(text).toContain('最多 3 个');
    expect(text).toContain('成果取「d」');
    // 未配置上限时按默认 2 提示
    expect(graphHint(diamond())).toContain('最多 2 个');
  });
});
