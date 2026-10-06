//! 流程库的载入/保存与内置流程:`AgentFlowLibrary::load/save`、`finalize_library`、`builtin_flow`。

// 子模块内统一用 super::* 拿到 mod.rs 的公共词汇(类型/常量)与 re-export 的自由函数。
use super::*;
use crate::models::types::PlanStep;
use serde_json::Value;
use std::path::Path;
use uuid::Uuid;

impl AgentFlowLibrary {
    /// 从 data/agent_flows.json 加载;缺失/损坏记录日志后回退默认(不静默)。
    /// 兼容旧单流程格式 `{enabled, steps}`:自动迁移为库(旧步骤保留)。
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("agent_flows.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                // 按顶层字段预判格式:含 flows/current_flow_id 为库格式,否则为旧单流程格式
                // (库结构字段全部带 default,直接按库解析会误吞旧格式 → 必须先预判)
                let value: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(
                            error = e.to_string(),
                            "自定义流程配置解析失败,已回退默认配置"
                        );
                        return AgentFlowLibrary::default();
                    }
                };
                if value.get("flows").is_some() || value.get("current_flow_id").is_some() {
                    match serde_json::from_value::<AgentFlowLibrary>(value) {
                        Ok(lib) => finalize_library(lib),
                        Err(e) => {
                            tracing::error!(
                                error = e.to_string(),
                                "自定义流程配置解析失败,已回退默认配置"
                            );
                            AgentFlowLibrary::default()
                        }
                    }
                } else {
                    // 旧版单流程格式 → 迁移为流程库
                    match serde_json::from_value::<AgentFlowConfig>(value) {
                        Ok(mut old) => {
                            tracing::info!("检测到旧版单流程配置,已迁移为流程库");
                            if old.id.trim().is_empty() {
                                old.id = Uuid::new_v4().to_string();
                            }
                            if old.name.trim().is_empty() {
                                old.name = "默认流程".into();
                            }
                            // 旧版空配置(从未编辑过)→ 替换为内置协调流程,开箱即用
                            if old.steps.is_empty() {
                                old = builtin_flow();
                            }
                            let lib = AgentFlowLibrary {
                                current_flow_id: Some(old.id.clone()),
                                flows: vec![old],
                                seeded_pack_ids: Vec::new(),
                            };
                            if let Err(e) = lib.save(data_dir) {
                                tracing::error!(error = e, "旧配置迁移写入失败");
                            }
                            lib
                        }
                        Err(e) => {
                            tracing::error!(
                                error = e.to_string(),
                                "自定义流程配置解析失败,已回退默认配置"
                            );
                            AgentFlowLibrary::default()
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 首次运行:注入内置协调流程,开箱即用
                // (能力包流程**不在这里**注入:load 读不到 settings,包并入统一走
                //  `pack_flows`/`merge_pack_flows`,由构造期与设置写入钩子调用——单一出处)
                let lib = AgentFlowLibrary {
                    current_flow_id: Some(builtin_flow().id.clone()),
                    flows: vec![builtin_flow()],
                    seeded_pack_ids: Vec::new(),
                };
                let _ = lib.save(data_dir);
                lib
            }
            Err(e) => {
                tracing::error!(
                    error = e.to_string(),
                    "自定义流程配置读取失败,已回退默认配置"
                );
                AgentFlowLibrary::default()
            }
        }
    }

    /// 持久化流程库到 data/agent_flows.json(原子写:崩溃不留半截 JSON)
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::utils::fs_atomic::write_atomic(&data_dir.join("agent_flows.json"), text.as_bytes())
            .map_err(|e| e.to_string())
    }
}

/// 库加载收尾:未选择流程时默认指向第一个(升级/迁移兜底)
fn finalize_library(mut lib: AgentFlowLibrary) -> AgentFlowLibrary {
    if lib.current_flow_id.is_none() && !lib.flows.is_empty() {
        lib.current_flow_id = lib.flows.first().map(|f| f.id.clone());
    }
    lib
}

// ==================== 能力包流程并入(CODE-5,2026-09-30) ====================
//
// 并入缝的**单一出处**:`pack_flows`(选包)+ `merge_pack_flows`(合库)。LIT-5(文学包
// 流程预设)落地时**在此加一行**、复用同一个 merge,不另开第二个缝(见 `计划.md` CODE 章)。
//
// 三条纪律(与 `builtin_flow` 的既有语义一致):
//   ① 既有条目**逐字不动**——只 push 新条目、不改 `current_flow_id`(并入≠改选中);
//   ② **幂等 + 不复活**:id 记进 `seeded_pack_ids` 后永不再注入,之后用户删它改它都不影响
//      ——「注入过」本身就是凭据,故不需要墓碑字段(`计划.md` CODE-5 的 D3=(a));
//   ③ **关包不回收**:已注入的副本留在用户库里(既定边界,同 LIT-5 的 Q2 裁定)。

/// 按包开关给出**应并入**的包流程(每个包一行;谁开谁并入)。
///
/// 这是「开关 → 流程常量」的唯一映射点:调用方(构造期与设置写入钩子)只需把开关传进来,
/// 不必知道包里有几条流程、id 叫什么。
///
/// **LIT-5(2026-10-06)**:文学包流程(a) 不复活**已落地**——用户改过 / 删过就是用户资产,
/// 语义由 [`merge_pack_flows`] 的账目承担,本函数只管「该并入哪些」。
pub fn pack_flows(
    coding_bundle_enabled: bool,
    literary_bundle_enabled: bool,
) -> Vec<AgentFlowConfig> {
    let mut packs: Vec<AgentFlowConfig> = Vec::new();
    if coding_bundle_enabled {
        packs.extend(coding_pack_flows());
    }
    if literary_bundle_enabled {
        packs.extend(literary_pack_flows());
    }
    packs
}

/// 把**内置流程**(示范流程 + 包流程,FLOW-DEMO-1 起合并为一个并入缝)**并入**库(加性):
/// 返回是否发生了变更(调用方据此决定要不要落盘)。
///
/// 判定只有一条:`id` 已在 `seeded_pack_ids` → 跳过;否则 push 并记 id。
/// 因此「用户改过的内置流程」与「用户删过的内置流程」都不会被覆盖或复活——它们要么以
/// 用户版存在于 `flows`(同 id)、要么已被删且 id 仍在 `seeded_pack_ids` 里。
pub fn merge_pack_flows(lib: &mut AgentFlowLibrary, packs: Vec<AgentFlowConfig>) -> bool {
    let mut changed = false;
    for pack in packs {
        if pack.id.trim().is_empty() || lib.seeded_pack_ids.iter().any(|id| id == &pack.id) {
            continue;
        }
        lib.seeded_pack_ids.push(pack.id.clone());
        lib.flows.push(pack);
        changed = true;
    }
    changed
}

/// 编码能力包的内置流程(**常量**,首版 2 条;Q5=(a) 拍板)。
///
/// 每条都按 `validate_flow` 的约束自检(步骤非空、至少一个 `action=direct && generates=true`、
/// reflect 步骤不带 `generates`/`system_prompt`/子流程、`tools ⊆ 已注册工具`),
/// 由 `tests.rs::coding_pack_flows_pass_validate_flow` 用**真实注册工具集**锁死。
///
/// 两条流程的**工具面刻意不同**(内容级安全,不靠模型自觉):
///   - 审查流程全程只用**只读三件套**,审查在物理上不可能改动工作区;
///   - 实现流程才放开 `fs_write`/`fs_edit`/`bash`,并把「被拒/失败如实报告」写进正文。
///
/// 末步用**严格档**(`kind=strict`,单次调用、不下发任何工具):它的职责只是把结论写成报告,
/// 不需要再动手;`tools` 留空,故严格档与「无工具步骤」在本流程里是同一执行路径。
pub fn coding_pack_flows() -> Vec<AgentFlowConfig> {
    // 工具名从**单一出处**常量取(`agent_tools_fs` 的逐工具名 + bash),不写字面量二次
    use crate::tools::agent_tools_fs::{EDIT_TOOL, GLOB_TOOL, GREP_TOOL, READ_TOOL, WRITE_TOOL};
    use crate::tools::bash::TOOL_NAME as BASH_TOOL;
    let readonly: Vec<String> = vec![
        READ_TOOL.to_string(),
        GLOB_TOOL.to_string(),
        GREP_TOOL.to_string(),
    ];
    let full: Vec<String> = vec![
        READ_TOOL.to_string(),
        WRITE_TOOL.to_string(),
        EDIT_TOOL.to_string(),
        GLOB_TOOL.to_string(),
        GREP_TOOL.to_string(),
        BASH_TOOL.to_string(),
    ];
    // 自测步骤:命令 + 读文件(看构建/测试输出)
    let selftest_tools: Vec<String> = vec![BASH_TOOL.to_string(), READ_TOOL.to_string()];

    vec![
        AgentFlowConfig {
            id: "builtin-code-review".into(),
            name: "代码审查流程".into(),
            description: Some(
                "Kedai 内置编码流程:只读侦察 → 逐项审查 → 核验结论 → 出具报告。\
                 全程只下发只读工具,审查不会改动工作区。"
                    .into(),
            ),
            enabled: true,
            max_parallel_nodes: None,
            steps: vec![
                PlanStep {
                    id: "scope".into(),
                    name: "确定审查范围".into(),
                    enabled: true,
                    goal: "先读工作区里的项目约定文件(AGENTS.md / CLAUDE.md / README 等)与相关代码,\
                           确定本次审查的范围与关注点;本步只读不改,产出简短的审查清单。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    tools: Some(readonly.clone()),
                    tool_choice: Some("auto".into()),
                    ..Default::default()
                },
                PlanStep {
                    id: "review".into(),
                    name: "逐项审查".into(),
                    enabled: true,
                    goal: "按清单逐项审查,对每个问题给出**位置(文件:行)**、严重度、依据与修复建议;\
                           没有问题的项也如实说明已看过。只用只读工具,不得修改任何文件。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    tools: Some(readonly.clone()),
                    tool_choice: Some("auto".into()),
                    ..Default::default()
                },
                PlanStep {
                    id: "verify".into(),
                    name: "核验结论".into(),
                    enabled: true,
                    goal: "核验审查结论:是否覆盖全部待审文件、每条问题是否都有位置与依据、\
                           是否把风格偏好当成缺陷、有无未经核实的猜测;输出 PASS 或 FAIL 并给出理由。"
                        .into(),
                    action: "reflect".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "finalize".into(),
                    name: "出具报告".into(),
                    enabled: true,
                    goal: "按核验结论补齐遗漏,输出最终审查报告:按严重度分组,每条含位置、问题、\
                           依据与建议;未见问题的部分明确写出「未见问题」。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    kind: Some("strict".into()),
                    ..Default::default()
                },
            ],
        },
        AgentFlowConfig {
            id: "builtin-code-impl".into(),
            name: "实现·自测·复盘流程".into(),
            description: Some(
                "Kedai 内置编码流程:阅读与计划 → 最小实现 → 跑验证并修复 → 复盘核验 → 交付摘要。\
                 自测步骤可执行命令;命令被拒或失败时如实报告,不假装通过。"
                    .into(),
            ),
            enabled: true,
            max_parallel_nodes: None,
            steps: vec![
                PlanStep {
                    id: "plan".into(),
                    name: "阅读与计划".into(),
                    enabled: true,
                    goal: "先读项目约定文件(AGENTS.md / CLAUDE.md / README 等)与相关代码,\
                           再产出不超过 300 字的实施计划:改哪些文件、怎么验证、风险点是什么;\
                           本步只读不改。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    tools: Some(readonly.clone()),
                    tool_choice: Some("auto".into()),
                    ..Default::default()
                },
                PlanStep {
                    id: "implement".into(),
                    name: "按计划实现".into(),
                    enabled: true,
                    goal: "按计划做**最小改动**:改前先读目标文件、只改与目标相关的部分,\
                           不顺手重构无关代码;完成后简述改了哪些文件、为什么这么改。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    tools: Some(full.clone()),
                    tool_choice: Some("auto".into()),
                    ..Default::default()
                },
                PlanStep {
                    id: "selftest".into(),
                    name: "跑验证并修复".into(),
                    enabled: true,
                    goal: "运行计划里的验证命令(构建 / 测试 / lint)并按结果修复,直到通过;\
                           命令被拒绝、工具不可用或命令失败,**都必须如实报告是哪一条没跑通、\
                           为什么**,不得假装通过、不得把「没跑」说成「通过」。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    tools: Some(selftest_tools.clone()),
                    tool_choice: Some("auto".into()),
                    ..Default::default()
                },
                PlanStep {
                    id: "reflect".into(),
                    name: "复盘核验".into(),
                    enabled: true,
                    goal: "核验本次实现:是否达成任务目标、有无未说明的副作用或未验证的改动、\
                           验证证据是否真实(跑了什么命令、结果如何);输出 PASS 或 FAIL 并给出理由。"
                        .into(),
                    action: "reflect".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "wrapup".into(),
                    name: "交付摘要".into(),
                    enabled: true,
                    goal: "按复盘结论输出交付摘要:改了什么、为何这么改、验证证据(跑了什么、结果如何)、\
                           未做或未验证的部分;不复述过程细节。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    kind: Some("strict".into()),
                    ..Default::default()
                },
            ],
        },
    ]
}

/// 文学能力包的内置流程(**常量**,首版 4 条;LIT-5,2026-10-06)。
///
/// 每条都按 `validate_flow` 的约束自检(步骤非空、至少一个 `action=direct && generates=true`、
/// reflect 步骤不带 `generates`/`system_prompt`/子流程),由
/// `tests.rs::literary_pack_flows_pass_validate_flow` 用**真实注册工具集**锁死。
///
/// **工具面刻意收窄**(与编码包「工具面刻意不同」同思路,内容级安全):
/// 四条文学流程**一律不下发工具**——它们只改文本,不碰工作区;需要素材时由用户在目标里给。
/// 末步用**严格档**(`kind=strict`,单次调用、不下发任何工具):它的职责只是把结论定稿,
/// 不需要再动手;`tools` 留空,故严格档与「无工具步骤」在本流程里是同一执行路径。
///
/// 归属规则(Q2=(a) 不复活):注入后即用户库里的普通流程,用户改过 / 删过都不会被覆盖或复活;
/// 关包也不回收已注入副本(与编码包同款边界,见 `merge_pack_flows` 头注)。
pub fn literary_pack_flows() -> Vec<AgentFlowConfig> {
    vec![
        AgentFlowConfig {
            id: "builtin-lit-chapter".into(),
            name: "章节续写流程".into(),
            description: Some(
                "Kedai 内置文学流程:列推进要点 → 成文 → 反思 → 修订定稿。\
                 用于按目标与上文续写章节;全程不下发工具,只改文本。"
                    .into(),
            ),
            enabled: true,
            max_parallel_nodes: None,
            steps: vec![
                PlanStep {
                    id: "outline".into(),
                    name: "列推进要点".into(),
                    enabled: true,
                    goal: "先读任务目标与上文,列出本章的推进要点:视角与人称、场景与时间、\
                           关键动作与转折、出场人物、收束钩子;只列要点不写正文,控制在 300 字内,供下一步成文。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "draft".into(),
                    name: "成文".into(),
                    enabled: true,
                    goal: "按要点写出正文:人称与叙述距离全程一致;用可观察的动作、器物、身体反应与对话\
                           承载情绪,不直接命名情绪、不写解释性旁白;不复述上文已写过的场景与比喻;\
                           不替用户角色说话或代做决定;按目标篇幅执行,把一个场景写透。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    ..Default::default()
                },
                PlanStep {
                    id: "reflect".into(),
                    name: "反思核验".into(),
                    enabled: true,
                    goal: "核验草稿:是否服从目标篇幅与走向要求、人称与视角有无漂移、有无 AI 腔套语与\
                           排比堆砌、有无复读上文的比喻、有无替用户角色代答、时间线与已确立事实有无矛盾;\
                           输出 PASS 或 FAIL 并给出理由(只判定,不重写)。"
                        .into(),
                    action: "reflect".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "revise".into(),
                    name: "修订定稿".into(),
                    enabled: true,
                    goal: "按反思结论只改被指出问题的部分,输出修订后的**完整正文**;未指出问题处保持原样;\
                           不复述修改说明之外的创作过程,不附检查清单。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    kind: Some("strict".into()),
                    ..Default::default()
                },
            ],
        },
        AgentFlowConfig {
            id: "builtin-lit-polish".into(),
            name: "润色去 AI 腔流程".into(),
            description: Some(
                "Kedai 内置文学流程:标出 AI 腔重灾区 → 重写 → 复查 → 定稿。\
                 用于把已有正文改写成更自然的文字;不改情节走向,全程不下发工具。"
                    .into(),
            ),
            enabled: true,
            max_parallel_nodes: None,
            steps: vec![
                PlanStep {
                    id: "locate".into(),
                    name: "标出 AI 腔重灾区".into(),
                    enabled: true,
                    goal: "逐段标出 AI 腔问题:套语(「不禁」「不由得」「心中一颤」「眼底闪过一丝……」)、\
                           三段排比连用、先否后肯句式(「不是……而是……」)、结尾强行升华、\
                           「仿佛 / 似乎」式空比喻、直接命名情绪、解释性旁白与总结句;\
                           每条给位置与替换方向。本步**不重写**。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "rewrite".into(),
                    name: "重写".into(),
                    enabled: true,
                    goal: "按标出的问题重写文本:用具体动作、器物、身体反应与对话承载情绪;\
                           长短句交错、段落长度参差;删除解释性旁白与总结句;\
                           **保留原意与情节走向不变**,不增删事件。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    ..Default::default()
                },
                PlanStep {
                    id: "reflect".into(),
                    name: "复查".into(),
                    enabled: true,
                    goal: "对照禁用清单复查改写稿:套语与排比是否清除、情绪是否由细节承载、\
                           是否引入了新的事实或情节变化(润色不得改动情节)、篇幅是否与原文相当;\
                           输出 PASS 或 FAIL 并给出理由。"
                        .into(),
                    action: "reflect".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "finalize".into(),
                    name: "定稿".into(),
                    enabled: true,
                    goal: "按复查结论修订后输出**最终稿**:只输出润色后的正文,不附修改说明、\
                           不附前后对比、不复述过程。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    kind: Some("strict".into()),
                    ..Default::default()
                },
            ],
        },
        AgentFlowConfig {
            id: "builtin-lit-consistency".into(),
            name: "一致性校对流程".into(),
            description: Some(
                "Kedai 内置文学流程:提取既定事实 → 比对矛盾 → 出具问题清单;\
                 只校对、不改写正文,全程不下发工具。"
                    .into(),
            ),
            enabled: true,
            max_parallel_nodes: None,
            steps: vec![
                PlanStep {
                    id: "extract".into(),
                    name: "提取既定事实".into(),
                    enabled: true,
                    goal: "从既有正文与设定中提取既定事实清单:人物(称呼、外貌、身份、已经知道的信息)、\
                           时间线(已发生事件及其先后)、世界设定与数值、已用过的比喻与场景。\
                           只摘录,不评判。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "compare".into(),
                    name: "比对矛盾".into(),
                    enabled: true,
                    goal: "用事实清单逐项比对待校文本,列出矛盾点:事实 / 称呼回退、时间顺序冲突、\
                           人物知晓了不该知道的信息、设定与数值不一致、同一比喻重复使用;\
                           每条给位置、依据与建议改法。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "report".into(),
                    name: "出具问题清单".into(),
                    enabled: true,
                    goal: "输出一致性问题清单:按严重度分组,每条含位置、矛盾点、依据、建议改法;\
                           未见问题的部分明确写出「未见问题」;本流程只校对,**不改写正文**。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    kind: Some("strict".into()),
                    ..Default::default()
                },
            ],
        },
        AgentFlowConfig {
            id: "builtin-lit-voice".into(),
            name: "人物声音校准流程".into(),
            description: Some(
                "Kedai 内置文学流程:摘既有台词特征 → 形成声音卡 → 按卡改写 → 复查。\
                 用于让角色说话「像他自己」;不改情节走向,全程不下发工具。"
                    .into(),
            ),
            enabled: true,
            max_parallel_nodes: None,
            steps: vec![
                PlanStep {
                    id: "sample".into(),
                    name: "摘既有台词特征".into(),
                    enabled: true,
                    goal: "摘出各角色已有的台词与行动片段,归纳每个角色的声音特征:用词习惯、\
                           句长与语速、口头禅、称呼方式、情绪表达方式。只归纳,不改写。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "profile".into(),
                    name: "形成声音卡".into(),
                    enabled: true,
                    goal: "为每个出场角色写一张声音卡:一句定位 + 3~5 条可检核的特征(用词、句式、\
                           称呼、反应模式),各附一句正例(可用原文或改写示例)。"
                        .into(),
                    action: "direct".into(),
                    generates: None,
                    ..Default::default()
                },
                PlanStep {
                    id: "apply".into(),
                    name: "按卡改写".into(),
                    enabled: true,
                    goal: "按声音卡改写目标段落中的角色台词与行动描写,使声音与卡一致;\
                           **不改动情节走向与他人的台词**;改写幅度以最小必要为准。"
                        .into(),
                    action: "direct".into(),
                    generates: Some(true),
                    ..Default::default()
                },
                PlanStep {
                    id: "reflect".into(),
                    name: "复查".into(),
                    enabled: true,
                    goal: "复查改写结果:每个角色的台词是否与声音卡一致、有无把不同角色写成同一种语气、\
                           情节是否被改动;输出 PASS 或 FAIL 并给出理由。"
                        .into(),
                    action: "reflect".into(),
                    generates: None,
                    ..Default::default()
                },
            ],
        },
    ]
}

/// 内置默认流程:与 Kedai harness 协调的文学创作/角色扮演流程。
/// 三步:起草正文(先写 ≤200 字计划再输出正文)→ 反思(判定 PASS/FAIL)→ 修订(生成)。
/// 步骤级提示词使用 Kedai 酒馆宏与 mvu 变量协议,与 custom 模式引擎语义对齐。
pub fn builtin_flow() -> AgentFlowConfig {
    AgentFlowConfig {
        id: "builtin-coordination".into(),
        name: "文学创作协调流程".into(),
        description: Some(
            "Kedai 内置协调流程:起草正文(先计划后正文)→ 反思质量 → 修订输出;与酒馆宏、世界书、mvu 变量协议(function calling 优先)对齐。"
                .into(),
        ),
        enabled: true,
        // 并行上限缺省 = DEFAULT_MAX_PARALLEL_NODES(2):内置流程是线性三步,
        // 并发度无实际影响,留 None 以便跟随默认值演进
        max_parallel_nodes: None,
        steps: vec![
            PlanStep {
                id: "draft".into(),
                name: "起草正文".into(),
                enabled: true,
                goal: "先撰写 ≤200 字写作计划,再按计划生成正文,服从用户字数/视角要求".into(),
                action: "direct".into(),
                generates: Some(true),
                system_prompt: Some(
                    "【本步指令·计划并起草】开始输出正文前,先在草稿区撰写一份 200 字以内的写作计划\
                     (段落结构、核心要点、情节走向、节奏安排),随后严格按该计划输出正文;\
                     正文中不得包含计划本身,不得出现「计划」「草稿」等字样。\
                     以 {{char}} 的视角与口吻输出:视角遵循系统提示词与用户要求\
                     (用户未指定时以角色自身视角叙述);不要出现旁白标题、「以上是回复」等元文本。\
                     服从用户的字数与风格要求;用户提问必须正面回答。\
                     变量更新优先通过 function calling 工具(update_variables)完成,模型未走工具时\
                     才回退 <UpdateVariable> 文本协议(JSONPatch 数组,支持 replace / delta / insert / remove / move,\
                     路径对应当前状态的键);无变化则不输出;纯状态更新时正文可以为空。除该块外不得使用任何自定义标签。"
                        .into(),
                ),
                tools: Some(vec!["update_variables".into()]),
                tool_choice: Some("auto".into()),
                ..Default::default()
            },
            PlanStep {
                id: "reflect".into(),
                name: "反思质量".into(),
                enabled: true,
                goal: "检查草稿:字数是否达标、疑问是否回答、视角是否一致、是否被截断;判定输出 PASS 或 FAIL".into(),
                action: "reflect".into(),
                generates: None,
                ..Default::default()
            },
            PlanStep {
                id: "revise".into(),
                name: "修订输出".into(),
                enabled: true,
                goal: "按反思结论修订并输出最终正文;未发现问题时直接输出原草稿".into(),
                action: "direct".into(),
                generates: Some(true),
                system_prompt: Some(
                    "【本步指令·修订输出】基于反思结论修订上一版草稿:修复缺陷后再次以 {{char}} 视角完整输出最终正文;\
                     若反思结论为通过,原样输出草稿。任何情况下都不得输出反思过程本身。\
                     变量更新同样优先经 function calling 工具(update_variables),未走工具时回退 <UpdateVariable> 文本协议,\
                     格式与起草步骤相同。"
                        .into(),
                ),
                tools: Some(vec!["update_variables".into()]),
                tool_choice: Some("auto".into()),
                ..Default::default()
            },
        ],
    }
}

/// 内置**画布示范流程**(FLOW-DEMO-1,2026-10-01):「多路调研示范流程」。
///
/// 与既有三条内置(全为线性,进画布只是一条竖链)不同,本条是**二维形态**:显式
/// `inputs` 连边 + 画布坐标 `x/y`,打开画布即分叉—汇合结构,演示四种组织形态——
/// 并行分支 / 多上游汇合 / 反思核验 / 严格收档。通用「调研—核验—报告」链路,
/// 不绑定任何能力包(构造期随内置簇并入,见 `service.rs::new`)。
///
/// 结构(5 节点,拓扑序 = 数组序):
///   `decompose`(源,内部规划)→ `facts` ‖ `risks`(两路并行)→ `verify`(reflect 汇合)
///   → `report`(strict 终稿,显式 `is_output`)。
///
/// 坐标按画布常量口径(NODE_W 220 / GAP_X 48 / NODE_H 64 / GAP_Y 56)预置:单点层横坐标
/// 居中 134,双点层 0/268,层距 120——前端 `savedPosition` 直接命中,「打开即二维」。
///
/// 工具面与编码审查流程同款**只读三件**(fs_read/fs_glob/fs_grep;工具名取单一出处常量):
/// 调研只读取,严格终稿不下发工具。节点不叠加 max_retries/超时等参数花样(示范结构,
/// 保持 canonical);`max_parallel_nodes` 留空跟随默认 2——两条支路天然并行(并行成本
/// 提示写进 description,WF-11 口径)。
///
/// 三条并入纪律与包流程一致(幂等、「删过/改过不复活」、关包不回收——同一 merge 缝);
/// 由 `tests.rs::builtin_demo_flow_passes_validate_flow` 等用例用**真实注册工具集**锁死。
pub fn builtin_demo_flows() -> Vec<AgentFlowConfig> {
    // 工具名从单一出处常量取(agent_tools_fs 逐工具名),不写字面量二次
    use crate::tools::agent_tools_fs::{GLOB_TOOL, GREP_TOOL, READ_TOOL};
    let readonly: Vec<String> = vec![
        READ_TOOL.to_string(),
        GLOB_TOOL.to_string(),
        GREP_TOOL.to_string(),
    ];
    vec![AgentFlowConfig {
        id: "builtin-research-demo".into(),
        name: "多路调研示范流程".into(),
        description: Some(
            "Kedai 内置画布示范流程:拆解研究目标 → 两路并行调研(事实与现状 / 问题与风险)\
             → 交叉核验 → 严格档输出综合报告。演示二维画布的四种组织形态:并行分支、\
             多上游汇合、反思核验、收紧输出的最终节点(两条支路并行执行,会成倍消耗 token)。"
                .into(),
        ),
        enabled: true,
        // 两条支路天然并行;缺省 = DEFAULT_MAX_PARALLEL_NODES(2),留 None 跟随默认演进
        max_parallel_nodes: None,
        steps: vec![
            PlanStep {
                id: "decompose".into(),
                name: "拆解研究目标".into(),
                enabled: true,
                goal: "阅读任务目标(必要时用只读工具查看工作区资料),把研究拆解为两个视角:\
                       「事实与现状」与「问题与风险」,为每路给出 2~4 条要调研的要点清单。\
                       本步只做内部分析,不产出面向用户的最终正文。"
                    .into(),
                action: "direct".into(),
                generates: Some(false),
                tools: Some(readonly.clone()),
                tool_choice: Some("auto".into()),
                x: Some(134.0),
                y: Some(0.0),
                ..Default::default()
            },
            PlanStep {
                id: "facts".into(),
                name: "事实与现状".into(),
                enabled: true,
                goal: "围绕「事实与现状」视角调研:梳理与主题相关的事实、数据与当前状态,\
                       每条注明依据来源(读过的文件/资料);读不到的部分如实标注「未能核实」。"
                    .into(),
                action: "direct".into(),
                generates: Some(true),
                tools: Some(readonly.clone()),
                tool_choice: Some("auto".into()),
                inputs: vec!["decompose".into()],
                x: Some(0.0),
                y: Some(120.0),
                ..Default::default()
            },
            PlanStep {
                id: "risks".into(),
                name: "问题与风险".into(),
                enabled: true,
                goal: "围绕「问题与风险」视角调研:梳理约束、风险、反例与不确定点,\
                       每条注明依据;与事实路重复的结论只做交叉引用,不重复展开。"
                    .into(),
                action: "direct".into(),
                generates: Some(true),
                tools: Some(readonly.clone()),
                tool_choice: Some("auto".into()),
                inputs: vec!["decompose".into()],
                x: Some(268.0),
                y: Some(120.0),
                ..Default::default()
            },
            PlanStep {
                id: "verify".into(),
                name: "交叉核验".into(),
                enabled: true,
                goal:
                    "核验两路调研:目标覆盖是否完整、每条结论是否有依据、有无未核实的臆测、\
                       两路之间有无矛盾。输出 PASS 或 FAIL 并给出理由;FAIL 时逐条列明需要补齐的点。"
                        .into(),
                action: "reflect".into(),
                inputs: vec!["facts".into(), "risks".into()],
                x: Some(134.0),
                y: Some(240.0),
                ..Default::default()
            },
            PlanStep {
                id: "report".into(),
                name: "综合报告".into(),
                enabled: true,
                goal: "按核验结论补齐遗漏,输出最终综合报告:先给结论,再分「事实与现状」\
                       「问题与风险」两节展开,每条保留依据;核验未通过且无法补齐的部分,\
                       在报告末尾单独列为「未核实事项」。"
                    .into(),
                action: "direct".into(),
                generates: Some(true),
                kind: Some("strict".into()),
                is_output: Some(true),
                inputs: vec!["verify".into()],
                x: Some(134.0),
                y: Some(360.0),
                ..Default::default()
            },
        ],
    }]
}

// ==================== 二维流程原语(二维批次 1) ====================
//
// **单一出处**:保存期校验(`validate_flow`)、任务侧执行器(`task_engine/custom.rs`)、
// 聊天侧线性化(`agents/planner.rs::make_custom_plan`)三处共用下面这些纯函数。
// 与 `prompt_kit` 的共享原语同一纪律:不得在任一侧复制实现(否则「什么算合法图」
// 会出现多个版本,前端能保存的图与能执行的图就会漂移)。
