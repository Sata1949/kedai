// 节点级调用预算纯函数测试(A 批 A1/A2:单次调用超时 + 空产出重试)。
//
// 锁三件事:
//   ① 区间常量与后端保存期校验同值(改一端不改另一端即 FAIL);
//   ② 读写口径:空值 → null(整键省略,存量流程逐字节不变)、越界夹取、非法输入 → null、合法值原样;
//   ③ 挂载子流程的节点上两项都被旁路(警示文案点名该纪律),草稿配置保留。
import { describe, expect, it } from 'vitest';
import {
  STEP_CALL_TIMEOUT_MAX,
  STEP_CALL_TIMEOUT_MIN,
  STEP_MAX_RETRIES_MAX,
  STEP_MAX_RETRIES_MIN,
  setStepCallTimeout,
  setStepMaxRetries,
  stepCallTimeoutText,
  stepCallTimeoutWarnings,
  stepMaxRetriesText,
  stepMaxRetriesWarnings,
} from './agentFlowStepLimits';
import type { AgentFlowStep } from '../api/types';

function step(over: Partial<AgentFlowStep> = {}): AgentFlowStep {
  return { id: 'a', name: '起草', enabled: true, goal: 'g', action: 'direct', ...over };
}

describe('agentFlowStepLimits 区间常量(与后端同源)', () => {
  it('超时 30..=3600 秒;重试 1..=5 次', () => {
    // 后端单一出处:server-rs/src/services/agent_flow_service.rs 的
    // MIN/MAX_STEP_CALL_TIMEOUT_SECS 与 MIN/MAX_STEP_MAX_RETRIES
    expect(STEP_CALL_TIMEOUT_MIN).toBe(30);
    expect(STEP_CALL_TIMEOUT_MAX).toBe(3600);
    expect(STEP_MAX_RETRIES_MIN).toBe(1);
    expect(STEP_MAX_RETRIES_MAX).toBe(5);
  });
});

describe('agentFlowStepLimits 单次调用超时读写', () => {
  it('读回:缺省/脏数据 = 空串(表示沿用宿主缺省)', () => {
    expect(stepCallTimeoutText(step())).toBe('');
    expect(stepCallTimeoutText(step({ call_timeout_secs: null }))).toBe('');
    expect(stepCallTimeoutText(step({ call_timeout_secs: 60 }))).toBe('60');
  });

  it('写入:空值/非法输入 = null(整键省略)', () => {
    const s = step({ call_timeout_secs: 60 });
    setStepCallTimeout(s, '');
    expect(s.call_timeout_secs).toBeNull();
    setStepCallTimeout(s, '   ');
    expect(s.call_timeout_secs).toBeNull();
    setStepCallTimeout(s, 'abc');
    expect(s.call_timeout_secs).toBeNull();
    setStepCallTimeout(s, 'NaN');
    expect(s.call_timeout_secs).toBeNull();
  });

  it('写入:越界夹取到 30-3600(可收紧也可放宽)', () => {
    const s = step();
    setStepCallTimeout(s, '5');
    expect(s.call_timeout_secs).toBe(STEP_CALL_TIMEOUT_MIN);
    setStepCallTimeout(s, '99999');
    expect(s.call_timeout_secs).toBe(STEP_CALL_TIMEOUT_MAX);
    // 放宽档同样在区间内保留原值
    setStepCallTimeout(s, '900');
    expect(s.call_timeout_secs).toBe(900);
  });

  it('写入:合法值原样落草稿(小数截断为整数)', () => {
    const s = step();
    setStepCallTimeout(s, '120');
    expect(s.call_timeout_secs).toBe(120);
    setStepCallTimeout(s, '120.9');
    expect(s.call_timeout_secs).toBe(120);
  });

  it('警示:未配置或未挂载子流程时无警示;挂载后点名旁路纪律', () => {
    expect(stepCallTimeoutWarnings(step())).toEqual([]);
    expect(stepCallTimeoutWarnings(step({ call_timeout_secs: 60 }))).toEqual([]);
    const mounted = step({ call_timeout_secs: 60, sub_flow_id: 'flow-x' });
    expect(stepCallTimeoutWarnings(mounted)).toHaveLength(1);
    expect(stepCallTimeoutWarnings(mounted)[0]).toContain('配置保留');
  });
});

describe('agentFlowStepLimits 空产出重试读写', () => {
  it('读回:缺省/脏数据 = 空串(表示不重试)', () => {
    expect(stepMaxRetriesText(step())).toBe('');
    expect(stepMaxRetriesText(step({ max_retries: null }))).toBe('');
    expect(stepMaxRetriesText(step({ max_retries: 2 }))).toBe('2');
  });

  it('写入:空值/非法输入 = null(不重试)', () => {
    const s = step({ max_retries: 3 });
    setStepMaxRetries(s, '');
    expect(s.max_retries).toBeNull();
    setStepMaxRetries(s, '-');
    expect(s.max_retries).toBeNull();
  });

  it('写入:越界夹取到 1-5', () => {
    const s = step();
    setStepMaxRetries(s, '0');
    expect(s.max_retries).toBe(STEP_MAX_RETRIES_MIN);
    setStepMaxRetries(s, '99');
    expect(s.max_retries).toBe(STEP_MAX_RETRIES_MAX);
  });

  it('写入:合法值原样落草稿', () => {
    const s = step();
    setStepMaxRetries(s, '3');
    expect(s.max_retries).toBe(3);
  });

  it('警示:挂载子流程时点名旁路纪律', () => {
    expect(stepMaxRetriesWarnings(step({ max_retries: 2 }))).toEqual([]);
    const mounted = step({ max_retries: 2, sub_flow_id: 'flow-x' });
    expect(stepMaxRetriesWarnings(mounted)).toHaveLength(1);
    expect(stepMaxRetriesWarnings(mounted)[0]).toContain('配置保留');
  });
});
