import { describe, expect, it } from 'vitest';
import {
  countTrace,
  flattenTrace,
  formatTraceContent,
  formatTraceCounts,
  groupTrace,
  requestedTranscriptMode,
  resolveTranscriptMode,
  TRANSCRIPT_MODE_LABELS,
  TRANSCRIPT_MODES,
  TRACE_KIND_LABELS,
} from './taskTranscript';
import type { TaskTrace } from '../api/types';

// 三档显示的纯函数单测(TMSHOW-2 排序去重/分类计数/分组 + TMSHOW-3 档位合成与降级)。
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

describe('formatTraceContent 历史坏数据修复(UIFIX-8)', () => {
  it('被双重转义的工具参数还原成多行结构化 JSON', () => {
    // 提交 1 之前落库的真实形状:整个参数被序列化成 JSON 字符串字面量
    const literal = JSON.stringify('{"command":"ls -la","cwd":"."}');
    const out = formatTraceContent('tool_call', literal);
    expect(out).toContain('\n');
    expect(out).toContain('"command": "ls -la"');
    expect(out).not.toContain('\\"');
  });

  it('纯文本工具结果(bash stdout)原样返回,自己的换行不动', () => {
    const stdout = 'total 8\ndrwxr-xr-x parts\n-rw-r--r-- pelican.svg';
    const literal = JSON.stringify(stdout);
    expect(formatTraceContent('tool_result', literal)).toBe(stdout);
  });

  it('已是多行结构化 JSON 的内容不重复处理(幂等)', () => {
    const pretty = '{\n  "a": 1,\n  "b": [\n    2\n  ]\n}';
    expect(formatTraceContent('tool_result', pretty)).toBe(pretty);
    // 再跑一次仍不变
    expect(formatTraceContent('tool_result', formatTraceContent('tool_result', pretty))).toBe(pretty);
  });

  it('思考类正文原样透传(碰巧是合法 JSON 的思考也不许重排)', () => {
    const thought = '{"looks": "like json", "but": "is prose"}';
    expect(formatTraceContent('reasoning', thought)).toBe(thought);
  });

  it('裸标量与非法 JSON 原样返回(不把 "123"/"true" 当结构化数据重排)', () => {
    expect(formatTraceContent('tool_result', '123')).toBe('123');
    expect(formatTraceContent('tool_result', 'true')).toBe('true');
    expect(formatTraceContent('tool_result', '{not json')).toBe('{not json');
  });

  it('空串原样返回(修复绝不能把可读内容变成空)', () => {
    expect(formatTraceContent('tool_call', '')).toBe('');
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

describe('requestedTranscriptMode 用户选择', () => {
  it('未覆盖时跟随默认档', () => {
    expect(requestedTranscriptMode(null, 'condensed')).toBe('condensed');
    expect(requestedTranscriptMode(undefined, 'legacy')).toBe('legacy');
  });

  it('有覆盖时覆盖优先于默认档', () => {
    expect(requestedTranscriptMode('legacy', 'full')).toBe('legacy');
    expect(requestedTranscriptMode('condensed', 'legacy')).toBe('condensed');
  });

  it('覆盖成与默认档相同的值也是一次覆盖(未来改默认时不跟随)', () => {
    expect(requestedTranscriptMode('full', 'full')).toBe('full');
  });
});

describe('resolveTranscriptMode 生效档位与降级', () => {
  it('有轨迹时各档位原样生效', () => {
    for (const mode of TRANSCRIPT_MODES) {
      expect(resolveTranscriptMode(mode, 'full', true)).toBe(mode);
    }
  });

  it('无轨迹时档 2 / 档 3 降级到档 1(不出现空时间线)', () => {
    expect(resolveTranscriptMode('full', 'full', false)).toBe('legacy');
    expect(resolveTranscriptMode('condensed', 'full', false)).toBe('legacy');
  });

  it('无轨迹时用户本来就选档 1,不重复降级(结果相同但路径不同)', () => {
    expect(resolveTranscriptMode('legacy', 'full', false)).toBe('legacy');
  });

  it('未覆盖 + 无轨迹 → 降到档 1(默认 full 也要降)', () => {
    expect(resolveTranscriptMode(null, 'full', false)).toBe('legacy');
  });

  it('未覆盖 + 有轨迹 → 用默认档', () => {
    expect(resolveTranscriptMode(null, 'condensed', true)).toBe('condensed');
    expect(resolveTranscriptMode(undefined, 'legacy', true)).toBe('legacy');
  });

  it('降级不改用户选择:同一份入参下 requested 与 resolved 可不同', () => {
    // 这条是组件层的口径钉子:分段控件高亮用 requested,渲染分支用 resolved。
    expect(requestedTranscriptMode('condensed', 'full')).toBe('condensed');
    expect(resolveTranscriptMode('condensed', 'full', false)).toBe('legacy');
  });
});
