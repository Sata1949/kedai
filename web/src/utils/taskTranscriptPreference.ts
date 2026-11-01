// 任务执行过程三档显示的偏好存储层(TMSHOW-3,2026-11-01)。
//
// 两级偏好,键空间**刻意同前缀**:
// - 默认档 `kedai.task-transcript-mode.v1`(设置页读写,全局唯一);
// - 单任务覆盖 `kedai.task-transcript-mode.v1.<taskId>`(任务详情顶栏写入)。
// 两级共用前缀不会互相串读——默认档键没有尾点和 id,任务键一定有,是两把不同的钥匙。
//
// 为什么单任务覆盖不塞进一个 JSON 映射(照 `renderHtmlPreference` 的做法):
// 任务 id 会随任务删除而消失,映射需要「随实体删除清条目」的维护点(见 lastPosition.ts);
// 而独立键在删任务时留几条几字节的孤儿,读取侧永不命中,代价远小于维护一张同步表。
// 孤儿清理登记在 `docs/遗留.md` 的 TMSHOW-L1(本批不做)。
//
// 注入式 `StorageLike` 而非直接摸 `localStorage`:存储层要能在纯 node 环境单测,
// 也便于上层在隐私模式/配额满(读写抛异常)时保持「回退默认、不崩」的语义。

import { TRANSCRIPT_MODES, type TranscriptMode } from './taskTranscript';

/** 默认档位的存储键(与单任务覆盖键共享前缀,见文件头) */
export const DEFAULT_TRANSCRIPT_MODE_KEY = 'kedai.task-transcript-mode.v1';

/** 单任务覆盖键的前缀(尾部带点,拼上 taskId) */
const TASK_OVERRIDE_PREFIX = 'kedai.task-transcript-mode.v1.';

/** 最小存储接口(与 `onboarding.ts` / `renderHtmlPreference.ts` 同形态) */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/**
 * 该任务的覆盖键。
 *
 * 空 id 回空串而不是拼出 `...v1.`:后者在读取侧会把「没有任务」伪装成「有一条覆盖」,
 * 且与默认档键仅差一个尾点,肉眼几乎分辨不出。调用方据此跳过存取。
 */
export function transcriptModeKey(taskId: string): string {
  const id = taskId.trim();
  return id ? `${TASK_OVERRIDE_PREFIX}${id}` : '';
}

/** 值是否是一个合法档位(白名单校验;localStorage 里可能是任意历史脏值) */
function isTranscriptMode(v: string | null): v is TranscriptMode {
  return v !== null && (TRANSCRIPT_MODES as readonly string[]).includes(v);
}

/**
 * 读默认档。
 *
 * 未设置、非法值、存储不可用一律回 `full`——**不是**回 `legacy`:
 * TMSHOW-2 拍板的默认档就是全消息,`legacy` 只是降级后的落点(轨迹为空时)。
 * 把「读不到」当成「用户选了步骤视图」会让大多数用户第一次打开就少看一档。
 */
export function readDefaultTaskTranscriptMode(storage: StorageLike): TranscriptMode {
  try {
    const v = storage.getItem(DEFAULT_TRANSCRIPT_MODE_KEY);
    return isTranscriptMode(v) ? v : 'full';
  } catch {
    return 'full';
  }
}

/** 写默认档(存储不可用时静默忽略,仅本次会话内生效) */
export function writeDefaultTaskTranscriptMode(storage: StorageLike, mode: TranscriptMode): void {
  try {
    storage.setItem(DEFAULT_TRANSCRIPT_MODE_KEY, mode);
  } catch {
    /* 忽略 */
  }
}

/**
 * 读某任务的档位覆盖。
 *
 * `null` = 该任务没有覆盖,**跟随默认档**——这与「覆盖值恰好等于默认档」是两件不同的事:
 * 前者会随设置页改默认而变,后者不会。故这里不能拿默认档兜底。
 */
export function readTaskTranscriptOverride(
  storage: StorageLike,
  taskId: string,
): TranscriptMode | null {
  const key = transcriptModeKey(taskId);
  if (!key) return null;
  try {
    const v = storage.getItem(key);
    return isTranscriptMode(v) ? v : null;
  } catch {
    return null;
  }
}

/** 写某任务的档位覆盖(空 id 或存储不可用时静默跳过) */
export function writeTaskTranscriptOverride(
  storage: StorageLike,
  taskId: string,
  mode: TranscriptMode,
): void {
  const key = transcriptModeKey(taskId);
  if (!key) return;
  try {
    storage.setItem(key, mode);
  } catch {
    /* 忽略 */
  }
}
