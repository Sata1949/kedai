// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

// 主题资源 Promise 由用例控制:验证 splash 不会在主题 CSS 就绪前揭幕。
const themeReady = vi.hoisted(() => {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => { resolve = r; });
  return { promise, resolve };
});
vi.mock('../theme/registry', () => ({
  whenThemeReady: () => themeReady.promise,
}));

import SplashScreen from './SplashScreen.vue';
import { useAppStore } from '../store';

const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

beforeEach(() => {
  vi.useFakeTimers();
  memStorage.clear();
  setActivePinia(createPinia());
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('SplashScreen 启动揭幕时序', () => {
  it('至少展示 1.2s;主题 CSS 未就绪时不淡出,就绪后 0.4s 收尾', async () => {
    const wrapper = mount(SplashScreen);
    const store = useAppStore();

    await vi.advanceTimersByTimeAsync(1199);
    expect(wrapper.find('.sv-splash').classes()).not.toContain('fade-out');
    expect(store.splashDone).toBe(false);

    // 最短展示时间到了,但主题 CSS 还没就绪:仍不能揭幕
    await vi.advanceTimersByTimeAsync(1);
    await flushPromises();
    expect(wrapper.find('.sv-splash').classes()).not.toContain('fade-out');
    expect(store.splashDone).toBe(false);

    themeReady.resolve();
    await flushPromises();
    expect(wrapper.find('.sv-splash').classes()).toContain('fade-out');
    expect(store.splashDone).toBe(false);

    await vi.advanceTimersByTimeAsync(400);
    expect(store.splashDone).toBe(true);
    wrapper.unmount();
  });
});
