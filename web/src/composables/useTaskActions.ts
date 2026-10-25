// 任务执行动作（C-2A，2026-10-25）：执行 / 停止 / 删除的单一出口。
//
// 原实现把 `runCurrent` / `stopCurrent` / `removeTask` 各写两遍（TaskBoard 与 Sidebar 的
// removeTask 逐字相同）。收敛到 composable 后，任务状态面板、侧栏任务历史与 pending 卡
// 都走同一份确认文案与失败提示；状态变更仍只经 store action，不在组件里直写。
import { storeToRefs } from 'pinia';
import { useAppStore } from '../store';
import type { TaskRecord } from '../api';

export function useTaskActions() {
  const store = useAppStore();
  const { currentTaskId } = storeToRefs(store);

  /** 执行当前任务 */
  async function runCurrent(): Promise<void> {
    const id = currentTaskId.value;
    if (!id) return;
    try {
      await store.runTask(id);
    } catch (err) {
      alert(`执行失败:${(err as Error).message}`);
    }
  }

  /** 停止当前任务 */
  async function stopCurrent(): Promise<void> {
    const id = currentTaskId.value;
    if (!id) return;
    try {
      await store.stopTask(id);
    } catch (err) {
      alert(`停止失败:${(err as Error).message}`);
    }
  }

  /** 删除任务（确认文案与原两处实现逐字一致） */
  async function removeTask(task: TaskRecord): Promise<void> {
    if (!confirm(`确定删除任务「${task.title}」?其子任务将一并删除。`)) return;
    try {
      await store.deleteTask(task.id);
    } catch (err) {
      alert(`删除失败:${(err as Error).message}`);
    }
  }

  return { runCurrent, stopCurrent, removeTask };
}
