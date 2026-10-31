// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SplitDataDirsHint from './SplitDataDirsHint.vue';

// 双库分叉提示条(DB-7)的展示判据:
//   ① 当前目录与**每一条**另一套库的路径都要出现——路径是可操作信息,少一条就少一个可选;
//   ② 关闭按钮把 dismiss 事件抛给父组件(组件自己不持有状态);
//   ③ 判据说明有则显示、无则不留下空括号。
// 用 jsdom + mount(而非 SSR 字符串):这里要断言「点击后 emit」,SSR 做不到。

const notice = {
  dataDir: 'C:\\Users\\u\\AppData\\Roaming\\com.kedai.app\\data',
  others: [
    { path: 'D:\\kedai\\data', evidence: 'settings.json + 数据库' },
    { path: 'E:\\backup\\data', evidence: '' },
  ],
};

describe('SplitDataDirsHint(双库分叉提示)', () => {
  it('列出当前目录与每一条另一套库的路径', () => {
    const wrapper = mount(SplitDataDirsHint, { props: { notice } });
    const text = wrapper.text();
    expect(text).toContain('检测到另一套数据目录也有用户数据');
    expect(text).toContain(notice.dataDir);
    for (const other of notice.others) expect(text, other.path).toContain(other.path);
    // 行动指引必须出现:只说「有两套库」而不说怎么办,等于把问题丢回给用户
    expect(text).toContain('两套库互不可见');
    expect(text).toContain('DATA_DIR');
  });

  it('有判据时括注出来,空判据不留空括号', () => {
    const wrapper = mount(SplitDataDirsHint, { props: { notice } });
    expect(wrapper.text()).toContain('(settings.json + 数据库)');
    expect(wrapper.text()).not.toContain('()');
  });

  it('role=status(非 alert):分叉是提示,不是故障播报', () => {
    const wrapper = mount(SplitDataDirsHint, { props: { notice } });
    expect(wrapper.attributes('role')).toBe('status');
  });

  it('点击关闭按钮抛出 dismiss(状态由父组件/store 持有)', async () => {
    const wrapper = mount(SplitDataDirsHint, { props: { notice } });
    await wrapper.get('button').trigger('click');
    expect(wrapper.emitted('dismiss')).toHaveLength(1);
  });
});
