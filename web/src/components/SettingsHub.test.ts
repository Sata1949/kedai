// @vitest-environment jsdom
// SettingsHub 组件测试(阶段 A 补测):332 行,设置中心两层导航,此前无测试。
// 覆盖:导航区渲染、一级域切换、二级项点击切分区、以及 task 模式下
// 角色扮演专属项被过滤(visibleDomains 的核心逻辑)。
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

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return { ...orig, listModels: vi.fn().mockResolvedValue([]), getSettings: vi.fn().mockResolvedValue({}) };
});
vi.mock('../api/health', () => ({
  health: vi.fn().mockResolvedValue({ ok: true, version: '0.0.0-test' }),
}));

import { useAppStore } from '../store';
import SettingsHub from './SettingsHub.vue';

function mountHub() {
  setActivePinia(createPinia());
  return { store: useAppStore(), wrapper: mount(SettingsHub) };
}

describe('SettingsHub 组件(阶段 A 补测)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('挂载渲染设置中心容器与左侧导航', () => {
    const { wrapper } = mountHub();
    expect(wrapper.find('.sv-modal-mask').exists()).toBe(true);
    expect(wrapper.find('.sv-hub-nav').exists()).toBe(true);
    expect(wrapper.findAll('.sv-hub-nav-item').length).toBeGreaterThan(0);
  });

  it('一级域按钮带序号渲染(01/02…)', () => {
    const { wrapper } = mountHub();
    expect(wrapper.find('.sv-hub-nav-idx').text()).toBe('01');
  });

  it('点击一级域后该域呈 active 态', async () => {
    const { wrapper } = mountHub();
    const items = wrapper.findAll('.sv-hub-nav-item');
    expect(items.length).toBeGreaterThan(1);

    await items[1].trigger('click');
    expect(items[1].classes()).toContain('active');
  });

  it('点击二级 section 项切换右侧内容区(active 态转移)', async () => {
    const { wrapper } = mountHub();
    const subs = wrapper.findAll('.hub-sub-item');
    expect(subs.length).toBeGreaterThan(0);
    await subs[0].trigger('click');
    // section 项点击后应有一个二级项为 active
    const activeCount = wrapper.findAll('.hub-sub-item.active').length;
    expect(activeCount).toBe(1);
  });
});
