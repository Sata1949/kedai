import { describe, expect, it } from 'vitest';
import {
  AMBIENT_FRAGMENT_SHADER,
  AMBIENT_VERTEX_SHADER,
  FALLBACK_PALETTE,
  parseCssColor,
  readAmbientPalette,
  resolveAmbientConfig,
  shouldAnimate,
  themeToVariant,
  variantIndex,
} from './ambient';

describe('ambient 配置与颜色解析', () => {
  it('解析 #rrggbb / #rgb / rgb() / rgba(),失败回退', () => {
    expect(parseCssColor('#fdfafb', FALLBACK_PALETTE.paper)).toEqual({ r: 253, g: 250, b: 251 });
    expect(parseCssColor('#abc', FALLBACK_PALETTE.paper)).toEqual({ r: 170, g: 187, b: 204 });
    expect(parseCssColor('rgb(1, 2, 3)', FALLBACK_PALETTE.paper)).toEqual({ r: 1, g: 2, b: 3 });
    expect(parseCssColor('rgba(1, 2, 3, 0.4)', FALLBACK_PALETTE.paper)).toEqual({ r: 1, g: 2, b: 3 });
    expect(parseCssColor('not-a-color', FALLBACK_PALETTE.paper)).toEqual(FALLBACK_PALETTE.paper);
    expect(parseCssColor(null, FALLBACK_PALETTE.paper)).toEqual(FALLBACK_PALETTE.paper);
  });

  it('无 DOM 时读回退色板,不抛错', () => {
    expect(readAmbientPalette(null)).toEqual(FALLBACK_PALETTE);
  });

  it('DPR 钳到 1.25、渲染缩放 0.6(窄屏 0.5)', () => {
    const wide = resolveAmbientConfig({
      width: 1000, height: 800, dpr: 2, reducedMotion: false, webgl: true,
      visible: true, paused: false, hardwareConcurrency: 8,
    });
    expect(wide.enabled).toBe(true);
    expect(wide.animate).toBe(true);
    expect(wide.dpr).toBe(1.25);
    expect(wide.width).toBe(750);
    expect(wide.height).toBe(600);
    expect(wide.frameMs).toBeCloseTo(1000 / 30);

    const narrow = resolveAmbientConfig({
      width: 400, height: 700, dpr: 1, reducedMotion: false, webgl: true,
      visible: true, paused: false, hardwareConcurrency: 8,
    });
    expect(narrow.width).toBe(200);
    expect(narrow.height).toBe(350);
  });

  it('减少动态 → 静态首帧;离屏/暂停/低核 → 不跑循环', () => {
    const base = {
      width: 800, height: 600, dpr: 1, webgl: true, visible: true, paused: false, hardwareConcurrency: 8,
    };
    const reduced = resolveAmbientConfig({ ...base, reducedMotion: true });
    expect(reduced.enabled).toBe(true);
    expect(reduced.staticOnly).toBe(true);
    expect(reduced.animate).toBe(false);

    expect(resolveAmbientConfig({ ...base, reducedMotion: false, paused: true }).animate).toBe(false);
    expect(resolveAmbientConfig({ ...base, reducedMotion: false, visible: false }).animate).toBe(false);
    expect(resolveAmbientConfig({ ...base, reducedMotion: false, webgl: false }).enabled).toBe(false);
    expect(resolveAmbientConfig({ ...base, reducedMotion: false, hardwareConcurrency: 2 }).enabled).toBe(false);
  });

  it('shouldAnimate 汇总可见/隐藏/暂停三态', () => {
    expect(shouldAnimate({ enabled: true, staticOnly: false }, { visible: true, hidden: false, paused: false })).toBe(true);
    expect(shouldAnimate({ enabled: true, staticOnly: false }, { visible: false, hidden: false, paused: false })).toBe(false);
    expect(shouldAnimate({ enabled: true, staticOnly: false }, { visible: true, hidden: true, paused: false })).toBe(false);
    expect(shouldAnimate({ enabled: true, staticOnly: false }, { visible: true, hidden: false, paused: true })).toBe(false);
    expect(shouldAnimate({ enabled: true, staticOnly: true }, { visible: true, hidden: false, paused: false })).toBe(false);
  });

  it('彩窗主题切 glass,其它主题保持主体场景', () => {
    expect(themeToVariant('stained-glass', 'paper')).toBe('glass');
    expect(themeToVariant('stained-glass', 'draft')).toBe('glass');
    expect(themeToVariant('default', 'paper')).toBe('paper');
    expect(themeToVariant(null, 'draft')).toBe('draft');
    expect(variantIndex('paper')).toBe(0);
    expect(variantIndex('draft')).toBe(1);
    expect(variantIndex('glass')).toBe(2);
  });

  it('shader 源包含全部 uniform/变化分支,且不依赖纹理', () => {
    expect(AMBIENT_VERTEX_SHADER).toContain('a_pos');
    for (const name of ['u_res', 'u_time', 'u_paper', 'u_rose', 'u_blue', 'u_red', 'u_ink', 'u_intensity', 'u_variant']) {
      expect(AMBIENT_FRAGMENT_SHADER).toContain(name);
    }
    expect(AMBIENT_FRAGMENT_SHADER).toContain('u_variant == 0');
    expect(AMBIENT_FRAGMENT_SHADER).toContain('u_variant == 1');
    expect(AMBIENT_FRAGMENT_SHADER).not.toContain('texture2D');
  });
});
