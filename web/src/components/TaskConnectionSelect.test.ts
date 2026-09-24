// @vitest-environment jsdom
// 任务**逐任务选用连接**选择器的挂载测试(B 批 B1)。
//
// 为什么用 jsdom 挂载而不是 TaskFlowSelect 那套 SSR 冒烟:本组件的关键口径——
// 「拉取到连接列表后把失效引用重置为默认连接并给一行提示」——发生在 onMounted 之后,
// SSR 不跑钩子就断言不到(Sidebar 的 /logo.png 问题在本组件不适用:它不引用静态资源)。
//
// 锁五件事:
//   ① 恒可见(与 task_mode 无关)且「默认连接(跟随设置)」总是第一项,缺省值 = 空;
//   ② 列出设置里的连接(停用的 disabled 并标注);
//   ③ 选中后写回 store 并持久化(刷新后保持);
//   ④ 持久化的 id 不在连接列表里 → **显式**重置为默认连接 + 一行提示(不静默);
//   ⑤ 拉取失败不判定失效(保留原选择),停用的当前选择给即时提示(创建必被后端 400)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import * as api from '../api';
import type { ConnectionProfile, RuntimeSettings } from '../api/types';
import { useTaskStore } from '../stores/task';
import { useAppStore } from '../store';
import TaskConnectionSelect from './TaskConnectionSelect.vue';

// node 环境无 localStorage,而 task store 初始化即访问持久化键,补内存桩
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
    enabled: true,
    api_key_masked: '****zzzz',
    has_api_key: true,
    ...overrides,
  };
}

function makeSettings(connections: ConnectionProfile[]): RuntimeSettings {
  return { connections, active_connection_id: connections[0]?.id ?? null } as RuntimeSettings;
}

/** 挂载组件并等待 onMounted 的异步拉取落地 */
async function mountSelect() {
  const wrapper = mount(TaskConnectionSelect);
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  memStorage.clear();
  setActivePinia(createPinia());
  vi.clearAllMocks();
  getSettingsMock.mockResolvedValue(makeSettings([connection()]));
});

describe('TaskConnectionSelect:任务级连接选择器(B 批 B1)', () => {
  it('恒显示「默认连接（跟随设置）」且缺省值为空(不下发 connection_id)', async () => {
    const store = useTaskStore();
    const wrapper = await mountSelect();
    const options = wrapper.findAll('option').map((o) => o.text());
    expect(options[0]).toBe('连接:默认连接（跟随设置）');
    expect((wrapper.find('select').element as HTMLSelectElement).value).toBe('');
    // 缺省 = 跟随设置的默认连接:store 里必须是空串,createTask 才不会下发该键
    expect(store.taskConnectionId).toBe('');
    wrapper.unmount();
  });

  it('列出设置里全部连接;停用项 disabled 并标注「(已停用)」', async () => {
    getSettingsMock.mockResolvedValue(
      makeSettings([connection(), connection({ id: 'c2', name: '备用', enabled: false })]),
    );
    const wrapper = await mountSelect();
    const texts = wrapper.findAll('option').map((o) => o.text());
    expect(texts).toContain('连接:主连接｜gpt-x');
    expect(texts).toContain('连接:备用｜gpt-x(已停用)');
    const disabled = wrapper.findAll('option').filter((o) => o.attributes('disabled') !== undefined);
    expect(disabled.map((o) => o.text())).toEqual(['连接:备用｜gpt-x(已停用)']);
    wrapper.unmount();
  });

  it('选中连接后写回 store 并持久化(刷新后保持)', async () => {
    const store = useTaskStore();
    const wrapper = await mountSelect();
    await wrapper.find('select').setValue('c1');
    await flushPromises();

    expect(store.taskConnectionId).toBe('c1');
    expect(memStorage.get('kedai.taskConnectionId.v1')).toBe('c1');
    // 选回默认连接 = 清掉持久化键(与「未指定」同一种表示)
    await wrapper.find('select').setValue('');
    await flushPromises();
    expect(store.taskConnectionId).toBe('');
    expect(memStorage.has('kedai.taskConnectionId.v1')).toBe(false);
    wrapper.unmount();
  });

  it('持久化的连接已不在列表里 → 重置为默认并显式提示(不静默)', async () => {
    const store = useTaskStore();
    store.taskConnectionId = 'c-gone';
    const wrapper = await mountSelect();

    expect(store.taskConnectionId).toBe('');
    expect((wrapper.find('select').element as HTMLSelectElement).value).toBe('');
    expect(wrapper.text()).toContain('原选择的连接已不存在，已重置为默认连接');
    wrapper.unmount();
  });

  it('拉取失败不判定失效:保留原选择,不当成「已不存在」(避免静默改绑)', async () => {
    const store = useTaskStore();
    store.taskConnectionId = 'c1';
    getSettingsMock.mockRejectedValue(new Error('离线'));
    const wrapper = await mountSelect();

    expect(store.taskConnectionId).toBe('c1');
    expect(wrapper.text()).not.toContain('已重置为默认连接');
    expect(wrapper.text()).toContain('加载连接列表失败');
    wrapper.unmount();
  });

  it('所选连接已停用时给即时提示(创建期后端会 400,不重置——连接还在)', async () => {
    const store = useTaskStore();
    store.taskConnectionId = 'c2';
    getSettingsMock.mockResolvedValue(
      makeSettings([connection(), connection({ id: 'c2', name: '备用', enabled: false })]),
    );
    const wrapper = await mountSelect();

    expect(store.taskConnectionId).toBe('c2');
    expect(wrapper.text()).toContain('已停用，创建任务会被拒绝');
    wrapper.unmount();
  });

  it('手动改选后撤下重置提示(提示描述的是本次校验结果,不长期挂着)', async () => {
    const store = useTaskStore();
    store.taskConnectionId = 'c-gone';
    const wrapper = await mountSelect();
    expect(wrapper.text()).toContain('已重置为默认连接');

    await wrapper.find('select').setValue('c1');
    await flushPromises();
    expect(wrapper.text()).not.toContain('已重置为默认连接');
    wrapper.unmount();
  });

  it('设置面板关闭后重拉连接列表:刚删掉的连接不再列着,已被选中的引用随之重置', async () => {
    const store = useTaskStore();
    const app = useAppStore();
    const wrapper = await mountSelect();
    await wrapper.find('select').setValue('c1');
    expect(store.taskConnectionId).toBe('c1');

    // 用户在设置里删掉了 c1,关闭设置面板 → 重拉 + 失效重置(不静默)。
    // 两次赋值之间必须让 watcher 落地一次:watch(ref) 按「当前值 vs 上次回调时的值」
    // 比较,同一 tick 内 true→false 会被合并成「没变过」而不触发(真实交互天然跨 tick)。
    getSettingsMock.mockResolvedValue(makeSettings([]));
    app.settingsOpen = true;
    await flushPromises();
    app.settingsOpen = false;
    await flushPromises();

    expect(getSettingsMock.mock.calls.length, '关闭设置面板应触发一次重拉').toBeGreaterThan(1);
    expect(store.taskConnectionId).toBe('');
    expect(wrapper.text()).toContain('原选择的连接已不存在，已重置为默认连接');
    wrapper.unmount();
  });
});
