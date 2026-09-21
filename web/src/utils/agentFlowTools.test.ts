import { describe, expect, it } from 'vitest';
import type { AgentFlowStep } from '../api/types';
import {
  cleanStepsTools,
  normalizeStepAction,
  cleanStepTools,
  setStepToolMode,
  setStepToolsText,
  stepToolMode,
  stepToolsText,
  stepToolsWarnings,
} from './agentFlowTools';

function step(tools: AgentFlowStep['tools'] = null): AgentFlowStep {
  return {
    id: 's1',
    name: '步骤',
    enabled: true,
    goal: 'g',
    action: 'direct',
    generates: true,
    tools,
    tool_choice: 'auto',
    tool_choice_function: null,
    parallel_tool_calls: null,
  };
}

describe('stepToolMode(读回)', () => {
  it('null/undefined → none,空数组 → all,非空数组 → list', () => {
    expect(stepToolMode(step(null))).toBe('none');
    expect(stepToolMode(step(undefined))).toBe('none');
    expect(stepToolMode(step([]))).toBe('all');
    expect(stepToolMode(step(['write']))).toBe('list');
    expect(stepToolMode(step([''])), '占位空串应视为 list(白名单输入中)').toBe('list');
  });
});

describe('setStepToolMode(切换)', () => {
  it('none → null;all → []', () => {
    const s = step(['write']);
    setStepToolMode(s, 'none');
    expect(s.tools).toBeNull();
    setStepToolMode(s, 'all');
    expect(s.tools).toEqual([]);
  });

  it('list → 占位空串,读回仍为 list(回归:修复前 list 分支置 [] 导致弹回 all)', () => {
    const s = step(null);
    setStepToolMode(s, 'list');
    expect(s.tools).toEqual(['']);
    expect(stepToolMode(s)).toBe('list');
  });

  it('list 分支保留已有非空列表(幂等,不重置为占位)', () => {
    const s = step(['write', 'read']);
    setStepToolMode(s, 'list');
    expect(s.tools).toEqual(['write', 'read']);
  });
});

describe('stepToolsText / setStepToolsText', () => {
  it('白名单文本 → 数组(逗号/中文逗号/空白分隔,去空项)', () => {
    const s = step([]);
    setStepToolsText(s, 'write, read,  计算器');
    expect(s.tools).toEqual(['write', 'read', '计算器']);
    expect(stepToolsText(s)).toBe('write, read, 计算器');
  });

  it('空输入 → 空数组(读回 all 而非 list)', () => {
    const s = step([]);
    setStepToolsText(s, '  ');
    expect(s.tools).toEqual([]);
    expect(stepToolMode(s)).toBe('all');
  });
});

describe('cleanStepTools(保存前清洗)', () => {
  it('null/undefined → null;[] (全部) 原样保留', () => {
    expect(cleanStepTools(null)).toBeNull();
    expect(cleanStepTools(undefined)).toBeNull();
    expect(cleanStepTools([])).toEqual([]);
  });

  it('去空白项;仅占位空串 → null(不使用)', () => {
    expect(cleanStepTools([''])).toBeNull();
    expect(cleanStepTools(['', ' '])).toBeNull();
    expect(cleanStepTools(['write', ' read ', ''])).toEqual(['write', 'read']);
    expect(cleanStepTools(['a', ' b '])).toEqual(['a', 'b']);
  });

  it('cleanStepsTools 就地清洗全部步骤', () => {
    const steps = [step(['write']), step(['']), step([])];
    cleanStepsTools(steps);
    expect(steps[0].tools).toEqual(['write']);
    expect(steps[1].tools).toBeNull();
    expect(steps[2].tools).toEqual([]);
  });
});

describe('stepToolsWarnings(F8:工具三态误配警示)', () => {
  it('全部工具 → 一条警示(点明含编排/写类)', () => {
    const w = stepToolsWarnings({ tools: [], generates: true });
    expect(w).toHaveLength(1);
    expect(w[0]).toContain('全部工具');
    expect(w[0]).toContain('agentgo');
  });

  it('不生成正文 + 全部工具 → 两条警示(第二条点名理解意图)', () => {
    const w = stepToolsWarnings({ tools: [], generates: false });
    expect(w).toHaveLength(2);
    expect(w[1]).toContain('不生成正文');
    expect(w[1]).toContain('理解意图');
  });

  it('不使用工具 / 白名单 → 无警示', () => {
    expect(stepToolsWarnings({ tools: null, generates: false })).toHaveLength(0);
    expect(stepToolsWarnings({ tools: ['read'], generates: false })).toHaveLength(0);
  });
});

describe('normalizeStepAction(切换动作后的字段归一化)', () => {
  it('切反思:生成类字段与「挂载子流程」一并清掉', () => {
    // sub_flow_id 与生成字段同纪律:后端同样拒「反思 + 挂载子流程」,
    // 不清就会「点一下动作下拉,流程立刻不可保存」
    const s = step(['read']);
    s.system_prompt = '本步指令';
    s.temperature = 0.5;
    s.max_tokens = 1024;
    s.sub_flow_id = 'flow-x';
    s.action = 'reflect';

    normalizeStepAction(s);

    expect(s.generates).toBeUndefined();
    expect(s.system_prompt).toBeNull();
    expect(s.temperature).toBeNull();
    expect(s.max_tokens).toBeNull();
    expect(s.tools).toBeNull();
    expect(s.tool_choice).toBe('auto');
    expect(s.tool_choice_function).toBeNull();
    expect(s.parallel_tool_calls).toBeNull();
    expect(s.sub_flow_id ?? null, '切反思必须清掉子流程引用').toBeNull();
  });

  it('切回执行:只补默认 generates,不恢复其他字段', () => {
    const s = step(null);
    s.generates = undefined;
    s.action = 'direct';
    normalizeStepAction(s);
    expect(s.generates).toBe(true);
  });
});
