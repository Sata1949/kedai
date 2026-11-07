// @vitest-environment jsdom
// UIREFINE：视觉减法不改变发送、模式切换或设置导航的行为。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { readFileSync } from 'node:fs';
import { useAppStore } from '../store';
import ChatInput from './ChatInput.vue';
import SidebarTop from './sidebar/SidebarTop.vue';
import SettingsHub from './SettingsHub.vue';
import TaskBoardEmpty from './task/TaskBoardEmpty.vue';

vi.mock('../api', async (original) => ({
  ...await original<typeof import('../api')>(),
  listSlashCommands: vi.fn().mockResolvedValue([]),
  listQuickReplies: vi.fn().mockResolvedValue([]),
}));
vi.mock('../api/health', () => ({ health: vi.fn().mockResolvedValue({ ok: true }) }));
vi.mock('./SettingsModal.vue', () => ({ default: { template: '<div>设置内容</div>' } }));
const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');
enableAutoUnmount(afterEach);
beforeEach(() => { localStorage.clear(); setActivePinia(createPinia()); });
afterEach(() => vi.restoreAllMocks());

describe('UIREFINE 视觉层级与入口', () => {
  it('附件与快速回复保持同一操作组，快捷菜单仍可打开', async () => {
    const wrapper = mount(ChatInput);
    await flushPromises();
    const actions = wrapper.get('.sv-composer-actions');
    expect(actions.findAll('button')).toHaveLength(2);
    expect(actions.get('[aria-label="选择文件"]').text()).toBe('附件');
    await actions.get('[aria-label="快速回复"]').trigger('click');
    expect(wrapper.get('.sv-slash-empty').text()).toContain('暂无启用');
    expect(wrapper.findAll('.sv-mode button')).toHaveLength(4);
    expect(wrapper.findAll('.sv-auth-toggle button')).toHaveLength(3);
  });

  it('侧栏只保留累计用量，上下文与命中率由顶栏承载', () => {
    const store = useAppStore();
    store.sessionTotalTokens = 120; store.globalTotalTokens = 450;
    const wrapper = mount(SidebarTop);
    expect(wrapper.findAll('.sv-token-cell')).toHaveLength(2);
    expect(wrapper.text()).toContain('120'); expect(wrapper.text()).toContain('450');
    expect(wrapper.text()).not.toContain('上下文 TOKEN');
    expect(wrapper.text()).not.toContain('缓存命中率');
  });

  it('正文页眉只显示会话，不重复顶栏角色和输入区模型', () => {
    const source = read('./ChatWindow.vue');
    const header = source.slice(source.indexOf('<div v-else class="sv-chat-masthead">'), source.indexOf('<!-- 消息列表'));
    expect(header).toContain('{{ currentSessionTitle }}');
    expect(header).toContain('class="sv-chat-masthead-title truncate" :title="currentSessionTitle"');
    expect(header).not.toContain('currentCharacterName');
    expect(header).not.toContain('{{ model');
    expect(read('./ChatTopbar.vue')).not.toContain('v-if="model"');
  });

  it('移动端任务面板与模式切换具有不同名称和切换图标', () => {
    const source = read('../App.vue');
    const nav = source.slice(source.indexOf('<nav class="sv-mobile-nav"'), source.indexOf('</nav>'));
    expect(nav).toContain('v-if="store.appMode === \'roleplay\'">角色库</span>');
    expect(nav).toContain('<span v-else>任务面板</span>');
    expect(nav).toContain('<span>切换模式</span>');
    expect(nav).toContain('@click="toggleAppMode"');
    expect(nav).not.toContain(':class="{ active: true }"');
  });

  it('设置子导航明确所属域，切换一级时同步更新分组名', async () => {
    const wrapper = mount(SettingsHub);
    expect(wrapper.get('.hub-sub-label').text()).toBe('连接与模型');
    await wrapper.findAll('.sv-hub-nav-item')[2].trigger('click');
    expect(wrapper.get('.hub-sub-label').text()).toBe('Agent 与任务');
    expect(wrapper.get('.hub-sub-item.active').text()).toContain('Agent 设置');
  });

  it('任务空态模式是说明清单而非假按钮，主入口仍打开目标输入区', async () => {
    const store = useAppStore();
    const open = vi.spyOn(store, 'openSidebar').mockImplementation(() => {});
    const wrapper = mount(TaskBoardEmpty);
    const modes = wrapper.get('[aria-label="执行模式一览"]');
    expect(modes.findAll('.sv-task-start-mode')).toHaveLength(6);
    expect(modes.findAll('button, .active')).toHaveLength(0);
    await wrapper.get('.sv-task-start-actions button').trigger('click');
    expect(open).toHaveBeenCalledOnce();
  });
});
