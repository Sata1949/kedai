import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useAppStore } from '../store';
import { useAgentSettings } from './useAgentSettings';

// useAgentSettings 的模式过滤测试(TM-SET-3):任务模式下隐藏的聊天专属项
// (变量组 / 反思提示词)不得随保存写入;角色扮演模式照常全量保存。
// mock 范式同 useDataManager.test.ts(../api 整体 mock + hoisted 句柄)。

// node 环境无 localStorage(子 store 初始化即访问),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

/** mock 控制句柄 */
const h = vi.hoisted(() => ({
  settingsBody: {} as Record<string, unknown>,
  savedPatches: [] as Array<Record<string, unknown>>,
}));

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    getSettings: vi.fn(async () => h.settingsBody),
    saveSettings: vi.fn(async (patch: Record<string, unknown>) => {
      h.savedPatches.push(patch);
      return { settings: h.settingsBody };
    }),
  };
});

beforeEach(() => {
  memStorage.clear();
  setActivePinia(createPinia());
  h.settingsBody = {};
  h.savedPatches = [];
});

/** 取最后一次保存的 patch(不存在即断言失败并回退空对象——避免非空断言,ratchet 只降不升) */
function lastSavedPatch(): Record<string, unknown> {
  const patch = h.savedPatches.at(-1);
  expect(patch).toBeDefined();
  return patch ?? {};
}

describe('Agent 设置保存按模式过滤(TM-SET-3)', () => {
  it('角色扮演(缺省):patch 含变量组与反思提示词', async () => {
    const { saveAgentNow } = useAgentSettings();
    await saveAgentNow();
    const patch = lastSavedPatch();
    expect(patch.mvu_vars_position).toBeDefined();
    expect(patch.mvu_model).toBeDefined();
    expect(patch.mvu_temperature).toBeDefined();
    expect(patch.reflect_prompt).toBeDefined();
    expect(patch.agent_system_prompt).toBeDefined();
  });

  it('任务模式:patch 不含变量组与反思提示词;系统提示词与搜索端点照常', async () => {
    const store = useAppStore();
    store.appMode = 'task';
    const { saveAgentNow } = useAgentSettings();
    await saveAgentNow();
    const patch = lastSavedPatch();
    expect(patch.mvu_vars_position, '任务模式不得写变量注入位置').toBeUndefined();
    expect(patch.mvu_model, '任务模式不得写变量模型').toBeUndefined();
    expect(patch.mvu_temperature, '任务模式不得写变量温度').toBeUndefined();
    expect(patch.reflect_prompt, '任务模式不得写反思提示词').toBeUndefined();
    expect(patch.agent_system_prompt).toBeDefined();
    expect(patch.search_endpoint).toBeDefined();
  });
});
