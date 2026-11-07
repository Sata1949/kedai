// 引擎事件桥:任务模式没有聊天 SSE 客户端,引擎(execute_generation/run_tool_loop)
// 只认 mpsc::Sender<SseEvent>。此处自建通道 + drain 任务,把引擎事件翻译为任务事件
// (SseEvent::Task kind=agent_status)经 TaskService broadcast 转发到任务事件流,
// 供「调用情况」面板/事件监控观察主 agent 进度(docs/功能.md 第三节·5)。
// label 为事件文案的执行者称谓(「主 agent」/team 的「主 agent N」/「子 agent」)。
// 批次 R4:Token 不再只计字数丢弃,改经 DeltaBatcher 攒批(200ms/80 字先到先发)
// 转发为 kind=delta 暂态事件,前端任务工作台据此实时显示「正在生成」;
// 攒批器携带 phase/step_index 调用上下文(与 task_llm_calls 落库行同口径)。
use crate::models::types::{SseEvent, TaskEventKind, TaskTraceKind};
use crate::services::task_core::{TaskBackend, DELTA_FLUSH_WINDOW};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// 事件桥通道容量:drain 即时消费(broadcast send 不阻塞),64 足以吸收流式 Token 突发。
const SINK_CAPACITY: usize = 64;

/// 事件 detail 截断上限(防异常长文本撑大 broadcast 帧)。
const DETAIL_MAX_CHARS: usize = 200;

/// 启动事件桥:返回 (引擎侧发送端, drain 任务句柄)。
/// 调用方在引擎循环结束后应先 drop 发送端再 await drain,保证事件全部转发完毕。
///
/// drain 必须持续消费到 tx 全部 drop(recv 返回 None)才退出:引擎侧 send_event
/// 在 rx 关闭时会置 abort 并中断整轮(对齐聊天路径「客户端断开即中断」语义)。
///
/// phase/step_index 为本桥对应调用的归属标识(run_agent_loop 恒 phase=agent,
/// team 主 agent 复用同桥、step_index 为子目标全局下标;custom 工具步骤 phase=step;
/// 子 agent phase=subagent),随 delta 事件透出供前端缓冲归键。
pub(crate) fn spawn(
    svc: Arc<dyn TaskBackend>,
    task_id: String,
    label: &str,
    phase: &str,
    step_index: Option<usize>,
) -> (mpsc::Sender<SseEvent>, JoinHandle<()>) {
    let label = label.to_string();
    let phase = phase.to_string(); // drain 任务要求 'static,引用先行 owned 化
    let (tx, mut rx) = mpsc::channel::<SseEvent>(SINK_CAPACITY);
    let drain = tokio::spawn(async move {
        // 流式正文攒批出口(批次 R4):Token 逐条到达,攒批(时间窗/字符阈值先到
        // 先发)后发射 kind=delta;不逐条直发是为保护 64 容量的任务事件 broadcast
        // (逐条会灌满通道,致 SSE 端 Lagged 连 status/llm_call 等关键事件一起丢)。
        let mut batcher = svc.delta_batcher(&task_id, &phase, step_index);
        // 时间窗闸门由定时 tick 驱动(字符阈值在 push 内即时判定)
        let mut tick = tokio::time::interval(DELTA_FLUSH_WINDOW);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // 流式正文字符聚合计数:Finish 收尾事件汇总携带(「流式输出 N 字」)
        let mut streamed_chars = 0usize;
        // 思考正文累积(TMSHOW-1):Reasoning 分块按**整轮一条**落轨迹,不逐块落库。
        // 为什么整轮一条:逐块会让一次思考变成几十行记录,前端无法成「一条思考消息」,
        // 也会把 64 容量的任务事件通道灌满——落库粒度必须等于展示粒度。
        let mut reasoning_buf = String::new();
        loop {
            tokio::select! {
                ev = rx.recv() => {
                    match ev {
                        Some(SseEvent::Token { text }) => {
                            streamed_chars += text.chars().count();
                            batcher.push(&text);
                        }
                        // 思考正文分块(TMSHOW-1):只累积,落库时机见 ToolCall 分支与 drain 收尾
                        Some(SseEvent::Reasoning { text }) => {
                            reasoning_buf.push_str(&text);
                        }
                        // 任务模式单次调用重试(TMR3-1):首次尝试已透出的正文增量收不回来,
                        // 重试会从头再流一遍。先丢弃本批并发 delta_reset,让前端清空该调用
                        // 归属的缓冲,再播报重试状态(TMR3-L2;聊天侧走自己的 Retry 展示)。
                        Some(SseEvent::Retry { attempt, max, reason }) => {
                            batcher.reset();
                            // 重试会从头再思考一遍:首次尝试的思考已无展示价值(正文同理被
                            // reset 丢弃),丢弃而不是拼接,避免同一段思考在时间线上出现两次。
                            reasoning_buf.clear();
                            let detail =
                                format!("{label} 上游重试:第 {attempt}/{max} 次({reason})");
                            svc.emit_event(
                                TaskEventKind::AgentStatus,
                                &task_id,
                                None,
                                None,
                                Some(detail.chars().take(DETAIL_MAX_CHARS).collect()),
                            );
                        }
                        Some(ev) => {
                            // 事件边界先落批:保证 delta 先于后续状态/收尾事件到达前端
                            batcher.flush();
                            // 工具调用前先落思考:时间线顺序 == 真实推理顺序
                            if matches!(ev, SseEvent::ToolCall { .. }) && !reasoning_buf.trim().is_empty() {
                                svc.record_trace(
                                    &task_id,
                                    &phase,
                                    step_index,
                                    TaskTraceKind::Reasoning,
                                    None,
                                    &reasoning_buf,
                                );
                                reasoning_buf.clear();
                            }
                            if let Some(detail) = map_event(&ev, streamed_chars, &label) {
                                svc.emit_event(TaskEventKind::AgentStatus, &task_id, None, None, Some(detail));
                            }
                            // 工具待确认(2026-11-06 三档授权):任务模式没有聊天 SSE,
                            // 不在这里翻译成任务事件,前端就只见一行 agent_status 文案、
                            // 无从点确认。翻译后仍保留上面的进展行(时间线连续性)。
                            if let SseEvent::ToolAuthorizationRequired {
                                name,
                                risk,
                                reason,
                                run_id,
                                call_id,
                                session_id,
                                code,
                                input_preview,
                                ..
                            } = &ev
                            {
                                svc.emit_tool_authorization_required(
                                    &task_id,
                                    crate::models::types::TaskToolAuthorization {
                                        name: name.clone(),
                                        risk: *risk,
                                        reason: reason.clone(),
                                        run_id: run_id.clone(),
                                        call_id: call_id.clone(),
                                        // 引擎事件已带精确 session_id;缺省回退到
                                        // task:{id}(旧引擎不该走到这里,但宁可给出可
                                        // resolve 的 id 也不要产生一条无法回填的卡片)
                                        session_id: session_id
                                            .clone()
                                            .unwrap_or_else(|| format!("task:{task_id}")),
                                        code: code.clone(),
                                        input_preview: input_preview.clone(),
                                    },
                                );
                            }
                            // 待授权已收束(2026-11-06):前端据此把确认卡出队,
                            // 不必等下一次全量刷新。
                            if let SseEvent::ToolAuthorizationResolved {
                                name,
                                run_id,
                                call_id,
                                session_id,
                                decision,
                                detail,
                            } = &ev
                            {
                                svc.emit_tool_authorization_resolved(
                                    &task_id,
                                    crate::models::types::TaskToolAuthorizationDecision {
                                        run_id: run_id.clone(),
                                        call_id: call_id.clone(),
                                        name: name.clone(),
                                        session_id: session_id
                                            .clone()
                                            .unwrap_or_else(|| format!("task:{task_id}")),
                                        decision: decision.clone(),
                                        detail: detail.clone(),
                                    },
                                );
                            }
                            // 工具轨迹(TMSHOW-1):入参与出参各落一行,前端按 seq 排序后
                            // 与思考条目并列显示;正文为空时 record_trace 内部早退。
                            if let Some((kind, name, content)) = trace_of(&ev) {
                                svc.record_trace(
                                    &task_id,
                                    &phase,
                                    step_index,
                                    kind,
                                    name.as_deref(),
                                    &content,
                                );
                            }
                        }
                        None => break,
                    }
                }
                _ = tick.tick() => batcher.flush_if_due(),
            }
        }
        // drain 收尾:末轮思考(其后再无工具调用)与不足一批的尾段都要发出
        if !reasoning_buf.trim().is_empty() {
            svc.record_trace(
                &task_id,
                &phase,
                step_index,
                TaskTraceKind::Reasoning,
                None,
                &reasoning_buf,
            );
        }
        batcher.flush();
    });
    (tx, drain)
}

/// 引擎事件 → 轨迹条目(TMSHOW-1);返回 None 表示本事件不产生轨迹。
///
/// 与 `map_event` 的分工是**内容 vs 通知**:`map_event` 产出「一行人类可读进展」
/// (200 字上限,只够说"还在动"),本函数产出**可渲染的完整正文**。同一条工具调用
/// 会走两者:一个进 agent_status 做活动指示,一个进 task_trace 做时间线。
///
/// 入参/出参统一渲染成 JSON 文本落 `content`,前端原样展示、不解析结构——
/// 轨迹表不为不同工具留结构化列是刻意的:工具形状由插件/MCP 决定,列会一直漂。
fn trace_of(ev: &SseEvent) -> Option<(TaskTraceKind, Option<String>, String)> {
    match ev {
        SseEvent::ToolCall { name, input, .. } => Some((
            TaskTraceKind::ToolCall,
            Some(name.clone()),
            render_payload(input),
        )),
        SseEvent::ToolResult { name, output, .. } => Some((
            TaskTraceKind::ToolResult,
            Some(name.clone()),
            render_payload(output),
        )),
        // 其余事件不产生轨迹:Token 是正文(已有 delta 通道与步骤产出)、
        // Step 是进度标题(agent_status 已承载)、Finish/Error/Interrupted 是终态通知。
        _ => None,
    }
}

/// 工具入参/出参 → 落库正文(UIFIX-8)。
///
/// **为什么不能直接 `to_string_pretty`**:流式路径把模型吐出的**原始参数字符串**当
/// `Value::String` 塞进 `SseEvent::ToolCall`(见 `executor/generation.rs` 的
/// `input: Value::String(call.arguments)`)。对字符串值做 pretty 只会输出「带引号的
/// 一行」:换行被转义成字面 `\n`、内层引号被转义成 `\"`,前端拿到就是整段挤在
/// 一行里的双重转义 JSON(实跑截图点名)。
///
/// 判别口径:字符串值**能二次解析成 JSON** 就说明它是「被当成字符串搬运的结构化
/// 参数」,解析后按结构美化;解析不了的才是真正的文本(bash 的 stdout、纯文字结果),
/// 原样保留——那类正文里的换行本来就该原样呈现。
fn render_payload(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .and_then(|parsed| serde_json::to_string_pretty(&parsed).ok())
            .unwrap_or_else(|| s.clone()),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// 引擎事件 → 任务事件 detail 文本映射;返回 None 表示不转发。
/// 仅覆盖 run_tool_loop 实际会产出的事件(Token/ToolCall/ToolResult/Step/授权请求),
/// 其余(Finish/Error/Interrupted 由聊天主流程发,任务模式走不到)防御性映射。
fn map_event(ev: &SseEvent, streamed_chars: usize, label: &str) -> Option<String> {
    let detail = match ev {
        SseEvent::Token { .. } => return None,
        SseEvent::Step { step, detail, .. } => match detail {
            Some(d) if !d.is_empty() => format!("{step}:{d}"),
            _ => step.clone(),
        },
        SseEvent::ToolCall { name, .. } => format!("{label} 调用工具 {name}"),
        // 被任务策略拒绝的工具要与正常返回区分开:否则调用情况里只看到「已返回结果」,
        // 用户不知道是策略拦截还是工具执行失败(tool_policy_denied 由闸门回灌)。
        SseEvent::ToolResult { name, output, .. } => {
            let denied = output
                .as_object()
                .and_then(|o| o.get("code"))
                .and_then(|c| c.as_str())
                .is_some_and(|code| code == "tool_policy_denied");
            if denied {
                let reason = output
                    .as_object()
                    .and_then(|o| o.get("error"))
                    .and_then(|e| e.as_str())
                    .unwrap_or("");
                if reason.is_empty() {
                    format!("工具 {name} 被任务策略拒绝")
                } else {
                    format!("工具 {name} 被任务策略拒绝:{reason}")
                }
            } else if let Some(text) = output
                .as_object()
                .and_then(|o| o.get("text"))
                .and_then(|t| t.as_str())
                .filter(|t| !t.trim().is_empty())
            {
                // 约定式返回(视觉工具等)自带摘要(含尺寸/差异统计):进展行直接采用,
                // 信息量远大于「已返回结果」;长度截断交既有 200 字符上限统一处理。
                format!("工具 {name}:{text}")
            } else {
                format!("工具 {name} 已返回结果")
            }
        }
        // 任务模式已改走策略闸门(未放行工具直接拒绝为 tool_policy_denied),
        // 正常不会产生授权请求;此映射仅作防御性兜底。
        SseEvent::ToolAuthorizationRequired { name, .. } => {
            format!("工具 {name} 需要授权(任务模式按策略处理)")
        }
        SseEvent::Finish { .. } => format!("{label} 生成完成(流式输出 {streamed_chars} 字)"),
        SseEvent::Error { message, .. } => format!("{label} 出错:{message}"),
        SseEvent::Interrupted => format!("{label} 已中断"),
        _ => return None,
    };
    Some(detail.chars().take(DETAIL_MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 策略拒绝的工具要与正常返回区分:调用情况里不能只显示「已返回结果」。
    #[test]
    fn policy_denied_tool_result_is_reported_as_rejected() {
        let ev = SseEvent::ToolResult {
            name: "write".into(),
            output: json!({ "error": "写入角色文件 a.md(宽松模式需授权)", "code": "tool_policy_denied" }),
            call_id: Some("c1".into()),
            render_kind: None,
            origin: None,
            snapshot_id: None,
        };
        let detail = map_event(&ev, 0, "主 agent").unwrap();
        assert!(detail.contains("被任务策略拒绝"), "实际:{detail}");
        assert!(
            detail.contains("写入角色文件 a.md"),
            "应带上拒绝理由,实际:{detail}"
        );
    }

    #[test]
    fn normal_tool_result_keeps_original_copy() {
        let ev = SseEvent::ToolResult {
            name: "read".into(),
            output: json!({ "ok": true }),
            call_id: Some("c1".into()),
            render_kind: None,
            origin: None,
            snapshot_id: None,
        };
        let detail = map_event(&ev, 0, "主 agent").unwrap();
        assert_eq!(detail, "工具 read 已返回结果");
    }

    /// UIFIX-8 的主症状:流式路径把模型原始参数字符串当 `Value::String` 塞进事件,
    /// 直接 pretty 序列化只会输出「带引号的单行 + 双重转义」,前端看到 `\n` 字面两字符。
    /// 修法判据:字符串值能二次解析成 JSON 就按结构美化。
    #[test]
    fn stringified_tool_arguments_are_reparsed_and_pretty_printed() {
        let raw = r#"{"command":"ls -la && ls -la parts 2>nul","cwd":"."}"#;
        let ev = SseEvent::ToolCall {
            name: "bash".into(),
            input: json!(raw),
            call_id: Some("c1".into()),
            render_kind: None,
            origin: None,
        };
        let (_, _, content) = trace_of(&ev).expect("工具调用应产生轨迹");
        // 多行:pretty 打出缩进与换行
        assert!(content.contains('\n'), "应为多行 JSON,实际:{content}");
        assert!(
            content.contains("\"command\": \"ls -la && ls -la parts 2>nul\""),
            "应还原成结构化字段,实际:{content}"
        );
        // 不再有「外层包一层引号 + 内层引号被判转义」的双重转义
        assert!(
            !content.contains(r#"\""#),
            "不应残留转义引号,实际:{content}"
        );
        assert!(
            !content.starts_with('"'),
            "不应整体被包成字符串,实际:{content}"
        );
    }

    /// bash stdout 是**真正的文本**,不是被搬运的结构化参数——不能二次解析,
    /// 原样保留(它自己的换行必须原样留着,前端 pre-wrap 才能正确分行)。
    #[test]
    fn plain_text_result_keeps_its_own_newlines() {
        let stdout = "total 8\ndrwxr-xr-x parts\n-rw-r--r-- pelican.svg";
        let ev = SseEvent::ToolResult {
            name: "bash".into(),
            output: json!(stdout),
            call_id: Some("c1".into()),
            render_kind: None,
            origin: None,
            snapshot_id: None,
        };
        let (_, _, content) = trace_of(&ev).expect("工具结果应产生轨迹");
        assert_eq!(content, stdout, "纯文本应原样保留");
        assert!(!content.contains(r#"\n"#), "换行不应被转义成字面 \\n");
    }

    /// 结构化 Value 的路径不受改动影响:仍是 pretty 多行。
    #[test]
    fn structured_value_still_prints_pretty() {
        let ev = SseEvent::ToolResult {
            name: "fs_glob".into(),
            output: json!({ "files": ["a.txt", "b.txt"], "total": 2 }),
            call_id: None,
            render_kind: None,
            origin: None,
            snapshot_id: None,
        };
        let (_, _, content) = trace_of(&ev).expect("工具结果应产生轨迹");
        assert!(content.contains('\n'));
        assert!(content.contains("\"total\": 2"));
    }

    /// 空字符串既不解析也不该报错,原样返回空(写入侧随后按空正文早退)。
    #[test]
    fn empty_string_payload_yields_empty_content() {
        assert_eq!(render_payload(&json!("")), "");
    }
}
