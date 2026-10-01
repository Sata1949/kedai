// 任务模式三层固定提示词(单一来源,批次 B.3 依赖倒置):
// 规划器/执行者/汇总者内置指令,执行器与设置预览共用同一份文本,改动只改这里,预览即真实下发。
// 自 task_service/prompt.rs 机械搬迁至此,使 task_engine 只依赖 task_core;内置指令不经
// untrusted 包裹(docs/契约-协议与配置.md 第三节)。

/// 规划器内置指令:目标 → JSON 步骤数组(严格 JSON,低温保证结构稳定)。
/// 问题②(2026-08-31 实测:模型只写计划不收集信息,凭空编造步骤):允许先用
/// 只读工具侦察(白名单/轮数上限由执行侧 PLANNER_SCOUT_* 保证),再产出计划;
/// 「严格只输出 JSON 数组」约束的是最终计划轮的产出形态,侦察轮的工具调用不算违约。
/// TM-SCOUT-1(2026-10-01 复现取证后)把侦察句由「可先」改强为「应先用…不要凭空编造」——
/// 侦察仍可能不发生(软机制),信息在场由系统侧预取快照保证(装配点见 `scout_snapshot`)。
pub(crate) const PLANNER_PROMPT: &str = "你是任务规划器。把用户目标拆解为 2~5 个可独立执行的具体步骤,每个步骤单一、明确、粒度适中(约 2~5 分钟可完成)。若目标涉及的信息不足,应先用可用的只读工具(如读取文件/搜索/查询记忆)查看真实文件与信息,再基于事实产出计划,不要凭空编造。计划须严格只输出 JSON 数组,不要输出任何解释或多余文字。数组元素格式:{\"name\":\"步骤名\",\"goal\":\"该步骤要完成的目标\"}。契约先行纪律:① 每个步骤的 goal 必须写明交付物与可判是的验收判据(能以是/否回答);② 同一交付物只允许一个权威版本,不得规划出会互相覆盖的重复步骤;③ 字段名与口径以本计划为唯一来源,执行方不得自拟字段名。";

/// 执行者内置指令:独立完成单个子任务并直接产出结果
pub(crate) const EXECUTOR_PROMPT: &str = "你是任务执行者,负责独立完成交给你的一个子任务。直接输出该子任务的最终结果:不要复述指令、不要输出计划或元文本、不要模拟对话、不要用标题包裹结果。引用或替换其他 agent 的既有产出时,必须写明取代对象(名称或版本);禁止出现未声明取代对象的「替换旧版/最新版为唯一口径」这类表述。若你的职责是验证/审计,只回填实测结论,不得另立一版交付物。交付物正文必须完整给出,写入文件不替代正文。";

/// 规划器修订指引段(批次 R2b plan-chat):追加在 PLANNER_PROMPT 之后,
/// 引导规划器按用户反馈修订已产出计划(输出契约不变:严格 JSON 数组)。
/// 文本含独特词「计划修订指引」:mock 测试据此经 [[reply_if:]] 区分首轮规划
/// 与修订轮(首轮 system 仅 PLANNER_PROMPT,修订轮才携带本段)。
pub(crate) const PLANNER_REVISE_GUIDANCE: &str = "## 计划修订指引\n你正在修订一份已产出的计划。用户会提供原始目标、当前计划(JSON)、与你就该计划的对话记录以及本轮反馈。请按本轮反馈修订计划:未受反馈影响的步骤尽量保持不变;输出契约不变——严格只输出 JSON 数组,不要输出任何解释或多余文字。";

/// 汇总者内置指令:综合各步骤结果产出最终成果
pub(crate) const SUMMARIZER_PROMPT: &str = "你是任务汇总者。下面是用户目标、执行计划与各步骤结果。请输出一份完整、有条理的最终成果,直接呈现结果本身(不要写「汇总如下」「以下是」等元文本)。";

/// team 模式规划器内置指令:目标 → 主 agent 分工拓扑 JSON(批次 4.3b)。
/// 严格 JSON 单一对象:mains 2~4 个主 agent,每主 1~4 个子目标,全局 ≤15 个
///(4×4=16 可超,全局封顶由解析侧截断保证);低温保证结构稳定(与 PLANNER_PROMPT
/// 同口径)。内置指令不经 untrusted 包裹。
pub(crate) const TEAM_PLANNER_PROMPT: &str = "你是团队规划器。把用户目标拆解并归并为 2~4 个主 agent 的分工拓扑:每个主 agent 领 1~4 个子目标(全局子目标总数不超过 15 个),每个子目标单一、明确、可独立执行。严格只输出 JSON 对象,不要输出任何解释或多余文字。格式:{\"mains\":[{\"name\":\"主 agent 分工名\",\"goals\":[{\"name\":\"子目标名\",\"goal\":\"该子目标要完成的具体目标\"}]}]}";

/// team 模式审计员内置指令:收齐各主 agent 产出后做一致性/质量/覆盖度审查
///(批次 4.3b)。严格 JSON 输出:通过=true;发现缺漏时打回指定主 agent
///(1-based 序号;可进一步用 step 指明该主第几个子目标,避免整主重跑造成多版本并存)。
/// 打回最多发生一轮(执行器保证);无打回时该结论进入最终结果的「## 审计结论」段,
/// 有打回时由补做后的终审结论替代(前端按此拆卡)。
/// 内置指令不经 untrusted 包裹。
pub(crate) const TEAM_AUDIT_PROMPT: &str = "你是团队审计员。下面是用户目标与各主 agent 的产出(各子目标以「子目标 N「子目标名」」标注,N 即该主内第几个子目标)。请做一致性(产出之间是否矛盾)、质量(是否达到目标要求)与覆盖度(目标各部分是否都有产出)审查。注意区分「中间稿/草稿」与「最终交付物」:同一交付物出现多个互相矛盾的版本即视为不一致,必须在结论中指出应以哪一版为准。严格只输出 JSON 对象,不要输出任何解释或多余文字。格式:{\"通过\":true或false,\"打回\":[{\"main\":主agent序号(从1开始),\"step\":该主第几个子目标(从1开始,必须与产出里的「子目标 N」编号一致;可省略表示整个主agent),\"instruction\":\"补做指令\"}],\"结论\":\"审查结论文本\"}。仅当确有缺漏且补做可修复时才打回,并尽量定位到具体子目标而非整个主 agent;无问题或问题不可经补做修复时通过=true、打回=[]。";

/// team 模式终审员内置指令:打回补做完成后的最终审查(2026-08 实测修复;
/// 实跑问题 2 改为结构化 JSON,使终审结论能作为终态闸门——旧实现只出自由文本,
/// 无法机器判定,审计不过也能落 done)。
/// 只产出 JSON(不再打回);终审结论进入最终结果的「## 审计结论」段,替代首次审计的
/// 打回原文;通过=false 时任务终态为 partial(不再静默 done)。内置指令不经 untrusted 包裹。
pub(crate) const TEAM_FINAL_AUDIT_PROMPT: &str = "你是团队终审员。下面是用户目标、各主 agent 补做后的最终产出与首轮审计意见。请基于最终产出做最终裁定:目标各部分是否已覆盖、质量是否达标、产出之间是否一致、首轮审计指出的问题是否已解决,以及是否仍存在同一交付物的多个矛盾版本。严格只输出 JSON 对象,不要输出任何解释或多余文字。格式:{\"通过\":true或false,\"结论\":\"终审结论文本\"}。只要仍存在未解决的缺漏、质量问题或产出间矛盾,通过=false 并在结论中说明。";

/// custom 模式反思步骤内置指令:检查上一版产出并输出判定结论(批次 4.3b)。
/// reflect 步骤按契约不携带用户 system_prompt(validate_flow 强制),统一用本内置指令。
pub(crate) const CUSTOM_REFLECT_PROMPT: &str = "你是反思审查员。下面是用户目标与上一版产出。检查:目标要求是否全部满足、有无事实/逻辑错误、是否被截断、有无复述指令或元文本。直接输出审查结论:无问题时输出 PASS 并附一句通过理由;有问题时输出 FAIL 并逐条列出需修复的问题。";

/// custom 模式内部规划步骤内置指令(generates=false 的 direct 步骤,如「理解意图」)。
/// 背景(2026-09-10 六模式实测):此类步骤此前复用 EXECUTOR_PROMPT「直接输出最终结果」,
/// 导致「只做内部规划」的步骤实际吐出完整正文,并被下一步原样回显。
/// 本指令明确「只做分析规划、产出要点、不产出面向用户的正文」,与步骤自身语义一致;
/// 其产出仅作后续步骤的参考上下文,不计入最终成果。内置指令不经 untrusted 包裹。
pub(crate) const TASK_INTERNAL_PLAN_PROMPT: &str = "你是任务内部规划者。请针对下面给出的目标做内部分析与规划:提炼关键要求、约束、需要参考的信息与执行要点,产出简洁的要点清单供后续步骤使用。只输出分析规划要点本身,不要输出面向用户的最终正文、不要模拟对话、不要用标题包裹结果。";

/// 执行者的**工具使用纪律**段(提交 3 · D3),仅在该执行者**本轮有工具**时追加
/// (legacy 的步骤没有工具,不追加——纪律讲的是怎么用工具,不是怎么写正文)。
///
/// 为什么需要:2026-09-26 真实模型实测里 solo/编程 2 分钟就改对了代码,之后 10 条命令
/// 全是反复自检(重跑测试、`pwd`、`ls`、`git status`、`md5sum`),单轮 LLM 往返 1.5~3
/// 分钟,15 分钟不收敛——执行者指令里**没有任何收尾纪律**,模型不知道「自测通过就该收尾」。
/// 另两条来自同一轮实测:模型按 bash 语法写命令(command/`;`/`/d/` 路径,cmd 不认),
/// 以及用 `echo 重定向`拼文件而不走 `fs_*`(D4)。
///
/// 单一出处:与 `capability_note`(规划器侧「本轮可用能力」段)同族但**受众不同**
/// (一个给规划器、一个给执行者),故各自成文;HARNESS 线的「编码执行者模板」
/// (原 docs/plans/HARNESS-PLAN.md 提交 3 第 6 项,该稿已并入 docs/计划.md 的
/// 「其余临时执行稿」章 HARNESS3-6)直接引用本常量,不写第二份。内置指令不经 untrusted 包裹。
pub(crate) const EXECUTOR_TOOL_DISCIPLINE: &str = "工具使用纪律:① 自测通过即收尾——同一事实不得反复验证,不要为「再确认一次」重跑已通过的检查;② 命令用本机 shell 语法(Windows 下由 cmd 解释:多命令用 && 连接,不支持 ; 分隔与 /d/ 这类 MSYS 路径);③ 改文件优先用 fs_write/fs_edit 工具,不要用 shell 重定向拼文件;④ 每轮只做一个动作,看完结果再决定下一步。";

/// 执行者**收尾提醒**(TM-EMPTY-1,2026-10-01):工具循环正常结束但正文为空时,
/// 追加的一次「无工具收尾轮」的 user 消息——只提醒直接产出最终成果,不引入新契约;
/// 是否发起与预算/温度分级由执行侧(`solo::run_agent_loop`)判定。
/// 属动态重试辅助语:不上设置预览(与 `planner_scout_guidance` 的登记口径一致)。
pub(crate) const EXECUTOR_FINAL_NUDGE: &str =
    "请基于以上进展直接输出最终成果(不要再调用工具;不要复述过程,只给结果)。";

/// 规划器**收尾提醒**(TM-EMPTY-1,2026-10-01):侦察终轮(未下发工具)仍返回
/// tool_calls 且正文为空时,追加的一次无工具调用的 user 提醒——把「工具调用轮不算输出、
/// 不违反 JSON 契约」的既有澄清(`planner_scout_guidance`)落到具体动作上。
pub(crate) const PLANNER_FINAL_NUDGE: &str =
    "不要再调用任何工具,直接按要求输出计划 JSON(只输出 JSON 数组,不要任何解释)。";

/// 「返回空内容」的用户可见错误文案(提交 3 · D6),任务侧三处消费点共用:
/// legacy/plan/team 的步骤与汇总(`task_engine::retry`)、solo 系主循环
/// (`task_engine::solo`)、自定义流程节点(`task_engine::custom` 的落库侧)。
///
/// 为什么要统一并补诊断:实测 `deepseek/deepseek-v4.1-flash` 约 90% completion token 是
/// 推理(78/100、1835/2000),`default_max_tokens=10000` 下出现 `finish=length` 且正文为空,
/// 而旧文案只说「返回空内容(finish_reason=length)」——用户不知道该调什么(遗留 TM-D6)。
/// 本函数给出三类可操作事实:finish_reason、思考占输出的比例、当前输出上限与建议动作。
///
/// `max_tokens`:该次调用的输出上限。拿不到时传 `None`(如自定义流程节点:预算在节点内
/// 算过但未回传),此时省略该子句而不是报一个可能不准的数。
pub(crate) fn empty_output_error(
    label: &str,
    out: &super::types::TaskGenOutput,
    max_tokens: Option<u32>,
) -> String {
    let reason = out.finish_reason.as_deref().unwrap_or("未知");
    // 占比口径:completion token 里推理占了多少(实测推理模型的主因)。
    // completion_tokens == 0 = 该次未上报用量(部分上游/mock),此时明说「未知」,
    // 不报一个假的 0%(0 会被读成「没思考」,正好把诊断带反)。
    let reasoning = if out.completion_tokens > 0 {
        format!(
            "思考占输出 {}%({}/{} token)",
            out.reasoning_tokens * 100 / out.completion_tokens,
            out.reasoning_tokens,
            out.completion_tokens
        )
    } else {
        "思考占比未知(该次未上报 completion token)".to_string()
    };
    let cap = max_tokens
        .map(|m| format!(",当前输出上限 {m}"))
        .unwrap_or_default();
    format!(
        "{label}返回空内容(finish_reason={reason};{reasoning}{cap})。\
         若反复出现:提高单次生成上限或改用非推理模型"
    )
}

/// 执行阶段的工具面事实(D1 修复):规划器必须知道执行者能做什么。
///
/// 为什么要有这个类型而不是让调用方传 bool:两个变体的语义差得远(一个说「只能出正文」、
/// 一个说「有文件工具与命令工具、文件根在哪」),裸 bool 在调用点读不出含义,而这条提示
/// 直接决定规划器会不会规划出执行者做不到的步骤(实测缺陷 D1)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepCapability {
    /// 执行阶段的步骤**没有任何工具**(legacy:`generate_step_retry` → 纯文本生成)
    NoTools,
    /// 执行阶段的步骤走工具循环(plan/team/custom:`run_agent_loop`),工具面由任务策略编译
    ToolLoop,
}

/// 「本轮可用能力(执行阶段)」提示段(D1)。
///
/// 为什么必须告诉规划器:2026-09 实测里 plan/写作的规划器把交付物规划成
/// `constraints.md`/`draft.md`/`check.md`,而 legacy 的步骤根本没有工具——模型只能
/// `echo 中文 > a.txt`,cmd 按 ANSI 落盘后读回是乱码,于是换 `chcp`、换 python、
/// 换 `-X utf8` 无限重试:一轮 77 条命令、15 分钟不收敛,还在用户数据目录留下 12 个
/// 垃圾文件。规划器不知道执行者能做什么,就必然规划出做不到的步骤。
///
/// `allowed`:执行阶段实际下发的工具名(任务策略编译结果);`workspace`:执行侧文件根
/// (绑定工作区或任务 scratch)。本段是内置指令,不经 untrusted 包裹。
pub(crate) fn capability_note(
    capability: StepCapability,
    allowed: &[String],
    workspace: Option<&std::path::Path>,
) -> String {
    let mut s = String::from("【本轮可用能力(执行阶段)】");
    match capability {
        StepCapability::NoTools => {
            s.push_str(
                "\n执行者没有任何文件或命令工具:不能读写文件、不能执行命令。每一步的产出都必须是\
                 正文文本,禁止把交付物规划成文件(如 constraints.md / draft.md / 大纲.md)——\
                 没有工具去创建它,那一步只会空转或失败。步骤 goal 请写清「产出哪段正文、\
                 以什么判据验收」。",
            );
        }
        StepCapability::ToolLoop => {
            let has_fs = allowed
                .iter()
                .any(|n| n == crate::tools::agent_tools_fs::READ_TOOL);
            let has_shell = allowed.iter().any(|n| n == crate::tools::bash::TOOL_NAME);
            if has_fs {
                let names = [
                    crate::tools::agent_tools_fs::READ_TOOL,
                    crate::tools::agent_tools_fs::WRITE_TOOL,
                    crate::tools::agent_tools_fs::EDIT_TOOL,
                    crate::tools::agent_tools_fs::GLOB_TOOL,
                    crate::tools::agent_tools_fs::GREP_TOOL,
                ]
                .join("/");
                s.push_str(&format!("\n执行者可以读写文件(工具 {names})。"));
                match workspace {
                    Some(w) => s.push_str(&format!(
                        "文件根目录:{}(相对路径都按此根解析,越出根会被拒绝)。",
                        w.display()
                    )),
                    None => s.push_str("但本轮未绑定文件根,步骤不得依赖文件读写。"),
                }
            } else {
                s.push_str("\n执行者没有文件工具:不能读写文件,交付物必须写成正文。");
            }
            if has_shell {
                s.push_str(
                    "另有命令工具 bash(Windows 下由 cmd 解释:不支持 `;` 分隔多条命令与 /d/ \
                     这类 MSYS 路径,多命令用 `&&` 连接)。",
                );
            } else {
                s.push_str("执行者不能执行命令,步骤不得依赖运行命令。");
            }
            s.push_str("交付物正文必须由执行者直接产出:文件只是过程物,不得只把成果留在文件里。");
        }
    }
    s
}

/// 规划器「侦察指引」条件段(TM-SCOUT-1 D1(a)):仅在侦察轮**确有可用只读工具**时
/// 追加(有无工具的判据在装配侧——`defs` 为空的裁剪环境不注;指引讲的是怎么用工具)。
///
/// 为什么需要:复现取证(2026-10-01)证实侦察是**模型主动请求才发生**的软机制,且
/// 「严格只输出 JSON」与「可先侦察」在同轮竞争——需要明确告诉规划器「工具调用轮不算
/// 最终输出」,把动作门槛从「可能违约」降下来;并给出建议首动作(先列根目录、读入口
/// 文档),提高主动深入的配合度。受众与 `capability_note` 区分:那段讲**执行者**能力面,
/// 本段讲**规划器自己**怎么先侦察。内置指令,不经 untrusted 包裹。
pub(crate) fn planner_scout_guidance(allowed: &[String], workspace: &std::path::Path) -> String {
    format!(
        "【侦察指引】本轮你可以直接调用只读工具({tools})查看工作区真实文件,工作区根目录: \
         {root}。工具调用轮不算最终输出、不违反「只输出 JSON」契约——先把信息看清,最终轮再交出 \
         JSON 计划即可。建议首动作:先列出根目录与关键子目录,再读取入口文档(README/AGENTS.md)\
         与目标直接相关的文件,基于真实内容拆解步骤。",
        tools = allowed.join("/"),
        root = workspace.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 能力段的两套文案(D1):legacy(无工具)必须明说「不要规划成文件」;
    /// 有工具时必须给出文件根与唯一出处纪律。文案本身是防回退断言——
    /// 这两句一旦被后续改动抹平,缺陷 D1 会原样复发。
    #[test]
    fn capability_note_covers_both_executor_shapes() {
        let no_tools = capability_note(StepCapability::NoTools, &[], None);
        assert!(no_tools.contains("没有任何文件或命令工具"), "{no_tools}");
        assert!(no_tools.contains("禁止把交付物规划成文件"), "{no_tools}");

        let allowed = vec![
            crate::tools::agent_tools_fs::READ_TOOL.to_string(),
            crate::tools::agent_tools_fs::WRITE_TOOL.to_string(),
            crate::tools::bash::TOOL_NAME.to_string(),
        ];
        let ws = std::path::Path::new("C:/scratch/t1");
        let tools = capability_note(StepCapability::ToolLoop, &allowed, Some(ws));
        assert!(
            tools.contains(crate::tools::agent_tools_fs::READ_TOOL),
            "{tools}"
        );
        assert!(tools.contains("C:/scratch/t1"), "应给出文件根:{tools}");
        assert!(tools.contains("cmd"), "应说明 shell 实为 cmd:{tools}");
        assert!(tools.contains("不得只把成果留在文件里"), "{tools}");

        // 无文件工具但有 shell(策略收窄的极端情形):不得声称能读写文件
        let shell_only = capability_note(
            StepCapability::ToolLoop,
            &[crate::tools::bash::TOOL_NAME.to_string()],
            None,
        );
        assert!(shell_only.contains("没有文件工具"), "{shell_only}");
        assert!(!shell_only.contains("可以读写文件"), "{shell_only}");
    }

    /// 工具纪律段(提交 3 · D3-c):三条实测教训各留一句钉子。
    /// 文案是防回退断言——这些句子一旦被后续改动抹平,D3 会原样复发:
    /// ① 「自测通过即收尾」(10 条反复自检);② cmd 语法边界(D4);③ 优先 fs_* 而非重定向。
    #[test]
    fn executor_tool_discipline_covers_the_three_lessons() {
        for needle in ["自测通过即收尾", "cmd", "fs_write", "每轮只做一个动作"] {
            assert!(
                EXECUTOR_TOOL_DISCIPLINE.contains(needle),
                "工具纪律段应含「{needle}」: {EXECUTOR_TOOL_DISCIPLINE}"
            );
        }
    }

    /// 收尾提醒两条(TM-EMPTY-1):核心指令是「不要再调用工具」——轻改为不减指令
    /// 强度的表述;行为侧断言在 tests/tasks_modes_solo_multi.rs 与 tasks_modes_plan.rs。
    #[test]
    fn final_nudges_keep_no_tool_directive() {
        assert!(
            EXECUTOR_FINAL_NUDGE.contains("不要再调用工具"),
            "执行者收尾提醒应含停用工具指令: {EXECUTOR_FINAL_NUDGE}"
        );
        assert!(
            PLANNER_FINAL_NUDGE.contains("不要再调用任何工具")
                && PLANNER_FINAL_NUDGE.contains("计划 JSON"),
            "规划器收尾提醒应含停用工具指令与产出形态: {PLANNER_FINAL_NUDGE}"
        );
    }

    /// 侦察指引段(TM-SCOUT-1 D1(a)):四要素必须齐备——可用工具名、工作区根、
    /// 「工具调用轮不违反 JSON 契约」澄清、建议首动作。文案是防回退断言。
    #[test]
    fn planner_scout_guidance_names_tools_root_and_json_contract() {
        let tools = vec![
            "fs_glob".to_string(),
            "fs_read".to_string(),
            "fs_grep".to_string(),
        ];
        let g = planner_scout_guidance(&tools, std::path::Path::new("C:/ws/demo"));
        for needle in [
            "【侦察指引】",
            "fs_glob/fs_read/fs_grep",
            "C:/ws/demo",
            "不违反「只输出 JSON」契约",
            "建议首动作",
            "README/AGENTS.md",
        ] {
            assert!(g.contains(needle), "侦察指引应含「{needle}」: {g}");
        }
    }

    /// PLANNER_PROMPT 侦察句加固(TM-SCOUT-1 D1(a)):由「可先调用」改强为「应先用…」,
    /// 并要求「不要凭空编造」。这两句是修复取向的文本载体,被抹平即回退。
    #[test]
    fn planner_prompt_scout_sentence_strengthened() {
        assert!(
            PLANNER_PROMPT.contains("应先用可用的只读工具"),
            "侦察句应为强默认: {PLANNER_PROMPT}"
        );
        assert!(
            PLANNER_PROMPT.contains("不要凭空编造"),
            "应含反凭空编造纪律: {PLANNER_PROMPT}"
        );
        assert!(
            !PLANNER_PROMPT.contains("可先调用可用的只读工具"),
            "旧的软约束措辞不得残留: {PLANNER_PROMPT}"
        );
    }

    /// 空产出错误文案(提交 3 · D6):三类可操作事实必须齐备。
    /// 用合成用量钉住**占比口径**(mock 永远上报 0,无法端到端检验百分比)。
    #[test]
    fn empty_output_error_carries_actionable_diagnostics() {
        use crate::services::task_core::types::TaskGenOutput;
        let mk = |reason: Option<&str>, rt: i64, ct: i64| TaskGenOutput {
            text: String::new(),
            finish_reason: reason.map(str::to_string),
            prompt_tokens: 100,
            completion_tokens: ct,
            reasoning_tokens: rt,
            reasoning_chars: 0,
            tool_calls: Vec::new(),
            dropped_tool_calls: 0,
        };

        let msg = empty_output_error("子任务", &mk(Some("length"), 78, 100), Some(10_000));
        assert!(
            msg.starts_with("子任务返回空内容(finish_reason=length;"),
            "{msg}"
        );
        assert!(msg.contains("思考占输出 78%(78/100 token)"), "{msg}");
        assert!(msg.contains("当前输出上限 10000"), "{msg}");
        assert!(msg.contains("提高单次生成上限"), "{msg}");
        assert!(msg.contains("非推理模型"), "{msg}");

        // 未上报用量(completion 0):明说「未知」,不得报一个假的 0%(会把诊断带反)
        let unknown = empty_output_error("汇总", &mk(None, 0, 0), None);
        assert!(unknown.contains("思考占比未知"), "{unknown}");
        assert!(!unknown.contains("思考占输出"), "{unknown}");
        assert!(
            !unknown.contains("当前输出上限"),
            "拿不到上限时应整句省略:{unknown}"
        );
        assert!(unknown.contains("finish_reason=未知"), "{unknown}");

        // 下限兜底:思考占比不得除零(completion 为 0 走未知分支;此处钉住不 panic)
        let _ = empty_output_error("步骤", &mk(Some("stop"), 5, 0), Some(1));
    }
}
