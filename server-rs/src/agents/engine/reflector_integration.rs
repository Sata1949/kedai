// 反思集成:按配置的反思提示词调用 LLM 判定草稿质量(失败回退机械规则由主流程处理),
// 以及反思未通过放弃重试时的改进建议自动生成(≤200 token,由引擎调用而非用户手填)。
use super::*;

/// 反思失败建议的输出上限(模型自行约束 ≤200 token,采样再截一刀)
pub(super) const REFLECT_ADVICE_MAX_TOKENS: u32 = 200;

/// 反思调用输出上限(RPFLOW-1:原 512 太小——推理模型(DeepSeek 系等)会把
/// max_tokens 烧在 reasoning 上,出现「HTTP 200 但判定为空」→ 解析失败 →
/// 回退机械规则(2 字符即过)。1024 给判定与短批判留出余量,
/// 另有 `generate_collect_healing` 的推理耗尽提额重试兜底。
pub(super) const REFLECT_MAX_TOKENS: u32 = 1024;

/// 反思调用的推理耗尽提额封顶(REFLECT_MAX_TOKENS × 4 = 4096):一次给足,
/// 但不无限上抬——与 executor 的截断自愈封顶同口径(`utils::retry` 自会拒绝无意义重发)。
const REFLECT_HEAL_MAX_TOKENS: u32 = REFLECT_MAX_TOKENS * 4;

/// 批判与修改的总轮数余量:首轮批判 + 每轮修改 + 最终判定轮
const CRITIQUE_EXTRA_ROUNDS: usize = 2;

/// LLM 反思:按配置的反思提示词调用模型检查草稿质量。
/// 消息 = system(反思提示词)+ user(用户输入 + 剥离变量块后的草稿);
/// 低温度 0.3、输出上限 512,判定 token 计入本轮 total_usage。
/// 返回 (判定结果, usage);调用失败、中断或输出无法解析返回 None(调用方回退机械规则,
/// 保证反思路径在模型异常时仍可判定且重试计数有界)。
pub(super) async fn reflect_with_llm(
    engine: &AgentEngine,
    prompt: &str,
    user_input: &str,
    draft: &str,
    abort: &watch::Receiver<bool>,
) -> Option<(ReflectionResult, TokenUsage)> {
    if *abort.borrow() {
        return None;
    }
    let messages = vec![
        LlmMessage::plain("system", prompt),
        LlmMessage::plain(
            "user",
            &format!("[用户输入]\n{user_input}\n\n[模型草稿]\n{draft}"),
        ),
    ];
    let params = GenerationParams {
        temperature: 0.3,
        top_p: 1.0,
        max_tokens: REFLECT_MAX_TOKENS,
        stop: None,
        tools: Vec::new(),
        max_tool_rounds: None,
        tool_choice: crate::models::types::ToolChoice::Auto,
        connection_id: None,
        parallel_tool_calls: None,
        step_budget: None,
        semantic_guard: None,
        response_format: None,
    };
    let (out, _calls, usage) = generate_collect_healing(engine, &messages, &params, abort).await?;
    parse_reflect_verdict(&out).map(|v| (v, usage))
}

/// 反思工具循环(第四点·阶段 C):反思模型带文本修正工具(censor_text / revise_passage),
/// 发现禁词或不合理段落时自主调用工具定点修正,再给出 PASS/FAIL 判定。
/// 最多 3 轮工具循环;无工具调用即解析最终判定;工具修正后的正文经返回值回传。
/// 返回 (判定, 修正后的正文 Option, usage);任何异常/中断返回 None(调用方回退机械规则,
/// 保证反思路径永远可判定、有界)。
pub(super) async fn reflect_with_tools(
    engine: &AgentEngine,
    prompt: &str,
    banned_hint: &str,
    user_input: &str,
    draft: &str,
    tool_ctx: &crate::models::types::ToolContext,
    abort: &watch::Receiver<bool>,
) -> Option<(ReflectionResult, Option<String>, TokenUsage)> {
    if *abort.borrow() {
        return None;
    }
    // 反思可用的工具:文本修正(禁词替换/定点修订)+ 只读检索(read,
    // 供反思时查世界书/资料佐证判定);仅取已注册的(缺失时退回无工具反思)。
    // 常量见 tools::tool_sets(REFLECT)。
    let mut names: Vec<&str> = crate::tools::tool_sets::REFLECT.to_vec();
    names.push("read");
    let tools: Vec<ToolDefinition> = names
        .iter()
        .filter_map(|name| engine.tool_registry.get(name).map(|t| t.definition))
        .collect();
    if tools.is_empty() {
        return reflect_with_llm(engine, prompt, user_input, draft, abort)
            .await
            .map(|(v, u)| (v, None, u));
    }
    let mut system_text = format!(
        "{prompt}\n\n若发现正文存在禁词、逻辑矛盾、表述不当或需要改写的段落,请先调用 \
         censor_text(替换禁词)或 revise_passage(定点修改段落)修正正文,再输出 PASS 或 FAIL 判定。"
    );
    // 禁词纪律(RPFLOW-2):把用户的禁词提示词/显式词表交给反思模型做同义改写,
    // 严禁空替换删词(引擎侧机械删词已废除)。
    if !banned_hint.trim().is_empty() {
        system_text.push_str(&format!(
            "\n【禁词纪律】本轮禁词约束:{}\n发现禁词必须改写为含义相近、更得体的表达,\
             严禁直接删词(直接删词会让语句不通)。",
            banned_hint.trim()
        ));
    }
    let mut messages = vec![
        LlmMessage::plain("system", &system_text),
        LlmMessage::plain(
            "user",
            &format!("[用户输入]\n{user_input}\n\n[模型草稿]\n{draft}"),
        ),
    ];
    let mut revised: Option<String> = None;
    let mut total_usage = TokenUsage::default();

    for _round in 0..3 {
        if *abort.borrow() {
            return None;
        }
        let params = GenerationParams {
            temperature: 0.3,
            top_p: 1.0,
            max_tokens: REFLECT_MAX_TOKENS,
            stop: None,
            tools: tools.clone(),
            max_tool_rounds: None,
            tool_choice: crate::models::types::ToolChoice::Auto,
            connection_id: None,
            parallel_tool_calls: None,
            step_budget: None,
            semantic_guard: None,
            response_format: None,
        };
        let (out, tool_calls, u) =
            generate_collect_healing(engine, &messages, &params, abort).await?;
        total_usage.prompt_tokens += u.prompt_tokens;
        total_usage.completion_tokens += u.completion_tokens;
        total_usage.total_tokens += u.total_tokens;
        total_usage.prompt_cache_hit_tokens += u.prompt_cache_hit_tokens;
        total_usage.prompt_cache_miss_tokens += u.prompt_cache_miss_tokens;

        if tool_calls.is_empty() {
            // 无工具调用:最终判定文本
            return parse_reflect_verdict(&out).map(|v| (v, revised, total_usage));
        }

        // 有工具调用:回填 assistant(tool_calls) 与 tool 结果,继续下一轮
        messages.push(LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            reasoning_content: None,
            tool_calls: Some(tool_calls.clone()),
            tool_call_id: None,
            images: Vec::new(),
        });
        for call in &tool_calls {
            let output = engine
                .tool_registry
                .execute(&call.name, &call.arguments, tool_ctx.clone())
                .await
                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"));
            // 只有文本修正类工具的输出才是修正后的正文(read 是检索类,输出是资料,
            // 混入会污染正文);修正类「最后一次调用胜出」。
            if crate::tools::tool_sets::REFLECT.contains(&call.name.as_str()) {
                revised = Some(output.clone());
            }
            messages.push(LlmMessage {
                role: "tool".into(),
                content: output,
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: Some(call.id.clone()),
                images: Vec::new(),
            });
        }
    }
    // 3 轮工具循环仍未给出判定:回退 None(机械规则)
    None
}

/// 批判与修改的产出(RPFLOW-1):`verdict` 为 None 表示模型未给出可解析判定
/// (交调用方按机械规则兜底);`revised` 为发生过定点修改时的**最新正文全文**。
pub(super) struct CritiqueOutcome {
    pub verdict: Option<ReflectionResult>,
    pub revised: Option<String>,
    /// 已用修改轮数(一次「本轮工具调用含至少一次有效修改」记 1)
    pub revisions: usize,
    /// 批判/判定摘要(日志与事件展示用,截断至 400 字符)
    pub summary: String,
    pub usage: TokenUsage,
}

/// 单次反思调用并聚合流(不做提额)。
/// 返回 None = 调用失败/中断(调用方回退既有路径)。
async fn generate_collect(
    engine: &AgentEngine,
    messages: &[LlmMessage],
    params: &GenerationParams,
    abort: &watch::Receiver<bool>,
) -> Option<(String, Vec<ToolCallArgs>, TokenUsage)> {
    let connector = engine.connector.read().await.clone();
    let chunks = connector
        .generate(messages, params.clone(), abort.clone())
        .await
        .ok()?;
    drop(connector);
    let mut out = String::new();
    let mut tool_calls: Vec<ToolCallArgs> = Vec::new();
    let mut usage = TokenUsage::default();
    for chunk in chunks {
        if *abort.borrow() {
            return None;
        }
        match chunk {
            LlmStreamChunk::Token(t) => out.push_str(&t),
            LlmStreamChunk::ToolCall(call) => {
                if !call.id.is_empty() && !call.name.is_empty() {
                    tool_calls.push(call);
                }
            }
            LlmStreamChunk::Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
                prompt_cache_hit_tokens,
                prompt_cache_miss_tokens,
                ..
            } => {
                usage.prompt_tokens += prompt_tokens;
                usage.completion_tokens += completion_tokens;
                usage.total_tokens += total_tokens;
                usage.prompt_cache_hit_tokens += prompt_cache_hit_tokens;
                usage.prompt_cache_miss_tokens += prompt_cache_miss_tokens;
            }
            _ => {}
        }
    }
    Some((out, tool_calls, usage))
}

/// 反思单次调用 + 「推理耗尽预算」提额重试(RPFLOW-1,镜像 executor::heal_budget_for
/// 的判定口径):正文为空、无工具调用、但已消耗 completion token(推理吃光预算)时,
/// 输出上限翻倍(不低于 REFLECT_MAX_TOKENS)重试一次;两次 usage 均计入。
async fn generate_collect_healing(
    engine: &AgentEngine,
    messages: &[LlmMessage],
    params: &GenerationParams,
    abort: &watch::Receiver<bool>,
) -> Option<(String, Vec<ToolCallArgs>, TokenUsage)> {
    let (out, calls, usage) = generate_collect(engine, messages, params, abort).await?;
    if !out.trim().is_empty() || !calls.is_empty() || usage.completion_tokens == 0 {
        return Some((out, calls, usage));
    }
    // 提额目标走全仓共享的「翻倍 + 下限 + 封顶」算法(utils::retry,四路合一),
    // 不再自写 `×2`;None = 已达封顶,重发无意义 → 直接采纳首轮结果。
    let Some(next_budget) = crate::utils::retry::heal_budget_with_floor(
        params.max_tokens,
        REFLECT_HEAL_MAX_TOKENS,
        REFLECT_MAX_TOKENS,
    ) else {
        return Some((out, calls, usage));
    };
    let mut retry_params = params.clone();
    retry_params.max_tokens = next_budget;
    let (out2, calls2, u2) = generate_collect(engine, messages, &retry_params, abort).await?;
    let mut total = usage;
    total.prompt_tokens += u2.prompt_tokens;
    total.completion_tokens += u2.completion_tokens;
    total.total_tokens += u2.total_tokens;
    total.prompt_cache_hit_tokens += u2.prompt_cache_hit_tokens;
    total.prompt_cache_miss_tokens += u2.prompt_cache_miss_tokens;
    Some((out2, calls2, total))
}

/// 覆写工具调用参数中的 `text` 字段为**当前正文**(RPFLOW-1):模型在多轮修改中
/// 可能仍在引用首轮草稿版本,直接执行会把之后几次修改作废。参数非对象或缺少
/// `text` 字段时原样返回(read 等工具不受影响)。
fn override_text_arg(arguments: &str, current_text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(serde_json::Value::Object(mut map)) if map.contains_key("text") => {
            map.insert(
                "text".to_string(),
                serde_json::Value::String(current_text.to_string()),
            );
            serde_json::Value::Object(map).to_string()
        }
        _ => arguments.to_string(),
    }
}

/// 强制批判 + 定点修改(deep/agent 反思步,RPFLOW-1):每轮让模型先逐项批判正文,
/// 发现问题**必须**调用工具(revise_passage / censor_text)做最小必要修改;
/// 修改后回填工具结果继续复查,直到判定 PASS/通过,或修改轮数达到上限
/// (deep 2 / agent 4)。达上限后不下发工具、注入「只给判定」说明。
///
/// 工具 `text` 参数由引擎覆写为当前正文(见 `override_text_arg`);修改工具输出与
/// 当前正文完全一致(fragment 未命中)时不记修改轮,并回灌错误提示。
///
/// 返回 None = 硬失败(工具不可用/调用失败/中断),调用方回退既有二轨;
/// 返回 Some 但 `verdict` 为 None = 模型未给判定,调用方按机械规则兜底(revised 仍生效)。
#[allow(clippy::too_many_arguments)]
pub(super) async fn critique_and_revise(
    engine: &AgentEngine,
    prompt: &str,
    banned_hint: &str,
    user_input: &str,
    draft: &str,
    tool_ctx: &crate::models::types::ToolContext,
    max_revisions: usize,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
) -> Option<CritiqueOutcome> {
    if *abort.borrow() {
        return None;
    }
    // 反思可用工具:文本修正(禁词替换/定点修订)+ 只读检索(read,供查世界书佐证)
    let mut names: Vec<&str> = crate::tools::tool_sets::REFLECT.to_vec();
    names.push("read");
    let tools: Vec<ToolDefinition> = names
        .iter()
        .filter_map(|name| engine.tool_registry.get(name).map(|t| t.definition))
        .collect();
    if tools.is_empty() {
        return None;
    }

    let mut system = String::from(prompt);
    system.push_str(&format!(
        "\n\n【反思纪律(本轮强制)】先逐项批判正文:需求符合度 / 人设与世界观一致 / 前后连贯 / \
         禁词 / 语言质量 / 输出纪律。发现任何不足时,必须调用工具做最小必要的定点修改\
         (revise_passage 修改段落、censor_text 做同义词替换),不要只报告问题、不要整段重写。\
         最多允许 {max_revisions} 次修改;用尽后即使仍有小瑕疵也应给出最终判定。\
         修改要求优先于提示词或上文任何「只判定不重写」的旧要求。\
         每轮回复最后单独一行输出判定:PASS=已达标可采纳;FAIL=仍有未改完的问题。"
    ));
    if !banned_hint.trim().is_empty() {
        system.push_str(&format!(
            "\n【禁词纪律】本轮禁词约束:{}\n发现禁词必须用工具改写为含义相近、更得体的表达,\
             严禁直接删词(直接删词会让语句不通)。",
            banned_hint.trim()
        ));
    }

    let mut messages = vec![
        LlmMessage::plain("system", &system),
        LlmMessage::plain(
            "user",
            &format!("[用户输入]\n{user_input}\n\n[模型草稿]\n{draft}"),
        ),
    ];
    let mut current_text = draft.to_string();
    let mut revised: Option<String> = None;
    let mut revisions = 0usize;
    let mut total_usage = TokenUsage::default();
    let mut last_summary = String::new();
    // 轮数硬上限:修改轮 + 首轮批判 + 最终判定各留余量(有界性不依赖模型自觉)
    let max_rounds = max_revisions + CRITIQUE_EXTRA_ROUNDS;

    for round in 0..max_rounds {
        if *abort.borrow() {
            return None;
        }
        // 修改额度用尽的收尾轮:不再下发工具,只取最终判定
        let final_only = revisions >= max_revisions;
        let params = GenerationParams {
            temperature: 0.3,
            top_p: 1.0,
            max_tokens: REFLECT_MAX_TOKENS,
            stop: None,
            tools: if final_only {
                Vec::new()
            } else {
                tools.clone()
            },
            max_tool_rounds: None,
            tool_choice: crate::models::types::ToolChoice::Auto,
            connection_id: None,
            parallel_tool_calls: None,
            step_budget: None,
            semantic_guard: None,
            response_format: None,
        };
        let (out, tool_calls, u) =
            generate_collect_healing(engine, &messages, &params, abort).await?;
        total_usage.prompt_tokens += u.prompt_tokens;
        total_usage.completion_tokens += u.completion_tokens;
        total_usage.total_tokens += u.total_tokens;
        total_usage.prompt_cache_hit_tokens += u.prompt_cache_hit_tokens;
        total_usage.prompt_cache_miss_tokens += u.prompt_cache_miss_tokens;
        last_summary = out.trim().to_string();

        // 执行本轮工具调用:修改工具的 text 参数覆写为当前正文,并**在本轮内逐次立即应用**
        // ——同轮多个修改调用按顺序作用在最新正文上(后一个修改看到前一个的结果,
        // 回灌给模型的 tool 结果与实际正文一致);修改次数逐次计数,额度在轮内用尽即
        // 拒绝后续修改。旧实现把本轮所有修改收集到轮末只取最后一条:同轮多次修改会
        // 静默丢弃前面的结果,而回灌的 tool 结果又声称它们都生效(审查发现的缺陷)。
        let mut modified_this_round = false;
        if !tool_calls.is_empty() {
            messages.push(LlmMessage {
                role: "assistant".into(),
                content: out.clone(),
                reasoning_content: None,
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
                images: Vec::new(),
            });
            for call in &tool_calls {
                let is_modify = crate::tools::tool_sets::REFLECT.contains(&call.name.as_str());
                let output = if is_modify && (final_only || revisions >= max_revisions) {
                    "{\"error\":\"修改次数已达上限,请直接输出最终判定(PASS/FAIL)\"}".to_string()
                } else if is_modify {
                    let args = override_text_arg(&call.arguments, &current_text);
                    match engine
                        .tool_registry
                        .execute(&call.name, &args, tool_ctx.clone())
                        .await
                    {
                        Ok(o) if o.trim_start().starts_with("{\"error\"") => o,
                        // 修改工具未产生任何变化(如 find 未命中,revise_passage 原样返回):
                        // 不算一次有效修改,回灌提示让模型基于最新正文重试
                        Ok(o) if o.trim() == current_text.trim() => {
                            "{\"error\":\"未发生任何替换(find 未在当前正文中匹配),\
                             请基于最新正文重试或改用 censor_text\"}"
                                .to_string()
                        }
                        Ok(o) => {
                            revisions += 1;
                            modified_this_round = true;
                            current_text = o.trim().to_string();
                            revised = Some(current_text.clone());
                            // 每一步修改都在推理链可见(反思不再是「一闪而过的通过」)
                            let _ = send_event(
                                step_evt(
                                    "批判与修改",
                                    Some(format!("第 {revisions}/{max_revisions} 次定点修改")),
                                    None,
                                    None,
                                ),
                                tx,
                                abort,
                                flag,
                            )
                            .await;
                            o
                        }
                        Err(e) => format!("{{\"error\":\"{e}\"}}"),
                    }
                } else {
                    engine
                        .tool_registry
                        .execute(&call.name, &call.arguments, tool_ctx.clone())
                        .await
                        .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
                };
                messages.push(LlmMessage {
                    role: "tool".into(),
                    content: output,
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: Some(call.id.clone()),
                    images: Vec::new(),
                });
            }
        }

        // 判定:达标 / 额度用尽 / 轮数用尽 → 结束;FAIL 且仍有额度却没动手 → 逼一轮修改。
        match parse_reflect_verdict(&out) {
            Some(v) => {
                if v.passed || revisions >= max_revisions || round + 1 >= max_rounds {
                    return Some(CritiqueOutcome {
                        verdict: Some(v),
                        revised,
                        revisions,
                        summary: truncate_str(&last_summary, 400),
                        usage: total_usage,
                    });
                }
                if !modified_this_round {
                    messages.push(LlmMessage::plain("assistant", &out));
                    messages.push(LlmMessage::plain(
                        "user",
                        "【必须修改】你判定为 FAIL。请立即调用 revise_passage 或 censor_text \
                         对正文做定点修改,不要只报告问题;若确实无问题可改,请直接输出 PASS。",
                    ));
                }
            }
            None => {
                if revisions >= max_revisions || round + 1 >= max_rounds {
                    return Some(CritiqueOutcome {
                        verdict: None,
                        revised,
                        revisions,
                        summary: truncate_str(&last_summary, 400),
                        usage: total_usage,
                    });
                }
                if !modified_this_round {
                    messages.push(LlmMessage::plain("assistant", &out));
                    messages.push(LlmMessage::plain(
                        "user",
                        "请按反思纪律执行:发现不足先调用工具定点修改(revise_passage / \
                         censor_text),并在最后单独一行输出判定:PASS(已达标)或 FAIL(仍需修改)。",
                    ));
                }
            }
        }
    }
    // 轮数用尽(理论不可达:轮内已有提前返回,此处兜底)
    Some(CritiqueOutcome {
        verdict: None,
        revised,
        revisions,
        summary: truncate_str(&last_summary, 400),
        usage: total_usage,
    })
}

/// 生成反思失败改进建议(自动注入,内容不由用户决定):
/// 引擎以反思判定的失败原因 + 用户输入 + 草稿摘要为上下文,调用模型产出针对性
/// 改进建议。输出上限 REFLECT_ADVICE_MAX_TOKENS(≤200 token),低温度保证稳定;
/// 硬截断到 300 字符兜底(长模型超限时也不注入过大建议)。
/// 返回 (建议文本, usage);调用失败、中断或输出为空返回 None(建议为可选增强,
/// 缺失时主流程仅使用可选的用户补充说明,不阻塞流程)。
pub(super) async fn generate_reflect_advice(
    engine: &AgentEngine,
    reason: &str,
    user_input: &str,
    draft: &str,
    abort: &watch::Receiver<bool>,
) -> Option<(String, TokenUsage)> {
    if *abort.borrow() {
        return None;
    }
    let messages = vec![
        LlmMessage::plain(
            "system",
            "你是写作质量改进顾问。依据反思反馈与草稿,给出一段简短、具体的改进建议\
             (不超过 200 token):指出问题所在并给出可操作的重写方向。\
             只输出建议正文,不要编号、不要复述原文、不要额外解释。",
        ),
        LlmMessage::plain(
            "user",
            &format!(
                "[反思反馈]\n{}\n\n[用户输入]\n{}\n\n[模型草稿(可能截断)]\n{}",
                reason,
                user_input,
                truncate_str(draft, 3000)
            ),
        ),
    ];
    let params = GenerationParams {
        temperature: 0.3,
        top_p: 1.0,
        max_tokens: REFLECT_ADVICE_MAX_TOKENS,
        stop: None,
        tools: Vec::new(),
        max_tool_rounds: None,
        tool_choice: crate::models::types::ToolChoice::Auto,
        connection_id: None,
        parallel_tool_calls: None,
        step_budget: None,
        semantic_guard: None,
        response_format: None,
    };
    let connector = engine.connector.read().await.clone();
    let chunks = connector
        .generate(&messages, params, abort.clone())
        .await
        .ok()?;
    drop(connector);
    let mut out = String::new();
    let mut usage = TokenUsage::default();
    for chunk in chunks {
        if *abort.borrow() {
            return None;
        }
        match chunk {
            LlmStreamChunk::Token(t) => out.push_str(&t),
            LlmStreamChunk::Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
                prompt_cache_hit_tokens,
                prompt_cache_miss_tokens,
                ..
            } => {
                usage.prompt_tokens += prompt_tokens;
                usage.completion_tokens += completion_tokens;
                usage.total_tokens += total_tokens;
                usage.prompt_cache_hit_tokens += prompt_cache_hit_tokens;
                usage.prompt_cache_miss_tokens += prompt_cache_miss_tokens;
            }
            _ => {}
        }
    }
    let text = out.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some((truncate_str(&text, 300), usage))
    }
}

/// 按字符数截断(字符边界安全,不截断多字节字符)
fn truncate_str(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        trimmed.chars().take(max_chars).collect()
    }
}

// ===== 反思失败建议组装(自 engine/mod.rs 拆分迁入,纯代码移动,逻辑不变)=====
// 原为 engine/mod.rs 私有函数,此处为 pub(super)(= 对 engine 可见),范围一致。
/// 生成反思失败建议并拼为注入文本(位置0 内容,自动而非用户决定):
/// 调用 LLM 产出 ≤200 token 的针对性改进建议(失败/空则仅保留用户补充说明),
/// 可选的用户补充说明附加在其后;建议与补充均为空时返回 None(不注入)。
/// 生成产生的 usage 累加进 total_usage。注入边(user/assistant)由构建期
/// reflect_advice_role 决定,与本函数无关。
pub(super) async fn build_reflect_advice(
    engine: &AgentEngine,
    reason: &str,
    user_input: &str,
    draft: &str,
    supplement: &str,
    abort: &watch::Receiver<bool>,
    total_usage: &mut TokenUsage,
) -> Option<String> {
    let mut text = String::new();
    if let Some((advice, u)) =
        generate_reflect_advice(engine, reason, user_input, draft, abort).await
    {
        total_usage.prompt_tokens += u.prompt_tokens;
        total_usage.completion_tokens += u.completion_tokens;
        total_usage.total_tokens += u.total_tokens;
        total_usage.prompt_cache_hit_tokens += u.prompt_cache_hit_tokens;
        total_usage.prompt_cache_miss_tokens += u.prompt_cache_miss_tokens;
        text.push_str("[反思反馈]\n");
        text.push_str(advice.trim());
    }
    let supplement = supplement.trim();
    if !supplement.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n[补充要求]\n");
        }
        text.push_str(supplement);
    }
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RPFLOW-1 工具参数覆写:只替换 `text` 字段(修改工具以当前正文为基准),
    /// 缺 `text` 字段(如 read)与非 JSON 参数原样返回。
    #[test]
    fn override_text_arg_replaces_only_text_field() {
        let out = override_text_arg(r#"{"find":"a","replace":"b","text":"old"}"#, "NEW");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], "NEW");
        assert_eq!(v["find"], "a");
        assert_eq!(v["replace"], "b");

        // 无 text 字段(read 等检索工具)原样返回
        let raw = r#"{"type":"world_book"}"#;
        assert_eq!(override_text_arg(raw, "NEW"), raw);
        // 非 JSON 原样返回(交给工具自身报错)
        assert_eq!(override_text_arg("not-json", "NEW"), "not-json");
        // 含 text 的数组/非对象形态原样返回
        assert_eq!(override_text_arg("[1,2]", "NEW"), "[1,2]");
    }
}
