/**
 * 环境层(WebGL)纯逻辑:场景配置、能力/性能钳制、主题令牌读取、shader 源。
 *
 * 边界:
 * - 零依赖、零纹理、零外部资源;只画「纸面光/制图台/玻璃光」三种低频背景。
 * - 输出亮度必须钳在主题纸面附近(±3%),正文永远落在实色气泡/卡片上——
 *   环境层只负责气氛,不参与文字对比度。
 * - 组件(AmbientField.vue)只做生命周期;所有可测判定集中在本文,避免把
 *   WebGL 细节散进模板。
 */

export type AmbientVariant = 'paper' | 'draft' | 'glass';

export interface AmbientRgb {
  r: number;
  g: number;
  b: number;
}

export interface AmbientPalette {
  paper: AmbientRgb;
  rose: AmbientRgb;
  blue: AmbientRgb;
  red: AmbientRgb;
  ink: AmbientRgb;
}

/** 与 tokens.css 同步的回退色:主题令牌缺失/CSS 读取失败时仍能出图。 */
export const FALLBACK_PALETTE: AmbientPalette = {
  paper: { r: 253, g: 250, b: 251 },
  rose: { r: 224, g: 104, b: 144 },
  blue: { r: 30, g: 58, b: 138 },
  red: { r: 214, g: 43, b: 37 },
  ink: { r: 23, g: 24, b: 26 },
};

/** 30fps 上限:环境层是低频氛围,不需要 60fps 的功耗。 */
export const AMBIENT_FRAME_MS = 1000 / 30;
export const AMBIENT_MAX_DPR = 1.25;
/** 低分辨率渲染再交给 CSS 拉伸,避免大画布逐像素开销。 */
export const AMBIENT_RENDER_SCALE = 0.6;
export const AMBIENT_MIN_SCALE = 0.5;

const VARIANT_INDEX: Record<AmbientVariant, number> = { paper: 0, draft: 1, glass: 2 };

export interface AmbientConfigInput {
  width: number;
  height: number;
  dpr: number;
  reducedMotion: boolean;
  webgl: boolean;
  visible: boolean;
  paused: boolean;
  hardwareConcurrency?: number;
}

export interface AmbientConfig {
  enabled: boolean;
  animate: boolean;
  /** reduced-motion:只渲染 t=0 的静态首帧,不启动循环。 */
  staticOnly: boolean;
  width: number;
  height: number;
  frameMs: number;
  dpr: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * 把一个尺寸/能力快照解析成渲染参数。
 * - DPR 上限 1.25、渲染缩放 0.6(窄屏 0.5),画布像素量约为 CSS 像素的 0.6 倍;
 * - 减少动态效果 → 静态首帧;离屏/页面隐藏/生成中 → 不跑循环;
 * - 低核数设备(≤2)直接不启用,避免与模型流式渲染抢主线程。
 */
export function resolveAmbientConfig(input: AmbientConfigInput): AmbientConfig {
  const width = Math.max(0, Math.floor(input.width));
  const height = Math.max(0, Math.floor(input.height));
  const dpr = clamp(Number.isFinite(input.dpr) ? input.dpr : 1, 1, AMBIENT_MAX_DPR);
  const scale = width > 0 && width < 640 ? AMBIENT_MIN_SCALE : AMBIENT_RENDER_SCALE;
  const cores = input.hardwareConcurrency ?? 8;
  const enabled = input.webgl && width > 0 && height > 0 && cores > 2;
  const staticOnly = input.reducedMotion;
  const animate = enabled && !staticOnly && input.visible && !input.paused;
  return {
    enabled,
    animate,
    staticOnly,
    width: Math.max(1, Math.round(width * dpr * scale)),
    height: Math.max(1, Math.round(height * dpr * scale)),
    frameMs: AMBIENT_FRAME_MS,
    dpr,
  };
}

/** 背景层永远是装饰:页面隐藏/离屏/降低动态时都不该继续跑 RAF。 */
export function shouldAnimate(
  cfg: Pick<AmbientConfig, 'enabled' | 'staticOnly'>,
  state: { visible: boolean; hidden: boolean; paused: boolean },
): boolean {
  return cfg.enabled && !cfg.staticOnly && state.visible && !state.hidden && !state.paused;
}

/** 读取 CSS 颜色;支持 #rgb/#rrggbb/rgb()/rgba(),失败返回 fallback。 */
export function parseCssColor(value: string | null | undefined, fallback: AmbientRgb): AmbientRgb {
  const raw = (value ?? '').trim();
  if (!raw) return { ...fallback };
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(raw);
  if (hex) {
    const body = hex[1];
    const full = body.length === 3 ? body.split('').map((c) => c + c).join('') : body;
    return {
      r: Number.parseInt(full.slice(0, 2), 16),
      g: Number.parseInt(full.slice(2, 4), 16),
      b: Number.parseInt(full.slice(4, 6), 16),
    };
  }
  const rgb = /^rgba?\(\s*([0-9.]+)\s*[, ]\s*([0-9.]+)\s*[, ]\s*([0-9.]+)/i.exec(raw);
  if (rgb) {
    return {
      r: clamp(Math.round(Number(rgb[1])), 0, 255),
      g: clamp(Math.round(Number(rgb[2])), 0, 255),
      b: clamp(Math.round(Number(rgb[3])), 0, 255),
    };
  }
  return { ...fallback };
}

function cssVar(style: CSSStyleDeclaration, name: string, fallback: AmbientRgb): AmbientRgb {
  try {
    return parseCssColor(style.getPropertyValue(name), fallback);
  } catch {
    return { ...fallback };
  }
}

/**
 * 从 :root 的计算样式读主题令牌。
 * 彩窗主题会把 --sv-paper 等令牌改成暗石/透光色,这里自然跟随,无需按主题写死。
 */
export function readAmbientPalette(root: HTMLElement | null): AmbientPalette {
  if (!root || typeof getComputedStyle !== 'function') return { ...FALLBACK_PALETTE };
  const style = getComputedStyle(root);
  return {
    paper: cssVar(style, '--sv-paper', FALLBACK_PALETTE.paper),
    rose: cssVar(style, '--sv-pink-deep', FALLBACK_PALETTE.rose),
    blue: cssVar(style, '--sv-blue', FALLBACK_PALETTE.blue),
    red: cssVar(style, '--sv-red', FALLBACK_PALETTE.red),
    ink: cssVar(style, '--sv-ink', FALLBACK_PALETTE.ink),
  };
}

/** 主题 → 场景:彩窗用 glass(焦散),其余按主体角色走 paper/draft。 */
export function themeToVariant(themeId: string | null | undefined, base: AmbientVariant): AmbientVariant {
  return themeId === 'stained-glass' ? 'glass' : base;
}

export function variantIndex(variant: AmbientVariant): number {
  return VARIANT_INDEX[variant];
}

export function rgbToVec3(c: AmbientRgb): [number, number, number] {
  return [c.r / 255, c.g / 255, c.b / 255];
}

/**
 * 片元 shader(WebGL1 / GLSL ES 1.0)。
 * 设计约束:
 * - 所有颜色混合系数 ≤ 0.18,最后再按纸面亮度钳制,保证不会把文字底衬拉暗;
 * - 低频 fbm(3 octave)+ 屏幕颗粒;draft 场景叠制图网格与缓慢红楔;
 * - glass 场景只做低强度焦散,让位给彩窗主题自己的拼片 CSS。
 */
export const AMBIENT_VERTEX_SHADER = `
attribute vec2 a_pos;
void main() {
  gl_Position = vec4(a_pos, 0.0, 1.0);
}
`;

export const AMBIENT_FRAGMENT_SHADER = `
precision mediump float;
uniform vec2 u_res;
uniform float u_time;
uniform vec3 u_paper;
uniform vec3 u_rose;
uniform vec3 u_blue;
uniform vec3 u_red;
uniform vec3 u_ink;
uniform float u_intensity;
uniform int u_variant;

float hash(vec2 p) {
  p = fract(p * vec2(123.34, 456.21));
  p += dot(p, p + 45.32);
  return fract(p.x * p.y);
}

float noise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(
    mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x),
    mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x),
    u.y
  );
}

float fbm(vec2 p) {
  float v = 0.0;
  float a = 0.5;
  for (int i = 0; i < 3; i++) {
    v += a * noise(p);
    p = p * 2.03 + 17.1;
    a *= 0.5;
  }
  return v;
}

void main() {
  vec2 p = (gl_FragCoord.xy - 0.5 * u_res) / max(u_res.y, 1.0);
  float t = u_time;
  vec3 col = u_paper;
  float k = clamp(u_intensity, 0.0, 1.0);

  if (u_variant == 0) {
    float n1 = fbm(p * 1.35 + vec2(t * 0.020, -t * 0.014));
    float n2 = fbm(p * 0.82 + vec2(-t * 0.011, t * 0.017) + 11.0);
    col = mix(col, u_rose, smoothstep(0.42, 0.95, n1) * 0.16 * k);
    col = mix(col, u_blue, smoothstep(0.48, 0.96, n2) * 0.09 * k);
    float light = smoothstep(0.95, 0.0, length(p - vec2(-0.46, 0.42)));
    col = mix(col, vec3(1.0), light * 0.05 * k);
  } else if (u_variant == 1) {
    vec2 minor = abs(fract(p * 13.0) - 0.5);
    float minorLine = 1.0 - smoothstep(0.0, 0.012, min(minor.x, minor.y));
    col = mix(col, u_ink, minorLine * 0.030 * k);
    vec2 major = abs(fract(p * 3.25) - 0.5);
    float majorLine = 1.0 - smoothstep(0.0, 0.008, min(major.x, major.y));
    col = mix(col, u_ink, majorLine * 0.026 * k);
    float s = p.x * 0.58 + p.y * 0.81;
    float wedge = 1.0 - smoothstep(0.0, 0.018, abs(fract(s * 0.9 - t * 0.011) - 0.5));
    col = mix(col, u_red, wedge * 0.030 * k);
  } else {
    float n1 = fbm(p * 1.15 + vec2(t * 0.008, -t * 0.006));
    float n2 = fbm(p * 0.9 + vec2(-t * 0.006, t * 0.009) + 7.0);
    col = mix(col, u_rose, smoothstep(0.38, 0.92, n1) * 0.20 * k);
    col = mix(col, u_blue, smoothstep(0.52, 0.98, n2) * 0.16 * k);
    col = mix(col, u_ink, smoothstep(0.70, 0.98, fbm(p * 2.1 - t * 0.004)) * 0.05 * k);
  }

  float grain = hash(gl_FragCoord.xy + fract(t * 0.1) * 131.0) - 0.5;
  col += grain * 0.010 * k;

  float paperLum = dot(u_paper, vec3(0.2126, 0.7152, 0.0722));
  float lum = dot(col, vec3(0.2126, 0.7152, 0.0722));
  float target = clamp(lum, paperLum - 0.030, paperLum + 0.030);
  col += (target - lum);

  gl_FragColor = vec4(col, 1.0);
}
`;
