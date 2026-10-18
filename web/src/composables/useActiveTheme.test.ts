// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { defineComponent, nextTick } from 'vue';
import { mount } from '@vue/test-utils';

const registry = vi.hoisted(() => {
  let listener: ((snapshot: { activeId: string }) => void) | undefined;
  let unsubscribeCalls = 0;
  return {
    getActiveThemeId: vi.fn(() => 'default'),
    subscribeThemes: vi.fn((fn: (snapshot: { activeId: string }) => void) => {
      listener = fn;
      fn({ activeId: 'default' });
      return () => {
        unsubscribeCalls += 1;
        listener = undefined;
      };
    }),
    emit: (id: string) => listener?.({ activeId: id }),
    unsubscribeCalls: () => unsubscribeCalls,
  };
});

vi.mock('../theme/registry', () => ({
  getActiveThemeId: registry.getActiveThemeId,
  subscribeThemes: registry.subscribeThemes,
}));

import { useActiveThemeId } from './useActiveTheme';

afterEach(() => {
  vi.clearAllMocks();
});

describe('useActiveThemeId', () => {
  it('挂载时订阅主题快照,主题变化时更新,卸载时退订', async () => {
    const Probe = defineComponent({
      setup() {
        const activeId = useActiveThemeId();
        return { activeId };
      },
      template: '<div>{{ activeId }}</div>',
    });

    const wrapper = mount(Probe);
    expect(registry.subscribeThemes).toHaveBeenCalledTimes(1);
    expect(wrapper.text()).toBe('default');

    registry.emit('stained-glass');
    await nextTick();
    expect(wrapper.text()).toBe('stained-glass');

    wrapper.unmount();
    expect(registry.unsubscribeCalls()).toBe(1);
  });
});
