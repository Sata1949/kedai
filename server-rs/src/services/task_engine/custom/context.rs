// custom 上下文辅助:调用路径拼接、可调用流程引用与成果选拔(纯函数)。
// 由原 `custom.rs` 整段搬来(C-2D,纯搬运)。
use crate::models::types::PlanStep;
use crate::services::agent_flow_service::AgentFlowConfig;

/// 本层节点调用的记账 phase:入口流程 = `step`,子图 = `subflow.<父节点下标链>`。
/// `path` 为空即入口流程。
pub(super) fn phase_for_path(path: &str) -> String {
    if path.is_empty() {
        "step".to_string()
    } else {
        format!("subflow.{}", path)
    }
}

/// 进入子图后的路径:入口层(空路径)记父节点下标,深层在其后追加。
pub(super) fn child_path(path: &str, index: usize) -> String {
    if path.is_empty() {
        index.to_string()
    } else {
        format!("{}.{}", path, index)
    }
}

/// 可调用集 → 流程引用(二维批次 7b;生成 `run_flow` 描述与「本节点是否要释放工具」用)。
/// 顺序恒为可调用集顺序(= 名单声明顺序);解析不到的 id 跳过——可调用集本就是从**同一份**
/// 冻结闭包里筛出来的,这里的跳过只是防御,不是常规路径。
pub(super) fn released_flows<'a>(
    callable: &[String],
    flows: &'a [AgentFlowConfig],
) -> Vec<&'a AgentFlowConfig> {
    callable
        .iter()
        .filter_map(|id| flows.iter().find(|f| &f.id == id))
        .collect()
}

/// 成果选拔(入口流程与子图共用,口径自二维批次 1 起未变):成果节点产出优先;
/// 为空则按**数组下标降序**回退到上一个非空生成产出——线性流程语义就是「末个有效
/// 生成步」,且该兜底与执行顺序无关(并行下成果节点失败时不会退化成「谁先跑完算谁」)。
pub(super) fn select_draft(
    steps: &[PlanStep],
    output_idx: Option<usize>,
    outputs: &[String],
) -> String {
    output_idx
        .map(|i| outputs[i].clone())
        .filter(|text| !text.is_empty())
        .or_else(|| {
            (0..steps.len())
                .rev()
                .find(|&i| {
                    steps[i].action == "direct"
                        && steps[i].generates == Some(true)
                        && !outputs[i].is_empty()
                })
                .map(|i| outputs[i].clone())
        })
        .unwrap_or_default()
}
