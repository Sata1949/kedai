// 任务执行者库 API(2026-09-17 执行者与角色扮演角色卡解耦)。
// 后端契约见 server-rs/src/api/task_executors.rs:
//   GET    /api/task-executors        → { ok, executors }
//   POST   /api/task-executors        → { ok, saved, executors }(config.id 空 = 新建)
//   DELETE /api/task-executors/{id}   → { ok, executors }
//
// 与 characters.ts 的差别(正是本次改造的目的):执行者不复用角色卡记录结构,
// 只有「名称 + 执行者指令 + 可选温度」,前端渲染与后端校验都以此为准。
import { request } from './client';
import { requireArrayField, requireObjectField } from './shape';

/** 任务执行者(与 Rust services::executor_service::TaskExecutorConfig 对齐) */
export interface TaskExecutor {
  /** 执行者 id(新建时为空,由后端分配) */
  id: string;
  /** 执行者名称(任务创建下拉与库列表展示) */
  name: string;
  /** 执行者指令:该执行者的职责/工作方式,整段注入任务执行 system 提示词 */
  instruction: string;
  /** 建议温度(可选;省略 = 沿用任务模式默认温度) */
  temperature?: number | null;
  created_at?: string;
  updated_at?: string;
}

/** 待保存的执行者(新建时 id 省略) */
export interface TaskExecutorInput {
  id?: string;
  name: string;
  instruction: string;
  temperature?: number | null;
}

/** GET /api/task-executors:执行者列表(库内顺序) */
export async function listExecutors(): Promise<TaskExecutor[]> {
  const data = await request<unknown>('/task-executors');
  // 形状闸门:任务创建下拉直接渲染该列表
  return requireArrayField<TaskExecutor>(data, 'executors', '执行者列表');
}

/**
 * POST /api/task-executors:保存(创建或更新)执行者并返回最新列表。
 * 名称/指令为空、温度越界 → 后端 400(VALIDATION)。
 */
export async function saveExecutor(
  config: TaskExecutorInput,
): Promise<{ saved: TaskExecutor; executors: TaskExecutor[] }> {
  const data = await request<unknown>('/task-executors', {
    method: 'POST',
    body: JSON.stringify({ config }),
  });
  const saved = requireObjectField<TaskExecutor>(data, 'saved', '执行者');
  const executors = requireArrayField<TaskExecutor>(data, 'executors', '执行者列表');
  return { saved, executors };
}

/**
 * DELETE /api/task-executors/{id}:删除执行者并返回最新列表。
 * 引用该执行者的任务不受影响:执行期查不到配置即回退通用执行者。
 */
export async function deleteExecutor(id: string): Promise<TaskExecutor[]> {
  const data = await request<unknown>(`/task-executors/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
  return requireArrayField<TaskExecutor>(data, 'executors', '执行者列表');
}
