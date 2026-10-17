// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
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

import { useAppStore } from '../store';
import ChatTopbar from './ChatTopbar.vue';

function mountTopbar() {
  setActivePinia(createPinia());
  return { store: useAppStore(), wrapper: mount(ChatTopbar) };
}

describe('ChatTopbar 顶栏', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('渲染技术矩阵与会话入口', () => {
    const { wrapper } = mountTopbar();
    expect(wrapper.find('.sv-topbar').exists()).toBe(true);
    const stats = wrapper.find('.sv-topbar-stats');
    expect(stats.exists()).toBe(true);
    expect(stats.text()).toContain('CTX');
    expect(stats.text()).toContain('本轮');
    wrapper.unmount();
  });

  it('⋯ 菜单可开合,Esc 关闭', async () => {
    const { wrapper } = mountTopbar();
    await wrapper.find('.sv-topbar-more-btn').trigger('click');
    expect(wrapper.find('.sv-topbar-more-menu').exists()).toBe(true);
    expect(wrapper.find('.sv-topbar-more-menu').attributes('role')).toBe('menu');

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    await wrapper.vm.$nextTick();
    expect(wrapper.find('.sv-topbar-more-menu').exists()).toBe(false);
    wrapper.unmount();
  });

  it('菜单里的 HTML 开关仍走 store action', async () => {
    const { store, wrapper } = mountTopbar();
    const spy = vi.spyOn(store, 'toggleRenderHtml');
    await wrapper.find('.sv-topbar-more-btn').trigger('click');
    await wrapper.findAll('.sv-topbar-more-item')[0].trigger('click');
    expect(spy).toHaveBeenCalled();
    wrapper.unmount();
  });
});
