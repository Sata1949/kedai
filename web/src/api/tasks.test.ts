import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  approveTask,
  bindTask,
  createTask,
  deleteTask,
  followupTask,
  getTask,
  getTaskCalls,
  getTaskUsageTotal,
  listTasks,
  planChatTask,
  runTask,
  stopTask,
  streamTaskEvents,
} from './tasks';
import { resetApiTokenForTest } from './client';
import type { TaskEvent, TaskStep } from './types';

// api/tasks.ts 封装层测试:REST 路径/方法契约 + streamTaskEvents 的 SSE 帧解析与关闭语义。

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

/** 把若干字符串块串成 SSE 响应(模拟分 chunk 到达) */
function sseResponse(chunks: string[], status = 200): Response {
  const encoder = new TextEncoder();
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) controller.enqueue(encoder.encode(c));
      controller.close();
    },
  });
  return new Response(stream, {
    status,
    headers: { 'Content-Type': 'text/event-stream' },
  });
}

describe('api/tasks REST 封装', () => {
  beforeEach(() => {
    resetApiTokenForTest();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = (init?.method ?? 'GET').toUpperCase();
      if (url.endsWith('/api/bootstrap')) return json({ token: 't' });
      if (url.endsWith('/api/tasks/usage-total')) {
        return json({ usage_total: { prompt_tokens: 1, completion_tokens: 2, reasoning_tokens: 3 } });
      }
      if (url.endsWith('/api/tasks') && method === 'GET') return json({ tasks: [] });
      if (url.endsWith('/api/tasks') && method === 'POST') {
        return json({ ok: true, task: { id: 't1', title: 'x', status: 'pending', plan: [], result: '', error: '', created_at: '', updated_at: '' } });
      }
      if (url.endsWith('/api/tasks/t1') && method === 'GET') {
        return json({ task: { id: 't1' }, subtasks: [], usage_total: { prompt_tokens: 0, completion_tokens: 0, reasoning_tokens: 0 } });
      }
      if (url.endsWith('/api/tasks/t1/calls') && method === 'GET') {
        return json({
          calls: [{
            id: 'c1', task_id: 't1', phase: 'step', step_index: 2, model: 'deepseek-chat',
            prompt_summary: 'p', response_summary: 'r',
            prompt_tokens: 10, completion_tokens: 20, reasoning_tokens: 0,
            elapsed_ms: 123, status: 'ok', created_at: '2026-08-28T00:00:00.000Z',
          }],
        });
      }
      if (url.endsWith('/api/tasks/a%2Fb/calls') && method === 'GET') return json({ calls: [] });
      if (url.endsWith('/api/tasks/t1/run') && method === 'POST') return json({ ok: true });
      if (url.endsWith('/api/tasks/t1/stop') && method === 'POST') return json({ ok: true });
      if (url.endsWith('/api/tasks/t1/approve') && method === 'POST') return json({ ok: true });
      if (url.endsWith('/api/tasks/t1/followup') && method === 'POST') return json({ ok: true });
      if (url.endsWith('/api/tasks/t1/bind') && method === 'POST') {
        return json({
          ok: true,
          task: {
            id: 't1', title: 'x', status: 'pending', plan: [], result: '', error: '',
            task_mode: 'custom', created_at: '', updated_at: '',
          },
        });
      }
      if (url.endsWith('/api/tasks/a%2Fb/bind') && method === 'POST') {
        return json({ ok: true, task: { id: 'a/b', title: 'x', status: 'pending', plan: [], result: '', error: '', created_at: '', updated_at: '' } });
      }
      if (url.endsWith('/api/tasks/t1/plan-chat') && method === 'POST') {
        return json({ ok: true, plan: [{ name: '修订步骤甲', goal: '修订目标甲', status: 'pending', result: '' }] });
      }
      if (url.endsWith('/api/tasks/t1') && method === 'DELETE') return new Response(null, { status: 204 });
      return json({ error: `未 mock 的请求: ${method} ${url}` }, 404);
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('list/get/create/run/stop/delete/usage-total 的路径与方法契约', async () => {
    await listTasks();
    await getTask('t1');
    await createTask('目标', 'exec-1');
    await runTask('t1');
    await stopTask('t1');
    await deleteTask('t1');
    const total = await getTaskUsageTotal();
    expect(total).toEqual({ prompt_tokens: 1, completion_tokens: 2, reasoning_tokens: 3 });

    const calls = vi.mocked(fetch).mock.calls.map(([input, init]) => [
      String(input),
      (init?.method ?? 'GET').toUpperCase(),
    ]);
    expect(calls).toContainEqual(['/api/tasks', 'GET']);
    expect(calls).toContainEqual(['/api/tasks/t1', 'GET']);
    expect(calls).toContainEqual(['/api/tasks', 'POST']);
    expect(calls).toContainEqual(['/api/tasks/t1/run', 'POST']);
    expect(calls).toContainEqual(['/api/tasks/t1/stop', 'POST']);
    expect(calls).toContainEqual(['/api/tasks/t1', 'DELETE']);
    expect(calls).toContainEqual(['/api/tasks/usage-total', 'GET']);

    // create 请求体带 executor_id(执行者与角色卡解耦);未指定模式时 task_mode 缺省 legacy(批次 4)
    const createCall = vi.mocked(fetch).mock.calls.find(
      ([input, init]) => String(input) === '/api/tasks' && (init?.method ?? 'GET') === 'POST',
    );
    expect(JSON.parse(String(createCall?.[1]?.body))).toEqual({ title: '目标', executor_id: 'exec-1', task_mode: 'legacy' });
  });

  it('createTask 指定 taskMode 时透传 task_mode(批次 4 六模式)', async () => {
    await createTask('目标', undefined, 'team');
    const createCall = vi.mocked(fetch).mock.calls.find(
      ([input, init]) => String(input) === '/api/tasks' && (init?.method ?? 'GET') === 'POST',
    );
    expect(JSON.parse(String(createCall?.[1]?.body))).toEqual({ title: '目标', executor_id: null, task_mode: 'team' });
  });

  it('createTask 的 characterId 兼容入参仅在显式给出时下发(新代码不应使用)', async () => {
    /** 取最后一次 /api/tasks POST 的请求体(同一用例内两次调用,find 会命中第一次) */
    const lastCreateBody = (): Record<string, unknown> => {
      const posts = vi.mocked(fetch).mock.calls.filter(
        ([input, init]) => String(input) === '/api/tasks' && (init?.method ?? 'GET') === 'POST',
      );
      return JSON.parse(String(posts[posts.length - 1]?.[1]?.body)) as Record<string, unknown>;
    };

    // 不传:请求体不得出现 character_id 键(避免污染新请求)
    await createTask('目标', 'exec-1', 'legacy');
    expect(lastCreateBody()).not.toHaveProperty('character_id');

    // 显式传(旧调用方):按兼容语义下发
    await createTask('目标', undefined, 'legacy', 'char-legacy');
    expect(lastCreateBody()).toMatchObject({
      title: '目标',
      executor_id: null,
      character_id: 'char-legacy',
    });
  });

  it('createTask 的 flowId 仅在显式给出时下发(二维批次 5a 流程绑定)', async () => {
    const lastCreateBody = (): Record<string, unknown> => {
      const posts = vi.mocked(fetch).mock.calls.filter(
        ([input, init]) => String(input) === '/api/tasks' && (init?.method ?? 'GET') === 'POST',
      );
      return JSON.parse(String(posts[posts.length - 1]?.[1]?.body)) as Record<string, unknown>;
    };

    // 不传(= 跟随当前流程):请求体不得出现 flow_id 键,旧客户端请求体逐字节不变
    await createTask('目标', undefined, 'custom');
    expect(lastCreateBody()).not.toHaveProperty('flow_id');
    expect(lastCreateBody()).toEqual({ title: '目标', executor_id: null, task_mode: 'custom' });

    // 显式传:随请求下发(后端据此在创建时冻结快照)
    await createTask('目标', undefined, 'custom', undefined, 'flow-b');
    expect(lastCreateBody()).toEqual({
      title: '目标',
      executor_id: null,
      task_mode: 'custom',
      flow_id: 'flow-b',
    });
  });

  it('createTask 的 flowIds 仅在非空时下发(二维批次 7b 对比模式名单)', async () => {
    const lastCreateBody = (): Record<string, unknown> => {
      const posts = vi.mocked(fetch).mock.calls.filter(
        ([input, init]) => String(input) === '/api/tasks' && (init?.method ?? 'GET') === 'POST',
      );
      return JSON.parse(String(posts[posts.length - 1]?.[1]?.body)) as Record<string, unknown>;
    };

    // 不给(强制模式):请求体不得出现 flow_ids 键,旧客户端请求体逐字节不变
    await createTask('目标', undefined, 'custom', undefined, 'flow-a');
    expect(lastCreateBody()).not.toHaveProperty('flow_ids');
    expect(lastCreateBody()).toEqual({
      title: '目标',
      executor_id: null,
      task_mode: 'custom',
      flow_id: 'flow-a',
    });

    // 空数组同样不下发:后端把「给了空数组」判 400(空名单 = 名存实亡),
    // 这里挡一次比让用户撞一次失败好
    await createTask('目标', undefined, 'custom', undefined, undefined, []);
    expect(lastCreateBody()).not.toHaveProperty('flow_ids');

    // 非空:原样下发,且**顺序保持**(后端工具描述按名单顺序列举)
    await createTask('目标', undefined, 'custom', undefined, undefined, ['f-2', 'f-1']);
    expect(lastCreateBody()).toEqual({
      title: '目标',
      executor_id: null,
      task_mode: 'custom',
      flow_ids: ['f-2', 'f-1'],
    });
  });

  it('createTask 的 connectionId 仅在非空时下发(B 批 B1 逐任务选用连接)', async () => {
    const lastCreateBody = (): Record<string, unknown> => {
      const posts = vi.mocked(fetch).mock.calls.filter(
        ([input, init]) => String(input) === '/api/tasks' && (init?.method ?? 'GET') === 'POST',
      );
      return JSON.parse(String(posts[posts.length - 1]?.[1]?.body)) as Record<string, unknown>;
    };

    // 不给(= 跟随设置的默认连接):请求体不得出现 connection_id 键,旧客户端请求体逐字节不变
    await createTask('目标', undefined, 'solo');
    expect(lastCreateBody()).not.toHaveProperty('connection_id');
    expect(lastCreateBody()).toEqual({ title: '目标', executor_id: null, task_mode: 'solo' });

    // 非空:原样下发,且**与任务模式无关**(solo 也吃:它绑的是 provider,不是编排)
    await createTask('目标', undefined, 'solo', undefined, undefined, undefined, 'conn-b');
    expect(lastCreateBody()).toEqual({
      title: '目标',
      executor_id: null,
      task_mode: 'solo',
      connection_id: 'conn-b',
    });
  });

  it('bindTask:POST /tasks/{id}/bind,flow_id 与 flow_ids 全量显式下发(B 批 B3)', async () => {
    /** 取最后一次 bind 的请求体(同一用例内多次调用,find 只命中第一次) */
    const lastBindBody = (): Record<string, unknown> => {
      const posts = vi.mocked(fetch).mock.calls.filter(
        ([input, init]) => String(input).includes('/bind') && (init?.method ?? 'GET') === 'POST',
      );
      return JSON.parse(String(posts[posts.length - 1]?.[1]?.body)) as Record<string, unknown>;
    };

    // 解绑 + 清空名单:两个键都必须**出现**——与 createTask 的「仅非空下发」口径相反,
    // 省掉它们后端无法区分「不改」与「清空」,「跟随当前流程 / 强制模式」就表达不出来
    await bindTask('t1', null, []);
    expect(lastBindBody()).toEqual({ flow_id: null, flow_ids: [] });

    // 绑定 + 保名单:原样下发,顺序保持(后端工具描述按名单顺序列举)
    await bindTask('t1', 'flow-b', ['f-2', 'f-1']);
    expect(lastBindBody()).toEqual({ flow_id: 'flow-b', flow_ids: ['f-2', 'f-1'] });

    // 保留绑定、只清名单(另一侧的组合:根流程在,强制模式)
    await bindTask('t1', 'flow-b', []);
    expect(lastBindBody()).toEqual({ flow_id: 'flow-b', flow_ids: [] });

    // 返回体取 task 字段(调用方据此就地更新列表与当前任务详情,不另发一次 GET)
    const task = await bindTask('t1', 'flow-b', []);
    expect(task.id).toBe('t1');

    // id 含特殊字符时路径必须转义
    await bindTask('a/b', null, []);
    const urls = vi.mocked(fetch).mock.calls.map(([input]) => String(input));
    expect(urls).toContain('/api/tasks/a%2Fb/bind');
  });

  it('approveTask:POST /tasks/{id}/approve;不给 plan 时空体,给了 plan 则带 plan 字段', async () => {
    const plan: TaskStep[] = [{ name: '步骤一', goal: '目标一', status: 'pending', result: '' }];
    const r1 = await approveTask('t1');
    expect(r1).toEqual({ ok: true });
    await approveTask('t1', plan);

    const approveCalls = vi.mocked(fetch).mock.calls.filter(
      ([input, init]) => String(input) === '/api/tasks/t1/approve' && (init?.method ?? 'GET') === 'POST',
    );
    expect(approveCalls).toHaveLength(2);
    expect(JSON.parse(String(approveCalls[0]?.[1]?.body))).toEqual({});
    expect(JSON.parse(String(approveCalls[1]?.[1]?.body))).toEqual({ plan });
  });

  it('approveTask 的执行方式:不传时不下发 exec_mode(后端按默认逐步执行)', async () => {
    await approveTask('t1', undefined, undefined);
    const call = vi.mocked(fetch).mock.calls.find(
      ([input, init]) => String(input) === '/api/tasks/t1/approve' && (init?.method ?? 'GET') === 'POST',
    );
    expect(JSON.parse(String(call?.[1]?.body))).not.toHaveProperty('exec_mode');
  });

  it('approveTask 的执行方式:传入时与 plan 一并下发(2026-09-17 批准界面可选模式)', async () => {
    const plan: TaskStep[] = [{ name: '步骤一', goal: '目标一', status: 'pending', result: '' }];
    await approveTask('t1', plan, 'team');
    const call = vi.mocked(fetch).mock.calls.find(
      ([input, init]) => String(input) === '/api/tasks/t1/approve' && (init?.method ?? 'GET') === 'POST',
    );
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({ plan, exec_mode: 'team' });

    // 不给 plan 只给执行方式(直接批准 + 换模式)
    await approveTask('t1', undefined, 'solo');
    const calls = vi.mocked(fetch).mock.calls.filter(
      ([input, init]) => String(input) === '/api/tasks/t1/approve' && (init?.method ?? 'GET') === 'POST',
    );
    expect(JSON.parse(String(calls[calls.length - 1]?.[1]?.body))).toEqual({ exec_mode: 'solo' });
  });

  it('followupTask:POST /tasks/{id}/followup,body 带 content 原文(批次 R2a)', async () => {
    const r = await followupTask('t1', '再补充一点秋色');
    expect(r).toEqual({ ok: true });

    const call = vi.mocked(fetch).mock.calls.find(
      ([input, init]) => String(input) === '/api/tasks/t1/followup' && (init?.method ?? 'GET') === 'POST',
    );
    expect(call).toBeTruthy();
    // 缺省 mode=append(F5:显式携带 mode,兼容后端严格解析)
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({
      content: '再补充一点秋色',
      mode: 'append',
    });

    // replace 模式(2026-09-10 F5):body.mode 传 replace
    await followupTask('t1', '把全文压缩到 200 字', 'replace');
    const replaceCall = vi
      .mocked(fetch)
      .mock.calls.filter(
        ([input, init]) => String(input) === '/api/tasks/t1/followup' && (init?.method ?? 'GET') === 'POST',
      )
      .at(-1);
    expect(JSON.parse(String(replaceCall?.[1]?.body))).toEqual({
      content: '把全文压缩到 200 字',
      mode: 'replace',
    });
  });

  it('planChatTask:POST /tasks/{id}/plan-chat,body 带 message 原文(批次 R2b),取响应 plan', async () => {
    const r = await planChatTask('t1', '把第二步换成先做竞品调研');
    expect(r.ok).toBe(true);
    expect(r.plan[0]?.name).toBe('修订步骤甲');

    const call = vi.mocked(fetch).mock.calls.find(
      ([input, init]) => String(input) === '/api/tasks/t1/plan-chat' && (init?.method ?? 'GET') === 'POST',
    );
    expect(call).toBeTruthy();
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({ message: '把第二步换成先做竞品调研' });
  });

  it('getTaskCalls:路径契约(encodeURIComponent 转义 id)并取响应 calls 字段', async () => {
    const calls = await getTaskCalls('t1');
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ id: 'c1', task_id: 't1', phase: 'step', step_index: 2, status: 'ok' });

    // id 含特殊字符时路径必须转义
    await getTaskCalls('a/b');
    const urls = vi.mocked(fetch).mock.calls.map(([input]) => String(input));
    expect(urls).toContain('/api/tasks/t1/calls');
    expect(urls).toContain('/api/tasks/a%2Fb/calls');
  });

  it('REST 请求带鉴权头(Authorization + X-Kedai-Client)', async () => {
    await listTasks();
    const listCall = vi.mocked(fetch).mock.calls.find(
      ([input]) => String(input) === '/api/tasks',
    );
    const headers = new Headers(listCall?.[1]?.headers);
    expect(headers.get('Authorization')).toBe('Bearer t');
    expect(headers.get('X-Kedai-Client')).toBe('kedai-web');
  });
});

describe('api/tasks streamTaskEvents(SSE 订阅)', () => {
  beforeEach(() => {
    resetApiTokenForTest();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('逐帧解析 data 行并回调 onEvent;坏帧与非 task 帧跳过不阻断', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/bootstrap')) return json({ token: 't' });
      if (url.endsWith('/api/tasks/events')) {
        return sseResponse([
          'data: {"type":"task","task_id":"t1","kind":"status","status":"running"}\n\n',
          'data: {坏帧\n\ndata: {"type":"token","text":"非task"}\n\n',
          'data: {"type":"task","task_id":"t1","kind":"plan"}\n\n',
        ]);
      }
      return json({ error: '未 mock' }, 404);
    });

    const events: TaskEvent[] = [];
    const closes: (Error | undefined)[] = [];
    streamTaskEvents(
      (ev) => events.push(ev),
      (err) => closes.push(err),
    );
    // 等待流消费完(流自然结束 → onClose 无 err)
    await vi.waitFor(() => expect(closes.length).toBe(1));
    expect(events.map((e) => e.kind)).toEqual(['status', 'plan']);
    expect(closes[0]).toBeUndefined();
  });

  it('兼容跨 chunk 分隔与 CRLF;无 data 的 KeepAlive 帧忽略', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/bootstrap')) return json({ token: 't' });
      if (url.endsWith('/api/tasks/events')) {
        return sseResponse([
          ': keep-alive\r\n\r\ndata: {"type":"task",',
          '"task_id":"t1","kind":"created"}\r\n\r\n',
        ]);
      }
      return json({ error: '未 mock' }, 404);
    });

    const events: TaskEvent[] = [];
    streamTaskEvents((ev) => events.push(ev), () => {});
    await vi.waitFor(() => expect(events.length).toBe(1));
    expect(events[0].kind).toBe('created');
  });

  it('非 2xx 响应走 onClose(ApiError);主动 close 不触发 onClose', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/bootstrap')) return json({ token: 't' });
      if (url.endsWith('/api/tasks/events')) return json({ error: '鉴权失败', code: 'UNAUTHORIZED' }, 401);
      return json({ error: '未 mock' }, 404);
    });

    const closes: (Error | undefined)[] = [];
    streamTaskEvents(() => {}, (err) => closes.push(err));
    await vi.waitFor(() => expect(closes.length).toBe(1));
    expect(closes[0]).toBeInstanceOf(Error);
    expect((closes[0] as Error & { status?: number }).status).toBe(401);

    // 主动关闭:流挂着(不 enqueue 不 close),close() 后不得回调 onClose
    vi.mocked(fetch).mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/bootstrap')) return json({ token: 't' });
      if (url.endsWith('/api/tasks/events')) {
        return new Response(new ReadableStream<Uint8Array>({ start: () => {} }), {
          status: 200,
          headers: { 'Content-Type': 'text/event-stream' },
        });
      }
      return json({ error: '未 mock' }, 404);
    });
    const closes2: (Error | undefined)[] = [];
    const close = streamTaskEvents(() => {}, (err) => closes2.push(err));
    await new Promise((r) => setTimeout(r, 20));
    close();
    await new Promise((r) => setTimeout(r, 50));
    expect(closes2.length).toBe(0);
  });
});

// 形状闸门(批次 1):200 + 形状不对的载荷必须变成可捕获的 Error,而不是
// 让 undefined 流进 store(报错点远离真正原因)。
describe('api/tasks 形状闸门', () => {
  beforeEach(() => {
    resetApiTokenForTest();
    vi.restoreAllMocks();
  });

  it('GET /tasks 返回 200 但缺 tasks 数组时透出服务端 error 原文', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(json({ token: 't' }))
      .mockResolvedValueOnce(json({ error: '任务服务异常' }));
    await expect(listTasks()).rejects.toThrow('任务服务异常');
  });

  it('GET /tasks 缺字段且无 error 原文时抛「任务列表响应格式异常」', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(json({ token: 't' }))
      .mockResolvedValueOnce(json({ oops: true }));
    await expect(listTasks()).rejects.toThrow('任务列表响应格式异常');
  });

  it('GET /tasks/:id 缺 subtasks 数组时抛错(不把形状错误带进 store)', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(json({ token: 't' }))
      .mockResolvedValueOnce(json({ task: { id: 't1' } }));
    await expect(getTask('t1')).rejects.toThrow('任务详情响应格式异常');
  });

  it('GET /tasks/usage-total 缺 usage_total 对象时抛错', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(json({ token: 't' }))
      .mockResolvedValueOnce(json({ ok: true }));
    await expect(getTaskUsageTotal()).rejects.toThrow('任务用量累计响应格式异常');
  });

  it('POST /tasks 缺 task 对象时抛错', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(json({ token: 't' }))
      .mockResolvedValueOnce(json({ ok: true }));
    await expect(createTask('目标')).rejects.toThrow('任务响应格式异常');
  });

  it('GET /tasks/:id/calls 缺 calls 数组时抛错', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(json({ token: 't' }))
      .mockResolvedValueOnce(json({ ok: true }));
    await expect(getTaskCalls('t1')).rejects.toThrow('任务调用记录响应格式异常');
  });

  it('POST /tasks/:id/bind 缺 task 对象时抛错(不把 undefined 带进 store)', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(json({ token: 't' }))
      .mockResolvedValueOnce(json({ ok: true }));
    await expect(bindTask('t1', null, [])).rejects.toThrow('任务响应格式异常');
  });
});
