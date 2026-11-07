// @vitest-environment jsdom
// 三档授权选择器(2026-11-06)的行为测试。
//
// 覆盖四件事:默认档 = workspace(旧行为)、三档可选、选择持久化、脏数据不放大权限。
// 用 jsdom 挂载真组件(参照 TaskConnectionSelect.test.ts),不打桩子组件。
import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';
import TaskAuthorizationSelect from './TaskAuthorizationSelect.vue';
import { useAppStore } from '../store';
import { reloadTaskAuthorizationTierForTest } from '../composables/useTaskAuthorizationTier';

const KEY = 'kedai.taskAuthorizationTier.v1';

function mountSelect() {
  // 档位是模块级单例:用例之间必须显式重载,否则上一例的选择会泄漏到下一例
  reloadTaskAuthorizationTierForTest();
  setActivePinia(createPinia());
  const store = useAppStore();
  const wrapper = mount(TaskAuthorizationSelect);
  return { wrapper, store };
}

describe('TaskAuthorizationSelect 三档授权选择器', () => {
  beforeEach(() => {
    localStorage.clear();
    setActivePinia(createPinia());
  });

  it('默认档是 workspace(等于本档上线前的行为)', () => {
    const { wrapper, store } = mountSelect();
    expect(store.taskAuthorizationTier).toBe('workspace');
    const select = wrapper.get('select');
    expect((select.element as HTMLSelectElement).value).toBe('workspace');
  });

  it('三个选项的顺序与文案固定(逐步 / 工作区内 / 自动)', () => {
    const { wrapper } = mountSelect();
    const labels = wrapper.findAll('option').map((o) => o.text());
    expect(labels).toHaveLength(3);
    expect(labels[0]).toContain('逐步');
    expect(labels[1]).toContain('工作区内');
    expect(labels[2]).toContain('自动');
  });

  it('选择 auto 写回 store 并持久化到 localStorage', async () => {
    const { wrapper, store } = mountSelect();
    await wrapper.get('select').setValue('auto');
    await nextTick();
    expect(store.taskAuthorizationTier).toBe('auto');
    expect(localStorage.getItem(KEY)).toBe('auto');
  });

  it('选择 step 时提示写类工具需确认,且不带高危警示样式', async () => {
    const { wrapper } = mountSelect();
    await wrapper.get('select').setValue('step');
    await nextTick();
    const note = wrapper.get('.sv-note');
    expect(note.text()).toContain('确认');
    expect(note.classes()).not.toContain('flow-tool-warn');
  });

  it('选择 auto 时给出高危警示(破坏性命令会执行)', async () => {
    const { wrapper } = mountSelect();
    await wrapper.get('select').setValue('auto');
    await nextTick();
    const note = wrapper.get('.sv-note');
    expect(note.classes()).toContain('flow-tool-warn');
    expect(note.text()).toContain('审计');
  });

  it('localStorage 里是脏值时回落到 workspace(不得放大权限)', () => {
    localStorage.setItem(KEY, 'full');
    const { store } = mountSelect();
    expect(store.taskAuthorizationTier).toBe('workspace');
  });

  it('已持久化的合法值会被恢复', () => {
    localStorage.setItem(KEY, 'step');
    const { store } = mountSelect();
    expect(store.taskAuthorizationTier).toBe('step');
  });
});
