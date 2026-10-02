// @vitest-environment jsdom
// VisionScreenshotSection 组件测试(移动端视觉能力包 A4):安卓无障碍截图块。
// 覆盖:非 Android 不渲染/不探测、Android 渲染状态与按钮、刷新强制重探、
// 跳系统设置调用、总开关保存走 queueSettingsSave。
//
// 为什么独立成文件:vi.mock('../../platform') 是**文件级**生效,而平台判定需按用例
// 切换(照 AndroidExecSection.test.ts 先例);SSR 冒烟断言另在 settingsSections.test.ts。
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

/** 状态响应工厂(`platform: 'android' as const` 收窄字面量,满足 ScreenStatus 形状) */
function screenStatus(accessibilityEnabled: boolean) {
  return {
    available: accessibilityEnabled,
    reason: accessibilityEnabled
      ? '无障碍截图服务已启用'
      : '无障碍截图服务未启用(系统设置 → 无障碍)',
    platform: 'android' as const,
    vision_screenshot_enabled: false,
    android: { accessibility_enabled: accessibilityEnabled },
  };
}

vi.mock('../../api/screen', () => ({
  getScreenStatus: vi.fn(),
  openAccessibilitySettings: vi.fn(),
}));

// 平台判定可按用例改写:默认非 Android
const platform = { isAndroidTauri: false };
vi.mock('../../platform', () => ({
  get isAndroidTauri() {
    return platform.isAndroidTauri;
  },
  inTauri: true,
  isAndroid: false,
}));

import * as screenApi from '../../api/screen';
import { useAppStore } from '../../store';
import VisionScreenshotSection from './VisionScreenshotSection.vue';

async function mountSection() {
  setActivePinia(createPinia());
  const store = useAppStore();
  const wrapper = mount(VisionScreenshotSection);
  // 冲刷 onMounted 里的异步状态读取(两次微任务,与 AndroidExecSection.test 同法)
  await Promise.resolve();
  await Promise.resolve();
  return { store, wrapper };
}

/** 按文案找按钮;找不到直接抛错,让失败信息可定位 */
function buttonByText(wrapper: ReturnType<typeof mount>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`未找到按钮:${text}`);
  return btn;
}

describe('VisionScreenshotSection 安卓块(移动端视觉能力包 A4)', () => {
  beforeEach(() => {
    memStorage.clear();
    platform.isAndroidTauri = false;
    vi.mocked(screenApi.getScreenStatus).mockReset();
    vi.mocked(screenApi.getScreenStatus).mockResolvedValue(screenStatus(false));
    vi.mocked(screenApi.openAccessibilitySettings).mockReset();
    vi.mocked(screenApi.openAccessibilitySettings).mockResolvedValue(undefined);
  });

  it('非 Android 平台不渲染安卓块、不请求状态', async () => {
    const { wrapper } = await mountSection();
    expect(wrapper.text()).toContain('允许截图工具取屏');
    expect(wrapper.text()).not.toContain('安卓无障碍截图');
    expect(vi.mocked(screenApi.getScreenStatus)).not.toHaveBeenCalled();
  });

  it('Android 首读走缓存(refresh=false),渲染未启用状态与开启入口', async () => {
    platform.isAndroidTauri = true;
    const { wrapper } = await mountSection();
    expect(vi.mocked(screenApi.getScreenStatus)).toHaveBeenCalledWith(false);
    expect(wrapper.text()).toContain('安卓无障碍截图');
    expect(wrapper.text()).toContain('未启用');
    expect(buttonByText(wrapper, '前往系统设置开启')).toBeTruthy();
  });

  it('已启用时显示启用态,且不渲染开启入口', async () => {
    platform.isAndroidTauri = true;
    vi.mocked(screenApi.getScreenStatus).mockResolvedValue(screenStatus(true));
    const { wrapper } = await mountSection();
    expect(wrapper.text()).toContain('已启用');
    const openBtn = wrapper.findAll('button').find((b) => b.text().trim() === '前往系统设置开启');
    expect(openBtn).toBeUndefined();
  });

  it('点「刷新」强制重探(refresh=true):否则刚在系统设置开启后一直看到缓存旧值', async () => {
    platform.isAndroidTauri = true;
    const { wrapper } = await mountSection();
    vi.mocked(screenApi.getScreenStatus).mockReset();
    vi.mocked(screenApi.getScreenStatus).mockResolvedValue(screenStatus(true));

    await buttonByText(wrapper, '刷新').trigger('click');
    await Promise.resolve();
    await Promise.resolve();

    expect(vi.mocked(screenApi.getScreenStatus)).toHaveBeenCalledWith(true);
    expect(wrapper.text()).toContain('已启用');
  });

  it('点「前往系统设置开启」调用跳转端点并给出回程指引', async () => {
    platform.isAndroidTauri = true;
    const { wrapper } = await mountSection();
    await buttonByText(wrapper, '前往系统设置开启').trigger('click');
    await Promise.resolve();
    expect(vi.mocked(screenApi.openAccessibilitySettings)).toHaveBeenCalled();
    expect(wrapper.text()).toContain('已跳转系统设置');
  });

  it('切换总开关走 queueSettingsSave(vision_screenshot_enabled)', async () => {
    const { store, wrapper } = await mountSection();
    const spy = vi.spyOn(store, 'queueSettingsSave').mockResolvedValue(undefined);
    const box = wrapper.find('input[type="checkbox"]');
    await box.setValue(true);
    await Promise.resolve();
    expect(spy).toHaveBeenCalledWith({ vision_screenshot_enabled: true });
  });
});
