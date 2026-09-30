// @vitest-environment jsdom
// Sidebar 组件测试(阶段 A 补测):585 行,此前无测试。侧栏承载角色列表 / 任务列表 /
// 会话入口三类主路径,验证空态、列表渲染与「收起」动作不回退为直改 state。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import type { TaskRecord } from '../api';

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
    listCharacters: vi.fn().mockResolvedValue([]),
    listTasks: vi.fn().mockResolvedValue([]),
    listSessions: vi.fn().mockResolvedValue([]),
  };
});

// CODE-4:画像探测走独立端点模块;此处统一 mock,避免防抖定时器晚发时打到真实网络
const { probeWorkspaceMock } = vi.hoisted(() => ({ probeWorkspaceMock: vi.fn() }));
vi.mock('../api/workspace', () => ({ probeWorkspace: probeWorkspaceMock }));

import { useAppStore } from '../store';
import Sidebar from './Sidebar.vue';

function mountSidebar() {
  setActivePinia(createPinia());
  return { store: useAppStore(), wrapper: mount(Sidebar) };
}

/** CODE-1 用:store.createTask 打桩的最小任务记录(仅本文件断言所需字段) */
function mkTaskRecord(): TaskRecord {
  return {
    id: 't1',
    title: '写个周报',
    status: 'pending',
    plan: [],
    result: '',
    error: '',
    character_id: null,
    created_at: '2026-09-30T00:00:00Z',
    updated_at: '2026-09-30T00:00:00Z',
    task_mode: 'legacy',
  };
}

/** 切到任务模式并等待重渲染(task 表单与历史列表在 v-else 分支) */
async function mountTaskSidebar() {
  const m = mountSidebar();
  m.store.appMode = 'task';
  await m.wrapper.vm.$nextTick();
  return m;
}

describe('Sidebar 组件(阶段 A 补测)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('挂载渲染侧栏容器与品牌区', () => {
    const { wrapper } = mountSidebar();
    expect(wrapper.find('.sv-sidebar').exists()).toBe(true);
    expect(wrapper.find('.sv-side-head').exists()).toBe(true);
  });

  it('角色模式且无角色时显示「暂无角色」空态', () => {
    const { wrapper } = mountSidebar();
    expect(wrapper.text()).toContain('暂无角色');
  });

  it('任务模式且无任务时显示任务空态文案', () => {
    const { store, wrapper } = mountSidebar();
    store.appMode = 'task';
    // appMode 变化后需等待渲染
    return wrapper.vm.$nextTick().then(() => {
      expect(wrapper.text()).toContain('暂无任务');
    });
  });

  it('有角色时按列表渲染角色项,不再显示空态', () => {
    const { store } = mountSidebar();
    store.characters = [
      { id: 'c1', chara_name: '绫波', name: '绫波' },
      { id: 'c2', chara_name: '明日香', name: '明日香' },
    ] as never;
    const wrapper = mount(Sidebar);
    expect(wrapper.text()).not.toContain('暂无角色');
    expect(wrapper.text()).toContain('绫波');
    expect(wrapper.text()).toContain('明日香');
  });

  it('侧栏 open 类随 store.sidebarOpen 变化(移动端抽屉)', async () => {
    const { store, wrapper } = mountSidebar();
    expect(wrapper.find('.sv-sidebar').classes()).not.toContain('open');
    store.sidebarOpen = true;
    await wrapper.vm.$nextTick();
    expect(wrapper.find('.sv-sidebar').classes()).toContain('open');
  });

  // ===== CODE-1:任务工作区入口(下达目标表单的「工作区」行) =====

  it('CODE-1:填目标 + 工作区后创建,store.createTask 携带 workspace,并记住最近列表', async () => {
    const { store, wrapper } = await mountTaskSidebar();
    const createSpy = vi.spyOn(store, 'createTask').mockResolvedValue(mkTaskRecord());
    const runSpy = vi.spyOn(store, 'runTask').mockResolvedValue(undefined);

    await wrapper.find('.sv-task-new textarea').setValue('写个周报');
    await wrapper.find('.sv-ws-row input').setValue('D:\\proj\\demo');
    await wrapper.find('.sv-task-new .sv-btn.primary').trigger('click');
    await flushPromises();

    expect(createSpy).toHaveBeenCalledWith('写个周报', undefined, 'D:\\proj\\demo');
    expect(runSpy).toHaveBeenCalledWith('t1');
    expect(memStorage.get('kedai.taskWorkspace.recent.v1')).toBe(
      JSON.stringify(['D:\\proj\\demo']),
    );
    // 创建成功后目标草稿清空(既有行为不因新增行而变)
    expect((wrapper.find('.sv-task-new textarea').element as HTMLTextAreaElement).value).toBe('');
    // 卸载:CODE-4 的探测防抖是 300ms,不卸载会把定时器与组件一起留给后面的用例
    wrapper.unmount();
  });

  it('CODE-1:未填工作区创建时传 undefined(不下发该键),不写最近列表', async () => {
    const { store, wrapper } = await mountTaskSidebar();
    const createSpy = vi.spyOn(store, 'createTask').mockResolvedValue(mkTaskRecord());
    vi.spyOn(store, 'runTask').mockResolvedValue(undefined);

    await wrapper.find('.sv-task-new textarea').setValue('写个周报');
    await wrapper.find('.sv-task-new .sv-btn.primary').trigger('click');
    await flushPromises();

    expect(createSpy).toHaveBeenCalledWith('写个周报', undefined, undefined);
    expect(memStorage.has('kedai.taskWorkspace.recent.v1')).toBe(false);
  });

  it('CODE-1:最近工作区最多展示 3 条,点击回填输入框;输入非空时列表隐藏', async () => {
    memStorage.set(
      'kedai.taskWorkspace.recent.v1',
      JSON.stringify(['D:\\a', 'D:\\b', 'D:\\c', 'D:\\d']),
    );
    const { wrapper } = await mountTaskSidebar();
    const items = wrapper.findAll('.sv-ws-recent-item');
    expect(items.length, '展示上限 3 条(存储上限 8 条)').toBe(3);
    expect(items[0].text()).toBe('D:\\a');
    await items[0].trigger('click');
    expect((wrapper.find('.sv-ws-row input').element as HTMLInputElement).value).toBe('D:\\a');
    expect(wrapper.find('.sv-ws-recent').exists()).toBe(false);
    wrapper.unmount();
  });

  it('CODE-1:编码能力包开启且未选工作区时显示提示;选定后隐藏(缺省关 = 不显示)', async () => {
    const { store, wrapper } = await mountTaskSidebar();
    expect(wrapper.find('.sv-ws-hint').exists()).toBe(false);

    store.taskCodingBundleEnabled = true;
    await wrapper.vm.$nextTick();
    expect(wrapper.find('.sv-ws-hint').exists()).toBe(true);
    expect(wrapper.find('.sv-ws-hint').text()).toContain('草稿目录');

    await wrapper.find('.sv-ws-row input').setValue('D:\\proj');
    expect(wrapper.find('.sv-ws-hint').exists()).toBe(false);
    wrapper.unmount();
  });

  it('CODE-1:浏览器形态不渲染原生「选择…」按钮;「清空」仅在有值时出现且能清空', async () => {
    const { wrapper } = await mountTaskSidebar();
    // jsdom 无 __TAURI_INTERNALS__ → canPickDir=false;未填时两个按钮都不渲染
    expect(wrapper.find('.sv-ws-row .sv-btn').exists()).toBe(false);

    await wrapper.find('.sv-ws-row input').setValue('D:\\proj');
    const btns = wrapper.findAll('.sv-ws-row .sv-btn');
    expect(btns.length).toBe(1);
    expect(btns[0].text()).toBe('清空');
    await btns[0].trigger('click');
    expect((wrapper.find('.sv-ws-row input').element as HTMLInputElement).value).toBe('');
    wrapper.unmount();
  });

  // ===== CODE-4:工作区画像(选定目录后 debounce 探测 + chips 展示) =====

  beforeEach(() => {
    probeWorkspaceMock.mockReset();
    probeWorkspaceMock.mockResolvedValue({
      root: '',
      detected: [],
      scanned_dirs: 0,
      truncated: false,
    });
  });

  it('CODE-4:填工作区后 debounce 探测并渲染 chips(展示名 · 建议命令)', async () => {
    vi.useFakeTimers();
    probeWorkspaceMock.mockResolvedValue({
      root: 'D:\\proj\\demo',
      detected: [
        { kind: 'rust', marker: 'server-rs/Cargo.toml', suggested_command: 'cargo test', depth: 1 },
        { kind: 'node', marker: 'package.json', suggested_command: 'npm test', depth: 0 },
      ],
      scanned_dirs: 1,
      truncated: false,
    });
    const { wrapper } = await mountTaskSidebar();

    await wrapper.find('.sv-ws-row input').setValue('D:\\proj\\demo');
    await wrapper.vm.$nextTick();
    expect(probeWorkspaceMock, '防抖期内不发请求').not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(300);
    await wrapper.vm.$nextTick();
    expect(probeWorkspaceMock).toHaveBeenCalledTimes(1);
    expect(probeWorkspaceMock).toHaveBeenCalledWith('D:\\proj\\demo');
    const chips = wrapper.findAll('.sv-ws-profile .sv-tag');
    expect(chips.map((c) => c.text())).toEqual(['Rust · cargo test', 'Node · npm test']);
    expect(chips[0].attributes('title')).toContain('server-rs/Cargo.toml');
    vi.useRealTimers();
  });

  it('CODE-4:探测失败时把后端中文原因原样上屏,不渲染 chips', async () => {
    vi.useFakeTimers();
    probeWorkspaceMock.mockRejectedValue(new Error('工作区不存在或不是目录:D:\\nope'));
    const { wrapper } = await mountTaskSidebar();

    await wrapper.find('.sv-ws-row input').setValue('D:\\nope');
    await wrapper.vm.$nextTick();
    await vi.advanceTimersByTimeAsync(300);
    await wrapper.vm.$nextTick();

    expect(wrapper.find('.sv-ws-profile').exists()).toBe(false);
    expect(wrapper.text()).toContain('工作区不存在或不是目录:D:\\nope');
    vi.useRealTimers();
  });

  it('CODE-4:清空工作区即撤下 chips 与错误;连续改路径只发最后一次(防抖重置)', async () => {
    vi.useFakeTimers();
    const { wrapper } = await mountTaskSidebar();
    const input = wrapper.find('.sv-ws-row input');

    await input.setValue('D:\\a');
    await wrapper.vm.$nextTick();
    await vi.advanceTimersByTimeAsync(100);
    await input.setValue('D:\\b');
    await wrapper.vm.$nextTick();
    await vi.advanceTimersByTimeAsync(300);
    await wrapper.vm.$nextTick();
    expect(probeWorkspaceMock.mock.calls).toEqual([['D:\\b']]);

    await input.setValue('');
    await wrapper.vm.$nextTick();
    expect(wrapper.find('.sv-ws-profile').exists()).toBe(false);
    // 清空后不再探测(定时器已撤)
    await vi.advanceTimersByTimeAsync(300);
    expect(probeWorkspaceMock).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
});
