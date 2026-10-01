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
import type { AgentFlowConfig, ConnectionProfile } from '../../api/types';

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

// 导出落盘在 jsdom 里无处可去(URL.createObjectURL 未实现);本批次只断言「导了什么」,
// 故把落盘换成捕获桩(与 useAgentFlow.test.ts 同款)。
vi.mock('../../exportFile', () => ({ downloadBlob: vi.fn(), saveExportFile: vi.fn() }));

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

/**
 * 第一跳 bootstrap 取 token,第二跳返回流程库,第三跳返回设置(节点级连接候选从那里来)。
 * 三个都给出确定响应:否则第三条 fetch 会落到真实网络(测试环境无服务端 → 抛错噪声)。
 */
function mockFetchLibrary(connections: ConnectionProfile[] = [], flows?: AgentFlowConfig[]): void {
  const spy = vi.spyOn(globalThis, 'fetch');
  spy.mockResolvedValueOnce(new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 }));
  spy.mockResolvedValueOnce(
    new Response(
      JSON.stringify({
        ok: true,
        library: { current_flow_id: 'f1', flows: flows ?? [sampleFlow()] },
        config: null,
      }),
      { status: 200 },
    ),
  );
  spy.mockResolvedValue(
    new Response(
      JSON.stringify({ connections, active_connection_id: connections[0]?.id ?? null }),
      { status: 200 },
    ),
  );
}

async function mountSection(connections: ConnectionProfile[] = [], flows?: AgentFlowConfig[]) {
  mockFetchLibrary(connections, flows);
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

// ===== 保存后继续编辑(遗留.md IFW-7① 的端到端回归)=====
// 保存链路 = PUT /agent-flows(入库并回显库)+ GET /agent-flows(重载草稿)。
// 重载会用深拷贝**整体替换**草稿;此时编辑区若还指着旧对象,第二次保存就会把
// 用户的第一处改动之外的内容按旧对象写回——即「保存后继续编辑会静默丢失」。
describe('AgentFlowSection 保存后继续编辑', () => {
  /** 保存链路打桩:PUT 回显入库后的库,GET 返回库(并记录每次 PUT 的请求体) */
  function mockSaveRoundTrip(lib: AgentFlowConfig[]): Array<{ config: AgentFlowConfig }> {
    const spy = vi.spyOn(globalThis, 'fetch');
    const puts: Array<{ config: AgentFlowConfig }> = [];
    spy.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes('/agent-flows') && init?.method === 'PUT') {
        const body = JSON.parse(String(init.body)) as { config: AgentFlowConfig };
        puts.push(body);
        // 服务端按 id 覆盖入库,再把整库回显(与后端 set 的响应形状一致)
        const saved = body.config;
        const next = lib.map((f) => (f.id === saved.id ? saved : f));
        return new Response(
          JSON.stringify({ ok: true, library: { current_flow_id: saved.id ?? null, flows: next }, config: null }),
          { status: 200 },
        );
      }
      if (url.includes('/agent-flows')) {
        return new Response(
          JSON.stringify({ ok: true, library: { current_flow_id: 'f1', flows: lib }, config: null }),
          { status: 200 },
        );
      }
      // 其余(bootstrap 取 token)
      return new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 });
    });
    return puts;
  }

  function singleStepLib(): AgentFlowConfig[] {
    return [
      {
        id: 'f1',
        name: '流程一',
        description: null,
        enabled: true,
        steps: [
          { id: 'a', name: '起草', enabled: true, goal: '初始目标', action: 'direct', generates: true, inputs: [] },
        ],
      },
    ];
  }

  const saveBtn = (wrapper: ReturnType<typeof mount>) =>
    wrapper.findAll('button').find((b) => b.text() === '保存执行流程')!;

  it('保存一次后继续编辑,再保存时两次改动都在(不写回旧对象)', async () => {
    const lib = singleStepLib();
    const puts = mockSaveRoundTrip(lib);
    const wrapper = mount(AgentFlowSection);
    await flushPromises();

    await expandStep(wrapper, 0);
    const goal = () => wrapper.find('.flow-edit input.sv-input');
    await goal().setValue('第一次改');
    await flushPromises();

    await saveBtn(wrapper).trigger('click');
    await flushPromises();
    expect(puts).toHaveLength(1);
    expect(puts[0].config.steps[0].goal).toBe('第一次改');
    // 保存后编辑区仍在(editingStepId 不重置):这正是「继续编辑」的场景
    expect(wrapper.find('.flow-edit').exists()).toBe(true);

    await goal().setValue('第二次改');
    await flushPromises();
    await saveBtn(wrapper).trigger('click');
    await flushPromises();

    // 判别性断言:第二次保存的载荷必须是「第二次改」。
    // 编辑器若按值解构 props,写入会落在保存前那个已被替换的对象上,此处会读到「第一次改」
    expect(puts).toHaveLength(2);
    expect(puts[1].config.steps[0].goal).toBe('第二次改');
    wrapper.unmount();
  });
});

// ===== 节点级连接与工具轮次上限(二维批次 5b)=====

function conn(id: string, name: string, enabled = true, model = 'model-x'): ConnectionProfile {
  return {
    id,
    name,
    connector_type: 'openai-compatible',
    base_url: 'https://api.example/v1',
    model,
    api_style: 'chat-completions',
    enabled,
    api_key_masked: '****abcd',
    has_api_key: true,
  };
}

/** 单步流程:第二步引用连接,用于「连接写回草稿」「失效引用警示」与列表摘要 */
function connLib(): AgentFlowConfig[] {
  return [
    {
      id: 'f1',
      name: '连接流程',
      description: null,
      enabled: true,
      steps: [
        { id: 'a', name: '默认步', enabled: true, goal: 'g', action: 'direct', generates: true, inputs: [] },
        {
          id: 'b',
          name: '外部步',
          enabled: true,
          goal: 'g',
          action: 'direct',
          generates: true,
          inputs: ['a'],
          is_output: true,
          connection_id: 'c-b',
          max_tool_rounds: 3,
        },
      ],
    },
  ];
}

describe('AgentFlowSection 节点级连接(二维批次 5b)', () => {
  it('连接选择写回草稿,并在列表行显示连接摘要', async () => {
    const wrapper = await mountSection([conn('c-b', '外部连接', true, 'model-b')], connLib());
    // 列表行摘要:换了 provider 的节点要一眼可见(默认连接的节点不显示,避免噪音)
    expect(wrapper.text()).toContain('连接:外部连接');

    // 打开第一步(默认连接)→ 连接选择器停在「默认连接」,且示出候选
    await expandStep(wrapper, 0);
    const editor = () => wrapper.find('.flow-edit');
    const select = editor()
      .findAll('select')
      .find((sel) => sel.text().includes('默认连接'))!;
    expect((select.element as HTMLSelectElement).value).toBe('');
    expect(select.text()).toContain('外部连接');

    // 选中外部连接 → 写回草稿(下拉读回该 id),并给出「引用失效才报错」的口径说明
    await select.setValue('c-b');
    await flushPromises();
    expect((select.element as HTMLSelectElement).value).toBe('c-b');
    expect(editor().text()).toContain('本步走所选连接');
  });

  it('轮次上限写回草稿并夹取越界值;留空则清回 null(沿用全局)', async () => {
    const wrapper = await mountSection([conn('c-b', '外部连接')], connLib());
    await expandStep(wrapper, 1);
    const editor = () => wrapper.find('.flow-edit');
    const rounds = editor()
      .findAll('input[type="number"]')
      .find((i) => (i.element as HTMLInputElement).title.includes('工具轮次上限'))!;
    // 草稿里的既有值原样读回
    expect((rounds.element as HTMLInputElement).value).toBe('3');

    await rounds.setValue('9999');
    await flushPromises();
    expect((rounds.element as HTMLInputElement).value).toBe('200');

    await rounds.setValue('');
    await flushPromises();
    expect((rounds.element as HTMLInputElement).value).toBe('');
  });

  it('引用已失效/已停用的连接:选择器保位显示并给出警示(不静默改绑)', async () => {
    // 设置里只有一条**停用**的连接,流程里引用的则是已删除的 id
    const wrapper = await mountSection([conn('c-off', '停用连接', false)], connLib());
    await expandStep(wrapper, 1);
    const editor = () => wrapper.find('.flow-edit');

    // 保位:当前值不在候选里也要有锚点文案,且该占位项不可选
    expect(editor().text()).toContain('c-b(引用已失效)');
    // 警示说清运行期后果(不回退默认连接)
    expect(editor().text()).toContain('运行时会报错');
    // 停用的连接也在候选里,但标出「已停用」且不可选(disabled)
    const disabledOpt = editor()
      .findAll('option')
      .find((o) => o.text().includes('停用连接'))!;
    expect(disabledOpt.text()).toContain('(已停用)');
    expect((disabledOpt.element as HTMLOptionElement).disabled).toBe(true);
  });
});

// ==================== 流程搬运(二维批次 7a) ====================

describe('AgentFlowSection 流程搬运', () => {
  /** 按 URL 应答(导出由点击触发,不能用 mountSection 的「按调用序」mock) */
  function mockByUrl(exportBody: unknown): string[] {
    const urls: string[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input: RequestInfo | URL) => {
      const url = String(input);
      urls.push(url);
      if (url.includes('/api/bootstrap')) {
        return new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 });
      }
      if (url.includes('/agent-flows/export')) {
        return new Response(JSON.stringify(exportBody), { status: 200 });
      }
      if (url.includes('/agent-flows')) {
        return new Response(
          JSON.stringify({
            ok: true,
            library: { current_flow_id: 'f1', flows: [sampleFlow()] },
            config: null,
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({ connections: [], active_connection_id: null }), {
        status: 200,
      });
    });
    return urls;
  }

  it('导出区:两个导出按钮都在,且说明导出范围与「不含连接与密钥」', async () => {
    mockByUrl({ ok: true, bundle: { kedai_flow_bundle: 1, flows: [] } });
    const wrapper = mount(AgentFlowSection);
    await flushPromises();

    const texts = wrapper.findAll('button').map((b) => b.text());
    expect(texts).toContain('导出流程(JSON)');
    expect(texts).toContain('导出全部流程');
    // 提示须说清:带上子流程、不含连接/密钥
    expect(wrapper.text()).toContain('挂载的子流程');
    expect(wrapper.text()).toContain('不含 API 连接与密钥');
  });

  it('点「导出全部流程」:不带 id 调搬运包端点,并提示流程总数', async () => {
    const urls = mockByUrl({
      ok: true,
      bundle: { kedai_flow_bundle: 1, root_id: 'f1', flows: [sampleFlow()] },
    });
    const wrapper = mount(AgentFlowSection);
    await flushPromises();

    await wrapper.findAll('button').find((b) => b.text() === '导出全部流程')?.trigger('click');
    await flushPromises();

    const call = urls.find((u) => u.includes('/agent-flows/export'));
    expect(call, '导出必须走搬运包端点').toBe('/api/agent-flows/export');
    expect(wrapper.text()).toContain('已导出全部 1 个流程');
  });

  it('点「导出流程」:带当前流程 id 调端点,并提示带上的子流程数', async () => {
    const urls = mockByUrl({
      ok: true,
      bundle: {
        kedai_flow_bundle: 1,
        root_id: 'f1',
        flows: [sampleFlow(), { ...sampleFlow(), id: 'f2', name: '子流程' }],
      },
    });
    const wrapper = mount(AgentFlowSection);
    await flushPromises();

    await wrapper.findAll('button').find((b) => b.text() === '导出流程(JSON)')?.trigger('click');
    await flushPromises();

    expect(urls.some((u) => u.includes('/agent-flows/export?id=f1'))).toBe(true);
    expect(wrapper.text()).toContain('已导出「测试流程」及其 1 个子流程');
  });
});

// ==================== 导入「覆盖」模式(B 批 B4)====================
//
// 覆盖是**不可逆**动作(同 id 那份被替换),故这里锁两件事:默认不勾选、勾选后
// 请求体真的带 on_conflict=replace,并且导入前有二次确认(取消则一个请求都不发)。
describe('AgentFlowSection 导入覆盖模式(B 批 B4)', () => {
  /** 捕获 import 请求体;report 由用例给出 */
  function mockImport(report: unknown): Array<Record<string, unknown>> {
    const bodies: Array<Record<string, unknown>> = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        if (url.includes('/api/bootstrap')) {
          return new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 });
        }
        if (url.includes('/agent-flows/import')) {
          bodies.push(JSON.parse(String(init?.body)) as Record<string, unknown>);
          return new Response(
            JSON.stringify({
              ok: true,
              library: { current_flow_id: 'f1', flows: [sampleFlow()] },
              report,
            }),
            { status: 200 },
          );
        }
        if (url.includes('/agent-flows')) {
          return new Response(
            JSON.stringify({ ok: true, library: { current_flow_id: 'f1', flows: [sampleFlow()] }, config: null }),
            { status: 200 },
          );
        }
        return new Response(JSON.stringify({ connections: [], active_connection_id: null }), {
          status: 200,
        });
      },
    );
    return bodies;
  }

  /** 选择文件:jsdom 里 input.files 只读,故就地定义一个带 text() 的伪 File */
  async function pickFile(wrapper: ReturnType<typeof mount>, payload: unknown): Promise<void> {
    const input = wrapper.find('input[type="file"]');
    Object.defineProperty(input.element, 'files', {
      value: [{ text: async () => JSON.stringify(payload) }],
      configurable: true,
    });
    await input.trigger('change');
    await flushPromises();
  }

  it('复选框默认未勾选;此时导入请求体不带 on_conflict(现状:新增副本 + 新 id)', async () => {
    const bodies = mockImport({ imported: 1, skipped: 0, renamed: [], replaced: [] });
    const wrapper = mount(AgentFlowSection);
    await flushPromises();

    const box = wrapper.find('#flow-import-replace-box');
    expect(box.exists(), '导入按钮旁应有「覆盖同名流程」复选框').toBe(true);
    expect((box.element as HTMLInputElement).checked).toBe(false);
    expect(wrapper.text()).toContain('覆盖同名流程(同 id 内容不同时替换本机那份)');

    await pickFile(wrapper, { config: sampleFlow() });
    expect(bodies).toHaveLength(1);
    expect(bodies[0]).not.toHaveProperty('on_conflict');
    wrapper.unmount();
  });

  it('勾选后:二次确认通过才带 on_conflict=replace,报告文案点名被覆盖的流程', async () => {
    const bodies = mockImport({
      imported: 1,
      skipped: 0,
      renamed: [],
      replaced: [{ id: 'f1', name: '测试流程' }],
    });
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const wrapper = mount(AgentFlowSection);
    await flushPromises();

    await wrapper.find('#flow-import-replace-box').setValue(true);
    expect(wrapper.text()).toContain('覆盖不可撤销');

    await pickFile(wrapper, { config: sampleFlow() });
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(String(confirmSpy.mock.calls[0][0])).toContain('无法撤销');
    expect(bodies[0]).toMatchObject({ on_conflict: 'replace' });
    expect(wrapper.text()).toContain('覆盖 1 个同 id 流程');
    expect(wrapper.text()).toContain('「测试流程」');
    wrapper.unmount();
  });

  it('二次确认被取消:一个请求都不发(库一字不动),也不卡在「导入中...」', async () => {
    const bodies = mockImport({ imported: 1, skipped: 0, renamed: [], replaced: [] });
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);
    const wrapper = mount(AgentFlowSection);
    await flushPromises();

    await wrapper.find('#flow-import-replace-box').setValue(true);
    await pickFile(wrapper, { config: sampleFlow() });

    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(bodies).toEqual([]);
    expect(wrapper.findAll('button').some((b) => b.text().includes('导入中'))).toBe(false);
    wrapper.unmount();
  });
});

