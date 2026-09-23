// 对比模式的**动态流程调用**守卫与文案(二维批次 7b)。
//
// 为什么单独成模块:这里的判定全是「输入确定」的纯逻辑(名单怎么算、什么算重名、
// 环与深度怎么拦、工具描述怎么写),而 custom.rs 的端到端用例要跑起整个 app 才动得了。
// 把判定收在这里用单测逐条锁死,端到端用例只负责验证「闸真的接上了」——
// 与 `prompt_kit` / `resolve_graph` 同款纪律:同一个判定只有一处实现。
//
// 三道闸(缺一条就是烧钱事故,计划 §四 7b 改动点 5):
//   ① 调用链环检测(A 调 B、B 调 A:图内环检测挡不住跨流程环);
//   ② 动态嵌套深度([`MAX_FLOW_CALL_DEPTH`],与静态子图深度**独立**计数);
//   ③ 每任务调用预算([`MAX_FLOW_CALLS_PER_TASK`],拦「串行地反复调用同一套流程」)。
// 三者都在**任何模型调用之前**判定:被拒的调用不产生任何 `task_llm_calls` 行(不烧钱)。
use crate::services::agent_flow_service::{
    flow_label, AgentFlowConfig, MAX_FLOW_CALLS_PER_TASK, MAX_FLOW_CALL_DEPTH,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 一次任务执行内共享的动态调用状态(入口层构造,各层原样下传)。
///
/// 进程内、按任务:预算随 run 起止,重跑重新计数(与 `phase` 的 `step` 同名跨跑重复
/// 同款语义——DB 行是按次累加的,不在本状态里管)。
pub(crate) struct FlowCallState {
    /// 可调用流程 id(名单 ∩ 冻结闭包 − 根流程;**顺序恒为名单声明顺序**,
    /// 于是工具描述里列举的顺序与用户配置一致,同一份名单两次运行给出同样文案)
    callable: Vec<String>,
    /// 已发起的动态调用次数(兼作 `d<序号>` 的序号来源)
    used: AtomicUsize,
    /// 被调流程内是否出现过失败/空产出节点(供根层把任务置 `partial`)
    degraded: AtomicBool,
}

impl FlowCallState {
    pub(crate) fn new(callable: Vec<String>) -> Self {
        FlowCallState {
            callable,
            used: AtomicUsize::new(0),
            degraded: AtomicBool::new(false),
        }
    }

    pub(crate) fn callable(&self) -> &[String] {
        &self.callable
    }

    /// 记一次动态调用并返回**序号**(第 n 次 → 路径段 `d{n}`);超预算即拒绝。
    ///
    /// 序号与预算是同一个计数器:先取号再判界,超过上限的调用**消耗掉一个号**但不产生
    /// 任何痕迹(路径永不落地),这是有意的——序号只需要在**已发生的调用**之间唯一。
    pub(crate) fn charge(&self) -> Result<usize, String> {
        let n = self.used.fetch_add(1, Ordering::SeqCst) + 1;
        if n > MAX_FLOW_CALLS_PER_TASK {
            return Err(format!(
                "本次任务已调用流程 {} 次,达到上限 {} 次(每次调用都会完整跑一套流程,预算用于防止反复调用烧 token);请拆分为多个任务,或在流程里改用静态子图",
                MAX_FLOW_CALLS_PER_TASK, MAX_FLOW_CALLS_PER_TASK
            ));
        }
        Ok(n)
    }

    /// 标记「被调流程内有节点失败/空产出」(不污染回灌文本,只影响任务终态)
    pub(crate) fn mark_degraded(&self) {
        self.degraded.store(true, Ordering::SeqCst);
    }

    pub(crate) fn degraded(&self) -> bool {
        self.degraded.load(Ordering::SeqCst)
    }
}

/// 可调用集 = 名单 ∩ 冻结闭包 − 根流程(名单顺序,去重,去空白项)。
///
/// 三处口径各自成立的理由:
///  - **∩ 冻结闭包**:与静态子图同一条纪律——「快照外的一律取不到」。名单里的 id 若不在
///    冻结域内(未绑定任务运行期删过流程、手改 DB 等),它不该因为名字还在名单上就可用;
///  - **− 根流程**:根流程正在执行,被调用必然撞调用链环守卫;早一步扣掉,工具描述里就
///    不会列举一个注定被拒的选项(文案与放行判定**同源**,不出现「描述了却调不动」);
///  - 顺序取名单顺序:用户配置顺序即模型看到的顺序。
pub(crate) fn callable_ids(
    list: &[String],
    closure: &[AgentFlowConfig],
    root_id: &str,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in list {
        let id = id.trim();
        if id.is_empty() || id == root_id || out.iter().any(|x| x == id) {
            continue;
        }
        if closure.iter().any(|f| f.id == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// 按「流程名或 id」解析被调流程,**解析范围限于可调用集**。
///
/// 判据顺序:精确 id → 精确名(唯一)。重名时不猜:列出候选 id 让模型改用 id——
/// 名字是用户运行期可改的展示字段,拿它当主键必然踩到「两份流程同名」。
pub(crate) fn resolve_flow_ref<'a>(
    closure: &'a [AgentFlowConfig],
    callable: &[String],
    key: &str,
) -> Result<&'a AgentFlowConfig, String> {
    let key = key.trim();
    let in_callable = |cfg: &AgentFlowConfig| callable.iter().any(|id| id == &cfg.id);
    let available: Vec<&AgentFlowConfig> = closure.iter().filter(|f| in_callable(f)).collect();
    // ① 精确 id
    if let Some(cfg) = available.iter().find(|f| f.id == key) {
        return Ok(cfg);
    }
    // ② 精确名(唯一才可用)
    let by_name: Vec<&&AgentFlowConfig> =
        available.iter().filter(|f| f.name.trim() == key).collect();
    match by_name.as_slice() {
        [cfg] => Ok(cfg),
        [] => Err(format!(
            "流程「{}」不在本任务的可调用名单内(可调用:{});名单外流程不会被调用,请改用名单内流程或不要调用",
            key,
            if available.is_empty() {
                "(空)".to_string()
            } else {
                available
                    .iter()
                    .map(|f| format!("「{}」", flow_label(f)))
                    .collect::<Vec<_>>()
                    .join("、")
            }
        )),
        many => Err(format!(
            "流程名「{}」对应多个流程,请改用流程 id:{}",
            key,
            many.iter()
                .map(|f| format!("「{}」", f.id))
                .collect::<Vec<_>>()
                .join("、")
        )),
    }
}

/// 动态调用守卫:**环**与**动态嵌套深度**。任一不过即拒绝,且拒绝发生在任何模型调用之前。
///
/// 环先判于深度:自调用(B 调 B)同时满足两条,环的文案更准确(「在当前调用链上已出现过」)。
/// `chain` 是**流程 id 调用链**(入口流程起头,静态子图与动态调用共用同一条链——
/// 于是「静态挂载回入口」与「动态调用回入口」由同一处拦下,不存在两条判断分叉)。
pub(crate) fn check_call_guards(
    chain: &[String],
    call_depth: usize,
    cfg: &AgentFlowConfig,
) -> Result<(), String> {
    if chain.iter().any(|id| id == &cfg.id) {
        return Err(format!(
            "流程调用链存在环:「{}」在当前调用链上已出现过,拒绝再次进入(调用链:{})",
            flow_label(cfg),
            chain
                .iter()
                .map(|id| format!("「{id}」"))
                .collect::<Vec<_>>()
                .join(" → ")
        ));
    }
    if call_depth + 1 > MAX_FLOW_CALL_DEPTH {
        return Err(format!(
            "动态调用嵌套超过 {} 层:流程「{}」再被调用会超出上限(每层都会把 token 消耗成倍放大)",
            MAX_FLOW_CALL_DEPTH,
            flow_label(cfg)
        ));
    }
    Ok(())
}

/// 动态调用层在调用链中的**路径段**:`d<序号>`;嵌套时追加在父路径之后(`d1.d2`)。
///
/// 与静态子图的 `child_path` 同型(父路径空则不带前导分隔符),于是记账 phase 只有两条
/// 前缀:`step`(入口)/ `subflow.<路径>`(静态子图)/ `call.<路径>`(动态调用),
/// 且**都不含冒号**(前端流式缓冲 key 按首个冒号切分)。
pub(crate) fn dynamic_path(parent: &str, n: usize) -> String {
    if parent.is_empty() {
        format!("d{n}")
    } else {
        format!("{parent}.d{n}")
    }
}

/// 逐节点下发的 `run_flow` 描述(即「释放名单」这一步的产物)。
///
/// 重名流程补上 id:描述里列举的名字必须**可解析**——否则模型照着描述调用会撞上
/// 「对应多个流程」的报错,白烧一轮。
pub(crate) fn run_flow_description(flows: &[&AgentFlowConfig]) -> String {
    let dup_names: Vec<&str> = flows
        .iter()
        .map(|f| f.name.trim())
        .filter(|n| flows.iter().filter(|f| f.name.trim() == *n).count() > 1)
        .collect();
    let mut out = String::from(
        "调用一套预置流程,取回它的完整成果(本任务允许的流程列在下方)。被调流程会独立跑完自己的编排,因此每次调用都较慢、也较贵。",
    );
    out.push_str("\n\n可用流程:");
    for cfg in flows {
        let label = flow_label(cfg);
        let suffix = if dup_names.contains(&cfg.name.trim()) {
            format!("(流程 id:{})", cfg.id)
        } else {
            String::new()
        };
        let desc = cfg
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(|d| format!(":{}", truncate(d, 80)))
            .unwrap_or_default();
        out.push_str(&format!("\n- 「{label}」{suffix}{desc}"));
    }
    out.push_str(&format!(
        "\n\n调用方式:flow = 上列流程名(重名时用流程 id);input = 传给它的输入文本,省略时用任务目标。本任务最多可调用 {} 次。",
        MAX_FLOW_CALLS_PER_TASK
    ));
    out
}

/// 按字符截断(中文安全,不切 char 边界)
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::types::PlanStep;

    fn flow(id: &str, name: &str) -> AgentFlowConfig {
        AgentFlowConfig {
            id: id.into(),
            name: name.into(),
            enabled: true,
            steps: vec![PlanStep {
                id: "s1".into(),
                name: "步骤".into(),
                enabled: true,
                goal: "g".into(),
                action: "direct".into(),
                generates: Some(true),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn ids(cfg: &[AgentFlowConfig]) -> Vec<String> {
        cfg.iter().map(|f| f.id.clone()).collect()
    }

    /// 可调用集 = 名单 ∩ 闭包 − 根流程,顺序取名单顺序(描述文案与放行判定同源的前提)
    #[test]
    fn callable_ids_intersects_closure_and_drops_root() {
        let closure = vec![flow("a", "根"), flow("b", "乙"), flow("c", "丙")];
        // 名单里有:闭包外的 x(剔除)、根 a(剔除)、重复的 b(去重)
        let list = vec![
            "b".to_string(),
            "x".to_string(),
            "a".to_string(),
            " b ".to_string(),
            "c".to_string(),
        ];
        assert_eq!(callable_ids(&list, &closure, "a"), ["b", "c"]);
        // 全空 / 只剩根:可调用集为空(调用方据此不下发工具)
        assert!(callable_ids(&[], &closure, "a").is_empty());
        assert!(callable_ids(&["a".to_string()], &closure, "a").is_empty());
    }

    /// 解析:id 与唯一名都认;重名列候选 id;名单外的流程拒绝并提示可调用集
    #[test]
    fn resolve_by_id_or_unique_name_with_ambiguity_guard() {
        let closure = vec![flow("f-1", "甲"), flow("f-2", "乙")];
        let callable = ids(&closure);
        assert_eq!(
            resolve_flow_ref(&closure, &callable, "f-1").unwrap().name,
            "甲"
        );
        assert_eq!(
            resolve_flow_ref(&closure, &callable, "乙").unwrap().id,
            "f-2"
        );
        // 名单外:不在可调用集里的流程一律拒绝(闭包里有一份但不在名单里)
        let only_a = vec!["f-1".to_string()];
        let err = resolve_flow_ref(&closure, &only_a, "乙").unwrap_err();
        assert!(err.contains("不在本任务的可调用名单内"), "{err}");
        assert!(err.contains("「甲」"), "错误文案应列出可调用集: {err}");
        // 重名:必须报错并给出候选 id,不得猜一个
        let dup = vec![flow("d-1", "同名"), flow("d-2", "同名")];
        let dup_callable = ids(&dup);
        let err = resolve_flow_ref(&dup, &dup_callable, "同名").unwrap_err();
        assert!(err.contains("对应多个流程"), "{err}");
        assert!(err.contains("d-1") && err.contains("d-2"), "{err}");
    }

    /// 环先于深度判:自调用同时满足两条时给的是环的文案(更准确的诊断)
    #[test]
    fn cycle_guard_precedes_depth_guard() {
        let cfg = flow("b", "乙");
        let err =
            check_call_guards(&["a".into(), "b".into()], MAX_FLOW_CALL_DEPTH, &cfg).unwrap_err();
        assert!(err.contains("调用链存在环"), "{err}");
        assert!(err.contains("「a」 → 「b」"), "应打印调用链: {err}");
    }

    /// 深度:第 1、2 层放行,第 3 层拒绝(MAX_FLOW_CALL_DEPTH = 2)
    #[test]
    fn depth_guard_allows_two_levels_only() {
        let cfg = flow("c", "丙");
        assert!(check_call_guards(&["a".into()], 0, &cfg).is_ok(), "第 1 层");
        assert!(
            check_call_guards(&["a".into(), "b".into()], 1, &cfg).is_ok(),
            "第 2 层"
        );
        let err = check_call_guards(&["a".into(), "b".into()], 2, &cfg).unwrap_err();
        assert!(err.contains("超过 2 层"), "{err}");
    }

    /// 预算:第 8 次照常放行,第 9 次起拒绝且**不消耗**已通过者的序号
    #[test]
    fn budget_rejects_after_limit() {
        let st = FlowCallState::new(vec!["b".into()]);
        for n in 1..=MAX_FLOW_CALLS_PER_TASK {
            assert_eq!(st.charge().unwrap(), n, "第 {n} 次应放行并给出序号");
        }
        let err = st.charge().unwrap_err();
        assert!(err.contains("达到上限"), "{err}");
        assert!(st.charge().is_err(), "超限后继续拒绝");
        // 降级标记:一旦置位不再复位(供根层把任务置 partial)
        assert!(!st.degraded());
        st.mark_degraded();
        assert!(st.degraded());
    }

    /// 路径命名:入口层 `d1`;嵌套为 `父路径.d<序号>`(与静态子图的 child_path 同型)
    #[test]
    fn dynamic_path_appends_to_parent() {
        assert_eq!(dynamic_path("", 1), "d1");
        assert_eq!(dynamic_path("d1", 2), "d1.d2");
        // 静态子图内发起的动态调用:父路径是子图路径(两条前缀各管一段)
        assert_eq!(dynamic_path("3", 1), "3.d1");
    }

    /// 描述必须列全可调用流程、带上用途说明与预算;重名时补 id 保证可解析
    #[test]
    fn description_lists_callable_flows_and_disambiguates_names() {
        let mut a = flow("f-1", "甲流程");
        a.description = Some("写周报".into());
        let b = flow("f-2", "乙流程");
        let dup1 = flow("d-1", "同名");
        let dup2 = flow("d-2", "同名");
        let desc = run_flow_description(&[&a, &b, &dup1, &dup2]);
        assert!(desc.contains("「甲流程」:写周报"), "{desc}");
        assert!(desc.contains("「乙流程」"), "{desc}");
        assert!(
            desc.contains("(流程 id:d-1)") && desc.contains("(流程 id:d-2)"),
            "{desc}"
        );
        assert!(
            desc.contains(&MAX_FLOW_CALLS_PER_TASK.to_string()),
            "应写明预算: {desc}"
        );
        // 无说明的流程不落空档
        assert!(!desc.contains("「乙流程」:"), "无说明时不应带冒号: {desc}");
    }

    /// 说明过长按字符截断(中文不切半)
    #[test]
    fn description_truncates_long_text() {
        let mut a = flow("f-1", "甲");
        a.description = Some("说明".repeat(100));
        let desc = run_flow_description(&[&a]);
        assert!(desc.contains('…'), "{desc}");
        assert!(desc.chars().count() < 400, "描述不应被长文本撑爆: {desc}");
    }
}
