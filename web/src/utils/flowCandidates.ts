// 可调用流程**名单的候选计算与失效判定**(根流程不可勾选 / 停用不可勾选 / 已失效成员保位)。
//
// 为什么单独成模块:同一套判定有两处消费方——创建任务的选择器(`TaskFlowSelect.vue`,二维
// 批次 7b)与任务改绑的入选区(`TaskBoard.vue`,B 批 B3)。判定各写一份必然漂移
// (「创建时不让勾、改绑时却能勾」这类只有实跑才会发现的不一致),故**判定**收在这里,
// 两处只负责渲染与各自的警示文案——创建期「空名单」非法、改绑期「空名单」= 强制模式,
// 语义本就不同,文案自然不同(不强行合并成一句)。
import type { AgentFlowConfig } from '../api/types';

/** 一个勾选行(根流程也在列表里:它只作展示并禁用) */
export interface FlowCandidate {
  id: string;
  label: string;
  disabled: boolean;
  stale: boolean;
}

/**
 * 勾选行 = 库内流程(根流程禁用 + 已停用禁用)+ 名单里**库里已不存在**的项(可取消勾选)。
 *
 * `rootId` 由调用方按同一口径给出:显式绑定优先,否则「跟随当前流程」= 库的当前流程
 * ——与后端「可调用集扣除根流程」同源(根流程正在执行,调用它必然撞调用链环守卫)。
 *
 * 已失效项**保位**展示而不是静默丢弃:它会让创建/保存被后端 400 挡下,偷偷删掉则用户
 * 无从知道点「保存」之前发生了什么。
 */
export function flowCandidates(
  flows: AgentFlowConfig[],
  rootId: string,
  selectedIds: string[],
): FlowCandidate[] {
  const out: FlowCandidate[] = [];
  for (const f of flows) {
    // 库内流程的 id 由后端分配(TaskSend 类型标 optional,故这里显式挡一次脏数据)
    const id = f.id ?? '';
    if (!id) continue;
    const name = f.name || id;
    if (id === rootId) {
      out.push({ id, label: `${name}(根流程,不可调用)`, disabled: true, stale: false });
      continue;
    }
    out.push({
      id,
      label: f.enabled ? name : `${name}(已停用)`,
      disabled: !f.enabled,
      stale: false,
    });
  }
  for (const id of selectedIds) {
    if (!flows.some((f) => f.id === id)) {
      out.push({ id, label: `(已失效) ${id}`, disabled: false, stale: true });
    }
  }
  return out;
}

/** 可勾选的候选数(排除根流程、已停用、已失效)——「一个都勾不了」的即时警示用它 */
export function usableCandidates(candidates: FlowCandidate[]): number {
  return candidates.filter((c) => !c.disabled && !c.stale).length;
}

/** 名单里**库中已不存在**的成员 id(创建与改绑都会被后端 400 挡下) */
export function staleMembers(candidates: FlowCandidate[], selectedIds: string[]): string[] {
  return selectedIds.filter((id) => candidates.some((c) => c.id === id && c.stale));
}
