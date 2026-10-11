// @vitest-environment jsdom
// WorldBooksModal 组件测试(M5 补测):654 行组件,此前无任何测试。
// 覆盖:弹窗装载渲染、空列表时的「暂无独立世界书」空态、加载失败时的错误提示、
// 以及关闭动作置 worldBooksOpen=false(该字段为纯 UI 开关,无配套 action,
// 见 tools/check-arch.mjs 的 UI_FLAG_WHITELIST);条目表单的扫描窗口编辑面(WB-D:
// 插入深度与 scan_depth 分栏、留空不提交该键)。
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
    listWorldBooks: vi.fn().mockResolvedValue([]),
    getWorldBookEntries: vi.fn().mockResolvedValue([]),
    getCharacterWorldEntries: vi.fn().mockResolvedValue([]),
    checkWorldBookAutoAssign: vi.fn().mockResolvedValue({ checks: [] }),
    saveWorldBookEntries: vi.fn().mockImplementation(async (_id: string, entries: unknown[]) => entries),
  };
});

import { getWorldBookEntries, saveWorldBookEntries } from '../api';
import type { WorldBookEntry, WorldBookRecord } from '../api';
import { useAppStore } from '../store';
import WorldBooksModal from './WorldBooksModal.vue';

/** 挂载并等待 onMounted 的两个异步加载落定 */
async function mountModal() {
  setActivePinia(createPinia());
  const store = useAppStore();
  // onMounted → store.loadWorldBooks() 走真实 fetch;替换为 no-op 并直接播种列表
  store.loadWorldBooks = vi.fn().mockResolvedValue(undefined);
  const wrapper = mount(WorldBooksModal);
  await Promise.resolve();
  await Promise.resolve();
  return { store, wrapper };
}

describe('WorldBooksModal 组件(M5 补测)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('挂载后渲染弹窗标题与容器', async () => {
    const { wrapper } = await mountModal();
    expect(wrapper.find('.sv-modal-mask').exists()).toBe(true);
    expect(wrapper.find('.sv-modal-head').text()).toContain('世界书');
  });

  it('列表为空时显示「暂无独立世界书」空态', async () => {
    const { wrapper } = await mountModal();
    expect(wrapper.text()).toContain('暂无独立世界书');
    expect(wrapper.findAll('.sv-wb-row').length).toBe(0);
  });

  it('已加载世界书时逐行渲染,不再显示空态', async () => {
    setActivePinia(createPinia());
    const store = useAppStore();
    store.loadWorldBooks = vi.fn().mockResolvedValue(undefined);
    store.worldBooks = [
      { id: 'wb1', name: '设定集', enabled: true },
      { id: 'wb2', name: '人物志', enabled: false },
    ] as never;
    const wrapper = mount(WorldBooksModal);
    await Promise.resolve();

    expect(wrapper.text()).not.toContain('暂无独立世界书');
    expect(wrapper.findAll('.sv-wb-row').length).toBe(2);
    expect(wrapper.text()).toContain('设定集');
  });

  it('点击关闭置 worldBooksOpen=false', async () => {
    const { store, wrapper } = await mountModal();
    store.worldBooksOpen = true;

    await wrapper.find('.sv-modal-head button').trigger('click');
    expect(store.worldBooksOpen).toBe(false);
  });

  it('条目表单区分「插入深度」与「扫描窗口」,留空保存不提交 scan_depth(WB-D)', async () => {
    setActivePinia(createPinia());
    const store = useAppStore();
    store.loadWorldBooks = vi.fn().mockResolvedValue(undefined);
    const book: WorldBookRecord = {
      id: 'wb1', name: '设定集', entry_count: 1, enabled: true,
      source: 'upload', created_at: '2026-10-11T00:00:00Z',
    };
    store.worldBooks = [book];
    const entry: WorldBookEntry = {
      id: 0, comment: '地点', keys: ['车站'], constant: false, enabled: true,
      content: '车站很繁忙。', position: 2, depth: 1, scan_depth: 2,
    };
    vi.mocked(getWorldBookEntries).mockResolvedValue([{ ...entry }]);
    const wrapper = mount(WorldBooksModal);
    await flushPromises();

    // 取控件并在缺失时抛可读断言(类型护栏 ratchet:不用 x! 逃逸,见 tools/check-type-ratchet.mjs)
    const btn = (txt: string) => {
      const found = wrapper.findAll('button').find((b) => b.text().trim() === txt);
      if (!found) throw new Error(`缺「${txt}」按钮`);
      return found;
    };
    const windowInput = () => {
      const found = wrapper.find('input[title*="scanDepth"]');
      if (!found.exists()) throw new Error('扫描窗口输入项缺失');
      return found;
    };

    await btn('条目').trigger('click');
    await flushPromises();

    const texts = wrapper.text();
    expect(texts).toContain('插入深度');
    expect(texts).not.toContain('扫描深度');
    expect(texts).toContain('扫描窗口');
    expect((windowInput().element as HTMLInputElement).value, 'GET 回显的窗口应出现在输入框').toBe('2');

    // 留空 = 不改动:请求体不得出现 scan_depth 键(写 null/0 会把已声明窗口打回 depth 兜底)
    await windowInput().setValue('');
    await btn('保存条目').trigger('click');
    await flushPromises();
    const firstBody = JSON.stringify(vi.mocked(saveWorldBookEntries).mock.calls[0]?.[1]);
    expect(firstBody).not.toContain('scan_depth');
    expect(firstBody).toContain('"depth":1');

    // 填写数值 = 提交该窗口
    await windowInput().setValue('5');
    await btn('保存条目').trigger('click');
    await flushPromises();
    expect(JSON.stringify(vi.mocked(saveWorldBookEntries).mock.calls[1]?.[1])).toContain('"scan_depth":5');
  });
});
