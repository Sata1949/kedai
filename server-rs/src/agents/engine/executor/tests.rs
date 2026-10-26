// executor 测试：自 executor.rs 搬出（C-2F-2 第一笔，纯搬运）。
// 首行 #![cfg(test)] 是 check-arch productionLineCount 的生产区切分标记，勿移。
#![cfg(test)]

use super::*;
use crate::models::llm_error::LlmError;
use std::time::Duration;
use tokio::sync::mpsc;

/// 任务模式名单是硬边界:名单外工具必须被闸门排除。
/// 回归用例——曾因三档文件规则(宽松模式写文件放行)而放过被策略排除的 write。
#[test]
fn task_gate_hard_excludes_tools_outside_whitelist() {
    let allowed = vec!["read".to_string(), "search".to_string()];
    let gate = ToolGate::listed(&allowed);
    assert!(gate.excludes("write"), "名单外 write 必须被排除");
    assert!(!gate.excludes("read"), "名单内 read 不应被排除");
}

/// 聊天路径不硬性排除名单外工具:走授权等待(与改造前行为一致)
#[test]
fn chat_gate_does_not_hard_exclude() {
    let allowed = vec!["read".to_string()];
    let chat = ToolGate {
        whitelist: Some(&allowed),
        no_ui_authorization: false,
    };
    assert!(!chat.excludes("write"), "聊天路径名单外工具应走授权等待");
}

/// 非白名单模式(whitelist=None)不排除任何工具
#[test]
fn wait_gate_excludes_nothing() {
    assert!(!ToolGate::wait().excludes("write"));
}

/// 空名单 = 拒绝一切(2026-09-24 修正)。回归用例——旧语义「空 = 全量放行」曾让
/// `custom` 节点在「策略 ∩ 白名单 = 空集」时退化成全开:模型臆造的工具调用被放行。
#[test]
fn empty_whitelist_denies_all() {
    let gate = ToolGate::listed(&[]);
    assert!(gate.excludes("write"), "空名单必须硬性排除一切工具");
    assert!(!gate.authorizes("write"));
}

/// C-2C:所有停止出口共用的 `ExecutorResult::stopped` 必须由 kind 派生
/// `budget_stopped`,且只有 4 个空转守卫经 `guard_stopped()` 透出
/// (预算/轮次上限不得被任务侧误降 partial)。
#[test]
fn stopped_result_derives_flags_from_kind() {
    use super::super::tool_loop::LoopStopKind;

    let cases = [
        (LoopStopKind::TokenBudget, true, false),
        (LoopStopKind::StepBudget, true, false),
        (LoopStopKind::RoundLimit, false, false),
        (LoopStopKind::Readonly, false, true),
        (LoopStopKind::NoProgress, false, true),
        (LoopStopKind::Semantic, false, true),
        (LoopStopKind::RepeatedCall, false, true),
    ];
    for (kind, budget_stopped, guard_stopped) in cases {
        let result = ExecutorResult::stopped(
            "已有正文".into(),
            Some("stop".into()),
            Vec::new(),
            LoopStop {
                kind,
                message: format!("{kind:?} 原因"),
                event_title: "事件",
                event_detail: String::new(),
            },
        );
        assert_eq!(result.budget_stopped, budget_stopped, "{kind:?}");
        assert_eq!(result.guard_stopped().is_some(), guard_stopped, "{kind:?}");
        assert_eq!(result.usage.total_tokens, 0, "停止处 usage 归零防双计");
        assert!(result.tool_calls.is_empty(), "停止处不再遗留待执行调用");
        assert_eq!(result.content, "已有正文", "停止处必须保留已有产出");
    }
}

// ===== 截断自愈(问题①)判定纯函数 =====

fn mk_result(content: &str, finish: Option<&str>, tool_calls: Vec<(&str, &str)>) -> ExecutorResult {
    ExecutorResult {
        content: content.into(),
        usage: TokenUsage::default(),
        interrupted: false,
        tool_calls: tool_calls
            .into_iter()
            .map(|(name, arguments)| ToolCallArgs {
                id: "c1".into(),
                name: name.into(),
                arguments: arguments.into(),
            })
            .collect(),
        reasoning: String::new(),
        finish_reason: finish.map(|s| s.into()),
        self_heals: Vec::new(),
        budget_stopped: false,
        stop: None,
    }
}

/// 半截 tool_call JSON + finish=length → 触发,成因指向工具参数截断
#[test]
fn heal_cause_detects_truncated_tool_call() {
    let r = mk_result(
        "",
        Some("length"),
        vec![("calculator", "{\"expression\": \"12*3")],
    );
    let cause = truncation_heal_cause(&r).expect("半截 tool_call 应触发自愈");
    assert!(cause.contains("工具参数 JSON 截断"), "{cause}");
    assert!(cause.contains("calculator"), "{cause}");
    // 正文非空但 tool_call 半截:同样触发(工具调用不可执行)
    let r2 = mk_result(
        "思考残余",
        Some("length"),
        vec![("agentgo", "{\"tasks\": [")],
    );
    assert!(truncation_heal_cause(&r2).is_some());
}

/// 空正文 + finish=length(推理烧光预算)→ 触发,成因为空内容
#[test]
fn heal_cause_detects_empty_content_length() {
    let r = mk_result("", Some("length"), vec![]);
    let cause = truncation_heal_cause(&r).unwrap();
    assert!(cause.contains("空内容"), "{cause}");
}

/// 不自愈的形态:普通文本截断(半截文本也是产出,既有语义)、正常收尾、
/// 合法 tool_call、无 finish_reason
#[test]
fn heal_cause_ignores_healthy_results() {
    // 半截文本截断:不触发(维持「截断仍 done」语义)
    assert!(
        truncation_heal_cause(&mk_result("这段成果被截断,后半", Some("length"), vec![])).is_none()
    );
    // 正常 stop:不触发
    assert!(truncation_heal_cause(&mk_result("", Some("stop"), vec![])).is_none());
    // length 但 tool_call 参数完整:正常执行,不触发
    assert!(truncation_heal_cause(&mk_result(
        "",
        Some("length"),
        vec![("calculator", "{\"expression\":\"1+1\"}")],
    ))
    .is_none());
    // 无 finish_reason:不触发
    assert!(truncation_heal_cause(&mk_result("", None, vec![])).is_none());
}

/// P2-1(2026-09-14):思考模型「推理耗尽预算」的成因必须被识别出来,
/// 与「输出预算本身不足」区分——前者需要一次给足预算,后者翻倍即可。
#[test]
fn heal_cause_identifies_reasoning_exhausted_budget() {
    let mut r = mk_result("", Some("length"), vec![]);
    r.reasoning = "推理内容很长".repeat(100);
    let cause = truncation_heal_cause(&r).unwrap();
    assert!(
        cause.contains("推理耗尽输出预算"),
        "应识别为推理挤占: {cause}"
    );
    assert!(cause.contains("字符"), "应带上推理长度便于诊断: {cause}");
}

/// 推理挤占时预算一次给足(而非仅翻倍),减少一次完整 LLM 往返;
/// 且严格受 CAP 封顶、不超上限。
#[test]
fn heal_budget_gives_enough_for_reasoning_exhaustion() {
    let mut r = mk_result("", Some("length"), vec![]);
    r.reasoning = "思考".repeat(200);
    // 已消耗 6000(推理占满),原预算 4000 → 翻倍仅 8000;
    // 推理感知应给 6000+4000=10000(新 CAP 131072 之下一路放行)
    r.usage.completion_tokens = 6000;
    let next = heal_budget_for(&r, 4000).unwrap();
    assert_eq!(next, 10000, "推理挤占时按已消耗+原预算给足");
    assert!(next > 8000, "应比单纯翻倍给得更多: {next}");
    // 封顶仍生效:给足值超过 CAP 时收敛到 CAP
    r.usage.completion_tokens = 200_000;
    assert_eq!(
        heal_budget_for(&r, 4000),
        Some(TRUNCATION_HEAL_MAX_TOKENS_CAP),
        "推理感知结果同样受 CAP 约束"
    );
}

/// 非推理形态维持既有「翻倍」语义(不改变原有行为)
#[test]
fn heal_budget_keeps_doubling_for_plain_truncation() {
    let r = mk_result("", Some("length"), vec![]);
    assert_eq!(heal_budget_for(&r, 1000), Some(2000), "无推理时仅翻倍");
    // 已封顶 → None(重发无意义)
    assert_eq!(heal_budget_for(&r, TRUNCATION_HEAL_MAX_TOKENS_CAP), None);
    assert_eq!(
        heal_budget_for(&r, 100_000),
        Some(TRUNCATION_HEAL_MAX_TOKENS_CAP),
        "翻倍后受 CAP 收敛"
    );
}

/// Err 形态判定:仅匹配连接器半截 tool_call flush 的专属文案
#[test]
fn heal_err_matches_connector_truncation_wording() {
    assert!(is_truncated_tool_call_error(
        "工具 \"agentgo\" 的 arguments 不是合法 JSON: {\"tasks\": ["
    ));
    assert!(!is_truncated_tool_call_error("上游连接超时"));
    assert!(!is_truncated_tool_call_error("生成已中断"));
}

/// 预算翻倍+封顶:1024→2048;8192→16384;封顶值 131072 之上不再重发(None)
#[test]
fn heal_budget_doubles_with_cap() {
    assert_eq!(doubled_heal_budget(1024), Some(2048));
    assert_eq!(doubled_heal_budget(4096), Some(8192));
    assert_eq!(doubled_heal_budget(8192), Some(16384));
    assert_eq!(doubled_heal_budget(131_072), None, "已封顶不重发");
    assert_eq!(doubled_heal_budget(u32::MAX), None, "饱和相乘不得溢出");
}

/// 2026-09-15 回归:封顶抬到 131072 后,子任务下限(16384)之上的单轮截断
/// 仍能翻倍重发;旧封顶 8192 会让 16384 判定「重发无意义」而静默放弃自愈。
#[test]
fn heal_budget_still_grows_above_new_subagent_floor() {
    assert_eq!(
        doubled_heal_budget(16_384),
        Some(32_768),
        "下限之上的截断必须仍可自愈(旧封顶 8192 会返回 None)"
    );
    assert_eq!(doubled_heal_budget(65_536), Some(131_072));
}

/// token 预算判定边界(HB-1):预算 0 恒不触发;「达到」即算超限(与轮次上限同口径)
#[test]
fn budget_reached_boundaries() {
    assert!(!budget_reached(0, 0), "预算 0 = 关闭");
    assert!(!budget_reached(9_999, 0), "预算 0 时与用量无关");
    assert!(!budget_reached(1023, 1024), "低于预算不触发");
    assert!(budget_reached(1024, 1024), "等于预算即触发(达到即算超限)");
    assert!(budget_reached(1025, 1024), "超过预算触发");
    assert!(budget_reached(i64::MAX, 1), "极值不溢出");
}

/// 步骤墙钟预算判定边界(D3):None 恒不触发(聊天路径不放这个字段,行为不变);
/// 「达到」即算用尽(与 token 预算/轮次上限同族口径)
#[test]
fn step_budget_reached_boundaries() {
    use std::time::Duration;
    assert!(
        !step_budget_reached(Duration::from_secs(9_999), None),
        "None = 不设预算,永不由本闸门收口"
    );
    assert!(
        !step_budget_reached(Duration::from_millis(999), Some(Duration::from_secs(1))),
        "低于预算不触发"
    );
    assert!(
        step_budget_reached(Duration::from_secs(1), Some(Duration::from_secs(1))),
        "等于预算即触发(达到即算用尽)"
    );
    assert!(
        step_budget_reached(Duration::from_secs(2), Some(Duration::from_secs(1))),
        "超过预算触发"
    );
}

/// 预算收尾的正文选择(D3):本轮有正文用本轮;只有工具调用则回退最近一次非空;
/// 两者皆空如实返回空(不伪造)
#[test]
fn budget_stop_content_prefers_current_then_falls_back() {
    assert_eq!(
        budget_stop_content("本轮正文".into(), "更早的正文"),
        "本轮正文",
        "本轮非空即用本轮"
    );
    assert_eq!(
        budget_stop_content(String::new(), "更早的正文"),
        "更早的正文",
        "本轮只有工具调用时回退最近非空正文(否则会被判「返回空内容」)"
    );
    assert_eq!(
        budget_stop_content("   \n".into(), "更早的正文"),
        "更早的正文",
        "纯空白等同空"
    );
    assert_eq!(
        budget_stop_content(String::new(), ""),
        "",
        "两者皆空如实返回空:不伪造正文"
    );
}

/// 任务模式虚拟 session(`task:` 前缀)必须启用总时长看门狗——这是「任务无限停在
/// running」缺口的封堵点,漏接线则该缺口原样存在。
#[test]
fn watchdog_enabled_for_task_sessions_only() {
    assert_eq!(
        call_watchdog_for("task:57cf15f9-0508-440a-b1a3-ae2762b58581"),
        Some(TASK_TOOL_LOOP_CALL_TIMEOUT),
        "任务模式必须启用"
    );
    assert_eq!(
        call_watchdog_for("task:abc:main:3:sub:def"),
        Some(TASK_TOOL_LOOP_CALL_TIMEOUT),
        "team/子 agent 的层叠 session 同样以 task: 开头,应一并启用"
    );
}

/// 聊天路径不得启用:长回复是正常形态,加总时长会把正常生成判成失败。
#[test]
fn watchdog_disabled_for_chat_sessions() {
    assert_eq!(call_watchdog_for("session-123"), None);
    assert_eq!(call_watchdog_for(""), None);
    // 边界:仅前缀匹配,含 task 但不以其开头的不算
    assert_eq!(call_watchdog_for("mytask:1"), None);
}

/// 节点级覆盖(A 批 A1):配了就必须**原样照用**——收紧(60s)与放宽(900s)都是合法用法,
/// 故实现不能写成 `min(覆盖, 缺省)`,否则「给慢节点放宽」这条诉求会被静默吃掉。
#[test]
fn node_call_timeout_overrides_default() {
    let tight = Duration::from_secs(60);
    let loose = Duration::from_secs(900);
    assert_eq!(
        resolve_call_watchdog("task:t1", Some(tight)),
        Some(tight),
        "任务会话:节点级覆盖优先于既有 300s"
    );
    assert_eq!(
        resolve_call_watchdog("task:t1", Some(loose)),
        Some(loose),
        "放宽用法同样生效(不是 min 语义)"
    );
    assert_eq!(
        resolve_call_watchdog("session-1", Some(tight)),
        Some(tight),
        "聊天会话:节点级覆盖也照用(该字段只由任务侧 PlanStep 提供,聊天侧恒为 None)"
    );
}

/// 未配覆盖时逐字节维持既有判定(任务会话 300s / 聊天会话无上限)。
#[test]
fn absent_node_call_timeout_keeps_default() {
    assert_eq!(
        resolve_call_watchdog("task:t1", None),
        Some(TASK_TOOL_LOOP_CALL_TIMEOUT)
    );
    assert_eq!(resolve_call_watchdog("session-1", None), None);
    assert_eq!(resolve_call_watchdog("", None), None);
}

/// 未超时:结果原样透出,错误路径不受影响。
#[tokio::test]
async fn watchdog_passes_through_when_within_limit() {
    let out: Result<u32, EngineError> =
        with_call_watchdog(async { Ok(42) }, Some(Duration::from_millis(500)), || {
            EngineError::Internal("不应触发".into())
        })
        .await;
    assert_eq!(out.unwrap(), 42);
}

/// 超时:放弃 future 并返回分类化超时错误(不是字符串猜测)。
#[tokio::test]
async fn watchdog_times_out_and_reports_timeout_kind() {
    let out: Result<u32, EngineError> = with_call_watchdog(
        async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(1)
        },
        Some(Duration::from_millis(50)),
        || {
            EngineError::Llm(crate::models::llm_error::LlmError::timeout(
                "模型调用超过 300s 未完成",
            ))
        },
    )
    .await;
    let err = out.expect_err("超过阈值必须超时");
    assert!(
        err.message().contains("超过 300s 未完成"),
        "实际:{}",
        err.message()
    );
    // 分类须为超时:任务侧据此落 error 行,聊天路径据此映射错误码
    match err {
        EngineError::Llm(e) => assert_eq!(
            e.kind(),
            crate::models::llm_error::LlmErrorKind::Timeout,
            "必须是超时分类,而不是内部错误"
        ),
        other => panic!("应为 EngineError::Llm(超时分类),实际 {other:?}"),
    }
}

/// None = 不限时:future 按原样等待,行为与裸调一致(聊天路径语义)。
#[tokio::test]
async fn watchdog_none_awaits_without_limit() {
    let out: Result<u32, EngineError> = with_call_watchdog(
        async {
            tokio::time::sleep(Duration::from_millis(80)).await;
            Ok(7)
        },
        None,
        || EngineError::Internal("不应触发".into()),
    )
    .await;
    assert_eq!(out.unwrap(), 7);
}

fn task_watchdog() -> Option<Duration> {
    Some(Duration::from_secs(300))
}

/// 可重试的三类上游失败 + 任务模式 + 未用尽次数 + 未取消 = 重试。
#[test]
fn retryable_task_failure_is_retried_once() {
    for err in [
        LlmError::timeout("超时"),
        LlmError::rate_limited("限流"),
        LlmError::upstream("流中断"),
    ] {
        let e = EngineError::Llm(err);
        assert!(
            should_retry_transport(&e, task_watchdog(), 0, false),
            "可重试上游失败应触发重试"
        );
        assert!(
            !should_retry_transport(&e, task_watchdog(), MAX_TRANSPORT_RETRIES, false),
            "次数用尽后不得再重试"
        );
    }
}

/// 聊天路径(watchdog=None)不重试:长回复是正常形态。
#[test]
fn chat_path_never_retries() {
    let e = EngineError::Llm(LlmError::timeout("超时"));
    assert!(!should_retry_transport(&e, None, 0, false));
}

/// 鉴权/参数类与内部错误不重试(与 EngineError::retryable 口径一致)。
#[test]
fn non_retryable_errors_are_not_retried() {
    for e in [
        EngineError::Llm(LlmError::auth_failed("鉴权失败")),
        EngineError::Llm(LlmError::generation("参数错误")),
        EngineError::Internal("引擎内部错误".into()),
        EngineError::Interrupted,
    ] {
        assert!(
            !should_retry_transport(&e, task_watchdog(), 0, false),
            "不可重试错误不得触发重试: {:?}",
            e
        );
    }
}

/// 已取消时不重试:用户停止优先,不能再用退避把收尾拖住。
#[test]
fn cancelled_never_retries() {
    let e = EngineError::Llm(LlmError::timeout("超时"));
    assert!(!should_retry_transport(&e, task_watchdog(), 0, true));
}

/// 截断工具调用错误不走传输重试:它有自己的「翻倍预算」自愈路径,
/// 原样重发同一预算只会再截断一次,白白多花一轮。
#[test]
fn truncated_tool_call_error_is_not_transport_retried() {
    let e = EngineError::Llm(LlmError::upstream(
        r#"工具 "fs_write" 的 arguments 不是合法 JSON"#,
    ));
    assert!(!should_retry_transport(&e, task_watchdog(), 0, false));
}

/// 旧调用方/聊天路径:缺省仍是 32 轮。
#[test]
fn missing_limit_keeps_default_32() {
    assert_eq!(resolve_max_rounds(None), Some(32));
}

/// 任务模式哨兵:Some(0) = 不限轮次。
#[test]
fn zero_means_unlimited() {
    assert_eq!(resolve_max_rounds(Some(0)), None);
}

/// 显式上限仍照常生效(聊天设置 / custom 节点级预算)。
#[test]
fn explicit_positive_limit_is_honored() {
    assert_eq!(resolve_max_rounds(Some(1)), Some(1));
    assert_eq!(resolve_max_rounds(Some(200)), Some(200));
}

fn declared() -> Vec<String> {
    vec!["parts/world.js".to_string(), "index.html".to_string()]
}

/// 只有成功意图命中声明路径的写工具才算进展。
#[test]
fn declared_workspace_write_is_progress() {
    assert!(is_declared_file_write(
        "fs_write",
        r#"{"path":"parts/world.js","content":"x"}"#,
        &declared()
    ));
    assert!(is_declared_file_write(
        "fs_edit",
        r#"{"path":"./parts\\world.js","old":"a","new":"b"}"#,
        &declared()
    ));
}

/// 临时文件、bash 探针、非法参数、空声明都不算进展(否则自检循环会被清零)。
#[test]
fn non_deliverable_writes_are_not_progress() {
    for (name, args) in [
        ("fs_write", r#"{"path":"_check.js","content":"x"}"#),
        ("fs_write", r#"{"path":"parts/other.js","content":"x"}"#),
        ("bash", r#"{"command":"node probe.cjs"}"#),
        ("fs_write", r#"{"path":"../outside.js"}"#),
        ("fs_write", r#"{"path":"C:/tmp/a.js"}"#),
        ("fs_write", r#"not-json"#),
    ] {
        assert!(
            !is_declared_file_write(name, args, &declared()),
            "{name} {args} 不应算声明文件进展"
        );
    }
    assert!(!is_declared_file_write(
        "fs_write",
        r#"{"path":"parts/world.js"}"#,
        &[]
    ));
}

/// TMR3-L6:台账增量(含 bash 写入)命中声明文件同样算进展。
#[test]
fn ledger_change_hits_declared_file() {
    let changed = vec!["parts/world.js".to_string(), "_probe.cjs".to_string()];
    assert!(has_declared_change(&declared(), &changed));
    // 只改临时文件 / 路径越界 / 空声明:不算进展
    assert!(!has_declared_change(
        &declared(),
        &["_probe.cjs".to_string()]
    ));
    assert!(!has_declared_change(
        &declared(),
        &["../outside.js".to_string()]
    ));
    assert!(!has_declared_change(&[], &changed));
}

/// TMR3-L1 的机理:外部累加器在 future 之外,看门狗 drop future 后仍保留
/// 「超时前已收到的 usage」。若把累加器放回 future 栈上,本用例必红。
#[tokio::test]
async fn timed_out_future_keeps_external_usage() {
    let mut live = TokenUsage::default();
    let out: Result<(), EngineError> = with_call_watchdog(
        async {
            live.prompt_tokens = 12;
            live.completion_tokens = 3;
            live.total_tokens = 15;
            std::future::pending::<Result<(), EngineError>>().await
        },
        Some(Duration::from_millis(30)),
        || {
            EngineError::Llm(crate::models::llm_error::LlmError::timeout(
                "模型调用超过 300s 未完成",
            ))
        },
    )
    .await;
    assert!(out.is_err(), "超过阈值必须超时");
    assert_eq!(
        live.prompt_tokens, 12,
        "future 被 drop 后,外部累加器必须保留已收用量"
    );
    assert_eq!(live.total_tokens, 15);
}

/// Usage 分块同时写入局部结果与外部实时累加器;成功路径由返回值入账,
/// 外部累加器只服务失败/超时补偿(此处只锁「都收到了」)。
#[tokio::test]
async fn usage_chunk_feeds_both_local_and_live_accumulator() {
    let (tx, _rx) = mpsc::channel::<SseEvent>(4);
    let (flag, abort) = AbortFlag::new();
    let registry = crate::tools::registry::ToolRegistry::new();
    let mut local = TokenUsage::default();
    let mut live = TokenUsage::default();
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    let mut finish_reason = None;
    let stopped = process_chunk(
        LlmStreamChunk::Usage {
            prompt_tokens: 10,
            completion_tokens: 4,
            total_tokens: 14,
            prompt_cache_hit_tokens: 2,
            prompt_cache_miss_tokens: 8,
            reasoning_tokens: 1,
        },
        &mut content,
        &mut reasoning,
        &mut calls,
        &mut local,
        Some(&mut live),
        &mut finish_reason,
        &registry,
        &tx,
        &abort,
        &flag,
        false,
    )
    .await
    .expect("Usage 分块不应失败");
    assert!(!stopped);
    for usage in [&local, &live] {
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 14);
        assert_eq!(usage.prompt_cache_hit_tokens, 2);
        assert_eq!(usage.prompt_cache_miss_tokens, 8);
    }
}

/// 失败尝试补记走同一累加实现,不能漏字段。
#[test]
fn add_usage_accumulates_every_field() {
    let mut total = TokenUsage::default();
    let delta = TokenUsage {
        prompt_tokens: 1,
        completion_tokens: 2,
        total_tokens: 3,
        context_tokens: 4,
        prompt_cache_hit_tokens: 5,
        prompt_cache_miss_tokens: 6,
    };
    add_usage(&mut total, &delta);
    assert_eq!(total.prompt_tokens, 1);
    assert_eq!(total.completion_tokens, 2);
    assert_eq!(total.total_tokens, 3);
    assert_eq!(total.context_tokens, 4);
    assert_eq!(total.prompt_cache_hit_tokens, 5);
    assert_eq!(total.prompt_cache_miss_tokens, 6);
}
