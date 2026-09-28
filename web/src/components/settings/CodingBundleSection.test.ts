// @vitest-environment jsdom
// CodingBundleSection 组件测试(任务模式编码能力包开关,默认关)。
// 覆盖:默认关闭态与语义文案、「只影响默认值 / 不影响角色扮演」的说明、
// show 显隐、切换走既有 queueSettingsSave 串行通道、保存失败回滚本地值。
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

import { useAppStore } from '../../store';
import CodingBundleSection from './CodingBundleSection.vue';

function mountSection(props: { show?: boolean } = {}) {
  setActivePinia(createPinia());
  const store = useAppStore();
  const wrapper = mount(CodingBundleSection, { props });
  return { store, wrapper };
}

/** 开关 input(本区只有一个 checkbox) */
function toggle(wrapper: ReturnType<typeof mount>) {
  return wrapper.find('input[type="checkbox"]');
}

describe('CodingBundleSection(编码能力包区)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('渲染开关与说明,默认关闭', () => {
    const { wrapper } = mountSection();
    expect(wrapper.text()).toContain('编码能力包');
    expect(wrapper.text()).toContain('启用编码能力包');
    expect(wrapper.text()).toContain('已关闭');
    expect((toggle(wrapper).element as HTMLInputElement).checked).toBe(false);
  });

  it('说明写清语义:编码执行者模板 / 自定义优先 / 不影响角色扮演 / 仅任务模式', () => {
    const { wrapper } = mountSection();
    const text = wrapper.text();
    expect(text).toContain('编码执行者模板');
    expect(text).toContain('先读后写');
    expect(text).toContain('以自定义值为准');
    expect(text).toContain('不影响角色扮演模式');
    expect(text).toContain('仅任务模式生效');
  });

  it('show=false 时根节点 display:none(embedded 模式按 activeSection 切换)', () => {
    const { wrapper } = mountSection({ show: false });
    expect(wrapper.find('.sv-field').attributes('style')).toContain('display: none');
  });

  it('开启开关走 queueSettingsSave,补丁字段名逐字为 task_coding_bundle_enabled', async () => {
    const { store, wrapper } = mountSection();
    const spy = vi.spyOn(store, 'queueSettingsSave').mockResolvedValue(undefined);

    await toggle(wrapper).setValue(true);
    await Promise.resolve();

    expect(spy).toHaveBeenCalledWith({ task_coding_bundle_enabled: true });
    expect(store.taskCodingBundleEnabled).toBe(true);
    expect(wrapper.text()).toContain('已开启');
  });

  it('再次关闭同样走 queueSettingsSave(携带 false)', async () => {
    const { store, wrapper } = mountSection();
    store.taskCodingBundleEnabled = true;
    await wrapper.vm.$nextTick();
    const spy = vi.spyOn(store, 'queueSettingsSave').mockResolvedValue(undefined);

    await toggle(wrapper).setValue(false);
    await Promise.resolve();

    expect(spy).toHaveBeenCalledWith({ task_coding_bundle_enabled: false });
    expect(store.taskCodingBundleEnabled).toBe(false);
  });

  it('保存失败回滚开关并给出失败提示(不留下与服务端分叉的 UI 态)', async () => {
    const { store, wrapper } = mountSection();
    vi.spyOn(store, 'queueSettingsSave').mockRejectedValue(new Error('503 服务不可用'));

    await toggle(wrapper).setValue(true);
    await Promise.resolve();
    await Promise.resolve();

    expect(store.taskCodingBundleEnabled).toBe(false);
    expect(wrapper.text()).toContain('保存失败');
    expect((toggle(wrapper).element as HTMLInputElement).checked).toBe(false);
  });
});
