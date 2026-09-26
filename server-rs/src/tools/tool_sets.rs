// 工具集合常量与过滤器(单一出处)。
//
// 背景:同一份「只读安全集」原先在规划器侦察、子智能体、反思三处各硬编码一遍,
// 新增只读工具需多处同步,极易漂移。这里集中定义,调用点只引用常量。
// 另集中「正文元工具」排除逻辑:get_state/apply_patch 只服务多步变量驱动器与显式
// 白名单,不应出现在正文默认工具列表(否则诱导模型在正文轮误用变量补丁通道)。
use crate::models::types::ToolDefinition;

/// 正文元工具:多步变量驱动器专用,不进正文默认列表。
/// 聊天 agent 模式与任务全量模式共用此排除口径,避免同一模型在两处看到不同工具。
///
/// `run_flow`(二维批次 7b 对比模式)加入本清单的理由与上两者同型:它是**按任务名单
/// 释放**的工具——名单是运行期数据,全局可见毫无意义(聊天与 solo/multi/team 都不该
/// 看到它);真正用得着它的那条路径由 `task_engine/custom.rs` 显式把定义塞进
/// `params.tools` 并进闸门名单,不依赖本清单。
pub const META_TOOLS: &[&str] = &["get_state", "apply_patch", "run_flow"];

/// 规划器只读侦察白名单:规划阶段允许模型先收集信息再产出计划 JSON;
/// 严禁写操作(违背 plan/legacy「只规划不执行」零副作用纪律)与编排类(会把规划变成执行)。
pub const READONLY_SCOUT: &[&str] = &["read", "search", "memory_read", "calculator"];

/// 子智能体工具白名单:读/搜索类安全工具;写类与编排类一律剔除
/// (子 agent 不得再派子 agent;嵌套由白名单与深度守卫双重排除)。
pub const SUBAGENT: &[&str] = &[
    "read",
    "search",
    "todo",
    "sleep",
    "calculator",
    "memory_read",
];

/// 反思阶段工具白名单:仅禁词替换与定点修订;dirty 文本修正不引入检索类工具。
pub const REFLECT: &[&str] = &["censor_text", "revise_passage"];

/// 工作区文件工具族(编码通道批次)的名字清单(**单一出处**:注册侧、风险级、任务工具策略
/// 的工作区闸门与「何时调用」指南都以它为准)。
///
/// **有意不进 `READONLY_SCOUT` / `SUBAGENT` / `REFLECT`**:那三份白名单是「既有能力面」的
/// 冻结清单,本族的可见性由**任务是否绑定工作区**决定(见 `task_engine/tool_policy.rs`)。
/// 纳入只读白名单会让规划侦察轮与子 agent(都跑在聊天/任务上下文里、未必有工作区)
/// 看到必然报错的工具,还会顺带扩大子 agent 的写入面——本批不做这两件事。
/// 故新增只读能力(如 fs_read)不自动进侦察轮,要放开请单独评估并改这里的注释。
pub const WORKSPACE_TOOLS: &[&str] = &["fs_read", "fs_write", "fs_edit", "fs_glob", "fs_grep"];

/// 剔除正文元工具(get_state/apply_patch),保留其余工具与原顺序。
/// 顺序稳定性是前缀缓存的前提,故不做排序。
pub fn exclude_meta(defs: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    defs.into_iter()
        .filter(|d| !META_TOOLS.contains(&d.name.as_str()))
        .collect()
}

/// 剔除工作区文件工具族。聊天路径(角色扮演、custom 流程步骤)没有工作区上下文,
/// 本族在那里必然报「未绑定工作区」——不下发即不给模型制造无效调用(口径同 exclude_meta)。
pub fn exclude_workspace(defs: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    defs.into_iter()
        .filter(|d| !WORKSPACE_TOOLS.contains(&d.name.as_str()))
        .collect()
}

/// 按名称白名单过滤工具定义(保留入参顺序;未注册的名称自然被忽略)。
/// 语义为「交集」:调用方传入的白名单 ∩ 实际注册工具。
pub fn filter_by_names(defs: Vec<ToolDefinition>, names: &[String]) -> Vec<ToolDefinition> {
    defs.into_iter()
        .filter(|d| names.iter().any(|n| n == &d.name))
        .collect()
}

/// 按 &str 常量白名单过滤(defs 顺序保留)
pub fn filter_by_const(defs: Vec<ToolDefinition>, names: &[&str]) -> Vec<ToolDefinition> {
    defs.into_iter()
        .filter(|d| names.contains(&d.name.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: name.into(),
            parameters: json!({ "type": "object" }),
        }
    }

    #[test]
    fn exclude_meta_removes_only_meta_tools() {
        let defs = vec![
            def("read"),
            def("get_state"),
            def("write"),
            def("apply_patch"),
        ];
        let out = exclude_meta(defs);
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read", "write"]);
    }

    #[test]
    fn filter_by_names_is_intersection_and_keeps_order() {
        let defs = vec![def("write"), def("read"), def("search")];
        let out = filter_by_names(
            defs,
            &[
                "read".to_string(),
                "search".to_string(),
                "missing".to_string(),
            ],
        );
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read", "search"]);
    }

    #[test]
    fn filter_by_const_works() {
        let defs = vec![def("read"), def("write"), def("search"), def("memory_read")];
        let out = filter_by_const(defs, READONLY_SCOUT);
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read", "search", "memory_read"]);
    }

    /// 常量内容与改造前的字面量逐一比对,防止无意改动导致能力漂移
    /// (META_TOOLS 于二维批次 7b 追加 `run_flow`,属**有意**扩列,理由见常量文档)
    #[test]
    fn constants_match_legacy_literals() {
        assert_eq!(META_TOOLS, &["get_state", "apply_patch", "run_flow"]);
        assert_eq!(
            READONLY_SCOUT,
            &["read", "search", "memory_read", "calculator"]
        );
        assert_eq!(
            SUBAGENT,
            &[
                "read",
                "search",
                "todo",
                "sleep",
                "calculator",
                "memory_read"
            ]
        );
        assert_eq!(REFLECT, &["censor_text", "revise_passage"]);
        assert_eq!(
            WORKSPACE_TOOLS,
            &["fs_read", "fs_write", "fs_edit", "fs_glob", "fs_grep"]
        );
    }

    /// 工作区工具族必须被 exclude_workspace 完整剔除,且不误伤同名前缀的其它工具
    #[test]
    fn exclude_workspace_removes_only_the_family() {
        let defs = vec![
            def("read"),
            def("fs_read"),
            def("fs_write"),
            def("fs_glob"),
            def("fs_edit"),
            def("fs_grep"),
            // 名字含 fs_ 但不在族内(锁定「按名字精确匹配」)
            def("fs_read2"),
        ];
        let names: Vec<String> = exclude_workspace(defs)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, vec!["read", "fs_read2"]);
    }
}
