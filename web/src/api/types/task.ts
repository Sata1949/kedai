// 任务模式域类型(与后端 models/types.rs、task_core 线格式一一对应)。

import type { AgentFlowConfig } from './agentFlow';
import type { TaskDeliverySummary, WorkspaceProfile } from './workspace';

/** 任务事件分类(WP4 后端推送):创建 / 状态变化 / 计划 / 子任务 / token 累计 / 删除 / LLM 调用落库
 *  批次 4 六模式追加:agent_status(主/子 agent 状态迁移)/ approval_required(plan 模式计划待批准)
 *  批次 R4 流式输出追加:delta(LLM 正文攒批增量,暂态不落库;权威数据以 llm_call 落库行为准)
 *  TMR3-L2 追加:delta_reset(重试前清空该调用归属的暂态流式缓冲,同样不落库)
 *  PRODCAP-1 追加:file_changed(文件变更台账新增一行,「文件变更」卡片据此实时刷新)
 *  真源为 Rust `models/types.rs` 的 `TaskEventKind`(serde snake_case);改一侧须同步另一侧,
 *  并更新 `tools/check-contract.mjs` 的映射表。 */
export type TaskEventKind = 'created' | 'status' | 'plan' | 'subtask' | 'usage' | 'deleted' | 'llm_call' | 'agent_status' | 'approval_required' | 'delta' | 'delta_reset' | 'flow_bound' | 'file_changed';

/**
 * 任务模式(task 工作台)事件(对齐 server-rs SseEvent::Task):
 * WP5 起由 GET /api/tasks/events 真实 SSE 推送,取代前端 1s REST 轮询与本地合成伪事件。
 * 除 task_id 外全部可选;kind 缺失(旧服务端/未知分类)时仅透传事件监控面板,不触发刷新。
 */
export type TaskEvent = {
  type: 'task';
  task_id: string;
  kind?: TaskEventKind;
  title?: string;
  /** 任务状态(snake_case,见 TaskStatus) */
  status?: string;
  /** 简短中文说明(事件监控面板展示用);kind=delta 时为攒批后的正文增量文本,
   *  kind=delta_reset 时不携带(仅表示重置该调用归属的缓冲) */
  detail?: string;
  /** 上游 finish_reason(仅 kind=llm_call 且成功调用携带,可观测性问题①截断标记;
   *  'length' = max_tokens 截断;缺省 = 不适用/未知,旧客户端直接忽略) */
  finish_reason?: string;
  /** 调用归属阶段(批次 R4;仅 kind=delta/delta_reset/llm_call 携带,与 task_llm_calls.phase 同口径;
   *  流式缓冲 key 前半:`${phase}:${step_index ?? ''}`) */
  phase?: string;
  /** 调用归属步骤下标(0 起;仅步骤类调用携带;流式缓冲 key 后半) */
  step_index?: number;
  /** 任务内单调事件序号(PRODCAP-1;来源 `task_events.seq`)。前端据
   *  `seq > lastSeq + 1` 判定丢帧并补拉;kind=delta/delta_reset 为暂态事件不落库,**不携带**本字段。
   *  缺省 = 未知/不适用(旧服务端,直接忽略) */
  seq?: number;
  /** 事件落库时间(ISO-8601,与 `task_events.created_at` 同源;仅落库事件携带) */
  at?: string;
};

/**
 * 任务事件补拉响应(PRODCAP-1;`GET /api/tasks/{id}/events?after=&limit=`)。
 * `events` 元素与 SSE 帧**同形**(可直接走同一分发,不搞第二套形状),按 `seq` 升序;
 * `truncated=true` 表示请求起点早于保留窗口(每任务 2000 条)——更早事件已被清理,
 * 前端接受并前进,不把它当失败。
 */
export type TaskEventsPull = {
  events: TaskEvent[];
  truncated: boolean;
};

/**
 * 任务计划步骤状态(与 server-rs 实际写入值对齐):
 * pending=待执行(parse 时 serde 默认值) / running=执行中 / done=完成
 * / partial=被守卫截断(带产出但没跑完,SKY-6 第 2 项) / error=失败
 * 写入点:executor.rs 步骤循环(running→done|partial|error)、mod.rs/parse.rs(初始 pending)
 */
export type TaskStepStatus = 'pending' | 'running' | 'done' | 'partial' | 'error';

/** 任务计划步骤 */
export interface TaskStep {
  name: string;
  goal: string;
  status: TaskStepStatus;
  result: string;
  /**
   * 该行对应的流程节点 id(custom 模式填 `PlanStep.id`;其余模式下发时不带此键)。
   *
   * plan 行由**过滤后的启用步骤**构造,故 plan 下标 ≠ 流程数组下标——界面要问
   * 「这一行是哪个节点」只能靠它(遗留.md IFW-5)。
   */
  node_id?: string | null;
  /**
   * 本步骤声明负责的工作区相对文件（TMR-4；team 规划器写入，其余模式不下发此键）。
   *
   * 运行期按它做文件级所有权拦截；TMR-6 的交付摘要「必需文件」也来自这里。
   * 后端空数组时整键省略，故消费侧按可选容错。
   */
  files?: string[];
}

/** 任务状态:待执行/规划中/执行中/计划待批准(plan 模式)/完成/部分完成(含失败步骤但成果已产出)/出错/已停止 */
export type TaskStatus = 'pending' | 'planning' | 'running' | 'planned' | 'done' | 'partial' | 'error' | 'ended';

/** 任务执行模式(批次 4 六模式,对齐 server-rs TaskRunMode):
 *  legacy 三段式(默认) / solo 单主工具循环 / multi 多agent / plan 先规划后批准 / team 多主+审计 / custom 自定义流程 */
export type TaskRunMode = 'legacy' | 'solo' | 'multi' | 'plan' | 'team' | 'custom';

/**
 * plan 模式**批准时**选择的执行方式(2026-09-17,对齐 server-rs TaskApproveExecMode)。
 * 只作用于本次批准续跑,不改任务自身的 task_mode。
 *
 * 不含 legacy(自带规划会与已批准计划重复劳动)与 plan(会再规划回到 planned 死循环)。
 */
export type TaskApproveExecMode = 'approved_plan' | 'solo' | 'multi' | 'team' | 'custom';

/** 任务记录 */
export interface TaskRecord {
  id: string;
  title: string;
  status: TaskStatus;
  plan: TaskStep[];
  result: string;
  error: string;
  /**
   * 旧角色卡执行者关联 id。**兼容字段**:执行身份已改由 executor_id 承担(独立执行者库),
   * 新前端不再发送;服务端仍接受并原样落库,供旧任务语义使用(世界书按角色过滤 /
   * 占位符渲染 / character_prompt 工具读取)。「执行者人设档位」注入已随 TM-SET-2 退役。
   */
  character_id?: string | null;
  created_at: string;
  updated_at: string;
  /** 执行模式(批次 4;旧服务端不带此字段,消费侧按 legacy 处理) */
  task_mode?: TaskRunMode;
  /** 执行者库 id(空 = 通用执行者);指向后端 data/task_executors.json */
  executor_id?: string | null;
  /**
   * 任务**绑定的自定义流程 id**(二维批次 5a;空/缺省 = 跟随当前流程)。
   *
   * 绑定即冻结:后端在创建时按该流程落一份快照(`TaskDetail.flow_snapshot`),
   * 此后改那份流程、或把「当前流程」换成别的,都不影响这个任务。
   */
  flow_id?: string | null;
  /**
   * 任务**对比模式的可调用流程名单**(二维批次 7b;空/缺省 = 强制模式)。
   *
   * 非空即「根流程照常执行 + 名单内流程作为 `run_flow` 工具释放给宽松节点」:
   * 模型在工具循环里自主调用某套流程、取回其成果。与 `flow_id` 相互独立——
   * `flow_id` 仍是根流程(缺省 = 跟随当前流程),名单只决定哪些流程可被调用
   * (根流程自身不可被调用,创建时后端即 400)。
   */
  flow_ids?: string[] | null;
  /**
   * 任务**逐任务选用连接**(B 批 B1;空/缺省 = 跟随设置的默认连接,与 B 批之前一致)。
   *
   * 语义:该任务**全部** LLM 调用的缺省连接(节点级 `connection_id` 优先)。
   * 应用于全部任务模式(规划 / 步骤 / 汇总 / 工具循环 / 自定义节点)——它绑的是
   * provider 而不是编排。创建期后端即校验「引用存在且启用」(不存在/停用 → 400 点名连接);
   * 任务不可跨机搬运,故不像流程那样留到运行期才报错。
   */
  connection_id?: string | null;
  /**
   * 任务绑定的**工作区**(编码通道批次;空/缺省 = 未绑定)。
   *
   * 非空时是创建期就绪冻结的 canonical 绝对路径:工作区文件工具族(`fs_*`)以它为
   * 路径闸门根,`bash` 以它为 cwd 缺省与 jail 边界。未绑定的任务不下发该工具族。
   */
  workspace?: string | null;
}

/**
 * 任务用的**流程快照**(二维批次 5a;任务详情顶层 `flow_snapshot`)。
 *
 * 入口流程 + 其可达子流程闭包,由后端在「创建时(绑定任务)/ 执行开始时(未绑定任务)」
 * 冻结:运行态徽标读它而不是「当前流程库」,于是跑过任务后再改流程也不会让徽标漂移。
 * 未绑定且尚未跑过的任务为 null(读取侧按 optional 容错,零噪音)。
 */
export interface TaskFlowSnapshot {
  /** 入口流程 id(快照里的根;`flows` 中必有其一与之相等) */
  root_id: string;
  /** 入口流程 + 可达子流程闭包(入口恒为首个) */
  flows: AgentFlowConfig[];
}

/**
 * 任务子任务状态(与 server-rs task_service 实际写入值对齐):
 * running=执行中(create_subtask 插入即 running) / done=完成 / error=失败 / ended=已停止(取消)
 * pending=待执行:当前无写入点,但 cancel 逻辑对其做防御性判断,保留在值域内
 *
 * 批次 4 起终态口径收窄:done/error 是终态,再被 agentend 召回**不再改写**为 ended
 * (只补 finished_at),故 ended 只表示「真正中途中断」。
 */
export type TaskSubtaskStatus = 'pending' | 'running' | 'done' | 'error' | 'ended';

/** 任务子任务 */
export interface TaskSubtask {
  id: string;
  task_id: string;
  name: string;
  instruction: string;
  status: TaskSubtaskStatus;
  result: string;
  error: string;
  created_at: string;
  updated_at: string;
  /** 首次进入终态(done/error/ended)的时刻;pending/running 期间为空串 */
  finished_at: string;
}

/** 任务 token 累计(规划/步骤/汇总各次 LLM 调用落库聚合) */
export interface TaskUsageTotal {
  prompt_tokens: number;
  completion_tokens: number;
  reasoning_tokens: number;
}

/** 任务消息角色(批次 R2):user=用户指令 / assistant=执行者产出 */
export type TaskMessageRole = 'user' | 'assistant';

/**
 * 任务消息种类(批次 R2;对齐 server-rs task_messages.kind):
 * goal=创建时的用户目标 / result=首轮成果(实跑问题 1 起落库)
 * / normal=普通(旧行默认值) / followup=终态追加指令(R2a) / plan_chat=批准环节规划对话(R2b)
 */
export type TaskMessageKind = 'normal' | 'goal' | 'result' | 'followup' | 'plan_chat';

/** 任务消息(批次 R2 多轮用户输入;详情响应 messages 数组,created_at 升序) */
export interface TaskMessage {
  id: string;
  task_id: string;
  role: TaskMessageRole;
  kind: TaskMessageKind;
  content: string;
  created_at: string;
}

/** 任务详情(含子任务与该任务 token 累计;批次 R2 起携 messages 用户指令历史) */
export interface TaskDetail {
  task: TaskRecord;
  subtasks: TaskSubtask[];
  usage_total: TaskUsageTotal;
  /** 用户指令历史(followup 追加 / plan_chat 规划对话);旧服务端无此字段,读取须容错 */
  messages?: TaskMessage[];
  /**
   * 本任务的流程快照(二维批次 5a);旧服务端/未绑定且未跑过的任务为 null 或缺失。
   * 运行态徽标的数据源(优先于「当前流程库」)。
   */
  flow_snapshot?: TaskFlowSnapshot | null;
  /**
   * 工作区画像(CODE-4);未绑定工作区、或旧服务端 → `null`/缺失。
   * 由后端**实时**探测(不落库),故目录内容变化后刷新即得新结果。
   */
  workspace_profile?: WorkspaceProfile | null;
  /**
   * 交付摘要（TMR-6）；未绑定工作区、或旧服务端 → `null`/缺失。
   * 读时合成，不落库；工作区变化后刷新即反映当前状态。
   */
  delivery?: TaskDeliverySummary | null;
}

/**
 * 任务单次 LLM 调用记录(批次 3 L3 调用追踪;GET /api/tasks/{id}/calls,按 created_at,id 升序)。
 * phase:planner=规划 / step=步骤 / summarize=汇总 / agent=主Agent / subagent=子Agent / audit=审计;
 * status:ok=正常 / empty=空响应 / error=错误。
 */
export interface TaskLlmCall {
  id: string;
  task_id: string;
  /**
   * 调用归属阶段。**不含冒号**(前端流式缓冲 key 以第一个冒号切分 phase 与步骤号):
   * `step` = 外层流程节点;`subflow.<父节点下标链>` = 静态子图的节点
   * (如 `subflow.1` = 「外层下标 1 的节点挂载的子流程」里的节点,`subflow.1.2` 再嵌一层);
   * `call.<路径>` = 对比模式**被调流程**的节点(二维批次 7b;路径段 `d<n>` = 本层第 n 次
   * 动态调用,纯数字 = 宿主当时在静态子图内的父下标)。被调流程内部的静态子图记为
   * `subflow.<含 d 段的路径>`(如 `subflow.d1.2`)——「被调流程花了多少」须把这两类都算上
   * (判定收在 `utils/flowCallStats::isDynamicPhase`)。
   */
  phase: string;
  /** 步骤序号(0 起,展示时 +1):step = 节点在外层流程中的下标;subflow 与 call 前缀的 = 节点在本层图(子图 / 被调流程)中的下标 */
  step_index: number | null;
  model: string;
  /** 提示词摘要(面板展开查看) */
  prompt_summary: string;
  /** 响应摘要(面板展开查看) */
  response_summary: string;
  prompt_tokens: number;
  completion_tokens: number;
  reasoning_tokens: number;
  elapsed_ms: number;
  status: string;
  /**
   * 上游 finish_reason(stop / length / content_filter 等;可观测性问题①截断标记)。
   * 'length' = max_tokens 截断(响应为半截文本);'' = 未知/未下发(旧行兼容值)。
   * 旧服务端无此列,字段可能缺省,读取须容错。
   */
  finish_reason?: string;
  created_at: string;
}

/**
 * 任务文件变更台账的一行(批次 4 / 4b,PRODCAP-4「交付可审计」)。
 * 与后端 `server-rs/src/models/types.rs::TaskFileChangeRecord` 一对一
 * (登记在 `tools/check-contract.mjs` 的 MAPPINGS;改字段须两侧同改)。
 */
export interface TaskFileChange {
  id: number;
  task_id: string;
  /** 工作区内**相对路径**(正斜杠);响应不携带绝对路径 */
  path: string;
  op: 'create' | 'modify' | 'delete' | 'rollback';
  /** `tool` = fs_write/fs_edit 精确记账;`bash` = 命令前后树扫描的**启发式**检出;`rollback` = 回滚自身 */
  source: 'tool' | 'bash' | 'rollback';
  /** 改动前正文的哈希;null = 无前态(新建)或基线未知 */
  before_hash: string | null;
  after_hash: string;
  before_bytes: number;
  after_bytes: number;
  /** 基线正文超留存上限(256KB)/ 驻留预算打满 / 读取失败 → 无正文可 diff、不可回滚 */
  truncated: boolean;
  /** 能否 diff / 回滚;`truncated` 或新建(可删除回滚)各有形态,以本字段为准 */
  has_baseline: boolean;
  created_at: string;
}

/**
 * `GET /api/tasks/{id}/changes` 的载荷。
 * **`undected` 是顶层字段而不是清单里的行**(批次 4b):`changes` 一行 = 一个文件改动,
 * 而「扫描没扫完」不是改动,且零改动的命令也要能留痕;混进数组会污染长度语义。
 */
export interface TaskChangesPayload {
  changes: TaskFileChange[];
  /** 有过「未能完整检出」的扫描(bash 侧);true 时 `undectedReason` 给出最近一次原因 */
  undected: boolean;
  /** 最近一次缺项的中文原因(前端横幅按它分档,不得用固定文案冒充);null = 从未缺项 */
  undectedReason: string | null;
}

/**
 * `POST /api/tasks/scratch/cleanup` 的载荷(PRODCAP-5):立即执行一次任务草稿回收。
 * `keep_days = 0` 表示保留策略已关闭(`KEDAI_TASK_SCRATCH_KEEP_DAYS=0`,不清理)。
 */
export interface TaskScratchCleanup {
  /** 被删除的任务目录名(任务 id) */
  removed: string[];
  /** 因任务在跑/待批准而跳过的目录名 */
  skipped_active: string[];
  /** 未超过保留期而保留的目录数 */
  kept_fresh: number;
  /** 删除失败的目录名(原因见服务端日志) */
  failed: string[];
  /** 生效的保留天数(0 = 已关闭,未执行清理) */
  keep_days: number;
}

/** `GET /api/tasks/{id}/changes/diff` 的载荷;三种形态都 200,靠 `available` 区分 */
export interface TaskChangeDiff {
  available: boolean;
  path: string;
  /** unified diff;**超预算时该字段本身是「改动过大…」说明文本**,不是空串 */
  diff?: string;
  /** 新建形态的补充说明(全量新增行) */
  note?: string;
  /** `available:false` 时为什么拿不到基线(绝不返回空 diff 冒充) */
  reason?: string;
}

/** `POST /api/tasks/{id}/changes/rollback` 的载荷;`ok:false` 时原因在 `reason` */
export interface TaskChangeRollback {
  ok: boolean;
  path: string;
  restored_bytes?: number;
  /** true = 该行是新建,回滚即删除 */
  removed?: boolean;
  note?: string;
  available?: boolean;
  reason?: string;
}

/** `POST /api/tasks/{id}/changes/rollback-all` 的**单项**报告(CODE-2) */
export interface TaskChangeRollbackItem {
  path: string;
  /** `restored`=恢复正文 / `removed`=新建项删除 / `skipped`=无基线跳过 / `failed`=落盘失败 */
  result: 'restored' | 'removed' | 'skipped' | 'failed';
  restored_bytes?: number;
  note?: string;
  /** skipped / failed 时的原因(原文展示给用户) */
  reason?: string;
}

/**
 * `POST /api/tasks/{id}/changes/rollback-all` 的载荷(CODE-2)。
 * **逐项报告、不整体否决**(计划 Q2=(a)):某项无基线/落盘失败不拦住其余项;
 * `ok = failed === 0`——skipped 不算失败,但必须在界面上说得出来(不静默)。
 */
export interface TaskChangeRollbackAll {
  ok: boolean;
  restored: number;
  removed: number;
  skipped: number;
  failed: number;
  results: TaskChangeRollbackItem[];
}
