// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import DataLocationSection from './DataLocationSection.vue';
import * as api from '../../api';
import { restartApp } from '../../platform';

vi.mock('../../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api')>();
  return {
    ...actual,
    getDataLocation: vi.fn(),
    selectDataLocation: vi.fn(),
  };
});
vi.mock('../../platform', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../platform')>();
  return { ...actual, restartApp: vi.fn(async () => true) };
});

const status = {
  managed: true,
  pending: false,
  root: 'D:\\Kedai-Data',
  data_dir: 'D:\\Kedai-Data\\data',
  images_dir: 'D:\\Kedai-Data\\outputs\\images',
  task_records_dir: 'D:\\Kedai-Data\\task-records',
  imports_dir: 'D:\\Kedai-Data\\imports',
  default_drive: 'D',
  drives: [{ letter: 'C' }, { letter: 'D' }],
  legacy_data: true,
  pointer: 'C:\\Users\\u\\AppData\\Roaming\\com.kedai.app\\data-location.json',
  error: null,
};

beforeEach(() => {
  vi.mocked(api.getDataLocation).mockReset();
  vi.mocked(api.selectDataLocation).mockReset();
  vi.mocked(restartApp).mockClear();
  vi.spyOn(window, 'confirm').mockReturnValue(true);
});

describe('DataLocationSection(设置 · 数据位置)', () => {
  it('展示当前根与四个子目录', async () => {
    vi.mocked(api.getDataLocation).mockResolvedValue(status);
    const wrapper = mount(DataLocationSection, { props: { show: true } });
    await flushPromises();
    expect(wrapper.text()).toContain('D:\\Kedai-Data');
    expect(wrapper.text()).toContain('D:\\Kedai-Data\\data');
    expect(wrapper.text()).toContain('outputs\\images');
    expect(wrapper.text()).toContain('task-records');
    expect(wrapper.text()).toContain('imports');
  });

  it('更改位置 → 确认 → 调 API → 请求重启', async () => {
    vi.mocked(api.getDataLocation).mockResolvedValue(status);
    vi.mocked(api.selectDataLocation).mockResolvedValue({
      root: 'C:\\Kedai-Data',
      migrated: true,
      restart_required: true,
    });
    const wrapper = mount(DataLocationSection, { props: { show: true } });
    await flushPromises();
    const change = wrapper.findAll('button').find((b) => b.text().includes('更改位置'));
    await change?.trigger('click');
    const c = wrapper.findAll('button').find((b) => b.text().includes('C:'));
    await c?.trigger('click');
    const apply = wrapper.findAll('button').find((b) => b.text().includes('使用该位置'));
    await apply?.trigger('click');
    await flushPromises();
    expect(window.confirm).toHaveBeenCalled();
    expect(api.selectDataLocation).toHaveBeenCalledWith('C');
    expect(restartApp).toHaveBeenCalledTimes(1);
  });
});
