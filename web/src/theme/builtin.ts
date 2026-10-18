// 内置主题:默认纸面 + 教堂拼接玻璃。
// 主题只声明令牌/字体/按需资源,不持有 DOM;外部插件通过 window.KedaiUI 注册同形状的主题。
import type { ThemeDefinition } from './registry';

/**
 * 教堂拼接玻璃的视觉参数。
 *
 * 参照真实教堂彩窗(沙特尔蓝圣母窗 / 圣礼拜堂 / 巴黎圣母院北玫瑰窗)提炼:
 *   - 石构与铅条是**近黑**的,玻璃是**自发光**的;所以面板底色是透光的白玻璃
 *     (grisaille),外壳与边框是暗石与铅条,而不是整片暗底。
 *   - 主色是钴蓝,红宝石与金只作点缀;旧版把六种宝石色等量铺开,读起来像色轮,
 *     这里收敛为「蓝场 + 红金珠宝 + 祖母绿少量」。
 *   - 现有组件用 --sv-white 同时承担白底与黑底白字,因此保持浅色面板 + 深色文字
 *     的可读性骨架,由主题 CSS 的暗石外壳、铅条格纹、珠宝色玻璃 chrome 建立教堂感。
 */
const STAINED_GLASS_TOKENS: Record<string, string> = {
  '--sv-paper': '#f1ead9',
  '--sv-surface': '#e7dfca',
  '--sv-surface-elevated': '#faf5e6',
  '--sv-white': '#fffdf2',
  '--sv-ink': '#171219',
  '--sv-ink-supreme': '#070509',
  '--sv-ink-soft': '#332a31',
  '--sv-ink-dim': '#554a4f',
  '--sv-ink-faint': '#5b4f54',
  '--sv-line': '#6f6058',
  '--sv-line-strong': '#3a3036',
  '--sv-pink': '#d9929e',
  '--sv-pink-light': '#f4e3e2',
  '--sv-pink-medium': '#c46576',
  '--sv-pink-deep': '#9c2540',
  '--sv-pink-dark': '#6d1429',
  '--sv-pink-deep-rgb': '156, 37, 64',
  '--sv-red': '#a3192c',
  '--sv-red-deep': '#7b0f1f',
  '--sv-blue': '#153c7e',
  '--sv-yellow': '#d9a441',
  '--sv-yellow-deep': '#78570f',
  '--sv-green': '#1f6b4a',
  '--sv-green-deep': '#145037',
  '--sv-purple': '#4c2a6e',
  '--sv-orange': '#b5651d',
  '--bw': '3px',
  '--bw-heavy': '4px',
  '--bw-thin': '1px',
  '--bw-hair': '1px',
  '--font-display': '"Cinzel", "Noto Serif SC", "Songti SC", "STSong", "SimSun", serif',
  '--font-body': '"Cormorant Garamond", "Noto Serif SC", "Songti SC", "STSong", "SimSun", serif',
  '--shadow-sm': '2px 2px 0 rgba(12, 8, 14, 0.14)',
  '--shadow-card': '3px 3px 0 rgba(12, 8, 14, 0.18)',
  '--shadow-pink': '4px 4px 0 rgba(156, 37, 64, 0.22)',
  '--shadow-pink-strong': '4px 4px 0 rgba(156, 37, 64, 0.30)',
  '--shadow-hover': '5px 5px 0 rgba(12, 8, 14, 0.22)',
  '--shadow-raise': '4px 4px 0 rgba(12, 8, 14, 0.24)',
  '--shadow-sunk': '4px 4px 0 rgba(12, 8, 14, 0.22)',
  '--shadow-card-soft': '5px 5px 0 rgba(12, 8, 14, 0.16)',
  '--shadow-modal': '8px 8px 0 rgba(12, 8, 14, 0.34)',
  '--shadow-ctx': '8px 8px 0 rgba(12, 8, 14, 0.34)',
  '--shadow-hard': '5px 5px 0 var(--sv-ink)',
  '--sv-top-stripe':
    'linear-gradient(90deg, #8e1b33 0 58px, #143a7c 58px 86px, #c99a3b 86px 94px, transparent 94px)',
};

let stainedGlassCss: Promise<unknown> | undefined;
let stainedGlassFonts: Promise<unknown> | undefined;

/**
 * 首次激活/预载时加载主题 CSS 与 OFL 字体。
 * CSS 是切换是否「完整」的判据,字体只影响字形:两者拆开,CSS 就绪即可切主题,
 * 字体在后台加载并由 font-display: swap 接管,避免字体拖住主题切换。
 */
function loadStainedGlass(): Promise<unknown> {
  stainedGlassCss ??= import('./stained-glass.css');
  stainedGlassFonts ??= Promise.all([
    import('@fontsource/cinzel/latin-600.css'),
    import('@fontsource/cormorant-garamond/latin-400.css'),
  ]).catch((err) => {
    console.warn('[KedaiUI] 彩窗主题字体加载失败:', err);
  });
  return stainedGlassCss;
}

export const DEFAULT_THEME: ThemeDefinition = {
  id: 'default',
  name: '纸面 · 至上主义',
  description: '淡粉纸面、黑方块与红斜线；Kedai 的原始构成主义骨架。',
};

export const STAINED_GLASS_THEME: ThemeDefinition = {
  id: 'stained-glass',
  name: '教堂拼接玻璃',
  description: '暗石外壳、白玻璃菱格与彩色玻璃拼片；密窗玫瑰，Cinzel / Cormorant Garamond 字体，中文回退系统宋体。',
  tokens: STAINED_GLASS_TOKENS,
  load: loadStainedGlass,
};

export const BUILTIN_THEMES: ThemeDefinition[] = [DEFAULT_THEME, STAINED_GLASS_THEME];
