// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils';
import { defineComponent, ref } from 'vue';
import { useDialogFocus } from './useDialogFocus';

enableAutoUnmount(afterEach);
afterEach(() => { document.body.innerHTML = ''; });

function makeHost(template: string, close: () => void = () => {}) {
  return defineComponent({
    setup() { const root = ref<HTMLElement | null>(null); useDialogFocus(root, close); return { root }; },
    template,
  });
}

describe('弹窗焦点边界', () => {
  it('没有可聚焦控件时落在弹窗本身，隐藏控件不参与循环', async () => {
    const Host = makeHost('<div class="sv-modal-mask"><div ref="root" role="dialog" tabindex="-1"><button hidden>隐藏</button><button disabled>禁用</button><input type="hidden"></div></div>');
    const wrapper = mount(Host, { attachTo: document.body }); await flushPromises();
    const root = wrapper.get('[role="dialog"]').element;
    expect(document.activeElement).toBe(root);
    const tab = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true });
    document.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(root);
  });

  it('更上层弹窗打开后，不抢焦点、不消费它的 Esc，卸载后移除监听', async () => {
    const close = vi.fn();
    const Host = makeHost('<div class="sv-modal-mask"><div ref="root" tabindex="-1"><button>设置</button></div></div>', close);
    const wrapper = mount(Host, { attachTo: document.body }); await flushPromises();
    const upper = document.createElement('div'); upper.className = 'sv-modal-mask';
    const button = document.createElement('button'); upper.append(button); document.body.append(upper); button.focus();
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect(document.activeElement).toBe(button);
    expect(close).not.toHaveBeenCalled();
    wrapper.unmount();
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect(close).not.toHaveBeenCalled();
  });
});
