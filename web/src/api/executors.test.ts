import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { deleteExecutor, listExecutors, saveExecutor } from './executors';
import { resetApiTokenForTest } from './client';

// api/executors.ts 封装层测试(2026-09-17 执行者库):REST 路径/方法/请求体契约。
// 范式同 tasks.test.ts / undo.test.ts。

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

const EXEC = {
  id: 'e1',
  name: '审稿员',
  instruction: '逐条指出问题',
  temperature: 0.3,
  created_at: '2026-09-17T00:00:00.000Z',
  updated_at: '2026-09-17T00:00:00.000Z',
};

describe('api/executors REST 封装', () => {
  beforeEach(() => {
    resetApiTokenForTest();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = String(input);
      const method = (init?.method ?? 'GET').toUpperCase();
      if (url.endsWith('/api/bootstrap')) return json({ token: 't' });
      if (url.endsWith('/api/task-executors') && method === 'GET') {
        return json({ ok: true, executors: [EXEC] });
      }
      if (url.endsWith('/api/task-executors') && method === 'POST') {
        return json({ ok: true, saved: EXEC, executors: [EXEC] });
      }
      if (url.endsWith('/api/task-executors/e1') && method === 'DELETE') {
        return json({ ok: true, executors: [] });
      }
      return json({ error: `未 mock 的请求: ${method} ${url}` }, 404);
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('listExecutors:GET /task-executors,返回 executors 数组', async () => {
    const list = await listExecutors();
    expect(list).toHaveLength(1);
    expect(list[0].name).toBe('审稿员');
    expect(list[0].temperature).toBe(0.3);

    const call = vi.mocked(globalThis.fetch).mock.calls.find(([input]) =>
      String(input).endsWith('/api/task-executors'),
    );
    expect(call, '应请求 GET /api/task-executors').toBeDefined();
    expect((call![1]?.method ?? 'GET').toUpperCase()).toBe('GET');
  });

  it('saveExecutor:POST 带 config 包装体,返回 saved 与最新列表', async () => {
    const r = await saveExecutor({ name: '审稿员', instruction: '逐条指出问题', temperature: 0.3 });
    expect(r.saved.id).toBe('e1');
    expect(r.executors).toHaveLength(1);

    const call = vi.mocked(globalThis.fetch).mock.calls.find(
      ([input, init]) =>
        String(input).endsWith('/api/task-executors') &&
        (init?.method ?? 'GET').toUpperCase() === 'POST',
    );
    // 后端请求体形状:{"config": {...}}(与 agent-flows 的 config 包装一致)
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({
      config: { name: '审稿员', instruction: '逐条指出问题', temperature: 0.3 },
    });
  });

  it('deleteExecutor:DELETE 指定 id 路径,返回最新列表', async () => {
    const list = await deleteExecutor('e1');
    expect(list).toEqual([]);

    const call = vi.mocked(globalThis.fetch).mock.calls.find(
      ([input, init]) =>
        String(input).endsWith('/api/task-executors/e1') &&
        (init?.method ?? 'GET').toUpperCase() === 'DELETE',
    );
    expect(call, '应请求 DELETE /api/task-executors/e1').toBeDefined();
  });

  it('响应形状不对时抛错(形状闸门,不透出 undefined 给下拉渲染)', async () => {
    vi.mocked(globalThis.fetch).mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/bootstrap')) return json({ token: 't' });
      return json({ ok: true }); // 缺 executors 字段
    });
    await expect(listExecutors()).rejects.toThrow(/执行者列表/);
  });
});
