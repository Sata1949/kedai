// 核心类型的 serde 快照 / 线格式冻结用例（自 `types.rs` 整体搬出，2026-11-06）。
//
// 搬出理由：`types.rs` 已接近 file-size ratchet 基线（2509 行），而三档授权新增的
// 字段让它继续增长。测试与类型定义没有耦合关系，整段搬运零语义变化。
//
// 首行 `#![cfg(test)]` 是 check-arch 的「生产区切分」标记，勿移。
#![cfg(test)]

#[cfg(test)]
mod tests {
    use crate::models::types::*;

    /// serde 快照(批次 R2):TaskMessageRecord 序列化键集合与线格式
    /// (snake_case,与 TaskRecord/TaskLlmCallRecord 同风格;前端 TaskMessage 类型对齐)。
    #[test]
    fn task_message_record_serde_snapshot() {
        let rec = TaskMessageRecord {
            id: "m1".into(),
            task_id: "t1".into(),
            role: "user".into(),
            kind: "followup".into(),
            content: "再补充一点".into(),
            created_at: "2026-09-03T00:00:00.000Z".into(),
        };
        let v = serde_json::to_value(&rec).unwrap();
        // serde_json 未启用 preserve_order:Map 键按字典序,断言键集合(排序后)
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["content", "created_at", "id", "kind", "role", "task_id"],
            "字段集合快照: {keys:?}"
        );
        assert_eq!(v["role"], "user");
        assert_eq!(v["kind"], "followup");
        assert_eq!(v["content"], "再补充一点");
    }

    /// serde 快照:三个状态枚举序列化输出必须与历史 String 形态(API JSON/DB 落盘)
    /// 逐字节一致;as_str 与 from_str_lossy 同表往返。
    #[test]
    fn task_status_serde_snapshot() {
        let cases = [
            (TaskStatus::Pending, "pending"),
            (TaskStatus::Planning, "planning"),
            (TaskStatus::Running, "running"),
            (TaskStatus::Done, "done"),
            (TaskStatus::Partial, "partial"),
            (TaskStatus::Error, "error"),
            (TaskStatus::Ended, "ended"),
        ];
        for (status, text) in cases {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{text}\"")
            );
            assert_eq!(status.as_str(), text);
            assert_eq!(TaskStatus::from_str_lossy(text), status);
        }
    }

    /// serde 快照:llm_call 事件线格式(批次 3 新增 kind)。kind 为 Option<TaskEventKind>,
    /// 本测试锁定「type=task + kind=llm_call」帧形态(None 字段省略),防线格式漂移。
    #[test]
    fn sse_task_llm_call_event_wire_format() {
        let ev = SseEvent::Task {
            task_id: "t1".into(),
            kind: Some(TaskEventKind::LlmCall),
            title: None,
            status: None,
            detail: Some("step #1 · mock · 5 tokens".into()),
            finish_reason: None,
            phase: None,
            step_index: None,
            // PRODCAP-1:未持久化的事件构造 seq/at 省略(序列化断言据此保持原样)
            seq: None,
            at: None,
            // 轨迹载荷仅 kind=trace 携带(TMSHOW-1):本帧序列化时应省略该键
            trace: None,
            tool_authorization: None,
            tool_authorization_decision: None,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "task",
                "task_id": "t1",
                "kind": "llm_call",
                "detail": "step #1 · mock · 5 tokens"
            })
        );
    }

    /// serde 快照:trace 事件携带轨迹载荷(TMSHOW-1)。
    ///
    /// 本测试守两件事,都为**加性兼容**服务:
    /// 1. `kind=trace` 时 `trace` 键确实出现,且字段名/形状与 `TaskTraceRecord` 一致
    ///    (前端 `TaskTrace` 与之逐字段对拍,见 `tools/check-contract.mjs` 第 32 组);
    /// 2. 载荷的 `Box` 包装对线格式**完全透明**——序列化结果与内联时逐字节相同。
    ///    这条必须钉住:Box 只是为了压枚举体积(clippy::large_enum_variant),
    ///    若哪天有人因为「反正是 Box」就去改载荷结构,这里会先红。
    #[test]
    fn sse_task_trace_event_wire_format() {
        let ev = SseEvent::Task {
            task_id: "t1".into(),
            kind: Some(TaskEventKind::Trace),
            title: None,
            status: None,
            detail: Some("轨迹 · tool_call".into()),
            finish_reason: None,
            phase: Some("agent".into()),
            step_index: None,
            seq: None,
            at: Some("2026-11-01T00:00:00.000Z".into()),
            trace: Some(Box::new(TaskTraceRecord {
                id: 7,
                task_id: "t1".into(),
                seq: 3,
                phase: "agent".into(),
                step_index: None,
                kind: "tool_call".into(),
                name: Some("bash".into()),
                content: "{\"cmd\":\"ls\"}".into(),
                truncated: false,
                created_at: "2026-11-01T00:00:00.000Z".into(),
            })),
            // 待确认载荷仅对应两条独立 kind;本帧序列化时应省略这两个键
            tool_authorization: None,
            tool_authorization_decision: None,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "task",
                "task_id": "t1",
                "kind": "trace",
                "detail": "轨迹 · tool_call",
                "phase": "agent",
                "at": "2026-11-01T00:00:00.000Z",
                "trace": {
                    "id": 7,
                    "task_id": "t1",
                    "seq": 3,
                    "phase": "agent",
                    "step_index": null,
                    "kind": "tool_call",
                    "name": "bash",
                    "content": "{\"cmd\":\"ls\"}",
                    "truncated": false,
                    "created_at": "2026-11-01T00:00:00.000Z"
                }
            })
        );
    }

    /// serde 快照:llm_call 事件携带 finish_reason(可观测性问题①,截断标记透出)。
    /// Some 时字段出现,None 时省略(旧客户端兼容);同时锁定 TaskLlmCallRecord
    /// 的 finish_reason 字段出现在 calls 端点 JSON(旧行为 '' 空串)。
    #[test]
    fn sse_task_llm_call_event_with_finish_reason_wire_format() {
        let ev = SseEvent::Task {
            task_id: "t1".into(),
            kind: Some(TaskEventKind::LlmCall),
            title: None,
            status: None,
            detail: Some("agent · mock · 5 tokens".into()),
            finish_reason: Some("length".into()),
            phase: None,
            step_index: None,
            seq: None,
            at: None,
            // 轨迹载荷仅 kind=trace 携带(TMSHOW-1)
            trace: None,
            tool_authorization: None,
            tool_authorization_decision: None,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["finish_reason"], serde_json::json!("length"));

        let rec = TaskLlmCallRecord {
            id: "c1".into(),
            task_id: "t1".into(),
            phase: "agent".into(),
            step_index: None,
            model: "m".into(),
            prompt_summary: String::new(),
            response_summary: String::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
            reasoning_tokens: 0,
            elapsed_ms: 0,
            status: "ok".into(),
            finish_reason: String::new(),
            created_at: "c".into(),
        };
        let v = serde_json::to_value(&rec).unwrap();
        assert_eq!(
            v["finish_reason"],
            serde_json::json!(""),
            "旧行(未知)应以空串透出,不等于 stop"
        );
    }

    /// serde 快照:聊天终态 Finish 事件携带 finish_reason(可观测性问题①,2026-09-15)。
    /// Some 时字段出现,None 时省略——旧客户端忽略未知字段即可,线格式向后兼容。
    /// 锁定聊天路径不再丢弃截断信息(此前 SseEvent::Finish 只有 usage/content)。
    #[test]
    fn sse_finish_event_carries_finish_reason_wire_format() {
        let truncated = SseEvent::Finish {
            usage: TokenUsage::default(),
            content: "半截回复".into(),
            finish_reason: Some("length".into()),
        };
        let v = serde_json::to_value(&truncated).unwrap();
        assert_eq!(v["type"], serde_json::json!("finish"));
        assert_eq!(v["finish_reason"], serde_json::json!("length"));

        let normal = SseEvent::Finish {
            usage: TokenUsage::default(),
            content: "完整回复".into(),
            finish_reason: Some("stop".into()),
        };
        let v = serde_json::to_value(&normal).unwrap();
        assert_eq!(v["finish_reason"], serde_json::json!("stop"));

        // 未下发 finish_reason 时字段省略(不污染线格式,旧客户端兼容)
        let unknown = SseEvent::Finish {
            usage: TokenUsage::default(),
            content: "x".into(),
            finish_reason: None,
        };
        let v = serde_json::to_value(&unknown).unwrap();
        assert!(v.get("finish_reason").is_none(), "None 应省略字段: {v}");
    }

    /// serde 快照:delta 事件线格式(批次 R4 任务模式流式输出)。kind=delta +
    /// detail 攒批文本 + phase/step_index 调用归属标识;暂态事件不落库(纪律例外,
    /// 见 events.rs emit_delta),None 字段序列化省略,旧客户端缺字段即忽略。
    #[test]
    fn sse_task_delta_event_wire_format() {
        let ev = SseEvent::Task {
            task_id: "t1".into(),
            kind: Some(TaskEventKind::Delta),
            title: None,
            status: None,
            detail: Some("攒批后的正文增量".into()),
            finish_reason: None,
            phase: Some("step".into()),
            step_index: Some(0),
            seq: None,
            at: None,
            // delta 是暂态流式增量,与轨迹无关(TMSHOW-1)
            trace: None,
            tool_authorization: None,
            tool_authorization_decision: None,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "task",
                "task_id": "t1",
                "kind": "delta",
                "detail": "攒批后的正文增量",
                "phase": "step",
                "step_index": 0
            })
        );
    }

    /// serde 快照:llm_call 事件携带 phase/step_index(批次 R4:前端据此清对应
    /// 流式缓冲,delta 暂态数据由落库行取代对齐权威)。非步骤类阶段(planner 等)
    /// step_index 为 None,序列化省略该字段。
    #[test]
    fn sse_task_llm_call_event_with_phase_wire_format() {
        let ev = SseEvent::Task {
            task_id: "t1".into(),
            kind: Some(TaskEventKind::LlmCall),
            title: None,
            status: None,
            detail: Some("step #1 · mock · 5 tokens".into()),
            finish_reason: None,
            phase: Some("step".into()),
            step_index: Some(0),
            seq: None,
            at: None,
            // 轨迹载荷仅 kind=trace 携带(TMSHOW-1)
            trace: None,
            tool_authorization: None,
            tool_authorization_decision: None,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["phase"], serde_json::json!("step"));
        assert_eq!(v["step_index"], serde_json::json!(0));

        let ev = SseEvent::Task {
            task_id: "t1".into(),
            kind: Some(TaskEventKind::LlmCall),
            title: None,
            status: None,
            detail: Some("planner · mock · 5 tokens".into()),
            finish_reason: None,
            phase: Some("planner".into()),
            step_index: None,
            seq: None,
            at: None,
            // 轨迹载荷仅 kind=trace 携带(TMSHOW-1)
            trace: None,
            tool_authorization: None,
            tool_authorization_decision: None,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["phase"], serde_json::json!("planner"));
        assert!(
            v.get("step_index").is_none(),
            "step_index 为 None 时应省略: {v}"
        );
    }

    /// serde 快照:TaskRunMode 六模式枚举(批次 4 契约)。线格式 snake_case;
    /// 旧行/旧配置缺省 = legacy;DB 未知值 from_str_lossy 容错回退 Legacy 不 panic。
    #[test]
    fn task_run_mode_serde_snapshot() {
        let cases = [
            (TaskRunMode::Legacy, "legacy"),
            (TaskRunMode::Solo, "solo"),
            (TaskRunMode::Multi, "multi"),
            (TaskRunMode::Plan, "plan"),
            (TaskRunMode::Team, "team"),
            (TaskRunMode::Custom, "custom"),
        ];
        for (mode, text) in cases {
            assert_eq!(serde_json::to_string(&mode).unwrap(), format!("\"{text}\""));
            assert_eq!(mode.as_str(), text);
            assert_eq!(TaskRunMode::from_str_lossy(text), mode);
        }
        assert_eq!(
            TaskRunMode::from_str_lossy("未来未知模式"),
            TaskRunMode::Legacy
        );
    }

    /// TaskStatus::Planned(plan 模式待批准态)线格式 + as_str/from_str_lossy 往返。
    #[test]
    fn task_status_planned_serde() {
        assert_eq!(
            serde_json::to_string(&TaskStatus::Planned).unwrap(),
            "\"planned\""
        );
        assert_eq!(TaskStatus::Planned.as_str(), "planned");
        assert_eq!(TaskStatus::from_str_lossy("planned"), TaskStatus::Planned);
    }

    /// TaskRecord 缺省 task_mode = legacy:旧 API 体/旧 tasks 行 JSON 反序列化零迁移。
    #[test]
    fn task_record_task_mode_defaults_legacy() {
        let t: TaskRecord = serde_json::from_value(serde_json::json!({
            "id": "t", "title": "x", "created_at": "c", "updated_at": "u"
        }))
        .unwrap();
        assert_eq!(t.task_mode, TaskRunMode::Legacy);
    }

    /// TaskStepStatus 快照(序列化/as_str/往返)
    #[test]
    fn task_step_status_serde_snapshot() {
        let cases = [
            (TaskStepStatus::Pending, "pending"),
            (TaskStepStatus::Running, "running"),
            (TaskStepStatus::Done, "done"),
            (TaskStepStatus::Partial, "partial"),
            (TaskStepStatus::Error, "error"),
        ];
        for (status, text) in cases {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{text}\"")
            );
            assert_eq!(status.as_str(), text);
            assert_eq!(TaskStepStatus::from_str_lossy(text), status);
        }
    }

    /// TaskSubtaskStatus 快照(序列化/as_str/往返)
    #[test]
    fn task_subtask_status_serde_snapshot() {
        let cases = [
            (TaskSubtaskStatus::Pending, "pending"),
            (TaskSubtaskStatus::Running, "running"),
            (TaskSubtaskStatus::Done, "done"),
            (TaskSubtaskStatus::Error, "error"),
            (TaskSubtaskStatus::Ended, "ended"),
        ];
        for (status, text) in cases {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{text}\"")
            );
            assert_eq!(status.as_str(), text);
            assert_eq!(TaskSubtaskStatus::from_str_lossy(text), status);
        }
    }

    /// from_str_lossy 未知值回退 Pending(记 warn,不 panic)
    #[test]
    fn status_from_str_lossy_fallback() {
        assert_eq!(TaskStatus::from_str_lossy("进行中"), TaskStatus::Pending);
        assert_eq!(TaskStatus::from_str_lossy(""), TaskStatus::Pending);
        assert_eq!(
            TaskStepStatus::from_str_lossy("ended"),
            TaskStepStatus::Pending
        );
        assert_eq!(
            TaskSubtaskStatus::from_str_lossy("PARTIAL"),
            TaskSubtaskStatus::Pending
        );
    }

    /// plan JSON 反序列化容错:未知 status 不破坏整个 plan 解析(回退 Pending);
    /// 字段缺失走 serde default;合法值正常解析。
    #[test]
    fn task_step_status_deserialize_lossy() {
        let step: TaskStep =
            serde_json::from_str(r#"{"name":"一","goal":"g","status":"进行中"}"#).unwrap();
        assert_eq!(step.status, TaskStepStatus::Pending);
        let step: TaskStep = serde_json::from_str(r#"{"name":"一","goal":"g"}"#).unwrap();
        assert_eq!(step.status, TaskStepStatus::Pending);
        let step: TaskStep =
            serde_json::from_str(r#"{"name":"一","goal":"g","status":"done"}"#).unwrap();
        assert_eq!(step.status, TaskStepStatus::Done);
        // 序列化回写仍是 snake_case 文本(plan 落盘形态不变)
        let json = serde_json::to_string(&step).unwrap();
        assert!(
            json.contains(r#""status":"done""#),
            "plan JSON 形态: {json}"
        );
    }

    /// `TaskStep.node_id`(遗留.md IFW-5)的三条线格式护栏:
    /// ① None 时**整键省略**——legacy/solo/multi/plan/team 与存量 custom 任务的
    ///    plan JSON 因此逐字节不变;② 旧数据(无该键)能反序列化;③ 有值时往返不丢。
    #[test]
    fn task_step_node_id_is_additive_and_omitted_when_none() {
        // ① None → 不出现 "node_id"(本仓 plan 是 tasks.plan 列里的整段 JSON,
        //    多一个 null 键就是存量任务的字节变化)
        let step = TaskStep {
            name: "起草".into(),
            goal: "g".into(),
            status: TaskStepStatus::Pending,
            result: String::new(),
            node_id: None,
            files: Vec::new(),
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(!json.contains("node_id"), "None 时不应落键: {json}");

        // ② 旧 plan JSON(无该键)照常解析,得到 None
        let old: TaskStep =
            serde_json::from_str(r#"{"name":"起草","goal":"g","status":"done","result":"r"}"#)
                .unwrap();
        assert_eq!(old.node_id, None);

        // ③ custom 填入的节点 id 往返不丢
        let custom: TaskStep = serde_json::from_str(
            r#"{"name":"起草","goal":"g","status":"done","result":"r","node_id":"node-a"}"#,
        )
        .unwrap();
        assert_eq!(custom.node_id.as_deref(), Some("node-a"));
        let back = serde_json::to_string(&custom).unwrap();
        assert!(back.contains(r#""node_id":"node-a""#), "往返: {back}");
    }

    /// TMR-4:`TaskStep.files` 的加性线格式——空数组整键省略(存量 plan JSON 不变),
    /// 旧 JSON 无该键按空数组解析,有值时往返不丢。
    #[test]
    fn task_step_files_is_additive_and_omitted_when_empty() {
        let step = TaskStep {
            name: "写分片".into(),
            goal: "g".into(),
            ..TaskStep::default()
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(!json.contains("files"), "空数组时不应落键: {json}");

        let old: TaskStep =
            serde_json::from_str(r#"{"name":"旧","goal":"g","status":"done","result":"r"}"#)
                .unwrap();
        assert!(old.files.is_empty(), "旧 JSON 应按空数组解析");

        let with_files: TaskStep = serde_json::from_str(
            r#"{"name":"写分片","goal":"g","files":["parts/a.js","parts/b.js"]}"#,
        )
        .unwrap();
        assert_eq!(with_files.files, vec!["parts/a.js", "parts/b.js"]);
        let back = serde_json::to_string(&with_files).unwrap();
        assert!(
            back.contains(r#""files":["parts/a.js","parts/b.js"]"#),
            "往返: {back}"
        );
    }

    /// `PlanStep.connection_id` / `max_tool_rounds`(二维批次 5b)的三条线格式护栏:
    /// ① 两个字段为 None 时**整键省略**——存量 `agent_flows.json` 与聊天侧流程快照
    ///    逐字节不变;② 旧 JSON(无这两个键)能反序列化;③ 有值时往返不丢。
    /// 空串 connection_id 按「未设置」处理(与 `sub_flow_ref` 同口径)。
    #[test]
    fn plan_step_connection_and_tool_rounds_are_additive_and_omitted_when_none() {
        // ① None → 两个键都不出现(存量流程 JSON 是 bytes 级契约)
        let step = PlanStep {
            id: "s1".into(),
            name: "起草".into(),
            enabled: true,
            goal: "g".into(),
            action: "direct".into(),
            generates: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(!json.contains("connection_id"), "None 时不应落键: {json}");
        assert!(!json.contains("max_tool_rounds"), "None 时不应落键: {json}");

        // ② 旧 JSON(无这两个键)照常解析
        let old: PlanStep = serde_json::from_str(
            r#"{"id":"s1","name":"起草","enabled":true,"goal":"g","action":"direct"}"#,
        )
        .unwrap();
        assert_eq!(old.connection_id, None);
        assert_eq!(old.max_tool_rounds, None);
        assert_eq!(old.connection_ref(), None);

        // ③ 有值时往返不丢
        let custom: PlanStep = serde_json::from_str(
            r#"{"id":"s1","name":"起草","goal":"g","action":"direct","connection_id":"conn-b","max_tool_rounds":3}"#,
        )
        .unwrap();
        assert_eq!(custom.connection_ref(), Some("conn-b"));
        assert_eq!(custom.max_tool_rounds, Some(3));
        let back = serde_json::to_string(&custom).unwrap();
        assert!(back.contains(r#""connection_id":"conn-b""#), "往返: {back}");
        assert!(back.contains(r#""max_tool_rounds":3"#), "往返: {back}");

        // ④ 空白 id = 未设置(手改 JSON 的脏值不得当成「引用了空 id 的连接」)
        let blank: PlanStep =
            serde_json::from_str(r#"{"id":"s1","goal":"g","connection_id":"   "}"#).unwrap();
        assert_eq!(blank.connection_ref(), None);
    }

    /// `TaskRecord.flow_id`(二维批次 5a)的三条线格式护栏:
    /// ① None 时**整键省略**——非 custom 与存量任务的 task JSON 逐字节不变;
    /// ② 旧任务 JSON(无该键)能反序列化;③ 有值时往返不丢。
    #[test]
    fn task_flow_id_is_additive_and_omitted_when_none() {
        let mut task = TaskRecord {
            id: "t1".into(),
            title: "任务".into(),
            status: TaskStatus::Pending,
            plan: Vec::new(),
            result: String::new(),
            error: String::new(),
            character_id: None,
            created_at: "c".into(),
            updated_at: "u".into(),
            task_mode: TaskRunMode::Custom,
            executor_id: None,
            flow_id: None,
            flow_ids: None,
            connection_id: None,
            workspace: None,
            tool_authorization: None,
        };
        let json = serde_json::to_string(&task).unwrap();
        assert!(!json.contains("flow_id"), "None 时不应落键: {json}");

        // ② 旧 JSON(无该键)照常解析
        let old: TaskRecord = serde_json::from_str(
            r#"{"id":"t1","title":"任务","status":"done","plan":[],"result":"",
                "error":"","created_at":"c","updated_at":"u","task_mode":"custom"}"#,
        )
        .unwrap();
        assert_eq!(old.flow_id, None);

        // ③ 绑定值往返不丢
        task.flow_id = Some("flow-a".into());
        let back = serde_json::to_string(&task).unwrap();
        assert!(back.contains(r#""flow_id":"flow-a""#), "往返: {back}");
        let round: TaskRecord = serde_json::from_str(&back).unwrap();
        assert_eq!(round.flow_id.as_deref(), Some("flow-a"));
    }

    /// `TaskRecord.flow_ids`(二维批次 7b 对比模式名单)的三条线格式护栏,口径同 flow_id:
    /// ① None 时**整键省略**(强制模式与存量任务逐字节不变);② 旧 JSON 无该键能解析;
    /// ③ 有值时往返不丢且**顺序保持**——名单顺序决定工具描述里的列举顺序,
    /// 与根流程排除、可调用集判定同源,乱序会让同一份名单两次运行给出不同文案。
    #[test]
    fn task_flow_ids_is_additive_and_omitted_when_none() {
        let mut task = TaskRecord {
            id: "t1".into(),
            title: "任务".into(),
            status: TaskStatus::Pending,
            plan: Vec::new(),
            result: String::new(),
            error: String::new(),
            character_id: None,
            created_at: "c".into(),
            updated_at: "u".into(),
            task_mode: TaskRunMode::Custom,
            executor_id: None,
            flow_id: None,
            flow_ids: None,
            connection_id: None,
            workspace: None,
            tool_authorization: None,
        };
        let json = serde_json::to_string(&task).unwrap();
        assert!(!json.contains("flow_ids"), "None 时不应落键: {json}");

        // ② 旧 JSON(无 flow_ids 键)照常解析
        let old: TaskRecord = serde_json::from_str(
            r#"{"id":"t1","title":"任务","status":"done","plan":[],"result":"",
                "error":"","created_at":"c","updated_at":"u","task_mode":"custom"}"#,
        )
        .unwrap();
        assert_eq!(old.flow_ids, None);

        // ③ 往返不丢 + 顺序保持
        task.flow_ids = Some(vec!["f-b".into(), "f-a".into()]);
        let back = serde_json::to_string(&task).unwrap();
        assert!(back.contains(r#""flow_ids":["f-b","f-a"]"#), "往返: {back}");
        let round: TaskRecord = serde_json::from_str(&back).unwrap();
        assert_eq!(round.flow_ids, Some(vec!["f-b".into(), "f-a".into()]));
    }

    /// ExecScope 的先读后写(编码通道批次 1):未读拒绝、读后被外部改动拒绝、刷新后放行。
    /// stamp 用真实文件取((mtime, len)),不构造假时间戳——否则测不到「比对真实状态」。
    #[test]
    fn exec_scope_requires_read_before_write() {
        let tmp = crate::utils::test_support::TempDataDir::new("exec-scope");
        let file = tmp.join("a.txt");
        std::fs::write(&file, "v1").unwrap();
        let scope = ExecScope::new(tmp.path().to_path_buf(), true);

        // 未读 → 拒绝,文案指明先读
        let err = scope.check_read_before_write(&file).unwrap_err();
        assert!(err.contains("fs_read"), "文案应指明先读: {err}");

        // 读过且未变 → 放行
        scope.note_read(&file, ExecScope::stamp_of(&file).unwrap());
        assert!(scope.check_read_before_write(&file).is_ok());

        // 读后被外部改动(长度变化即命中,不依赖 mtime 分辨率)→ 拒绝并要求重读
        std::fs::write(&file, "v2-longer").unwrap();
        let err = scope.check_read_before_write(&file).unwrap_err();
        assert!(err.contains("重新"), "文案应要求重新读取: {err}");

        // 刷新读记录后重新放行(否则同一次 run 里连改两次会被自己拦住)
        scope.note_read(&file, ExecScope::stamp_of(&file).unwrap());
        assert!(scope.check_read_before_write(&file).is_ok());
    }

    /// 作用域携带的工作区与 jail 标志(与 TaskRunContext.scope = None 的分支配套)
    #[test]
    fn exec_scope_carries_workspace_and_jail_flag() {
        let tmp = crate::utils::test_support::TempDataDir::new("exec-scope-flags");
        let scope = ExecScope::new(tmp.path().to_path_buf(), false);
        assert_eq!(scope.workspace(), tmp.path());
        assert!(!scope.jail());
    }
}
