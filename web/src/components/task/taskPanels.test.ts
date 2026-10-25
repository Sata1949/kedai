// @vitest-environment jsdom
// C-2A（2026-10-25）TaskBoard 拆分后的面板级测试。
//
// 为什么一个文件覆盖 7 个组件：拆分后的面板共享同一套 store 播种与详情夹具，
// 分成 7 个文件只会重复 7 次 beforeEach；零测试 ratchet 只要求“测试直接 import 源码”，
// 本文件逐个直接 import，满足门禁。深度交互（改绑全量下发、详情回退等）仍由
// TaskBoard.bind.test.ts / stores/task.test.ts 那两张既有回归网覆盖。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import type { TaskDetail, TaskRecord, TaskStep } from '../../api';
import { useAppStore } from '../../store';
import MarkdownBlock from './MarkdownBlock.vue';
import TaskStatusPanel from './TaskStatusPanel.vue';
import TaskFlowBindPanel from './TaskFlowBindPanel.vue';
import TaskApprovalPanel from './TaskApprovalPanel.vue';
import TaskExecutionPanel from './TaskExecutionPanel.vue';
import TaskResultPanel from './TaskResultPanel.vue';
import TaskBoardEmpty from './TaskBoardEmpty.vue';

const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

function makeTask(overrides: Partial<TaskRecord> = {}): TaskRecord {
  return {
    id: 't1',
    title: '拆分后的任务',
    status: 'pending',
    plan: [],
    result: '',
    error: '',
    task_mode: 'solo',
    created_at: '2026-10-25T00:00:00.000Z',
    updated_at: '2026-10-25T00:00:00.000Z',
    ...overrides,
  };
}

function makeDetail(overrides: Partial<TaskDetail> = {}, task: Partial<TaskRecord> = {}): TaskDetail {
  return {
    task: makeTask(task),
    subtasks: [],
    usage_total: { prompt_tokens: 0, completion_tokens: 0, reasoning_tokens: 0 },
    messages: [],
    ...overrides,
  };
}

function seed(detail: TaskDetail): ReturnType<typeof useAppStore> {
  setActivePinia(createPinia());
  const store = useAppStore();
  store.currentTaskId = detail.task.id;
  store.currentTask = detail;
  store.currentTaskUsage = detail.usage_total;
  // FileChangesPanel 挂载会拉变更；这里统一挡掉真实请求
  store.loadTaskChanges = vi.fn().mockResolvedValue(undefined);
  return store;
}

describe('C-2A 任务面板拆分', () => {
  beforeEach(() => {
    memStorage.clear();
    vi.restoreAllMocks();
    vi.stubGlobal('confirm', vi.fn(() => true));
    vi.stubGlobal('alert', vi.fn());
  });

  it('MarkdownBlock:把已渲染 html 放进 hero 容器,并允许追加类名', () => {
    const wrapper = mount(MarkdownBlock, {
      props: { html: '<p>成果 <strong>正文</strong></p>' },
      attrs: { class: 'sv-task-audit' },
    });
    expect(wrapper.find('.sv-task-result.sv-task-audit').exists()).toBe(true);
    expect(wrapper.find('strong').text()).toBe('正文');
  });

  it('TaskStatusPanel:渲染头/状态/工作区,执行与停止按钮按 running 切换', async () => {
    const detail = makeDetail(
      { workspace_profile: { root: '/w', detected: [{ kind: 'rust', marker: 'Cargo.toml', suggested_command: 'cargo test', depth: 0 }], scanned_dirs: 0, truncated: false } },
      { status: 'running', workspace: 'D:/work', task_mode: 'solo' },
    );
    const store = seed(detail);
    store.stopTask = vi.fn().mockResolvedValue(undefined);
    store.runTask = vi.fn().mockResolvedValue(undefined);
    const wrapper = mount(TaskStatusPanel, { props: { detail, running: true } });
    expect(wrapper.find('.sv-task-head-title').text()).toContain('拆分后的任务');
    expect(wrapper.text()).toContain('模式:');
    expect(wrapper.find('.sv-task-workspace-path').text()).toBe('D:/work');
    expect(wrapper.text()).toContain('cargo test');
    await wrapper.find('button[title="停止执行"]').trigger('click');
    expect(store.stopTask).toHaveBeenCalledWith('t1');
    await wrapper.setProps({ running: false });
    await wrapper.find('button[title="执行(重新)此任务"]').trigger('click');
    expect(store.runTask).toHaveBeenCalledWith('t1');
  });

  it('TaskStatusPanel:删除走 useTaskActions 的确认与 store.deleteTask', async () => {
    const detail = makeDetail();
    const store = seed(detail);
    store.deleteTask = vi.fn().mockResolvedValue(undefined);
    const wrapper = mount(TaskStatusPanel, { props: { detail, running: false } });
    await wrapper.find('button[title="删除任务"]').trigger('click');
    await flushPromises();
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('拆分后的任务'));
    expect(store.deleteTask).toHaveBeenCalledWith('t1');
  });

  it('TaskFlowBindPanel:custom 非运行态给入口,点击展开内联入选区', async () => {
    const detail = makeDetail({}, { task_mode: 'custom', status: 'done' });
    const store = seed(detail);
    store.agentFlowLibrary = { current_flow_id: null, flows: [] };
    const wrapper = mount(TaskFlowBindPanel, { props: { detail } });
    const entry = wrapper.find('button[title^="改绑本任务执行的流程"]');
    expect(entry.exists()).toBe(true);
    await entry.trigger('click');
    expect(wrapper.find('.sv-task-flow-bind-panel').exists()).toBe(true);
    expect(wrapper.text()).toContain('流程库为空');
  });

  it('TaskFlowBindPanel:进行中不给入口', () => {
    const detail = makeDetail({}, { task_mode: 'custom', status: 'running' });
    seed(detail);
    const wrapper = mount(TaskFlowBindPanel, { props: { detail } });
    expect(wrapper.find('.sv-task-flow-bind').exists()).toBe(false);
  });

  it('TaskApprovalPanel:planned 态渲染批准区,批准调用 store.approveTask', async () => {
    const plan: TaskStep[] = [{ name: '第一步', goal: '做调研', status: 'pending', result: '' }];
    const detail = makeDetail({}, { task_mode: 'plan', status: 'planned', plan });
    const store = seed(detail);
    store.approveTask = vi.fn().mockResolvedValue(undefined);
    const wrapper = mount(TaskApprovalPanel, { props: { detail } });
    expect(wrapper.text()).toContain('计划待批准');
    expect(wrapper.text()).toContain('与规划器对话');
    const approve = wrapper.findAll('button').find((b) => b.text().includes('批准执行'));
    expect(approve).toBeTruthy();
    if (!approve) throw new Error('批准按钮应存在');
    await approve.trigger('click');
    await flushPromises();
    expect(store.approveTask).toHaveBeenCalledWith('t1', undefined, 'approved_plan');
  });

  it('TaskApprovalPanel:非 planned 态整块不渲染', () => {
    const detail = makeDetail({}, { status: 'running' });
    seed(detail);
    const wrapper = mount(TaskApprovalPanel, { props: { detail } });
    expect(wrapper.find('.sv-task-approve').exists()).toBe(false);
  });

  it('TaskExecutionPanel:team 分工卡 + 计划步骤 + 流式文本', async () => {
    const plan: TaskStep[] = [
      { name: '【主Agent-1】调研', goal: '调研', status: 'done', result: '**调研完成**' },
      { name: '【主Agent-2】撰写', goal: '撰写', status: 'running', result: '' },
    ];
    const detail = makeDetail({}, { task_mode: 'team', status: 'running', plan });
    const store = seed(detail);
    store.liveBuffers = new Map([['step:1', '正在写出第一段']]);
    const wrapper = mount(TaskExecutionPanel, { props: { detail } });
    expect(wrapper.text()).toContain('主 Agent 分工');
    expect(wrapper.text()).toContain('【主Agent-1】');
    expect(wrapper.find('.sv-task-step-result').html()).toContain('<strong>');
    expect(wrapper.find('.sv-task-live-text').text()).toContain('正在写出第一段');
    // team 模式不渲染「计划步骤」区（与分工卡互斥）
    expect(wrapper.text()).not.toContain('计划步骤');
  });

  it('TaskExecutionPanel:solo 模式渲染调用情况入口与计划步骤', () => {
    const plan: TaskStep[] = [{ name: '第一步', goal: '做调研', status: 'done', result: '已完成' }];
    const detail = makeDetail({}, { task_mode: 'solo', status: 'done', plan });
    seed(detail);
    const wrapper = mount(TaskExecutionPanel, { props: { detail } });
    expect(wrapper.text()).toContain('查看调用情况');
    expect(wrapper.text()).toContain('计划步骤');
  });

  it('TaskResultPanel:成果汇总/交付校验/文件变更/对话记录/追加指令全集', () => {
    const detail = makeDetail(
      {
        messages: [
          { id: 'm1', task_id: 't1', role: 'user', kind: 'goal', content: '目标', created_at: '' },
          { id: 'm2', task_id: 't1', role: 'assistant', kind: 'result', content: '**产出**', created_at: '' },
        ],
      },
      { status: 'done', task_mode: 'team', result: '主体成果\n\n## 审计结论\n审计通过' },
    );
    seed(detail);
    const wrapper = mount(TaskResultPanel, { props: { detail } });
    expect(wrapper.text()).toContain('成果汇总');
    expect(wrapper.text()).toContain('交付校验');
    expect(wrapper.text()).toContain('文件变更');
    expect(wrapper.text()).toContain('对话记录');
    expect(wrapper.text()).toContain('追加指令');
    expect(wrapper.html()).toContain('<strong>产出</strong>');
  });

  it('TaskResultPanel:执行中追加输入框禁用并给出下一步提示', () => {
    const detail = makeDetail({}, { status: 'running' });
    seed(detail);
    const wrapper = mount(TaskResultPanel, { props: { detail } });
    const textarea = wrapper.find('.sv-task-followup textarea');
    expect((textarea.element as HTMLTextAreaElement).disabled).toBe(true);
    expect(wrapper.text()).toContain('任务执行中');
  });

  it('TaskBoardEmpty:渲染空态与执行设置,按钮打开侧栏', async () => {
    setActivePinia(createPinia());
    const store = useAppStore();
    store.taskRunMode = 'custom';
    store.taskFlowId = 'f1';
    store.agentFlowLibrary = { current_flow_id: 'f1', flows: [{ id: 'f1', name: '我的流程', enabled: true, steps: [] }] };
    store.openSidebar = vi.fn();
    const wrapper = mount(TaskBoardEmpty);
    expect(wrapper.text()).toContain('把目标交给 Kedai');
    expect(wrapper.text()).toContain('我的流程');
    await wrapper.find('button').trigger('click');
    expect(store.openSidebar).toHaveBeenCalled();
  });
});
