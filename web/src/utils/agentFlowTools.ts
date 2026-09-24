// Agent 执行流程步骤的工具模式三态工具函数
// 与后端 PlanStep.tools 语义对齐:null=不使用工具;[]=全部工具;非空数组=白名单。
// 从 SettingsModal.vue 抽出为纯函数,便于单测(含「白名单模式切换」回归测试)。

import type { AgentFlowStep } from '../api/types';

/**
 * 步骤级输出上限(tokens)的允许区间——**与后端校验同口径**。
 * 后端单一出处:`server-rs/src/services/agent_flow_service.rs` 的 `validate_flow`
 * 只收 1..=32768(超出即 400「输出上限需在 1-32768 之间」)。
 * 编辑器此前把 `max` 写死 131072,用户可填不可存(收口批 2026-09-24 对齐);
 * 两端若再改区间,必须同批改这里与后端,`AgentFlowStepEditor.limits.test.ts` 锁定。
 */
export const STEP_OUTPUT_TOKENS_MIN = 1;
export const STEP_OUTPUT_TOKENS_MAX = 32768;

/** 工具模式三态 */
export type StepToolMode = 'none' | 'all' | 'list';

/**
 * 由 tools 字段读回当前模式:
 *   null/undefined → none;空数组 → all;非空数组(含仅占位空串的 ['']) → list
 */
export function stepToolMode(s: Pick<AgentFlowStep, 'tools'>): StepToolMode {
  if (s.tools === null || s.tools === undefined) return 'none';
  return s.tools.length === 0 ? 'all' : 'list';
}

/**
 * 切换工具模式(直接写回 s.tools):
 *   none → null;all → [];list → 非空数组(占位空串,由白名单输入框填充后保存时清洗)
 * 注意:list 必须产生非空数组,否则 stepToolMode 读回 all,下拉弹回、白名单输入框不显示。
 */
export function setStepToolMode(s: Pick<AgentFlowStep, 'tools'>, mode: StepToolMode): void {
  if (mode === 'none') {
    s.tools = null;
  } else if (mode === 'all') {
    s.tools = [];
  } else {
    // 白名单:已有非空列表保留(来回切换不清空);否则置占位空串等待用户输入
    s.tools = Array.isArray(s.tools) && s.tools.length > 0 ? s.tools : [''];
  }
}

/** 白名单文本 → tools 数组(逗号/中文逗号/空白分隔,去空白与空项) */
export function stepToolsText(s: Pick<AgentFlowStep, 'tools'>): string {
  return (s.tools ?? []).join(', ');
}

/** 白名单输入文本 → tools 数组 */
export function setStepToolsText(s: Pick<AgentFlowStep, 'tools'>, text: string): void {
  s.tools = text
    .split(/[,，\s]+/)
    .map((t) => t.trim())
    .filter(Boolean);
}

/**
 * 保存前清洗 tools:去空白项。语义区分:
 *   - 显式 [] (全部工具) 原样保留为 []——绝不能转成 null(否则"全部"变"不使用")
 *   - 仅占位空串 [''](选了白名单但没填工具名)→ null(不使用),避免提交空字符串
 *   - 真实非空列表去空白项后保留
 */
export function cleanStepTools(tools: AgentFlowStep['tools']): AgentFlowStep['tools'] {
  if (tools === null || tools === undefined) return null;
  const cleaned = tools
    .map((t) => (typeof t === 'string' ? t.trim() : String(t)))
    .filter((t) => t.length > 0);
  if (cleaned.length > 0) return cleaned;
  // 清洗后为空:显式 [] = 全部工具(保留);[''] 占位 = 白名单未填(不使用)
  return tools.length === 0 ? [] : null;
}

/** 全部步骤保存前统一清洗 tools(就地修改步骤数组元素) */
export function cleanStepsTools(steps: AgentFlowStep[]): void {
  for (const s of steps) {
    s.tools = cleanStepTools(s.tools);
  }
}

/**
 * 切换动作(执行/反思)后的字段归一化:反思步骤不生成正文,故清掉生成类字段。
 * 由 useAgentFlow 下沉为纯函数——列表视图与画布 Inspector 共用同一份,
 * 避免两处各写一遍后漂移(该文件历史上出过 embedded/standalone 双模板漂移)。
 *
 * `sub_flow_id` 与生成类别同纪律:后端 `validate_flow` 同样拒「反思 + 挂载子流程」,
 * 不清就会「点一下动作下拉,流程立刻不可保存」,而折叠态的行/卡片上没有任何标记
 * (警示只在展开的编辑区里)。
 */
export function normalizeStepAction(s: AgentFlowStep): void {
  if (s.action === 'reflect') {
    s.generates = undefined;
    s.system_prompt = null;
    s.temperature = null;
    s.max_tokens = null;
    s.tools = null;
    s.tool_choice = 'auto';
    s.tool_choice_function = null;
    s.parallel_tool_calls = null;
    s.sub_flow_id = null;
  } else if (s.generates === undefined) {
    s.generates = true;
  }
}

/**
 * 任务工具策略(与后端 `settings.task_tool_policy` 同口径)。
 * 仅用于编辑区本地提示,**不参与保存**——权威判定始终在后端。
 */
export type TaskToolPolicy = 'all' | 'deny_dangerous' | 'allowlist';

/** 取交提示的上下文(缺省 = 策略未知,不产出取交提示) */
export interface ToolPolicyCtx {
  policy: TaskToolPolicy;
  /** `policy=allowlist` 时的策略白名单(前端据此**精确**判定交集是否为空) */
  allowlist?: readonly string[];
}

/**
 * 白名单档与任务工具策略取交后的可用性提示(自定义流程收口批,2026-09-24)。
 *
 * 背景:后端 custom 节点实际下发/放行的工具 = **策略编译集 ∩ 步骤白名单**
 * (`task_engine/tool_policy.rs::compile` 与 `custom.rs` 的取交;策略自身还会剔除元工具与
 * 危险级,`deny_dangerous` 按工具名给 bash 开例外)。2026-09-24 起空名单闸门是 fail-closed
 * ——交集为空时该节点**不下发任何工具**,模型臆造的工具调用被直接拒绝;而编辑区原本毫无提示,
 * 用户配了白名单却拿不到工具时无从判断。
 *
 * 精确性纪律:危险级分类与元工具名单都在后端,前端**不复制**这两份分类,故:
 *   - `allowlist` 档可精确判定(策略白名单在前端设置里可见):无交集 → 告警;有交集 → 不提示;
 *   - `deny_dangerous` 档无法精确判定 → 只给「可能被整体剔除」的说明,不臆断为错误;
 *   - `all` 档 → 不提示(策略侧只再剔除元工具,已由「全部工具」那条警告覆盖口径)。
 */
function listPolicyNotes(
  s: Pick<AgentFlowStep, 'tools'>,
  ctx: ToolPolicyCtx,
): string[] {
  // 仅占位空串(选了白名单但没填工具名):保存时会清洗为「不使用工具」,此处不打扰
  const names = (s.tools ?? []).map((t) => String(t).trim()).filter(Boolean);
  if (names.length === 0) return [];
  if (ctx.policy === 'allowlist') {
    const allow = ctx.allowlist ?? [];
    if (names.some((n) => allow.includes(n))) return [];
    return [
      '本节点白名单与「任务工具白名单」(设置 → 工具策略)无交集 → 本节点不会下发任何工具,模型发起的工具调用会被直接拒绝;请至少保留一个两边都有的工具名。',
    ];
  }
  if (ctx.policy === 'deny_dangerous') {
    return [
      '任务工具策略(设置 → 工具策略)会先剔除元工具(agentgo/agentend 等编排类)与危险级工具,再与本白名单取交;若白名单里的工具全被剔除,交集为空 → 本节点不下发任何工具,模型发起的工具调用会被直接拒绝。',
    ];
  }
  return [];
}

/**
 * 步骤工具配置的警示文案(F8,2026-09-10 六模式实跑修复)。
 * 背景:`tools: []` 语义是「全部工具」(非「不使用」),极易误配——实测「理解意图」
 * 这类 generates=false 的分析步骤被配成全部工具,实际下发 11 个工具(含编排/写类)。
 * 返回需展示的警示列表(空数组 = 无警示);纯函数便于单测。
 *
 * `policyCtx` 为白名单档的取交提示(自定义流程收口批);不传则该提示不产出
 * (旧调用点行为不变)。
 */
export function stepToolsWarnings(
  s: Pick<AgentFlowStep, 'tools' | 'generates'>,
  policyCtx?: ToolPolicyCtx,
): string[] {
  const mode = stepToolMode(s);
  const out: string[] = [];
  if (mode === 'all') {
    out.push(
      '「全部工具」会下发全部已注册工具(含 agentgo/agentend 等编排类与 write 等写类);仅需要自由选用工具的生成步骤才选它。',
    );
  }
  if (s.generates === false && mode === 'all') {
    out.push(
      '本步骤已设为「不生成正文」(如「理解意图」),通常只需只读工具或不用工具;选「全部工具」会把它当作执行步骤,建议改为「不使用工具」或「白名单」。',
    );
  }
  if (mode === 'list' && policyCtx) {
    out.push(...listPolicyNotes(s, policyCtx));
  }
  return out;
}
