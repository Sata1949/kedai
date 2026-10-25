// 工具循环的单轮执行(C-2C,自 `executor.rs::run_tool_loop` 抽出)。
//
// 职责边界:本模块只负责「模型给出的工具调用」到「工具结果落库/推送/回灌消息」这一段
// 副作用;守卫计数与停止判定在 `guard.rs`,循环编排在 `mod.rs`。
//
// 顺序是契约(与抽取前逐字一致):
//   裁决(按模型顺序,无副作用)→ 分组执行(连续 Safe 并发、其余串行屏障)→
//   按原调用顺序收尾(状态机 → agent_sessions → ToolResult 事件 → tool 消息回灌)。
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};

use crate::agents::state_machine::{AgentState, StateMachine};
use crate::models::types::{AgentSessionRecord, LlmMessage, SseEvent, ToolCallArgs, ToolContext};

use super::super::executor::{tool_origin_str, ToolGate};
use super::super::util::{send_event, step_evt};
use super::super::{AbortFlag, AgentEngine, EngineError};

/// 单轮执行所需的不可变/可变上下文聚合(避免 10+ 位置参数)。
pub(super) struct RoundContext<'a> {
    pub(super) engine: &'a AgentEngine,
    pub(super) state_machine: &'a mut StateMachine,
    pub(super) agent_session: Option<&'a AgentSessionRecord>,
    pub(super) session_id: &'a str,
    pub(super) llm_messages: &'a mut Vec<LlmMessage>,
    pub(super) tool_ctx: &'a ToolContext,
    pub(super) tx: &'a mpsc::Sender<SseEvent>,
    pub(super) abort: &'a watch::Receiver<bool>,
    pub(super) flag: &'a AbortFlag,
    pub(super) run_id: &'a str,
    pub(super) gate: ToolGate<'a>,
}

/// 单轮执行的可见结果。
pub(super) struct RoundOutcome {
    /// 已执行并按原顺序收尾的工具调用(守卫判定据此统计只读/写类与声明文件进展)。
    pub(super) outputs: Vec<ExecutedTool>,
    /// 工具执行期被用户中断(收尾未完成,调用方直接返回中断终态)。
    pub(super) interrupted: bool,
}

/// 单轮中一个待执行/已执行的工具调用(裁决结果 + 执行输出 + 耗时 + 快照绑定)
pub(super) struct ExecutedTool {
    pub(super) call: ToolCallArgs,
    permission: crate::tools::permissions::PermissionDecision,
    pub(super) output: Value,
    /// 工具执行耗时(毫秒;未执行时为 0)
    duration_ms: i64,
    /// 本次执行成功落库的回退快照 id(批 D-1;仅写工具且快照开关开时 Some):
    /// 随 ToolResult 事件透出、随 tool_calls 落库,供前端精确绑定「回退到此处」
    snapshot_id: Option<String>,
}

/// 执行本轮全部工具调用,并完成状态/session/SSE/消息回灌的收尾。
///
/// 中断语义:串行组执行中检测到取消时立即返回 `interrupted=true`,**不再**收尾
/// 本轮已执行的工具——与抽取前一致(此前也直接 return,前面的输出不落事件/消息)。
pub(super) async fn execute_round(
    ctx: RoundContext<'_>,
    tool_calls: &[ToolCallArgs],
) -> Result<RoundOutcome, EngineError> {
    // 1) 裁决(顺序,无副作用):白名单工具自动放行,其余走三档授权模式裁决
    //    (任务模式无 UI 授权上下文时,未放行工具在执行阶段直接拒绝,不空等)。
    let mut executed: Vec<ExecutedTool> = tool_calls
        .iter()
        .map(|call| {
            let registered = ctx.engine.tool_registry.get(&call.name).is_some();
            let custom_authorized = ctx.gate.authorizes(&call.name);
            let (mode, always_required) = {
                // 设置快照:不留锁跨 await
                let settings = ctx.engine.settings_snapshot();
                (
                    settings.authorization_mode,
                    settings
                        .bypass_blacklist
                        .iter()
                        .any(|name| name == &call.name),
                )
            };
            // 操作分类:读/写/删文件与系统路径区域,决定三档矩阵的走向
            let origin = ctx
                .engine
                .tool_registry
                .origin_of(&call.name)
                .unwrap_or(crate::tools::action_class::ToolOrigin::Builtin);
            let action = crate::tools::action_class::classify(&call.name, &call.arguments, origin);
            let permission = if ctx.gate.excludes(&call.name) {
                // 任务模式名单是硬边界:名单外工具直接拒绝,不进入三档文件规则
                // (否则宽松模式会放过被任务策略排除的写类工具)
                crate::tools::permissions::PermissionDecision {
                    allowed: false,
                    risk: ctx.engine.tool_registry.permissions().risk_for(&call.name),
                    reason: "该工具不在当前任务策略允许的工具清单内".into(),
                }
            } else {
                ctx.engine.tool_registry.permissions().decide_with_policy(
                    &call.name,
                    ctx.tool_ctx,
                    registered,
                    custom_authorized,
                    mode,
                    &action,
                    always_required,
                )
            };
            tracing::info!(
                tool = call.name.clone(),
                risk = format!("{:?}", permission.risk).to_lowercase(),
                allowed = permission.allowed,
                result = permission.reason.clone(),
                "tool_permission"
            );
            ExecutedTool {
                call: call.clone(),
                permission,
                output: Value::Null,
                duration_ms: 0,
                snapshot_id: None,
            }
        })
        .collect();

    // 2) 执行:按模型顺序分组调度——连续 Safe 工具组内并发(join_all 保序),
    //    非 Safe/未注册工具单独成组、串行执行(含授权等待),组间串行,
    //    保证授权语义与副作用顺序稳定。
    let parallel_safe: Vec<bool> = executed
        .iter()
        .map(|e| {
            ctx.engine.tool_registry.get(&e.call.name).is_some()
                && ctx
                    .engine
                    .tool_registry
                    .permissions()
                    .risk_for(&e.call.name)
                    == crate::tools::permissions::ToolRisk::Safe
        })
        .collect();
    let groups = split_parallel_groups(&parallel_safe);
    for group in groups {
        if group.len() > 1 || parallel_safe[group[0]] {
            // 并发组:组内全部为 Safe 工具,并发执行并按原顺序收集
            let futures: Vec<_> = group
                .iter()
                .map(|&idx| {
                    let e = &executed[idx];
                    let call = &e.call;
                    let perm = &e.permission;
                    let tool_ctx = ctx.tool_ctx.clone();
                    let engine = ctx.engine;
                    let tx = ctx.tx;
                    let abort = ctx.abort;
                    let flag = ctx.flag;
                    let run_id = ctx.run_id;
                    async move {
                        execute_call(engine, call, perm, &tool_ctx, tx, abort, flag, run_id).await
                    }
                })
                .collect();
            let outputs = futures::future::join_all(futures).await;
            for (&idx, out) in group.iter().zip(outputs) {
                executed[idx].output = out.0;
                executed[idx].duration_ms = out.1;
                executed[idx].snapshot_id = out.2;
            }
        } else {
            // 串行组:单个非 Safe/未注册工具,含授权等待
            let interrupted = execute_serial_tool(
                ctx.engine,
                &mut executed[group[0]],
                ctx.tool_ctx,
                ctx.tx,
                ctx.abort,
                ctx.flag,
                ctx.run_id,
                ctx.gate,
            )
            .await?;
            if interrupted {
                return Ok(RoundOutcome {
                    outputs: Vec::new(),
                    interrupted: true,
                });
            }
        }
    }

    // 3) 收尾(按原调用顺序):状态机/持久化/SSE 推送/模型消息回填。
    //    ToolResult 与 ToolAuthorizationRequired 均在此按序推送,保证前端配对稳定。
    for e in &executed {
        ctx.state_machine
            .transition_best_effort(AgentState::ToolCall, ctx.session_id);
        // 任务模式 agent_session=None(不建影子会话行):跳过 agent_sessions 落库;
        // 聊天路径恒 Some,落库顺序与原实现逐字节一致。
        if let Some(agent_session) = ctx.agent_session {
            let _ = ctx.engine.agent_sessions.update(
                &agent_session.id,
                Some("tool_call"),
                None,
                None,
                None,
            );
            let input_val: Value = serde_json::from_str(&e.call.arguments)
                .unwrap_or_else(|_| Value::String(e.call.arguments.clone()));
            let _ = ctx.engine.agent_sessions.add_tool_call(
                &agent_session.id,
                &e.call.name,
                input_val,
                e.output.clone(),
                e.duration_ms,
                // 批 D-1:快照绑定随工具调用落库,历史重载后「回退到此处」仍可精确匹配
                e.snapshot_id.clone(),
            );
        }
        ctx.state_machine
            .transition_best_effort(AgentState::Executing, ctx.session_id);
        if let Some(agent_session) = ctx.agent_session {
            let _ = ctx.engine.agent_sessions.update(
                &agent_session.id,
                Some("executing"),
                None,
                None,
                None,
            );
        }
        // 授权已在执行前等待;此处统一发送最终结果,允许与拒绝都按 call_id 收束状态。
        send_event(
            SseEvent::ToolResult {
                name: e.call.name.clone(),
                output: e.output.clone(),
                call_id: Some(e.call.id.clone()),
                render_kind: crate::tools::registry::render_kind_for(&e.call.name)
                    .map(|s| s.to_string()),
                origin: tool_origin_str(&ctx.engine.tool_registry, &e.call.name),
                // 批 D-1:写工具成功落库的回退快照 id(加性字段;None 不序列化)
                snapshot_id: e.snapshot_id.clone(),
            },
            ctx.tx,
            ctx.abort,
            ctx.flag,
        )
        .await?;
        // tool 结果消息:与循环前的 assistant(tool_calls) 成对,供下一轮生成参考。
        // 工具图像通道(视觉能力包 D4):约定式返回 `{text, images:[引用]}` 时,
        // 把引用解析成 data URL 挂到 tool 消息上(内容取 text 字段);其它返回形状
        // 原样透传(既有工具零改动)。SSE 的 ToolResult 沿用原值,前端按 images 渲染缩略图。
        let (tool_content, tool_images) = ctx.engine.images.parse_tool_output(&e.output);
        ctx.llm_messages.push(LlmMessage {
            role: "tool".into(),
            content: tool_content,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(e.call.id.clone()),
            images: tool_images,
        });
    }

    Ok(RoundOutcome {
        outputs: executed,
        interrupted: false,
    })
}

/// 按模型顺序把工具分成「并行组」:连续 Safe 工具为同一组(组内并发),
/// 非 Safe/未注册工具各自成组(串行屏障)。返回每组在 executed 里的下标集合。
fn split_parallel_groups(parallel_safe: &[bool]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (idx, &safe) in parallel_safe.iter().enumerate() {
        if safe {
            // Safe 且上一组也是 Safe 组 → 并入,否则新开一组
            if let Some(last) = groups.last_mut() {
                if parallel_safe[last[0]] {
                    last.push(idx);
                    continue;
                }
            }
            groups.push(vec![idx]);
        } else {
            // 非 Safe → 单独成组(屏障)
            groups.push(vec![idx]);
        }
    }
    groups
}

/// 串行执行单个工具调用(含未授权时的等待授权)。返回是否被中止。
/// 与并发组不同,非 Safe/未注册工具必须走此路径,保证授权语义与副作用顺序稳定。
/// `gate.no_ui_authorization = true`(任务模式)时,未放行工具直接拒绝并回灌错误,
/// 不进入 300 秒授权等待——任务模式没有 UI 授权上下文,等待必然超时。
#[allow(clippy::too_many_arguments)]
async fn execute_serial_tool(
    engine: &AgentEngine,
    e: &mut ExecutedTool,
    tool_ctx: &ToolContext,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    run_id: &str,
    gate: ToolGate<'_>,
) -> Result<bool, String> {
    if *abort.borrow() {
        return Ok(true);
    }
    if !e.permission.allowed && engine.tool_registry.get(&e.call.name).is_some() {
        // 任务模式(无 UI 授权上下文):立即拒绝,不空等
        if gate.no_ui_authorization {
            e.output = json!({
                "error": e.permission.reason,
                "code": "tool_policy_denied"
            });
            return Ok(false);
        }
        let receiver = engine.tool_registry.permissions().begin_wait(
            run_id,
            &e.call.id,
            &tool_ctx.session_id,
            &e.call.name,
        );
        send_event(
            SseEvent::ToolAuthorizationRequired {
                name: e.call.name.clone(),
                risk: e.permission.risk,
                reason: e.permission.reason.clone(),
                run_id: run_id.to_string(),
                call_id: e.call.id.clone(),
                origin: tool_origin_str(&engine.tool_registry, &e.call.name),
            },
            tx,
            abort,
            flag,
        )
        .await?;
        let mut abort_wait = abort.clone();
        let timeout_secs = {
            let s = engine.settings_snapshot();
            s.tool_authorization_timeout_secs as u64
        };
        let resolved = tokio::select! {
            decision = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), receiver) => {
                match decision {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(_)) => Err(("authorization_disconnected", "授权通道已断开")),
                    Err(_) => Err(("authorization_timeout", "等待授权超时")),
                }
            }
            changed = abort_wait.changed() => {
                let _ = changed;
                Err(("authorization_disconnected", "生成连接已断开"))
            }
        };
        engine
            .tool_registry
            .permissions()
            .cancel_wait(run_id, &e.call.id);
        match resolved {
            Ok(crate::tools::permissions::PendingAuthorizationDecision::AllowOnce) => {
                e.permission = crate::tools::permissions::PermissionDecision::allowed(
                    e.permission.risk,
                    "用户允许本次调用".into(),
                );
            }
            Ok(crate::tools::permissions::PendingAuthorizationDecision::AllowSession) => {
                // 授权落盘失败(如匿名会话/磁盘异常)不应中断整轮生成:降级为「仅本次允许」
                // 并留痕,否则用户看到的是生成失败而非授权失败。
                match engine.tool_registry.permissions().authorize(
                    &e.call.name,
                    "session",
                    &tool_ctx.session_id,
                ) {
                    Ok(()) => {
                        e.permission = crate::tools::permissions::PermissionDecision::allowed(
                            e.permission.risk,
                            "用户授权当前会话".into(),
                        );
                    }
                    Err(err) => {
                        tracing::warn!(tool = e.call.name, error = %err, "会话授权落盘失败,降级为仅本次允许");
                        e.permission = crate::tools::permissions::PermissionDecision::allowed(
                            e.permission.risk,
                            format!("用户允许本次调用(会话授权未保存:{err})"),
                        );
                    }
                }
            }
            Ok(crate::tools::permissions::PendingAuthorizationDecision::AllowRole) => {
                match engine.tool_registry.permissions().authorize(
                    &e.call.name,
                    "role",
                    &tool_ctx.character_id,
                ) {
                    Ok(()) => {
                        e.permission = crate::tools::permissions::PermissionDecision::allowed(
                            e.permission.risk,
                            "用户授权当前角色".into(),
                        );
                    }
                    Err(err) => {
                        tracing::warn!(tool = e.call.name, error = %err, "角色授权落盘失败,降级为仅本次允许");
                        e.permission = crate::tools::permissions::PermissionDecision::allowed(
                            e.permission.risk,
                            format!("用户允许本次调用(角色授权未保存:{err})"),
                        );
                    }
                }
            }
            Ok(crate::tools::permissions::PendingAuthorizationDecision::Deny) => {
                e.output =
                    json!({ "error": "用户拒绝工具调用", "code": "tool_authorization_denied" });
                emit_authorization_outcome(tx, abort, flag, &e.call.name, "已拒绝").await;
                return Ok(false);
            }
            Err((code, message)) => {
                e.output = json!({ "error": message, "code": code });
                emit_authorization_outcome(tx, abort, flag, &e.call.name, message).await;
                return Ok(false);
            }
        }
    }
    let (output, duration_ms, snapshot_id) = execute_call(
        engine,
        &e.call,
        &e.permission,
        tool_ctx,
        tx,
        abort,
        flag,
        run_id,
    )
    .await;
    e.output = output;
    e.duration_ms = duration_ms;
    e.snapshot_id = snapshot_id;
    Ok(false)
}

/// 授权终态事件(拒绝/超时/断开):原实现只回灌 error JSON,前端工具卡片显示为
/// 普通失败,用户无法区分「授权被拒」与「工具报错」。发一条 step 事件补足可见性。
async fn emit_authorization_outcome(
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    tool: &str,
    outcome: &str,
) {
    let _ = send_event(
        step_evt(
            &format!("工具 {tool} 未执行"),
            Some(format!("授权结果:{outcome}")),
            None,
            None,
        ),
        tx,
        abort,
        flag,
    )
    .await;
}

/// 工具执行的取消闸门(HARNESS3-1):工具 future 与 abort 信号竞争,先到的胜出。
///
/// 为什么在**调用侧**竞争,而不是给 `ToolRegistry::execute*` 加取消形参:
/// 执行器类型 `ToolExecutor` 是 L1 公共契约(`models::types::ToolExecutor`,
/// `tools/registry.rs:17-21` 的下沉说明写明「`plugins/` 与 `mcp/` 都要构造它」),
/// 加形参等于动 L1 类型 + 全部内置工具闭包 + 两个 L3 构造点,与本批「取消只在工具循环生效」
/// 的需求不成比例。
///
/// `None` = 已取消。竞争赢时工具 future 被 drop:对 `bash` 而言这意味着 exec 层的
/// future 被丢弃,进程由 `kill_on_drop` 与 Job Object 句柄关闭收掉
/// (见 `services/exec/desktop.rs`)。
///
/// 循环而非单次 `select!` 的原因:`changed()` 返回 `Err` 只表示**发送端已全部 drop**
/// (引擎 run 收尾),那不是取消信号——若把它当取消,正常完成的工具会被误判成
/// 「已取消」;若不管它,`changed()` 会立刻重复就绪导致忙循环。故 `Err` 分支改为
/// 等 future 自己跑完,`Ok` 分支回到循环顶重读当前值。
async fn await_or_cancel<F>(fut: F, abort: &watch::Receiver<bool>) -> Option<F::Output>
where
    F: std::future::Future,
{
    tokio::pin!(fut);
    let mut wait = abort.clone();
    loop {
        // borrow() 不标记 seen:标记会让克隆出的接收端再也等不到「变化」,
        // 于是「起跑前已取消」反而要等工具自然结束(正是本条要修的行为)。
        if *wait.borrow() {
            return None;
        }
        tokio::select! {
            biased;
            value = &mut fut => return Some(value),
            changed = wait.changed() => match changed {
                Ok(()) => continue,
                Err(_) => return Some((&mut fut).await),
            }
        }
    }
}

/// 执行单个工具调用(已裁决)。未授权不执行:未注册 → 错误 JSON;已注册 → 错误 JSON
/// (授权事件由收尾阶段按序推送)。执行失败 → 结构化错误 JSON + step 提示,不终止整轮。
/// 返回 (输出, 耗时毫秒, 回退快照 id)。`run_id` 用于构造工具调用身份(批 D-1),
/// 随快照落库并绑定到工具卡(SSE/tool_calls),供「回退到此处」精确匹配。
#[allow(clippy::too_many_arguments)]
async fn execute_call(
    engine: &AgentEngine,
    call: &ToolCallArgs,
    permission: &crate::tools::permissions::PermissionDecision,
    tool_ctx: &ToolContext,
    tx: &mpsc::Sender<SseEvent>,
    abort: &watch::Receiver<bool>,
    flag: &AbortFlag,
    run_id: &str,
) -> (Value, i64, Option<String>) {
    if !permission.allowed {
        return (
            if engine.tool_registry.get(&call.name).is_none() {
                json!({ "error": format!("工具未注册:{}", call.name), "code": "tool_not_registered" })
            } else {
                json!({ "error": permission.reason, "code": "tool_authorization_required" })
            },
            0,
            None,
        );
    }
    let start = std::time::Instant::now();
    // 白名单放行:已裁决执行,避免 execute 二次权限裁决拒绝白名单危险工具
    // (旧实现外层 allowed=true 但 execute 内部再裁决,敏感/危险工具实际仍被拒)
    let identity = crate::services::undo_service::ToolCallIdentity {
        run_id: run_id.to_string(),
        call_id: call.id.clone(),
    };
    let executed = await_or_cancel(
        engine.tool_registry.execute_with_decision_meta(
            &call.name,
            &call.arguments,
            tool_ctx.clone(),
            permission,
            Some(identity),
        ),
        abort,
    )
    .await;
    let (out, snapshot_id) = match executed {
        // 取消:回填结构化错误(形状与「执行失败」同款,不终止整轮),真正的收场
        // 由调用侧既有的轮末 / 串行路径 abort 检查负责。这里**不**返回 Err,
        // 否则整轮被判失败会把已完成的工作一起丢掉(口径同步骤预算的「带着产出收尾」)。
        None => {
            tracing::info!(tool = call.name.as_str(), "工具执行中被取消,提前结束等待");
            return (
                json!({
                    "error": "已取消:工具在执行中被中止(用户停止)",
                    "code": "tool_cancelled"
                }),
                start.elapsed().as_millis() as i64,
                None,
            );
        }
        Some(Ok(o)) => (
            serde_json::from_str(&o.output).unwrap_or(Value::String(o.output)),
            o.snapshot_id,
        ),
        Some(Err(e)) => {
            let _ = send_event(
                step_evt(
                    &format!("工具 {} 调用失败", call.name),
                    Some(e.clone()),
                    None,
                    None,
                ),
                tx,
                abort,
                flag,
            )
            .await;
            (json!({ "error": e }), None)
        }
    };
    (out, start.elapsed().as_millis() as i64, snapshot_id)
}

#[cfg(test)]
mod tests {
    use super::split_parallel_groups;

    /// 全 Safe:连续工具归入同一并行组
    #[test]
    fn split_parallel_groups_all_safe_one_group() {
        let flags = vec![true, true, true];
        assert_eq!(split_parallel_groups(&flags), vec![vec![0, 1, 2]]);
    }

    /// 非 Safe 是串行屏障:Safe 段被切成前后两组
    #[test]
    fn split_parallel_groups_barrier_splits_safe_runs() {
        let flags = vec![true, true, false, true];
        assert_eq!(
            split_parallel_groups(&flags),
            vec![vec![0, 1], vec![2], vec![3]]
        );
    }

    /// 两个非 Safe 各自成组(不合并)
    #[test]
    fn split_parallel_groups_no_safe_each_solo() {
        let flags = vec![false, false];
        assert_eq!(split_parallel_groups(&flags), vec![vec![0], vec![1]]);
    }

    #[test]
    fn split_parallel_groups_empty() {
        assert!(split_parallel_groups(&[]).is_empty());
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::await_or_cancel;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::watch;

    /// **正常路径必须仍然放行**(与下面两条同权重):取消守卫一旦误判,表现不是报错
    /// 而是「工具永远返回已取消」——比现状(取消打断不了工具)更糟,所以必须单独钉。
    #[tokio::test]
    async fn not_cancelled_returns_tool_output() {
        let (_tx, rx) = watch::channel(false);
        let got = await_or_cancel(async { "工具输出" }, &rx).await;
        assert_eq!(got, Some("工具输出"), "未取消时必须原样带回工具输出");
    }

    /// 本批的正面断言:工具执行途中取消 → **立即**返回,不等工具自身时长。
    /// 修复前该用例要等满 5s(工具自身 sleep)才返回,故时间余量给到 1s 足以判别。
    #[tokio::test]
    async fn cancel_during_tool_execution_returns_immediately() {
        let (tx, rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = tx.send(true);
        });
        let start = Instant::now();
        let got = await_or_cancel(tokio::time::sleep(Duration::from_secs(5)), &rx).await;
        let elapsed = start.elapsed();
        assert!(got.is_none(), "取消途中应返回 None(实际: {got:?})");
        assert!(
            elapsed < Duration::from_secs(1),
            "取消应在毫秒级生效,实际等了一轮工具时长: {elapsed:?}"
        );
    }

    /// 起跑前已置位:直接判取消,且**不轮询**工具 future(否则副作用已经发生才说「取消了」)。
    #[tokio::test]
    async fn already_aborted_before_start_skips_tool() {
        let (tx, rx) = watch::channel(true);
        let polled = Arc::new(AtomicBool::new(false));
        let flag = polled.clone();
        let got = await_or_cancel(
            async move {
                flag.store(true, Ordering::SeqCst);
                1u8
            },
            &rx,
        )
        .await;
        assert_eq!(got, None);
        assert!(
            !polled.load(Ordering::SeqCst),
            "已取消时不该把工具放下去跑(否则取消只是事后通知)"
        );
        drop(tx);
    }

    /// `changed()` 返回 Err 只代表**发送端已全部 drop**(run 收尾的正常路径),那不是取消
    /// 信号——必须等工具跑完并带回结果。此用例同时兜住「Err 分支被误写成 continue 造成忙循环」。
    #[tokio::test]
    async fn dropped_sender_is_not_treated_as_cancel() {
        let (tx, rx) = watch::channel(false);
        drop(tx);
        let start = Instant::now();
        let got = await_or_cancel(tokio::time::sleep(Duration::from_millis(40)), &rx).await;
        assert!(got.is_some(), "发送端断开不等于取消,应等工具完成");
        assert!(
            start.elapsed() >= Duration::from_millis(30),
            "应真的等完工具,而不是立刻返回"
        );
    }
}
