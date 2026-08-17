import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useMemoryPanel } from './useMemoryPanel';
import * as api from '../api';
import { resetApiTokenForTest } from '../api/client';

// 记忆库面板交互测试:直调 useMemoryPanel composable(纯 TS,无组件实例依赖),
// 覆盖蒸馏 / 手动添加 / 行内编辑 / 注入开关 / 删除两段式确认的请求与状态反馈。
// 渲染断言在 components/MemoryPanelUi.test.ts(SSR 通道)。

/** 样例条目(与 api/memory.test.ts 同源字段) */
function memEntry(overrides: Partial<api.MemoryEntry> = {}): api.MemoryEntry {
  return {
    id: 7,
    character_id: 'charA',
    source_session_id: 's1',
    kind: 'distilled',
    content: '用户与角色在图书馆初识',
    usage_count: 3,
    last_usage: '2026-08-15T10:30:00Z',
    selected: true,
    created_at: '2026-08-14T08:00:00Z',
    updated_at: '2026-08-15T10:30:00Z',
    ...overrides,
  };
}

/** 三条样例:蒸馏(已选/有使用)、工具(未选/未使用)、手动(已选) */
function sampleList(): api.MemoryEntry[] {
  return [
    memEntry(),
    memEntry({ id: 8, kind: 'tool', content: '工具写入的记忆', selected: false, usage_count: 0, last_usage: null, source_session_id: null }),
    memEntry({ id: 9, kind: 'manual', content: '手动添加的记忆', source_session_id: null }),
  ];
}

/** mock fetch:第一跳 bootstrap,后续按序消费 */
function mockFetch(...responses: Array<{ body: unknown; status?: number }>): ReturnType<typeof vi.spyOn> {
  const spy = vi.spyOn(globalThis, 'fetch');
  spy.mockResolvedValueOnce(new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 }));
  for (const r of responses) {
    spy.mockResolvedValueOnce(new Response(r.body === null ? null : JSON.stringify(r.body), { status: r.status ?? 200 }));
  }
  return spy;
}

function mountPanel(characterId = 'charA', sessionId: string | null = 's1') {
  return useMemoryPanel({
    characterId: () => characterId,
    sessionId: () => sessionId,
  });
}

/** 按调用序号取请求(0 = bootstrap,1 起 = 目标请求) */
function requestOf(calls: Array<[unknown, ...unknown[]]>, index: number): { url: string; init: RequestInit } {
  return { url: String(calls[index][0]), init: (calls[index][1] ?? {}) as RequestInit };
}

beforeEach(() => {
  resetApiTokenForTest();
  vi.restoreAllMocks();
});

describe('useMemoryPanel(加载)', () => {
  it('无角色:load 清空列表且不发列表请求', async () => {
    const spy = mockFetch();
    const p = mountPanel(null, null);
    await p.load();
    expect(spy.mock.calls).toHaveLength(0); // 无目标请求则连 bootstrap 也不发(token 惰性获取)
    expect(p.rows.value).toEqual([]);
  });

  it('加载失败:error 反馈带后端文案', async () => {
    mockFetch({ body: { error: '数据库不可用' }, status: 500 });
    const p = mountPanel();
    await p.load();
    expect(p.error.value).toBe('加载失败:数据库不可用');
  });
});

describe('useMemoryPanel(蒸馏当前会话)', () => {
  it('成功:POST /api/memory/distill 携带 session_id,反馈新增条数并刷新列表', async () => {
    const spy = mockFetch(
      { body: { ok: true, inserted: 3, character_id: 'charA' } },
      { body: { memories: sampleList() } },
    );
    const p = mountPanel();
    await p.distillNow();
    const post = requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 1);
    expect(post.url).toBe('/api/memory/distill');
    expect(post.init.method).toBe('POST');
    expect(JSON.parse(String(post.init.body))).toEqual({ session_id: 's1' });
    expect(p.distillMsg.value).toEqual({ kind: 'ok', text: '蒸馏完成:新增 3 条记忆' });
    // 成功后自动刷新:第三跳再次 GET 列表
    expect(requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 2).url).toBe('/api/memory?character_id=charA');
    expect(p.distilling.value).toBe(false);
  });

  it('未开启(400):错误反馈带后端指引文案与去设置路径', async () => {
    mockFetch({ body: { error: '跨会话记忆蒸馏未开启,请先在设置中打开 memory_distill_enabled' }, status: 400 });
    const p = mountPanel();
    await p.distillNow();
    expect(p.distillMsg.value?.kind).toBe('err');
    expect(p.distillMsg.value?.text).toContain('未开启');
    expect(p.distillMsg.value?.text).toContain('设置');
  });

  it('无当前会话:直接提示,不发请求', async () => {
    const spy = mockFetch();
    const p = mountPanel('charA', null);
    await p.distillNow();
    expect(spy.mock.calls).toHaveLength(0); // 不发任何请求(token 惰性获取)
    expect(p.distillMsg.value?.kind).toBe('err');
    expect(p.distillMsg.value?.text).toContain('当前无会话');
  });
});

describe('useMemoryPanel(手动添加)', () => {
  it('成功:POST /api/memory 携带 character_id 与 trim 后内容,清空输入并刷新', async () => {
    const spy = mockFetch(
      { body: { ok: true, memory: memEntry({ id: 10, kind: 'manual' }) }, status: 201 },
      { body: { memories: sampleList() } },
    );
    const p = mountPanel();
    p.newContent.value = '  用户喜欢薄荷茶  ';
    await p.addNow();
    const post = requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 1);
    expect(post.url).toBe('/api/memory');
    expect(JSON.parse(String(post.init.body))).toEqual({ character_id: 'charA', content: '用户喜欢薄荷茶' });
    expect(p.newContent.value).toBe('');
    expect(requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 2).url).toBe('/api/memory?character_id=charA');
  });

  it('空白内容:提示不发请求', async () => {
    const spy = mockFetch();
    const p = mountPanel();
    p.newContent.value = '   ';
    await p.addNow();
    expect(spy.mock.calls).toHaveLength(0); // 不发任何请求(token 惰性获取)
    expect(p.actionMsg.value?.text).toContain('记忆内容不能为空');
  });

  it('无角色:直接返回不发请求', async () => {
    const spy = mockFetch();
    const p = mountPanel(null, null);
    p.newContent.value = 'x';
    await p.addNow();
    expect(spy.mock.calls).toHaveLength(0);
  });
});

describe('useMemoryPanel(注入开关 selected)', () => {
  it('切到未选:PATCH /api/memory/:id 仅携带 selected,本地行状态同步', async () => {
    const spy = mockFetch(
      { body: { memories: sampleList() } },
      { body: { ok: true, memory: memEntry({ id: 9, selected: false }) } },
    );
    const p = mountPanel();
    await p.load();
    // rows 按 id 降序:rows[0] = id 9(manual)
    await p.toggleSelected(p.rows.value[0], false);
    const patch = requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 2);
    expect(patch.url).toBe('/api/memory/9');
    expect(patch.init.method).toBe('PATCH');
    expect(JSON.parse(String(patch.init.body))).toEqual({ selected: false });
    expect(p.rows.value[0].selected).toBe(false);
  });

  it('失败:反馈错误并重拉列表回滚勾选状态', async () => {
    const spy = mockFetch(
      { body: { memories: sampleList() } },
      { body: { error: '记忆 9 不存在或更新被拒绝' }, status: 404 },
      { body: { memories: sampleList() } },
    );
    const p = mountPanel();
    await p.load();
    await p.toggleSelected(p.rows.value[0], false);
    expect(p.actionMsg.value?.kind).toBe('err');
    expect(p.actionMsg.value?.text).toContain('记忆 9 不存在或更新被拒绝');
    // 回滚:重拉后 id 9 仍为已选
    expect(p.rows.value.find((r) => r.id === 9)?.selected).toBe(true);
    expect(requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 3).url).toBe('/api/memory?character_id=charA');
  });
});

describe('useMemoryPanel(行内编辑 content)', () => {
  it('startEdit 进入编辑态,saveEdit PATCH content 成功后退出并刷新', async () => {
    const spy = mockFetch(
      { body: { memories: sampleList() } },
      { body: { ok: true, memory: memEntry({ id: 9, content: '新内容' }) } },
      { body: { memories: [memEntry({ id: 9, content: '新内容' })] } },
    );
    const p = mountPanel();
    await p.load();
    p.startEdit(p.rows.value[0]);
    expect(p.editingId.value).toBe(9);
    expect(p.editContent.value).toBe('手动添加的记忆');
    p.editContent.value = '新内容';
    await p.saveEdit();
    const patch = requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 2);
    expect(patch.url).toBe('/api/memory/9');
    expect(JSON.parse(String(patch.init.body))).toEqual({ content: '新内容' });
    expect(p.editingId.value).toBeNull();
    expect(p.rows.value[0].content).toBe('新内容');
  });

  it('空白内容:提示不发请求,保持编辑态', async () => {
    const spy = mockFetch({ body: { memories: sampleList() } });
    const p = mountPanel();
    await p.load();
    p.startEdit(p.rows.value[0]);
    p.editContent.value = '   ';
    await p.saveEdit();
    expect(spy.mock.calls).toHaveLength(2); // bootstrap + 初始 load,无 PATCH
    expect(p.editingId.value).toBe(9);
    expect(p.actionMsg.value?.text).toContain('记忆内容不能为空');
  });

  it('cancelEdit 退出编辑态不发文', async () => {
    const spy = mockFetch({ body: { memories: sampleList() } });
    const p = mountPanel();
    await p.load();
    p.startEdit(p.rows.value[0]);
    p.cancelEdit();
    expect(p.editingId.value).toBeNull();
    expect(spy.mock.calls).toHaveLength(2);
  });
});

describe('useMemoryPanel(删除两段式确认)', () => {
  it('requestDelete 进入确认态,cancelDelete 不发请求', async () => {
    const spy = mockFetch({ body: { memories: sampleList() } });
    const p = mountPanel();
    await p.load();
    p.requestDelete(p.rows.value[0]);
    expect(p.pendingDelete.value).toBe(9);
    p.cancelDelete();
    expect(p.pendingDelete.value).toBeNull();
    expect(spy.mock.calls).toHaveLength(2); // 无 DELETE 发出
  });

  it('确认删除:DELETE /api/memory/:id(204)后清除确认态并刷新列表', async () => {
    const spy = mockFetch(
      { body: { memories: sampleList() } },
      { body: null, status: 204 },
      { body: { memories: sampleList().filter((m) => m.id !== 9) } },
    );
    const p = mountPanel();
    await p.load();
    p.requestDelete(p.rows.value[0]); // 进入确认态
    await p.confirmDelete();
    const del = requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 2);
    expect(del.url).toBe('/api/memory/9');
    expect(del.init.method).toBe('DELETE');
    expect(p.pendingDelete.value).toBeNull();
    expect(requestOf(spy.mock.calls as Array<[unknown, ...unknown[]]>, 3).url).toBe('/api/memory?character_id=charA');
    expect(p.rows.value.map((r) => r.id)).toEqual([8, 7]);
  });

  it('删除失败(404):反馈错误文案', async () => {
    mockFetch({ body: { error: '记忆 9 不存在' }, status: 404 });
    const p = mountPanel();
    p.pendingDelete.value = 9;
    await p.confirmDelete();
    expect(p.actionMsg.value?.kind).toBe('err');
    expect(p.actionMsg.value?.text).toContain('记忆 9 不存在');
  });
});
