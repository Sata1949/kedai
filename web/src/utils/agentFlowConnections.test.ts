// 节点级连接与工具轮次上限的纯函数测试(二维批次 5b)。
//
// 与后端同口径的三条最容易出错的地方各有用例:
//   ① 空白 connection_id = 「未设置」(后端 `PlanStep::connection_ref` 同款),不能当成
//      「引用了空 id 的连接」——否则流程里手改的空串会变成一个永远失效的引用;
//   ② 引用失效要**保位显示**且给警示(宁可提示,不静默改绑);
//   ③ 轮次上限越界要夹取(数字输入框的手滑不该变成本该被后端 400 拒绝的草稿)。
import { describe, expect, it } from 'vitest';
import {
  MAX_TOOL_ROUNDS_MAX,
  MAX_TOOL_ROUNDS_MIN,
  connectionOptions,
  connectionSummary,
  findConnection,
  setStepConnectionId,
  setStepToolRounds,
  staleConnectionLabel,
  stepConnectionId,
  stepConnectionWarnings,
  stepToolRoundsText,
  stepToolRoundsWarnings,
  type FlowConnectionOption,
} from './agentFlowConnections';
import type { ConnectionProfile } from '../api/types';

function profile(id: string, overrides: Partial<ConnectionProfile> = {}): ConnectionProfile {
  return {
    id,
    name: `连接${id}`,
    connector_type: 'openai-compatible',
    base_url: 'https://api.example/v1',
    model: 'model-x',
    enabled: true,
    api_key_masked: '****abcd',
    has_api_key: true,
    ...overrides,
  };
}

const OPTS: FlowConnectionOption[] = connectionOptions([
  profile('a'),
  profile('b', { name: '外部连接', model: 'model-b' }),
  profile('off', { enabled: false }),
]);

describe('connectionOptions:由设置派生选项', () => {
  it('名称空回退 id、模型带入、停用标记带入', () => {
    const opts = connectionOptions([profile('x', { name: '  ' }), profile('off', { enabled: false })]);
    expect(opts[0]).toEqual({
      id: 'x',
      name: 'x',
      enabled: true,
      model: 'model-x',
      connectorType: 'openai-compatible',
    });
    expect(opts[1].enabled).toBe(false);
  });

  it('null/undefined 数组退化为空选项(设置还没加载出来时不炸)', () => {
    expect(connectionOptions(null)).toEqual([]);
    expect(connectionOptions(undefined)).toEqual([]);
  });
});

describe('stepConnectionId / setStepConnectionId:空白即未设置', () => {
  it('空白串与 undefined 都读回 null(不是「引用了空 id 的连接」)', () => {
    expect(stepConnectionId({})).toBeNull();
    expect(stepConnectionId({ connection_id: null })).toBeNull();
    expect(stepConnectionId({ connection_id: '   ' })).toBeNull();
    expect(stepConnectionId({ connection_id: ' b ' })).toBe('b');
  });

  it('写入时 trim;空串 = 清除引用(回到默认连接)', () => {
    const step: { connection_id?: string | null } = {};
    setStepConnectionId(step, '  b  ');
    expect(step.connection_id).toBe('b');
    setStepConnectionId(step, '');
    expect(step.connection_id).toBeNull();
    expect(stepConnectionId(step)).toBeNull();
  });
});

describe('findConnection / staleConnectionLabel / connectionSummary', () => {
  it('命中与未命中', () => {
    expect(findConnection(OPTS, 'b')?.name).toBe('外部连接');
    expect(findConnection(OPTS, 'missing')).toBeNull();
    expect(findConnection(OPTS, null)).toBeNull();
  });

  it('占位文案三态:无引用空串 / 未命中「已失效」/ 停用「已停用」', () => {
    expect(staleConnectionLabel(OPTS, null)).toBe('');
    expect(staleConnectionLabel(OPTS, 'a')).toBe('');
    expect(staleConnectionLabel(OPTS, 'gone')).toBe('gone(引用已失效)');
    expect(staleConnectionLabel(OPTS, 'off')).toBe('连接off(已停用)');
  });

  it('列表行摘要:未指定连接返回 null(不给一维用户加噪音),失效如实标出', () => {
    expect(connectionSummary(OPTS, {})).toBeNull();
    expect(connectionSummary(OPTS, { connection_id: 'b' })).toBe('连接:外部连接');
    expect(connectionSummary(OPTS, { connection_id: 'gone' })).toBe('连接:引用已失效');
    expect(connectionSummary(OPTS, { connection_id: 'off' })).toBe('连接:连接off(已停用)');
  });
});

describe('stepConnectionWarnings:失效与旁路都要说出来', () => {
  it('无引用无警示', () => {
    expect(stepConnectionWarnings({}, OPTS)).toEqual([]);
  });

  it('命中且启用:无警示', () => {
    expect(stepConnectionWarnings({ connection_id: 'b' }, OPTS)).toEqual([]);
  });

  it('未命中:说清后果(该节点运行时会报错)与出路', () => {
    const [warn] = stepConnectionWarnings({ connection_id: 'gone' }, OPTS);
    expect(warn).toContain('不在本机设置里');
    expect(warn).toContain('运行时会报错');
  });

  it('停用:与「不存在」区分开(下一步动作不同)', () => {
    const [warn] = stepConnectionWarnings({ connection_id: 'off' }, OPTS);
    expect(warn).toContain('已停用');
    expect(warn).not.toContain('不在本机设置里');
  });

  it('挂载子流程:连接被旁路,单独提示(配置保留)', () => {
    const warns = stepConnectionWarnings({ connection_id: 'b', sub_flow_id: 'f2' }, OPTS);
    expect(warns[0]).toContain('挂载了子流程');
    expect(warns[0]).toContain('不生效');
  });
});

describe('setStepToolRounds / stepToolRoundsText:越界夹取', () => {
  it('空串与非法输入 → null(沿用全局)', () => {
    const step: { max_tool_rounds?: number | null } = { max_tool_rounds: 5 };
    setStepToolRounds(step, '  ');
    expect(step.max_tool_rounds).toBeNull();
    setStepToolRounds(step, 'abc');
    expect(step.max_tool_rounds).toBeNull();
  });

  it('越界夹到 1-200(与后端 validate_flow 的区间一致,保存期必过)', () => {
    const step: { max_tool_rounds?: number | null } = {};
    setStepToolRounds(step, '0');
    expect(step.max_tool_rounds).toBe(MAX_TOOL_ROUNDS_MIN);
    setStepToolRounds(step, '9999');
    expect(step.max_tool_rounds).toBe(MAX_TOOL_ROUNDS_MAX);
    setStepToolRounds(step, '12.7');
    expect(step.max_tool_rounds).toBe(12);
  });

  it('读回文本:缺省为空串(输入框显示占位),有值原样', () => {
    expect(stepToolRoundsText({})).toBe('');
    expect(stepToolRoundsText({ max_tool_rounds: null })).toBe('');
    expect(stepToolRoundsText({ max_tool_rounds: 3 })).toBe('3');
  });
});

describe('stepToolRoundsWarnings:不生效的组合要说明', () => {
  it('未配置 → 无警示;宽松 + 有工具 → 无警示', () => {
    expect(stepToolRoundsWarnings({})).toEqual([]);
    expect(stepToolRoundsWarnings({ max_tool_rounds: 3, tools: ['read'] })).toEqual([]);
  });

  it('严格档与「不使用工具」两种不生效组合各说各的', () => {
    const strict = stepToolRoundsWarnings({ max_tool_rounds: 3, kind: 'strict', tools: ['read'] });
    expect(strict[0]).toContain('严格档');
    const noTools = stepToolRoundsWarnings({ max_tool_rounds: 3, tools: null });
    expect(noTools[0]).toContain('不使用工具');
  });
});
