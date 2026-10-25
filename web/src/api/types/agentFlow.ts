// 自定义执行流程域类型(聊天 Agent 计划 / 流程库 / 导入导出)。

export interface AgentPlan {
  plan: {
    steps: Array<{ goal: string; action: string; generates?: boolean; name?: string }>;
    summary: string;
  };
  summary: string;
  tools: string[];
  history: { state: string; plan: string[]; step_index: number } | null;
}

export type AgentMode = 'fast' | 'deep' | 'agent' | 'custom';

/** 单步执行流程(与后端 PlanStep 对齐;tools:null=不使用,[]=全部,列表=白名单) */
export interface AgentFlowStep {
  id: string;
  name: string;
  enabled: boolean;
  goal: string;
  /** direct=执行/生成, reflect=反思(不生成) */
  action: 'direct' | 'reflect';
  /** 是否生成正文(direct 步骤必选;reflect 步骤必须缺省) */
  generates?: boolean;
  /** 步骤级系统提示词(支持酒馆宏),追加到 system 末尾,带 [本步指令] 标记 */
  system_prompt?: string | null;
  temperature?: number | null;
  max_tokens?: number | null;
  tools?: string[] | null;
  /** 工具选择策略;function 时需同时填写 tool_choice_function */
  tool_choice?: 'auto' | 'none' | 'required' | 'function' | null;
  tool_choice_function?: string | null;
  parallel_tool_calls?: boolean | null;
  /** 上游步骤 id 列表(二维流程依赖边;空/缺省 = 线性串联或源节点) */
  inputs?: string[];
  /** 是否显式标注为最终成果节点(true = 标注;null/缺省 = 按无后继汇点自动判定) */
  is_output?: boolean | null;
  /** 画布坐标(二维批次 3 画布用;列表视图不写) */
  x?: number | null;
  y?: number | null;
  /** 节点档位(二维批次 6a):loose/缺省 = 允许工具自循环;strict = 单次模型调用、不下发工具 */
  kind?: 'loose' | 'strict' | null;
  /**
   * 挂载的静态子流程 id(二维批次 6b):非空时本节点**不自己发起模型调用**,
   * 改为把被引用流程当子图跑一遍,子图成果即本节点产出。
   * 与档位同一纪律——`goal`/`action`/`kind`/`tools`/`system_prompt`/`temperature`/
   * `max_tokens` 被旁路但**配置保留**,清空本字段即恢复生效。
   */
  sub_flow_id?: string | null;
  /**
   * 节点级连接(二维批次 5b):该节点走哪一套 API 连接(`ConnectionProfile.id`)。
   * 缺省 = 默认连接(与 5b 之前一致);provider 与模型随该连接,故「节点级模型」= 选连接。
   * 保存期**不校验引用是否存在**(流程可导出/跨机导入,而连接是本机设置):
   * 运行期若该连接已删除/已停用,该节点**明确报错**,不回退默认连接。
   * 挂载子流程的节点上本字段被旁路(子图各节点各自解析),配置保留。
   */
  connection_id?: string | null;
  /** 节点级工具循环轮次上限(二维批次 5b;1-200,缺省 = 沿用全局设置)。严格档下不下发工具,该配置保留但不生效 */
  max_tool_rounds?: number | null;
  /**
   * 节点级上下文上限(二维批次 8;256-1048576,缺省 = **不裁剪**)。
   * 语义:进入本节点的输入(系统提示 + 任务目标 + 各上游产出)整体的 token 预算;
   * 超限时从**最旧的上游产出段**起省略(正文换成标记、标签行保留),任务目标恒保留——
   * 它单独超限即该节点报错。**不裁工具轮内历史**(那是工具历史预算设置的事)。
   * 挂载子流程的节点上本字段被旁路(子图各节点各自裁剪),配置保留。
   */
  max_context?: number | null;
  /**
   * 节点级**单次 LLM 调用**超时(秒;A 批 A1;30-3600,缺省 = 沿用宿主缺省 300 秒看门狗)。
   * 语义:覆盖本节点每次调用的时间预算——可收紧(如 60)也可放宽(如 900);
   * 宽松档的工具循环**逐轮**各按它计(每轮一次调用,与全局看门狗同一粒度)。
   * 超时 = 本节点失败,**不自动重试**(重试是 `max_retries` 的显式配置)。
   * 挂载子流程的节点上本字段被旁路(子图各节点各自计时),配置保留。
   */
  call_timeout_secs?: number | null;
  /**
   * 节点级**空产出重试次数**(A 批 A2;1-5 = 额外尝试上限,总尝试 = 1 + n;
   * 缺省/无值 = 不重试,与 A 批之前逐字节一致)。
   * 语义:**只重试产出为空**的尝试;连接失败/超时/解析错误等硬错误一律不重试;
   * 每次尝试各记一行调用记录,重试前输出预算翻倍(封顶 131072)。
   * 挂载子流程的节点上本字段被旁路(子图各节点各自重试),配置保留。
   */
  max_retries?: number | null;
}

/** 自定义执行流程配置(单个流程,data/agent_flows.json 流程库中的一项) */
export interface AgentFlowConfig {
  /** 流程 id(库内唯一;空 = 新建,由后端分配) */
  id?: string;
  /** 流程名称(选择器展示) */
  name?: string;
  /** 流程说明 */
  description?: string | null;
  enabled: boolean;
  steps: AgentFlowStep[];
  /** 并行节点数上限(1-8;缺省 2;1 = 完全串行)。并行会成倍消耗 token */
  max_parallel_nodes?: number | null;
  /**
   * 虚拟文件夹路径(IMPHUB 附件管理;空串 = 未分组)。
   * 后端空值**省略序列化**(保 flow_fingerprint 逐字节不变),故本字段可缺省。
   */
  folder?: string;
}

/** 流程库(全局):当前选中的流程 + 全部流程 */
export interface AgentFlowLibrary {
  current_flow_id: string | null;
  flows: AgentFlowConfig[];
  /**
   * 已注入过的**能力包流程 id**(CODE-5):后端的「防重复注入 + 删过/改过不复活」凭据。
   * 前端不消费(纯后端簿记),但它确实在线上格式里(非空时才出现),故按可选声明——
   * 漏声明会让 `check-contract` 判「前端缺字段」。
   */
  seeded_pack_ids?: string[];
}

/** 一条 id 重映射记录(二维批次 7a 导入报告):同 id 内容不同时,导入的那份被分配了新 id */
export interface FlowIdRemap {
  /** 文件里的原 id */
  old_id: string;
  /** 本库实际分配的新 id */
  new_id: string;
  /** 流程展示名(报告文案里点名用) */
  name: string;
}

/** 导入报告(二维批次 7a):导入 N 个 / 跳过 M 个(内容已存在)/ 其中 K 个分配了新 id */
export interface FlowImportReport {
  /** 本次写入库的流程数(新增 + 覆盖) */
  imported: number;
  skipped: number;
  renamed: FlowIdRemap[];
  /**
   * 被**覆盖**的流程(A 批 B4;仅 `on_conflict=replace` 会非空)。
   * 覆盖保留 id(引用不破),故只有 id 与名称,没有「谁变成了谁」的映射。
   */
  replaced: FlowReplacedFlow[];
}

/** 被覆盖的流程(A 批 B4):覆盖保留 id,故只记 id 与展示名 */
export interface FlowReplacedFlow {
  id: string;
  name: string;
}

/**
 * 流程搬运包(导出文件,二维批次 7a):入口(或当前)流程 + 其可达子流程闭包,一次性冻结。
 * 导入侧按版本键判定:`kedai_flow_bundle` 缺失 = 旧格式(单流程 / {config} 包装 / 库格式)。
 * **不含**连接定义与密钥(连接是本机设置);节点级 `connection_id` 原样保留,跨机后会显示
 * 「(引用已失效)」并由运行期报错兜住(二维批次 5b 口径)。
 */
export interface AgentFlowBundle {
  kedai_flow_bundle: number;
  exported_at?: string;
  root_id?: string | null;
  flows: AgentFlowConfig[];
}
