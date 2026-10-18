<script setup lang="ts">
/**
 * AmbientField —— 手写 WebGL 环境层(零依赖)。
 *
 * 使用位置:角色扮演/任务工作台的 stage 背景。它只画低频氛围,不承载任何信息;
 * 文字始终落在不透明的气泡/卡片/白色场上,因此环境层不会参与文字对比度。
 *
 * 降级链:
 *   1. WebGL 可用 + 未减少动态 → 20/30fps 低频 shader(低档 20fps);
 *   2. 减少动态效果 → 只画 t=0 的静态首帧;
 *   3. 无 WebGL / 上下文丢失 / 组件离屏 → CSS 静态渐变兜底(布局不变)。
 */
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import {
  AMBIENT_FRAGMENT_SHADER,
  AMBIENT_VERTEX_SHADER,
  type AmbientPalette,
  type AmbientVariant,
  readAmbientPalette,
  resolveAmbientConfig,
  rgbToVec3,
  shouldAnimate,
  themeToVariant,
  variantIndex,
} from '../visual/ambient';

const props = withDefaults(
  defineProps<{
    /** paper=角色扮演纸面光;draft=任务制图台;glass=彩窗主题焦散。 */
    variant?: AmbientVariant;
    /** 0..1:空态更亮,有内容时压到 0.2 左右。 */
    intensity?: number;
    /** 生成中/任务执行中暂停,把主线程让给流式渲染。 */
    paused?: boolean;
  }>(),
  { variant: 'paper', intensity: 0.8, paused: false },
);

const rootEl = ref<HTMLElement | null>(null);
const canvasEl = ref<HTMLCanvasElement | null>(null);
const staticOnly = ref(false);
const ready = ref(false);
const webglState = ref<'pending' | 'ready' | 'unavailable' | 'lost'>('pending');

let gl: WebGLRenderingContext | null = null;
let program: WebGLProgram | null = null;
let buffer: WebGLBuffer | null = null;
let uniforms: Record<string, WebGLUniformLocation | null> = {};
let palette: AmbientPalette = readAmbientPalette(null);
let rafId: number | null = null;
let lastFrameAt = 0;
let startedAt = 0;
let visible = true;
let pageHidden = false;
let themeId: string | null = null;
let currentVariant: AmbientVariant = props.variant;
let currentQuality: 0 | 1 = 1;
let resizeTimer: ReturnType<typeof setTimeout> | null = null;

let resizeObserver: ResizeObserver | null = null;
let intersectionObserver: IntersectionObserver | null = null;
let motionQuery: MediaQueryList | null = null;
let unsubscribeTheme: (() => void) | null = null;

function reducedMotion(): boolean {
  return motionQuery?.matches ?? false;
}

function variantNow(): AmbientVariant {
  return themeToVariant(themeId, props.variant);
}

function compile(type: number, source: string): WebGLShader | null {
  if (!gl) return null;
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    console.warn('[AmbientField] shader 编译失败:', gl.getShaderInfoLog(shader));
    gl.deleteShader(shader);
    return null;
  }
  return shader;
}

function setupGl(): boolean {
  const canvas = canvasEl.value;
  if (!canvas) return false;
  try {
    gl = canvas.getContext('webgl', {
      alpha: false,
      antialias: false,
      depth: false,
      stencil: false,
      powerPreference: 'low-power',
      preserveDrawingBuffer: false,
      // 环境层是纯装饰:软件渲染(SwiftShader/基本显示适配器)直接不走 WebGL,
      // 避免全屏 fbm 落到 CPU 上把主线程拖死。
      failIfMajorPerformanceCaveat: true,
    }) as WebGLRenderingContext | null;
  } catch {
    gl = null;
  }
  if (!gl) {
    webglState.value = 'unavailable';
    return false;
  }

  const vertex = compile(gl.VERTEX_SHADER, AMBIENT_VERTEX_SHADER);
  const fragment = compile(gl.FRAGMENT_SHADER, AMBIENT_FRAGMENT_SHADER);
  if (!vertex || !fragment) {
    gl = null;
    webglState.value = 'unavailable';
    return false;
  }
  program = gl.createProgram();
  if (!program) {
    gl = null;
    webglState.value = 'unavailable';
    return false;
  }
  gl.attachShader(program, vertex);
  gl.attachShader(program, fragment);
  gl.linkProgram(program);
  gl.deleteShader(vertex);
  gl.deleteShader(fragment);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    console.warn('[AmbientField] program 链接失败:', gl.getProgramInfoLog(program));
    gl.deleteProgram(program);
    program = null;
    gl = null;
    webglState.value = 'unavailable';
    return false;
  }

  buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const pos = gl.getAttribLocation(program, 'a_pos');
  gl.enableVertexAttribArray(pos);
  gl.vertexAttribPointer(pos, 2, gl.FLOAT, false, 0, 0);

  uniforms = {
    u_res: gl.getUniformLocation(program, 'u_res'),
    u_time: gl.getUniformLocation(program, 'u_time'),
    u_paper: gl.getUniformLocation(program, 'u_paper'),
    u_rose: gl.getUniformLocation(program, 'u_rose'),
    u_blue: gl.getUniformLocation(program, 'u_blue'),
    u_red: gl.getUniformLocation(program, 'u_red'),
    u_ink: gl.getUniformLocation(program, 'u_ink'),
    u_intensity: gl.getUniformLocation(program, 'u_intensity'),
    u_variant: gl.getUniformLocation(program, 'u_variant'),
    u_quality: gl.getUniformLocation(program, 'u_quality'),
  };
  gl.useProgram(program);
  gl.disable(gl.DEPTH_TEST);
  webglState.value = 'ready';
  return true;
}

function applyPalette(): void {
  if (!gl) return;
  palette = readAmbientPalette(document.documentElement);
  gl.uniform3fv(uniforms.u_paper, rgbToVec3(palette.paper));
  gl.uniform3fv(uniforms.u_rose, rgbToVec3(palette.rose));
  gl.uniform3fv(uniforms.u_blue, rgbToVec3(palette.blue));
  gl.uniform3fv(uniforms.u_red, rgbToVec3(palette.red));
  gl.uniform3fv(uniforms.u_ink, rgbToVec3(palette.ink));
  gl.uniform1f(uniforms.u_intensity, props.intensity);
  gl.uniform1i(uniforms.u_variant, variantIndex(currentVariant));
  gl.uniform1i(uniforms.u_quality, currentQuality);
}

function draw(seconds: number): void {
  const canvas = canvasEl.value;
  if (!gl || !canvas) return;
  gl.viewport(0, 0, canvas.width, canvas.height);
  gl.uniform2f(uniforms.u_res, canvas.width, canvas.height);
  gl.uniform1f(uniforms.u_time, seconds);
  gl.drawArrays(gl.TRIANGLES, 0, 3);
  if (!ready.value) ready.value = true;
}

/**
 * 面板宽度过渡(侧栏/Agent 面板 150ms)会连续触发 ResizeObserver;每次回调都重分配
 * WebGL drawing buffer 会造成开合瞬间的抖动。这里把非首次的尺寸变更合并到 120ms 后
 * 一次应用,过渡期间由 CSS 拉伸旧画布(环境层是低频背景,拉伸不可见)。
 */
function resize(immediate = false): void {
  if (resizeTimer !== null) {
    if (!immediate) return;
    clearTimeout(resizeTimer);
    resizeTimer = null;
  }
  const canvas = canvasEl.value;
  if (immediate || !gl || !canvas || canvas.width === 0 || canvas.height === 0) {
    applyResize();
    return;
  }
  resizeTimer = setTimeout(() => {
    resizeTimer = null;
    applyResize();
  }, 120);
}

function applyResize(): void {
  const root = rootEl.value;
  const canvas = canvasEl.value;
  if (!root || !canvas) return;
  const rect = root.getBoundingClientRect();
  const cfg = resolveAmbientConfig({
    width: rect.width,
    height: rect.height,
    dpr: window.devicePixelRatio || 1,
    reducedMotion: reducedMotion(),
    webgl: webglState.value !== 'unavailable',
    visible,
    paused: props.paused || pageHidden,
    hardwareConcurrency: navigator.hardwareConcurrency,
    lowTier: document.documentElement.dataset.perfTier === 'low',
  });
  staticOnly.value = cfg.staticOnly;
  if (!cfg.enabled) {
    cancelLoop();
    ready.value = false;
    return;
  }
  if (!gl && !setupGl()) {
    cancelLoop();
    return;
  }
  if (canvas.width !== cfg.width || canvas.height !== cfg.height) {
    canvas.width = cfg.width;
    canvas.height = cfg.height;
  }
  currentVariant = variantNow();
  currentQuality = cfg.quality;
  gl?.useProgram(program);
  applyPalette();
  if (cfg.animate) {
    startLoop();
  } else {
    cancelLoop();
    draw(0);
  }
}

function shouldRun(): boolean {
  return shouldAnimate(
    { enabled: webglState.value === 'ready', staticOnly: staticOnly.value },
    { visible, hidden: pageHidden, paused: props.paused },
  );
}

function startLoop(): void {
  if (rafId !== null) return;
  startedAt = performance.now() - lastFrameAt;
  const tick = (now: number): void => {
    rafId = null;
    if (!shouldRun()) return;
    if (now - lastFrameAt < 1000 / 30) {
      rafId = requestAnimationFrame(tick);
      return;
    }
    lastFrameAt = now;
    draw((now - startedAt) / 1000);
    rafId = requestAnimationFrame(tick);
  };
  rafId = requestAnimationFrame(tick);
}

function cancelLoop(): void {
  if (rafId !== null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
}

function syncAnimation(): void {
  if (shouldRun()) {
    resize();
    startLoop();
  } else {
    cancelLoop();
    // 离屏/暂停时仍保留一张静态画面,避免回到视口瞬间空白。
    if (webglState.value === 'ready') draw(lastFrameAt / 1000);
  }
}

function onVisibilityChange(): void {
  pageHidden = document.hidden;
  syncAnimation();
}

function onContextLost(event: Event): void {
  event.preventDefault();
  cancelLoop();
  ready.value = false;
  webglState.value = 'lost';
}

function onContextRestored(): void {
  webglState.value = 'pending';
  program = null;
  buffer = null;
  gl = null;
  resize();
}

function observe(): void {
  const root = rootEl.value;
  if (!root) return;
  if (typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(() => resize());
    resizeObserver.observe(root);
  }
  if (typeof IntersectionObserver !== 'undefined') {
    intersectionObserver = new IntersectionObserver(
      (entries) => {
        visible = entries.some((entry) => entry.isIntersecting);
        syncAnimation();
      },
      { threshold: 0 },
    );
    intersectionObserver.observe(root);
  }
  motionQuery = typeof window.matchMedia === 'function'
    ? window.matchMedia('(prefers-reduced-motion: reduce)')
    : null;
  motionQuery?.addEventListener?.('change', syncAnimation);
  document.addEventListener('visibilitychange', onVisibilityChange);
  canvasEl.value?.addEventListener('webglcontextlost', onContextLost);
  canvasEl.value?.addEventListener('webglcontextrestored', onContextRestored);
  const api = window.KedaiUI;
  if (api?.onChange) {
    unsubscribeTheme = api.onChange((snapshot) => {
      themeId = snapshot.activeId;
      currentVariant = variantNow();
      if (gl) {
        gl.useProgram(program);
        applyPalette();
      }
    });
  }
  themeId = document.documentElement.dataset.theme ?? api?.getActiveTheme?.() ?? null;
}

function teardown(): void {
  cancelLoop();
  if (resizeTimer !== null) {
    clearTimeout(resizeTimer);
    resizeTimer = null;
  }
  resizeObserver?.disconnect();
  intersectionObserver?.disconnect();
  motionQuery?.removeEventListener?.('change', syncAnimation);
  document.removeEventListener('visibilitychange', onVisibilityChange);
  canvasEl.value?.removeEventListener('webglcontextlost', onContextLost);
  canvasEl.value?.removeEventListener('webglcontextrestored', onContextRestored);
  unsubscribeTheme?.();
  unsubscribeTheme = null;
  if (gl) {
    if (program) gl.deleteProgram(program);
    if (buffer) gl.deleteBuffer(buffer);
    const lose = gl.getExtension('WEBGL_lose_context');
    lose?.loseContext();
  }
  gl = null;
  program = null;
  buffer = null;
  uniforms = {};
}

onMounted(() => {
  palette = readAmbientPalette(document.documentElement);
  currentVariant = variantNow();
  observe();
  resize();
});

onBeforeUnmount(teardown);

watch(
  () => props.paused,
  () => syncAnimation(),
);
watch(
  () => props.variant,
  () => {
    currentVariant = variantNow();
    if (gl) applyPalette();
    resize();
  },
);
watch(
  () => props.intensity,
  (value) => {
    if (gl) gl.uniform1f(uniforms.u_intensity, value);
  },
);
</script>

<template>
  <div
    ref="rootEl"
    class="sv-ambient"
    :data-variant="variant"
    :data-webgl="webglState"
    :class="{ 'is-ready': ready, 'is-static': staticOnly }"
    aria-hidden="true"
  >
    <canvas ref="canvasEl" class="sv-ambient-canvas"></canvas>
  </div>
</template>

<style scoped>
/* 静态兜底:CSS 渐变在 shader 就绪前/不可用时承担同一氛围;布局与尺寸完全一致。 */
.sv-ambient {
  position: absolute;
  inset: 0;
  z-index: 0;
  overflow: hidden;
  pointer-events: none;
  background:
    radial-gradient(70% 55% at 78% 12%, color-mix(in srgb, var(--sv-pink) 14%, transparent), transparent 72%),
    radial-gradient(60% 50% at 16% 88%, color-mix(in srgb, var(--sv-blue) 5%, transparent), transparent 75%),
    var(--sv-paper);
  mask-image: linear-gradient(to bottom, #000 0%, #000 74%, transparent 100%);
}
.sv-ambient[data-variant='draft'] {
  background:
    linear-gradient(to right, color-mix(in srgb, var(--sv-ink) 4%, transparent) 1px, transparent 1px),
    linear-gradient(to bottom, color-mix(in srgb, var(--sv-ink) 4%, transparent) 1px, transparent 1px),
    radial-gradient(60% 55% at 82% 10%, color-mix(in srgb, var(--sv-red) 5%, transparent), transparent 72%),
    var(--sv-paper);
  background-size: 24px 24px, 24px 24px, auto, auto;
}
.sv-ambient-canvas {
  display: block;
  width: 100%;
  height: 100%;
  opacity: 0;
  transition: opacity var(--dur-slower) var(--ease-standard);
}
.sv-ambient.is-ready .sv-ambient-canvas {
  opacity: 1;
}
.sv-ambient.is-static .sv-ambient-canvas {
  transition: none;
}
@media (prefers-reduced-motion: reduce) {
  .sv-ambient-canvas { transition: none; }
}
@media print {
  .sv-ambient { display: none; }
}
</style>
