// 主状态机循环:step_loop(规划步骤的 direct/tool/reflect 执行、反思回退重生成、
// custom 模式即时补丁应用)。自 engine/mod.rs 拆分迁入,纯代码移动,逻辑不变。
// 依赖经 `use super::*` 取自 engine/mod.rs(与 executor/mvu 等子模块同一约定)。
use super::*;

impl AgentEngine {
    /// 阶段 3「步骤循环」:按计划顺序执行每个步骤(反思判定 / 工具循环 / 生成),
    /// 维护 attempt 与反思重试计数、custom 模式变量基线、mergeUsage 累计。
    /// 返回 (content, custom_vars_snapshot);中断时提前结束循环。
    /// 对应 run_body 内「2. 执行阶段」段;L2 中层定位:计划 → 生成内容的推进循环。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn step_loop(
        &self,
        req: &AgentRunRequest,
        ctx: &CollectedCtx,
        plan: &Plan,
        state_machine: &mut StateMachine,
        agent_session: &AgentSessionRecord,
        user_input: &str,
        tool_ctx: &ToolContext,
        tx: &mpsc::Sender<SseEvent>,
        abort: &watch::Receiver<bool>,
        flag: &AbortFlag,
        session_id: &str,
        run_id: &uuid::Uuid,
        rctx: &mut RunContext<'_>,
    ) -> Result<
        (
            String,
            Option<Value>,
            Vec<crate::contracts::ChangelogEntry>,
            Vec<crate::contracts::PatchOp>,
        ),
        EngineError,
    > {
        let mut content = String::new();
        // custom 模式:每步生成后立即应用的变量树快照(收尾据此落库 extra.mvu,
        // 避免内容已剥离后无法再解析出补丁)
        let mut custom_vars_snapshot: Option<Value> = None;
        // P5:custom 模式各步即时应用产生的契约 changelog 条目与 pending(收尾统一
        // commit 到 kaleido_state/kaleido_changelog;无契约时两者恒空)
        let mut custom_contract_entries: Vec<crate::contracts::ChangelogEntry> = Vec::new();
        let mut custom_contract_pending: Vec<crate::contracts::PatchOp> = Vec::new();
        // custom 模式:各生成步骤开始前的变量树基线——反思回退重生成时恢复基线,
        // 避免 delta 等非幂等补丁被重复应用导致数值翻倍(回退步骤已应用的补丁随内容一并丢弃)
        let mut step_vars_baseline: HashMap<usize, AssistantVars> = HashMap::new();
        let mut attempt: usize = 0;
        // 反思重试独立计数(兜底防护:即使回退路径存在缺陷,反思重试也有硬上限,绝不无限循环)
        let mut reflect_retries: usize = 0;
        let max_attempts = if req.mode == "deep" || req.mode == "agent" || req.mode == "custom" {
            3
        } else {
            1
        };
        let mut tool_triggered = false;

        let mut idx = 0usize;
        // 反思重试回退时,丢弃上一个 direct 步骤里工具循环追加的中间消息
        // (可变:草稿步把下界抬到注记之后,见 draft 分支——注记必须在场)
        let mut base_len = rctx.llm_messages.len();
        while idx < plan.steps.len() {
            check_aborted(abort)?;
            let step = &plan.steps[idx];
            // 自定义流程步骤进度(SSE Step 事件的 index/total;其余模式不携带)
            let progress = if req.mode == "custom" {
                (Some(idx + 1), Some(plan.steps.len()))
            } else {
                (None, None)
            };

            if step.action == "reflect" {
                // ===== 反思阶段 =====
                // 简单模式字数要求(如「输出约 1200 字」):机械规则按此校验明显偏短,
                // 与 LLM 反思提示词(用户可配)互补;未启用时无字数约束
                let min_chars = if ctx.inject_snapshot.mode == InjectMode::Simple
                    && ctx.inject_snapshot.simple.word_count_enabled
                    && ctx.inject_snapshot.simple.word_count > 0
                {
                    Some(ctx.inject_snapshot.simple.word_count as usize)
                } else {
                    None
                };
                state_machine.transition(AgentState::Reflecting, session_id)?;
                self.agent_sessions
                    .update(&agent_session.id, Some("reflecting"), None, None, None)
                    .map_err(|e| e.to_string())?;
                send_event(
                    step_evt(
                        "反思中…",
                        Some("检查输出质量与连贯性".into()),
                        progress.0,
                        progress.1,
                    ),
                    tx,
                    abort,
                    flag,
                )
                .await?;
                // 反思判定前剥离 mvu <UpdateVariable> 块:变量补丁不参与质量检查,
                // 含补丁时视为有效输出(纯变量更新响应不再被规则 1/3 误判而白重试)。
                let (mut reflect_text, reflect_patches) = parse_update_variable(&content);
                let has_updates = !reflect_patches.is_empty();
                // 反思双轨(RPFLOW-1 扩展为三档):deep/agent → 强制批判 + 定点修改
                // (上限 deep 2 / agent 4,critique_and_revise);custom/其余 → 既有
                // 「LLM 判定或机械规则」二轨。禁词的机械空替换删除已废除(观感不自然,
                // 同义改写由反思模型经 revise_passage 承担,纪律段见 critique_and_revise)。
                let reflect_tool_ctx = ToolContext {
                    session_id: session_id.to_string(),
                    character_id: req.character_id.clone(),
                    agent_depth: 0,
                    // 反思阶段的文本修正工具不触达文件系统,无工作区语义
                    scope: None,
                    budget: None,
                };
                // deep/agent 的修改轮上限(RPFLOW-1):deep 2 次、agent 4 次;
                // custom 与其余模式保持既有二轨(custom 流程语义冻结)。
                let revise_cap = match req.mode.as_str() {
                    "deep" => Some(2usize),
                    "agent" => Some(4usize),
                    _ => None,
                };
                // 禁词纪律素材(RPFLOW-2):禁词库启用时把用户的提示词(或显式词表)交给反思
                // 模型,由它在上下文中用 revise_passage/censor_text 做同义改写;引擎不再做
                // 任何机械空替换删除(旧实现即实测投诉的「暴力去除」)。
                let banned_hint = ctx.inject_snapshot.simple.banned_words_hint();
                // 达修改上限后采纳(不再整篇重生成)的标记与已用修改轮数
                let mut accept_capped = false;
                let mut revisions_used = 0usize;
                // 批判摘要(事件展示用;无 LLM 批判时为空串)
                let mut critique_summary = String::new();
                let verdict = if let Some(cap) = revise_cap {
                    if !ctx.reflect_prompt.trim().is_empty() && !reflect_text.trim().is_empty() {
                        let mut llm_verdict = None;
                        if let Some(mut outcome) = critique_and_revise(
                            self,
                            &ctx.reflect_prompt,
                            &banned_hint,
                            user_input,
                            &reflect_text,
                            &reflect_tool_ctx,
                            cap,
                            tx,
                            abort,
                            flag,
                        )
                        .await
                        {
                            rctx.total_usage.prompt_tokens += outcome.usage.prompt_tokens;
                            rctx.total_usage.completion_tokens += outcome.usage.completion_tokens;
                            rctx.total_usage.total_tokens += outcome.usage.total_tokens;
                            rctx.total_usage.prompt_cache_hit_tokens +=
                                outcome.usage.prompt_cache_hit_tokens;
                            rctx.total_usage.prompt_cache_miss_tokens +=
                                outcome.usage.prompt_cache_miss_tokens;
                            revisions_used = outcome.revisions;
                            critique_summary = outcome.summary.clone();
                            // 模型定点修改的正文写回(保留 <UpdateVariable> 补丁块,
                            // 与既有修正路径 rebuild_content_keeping_blocks 同语义)
                            if let Some(revised_text) = outcome.revised.take() {
                                let trimmed = revised_text.trim().to_string();
                                if !trimmed.is_empty() && trimmed != reflect_text {
                                    content = rebuild_content_keeping_blocks(&content, &trimmed);
                                    reflect_text = trimmed;
                                    send_event(
                                        step_evt(
                                            "反思修正",
                                            Some(format!("已定点修改 {} 次", outcome.revisions)),
                                            progress.0,
                                            progress.1,
                                        ),
                                        tx,
                                        abort,
                                        flag,
                                    )
                                    .await?;
                                }
                            }
                            llm_verdict = outcome.verdict;
                        }
                        // 机械安全闸:结构性问题(空/截断/字数/未答疑问)仍走重生成兜底;
                        // 机械通过后按 LLM 判定:达标 → 通过;达上限仍 FAIL → 采纳当前文本;
                        // 未给判定(不可解析/未配置)→ 机械通过即通过。
                        let mech = reflect(
                            &reflect_text,
                            user_input,
                            attempt,
                            max_attempts,
                            has_updates,
                            min_chars,
                        );
                        if !mech.passed {
                            mech
                        } else {
                            match llm_verdict {
                                Some(v) if v.passed => v,
                                Some(v) => {
                                    accept_capped = true;
                                    v
                                }
                                None => mech,
                            }
                        }
                    } else {
                        reflect(
                            &reflect_text,
                            user_input,
                            attempt,
                            max_attempts,
                            has_updates,
                            min_chars,
                        )
                    }
                } else if !ctx.reflect_prompt.trim().is_empty() && !reflect_text.trim().is_empty() {
                    match reflect_with_tools(
                        self,
                        &ctx.reflect_prompt,
                        &banned_hint,
                        user_input,
                        &reflect_text,
                        &reflect_tool_ctx,
                        abort,
                    )
                    .await
                    {
                        Some((v, revised, u)) => {
                            rctx.total_usage.prompt_tokens += u.prompt_tokens;
                            rctx.total_usage.completion_tokens += u.completion_tokens;
                            rctx.total_usage.total_tokens += u.total_tokens;
                            rctx.total_usage.prompt_cache_hit_tokens += u.prompt_cache_hit_tokens;
                            rctx.total_usage.prompt_cache_miss_tokens += u.prompt_cache_miss_tokens;
                            // 模型自主修正的正文写回(保留 <UpdateVariable> 补丁块,
                            // 与既有禁词修正的 rebuild_content_keeping_blocks 同语义)
                            if let Some(revised_text) = revised {
                                let trimmed = revised_text.trim().to_string();
                                if !trimmed.is_empty() && trimmed != reflect_text {
                                    content = rebuild_content_keeping_blocks(&content, &trimmed);
                                    reflect_text = trimmed;
                                    send_event(
                                        step_evt(
                                            "反思修正",
                                            Some("反思模型已定点修正正文段落".into()),
                                            progress.0,
                                            progress.1,
                                        ),
                                        tx,
                                        abort,
                                        flag,
                                    )
                                    .await?;
                                }
                            }
                            v
                        }
                        None => reflect(
                            &reflect_text,
                            user_input,
                            attempt,
                            max_attempts,
                            has_updates,
                            min_chars,
                        ),
                    }
                } else {
                    reflect(
                        &reflect_text,
                        user_input,
                        attempt,
                        max_attempts,
                        has_updates,
                        min_chars,
                    )
                };
                logging::agent_step(session_id, "reflect", Some(&verdict.reason));
                if verdict.passed {
                    send_event(
                        step_evt(
                            "反思通过",
                            Some(if critique_summary.is_empty() {
                                verdict.reason.clone()
                            } else {
                                critique_summary.chars().take(160).collect()
                            }),
                            progress.0,
                            progress.1,
                        ),
                        tx,
                        abort,
                        flag,
                    )
                    .await?;
                    idx += 1;
                    continue;
                }
                if accept_capped {
                    // deep/agent:修改轮已达上限仍判 FAIL → 采纳当前文本(不再整篇重生成,
                    // RPFLOW-1:反思靠工具定点修改收敛,重生成只服务结构性问题兜底)
                    send_event(
                        step_evt(
                            "反思已达修改上限",
                            Some(format!(
                                "已定点修改 {revisions_used}/{} 次,采纳当前文本;遗留:{}",
                                revise_cap.unwrap_or(0),
                                if critique_summary.is_empty() {
                                    verdict.reason.clone()
                                } else {
                                    critique_summary.chars().take(160).collect()
                                }
                            )),
                            progress.0,
                            progress.1,
                        ),
                        tx,
                        abort,
                        flag,
                    )
                    .await?;
                    logging::agent_step(
                        session_id,
                        "reflect_capped",
                        Some(&format!("反思达修改上限({revisions_used})后采纳当前文本")),
                    );
                    idx += 1;
                    continue;
                }
                send_event(
                    step_evt(
                        "反思未通过",
                        Some(verdict.reason.clone()),
                        progress.0,
                        progress.1,
                    ),
                    tx,
                    abort,
                    flag,
                )
                .await?;
                // 放弃条件:retry_action=stop(规则在 attempt 达上限时返回 stop)或反思重试达硬上限。
                // 不再检查 attempt >= max_attempts:多生成步骤流程(如 custom 多步)中 attempt 被
                // 生成步骤累计,反思重试次数应独立由 reflect_retries 限制(有界性不变)。
                if verdict.retry_action == Some("stop") || reflect_retries >= max_attempts {
                    // 达到重试上限:放弃反思,继续后续步骤(保证任何路径下有界)。
                    // 反思失败建议(位置0):由引擎自动生成(≤200 token)并注入到位置1 激发之后、
                    // 预设尾部之前;生成失败时仅存可选的用户补充说明。角色沿用配置(user/assistant)。
                    if let Some(a) = build_reflect_advice(
                        self,
                        &verdict.reason,
                        user_input,
                        &reflect_text,
                        &ctx.reflect_advice_supplement,
                        abort,
                        rctx.total_usage,
                    )
                    .await
                    {
                        // 同一 run 生效:注入位置0(预设尾部之前),后续步骤生成可见
                        inject_reflect_advice(
                            rctx.llm_messages,
                            &a,
                            &ctx.reflect_advice_role,
                            ctx.preset_tail.as_deref(),
                        );
                    }
                    idx += 1;
                    continue;
                }
                reflect_retries += 1;
                send_event(
                    step_evt(
                        "重新生成",
                        Some(format!("第 {}/{} 次重试", reflect_retries, max_attempts)),
                        progress.0,
                        progress.1,
                    ),
                    tx,
                    abort,
                    flag,
                )
                .await?;
                // 回退到上一个会生成内容的 direct 步骤重新生成。
                // 旧实现 while idx > 0 && plan.steps[idx - 1].action != "direct" 有缺陷:
                // plan 中反思步骤前一个 direct 步骤恒为「理解意图」(generates=false,不生成、
                // 不递增 attempt),条件不成立导致 idx 原地打转 → attempt 永不递增 →
                // 反思失败进入无限紧密循环(日志佐证:同一会话每毫秒一条 reflect)。
                match retreat_to_generating_step(&plan.steps, idx) {
                    Some(target) => {
                        // custom 模式即时应用语义:回退重生成会丢弃该步骤起的内容,
                        // 一并回滚其已应用的变量补丁(基线恢复 + Vars 事件同步前端);
                        // 否则 delta 补丁会被重生成重复应用,变量值翻倍。
                        if req.mode == "custom" {
                            if let Some(baseline) = step_vars_baseline.get(&target).cloned() {
                                *rctx.assistant_vars = baseline;
                                custom_vars_snapshot = None;
                                let tree = rctx.assistant_vars.tree().clone();
                                let _ =
                                    send_event(SseEvent::Vars { stat_data: tree }, tx, abort, flag)
                                        .await;
                                step_vars_baseline.retain(|&k, _| k <= target);
                            }
                        }
                        idx = target;
                    }
                    None => {
                        // 找不到可回退的生成步骤(理论不可达):放弃反思继续。
                        // 同样自动生成反思失败建议(位置0,激发之后、预设尾部之前)
                        if let Some(a) = build_reflect_advice(
                            self,
                            &verdict.reason,
                            user_input,
                            &reflect_text,
                            &ctx.reflect_advice_supplement,
                            abort,
                            rctx.total_usage,
                        )
                        .await
                        {
                            // 同一 run 生效:注入位置0(预设尾部之前),后续步骤生成可见
                            inject_reflect_advice(
                                rctx.llm_messages,
                                &a,
                                &ctx.reflect_advice_role,
                                ctx.preset_tail.as_deref(),
                            );
                        }
                        idx += 1;
                        continue;
                    }
                }
                // 丢弃上一个 direct 步骤的工具调用中间消息,干净地重新生成
                rctx.llm_messages.truncate(base_len);
                continue;
            }

            // ===== 草稿步(deep/agent 流程第 1 步,RPFLOW-1)=====
            // 产物**默认隐藏**:不写 content、不透出 token(emit_tokens=false);
            // 仅经 SseEvent::Draft 供 Agent 面板折叠展示,并以 **system 角色**注记并入
            // 上下文供正文步参考(不落库、不进气泡;user 角色会把用户原文顶出「末条 user」位)。
            if step.action == "draft" {
                state_machine.transition(AgentState::Executing, session_id)?;
                self.agent_sessions
                    .update(
                        &agent_session.id,
                        Some("executing"),
                        None,
                        Some((idx + 1) as i64),
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                send_event(
                    step_evt(
                        "草稿中…",
                        Some("撰写剧情草稿(默认隐藏,面板可展开)".into()),
                        progress.0,
                        progress.1,
                    ),
                    tx,
                    abort,
                    flag,
                )
                .await?;
                let step_msgs = step_messages(rctx, ctx, user_input, step);
                let mut step_params = step_params_for(&req.params, step, &self.tool_registry);
                // 草稿步恒无工具:纯规划性输出;工具调用留给正文步与反思步。
                // tool_choice=None 显式禁止(provider 语义:不会返回 tool_calls);
                // 测试侧由 mock 的「非主线步骤」判据兜住(草稿步 system 带步骤前缀,
                // 主线工具钩子不产出——见 mock::NON_MAINLINE_STEP_NEEDLES)。
                step_params.tools.clear();
                step_params.tool_choice = crate::models::types::ToolChoice::None;
                let mut result = execute_generation(
                    self,
                    session_id,
                    &run_id.to_string(),
                    &step_msgs,
                    &step_params,
                    tx,
                    abort,
                    flag,
                    false,
                )
                .await?;
                // 推理耗尽防护(2026-10-09 真实模型实测):草稿预算被 reasoning 吃光时
                // content 为空(HTTP 200 + completion_tokens>0、草稿静默缺失)——
                // 提额重试一次,判定见 `draft_heal_budget`(纯函数带单测)。
                if !result.interrupted {
                    if let Some(next_budget) = draft_heal_budget(
                        &result.content,
                        result.usage.completion_tokens,
                        step_params.max_tokens,
                    ) {
                        let mut retry_params = step_params.clone();
                        retry_params.max_tokens = next_budget;
                        let retry = execute_generation(
                            self,
                            session_id,
                            &run_id.to_string(),
                            &step_msgs,
                            &retry_params,
                            tx,
                            abort,
                            flag,
                            false,
                        )
                        .await?;
                        // 两轮 usage 均计入(与反思链路 generate_collect_healing 同口径)
                        let mut merged = result.usage;
                        merged.prompt_tokens += retry.usage.prompt_tokens;
                        merged.completion_tokens += retry.usage.completion_tokens;
                        merged.total_tokens += retry.usage.total_tokens;
                        merged.prompt_cache_hit_tokens += retry.usage.prompt_cache_hit_tokens;
                        merged.prompt_cache_miss_tokens += retry.usage.prompt_cache_miss_tokens;
                        result = retry;
                        result.usage = merged;
                    }
                }
                rctx.total_usage.prompt_tokens += result.usage.prompt_tokens;
                rctx.total_usage.completion_tokens += result.usage.completion_tokens;
                rctx.total_usage.total_tokens += result.usage.total_tokens;
                rctx.total_usage.prompt_cache_hit_tokens += result.usage.prompt_cache_hit_tokens;
                rctx.total_usage.prompt_cache_miss_tokens += result.usage.prompt_cache_miss_tokens;
                if result.interrupted {
                    break;
                }
                let draft_text = result.content.trim().to_string();
                if !draft_text.is_empty() {
                    send_event(
                        SseEvent::Draft {
                            text: draft_text.clone(),
                        },
                        tx,
                        abort,
                        flag,
                    )
                    .await?;
                    // 内部草稿以 **system** 角色注记进上下文(与位置3 世界书注入同型):
                    // ① 对真实模型 = 内部素材而非「用户刚说的话」,不会把草稿当末条
                    //    用户输入来回应;② 不占据「末条 user」位,避免遮蔽用户消息
                    //    对下游步骤可见性(RPFLOW 实测:注记用 user 角色时把用户原文
                    //    顶掉,mock 钩子与真实语义都受影响)。
                    rctx.llm_messages.push(LlmMessage::plain(
                        "system",
                        &format!("【内部草稿·仅你可见,请勿在正文中复述】\n{draft_text}"),
                    ));
                    // 反思回退的截断下界抬到草稿注记**之后**(RPFLOW-1 审查回归):
                    // 回退重生成只该丢正文步工具循环追加的中间消息,不能把注记一并截掉
                    // ——正文步提示词恒引用「内部草稿」(deep_retreat_regeneration_keeps_draft_note)。
                    base_len = rctx.llm_messages.len();
                    logging::agent_step(session_id, "draft", Some("已生成隐藏草稿"));
                } else {
                    // 空草稿可见化(2026-10-09 实测):提额重试后仍为空(模型把预算全烧在
                    // reasoning 上,HTTP 200 但 content 空)——不再静默跳过:step 事件让
                    // 推理链可查、warn 带用量便于排查;正文步按用户原文照常生成。
                    tracing::warn!(
                        session_id,
                        completion_tokens = result.usage.completion_tokens,
                        max_tokens = step_params.max_tokens,
                        "草稿步输出为空(推理耗尽预算),已跳过"
                    );
                    send_event(
                        step_evt(
                            "草稿为空",
                            Some("模型未产出草稿(推理耗尽预算),已跳过;不影响正文".into()),
                            progress.0,
                            progress.1,
                        ),
                        tx,
                        abort,
                        flag,
                    )
                    .await?;
                    logging::agent_step(session_id, "draft", Some("草稿为空(推理耗尽预算),已跳过"));
                }
                idx += 1;
                continue;
            }

            // ===== 归档步(deep/agent 流程第 4 步,RPFLOW-2)=====
            // 产物不进正文(emit_tokens=false,最终文本丢弃):本步只做副作用——
            // 更新角色文件区 大纲.md/人物关系.md,并按开关同步世界书词条(RPFLOW-2)。
            // 失败绝不影响本轮正文(仅告警);工具调用经 Agent 面板可见。
            if step.action == "archive" {
                // 无正文可归档 / 轮内已中断 / token 预算已越线(无论 stop/warn 档):
                // 跳过归档——预算超额后不再追加自动调用(判定与 executor 轮末闸门
                // 同源:单一实现 budget_reached)。
                let budget = self.settings_snapshot().session_token_budget;
                let used = rctx.total_usage.prompt_tokens + rctx.total_usage.completion_tokens;
                if content.trim().is_empty() || rctx.budget_stopped || budget_reached(used, budget)
                {
                    idx += 1;
                    continue;
                }
                state_machine.transition(AgentState::Executing, session_id)?;
                self.agent_sessions
                    .update(
                        &agent_session.id,
                        Some("executing"),
                        None,
                        Some((idx + 1) as i64),
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                send_event(
                    step_evt(
                        "归档与词条同步中…",
                        Some("更新大纲/人物关系,并同步世界书词条".into()),
                        progress.0,
                        progress.1,
                    ),
                    tx,
                    abort,
                    flag,
                )
                .await?;
                let mut step_msgs = step_messages(rctx, ctx, user_input, step);
                // 归档模型需要看到本轮最终正文(生成侧产物不在 llm_messages 里)。
                // 以 **system** 角色注记(同草稿注记口径):不占据「末条 user」位,
                // 用户原文对下游工具钩子/模型保持可见。
                step_msgs.push(LlmMessage::plain(
                    "system",
                    &format!("【本轮最终正文·供归档】\n{}", content.trim()),
                ));
                let step_params = step_params_for(&req.params, step, &self.tool_registry);
                // 步骤白名单闸门(同 custom 用法):名单内自动放行,名单外回到等待授权
                let gate = match step.tools.as_deref() {
                    Some(list) if !list.is_empty() => crate::agents::engine::executor::ToolGate {
                        whitelist: Some(list),
                        no_ui_authorization: false,
                    },
                    _ => crate::agents::engine::executor::ToolGate::wait(),
                };
                match run_tool_loop(
                    self,
                    state_machine,
                    Some(agent_session),
                    session_id,
                    &mut step_msgs,
                    &step_params,
                    tool_ctx,
                    tx,
                    abort,
                    flag,
                    rctx.total_usage,
                    &run_id.to_string(),
                    gate,
                    // 聊天路径无节点级超时;emit_tokens=false(产物不进气泡)
                    None,
                    false,
                )
                .await
                {
                    Ok(_res) => {
                        logging::agent_step(session_id, "archive", Some("归档与词条同步完成"));
                    }
                    // 归档失败不影响本轮正文:仅告警(中断会在循环顶 check_aborted 兜住)
                    Err(e) => tracing::warn!(error = %e, "归档步执行失败(不影响本轮正文)"),
                }
                idx += 1;
                continue;
            }

            // ===== 执行步骤(direct / tool)=====
            state_machine.transition(AgentState::Executing, session_id)?;
            self.agent_sessions
                .update(
                    &agent_session.id,
                    Some("executing"),
                    None,
                    Some((idx + 1) as i64),
                    None,
                )
                .map_err(|e| e.to_string())?;
            send_event(
                step_evt("执行中…", Some(step.goal.clone()), progress.0, progress.1),
                tx,
                abort,
                flag,
            )
            .await?;

            // 非 AGENT/CUSTOM 模式:工具步骤与计算启发式保留原行为
            if req.mode != "agent" && req.mode != "custom" {
                if step.generates == Some(false) {
                    maybe_run_tool(
                        self,
                        state_machine,
                        &mut tool_triggered,
                        &agent_session.id,
                        session_id,
                        user_input,
                        tool_ctx,
                        rctx.llm_messages,
                        tx,
                        abort,
                        flag,
                    )
                    .await?;
                    idx += 1;
                    continue;
                }

                maybe_run_tool(
                    self,
                    state_machine,
                    &mut tool_triggered,
                    &agent_session.id,
                    session_id,
                    user_input,
                    tool_ctx,
                    rctx.llm_messages,
                    tx,
                    abort,
                    flag,
                )
                .await?;
            } else if step.generates == Some(false) {
                // AGENT/CUSTOM 模式:计划中的「理解意图」类步骤不生成,直接进入下一步;
                // 是否调用工具完全由模型通过 function calling 自主决定(或按步骤工具配置)
                idx += 1;
                continue;
            }
            attempt += 1;
            // custom 模式:记录该生成步骤开始前的变量树基线(反思回退时恢复用)
            if req.mode == "custom" {
                step_vars_baseline.insert(idx, rctx.assistant_vars.clone());
            }
            // 步骤级消息视图(agent/deep/custom 统一,装配语义见 `step_messages`):
            // 步骤 system_prompt 宏展开后以「[本步指令]」追加到 system 末尾,下方再按
            // mode 追加动态工具指南。独立视图不污染共享 llm_messages,反思回退后
            // 按步骤重建、无中间消息残留。
            let mut step_msgs = step_messages(rctx, ctx, user_input, step);
            // 先计算本步骤实际下发的工具,再按 effective tools 注入指南。
            // Custom 的 null/[]/白名单语义只影响能力与永久授权,不能只做可见性过滤。
            let mut step_params = step_params_for(&req.params, step, &self.tool_registry);
            if req.mode == "agent" && step.tools.is_none() && step_params.tools.is_empty() {
                step_params.tools = req.params.tools.clone();
            }
            if matches!(req.mode.as_str(), "agent" | "custom") && !step_params.tools.is_empty() {
                if let Some(s) = step_msgs.first_mut() {
                    if s.role == "system" {
                        s.content.push_str(&format!(
                            "\n\n【可用工具】\n{}",
                            self.tool_registry.tool_guidance_for(&step_params.tools)
                        ));
                        // 技能渐进披露(落地项 3):system 只注入「技能名:一句话用途」紧凑清单,
                        // 正文按需 read(type=skill) 加载;清单按 name 排序,跨轮字节稳定(前缀缓存)。
                        // skill_progressive_disclosure = false 时回退旧行为(完全不注入清单)。
                        // (设置快照:不留锁跨 await)
                        if self.settings_snapshot().skill_progressive_disclosure {
                            let manifest = crate::services::skill_service::skill_manifest(
                                &self.skills.list(true),
                            );
                            if !manifest.is_empty() {
                                s.content.push_str(&format!(
                                    "\n\n【可用技能】(需要时用 read 工具 type=skill 按名读取正文)\n{manifest}"
                                ));
                            }
                        }
                    }
                }
            }
            send_event(
                step_evt(
                    "生成中…",
                    Some(format!("第 {} 次尝试", attempt)),
                    progress.0,
                    progress.1,
                ),
                tx,
                abort,
                flag,
            )
            .await?;
            // AGENT 模式:完整 function calling 工具循环;custom 模式:按步骤派生参数
            // (温度/输出上限/工具白名单);三者统一使用上方构造的步骤级消息视图 step_msgs
            // (含 [本步指令] 与 agent 模式的动态工具指南),反思回退后按步骤重建。
            let result = if req.mode == "agent" {
                // agent 兼容旧行为:planner 未配步骤 tools 时使用请求级工具。
                // 聊天路径保留「未放行即等待授权」语义(有 UI 授权上下文)。
                let effective = step_params.clone();
                let gate = match step.tools.as_deref() {
                    // planner 显式配了步骤白名单:名单内自动放行,名单外回到等待授权
                    Some(list) if !list.is_empty() => crate::agents::engine::executor::ToolGate {
                        whitelist: Some(list),
                        no_ui_authorization: false,
                    },
                    _ => crate::agents::engine::executor::ToolGate::wait(),
                };
                run_tool_loop(
                    self,
                    state_machine,
                    // 聊天路径恒有 agent_sessions 行,包 Some(行为不变;任务模式传 None)
                    Some(agent_session),
                    session_id,
                    &mut step_msgs,
                    &effective,
                    tool_ctx,
                    tx,
                    abort,
                    flag,
                    rctx.total_usage,
                    &run_id.to_string(),
                    gate,
                    // 聊天路径无节点级超时(A 批 A1):恒走宿主既有判定(聊天侧本无总时长上限)
                    None,
                    true,
                )
                .await?
            } else if req.mode == "custom" {
                let r = if step_params.tools.is_empty() {
                    execute_generation(
                        self,
                        session_id,
                        &run_id.to_string(),
                        &step_msgs,
                        &step_params,
                        tx,
                        abort,
                        flag,
                        true,
                    )
                    .await?
                } else {
                    // custom 模式白名单:步骤配置了 tools 白名单时,名单内工具自动放行;
                    // 名单外工具回到等待授权(聊天有 UI 授权上下文)
                    let gate = match step.tools.as_deref() {
                        Some(list) if !list.is_empty() => {
                            crate::agents::engine::executor::ToolGate {
                                whitelist: Some(list),
                                no_ui_authorization: false,
                            }
                        }
                        _ => crate::agents::engine::executor::ToolGate::wait(),
                    };
                    run_tool_loop(
                        self,
                        state_machine,
                        // 聊天路径恒有 agent_sessions 行,包 Some(行为不变;任务模式传 None)
                        Some(agent_session),
                        session_id,
                        &mut step_msgs,
                        &step_params,
                        tool_ctx,
                        tx,
                        abort,
                        flag,
                        rctx.total_usage,
                        &run_id.to_string(),
                        gate,
                        // 聊天 custom 步骤无节点级超时(A 批 A1:该字段只服务任务侧的
                        // `PlanStep`);恒走宿主既有判定
                        None,
                        true,
                    )
                    .await?
                };
                // 步骤提示词展开可能产生新变量:执行后写回持久化(与构建阶段一致)
                self.sessions
                    .save_session_vars(session_id, rctx.session_vars);
                r
            } else {
                // deep / fast:应用步骤级消息视图([本步指令])与步骤级温度/输出上限(无工具)
                execute_generation(
                    self,
                    session_id,
                    &run_id.to_string(),
                    &step_msgs,
                    &step_params,
                    tx,
                    abort,
                    flag,
                    true,
                )
                .await?
            };
            if result.interrupted {
                // 中断语义统一(HB-3):部分正文与中断轮 usage 必须流到收尾端——
                // 此前 break 发生在 content/usage 赋值之前,两者随 run 返回被丢弃,
                // 「保留部分产出 + 中断轮记账」无从实现。
                content = result.content;
                rctx.total_usage.prompt_tokens += result.usage.prompt_tokens;
                rctx.total_usage.completion_tokens += result.usage.completion_tokens;
                rctx.total_usage.total_tokens += result.usage.total_tokens;
                rctx.total_usage.prompt_cache_hit_tokens += result.usage.prompt_cache_hit_tokens;
                rctx.total_usage.prompt_cache_miss_tokens += result.usage.prompt_cache_miss_tokens;
                break;
            }
            // update_variables 通过工具注册表直接持久化当前会话变量树;工具循环结束后
            // 必须同步回引擎内存副本,否则后续文本协议或状态栏生成会用旧树覆盖工具结果。
            if req.mode == "custom" && !step_params.tools.is_empty() {
                let persisted_vars = self.sessions.load_assistant_vars(session_id);
                if persisted_vars.tree() != rctx.assistant_vars.tree() {
                    *rctx.assistant_vars = persisted_vars;
                    custom_vars_snapshot = Some(rctx.assistant_vars.tree().clone());
                }
            }
            content = result.content;
            // finish_reason 与 content 同生命周期:逐轮覆盖,收尾即为「最终采纳那一步」
            // 的上游结束原因(可观测性问题①;聊天截断提示依此判定)。
            rctx.last_finish_reason = result.finish_reason.clone();
            // 预算停止标记同样逐轮覆盖(HB-1):最终采纳的那一步才是收尾依据
            rctx.budget_stopped = result.budget_stopped;
            // custom 模式:每步生成后立即解析并应用 mvu <UpdateVariable> 补丁。
            // 中间步骤的内容不保留(循环内被覆盖、不在收尾解析),延迟应用会丢;
            // 即时语义与 MagVarUpdate 原版一致(反思回退时已应用的补丁不回滚)。
            // P5:有契约时补丁先经契约门控(与收尾正文路径同构),被拒/低置信仅计
            // 数告警,applied 生效并生成 changelog 条目(收尾统一 commit)。
            if req.mode == "custom" {
                let (clean, patches) = parse_update_variable(&content);
                content = clean;
                let contract = self.load_character_contract(&req.character_id);
                let gated = crate::contracts::gate_assistant_patches_detailed(
                    contract.as_ref(),
                    &patches,
                    "agent",
                );
                if !gated.rejected.is_empty() || !gated.pending.is_empty() {
                    tracing::warn!(
                        rejected = gated.rejected.len(),
                        pending = gated.pending.len(),
                        "契约门控过滤了部分自定义模式正文补丁"
                    );
                }
                custom_contract_pending.extend(gated.pending);
                if contract.is_some() && !gated.applied.is_empty() {
                    let tree_before = rctx.assistant_vars.tree().clone();
                    if let Some(tree) = apply_mvu_patches(
                        self,
                        session_id,
                        rctx.assistant_vars,
                        &gated.applied,
                        tx,
                        abort,
                        flag,
                    )
                    .await
                    {
                        custom_vars_snapshot = Some(tree.clone());
                        custom_contract_entries.extend(crate::contracts::entries_from_applied(
                            &tree_before,
                            &tree,
                            ctx.history.len() as u64,
                            &gated.applied_ops,
                            crate::contracts::ChangelogSource::Agent,
                        ));
                    }
                } else {
                    if let Some(tree) = apply_mvu_patches(
                        self,
                        session_id,
                        rctx.assistant_vars,
                        &gated.applied,
                        tx,
                        abort,
                        flag,
                    )
                    .await
                    {
                        custom_vars_snapshot = Some(tree);
                    }
                }
            }
            // mergeUsage:累加三个字段;context_tokens 固定为该轮上下文值
            rctx.total_usage.prompt_tokens += result.usage.prompt_tokens;
            rctx.total_usage.completion_tokens += result.usage.completion_tokens;
            rctx.total_usage.total_tokens += result.usage.total_tokens;
            rctx.total_usage.prompt_cache_hit_tokens += result.usage.prompt_cache_hit_tokens;
            rctx.total_usage.prompt_cache_miss_tokens += result.usage.prompt_cache_miss_tokens;
            idx += 1;
        }
        Ok((
            content,
            custom_vars_snapshot,
            custom_contract_entries,
            custom_contract_pending,
        ))
    }
}

/// 构造步骤级消息视图(RPFLOW-1:草稿步与既有生成步共用同一装配语义):
/// 先把 scopes 镜像同步到当前变量树(步骤提示词的 getvar 类宏能读到本步骤之前的
/// setvar 副作用),再把步骤 system_prompt 宏展开后以「[本步指令]」追加到 system 末尾。
/// 返回独立视图,不污染共享 llm_messages(反思回退后按步骤重建、无中间消息残留)。
fn step_messages(
    rctx: &mut RunContext<'_>,
    ctx: &CollectedCtx,
    user_input: &str,
    step: &crate::models::types::PlanStep,
) -> Vec<LlmMessage> {
    {
        let mut s = rctx.scopes.lock().unwrap_or_else(|e| e.into_inner());
        s.sync_chat_tree(rctx.assistant_vars);
        s.sync_chat_flat(rctx.session_vars);
    }
    let mut step_msgs = rctx.llm_messages.clone();
    if step
        .system_prompt
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        let mut scopes_guard = rctx.scopes.lock().unwrap_or_else(|e| e.into_inner());
        let mut mctx = MacroCtx {
            character_name: &ctx.chara_name,
            character_description: &ctx.chara_desc,
            user_name: "用户",
            user_input,
            personality: &ctx.personality,
            scenario: &ctx.scenario,
            history: &ctx.history_tuples,
            vars: &mut *rctx.session_vars,
            assistant_vars: Some(&mut *rctx.assistant_vars),
            scopes: Some(&mut *scopes_guard),
        };
        step_msgs = with_step_prompt(rctx.llm_messages, step, &mut mctx);
    }
    step_msgs
}
