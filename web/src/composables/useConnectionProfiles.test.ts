import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import * as api from '../api';
import type { ConnectionProfile, RuntimeSettings, RuntimeSettingsPatch } from '../api/types';
import { useConnectionProfiles } from './useConnectionProfiles';

// 多套连接配置 composable 测试(批次 4)。
// 这里直接调用 composable(不经 SSR):保存/删除等交互在 SSR 下点不到,而契约的核心
// ——「patch 组装规则」与「删除二次确认」—— 正是必须锁住的部分。

// node 环境无 localStorage,而 store 初始化即访问(appMode / 渲染记忆),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    getSettings: vi.fn(),
    saveSettings: vi.fn(),
  };
});

const getSettingsMock = vi.mocked(api.getSettings);
const saveSettingsMock = vi.mocked(api.saveSettings);

function connection(overrides: Partial<ConnectionProfile> = {}): ConnectionProfile {
  return {
    id: 'c1',
    name: '主连接',
    connector_type: 'openai-compatible',
    base_url: 'https://api.example/v1',
    model: 'gpt-x',
    api_style: 'chat-completions',
    enabled: true,
    api_key_masked: '****zzzz',
    has_api_key: true,
    ...overrides,
  };
}

/** 最小可用的设置体(只列本测试用到的字段,其余按空值) */
function makeSettings(connections: ConnectionProfile[], active: string | null): RuntimeSettings {
  return {
    openai_base_url: connections[0]?.base_url ?? '',
    api_key_masked: '****zzzz',
    has_api_key: true,
    model: 'gpt-x',
    connections,
    active_connection_id: active,
  } as RuntimeSettings;
}

/** 「服务端」状态:保存即回显,并按后端语义给新条目补 id、回退失效的默认连接 */
function makeServer(initial: RuntimeSettings): RuntimeSettings {
  return initial;
}

function installServer(initial: RuntimeSettings): { current: () => RuntimeSettings } {
  let server = makeServer(initial);
  getSettingsMock.mockImplementation(() => Promise.resolve(server));
  saveSettingsMock.mockImplementation((patch: RuntimeSettingsPatch) => {
    // patch 不带 connections = 不动连接数组(与后端 `if let Some(list)` 同语义)
    const conns: ConnectionProfile[] = patch.connections
      ? patch.connections.map((row, i) =>
          connection({
            id: row.id || `new-${i}`,
            name: row.name ?? '',
            base_url: row.base_url ?? '',
            model: row.model ?? '',
            enabled: row.enabled ?? true,
            has_api_key: Boolean(row.api_key),
            api_key_masked: row.api_key ? `****${row.api_key.slice(-4)}` : '',
          }),
        )
      : server.connections;
    const active = patch.active_connection_id ?? server.active_connection_id;
    const activeOk = conns.some((c) => c.id === active && c.enabled);
    server = makeSettings(conns, activeOk ? active : (conns.find((c) => c.enabled)?.id ?? null));
    return Promise.resolve({ ok: true, settings: server });
  });
  return { current: () => server };
}

beforeEach(() => {
  memStorage.clear();
  setActivePinia(createPinia());
  getSettingsMock.mockReset();
  saveSettingsMock.mockReset();
});

describe('useConnectionProfiles 载入与选择', () => {
  it('载入后回填草稿并把默认连接映射为行下标;密钥输入框必须为空', async () => {
    installServer(makeSettings([connection(), connection({ id: 'c2', name: '备用' })], 'c2'));
    const state = useConnectionProfiles();
    await state.load();
    expect(state.drafts.value.map((d) => d.name)).toEqual(['主连接', '备用']);
    expect(state.activeIndex.value).toBe(1);
    // 密钥是只写字段:载入后输入框为空,否则会把掩码当明文发回后端
    expect(state.drafts.value[0].api_key).toBe('');
    expect(state.drafts.value[0].api_key_masked).toBe('****zzzz');
  });

  it('设为默认 → patch 带 active_connection_id;未输入密钥的连接不下发 api_key', async () => {
    installServer(makeSettings([connection(), connection({ id: 'c2', name: '备用' })], 'c1'));
    const state = useConnectionProfiles();
    await state.load();
    state.setActive(1);
    const patch = state.buildPatch();
    expect(patch.active_connection_id).toBe('c2');
    expect(patch.connections?.map((c) => c.id)).toEqual(['c1', 'c2']);
    expect(patch.connections?.[0].api_key).toBeUndefined();
  });

  it('输入了密钥的连接才下发 api_key(去除首尾空白)', async () => {
    installServer(makeSettings([connection(), connection({ id: 'c2', name: '备用' })], 'c1'));
    const state = useConnectionProfiles();
    await state.load();
    state.drafts.value[1].api_key = ' sk-new-9999 ';
    const patch = state.buildPatch();
    expect(patch.connections?.[0].api_key).toBeUndefined();
    expect(patch.connections?.[1].api_key).toBe('sk-new-9999');
  });
});

describe('useConnectionProfiles 删除二次确认', () => {
  it('确认框被拒绝时连接原样保留', async () => {
    installServer(makeSettings([connection(), connection({ id: 'c2', name: '备用' })], 'c1'));
    const state = useConnectionProfiles();
    await state.load();
    const confirmMock = vi.fn(() => false);
    vi.stubGlobal('confirm', confirmMock);
    state.requestRemove(0);
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(state.drafts.value.map((d) => d.name)).toEqual(['主连接', '备用']);
  });

  it('确认后删除该行,且默认连接的行下标重新对齐', async () => {
    installServer(makeSettings([connection(), connection({ id: 'c2', name: '备用' })], 'c2'));
    const state = useConnectionProfiles();
    await state.load();
    vi.stubGlobal('confirm', vi.fn(() => true));
    expect(state.activeIndex.value).toBe(1);
    state.requestRemove(0);
    expect(state.drafts.value.map((d) => d.name)).toEqual(['备用']);
    expect(state.activeIndex.value).toBe(0);
  });
});

describe('useConnectionProfiles 保存', () => {
  it('新增行被设为默认:保存拿到 id 后补一次「设为默认」,并回填服务端结果', async () => {
    installServer(makeSettings([connection()], 'c1'));
    const state = useConnectionProfiles();
    await state.load();
    state.addDraft();
    state.drafts.value[1].name = '新连接';
    state.drafts.value[1].api_key = 'sk-new-0001';
    state.setActive(1);
    await state.save();
    // 后端不接收客户端自带的 id(未命中一律视作新建),故必须补第二次 PUT
    expect(saveSettingsMock).toHaveBeenCalledTimes(2);
    expect(saveSettingsMock.mock.calls[1][0]).toEqual({ active_connection_id: 'new-1' });
    expect(state.drafts.value).toHaveLength(2);
    expect(state.drafts.value[1].id).toBe('new-1');
    expect(state.activeIndex.value).toBe(1);
    expect(state.feedback.value?.kind).toBe('ok');
  });

  it('保存失败时给出中文错误提示,不误报成功', async () => {
    installServer(makeSettings([connection()], 'c1'));
    saveSettingsMock.mockRejectedValue(new Error('服务端拒绝'));
    const state = useConnectionProfiles();
    await state.load();
    await state.save();
    expect(state.feedback.value?.kind).toBe('err');
    expect(state.feedback.value?.text).toContain('服务端拒绝');
  });
});
