// 工作区 API(CODE-4):项目类型画像探测。
//
// 为什么要有独立端点:创建表单在**任务存在之前**就要显示「这个目录是什么项目」,
// 详情接口那时还没得查。任务详情顶层的 `workspace_profile` 与它是同一个后端函数
// (`services::workspace_profile::probe`)的两处出口。
//
// 路径用 `URLSearchParams` 编码,不手工拼接——Windows 绝对路径含 `\` 与盘符冒号,
// 中文目录名也常见,手工拼 query 会失真(`AGENTS.md` 记过同类教训:curl 直发中文 JSON 被破坏)。
import { request } from './client';
import { requireObjectField } from './shape';
import type { WorkspaceProfile } from './types';

/** 探测工作区的项目类型画像(只读;目录不存在 / 非目录 / 与数据目录冲突 → 后端 400 中文原因) */
export async function probeWorkspace(path: string): Promise<WorkspaceProfile> {
  const q = new URLSearchParams({ path });
  const data = await request<unknown>(`/workspace/profile?${q.toString()}`);
  return requireObjectField<WorkspaceProfile>(data, 'profile', '工作区画像');
}
