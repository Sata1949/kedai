// 内置主题:默认纸面 + 教堂拼接玻璃。
// 主题只声明令牌/字体/按需资源,不持有 DOM;外部插件通过 window.KedaiUI 注册同形状的主题。
import type { ThemeDefinition } from './registry';

/**
 * 教堂拼接玻璃的视觉参数（2026-11-02 重做：石室与光）。
 *
 * 参照真实教堂彩窗（哥特长窗 / 布拉索夫暗室暖窗 / 科隆 Richter 窗）提炼的三层模型：
 *   层 0 石室 —— 近黑微暖，四角暗角，只承载对比；
 *   层 1 窗   —— 面板就是一整块**从背后发光**的乳白玻璃（透射光，不是反射光）；
 *   层 2 石梁 —— 顶栏/输入区近黑，只留被窗光照到的一条暖金边。
 *
 * 两条决定成败的经验（均由 1440×900 实测得出）：
 *   - **玻璃不能用纯白**：真实彩窗的眩光风险（高亮且近乎无彩的面积）远低于屏幕白；
 *     这里改用暖象牙一族（#ece5d6 / #ddd5c2），实测眩光 12.5% → 0.3%。
 *   - **彩色是稀缺资源**：真实窗的彩色只镶在窗缘一窄条 + 顶部圆花窗。旧版把彩色拼片
 *     当主材质铺满 chrome，读作「印着菱形花纹的纸」；本版只在窗楣 / 选中态 / 主按钮
 *     三处上色，其余一律素玻璃。
 *
 * 现有组件用 --sv-white 同时承担白底与黑底白字，因此保持浅色正文 + 深色文字的
 * 可读性骨架，由主题 CSS 的暗室外壳、玻璃材质与层级光建立教堂感。
 */
const STAINED_GLASS_TOKENS: Record<string, string> = {
  '--sv-paper': '#ece5d6',
  '--sv-surface': '#ddd5c2',
  '--sv-surface-elevated': '#faf5e8',
  '--sv-white': '#fdf8ec',
  '--sv-ink': '#221b27',
  '--sv-ink-supreme': '#05040a',
  '--sv-ink-soft': '#4c4249',
  '--sv-ink-dim': '#5c504b',
  /* 次级文字：在 --sv-surface(#ddd5c2) 上实测 4.69:1、paper 上 5.46:1，
     过 WCAG AA 4.5:1（原 #6f6159 只有 4.07:1，被 registry.test.ts 抓出后加深一档） */
  '--sv-ink-faint': '#655850',
  '--sv-line': '#8b7a63',
  '--sv-line-strong': '#5a4c3c',
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
  description: '暗石室中一扇从背后发光的乳白玻璃：暖象牙不透光面、石框与窗棂、彩色只镶在窗楣与选中态；宋体正文，Cinzel / Cormorant Garamond 字标。',
  tokens: STAINED_GLASS_TOKENS,
  load: loadStainedGlass,
};

export const BUILTIN_THEMES: ThemeDefinition[] = [DEFAULT_THEME, STAINED_GLASS_THEME];
