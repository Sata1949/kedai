// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import DataLocationGate from './DataLocationGate.vue';
import * as api from '../api';
import { restartApp } from '../platform';
import { useAppStore } from '../store';
import type { DataLocationStatus } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    getDataLocation: vi.fn(),
    selectDataLocation: vi.fn(),
  };
});
vi.mock('../platform', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform')>();
  return { ...actual, restartApp: vi.fn(async () => true) };
});

const status: DataLocationStatus = {
  managed: true,
  pending: true,
  root: null,
  data_dir: 'C:\\old\\data',
  images_dir: 'C:\\old\\data\\images',
  task_records_dir: 'C:\\old\\task_scratch',
  imports_dir: 'C:\\old\\data\\imports',
  default_drive: 'D',
  drives: [{ letter: 'C' }, { letter: 'D' }],
  legacy_data: true,
  pointer: null,
  error: null,
};

beforeEach(() => {
  setActivePinia(createPinia());
  vi.mocked(api.getDataLocation).mockReset();
  vi.mocked(api.selectDataLocation).mockReset();
  vi.mocked(restartApp).mockClear();
});

describe('DataLocationGate(老用户强制弹窗)', () => {
  it('pending 且教程未打开时显示,不可关闭', async () => {
    vi.mocked(api.getDataLocation).mockResolvedValue(status);
    const wrapper = mount(DataLocationGate);
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    expect(wrapper.text()).toContain('选择数据保存位置');
    // 没有关闭按钮(弹窗头只有标题)
    expect(wrapper.findAll('button').some((b) => b.text().includes('✕'))).toBe(false);
  });

  it('教程打开时让位,教程关闭后才显示', async () => {
    vi.mocked(api.getDataLocation).mockResolvedValue(status);
    const store = useAppStore();
    store.onboardingOpen = true;
    const wrapper = mount(DataLocationGate);
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    store.onboardingOpen = false;
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
  });

  it('pending=false 的已选择用户不弹窗', async () => {
    vi.mocked(api.getDataLocation).mockResolvedValue({
      ...status,
      pending: false,
      root: 'D:\\Kedai-Data',
    });
    const wrapper = mount(DataLocationGate);
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
  });

  it('Android:默认路径 → 调 API 时发送 root(不是盘符)', async () => {
    vi.mocked(api.getDataLocation).mockResolvedValue({
      ...status,
      platform: 'android',
      suggested_root: '/storage/emulated/0/Download/Kedai-Data',
      needs_permission: false,
      drives: [],
      default_drive: null,
    });
    vi.mocked(api.selectDataLocation).mockResolvedValue({
      root: '/storage/emulated/0/Download/Kedai-Data',
      migrated: true,
      restart_required: true,
    });
    const wrapper = mount(DataLocationGate);
    await flushPromises();
    const apply = wrapper.findAll('button').find((b) => b.text().includes('使用该位置'));
    await apply?.trigger('click');
    await flushPromises();
    expect(api.selectDataLocation).toHaveBeenCalledWith({
      root: '/storage/emulated/0/Download/Kedai-Data',
    });
    expect(restartApp).toHaveBeenCalledTimes(1);
  });

  it('选择盘符 → 调 API → 请求重启', async () => {
    vi.mocked(api.getDataLocation).mockResolvedValue(status);
    vi.mocked(api.selectDataLocation).mockResolvedValue({
      root: 'D:\\Kedai-Data',
      migrated: true,
      restart_required: true,
    });
    const wrapper = mount(DataLocationGate);
    await flushPromises();
    const d = wrapper.findAll('button').find((b) => b.text().includes('D:'));
    await d?.trigger('click');
    const apply = wrapper.findAll('button').find((b) => b.text().includes('使用该位置'));
    await apply?.trigger('click');
    await flushPromises();
    expect(api.selectDataLocation).toHaveBeenCalledWith('D');
    expect(restartApp).toHaveBeenCalledTimes(1);
  });
});
