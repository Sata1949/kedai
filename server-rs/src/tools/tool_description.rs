// 内置工具描述的按模式分叉（RTSPLIT-R3，2026-11-02）。
//
// 工具 `description` 与 system 同级进入模型上下文——模型会把描述里的名词当成真实存在的
// 概念。`read` / `memory_*` / `censor_text` / `revise_passage` / `agentgo` / `role` 六族的
// 原文案是按角色扮演语义写的（「角色提示词」「本角色」「正文」「角色设定」「掷骰子」），
// 聊天链消费它们完全正确；但任务链（solo / multi / team / custom / followup / 规划器侦察）
// 用的是同一份注册表，任务模型因此会看到「本角色」「正文」这类在该模式下不存在的概念，
// 最强的两处直接指错工具面：
//   - `read`：任务侧读的是**工作区文件**（`fs_*` 族）与子任务结果，不是角色文件区；
//   - `memory_*`：无执行者/无角色卡的任务会落到空记忆桶，措辞必须中性。
//
// 为什么在读取侧分叉而不是在注册点写两份：注册点是聊天链的唯一来源，改写它会波及
// 20 余处角色扮演调用点；而「任务侧要什么描述」只在任务工具面的出口才成立。因此保持
// 注册原文逐字节不变（`tool_description_is_verbatim_in_roleplay` 锁死），由任务侧出口
// 经 [`for_task`] 重写。新增按模式分叉的工具时，同时补 `task_description` 分支与本表。
//
// 复用的既有原语：模式枚举用 `settings_service::AppMode`（RTSPLIT-1/3 已用它承载
// 双模式分叉），不为本模块另造枚举。
use crate::models::types::ToolDefinition;
use crate::services::settings_service::AppMode;

/// 按模式返回工具描述。
///
/// `Roleplay` 恒返回 `original` 本身（聊天链零改动）；`Task` 只对已知的六族工具换成
/// 任务向文案，其余工具（含插件 / MCP 自定义工具）同样原样返回——静默清空会让那些工具
/// 在任务侧彻底失去语义，比保留创作措辞更糟。
pub fn tool_description_for(mode: AppMode, name: &str, original: &str) -> String {
    match mode {
        AppMode::Roleplay => original.to_string(),
        AppMode::Task => task_description(name).unwrap_or_else(|| original.to_string()),
    }
}

/// 把一组工具定义按模式重写描述（任务侧工具面的统一出口）。
///
/// 任务侧有四条下发路径（solo / custom 步骤 / team 审计 / 规划器侦察），都在各自的
/// 定义集装配处调用本函数；放在这里而不是各写一遍，是为了让「哪条路径漏了」在
/// 类型层面就只有一个答案可查。
pub fn with_mode(mode: AppMode, mut defs: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    if matches!(mode, AppMode::Roleplay) {
        return defs;
    }
    for d in &mut defs {
        d.description = tool_description_for(mode, &d.name, &d.description);
    }
    defs
}

/// 任务向描述。`None` = 该工具不分叉，沿用注册原文。
fn task_description(name: &str) -> Option<String> {
    let s = match name {
        // 任务侧的 read 读工作区文件与子任务结果；`character_prompt` 在任务链
        // 只有旧任务的角色卡回退路径会命中，不再作为主语义宣传。
        "read" => "读取资料:世界书条目(world_book)、技能库(skill)、工作区文件(file)、子智能体任务结果(subtask)。\
                   支持一次查询多个目标(queries 数组)。读工作区文件时优先用 fs_read(带行号、可分页),\
                   本工具用于按名称或关键词检索。",
        "memory_read" => "读取本会话/本任务的跨会话长期记忆(无执行者身份时为空桶)",
        "memory_write" => "写入一条本会话/本任务的跨会话长期记忆",
        // 禁词库是角色扮演侧的注入配置；任务侧没有这份配置，描述不得暗示它存在。
        "censor_text" => "对文本做替换处理:把给定词表里的词替换为指定替词(精确子串匹配,保留其余内容语义)",
        "revise_passage" => "定点修改文本中的某一段:把原文片段(find)替换为新文本(replace)。\
                             用于修正不合逻辑的段落、不当表述或需要改写的句子,而非整体重生成。",
        "agentgo" => "排出子智能体:为每个任务在后台用当前模型独立生成一段内容(带常驻世界书记忆与任务上下文)。\
                      返回 task_id,后续用 read(type=subtask) 或 todo 轮询结果;用 agentend 结束任务。\
                      写指令时按此模板逐条给全:\n契约: <引用来源与版本>\n角色: <谁>\n单一交付物: <一个名词短语>\n\
                      判据: <能以是/否回答的验收句>\n输出: <形态与边界(如仅 JSON,首字符 { 末字符 },不得含围栏;或散文不限格式)>\n\
                      长度: 目标 ≤ <N> 字;若被迫截断,须以单独一行 END 收尾\n引用: 只允许引用既有字段名,禁止自拟字段\n\
                      结果末尾缺 END 即视为截断,应重发或明确标注 gap,不得当作完成。",
        // 「掷骰子 / 1d20」是角色扮演动作；任务侧中性化为随机整数。
        "role" => "生成随机整数。两种模式:按面数模式(sides=上限值,count=个数)或区间模式(min/max)。",
        _ => return None,
    };
    Some(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 会被按模式改写描述的工具名全集。新增成员时必须同时补 `task_description` 分支。
    const AWARE_TOOLS: &[&str] = &[
        "read",
        "memory_read",
        "memory_write",
        "censor_text",
        "revise_passage",
        "agentgo",
        "role",
    ];

    /// 角色扮演专属词汇：任务向文案里一律不得出现。
    /// 清单来自 `docs/plans/PROMPT-BOUNDARY-HANDOFF-2026-11-02.md` §R-3 的逐条实测结论。
    const ROLEPLAY_VOCAB: &[&str] = &["角色提示词", "本角色", "正文", "角色设定", "掷骰子", "人设"];

    /// 角色扮演侧逐字节等于注册点原文——这是「聊天链零回归」的机器承诺。
    /// 原文取自各注册点，改写其中任何一处都会让本用例失败。
    #[test]
    fn tool_description_is_verbatim_in_roleplay() {
        let cases: &[(&str, &str)] = &[
            ("read", "读取上下文资料:世界书条目(world_book)、角色提示词(character_prompt)、技能库(skill)、角色文件区文件(file)、子智能体任务结果(subtask)。支持一次查询多个目标(queries 数组)。"),
            ("memory_read", "读取本角色的跨会话长期记忆"),
            ("memory_write", "写入一条本角色的跨会话长期记忆"),
            ("censor_text", "对文本进行禁词替换:把出现禁词的位置改写为含义相近、更得体的替换词(同义改写,保留上下文语义)"),
            ("revise_passage", "定点修改正文中的某段文本:把原文片段(find)替换为新文本(replace)。用于修正不合逻辑的段落、不当表述或需要改写的句子,而非整体重生成。"),
            ("role", "创建随机数/掷骰子。支持两种模式:骰子(sides=面数,count=次数,如 1d20)或范围(min/max)。"),
        ];
        for (name, original) in cases {
            assert_eq!(
                tool_description_for(AppMode::Roleplay, name, original),
                *original,
                "角色扮演侧 {name} 的描述必须逐字节不变"
            );
        }
    }

    /// 任务侧：文案必须存在、必须与原文不同、且不得含角色扮演专属词汇。
    #[test]
    fn task_description_differs_and_has_no_roleplay_vocabulary() {
        for name in AWARE_TOOLS {
            let original = format!("{name} 的角色扮演原文");
            let task = tool_description_for(AppMode::Task, name, &original);
            assert_ne!(
                task, original,
                "任务侧 {name} 必须有独立描述，不得静默回退原文"
            );
            assert!(
                !task.trim().is_empty(),
                "任务侧 {name} 的描述不得为空(空描述会让模型失去工具语义)"
            );
            for word in ROLEPLAY_VOCAB {
                assert!(
                    !task.contains(word),
                    "任务侧 {name} 的描述不得含角色扮演词汇「{word}」: {task}"
                );
            }
        }
    }

    /// 插件 / MCP 自定义工具没有任务向文案：两种模式都必须原样返回。
    /// 静默清空会让插件工具在任务侧失去全部语义，比保留创作措辞更糟。
    #[test]
    fn unknown_tool_name_is_original_in_both_modes() {
        let original = "插件自定义工具的原始描述";
        for mode in [AppMode::Roleplay, AppMode::Task] {
            assert_eq!(
                tool_description_for(mode, "my_plugin_tool", original),
                original,
                "{mode:?} 侧未知工具必须原样返回"
            );
        }
    }

    /// agentgo 的契约模板（契约/交付物/判据/END）两个模式都要用，任务侧尤其依赖它。
    /// 分叉只该去掉「角色设定」这类创作措辞，不该把模板整段丢掉。
    #[test]
    fn agentgo_task_description_keeps_contract_template() {
        let task = tool_description_for(AppMode::Task, "agentgo", "");
        for needed in ["契约:", "判据:", "交付物", "END"] {
            assert!(
                task.contains(needed),
                "任务侧 agentgo 必须保留契约模板的「{needed}」: {task}"
            );
        }
        assert!(
            !task.contains("角色设定"),
            "任务侧 agentgo 不得再说「角色设定」: {task}"
        );
        // 原文案「带角色设定与常驻世界书上下文」只有聊天链适用；任务侧要说清子 agent
        // 拿到的到底是什么上下文，否则模型会以为子 agent 带着角色卡跑。
        assert!(
            task.contains("世界书") || task.contains("工作区"),
            "任务侧 agentgo 应说明子 agent 的上下文来源: {task}"
        );
    }

    /// read 在任务侧读的是工作区文件 / 子任务结果，不是「角色文件区」；
    /// memory 归属会话或任务维度，不是「本角色」——否则无执行者的任务会写进空桶。
    #[test]
    fn read_and_memory_task_descriptions_match_task_semantics() {
        let read = tool_description_for(AppMode::Task, "read", "");
        assert!(
            read.contains("工作区"),
            "任务侧 read 应说明读的是工作区文件: {read}"
        );
        assert!(
            read.contains("subtask"),
            "任务侧 read 仍应说明可读子任务结果: {read}"
        );
        for name in ["memory_read", "memory_write"] {
            let text = tool_description_for(AppMode::Task, name, "");
            assert!(
                !text.contains("角色"),
                "任务侧 {name} 不得归属到角色: {text}"
            );
        }
    }

    /// 「正文」是创作术语；任务侧改「文本片段」。禁词库是角色扮演注入配置，
    /// 任务侧没有这份配置，描述不该暗示它存在。
    #[test]
    fn revise_and_censor_task_descriptions_avoid_creative_terms() {
        let revise = tool_description_for(AppMode::Task, "revise_passage", "");
        assert!(
            revise.contains("文本"),
            "任务侧 revise_passage 应说明改的是文本片段: {revise}"
        );
        let censor = tool_description_for(AppMode::Task, "censor_text", "");
        assert!(
            !censor.contains("正文") && !censor.contains("禁词"),
            "任务侧 censor_text 不得使用「正文/禁词」: {censor}"
        );
    }

    /// 「掷骰子 / 1d20」是角色扮演动作；任务侧应中性化为随机整数。
    #[test]
    fn role_task_description_is_neutral_random() {
        let task = tool_description_for(AppMode::Task, "role", "");
        assert!(
            task.contains("随机"),
            "任务侧 role 应说明生成随机值: {task}"
        );
        assert!(!task.contains("骰"), "任务侧 role 不得使用骰子术语: {task}");
    }

    /// 批量重写只动已知工具，且 `Roleplay` 侧原样返回同一批定义（零改动语义）。
    #[test]
    fn with_mode_rewrites_only_known_tools() {
        let defs = vec![
            ToolDefinition {
                name: "read".into(),
                description: "原文".into(),
                parameters: serde_json::json!({}),
            },
            ToolDefinition {
                name: "my_plugin_tool".into(),
                description: "插件原文".into(),
                parameters: serde_json::json!({}),
            },
        ];
        let task = with_mode(AppMode::Task, defs.clone());
        assert_ne!(task[0].description, "原文", "已知工具应换成任务向文案");
        assert_eq!(task[1].description, "插件原文", "未知工具必须原样返回");

        let rp = with_mode(AppMode::Roleplay, defs);
        assert_eq!(rp[0].description, "原文", "角色扮演侧不得改写");
        assert_eq!(rp[1].description, "插件原文");
    }
}
