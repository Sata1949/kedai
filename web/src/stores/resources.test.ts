// 资源 store 的世界书加载错误态(WBCONC 提交 2):失败必须留下可见状态,
// 同时保持 loadWorldBooks resolve(调用方 PromptManager 无 try/catch)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return { ...orig, listWorldBooks: vi.fn() };
});

import { listWorldBooks } from '../api';
import { useResourcesStore } from './resources';

describe('resources store 世界书加载错误态', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(listWorldBooks).mockReset();
  });

  it('加载失败:写入 worldBooksError 且不 rethrow', async () => {
    vi.mocked(listWorldBooks).mockRejectedValueOnce(new Error('后端不可用'));
    const store = useResourcesStore();

    await expect(store.loadWorldBooks()).resolves.toBeUndefined();
    expect(store.worldBooksError).toBe('后端不可用');
  });

  it('后续加载成功:清空错误态并落列表', async () => {
    vi.mocked(listWorldBooks).mockRejectedValueOnce(new Error('后端不可用'));
    const store = useResourcesStore();
    await store.loadWorldBooks();
    expect(store.worldBooksError).toBe('后端不可用');

    vi.mocked(listWorldBooks).mockResolvedValueOnce([]);
    await store.loadWorldBooks();
    expect(store.worldBooksError).toBeNull();
    expect(store.worldBooks).toEqual([]);
  });
});
