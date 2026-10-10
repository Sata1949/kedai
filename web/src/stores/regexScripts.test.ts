// 全局正则库 store 回归(IMPHUB ⑥):整体读写 + 幂等加载语义(失败可重试)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

vi.mock('../api', () => ({
  getRegexScripts: vi.fn(),
  saveRegexScripts: vi.fn(),
}));

import * as api from '../api';
import { useRegexScriptsStore } from './regexScripts';

const mocked = api as unknown as Record<string, ReturnType<typeof vi.fn>>;

function script(id: string) {
  return { id, script_name: id, find_regex: id, replace_string: '', markdown_only: false, enabled: true };
}

beforeEach(() => {
  vi.clearAllMocks();
  setActivePinia(createPinia());
});

describe('regexScripts store', () => {
  it('load:整体拉取并置 loaded;save:整体写后回填(含 skipped 回报)', async () => {
    mocked.getRegexScripts.mockResolvedValue([script('g1')]);
    mocked.saveRegexScripts.mockResolvedValue({ count: 2, skipped: 1, scripts: [script('g1'), script('g2')] });
    const store = useRegexScriptsStore();

    await store.loadRegexScripts();
    expect(store.regexScripts.map((s) => s.id)).toEqual(['g1']);
    expect(store.regexScriptsLoaded).toBe(true);

    const res = await store.saveRegexScripts([script('g1'), script('g2'), { bad: 1 }]);
    expect(res).toEqual({ count: 2, skipped: 1 });
    expect(store.regexScripts.map((s) => s.id)).toEqual(['g1', 'g2']);
  });

  it('ensure:已加载不重复拉取;并发调用共享同一次在飞请求', async () => {
    mocked.getRegexScripts.mockResolvedValue([script('g1')]);
    const store = useRegexScriptsStore();

    await Promise.all([store.ensureRegexScriptsLoaded(), store.ensureRegexScriptsLoaded()]);
    expect(mocked.getRegexScripts).toHaveBeenCalledTimes(1);
    await store.ensureRegexScriptsLoaded();
    expect(mocked.getRegexScripts).toHaveBeenCalledTimes(1);
  });

  it('ensure:失败不置 loaded(下次重试),且不向调用方抛错(不挡选角/渲染)', async () => {
    mocked.getRegexScripts.mockRejectedValueOnce(new Error('网络失败'));
    const store = useRegexScriptsStore();

    await expect(store.ensureRegexScriptsLoaded()).resolves.toBeUndefined();
    expect(store.regexScriptsLoaded).toBe(false);

    mocked.getRegexScripts.mockResolvedValue([script('g1')]);
    await store.ensureRegexScriptsLoaded();
    expect(store.regexScriptsLoaded).toBe(true);
    expect(store.regexScripts.map((s) => s.id)).toEqual(['g1']);
  });
});
