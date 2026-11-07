// 任务工具待确认队列（2026-11-06 三档授权）。
//
// 为什么是 composable 的模块级状态而不是 Pinia store：
// `stores/task.ts` 已在只降不升的 file-size 基线内，本批只在事件分发 / 切任务 /
// 统一重置三处各加一两行；队列主体与决议动作下沉到这里，可脱离 Pinia 单测。
// 形态沿用 `composables/useTaskTrace.ts` 的模块级 ref 共享状态先例。
//
// 数据面两条通道（都必要）：
// 1. SSE `tool_authorization_required` / `tool_authorization_resolved`——瞬态、不落库；
// 2. `GET /api/tasks/{id}/tool-authorizations` 快照——刷新 / 断线重连 / 切换任务后
//    恢复队列。**不能靠重放旧事件**：那会让用户对早已超时的调用点头。
//
// 决议走既有的 `POST /api/agent/tool-permissions/resolve`：任务侧只接受 `once` / `deny`
// （后端对其他 scope 明确 400，不静默降级——任务虚拟会话没有角色/会话授权维度）。
//
// 归属纪律：队列只服务**当前任务**。非当前任务的待授权事件直接丢弃（切到该任务时
// 由快照端点补回）；`resolve` 的响应与事件都做迟到守卫，避免切任务后把 A 的结果
// 写进 B 的界面。

import { ref } from 'vue';
import * as api from '../api';
import type { TaskEvent, TaskToolAuthorization } from '../api/types';

/** 任务侧允许的决议（与后端任务分支的 400 门禁一致） */
export type TaskToolAuthScope = 'once' | 'deny';

/** 面板底部的单行回执：ok=已允许；err=拒绝/失败；info=超时/取消等非用户过错 */
export type TaskToolAuthFeedback = { kind: 'ok' | 'err' | 'info'; text: string };

/** 待确认队列（同一时刻只看一个任务，故模块级单例） */
const queue = ref<TaskToolAuthorization[]>([]);
/** 队列所属任务 id（null = 无）；事件与迟到响应据此判定归属 */
const ownerTaskId = ref<string | null>(null);
/** 正在提交的决议：key = `${run_id}:${call_id}`，值 = 用户点的按钮 */
const busy = ref<Record<string, TaskToolAuthScope>>({});
/** 最近一次决议回执（成功/失败/收束事件共用；切任务时清空） */
const feedback = ref<TaskToolAuthFeedback | null>(null);
/** 快照拉取中（只给「刷新」按钮用，不做逐帧 loading） */
const loading = ref(false);
/** 快照拉取失败原因（null = 无错）；与「没有待确认」是两件不同的事 */
const error = ref<string | null>(null);
/** 快照请求代际：切任务 / 重置后旧响应一律丢弃 */
let snapshotSeq = 0;

/** 队列项唯一键：后端 wait 表也按 (run_id, call_id) 建键 */
export function taskToolAuthKey(item: Pick<TaskToolAuthorization, 'run_id' | 'call_id'>): string {
  return `${item.run_id}:${item.call_id}`;
}

/** 队列状态入口（组件消费） */
export function useTaskToolAuth() {
  return { queue, ownerTaskId, busy, feedback, loading, error };
}

/** 移出队列（按 call_id；`call_id` 在同一 run 内唯一） */
function dropFromQueue(callId: string): void {
  if (!queue.value.some((x) => x.call_id === callId)) return;
  queue.value = queue.value.filter((x) => x.call_id !== callId);
}

/** 收束结果的展示文案：后端给了人类可读 `detail` 就优先用它（超时/断线原因更具体） */
const DECISION_TEXT: Record<string, string> = {
  allow: '已允许本次执行，模型将继续',
  deny: '已拒绝，模型会收到拒绝结果',
  timeout: '等待超时，已自动拒绝',
  cancelled: '任务已停止，该调用未执行',
};

/**
 * 并入一条实时待授权事件（`kind=tool_authorization_required`）。
 *
 * 只接受当前任务；按 `call_id` 去重——同名工具并发多条是正常情况（各条 call_id 不同），
 * 同一条重复到达（重连补偿与实时帧重叠）则用新载荷覆盖旧载荷。
 */
export function applyTaskToolAuthRequired(ev: TaskEvent): void {
  const auth = ev.tool_authorization;
  if (!auth) return; // 旧服务端 / 非本类事件：无载荷
  if (ownerTaskId.value !== ev.task_id) return; // 非当前任务
  const idx = queue.value.findIndex((x) => x.call_id === auth.call_id);
  if (idx >= 0) {
    const next = queue.value.slice();
    next[idx] = auth;
    queue.value = next;
  } else {
    queue.value = [...queue.value, auth];
  }
  // 新一条待确认到达 = 上一轮的结论已经过期（重新执行 / 追加指令续跑都会走到这里）。
  // 留着旧回执会让人在一条新卡片旁边读到上次的「已拒绝」，误导判断。
  feedback.value = null;
  error.value = null;
}

/**
 * 并入一条收束事件（`kind=tool_authorization_resolved`）。
 *
 * 快照是「当前仍待确认」的权威来源，但收束事件能让卡片**立刻**消失，不必等下一次刷新。
 * 即使先收到 HTTP 成功响应（本端已出队），这里也照常给回执——两条路径的结果一致。
 */
export function applyTaskToolAuthResolved(ev: TaskEvent): void {
  const d = ev.tool_authorization_decision;
  if (!d) return;
  if (ownerTaskId.value !== ev.task_id) return;
  dropFromQueue(d.call_id);
  const nextBusy = { ...busy.value };
  delete nextBusy[`${d.run_id}:${d.call_id}`];
  busy.value = nextBusy;
  const detail = d.detail?.trim();
  feedback.value = {
    kind: d.decision === 'allow' ? 'ok' : d.decision === 'deny' ? 'err' : 'info',
    text: `「${d.name}」${detail || DECISION_TEXT[d.decision] || d.decision}`,
  };
}

/**
 * 拉取当前任务的待确认快照（切任务 / 刷新 / 断线重连 / 手动刷新）。
 *
 * 失败**不清空已有队列**，只置 `error`——把一次网络故障显示成「没有待确认」会让用户
 * 以为工具已放行。任务不存在（404）也走同一分支：卡片保留 + 显式报错，由用户处理。
 */
export async function loadTaskToolAuthSnapshot(taskId: string): Promise<void> {
  ownerTaskId.value = taskId;
  loading.value = true;
  error.value = null;
  const seq = ++snapshotSeq;
  try {
    const pending = await api.getTaskToolAuthorizations(taskId);
    // 迟到守卫：响应到达时已切任务 / 已重置则整轮丢弃
    if (seq !== snapshotSeq || ownerTaskId.value !== taskId) return;
    // 按 call_id 去重（后端已排序；重复只可能来自脏数据），保持服务端顺序
    const seen = new Set<string>();
    queue.value = pending.filter((x) => {
      if (seen.has(x.call_id)) return false;
      seen.add(x.call_id);
      return true;
    });
    // **刻意不动 `feedback`**：它记录的是最近一次决议的结论（已允许 / 已拒绝 / 已超时）。
    // 断线重连的 settle、手动点「刷新」、切任务回来都会走到这里——顺手清掉等于让
    // 用户在任务跑完后再也看不到「那次调用到底通过了没有」（实测复现：拒绝后卡片消失、
    // 回执同时消失，界面只剩一个 partial 终态）。清回执的时机只有两个：
    // 真正换了任务（clearTaskToolAuth）与发起新一次决议。
  } catch (e) {
    if (seq !== snapshotSeq || ownerTaskId.value !== taskId) return;
    error.value = `读取待确认调用失败：${e instanceof Error ? e.message : String(e)}`;
  } finally {
    if (seq === snapshotSeq) loading.value = false;
  }
}

/**
 * 提交一次决议（仅本次允许 / 拒绝）。
 *
 * 成功后立即出队并给回执：后端会另外广播 `tool_authorization_resolved`，两条路径都
 * 收敛到「卡片消失 + 一行结果」，先后到达不会互相破坏。
 * 失败**保留卡片**并把后端原文（如 409「待授权调用不存在或已结束」）显示出来。
 */
export async function decideTaskToolAuth(
  item: TaskToolAuthorization,
  decision: TaskToolAuthScope,
): Promise<void> {
  const key = taskToolAuthKey(item);
  if (busy.value[key]) return; // 同一调用只允许一次在飞
  const taskId = ownerTaskId.value;
  busy.value = { ...busy.value, [key]: decision };
  feedback.value = null;
  try {
    await api.resolveToolAuthorization(
      item.name,
      decision,
      item.session_id,
      item.run_id,
      item.call_id,
    );
    if (ownerTaskId.value !== taskId) return; // 迟到守卫：已切任务
    dropFromQueue(item.call_id);
    feedback.value = {
      kind: 'ok',
      text:
        decision === 'once'
          ? `「${item.name}」已允许本次执行，模型将继续`
          : `「${item.name}」已拒绝，模型会收到拒绝结果`,
    };
  } catch (e) {
    if (ownerTaskId.value !== taskId) return;
    feedback.value = {
      kind: 'err',
      text: `决议失败：${e instanceof Error ? e.message : String(e)}`,
    };
  } finally {
    if (busy.value[key]) {
      const next = { ...busy.value };
      delete next[key];
      busy.value = next;
    }
  }
}

/**
 * 清空待确认**队列**但保留回执（任务进终态时用）。
 *
 * 为什么要和 `clearTaskToolAuth` 分开：终态时后端已取消全部等待，残留卡片点下去
 * 只会 409，故队列必须清；但「已拒绝 / 已超时」这条回执恰恰是终态时用户最想看到的
 * 结论——一起清掉等于把结果也吞了。
 */
export function clearPendingTaskToolAuth(): void {
  snapshotSeq += 1;
  queue.value = [];
  busy.value = {};
  error.value = null;
  loading.value = false;
}

/**
 * 清空队列（切任务 / 删除任务 / 清选择）。
 *
 * 回执也一并清：它描述的是**上一个任务**的调用结果，留在新任务上是错位信息。
 * `snapshotSeq` 自增使在飞快照失效——否则切到新任务后，旧任务的迟到响应会把
 * 别人的待确认卡片栽到新任务上。
 */
export function clearTaskToolAuth(): void {
  clearPendingTaskToolAuth();
  ownerTaskId.value = null;
  feedback.value = null;
}

/** 测试用：同清空，语义单列以免用例误以为只是「清队列」 */
export function resetTaskToolAuthForTest(): void {
  clearTaskToolAuth();
}
