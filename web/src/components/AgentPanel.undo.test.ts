// @vitest-environment jsdom
// 批 D-1/D-2:「回退到此处」真实组件点击链路——
//   ① 点击必须恢复**该调用绑定的** snapshot_id(即使列表里有更新的同名快照,也不许猜);
//   ② 绑定的快照已失效(不在列表)→ 不调用恢复 API,标记失效并解释;
//   ③ 无绑定标识(旧记录)→ 按钮禁用,点击不发起任何恢复。
// mock 手法同 ChatInput.test.ts(partial mock '../api')。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

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
    getCuStatus: vi.fn().mockResolvedValue({ stopped: false }),
    fetchHistory: vi.fn().mockResolvedValue([]),
    listUndoSnapshots: vi.fn(),
    restoreUndoSnapshot: vi.fn(),
  };
});

import * as api from '../api';
import { useAppStore } from '../store';
import { idleAgent } from '../sseReducer';
import AgentPanel from './AgentPanel.vue';

function snap(id: string, label: string) {
  return {
    id,
    tool_name: 'write',
    label,
    anchor_message_id: 1,
    created_at: '2026-10-16T00:00:00.000Z',
  };
}

function mountPanel(seed: () => void) {
  setActivePinia(createPinia());
  const store = useAppStore();
  store.currentSessionId = 's1';
  store.undoEnabled = true;
  seed();
  const wrapper = mount(AgentPanel);
  return { store, wrapper };
}

function undoButton(wrapper: ReturnType<typeof mount>) {
  return wrapper.find('button.sv-ml-auto');
}

beforeEach(() => {
  memStorage.clear();
  vi.clearAllMocks();
  vi.mocked(api.getCuStatus).mockResolvedValue({ stopped: false });
  vi.mocked(api.fetchHistory).mockResolvedValue([]);
});

describe('AgentPanel「回退到此处」点击链路(批 D-1/D-2)', () => {
  it('点击较早的 write:恢复该卡绑定的 snapshot_id,不选更新的同名快照', async () => {
    const { wrapper } = mountPanel(() => {
      const store = useAppStore();
      store.agent = {
        ...idleAgent(),
        toolCalls: [
          { name: 'write', input: { path: 'a.md' }, status: 'done', callId: 'cA', snapshotId: 'snap-A' },
          { name: 'write', input: { path: 'b.md' }, status: 'done', callId: 'cB', snapshotId: 'snap-B' },
        ],
      };
    });
    // 列表里 snap-B 更新(新→旧),但点击卡绑定的是 snap-A
    vi.mocked(api.listUndoSnapshots).mockResolvedValue([snap('snap-B', '写入文件 b.md'), snap('snap-A', '写入文件 a.md')]);
    vi.mocked(api.restoreUndoSnapshot).mockResolvedValue({ restored: '写入文件 a.md', restored_count: 2 });

    const buttons = wrapper.findAll('button.sv-ml-auto');
    expect(buttons, '两张写工具卡都应有回退按钮').toHaveLength(2);
    await buttons[0].trigger('click');
    await flushPromises();

    expect(vi.mocked(api.restoreUndoSnapshot)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.restoreUndoSnapshot)).toHaveBeenCalledWith('snap-A');
    // 回执含多快照时提示口径正确
    expect(wrapper.find('.sv-feedback').text()).toContain('已回退 2 条快照');
  });

  it('绑定的快照已失效(不在列表)→ 不调用恢复 API,标记「已回退」并解释', async () => {
    const { wrapper } = mountPanel(() => {
      const store = useAppStore();
      store.agent = {
        ...idleAgent(),
        toolCalls: [{ name: 'write', input: {}, status: 'done', callId: 'cA', snapshotId: 'snap-A' }],
      };
    });
    vi.mocked(api.listUndoSnapshots).mockResolvedValue([snap('snap-B', '写入文件 b.md')]);

    await undoButton(wrapper).trigger('click');
    await flushPromises();

    expect(vi.mocked(api.restoreUndoSnapshot), '不得回退到其它快照').not.toHaveBeenCalled();
    expect(wrapper.find('.sv-feedback').text()).toContain('已不存在');
    // 按钮转为「已回退」禁用态
    expect(undoButton(wrapper).text()).toBe('已回退');
    expect(undoButton(wrapper).attributes('disabled')).toBeDefined();
  });

  it('无绑定标识(旧记录)→ 按钮禁用,点击不发起任何恢复', async () => {
    const { wrapper } = mountPanel(() => {
      const store = useAppStore();
      store.agent = {
        ...idleAgent(),
        toolCalls: [{ name: 'write', input: {}, status: 'done', callId: 'cA' }],
      };
    });
    const btn = undoButton(wrapper);
    expect(btn.attributes('disabled'), '缺绑定标识必须禁用').toBeDefined();
    expect(btn.attributes('title')).toContain('没有绑定回退快照标识');

    await btn.trigger('click');
    await flushPromises();
    expect(vi.mocked(api.listUndoSnapshots), '禁用态不应发起列表请求').not.toHaveBeenCalled();
    expect(vi.mocked(api.restoreUndoSnapshot)).not.toHaveBeenCalled();
  });

  it('生成进行中:按钮禁用(后端同款前置拒绝的界面侧)', async () => {
    const { wrapper } = mountPanel(() => {
      const store = useAppStore();
      store.generating = true;
      store.agent = {
        ...idleAgent(),
        toolCalls: [{ name: 'write', input: {}, status: 'done', callId: 'cA', snapshotId: 'snap-A' }],
      };
    });
    const btn = undoButton(wrapper);
    expect(btn.attributes('disabled')).toBeDefined();
    expect(btn.attributes('title')).toContain('生成进行中');
  });
});
