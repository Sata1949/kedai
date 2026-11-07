// @vitest-environment jsdom
// 任务工具待确认队列(2026-11-06 三档授权)的状态机测试。
//
// api 层整体打桩——本文件只验证队列语义(入队/去重/收束/决议/归属)，不验证 HTTP。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  applyTaskToolAuthRequired,
  applyTaskToolAuthResolved,
  clearPendingTaskToolAuth,
  clearTaskToolAuth,
  decideTaskToolAuth,
  loadTaskToolAuthSnapshot,
  resetTaskToolAuthForTest,
  taskToolAuthKey,
  useTaskToolAuth,
} from './useTaskToolAuth';
import type { TaskEvent, TaskToolAuthorization } from '../api/types';

vi.mock('../api', () => ({
  getTaskToolAuthorizations: vi.fn(),
  resolveToolAuthorization: vi.fn(),
}));

import * as api from '../api';

const getTaskToolAuthorizations = vi.mocked(api.getTaskToolAuthorizations);
const resolveToolAuthorization = vi.mocked(api.resolveToolAuthorization);

function auth(over: Partial<TaskToolAuthorization> = {}): TaskToolAuthorization {
  return {
    name: 'fs_write',
    risk: 'sensitive',
    reason: '逐步授权:该工具会写入或发起执行,需要你确认后才执行',
    run_id: 'run-1',
    call_id: 'call-1',
    session_id: 'task:t1',
    code: 'step_confirm',
    input_preview: '{"path":"a.txt"}',
    ...over,
  };
}

function requiredEvent(taskId: string, item: TaskToolAuthorization): TaskEvent {
  return { type: 'task', task_id: taskId, kind: 'tool_authorization_required', tool_authorization: item };
}

function resolvedEvent(
  taskId: string,
  decision: {
    call_id: string;
    run_id?: string;
    name?: string;
    decision: 'allow' | 'deny' | 'timeout' | 'cancelled';
    detail?: string;
  },
): TaskEvent {
  return {
    type: 'task',
    task_id: taskId,
    kind: 'tool_authorization_resolved',
    tool_authorization_decision: {
      run_id: decision.run_id ?? 'run-1',
      call_id: decision.call_id,
      name: decision.name ?? 'fs_write',
      session_id: `task:${taskId}`,
      decision: decision.decision,
      ...(decision.detail ? { detail: decision.detail } : {}),
    },
  };
}

/** 让队列归属到某任务(正常路径由 store.selectTask 调快照端点) */
async function own(taskId: string): Promise<void> {
  getTaskToolAuthorizations.mockResolvedValueOnce([]);
  await loadTaskToolAuthSnapshot(taskId);
}

beforeEach(() => {
  resetTaskToolAuthForTest();
  getTaskToolAuthorizations.mockReset();
  resolveToolAuthorization.mockReset();
});

describe('入队与去重', () => {
  it('非当前任务的事件直接丢弃（切过去时由快照补回）', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t2', auth()));
    expect(useTaskToolAuth().queue.value).toEqual([]);
  });

  it('同一 call_id 重复到达时用新载荷覆盖，不产生第二张卡', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth()));
    applyTaskToolAuthRequired(requiredEvent('t1', auth({ reason: '原因已更新' })));
    const { queue } = useTaskToolAuth();
    expect(queue.value).toHaveLength(1);
    expect(queue.value[0].reason).toBe('原因已更新');
  });

  it('不同 call_id 的并发调用各自排队', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth({ call_id: 'call-1' })));
    applyTaskToolAuthRequired(requiredEvent('t1', auth({ call_id: 'call-2', name: 'bash' })));
    expect(useTaskToolAuth().queue.value.map((x) => x.call_id)).toEqual(['call-1', 'call-2']);
  });

  it('无载荷事件（旧服务端）不影响队列', async () => {
    await own('t1');
    applyTaskToolAuthRequired({ type: 'task', task_id: 't1', kind: 'tool_authorization_required' });
    expect(useTaskToolAuth().queue.value).toEqual([]);
  });

  it('新一条待确认到达时清掉上一轮回执（重新执行/续跑不显示旧结论）', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth({ call_id: 'call-1' })));
    applyTaskToolAuthResolved(resolvedEvent('t1', { call_id: 'call-1', decision: 'deny' }));
    expect(useTaskToolAuth().feedback.value?.kind).toBe('err');
    applyTaskToolAuthRequired(requiredEvent('t1', auth({ call_id: 'call-2' })));
    expect(useTaskToolAuth().feedback.value).toBeNull();
    expect(useTaskToolAuth().queue.value).toHaveLength(1);
  });
});

describe('收束事件', () => {
  it('按 call_id 出队并给出结果回执（优先用后端 detail）', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth()));
    applyTaskToolAuthResolved(resolvedEvent('t1', { call_id: 'call-1', decision: 'timeout', detail: '等待授权超时' }));
    const { queue, feedback } = useTaskToolAuth();
    expect(queue.value).toEqual([]);
    expect(feedback.value?.kind).toBe('info');
    expect(feedback.value?.text).toContain('等待授权超时');
  });

  it('拒绝事件的回执是 err 档', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth()));
    applyTaskToolAuthResolved(resolvedEvent('t1', { call_id: 'call-1', decision: 'deny' }));
    expect(useTaskToolAuth().feedback.value?.kind).toBe('err');
  });

  it('非当前任务的收束事件不动作', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth()));
    applyTaskToolAuthResolved(resolvedEvent('t2', { call_id: 'call-1', decision: 'allow' }));
    expect(useTaskToolAuth().queue.value).toHaveLength(1);
  });
});

describe('快照恢复', () => {
  it('成功后队列等于服务端顺序，并按 call_id 去重', async () => {
    getTaskToolAuthorizations.mockResolvedValueOnce([
      auth({ call_id: 'call-2' }),
      auth({ call_id: 'call-1' }),
      auth({ call_id: 'call-2', name: 'bash' }),
    ]);
    await loadTaskToolAuthSnapshot('t1');
    const { queue, ownerTaskId } = useTaskToolAuth();
    expect(ownerTaskId.value).toBe('t1');
    expect(queue.value.map((x) => x.call_id)).toEqual(['call-2', 'call-1']);
  });

  it('失败不清空已有队列，只置 error（不得伪装成「没有待确认」）', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth()));
    getTaskToolAuthorizations.mockRejectedValueOnce(new Error('任务不存在'));
    await loadTaskToolAuthSnapshot('t1');
    const { queue, error } = useTaskToolAuth();
    expect(queue.value).toHaveLength(1);
    expect(error.value).toContain('任务不存在');
  });

  it('快照刷新不清掉已有回执（否则终态后看不到「那次调用通过了没有」）', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth()));
    applyTaskToolAuthResolved(resolvedEvent('t1', { call_id: 'call-1', decision: 'deny' }));
    getTaskToolAuthorizations.mockResolvedValueOnce([]);
    await loadTaskToolAuthSnapshot('t1');
    expect(useTaskToolAuth().queue.value).toEqual([]);
    expect(useTaskToolAuth().feedback.value?.kind).toBe('err');
  });

  it('切换任务后旧快照的迟到响应被丢弃', async () => {
    let resolveOld!: (v: TaskToolAuthorization[]) => void;
    getTaskToolAuthorizations.mockReturnValueOnce(
      new Promise<TaskToolAuthorization[]>((resolve) => {
        resolveOld = resolve;
      }),
    );
    const oldLoad = loadTaskToolAuthSnapshot('t1');
    getTaskToolAuthorizations.mockResolvedValueOnce([auth({ call_id: 'new-call', session_id: 'task:t2' })]);
    await loadTaskToolAuthSnapshot('t2');
    resolveOld([auth({ call_id: 'old-call' })]);
    await oldLoad;
    const { queue, ownerTaskId } = useTaskToolAuth();
    expect(ownerTaskId.value).toBe('t2');
    expect(queue.value.map((x) => x.call_id)).toEqual(['new-call']);
  });
});

describe('决议', () => {
  it('once 用事件里的 session_id / run_id / call_id 回传，成功后出队并回执', async () => {
    await own('t1');
    const item = auth();
    applyTaskToolAuthRequired(requiredEvent('t1', item));
    resolveToolAuthorization.mockResolvedValueOnce();
    await decideTaskToolAuth(item, 'once');
    expect(resolveToolAuthorization).toHaveBeenCalledWith(
      'fs_write',
      'once',
      'task:t1',
      'run-1',
      'call-1',
    );
    expect(useTaskToolAuth().queue.value).toEqual([]);
    expect(useTaskToolAuth().feedback.value?.kind).toBe('ok');
  });

  it('deny 同样只下发 once/deny 二选一', async () => {
    await own('t1');
    const item = auth();
    applyTaskToolAuthRequired(requiredEvent('t1', item));
    resolveToolAuthorization.mockResolvedValueOnce();
    await decideTaskToolAuth(item, 'deny');
    expect(resolveToolAuthorization.mock.calls[0][1]).toBe('deny');
    expect(useTaskToolAuth().feedback.value?.text).toContain('拒绝');
  });

  it('失败保留卡片并显示后端原文（如 409「已结束」）', async () => {
    await own('t1');
    const item = auth();
    applyTaskToolAuthRequired(requiredEvent('t1', item));
    resolveToolAuthorization.mockRejectedValueOnce(new Error('待授权调用不存在或已结束'));
    await decideTaskToolAuth(item, 'once');
    const { queue, feedback } = useTaskToolAuth();
    expect(queue.value).toHaveLength(1);
    expect(feedback.value?.kind).toBe('err');
    expect(feedback.value?.text).toContain('待授权调用不存在或已结束');
  });

  it('同一调用提交中时忽略重复点击', async () => {
    await own('t1');
    const item = auth();
    applyTaskToolAuthRequired(requiredEvent('t1', item));
    let finish!: () => void;
    resolveToolAuthorization.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
    );
    const first = decideTaskToolAuth(item, 'once');
    await decideTaskToolAuth(item, 'once');
    expect(resolveToolAuthorization).toHaveBeenCalledTimes(1);
    finish();
    await first;
  });

  it('切任务后决议的迟到响应不写进新任务', async () => {
    await own('t1');
    const item = auth();
    applyTaskToolAuthRequired(requiredEvent('t1', item));
    let finish!: () => void;
    resolveToolAuthorization.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
    );
    const pending = decideTaskToolAuth(item, 'once');
    clearTaskToolAuth();
    finish();
    await pending;
    expect(useTaskToolAuth().feedback.value).toBeNull();
    expect(useTaskToolAuth().queue.value).toEqual([]);
  });
});

describe('清空', () => {
  it('终态只清队列、保留回执与归属（结论不能被吞掉）', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth()));
    applyTaskToolAuthResolved(resolvedEvent('t1', { call_id: 'call-1', decision: 'deny' }));
    clearPendingTaskToolAuth();
    const s = useTaskToolAuth();
    expect(s.queue.value).toEqual([]);
    expect(s.feedback.value?.kind).toBe('err');
    // 归属保留:同一任务被追加指令续跑时,后续待授权事件仍能入队
    expect(s.ownerTaskId.value).toBe('t1');
  });

  it('clear 清队列/归属/busy/回执/错误，并让在飞快照失效', async () => {
    await own('t1');
    applyTaskToolAuthRequired(requiredEvent('t1', auth()));
    clearTaskToolAuth();
    const s = useTaskToolAuth();
    expect(s.queue.value).toEqual([]);
    expect(s.ownerTaskId.value).toBeNull();
    expect(s.busy.value).toEqual({});
    expect(s.feedback.value).toBeNull();
    expect(s.error.value).toBeNull();
    expect(s.loading.value).toBe(false);
  });

  it('key 用 run_id + call_id（与后端 wait 表同口径）', () => {
    expect(taskToolAuthKey({ run_id: 'r', call_id: 'c' })).toBe('r:c');
  });
});
