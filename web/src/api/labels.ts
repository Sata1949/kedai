// 任务模式 / 任务消息种类的**唯一中文文案源**(批次 G.2)。
//
// 背景:这些文案此前分别手写在 TaskBoard.vue(MODE_LABELS / MESSAGE_KIND_LABELS)与
// TaskModeSelect.vue(<option> 内联)两处,新增一个模式要改「后端枚举 + 发射点 + types.ts
// + task store switch + 两处组件文案」——漏改不会报错,只会在 UI 上暴露为空白/错文案。
//
// 收敛方式:
//   - 文案集中本文件,组件一律引用;
//   - 用 `satisfies Record<TaskRunMode, string>` 做**穷尽校验**:后端新增模式而此处漏配,
//     `vue-tsc` 直接报错(把「漏改」从运行期静默变成编译期失败)。
//   - MESSAGE_KIND_LABELS 的键来自后端 task_messages.kind,非联合类型,故用
//     `Record<string, string>` 并保留「未登记返回空」的既有语义。
import type { ApiStyle, ConnectorType, TaskApproveExecMode, TaskRunMode } from './types';

/** 连接器类型 → 展示文案(多套连接批次)。
 *  键来自后端 `connectors::available_connector_types()`(非联合类型可穷尽),
 *  故按 MESSAGE_KIND_LABELS 先例用 `Record<string, string>`,未登记项原样展示。 */
export const CONNECTOR_TYPE_LABELS: Record<string, string> = {
  'openai-compatible': 'OpenAI 兼容',
  mock: '演示(Mock)',
};

/** 连接器类型下拉顺序(与后端定义顺序一致,便于对照) */
export const CONNECTOR_TYPE_ORDER: ConnectorType[] = ['openai-compatible', 'mock'];

/** 连接能力位 → 展示文案与说明(2026-10-02 视觉能力包 D1;2026-10-03 VISION-L6 收口)。
 *  键与后端 `ConnectionProfile` 的 supports_* / image_auto_split 字段一一对应;
 *  `reserved` = 效果面暂无消费、勾选不改变行为(如实标注,避免「勾了没反应」的困惑)。 */
export interface ConnectionCapabilityMeta {
  key:
    | 'supports_vision'
    | 'supports_structured_output'
    | 'supports_prefix_completion'
    | 'supports_mid_conversation_system'
    | 'image_auto_split';
  label: string;
  tip: string;
  reserved?: boolean;
}

/** 勾选组展示顺序:先功能项、后预留项 */
export const CONNECTION_CAPABILITIES: ConnectionCapabilityMeta[] = [
  {
    key: 'supports_vision',
    label: '视觉输入',
    tip: '该连接对应模型可接收图像:聊天贴图、截图与视觉工具的图像会随请求发送。纯文本模型(不支持视觉)请勿勾选——未勾选时贴图不会随请求发送,避免无效报错。',
  },
  {
    key: 'image_auto_split',
    label: '大图自动拆分',
    tip: '对图像尺寸限制较严的端点(如 DeepSeek):长边超过 1300px 的大图会在发送前自动等比切块,提升识别成功率。可直接接收大图的模型(如 Kimi / GPT-4o 等)无需勾选。',
  },
  {
    key: 'supports_structured_output',
    label: '结构化输出',
    tip: '勾选后,引擎的结构化产物调用(如变量状态更新)会随请求要求 JSON 模式(Chat Completions: response_format;Responses: text.format)。Anthropic 方言无等价参数,自动走提示词约定。',
  },
  {
    key: 'supports_prefix_completion',
    label: '前缀续写',
    tip: '预留能力位:已接入连接构建与指纹,引擎当前没有前缀续写生成路径,勾选暂不改变行为。',
    reserved: true,
  },
  {
    key: 'supports_mid_conversation_system',
    label: '中途系统插入',
    tip: '勾选后,对话中途插入的系统级内容(世界书 @INJECT、生成前激发条目)在 Anthropic / Responses 方言下就地以 user 消息承载(保留位置);不勾选时这类内容上提合并到顶部系统提示。Chat Completions 方言本就原样透传。',
  },
];

/** 接口方言 → 展示文案(接口方言批次;键与后端 `connectors::openai_compatible` 取值域一致) */
export const API_STYLE_LABELS: Record<string, string> = {
  'chat-completions': 'OpenAI Chat Completions',
  responses: 'OpenAI Responses',
  anthropic: 'Anthropic Messages',
};

/** 接口方言下拉顺序(默认档在前) */
export const API_STYLE_ORDER: ApiStyle[] = ['chat-completions', 'responses', 'anthropic'];

/** 模式 → 短标签(任务卡头部展示)。穷尽校验:漏配模式编译期报错。 */
export const MODE_LABELS = {
  legacy: '三段式',
  solo: '单 Agent',
  multi: '多 Agent',
  plan: '先规划后批准',
  team: '团队协作',
  custom: '自定义流程',
} satisfies Record<TaskRunMode, string>;

/** 模式 → 下拉选项文案(选择器)。在短标签基础上标注默认项。 */
export const MODE_OPTION_LABELS = {
  legacy: '模式:三段式(默认)',
  solo: '模式:单 Agent',
  multi: '模式:多 Agent',
  plan: '模式:先规划后批准',
  team: '模式:团队协作',
  custom: '模式:自定义流程',
} satisfies Record<TaskRunMode, string>;

/** 下拉展示顺序(与后端 TaskRunMode 定义顺序一致,便于对照) */
export const MODE_ORDER: TaskRunMode[] = ['legacy', 'solo', 'multi', 'plan', 'team', 'custom'];

/**
 * **流程用法**(二维批次 7b;前端概念,**不是线格式字段**)。
 * - `force` 强制:任务只跑指定流程(或当前流程),模型不能调用别的流程;
 * - `compare` 对比:根流程照常执行,随后**名单内流程**作为 `run_flow` 工具释放给宽松节点,
 *   模型在工具循环里自主调用、取回成果。
 *
 * 与后端的对应关系只有一条:`compare` ⇔ 创建任务时下发了**非空** `flow_ids`
 * (后端把「给了空数组」判 400,故「空名单」不是强制模式的写法)。仅 custom 模式可见。
 */
export type TaskFlowMode = 'force' | 'compare';

/** 流程用法 → 下拉文案(穷尽校验同 MODE_LABELS) */
export const FLOW_MODE_OPTION_LABELS = {
  force: '流程模式:强制',
  compare: '流程模式:对比(模型可调用名单内流程)',
} satisfies Record<TaskFlowMode, string>;

/** 流程用法 → 徽标短标签(任务详情/看台展示用;对比模式的可调用数另拼) */
export const FLOW_MODE_LABELS = {
  force: '强制',
  compare: '对比',
} satisfies Record<TaskFlowMode, string>;

/** 流程用法下拉顺序(默认项置顶) */
export const FLOW_MODE_ORDER: TaskFlowMode[] = ['force', 'compare'];

/**
 * plan 批准时的执行方式 → 下拉文案(2026-09-17)。
 * 穷尽校验同上:后端新增可选值而此处漏配,`vue-tsc` 直接报错。
 * 顺序 = 下拉展示顺序:默认项(按计划逐步执行)在最前。
 */
export const APPROVE_EXEC_MODE_LABELS = {
  approved_plan: '按计划逐步执行(默认)',
  solo: '单 Agent 整体执行',
  multi: '多 Agent 执行',
  team: '团队协作执行',
  custom: '自定义流程执行',
} satisfies Record<TaskApproveExecMode, string>;

/** 批准下拉展示顺序(默认项置顶) */
export const APPROVE_EXEC_MODE_ORDER: TaskApproveExecMode[] = [
  'approved_plan',
  'solo',
  'multi',
  'team',
  'custom',
];

/**
 * 任务消息种类 → 小标签。键为后端 `task_messages.kind`(字符串,非联合类型),
 * 故不设穷尽校验;未登记种类返回空串(不显示标签,与既有行为一致)。
 * `normal` 为旧行遗留,不显示标签。
 */
export const MESSAGE_KIND_LABELS: Record<string, string> = {
  goal: '目标',
  result: '成果',
  followup: '追加',
  plan_chat: '规划对话',
};

/** 消息种类标签(未登记返回空串) */
export function messageKindLabel(kind: string): string {
  return MESSAGE_KIND_LABELS[kind] ?? '';
}
