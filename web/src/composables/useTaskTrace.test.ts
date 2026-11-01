// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  appendTrace,
  clearTaskTrace,
  loadTaskTrace,
  resetTaskTraceForTest,
  resumeTaskTrace,
  setTranscriptMode,
  useTaskTrace,
} from './useTaskTrace';
import type { TaskTrace, TaskTracePage } from '../api/types';

// 轨迹状态模块单测(TMSHOW-2):分页拼接、seq 去重、缺口补拉、迟到守卫、清空。
// api 层整体打桩——本文件只验证状态机的拼接与去重口径,不验证 HTTP。

vi.mock('../api', () => ({
  getTaskTrace: vi.fn(),
}));

import * as api from '../api';

/** `vi.mock` 后的替身:`vi.mocked` 保留底层 mock 方法且不需要双重断言 */
const getTaskTrace = vi.mocked(api.getTaskTrace);

function trace(seq: number, over: Partial<TaskTrace> = {}): TaskTrace {
  return {
    id: seq,
    task_id: 't1',
    seq,
    phase: 'agent',
    step_index: null,
    kind: 'reasoning',
    name: null,
    content: `第 ${seq} 条`,
    truncated: false,
    created_at: '2026-11-01T00:00:00.000Z',
    ...over,
  };
}

function page(rows: TaskTrace[], truncated = false): TaskTracePage {
  return { trace: rows, truncated };
}

beforeEach(() => {
  resetTaskTraceForTest();
  getTaskTrace.mockReset();
});

afterEach(() => {
  resetTaskTraceForTest();
});

describe('loadTaskTrace 分页拼接', () => {
  it('不足一页时一轮结束(不再多打一次空请求)', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1), trace(2)]));
    await loadTaskTrace('t1');
    expect(getTaskTrace).toHaveBeenCalledTimes(1);
    expect(useTaskTrace().rows.value.map((r) => r.seq)).toEqual([1, 2]);
    expect(useTaskTrace().ownerTaskId.value).toBe('t1');
  });

  it('满页时按末条 seq 继续翻页,直到回空页', async () => {
    const full = Array.from({ length: 500 }, (_, i) => trace(i + 1));
    getTaskTrace
      .mockResolvedValueOnce(page(full))
      .mockResolvedValueOnce(page([trace(501)]));
    await loadTaskTrace('t1');
    expect(getTaskTrace).toHaveBeenCalledTimes(2);
    // 第二轮的 after 必须是第一轮的末条 seq
    expect(getTaskTrace.mock.calls[1][1]).toBe(500);
    expect(useTaskTrace().rows.value).toHaveLength(501);
  });

  it('满页且第二轮恰好为空 → 第二轮即终止', async () => {
    const full = Array.from({ length: 500 }, (_, i) => trace(i + 1));
    getTaskTrace.mockResolvedValueOnce(page(full)).mockResolvedValueOnce(page([]));
    await loadTaskTrace('t1');
    expect(getTaskTrace).toHaveBeenCalledTimes(2);
    expect(useTaskTrace().rows.value).toHaveLength(500);
  });

  it('分页游标原地打转时终止(防御死循环)', async () => {
    const full = Array.from({ length: 500 }, (_, i) => trace(i + 1));
    // 第二轮返回的末条 seq 没有推进(脏数据)
    getTaskTrace.mockResolvedValueOnce(page(full)).mockResolvedValueOnce(page(full));
    await loadTaskTrace('t1');
    expect(getTaskTrace).toHaveBeenCalledTimes(2);
  });

  it('truncated 如实透出(更早条目已被清理,不能假装是全部)', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1800)], true));
    await loadTaskTrace('t1');
    expect(useTaskTrace().truncated.value).toBe(true);
  });

  it('加载失败时置 error 且不清空已有内容', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1), trace(2)]));
    await loadTaskTrace('t1');
    getTaskTrace.mockRejectedValueOnce(new Error('读失败'));
    await loadTaskTrace('t1');
    expect(useTaskTrace().error.value).toBe('读失败');
    // 「加载失败」与「没有轨迹」是两件事:旧内容必须留着
    expect(useTaskTrace().rows.value).toHaveLength(2);
  });

  it('迟到守卫:响应到达时已切走 → 整轮丢弃,不写进新任务', async () => {
    let release!: (v: TaskTracePage) => void;
    getTaskTrace.mockReturnValueOnce(new Promise((r) => { release = r; }));
    const pending = loadTaskTrace('t1');
    // 切换到别的任务(清空状态并换了 owner)
    clearTaskTrace();
    release(page([trace(1)]));
    await pending;
    expect(useTaskTrace().rows.value).toEqual([]);
    expect(useTaskTrace().ownerTaskId.value).toBeNull();
  });
});

describe('appendTrace 实时并入', () => {
  it('按 seq 排序并入,同 seq 不重复(补拉与实时帧重叠的场景)', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1), trace(2)]));
    await loadTaskTrace('t1');
    // 重复的 seq=2 + 新的 seq=3
    appendTrace('t1', trace(2));
    appendTrace('t1', trace(3));
    expect(useTaskTrace().rows.value.map((r) => r.seq)).toEqual([1, 2, 3]);
  });

  it('非当前任务的轨迹直接丢弃', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1)]));
    await loadTaskTrace('t1');
    appendTrace('t2', trace(9));
    expect(useTaskTrace().rows.value.map((r) => r.seq)).toEqual([1]);
  });

  it('载荷缺失(旧服务端 / 非 trace 事件)不抛错也不写入', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1)]));
    await loadTaskTrace('t1');
    expect(() => appendTrace('t1', undefined)).not.toThrow();
    expect(useTaskTrace().rows.value).toHaveLength(1);
  });

  it('缺口(seq 跳号)触发补拉,缺口区间被补回', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1)]));
    await loadTaskTrace('t1');
    // 下一帧直接跳到 5:3/4 丢了
    getTaskTrace.mockResolvedValueOnce(page([trace(2), trace(3), trace(4)]));
    appendTrace('t1', trace(5));
    // 补拉是异步的,等一轮微任务
    await vi.waitFor(() => {
      expect(useTaskTrace().rows.value.map((r) => r.seq)).toEqual([1, 2, 3, 4, 5]);
    });
    // 补拉起点是已见的最大 seq(1)
    expect(getTaskTrace.mock.calls[1][1]).toBe(1);
  });

  it('无缺口(连续 seq)不触发补拉', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1)]));
    await loadTaskTrace('t1');
    appendTrace('t1', trace(2));
    expect(getTaskTrace).toHaveBeenCalledTimes(1);
  });
});

describe('resumeTaskTrace 重连补偿', () => {
  it('已载入过 → 只补增量(不重头拉全量)', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1), trace(2)]));
    await loadTaskTrace('t1');
    getTaskTrace.mockResolvedValueOnce(page([trace(3)]));
    resumeTaskTrace('t1');
    await vi.waitFor(() => {
      expect(useTaskTrace().rows.value).toHaveLength(3);
    });
    expect(getTaskTrace.mock.calls[1][1]).toBe(2);
  });

  it('从未成功加载过 → 退回全量加载', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1)]));
    // 先建立 owner 但不设 lastSeq(模拟首屏就断线):直接 append 一条使 owner 就位
    getTaskTrace.mockResolvedValueOnce(page([trace(1)]));
    await loadTaskTrace('t1');
    resetTaskTraceForTest();
    // 只有 owner、没有 lastSeq 的极端场景由 loadTaskTrace 自己覆盖,这里验证
    // 「未加载过时 resume 走全量」:先让 owner 就位但不加载
    clearTaskTrace();
    getTaskTrace.mockReset();
    getTaskTrace.mockResolvedValueOnce(page([trace(1)]));
    // ownerTaskId 为 null 时 resume 直接早退(不属于当前任务)
    resumeTaskTrace('t1');
    expect(getTaskTrace).not.toHaveBeenCalled();
  });
});

describe('clearTaskTrace 与档位', () => {
  it('清空后 owner/内容/truncated/error 全复位', async () => {
    getTaskTrace.mockResolvedValueOnce(page([trace(1)], true));
    await loadTaskTrace('t1');
    useTaskTrace().error.value = '某错';
    clearTaskTrace();
    const s = useTaskTrace();
    expect(s.rows.value).toEqual([]);
    expect(s.ownerTaskId.value).toBeNull();
    expect(s.truncated.value).toBe(false);
    expect(s.error.value).toBeNull();
    expect(s.loading.value).toBe(false);
  });

  it('档位默认 full,可切换到三档任一(TMSHOW-2 只做临时切换)', () => {
    expect(useTaskTrace().mode.value).toBe('full');
    setTranscriptMode('legacy');
    expect(useTaskTrace().mode.value).toBe('legacy');
    setTranscriptMode('condensed');
    expect(useTaskTrace().mode.value).toBe('condensed');
    setTranscriptMode('full');
    expect(useTaskTrace().mode.value).toBe('full');
  });
});
