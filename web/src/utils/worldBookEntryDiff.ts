// 世界书保存的 id 差集(WBCONC 提交 2):前端打开编辑器时记基线 id 集,保存时把
// 「基线 − 当前」作为显式删除、「当前 − 基线」作为显式新增发给后端。
//
// 为什么不能只发快照让后端猜删除:不在视图内的条目可能是窗口期模型/卡脚本新写入的,
// 服务端无法区分「用户删了」与「用户没加载到」。显式 removed 是唯一可靠信号。
// 新增同理:本地新增行 id 是草稿 max+1,窗口期模型也可能从同一 max 出发——撞号时
// 后端必须靠显式 added 才敢重分配,否则会把模型新条目当旧条目覆盖。
import type { WorldBookEntry } from '../api';

export interface EntryIdDiff {
  removed: number[];
  added: number[];
}

/** 基线 id 集与当前草稿 id 集的差集(保持输入顺序,便于测试与日志阅读) */
export function diffEntryIds(
  baseline: Iterable<number>,
  current: Iterable<number>,
): EntryIdDiff {
  const base = new Set(baseline);
  const cur = new Set(current);
  return {
    removed: [...base].filter((id) => !cur.has(id)),
    added: [...cur].filter((id) => !base.has(id)),
  };
}

/**
 * 与服务端 `normalize_entry` 同口径的「可入库」判定:content / keys / regex 全空
 * 的条目会被 GET 过滤,不应作为保存响应覆盖本地草稿的依据(否则空行保存后凭空消失)。
 */
export function isRenderableEntry(
  entry: Pick<WorldBookEntry, 'content' | 'keys' | 'regex'>,
): boolean {
  const content = entry.content ?? '';
  const keys = (entry.keys ?? []).filter((k) => k.length > 0);
  const regex = entry.regex ?? '';
  return content.length > 0 || keys.length > 0 || regex.length > 0;
}
