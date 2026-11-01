// 任务执行轨迹的前端状态(TMSHOW-2,2026-11-01)。
//
// 为什么是 composable 级的模块状态而不是 Pinia store:
// - `stores/task.ts` 已在只降不升的文件大小基线内(TMSHOW-1 又给它加了 5 行),
//   本批只在三处各加一行调用(事件分发 / 切任务 / 统一重置),逻辑主体下沉到这里;
// - 轨迹是**纯展示数据**,不参与任务状态机,不需要跨 store 的动作编排;
// - 独立模块可脱离 Pinia 单测(分页拼接、seq 去重、缺口补拉全在 node 环境下可钉住)。
// 形态沿用 `composables/useUserPersona.ts` 的模块级 `ref` 共享状态先例。
//
// 两条数据通道(**都必要**):
// 1. SSE `kind=trace` 增量帧 → `appendTrace`,逐条并入,不必重拉端点;
// 2. `GET /api/tasks/{id}/trace` 补拉 → `loadTrace`,负责首屏、断线恢复与缺口补齐。
// 两路会重叠(重连补偿/缺口补拉都会重新拿到已有区间),故 `appendTrace` 与 `loadTrace`
// 都按轨迹自己的 `seq` 去重——注意**不是**事件 `seq`,两者不同源。

import { ref } from 'vue';
import * as api from '../api';
import { flattenTrace, type TranscriptMode } from '../utils/taskTranscript';

/** 单次补拉请求的条数(与后端 `limit` 缺省 500 / 夹取 1..=1000 同口径) */
const TRACE_PAGE_LIMIT = 500;

/**
 * 补拉上限:后端每任务保留 2000 条,一次加载最多翻这么多条就停。
 *
 * 为什么要设**前端**上限:补拉是 while 循环(一直翻到不足一页),若后端窗口策略变化
 * 或 truncated 判定出现异常,循环会无限打请求。2000 与后端窗口同值,正常情况下永远
 * 翻不到这个数。
 */
const TRACE_MAX_ROWS = 2000;

/** 轨迹状态(模块级单例:任务工作台同一时刻只看一个任务) */
const rows = ref<api.TaskTrace[]>([]);
/** 轨迹所属任务 id(null = 无当前轨迹);切换任务时据此丢弃迟到响应 */
const ownerTaskId = ref<string | null>(null);
/** 后端报告「请求起点早于保留窗口」(更早条目已被清理) */
const truncated = ref(false);
/** 首屏/补拉是否在飞(只给首屏空态用,不做逐帧 loading) */
const loading = ref(false);
/** 最近一次加载失败的原因(null = 无错);与「没有轨迹」是**两件不同的事** */
const error = ref<string | null>(null);
/** 档位(临时;TMSHOW-3 接 uiPrefs 持久化与单任务覆盖) */
const mode = ref<TranscriptMode>('full');

/** 当前已载入的最大轨迹 seq(缺口判定与增量补拉的起点) */
const lastSeqByTask = new Map<string, number>();
/** 有补拉在飞的 task_id 集合(缺口密集时不叠加并发请求) */
const pullRunning = new Set<string>();

/** 轨迹状态与模式入口(组件消费) */
export function useTaskTrace() {
  return { rows, ownerTaskId, truncated, loading, error, mode };
}

/** 设置档位(临时切换;不持久化——TMSHOW-3 才接 localStorage 与设置页) */
export function setTranscriptMode(next: TranscriptMode): void {
  mode.value = next;
}

/**
 * 清空轨迹状态(切任务 / 删除任务 / 清选择时调用)。
 *
 * `ownerTaskId` 一并清空:迟到响应靠它比对丢弃,若留着旧 id,切到新任务后
 * 旧请求的响应会被误认为「还是我的」而写进新任务的视图。
 */
export function clearTaskTrace(): void {
  rows.value = [];
  ownerTaskId.value = null;
  truncated.value = false;
  loading.value = false;
  error.value = null;
}

/**
 * 全量加载某任务的轨迹(首屏 / 重试 / 切换任务)。
 *
 * 分页循环:每轮取 `after = 已载入最大 seq`,直到后端回空页(说明到底)或达到
 * `TRACE_MAX_ROWS` 上限。`truncated` 由后端给出并如实透出——「更早的条目已被清理」
 * 必须显示给用户,不能假装那就是全部。
 *
 * 失败**不清空已有内容**(保留上次成功的结果),只置 `error`——把一次网络故障显示成
 * 「这个任务没有执行过程」是最坏的一种谎报。
 */
export async function loadTaskTrace(taskId: string): Promise<void> {
  if (ownerTaskId.value === taskId && loading.value) return; // 同任务已在飞:不叠加并发
  ownerTaskId.value = taskId;
  loading.value = true;
  error.value = null;
  const collected: api.TaskTrace[] = [];
  let after = 0;
  let sawTruncated = false;
  try {
    for (;;) {
      const page = await api.getTaskTrace(taskId, after, TRACE_PAGE_LIMIT);
      // 迟到守卫:响应到达时已切到别的任务(或已清选择)则整轮丢弃
      if (ownerTaskId.value !== taskId) return;
      if (page.truncated) sawTruncated = true;
      if (page.trace.length === 0) break;
      collected.push(...page.trace);
      const nextAfter = page.trace[page.trace.length - 1].seq;
      // 后端每页按 seq 升序;若末条 seq 没有推进,说明分页游标原地打转(防御死循环)
      if (nextAfter <= after) break;
      after = nextAfter;
      if (collected.length >= TRACE_MAX_ROWS) break;
      // 不足一页 = 已到底,不再多打一次空请求
      if (page.trace.length < TRACE_PAGE_LIMIT) break;
    }
    const merged = flattenTrace(collected);
    rows.value = merged;
    truncated.value = sawTruncated;
    const last = merged.length ? merged[merged.length - 1].seq : 0;
    lastSeqByTask.set(taskId, last);
  } catch (e) {
    // 迟到守卫同样适用于失败分支:切走后不要在新任务上写错误
    if (ownerTaskId.value !== taskId) return;
    console.warn('[kedai] 任务执行轨迹加载失败', e);
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    if (ownerTaskId.value === taskId) loading.value = false;
  }
}

/**
 * 缺口补拉:从 `after` 起把缺失区间拉回来并并入当前列表。
 *
 * 与 `loadTaskTrace` 的差别:**只拉增量、不清已载入内容**。缺口出现在
 * `trace.seq > last + 1` 时(SSE 丢帧 / 事件通道 Lagged)。
 *
 * `pullRunning` 防抖:密集缺口时合并成一次在飞请求(与 `stores/task.ts` 的
 * `pullTaskEvents` 同款策略)。失败静默——下一次缺口或重连补偿会再触发。
 */
async function pullTraceGap(taskId: string, after: number): Promise<void> {
  if (pullRunning.has(taskId)) return;
  pullRunning.add(taskId);
  try {
    const page = await api.getTaskTrace(taskId, after, TRACE_PAGE_LIMIT);
    if (ownerTaskId.value !== taskId) return;
    if (page.truncated) truncated.value = true;
    if (page.trace.length === 0) return;
    const merged = flattenTrace([...rows.value, ...page.trace]);
    rows.value = merged;
    const last = merged[merged.length - 1].seq;
    lastSeqByTask.set(taskId, last);
  } catch (e) {
    console.warn('[kedai] 任务轨迹缺口补拉失败(等待下一次缺口或重连补偿)', e);
  } finally {
    pullRunning.delete(taskId);
  }
}

/**
 * 并入一条实时轨迹(`kind=trace` 事件 → `TaskEvent.trace`)。
 *
 * 只接受**当前任务**的轨迹;非当前任务直接丢弃(与 `lastAgentStatus` 同口径——
 * 事件不落前端缓存,切任务后从端点重建)。
 *
 * 缺口检测:`trace.seq > last + 1` 时先不并这条(交给补拉按序补齐),
 * 但仍会把它放进列表——否则用户会看到一条永久的空洞。补拉返回后 `flattenTrace`
 * 按 seq 排序去重,顺序自愈。
 */
export function appendTrace(taskId: string, trace: api.TaskTrace | undefined): void {
  if (!trace) return; // 旧服务端 / 非 trace 事件:无载荷
  if (ownerTaskId.value !== taskId) return; // 非当前任务
  const last = lastSeqByTask.get(taskId);
  if (last !== undefined && trace.seq > last + 1) {
    console.warn(
      `[kedai] 任务轨迹缺口(任务 ${taskId}:已见 seq=${last},收到 seq=${trace.seq}),开始补拉`,
    );
    void pullTraceGap(taskId, last);
  }
  if (last === undefined || trace.seq > last) lastSeqByTask.set(taskId, trace.seq);
  // 逐条并入也要去重:补拉与实时帧重叠时同一条会到两次
  if (rows.value.some((r) => r.seq === trace.seq)) return;
  rows.value = flattenTrace([...rows.value, trace]);
}

/**
 * 重连补偿:SSE 断线恢复后把断开期间的轨迹补回来。
 *
 * 与 `loadTaskTrace` 的差别:不重头拉全量,从已载入的最大 seq 起补——长任务的轨迹
 * 可能有几千条,重连一次就重传一遍全量并不划算。
 */
export function resumeTaskTrace(taskId: string): void {
  if (ownerTaskId.value !== taskId) return;
  const last = lastSeqByTask.get(taskId);
  if (last === undefined) {
    // 从未加载成功过(首屏就断线):退回全量加载
    void loadTaskTrace(taskId);
    return;
  }
  void pullTraceGap(taskId, last);
}

/** 测试用:把内部记账清干净(避免用例间互相污染) */
export function resetTaskTraceForTest(): void {
  clearTaskTrace();
  lastSeqByTask.clear();
  pullRunning.clear();
  mode.value = 'full';
}
