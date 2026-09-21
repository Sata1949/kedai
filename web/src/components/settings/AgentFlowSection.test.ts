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

  it('档位选择:切到严格写回草稿并收起工具区,切回宽松恢复', async () => {
    const wrapper = await mountSection();
    await expandStep(wrapper, 0);
    const editor = () => wrapper.find('.flow-edit');
    // 宽松(缺省):工具区可见
    expect(editor().text()).toContain('工具策略');

    // 切到严格:档位写回草稿(下拉读回 strict),工具区让位给档位说明
    const kindSelect = editor()
      .findAll('select')
      .find((s) => s.text().includes('严格'))!;
    await kindSelect.setValue('strict');
    await flushPromises();
    expect((kindSelect.element as HTMLSelectElement).value).toBe('strict');
    expect(editor().text()).not.toContain('工具策略');
    expect(editor().text()).toContain('严格档:一次模型调用即完成');

    // 切回宽松:工具区恢复(说明草稿里的工具配置没有被清掉)
    await kindSelect.setValue('loose');
    await flushPromises();
    expect(editor().text()).toContain('工具策略');
  });
});

// ===== 静态子图(二维批次 6b)=====

/** 流程库:当前流程 f1(单步,带工具与提示词)+ 可被引用的子流程 f2 */
function subFlowLib(): AgentFlowConfig[] {
  return [
    {
      id: 'f1',
      name: '主流程',
      description: null,
      enabled: true,
      steps: [
        {
          id: 'a',
          name: '起草',
          enabled: true,
          goal: 'g',
          action: 'direct',
          generates: true,
          inputs: [],
          tools: ['read'],
          tool_choice: 'auto',
          system_prompt: '本步指令',
        },
      ],
    },
    {
      id: 'f2',
      name: '摘要流程',
      description: null,
      enabled: true,
      steps: [{ id: 'x', name: '压缩', enabled: true, goal: 'g', action: 'direct', generates: true, inputs: [] }],
    },
  ];
}

async function mountSubFlowSection(lib: AgentFlowConfig[] = subFlowLib()) {
  const spy = vi.spyOn(globalThis, 'fetch');
  spy.mockResolvedValueOnce(new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 }));
  spy.mockResolvedValueOnce(
    new Response(
      JSON.stringify({ ok: true, library: { current_flow_id: 'f1', flows: lib }, config: null }),
      { status: 200 },
    ),
  );
  const wrapper = mount(AgentFlowSection);
  await flushPromises();
  return wrapper;
}

/** 二维流程库:主流程 a → b(两步有显式上游)+ 可被引用的子流程 f2 */
function subFlow2dLib(): AgentFlowConfig[] {
  const lib = subFlowLib();
  lib[0].steps = [
    { ...lib[0].steps[0], id: 'a', name: '起草', inputs: [] },
    { ...lib[0].steps[0], id: 'b', name: '扩写', inputs: ['a'], sub_flow_id: 'f2' },
  ];
  return lib;
}

describe('AgentFlowSection 静态子图(二维批次 6b)', () => {
  it('挂载子流程写回草稿、收起被旁路的执行参数,清空后恢复', async () => {
    const wrapper = await mountSubFlowSection();
    await wrapper.findAll('button[title="编辑详情"]')[0].trigger('click');
    await flushPromises();
    const editor = () => wrapper.find('.flow-edit');

    // 未挂载:子流程选择器在场,执行参数(档位/提示词/工具)都在
    expect(editor().text()).toContain('不挂载');
    expect(editor().text()).toContain('系统提示词');
    expect(editor().text()).toContain('工具策略');
    expect(editor().findAll('select').length).toBeGreaterThan(1);

    // 挂载:子流程选择器写回草稿(下拉读回 f2),被旁路的执行参数整体让位
    const subSelect = editor()
      .findAll('select')
      .find((s) => s.text().includes('不挂载'))!;
    await subSelect.setValue('f2');
    await flushPromises();
    expect((subSelect.element as HTMLSelectElement).value).toBe('f2');
    expect(editor().text()).toContain('本节点已挂载子流程「摘要流程」');
    expect(editor().text()).toContain('清空子流程即恢复生效');
    expect(editor().text()).not.toContain('系统提示词');
    expect(editor().text()).not.toContain('工具策略');
    // 挂载后被旁路的执行参数整块收起(按**字段名**断言:数 select 个数是脆的实现细节,
    // 用户看到的是这些字段名。注意别用「档位/工具/提示词」这些词——它们会命中旁路说明
    // 那句「目标/动作/档位/工具/提示词/温度都不参与执行」)
    for (const label of ['系统提示词', '工具策略', '并行调用', '输出上限']) {
      expect(editor().text()).not.toContain(label);
    }
    // 子流程下拉仍在(挂载状态要能改回去)
    expect(editor().text()).toContain('不挂载');

    // 清空:执行参数恢复(说明草稿里的配置没有被清掉)
    await subSelect.setValue('');
    await flushPromises();
    expect(editor().text()).toContain('系统提示词');
    expect(editor().text()).toContain('工具策略');
  });

  it('列表行摘要显示挂载关系(线性流程也给提示)', async () => {
    const wrapper = await mountSubFlowSection();
    // 线性单步流程原本没有任何二维摘要;挂载子流程后要有可读的摘要
    expect(wrapper.text()).not.toContain('子流程:');
    await wrapper.findAll('button[title="编辑详情"]')[0].trigger('click');
    await flushPromises();
    const subSelect = wrapper
      .find('.flow-edit')
      .findAll('select')
      .find((s) => s.text().includes('不挂载'))!;
    await subSelect.setValue('f2');
    await flushPromises();
    expect(wrapper.find('.flow-row').text()).toContain('子流程:摘要流程');
  });

  it('二维流程的行摘要:层级/上游之后接「· 子流程:X」', async () => {
    const wrapper = await mountSubFlowSection(subFlow2dLib());
    const rows = wrapper.findAll('.flow-row');
    // 挂载点在下标 1(扩写):二维摘要里要能同时读到上游与子流程
    const summary = rows[1].find('.flow-graph-note').text();
    expect(summary).toContain('上游:起草');
    expect(summary).toContain('· 子流程:摘要流程');
  });

  it('引用失效的行摘要点名「引用已失效」(与画布徽标/编辑器下拉同文案)', async () => {
    const lib = subFlow2dLib();
    lib[0].steps[1].sub_flow_id = 'gone';
    const wrapper = await mountSubFlowSection(lib);
    const summary = wrapper.findAll('.flow-row')[1].find('.flow-graph-note').text();
    expect(summary).toContain('子流程:gone(引用已失效)');
  });
});
