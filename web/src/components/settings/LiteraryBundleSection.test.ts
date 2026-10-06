// @vitest-environment jsdom
// LiteraryBundleSection 组件测试(文学能力包双开关,默认关;LIT-1)。
// 覆盖:默认关闭态与语义文案、「只影响默认值 / 两模式各自生效 / 编码优先」的说明、
// show 显隐、两个开关各自走既有 queueSettingsSave 串行通道(补丁字段名逐字正确)、
// 保存失败回滚本地值且不波及另一个开关。
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
import LiteraryBundleSection from './LiteraryBundleSection.vue';

function mountSection(props: { show?: boolean } = {}) {
  setActivePinia(createPinia());
  const store = useAppStore();
  const wrapper = mount(LiteraryBundleSection, { props });
  return { store, wrapper };
}

/** 本区有两个开关:0 = 角色扮演侧,1 = 任务侧。取不到即抛——
 *  不用非空断言(`x!`),避免推高 `check-frontend-lint` 的 ratchet 基线(只降不升)。 */
function toggleAt(wrapper: ReturnType<typeof mount>, index: number) {
  const box = wrapper.findAll('input[type="checkbox"]')[index];
  if (!box) throw new Error(`缺少第 ${index} 个开关`);
  return box;
}

/** 本区有两个下拉:0 = 文风预设(LIT-6),1 = 长程一致性推荐档(LIT-7)。取不到即抛(同上)。 */
function selectAt(wrapper: ReturnType<typeof mount>, index: number) {
  const sel = wrapper.findAll('select')[index];
  if (!sel) throw new Error(`缺少第 ${index} 个下拉`);
  return sel;
}

describe('LiteraryBundleSection(文学能力包区)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('渲染两个开关与说明,默认均关闭', () => {
    const { wrapper } = mountSection();
    expect(wrapper.text()).toContain('文学能力包');
    expect(wrapper.text()).toContain('启用文学能力包(角色扮演)');
    expect(wrapper.text()).toContain('任务模式启用文学能力包');
    expect(wrapper.text()).toContain('已关闭');
    expect(wrapper.findAll('input[type="checkbox"]')).toHaveLength(2);
    expect((toggleAt(wrapper, 0).element as HTMLInputElement).checked).toBe(false);
    expect((toggleAt(wrapper, 1).element as HTMLInputElement).checked).toBe(false);
  });

  it('说明写清语义:只影响默认值 / 自定义优先 / 两模式各自生效 / 与编码包同开时编码优先', () => {
    const { wrapper } = mountSection();
    const text = wrapper.text();
    expect(text).toContain('只影响默认值');
    expect(text).toContain('以自定义值为准');
    expect(text).toContain('只影响角色扮演模式');
    expect(text).toContain('只影响任务模式');
    expect(text).toContain('以编码模板优先');
  });

  it('show=false 时根节点 display:none(embedded 模式按 activeSection 切换)', () => {
    const { wrapper } = mountSection({ show: false });
    expect(wrapper.find('.sv-field').attributes('style')).toContain('display: none');
  });

  it('角色扮演侧开关走 queueSettingsSave,补丁字段名逐字为 literary_bundle_enabled', async () => {
    const { store, wrapper } = mountSection();
    const spy = vi.spyOn(store, 'queueSettingsSave').mockResolvedValue(undefined);
    const loadSpy = vi.spyOn(store, 'loadAgentFlow').mockResolvedValue(undefined);

    await toggleAt(wrapper, 0).setValue(true);
    await Promise.resolve();
    await Promise.resolve();

    expect(spy).toHaveBeenCalledWith({ literary_bundle_enabled: true });
    expect(store.literaryBundleEnabled).toBe(true);
    expect(store.taskLiteraryBundleEnabled).toBe(false); // 另一个开关不被波及
    expect(wrapper.text()).toContain('已开启');
    // LIT-5:开包会在服务端并入四条文学流程 → 保存成功后必须刷新流程库缓存
    expect(loadSpy).toHaveBeenCalled();
  });

  it('任务侧开关走 queueSettingsSave,补丁字段名逐字为 task_literary_bundle_enabled;再次关闭携带 false', async () => {
    const { store, wrapper } = mountSection();
    const spy = vi.spyOn(store, 'queueSettingsSave').mockResolvedValue(undefined);
    const loadSpy = vi.spyOn(store, 'loadAgentFlow').mockResolvedValue(undefined);

    await toggleAt(wrapper, 1).setValue(true);
    await Promise.resolve();
    await Promise.resolve();
    expect(spy).toHaveBeenCalledWith({ task_literary_bundle_enabled: true });
    expect(store.taskLiteraryBundleEnabled).toBe(true);

    await toggleAt(wrapper, 1).setValue(false);
    await Promise.resolve();
    await Promise.resolve();
    expect(spy).toHaveBeenLastCalledWith({ task_literary_bundle_enabled: false });
    expect(store.taskLiteraryBundleEnabled).toBe(false);
    // 开/关都要刷新:关包不回收已注入副本,刷新只是让缓存与服务端一致
    expect(loadSpy).toHaveBeenCalledTimes(2);
  });

  it('保存失败回滚开关并给出失败提示(不留下与服务端分叉的 UI 态)', async () => {
    const { store, wrapper } = mountSection();
    store.taskLiteraryBundleEnabled = true; // 另一开关已开:失败回滚不得波及它
    await wrapper.vm.$nextTick();
    vi.spyOn(store, 'queueSettingsSave').mockRejectedValue(new Error('503 服务不可用'));
    const loadSpy = vi.spyOn(store, 'loadAgentFlow').mockResolvedValue(undefined);

    await toggleAt(wrapper, 0).setValue(true);
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(store.literaryBundleEnabled).toBe(false);
    expect(store.taskLiteraryBundleEnabled).toBe(true);
    expect(wrapper.text()).toContain('保存失败');
    expect((toggleAt(wrapper, 0).element as HTMLInputElement).checked).toBe(false);
    expect(loadSpy).not.toHaveBeenCalled(); // 保存失败不刷新(服务端没并入任何东西)
  });

  it('文风预设下拉:选项键与服务端常量逐字对齐,改动走 queueSettingsSave 且字段名逐字正确', async () => {
    const { store, wrapper } = mountSection();
    const spy = vi.spyOn(store, 'queueSettingsSave').mockResolvedValue(undefined);
    vi.spyOn(store, 'loadAgentFlow').mockResolvedValue(undefined);

    expect(wrapper.findAll('select')).toHaveLength(2);
    const style = selectAt(wrapper, 0);
    expect(style.findAll('option').map((o) => o.attributes('value'))).toEqual([
      '',
      'plain',
      'classical',
      'lightnovel',
      'hardboiled',
    ]);

    await style.setValue('hardboiled');
    await Promise.resolve();
    await Promise.resolve();
    expect(spy).toHaveBeenCalledWith({ literary_style_preset: 'hardboiled' });
    expect(store.literaryStylePreset).toBe('hardboiled');
    expect(store.literaryRecommendPreset).toBe(''); // 另一个控件不被波及
    expect(wrapper.text()).toContain('悬疑冷硬');
  });

  it('推荐档下拉:选项键对齐;选回空串=回到「不改变」(服务端按快照恢复原值)', async () => {
    const { store, wrapper } = mountSection();
    const spy = vi.spyOn(store, 'queueSettingsSave').mockResolvedValue(undefined);
    vi.spyOn(store, 'loadAgentFlow').mockResolvedValue(undefined);

    const rec = selectAt(wrapper, 1);
    expect(rec.findAll('option').map((o) => o.attributes('value'))).toEqual(['', 'medium', 'long']);

    await rec.setValue('long');
    await Promise.resolve();
    await Promise.resolve();
    expect(spy).toHaveBeenCalledWith({ literary_recommend_preset: 'long' });
    expect(store.literaryRecommendPreset).toBe('long');

    await rec.setValue('');
    await Promise.resolve();
    await Promise.resolve();
    expect(spy).toHaveBeenLastCalledWith({ literary_recommend_preset: '' });
    expect(store.literaryRecommendPreset).toBe('');
  });

  it('选择型控件保存失败时回滚本地值(不留下与服务端分叉的 UI 态)', async () => {
    const { store, wrapper } = mountSection();
    vi.spyOn(store, 'queueSettingsSave').mockRejectedValue(new Error('503 服务不可用'));
    vi.spyOn(store, 'loadAgentFlow').mockResolvedValue(undefined);

    const style = selectAt(wrapper, 0);
    await style.setValue('plain');
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(store.literaryStylePreset).toBe('');
    expect(wrapper.text()).toContain('保存失败');
  });
});
