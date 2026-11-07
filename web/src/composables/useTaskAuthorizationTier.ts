// 新建任务参数：三档授权（2026-11-06）。
//
// 为什么单独成模块而不是塞进 `stores/task.ts`：该文件已在只降不升的 file-size 基线内，
// 而「读取持久化 → 校验脏值 → 写回」这段不需要 Pinia 的任何能力，独立后可脱离 store 单测。
// 形态沿用 `composables/useTaskTrace.ts` 的模块级 ref 共享状态先例。
import { ref } from 'vue';
import type { TaskAuthorizationTier } from '../api/types';

/** 持久化键（与其它逐任务参数同款：`kedai.<name>.v1`） */
export const TASK_AUTHORIZATION_KEY = 'kedai.taskAuthorizationTier.v1';

/** 合法档位白名单（单点维护；脏值一律回落默认，不放大权限） */
const TIERS: readonly TaskAuthorizationTier[] = ['step', 'workspace', 'auto'];

/**
 * 读取三档授权。缺省 `workspace` = 本档上线前的行为
 * （任务策略内工具自动执行、高危命令直接拒绝），故老用户升级后不受影响。
 *
 * **只认三个字面量**：手改成别的值（如 `'full'`）一律回落默认——授权档位不得被
 * 本地存储里的脏数据放大。
 */
export function readStoredTaskAuthorizationTier(): TaskAuthorizationTier {
  try {
    const raw = localStorage.getItem(TASK_AUTHORIZATION_KEY);
    return TIERS.includes(raw as TaskAuthorizationTier)
      ? (raw as TaskAuthorizationTier)
      : 'workspace';
  } catch {
    return 'workspace';
  }
}

/** 模块级共享状态（store 只转发，不复制一份） */
const taskAuthorizationTier = ref<TaskAuthorizationTier>(readStoredTaskAuthorizationTier());

/** 持久化当前档位（写失败退化为「本次会话有效」） */
export function persistTaskAuthorizationTier(): void {
  try {
    localStorage.setItem(TASK_AUTHORIZATION_KEY, taskAuthorizationTier.value);
  } catch {
    /* 忽略 */
  }
}

export function taskAuthorizationTierState() {
  return taskAuthorizationTier;
}

/**
 * 从 localStorage 重新载入档位（**仅供测试**在用例间隔离模块级状态）。
 *
 * 生产路径刻意不调它：档位是模块级单例，每次挂载都重读会把「用户刚选的值」
 * 覆盖成本地存储里的旧值（写入是异步 watch，中间存在一瞬窗口）。
 */
export function reloadTaskAuthorizationTierForTest(): TaskAuthorizationTier {
  taskAuthorizationTier.value = readStoredTaskAuthorizationTier();
  return taskAuthorizationTier.value;
}
