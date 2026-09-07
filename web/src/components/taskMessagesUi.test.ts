import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createSSRApp, h, type Component } from 'vue';
import { renderToString } from 'vue/server-renderer';
import { createPinia, setActivePinia, type Pinia } from 'pinia';
import { useTaskStore } from '../stores/task';
import TaskBoard from './TaskBoard.vue';
import type { TaskDetail, TaskMessage, TaskMessageKind, TaskMessageRole, TaskRecord, TaskRunMode, TaskStatus, TaskStep } from '../api';

// 批次 R2 多轮用户输入 UI 冒烟:沿用 taskModeUi.test.ts 的 SSR 模式
// (createSSRApp + renderToString + pinia 播种;无 jsdom/@vue/test-utils)。
// 验证:① 追加指令条——终态可用、非终态(running/planned/pending)禁用并给提示;
// ② 用户指令历史区——user 气泡(sv-msg user/sv-msg-bubble)+ assistant 截断概要
// + kind 小标签;③ 旧服务端详情无 messages 字段时历史区不渲染、输入条照常(容错)。

// node 环境无 localStorage(store 初始化即访问),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

function makeTask(status: TaskStatus, result = '首轮成果文本', mode: TaskRunMode = 'solo', plan: TaskStep[] = []): TaskRecord {
  return {
    id: 't1',
    title: '写一段关于秋天的短文',
    status,
    plan,
    result,
    error: '',
    task_mode: mode,
    created_at: '2026-09-03T00:00:00.000Z',
    updated_at: '2026-09-03T00:00:00.000Z',
  };
}

function makeDetail(task: TaskRecord, messages?: TaskMessage[]): TaskDetail {
  const detail: TaskDetail = {
    task,
    subtasks: [],
    usage_total: { prompt_tokens: 0, completion_tokens: 0, reasoning_tokens: 0 },
  };
  // 仅在显式给出时携带(模拟旧服务端无 messages 字段的形态靠缺省)
  if (messages) detail.messages = messages;
  return detail;
}

function makeMsg(role: TaskMessageRole, kind: TaskMessageKind, content: string, seq: number): TaskMessage {
  return {
    id: `m${seq}`,
    task_id: 't1',
    role,
    kind,
    content,
    created_at: `2026-09-03T00:00:0${seq}.000Z`,
  };
}

/** 以 pinia 上下文 SSR 渲染组件为 HTML 字符串;seed 在渲染前对 task store 播种状态 */
async function render(comp: Component, seed?: (pinia: Pinia) => void): Promise<string> {
  const pinia = createPinia();
  setActivePinia(pinia);
  seed?.(pinia);
  const app = createSSRApp({ render: () => h(comp) });
  app.use(pinia);
  return renderToString(app);
}

/** 播种当前任务详情(选中态;供 TaskBoard 渲染) */
function seedCurrentTask(pinia: Pinia, detail: TaskDetail): void {
  void pinia;
  const task = useTaskStore();
  task.currentTask = detail;
  task.currentTaskId = detail.task.id;
}

beforeEach(() => {
  memStorage.clear();
});

describe('TaskBoard 批次 R2a:追加指令条', () => {
  // 精确匹配标题 div 收尾:SSR 输出保留模板注释,纯文本断言会被注释文本污染
  //(同 taskModeUi.test.ts 的「计划步骤</div>」先例)
  it('done 终态:输入条出现且可用(textarea/按钮无 disabled),禁用提示不出现', async () => {
    const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(makeTask('done'), [])));
    expect(html).toContain('追加指令</div>');
    // textarea 未禁用(SSR 对 disabled=true 输出 disabled 属性,false 时无该属性)
    expect(html).not.toMatch(/<textarea[^>]*\sdisabled/);
    expect(html).not.toContain('sv-task-followup-hint');
    expect(html).toContain('在既有成果基础上继续补充或修改');
  });

  it('partial/error/ended 同为终态:输入条可用', async () => {
    for (const st of ['partial', 'error', 'ended'] as TaskStatus[]) {
      const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(makeTask(st), [])));
      expect(html).not.toMatch(/<textarea[^>]*\sdisabled/, `${st} 终态输入条不应禁用`);
      expect(html).not.toContain('sv-task-followup-hint');
    }
  });

  it('running:输入条禁用并提示「任务执行中」', async () => {
    const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(makeTask('running'), [])));
    expect(html).toContain('追加指令</div>');
    expect(html).toMatch(/<textarea[^>]*\sdisabled/);
    expect(html).toContain('任务执行中');
  });

  it('planned:输入条禁用并提示「计划待批准」(批准/对话走批准区,R2b)', async () => {
    const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(makeTask('planned'), [])));
    expect(html).toMatch(/<textarea[^>]*\sdisabled/);
    expect(html).toContain('计划待批准');
  });

  it('pending:输入条禁用并提示「任务尚未执行」', async () => {
    const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(makeTask('pending'), [])));
    expect(html).toMatch(/<textarea[^>]*\sdisabled/);
    expect(html).toContain('任务尚未执行');
  });
});

describe('TaskBoard 批次 R2:用户指令历史区', () => {
  it('渲染 user 气泡(原文 + 「追加」标签)与 assistant 截断概要,按 created_at 升序', async () => {
    const longOutput = '追加产出正文。' + '产'.repeat(200); // 超 160 字触发截断
    const messages: TaskMessage[] = [
      makeMsg('user', 'followup', '再补充一点秋色', 1),
      makeMsg('assistant', 'followup', longOutput, 2),
      makeMsg('user', 'followup', '再润色一遍结尾', 3),
    ];
    const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(makeTask('done'), messages)));
    // 标题 div 收尾精确匹配(防模板注释污染)
    expect(html).toContain('用户指令历史</div>');
    // user 气泡样式类(搬 ChatMessageItem user 分支)+ 指令原文
    expect(html).toContain('sv-msg user');
    expect(html).toContain('sv-msg-bubble');
    expect(html).toContain('再补充一点秋色');
    expect(html).toContain('再润色一遍结尾');
    // kind 小标签(span 收尾精确匹配,防「追加指令」标题歧义)
    expect(html).toContain('>追加</span>');
    // assistant 概要:截断为 160 字 + …,精确匹配 span 可见文本(> 前缀定位到标签
    // 收尾,避免与 title 属性里的全文混淆——title 携带全文悬停可见,是预期形态)
    const expectedPreview = `${longOutput.trim().slice(0, 160)}…`;
    expect(html).toContain(`>${expectedPreview}</span>`);
    expect(html).not.toContain(`>${longOutput.trim()}</span>`);
  });

  it('旧服务端详情无 messages 字段:历史区不渲染,输入条照常可用(读取容错)', async () => {
    const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(makeTask('done'))));
    expect(html).not.toContain('用户指令历史</div>');
    expect(html).toContain('追加指令</div>');
    expect(html).not.toMatch(/<textarea[^>]*\sdisabled/);
  });
});

describe('TaskBoard 批次 R2b:批准区「与规划器对话」', () => {
  const plannedPlan: TaskStep[] = [
    { name: '搜集资料', goal: '收集季度数据', status: 'pending', result: '' },
  ];

  it('planned 态:批准区渲染对话输入框与 plan_chat 对话记录;底部历史区不重复显示 plan_chat', async () => {
    const messages: TaskMessage[] = [
      makeMsg('user', 'plan_chat', '把步骤换成先做竞品调研', 1),
      makeMsg('assistant', 'plan_chat', '计划已修订,共 1 步:修订步骤甲', 2),
    ];
    // planned + plan 模式(批准区清单由 plannedPlanHtml 渲染需 mode=plan)
    const task = makeTask('planned', '计划已修订,共 1 步:\n1. 修订步骤甲:修订目标甲', 'plan', plannedPlan);
    const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(task, messages)));
    // 批准区对话区存在(标题 div 收尾精确匹配,防注释污染)
    expect(html).toContain('与规划器对话(按反馈修订计划)</div>');
    // plan_chat 对话记录:user 气泡原文 + assistant 修订说明
    expect(html).toContain('把步骤换成先做竞品调研');
    expect(html).toContain('计划已修订,共 1 步:修订步骤甲');
    // 反馈输入框 placeholder
    expect(html).toContain('对计划提出修改意见');
    // planned 态 plan_chat 归批准区专属:底部历史区被过滤为空、整块不渲染
    expect(html).not.toContain('用户指令历史</div>');
    // 反馈原文仅出现一次(批准区;无同屏重复)
    const occurrences = html.split('把步骤换成先做竞品调研').length - 1;
    expect(occurrences).toBe(1);
  });

  it('done 态:plan_chat 历史回到底部历史区(带「规划对话」标签),批准区消失', async () => {
    const messages: TaskMessage[] = [
      makeMsg('user', 'plan_chat', '把步骤换成先做竞品调研', 1),
      makeMsg('assistant', 'plan_chat', '计划已修订,共 1 步:修订步骤甲', 2),
    ];
    const html = await render(TaskBoard, (p) => seedCurrentTask(p, makeDetail(makeTask('done'), messages)));
    expect(html).toContain('用户指令历史</div>');
    expect(html).toContain('把步骤换成先做竞品调研');
    // kind 小标签
    expect(html).toContain('>规划对话</span>');
    // 非 planned 态无批准区对话区
    expect(html).not.toContain('与规划器对话(按反馈修订计划)</div>');
  });
});
