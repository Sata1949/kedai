// useFlowConnections 测试(二维批次 5b)。
//
// 锁三件事:
//   ① 只取**已落库**的连接(经 api.getSettings),不是连接配置区的编辑态草稿;
//   ② 分区可见时按需加载,`show` 由 false 变 true 时重拉(「先改连接再切回流程」不吃陈旧值);
//   ③ 拉取失败不炸:退化成空候选(节点上的引用会显示成「失效」而不是静默消失)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick, ref } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import * as api from '../api';
import type { ConnectionProfile, RuntimeSettings } from '../api/types';
import { useFlowConnections } from './useFlowConnections';

// node 环境无 localStorage,而 store 初始化即访问(appMode),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() {
    return memStorage.size;
  },
});

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return { ...orig, getSettings: vi.fn() };
});

const getSettingsMock = vi.mocked(api.getSettings);

function connection(overrides: Partial<ConnectionProfile> = {}): ConnectionProfile {
  return {
    id: 'c1',
    name: '主连接',
    connector_type: 'openai-compatible',
    base_url: 'https://api.example/v1',
    model: 'gpt-x',
    api_style: 'chat-completions',
    enabled: true,
    supports_vision: false,
    supports_structured_output: false,
    supports_prefix_completion: false,
    supports_mid_conversation_system: false,
    image_auto_split: false,
    api_key_masked: '****zzzz',
    has_api_key: true,
    ...overrides,
  };
}

function makeSettings(connections: ConnectionProfile[]): RuntimeSettings {
  return { connections, active_connection_id: connections[0]?.id ?? null } as RuntimeSettings;
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.clearAllMocks();
});

describe('useFlowConnections', () => {
  it('加载后把连接转成选项(启用与停用都带上,由组件决定可否选)', async () => {
    getSettingsMock.mockResolvedValue(
      makeSettings([connection(), connection({ id: 'c2', name: '备用', enabled: false })]),
    );
    const show = ref(true);
    const conn = useFlowConnections(show);
    await conn.load();
    expect(conn.options.value.map((o) => [o.id, o.enabled])).toEqual([
      ['c1', true],
      ['c2', false],
    ]);
    expect(conn.error.value).toBe('');
  });

  it('构造时不发请求(SSR 与首屏纪律:由组件 onMounted 触发首次加载)', async () => {
    const show = ref(true);
    useFlowConnections(show);
    expect(getSettingsMock).not.toHaveBeenCalled();
  });

  it('show 由不可见变可见 → 重拉一次(连完连接再切回流程能吃上新列表)', async () => {
    getSettingsMock.mockResolvedValue(makeSettings([connection()]));
    const show = ref(false);
    const conn = useFlowConnections(show);
    expect(getSettingsMock).not.toHaveBeenCalled();

    show.value = true;
    await nextTick();
    expect(getSettingsMock).toHaveBeenCalledTimes(1);

    // 切走再切回:再拉一次,即便上次已加载(用户可能刚在连接配置区改了东西)
    show.value = false;
    await nextTick();
    show.value = true;
    await nextTick();
    expect(getSettingsMock).toHaveBeenCalledTimes(2);
  });

  it('拉取失败:退化为空候选并记录提示(不抛、不静默当成「没有引用」)', async () => {
    getSettingsMock.mockRejectedValue(new Error('boom'));
    const show = ref(true);
    const conn = useFlowConnections(show);
    await conn.load();
    expect(conn.options.value).toEqual([]);
    expect(conn.error.value).toContain('加载连接列表失败');
  });
});
