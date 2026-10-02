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
/// **有意不进 `SUBAGENT` / `REFLECT`**:那两份白名单是「既有能力面」的冻结清单,本族的
/// 可见性由**任务是否绑定作用域**决定(见 `task_engine/tool_policy.rs`)。纳入子 agent /
/// 反思轮会让它们看到必然报错的工具(子 agent 未必有工作区),还会顺带扩大写入面。
///
/// 规划侦察轮(READONLY_SCOUT)自任务模式 D1 起**有条件**放开只读子集:
/// 见 `WORKSPACE_READONLY_TOOLS` 与 `scout_tools`——规划器看不到工作区就只能盲规划
/// (实测缺陷 D5:`read` 走角色扮演文件区语义,报「读取文件 package.json 失败」)。
///
/// 视觉工具族(2026-10-02 视觉能力包 D4)并入本族:三件同样以工作区文件为输入,聊天
/// 路径(`exclude_workspace`)与无作用域任务(`workspace_gate`)对它们一体剔除;
/// 另受 `tool_policy::vision_gate` 按连接能力位约束(见 `VISION_TOOLS`)。
pub const WORKSPACE_TOOLS: &[&str] = &[
    "fs_read",
    "fs_write",
    "fs_edit",
    "fs_glob",
    "fs_grep",
    // Q6(2026-10-01)第六员:结构化补丁;另受任务工具策略的**编码包闸门**约束
    // (关包时不下发,见 task_engine/tool_policy.rs::coding_pack_gate)
    "fs_patch",
    // 视觉三件(2026-10-02):view_image / zoom_image / image_diff
    "view_image",
    "zoom_image",
    "image_diff",
];

/// 视觉工具族(视觉能力包 D4;单一出处:注册名、风险级、工作区族、视觉闸门与提示词
/// 条件段都以它为准)。三件都只读工作区文件(图像输出重定向到 DATA_DIR/images),
/// 可见性 = 工作区族闸门 ∩ `vision_gate`(生效连接开启「视觉输入」能力位)。
pub const VISION_TOOLS: &[&str] = &["view_image", "zoom_image", "image_diff"];

/// 名称是否属「图像工具」:视觉三件 ∪ 截图(2026-10-02 修复批次——截图同样把图像
/// 交回模型,视觉验证纪律段的条件判据必须含它,否则截图路径静默无纪律段)。
pub fn is_image_tool(name: &str) -> bool {
    VISION_TOOLS.contains(&name) || name == crate::tools::screenshot::TOOL_NAME
}

/// defs 是否含任一图像工具(条件提示词段与相关判据共用)
pub fn has_image_tools(defs: &[ToolDefinition]) -> bool {
    defs.iter().any(|d| is_image_tool(&d.name))
}

/// 按「视觉与截图」总开关过滤截图工具(视觉能力包 D5):关(默认)时剔除。
/// 聊天路径(api/chat.rs)与任务路径(tool_policy::screenshot_gate)共用本判据,
/// 单点维护工具名(crate::tools::screenshot::TOOL_NAME)。
pub fn filter_screenshot(defs: Vec<ToolDefinition>, enabled: bool) -> Vec<ToolDefinition> {
    if enabled {
        return defs;
    }
    defs.into_iter()
        .filter(|d| d.name != crate::tools::screenshot::TOOL_NAME)
        .collect()
}

/// 工作区文件工具族的**只读子集**:`fs_read`/`fs_glob`/`fs_grep`。
///
/// 用途只有一个:任务绑定了作用域时并入规划侦察轮白名单(D5),让规划器能看见工作区里的
/// 真实文件再产出计划。写/改(fs_write/fs_edit)与 bash 不进侦察轮——规划阶段仍是
/// 「只规划不执行」的零副作用纪律(零副作用靠白名单保证,不靠模型自觉)。
pub const WORKSPACE_READONLY_TOOLS: &[&str] = &["fs_read", "fs_glob", "fs_grep"];

/// 工具 → 「工作状态锚点」所在的参数键名(2026-09-30 批次 3,HARNESS3-3)。
///
/// 用途:工具历史摘要化时**把这个键留在占位里**(消费点 `agents/engine/messages/trim.rs`)。
/// 编码类任务的长循环里,旧轮次被摘要后模型若只看见「工具 "fs_write" 原输出约 N 字符」,
/// 就不知道自己改过哪些文件、跑过什么命令 → 重复读、重复改(台账登记的实测缺陷)。
/// 锚点只占几十字符,**不是**撤销回收能力:整轮结果/参数照旧回收,只留这一句坐标。
///
/// 成员口径 = 会改变工作区或角色文件区状态的工具:工作区写族
/// (`WORKSPACE_TOOLS` 去掉 `WORKSPACE_READONLY_TOOLS` 的三个只读成员)+ 角色文件区写族
/// (`write`/`replace`/`create`)+ 命令执行(`bash`,锚点是命令首行)。
/// **`fs_patch`(Q6,2026-10-01)是写族成员但走「值制」锚点**——补丁首行
/// (「*** Begin Patch」)无信息量,其锚点取**补丁目标路径清单**,提取在
/// `agent_tools_fs_patch::patch_anchor`,消费在 `trim.rs::anchor_of`;**不要**在本函数
/// 里给 fs_patch 补键制映射(那会得到无意义的首行锚点)。
/// **不要**与 `services/undo_service.rs` 的 `WRITE_TOOLS` 合并:那份管「可回退快照」
/// (含 `update_variables`/`memory_write`、不含 `bash`),语义不同,各自演进。
/// 也别与 `tools/action_class.rs::classify` 混用——那份只 match
/// `read/write/create/replace/bash`,**不含 `fs_write`/`fs_edit`**(它们落 `other()`),
/// 拿它判「写类」会静默恒假。
pub fn state_anchor_arg(name: &str) -> Option<&'static str> {
    match name {
        "fs_write" | "fs_edit" | "write" | "replace" | "create" => Some("path"),
        "bash" => Some("command"),
        _ => None,
    }
}

/// 剔除正文元工具(get_state/apply_patch),保留其余工具与原顺序。
/// 顺序稳定性是前缀缓存的前提,故不做排序。
pub fn exclude_meta(defs: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    defs.into_iter()
        .filter(|d| !META_TOOLS.contains(&d.name.as_str()))
        .collect()
}

/// 规划侦察轮白名单(按「本任务是否绑定作用域」求值):
/// 无作用域 = 既有只读清单(角色文件区语义的 read/search 等);
/// 有作用域 = 既有清单 + 工作区只读子集(仍禁写)。
/// 单一出处在 `tools/tool_sets.rs`——侦察装配点(plan_scout_loop)与冻结测试都引用它,
/// 避免「白名单常量改了、装配点没改」的漂移。
pub fn scout_tools(has_workspace: bool) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = READONLY_SCOUT.to_vec();
    if has_workspace {
        names.extend_from_slice(WORKSPACE_READONLY_TOOLS);
    }
    names
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
    /// (META_TOOLS 于二维批次 7b 追加 `run_flow`;WORKSPACE_TOOLS 于 2026-10-01 Q6
    /// 追加 `fs_patch`——两者均属**有意**扩列,理由见常量文档)
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
            &[
                "fs_read",
                "fs_write",
                "fs_edit",
                "fs_glob",
                "fs_grep",
                "fs_patch",
                "view_image",
                "zoom_image",
                "image_diff"
            ]
        );
        assert_eq!(WORKSPACE_READONLY_TOOLS, &["fs_read", "fs_glob", "fs_grep"]);
        assert_eq!(VISION_TOOLS, &["view_image", "zoom_image", "image_diff"]);
        for n in VISION_TOOLS {
            assert!(
                WORKSPACE_TOOLS.contains(n),
                "视觉工具必须并入工作区族(聊天路径与无作用域任务一体剔除):{n}"
            );
        }
    }

    /// 图像工具判据(条件提示词段用):视觉三件 ∪ 截图;精确名匹配,前缀同名不误伤
    /// (修复批次:判据此前只认视觉三件,截图在聊天可下发却拿不到纪律段)
    #[test]
    fn has_image_tools_detects_membership() {
        assert!(!has_image_tools(&[def("read")]));
        assert!(has_image_tools(&[def("read"), def("view_image")]));
        assert!(has_image_tools(&[def("screenshot")]));
        assert!(!has_image_tools(&[def("screenshot2"), def("view_image2")]));
        assert!(is_image_tool("screenshot"));
        assert!(!is_image_tool("screenshot_full"));
    }

    /// 视觉三件名单与注册常量单一出处一致:vision_tools 的三个 TOOL 常量改名时锁死,
    /// 防止常量漂移导致闸门/提示词条件静默失效
    #[test]
    fn vision_tools_match_registration_constants() {
        use crate::tools::vision_tools::{DIFF_TOOL, VIEW_TOOL, ZOOM_TOOL};
        assert_eq!(VISION_TOOLS, &[VIEW_TOOL, ZOOM_TOOL, DIFF_TOOL]);
        assert!(is_image_tool(crate::tools::screenshot::TOOL_NAME));
    }

    /// 侦察白名单的两档(D1):无作用域 = 既有只读清单(逐字不变),有作用域 = 追加
    /// 工作区只读三件且**只追加不替换**;写类与 bash 任何一档都不得出现。
    #[test]
    fn scout_tools_adds_workspace_readonly_only_with_scope() {
        assert_eq!(scout_tools(false), READONLY_SCOUT.to_vec());
        let with_ws = scout_tools(true);
        assert_eq!(
            with_ws.len(),
            READONLY_SCOUT.len() + WORKSPACE_READONLY_TOOLS.len()
        );
        for n in READONLY_SCOUT {
            assert!(with_ws.contains(n), "既有只读清单不得丢失:{n}");
        }
        for n in WORKSPACE_READONLY_TOOLS {
            assert!(with_ws.contains(n), "有作用域时应放开 {n}");
            assert!(WORKSPACE_TOOLS.contains(n), "只读子集必须是工作区族的子集");
        }
        for forbidden in ["fs_write", "fs_edit", "bash", "write", "replace", "create"] {
            assert!(
                !with_ws.contains(&forbidden),
                "规划侦察轮不得出现写/命令工具:{forbidden}"
            );
        }
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

    /// 截图开关过滤(视觉能力包 D5):关(默认)只剔截图工具本身(精确名匹配),
    /// 开时原样;对照项 screenshot2 不受影响。聊天路径与任务闸门共用本判据。
    #[test]
    fn filter_screenshot_is_exact_and_switchable() {
        let defs = vec![def("read"), def("screenshot"), def("screenshot2")];
        let off: Vec<String> = filter_screenshot(defs.clone(), false)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(off, vec!["read", "screenshot2"]);
        let on: Vec<String> = filter_screenshot(defs, true)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(on, vec!["read", "screenshot", "screenshot2"]);
    }
}
