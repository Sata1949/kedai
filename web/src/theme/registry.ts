// 前端主题注册表 + 插件扩展点。
//
// 设计边界(2026-10-17 主题插件批):
//   - 主题是**声明式**的:只允许覆盖 CSS 自定义属性(--*)、根 class、@font-face 与
//     已由插件自行作用域化的 CSS 文本;宿主不向插件暴露 DOM 写入口,也不执行插件回调。
//   - 内置主题与外部插件主题走同一注册表:官方主题只是第一个内置消费者。
//   - 主题 id 写入 localStorage(前端偏好,不落 settings.json),跨重启保留;
//     插件主题若在启动时尚未注册,回退默认主题,注册后可再次切换。
//   - 外部插件通过 window.KedaiUI 注册/切换主题;window 上同时派发一次
//     `kedai:ui-ready` 事件,方便插件在宿主就绪后注册。
import { BUILTIN_THEMES } from './builtin';

export interface ThemeFontFace {
  family: string;
  /** 完整 src 值,如 `url("/fonts/x.woff2") format("woff2")` */
  src: string;
  weight?: string;
  style?: string;
  display?: string;
}

export interface ThemeDefinition {
  id: string;
  name: string;
  description?: string;
  /** CSS 自定义属性覆盖;键必须匹配 --* */
  tokens?: Record<string, string>;
  /** 应用到 <html> 的单个 class(可选) */
  className?: string;
  /** 插件自带的 @font-face 声明(可选) */
  fonts?: ThemeFontFace[];
  /** 插件作用域 CSS;请自行以 [data-theme="<id>"] 开头,避免污染其它主题 */
  css?: string;
  /** 按需加载主题资源(CSS/字体 chunk);只在首次激活时调用一次 */
  load?: () => Promise<unknown>;
}

export interface ThemeSummary {
  id: string;
  name: string;
  description: string;
  builtin: boolean;
}

export interface ThemeSnapshot {
  themes: ThemeSummary[];
  activeId: string;
}

export type ThemeListener = (snapshot: ThemeSnapshot) => void;

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem?(key: string): void;
}

export interface ThemeRegistryOptions {
  storage?: StorageLike;
  root?: HTMLElement;
  document?: Document;
  /** 测试可传入自定义内置主题;生产默认 BUILTIN_THEMES */
  builtinThemes?: ThemeDefinition[];
}

export const THEME_STORAGE_KEY = 'kedai.theme.v1';
export const DEFAULT_THEME_ID = 'default';
export const THEME_API_VERSION = 1;

const THEME_ID_RE = /^[a-z][a-z0-9-]{1,63}$/;
const TOKEN_RE = /^--[a-zA-Z0-9-]+$/;
const CLASS_RE = /^[A-Za-z_][\w-]*$/;

export interface ThemeRegistry {
  register(theme: ThemeDefinition, opts?: { builtin?: boolean }): boolean;
  unregister(id: string): boolean;
  list(): ThemeSummary[];
  getActive(): string;
  apply(id: string, opts?: { persist?: boolean; suppressTransitions?: boolean }): Promise<boolean>;
  /** 只加载主题资源,不改变 activeId / data-theme / 持久化(切换前预载用)。 */
  preload(id: string): Promise<boolean>;
  subscribe(listener: ThemeListener): () => void;
  init(): Promise<void>;
  snapshot(): ThemeSnapshot;
}

function defaultStorage(): StorageLike | undefined {
  try {
    return typeof localStorage !== 'undefined' ? localStorage : undefined;
  } catch {
    return undefined;
  }
}

function validateTheme(theme: ThemeDefinition): string | null {
  if (!theme || typeof theme !== 'object') return '主题定义为空';
  if (!THEME_ID_RE.test(theme.id)) return `主题 id 非法: ${String(theme.id)}`;
  if (!theme.name || !theme.name.trim()) return `主题 ${theme.id} 缺少名称`;
  if (theme.className && !CLASS_RE.test(theme.className)) return `主题 ${theme.id} 的 className 非法`;
  if (theme.tokens) {
    for (const [key, value] of Object.entries(theme.tokens)) {
      if (!TOKEN_RE.test(key)) return `主题 ${theme.id} 的令牌名非法: ${key}`;
      if (typeof value !== 'string') return `主题 ${theme.id} 的令牌 ${key} 必须是字符串`;
    }
  }
  if (theme.fonts) {
    for (const f of theme.fonts) {
      if (!f.family || !f.src) return `主题 ${theme.id} 的字体声明缺少 family/src`;
    }
  }
  return null;
}

/**
 * 创建独立注册表。生产用下方 themeRegistry 单例;测试用本工厂注入内存 storage/root,
 * 避免全局状态互相污染。
 */
export function createThemeRegistry(options: ThemeRegistryOptions = {}): ThemeRegistry {
  const storage = options.storage ?? defaultStorage();
  const doc = options.document ?? (typeof document !== 'undefined' ? document : undefined);
  const root = options.root ?? doc?.documentElement;
  const themes = new Map<string, { theme: ThemeDefinition; builtin: boolean }>();
  const listeners = new Set<ThemeListener>();
  const loaded = new Map<string, Promise<unknown>>();
  let activeId = DEFAULT_THEME_ID;
  let appliedTokens: string[] = [];
  let appliedClass: string | null = null;

  function summaries(): ThemeSummary[] {
    return [...themes.values()]
      .map(({ theme, builtin }) => ({
        id: theme.id,
        name: theme.name,
        description: theme.description ?? '',
        builtin,
      }))
      .sort((a, b) => {
        if (a.id === DEFAULT_THEME_ID) return -1;
        if (b.id === DEFAULT_THEME_ID) return 1;
        if (a.builtin !== b.builtin) return a.builtin ? -1 : 1;
        return a.name.localeCompare(b.name, 'zh-CN');
      });
  }

  function snapshot(): ThemeSnapshot {
    return { themes: summaries(), activeId };
  }

  function notify(): void {
    const snap = snapshot();
    for (const listener of [...listeners]) listener(snap);
  }

  function removeApplied(): void {
    if (root) {
      for (const key of appliedTokens) root.style.removeProperty(key);
      if (appliedClass) root.classList.remove(appliedClass);
    }
    appliedTokens = [];
    appliedClass = null;
  }

  function injectAssets(theme: ThemeDefinition): void {
    if (!doc) return;
    if (theme.css) {
      const style = doc.createElement('style');
      style.dataset.kedaiThemeCss = theme.id;
      style.textContent = theme.css;
      doc.head.appendChild(style);
    }
    if (theme.fonts?.length) {
      const style = doc.createElement('style');
      style.dataset.kedaiThemeFonts = theme.id;
      style.textContent = theme.fonts
        .map(
          (f) =>
            `@font-face{font-family:"${f.family}";src:${f.src};font-weight:${f.weight ?? '400'};` +
            `font-style:${f.style ?? 'normal'};font-display:${f.display ?? 'swap'};}`,
        )
        .join('\n');
      doc.head.appendChild(style);
    }
  }

  function removeAssets(id: string): void {
    if (!doc) return;
    doc
      .querySelectorAll(`style[data-kedai-theme-css="${id}"], style[data-kedai-theme-fonts="${id}"]`)
      .forEach((el) => el.remove());
  }

  function register(theme: ThemeDefinition, opts: { builtin?: boolean } = {}): boolean {
    const error = validateTheme(theme);
    if (error) {
      console.warn('[KedaiUI] 主题注册失败:', error);
      return false;
    }
    if (themes.has(theme.id)) {
      console.warn('[KedaiUI] 主题 id 已存在:', theme.id);
      return false;
    }
    const definition: ThemeDefinition = { ...theme, tokens: theme.tokens ? { ...theme.tokens } : undefined };
    themes.set(theme.id, { theme: definition, builtin: opts.builtin === true });
    injectAssets(definition);
    notify();
    return true;
  }

  function unregister(id: string): boolean {
    const entry = themes.get(id);
    if (!entry || entry.builtin) return false;
    removeAssets(id);
    themes.delete(id);
    if (activeId === id) {
      void apply(DEFAULT_THEME_ID);
    } else {
      notify();
    }
    return true;
  }

  function loadTheme(id: string): Promise<unknown> | undefined {
    const entry = themes.get(id);
    if (!entry?.theme.load) return undefined;
    let pending = loaded.get(id);
    if (!pending) {
      try {
        pending = Promise.resolve(entry.theme.load()).catch((err) => {
          console.warn('[KedaiUI] 主题资源加载失败:', id, err);
        });
      } catch (err) {
        console.warn('[KedaiUI] 主题资源加载失败:', id, err);
        pending = Promise.resolve();
      }
      loaded.set(id, pending);
    }
    return pending;
  }

  /** 只预载资源;不改 activeId,不写 storage,不写 data-theme。 */
  async function preload(id: string): Promise<boolean> {
    const entry = themes.get(id);
    if (!entry) return false;
    const pending = loadTheme(id);
    if (pending) await pending;
    return true;
  }

  /**
   * 主题切换期间临时归零过渡/动画时长:换令牌会让全站颜色/背景同时变化,
   * 若各自按自己的 transition 播一遍,就是一次「过渡风暴」。两帧后移除。
   * 用临时 style 而不是全局 CSS:主题切换不是常驻状态,也不增加全局域行数。
   */
  function suppressTransitionStorm(): void {
    if (!doc || !root) return;
    const style = doc.createElement('style');
    style.dataset.kedaiThemeSwitching = '1';
    style.textContent =
      'html[data-theme-switching] *,html[data-theme-switching] *::before,' +
      'html[data-theme-switching] *::after{transition-duration:0s;animation-duration:0s;}';
    doc.head.appendChild(style);
    root.dataset.themeSwitching = '1';
    let cleared = false;
    const clear = (): void => {
      if (cleared) return;
      cleared = true;
      delete root.dataset.themeSwitching;
      style.remove();
    };
    if (typeof requestAnimationFrame === 'function') {
      requestAnimationFrame(() => requestAnimationFrame(clear));
    } else {
      setTimeout(clear, 32);
    }
  }

  async function apply(
    id: string,
    opts: { persist?: boolean; suppressTransitions?: boolean } = {},
  ): Promise<boolean> {
    const entry = themes.get(id);
    if (!entry) return false;
    const theme = entry.theme;
    if (opts.suppressTransitions !== false) suppressTransitionStorm();
    removeApplied();
    if (root) {
      root.dataset.theme = id;
      if (theme.tokens) {
        for (const [key, value] of Object.entries(theme.tokens)) {
          if (!TOKEN_RE.test(key)) continue;
          root.style.setProperty(key, value);
          appliedTokens.push(key);
        }
      }
      if (theme.className) {
        root.classList.add(theme.className);
        appliedClass = theme.className;
      }
    }
    activeId = id;
    if (opts.persist !== false) {
      try {
        storage?.setItem(THEME_STORAGE_KEY, id);
      } catch {
        /* localStorage 不可用时仅本次会话生效 */
      }
    }
    notify();

    const pending = loadTheme(id);
    if (pending) await pending;
    return true;
  }

  async function init(): Promise<void> {
    let saved: string | null;
    try {
      saved = storage?.getItem(THEME_STORAGE_KEY) ?? null;
    } catch {
      saved = null;
    }
    const id = saved && themes.has(saved) ? saved : DEFAULT_THEME_ID;
    if (saved && saved !== id) {
      try {
        storage?.setItem(THEME_STORAGE_KEY, id);
      } catch {
        /* 忽略 */
      }
    }
    // 启动期不要抑制进入动画:主界面是预挂载的,splash 结束后不应再播一次。
    await apply(id, { persist: false, suppressTransitions: false });
  }

  function subscribe(listener: ThemeListener): () => void {
    listeners.add(listener);
    listener(snapshot());
    return () => listeners.delete(listener);
  }

  for (const theme of options.builtinThemes ?? BUILTIN_THEMES) {
    register(theme, { builtin: true });
  }

  return {
    register,
    unregister,
    list: summaries,
    getActive: () => activeId,
    apply,
    preload,
    subscribe,
    init,
    snapshot,
  };
}

/** 生产单例:内置主题在模块加载时注册;init/apply 由 main.ts 与设置页调用。 */
export const themeRegistry = createThemeRegistry();

export const listThemes = (): ThemeSummary[] => themeRegistry.list();
export const getActiveThemeId = (): string => themeRegistry.getActive();
export const applyTheme = (id: string): Promise<boolean> => themeRegistry.apply(id);
export const preloadTheme = (id: string): Promise<boolean> => themeRegistry.preload(id);
export const subscribeThemes = (listener: ThemeListener): (() => void) => themeRegistry.subscribe(listener);

/** 启动期主题资源 Promise;SplashScreen 用它避免主题 CSS 晚到造成揭幕后二次重绘。 */
let themeReady: Promise<void> = Promise.resolve();

export const initTheme = (): Promise<void> => {
  themeReady = themeRegistry.init();
  return themeReady;
};

export const whenThemeReady = (): Promise<void> => themeReady;

/** 暴露给外部前端插件的窄接口:只做主题注册/切换,不提供 DOM 写入口。 */
export interface KedaiUiApi {
  readonly version: number;
  registerTheme(theme: ThemeDefinition): boolean;
  unregisterTheme(id: string): boolean;
  listThemes(): ThemeSummary[];
  getActiveTheme(): string;
  applyTheme(id: string): Promise<boolean>;
  onChange(listener: ThemeListener): () => void;
}

declare global {
  interface Window {
    KedaiUI?: KedaiUiApi;
  }
}

export function installThemeApi(): KedaiUiApi | undefined {
  if (typeof window === 'undefined') return undefined;
  const api: KedaiUiApi = {
    version: THEME_API_VERSION,
    registerTheme: (theme) => themeRegistry.register(theme),
    unregisterTheme: (id) => themeRegistry.unregister(id),
    listThemes: () => themeRegistry.list(),
    getActiveTheme: () => themeRegistry.getActive(),
    applyTheme: (id) => themeRegistry.apply(id),
    onChange: (listener) => themeRegistry.subscribe(listener),
  };
  window.KedaiUI = api;
  return api;
}

/** 挂载完成后通知外部插件:API 已就绪。插件也可直接轮询/读取 window.KedaiUI。 */
export function announceUiReady(): void {
  if (typeof window === 'undefined' || !window.KedaiUI) return;
  window.dispatchEvent(new CustomEvent('kedai:ui-ready', { detail: window.KedaiUI }));
}
