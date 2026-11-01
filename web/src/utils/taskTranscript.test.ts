import { describe, expect, it } from 'vitest';
import {
  countTrace,
  flattenTrace,
  formatTraceCounts,
  groupTrace,
  TRANSCRIPT_MODE_LABELS,
  TRANSCRIPT_MODES,
  TRACE_KIND_LABELS,
} from './taskTranscript';
import type { TaskTrace } from '../api/types';

// 三档显示的纯函数单测(TMSHOW-2):排序去重、分类计数、按归属分组。
// 三档的 DOM 结构由 components/task/taskTranscript.test.ts 覆盖;本文件只管逻辑。

/** 构造一条轨迹(默认值覆盖大部分字段,单测只关心被断言的那几项) */
function trace(over: Partial<TaskTrace> & { seq: number }): TaskTrace {
  return {
    id: over.seq,
    task_id: 't1',
    phase: 'agent',
    step_index: null,
    kind: 'reasoning',
    name: null,
    content: '内容',
    truncated: false,
    created_at: '2026-11-01T00:00:00.000Z',
    ...over,
  };
}

describe('flattenTrace 排序与去重', () => {
  it('按 seq 升序重排(防御脏顺序)', () => {
    const rows = [trace({ seq: 3 }), trace({ seq: 1 }), trace({ seq: 2 })];
    expect(flattenTrace(rows).map((r) => r.seq)).toEqual([1, 2, 3]);
  });

  it('同 seq 只保留首条(实时帧与补拉结果重叠时的唯一防线)', () => {
    // 真实场景:SSE 增量帧先到,随后断线重连触发 /trace 全量补拉,同一批 seq 又到一次。
    const first = trace({ seq: 7, content: '来自 SSE' });
    const second = trace({ seq: 7, content: '来自补拉' });
    const out = flattenTrace([first, second]);
    expect(out).toHaveLength(1);
    expect(out[0].content).toBe('来自 SSE');
  });

  it('不修改入参数组的顺序(调用方可能持有 store 的同一引用)', () => {
    const rows = [trace({ seq: 2 }), trace({ seq: 1 })];
    const before = rows.map((r) => r.seq);
    flattenTrace(rows);
    expect(rows.map((r) => r.seq)).toEqual(before);
  });

  it('空数组返回空数组', () => {
    expect(flattenTrace([])).toEqual([]);
  });
});

describe('countTrace 分类计数口径', () => {
  it('思考数与 tool_call 数分别计数,tool_result 不计入', () => {
    const rows = [
      trace({ seq: 1, kind: 'reasoning' }),
      trace({ seq: 2, kind: 'reasoning' }),
      trace({ seq: 3, kind: 'tool_call', name: 'bash' }),
      trace({ seq: 4, kind: 'tool_result', name: 'bash' }),
      trace({ seq: 5, kind: 'tool_call', name: 'fs_read' }),
      trace({ seq: 6, kind: 'tool_result', name: 'fs_read' }),
    ];
    // 一次工具调用是 call + result 两行,但只算**一次**调用——
    // 否则「工具 N」会显示成实际调用次数的两倍。
    expect(countTrace(rows)).toEqual({ reasoning: 2, tool: 2, bash: 1 });
  });

  it('bash 只按 tool_call 行计数(不数它的 result 行)', () => {
    const rows = [
      trace({ seq: 1, kind: 'tool_call', name: 'bash' }),
      trace({ seq: 2, kind: 'tool_result', name: 'bash' }),
      trace({ seq: 3, kind: 'tool_result', name: 'bash' }),
    ];
    expect(countTrace(rows)).toEqual({ reasoning: 0, tool: 1, bash: 1 });
  });

  it('step_result 不进任何一项', () => {
    expect(countTrace([trace({ seq: 1, kind: 'step_result' })])).toEqual({
      reasoning: 0,
      tool: 0,
      bash: 0,
    });
  });

  it('空数组三项皆零', () => {
    expect(countTrace([])).toEqual({ reasoning: 0, tool: 0, bash: 0 });
  });
});

describe('groupTrace 按 phase + step_index 分组', () => {
  it('两个维度都要参与:同 phase 不同步骤不混组,同步骤不同 phase 也不混组', () => {
    const rows = [
      trace({ seq: 1, phase: 'agent', step_index: 0 }),
      trace({ seq: 2, phase: 'agent', step_index: 1 }),
      trace({ seq: 3, phase: 'audit', step_index: 0 }),
    ];
    const groups = groupTrace(rows);
    expect(groups).toHaveLength(3);
    expect(groups.map((g) => g.key)).toEqual(['agent:0', 'agent:1', 'audit:0']);
  });

  it('step_index 为 null 与非 null 不混组', () => {
    const rows = [
      trace({ seq: 1, phase: 'subagent', step_index: null }),
      trace({ seq: 2, phase: 'subagent', step_index: 0 }),
    ];
    const groups = groupTrace(rows);
    expect(groups).toHaveLength(2);
    expect(groups[0].key).toBe('subagent:');
    expect(groups[0].stepIndex).toBeNull();
    expect(groups[1].key).toBe('subagent:0');
  });

  it('组的顺序按首条轨迹出现顺序(执行顺序),不是 phase 字典序', () => {
    const rows = [
      trace({ seq: 1, phase: 'step', step_index: 0 }),
      trace({ seq: 2, phase: 'agent', step_index: 0 }),
      trace({ seq: 3, phase: 'step', step_index: 0 }),
    ];
    expect(groupTrace(rows).map((g) => g.phase)).toEqual(['step', 'agent']);
  });

  it('组内仍按 seq 升序,且组内计数与行数逐一相符', () => {
    const rows = [
      trace({ seq: 3, phase: 'agent', step_index: 0, kind: 'tool_call', name: 'bash' }),
      trace({ seq: 1, phase: 'agent', step_index: 0, kind: 'reasoning' }),
      trace({ seq: 2, phase: 'agent', step_index: 0, kind: 'reasoning' }),
    ];
    const [group] = groupTrace(rows);
    expect(group.rows.map((r) => r.seq)).toEqual([1, 2, 3]);
    expect(group.counts).toEqual({ reasoning: 2, tool: 1, bash: 1 });
  });

  it('label 走 phaseText(不另立一份中文映射)', () => {
    const groups = groupTrace([
      trace({ seq: 1, phase: 'agent', step_index: 1 }),
      trace({ seq: 2, phase: 'final_audit', step_index: null }),
      trace({ seq: 3, phase: 'subflow.2', step_index: 0 }),
    ]);
    expect(groups[0].label).toBe('主Agent #2');
    expect(groups[1].label).toBe('终审');
    expect(groups[2].label).toBe('子流程(#3 内) #1');
  });

  it('stepName 透传:步骤类阶段补可读步骤名', () => {
    const [group] = groupTrace([trace({ seq: 1, phase: 'step', step_index: 0 })] as TaskTrace[], (i) =>
      i === 0 ? '调研' : null,
    );
    expect(group.label).toBe('步骤 #1 · 调研');
  });

  it('重复 seq 先被 flattenTrace 去重,不会在同一组里出现两次', () => {
    const rows = [
      trace({ seq: 1, phase: 'agent', step_index: 0 }),
      trace({ seq: 1, phase: 'agent', step_index: 0 }),
    ];
    expect(groupTrace(rows)[0].rows).toHaveLength(1);
  });
});

describe('formatTraceCounts 角标文案', () => {
  it('三项齐备时按「思考 · 工具 · bash」顺序拼接', () => {
    expect(formatTraceCounts({ reasoning: 3, tool: 5, bash: 2 })).toBe('思考 3 · 工具 5 · bash 2');
  });

  it('零值项不显示(角标是「发生了什么」,不是零值清单)', () => {
    expect(formatTraceCounts({ reasoning: 0, tool: 4, bash: 1 })).toBe('工具 4 · bash 1');
    expect(formatTraceCounts({ reasoning: 2, tool: 0, bash: 0 })).toBe('思考 2');
  });

  it('三项全零返回空串(调用方据此不渲染角标)', () => {
    expect(formatTraceCounts({ reasoning: 0, tool: 0, bash: 0 })).toBe('');
  });
});

describe('常量表的穷尽性', () => {
  it('TRANSCRIPT_MODES 与标签表一一对应,顺序为 步骤 → 全消息 → 压缩', () => {
    expect(TRANSCRIPT_MODES).toEqual(['legacy', 'full', 'condensed']);
    for (const mode of TRANSCRIPT_MODES) {
      expect(TRANSCRIPT_MODE_LABELS[mode]).toBeTruthy();
    }
  });

  it('四种轨迹 kind 都有中文标签(漏一种就是一枚空标签)', () => {
    expect(Object.keys(TRACE_KIND_LABELS).sort()).toEqual(
      ['reasoning', 'step_result', 'tool_call', 'tool_result'].sort(),
    );
  });
});
