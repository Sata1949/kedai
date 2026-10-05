// 任务模式 API:任务 CRUD + 执行/停止 + 流程改绑 + 任务事件 SSE 订阅(WP5)。
// 后端契约:GET /api/tasks → { tasks };POST /api/tasks → { ok, task };
// GET /api/tasks/{id} → { task, subtasks };POST /api/tasks/{id}/run | /stop | /bind;
// DELETE /api/tasks/{id} → 204;GET /api/tasks/events → SSE(KeepAlive 30s)。
import { BASE, authorizedFetch, request, requestText } from './client';
import { requireArrayField, requireBoolField, requireObject, requireObjectField, requireStringField } from './shape';
import { pumpSseFrames, toApiError } from './stream';
import type { TaskApproveExecMode, TaskChangeDiff, TaskChangeRollback, TaskChangeRollbackAll, TaskChangeRollbackItem, TaskChangesPayload, TaskDetail, TaskEvent, TaskEventsPull, TaskFileChange, TaskLlmCall, TaskRecord, TaskRunMode, TaskStep, TaskUsageTotal } from './types';

/** 读取任务列表(最新在前) */
export async function listTasks(): Promise<TaskRecord[]> {
  const data = await request<unknown>('/tasks');
  // 形状闸门:任务板直接落列表渲染
  return requireArrayField<TaskRecord>(data, 'tasks', '任务列表');
}

/**
 * 新建任务。
 * - `executorId`:执行者库 id(可选,缺省 = 通用执行者);
 * - `characterId`:**兼容入参**,旧形态(角色卡执行者),新代码不应使用——
 *   执行者已与角色扮演角色卡解耦,该字段仅为旧调用方保留(两者同时给出时执行者优先);
 * - `taskMode`:批次 4 六模式,缺省 legacy;
 * - `flowId`:二维批次 5a 的**流程绑定**(可选;仅 custom 模式可给,缺省 = 跟随当前流程)。
 *   仅在显式给出时下发,旧调用方请求体不变;
 * - `flowIds`:二维批次 7b 的**对比模式名单**(可选;仅 custom 模式可给,根流程 + 名单内流程
 *   作为 `run_flow` 工具释放给宽松节点)。同样仅在非空时下发——空数组会被后端判 400
 *   (「空名单 = 名存实亡」),故调用方不必也不应传空数组;
 * - `connectionId`:B 批 B1 的**逐任务选用连接**(可选;**所有任务模式**都适用——它绑的是
 *   provider 而不是编排)。语义 = 该任务所有 LLM 调用的缺省连接(节点级 `connection_id`
 *   优先);仅非空时下发,缺省即跟随设置的默认连接(零行为变化)。
 *   创建期后端即校验「引用存在且启用」,不存在/停用 → 400 点名该连接。
 * - `workspace`:CODE-1 的**任务工作区**(可选;项目目录的绝对路径)。非空时随请求下发,
 *   后端创建期 canonical 化并冻结(**不存在 / 指向数据目录 → 400 点名原因**);空/缺省
 *   = 未绑定(任务在草稿目录工作)。仅在显式给出时下发,旧调用方请求体逐字节不变。
 */
export async function createTask(
  title: string,
  executorId?: string,
  taskMode?: TaskRunMode,
  characterId?: string,
  flowId?: string,
  flowIds?: string[],
  connectionId?: string,
  workspace?: string,
): Promise<TaskRecord> {
  const data = await request<unknown>('/tasks', {
    method: 'POST',
    body: JSON.stringify({
      title,
      executor_id: executorId ?? null,
      task_mode: taskMode ?? 'legacy',
      // 兼容入参:仅在显式给出时下发,避免污染新请求
      ...(characterId ? { character_id: characterId } : {}),
      // 流程绑定:空串 = 跟随当前流程(不下发该键)
      ...(flowId ? { flow_id: flowId } : {}),
      // 对比模式名单:仅非空时下发(空数组 = 后端 400,不是「强制模式」的写法)
      ...(flowIds && flowIds.length > 0 ? { flow_ids: flowIds } : {}),
      // 逐任务选用连接:仅非空时下发(空 = 跟随设置的默认连接,与 B 批之前逐字节一致)
      ...(connectionId ? { connection_id: connectionId } : {}),
      // 工作区(CODE-1):仅非空下发(空/纯空白 = 未绑定,不下发该键);值去两端空白
      ...(workspace && workspace.trim() ? { workspace: workspace.trim() } : {}),
    }),
  });
  return requireObjectField<TaskRecord>(data, 'task', '任务');
}

/**
 * 改绑自定义流程(B 批 B3;仅 custom 模式任务可用)。
 *
 * **全量替换**语义——与 createTask 的「仅非空下发」口径**相反**,两个键都必须显式下发:
 *  - `flowId: null` = 跟随当前流程(解绑);
 *  - `flowIds: []` = 强制模式(清空名单)。
 * 省略任一键,后端都无法区分「不改」与「清空」,故这里不做任何省略处理。
 *
 * 语义 = **重新冻结快照**(`flow_snapshot` 换成新流程的闭包);历史 plan 行不动,
 * 其编排徽标按新快照解析(对不上就不显示,沿用 IFW-5 口径)。
 * 非 custom 模式 / `planning|running|planned` 态 / 名单成员不存在或未启用 → 400
 * (中文文案点名原因),且库不变。
 */
export async function bindTask(
  id: string,
  flowId: string | null,
  flowIds: string[],
): Promise<TaskRecord> {
  const data = await request<unknown>(`/tasks/${encodeURIComponent(id)}/bind`, {
    method: 'POST',
    body: JSON.stringify({ flow_id: flowId, flow_ids: flowIds }),
  });
  return requireObjectField<TaskRecord>(data, 'task', '任务');
}

/** 读取任务详情(含子任务) */
export async function getTask(id: string): Promise<TaskDetail> {
  const data = await request<unknown>(`/tasks/${id}`);
  // 形状闸门:task store 的 contentSignature(detail) 直接读 task/subtasks,
  // 解出 undefined 会在下游以随机 TypeError 崩溃(报错点远离真正原因)
  requireObjectField<TaskRecord>(data, 'task', '任务详情');
  requireArrayField<TaskDetail['subtasks'][number]>(data, 'subtasks', '任务详情');
  return data as TaskDetail;
}

/** 启动任务执行(后台) */
export async function runTask(id: string): Promise<void> {
  await request<{ ok: boolean }>(`/tasks/${id}/run`, { method: 'POST' });
}

/** 停止任务执行 */
export async function stopTask(id: string): Promise<void> {
  await request<{ ok: boolean }>(`/tasks/${id}/stop`, { method: 'POST' });
}

/**
 * 批准计划(plan 模式,批次 4):仅 status='planned' 时合法,否则后端 400;
 * 传入 plan 则替换计划。
 *
 * `execMode`(2026-09-17)为**本次批准续跑的执行方式**,缺省 `approved_plan`
 * (按计划逐步执行,即改造前行为);可选 solo/multi/team/custom。
 * 它不改任务的 `task_mode`(仍为 plan,记录任务当初怎么产出计划)。
 */
export async function approveTask(
  id: string,
  plan?: TaskStep[],
  execMode?: TaskApproveExecMode,
): Promise<{ ok: boolean }> {
  const body: Record<string, unknown> = {};
  if (plan) body.plan = plan;
  if (execMode) body.exec_mode = execMode;
  return request<{ ok: boolean }>(`/tasks/${id}/approve`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

/**
 * 终态追加指令(批次 R2a;R2b+ 扩 mode):仅终态(done/partial/error/ended)可追加,
 * 空指令 400(VALIDATION)/ 非法 mode 400 / 非终态 409(CONFLICT);任务回 running
 * 以「原目标 + 上轮结果 + 追加指令」solo 续跑,产出落 messages。
 * mode=append(缺省)追加进 result;mode=replace 整体替换 result(段标「修订 N」)。
 */
export async function followupTask(
  id: string,
  content: string,
  mode: 'append' | 'replace' = 'append',
): Promise<{ ok: boolean }> {
  return request<{ ok: boolean }>(`/tasks/${id}/followup`, {
    method: 'POST',
    body: JSON.stringify({ content, mode }),
  });
}

/**
 * 批准环节规划对话(批次 R2b):仅 planned 态合法(空反馈 400 / 非 planned 409);
 * 同步等待规划器按反馈修订计划(数秒~数十秒),响应携带修订后计划;
 * 任务保持 planned,需重新批准。
 */
export async function planChatTask(id: string, message: string): Promise<{ ok: boolean; plan: TaskStep[] }> {
  return request<{ ok: boolean; plan: TaskStep[] }>(`/tasks/${id}/plan-chat`, {
    method: 'POST',
    body: JSON.stringify({ message }),
  });
}

/** 删除任务(含子任务) */
export async function deleteTask(id: string): Promise<void> {
  await request<void>(`/tasks/${id}`, { method: 'DELETE' });
}

/** 全部任务 token 累计(侧栏任务模式「全局累计」) */
export async function getTaskUsageTotal(): Promise<TaskUsageTotal> {
  const data = await request<unknown>('/tasks/usage-total');
  // 形状闸门:侧栏直接读 usage_total.total_tokens 显示累计
  return requireObjectField<TaskUsageTotal>(data, 'usage_total', '任务用量累计');
}

/** 任务 LLM 调用记录(批次 3 L3 调用追踪面板;按 created_at,id 升序) */
export async function getTaskCalls(taskId: string): Promise<TaskLlmCall[]> {
  const data = await request<unknown>(`/tasks/${encodeURIComponent(taskId)}/calls`);
  return requireArrayField<TaskLlmCall>(data, 'calls', '任务调用记录');
}

/**
 * 任务事件补拉(PRODCAP-1;`GET /api/tasks/{id}/events?after=&limit=`)。
 * 无流场景的权威来源:前端检测到 `seq` 缺口 / 重连补偿时调用。
 * `events` 元素与 SSE 帧**同形**(含 `seq`/`at`),可直接走同一分发;
 * `truncated=true` 表示起点早于保留窗口(每任务 2000 条),更早事件已被清理——
 * 明确告知而不是假装「没有更多」,调用方记一行日志后接受并前进。
 */
export async function getTaskEvents(
  taskId: string,
  after: number,
  limit?: number,
): Promise<TaskEventsPull> {
  const qs = new URLSearchParams({ after: String(after) });
  if (limit !== undefined) qs.set('limit', String(limit));
  const data = await request<unknown>(
    `/tasks/${encodeURIComponent(taskId)}/events?${qs.toString()}`,
  );
  const payload = requireObject<TaskEventsPull>(data, '任务事件补拉');
  requireArrayField<TaskEvent>(data, 'events', '任务事件补拉');
  requireBoolField(data, 'truncated', '任务事件补拉');
  return payload;
}

/**
 * 任务文件变更清单(批次 4 / 4b,PRODCAP-4)。
 * 响应除 `changes` 数组外还有**顶层** `undected`/`undected_reason`(扫描是否完整),
 * 故这里返回整个载荷而不是数组——调用方一次性拿全语义
 * (「本轮未改动文件」「扫描缺项」「没有记账」是**三件不同的事**)。
 */
export async function getTaskChanges(taskId: string): Promise<TaskChangesPayload> {
  const data = await request<unknown>(`/tasks/${encodeURIComponent(taskId)}/changes`);
  const changes = requireArrayField<TaskFileChange>(data, 'changes', '任务文件变更');
  const obj = data as Record<string, unknown>;
  const reason = typeof obj.undected_reason === 'string' ? obj.undected_reason : null;
  // `undected` 以服务端布尔为准,原因在场的兜底只在缺布尔时生效(旧服务端容错)
  return { changes, undected: obj.undected === true || reason !== null, undectedReason: reason };
}

/** 单文件 unified diff(三种形态都 200,靠 `available` 区分;`path` 必须整体编码) */
export async function getTaskChangeDiff(
  taskId: string,
  path: string,
): Promise<TaskChangeDiff> {
  const data = await request<unknown>(
    `/tasks/${encodeURIComponent(taskId)}/changes/diff?path=${encodeURIComponent(path)}`,
  );
  // 形状闸门:组件按 available 分支渲染,解出 undefined 会把两种形态混成一种
  const payload = requireObject<TaskChangeDiff>(data, '任务文件变更 diff');
  requireStringField(data, 'path', '任务文件变更 diff');
  return payload;
}

/** 单文件回滚(任务进行中后端 409;`ok:false` 时原因在 `reason`,原文显示给用户) */
export async function rollbackTaskChange(
  taskId: string,
  path: string,
): Promise<TaskChangeRollback> {
  const data = await request<unknown>(
    `/tasks/${encodeURIComponent(taskId)}/changes/rollback?path=${encodeURIComponent(path)}`,
    { method: 'POST' },
  );
  // 形状闸门:调用方按 ok 判成败,undefined 会被当成功吞掉
  const payload = requireObject<TaskChangeRollback>(data, '任务文件变更回滚');
  requireBoolField(data, 'ok', '任务文件变更回滚');
  return payload;
}

/** 整任务回滚(CODE-2;任务进行中后端 409;逐项报告,不整体否决) */
export async function rollbackAllTaskChanges(taskId: string): Promise<TaskChangeRollbackAll> {
  const data = await request<unknown>(
    `/tasks/${encodeURIComponent(taskId)}/changes/rollback-all`,
    { method: 'POST' },
  );
  // 形状闸门:调用方按 ok 判成败并渲染逐项报告,undefined 会被当成功吞掉
  const payload = requireObject<TaskChangeRollbackAll>(data, '任务文件变更整任务回滚');
  requireBoolField(data, 'ok', '任务文件变更整任务回滚');
  requireArrayField<TaskChangeRollbackItem>(data, 'results', '任务文件变更整任务回滚');
  return payload;
}

/** 整任务 patch 导出(CODE-2;`text/plain` 文本通道,不套 JSON 形状闸门) */
export async function getTaskChangesPatch(taskId: string): Promise<string> {
  return requestText(`/tasks/${encodeURIComponent(taskId)}/changes/patch`);
}

/**
 * 订阅任务事件流(GET /api/tasks/events,WP4 后端 SSE,WP5 取代前端 1s 轮询)。
 * 帧解析复用 sseParser.ts 共享层(兼容 CRLF/LF、跨 chunk 分隔、多 data 行拼接),
 * 本层只保留任务流专属语义——长连接,无聊天终态,只有「对端断开 / 网络错误 / 主动关闭」三种结局:
 * - onEvent:每收到一帧 data: {…task 事件…} 触发(坏帧与非 task 类型帧跳过,不阻断后续);
 * - onClose:非 2xx 响应(err 为 ApiError)/ 网络错误 / 流自然结束(err 为空)时触发,由调用方退避重连;
 * - 返回关闭函数:主动关闭走 AbortController,不触发 onClose。
 */
export function streamTaskEvents(
  onEvent: (ev: TaskEvent) => void,
  onClose: (err?: Error) => void,
): () => void {
  const controller = new AbortController();
  /** 主动关闭标记:abort 触发的异常与自然结束都不再回调 onClose */
  let closed = false;
  const close = (): void => {
    if (closed) return;
    closed = true;
    controller.abort();
  };

  void (async () => {
    try {
      const res = await authorizedFetch(`${BASE}/tasks/events`, {
        method: 'GET',
        headers: { Accept: 'text/event-stream' },
        signal: controller.signal,
      });
      if (!res.ok || !res.body) {
        // 错误口径与 client.ts request() 一致:结构化 code 分类 + ApiError(共享 stream.ts)
        throw await toApiError(res);
      }

      // 帧解析与读循环走共享底座;非 task 类型/坏帧跳过,不阻断后续事件
      await pumpSseFrames(res, (data) => {
        try {
          const ev = JSON.parse(data) as TaskEvent;
          if (ev && ev.type === 'task' && typeof ev.task_id === 'string') onEvent(ev);
        } catch {
          // 单个坏事件不阻断后续事件
        }
      });
      // 流自然结束(对端关闭):非主动关闭,通知调用方重连
      if (!closed) {
        closed = true;
        onClose();
      }
    } catch (e) {
      // 主动关闭的 AbortError 不回调;其余(非 2xx ApiError / 网络错误)通知调用方重连
      if (closed || (e as Error).name === 'AbortError') return;
      closed = true;
      onClose(e as Error);
    }
  })();

  return close;
}
