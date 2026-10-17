// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';
import AmbientField from './AmbientField.vue';

function mockRect(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width: 800,
    height: 600,
    top: 0,
    left: 0,
    right: 800,
    bottom: 600,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect);
}

function fakeGl(): WebGLRenderingContext {
  const gl: Record<string, unknown> = {};
  const constants = [
    'VERTEX_SHADER',
    'FRAGMENT_SHADER',
    'COMPILE_STATUS',
    'LINK_STATUS',
    'ARRAY_BUFFER',
    'STATIC_DRAW',
    'FLOAT',
    'TRIANGLES',
    'DEPTH_TEST',
  ];
  constants.forEach((key, index) => { gl[key] = index + 1; });
  Object.assign(gl, {
    createShader: () => ({}),
    shaderSource: () => undefined,
    compileShader: () => undefined,
    getShaderParameter: () => true,
    getShaderInfoLog: () => '',
    deleteShader: () => undefined,
    createProgram: () => ({}),
    attachShader: () => undefined,
    linkProgram: () => undefined,
    getProgramParameter: () => true,
    getProgramInfoLog: () => '',
    deleteProgram: () => undefined,
    createBuffer: () => ({}),
    bindBuffer: () => undefined,
    bufferData: () => undefined,
    getAttribLocation: () => 0,
    enableVertexAttribArray: () => undefined,
    vertexAttribPointer: () => undefined,
    getUniformLocation: () => ({}),
    useProgram: () => undefined,
    disable: () => undefined,
    viewport: () => undefined,
    uniform2f: () => undefined,
    uniform1f: () => undefined,
    uniform1i: () => undefined,
    uniform3fv: () => undefined,
    drawArrays: () => undefined,
    deleteBuffer: () => undefined,
    getExtension: () => null,
  });
  return gl as unknown as WebGLRenderingContext;
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  delete (window as unknown as Record<string, unknown>).KedaiUI;
});

describe('AmbientField 降级链', () => {
  it('无 WebGL 时只渲染 CSS 兜底,不抛错', async () => {
    mockRect();
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null as never);

    const wrapper = mount(AmbientField, { props: { variant: 'paper', intensity: 0.8 } });
    await nextTick();

    expect(wrapper.find('.sv-ambient').exists()).toBe(true);
    expect(wrapper.find('.sv-ambient').attributes('data-webgl')).toBe('unavailable');
    expect(wrapper.find('canvas').exists()).toBe(true);
    wrapper.unmount();
  });

  it('减少动态效果时只画静态首帧,WebGL 上下文就绪后标记 ready', async () => {
    mockRect();
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(fakeGl() as never);
    vi.stubGlobal('matchMedia', vi.fn(() => ({
      matches: true,
      media: '(prefers-reduced-motion: reduce)',
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })));

    const wrapper = mount(AmbientField, { props: { variant: 'draft', intensity: 0.6 } });
    await nextTick();

    expect(wrapper.find('.sv-ambient').attributes('data-webgl')).toBe('ready');
    expect(wrapper.find('.sv-ambient').classes()).toContain('is-static');
    const canvas = wrapper.find('canvas').element as HTMLCanvasElement;
    expect(canvas.width).toBeGreaterThan(0);
    expect(canvas.height).toBeGreaterThan(0);
    wrapper.unmount();
  });
});
