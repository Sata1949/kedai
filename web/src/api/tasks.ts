// 任务模式 API:任务 CRUD + 执行/停止。
// 后端契约:GET /api/tasks → { tasks };POST /api/tasks → { ok, task };
// GET /api/tasks/{id} → { task, subtasks };POST /api/tasks/{id}/run | /stop;
// DELETE /api/tasks/{id} → 204。
import { request } from './client';
import type { TaskDetail, TaskRecord } from './types';

/** 读取任务列表(最新在前) */
export async function listTasks(): Promise<TaskRecord[]> {
  const data = await request<{ tasks: TaskRecord[] }>('/tasks');
  return data.tasks;
}

/** 新建任务;character_id 可选(执行者人设角色) */
export async function createTask(title: string, characterId?: string): Promise<TaskRecord> {
  const data = await request<{ ok: boolean; task: TaskRecord }>('/tasks', {
    method: 'POST',
    body: JSON.stringify({ title, character_id: characterId ?? null }),
  });
  return data.task;
}

/** 读取任务详情(含子任务) */
export async function getTask(id: string): Promise<TaskDetail> {
  return request<TaskDetail>(`/tasks/${id}`);
}

/** 启动任务执行(后台) */
export async function runTask(id: string): Promise<void> {
  await request<{ ok: boolean }>(`/tasks/${id}/run`, { method: 'POST' });
}

/** 停止任务执行 */
export async function stopTask(id: string): Promise<void> {
  await request<{ ok: boolean }>(`/tasks/${id}/stop`, { method: 'POST' });
}

/** 删除任务(含子任务) */
export async function deleteTask(id: string): Promise<void> {
  await request<void>(`/tasks/${id}`, { method: 'DELETE' });
}
