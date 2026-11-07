// @vitest-environment jsdom
// 前端细节修复：覆盖调查中实际复现的输入、异步回执和弹窗边界。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';
import * as api from '../api';
import type { WorkspaceProfile } from '../api';
import { useAppStore } from '../store';
import { useApiSettings } from '../composables/useApiSettings';
import ChatInput from './ChatInput.vue';
import Sidebar from './Sidebar.vue';
import TaskComposer from './sidebar/TaskComposer.vue';
import SettingsHub from './SettingsHub.vue';
import ConnectionSection from './settings/ConnectionSection.vue';
import appSource from '../App.vue?raw';

const { probeWorkspace } = vi.hoisted(() => ({ probeWorkspace: vi.fn() }));
vi.mock('../api/workspace', () => ({ probeWorkspace }));
vi.mock('../api', async (importOriginal) => ({
  ...await importOriginal<typeof import('../api')>(),
  listSlashCommands: vi.fn().mockResolvedValue([]),
  listQuickReplies: vi.fn().mockResolvedValue([]),
  settingsInfo: vi.fn().mockResolvedValue({ connector: 'mock', model: 'test', models: [] }),
  getSettings: vi.fn().mockResolvedValue({ openai_base_url: '', api_key_masked: '', has_api_key: false, model: 'test' }),
}));
vi.mock('../api/health', () => ({ health: vi.fn().mockResolvedValue({ ok: true }) }));
vi.mock('./SettingsModal.vue', () => ({ default: { template: '<div><div style="display:none"><button>隐藏控件</button></div><button disabled>禁用控件</button><button data-last>末项</button></div>' } }));

const stubs = { TaskModeSelect: true, TaskFlowSelect: true, TaskConnectionSelect: true, TaskAuthorizationSelect: true, SidebarTop: true, CharacterListPanel: true, SidebarFooter: true, TaskHistoryList: true };
enableAutoUnmount(afterEach);
beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
  vi.spyOn(useAppStore(), 'loadExecutors').mockResolvedValue(undefined);
  probeWorkspace.mockReset().mockResolvedValue({ detected: [], truncated: false });
});
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); document.body.innerHTML = ''; });

function deferred<T>() {
  let finish: ((value: T) => void) | undefined;
  const promise = new Promise<T>((resolve) => { finish = resolve; });
  return { promise, resolve: (value: T) => finish?.(value) };
}

describe('前端细节：草稿与发送快照', () => {
  it('首尾空白只影响发送载荷，受理后仍清空原草稿', async () => {
    const store = useAppStore(); store.currentCharacterId = 'c1';
    const send = vi.spyOn(store, 'sendMessage').mockResolvedValue({ accepted: true });
    const wrapper = mount(ChatInput);
    await wrapper.get('textarea').setValue('  草稿\n');
    await wrapper.get('.sv-btn-send').trigger('click'); await flushPromises();
    expect(send).toHaveBeenCalledWith('草稿', []);
    expect(wrapper.get('textarea').element.value).toBe('');
  });

  it('等待受理时新增的文本与附件不被上一条回执清掉', async () => {
    const store = useAppStore(); store.currentCharacterId = 'c1';
    const pending = deferred<api.SendOutcome>();
    const send = vi.spyOn(store, 'sendMessage').mockReturnValue(pending.promise);
    const wrapper = mount(ChatInput);
    await wrapper.get('textarea').setValue('上一条');
    await wrapper.get('.sv-btn-send').trigger('click');
    await wrapper.get('textarea').setValue('下一条');
    const picker = wrapper.get('input[type="file"]');
    Object.defineProperty(picker.element, 'files', { configurable: true, value: [new File(['内容'], '下一条.txt')] });
    await picker.trigger('change');
    pending.resolve({ accepted: true }); await flushPromises();
    expect(send).toHaveBeenCalledWith('上一条', []);
    expect(wrapper.get('textarea').element.value).toBe('下一条');
    expect(wrapper.get('.sv-attach-chip').text()).toContain('下一条.txt');
  });

  it('任务模式重建后恢复目标、工作区和执行者', async () => {
    const store = useAppStore(); store.appMode = 'task';
    store.executors = [{ id: 'e1', name: '执行者', instruction: '', created_at: '', updated_at: '' }];
    const wrapper = mount(Sidebar, { global: { stubs } });
    await wrapper.get('textarea').setValue('未提交目标');
    await wrapper.get('.sv-ws-row input').setValue('D:/example');
    await wrapper.get('select[title="选择任务执行者"]').setValue('e1');
    store.appMode = 'roleplay'; await nextTick();
    store.appMode = 'task'; await nextTick();
    expect(wrapper.get('textarea').element.value).toBe('未提交目标');
    expect(wrapper.get<HTMLInputElement>('.sv-ws-row input').element.value).toBe('D:/example');
    expect(wrapper.get<HTMLSelectElement>('select[title="选择任务执行者"]').element.value).toBe('e1');
  });

  it('聊天草稿与附件跨挂载保留，按会话隔离且不写浏览器存储', async () => {
    const store = useAppStore(); store.currentCharacterId = 'c1'; store.currentSessionId = 's1';
    const initialStorage = localStorage.length;
    const wrapper = mount(ChatInput);
    await wrapper.get('textarea').setValue('会话一草稿');
    const picker = wrapper.get('input[type="file"]');
    Object.defineProperty(picker.element, 'files', { configurable: true, value: [new File(['内容'], '草稿.txt')] });
    await picker.trigger('change'); wrapper.unmount();
    const remounted = mount(ChatInput);
    expect(remounted.get('textarea').element.value).toBe('会话一草稿');
    expect(remounted.get('.sv-attach-chip').text()).toContain('草稿.txt');
    store.currentSessionId = 's2'; await nextTick();
    expect(remounted.get('textarea').element.value).toBe('');
    await remounted.get('textarea').setValue('会话二草稿');
    store.currentSessionId = 's1'; await nextTick();
    expect(remounted.get('textarea').element.value).toBe('会话一草稿');
    expect(localStorage.length).toBe(initialStorage);
  });

  it('首条发送新建会话时承接原草稿，失败时仍可重试', async () => {
    const store = useAppStore(); store.currentCharacterId = 'c1';
    vi.spyOn(store, 'sendMessage').mockImplementation(async () => {
      store.currentSessionId = 'new-session';
      return { accepted: false };
    });
    const wrapper = mount(ChatInput);
    await wrapper.get('textarea').setValue('首条草稿');
    await wrapper.get('.sv-btn-send').trigger('click'); await flushPromises();
    expect(wrapper.get('textarea').element.value).toBe('首条草稿');
    wrapper.unmount();
    expect(mount(ChatInput).get('textarea').element.value).toBe('首条草稿');
  });

  it('等待受理时重挂载输入组件也不能重复提交', async () => {
    const store = useAppStore(); store.currentCharacterId = 'c1';
    const pending = deferred<api.SendOutcome>();
    const send = vi.spyOn(store, 'sendMessage').mockReturnValue(pending.promise);
    const first = mount(ChatInput);
    await first.get('textarea').setValue('正在提交');
    await first.get('.sv-btn-send').trigger('click'); first.unmount();
    const next = mount(ChatInput);
    expect(next.get('.sv-btn-send').attributes('disabled')).toBeDefined();
    await next.get('textarea').trigger('keydown', { key: 'Enter' });
    expect(send).toHaveBeenCalledTimes(1);
    pending.resolve({ accepted: false }); await flushPromises();
    expect(next.get('textarea').element.value).toBe('正在提交');
    expect(next.get('.sv-btn-send').attributes('disabled')).toBeUndefined();
  });

  it('旧会话的迟到受理回执不能清空新会话草稿', async () => {
    const store = useAppStore(); store.currentCharacterId = 'c1'; store.currentSessionId = 's1';
    const pending = deferred<api.SendOutcome>();
    vi.spyOn(store, 'sendMessage').mockReturnValue(pending.promise);
    const wrapper = mount(ChatInput);
    await wrapper.get('textarea').setValue('会话一消息');
    await wrapper.get('.sv-btn-send').trigger('click');
    store.currentSessionId = 's2'; await nextTick();
    await wrapper.get('textarea').setValue('会话二草稿');
    pending.resolve({ accepted: true }); await flushPromises();
    expect(wrapper.get('textarea').element.value).toBe('会话二草稿');
    store.currentSessionId = 's1'; await nextTick();
    expect(wrapper.get('textarea').element.value).toBe('');
  });
});

describe('前端细节：过期探测与加载状态', () => {
  it('清空或改写工作区后，旧探测结果立即作废', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const store = useAppStore(); store.appMode = 'task';
    const pending = deferred<WorkspaceProfile>(); probeWorkspace.mockReturnValue(pending.promise);
    const wrapper = mount(TaskComposer, { global: { stubs } });
    await wrapper.get('.sv-ws-row input').setValue('D:/old');
    await vi.advanceTimersByTimeAsync(300);
    await wrapper.get('.sv-ws-row input').setValue('');
    pending.resolve({ root: 'D:/old', detected: [{ kind: 'node', marker: 'package.json', depth: 0, suggested_command: 'npm test' }], truncated: false, scanned_dirs: 1 });
    await flushPromises();
    expect(wrapper.find('.sv-ws-profile').exists()).toBe(false);
  });

  it('模型列表加载完但为空时显示空态，不继续显示骨架', async () => {
    const state = useApiSettings(); await state.loadApiSettings();
    const wrapper = mount(ConnectionSection, { props: { state } });
    expect(wrapper.findComponent({ name: 'SkeletonBlock' }).exists()).toBe(false);
    expect(wrapper.text()).toContain('暂无可用模型');
  });

  it('初始连接信息加载失败时显示错误和重试，而非无限等待', async () => {
    vi.spyOn(api, 'settingsInfo').mockRejectedValueOnce(new Error('网络不可达'));
    const state = useApiSettings(); await state.loadApiSettings();
    const wrapper = mount(ConnectionSection, { props: { state } });
    expect(wrapper.findComponent({ name: 'SkeletonBlock' }).exists()).toBe(false);
    expect(wrapper.text()).toContain('网络不可达');
    expect(wrapper.text()).toContain('重试');
  });
});

describe('前端细节：综合设置键盘操作', () => {
  it('自动聚焦、首尾 Tab 循环、Esc 关闭并恢复入口焦点', async () => {
    const store = useAppStore(); store.settingsOpen = true;
    const trigger = document.createElement('button'); document.body.append(trigger); trigger.focus();
    const wrapper = mount(SettingsHub, { attachTo: document.body }); await flushPromises();
    const dialog = wrapper.get('[role="dialog"]');
    expect(dialog.attributes('aria-modal')).toBe('true');
    const close = wrapper.get<HTMLButtonElement>('button[aria-label="关闭综合设置"]');
    expect(document.activeElement).toBe(close.element);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(wrapper.get('[data-last]').element);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(close.element);
    trigger.focus(); expect(document.activeElement).toBe(close.element);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    expect(store.settingsOpen).toBe(false);
    wrapper.unmount(); expect(document.activeElement).toBe(trigger);
  });

  it('移动端侧栏入口文案随顶层模式切换', () => {
    expect(appSource).toContain('<span v-if="store.appMode === \'roleplay\'">角色库</span>');
    expect(appSource).toContain('<span v-else>任务面板</span>');
  });
});
