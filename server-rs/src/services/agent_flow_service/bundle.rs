//! 流程搬运包:保留 id 表、指纹、快照校验与导入/导出所需的纯逻辑。

// 子模块内统一用 super::* 拿到 mod.rs 的公共词汇(类型/常量)与 re-export 的自由函数。
use super::*;
use std::collections::BTreeSet;

/// 路由**保留段**:这些字符串是 `/api/agent-flows/{export,import,select}` 的静态路径段,
/// 而 axum 的静态段优先于 `/{id}`——所以 id 恰为它们的流程**删不掉**
/// (`DELETE /api/agent-flows/export` 命中静态路由后按方法不匹配回 405,不会回落到 `/{id}`)。
/// 导入是唯一能引入任意 id 的入口(旧路径恒分配 UUID),故这类 id 在导入侧一律重映射,
/// 不让库里出现「建得出来、删不掉」的流程。
const RESERVED_FLOW_IDS: &[&str] = &["export", "import", "select"];

/// 是否为保留段 id(见 [`RESERVED_FLOW_IDS`])
pub(super) fn is_reserved_flow_id(id: &str) -> bool {
    RESERVED_FLOW_IDS.contains(&id)
}

/// 一份流程配置的**内容指纹**(重复导入幂等的判定基准)。
///
/// 用同一个结构的 serde 序列化:`skip_serializing_if` 决定缺省键是否省略,故两侧都经一次
/// parse→serialize 后即可直接比字符串(手写 JSON 的空白差异在 parse 时消失)。
/// **不含 `id`**:内容相同但 id 不同的两份流程,对用户而言同一份,不该重复进库。
pub(super) fn flow_fingerprint(cfg: &AgentFlowConfig) -> Result<String, String> {
    let mut key = cfg.clone();
    key.id = String::new();
    serde_json::to_string(&key).map_err(|e| format!("流程序列化失败: {e}"))
}

/// 校验**冻结快照自身**(绑定任务的执行前校验):结构 + 闭包内的引用链。
///
/// 与 [`AgentFlowService::validate`] 的关键差别是**只看快照、不看流程库**——被冻结的
/// 任务不该因为库里子流程被改/被删而失效(那正是二维批次 5a 要收掉的行为漂移)。
/// 校验规则本身仍是同一套(`validate_flow` + `validate_sub_flows`),不复制判定。
pub fn validate_snapshot(
    snapshot: &FlowSnapshot,
    registered_tools: &BTreeSet<String>,
) -> Result<(), String> {
    let root = snapshot
        .root()
        .ok_or_else(|| format!("流程快照缺少入口流程:{}", snapshot.root_id))?;
    validate_flow(root, registered_tools)?;
    validate_sub_flows(&snapshot.as_library(), registered_tools)
}
