// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import DataLocationChooser from './DataLocationChooser.vue';
import type { DataLocationStatus } from '../api';

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

describe('DataLocationChooser(盘符选择器)', () => {
  it('列出全部盘符,默认选中后端的 default_drive(D 优先)', () => {
    const wrapper = mount(DataLocationChooser, { props: { status } });
    expect(wrapper.text()).toContain('C:');
    expect(wrapper.text()).toContain('D:');
    expect(wrapper.text()).toContain('D:\\Kedai-Data');
    const active = wrapper.findAll('button').filter((b) => b.classes().includes('active'));
    expect(active).toHaveLength(1);
    expect(active[0].text()).toContain('D:');
  });

  it('选择其他盘符后点确认,emit 的是所选盘符', async () => {
    const wrapper = mount(DataLocationChooser, { props: { status } });
    const c = wrapper.findAll('button').find((b) => b.text().includes('C:'));
    expect(c).toBeTruthy();
    await c?.trigger('click');
    const apply = wrapper.findAll('button').find((b) => b.text().includes('使用该位置'));
    expect(apply).toBeTruthy();
    await apply?.trigger('click');
    expect(wrapper.emitted('choose')).toEqual([['C']]);
  });

  it('无盘符时给出明确错误且确认按钮禁用', () => {
    const wrapper = mount(DataLocationChooser, {
      props: { status: { ...status, drives: [], default_drive: null } },
    });
    expect(wrapper.text()).toContain('未检测到可用盘符');
    const apply = wrapper.findAll('button').find((b) => b.text().includes('使用该位置'));
    expect(apply?.attributes('disabled')).toBeDefined();
  });

  it('Android:默认路径模式,缺权限时显示授权入口,确认 emit 绝对路径', async () => {
    const wrapper = mount(DataLocationChooser, {
      props: {
        status: {
          ...status,
          platform: 'android',
          suggested_root: '/storage/emulated/0/Download/Kedai-Data',
          needs_permission: true,
          drives: [],
          default_drive: null,
        },
      },
    });
    // 不再展示盘符错误,而是默认路径 + 权限说明
    expect(wrapper.text()).not.toContain('未检测到可用盘符');
    expect(wrapper.text()).toContain('去授权所有文件访问');
    const input = wrapper.find('input.sv-input');
    expect((input.element as HTMLInputElement).value).toBe('/storage/emulated/0/Download/Kedai-Data');
    const apply = wrapper.findAll('button').find((b) => b.text().includes('使用该位置'));
    await apply?.trigger('click');
    expect(wrapper.emitted('choose')).toEqual([['/storage/emulated/0/Download/Kedai-Data']]);
  });

  it('Android:授权后可点「重新检测」请求父级刷新', async () => {
    const wrapper = mount(DataLocationChooser, {
      props: {
        status: {
          ...status,
          platform: 'android',
          suggested_root: '/storage/emulated/0/Download/Kedai-Data',
          needs_permission: false,
          drives: [],
          default_drive: null,
        },
      },
    });
    const refresh = wrapper.findAll('button').find((b) => b.text().includes('重新检测'));
    await refresh?.trigger('click');
    expect(wrapper.emitted('refresh')).toHaveLength(1);
  });
});
