// @vitest-environment jsdom
// 主题注册表 / 插件 API 的最小契约测试:
//   - 内置主题与插件主题同一注册表,id 唯一、非法定义拒绝;
//   - apply 同步写 data-theme / 令牌 / class 并持久化,切换时清理旧令牌;
//   - 插件 CSS/字体声明注入 <head>,卸载时移除;
//   - 订阅者在注册/切换/卸载时收到快照;
//   - 彩窗主题的令牌仍满足小字 WCAG AA(≥4.5:1)。
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  DEFAULT_THEME_ID,
  THEME_API_VERSION,
  THEME_STORAGE_KEY,
  createThemeRegistry,
  installThemeApi,
  announceUiReady,
} from './registry';
import { BUILTIN_THEMES, DEFAULT_THEME, STAINED_GLASS_THEME } from './builtin';

interface MemoryStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function memoryStorage(initial: Record<string, string> = {}): MemoryStorage {
  const map = new Map<string, string>(Object.entries(initial));
  return {
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
  };
}

/** 每个用例用独立 detached document,避免污染 jsdom 主文档。 */
function testDoc(): Document {
  return document.implementation.createHTMLDocument('theme-test');
}

const linear = (c: number): number => {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
};

function luminance(hex: string): number {
  const h = hex.replace('#', '');
  return (
    0.2126 * linear(parseInt(h.slice(0, 2), 16)) +
    0.7152 * linear(parseInt(h.slice(2, 4), 16)) +
    0.0722 * linear(parseInt(h.slice(4, 6), 16))
  );
}

function contrast(a: string, b: string): number {
  const l1 = Math.max(luminance(a), luminance(b));
  const l2 = Math.min(luminance(a), luminance(b));
  return (l1 + 0.05) / (l2 + 0.05);
}

afterEach(() => {
  delete window.KedaiUI;
});

describe('主题注册表', () => {
  it('内置主题包含默认纸面与教堂拼接玻璃', () => {
    expect(BUILTIN_THEMES.map((t) => t.id)).toEqual([DEFAULT_THEME_ID, 'stained-glass']);
    expect(DEFAULT_THEME.name).toContain('纸面');
    expect(STAINED_GLASS_THEME.name).toBe('教堂拼接玻璃');
    expect(typeof STAINED_GLASS_THEME.load).toBe('function');
  });

  it('应用主题:写 data-theme / 令牌 / class 并持久化,切换时清理旧值', async () => {
    const doc = testDoc();
    const root = doc.documentElement;
    const storage = memoryStorage();
    const reg = createThemeRegistry({
      storage,
      root,
      document: doc,
      builtinThemes: [
        { id: 'default', name: '默认' },
        { id: 'theme-a', name: 'A', tokens: { '--x': '1' }, className: 'is-a' },
        { id: 'theme-b', name: 'B', tokens: { '--y': '2' } },
      ],
    });

    await reg.apply('theme-a');
    expect(root.dataset.theme).toBe('theme-a');
    expect(root.style.getPropertyValue('--x')).toBe('1');
    expect(root.classList.contains('is-a')).toBe(true);
    expect(storage.getItem(THEME_STORAGE_KEY)).toBe('theme-a');

    await reg.apply('theme-b');
    expect(root.dataset.theme).toBe('theme-b');
    expect(root.style.getPropertyValue('--x')).toBe('');
    expect(root.style.getPropertyValue('--y')).toBe('2');
    expect(root.classList.contains('is-a')).toBe(false);
    expect(storage.getItem(THEME_STORAGE_KEY)).toBe('theme-b');
  });

  it('拒绝重复 id、非法 id 与非 --* 令牌', () => {
    const reg = createThemeRegistry({
      storage: memoryStorage(),
      document: testDoc(),
      builtinThemes: [{ id: 'default', name: '默认' }],
    });
    expect(reg.register({ id: 'default', name: '重复' })).toBe(false);
    expect(reg.register({ id: 'Bad ID', name: '非法' })).toBe(false);
    expect(reg.register({ id: 'bad-token', name: '坏令牌', tokens: { color: 'red' } })).toBe(false);
  });

  it('插件主题的 CSS/字体声明注入 head,卸载时移除并回退默认', async () => {
    const doc = testDoc();
    const reg = createThemeRegistry({
      storage: memoryStorage(),
      document: doc,
      builtinThemes: [{ id: 'default', name: '默认' }],
    });
    expect(
      reg.register({
        id: 'plugin-theme',
        name: '插件主题',
        className: 'plugin-theme-class',
        css: '[data-theme="plugin-theme"] .x{color:red}',
        fonts: [{ family: 'PluginFont', src: 'url("/plugin.woff2") format("woff2")' }],
      }),
    ).toBe(true);

    expect(doc.querySelector('style[data-kedai-theme-css="plugin-theme"]')).toBeTruthy();
    expect(doc.querySelector('style[data-kedai-theme-fonts="plugin-theme"]')).toBeTruthy();

    await reg.apply('plugin-theme');
    expect(doc.documentElement.dataset.theme).toBe('plugin-theme');
    expect(reg.unregister('plugin-theme')).toBe(true);
    expect(doc.querySelector('style[data-kedai-theme-css="plugin-theme"]')).toBeNull();
    expect(doc.documentElement.dataset.theme).toBe('default');
    expect(reg.getActive()).toBe('default');
    // 内置主题受保护,不能卸载
    expect(reg.unregister('default')).toBe(false);
  });

  it('订阅者在注册/切换时收到快照;init 读取已保存主题', async () => {
    const doc = testDoc();
    const storage = memoryStorage({ [THEME_STORAGE_KEY]: 'theme-b' });
    const reg = createThemeRegistry({
      storage,
      document: doc,
      builtinThemes: [
        { id: 'default', name: '默认' },
        { id: 'theme-b', name: 'B' },
      ],
    });
    const seen: string[] = [];
    const off = reg.subscribe((snap) => seen.push(snap.activeId));
    reg.init();
    await Promise.resolve();
    expect(reg.getActive()).toBe('theme-b');
    expect(seen).toContain('theme-b');
    off();
    await reg.apply('default');
    expect(seen[seen.length - 1]).toBe('theme-b');
  });

  it('载入资源只调用一次,失败不阻断主题应用', async () => {
    const doc = testDoc();
    const load = vi.fn().mockRejectedValue(new Error('asset down'));
    const reg = createThemeRegistry({
      storage: memoryStorage(),
      document: doc,
      builtinThemes: [{ id: 'default', name: '默认' }, { id: 'lazy', name: 'Lazy', load }],
    });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await expect(reg.apply('lazy')).resolves.toBe(true);
    await reg.apply('default');
    await reg.apply('lazy');
    expect(load).toHaveBeenCalledTimes(1);
    expect(doc.documentElement.dataset.theme).toBe('lazy');
    warn.mockRestore();
  });
});

describe('教堂拼接玻璃主题的令牌对比度(WCAG AA ≥ 4.5:1)', () => {
  const tokens = STAINED_GLASS_THEME.tokens ?? {};

  it.each([
    ['--sv-ink-faint', '--sv-paper'],
    ['--sv-ink-faint', '--sv-surface'],
    ['--sv-ink-faint', '--sv-pink-light'],
    ['--sv-ink', '--sv-paper'],
    ['--sv-white', '--sv-blue'],
    ['--sv-white', '--sv-green'],
    ['--sv-white', '--sv-red'],
    ['--sv-ink', '--sv-yellow'],
  ])('%s on %s', (fg, bg) => {
    const ratio = contrast(tokens[fg], tokens[bg]);
    expect(ratio, `${fg}(${tokens[fg]}) on ${bg}(${tokens[bg]}) = ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
  });
});

describe('window.KedaiUI 插件接口', () => {
  it('暴露版本号/注册/切换/订阅函数,并在挂载后广播 ui-ready', () => {
    const api = installThemeApi();
    expect(api?.version).toBe(THEME_API_VERSION);
    expect(window.KedaiUI).toBe(api);
    expect(typeof window.KedaiUI?.registerTheme).toBe('function');
    expect(typeof window.KedaiUI?.applyTheme).toBe('function');
    expect(typeof window.KedaiUI?.onChange).toBe('function');

    const listener = vi.fn();
    window.addEventListener('kedai:ui-ready', listener);
    announceUiReady();
    expect(listener).toHaveBeenCalledTimes(1);
    window.removeEventListener('kedai:ui-ready', listener);
  });
});
