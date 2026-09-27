//! 步骤图:线性兼容判定、输入解析、Kahn 拓扑排序、输出节点与整图校验(`validate_flow`)。

// 子模块内统一用 super::* 拿到 mod.rs 的公共词汇(类型/常量)与 re-export 的自由函数。
use super::*;
use crate::models::types::PlanStep;
use std::collections::{BTreeMap, BTreeSet};

/// 线性兼容模式判定:全部步骤的 `inputs` 均为空 → 语义回退为「按数组顺序串联」。
///
/// 该模式是**存量一维流程零迁移的保证**:线性模式下第 i 步的输入恒为第 i-1 步,
/// 拓扑序 == 数组顺序,生成消息格式与旧版逐字节一致(见 custom.rs 的单父模板)。
pub fn is_linear_compat(steps: &[PlanStep]) -> bool {
    steps.iter().all(|s| s.inputs.is_empty())
}

/// 解析每个步骤的有效输入集(逐个对应 `steps`)。
///
/// - 线性兼容模式:第 i 步输入 = 第 i-1 步(首步无输入);
/// - 二维模式:显式 `inputs` 按 id 解析;`inputs` 为空的步骤即**源节点**(只用任务目标)。
///
/// 校验(失败返回中文错误,供 400 展示):步骤 id 非空且唯一、上游必须存在且已启用
/// (调用方传入的已是启用步骤集)、不得自环。父节点下标按**升序**排列
/// (D3:多父合并顺序确定、可复现),且已去重(同一父写两次不产生重复段落)。
pub fn effective_inputs(steps: &[PlanStep]) -> Result<Vec<Vec<usize>>, String> {
    let mut index_of: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, s) in steps.iter().enumerate() {
        if s.id.trim().is_empty() {
            return Err(format!("步骤「{}」缺少 id(二维流程按 id 连接)", s.name));
        }
        if index_of.insert(s.id.as_str(), i).is_some() {
            return Err(format!("步骤 id 重复:{}", s.id));
        }
    }
    let linear = is_linear_compat(steps);
    let mut out: Vec<Vec<usize>> = Vec::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        if linear {
            out.push(if i == 0 { Vec::new() } else { vec![i - 1] });
            continue;
        }
        let mut parents: Vec<usize> = Vec::with_capacity(s.inputs.len());
        for parent_id in &s.inputs {
            if parent_id.trim().is_empty() {
                return Err(format!("步骤「{}」的上游列表含空 id", s.name));
            }
            let Some(&pi) = index_of.get(parent_id.trim()) else {
                return Err(format!(
                    "步骤「{}」引用了不存在或未启用的上游:{}",
                    s.name, parent_id
                ));
            };
            if pi == i {
                return Err(format!("步骤「{}」不能把自身作为上游", s.name));
            }
            if !parents.contains(&pi) {
                parents.push(pi);
            }
        }
        parents.sort_unstable();
        out.push(parents);
    }
    Ok(out)
}

/// 成果节点选拔(D4):显式 `is_output=true` 优先;未标注时取**无后继汇点**。
/// 候选集内取下标最大的「direct + generates=true」步骤;候选内无生成步时回退
/// 全流程最后一个生成步(存量线性流程:汇点即末步,语义与旧版一致);都没有 → None。
///
/// 注意:本函数只回答「谁是成果节点」;该节点产出为空时的兜底见 custom.rs
/// (按数组下标降序取上一个非空生成产出,保证结果与执行顺序无关)。
pub fn output_index(steps: &[PlanStep], inputs: &[Vec<usize>]) -> Option<usize> {
    let is_generating = |i: usize| steps[i].action == "direct" && steps[i].generates == Some(true);
    let explicit: Vec<usize> = (0..steps.len())
        .filter(|&i| steps[i].is_output == Some(true))
        .collect();
    let candidates: Vec<usize> = if explicit.is_empty() {
        let mut has_successor = vec![false; steps.len()];
        for parents in inputs {
            for &p in parents {
                if p < has_successor.len() {
                    has_successor[p] = true;
                }
            }
        }
        (0..steps.len()).filter(|&i| !has_successor[i]).collect()
    } else {
        explicit
    };
    candidates
        .iter()
        .rev()
        .copied()
        .find(|&i| is_generating(i))
        .or_else(|| (0..steps.len()).rev().find(|&i| is_generating(i)))
}

/// 流程图解析:有效输入集 + 执行拓扑序(校验与执行共用的一步到位入口)。
pub fn resolve_graph(steps: &[PlanStep]) -> Result<FlowGraph, String> {
    let inputs = effective_inputs(steps)?;
    let order = kahn_order(&inputs, steps)?;
    Ok(FlowGraph { inputs, order })
}

/// Kahn 拓扑排序:就绪队列用 BTreeSet 维护,恒按下标升序出队——线性兼容流程的
/// 拓扑序因此等于数组顺序(执行顺序与旧版完全一致),同一流程两次运行结果可复现。
fn kahn_order(inputs: &[Vec<usize>], steps: &[PlanStep]) -> Result<Vec<usize>, String> {
    let n = inputs.len();
    let mut indegree = vec![0usize; n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, parents) in inputs.iter().enumerate() {
        for &p in parents {
            if p >= n || p == i {
                continue; // 越界/自环已由 effective_inputs 拦截,此处仅防御
            }
            children[p].push(i);
            indegree[i] += 1;
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(&i) = ready.iter().next() {
        ready.remove(&i);
        order.push(i);
        for &child in &children[i] {
            indegree[child] -= 1;
            if indegree[child] == 0 {
                ready.insert(child);
            }
        }
    }
    if order.len() != n {
        let stuck: Vec<String> = (0..n)
            .filter(|&i| indegree[i] > 0)
            .map(|i| format!("「{}」", steps[i].name))
            .collect();
        return Err(format!(
            "自定义流程存在环,无法确定执行顺序:{}",
            stuck.join("、")
        ));
    }
    Ok(order)
}

/// 校验自定义流程(失败返回中文错误,PUT 时 400 给用户):
///  - 启用的步骤非空
///  - 至少一个「生成正文」的 direct 步骤(反思回退/输出依赖它)
///  - action 仅 direct / reflect;reflect 步骤不得携带 generates=Some(true)
///  - 温度 0.0-2.0、输出上限 1-32768(仅校验显式提供的值)
///  - 二维依赖边:步骤 id 唯一、上游必须存在且已启用、无自环、无环(见 `resolve_graph`)
///  - 反思步骤不得挂载静态子流程(反思不产出正文,挂子图无意义且会绕过反思语义)
///
/// **只管单个流程自身**:跨流程的子流程引用链由 `validate_sub_flows` 负责(它需要库)。
pub fn validate_flow(
    cfg: &AgentFlowConfig,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    // 未启用(开关关闭):允许保存任意状态(用户可先编辑、后启用);启用时才校验
    if !cfg.enabled {
        return Ok(());
    }
    let active: Vec<PlanStep> = cfg.steps.iter().filter(|s| s.enabled).cloned().collect();
    if active.is_empty() {
        return Err("自定义流程为空:请启用至少一个步骤".into());
    }
    if !active
        .iter()
        .any(|s| s.action == "direct" && s.generates == Some(true))
    {
        return Err("自定义流程缺少生成步骤:至少需要一个「生成正文」的 direct 步骤".into());
    }
    // 二维依赖边校验(线性兼容流程恒通过:隐式串联无悬空/无环风险)
    resolve_graph(&active)?;
    // 并行上限(二维批次 2):越界直接拒绝,避免执行期静默夹取
    if let Some(n) = cfg.max_parallel_nodes {
        if !(1..=MAX_PARALLEL_NODES_LIMIT).contains(&n) {
            return Err(format!(
                "并行节点上限需在 1-{} 之间(当前 {})",
                MAX_PARALLEL_NODES_LIMIT, n
            ));
        }
    }
    for s in &active {
        if s.action != "direct" && s.action != "reflect" {
            return Err(format!(
                "步骤「{}」动作非法(仅支持 direct / reflect)",
                s.name
            ));
        }
        if s.action == "reflect" && s.generates == Some(true) {
            return Err(format!("步骤「{}」为反思步骤,不应开启生成正文", s.name));
        }
        if s.action == "reflect" && s.system_prompt.is_some() {
            return Err(format!("步骤「{}」为反思步骤,不支持系统提示词", s.name));
        }
        // 反思步骤不产出正文,挂子图会绕过「反思判定 + 失败回退」语义 → 保存期直接拒绝
        if s.action == "reflect" && s.is_sub_flow() {
            return Err(format!("步骤「{}」为反思步骤,不支持挂载子流程", s.name));
        }
        // 节点档位(二维批次 6a):缺省/loose = 工具自循环;strict = 单次调用、不下发工具。
        // 只拒绝未知取值——允许的取值都有已实现的语义,不存在「静默无效」(裁定 15 差异 2 口径)
        if let Some(kind) = s.kind.as_deref() {
            if kind != "loose" && kind != "strict" {
                return Err(format!(
                    "步骤「{}」的节点档位「{}」非法(仅支持 loose / strict)",
                    s.name, kind
                ));
            }
        }
        if let Some(t) = s.temperature {
            if !(0.0..=2.0).contains(&t) {
                return Err(format!("步骤「{}」温度需在 0.0-2.0 之间", s.name));
            }
        }
        if let Some(m) = s.max_tokens {
            if !(1..=32768).contains(&m) {
                return Err(format!("步骤「{}」输出上限需在 1-32768 之间", s.name));
            }
        }
        // 节点级工具轮次上限(二维批次 5b):与设置项 `max_tool_rounds` 同口径 1-200。
        // 严格档下该参数无意义,但**不因此拒绝**——与「严格档保留工具配置」同一纪律
        // (配置保留,切回宽松档即生效;编辑器在严格档收起该输入)。
        if let Some(r) = s.max_tool_rounds {
            if !(1..=MAX_TOOL_ROUNDS_LIMIT).contains(&r) {
                return Err(format!(
                    "步骤「{}」工具轮次上限需在 1-{} 之间(当前 {})",
                    s.name, MAX_TOOL_ROUNDS_LIMIT, r
                ));
            }
        }
        // 节点级连接(二维批次 5b)**有意不校验引用是否存在**:流程库可导出/跨机导入,
        // 而连接是本机设置(settings.json),保存期拒绝会把可移植流程变成本机绑定。
        // 失效引用在运行期明确报错(不静默回退默认连接),编辑期由前端显示「(已失效)」。
        // 节点级上下文上限(二维批次 8):严格档/宽松档**都**生效(它管的是输入装配,不是工具面),
        // 故不像 max_tool_rounds 那样区分档位;挂载子流程的节点在运行期旁路该字段(配置保留)。
        if let Some(c) = s.max_context {
            if !(MIN_STEP_MAX_CONTEXT..=MAX_STEP_MAX_CONTEXT).contains(&c) {
                return Err(format!(
                    "步骤「{}」上下文上限需在 {}-{} 之间(当前 {})",
                    s.name, MIN_STEP_MAX_CONTEXT, MAX_STEP_MAX_CONTEXT, c
                ));
            }
        }
        // 节点级单次调用超时 / 空产出重试(A 批):与既有节点级参数同一纪律——保存期只校验
        // 区间,取值语义由执行器消费;挂载子流程的节点上两者都被旁路,但**不拒绝**配置保留
        // (与「严格档保留工具配置」一致:清空 sub_flow_id 即生效)。
        if let Some(t) = s.call_timeout_secs {
            if !(MIN_STEP_CALL_TIMEOUT_SECS..=MAX_STEP_CALL_TIMEOUT_SECS).contains(&t) {
                return Err(format!(
                    "步骤「{}」单次调用超时需在 {}-{} 秒之间(当前 {})",
                    s.name, MIN_STEP_CALL_TIMEOUT_SECS, MAX_STEP_CALL_TIMEOUT_SECS, t
                ));
            }
        }
        if let Some(n) = s.max_retries {
            if !(MIN_STEP_MAX_RETRIES..=MAX_STEP_MAX_RETRIES).contains(&n) {
                return Err(format!(
                    "步骤「{}」空产出重试次数需在 {}-{} 之间(当前 {})",
                    s.name, MIN_STEP_MAX_RETRIES, MAX_STEP_MAX_RETRIES, n
                ));
            }
        }
        if s.goal.trim().is_empty() {
            return Err(format!("步骤「{}」缺少目标说明", s.name));
        }
        if let Some(names) = &s.tools {
            for name in names {
                if name.trim().is_empty() || !registered_tools.contains(name) {
                    return Err(format!("步骤「{}」引用了未注册工具: {}", s.name, name));
                }
            }
        }
        let choice = s.tool_choice.as_deref().unwrap_or("auto");
        if !matches!(choice, "auto" | "none" | "required" | "function") {
            return Err(format!("步骤「{}」tool_choice 非法", s.name));
        }
        if choice == "required" && s.tools.is_none() {
            return Err(format!(
                "步骤「{}」tool_choice=required 时必须配置有效工具",
                s.name
            ));
        }
        if choice == "function" {
            let function = s
                .tool_choice_function
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("步骤「{}」tool_choice=function 时必须指定工具", s.name))?;
            let effective = match &s.tools {
                None => false,
                Some(names) if names.is_empty() => registered_tools.contains(function),
                Some(names) => names.iter().any(|name| name == function),
            };
            if !effective {
                return Err(format!(
                    "步骤「{}」指定的 function 工具不在有效工具内: {}",
                    s.name, function
                ));
            }
        }
    }
    Ok(())
}

// ==================== 静态子图原语(二维批次 6b) ====================
//
// **单一出处**:保存期校验(`validate_sub_flows`)、任务侧执行器
// (`task_engine/custom.rs::run_sub_flow`)、聊天侧线性化(`expand_sub_flows`)三处共用
// 下面这些原语。与二维批次 1 的图原语同一纪律:不得在任一侧复制「什么算合法子流程引用」
// 的判断(否则编辑器能保存的引用与能执行的引用会漂移)。
//
// 形态决策(见 `docs/计划.md` 裁定 18):子图取**引用流程 id**形态,不取内联嵌套。
// 理由:内联嵌套需要一整套嵌套编辑 UI(列表 + 画布双层),成本远超本批规模;而引用形态
// 与批次 7 的「子执行 + 深度守卫 + 记账」原语同构,且静态引用的环/深度都能在保存期完全
// 判定(内联嵌套没有这个问题,但也没有跨流程复用的价值)。

/// 生效的并行上限:缺省取默认值,越界取边界(1 = 完全串行,行为与二维批次 1 一致)
pub fn effective_max_parallel(cfg: &AgentFlowConfig) -> usize {
    cfg.max_parallel_nodes
        .unwrap_or(DEFAULT_MAX_PARALLEL_NODES)
        .clamp(1, MAX_PARALLEL_NODES_LIMIT) as usize
}
