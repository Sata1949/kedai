// 回退快照(undo)API(批次 6.1b;批 D 起按调用身份精确绑定):
// 后端契约:GET /api/chat/sessions/{id}/undo → { snapshots }(新→旧排序,可带 call_id);
// POST /api/undo/{id}/restore(空 JSON 体)→ { ok, restored, restored_count };失败后端 500 带中文错误。
// 快照仅对写工具(write/replace/create/update_variables/memory_write)且 undo_enabled 开时产生;
// 恢复语义(批 D-2)= 回到所选快照**之前**的状态:目标及其后续快照逆序应用后一并清理,
// 成功后 restored_count 为实际恢复的快照条数。
import { request } from './client';

/** 回退快照(写工具执行前的状态存档) */
export interface UndoSnapshot {
  id: string;
  /** 产生快照的写工具名(write / replace / create / update_variables / memory_write) */
  tool_name: string;
  /** 中文简述标签(如「写入文件 notes.md」「更新变量树」) */
  label: string;
  /** 锚定消息 id(快照产生时所在的聊天消息;可能为 null) */
  anchor_message_id: number | null;
  created_at: string;
  /** 产生快照的工具调用 call_id(批 D-1;旧记录缺省——不伪造对应关系) */
  call_id?: string | null;
}

/** 回退结果(批 D-2):restored = 点击目标快照的中文 label;restored_count = 实际恢复条数 */
export interface UndoRestoreResult {
  restored: string;
  restored_count: number;
}

/** 读取会话的回退快照列表(新→旧排序) */
export async function listUndoSnapshots(sessionId: string): Promise<UndoSnapshot[]> {
  const data = await request<{ snapshots: UndoSnapshot[] }>(
    `/chat/sessions/${encodeURIComponent(sessionId)}/undo`,
  );
  return data.snapshots;
}

/** 回退到指定快照(回到该快照执行前的状态;目标及后续快照被一并应用并清理) */
export async function restoreUndoSnapshot(id: string): Promise<UndoRestoreResult> {
  const data = await request<{ ok: boolean; restored?: string; restored_count?: number }>(
    `/undo/${encodeURIComponent(id)}/restore`,
    {
      method: 'POST',
      body: JSON.stringify({}),
    },
  );
  return { restored: data.restored ?? '', restored_count: data.restored_count ?? 1 };
}
