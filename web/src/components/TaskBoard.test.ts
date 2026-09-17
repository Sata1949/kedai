// @vitest-environment jsdom
// TaskBoard 组件测试(M5 补测):749 行「万能组件」,此前无任何测试。
// 用 jsdom + @vue/test-utils 真实挂载,验证三条最高风险路径:
//   ① 无当前任务时的空态渲染;
//   ② 有任务时的标题/状态/模式徽标渲染(M0 引入 TaskRunMode 枚举后易漂移);
//   ③ 执行/停止按钮按 taskRunning 切换——这是用户最主要的操作入口。
// api 模块整体 mock:组件直接调 api,不 mock 会打真实网络。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import type { TaskDetail } from '../api';

// node/jsdom 无 localStorage:子 store 初始化即访问,补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    listTasks: vi.fn().mockResolvedValue([]),
    getTask: vi.fn().mockResolvedValue(null),
    loadTaskCalls: vi.fn().mockResolvedValue([]),
    listTaskCalls: vi.fn().mockResolvedValue([]),
    runTask: vi.fn().mockResolvedValue(undefined),
    stopTask: vi.fn().mockResolvedValue(undefined),
    deleteTask: vi.fn().mockResolvedValue(undefined),
  };
});

import { useAppStore } from '../store';
import TaskBoard from './TaskBoard.vue';

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
      created_at: '2026-09-12T00:00:00Z',
      updated_at: '2026-09-12T00:00:00Z',
      task_mode: 'legacy',
      ...overrides,
    },
    subtasks: [],
    usage_total: { prompt_tokens: 0, completion_tokens: 0, reasoning_tokens: 0 },
    messages: [],
  };
}

function mountBoard() {
  setActivePinia(createPinia());
  return mount(TaskBoard, { global: { plugins: [] } });
}

describe('TaskBoard 组件(M5 补测)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('无当前任务时渲染面包屑头部与空详情区,不崩溃', () => {
    const wrapper = mountBoard();
    // 头部常驻;无 currentTask 时不渲染详情标题
    expect(wrapper.find('.sv-taskboard').exists()).toBe(true);
    expect(wrapper.find('.sv-task-head-title').exists()).toBe(false);
  });

  it('有任务时渲染标题、状态标签与执行模式徽标', () => {
    const store = useAppStore();
    store.currentTask = makeTaskDetail({ title: '写一份周报', status: 'done', task_mode: 'solo' });
    const wrapper = mount(TaskBoard);

    expect(wrapper.find('.sv-task-head-title').text()).toContain('写一份周报');
    // 状态经 taskStatusLabel 中文化(M0 的 TaskStatus 枚举消费点)
    const statusLine = wrapper.find('.sv-task-status-line').text();
    expect(statusLine).toContain('模式:');
    expect(statusLine.length).toBeGreaterThan(0);
  });

  it('六模式徽标随 task_mode 变化(防 TaskRunMode 枚举与 UI 文案漂移)', () => {
    const store = useAppStore();
    // 期望文案取自 TaskBoard.vue 的 MODE_LABELS(六模式全量,枚举新增值时应同步更新)
    for (const [mode, label] of [
      ['legacy', '三段式'],
      ['solo', '单 Agent'],
      ['multi', '多 Agent'],
      ['plan', '先规划后批准'],
      ['team', '团队协作'],
      ['custom', '自定义流程'],
    ] as const) {
      store.currentTask = makeTaskDetail({ task_mode: mode });
      const wrapper = mount(TaskBoard);
      const line = wrapper.find('.sv-task-status-line').text();
      expect(line).toContain(`模式:${label}`);
      wrapper.unmount();
    }
  });

  it('plan 批准区渲染执行方式下拉,默认「按计划逐步执行」(2026-09-17 新增)', () => {
    const store = useAppStore();
    store.currentTask = makeTaskDetail({
      status: 'planned',
      task_mode: 'plan',
      plan: [{ name: '步骤一', goal: '目标一', status: 'pending', result: '' }],
    });
    const wrapper = mount(TaskBoard);

    const select = wrapper.find('.sv-task-approve-exec select');
    expect(select.exists(), '批准区应有执行方式下拉').toBe(true);
    expect(select.text()).toContain('按计划逐步执行');
    // 可选集:不含 legacy(重复规划)与 plan(死循环),这两个值不在下拉里
    const options = select.findAll('option').map((o) => o.attributes('value'));
    expect(options).toEqual(['approved_plan', 'solo', 'multi', 'team', 'custom']);
    expect(options).not.toContain('legacy');
    expect(options).not.toContain('plan');
    // 默认选中默认项(改造前行为的等价物)
    expect((select.element as HTMLSelectElement).value).toBe('approved_plan');
    wrapper.unmount();
  });

  it('批准时把所选执行方式下发给 store(缺省 approved_plan)', async () => {
    const store = useAppStore();
    // currentTaskId 是组件的守门与任务 id 来源(仅设 currentTask 时 approveCurrent 会提前返回)
    store.currentTaskId = 't1';
    store.currentTask = makeTaskDetail({
      status: 'planned',
      task_mode: 'plan',
      plan: [{ name: '步骤一', goal: '目标一', status: 'pending', result: '' }],
    });
    const spy = vi.spyOn(store, 'approveTask').mockResolvedValue(undefined);
    const wrapper = mount(TaskBoard);

    // 选团队协作后批准
    await wrapper.find('.sv-task-approve-exec select').setValue('team');
    const approveBtn = wrapper
      .findAll('.sv-task-approve-actions button')
      .find((b) => b.text().includes('批准执行'))!;
    await approveBtn.trigger('click');
    await Promise.resolve();

    expect(spy).toHaveBeenCalledWith('t1', undefined, 'team');
    wrapper.unmount();
  });

  it('切任务时执行方式复位为默认(不把上个任务的选择带过来)', async () => {
    const store = useAppStore();
    store.currentTask = makeTaskDetail({
      id: 't1',
      status: 'planned',
      task_mode: 'plan',
      plan: [{ name: '步骤一', goal: '目标一', status: 'pending', result: '' }],
    });
    const wrapper = mount(TaskBoard);
    store.currentTaskId = 't1';
    await wrapper.find('.sv-task-approve-exec select').setValue('solo');
    expect((wrapper.find('.sv-task-approve-exec select').element as HTMLSelectElement).value).toBe('solo');

    // 切到另一个任务(复位 watch 的触发源是 currentTaskId)
    store.currentTaskId = 't2';
    store.currentTask = makeTaskDetail({
      id: 't2',
      status: 'planned',
      task_mode: 'plan',
      plan: [{ name: '步骤一', goal: '目标一', status: 'pending', result: '' }],
    });
    await Promise.resolve();
    await wrapper.vm.$nextTick();
    expect((wrapper.find('.sv-task-approve-exec select').element as HTMLSelectElement).value).toBe('approved_plan');
    wrapper.unmount();
  });

  it('非运行态显示「执行」按钮,运行态显示「停止」(用户主操作入口)', async () => {
    const store = useAppStore();
    store.currentTask = makeTaskDetail({ status: 'pending' });
    let wrapper = mount(TaskBoard);
    const buttons = () => wrapper.findAll('.sv-task-head-actions button').map((b) => b.text());
    expect(buttons().some((t) => t.includes('执行'))).toBe(true);
    expect(buttons().some((t) => t.includes('停止'))).toBe(false);
    wrapper.unmount();

    store.currentTask = makeTaskDetail({ status: 'running' });
    wrapper = mount(TaskBoard);
    const running = wrapper.findAll('.sv-task-head-actions button').map((b) => b.text());
    expect(running.some((t) => t.includes('停止'))).toBe(true);
    expect(running.some((t) => t.includes('执行'))).toBe(false);
    wrapper.unmount();
  });
});
