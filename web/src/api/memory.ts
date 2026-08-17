// 记忆库 API:跨会话记忆蒸馏(server-rs/src/api/memory.rs)。
//   POST   /api/memory/distill   蒸馏指定会话(需开启 memory_distill_enabled)
//   GET    /api/memory           按角色列出全部记忆(最新在前,含未选中条目与计数)
//   POST   /api/memory           手动添加(kind='manual')
//   PATCH  /api/memory/:id       编辑 content / selected
//   DELETE /api/memory/:id       删除(204 无正文)
import { request } from './client';

/** 记忆条目(memory_entries 表行;kind: distilled | tool | manual) */
export interface MemoryEntry {
  id: number;
  character_id: string;
  /** 来源会话(蒸馏记忆记录来源;手动/工具记忆为 null) */
  source_session_id: string | null;
  kind: string;
  content: string;
  /** 注入次数(每次注入后 +1) */
  usage_count: number;
  /** 最近一次注入时间(null = 从未注入) */
  last_usage: string | null;
  /** 是否参与注入(精选开关) */
  selected: boolean;
  created_at: string;
  updated_at: string;
}

/** 蒸馏结果:inserted = 本次落库条数(空历史为 0,不调模型) */
export interface DistillResult {
  ok: boolean;
  inserted: number;
  character_id: string;
}

/** GET /api/memory?character_id=:角色全部记忆(最新在前) */
export async function listMemories(characterId: string): Promise<MemoryEntry[]> {
  const data = await request<{ memories: MemoryEntry[] }>(
    `/memory?character_id=${encodeURIComponent(characterId)}`,
  );
  return data.memories;
}

/** POST /api/memory/distill:蒸馏当前会话为角色记忆(未开启蒸馏时 400 带指引文案) */
export async function distillMemory(sessionId: string): Promise<DistillResult> {
  return request('/memory/distill', {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId }),
  });
}

/** POST /api/memory:手动添加一条记忆(kind='manual'),返回落库条目 */
export async function createMemory(characterId: string, content: string): Promise<MemoryEntry> {
  const data = await request<{ ok: boolean; memory: MemoryEntry }>('/memory', {
    method: 'POST',
    body: JSON.stringify({ character_id: characterId, content }),
  });
  return data.memory;
}

/** PATCH /api/memory/:id:编辑 content 与/或 selected(空白 content 后端拒绝) */
export async function updateMemory(
  id: number,
  patch: { content?: string; selected?: boolean },
): Promise<MemoryEntry> {
  const data = await request<{ ok: boolean; memory: MemoryEntry }>(`/memory/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(patch),
  });
  return data.memory;
}

/** DELETE /api/memory/:id:删除一条记忆(204 无正文) */
export async function deleteMemory(id: number): Promise<void> {
  await request(`/memory/${id}`, { method: 'DELETE' });
}
