// UIP-10 令牌级对比度契约(2026-10-02)。把「小字可读性」从注释纪律变成机器事实:
// 从 tokens.css 现取令牌值,按 WCAG 2.x 相对亮度公式算对比度,锁定强制对 ≥ 4.5:1。
// 只覆盖「令牌对令牌」的稳定承诺;选择器级的实色修正(如 MemoryPanel kind 标签)由
// 探针实测量测取证,见变更史。`.css` 的 `?raw` 在本仓返回空串,故走 node:fs。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

const tokensCss = readFileSync(new URL('tokens.css', import.meta.url), 'utf8');

/** 取 `--name: value;` 声明(值含多行时截到分号,仅用于色值)。 */
const token = (name: string): string => {
  const m = tokensCss.match(new RegExp(`^\\s*${name}\\s*:\\s*([^;]+);`, 'm'));
  if (!m) throw new Error(`tokens.css 缺少 ${name}`);
  return m[1].trim();
};

interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

const toRgba = (raw: string): Rgba => {
  const hex = raw.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (hex) {
    const h = hex[1].length === 3 ? hex[1].split('').map((c) => c + c).join('') : hex[1];
    return {
      r: parseInt(h.slice(0, 2), 16),
      g: parseInt(h.slice(2, 4), 16),
      b: parseInt(h.slice(4, 6), 16),
      a: 1,
    };
  }
  const fn = raw.match(/^rgba?\(([^)]+)\)$/i);
  if (fn) {
    const p = fn[1].split(',').map((s) => Number(s.trim()));
    return { r: p[0], g: p[1], b: p[2], a: p[3] ?? 1 };
  }
  throw new Error(`无法解析颜色:${raw}`);
};

/** 前景半透明时按「over 不透明底」合成(仅用于 --sv-on-ink-soft 这类场景)。 */
const over = (fg: Rgba, bg: Rgba): Rgba => ({
  r: fg.a * fg.r + (1 - fg.a) * bg.r,
  g: fg.a * fg.g + (1 - fg.a) * bg.g,
  b: fg.a * fg.b + (1 - fg.a) * bg.b,
  a: 1,
});

const linear = (c: number): number => {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
};

const luminance = (c: Rgba): number => 0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b);

/** WCAG 对比度:fg 半透明时先与 bg 合成。 */
const contrast = (fgRaw: string, bgRaw: string): number => {
  const bg = toRgba(bgRaw);
  const fg0 = toRgba(fgRaw);
  const fg = fg0.a < 1 ? over(fg0, bg) : fg0;
  const l1 = Math.max(luminance(fg), luminance(bg));
  const l2 = Math.min(luminance(fg), luminance(bg));
  return (l1 + 0.05) / (l2 + 0.05);
};

/** 强制对:小字(≤12px)字色 × 底色。 */
const PAIRS: Array<[string, string]> = [
  ['--sv-ink-faint', '--sv-paper'], // 侧栏/画布上的说明文字
  ['--sv-ink-faint', '--sv-surface'], // 设置字段(.sv-field)、顶栏副标、输入栏提示
  ['--sv-ink-faint', '--sv-pink-light'], // hover 底(角色行说明等)
  ['--sv-ink-faint', '--sv-white'], // 白卡片内数据行
  ['--sv-ink-dim', '--sv-surface'],
  ['--sv-ink', '--sv-paper'],
  ['--sv-ink', '--sv-white'],
  ['--sv-white', '--sv-ink'], // 黑底主文字
  ['--sv-white', '--sv-ink-supreme'],
  ['--sv-white', '--sv-blue'], // MemoryPanel kind=distilled
  ['--sv-white', '--sv-ink-faint'], // MemoryPanel kind=unknown
  ['--sv-ink', '--sv-yellow'], // MemoryPanel kind=tool(UIP-10 由白字改 ink 字)
  ['--sv-ink', '--sv-green'], // MemoryPanel kind=manual
  ['--sv-pink-dark', '--sv-white'], // MemoryPanel 置顶标记(擦线 4.51,只需不退)
  ['--sv-on-ink-soft', '--sv-ink'], // 黑底次级文字(半透明合成后)
];

describe('UIP-10 令牌级小字对比度契约(WCAG AA ≥ 4.5:1)', () => {
  for (const [fgName, bgName] of PAIRS) {
    it(`${fgName} on ${bgName}`, () => {
      const ratio = contrast(token(fgName), token(bgName));
      expect(ratio, `${fgName}(${token(fgName)}) on ${bgName}(${token(bgName)}) = ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
    });
  }

  it('旧 ink-faint #6b6b6b 在 surface 上确实不达标(本批加深的动机可复算)', () => {
    expect(contrast('#6b6b6b', token('--sv-surface'))).toBeLessThan(4.5);
  });
});
