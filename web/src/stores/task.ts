// 任务模式 store:顶层模式(roleplay/task)切换与持久化、任务列表/详情、执行轮询。
// 从 store.ts 按领域拆分。跨 store 引用(genSettings.loadSettings / uiPrefs.agentPanelOpen /
// chat.onSseEvent 事件上报)均在动作运行时解析,setup 阶段不实例化其他 store。
import { defineStore } from 'pinia';
import { ref } from 'vue';
import * as api from '../api';
import { useChatStore } from './chat';
import { useGenSettingsStore } from './genSettings';
import { useUiPrefsStore } from './uiPrefs';

/** 顶层模式持久化键:刷新后停留在上次模式 */
const APP_MODE_KEY = 'kedai.appMode';

function readStoredAppMode(): 'roleplay' | 'task' {
  try {
    return localStorage.getItem(APP_MODE_KEY) === 'task' ? 'task' : 'roleplay';
  } catch {
    return 'roleplay';
  }
}

export const useTaskStore = defineStore('app.task', () => {
  // ===== 状态 =====
  /** 顶层模式:roleplay = 角色扮演,task = 任务工作台;持久化到 localStorage */
  const appMode = ref<'roleplay' | 'task'>(readStoredAppMode());
  const tasks = ref<api.TaskRecord[]>([]);
  const currentTaskId = ref<string | null>(null);
  /** 当前任务详情(含子任务) */
  const currentTask = ref<api.TaskDetail | null>(null);
  /** 任务轮询定时器句柄(非响应式) */
  let taskPollTimer: ReturnType<typeof setInterval> | null = null;

  function persistAppMode(): void {
    try {
      localStorage.setItem(APP_MODE_KEY, appMode.value);
    } catch {
      /* 忽略 */
    }
  }

  /** 最近一次任务事件的 task_id 与详情签名:轮询每秒一次,仅状态变化时发事件,避免刷屏 */
  let lastTaskEventId = '';
  let lastTaskEventSig = '';

  /** 任务详情签名(任务状态 + 计划步骤状态 + 子任务状态),用于检测轮询间变化 */
  function taskDetailSig(d: api.TaskDetail): string {
    return [
      d.task.status,
      d.task.plan.map((s) => s.status).join(','),
      d.subtasks.map((s) => s.status).join(','),
    ].join('|');
  }

  /** 切换顶层模式;进入任务模式时加载任务列表,退出时停止轮询;并加载该模式的设置 */
  function setAppMode(mode: 'roleplay' | 'task'): void {
    appMode.value = mode;
    if (mode === 'task') {
      void loadTasks();
      useUiPrefsStore().agentPanelOpen = false;
    } else {
      stopTaskPolling();
    }
    void useGenSettingsStore().loadSettings();
    persistAppMode();
  }

  async function loadTasks(): Promise<void> {
    try {
      tasks.value = await api.listTasks();
    } catch (e) {
      console.error('加载任务列表失败', e);
    }
  }

  async function createTask(title: string, characterId?: string): Promise<api.TaskRecord> {
    const task = await api.createTask(title, characterId);
    tasks.value = [task, ...tasks.value];
    useChatStore().onSseEvent({ type: 'task', task_id: task.id, title: task.title, status: task.status, detail: '任务已创建' });
    await selectTask(task.id);
    return task;
  }

  async function selectTask(id: string): Promise<void> {
    currentTaskId.value = id;
    await loadTaskDetail(id);
  }

  async function loadTaskDetail(id: string): Promise<void> {
    try {
      const detail = await api.getTask(id);
      currentTask.value = detail;
      // 事件监控:首次加载或状态变化时发任务事件(轮询去重,同状态不重复)
      const sig = taskDetailSig(detail);
      if (detail.task.id !== lastTaskEventId || sig !== lastTaskEventSig) {
        lastTaskEventId = detail.task.id;
        lastTaskEventSig = sig;
        const doneSteps = detail.task.plan.filter((s) => s.status === 'done').length;
        const doneSubs = detail.subtasks.filter((s) => s.status === 'done').length;
        useChatStore().onSseEvent({
          type: 'task',
          task_id: detail.task.id,
          title: detail.task.title,
          status: detail.task.status,
          detail: detail.task.plan.length
            ? `计划步骤 ${doneSteps}/${detail.task.plan.length}`
            : detail.subtasks.length
              ? `子任务 ${doneSubs}/${detail.subtasks.length}`
              : undefined,
        });
      }
    } catch (e) {
      console.error('加载任务详情失败', e);
    }
  }

  async function runTask(id: string): Promise<void> {
    await api.runTask(id);
    useChatStore().onSseEvent({ type: 'task', task_id: id, status: 'running', detail: '执行已启动' });
    await loadTaskDetail(id);
    startTaskPolling();
  }

  async function stopTask(id: string): Promise<void> {
    await api.stopTask(id);
    useChatStore().onSseEvent({ type: 'task', task_id: id, detail: '已请求停止' });
    stopTaskPolling();
    await loadTaskDetail(id);
  }

  async function deleteTask(id: string): Promise<void> {
    await api.deleteTask(id);
    useChatStore().onSseEvent({ type: 'task', task_id: id, detail: '任务已删除' });
    tasks.value = tasks.value.filter((t) => t.id !== id);
    if (currentTaskId.value === id) {
      currentTaskId.value = null;
      currentTask.value = null;
      stopTaskPolling();
    }
  }

  /** 执行期间每 1s 轮询当前任务详情;终态(done/error/ended)自动停止并刷新列表 */
  function startTaskPolling(): void {
    stopTaskPolling();
    taskPollTimer = setInterval(() => {
      const id = currentTaskId.value;
      if (!id) {
        stopTaskPolling();
        return;
      }
      void loadTaskDetail(id).then(() => {
        const s = currentTask.value?.task.status;
        if (s === 'done' || s === 'error' || s === 'ended') {
          stopTaskPolling();
          void loadTasks();
        }
      });
    }, 1000);
  }

  function stopTaskPolling(): void {
    if (taskPollTimer) {
      clearInterval(taskPollTimer);
      taskPollTimer = null;
    }
  }

  return {
    appMode,
    tasks,
    currentTaskId,
    currentTask,
    setAppMode,
    loadTasks,
    createTask,
    selectTask,
    loadTaskDetail,
    runTask,
    stopTask,
    deleteTask,
    startTaskPolling,
    stopTaskPolling,
  };
});
