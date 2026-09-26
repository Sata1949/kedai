// @vitest-environment jsdom
// 任务**改绑流程**(B 批 B3)的交互测试。
//
// 为什么单独成文件:`vi.mock` 的作用域是整文件——本文件要给 api.bindTask 装可控桩
// (成功 / 400 两种应答),而同目录 TaskBoard.test.ts 的 mock 是「全部成功」的静态形状
// (先例:AgentFlowSection.canvasLoadError.test.ts 的拆文件理由)。
//
// 覆盖:入口门控(非 custom / 进行中 / 待批准都不给入口)、点击展开内联入选区、
// 保存时的**全量下发**(flow_id: null 与 flow_ids: [] 都必须出现)、成功后详情与列表
// 就地更新、400 文案透出、二次确认取消与「取消」按钮都不发请求。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';

// node/jsdom 无 localStorage:子 store 初始化即访问,补内存桩(与 TaskBoard.test.ts 同款)
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

/** mock 控制句柄(vi.hoisted 保证工厂内可安全引用) */
const h = vi.hoisted(() => ({
  /** bindTask 收到的参数序列(全量下发断言用) */
  bindCalls: [] as Array<{ id: string; flowId: string | null; flowIds: string[] }>,
  /** 非空时 bindTask 抛它(模拟后端 400) */
  bindFail: null as Error | null,
}));

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    // 库未加载时的惰性拉取:测试里显式播种库,故这里兜底,避免真实请求
    getAgentFlow: vi.fn().mockResolvedValue(null),
    listTasks: vi.fn().mockResolvedValue([]),
    getTask: vi.fn().mockResolvedValue(null),
    bindTask: vi.fn(async (id: string, flowId: string | null, flowIds: string[]) => {
      h.bindCalls.push({ id, flowId, flowIds });
      if (h.bindFail) throw h.bindFail;
      // 后端回执 = 改绑后的任务行(调用方据此就地更新列表与详情)
      return {
        id,
        title: '测试任务目标',
        status: 'pending' as const,
        plan: [],
        result: '',
        error: '',
        created_at: '2026-09-24T00:00:00Z',
        updated_at: '2026-09-24T00:00:00Z',
        task_mode: 'custom' as const,
        flow_id: flowId,
        flow_ids: flowIds,
      };
    }),
  };
});

import { useAppStore } from '../store';
import type { AgentFlowLibrary, TaskDetail } from '../api';
import TaskBoard from './TaskBoard.vue';

/** 流程库:当前流程 f1 + 可调用候选 f2 / 停用 f3 */
const library: AgentFlowLibrary = {
  current_flow_id: 'f1',
  flows: [
    { id: 'f1', name: '主流程', enabled: true, steps: [] },
    { id: 'f2', name: '备用流程', enabled: true, steps: [] },
    { id: 'f3', name: '停用流程', enabled: false, steps: [] },
  ],
};

/** 构造最小可用的任务详情(仅本测试断言所需字段) */
function makeTaskDetail(overrides: Partial<TaskDetail['task']> = {}): TaskDetail {
  return {
    task: {
      id: 't1',
      title: '测试任务目标',
      status: 'pending',
      plan: [],
      result: '',
      error: '',
      character_id: null,
      created_at: '2026-09-24T00:00:00Z',
      updated_at: '2026-09-24T00:00:00Z',
      task_mode: 'custom',
      flow_id: 'f1',
      flow_ids: ['f2'],
      ...overrides,
    },
    subtasks: [],
    usage_total: { prompt_tokens: 0, completion_tokens: 0, reasoning_tokens: 0 },
    messages: [],
  };
}

/** 挂载看台:播种当前任务 + 流程库(库已播种 → 组件不再惰性拉库) */
function mountBoard(
  overrides: Partial<TaskDetail['task']> = {},
  opts: { withSnapshot?: boolean } = {},
) {
  const pinia = createPinia();
  setActivePinia(pinia);
  const store = useAppStore();
  store.currentTaskId = 't1';
  store.currentTask = makeTaskDetail(overrides);
  if (opts.withSnapshot) {
    store.currentTask.flow_snapshot = {
      root_id: 'f1',
      flows: [{ id: 'f1', name: '主流程', enabled: true, steps: [] }],
    };
  }
  store.tasks = [store.currentTask.task];
  store.agentFlowLibrary = library;
  const wrapper = mount(TaskBoard, { global: { plugins: [pinia] } });
  return { wrapper, store };
}

/** 点「改绑流程」展开入选区 */
async function openBindPanel(wrapper: ReturnType<typeof mount>): Promise<void> {
  await wrapper.findAll('button').find((b) => b.text() === '改绑流程')?.trigger('click');
  await nextTick();
}

/** 点入选区的「保存」(文案在保存中会变,故按包含匹配) */
async function clickSave(wrapper: ReturnType<typeof mount>): Promise<void> {
  const btn = wrapper.findAll('.sv-task-flow-bind-actions button').find((b) => b.text().includes('保存'));
  await btn?.trigger('click');
  await flushPromises();
}

function candidateBox(wrapper: ReturnType<typeof mount>, label: string) {
  const item = wrapper.findAll('.sv-task-flow-bind-panel label.flow-id-item').find((l) => l.text().includes(label));
  if (!item) throw new Error(`未找到勾选项:${label}`);
  return item.find('input[type="checkbox"]');
}

/**
 * window.confirm 桩:同一文件内 `vi.spyOn` 会**复用同一个 mock**(计数跨用例累积),
 * 故在 beforeEach 里重建并清空调用记录,用例内才敢断言调用次数。
 */
let confirmSpy: ReturnType<typeof vi.spyOn>;

/** 最近一次 confirm 的文案(断言「说清后果」用) */
function lastConfirmText(): string {
  return String(confirmSpy.mock.calls.at(-1)?.[0] ?? '');
}

beforeEach(() => {
  memStorage.clear();
  h.bindCalls = [];
  h.bindFail = null;
  confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
  confirmSpy.mockClear();
});

describe('TaskBoard 改绑流程入口门控(B 批 B3)', () => {
  it('非 custom 模式不给入口(后端也会 400)', () => {
    for (const mode of ['legacy', 'solo', 'multi', 'plan', 'team'] as const) {
      const { wrapper } = mountBoard({ task_mode: mode });
      expect(wrapper.text(), `${mode} 模式不该出现改绑入口`).not.toContain('改绑流程');
      wrapper.unmount();
    }
  });

  it('custom 模式下:进行中(planning/running)与待批准(planned)不给入口,其余状态给', async () => {
    for (const status of ['planning', 'running', 'planned'] as const) {
      const { wrapper } = mountBoard({ status });
      expect(wrapper.text(), `${status} 态不该出现改绑入口`).not.toContain('改绑流程');
      wrapper.unmount();
    }
    for (const status of ['pending', 'done', 'partial', 'error', 'ended'] as const) {
      const { wrapper } = mountBoard({ status });
      expect(wrapper.text(), `${status} 态应可改绑(后端门禁允许)`).toContain('改绑流程');
      wrapper.unmount();
    }
  });
});

describe('TaskBoard 改绑流程入选区(B 批 B3)', () => {
  it('点击展开内联入选区:根流程下拉 + 可调用名单 + 保存/取消,不新开模态', async () => {
    const { wrapper } = mountBoard();
    expect(wrapper.find('.sv-task-flow-bind-panel').exists()).toBe(false);
    await openBindPanel(wrapper);

    const panel = wrapper.find('.sv-task-flow-bind-panel');
    expect(panel.exists(), '入选区是在详情区里就地展开的').toBe(true);
    // 根流程下拉:含「跟随当前流程」,当前绑定原样读回
    const select = panel.find('select.sv-task-flow-bind-select');
    expect((select.element as HTMLSelectElement).value).toBe('f1');
    expect(select.text()).toContain('流程:跟随当前流程');
    // 名单勾选:根流程禁用、停用禁用、当前勾选的 f2 在场
    expect(panel.text()).toContain('主流程(根流程,不可调用)');
    expect(panel.text()).toContain('停用流程(已停用)');
    expect((candidateBox(wrapper, '主流程').element as HTMLInputElement).disabled).toBe(true);
    expect((candidateBox(wrapper, '停用流程').element as HTMLInputElement).disabled).toBe(true);
    expect((candidateBox(wrapper, '备用流程').element as HTMLInputElement).checked).toBe(true);
    // 保存 / 取消两个动作都在
    const actions = wrapper.findAll('.sv-task-flow-bind-actions button').map((b) => b.text());
    expect(actions.some((t) => t.includes('保存'))).toBe(true);
    expect(actions).toContain('取消');
    wrapper.unmount();
  });

  it('保存时全量下发:flow_id 为 null 与 flow_ids 为空数组都必须显式带上', async () => {
    const { wrapper } = mountBoard();
    await openBindPanel(wrapper);

    // ① 解绑(跟随当前流程)+ 清空名单(强制模式):两个「清空」语义都要能表达
    await wrapper.find('select.sv-task-flow-bind-select').setValue('');
    await candidateBox(wrapper, '备用流程').setValue(false);
    await clickSave(wrapper);
    expect(h.bindCalls).toEqual([{ id: 't1', flowId: null, flowIds: [] }]);

    // ② 换根流程为 f2 + 勾选可调用成员:名单顺序即勾选顺序(后端工具描述按此列举)
    await openBindPanel(wrapper);
    await wrapper.find('select.sv-task-flow-bind-select').setValue('f2');
    await candidateBox(wrapper, '主流程').setValue(true);
    await clickSave(wrapper);
    expect(h.bindCalls[1]).toEqual({ id: 't1', flowId: 'f2', flowIds: ['f1'] });

    // 二次确认:每次保存都问,且文案说清「替换快照」这件不可逆的事
    expect(confirmSpy).toHaveBeenCalledTimes(2);
    expect(lastConfirmText()).toContain('替换流程快照');
    wrapper.unmount();
  });

  it('根流程随下拉切换:新根立刻从可勾选变「根流程,不可调用」', async () => {
    const { wrapper } = mountBoard();
    await openBindPanel(wrapper);
    // 原根 f1 不可勾;切根到 f2 后 f1 可勾、f2 转为不可勾(与创建选择器同一判定)
    expect(candidateBox(wrapper, '主流程').element).toHaveProperty('disabled', true);
    await wrapper.find('select.sv-task-flow-bind-select').setValue('f2');
    await nextTick();
    expect(candidateBox(wrapper, '主流程').element).toHaveProperty('disabled', false);
    expect(wrapper.find('.sv-task-flow-bind-panel').text()).toContain('备用流程(根流程,不可调用)');
    wrapper.unmount();
  });

  it('保存成功后:详情与列表就地更新,入选区收起,快照先置空等事件补新', async () => {
    const { wrapper, store } = mountBoard({}, { withSnapshot: true });
    await openBindPanel(wrapper);
    await wrapper.find('select.sv-task-flow-bind-select').setValue('f2');
    await candidateBox(wrapper, '备用流程').setValue(true);
    await clickSave(wrapper);

    expect(store.currentTask?.task.flow_id).toBe('f2');
    expect(store.currentTask?.task.flow_ids).toEqual(['f2']);
    expect(store.tasks.find((t) => t.id === 't1')?.flow_id).toBe('f2');
    // 旧快照必然过期(后端已重新冻结),本地置空让徽标/流程名回退到当前库;
    // 权威快照由 flow_bound 事件的详情刷新补上
    expect(store.currentTask?.flow_snapshot).toBeNull();
    // 保存成功即收起(不需要用户再点一次取消)
    expect(wrapper.find('.sv-task-flow-bind-panel').exists()).toBe(false);
    wrapper.unmount();
  });

  it('后端 400:原文(点名原因)透出,入选区保持展开可重试', async () => {
    const { wrapper, store } = mountBoard();
    h.bindFail = new Error('任务已在执行中,不能改绑流程');
    await openBindPanel(wrapper);
    await clickSave(wrapper);

    expect(wrapper.find('.sv-task-flow-bind-panel').exists(), '失败后不该把入选区关掉').toBe(true);
    expect(wrapper.find('.sv-task-flow-bind-err').text()).toContain('任务已在执行中,不能改绑流程');
    // 失败不落地:详情里的绑定保持不变
    expect(store.currentTask?.task.flow_id).toBe('f1');
    wrapper.unmount();
  });

  it('「取消」与二次确认取消都不发请求(草稿丢弃,绑定不受影响)', async () => {
    const { wrapper, store } = mountBoard();
    await openBindPanel(wrapper);
    await wrapper.find('select.sv-task-flow-bind-select').setValue('f2');
    await wrapper.findAll('.sv-task-flow-bind-actions button').find((b) => b.text() === '取消')?.trigger('click');
    await nextTick();
    expect(h.bindCalls).toEqual([]);
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(wrapper.find('.sv-task-flow-bind-panel').exists()).toBe(false);

    // 二次确认取消:请求不发,入选区留在原地(用户可以改完再存)
    confirmSpy.mockReturnValue(false);
    await openBindPanel(wrapper);
    await clickSave(wrapper);
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(h.bindCalls).toEqual([]);
    expect(wrapper.find('.sv-task-flow-bind-panel').exists()).toBe(true);
    expect(store.currentTask?.task.flow_id).toBe('f1');
    wrapper.unmount();
  });

  it('切换任务时入选区收起(草稿不跨任务沿用)', async () => {
    const { wrapper, store } = mountBoard();
    await openBindPanel(wrapper);
    expect(wrapper.find('.sv-task-flow-bind-panel').exists()).toBe(true);

    store.currentTaskId = 't2';
    store.currentTask = makeTaskDetail({ id: 't2' });
    await flushPromises();
    expect(wrapper.find('.sv-task-flow-bind-panel').exists()).toBe(false);
    wrapper.unmount();
  });
});
