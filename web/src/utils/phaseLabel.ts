// 阶段标签(phase → 中文)与流式缓冲键解析:调用面板(CallTracePanel)与任务看台
// 「正在生成」块(TaskBoard)共用同一份实现。
//
// 为什么单独成模块:两处都要把内部 phase 词翻译成人话。原先只有调用面板有实现,
// 任务看台的多缓冲兜底直接插值原始 key,于是**内部键裸露给用户**(如 `【subflow.1:0】`,
// 见 `遗留.md` IFW-7③)。抽到此处后,新增消费方只需调用,不会再各写一份导致口径漂移。

/** 步骤名解析器:给 step 阶段的标签补一个可读的步骤名(取不到则返回 null) */
export type StepNameAt = (index: number) => string | null;

/**
 * 阶段中文标签。
 *
 * 契约(与后端 `TaskLlmCallRecord.phase` 的取值一一对应):
 *  - `step` = 自定义流程/legacy 的步骤执行,`stepIndex` 为 0 起下标,展示 +1;
 *  - `agent` = team 模式主 Agent 的子目标(`step_index` = 该子目标的**全局步骤下标**,
 *    见 `task_engine/team.rs`);带下标时一并显示,否则多缓冲前缀无法区分是哪一条;
 *  - `subagent` = 子 Agent 调用(恒无 step_index,`agent_tools_agent.rs` 落 None);
 *  - `subflow` / `subflow.<父下标>[.<父下标>…]` = 静态子图节点的调用(二维批次 6b),
 *    路径段是「从哪一步挂进来的」;子图内部节点不进 plan,故无步骤名可补;
 *  - 其余为固定词(规划/汇总/审计/终审),未命中分支原样回退 phase 本身
 *    ——脏数据宁可照实显示,也不要变成空白让人无从判断。
 */
export function phaseText(phase: string, stepIndex?: number | null, stepName?: StepNameAt): string {
  // 静态子图:phase = `subflow` 或 `subflow.<父下标>[.<父下标>…]`(父下标 0 起,展示 +1)。
  // 前缀判定放在 switch 之前:phase 不是固定词,故不能列成 case。
  if (phase === 'subflow' || phase.startsWith('subflow.')) {
    const path = phase
      .slice('subflow'.length)
      .split('.')
      .filter((seg) => seg !== '')
      // 脏数据(手改 phase)里的非数字段原样回退:不能把 NaN 渲染成 `#NaN`
      .map((seg) => {
        const n = Number(seg);
        return Number.isFinite(n) ? `#${n + 1}` : seg;
      })
      .join('›');
    const leaf = stepIndex != null ? `#${stepIndex + 1}` : '';
    const head = path ? `子流程(${path} 内)` : '子流程';
    return leaf ? `${head} ${leaf}` : head;
  }
  switch (phase) {
    case 'planner': return '规划';
    case 'step': return indexed('步骤', stepIndex, stepName);
    case 'summarize': return '汇总';
    case 'summary': return '汇总';
    case 'agent': return indexed('主Agent', stepIndex, stepName);
    case 'subagent': return '子Agent';
    case 'audit': return '审计';
    case 'final_audit': return '终审';
    default: return phase;
  }
}

/** 「<词> #N」+ 可选步骤名后缀(无序号时只返回词;名称为空/越界时不加后缀) */
function indexed(head: string, stepIndex?: number | null, stepName?: StepNameAt): string {
  if (stepIndex == null) return head;
  const withIndex = `${head} #${stepIndex + 1}`;
  const name = stepName?.(stepIndex)?.trim();
  return name ? `${withIndex} · ${name}` : withIndex;
}

/**
 * 流式缓冲 key → 可读标签。
 *
 * key 契约(后端 delta 事件的 `phase` / `step_index` 透出,前端 `stores/task.ts`
 * 的 `liveKey` 组装):`${phase}:${step_index ?? ''}`。**phase 不含冒号**
 * (子图为 `subflow.<父下标链>` 这类带点的非固定词,见 `docs/经验.md` E49),
 * 故按**首个**冒号切分即可。
 *
 * 非法 key(无冒号,或步骤号不是数字)按「只有 phase」处理:照实回退,不猜。
 */
export function bufferLabel(key: string, stepName?: StepNameAt): string {
  const sep = key.indexOf(':');
  const phase = sep < 0 ? key : key.slice(0, sep);
  const raw = sep < 0 ? '' : key.slice(sep + 1);
  const n = raw === '' ? Number.NaN : Number(raw);
  return phaseText(phase, Number.isFinite(n) ? n : undefined, stepName);
}
