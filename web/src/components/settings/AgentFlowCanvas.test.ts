// @vitest-environment jsdom
// 画布组件测试(二维批次 3 前端)。
//
// 这里是**真实挂载** @vue-flow/core(不 mock 画布库):jsdom 缺 ResizeObserver/DOMMatrix,
// 下面补最小桩后即可挂载——节点是否真的画出来、连线事件是否真的接到逻辑层,都在测试里
// 得到验证。仍不触碰像素/缩放/minimap(计划 §四 批次 3 的验收口径)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';
import { VueFlow } from '@vue-flow/core';
import AgentFlowCanvas from './AgentFlowCanvas.vue';
import type { AgentFlowConfig, AgentFlowStep } from '../../api/types';
import type { FlowConnectionOption } from '../../utils/agentFlowConnections';

// jsdom 未实现、而 Vue Flow 挂载即依赖的两个 API(缺 ResizeObserver 时一个节点都不渲染)
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
// jsdom 无布局引擎:Vue Flow 用 offsetWidth/offsetHeight 读容器与节点尺寸,不补就会一直
// 打「需要宽高才能渲染图」的告警(节点与边仍在,只是尺寸恒 0)。
Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, value: 800 });
Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, value: 460 });

function step(
  id: string,
  inputs: string[] = [],
  overrides: Partial<AgentFlowStep> = {},
): AgentFlowStep {
  return {
    id,
    name: id,
    enabled: true,
    goal: `${id} 目标`,
    action: 'direct',
    generates: true,
    inputs,
    ...overrides,
  };
}

/** 菱形:a、b 源节点 → c 合并 → d 收口 */
function diamond(): AgentFlowStep[] {
  return [step('a', [], { name: '左路' }), step('b', [], { name: '右路' }), step('c', ['a', 'b'], { name: '合并' }), step('d', ['c'], { name: '收口' })];
}

/** 挂载画布(异步组件渲染完成后再断言);flows/connections 用于两类徽标用例 */
async function mountCanvas(
  steps: AgentFlowStep[],
  flows?: AgentFlowConfig[],
  connections?: FlowConnectionOption[],
) {
  const wrapper = mount(AgentFlowCanvas, { props: { steps, flows, connections } });
  await flushPromises();
  return wrapper;
}

const CONNS: FlowConnectionOption[] = [
  { id: 'c-b', name: '外部连接', enabled: true, model: 'model-b', connectorType: 'openai-compatible' },
  { id: 'c-off', name: '停用连接', enabled: false, model: 'model-x', connectorType: 'openai-compatible' },
];

/** 触发画布库的连线事件(等价用户在两个端口之间拖出一条线) */
async function connect(wrapper: Awaited<ReturnType<typeof mountCanvas>>, source: string, target: string) {
  wrapper.findComponent(VueFlow).vm.$emit('connect', { source, target });
  await flushPromises();
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('AgentFlowCanvas 渲染', () => {
  it('把每个步骤画成节点卡片,并标出层级与成果节点', async () => {
    const steps = diamond();
    steps[3].is_output = true;
    const wrapper = await mountCanvas(steps);
    const text = wrapper.text();
    for (const name of ['左路', '右路', '合并', '收口']) {
      expect(text).toContain(name);
    }
    expect(text).toContain('第 1 层');
    expect(text).toContain('第 3 层');
    expect(text).toContain('成果');
  });

  it('反思步骤与停用步骤在卡片上有区分标记', async () => {
    const wrapper = await mountCanvas([
      step('a', [], { name: '分析', action: 'reflect', generates: undefined }),
      step('b', ['a'], { name: '停用步', enabled: false }),
    ]);
    /** 按节点定位断言:整页文本会把别的节点的徽标也带进来 */
    const cardOf = (name: string) =>
      wrapper.findAll('.vue-flow__node').find((n) => n.text().includes(name));
    const reflect = cardOf('分析');
    expect(reflect?.text()).toContain('反思');
    expect(reflect?.text()).not.toContain('生成');
    expect(cardOf('停用步')?.text()).toContain('已停用');
  });

  it('严格档节点在卡片上有档位标记(二维批次 6a)', async () => {
    const wrapper = await mountCanvas([
      step('a', [], { name: '原子步', kind: 'strict' }),
      step('b', ['a'], { name: '循环步' }),
    ]);
    /** 按节点定位断言:整页文本会把别的节点的徽标也带进来 */
    const cardOf = (name: string) =>
      wrapper.findAll('.vue-flow__node').find((n) => n.text().includes(name));
    expect(cardOf('原子步')?.text()).toContain('严格');
    expect(cardOf('循环步')?.text()).not.toContain('严格');
  });

  it('挂载子流程的节点带徽标:传库显示名字,库未传入只说「子流程」,失效才点名失效', async () => {
    const steps = [step('a', [], { name: '挂载步', sub_flow_id: 'f2' })];
    const cardText = (wrapper: Awaited<ReturnType<typeof mountCanvas>>) =>
      wrapper.findAll('.vue-flow__node').find((n) => n.text().includes('挂载步'))?.text() ?? '';

    // 库传了且命中 → 显示流程名
    const withLib = await mountCanvas(steps, [
      { id: 'f2', name: '摘要流程', enabled: true, steps: [] },
    ]);
    expect(cardText(withLib)).toContain('子流程:摘要流程');

    // 库**未传入**(还没加载/画布独立用时)→ 只说「子流程」,不得误报失效
    const noLib = await mountCanvas(steps);
    expect(cardText(noLib)).toContain('子流程');
    expect(cardText(noLib)).not.toContain('引用已失效');

    // 库传了但没有这个 id → 引用已失效
    const stale = await mountCanvas(steps, [
      { id: 'other', name: '别的流程', enabled: true, steps: [] },
    ]);
    expect(cardText(stale)).toContain('子流程(引用已失效)');
  });

  it('节点级连接的卡片徽标(二维批次 5b):显示连接名,失效/停用如实标出', async () => {
    const steps = [
      step('a', [], { name: '默认步' }),
      step('b', ['a'], { name: '外部步', connection_id: 'c-b' }),
      step('c', ['b'], { name: '失效步', connection_id: 'gone' }),
      step('d', ['c'], { name: '停用步', connection_id: 'c-off' }),
    ];
    const cardOf = (wrapper: Awaited<ReturnType<typeof mountCanvas>>, name: string) =>
      wrapper.findAll('.vue-flow__node').find((n) => n.text().includes(name))?.text() ?? '';

    const wrapper = await mountCanvas(steps, undefined, CONNS);
    // 未指定连接 = 走默认连接:不挂徽标(避免给常态加噪音)
    expect(cardOf(wrapper, '默认步')).not.toContain('连接');
    expect(cardOf(wrapper, '外部步')).toContain('连接:外部连接');
    expect(cardOf(wrapper, '失效步')).toContain('连接(引用已失效)');
    expect(cardOf(wrapper, '停用步')).toContain('连接:停用连接(已停用)');

    // 连接列表**未传入**(还没加载完/画布独立使用)→ 只说「连接」,不得误报失效
    const noList = await mountCanvas(steps);
    expect(cardOf(noList, '外部步')).toContain('连接');
    expect(cardOf(noList, '外部步')).not.toContain('引用已失效');
  });

  it('线性流程(存量一维)进来就是一条链:节点逐层下降且有连线', async () => {
    const wrapper = await mountCanvas([step('a'), step('b'), step('c')]);
    // 节点层级按执行器视角递增
    expect(wrapper.text()).toContain('第 3 层');
    // 隐式串联边被画出来(vue-flow 的边元素)
    expect(wrapper.findAll('.vue-flow__edge').length).toBe(2);
  });
});

describe('AgentFlowCanvas 交互', () => {
  it('连线中点的「×」断开该上游;线性流程的隐式边不给按钮', async () => {
    // 只留一条显式边(多条边会依赖渲染顺序定位,这里要确定性):a → c
    const steps = [step('a'), step('b'), step('c', ['a'])];
    const wrapper = await mountCanvas(steps);
    const buttons = wrapper.findAll('.flow-edge-del');
    expect(buttons).toHaveLength(1);
    await buttons[0].trigger('click');
    await flushPromises();
    expect(steps[2].inputs).toEqual([]);
    expect(wrapper.text()).toContain('已断开');

    // 线性流程只有隐式串联边:没有可断开的 inputs 记录 → 不画按钮
    const linear = await mountCanvas([step('a'), step('b')]);
    expect(linear.findAll('.vue-flow__edge')).toHaveLength(1);
    expect(linear.findAll('.flow-edge-del')).toHaveLength(0);
  });

  it('点击节点在右侧打开 Inspector(逐字段编辑同一份草稿)', async () => {
    const steps = diamond();
    const wrapper = await mountCanvas(steps);
    expect(wrapper.text()).not.toContain('上游步骤');

    wrapper.findComponent(VueFlow).vm.$emit('nodeClick', { node: { id: 'c' } });
    await flushPromises();

    const text = wrapper.text();
    expect(text).toContain('上游步骤');
    expect(text).toContain('最终成果');
    expect(text).toContain('删除步骤');
  });

  it('合法连线写入下游步骤的上游', async () => {
    const steps = diamond();
    const wrapper = await mountCanvas(steps);
    await connect(wrapper, 'b', 'd');
    // 归一化为流程数组下标序:b(下标 1)排在 c(下标 2)前,与后端多父合并顺序一致
    expect(steps[3].inputs).toEqual(['b', 'c']);
    expect(wrapper.text()).toContain('已连接');
  });

  it('成环连线当场被拒并给中文原因,不写入草稿', async () => {
    const steps = diamond();
    const wrapper = await mountCanvas(steps);
    await connect(wrapper, 'd', 'a');
    expect(steps[0].inputs).toEqual([]);
    expect(wrapper.text()).toContain('会形成环');
  });

  it('重复上游被拒', async () => {
    const steps = diamond();
    const wrapper = await mountCanvas(steps);
    await connect(wrapper, 'a', 'c');
    expect(steps[2].inputs).toEqual(['a', 'b']);
    expect(wrapper.text()).toContain('已经有');
  });

  it('拖拽结束把坐标写回步骤', async () => {
    const steps = diamond();
    const wrapper = await mountCanvas(steps);
    wrapper.findComponent(VueFlow).vm.$emit('nodeDragStop', {
      node: { id: 'a', computedPosition: { x: 40, y: 60 } },
    });
    await flushPromises();
    expect(steps[0].x).toBe(40);
    expect(steps[0].y).toBe(60);
    expect(wrapper.text()).toContain('保存');
  });

  it('重新布局:无手工坐标时不打扰用户,直接按层级重排', async () => {
    const steps = diamond();
    const confirm = vi.spyOn(window, 'confirm');
    const wrapper = await mountCanvas(steps);
    await wrapper.find('button[title="按依赖层级重新排列节点"]').trigger('click');
    expect(confirm).not.toHaveBeenCalled();
    expect(steps.every((s) => typeof s.y === 'number')).toBe(true);
  });

  it('重新布局:已有手工坐标时先二次确认,取消则不动坐标', async () => {
    const steps = diamond();
    steps[0].x = 7;
    steps[0].y = 8;
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    const wrapper = await mountCanvas(steps);
    await wrapper.find('button[title="按依赖层级重新排列节点"]').trigger('click');
    expect(confirm).toHaveBeenCalled();
    expect(steps[0].y).toBe(8);
  });

  it('删除步骤经事件上抛给调用方(连带清理悬空上游引用归调用方)', async () => {
    const steps = diamond();
    const wrapper = await mountCanvas(steps);
    wrapper.findComponent(VueFlow).vm.$emit('nodeClick', { node: { id: 'c' } });
    await flushPromises();
    await wrapper.find('button[title="删除该步骤"]').trigger('click');
    expect(wrapper.emitted('remove')).toEqual([['c']]);
  });
});
