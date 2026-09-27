//! 子流程:调用深度/次数校验、子流程展开与步骤 id 去重。

// 子模块内统一用 super::* 拿到 mod.rs 的公共词汇(类型/常量)与 re-export 的自由函数。
use super::*;
use crate::models::types::PlanStep;
use std::collections::{BTreeMap, BTreeSet};

/// 子流程引用链校验(二维批次 6b,保存期 400):
///  - 被引用的流程必须在本库中存在(悬空引用);
///  - 不得引用流程自身;
///  - 调用链不得成环(A → B → A;**图内**环检测挡不住跨流程环);
///  - 嵌套深度 ≤ [`MAX_SUB_FLOW_DEPTH`];
///  - **被引用的**流程必须结构合法(启用步骤非空 / 有生成步 / 图无环 / 工具已注册),
///    即把它当作「启用态」复用 [`validate_flow`] 的同一套规则——被引用流程自身的
///    `enabled` 开关只决定它能否作为当前流程直接执行,不影响它能否被引用。
///
/// 遍历从**库内每个流程**出发,而不是只从本次编辑的流程:B 被编辑成引用 A 时,
/// 引用 B 的 C 也会跟着变深/成环,只查 B 会漏。库规模是「用户手写的几十个流程」,
/// 全量遍历的成本可忽略(保存是低频操作;执行期另有只查入口链的 [`walk_sub_flows_from`])。
pub fn validate_sub_flows(
    library: &AgentFlowLibrary,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    let by_id = flow_index(library);
    for flow in &library.flows {
        walk_sub_flows_from(&by_id, flow, registered_tools)?;
    }
    Ok(())
}

/// 流程展示名(命名为空时回退 id;错误文案与 plan 前缀都用它)
pub fn flow_label(cfg: &AgentFlowConfig) -> String {
    if cfg.name.trim().is_empty() {
        cfg.id.clone()
    } else {
        cfg.name.trim().to_string()
    }
}

/// 流程中**启用步骤**挂载的子流程引用:(步骤展示名, 子流程 id)。
/// 停用步骤不参与执行,故不参与引用链校验(与 `validate_flow` 只看启用步骤同口径)。
fn sub_flow_edges(cfg: &AgentFlowConfig) -> Vec<(String, String)> {
    cfg.steps
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| s.sub_flow_ref().map(|id| (s.name.clone(), id.to_string())))
        .collect()
}

/// 同上,只要 id(快照闭包收集用;与 `sub_flow_edges` 同一条「什么算被引用」的判定)
pub(super) fn sub_flow_ids(cfg: &AgentFlowConfig) -> Vec<String> {
    sub_flow_edges(cfg).into_iter().map(|(_, id)| id).collect()
}

/// 流程库的 id → 流程索引(引用解析共用;库内 id 唯一由 `set` 的写入路径保证)。
pub(super) fn flow_index(library: &AgentFlowLibrary) -> BTreeMap<&str, &AgentFlowConfig> {
    library.flows.iter().map(|f| (f.id.as_str(), f)).collect()
}

/// 从**单个流程**起走一遍引用链(规则见 [`validate_sub_flows`] 的清单)。
/// `by_id` 只用于解析被引用方,链首流程自身无须在库内(set 的候选库场景反之亦然)。
pub(super) fn walk_sub_flows_from(
    by_id: &BTreeMap<&str, &AgentFlowConfig>,
    flow: &AgentFlowConfig,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    // 链首是该流程自身;确实没挂子流程的流程无活可干
    if sub_flow_edges(flow).is_empty() {
        return Ok(());
    }
    let mut chain: Vec<String> = vec![flow.id.clone()];
    walk_sub_flows(by_id, flow, &mut chain, registered_tools)
}

/// 引用链深度优先遍历(见 [`validate_sub_flows`] 的规则清单)。
/// `chain` 是自链首起的**流程 id 链(含当前流程)**,用于环检测与深度判定。
fn walk_sub_flows(
    by_id: &BTreeMap<&str, &AgentFlowConfig>,
    flow: &AgentFlowConfig,
    chain: &mut Vec<String>,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    let label_of = |id: &str| -> String {
        by_id
            .get(id)
            .map(|f| flow_label(f))
            .unwrap_or_else(|| id.to_string())
    };
    for (step_label, sub_id) in sub_flow_edges(flow) {
        if sub_id == flow.id {
            return Err(format!(
                "流程「{}」的步骤「{}」不能引用流程自身",
                flow_label(flow),
                step_label
            ));
        }
        if let Some(pos) = chain.iter().position(|id| *id == sub_id) {
            let mut cycle: Vec<String> = chain[pos..].iter().map(|id| label_of(id)).collect();
            cycle.push(label_of(&sub_id));
            return Err(format!("子流程调用链存在环:{}", cycle.join(" → ")));
        }
        let Some(target) = by_id.get(sub_id.as_str()) else {
            return Err(format!(
                "流程「{}」的步骤「{}」引用的子流程不存在:{}",
                flow_label(flow),
                step_label,
                sub_id
            ));
        };
        if chain.len() > MAX_SUB_FLOW_DEPTH {
            let mut path: Vec<String> = chain.iter().map(|id| label_of(id)).collect();
            path.push(flow_label(target));
            return Err(format!(
                "子流程嵌套超过 {} 层:{}",
                MAX_SUB_FLOW_DEPTH,
                path.join(" → ")
            ));
        }
        // 被引用流程按「启用态」复用同一套结构规则(见函数文档)
        let mut as_enabled = (*target).clone();
        as_enabled.enabled = true;
        validate_flow(&as_enabled, registered_tools).map_err(|e| {
            format!(
                "流程「{}」引用的子流程「{}」无效:{}",
                flow_label(flow),
                flow_label(target),
                e
            )
        })?;
        chain.push(sub_id.clone());
        walk_sub_flows(by_id, target, chain, registered_tools)?;
        chain.pop();
    }
    Ok(())
}

/// 聊天侧线性化:把挂载了子流程的节点**就地展开**成被引用流程的启用步骤(递归),
/// 按各级各自的拓扑序拼成一张扁平列表。
///
/// 为什么是「展开」而不是「让聊天引擎跑嵌套」:聊天侧本来就**按拓扑序串行、步骤之间
/// 不传递产出**(见 `agents/planner.rs::make_custom_plan` 注释),在该位置依次跑子图节点
/// 与「先跑子图、产出交给下游」在聊天里等价;展开还能让 plan 面板逐节点显示进度,
/// 且无须给引擎引入流程库依赖。
///
/// 展开口径:
///  - 产出步骤的 `inputs` **一律清空**——扁平列表已按拓扑序排好,再保留 id 依赖会与
///    拼接后的图不一致(外层节点的 id 在展开后已不存在,留着就是悬空上游);
///  - 展开步骤的**名字**加 `【子流程「X」】` 前缀,让 plan 里能看出边界;
///  - 展开步骤的 **id 按挂载位置重写**(`前缀 + 原 id`,见 `expand_ordered`):同一子流程
///    被两处挂载、或父子流程撞 id(「复制流程」原样保留 step id)时,扁平列表会出现重复 id,
///    而聊天侧随后还要再跑一遍 `make_custom_plan` → `resolve_graph`,那里对重复 id 直接拒绝
///    (`effective_inputs`,文案「步骤 id 重复」)——于是**同一份流程任务侧能跑、聊天侧 400**。
///    顶层前缀为空串,故顶层步骤 id 逐字节不变;
///  - `entry_flow_id` 是本次展开的入口流程 id,用于自引用/环守卫(与保存期同一套判定,
///    此处是防御外部手改配置的兜底)。
pub fn expand_sub_flows(
    library: &AgentFlowLibrary,
    entry_flow_id: &str,
    steps: &[PlanStep],
) -> Result<Vec<PlanStep>, String> {
    let by_id = flow_index(library);
    let mut chain: Vec<String> = vec![entry_flow_id.to_string()];
    let out = expand_ordered(&by_id, steps, &mut chain, "")?;
    // 兜底自检:展平后 `inputs` 已清空(线性兼容),该调用只剩「id 非空且唯一」的判定意义。
    // 前缀派生已保证各挂载点不撞 id,这里防的是「用户把步骤 id 写成与派生前缀同形」这类
    // 极端配置:给出点名 id 的中文错误,而不是让下游报「步骤 id 重复」。
    if let Err(e) = resolve_graph(&out) {
        let dups = duplicated_step_ids(&out);
        if !dups.is_empty() {
            return Err(format!(
                "子流程展开后步骤 id 冲突:重复的步骤 id「{}」——请检查子流程的步骤 id 是否与其它流程重复(原始错误:{e})",
                dups.join("、")
            ));
        }
        return Err(e);
    }
    Ok(out)
}

/// 扁平列表里重复出现的步骤 id(升序;无重复返回空)
fn duplicated_step_ids(steps: &[PlanStep]) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut dups: BTreeSet<String> = BTreeSet::new();
    for s in steps {
        if !seen.insert(s.id.as_str()) {
            dups.insert(s.id.clone());
        }
    }
    dups.into_iter().collect()
}

/// `expand_sub_flows` 的递归体:解析本层拓扑序 → 逐节点产出(子流程节点就地展开)。
///
/// `prefix` 是本层的 id 前缀:入口流程为空串;每进入一层子图,按**挂载位置**追加
/// `s{下标}/`(下标 = 挂载节点在本层数组中的下标)。「同一子流程挂两处」因此得到两个不同
/// 前缀,id 天然唯一。重写是安全的:`PlanStep.id` 在聊天引擎侧不被消费——它只服务于图原语
/// 自身的连接与校验(见 `effective_inputs`),展开后 `inputs` 已清空,id 不再参与执行。
fn expand_ordered(
    by_id: &BTreeMap<&str, &AgentFlowConfig>,
    steps: &[PlanStep],
    chain: &mut Vec<String>,
    prefix: &str,
) -> Result<Vec<PlanStep>, String> {
    let graph = resolve_graph(steps)?;
    let mut out: Vec<PlanStep> = Vec::with_capacity(steps.len());
    for &i in &graph.order {
        let step = &steps[i];
        let Some(sub_id) = step.sub_flow_ref() else {
            out.push(flattened_step(step, prefix));
            continue;
        };
        let label_of = |id: &str| -> String {
            by_id
                .get(id)
                .map(|f| flow_label(f))
                .unwrap_or_else(|| id.to_string())
        };
        let Some(target) = by_id.get(sub_id) else {
            return Err(format!(
                "步骤「{}」引用的子流程不存在:{}",
                step.name, sub_id
            ));
        };
        if let Some(pos) = chain.iter().position(|id| id == sub_id) {
            let mut cycle: Vec<String> = chain[pos..].iter().map(|id| label_of(id)).collect();
            cycle.push(label_of(sub_id));
            return Err(format!("子流程调用链存在环:{}", cycle.join(" → ")));
        }
        if chain.len() > MAX_SUB_FLOW_DEPTH {
            let mut path: Vec<String> = chain.iter().map(|id| label_of(id)).collect();
            path.push(flow_label(target));
            return Err(format!(
                "子流程嵌套超过 {} 层:{}",
                MAX_SUB_FLOW_DEPTH,
                path.join(" → ")
            ));
        }
        let active: Vec<PlanStep> = target.steps.iter().filter(|s| s.enabled).cloned().collect();
        if active.is_empty() {
            return Err(format!(
                "步骤「{}」引用的子流程「{}」没有启用的步骤",
                step.name,
                flow_label(target)
            ));
        }
        chain.push(sub_id.to_string());
        // 子层前缀按挂载位置派生:`s{本层下标}/`(下标取自 graph.order,即数组下标)
        let child = format!("{}s{}/", prefix, i);
        let nested = expand_ordered(by_id, &active, chain, &child)?;
        chain.pop();
        let label = flow_label(target);
        for mut s in nested {
            s.name = format!("【子流程「{}」】{}", label, s.name);
            out.push(s);
        }
    }
    Ok(out)
}

/// 展开产物的扁平化:清空 `inputs`(见 `expand_sub_flows` 的展开口径),并把 id 打上本层前缀。
/// 顶层前缀为空串 → 顶层步骤 id 与配置里的原值逐字节相同。
fn flattened_step(step: &PlanStep, prefix: &str) -> PlanStep {
    PlanStep {
        id: format!("{}{}", prefix, step.id),
        inputs: Vec::new(),
        ..step.clone()
    }
}
