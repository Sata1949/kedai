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
import type { AgentFlowStep } from '../../api/types';

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

/** 挂载画布(异步组件渲染完成后再断言) */
async function mountCanvas(steps: AgentFlowStep[]) {
  const wrapper = mount(AgentFlowCanvas, { props: { steps } });
  await flushPromises();
  return wrapper;
}

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

  it('线性流程(存量一维)进来就是一条链:节点逐层下降且有连线', async () => {
    const wrapper = await mountCanvas([step('a'), step('b'), step('c')]);
    // 节点层级按执行器视角递增
    expect(wrapper.text()).toContain('第 3 层');
    // 隐式串联边被画出来(vue-flow 的边元素)
    expect(wrapper.findAll('.vue-flow__edge').length).toBe(2);
  });
});

describe('AgentFlowCanvas 交互', () => {
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
    expect(steps[3].inputs).toEqual(['c', 'b']);
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
