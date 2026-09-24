// Agent 执行流程节点的**调用预算**纯函数(A 批 A1/A2:单次调用超时 + 空产出重试)。
//
// 与后端同口径(`server-rs/src/services/agent_flow_service.rs` 的保存期校验):
//   - `PlanStep.call_timeout_secs` ∈ 30..=3600 秒,缺省(无值)= 沿用宿主 300 秒看门狗;
//   - `PlanStep.max_retries` ∈ 1..=5(额外尝试上限,总尝试 = 1 + n),缺省 = 不重试。
// 两个字段都是**加性**的:置空即写回 null,序列化时整键省略(存量流程逐字节不变)。
// 挂载子流程的节点上两者都被旁路(子图各节点各自的配置生效),草稿里的配置保留。
//
// 与 agentFlowConnections / agentFlowTools 同一组织方式:从组件抽出为纯函数便于单测,
// 区间常量与后端校验同源(两端改区间必须同批改这里,`agentFlowStepLimits.test.ts` 锁定)。

import type { AgentFlowStep } from '../api/types';

/** 节点级单次调用超时的取值范围(与后端 `MIN_STEP_CALL_TIMEOUT_SECS` / `MAX_STEP_CALL_TIMEOUT_SECS` 一致) */
export const STEP_CALL_TIMEOUT_MIN = 30;
export const STEP_CALL_TIMEOUT_MAX = 3600;

/** 节点级空产出重试次数的取值范围(与后端 `MIN_STEP_MAX_RETRIES` / `MAX_STEP_MAX_RETRIES` 一致) */
export const STEP_MAX_RETRIES_MIN = 1;
export const STEP_MAX_RETRIES_MAX = 5;

/**
 * 读回节点的单次调用超时输入框文本(缺省 = 空串,表示沿用宿主缺省)
 */
export function stepCallTimeoutText(s: Pick<AgentFlowStep, 'call_timeout_secs'>): string {
  return typeof s.call_timeout_secs === 'number' ? String(s.call_timeout_secs) : '';
}

/**
 * 写入节点的单次调用超时:空/非法 → null(沿用宿主缺省);合法则夹到 30-3600。
 * 夹取而不是拒绝:输入框是数字控件,越界值多半是手滑,夹取后保存期必过
 * (与 `setStepToolRounds` 同一口径)。
 */
export function setStepCallTimeout(
  s: Pick<AgentFlowStep, 'call_timeout_secs'>,
  text: string,
): void {
  const t = text.trim();
  if (t === '') {
    s.call_timeout_secs = null;
    return;
  }
  const n = Number(t);
  if (!Number.isFinite(n)) {
    s.call_timeout_secs = null;
    return;
  }
  s.call_timeout_secs = Math.min(
    STEP_CALL_TIMEOUT_MAX,
    Math.max(STEP_CALL_TIMEOUT_MIN, Math.trunc(n)),
  );
}

/**
 * 单次调用超时的警示(空数组 = 无警示)。
 * 挂载子流程的节点上本字段被旁路——子图各节点各自计时(配置保留,清空挂载即生效)。
 * 注:这类节点的执行参数区整块不渲染,该条属**防御性**提示(与 `stepConnectionWarnings` 同款)。
 */
export function stepCallTimeoutWarnings(
  s: Pick<AgentFlowStep, 'call_timeout_secs' | 'sub_flow_id'>,
): string[] {
  if (typeof s.call_timeout_secs !== 'number') return [];
  if ((s.sub_flow_id ?? '').trim().length === 0) return [];
  return [
    '本步骤挂载了子流程:单次调用超时对其不生效(子图各节点各自计时),配置保留。',
  ];
}

/** 读回节点的空产出重试次数输入框文本(缺省 = 空串,表示不重试) */
export function stepMaxRetriesText(s: Pick<AgentFlowStep, 'max_retries'>): string {
  return typeof s.max_retries === 'number' ? String(s.max_retries) : '';
}

/**
 * 写入节点的空产出重试次数:空/非法 → null(不重试);合法则夹到 1-5。
 * 语义是**额外**尝试上限:填 2 = 最多再试 2 次(总尝试 3 次)。
 */
export function setStepMaxRetries(s: Pick<AgentFlowStep, 'max_retries'>, text: string): void {
  const t = text.trim();
  if (t === '') {
    s.max_retries = null;
    return;
  }
  const n = Number(t);
  if (!Number.isFinite(n)) {
    s.max_retries = null;
    return;
  }
  s.max_retries = Math.min(STEP_MAX_RETRIES_MAX, Math.max(STEP_MAX_RETRIES_MIN, Math.trunc(n)));
}

/**
 * 空产出重试的警示(空数组 = 无警示)。
 * 挂载子流程的节点上本字段被旁路(子图各节点各自重试),与超时同一纪律。
 */
export function stepMaxRetriesWarnings(
  s: Pick<AgentFlowStep, 'max_retries' | 'sub_flow_id'>,
): string[] {
  if (typeof s.max_retries !== 'number') return [];
  if ((s.sub_flow_id ?? '').trim().length === 0) return [];
  return [
    '本步骤挂载了子流程:空产出重试对其不生效(子图各节点各自重试),配置保留。',
  ];
}
