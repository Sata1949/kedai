// 规划器(与 Node 版 planner.ts 对齐)
use crate::models::types::{Plan, PlanStep};
use crate::services::agent_flow_service::resolve_graph;
use crate::tools::tool_sets::ARCHIVE_TOOLS;

/// fast: 单步直接生成;deep/agent: 草稿(隐藏) → 正文 → 反思(强制批判+定点修改)
/// → 归档与词条同步(RPFLOW-2);
/// agent 的正文步启用完整 function calling 工具循环(模型可多轮自主调用工具);
/// custom: 自定义流程(见 make_custom_plan),由设置中编辑的步骤序列驱动。
pub fn make_plan(user_input: &str, mode: &str) -> Plan {
    if mode == "agent" {
        let _ = user_input;
        Plan {
            steps: vec![
                PlanStep {
                    goal: "撰写剧情草稿(默认隐藏)".into(),
                    action: "draft".into(),
                    generates: None,
                    system_prompt: Some(DRAFT_PROMPT.into()),
                    // ≤200 字草稿的硬界:提示词约束之外再压输出上限(推理模型留余量)
                    max_tokens: Some(DRAFT_MAX_TOKENS),
                    ..Default::default()
                },
                PlanStep {
                    goal: "生成正文回复".into(),
                    action: "direct".into(),
                    generates: Some(true),
                    system_prompt: Some(
                        "【本步指令·正文】严格依据上方「内部草稿」扩写为完整正文:\
                         可自由充实细节与对话,但不得复述草稿本身,不得出现「草稿」「计划」等字样。\
                         以 {{char}} 的视角与口吻输出:视角遵循系统提示词与用户要求\
                         (用户未指定时以角色自身视角叙述);服从用户字数与风格要求,用户提问必须正面回答。\
                         你可以通过 function calling 自主调用下方列出的工具:\
                         需要随机数用 role、联网查最新信息用 search、读世界书/角色资料用 read、写对话气泡或文件用 write、\
                         需要后台并行处理子任务用 agentgo;需要时再调用,不必每轮都调。"
                            .into(),
                    ),
                    ..Default::default()
                },
                PlanStep {
                    goal: "批判与定点修改正文".into(),
                    action: "reflect".into(),
                    generates: None,
                    ..Default::default()
                },
                archive_step(),
            ],
            summary:
                "Agent 模式:草稿(隐藏) → 正文(工具循环) → 强制批判与修改(最多 4 次) → 归档与词条同步。"
                    .into(),
        }
    } else if mode == "deep" {
        Plan {
            steps: vec![
                PlanStep {
                    goal: "撰写剧情草稿(默认隐藏)".into(),
                    action: "draft".into(),
                    generates: None,
                    system_prompt: Some(DRAFT_PROMPT.into()),
                    max_tokens: Some(DRAFT_MAX_TOKENS),
                    ..Default::default()
                },
                PlanStep {
                    goal: "生成正文回复".into(),
                    action: "direct".into(),
                    generates: Some(true),
                    system_prompt: Some(
                        "【本步指令·正文】严格依据上方「内部草稿」扩写为完整正文:\
                         可自由充实细节与对话,但不得复述草稿本身,不得出现「草稿」「计划」等字样。\
                         以 {{char}} 的视角与口吻输出:视角遵循系统提示词与用户要求\
                         (用户未指定时以角色自身视角叙述);服从用户字数与风格要求,用户提问必须正面回答。"
                            .into(),
                    ),
                    ..Default::default()
                },
                PlanStep {
                    goal: "批判与定点修改正文".into(),
                    action: "reflect".into(),
                    generates: None,
                    ..Default::default()
                },
                archive_step(),
            ],
            summary: "深度模式:草稿(隐藏) → 正文 → 强制批判与修改(最多 2 次) → 归档与词条同步。".into(),
        }
    } else {
        let _ = user_input;
        Plan {
            steps: vec![PlanStep {
                goal: "根据上下文生成回复".into(),
                action: "direct".into(),
                generates: Some(true),
                ..Default::default()
            }],
            summary: "快速模式:直接生成回复".into(),
        }
    }
}

/// 草稿步输出上限(步骤级 max_tokens 硬界):≤200 字中文正文的合理预算,
/// 同时给推理模型留 reasoning 余量。
///
/// **2026-10-09 真实模型实测(deepseek-v4.1-flash)**:512 必空(整段预算被 reasoning
/// 吃光,HTTP 200 + `content` 空,草稿静默缺失);2048 通常够(复刻请求 132 字成功、
/// 全流程实测 699/131/174 字三例),但在「禁止出现:霓虹」这类强约束题面上偶发仍被
/// 烧空。故取 4096(普通情形只是上限、不额外花钱;烧空时由引擎侧提额重试兜底,
/// 见 `messages::steps::draft_heal_budget`),重试封顶见 [`DRAFT_HEAL_MAX_TOKENS`]。
pub const DRAFT_MAX_TOKENS: u32 = 4096;

/// 草稿「推理耗尽」提额重试的封顶(翻倍一次、封顶 4×;与 executor 截断自愈同口径)
pub const DRAFT_HEAL_MAX_TOKENS: u32 = DRAFT_MAX_TOKENS * 4;

/// 草稿步系统提示词(deep/agent 共用):产物默认隐藏、只服务正文步,
/// 以 **system 角色**注记并入上下文(见 engine/run_loop.rs 的 draft 分支)。
pub const DRAFT_PROMPT: &str = "【本步指令·草稿】为 {{char}} 的下一轮回复做准备,\
     只输出一段不超过 200 字的剧情草稿:本回合要推进的情节、情绪走向、\
     关键动作/台词要点、需要呼应或埋设的线索。\
     只输出草稿本身,不要写正文,不要解释,不要复述用户输入。";

/// 归档步(RPFLOW-2:deep/agent 流程第 4 步;产物不进正文,失败不影响本轮):
/// 更新角色文件区的 大纲.md / 人物关系.md,并按开关同步世界书词条
/// (绿灯=触发条目可更新;蓝灯=常驻条目不改写,改为新建绿灯条目)。
pub const ARCHIVE_PROMPT: &str = "【本步指令·归档与同步】本步不面向用户,不要输出给用户看的正文。\
     按顺序处理,无变化就跳过(不要为打卡而写入):\
     ① 用 read(type=file) 查看角色文件区的 大纲.md 与 人物关系.md(不存在则用 create 新建);\
     依据本轮正文只更新有变化的部分——大纲.md 记剧情主线与当前进展,人物关系.md 记角色关系与状态变化;\
     保持精炼(每次增量几十字以内),不要复述全文。\
     ② 若本轮剧情推演导致角色设定或世界观发生变化:先用 read(type=world_book) 查看相关条目,\
     再用 worldbook_update(topic, content, keywords) 写入最新情况——对应条目是绿灯(触发)条目则更新它;\
     是蓝灯(常驻)条目则**不要改写它**,改为新建一条绿灯条目。\
     ③ 无变化时不要做任何写入。完成后用一两句话报告你做了什么。";

/// 归档步骤构造(deep/agent 共用):工具白名单 = [`ARCHIVE_TOOLS`](只按名单释放;
/// `worldbook_update` 不在默认工具列表内,见 tools::tool_sets::META_TOOLS)。
fn archive_step() -> PlanStep {
    PlanStep {
        goal: "归档剧情与同步词条".into(),
        action: "archive".into(),
        generates: None,
        system_prompt: Some(ARCHIVE_PROMPT.into()),
        tools: Some(ARCHIVE_TOOLS.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    }
}

/// 自定义流程(custom 模式):按用户配置的步骤序列构建计划。
/// 过滤 disabled 步骤;兜底校验至少一个会生成的 direct 步骤(由 PUT /api/agent-flows
/// 的 validate_flow 强制,此处防御外部手改配置后再次兜底)。
pub fn make_custom_plan(steps: &[PlanStep]) -> Result<Plan, String> {
    let active: Vec<PlanStep> = steps.iter().filter(|s| s.enabled).cloned().collect();
    if active.is_empty() {
        return Err("自定义流程为空:请先在设置中启用至少一个步骤".into());
    }
    if !active
        .iter()
        .any(|s| s.action == "direct" && s.generates == Some(true))
    {
        return Err("自定义流程缺少生成步骤:至少需要一个「生成正文」的 direct 步骤".into());
    }
    // 二维流程:聊天侧按**拓扑序线性化**(聊天不并行,分支结构被忽略——见
    // docs/契约.md「Agent 执行流程(custom 模式)」执行语义);线性兼容流程顺序不变。
    // 图合法性由保存期 validate_flow 保证,此处按同一原语再解析一次做兜底。
    let graph = resolve_graph(&active)?;
    let ordered: Vec<PlanStep> = graph.order.iter().map(|&i| active[i].clone()).collect();
    let summary = format!("自定义流程:共 {} 步", ordered.len());
    Ok(Plan {
        steps: ordered,
        summary,
    })
}

/// 判断输入是否像计算式:明确「计算」指令、含数字的「算一下」口语,或数字+运算符算式。
/// 「算一下」不再无条件触发——口语如「算一下我欠你多少人情」不含数字,不应打断角色扮演。
pub fn looks_like_calculation(input: &str) -> bool {
    // 明确的「计算」指令:直接触发
    if input.contains("计算") {
        return true;
    }
    // 「算一下」等口语:仅当句中同时含数字才触发(避免「算一下我欠你多少人情」误判)
    if input.contains("算一下") && input.bytes().any(|b| b.is_ascii_digit()) {
        return true;
    }
    // 其余:数字+运算符 的算式
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            // 找到数字后,跳过它及空白,看是否跟运算符
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                j += 1;
            }
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && matches!(bytes[j], b'+' | b'-' | b'*' | b'/') {
                return true;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    false
}

/// 提取计算表达式:去掉中文词/问号/句号后,匹配加减乘除表达式,去空格返回
pub fn extract_expression(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .filter(|c| {
            // 注意:小数点 '.' 必须保留(小数算式 12.5*2),仅过滤中文句号与标点
            !matches!(
                c,
                '计' | '算' | '一' | '下' | '？' | '?' | '。' | ':' | '：' | ' ' | '　'
            )
        })
        .collect();
    // 匹配:括号子表达式或数字,以运算符连接,至少一次
    let re = regex_multi_expr(&cleaned);
    if let Some(m) = re {
        return m;
    }
    cleaned
}

/// 手写扫描:匹配 (?:\([^)]+\)|-?\d+(?:\.\d+)?)(?:\s*[+\-*/]\s*(?:\([^)]+\)|-?\d+(?:\.\d+)?))+
fn regex_multi_expr(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((end, _)) = parse_operand(&bytes[i..]) {
            // 尝试扩展操作数链
            let mut j = i + end;
            let mut ops = 0;
            loop {
                // 跳过空白
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && matches!(bytes[j], b'+' | b'-' | b'*' | b'/') {
                    j += 1;
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if let Some((len2, _)) = parse_operand(&bytes[j..]) {
                        j += len2;
                        ops += 1;
                        continue;
                    }
                }
                break;
            }
            if ops >= 1 {
                let expr = &s[i..j];
                let no_space: String = expr.chars().filter(|c| !c.is_whitespace()).collect();
                return Some(no_space);
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

/// 解析一个操作数:括号子表达式 或 带可选负号的数字;返回(长度, 是否括号)
fn parse_operand(s: &[u8]) -> Option<(usize, bool)> {
    if s.is_empty() {
        return None;
    }
    if s[0] == b'(' {
        let mut depth = 0;
        for (k, &b) in s.iter().enumerate() {
            if b == b'(' {
                depth += 1;
            } else if b == b')' {
                depth -= 1;
                if depth == 0 {
                    return Some((k + 1, true));
                }
            }
        }
        return None;
    }
    let mut k = 0;
    if s[0] == b'-' {
        k = 1;
    }
    let mut seen_digit = false;
    while k < s.len() && (s[k].is_ascii_digit() || s[k] == b'.') {
        if s[k].is_ascii_digit() {
            seen_digit = true;
        }
        k += 1;
    }
    if seen_digit {
        Some((k, false))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_plan() {
        let p = make_plan("你好", "fast");
        assert_eq!(p.steps.len(), 1);
        assert_eq!(p.steps[0].action, "direct");
        assert_eq!(p.summary, "快速模式:直接生成回复");
    }

    #[test]
    fn test_deep_plan() {
        let p = make_plan("你好", "deep");
        // RPFLOW 流程:草稿(隐藏) → 正文 → 反思(批判+定点修改) → 归档与词条同步
        assert_eq!(p.steps.len(), 4);
        assert_eq!(p.steps[0].action, "draft");
        assert_eq!(p.steps[1].action, "direct");
        assert_eq!(p.steps[1].generates, Some(true));
        assert_eq!(p.steps[2].action, "reflect");
        assert_eq!(p.steps[3].action, "archive");
        let expected: Vec<String> = ARCHIVE_TOOLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(p.steps[3].tools.as_ref(), Some(&expected));
    }

    #[test]
    fn test_agent_plan_step_prompts() {
        let p = make_plan("你好", "agent");
        assert_eq!(p.steps.len(), 4);
        // 草稿步:隐藏、≤200 字、有硬性输出上限
        let draft = &p.steps[0];
        assert_eq!(draft.action, "draft");
        let dp = draft.system_prompt.as_deref().unwrap_or("");
        assert!(dp.contains("200 字"), "草稿步应限定 ≤200 字: {dp}");
        assert_eq!(draft.max_tokens, Some(DRAFT_MAX_TOKENS));
        // 正文步:扩写自草稿、仍保留工具说明
        let gen = &p.steps[1];
        assert_eq!(gen.generates, Some(true));
        let sp = gen.system_prompt.as_deref().unwrap_or("");
        assert!(
            sp.contains("内部草稿") && sp.contains("不得复述"),
            "正文步应基于内部草稿扩写且禁止复述: {sp}"
        );
        assert!(
            !sp.contains("先在草稿区撰写"),
            "草稿已独立成步,正文步不应再要求先写计划: {sp}"
        );
        assert!(
            sp.contains("function calling"),
            "正文步应提 function calling: {sp}"
        );
        assert!(
            sp.contains("role") && sp.contains("search"),
            "正文步应含工具名: {sp}"
        );
        // 反思步:reflect 不带 system_prompt
        assert_eq!(p.steps[2].action, "reflect");
        assert!(p.steps[2].system_prompt.is_none());
        // 归档步:白名单含 worldbook_update,提示词含蓝/绿灯规则
        let ar = &p.steps[3];
        assert_eq!(ar.action, "archive");
        let ap = ar.system_prompt.as_deref().unwrap_or("");
        assert!(
            ap.contains("大纲.md") && ap.contains("人物关系.md"),
            "归档步应点名两份 md: {ap}"
        );
        assert!(
            ap.contains("worldbook_update"),
            "归档步应提词条同步工具: {ap}"
        );
        assert!(
            ap.contains("蓝灯") && ap.contains("绿灯"),
            "归档步应带蓝/绿灯规则: {ap}"
        );
    }

    #[test]
    fn test_deep_plan_step_prompts() {
        let p = make_plan("你好", "deep");
        assert_eq!(p.steps.len(), 4);
        assert_eq!(p.steps[0].action, "draft");
        assert!(
            p.steps[0]
                .system_prompt
                .as_deref()
                .unwrap_or("")
                .contains("200 字"),
            "deep 草稿步应限定 ≤200 字"
        );
        let sp = p.steps[1].system_prompt.as_deref().unwrap_or("");
        assert!(sp.contains("内部草稿"), "deep 正文步应基于内部草稿: {sp}");
        assert!(
            !sp.contains("function calling"),
            "deep 正文步不带工具说明(主生成无工具属既有裁剪): {sp}"
        );
        assert_eq!(p.steps[1].generates, Some(true));
        assert_eq!(p.steps[2].action, "reflect");
        assert!(p.steps[2].system_prompt.is_none());
        assert_eq!(p.steps[3].action, "archive");
    }

    #[test]
    fn test_looks_like_calculation() {
        assert!(looks_like_calculation("帮我算一下 12*34"));
        assert!(looks_like_calculation("计算 1 + 2"));
        assert!(looks_like_calculation("1+2"));
        assert!(!looks_like_calculation("你好世界"));
        // 口语「算一下」无数字:不触发(避免打断角色扮演)
        assert!(!looks_like_calculation("算一下我欠你多少人情"));
        assert!(!looks_like_calculation("你算一下这事该怎么办"));
        // 口语「算一下」含数字:触发
        assert!(looks_like_calculation("帮我算一下 12*34 等于多少"));
    }

    #[test]
    fn test_extract_expression() {
        assert_eq!(extract_expression("帮我算一下 12*34"), "12*34");
        assert_eq!(extract_expression("计算 (1+2)*3"), "(1+2)*3");
        assert_eq!(extract_expression("1 + 2"), "1+2");
    }

    /// 回归:小数点不能被过滤(12.5*2 = 25,过滤后 125*2 = 250 算错)
    #[test]
    fn extract_expression_keeps_decimal_point() {
        assert_eq!(extract_expression("帮我算一下 12.5*2"), "12.5*2");
        assert_eq!(extract_expression("计算 3.14*4"), "3.14*4");
        assert_eq!(extract_expression("0.5+0.25"), "0.5+0.25");
    }

    #[test]
    fn custom_plan_filters_disabled_steps() {
        let steps = vec![
            PlanStep {
                id: "a".into(),
                name: "理解".into(),
                enabled: false,
                goal: "理解意图".into(),
                action: "direct".into(),
                generates: Some(false),
                ..Default::default()
            },
            PlanStep {
                id: "b".into(),
                name: "生成".into(),
                enabled: true,
                goal: "生成正文".into(),
                action: "direct".into(),
                generates: Some(true),
                ..Default::default()
            },
            PlanStep {
                id: "c".into(),
                name: "反思".into(),
                enabled: true,
                goal: "检查质量".into(),
                action: "reflect".into(),
                generates: None,
                ..Default::default()
            },
        ];
        let plan = make_custom_plan(&steps).unwrap();
        assert_eq!(plan.steps.len(), 2, "disabled 步骤应被过滤");
        assert_eq!(plan.steps[0].id, "b");
        assert_eq!(plan.steps[1].action, "reflect");
        assert_eq!(plan.summary, "自定义流程:共 2 步");
    }

    #[test]
    fn custom_plan_empty_rejected() {
        assert!(make_custom_plan(&[]).is_err());
        let disabled = vec![PlanStep {
            id: "a".into(),
            name: "禁用".into(),
            enabled: false,
            goal: "g".into(),
            action: "direct".into(),
            generates: Some(true),
            ..Default::default()
        }];
        assert!(make_custom_plan(&disabled).is_err());
    }

    #[test]
    fn custom_plan_missing_generating_step_rejected() {
        let steps = vec![
            PlanStep {
                id: "a".into(),
                name: "理解".into(),
                enabled: true,
                goal: "理解意图".into(),
                action: "direct".into(),
                generates: Some(false),
                ..Default::default()
            },
            PlanStep {
                id: "b".into(),
                name: "反思".into(),
                enabled: true,
                goal: "检查质量".into(),
                action: "reflect".into(),
                generates: None,
                ..Default::default()
            },
        ];
        assert!(make_custom_plan(&steps).is_err());
    }
}
