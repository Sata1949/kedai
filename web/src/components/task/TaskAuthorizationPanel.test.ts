// @vitest-environment jsdom
// 任务「工具调用待确认」卡片(2026-11-06 三档授权)的渲染与交互测试。
//
// 覆盖:空态不渲染 / 风险与原因展示 / 参数折叠 / 两条按钮的决议 / 失败态保留卡片 /
// 多条排队 / 超时时限文案。队列状态是模块级单例，每个用例前必须显式清空。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import TaskAuthorizationPanel from './TaskAuthorizationPanel.vue';
import { useAppStore } from '../../store';
import * as api from '../../api';
import {
  applyTaskToolAuthRequired,
  clearTaskToolAuth,
  useTaskToolAuth,
} from '../../composables/useTaskToolAuth';
import type { TaskDetail, TaskToolAuthorization } from '../../api';

function detail(tier: 'step' | 'workspace' | 'auto' | null = 'step'): TaskDetail {
  return {
    task: {
      id: 't1',
      title: '待确认任务',
      status: 'running',
      plan: [],
      result: '',
      error: '',
      task_mode: 'solo',
      tool_authorization: tier,
      created_at: '2026-11-06T00:00:00.000Z',
      updated_at: '2026-11-06T00:00:00.000Z',
    },
    subtasks: [],
    usage_total: { prompt_tokens: 0, completion_tokens: 0, reasoning_tokens: 0 },
    messages: [],
  };
}

function auth(over: Partial<TaskToolAuthorization> = {}): TaskToolAuthorization {
  return {
    name: 'fs_write',
    risk: 'sensitive',
    reason: '逐步授权:该工具会写入或发起执行,需要你确认后才执行',
    run_id: 'run-1',
    call_id: 'call-1',
    session_id: 'task:t1',
    code: 'step_confirm',
    input_preview: '{"path":"a.txt","content":"hello"}',
    ...over,
  };
}

function seed(item?: TaskToolAuthorization, tier: 'step' | 'workspace' | 'auto' | null = 'step') {
  setActivePinia(createPinia());
  const store = useAppStore();
  store.currentTask = detail(tier);
  // 队列归属到当前任务(正常路径由 store.selectTask 拉快照建立)
  useTaskToolAuth().ownerTaskId.value = 't1';
  if (item) {
    applyTaskToolAuthRequired({
      type: 'task',
      task_id: 't1',
      kind: 'tool_authorization_required',
      tool_authorization: item,
    });
  }
  return store;
}

beforeEach(() => {
  clearTaskToolAuth();
  vi.restoreAllMocks();
});

describe('TaskAuthorizationPanel 渲染', () => {
  it('没有待确认时整块不渲染（不给空态噪音）', () => {
    seed();
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    expect(wrapper.find('.sv-task-toolauth').exists()).toBe(false);
  });

  it('渲染工具名/风险中文/原因/原因码/参数折叠', () => {
    seed(auth());
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    expect(wrapper.find('.sv-task-toolauth').exists()).toBe(true);
    expect(wrapper.text()).toContain('工具调用待确认');
    expect(wrapper.text()).toContain('fs_write');
    expect(wrapper.text()).toContain('敏感');
    expect(wrapper.text()).toContain('逐步档确认');
    expect(wrapper.text()).toContain('需要你确认后才执行');
    expect(wrapper.find('.sv-task-toolauth-input pre').text()).toContain('hello');
  });

  it('多条待确认各自成卡，计数可见', () => {
    seed(auth({ call_id: 'call-1' }));
    applyTaskToolAuthRequired({
      type: 'task',
      task_id: 't1',
      kind: 'tool_authorization_required',
      tool_authorization: auth({ call_id: 'call-2', name: 'bash', risk: 'dangerous' }),
    });
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    expect(wrapper.findAll('.sv-task-toolauth-card')).toHaveLength(2);
    expect(wrapper.text()).toContain('2 项');
    expect(wrapper.text()).toContain('危险');
  });

  it('超时时限取自设置（缺省 300 秒）', () => {
    const store = seed(auth());
    store.toolAuthorizationTimeoutSecs = 120;
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    expect(wrapper.text()).toContain('等待超过 120 秒会自动拒绝');
  });

  it('档位回显写进提示（说明为什么会出现这张卡）', () => {
    seed(auth(), 'workspace');
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    expect(wrapper.text()).toContain('本任务授权档位：工作区内');
  });
});

describe('TaskAuthorizationPanel 决议', () => {
  it('「仅允许本次」用事件里的 session_id 回传，成功后出队并给回执', async () => {
    seed(auth());
    const resolve = vi.spyOn(api, 'resolveToolAuthorization').mockResolvedValue(undefined);
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    const allow = wrapper.findAll('button').find((b) => b.text().includes('仅允许本次'));
    expect(allow).toBeTruthy();
    await allow?.trigger('click');
    await flushPromises();
    expect(resolve).toHaveBeenCalledWith('fs_write', 'once', 'task:t1', 'run-1', 'call-1');
    expect(useTaskToolAuth().queue.value).toEqual([]);
    expect(wrapper.find('.sv-feedback.ok').text()).toContain('已允许本次执行');
  });

  it('「拒绝」下发 deny 并把失败/拒绝结果留在面板上', async () => {
    seed(auth());
    const resolve = vi.spyOn(api, 'resolveToolAuthorization').mockResolvedValue(undefined);
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    const deny = wrapper.findAll('button').find((b) => b.text().includes('拒绝'));
    await deny?.trigger('click');
    await flushPromises();
    expect(resolve).toHaveBeenCalledWith('fs_write', 'deny', 'task:t1', 'run-1', 'call-1');
    expect(wrapper.text()).toContain('已拒绝');
  });

  it('决议失败时卡片保留并显示后端原文', async () => {
    seed(auth());
    vi.spyOn(api, 'resolveToolAuthorization').mockRejectedValueOnce(
      new Error('待授权调用不存在或已结束'),
    );
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    const allow = wrapper.findAll('button').find((b) => b.text().includes('仅允许本次'));
    await allow?.trigger('click');
    await flushPromises();
    expect(wrapper.findAll('.sv-task-toolauth-card')).toHaveLength(1);
    expect(wrapper.find('.sv-feedback.err').text()).toContain('待授权调用不存在或已结束');
  });

  it('只剩回执时标题改为「工具授权结果」且不再显示操作说明', async () => {
    seed(auth());
    vi.spyOn(api, 'resolveToolAuthorization').mockResolvedValue(undefined);
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    const allow = wrapper.findAll('button').find((b) => b.text().includes('仅允许本次'));
    await allow?.trigger('click');
    await flushPromises();
    expect(wrapper.text()).toContain('工具授权结果');
    expect(wrapper.text()).not.toContain('工具调用待确认');
    expect(wrapper.find('.sv-task-toolauth-hint').exists()).toBe(false);
  });

  it('刷新按钮走快照端点（断线/超时后的恢复入口）', async () => {
    seed(auth());
    const snapshot = vi.spyOn(api, 'getTaskToolAuthorizations').mockResolvedValue([]);
    const wrapper = mount(TaskAuthorizationPanel, { props: { taskId: 't1' } });
    const refresh = wrapper.findAll('button').find((b) => b.text().includes('刷新'));
    await refresh?.trigger('click');
    await flushPromises();
    expect(snapshot).toHaveBeenCalledWith('t1');
    expect(wrapper.findAll('.sv-task-toolauth-card')).toHaveLength(0);
  });
});
