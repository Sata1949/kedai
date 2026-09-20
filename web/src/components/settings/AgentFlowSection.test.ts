// @vitest-environment jsdom
// 执行流程编辑区的视图切换测试(二维批次 3 前端)。
//
// 覆盖三件事:
//   1. **默认是列表视图**(WF-10:线性列表保持默认,画布只是高级模式);
//   2. 切到画布能渲染出来(画布是异步组件:懒加载 + 真实 @vue-flow 挂载);
//   3. 步骤编辑表单抽成共用组件后,列表视图的展开编辑**没有回归**
//      (上游勾选 / 成果标注 / 工具三态都还在,且写回同一份草稿)。
// 图算法与画布交互分别由 utils/agentFlowGraph.test.ts、useFlowCanvas.test.ts、
// AgentFlowCanvas.test.ts 覆盖,此处只验证接线。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import AgentFlowSection from './AgentFlowSection.vue';
import { resetApiTokenForTest } from '../../api/client';
import type { AgentFlowConfig } from '../../api/types';

// 画布为异步组件且真实挂载 @vue-flow:补 jsdom 缺的两个 API(见 AgentFlowCanvas.test.ts)
class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
vi.stubGlobal('ResizeObserver', ResizeObserverStub);
vi.stubGlobal(
  'DOMMatrixReadOnly',
  class {
    m22 = 1;
  },
);
Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, value: 800 });
Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, value: 460 });

/** 二维流程:两源节点 → 合并(显式成果节点) */
function sampleFlow(): AgentFlowConfig {
  return {
    id: 'f1',
    name: '测试流程',
    description: null,
    enabled: true,
    steps: [
      { id: 'a', name: '左路', enabled: true, goal: 'g', action: 'direct', generates: true, inputs: [] },
      { id: 'b', name: '右路', enabled: true, goal: 'g', action: 'direct', generates: true, inputs: [] },
      {
        id: 'c',
        name: '合并',
        enabled: true,
        goal: 'g',
        action: 'direct',
        generates: true,
        inputs: ['a', 'b'],
        is_output: true,
      },
    ],
    max_parallel_nodes: 2,
  };
}

/** 第一跳 bootstrap 取 token,第二跳返回流程库 */
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

async function mountSection() {
  mockFetchLibrary();
  const wrapper = mount(AgentFlowSection);
  await flushPromises();
  return wrapper;
}

/** 切到指定视图(画布是异步组件:要等动态 import 落定 + 渲染) */
async function switchView(wrapper: Awaited<ReturnType<typeof mountSection>>, label: string) {
  await wrapper.findAll('button').find((b) => b.text() === label)?.trigger('click');
  await vi.dynamicImportSettled();
  await flushPromises();
}

/** 打开第 n 个步骤的展开编辑区(✎ 按钮) */
async function expandStep(wrapper: Awaited<ReturnType<typeof mountSection>>, n: number) {
  await wrapper.findAll('button[title="编辑详情"]')[n].trigger('click');
  await flushPromises();
}

beforeEach(() => {
  setActivePinia(createPinia());
  resetApiTokenForTest();
  vi.restoreAllMocks();
});

describe('AgentFlowSection 视图切换', () => {
  it('默认是列表视图:步骤行与展开编辑区都在,画布不渲染', async () => {
    const wrapper = await mountSection();
    expect(wrapper.text()).toContain('测试流程');
    // 列表行:拖拽手柄 + 展开编辑入口
    expect(wrapper.findAll('.flow-row')).toHaveLength(3);
    expect(wrapper.find('.flow-canvas-graph').exists()).toBe(false);
    // 二维流程的依赖摘要(列表视图的既有能力)
    expect(wrapper.text()).toContain('上游:左路、右路');
  });

  it('切到画布:异步加载画布并渲染出节点卡片,列表行让位', async () => {
    const wrapper = await mountSection();
    await switchView(wrapper, '画布');

    expect(wrapper.find('.flow-canvas-graph').exists()).toBe(true);
    expect(wrapper.findAll('.flow-row')).toHaveLength(0);
    // 节点卡片渲染出来了(三个步骤名)
    const text = wrapper.text();
    for (const name of ['左路', '右路', '合并']) {
      expect(text).toContain(name);
    }
  });

  it('切回列表仍是列表(视图切换不互斥丢失)', async () => {
    const wrapper = await mountSection();
    await switchView(wrapper, '画布');
    expect(wrapper.find('.flow-canvas-graph').exists()).toBe(true);
    await switchView(wrapper, '列表');
    expect(wrapper.findAll('.flow-row')).toHaveLength(3);
    expect(wrapper.find('.flow-canvas-graph').exists()).toBe(false);
  });
});

describe('AgentFlowSection 列表编辑未因抽取共用组件而回归', () => {
  it('展开编辑区仍有上游勾选与成果标注,取消勾选写回草稿', async () => {
    const wrapper = await mountSection();
    await expandStep(wrapper, 2); // 合并步骤
    expect(wrapper.text()).toContain('上游步骤');
    expect(wrapper.text()).toContain('本步产出即成果');

    // 上游勾选:取消「左路」,inputs 由 ['a','b'] 变 ['b']
    const boxes = wrapper.findAll('.flow-upstream-item input[type="checkbox"]');
    await boxes[0].trigger('change');
    await flushPromises();
    expect(wrapper.text()).toContain('上游:右路');
  });

  it('展开编辑区仍有工具三态与系统提示词(direct 步骤)', async () => {
    const wrapper = await mountSection();
    await expandStep(wrapper, 0);
    const text = wrapper.text();
    expect(text).toContain('系统提示词');
    expect(text).toContain('不使用工具');
    expect(text).toContain('工具策略');
  });

  it('反思步骤不显示生成类字段(动作切换的归一化仍在列表视图生效)', async () => {
    const wrapper = await mountSection();
    await expandStep(wrapper, 0);
    // 断言只看展开编辑区:流程级说明文案里也含「系统提示词」四个字
    const editor = () => wrapper.find('.flow-edit');
    expect(editor().text()).toContain('系统提示词');

    const actionSelect = wrapper.findAll('.flow-row select')[0];
    await actionSelect.setValue('reflect');
    await flushPromises();
    expect(editor().text()).not.toContain('系统提示词');
  });
});
