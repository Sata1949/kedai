// @vitest-environment jsdom
// C-2A：ToolIoPane 从 AgentPanel 6 段重复模板中抽出后的契约测试。
import { describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import ToolIoPane from './ToolIoPane.vue';

describe('ToolIoPane', () => {
  it('展示 JSON 文本、标签与展开态', () => {
    const wrapper = mount(ToolIoPane, {
      props: { label: '入参', value: { a: 1, b: '二' }, open: false, copied: false },
    });
    expect(wrapper.find('.sv-tool-io-toggle').text()).toBe('入参');
    expect(wrapper.find('.sv-tool-io-body').text()).toContain('"a": 1');
    expect(wrapper.classes()).not.toContain('open');
    expect(wrapper.find('.sv-tool-io-copy').text()).toBe('复制');
  });

  it('open/copied prop 控制类名与回执文案', () => {
    const wrapper = mount(ToolIoPane, {
      props: { label: '出参', value: 'x', open: true, copied: true },
    });
    expect(wrapper.classes()).toContain('open');
    expect(wrapper.find('.sv-tool-io-copy').text()).toBe('已复制');
  });

  it('折叠与复制只上抛事件,复制载荷为已序列化文本', async () => {
    const wrapper = mount(ToolIoPane, {
      props: { label: '入参', value: { n: 2 }, open: false, copied: false },
    });
    await wrapper.find('.sv-tool-io-toggle').trigger('click');
    await wrapper.find('.sv-tool-io-copy').trigger('click');
    expect(wrapper.emitted('toggle')).toHaveLength(1);
    expect(wrapper.emitted('copy')).toEqual([['{\n  "n": 2\n}']]);
  });

  it('滚动到底时移除 has-scroll,内容溢出则加回', async () => {
    // jsdom 无布局引擎:直接定义 scrollHeight/clientHeight/scrollTop 驱动判定
    const wrapper = mount(ToolIoPane, {
      props: { label: '入参', value: { big: 'x' }, open: true, copied: false },
    });
    const body = wrapper.find('.sv-tool-io-body').element as HTMLElement;
    Object.defineProperty(body, 'scrollHeight', { configurable: true, value: 200 });
    Object.defineProperty(body, 'clientHeight', { configurable: true, value: 100 });
    Object.defineProperty(body, 'scrollTop', { configurable: true, value: 50 });
    await wrapper.find('.sv-tool-io-body').trigger('scroll');
    expect(wrapper.classes()).toContain('has-scroll');
    Object.defineProperty(body, 'scrollTop', { configurable: true, value: 100 });
    await wrapper.find('.sv-tool-io-body').trigger('scroll');
    expect(wrapper.classes()).not.toContain('has-scroll');
  });

  it('卸载时取消尚未执行的 rAF(不抛错)', () => {
    const raf = vi.spyOn(window, 'requestAnimationFrame').mockImplementation(() => 1);
    const wrapper = mount(ToolIoPane, {
      props: { label: '入参', value: 1, open: false, copied: false },
    });
    expect(raf).toHaveBeenCalled();
    wrapper.unmount();
    raf.mockRestore();
  });
});
