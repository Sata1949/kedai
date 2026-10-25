// @vitest-environment jsdom
// C-2A（2026-10-25）Sidebar 拆分后的面板级测试。
// 直接 import 6 个新组件，满足零测试 ratchet；深度交互（建任务全链路、工作区探测）
// 仍由既有 Sidebar.test.ts 覆盖，这里只锁「拆出去后各自仍能独立渲染/触发动作」。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import type { CharacterRecord, TaskRecord } from '../../api';
import { useAppStore } from '../../store';
import SidebarTop from './SidebarTop.vue';
import CharacterListPanel from './CharacterListPanel.vue';
import CharacterPromptEditor from './CharacterPromptEditor.vue';
import TaskComposer from './TaskComposer.vue';
import TaskHistoryList from './TaskHistoryList.vue';
import SidebarFooter from './SidebarFooter.vue';

// 三个选择器子组件各自带连接/流程加载;本文件只测版式与父级动作,统一替换为空壳。
vi.mock('../TaskModeSelect.vue', () => ({ default: { render: () => null } }));
vi.mock('../TaskFlowSelect.vue', () => ({ default: { render: () => null } }));
vi.mock('../TaskConnectionSelect.vue', () => ({ default: { render: () => null } }));

const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

function makeCharacter(overrides: Partial<CharacterRecord> = {}): CharacterRecord {
  return {
    id: 'c1',
    name: 'c1.png',
    chara_name: '小春',
    description: '一位编辑',
    file_path: 'c1.png',
    avatar_path: null,
    created_at: '2026-10-25T00:00:00.000Z',
    ...overrides,
  };
}

function makeTask(overrides: Partial<TaskRecord> = {}): TaskRecord {
  return {
    id: 't1',
    title: '写季度报告',
    status: 'pending',
    plan: [],
    result: '',
    error: '',
    created_at: '2026-10-25T00:00:00.000Z',
    updated_at: '2026-10-25T00:00:00.000Z',
    ...overrides,
  };
}

function freshStore(): ReturnType<typeof useAppStore> {
  setActivePinia(createPinia());
  return useAppStore();
}

describe('C-2A 侧栏面板拆分', () => {
  beforeEach(() => {
    memStorage.clear();
    vi.restoreAllMocks();
    vi.stubGlobal('confirm', vi.fn(() => true));
    vi.stubGlobal('alert', vi.fn());
  });

  it('SidebarTop:连接状态、Token 统计与模式切换', async () => {
    const store = freshStore();
    store.connStatus = 'ok';
    store.appMode = 'task';
    store.currentTaskUsage = { prompt_tokens: 10, completion_tokens: 5, reasoning_tokens: 0 };
    store.globalTaskUsage = { prompt_tokens: 100, completion_tokens: 20, reasoning_tokens: 0 };
    store.setAppMode = vi.fn();
    const wrapper = mount(SidebarTop);
    expect(wrapper.text()).toContain('KEDAI');
    expect(wrapper.text()).toContain('已连接');
    expect(wrapper.text()).toContain('15');
    expect(wrapper.text()).toContain('120');
    expect(wrapper.text()).toContain('全部任务累计');
    const roleplay = wrapper.findAll('.sv-appmode button').find((b) => b.text().includes('角色扮演'));
    if (!roleplay) throw new Error('角色扮演按钮应存在');
    await roleplay.trigger('click');
    expect(store.setAppMode).toHaveBeenCalledWith('roleplay');
  });

  it('CharacterListPanel:列表渲染、搜索 action 与点击选中', async () => {
    const store = freshStore();
    store.characters = [makeCharacter(), makeCharacter({ id: 'c2', chara_name: '阿岚' })];
    store.currentCharacterId = 'c1';
    store.selectCharacter = vi.fn().mockResolvedValue(undefined);
    store.setSearchQuery = vi.fn();
    const wrapper = mount(CharacterListPanel);
    expect(wrapper.text()).toContain('小春');
    expect(wrapper.text()).toContain('阿岚');
    await wrapper.find('.sv-search').setValue('岚');
    expect(store.setSearchQuery).toHaveBeenCalledWith('岚');
    const second = wrapper.findAll('.sv-char-item')[1];
    await second.trigger('click');
    expect(store.selectCharacter).toHaveBeenCalledWith('c2');
  });

  it('CharacterListPanel:右键菜单打开后可进入提示词编辑', async () => {
    const store = freshStore();
    store.characters = [makeCharacter()];
    const wrapper = mount(CharacterListPanel, { attachTo: document.body });
    await wrapper.find('.sv-char-item').trigger('contextmenu', { clientX: 10, clientY: 10 });
    await flushPromises();
    const menu = document.body.querySelector('.sv-ctx-menu');
    expect(menu).not.toBeNull();
    const edit = document.body.querySelector<HTMLButtonElement>('.sv-ctx-item');
    if (!edit) throw new Error('编辑提示词入口应存在');
    edit.click();
    await flushPromises();
    expect(document.body.querySelector('.pm-edit')).not.toBeNull();
    wrapper.unmount();
  });

  it('CharacterPromptEditor:编辑草稿并保存到 store.updateCharacterPrompt', async () => {
    const store = freshStore();
    store.updateCharacterPrompt = vi.fn().mockResolvedValue(undefined);
    const wrapper = mount(CharacterPromptEditor, {
      props: { character: makeCharacter({ first_mes: '你好', alternate_greetings: ['早'] }) },
      attachTo: document.body,
    });
    await flushPromises();
    expect(document.body.textContent).toContain('编辑提示词');
    const textareas = document.body.querySelectorAll<HTMLTextAreaElement>('textarea');
    if (!textareas[0]) throw new Error('提示词输入框应存在');
    textareas[0].value = '新提示词';
    textareas[0].dispatchEvent(new Event('input'));
    await flushPromises();
    const save = [...document.body.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.includes('保存'));
    if (!save) throw new Error('保存按钮应存在');
    save.click();
    await flushPromises();
    expect(store.updateCharacterPrompt).toHaveBeenCalledWith('c1', expect.objectContaining({ description: '新提示词', first_mes: '你好' }));
    wrapper.unmount();
  });

  it('TaskComposer:输入目标后创建并执行', async () => {
    const store = freshStore();
    store.appMode = 'task';
    store.createTask = vi.fn().mockResolvedValue(makeTask());
    store.runTask = vi.fn().mockResolvedValue(undefined);
    store.loadExecutors = vi.fn().mockResolvedValue(undefined);
    const wrapper = mount(TaskComposer);
    expect(wrapper.find('textarea').attributes('placeholder')).toContain('描述要完成的任务');
    await wrapper.find('textarea').setValue('写一份周报');
    const submit = wrapper.findAll('button').find((b) => b.text().includes('创建并执行'));
    if (!submit) throw new Error('创建按钮应存在');
    await submit.trigger('click');
    await flushPromises();
    expect(store.createTask).toHaveBeenCalledWith('写一份周报', undefined, undefined);
    expect(store.runTask).toHaveBeenCalledWith('t1');
  });

  it('TaskComposer:工作区 placeholder 与清空入口保留(源契约的 DOM 侧)', async () => {
    const store = freshStore();
    store.appMode = 'task';
    store.loadExecutors = vi.fn().mockResolvedValue(undefined);
    const wrapper = mount(TaskComposer);
    const ws = wrapper.find('input[placeholder="工作区:项目目录绝对路径"]');
    expect(ws.exists()).toBe(true);
    await ws.setValue('D:/work');
    const clear = wrapper.findAll('button').find((b) => b.text() === '清空');
    if (!clear) throw new Error('清空按钮应存在');
    await clear.trigger('click');
    expect((ws.element as HTMLInputElement).value).toBe('');
  });

  it('TaskHistoryList:任务行、状态文案与删除动作', async () => {
    const store = freshStore();
    store.tasks = [makeTask(), makeTask({ id: 't2', title: '第二件事', status: 'done' })];
    store.currentTaskId = 't1';
    store.deleteTask = vi.fn().mockResolvedValue(undefined);
    store.selectTask = vi.fn().mockResolvedValue(true);
    const wrapper = mount(TaskHistoryList);
    expect(wrapper.text()).toContain('写季度报告');
    expect(wrapper.text()).toContain('已完成');
    const del = wrapper.findAll('.sv-task-item-del')[1];
    await del.trigger('click');
    await flushPromises();
    expect(confirm).toHaveBeenCalled();
    expect(store.deleteTask).toHaveBeenCalledWith('t2');
    const first = wrapper.findAll('.sv-task-item')[0];
    await first.trigger('click');
    expect(store.selectTask).toHaveBeenCalledWith('t1');
  });

  it('SidebarFooter:导入/设置/退出入口与 state 开关', async () => {
    const store = freshStore();
    const wrapper = mount(SidebarFooter);
    expect(wrapper.text()).toContain('导入角色卡');
    expect(wrapper.text()).toContain('综合设置');
    expect(wrapper.text()).toContain('退出 Kedai');
    const settings = wrapper.findAll('button').find((b) => b.text().includes('综合设置'));
    const exit = wrapper.findAll('button').find((b) => b.text().includes('退出 Kedai'));
    if (!settings || !exit) throw new Error('底部入口应存在');
    await settings.trigger('click');
    expect(store.settingsOpen).toBe(true);
    await exit.trigger('click');
    expect(store.exitConfirmOpen).toBe(true);
    // 隐藏 file input 用于导入(事件通道由 ChatWindow 空态复用)
    expect(wrapper.find('input[type="file"]').exists()).toBe(true);
  });
});
