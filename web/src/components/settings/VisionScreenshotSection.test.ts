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

// CU-1:电脑操作治理(急停 + 审计)。onMounted 会读状态与审计列表,故整个模块必须 mock
// (未 mock 会发起真实请求,与「mock 契约对齐」纪律相悖)。
vi.mock('../../api/computerUse', () => ({
  getCuStatus: vi.fn(),
  listCuAudit: vi.fn(),
  stopComputerUse: vi.fn(),
  resumeComputerUse: vi.fn(),
  clearCuAudit: vi.fn(),
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
import * as cuApi from '../../api/computerUse';
import { useAppStore } from '../../store';
import VisionScreenshotSection from './VisionScreenshotSection.vue';

async function mountSection() {
  setActivePinia(createPinia());
  const store = useAppStore();
  const wrapper = mount(VisionScreenshotSection);
  // 冲刷 onMounted 里的异步状态读取(微任务多轮:screen 状态 + CU 状态 + 审计列表)
  for (let i = 0; i < 6; i++) await Promise.resolve();
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

describe('VisionScreenshotSection 电脑操作急停与审计(CU-1)', () => {
  beforeEach(() => {
    memStorage.clear();
    platform.isAndroidTauri = false;
    vi.mocked(cuApi.getCuStatus).mockReset();
    vi.mocked(cuApi.getCuStatus).mockResolvedValue({ stopped: false });
    vi.mocked(cuApi.listCuAudit).mockReset();
    vi.mocked(cuApi.listCuAudit).mockResolvedValue([]);
    vi.mocked(cuApi.stopComputerUse).mockReset();
    vi.mocked(cuApi.stopComputerUse).mockResolvedValue({ stopped: true });
    vi.mocked(cuApi.resumeComputerUse).mockReset();
    vi.mocked(cuApi.resumeComputerUse).mockResolvedValue({ stopped: false });
    vi.mocked(cuApi.clearCuAudit).mockReset();
    vi.mocked(cuApi.clearCuAudit).mockResolvedValue(1);
  });

  it('挂载即读状态与审计;未停止时展示「停止操作电脑」入口', async () => {
    const { wrapper } = await mountSection();
    expect(vi.mocked(cuApi.getCuStatus)).toHaveBeenCalled();
    expect(vi.mocked(cuApi.listCuAudit)).toHaveBeenCalledWith(100);
    expect(wrapper.text()).toContain('操作审计与急停');
    expect(wrapper.text()).toContain('未停止');
    expect(buttonByText(wrapper, '停止操作电脑')).toBeTruthy();
    expect(wrapper.text()).toContain('暂无电脑操作记录');
  });

  it('点「停止操作电脑」调 stop 端点,标签切「恢复操作电脑」', async () => {
    const { wrapper } = await mountSection();
    await buttonByText(wrapper, '停止操作电脑').trigger('click');
    for (let i = 0; i < 3; i++) await Promise.resolve();
    expect(vi.mocked(cuApi.stopComputerUse)).toHaveBeenCalled();
    expect(wrapper.text()).toContain('已停止');
    expect(buttonByText(wrapper, '恢复操作电脑')).toBeTruthy();
    // 恢复路径同样打到 resume 端点
    await buttonByText(wrapper, '恢复操作电脑').trigger('click');
    for (let i = 0; i < 3; i++) await Promise.resolve();
    expect(vi.mocked(cuApi.resumeComputerUse)).toHaveBeenCalled();
    expect(wrapper.text()).toContain('未停止');
  });

  it('审计行渲染:错误码转中文标签、被拒行带 denied、引用行只显示哈希摘要', async () => {
    vi.mocked(cuApi.listCuAudit).mockResolvedValue([
      {
        id: 2, ts: '2026-10-06T12:00:00Z', source: 'task', task_id: 't1', session_id: null,
        platform: 'windows', action: 'read_screen', target: '窗口「记事本」',
        decision: 'denied', result: 'refused', error_code: 'CONTROL_STOPPED',
        image_ref: '', png_bytes: null, sha256: '',
      },
      {
        id: 1, ts: '2026-10-06T11:59:00Z', source: 'chat', task_id: null, session_id: 's1',
        platform: 'windows', action: 'read_screen', target: '全屏',
        decision: 'allowed', result: 'ok', error_code: '',
        image_ref: 'abc-123.png', png_bytes: 4096, sha256: 'deadbeefcafe1234',
      },
    ]);
    const { wrapper } = await mountSection();
    const rows = wrapper.findAll('.sv-cu-audit-row');
    expect(rows.length).toBe(2);
    expect(wrapper.text()).toContain('急停拒绝');
    expect(wrapper.text()).toContain('已截图');
    expect(wrapper.text()).toContain('窗口「记事本」');
    expect(wrapper.findAll('.sv-cu-audit-row.denied').length).toBe(1);
    // 图像引用行:显示引用名 + 字节数 + 哈希前 12 位;完整哈希不进 DOM(减噪)
    expect(wrapper.text()).toContain('abc-123.png');
    expect(wrapper.text()).toContain('4096 字节');
    expect(wrapper.text()).toContain('deadbeefcafe');
    expect(wrapper.text()).not.toContain('deadbeefcafe1234');
  });

  it('清空审计仅删行:调 clear 端点并清空列表', async () => {
    vi.mocked(cuApi.listCuAudit).mockResolvedValue([
      {
        id: 1, ts: '2026-10-06T11:59:00Z', source: 'chat', task_id: null, session_id: 's1',
        platform: 'windows', action: 'read_screen', target: '全屏',
        decision: 'allowed', result: 'ok', error_code: '',
        image_ref: '', png_bytes: null, sha256: '',
      },
    ]);
    const { wrapper } = await mountSection();
    expect(wrapper.findAll('.sv-cu-audit-row').length).toBe(1);
    await buttonByText(wrapper, '清空').trigger('click');
    for (let i = 0; i < 3; i++) await Promise.resolve();
    expect(vi.mocked(cuApi.clearCuAudit)).toHaveBeenCalled();
    expect(wrapper.findAll('.sv-cu-audit-row').length).toBe(0);
    expect(wrapper.text()).toContain('已清空 1 条记录');
  });

  it('急停状态读取失败时给出反馈且不清空按钮(辅助入口不得静默)', async () => {
    vi.mocked(cuApi.getCuStatus).mockRejectedValue(new Error('boom'));
    const { wrapper } = await mountSection();
    expect(wrapper.text()).toContain('读取失败');
    expect(buttonByText(wrapper, '停止操作电脑')).toBeTruthy();
  });
});
