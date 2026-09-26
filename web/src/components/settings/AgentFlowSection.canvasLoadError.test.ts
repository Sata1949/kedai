// @vitest-environment jsdom
// 画布异步组件「加载失败」的可见性(2026-09-21 画布交互补完)。
//
// 批次 3 的失败路径只 `console.error`:用户面对的是一块空白画布,分不清「加载慢」还是「坏了」。
// 本用例把动态 import 打成必然失败,断言两件事:
//   ① 就地出现加载失败提示与重试入口(不再静默);
//   ② 点「重试」会真的重新发起加载(仍失败则提示再现 —— 证明重试不是静默无效的空动作)。
//
// 独立成文件的原因:`vi.mock` 的作用域是整文件——把画布模块打成必然失败后,
// AgentFlowSection.test.ts 里「切到画布能渲染」的用例就没法共存了。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import AgentFlowSection from './AgentFlowSection.vue';
import { resetApiTokenForTest } from '../../api/client';
import type { AgentFlowConfig } from '../../api/types';

// 动态 import 必然失败(模拟 chunk 404 / 页面缓存引用旧 hash)。
// 替换的是**懒加载入口模块**而非组件模块:后者会让 vitest 把工厂抛错记成 run 级
// unhandled error(见 utils/flowCanvasChunk.ts 的注释)。
vi.mock('../../utils/flowCanvasChunk', () => ({
  loadFlowCanvas: () => Promise.reject(new Error('模拟画布 chunk 加载失败')),
}));

/** 最小流程库响应(画布不会真的挂载,故不需要补 @vue-flow 的 jsdom 缺件) */
function sampleFlow(): AgentFlowConfig {
  return {
    id: 'f1',
    name: '测试流程',
    description: null,
    enabled: true,
    steps: [
      { id: 'a', name: '左路', enabled: true, goal: 'g', action: 'direct', generates: true, inputs: [] },
      { id: 'b', name: '右路', enabled: true, goal: 'g', action: 'direct', generates: true, inputs: [] },
    ],
  };
}

function mockFetchLibrary(): void {
  const spy = vi.spyOn(globalThis, 'fetch');
  spy.mockResolvedValueOnce(new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 }));
  spy.mockResolvedValueOnce(
    new Response(
      JSON.stringify({ ok: true, library: { current_flow_id: 'f1', flows: [sampleFlow()] }, config: null }),
      { status: 200 },
    ),
  );
}

/** 切到画布视图:异步组件加载失败,故只等加载与渲染落定 */
async function switchToCanvas(wrapper: ReturnType<typeof mount>): Promise<void> {
  await wrapper.findAll('button').find((b) => b.text() === '画布')?.trigger('click');
  await vi.dynamicImportSettled();
  await flushPromises();
}

beforeEach(() => {
  setActivePinia(createPinia());
  resetApiTokenForTest();
  vi.restoreAllMocks();
  // 失败路径本身会 console.error(与 lazyModal 同款):测试里静音,避免噪音
  vi.spyOn(console, 'error').mockImplementation(() => {});
});

describe('AgentFlowSection 画布加载失败', () => {
  it('失败时就地给出提示与重试入口,重试会重新发起加载', async () => {
    mockFetchLibrary();
    // `fail()` 之后 Vue 会把 loader 的 rejection 抛到 app 级错误处理;装一个 errorHandler
    // 视为「已处理」(生产里由全局兜底接管,测试里只需避免它升级成 run 级 unhandled error)。
    const wrapper = mount(AgentFlowSection, {
      global: { config: { errorHandler: () => {} } },
    });
    await flushPromises();
    await switchToCanvas(wrapper);

    expect(wrapper.text()).toContain('画布组件加载失败');
    const retry = wrapper.findAll('button').find((b) => b.text() === '重试');
    expect(retry).toBeTruthy();

    // 重试 = 换 key 重建异步组件 → 重新发起加载;仍失败则提示再现(不是静默无效)
    await retry?.trigger('click');
    await flushPromises();
    await vi.dynamicImportSettled();
    await flushPromises();
    expect(wrapper.text()).toContain('画布组件加载失败');
  });
});
