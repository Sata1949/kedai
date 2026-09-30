// @vitest-environment jsdom
// TaskBoard 组件测试(M5 补测):749 行「万能组件」,此前无任何测试。
// 用 jsdom + @vue/test-utils 真实挂载,验证三条最高风险路径:
//   ① 无当前任务时的空态渲染;
//   ② 有任务时的标题/状态/模式徽标渲染(M0 引入 TaskRunMode 枚举后易漂移);
//   ③ 执行/停止按钮按 taskRunning 切换——这是用户最主要的操作入口。
// api 模块整体 mock:组件直接调 api,不 mock 会打真实网络。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
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

  it('CODE-1:绑定工作区时展示路径与复制入口;未绑定不渲染该行', async () => {
    const store = useAppStore();
    store.currentTask = makeTaskDetail({ workspace: 'D:\\proj\\demo' });
    const wrapper = mount(TaskBoard);
    expect(wrapper.find('.sv-task-workspace').exists()).toBe(true);
    expect(wrapper.find('.sv-task-workspace-path').text()).toBe('D:\\proj\\demo');
    expect(wrapper.find('.sv-task-workspace .sv-btn').text()).toContain('复制');

    // 切到未绑定任务 → 整行消失(不把「没有」渲染成一行噪音)
    store.currentTask = makeTaskDetail();
    await wrapper.vm.$nextTick();
    expect(wrapper.find('.sv-task-workspace').exists()).toBe(false);
    wrapper.unmount();
  });

  it('CODE-1:复制工作区路径写入剪贴板并给「已复制」回执;剪贴板不可用时不虚报', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    const store = useAppStore();
    store.currentTask = makeTaskDetail({ workspace: 'D:\\proj\\demo' });
    let wrapper = mount(TaskBoard);
    await wrapper.find('.sv-task-workspace .sv-btn').trigger('click');
    await flushPromises();
    expect(writeText).toHaveBeenCalledWith('D:\\proj\\demo');
    expect(wrapper.find('.sv-task-workspace .sv-btn').text()).toBe('已复制');
    wrapper.unmount();

    // 剪贴板抛错:文案保持「复制」,不做无证据的成功声称
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockRejectedValue(new Error('denied')) },
      configurable: true,
    });
    wrapper = mount(TaskBoard);
    await wrapper.find('.sv-task-workspace .sv-btn').trigger('click');
    await flushPromises();
    expect(wrapper.find('.sv-task-workspace .sv-btn').text()).toBe('复制');
    wrapper.unmount();
  });
});

// 多缓冲的标签中文化(遗留.md IFW-7③):liveBuffers 的 key 是内部键
// (`${phase}:${step_index ?? ''}`),多缓冲兜底(team 并行 / 静态子图)时曾把它直接插值给
// 用户看(如 `【subflow.1:0】`)。现在统一走 utils/phaseLabel 的中文标签。
describe('TaskBoard 多缓冲「正在生成」块(IFW-7③)', () => {
  beforeEach(() => {
    memStorage.clear();
  });

  it('多缓冲前缀用中文阶段标签,不裸露内部 key;step 阶段补步骤名', () => {
    const store = useAppStore();
    store.currentTaskId = 't1';
    store.currentTask = makeTaskDetail({
      id: 't1',
      status: 'running',
      task_mode: 'custom',
      plan: [{ name: '起草', goal: 'g', status: 'running', result: '' }],
    });
    store.liveBuffers = new Map([
      ['step:0', '第一条增量'],
      ['subflow.1:0', '第二条增量'],
    ]);
    const wrapper = mount(TaskBoard);
    const text = wrapper.text();

    // 前缀可读:step 带步骤名、子图带挂载路径
    expect(text).toContain('【步骤 #1 · 起草】');
    expect(text).toContain('【子流程(#2 内) #1】');
    // 内部 key 不再出现在界面里
    expect(text).not.toContain('subflow.1:0');
    expect(text).not.toContain('step:0');
    wrapper.unmount();
  });

  it('单缓冲不画前缀(纯文本流式块与改造前一致)', () => {
    const store = useAppStore();
    store.currentTaskId = 't1';
    store.currentTask = makeTaskDetail({ id: 't1', status: 'running', task_mode: 'solo', plan: [] });
    store.liveBuffers = new Map([['step:0', '唯一一条增量']]);
    const wrapper = mount(TaskBoard);
    const text = wrapper.text();
    expect(text).toContain('唯一一条增量');
    expect(text).not.toContain('【步骤');
    wrapper.unmount();
  });
});
