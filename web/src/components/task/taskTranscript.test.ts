// @vitest-environment jsdom
// TMSHOW-2 三档渲染测试:档 1 精修 / 档 2 扁平时间线 / 档 3 分组折叠 + 降级与窗口化。
//
// 为什么单独成文件而不塞进 taskPanels.test.ts:那个文件覆盖 7 个面板的装配,
// 本文件专门钉三档的 DOM 结构与边界(空轨迹降级、角标计数、虚拟化、折叠开关)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';
import type { TaskDetail, TaskRecord, TaskStep, TaskTrace } from '../../api';
import { useAppStore } from '../../store';
import { resetTaskTraceForTest } from '../../composables/useTaskTrace';
import TaskTraceItem from './TaskTraceItem.vue';
import TaskTranscript from './TaskTranscript.vue';
import TaskTranscriptModes from './TaskTranscriptModes.vue';

const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

// 组件挂载会在轨迹为空时触发一次 loadTaskTrace;打桩避免真发请求
vi.mock('../../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api')>();
  return { ...actual, getTaskTrace: vi.fn(async () => ({ trace: [], truncated: false })) };
});

function makeTask(overrides: Partial<TaskRecord> = {}): TaskRecord {
  return {
    id: 't1',
    title: '任务',
    status: 'running',
    plan: [],
    result: '',
    error: '',
    task_mode: 'solo',
    created_at: '2026-11-01T00:00:00.000Z',
    updated_at: '2026-11-01T00:00:00.000Z',
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

function traceRow(seq: number, over: Partial<TaskTrace> = {}): TaskTrace {
  return {
    id: seq,
    task_id: 't1',
    seq,
    phase: 'agent',
    step_index: null,
    kind: 'reasoning',
    name: null,
    content: `第 ${seq} 条轨迹`,
    truncated: false,
    created_at: '2026-11-01T00:00:00.000Z',
    ...over,
  };
}

/** 播种 store(含轨迹行与档位),返回 store 便于后续断言 */
function seed(detail: TaskDetail, rows: TaskTrace[] = [], override: 'legacy' | 'full' | 'condensed' | null = null) {
  setActivePinia(createPinia());
  const store = useAppStore();
  store.currentTaskId = detail.task.id;
  store.currentTask = detail;
  store.taskTrace = rows;
  if (override) store.setTaskTranscriptMode(override);
  return store;
}

beforeEach(() => {
  resetTaskTraceForTest();
  memStorage.clear();
});

describe('档位切换条(UIFIX-8 起独立挂载于常驻表头)', () => {
  it('渲染三个档位按钮,当前档带 active 标记', () => {
    const detail = makeDetail();
    seed(detail, [traceRow(1)], 'full');
    const wrapper = mount(TaskTranscriptModes);
    const buttons = wrapper.findAll('.sv-transcript-mode');
    expect(buttons).toHaveLength(3);
    expect(buttons.map((b) => b.text())).toEqual(['步骤', '全消息', '压缩']);
    expect(wrapper.find('.sv-transcript-mode.active').text()).toBe('全消息');
  });

  it('未设过覆盖时跟随设置页的默认档', () => {
    const detail = makeDetail();
    const store = seed(detail, [traceRow(1)], null);
    store.setDefaultTaskTranscriptMode('condensed');
    // 高亮在表头控件里(UIFIX-8 迁走后 TaskTranscript 不再渲染档位条),
    // 渲染档位仍看本组件的 data-mode——两处同源但不同组件,故分别挂载。
    const modes = mount(TaskTranscriptModes);
    expect(modes.find('.sv-transcript-mode.active').text()).toBe('压缩');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.attributes('data-mode')).toBe('condensed');
  });

  it('点档位按钮写本任务的覆盖键(不动全局默认档)', async () => {
    const detail = makeDetail();
    const store = seed(detail, [traceRow(1)], 'full');
    // 先把默认档设成压缩并落盘(watch 只在值变化时写,故这里用与初值不同的档)
    store.setDefaultTaskTranscriptMode('condensed');
    await nextTick();
    expect(memStorage.get('kedai.task-transcript-mode.v1')).toBe('condensed');

    const wrapper = mount(TaskTranscriptModes);
    await wrapper.findAll('.sv-transcript-mode')[0].trigger('click');
    expect(store.taskTranscriptOverride).toBe('legacy');
    // 覆盖写任务级键
    expect(memStorage.get('kedai.task-transcript-mode.v1.t1')).toBe('legacy');
    // 设置页的默认档不被顶栏控件改动
    expect(store.taskTranscriptMode).toBe('condensed');
    await nextTick();
    expect(memStorage.get('kedai.task-transcript-mode.v1')).toBe('condensed');
  });

  it('降级不改用户选择:空轨迹下高亮仍是用户选的档,但渲染落在档 1', () => {
    const detail = makeDetail({}, {
      plan: [{ name: '第一步', goal: '目标', status: 'done', result: '成果' }],
    });
    seed(detail, [], 'condensed');
    // 高亮:用户选的是压缩(在表头控件里读,与渲染组件分离)
    const modes = mount(TaskTranscriptModes);
    expect(modes.find('.sv-transcript-mode.active').text()).toBe('压缩');
    // 渲染:降到档 1
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.attributes('data-mode')).toBe('legacy');
    expect(wrapper.text()).toContain('本任务没有可展示的执行轨迹');
  });

  it('档位条不再渲染在 TaskTranscript 内部(否则会随轨迹滚走)', () => {
    const detail = makeDetail();
    seed(detail, [traceRow(1)], 'full');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.find('.sv-transcript-mode').exists()).toBe(false);
  });

  it('角标按全量轨迹计数(与档位条同处,不随档 3 分组变化)', () => {
    const detail = makeDetail();
    seed(
      detail,
      [
        traceRow(1, { kind: 'reasoning' }),
        traceRow(2, { kind: 'tool_call', name: 'bash' }),
        traceRow(3, { kind: 'tool_call', name: 'fs_read' }),
      ],
      'condensed',
    );
    const wrapper = mount(TaskTranscriptModes);
    expect(wrapper.find('.sv-transcript-badge').text()).toBe('思考 1 · 工具 2 · bash 1');
  });
});

describe('档 2 full:扁平时间线', () => {
  it('每条轨迹独立成条,按 seq 顺序', () => {
    const detail = makeDetail();
    seed(
      detail,
      [
        traceRow(2, { kind: 'tool_call', name: 'bash', content: '{"cmd":"ls"}' }),
        traceRow(1, { content: '先想一下' }),
      ],
      'full',
    );
    const wrapper = mount(TaskTranscript, { props: { detail } });
    const items = wrapper.findAll('.sv-trace-item');
    expect(items).toHaveLength(2);
    // 排序由 flattenTrace 保证:seq=1 在前
    expect(items[0].text()).toContain('先想一下');
    expect(items[1].text()).toContain('bash');
  });

  it('轨迹为空时回退档 1 并显示说明,不出现空时间线', () => {
    const plan: TaskStep[] = [{ name: '第一步', goal: 'g', status: 'done', result: '成果' }];
    const detail = makeDetail({}, { plan });
    seed(detail, [], 'full');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.text()).toContain('本任务没有可展示的执行轨迹');
    // 档 1 的步骤区在
    expect(wrapper.text()).toContain('计划步骤');
    // 档 2 的容器不在
    expect(wrapper.find('.sv-transcript-full').exists()).toBe(false);
  });

  it('300 条以上时窗口化生效:非尾部行渲染为占位而非真实条目', () => {
    const detail = makeDetail();
    const rows = Array.from({ length: 320 }, (_, i) =>
      traceRow(i + 1, { content: `轨迹 ${i + 1}` }),
    );
    seed(detail, rows, 'full');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    // 320 条 > 阈值 120 → 只有尾部 30 条常驻挂载
    const mounted = wrapper.findAll('.sv-trace-item').length;
    expect(mounted).toBeLessThanOrEqual(30);
    expect(mounted).toBeLessThan(320);
  });

  it('占位行**同样**经 bindRow 登记(否则滚动时占位永远换不回真实条目)', () => {
    const detail = makeDetail();
    const rows = Array.from({ length: 200 }, (_, i) => traceRow(i + 1));
    seed(detail, rows, 'full');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    const placeholders = wrapper.findAll('.sv-transcript-placeholder');
    expect(placeholders.length).toBeGreaterThan(0);
    // 占位行必须带 bindRow 写入的 data 标记。首轮实现只给真实行挂了 ref,
    // 实测滚动后 items 恒为 30(占位永远等不到「进入视口」上报)——这条是那个缺陷的回归网。
    expect(placeholders[0].attributes('data-kd-vm-id')).toBeTruthy();
  });

  it('条数未超阈值时全部真实挂载(短任务零虚拟化开销)', () => {
    const detail = makeDetail();
    const rows = Array.from({ length: 20 }, (_, i) => traceRow(i + 1));
    seed(detail, rows, 'full');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.findAll('.sv-trace-item')).toHaveLength(20);
  });
});

describe('档 3 condensed:分组折叠 + 分类计数', () => {
  it('按 phase + step_index 分组,每组一张卡并显示角标', () => {
    const detail = makeDetail();
    seed(
      detail,
      [
        traceRow(1, { phase: 'agent', step_index: 0, kind: 'reasoning' }),
        traceRow(2, { phase: 'agent', step_index: 0, kind: 'reasoning' }),
        traceRow(3, { phase: 'agent', step_index: 0, kind: 'tool_call', name: 'bash' }),
        traceRow(4, { phase: 'final_audit', step_index: null, kind: 'reasoning' }),
      ],
      'condensed',
    );
    const wrapper = mount(TaskTranscript, { props: { detail } });
    const groups = wrapper.findAll('.sv-transcript-group');
    expect(groups).toHaveLength(2);
    // 第一组:思考 2 · 工具 1 · bash 1
    expect(groups[0].text()).toContain('思考 2');
    expect(groups[0].text()).toContain('工具 1');
    expect(groups[0].text()).toContain('bash 1');
    expect(groups[1].text()).toContain('终审');
  });

  it('组默认收起,点击后展开并显示组内条目', async () => {
    const detail = makeDetail();
    seed(detail, [traceRow(1), traceRow(2)], 'condensed');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.findAll('.sv-trace-item')).toHaveLength(0);
    await wrapper.find('.sv-transcript-group-head').trigger('click');
    expect(wrapper.findAll('.sv-trace-item')).toHaveLength(2);
  });

  it('顶部总览角标已随档位条迁出(档 3 内部不再有第二份计数)', () => {
    const detail = makeDetail();
    seed(
      detail,
      [
        traceRow(1, { kind: 'reasoning' }),
        traceRow(2, { kind: 'tool_call', name: 'bash' }),
        traceRow(3, { kind: 'tool_call', name: 'fs_read' }),
        traceRow(4, { kind: 'tool_result', name: 'bash' }),
      ],
      'condensed',
    );
    const wrapper = mount(TaskTranscript, { props: { detail } });
    // 全量角标住在 TaskTranscriptModes(见本文件「档位切换条」组),
    // 这里只钉住「档 3 卡内不重复渲染一份」;各分组的角标仍在,计数口径不变。
    expect(wrapper.find('.sv-transcript-badge').exists()).toBe(false);
    expect(wrapper.find('.sv-transcript-group-count').text()).toBe('思考 1 · 工具 2 · bash 1');
  });
});

describe('档 1 legacy:步骤视图精修', () => {
  it('渲染步骤序号/状态徽标/目标,长结果默认折叠', async () => {
    const longResult = '很长的结果'.repeat(200);
    const plan: TaskStep[] = [
      { name: '短结果步骤', goal: 'g1', status: 'done', result: '短结果' },
      { name: '长结果步骤', goal: 'g2', status: 'done', result: longResult },
    ];
    const detail = makeDetail({}, { plan });
    seed(detail, [], 'legacy');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.text()).toContain('计划步骤');
    // 短结果直接渲染
    expect(wrapper.text()).toContain('短结果');
    // 长结果折叠成按钮
    expect(wrapper.text()).toContain('展开步骤结果');
    expect(wrapper.text()).not.toContain(longResult);
    // 点开(折叠按钮就在第二步的 body 里)
    await wrapper.find('.sv-transcript-expand').trigger('click');
    const results = wrapper.findAll('.sv-task-step-result');
    // 第一步的短结果 + 第二步展开后的长结果
    expect(results).toHaveLength(2);
    expect(results[1].text()).toContain('很长的结果');
  });

  it('四种状态各自带状态色带 class', () => {
    const plan: TaskStep[] = [
      { name: 'a', goal: '', status: 'done', result: '' },
      { name: 'b', goal: '', status: 'running', result: '' },
      { name: 'c', goal: '', status: 'error', result: '' },
      { name: 'd', goal: '', status: 'partial', result: '' },
    ];
    const detail = makeDetail({}, { plan });
    seed(detail, [], 'legacy');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    const steps = wrapper.findAll('.sv-task-step');
    expect(steps[0].classes()).toContain('done');
    expect(steps[1].classes()).toContain('active');
    expect(steps[2].classes()).toContain('error');
    expect(steps[3].classes()).toContain('partial');
  });

  it('planned 态 plan 模式且已渲染批准区清单时隐藏步骤区(视觉去重)', () => {
    const plan: TaskStep[] = [{ name: '第一步', goal: '', status: 'pending', result: '' }];
    const detail = makeDetail({}, { status: 'planned', task_mode: 'plan', plan, result: '计划清单' });
    seed(detail, [], 'legacy');
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.text()).not.toContain('计划步骤');
  });
});

describe('错误态与截断提示', () => {
  it('加载失败显示错误条与重试按钮(与「没有轨迹」严格区分)', () => {
    const detail = makeDetail();
    const store = seed(detail, [], 'full');
    store.taskTraceError = '读失败';
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.find('.sv-transcript-error').exists()).toBe(true);
    expect(wrapper.text()).toContain('读失败');
    expect(wrapper.text()).toContain('重试');
  });

  it('窗口被清理时显示截断说明', () => {
    const detail = makeDetail();
    const store = seed(detail, [traceRow(1)], 'full');
    store.taskTraceTruncated = true;
    const wrapper = mount(TaskTranscript, { props: { detail } });
    expect(wrapper.text()).toContain('轨迹保留窗口已清理更早的条目');
  });
});

describe('TaskTraceItem 单条卡', () => {
  it('思考短文本默认展开,工具调用默认收起', () => {
    const reasoning = mount(TaskTraceItem, {
      props: { trace: traceRow(1, { kind: 'reasoning', content: '短思考' }) },
    });
    // 正文在模板里直接渲染(未折叠时 <pre> 存在)
    expect(reasoning.find('.sv-trace-body').exists()).toBe(true);

    const tool = mount(TaskTraceItem, {
      props: { trace: traceRow(2, { kind: 'tool_call', name: 'bash', content: '{"cmd":"ls"}' }) },
    });
    // 内容短(<120)→ 不折叠,直接显示
    expect(tool.find('.sv-trace-body').exists()).toBe(true);
  });

  it('长正文默认收起,点击标题展开(aria-expanded 同步)', async () => {
    const long = 'x'.repeat(500);
    const wrapper = mount(TaskTraceItem, {
      props: { trace: traceRow(1, { kind: 'tool_result', name: 'bash', content: long }) },
    });
    const head = wrapper.find('.sv-trace-head');
    expect(head.attributes('aria-expanded')).toBe('false');
    expect(wrapper.find('.sv-trace-body').exists()).toBe(false);
    await head.trigger('click');
    expect(wrapper.find('.sv-trace-head').attributes('aria-expanded')).toBe('true');
    expect(wrapper.find('.sv-trace-body').exists()).toBe(true);
  });

  it('正文一律纯文本插值(不解析 markdown / 不注入 HTML)', () => {
    const wrapper = mount(TaskTraceItem, {
      props: {
        trace: traceRow(1, { kind: 'step_result', content: '<b>不该加粗</b> **也不该**' }),
      },
    });
    expect(wrapper.find('.sv-trace-body').text()).toContain('<b>不该加粗</b>');
    expect(wrapper.html()).not.toContain('<strong>');
  });

  it('历史坏数据的工具参数在卡内被还原成多行(UIFIX-8 前端兜底)', () => {
    // 旧轨迹:整段参数被序列化成 JSON 字符串字面量,`\n` 是字面两字符
    const literal = JSON.stringify('{"command":"ls -la && ls parts","cwd":"."}');
    const wrapper = mount(TaskTraceItem, {
      props: { trace: traceRow(1, { kind: 'tool_call', name: 'bash', content: literal, truncated: false }) },
      // 强制展开,否则内容被折叠开关挡住、断言看不到正文
    });
    // 默认收起(>120 字才折叠;这条很短,直接可见)
    const body = wrapper.find('.sv-trace-body');
    expect(body.exists()).toBe(true);
    expect(body.text()).toContain('\n');
    expect(body.text()).toContain('"command": "ls -la && ls parts"');
    expect(body.text()).not.toContain('\\"');
  });

  it('truncated 显示「已截断」标记', () => {
    const wrapper = mount(TaskTraceItem, {
      props: { trace: traceRow(1, { content: 'x', truncated: true }) },
    });
    expect(wrapper.text()).toContain('已截断');
  });

  it('副标题带阶段中文标签与工具名', () => {
    const wrapper = mount(TaskTraceItem, {
      props: { trace: traceRow(1, { kind: 'tool_call', name: 'bash', phase: 'agent', step_index: 2 }) },
    });
    expect(wrapper.find('.sv-trace-sub').text()).toBe('主Agent #3 · bash');
  });
});
