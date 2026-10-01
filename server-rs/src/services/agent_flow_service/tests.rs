//! `agent_flow_service` 的单元测试(由原文件内的 `#[cfg(test)] mod tests { … }` 整段搬来,原样保留)。
//!
//! `#![cfg(test)]` 必须留在**列首**且字面为 `#[cfg(test)]`:`tools/check-arch.mjs` 的
//! `productionLineCount`/`productionSource` 靠 `^#!?\[cfg\(test\)\]` 切出生产区,否则本文件会被
//! 当成生产代码,规则 E(`services` 生产代码禁用 `.expect(`,基线为空集)会把测试里的断言全部判红。
#![cfg(test)]
use super::*;
use crate::utils::test_support::TempDataDir;

fn tools() -> BTreeSet<String> {
    ["read", "search", "update_variables", "calculator"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn step(name: &str, action: &str, generates: Option<bool>) -> PlanStep {
    PlanStep {
        id: format!("s-{name}"),
        name: name.into(),
        enabled: true,
        goal: format!("{name} 目标"),
        action: action.into(),
        generates,
        ..Default::default()
    }
}

fn flow(name: &str, steps: Vec<PlanStep>) -> AgentFlowConfig {
    AgentFlowConfig {
        id: format!("flow-{name}"),
        name: name.into(),
        description: None,
        enabled: true,
        steps,
        max_parallel_nodes: None,
    }
}

#[test]
fn builtin_flow_step_tools_and_mvu_wording() {
    let f = builtin_flow();
    // 内置流程:起草(先计划后正文)→ 反思 → 修订(理解意图步已废弃)
    assert_eq!(f.steps.len(), 3);
    // 起草步:配 update_variables,提示词含「先写计划再输出正文」且点名工具
    let draft = &f.steps[0];
    assert_eq!(
        draft.tools.as_deref(),
        Some(&["update_variables".to_string()][..]),
        "起草步应实际下发 update_variables"
    );
    let dsp = draft.system_prompt.as_deref().unwrap_or("");
    assert!(
        dsp.contains("update_variables"),
        "起草步应点名 update_variables 工具: {dsp}"
    );
    assert!(
        dsp.contains("回退") && dsp.contains("<UpdateVariable>"),
        "起草步应说明文本协议是回退: {dsp}"
    );
    assert!(
        dsp.contains("200 字") && dsp.contains("计划") && dsp.contains("正文"),
        "起草步应先写 ≤200 字计划再输出正文: {dsp}"
    );
    // 反思步:无工具、无 system_prompt
    assert!(f.steps[1].tools.is_none());
    assert!(f.steps[1].system_prompt.is_none());
    // 修订步:update_variables + 回退协议
    let revise = &f.steps[2];
    assert_eq!(
        revise.tools.as_deref(),
        Some(&["update_variables".to_string()][..])
    );
    let rsp = revise.system_prompt.as_deref().unwrap_or("");
    assert!(
        rsp.contains("update_variables") && rsp.contains("回退"),
        "修订步应点名 update_variables 与回退协议: {rsp}"
    );
}

#[test]
fn valid_flow_passes() {
    let cfg = flow(
        "ok",
        vec![
            step("理解", "direct", Some(false)),
            step("生成", "direct", Some(true)),
            step("反思", "reflect", None),
        ],
    );
    assert!(validate_flow(&cfg, &tools()).is_ok());
}

#[test]
fn empty_flow_rejected() {
    // 未启用(开关关闭):允许保存空流程(用户可先编辑后启用)
    assert!(validate_flow(&AgentFlowConfig::default(), &tools()).is_ok());
    // 启用但无步骤 → 拒绝
    let mut cfg = flow("x", Vec::new());
    cfg.enabled = true;
    assert!(validate_flow(&cfg, &tools()).is_err());
    // 启用但全部步骤 disabled → 拒绝
    let mut cfg = flow("x", vec![step("禁用", "direct", Some(true))]);
    cfg.steps[0].enabled = false;
    assert!(validate_flow(&cfg, &tools()).is_err());
}

#[test]
fn no_generating_step_rejected() {
    let cfg = flow(
        "x",
        vec![
            step("理解", "direct", Some(false)),
            step("反思", "reflect", None),
        ],
    );
    assert!(validate_flow(&cfg, &tools()).is_err());
}

#[test]
fn unregistered_whitelist_tool_is_rejected() {
    let mut generation = step("生成", "direct", Some(true));
    generation.tools = Some(vec!["missing_tool".into()]);
    let error = validate_flow(&flow("x", vec![generation]), &tools()).unwrap_err();
    assert!(error.contains("未注册工具"), "实际错误: {error}");
}

#[test]
fn function_choice_must_reference_effective_tool() {
    let mut generation = step("生成", "direct", Some(true));
    generation.tools = Some(vec!["read".into()]);
    generation.tool_choice = Some("function".into());
    generation.tool_choice_function = Some("calculator".into());
    let error = validate_flow(&flow("x", vec![generation]), &tools()).unwrap_err();
    assert!(error.contains("不在有效工具内"), "实际错误: {error}");
}

#[test]
fn bad_action_rejected() {
    let cfg = flow("x", vec![step("怪步", "teleport", Some(true))]);
    assert!(validate_flow(&cfg, &tools()).is_err());
}

#[test]
fn reflect_with_prompt_rejected() {
    let cfg = flow(
        "x",
        vec![
            step("生成", "direct", Some(true)),
            PlanStep {
                system_prompt: Some("不应支持".into()),
                ..step("反思", "reflect", None)
            },
        ],
    );
    assert!(validate_flow(&cfg, &tools()).is_err());
}

#[test]
fn load_corrupted_file_falls_back_to_default() {
    let dir = TempDataDir::new("flow-test");
    std::fs::write(dir.join("agent_flows.json"), "{not json").unwrap();
    let lib = AgentFlowLibrary::load(&dir);
    assert!(lib.flows.is_empty());
}

#[test]
fn load_missing_file_creates_builtin() {
    let dir = TempDataDir::new("flow-missing");
    let lib = AgentFlowLibrary::load(&dir);
    assert_eq!(lib.flows.len(), 1);
    assert_eq!(lib.flows[0].id, "builtin-coordination");
    assert_eq!(lib.current_flow_id.as_deref(), Some("builtin-coordination"));
    assert!(validate_flow(&lib.flows[0], &tools()).is_ok());
}

#[test]
fn load_legacy_single_flow_migrates() {
    let dir = TempDataDir::new("flow-legacy");
    std::fs::write(
            dir.join("agent_flows.json"),
            r#"{"enabled":true,"steps":[{"id":"s1","name":"生成","enabled":true,"goal":"生成正文","action":"direct","generates":true}]}"#,
        )
        .unwrap();
    let lib = AgentFlowLibrary::load(&dir);
    assert_eq!(lib.flows.len(), 1);
    assert_eq!(lib.flows[0].steps.len(), 1);
    assert_eq!(lib.flows[0].name, "默认流程");
    assert_eq!(
        lib.current_flow_id.as_deref(),
        Some(lib.flows[0].id.as_str())
    );
}

#[test]
fn service_set_select_remove_roundtrip() {
    let dir = TempDataDir::new("flow-svc");
    // 关包构造(CODE-5 起 new 多一个开关参数):本用例只验读写回合,与包无关
    let mut svc = AgentFlowService::new(
        dir.path().to_path_buf(),
        tools().into_iter().collect(),
        false,
    );
    // 首次加载注入内置簇:协调 + 示范(FLOW-DEMO-1;示范与包开关无关)
    assert_eq!(svc.get_library().flows.len(), 2);

    // 新建流程(set 空 id → 自动分配)
    let cfg = flow("my", vec![step("生成", "direct", Some(true))]);
    let mut new_cfg = cfg.clone();
    new_cfg.id = String::new();
    svc.set(new_cfg).unwrap();
    let lib = svc.get_library();
    assert_eq!(lib.flows.len(), 3);
    let mine = lib.flows.iter().find(|f| f.name == "my").unwrap();
    assert!(!mine.id.is_empty());
    assert_eq!(lib.current_flow_id.as_deref(), Some(mine.id.as_str()));

    // 选择内置流程
    svc.select("builtin-coordination").unwrap();
    assert_eq!(svc.get().map(|f| f.name.as_str()), Some("文学创作协调流程"));

    // 删除当前流程 → 回退到第一个
    svc.remove("builtin-coordination").unwrap();
    assert_eq!(svc.get_library().flows.len(), 2);
    assert_eq!(
        svc.get_library().current_flow_id.as_deref(),
        Some(svc.get_library().flows[0].id.as_str())
    );

    // 重新加载(持久化验证):删过的协调流程不复活(并入账目已有其 id)
    let svc2 = AgentFlowService::new(
        dir.path().to_path_buf(),
        tools().into_iter().collect(),
        false,
    );
    assert_eq!(svc2.get_library().flows.len(), 2);
    assert!(svc2.flow_by_id("builtin-coordination").is_none());
}

// ===== 二维流程原语(二维批次 1) =====

/// 生成步骤(可带上游 id):二维测试的图节点
fn graph_step(id: &str, inputs: &[&str]) -> PlanStep {
    PlanStep {
        id: id.into(),
        name: format!("节点{id}"),
        enabled: true,
        goal: format!("{id} 目标"),
        action: "direct".into(),
        generates: Some(true),
        inputs: inputs.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn linear_flow_chains_previous_step_as_input() {
    // 存量一维流程:inputs 全空 → 线性兼容,输入即前一步(首步无输入)
    let steps = vec![
        step("起草", "direct", Some(true)),
        step("修订", "direct", Some(true)),
    ];
    assert!(is_linear_compat(&steps));
    let graph = resolve_graph(&steps).unwrap();
    assert_eq!(graph.inputs, vec![Vec::<usize>::new(), vec![0]]);
    // 拓扑序 == 数组顺序(就绪队列按下标升序),执行顺序与旧版一致
    assert_eq!(graph.order, vec![0, 1]);
}

#[test]
fn two_dimensional_flow_resolves_inputs_and_topo_order() {
    // 菱形:a、b 为源节点,c 合并两路,d 收口
    let steps = vec![
        graph_step("a", &[]),
        graph_step("b", &[]),
        graph_step("c", &["a", "b"]),
        graph_step("d", &["c"]),
    ];
    assert!(!is_linear_compat(&steps));
    let graph = resolve_graph(&steps).unwrap();
    assert_eq!(graph.inputs[2], vec![0, 1], "多父按数组下标升序");
    assert_eq!(graph.inputs[3], vec![2]);
    assert_eq!(graph.order, vec![0, 1, 2, 3]);
}

#[test]
fn multi_parent_order_is_by_index_and_deduped() {
    // 声明顺序 b→a 也不影响合并顺序(D3:按下标升序);同一父写两次只算一次
    let steps = vec![
        graph_step("a", &[]),
        graph_step("b", &[]),
        graph_step("c", &["b", "a", "a"]),
    ];
    let graph = resolve_graph(&steps).unwrap();
    assert_eq!(graph.inputs[2], vec![0, 1]);
}

#[test]
fn graph_edges_rejected_on_dangling_duplicate_and_self_loop() {
    let dangling = vec![graph_step("a", &[]), graph_step("b", &["zzz"])];
    let err = resolve_graph(&dangling).unwrap_err();
    assert!(err.contains("不存在或未启用"), "实际错误:{err}");

    let duplicate = vec![graph_step("a", &[]), graph_step("a", &[])];
    let err = resolve_graph(&duplicate).unwrap_err();
    assert!(err.contains("重复"), "实际错误:{err}");

    let self_loop = vec![graph_step("a", &["a"])];
    let err = resolve_graph(&self_loop).unwrap_err();
    assert!(err.contains("自身"), "实际错误:{err}");

    // 缺 id:二维模式按 id 连接,空 id 无法解析
    let mut no_id = graph_step("a", &[]);
    no_id.id = String::new();
    let err = resolve_graph(&[no_id, graph_step("b", &["a"])]).unwrap_err();
    assert!(err.contains("缺少 id"), "实际错误:{err}");
}

#[test]
fn cycle_rejected_and_nodes_named() {
    let steps = vec![
        graph_step("a", &["c"]),
        graph_step("b", &["a"]),
        graph_step("c", &["b"]),
    ];
    let err = resolve_graph(&steps).unwrap_err();
    assert!(err.contains("环"), "实际错误:{err}");
    assert!(
        err.contains("节点a") && err.contains("节点c"),
        "应点名环上节点:{err}"
    );
}

#[test]
fn output_node_prefers_explicit_then_sink() {
    // 显式标注优先于汇点:c 是唯一汇点,但 b 被标注为成果节点
    let steps = vec![
        graph_step("a", &[]),
        PlanStep {
            is_output: Some(true),
            ..graph_step("b", &[])
        },
        graph_step("c", &["b"]),
    ];
    let graph = resolve_graph(&steps).unwrap();
    assert_eq!(output_index(&steps, &graph.inputs), Some(1));

    // 未标注:取无后继汇点
    let sink = vec![graph_step("a", &[]), graph_step("b", &["a"])];
    let graph = resolve_graph(&sink).unwrap();
    assert_eq!(output_index(&sink, &graph.inputs), Some(1));
}

#[test]
fn output_node_skips_non_generating_sink_then_falls_back() {
    // 汇点是反思步(不生成)→ 回退最后一个生成步(存量线性流程 [生成, 反思] 语义)
    let reflect_tail = vec![
        step("生成", "direct", Some(true)),
        step("反思", "reflect", None),
    ];
    let graph = resolve_graph(&reflect_tail).unwrap();
    assert_eq!(output_index(&reflect_tail, &graph.inputs), Some(0));

    // 全流程无生成步 → None(调用方按「未产出任何成果」报错)
    let no_gen = vec![
        step("反思一", "reflect", None),
        step("反思二", "reflect", None),
    ];
    let graph = resolve_graph(&no_gen).unwrap();
    assert_eq!(output_index(&no_gen, &graph.inputs), None);
}

#[test]
fn validate_flow_rejects_broken_graph_and_unknown_kind() {
    // 环 → 400 文案点名节点
    let cyclic = flow(
        "cyclic",
        vec![graph_step("a", &["b"]), graph_step("b", &["a"])],
    );
    let err = validate_flow(&cyclic, &tools()).unwrap_err();
    assert!(err.contains("环"), "实际错误:{err}");

    // 悬空上游 → 拒绝
    let dangling = flow("dangling", vec![graph_step("a", &["missing"])]);
    assert!(validate_flow(&dangling, &tools()).is_err());

    // 节点档位(二维批次 6a):loose / strict 都有已实现的语义 → 放行
    for kind in ["loose", "strict"] {
        let ok = flow(
            "kind",
            vec![PlanStep {
                kind: Some(kind.into()),
                ..graph_step("a", &[])
            }],
        );
        assert!(validate_flow(&ok, &tools()).is_ok(), "kind={kind}");
    }

    // 未知档位 → 明确报错而非静默无效
    let unknown = flow(
        "unknown-kind",
        vec![PlanStep {
            kind: Some("medium".into()),
            ..graph_step("a", &[])
        }],
    );
    let err = validate_flow(&unknown, &tools()).unwrap_err();
    assert!(err.contains("档位"), "实际错误:{err}");
}

/// 节点级工具轮次上限(二维批次 5b):1-200 放行,越界明确报错并点名步骤;
/// 严格档下该参数无意义但**不拒绝**(与「严格档保留工具配置」同一纪律)。
#[test]
fn validate_flow_checks_node_tool_rounds_and_ignores_connection_refs() {
    for rounds in [1u32, 32, 200] {
        let ok = flow(
            "rounds",
            vec![PlanStep {
                max_tool_rounds: Some(rounds),
                ..graph_step("a", &[])
            }],
        );
        assert!(validate_flow(&ok, &tools()).is_ok(), "rounds={rounds}");
    }
    let over = flow(
        "rounds-over",
        vec![PlanStep {
            max_tool_rounds: Some(201),
            ..graph_step("a", &[])
        }],
    );
    let err = validate_flow(&over, &tools()).unwrap_err();
    assert!(
        err.contains("工具轮次上限") && err.contains("节点a"),
        "实际错误:{err}"
    );

    // 严格档 + 轮次上限:配置保留,不因「用不上」而拒绝
    let strict = flow(
        "rounds-strict",
        vec![PlanStep {
            kind: Some("strict".into()),
            max_tool_rounds: Some(5),
            ..graph_step("a", &[])
        }],
    );
    assert!(validate_flow(&strict, &tools()).is_ok());

    // 节点级连接**不做保存期校验**:流程可导出/跨机导入,连接是本机设置
    // (失效引用在运行期报错,不静默回退)。所以这里引用一个不存在的连接也必须放行。
    let foreign_ref = flow(
        "conn-ref",
        vec![PlanStep {
            connection_id: Some("本机不存在的连接".into()),
            ..graph_step("a", &[])
        }],
    );
    assert!(validate_flow(&foreign_ref, &tools()).is_ok());
}

/// 节点级上下文上限(二维批次 8):区间 256..=1_048_576;
/// **严格档同样校验**(它管输入装配,与工具面无关,不像 max_tool_rounds 那样只在宽松档有意义);
/// 挂载子流程的节点也不放行越界值(运行期旁路是执行语义,配置仍需合法)。
#[test]
fn validate_flow_checks_step_max_context_range() {
    for v in [MIN_STEP_MAX_CONTEXT, 4096, MAX_STEP_MAX_CONTEXT] {
        let ok = flow(
            "ctx",
            vec![PlanStep {
                max_context: Some(v),
                ..graph_step("a", &[])
            }],
        );
        assert!(validate_flow(&ok, &tools()).is_ok(), "max_context={v}");
    }
    for bad in [MIN_STEP_MAX_CONTEXT - 1, MAX_STEP_MAX_CONTEXT + 1] {
        let over = flow(
            "ctx-bad",
            vec![PlanStep {
                max_context: Some(bad),
                ..graph_step("a", &[])
            }],
        );
        let err = validate_flow(&over, &tools()).unwrap_err();
        assert!(
            err.contains("上下文上限") && err.contains("节点a") && err.contains(&bad.to_string()),
            "max_context={bad} 的实际错误:{err}"
        );
    }
    // 严格档 + 上限:有效(输入装配与档位无关)
    let strict = flow(
        "ctx-strict",
        vec![PlanStep {
            kind: Some("strict".into()),
            max_context: Some(1024),
            ..graph_step("a", &[])
        }],
    );
    assert!(validate_flow(&strict, &tools()).is_ok());
}

/// 存量流程 JSON 逐字节不变(二维批次 8 的加性字段纪律):
/// `max_context = None` 时**整键省略**,Level-1/5b 的字段集合一字不动。
#[test]
fn step_max_context_is_absent_from_wire_when_none() {
    let step = graph_step("a", &[]);
    let v = serde_json::to_value(&step).unwrap();
    assert!(
        v.get("max_context").is_none(),
        "None 必须整键省略(存量流程读写逐字节不变): {v}"
    );
    // 设值时才出现,且值原样往返
    let with_ctx = PlanStep {
        max_context: Some(4096),
        ..graph_step("a", &[])
    };
    let v2 = serde_json::to_value(&with_ctx).unwrap();
    assert_eq!(
        v2.get("max_context").and_then(serde_json::Value::as_u64),
        Some(4096)
    );
    let back: PlanStep = serde_json::from_value(v2).unwrap();
    assert_eq!(back.max_context, Some(4096));
}

#[test]
fn strict_step_wins_over_tools_config() {
    // 档位优先于 tools(二维批次 6a):严格节点配了工具也仍判为严格;
    // 「严格 + 工具」允许保存(工具配置保留以便切回宽松档,执行期不下发)
    let strict_with_tools = flow(
        "strict-tools",
        vec![PlanStep {
            kind: Some("strict".into()),
            tools: Some(vec!["read".into()]),
            ..graph_step("a", &[])
        }],
    );
    assert!(validate_flow(&strict_with_tools, &tools()).is_ok());
    assert!(strict_with_tools.steps[0].is_strict());

    // 缺省档位 = 宽松(存量流程语义不变)
    let mut plain = graph_step("a", &[]);
    assert!(!plain.is_strict());
    plain.kind = Some("loose".into());
    assert!(!plain.is_strict());
}

#[test]
fn parallel_limit_defaults_clamped_and_validated() {
    let mut cfg = flow("parallel", vec![graph_step("a", &[])]);
    // 缺省 2(保守:并行会成倍消耗 token,WF-11)
    assert_eq!(effective_max_parallel(&cfg), 2);
    // 显式值生效
    cfg.max_parallel_nodes = Some(4);
    assert_eq!(effective_max_parallel(&cfg), 4);
    // 执行期防护:越界取边界(1 = 完全串行)
    cfg.max_parallel_nodes = Some(0);
    assert_eq!(effective_max_parallel(&cfg), 1);
    cfg.max_parallel_nodes = Some(99);
    assert_eq!(
        effective_max_parallel(&cfg),
        MAX_PARALLEL_NODES_LIMIT as usize
    );
    // 保存期校验:越界直接 400,不静默夹取
    assert!(validate_flow(&cfg, &tools()).is_err());
    cfg.max_parallel_nodes = Some(0);
    assert!(validate_flow(&cfg, &tools()).is_err());
    cfg.max_parallel_nodes = Some(MAX_PARALLEL_NODES_LIMIT);
    assert!(validate_flow(&cfg, &tools()).is_ok());
}

#[test]
fn topo_order_is_min_index_ready_expansion() {
    // 拓扑序 = 反复取「当前就绪节点中下标最小者」的展开
    //(并行调度器的就绪队列按下标升序出队,用的正是同一条规则)
    let steps = vec![
        graph_step("丙", &["乙"]), // 下标 0:依赖下标 1
        graph_step("乙", &["甲"]), // 下标 1:依赖下标 2
        graph_step("甲", &[]),     // 下标 2:源节点
    ];
    let graph = resolve_graph(&steps).unwrap();
    assert_eq!(
        graph.order,
        vec![2, 1, 0],
        "应从下标最小的就绪节点开始,而不是数组声明顺序"
    );
    assert_eq!(graph.inputs[0], vec![1]);
    assert!(graph.inputs[2].is_empty());
}

// ===== 静态子图(二维批次 6b) =====

/// 挂载子流程的节点:图结构同 `graph_step`,只多一个 sub_flow_id
fn sub_step(id: &str, sub_id: &str) -> PlanStep {
    PlanStep {
        sub_flow_id: Some(sub_id.into()),
        ..graph_step(id, &[])
    }
}

fn library(flows: Vec<AgentFlowConfig>) -> AgentFlowLibrary {
    AgentFlowLibrary {
        current_flow_id: flows.first().map(|f| f.id.clone()),
        flows,
        seeded_pack_ids: Vec::new(),
    }
}

/// 保存流程并返回分配到的 id(库内按名字回查)
fn save_flow(svc: &mut AgentFlowService, name: &str, steps: Vec<PlanStep>) -> String {
    let mut cfg = flow(name, steps);
    cfg.id = String::new();
    svc.set(cfg).unwrap();
    svc.get_library()
        .flows
        .iter()
        .find(|f| f.name == name)
        .expect("刚保存的流程应在库内")
        .id
        .clone()
}

fn service(dir: &TempDataDir) -> AgentFlowService {
    // 关包构造:本文件绝大多数用例与能力包无关,保持「今天的内置集」是默认前提
    AgentFlowService::new(
        dir.path().to_path_buf(),
        tools().into_iter().collect(),
        false,
    )
}

#[test]
fn sub_flow_refs_reject_dangling_self_and_cycle() {
    // 悬空引用:本库没有该 id
    let lib = library(vec![flow("主", vec![sub_step("n1", "missing")])]);
    let err = validate_sub_flows(&lib, &tools()).unwrap_err();
    assert!(err.contains("不存在"), "实际错误:{err}");

    // 自引用:flow() 的 id 形如 flow-<名字>
    let lib = library(vec![flow("自", vec![sub_step("n1", "flow-自")])]);
    let err = validate_sub_flows(&lib, &tools()).unwrap_err();
    assert!(err.contains("自身"), "实际错误:{err}");

    // 跨流程环 A → B → A(图内环检测挡不住,必须按调用链拦)
    let lib = library(vec![
        flow("A", vec![sub_step("n1", "flow-B")]),
        flow("B", vec![sub_step("n1", "flow-A")]),
    ]);
    let err = validate_sub_flows(&lib, &tools()).unwrap_err();
    assert!(err.contains("环"), "实际错误:{err}");
    assert!(
        err.contains("A") && err.contains("B"),
        "应点名环上流程:{err}"
    );
}

#[test]
fn sub_flow_depth_limit_counts_nesting_levels() {
    // 入口 0 层 + 3 层子流程(A→B→C→D)= 上限内
    let ok = library(vec![
        flow("A", vec![sub_step("n", "flow-B")]),
        flow("B", vec![sub_step("n", "flow-C")]),
        flow("C", vec![sub_step("n", "flow-D")]),
        flow("D", vec![graph_step("n", &[])]),
    ]);
    assert!(
        validate_sub_flows(&ok, &tools()).is_ok(),
        "3 层嵌套应放行(上限 {MAX_SUB_FLOW_DEPTH})"
    );

    // 再加一层 E → 第 4 层,越界
    let too_deep = library(vec![
        flow("A", vec![sub_step("n", "flow-B")]),
        flow("B", vec![sub_step("n", "flow-C")]),
        flow("C", vec![sub_step("n", "flow-D")]),
        flow("D", vec![sub_step("n", "flow-E")]),
        flow("E", vec![graph_step("n", &[])]),
    ]);
    let err = validate_sub_flows(&too_deep, &tools()).unwrap_err();
    assert!(err.contains("嵌套超过"), "实际错误:{err}");
    assert!(
        err.contains(&MAX_SUB_FLOW_DEPTH.to_string()),
        "应点名上限:{err}"
    );
}

#[test]
fn referenced_flow_must_be_structurally_valid_regardless_of_enabled() {
    // 被引用流程没有启用的步骤 → 拒绝(结构规则复用 validate_flow)
    let mut empty = flow("空子", vec![graph_step("n", &[])]);
    empty.steps[0].enabled = false;
    let lib = library(vec![flow("主", vec![sub_step("n", "flow-空子")]), empty]);
    let err = validate_sub_flows(&lib, &tools()).unwrap_err();
    assert!(err.contains("引用的子流程"), "实际错误:{err}");

    // 被引用流程缺少生成步 → 同样拒绝
    let lib = library(vec![
        flow("主", vec![sub_step("n", "flow-反思子")]),
        flow("反思子", vec![step("反思", "reflect", None)]),
    ]);
    let err = validate_sub_flows(&lib, &tools()).unwrap_err();
    assert!(err.contains("缺少生成步骤"), "实际错误:{err}");

    // 被引用流程**未启用**:仍可作为子流程(
    // 「已启用」只决定它能否作为当前流程直接执行)
    let mut helper = flow("辅助", vec![graph_step("n", &[])]);
    helper.enabled = false;
    let lib = library(vec![flow("主", vec![sub_step("n", "flow-辅助")]), helper]);
    assert!(
        validate_sub_flows(&lib, &tools()).is_ok(),
        "未启用的流程仍可作为子流程被引用"
    );
}

#[test]
fn reflect_step_cannot_mount_sub_flow() {
    let cfg = flow(
        "x",
        vec![
            graph_step("gen", &[]),
            PlanStep {
                sub_flow_id: Some("flow-子".into()),
                ..step("反思", "reflect", None)
            },
        ],
    );
    let err = validate_flow(&cfg, &tools()).unwrap_err();
    assert!(err.contains("反思步骤,不支持挂载子流程"), "实际错误:{err}");
}

#[test]
fn blank_sub_flow_id_means_not_mounted() {
    // 空串按未挂载处理(与 inputs 同一「空值即缺省」口径):编辑器清空选择时不写脏引用
    let mut s = graph_step("n", &[]);
    s.sub_flow_id = Some("   ".into());
    assert!(!s.is_sub_flow());
    assert_eq!(s.sub_flow_ref(), None);
    s.sub_flow_id = Some(" flow-x ".into());
    assert!(s.is_sub_flow());
    assert_eq!(s.sub_flow_ref(), Some("flow-x"), "引用应 trim 后使用");
}

#[test]
fn set_rejects_edit_that_deepens_another_flows_chain() {
    // 「后门」用例:编辑 B 可能让引用 B 的 A 变深,只查被编辑流程自身会漏。
    let dir = TempDataDir::new("flow-subflow-deepen");
    let mut svc = service(&dir);
    let d = save_flow(&mut svc, "D", vec![graph_step("n", &[])]);
    let c = save_flow(&mut svc, "C", vec![sub_step("n", &d)]);
    let b = save_flow(&mut svc, "B", vec![sub_step("n", &c)]);
    save_flow(&mut svc, "A", vec![sub_step("n", &b)]);
    // 此刻 A→B→C→D 正好 3 层,合法
    // 把 D 改成再嵌一层 E:C 自身只深 2 层,但 A 的链变成 4 层 → 必须被拒
    let e = save_flow(&mut svc, "E", vec![graph_step("n", &[])]);
    let mut d_cfg = svc.flow_by_id(&d).unwrap().clone();
    d_cfg.steps = vec![sub_step("n", &e)];
    let err = svc.set(d_cfg).unwrap_err();
    assert!(err.contains("嵌套超过"), "实际错误:{err}");
    // 拒绝不落盘:库里 D 仍是原来的单节点
    assert_eq!(svc.flow_by_id(&d).unwrap().steps.len(), 1);
}

#[test]
fn remove_rejects_flow_referenced_as_sub_flow() {
    let dir = TempDataDir::new("flow-subflow-del");
    let mut svc = service(&dir);
    let referrer = save_flow(&mut svc, "主", vec![sub_step("n", "builtin-coordination")]);
    let err = svc.remove("builtin-coordination").unwrap_err();
    assert!(err.contains("子流程引用"), "实际错误:{err}");
    assert!(err.contains("主"), "应点名引用方:{err}");
    // 解除引用后即可删除
    let mut fixed = svc.flow_by_id(&referrer).unwrap().clone();
    fixed.steps[0].sub_flow_id = None;
    svc.set(fixed).unwrap();
    assert!(svc.remove("builtin-coordination").is_ok());
}

/// 把流程库写进临时数据目录(模拟「手改 data/agent_flows.json」这类脏数据来源)
fn write_library(dir: &TempDataDir, lib: &AgentFlowLibrary) {
    std::fs::write(
        dir.join("agent_flows.json"),
        serde_json::to_string_pretty(lib).unwrap(),
    )
    .unwrap();
}

#[test]
fn validate_checks_only_entry_chain_while_set_scans_whole_library() {
    let dir = TempDataDir::new("flow-validate-scope");
    // 库内有一个**无关**的非法流程(悬空引用)——只能来自手改文件/外部导入,
    // 因为 `set` 的全库校验不允许它经正常写入路径进库
    write_library(
        &dir,
        &library(vec![
            flow("入口", vec![graph_step("n", &[])]),
            flow("脏流程", vec![sub_step("bad", "missing-flow")]),
        ]),
    );
    let mut svc = service(&dir);
    let entry = svc.get().cloned().expect("当前流程应存在");
    assert_eq!(entry.name, "入口");

    // 执行期:只从入口起链 → 无关脏流程不拦(否则所有 custom 任务与聊天预览都起不来)
    assert!(
        svc.validate(&entry).is_ok(),
        "库内无关流程的脏引用不应拦死本次执行"
    );

    // 保存期:仍是全库口径 → 拒绝,且拒绝不落盘
    let err = svc
        .set(flow("新流程", vec![graph_step("n", &[])]))
        .unwrap_err();
    assert!(err.contains("不存在"), "实际错误:{err}");
    assert!(
        !svc.get_library().flows.iter().any(|f| f.name == "新流程"),
        "被拒的保存不应进库"
    );
}

#[test]
fn validate_rejects_broken_entry_chain() {
    let dir = TempDataDir::new("flow-validate-entry");
    // 入口流程**自己**引用了不存在的流程 → 可达链非法,执行期必须拒
    write_library(
        &dir,
        &library(vec![
            flow("入口", vec![sub_step("n", "missing")]),
            flow("旁支", vec![graph_step("n", &[])]),
        ]),
    );
    let svc = service(&dir);
    let entry = svc.get().cloned().unwrap();
    let err = svc.validate(&entry).unwrap_err();
    assert!(err.contains("不存在"), "实际错误:{err}");
}

#[test]
fn remove_ignores_references_from_disabled_steps() {
    // 引用口径与校验口径一致:只看**启用步骤**(停用步骤的残留引用不参与执行,
    // 也不参与保存期校验,拿它卡住删除只会让用户被一条不存在的约束拦住)
    let dir = TempDataDir::new("flow-subflow-del-disabled");
    let mut svc = service(&dir);
    let mut stale = sub_step("n", "builtin-coordination");
    stale.enabled = false;
    save_flow(&mut svc, "主", vec![stale, graph_step("g", &[])]);
    assert!(
        svc.remove("builtin-coordination").is_ok(),
        "停用步骤里的残留引用不应阻止删除"
    );

    // 启用步骤里的引用仍然阻止(既有保护不变)
    let dir2 = TempDataDir::new("flow-subflow-del-enabled");
    let mut svc2 = service(&dir2);
    save_flow(
        &mut svc2,
        "主",
        vec![sub_step("n", "builtin-coordination"), graph_step("g", &[])],
    );
    let err = svc2.remove("builtin-coordination").unwrap_err();
    assert!(err.contains("子流程引用"), "实际错误:{err}");
}

#[test]
fn expand_sub_flows_splices_nested_steps_with_name_prefix() {
    // 子流程两步线性;主流程三步线性(起草 → 挂子流程 → 收尾)
    let sub = flow("子", vec![graph_step("s1", &[]), graph_step("s2", &["s1"])]);
    let main = flow(
        "主",
        vec![
            graph_step("a", &[]),
            sub_step("b", "flow-子"),
            graph_step("c", &["b"]),
        ],
    );
    let lib = library(vec![main.clone(), sub]);
    let active: Vec<PlanStep> = main.steps.iter().filter(|s| s.enabled).cloned().collect();
    let out = expand_sub_flows(&lib, &main.id, &active).unwrap();
    let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "节点a",
            "【子流程「子」】节点s1",
            "【子流程「子」】节点s2",
            "节点c"
        ],
        "子流程节点就地展开,位置即外层拓扑序中的位置"
    );
    assert!(
        out.iter().all(|s| s.inputs.is_empty()),
        "展开产物一律清空 id 依赖(扁平列表已按拓扑序排好)"
    );
    assert!(
        out[1].sub_flow_id.is_none() && out[2].sub_flow_id.is_none(),
        "子图内部节点不应残留 sub_flow_id"
    );
}

#[test]
fn expand_sub_flows_rewrites_ids_per_mount_point() {
    // 同一子流程被两个节点挂载:展开后 id 必须全局唯一,否则聊天侧
    // make_custom_plan → resolve_graph 会以「步骤 id 重复」直接 400,
    // 而同一份流程在任务侧(custom)能正常跑完(任务侧不展开,走 run_sub_flow)。
    let sub = flow("子", vec![graph_step("s1", &[]), graph_step("s2", &["s1"])]);
    let main = flow(
        "主",
        vec![
            graph_step("a", &[]),
            sub_step("b", "flow-子"),
            graph_step("c", &["b"]),
            sub_step("d", "flow-子"),
            graph_step("e", &["d"]),
        ],
    );
    let lib = library(vec![main.clone(), sub]);
    let active: Vec<PlanStep> = main.steps.iter().filter(|s| s.enabled).cloned().collect();
    let out = expand_sub_flows(&lib, &main.id, &active).unwrap();
    let ids: Vec<&str> = out.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["a", "s1/s1", "s1/s2", "c", "s3/s1", "s3/s2", "e"],
        "子图步骤 id 按挂载位置加前缀(下标取挂载节点在本层数组中的位置),顶层 id 不变"
    );
    assert!(resolve_graph(&out).is_ok(), "展开产物 id 必须唯一且非空");

    // 父子撞 id(「复制流程」只换流程 id、steps 深拷贝 → 两边步骤 id 相同)
    let main2 = flow(
        "主2",
        vec![
            graph_step("a", &[]),
            sub_step("b", "flow-撞"),
            graph_step("c", &["b"]),
        ],
    );
    let clash = library(vec![main2.clone(), flow("撞", vec![graph_step("b", &[])])]);
    let out2 = expand_sub_flows(&clash, &main2.id, &main2.steps).unwrap();
    let ids2: Vec<&str> = out2.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids2, vec!["a", "s1/b", "c"], "父子同 id 也被前缀隔开");
}

#[test]
fn expand_sub_flows_self_check_reports_duplicate_ids() {
    // 极端配置:顶层步骤 id 与「子图步骤 + 派生前缀」同形。
    // 自检兜底给出点名 id 的中文错误,而不是让下游报「步骤 id 重复」。
    let sub = flow("子", vec![graph_step("leaf", &[])]);
    let main = flow(
        "主",
        vec![
            graph_step("a", &[]),
            sub_step("b", "flow-子"),
            graph_step("s1/leaf", &["b"]),
        ],
    );
    let lib = library(vec![main.clone(), sub]);
    let active: Vec<PlanStep> = main.steps.iter().filter(|s| s.enabled).cloned().collect();
    let err = expand_sub_flows(&lib, &main.id, &active).unwrap_err();
    assert!(err.contains("子流程展开后步骤 id 冲突"), "实际错误:{err}");
    assert!(err.contains("s1/leaf"), "应点名重复的 id:{err}");
}

#[test]
fn sub_flow_depth_limit_counts_ancestor_chain() {
    // 祖先链:保存期从**库里每个流程**起链,故 H 在最上层就让整条链超限。
    // 前端 flowAncestorDepth + stepSubFlowWarnings 的深度判定与这条等价。
    let too_deep = library(vec![
        flow("H", vec![sub_step("n", "flow-G")]),
        flow("G", vec![sub_step("n", "flow-F")]),
        flow("F", vec![sub_step("n", "flow-X")]),
        flow("X", vec![sub_step("n", "flow-W")]),
        flow("W", vec![graph_step("n", &[])]),
    ]);
    let err = validate_sub_flows(&too_deep, &tools()).unwrap_err();
    assert!(err.contains("嵌套超过"), "实际错误:{err}");
    assert!(
        err.contains("H") && err.contains("W"),
        "应点名整条链的两端:{err}"
    );

    // 去掉最上层的 H → 4 个流程 = 3 层,上限内
    let ok = library(vec![
        flow("G", vec![sub_step("n", "flow-F")]),
        flow("F", vec![sub_step("n", "flow-X")]),
        flow("X", vec![sub_step("n", "flow-W")]),
        flow("W", vec![graph_step("n", &[])]),
    ]);
    assert!(
        validate_sub_flows(&ok, &tools()).is_ok(),
        "3 层嵌套应放行(上限 {MAX_SUB_FLOW_DEPTH})"
    );
}

#[test]
fn expand_sub_flows_rejects_runtime_cycle_and_depth() {
    // 环(保存期已拒,此处是外部手改配置的运行期兜底)
    let a = flow("A", vec![sub_step("n", "flow-B")]);
    let lib = library(vec![a.clone(), flow("B", vec![sub_step("n", "flow-A")])]);
    let err = expand_sub_flows(&lib, &a.id, &a.steps).unwrap_err();
    assert!(err.contains("环"), "实际错误:{err}");

    // 深度:A→B→C→D→E(4 层嵌套)越界
    let deep = library(vec![
        flow("A", vec![sub_step("n", "flow-B")]),
        flow("B", vec![sub_step("n", "flow-C")]),
        flow("C", vec![sub_step("n", "flow-D")]),
        flow("D", vec![sub_step("n", "flow-E")]),
        flow("E", vec![graph_step("n", &[])]),
    ]);
    let entry = deep.flows[0].clone();
    let err = expand_sub_flows(&deep, &entry.id, &entry.steps).unwrap_err();
    assert!(err.contains("嵌套超过"), "实际错误:{err}");

    // 悬空引用
    let dangling = library(vec![flow("A", vec![sub_step("n", "missing")])]);
    let entry = dangling.flows[0].clone();
    let err = expand_sub_flows(&dangling, &entry.id, &entry.steps).unwrap_err();
    assert!(err.contains("不存在"), "实际错误:{err}");
}

#[test]
fn sub_flow_field_is_omitted_when_unset() {
    // 兼容承诺(二维批次 1):存量流程 JSON 读写逐字节不变——
    // 未挂子流程的步骤不得序列化出 sub_flow_id 键
    let s = graph_step("n", &[]);
    let text = serde_json::to_string(&s).unwrap();
    assert!(
        !text.contains("sub_flow_id"),
        "未挂子流程时不应出现该键:{text}"
    );
    let mounted = sub_step("n", "flow-x");
    let text = serde_json::to_string(&mounted).unwrap();
    assert!(text.contains("sub_flow_id"), "挂载时应落盘:{text}");
}

// ===== 任务用流程快照(二维批次 5a) =====

#[test]
fn snapshot_for_collects_transitive_sub_flow_closure() {
    let dir = TempDataDir::new("flow-snapshot-closure");
    let mut svc = service(&dir);
    // C(叶子)← B(挂 C)← A(挂 B):入口 A 的闭包应含 A + B + C,入口恒首个
    let c = save_flow(&mut svc, "C", vec![graph_step("n", &[])]);
    let b = save_flow(&mut svc, "B", vec![sub_step("n", &c)]);
    let a = save_flow(&mut svc, "A", vec![sub_step("n", &b)]);

    let snap = svc.snapshot_for(&a, &[]).unwrap();
    assert_eq!(snap.root_id, a, "根应为入口流程");
    let ids: Vec<&str> = snap.flows.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![a.as_str(), b.as_str(), c.as_str()],
        "闭包应逐层可达"
    );
    assert_eq!(snap.root().map(|f| f.id.as_str()), Some(a.as_str()));
    assert!(snap.flow_by_id(&c).is_some(), "闭包内可取到深层子流程");
    assert!(
        snap.flow_by_id("flow-不存在").is_none(),
        "闭包外的一律取不到(运行期子图解析只认快照)"
    );
}

#[test]
fn snapshot_for_rejects_unknown_and_disabled_flow() {
    let dir = TempDataDir::new("flow-snapshot-reject");
    write_library(
        &dir,
        &library(vec![flow("启用流程", vec![graph_step("n", &[])]), {
            let mut off = flow("停用流程", vec![graph_step("n", &[])]);
            off.enabled = false;
            off
        }]),
    );
    let svc = service(&dir);

    let err = svc.snapshot_for("flow-不存在", &[]).unwrap_err();
    assert!(err.contains("不存在"), "实际错误:{err}");
    // 未启用流程不可绑定:与执行期 current_flow 的 enabled 口径一致,提前拒绝
    let err = svc.snapshot_for("flow-停用流程", &[]).unwrap_err();
    assert!(err.contains("未启用"), "实际错误:{err}");
}

// ===== 对比模式名单(二维批次 7b) =====

/// 名单成员的**闭包**同样进冻结域:被调流程自带子流程时,子流程也必须取得到——
/// 否则「动态调用一套挂过子图的流程」会在运行期撞「快照外的一律取不到」。
/// 顺序:入口首个 → 名单按声明顺序 → 子流程按发现顺序。
#[test]
fn snapshot_with_members_extends_closure_and_keeps_order() {
    let dir = TempDataDir::new("flow-snapshot-members");
    let mut svc = service(&dir);
    let deep = save_flow(&mut svc, "深", vec![graph_step("n", &[])]);
    let member = save_flow(&mut svc, "被调", vec![sub_step("n", &deep)]);
    let main = save_flow(&mut svc, "主", vec![graph_step("n", &[])]);

    let snap = svc
        .snapshot_for(&main, std::slice::from_ref(&member))
        .unwrap();
    let ids: Vec<&str> = snap.flows.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![main.as_str(), member.as_str(), deep.as_str()],
        "入口首个 → 名单成员 → 成员自己的子流程"
    );
    assert_eq!(snap.root_id, main, "根仍是入口流程(名单不改根)");
}

/// 名单成员严格校验(创建期):不存在 / 未启用 / 结构不合法各自点名拒绝;
/// **根流程出现在名单里**同样拒绝(它正在执行,被调用必然撞环守卫)
#[test]
fn members_are_strictly_validated_at_creation() {
    let dir = TempDataDir::new("flow-members-strict");
    write_library(
        &dir,
        &library(vec![flow("主", vec![graph_step("n", &[])]), {
            let mut off = flow("停用", vec![graph_step("n", &[])]);
            off.enabled = false;
            off
        }]),
    );
    let svc = service(&dir);

    let err = svc.validate_members(&["flow-不存在".into()]).unwrap_err();
    assert!(err.contains("不存在"), "实际错误:{err}");
    let err = svc.validate_members(&["flow-停用".into()]).unwrap_err();
    assert!(err.contains("未启用"), "实际错误:{err}");
    // 结构不合法(启用步骤为空)的流程不得进名单:否则要等模型调用时才失败
    let mut broken = flow("坏", vec![]);
    broken.steps = vec![graph_step("n", &[])];
    broken.steps[0].enabled = false;
    let lib = library(vec![flow("主", vec![graph_step("n", &[])]), broken]);
    write_library(&dir, &lib);
    let svc = service(&dir);
    let err = svc.validate_members(&["flow-坏".into()]).unwrap_err();
    assert!(err.contains("结构不合法"), "实际错误:{err}");

    // 根流程在名单里 → 创建期即拒(名单与根的关系在绑定分支判定)
    let err = svc
        .snapshot_for("flow-主", &["flow-主".into(), "flow-主".into()])
        .unwrap_err();
    assert!(err.contains("不能包含根流程"), "实际错误:{err}");
}

/// 未绑定任务的**运行期**口径是宽松的:成员被删/停用即剔除,不阻断根流程执行
/// (「根流程照常跑」是主语义);严格口径只属于创建期
#[test]
fn runtime_snapshot_drops_unavailable_members() {
    let dir = TempDataDir::new("flow-members-lenient");
    let mut svc = service(&dir);
    let member = save_flow(&mut svc, "被调", vec![graph_step("n", &[])]);
    let main = save_flow(&mut svc, "主", vec![graph_step("n", &[])]);
    // 把「被调」停用 + 另给一个不存在的 id:两者都应被剔除,根流程照常
    {
        let mut lib = svc.get_library().clone();
        for f in lib.flows.iter_mut() {
            if f.id == member {
                f.enabled = false;
            }
        }
        write_library(&dir, &lib);
    }
    let svc = service(&dir);
    let snap = svc
        .current_snapshot(&[member.clone(), "flow-不存在".into()])
        .unwrap();
    let ids: Vec<&str> = snap.flows.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec![main.as_str()], "不可用的成员不进闭包: {ids:?}");
}

#[test]
fn frozen_snapshot_stays_valid_after_library_changes() {
    let dir = TempDataDir::new("flow-snapshot-frozen");
    let mut svc = service(&dir);
    let sub = save_flow(&mut svc, "子", vec![graph_step("n", &[])]);
    let main = save_flow(&mut svc, "主", vec![sub_step("n", &sub)]);
    let snap = svc.snapshot_for(&main, &[]).unwrap();

    // 库里把子流程改成悬空引用(只有手改文件能做到)→ 快照自身仍合法:
    // 被冻结的任务不该因为库里被改而失效
    let mut broken = flow("子", vec![sub_step("n", "missing")]);
    broken.id = sub.clone();
    write_library(
        &dir,
        &library(vec![
            {
                let mut m = flow("主", vec![sub_step("n", &sub)]);
                m.id = main.clone();
                m
            },
            broken,
        ]),
    );
    let svc2 = service(&dir);
    assert!(
        svc2.validate(svc2.get().unwrap()).is_err(),
        "实时库口径下入口链已非法(对照:证明上面确实把库改坏了)"
    );
    assert!(
        validate_snapshot(&snap, &tools()).is_ok(),
        "冻结快照的校验只看快照自身"
    );
}

// ===== 流程搬运:导出/导入(二维批次 7a) =====

/// 导出→导入空库:带子流程的流程必须整体搬走,引用不断。
#[test]
fn export_import_bundle_keeps_sub_flow_references_intact() {
    let dir = TempDataDir::new("flow-bundle-move");
    let mut svc = service(&dir);
    let sub = save_flow(&mut svc, "子", vec![graph_step("n", &[])]);
    let main = save_flow(&mut svc, "主", vec![sub_step("n", &sub)]);

    let bundle = svc.export_bundle(Some(&main)).unwrap();
    let flows = bundle.flows.clone();
    assert_eq!(flows.len(), 2, "闭包应带走入口 + 子流程");
    assert_eq!(
        bundle.root_id.as_deref(),
        Some(main.as_str()),
        "root_id = 入口流程"
    );
    assert_eq!(bundle.kedai_flow_bundle, FLOW_BUNDLE_VERSION);

    // 空库导入:不撞 id,故原 id 全部保留,引用自然不断
    let dir2 = TempDataDir::new("flow-bundle-move-target");
    let mut dst = service(&dir2);
    let report = dst
        .import_bundle(flows, Some(&main), ImportConflictMode::Rename)
        .unwrap();
    assert_eq!(report.imported, 2);
    assert_eq!(report.skipped, 0);
    assert!(report.renamed.is_empty(), "无冲突不应改 id");
    let got = dst.flow_by_id(&main).expect("入口流程应搬过来");
    assert_eq!(
        got.steps[0].sub_flow_ref(),
        Some(sub.as_str()),
        "子流程引用必须原样可达(旧的「只导单流程」会把这里变成悬空引用)"
    );
    assert_eq!(
        dst.get_library().current_flow_id.as_deref(),
        Some(main.as_str()),
        "导入后选中入口流程"
    );
}

/// 同 id 内容不同 → 新增并重映射,库里既有流程逐字节不变。
#[test]
fn import_remaps_conflicting_id_and_leaves_existing_flow_untouched() {
    let dir = TempDataDir::new("flow-bundle-conflict");
    let mut svc = service(&dir);
    let sub = save_flow(&mut svc, "子", vec![graph_step("n", &[])]);
    let main = save_flow(&mut svc, "主", vec![sub_step("n", &sub)]);
    let bundle = svc.export_bundle(Some(&main)).unwrap().flows;

    // 本机把两份都改掉(同 id 异内容)——**两份都要重映射**,才能真正验到引用改写
    for id in [&sub, &main] {
        let mut edited = svc.flow_by_id(id).unwrap().clone();
        edited.description = Some("本机改过".into());
        svc.set(edited).unwrap();
    }
    let before_main = serde_json::to_string(svc.flow_by_id(&main).unwrap()).unwrap();
    let before_sub = serde_json::to_string(svc.flow_by_id(&sub).unwrap()).unwrap();

    let report = svc
        .import_bundle(bundle, Some(&main), ImportConflictMode::Rename)
        .unwrap();
    assert_eq!(report.imported, 2, "两份都应新增");
    assert_eq!(report.renamed.len(), 2, "两份都因 id 冲突分配新 id");
    assert_eq!(report.skipped, 0);
    let new_of = |old: &str| -> String {
        report
            .renamed
            .iter()
            .find(|r| r.old_id == old)
            .unwrap_or_else(|| panic!("{old} 应在重映射清单里"))
            .new_id
            .clone()
    };
    let (new_sub, new_main) = (new_of(&sub), new_of(&main));
    assert_ne!(new_main, main);
    assert_eq!(
        svc.flow_by_id(&new_main).unwrap().steps[0].sub_flow_ref(),
        Some(new_sub.as_str()),
        "批内引用必须改写为新 id(不能指向本机那份被改过的流程)"
    );
    assert_eq!(
        svc.get_library().current_flow_id.as_deref(),
        Some(new_main.as_str()),
        "root_id 经重映射后仍选中导入的那一份"
    );
    // 本机既有两份逐字节不变(口径 1:只新增,绝不改写既有 id 的既有流程)
    assert_eq!(
        serde_json::to_string(svc.flow_by_id(&main).unwrap()).unwrap(),
        before_main
    );
    assert_eq!(
        serde_json::to_string(svc.flow_by_id(&sub).unwrap()).unwrap(),
        before_sub
    );
}

/// 跳过与重映射并存的边界:被跳过的那份仍指向本机编排(口径 1 的必然结果,如实锁定)。
#[test]
fn import_skips_identical_flow_and_adds_only_the_changed_one() {
    let dir = TempDataDir::new("flow-bundle-mixed");
    let mut svc = service(&dir);
    let sub = save_flow(&mut svc, "子", vec![graph_step("n", &[])]);
    let main = save_flow(&mut svc, "主", vec![sub_step("n", &sub)]);
    let bundle = svc.export_bundle(Some(&main)).unwrap().flows;

    // 只改子流程:入口内容与文件完全一致 → 跳过;子流程 → 重映射新增
    {
        let mut edited = svc.flow_by_id(&sub).unwrap().clone();
        edited.description = Some("本机改过".into());
        svc.set(edited).unwrap();
    }
    let size = svc.get_library().flows.len();
    let report = svc
        .import_bundle(bundle, Some(&main), ImportConflictMode::Rename)
        .unwrap();
    assert_eq!(report.imported, 1);
    assert_eq!(report.skipped, 1, "入口与文件逐字节相同 → 跳过而非新增");
    assert_eq!(report.renamed.len(), 1);
    assert_eq!(svc.get_library().flows.len(), size + 1);
    assert_eq!(
        svc.flow_by_id(&main).unwrap().steps[0].sub_flow_ref(),
        Some(sub.as_str()),
        "被跳过的那份(本机入口)仍指向本机的子流程——口径 1 下不允许改写既有流程"
    );
}

/// 同 id 同内容 → 跳过:重复导入同一份文件幂等,库规模不变。
#[test]
fn import_same_content_is_idempotent() {
    let dir = TempDataDir::new("flow-bundle-idempotent");
    let mut svc = service(&dir);
    let sub = save_flow(&mut svc, "子", vec![graph_step("n", &[])]);
    let main = save_flow(&mut svc, "主", vec![sub_step("n", &sub)]);
    let bundle = svc.export_bundle(Some(&main)).unwrap().flows;
    let size = svc.get_library().flows.len();
    let current = svc.get_library().current_flow_id.clone();

    let report = svc
        .import_bundle(bundle, Some(&main), ImportConflictMode::Rename)
        .unwrap();
    assert_eq!(report.imported, 0, "全部已存在 → 一个都不新增");
    assert_eq!(report.skipped, 2);
    assert!(report.renamed.is_empty());
    assert_eq!(svc.get_library().flows.len(), size, "库规模不变");
    assert_eq!(
        svc.get_library().current_flow_id,
        current,
        "一条都没导入 → 当前选择也不动"
    );
}

/// 校验失败 → 库逐字节不变(原子性)。
#[test]
fn import_rejects_dangling_reference_and_keeps_library_unchanged() {
    let dir = TempDataDir::new("flow-bundle-atomic");
    let mut svc = service(&dir);
    let keep = save_flow(&mut svc, "本机", vec![graph_step("n", &[])]);
    let before = std::fs::read_to_string(dir.join("agent_flows.json")).unwrap();
    let size = svc.get_library().flows.len();

    // 文件里的流程引用了「既不在文件里、本库也没有」的子流程
    let mut bad = flow("外来", vec![sub_step("n", "flow-不存在")]);
    bad.id = String::new();
    let err = svc
        .import_bundle(vec![bad], None, ImportConflictMode::Rename)
        .unwrap_err();
    assert!(
        err.contains("不存在") && err.contains("子流程"),
        "应点名悬空引用:{err}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("agent_flows.json")).unwrap(),
        before,
        "校验失败必须不落盘(旧逐个 PUT 路径做不到这点)"
    );
    assert_eq!(svc.get_library().flows.len(), size, "库内一份都没多");
    assert!(svc.flow_by_id(&keep).is_some());
}

/// 空批与批内重复 id 都明确拒绝(后者会让库内出现重复 id 的流程)。
#[test]
fn import_rejects_empty_batch_and_duplicate_ids_in_file() {
    let dir = TempDataDir::new("flow-bundle-badinput");
    let mut svc = service(&dir);
    let size = svc.get_library().flows.len();
    let err = svc
        .import_bundle(Vec::new(), None, ImportConflictMode::Rename)
        .unwrap_err();
    assert!(err.contains("没有流程"), "实际错误:{err}");

    let dup_a = flow("重复", vec![graph_step("n", &[])]);
    let dup_b = flow("重复", vec![graph_step("m", &[])]);
    assert_eq!(dup_a.id, dup_b.id, "helper 给的是同名同 id");
    let err = svc
        .import_bundle(vec![dup_a, dup_b], None, ImportConflictMode::Rename)
        .unwrap_err();
    assert!(err.contains("重复"), "实际错误:{err}");
    assert_eq!(svc.get_library().flows.len(), size, "两次都被拒,库不变");
}

/// 全库导出:不校验、不缩水(脏引用也照导),root_id 取当前选择。
#[test]
fn export_whole_library_dumps_every_flow() {
    let dir = TempDataDir::new("flow-bundle-all");
    let mut svc = service(&dir);
    let a = save_flow(&mut svc, "A", vec![graph_step("n", &[])]);
    let b = save_flow(&mut svc, "B", vec![graph_step("n", &[])]);
    svc.select(&a).unwrap();
    let size = svc.get_library().flows.len();

    let bundle = svc.export_bundle(None).unwrap();
    let flows = bundle.flows.clone();
    assert_eq!(flows.len(), size, "全库导出应带走每一份流程(含内置流程)");
    assert_eq!(
        bundle.root_id.as_deref(),
        Some(a.as_str()),
        "root_id = 当前选中流程"
    );
    assert!(flows.iter().any(|f| f.id == b), "非当前的流程同样要在包里");
}

/// 未启用流程同样能导出(搬运路径不看 enabled):草稿与辅助子流程都要能带走。
#[test]
fn export_bundle_allows_disabled_flow_but_unknown_id_errors() {
    let dir = TempDataDir::new("flow-bundle-disabled");
    write_library(
        &dir,
        &library(vec![{
            let mut off = flow("停用的", vec![graph_step("n", &[])]);
            off.enabled = false;
            off
        }]),
    );
    let svc = service(&dir);
    let bundle = svc.export_bundle(Some("flow-停用的")).unwrap();
    assert_eq!(
        bundle.flows.len(),
        1,
        "停用流程应可导出(与 snapshot_for 的 enabled 口径不同)"
    );
    let err = svc.export_bundle(Some("flow-不存在")).unwrap_err();
    assert!(err.contains("不存在"), "实际错误:{err}");
}

/// 被跳过的子流程:批内对它的引用必须改指向**本库那份等价副本**,否则会留下悬空引用
/// (入口被导入、子流程被跳过 = 校验必然失败)。
#[test]
fn import_rewrites_reference_to_existing_copy_when_sub_flow_is_skipped() {
    let dir = TempDataDir::new("flow-bundle-skip-rewrite");
    // 本库:子内容(C)挂在 other-sub 这个 id 下;入口 old-main 引用它(库自身合法)。
    // 注意 old-sub 这个 id 本库**没有**,但同内容已存在 → 导入时会被跳过。
    let mut local_sub = flow("子", vec![graph_step("n", &[])]);
    local_sub.id = "other-sub".into();
    let mut local_main = flow("主", vec![sub_step("n", "other-sub")]);
    local_main.id = "old-main".into();
    write_library(&dir, &library(vec![local_main.clone(), local_sub]));
    let mut svc = service(&dir);

    // 文件:入口引用 old-sub(内容与本库入口不同 → 会被导入并重映射),
    // 子流程 id 为 old-sub(内容与本库 other-sub 相同 → 被跳过)
    let mut incoming_main = flow("主", vec![sub_step("n", "old-sub")]);
    incoming_main.id = "old-main".into();
    let mut incoming_sub = flow("子", vec![graph_step("n", &[])]);
    incoming_sub.id = "old-sub".into();
    let report = svc
        .import_bundle(
            vec![incoming_main, incoming_sub],
            Some("old-main"),
            ImportConflictMode::Rename,
        )
        .expect("跳过的子流程必须改指向本库等价副本,而不是让校验失败");

    assert_eq!(report.imported, 1);
    assert_eq!(report.skipped, 1, "同内容的子流程应被跳过");
    assert_eq!(report.renamed.len(), 1, "只有入口因 id 冲突重映射");
    let new_main = report.renamed[0].new_id.clone();
    assert_eq!(
        svc.flow_by_id(&new_main).unwrap().steps[0].sub_flow_ref(),
        Some("other-sub"),
        "批内引用应改指向本库那份等价副本"
    );
    assert_eq!(
        svc.get_library().current_flow_id.as_deref(),
        Some(new_main.as_str()),
        "root_id 经重映射后仍选中导入的那一份"
    );
}

/// 文件里没有 id(旧单流程导出的形状)→ 分配新 id,内容照搬。
#[test]
fn import_assigns_new_id_when_file_has_none() {
    let dir = TempDataDir::new("flow-bundle-noid");
    let mut svc = service(&dir);
    let mut legacy = flow("旧文件", vec![graph_step("n", &[])]);
    legacy.id = String::new();
    legacy.enabled = false; // 旧导出不含 enabled 时也该能进来

    let report = svc
        .import_bundle(vec![legacy], None, ImportConflictMode::Rename)
        .unwrap();
    assert_eq!(report.imported, 1);
    assert!(report.renamed.is_empty(), "原文件没有 id,谈不上重映射");
    let added = svc
        .get_library()
        .flows
        .iter()
        .find(|f| f.name == "旧文件")
        .expect("应新增进库");
    assert!(!added.id.is_empty(), "必须分配 id");
    assert_eq!(added.steps.len(), 1, "步骤内容照搬");
    assert_eq!(
        svc.get_library().current_flow_id.as_deref(),
        Some(added.id.as_str()),
        "root_id 缺省 = 批内第一个"
    );
}

/// 包内成环 / 自引用:文件自相矛盾,必须 400 且不写库(与保存期同一套判定)。
#[test]
fn import_rejects_in_batch_cycle_and_self_reference() {
    let dir = TempDataDir::new("flow-bundle-badrefs");
    let mut svc = service(&dir);
    let before = svc.get_library().flows.len();

    // A → B、B → A:批内互挂成环(图内环检测挡不住,必须按调用链拦)
    let mut a = flow("甲", vec![sub_step("n", "bundle-b")]);
    a.id = "bundle-a".into();
    let mut b = flow("乙", vec![sub_step("n", "bundle-a")]);
    b.id = "bundle-b".into();
    let err = svc
        .import_bundle(vec![a, b], None, ImportConflictMode::Rename)
        .unwrap_err();
    assert!(err.contains("环"), "应点名调用链成环:{err}");
    assert!(
        err.contains("甲") && err.contains("乙"),
        "应点名环上两套流程:{err}"
    );
    assert_eq!(svc.get_library().flows.len(), before, "拒绝时不写库");

    // A → A:自引用
    let mut self_ref = flow("自", vec![sub_step("n", "bundle-self")]);
    self_ref.id = "bundle-self".into();
    let err = svc
        .import_bundle(vec![self_ref], None, ImportConflictMode::Rename)
        .unwrap_err();
    assert!(err.contains("自身"), "应点名自引用:{err}");
    assert_eq!(svc.get_library().flows.len(), before);
}

/// 批内同内容不同 id:第二份按**本批**已接受的那份跳过(而非只认本库)。
#[test]
fn import_skips_duplicate_content_within_batch() {
    let dir = TempDataDir::new("flow-bundle-batch-dup");
    let mut svc = service(&dir);
    let mut first = flow("同款", vec![graph_step("n", &[])]);
    first.id = "dup-a".into();
    let mut second = flow("同款", vec![graph_step("n", &[])]);
    second.id = "dup-b".into();

    let report = svc
        .import_bundle(
            vec![first, second],
            Some("dup-b"),
            ImportConflictMode::Rename,
        )
        .unwrap();
    assert_eq!(report.imported, 1, "同内容只进一份");
    assert_eq!(report.skipped, 1);
    assert!(
        report.renamed.is_empty(),
        "第二份是「内容已存在」的跳过,不是 id 冲突重命名"
    );
    assert!(
        svc.flow_by_id("dup-a").is_some() && svc.flow_by_id("dup-b").is_none(),
        "留下的是第一份"
    );
    assert_eq!(
        svc.get_library().current_flow_id.as_deref(),
        Some("dup-a"),
        "root_id 指向被跳过的第二份时,落到本批等价副本上(remap 已登记)"
    );
}

/// 路由保留段 id(export/import/select)会被静态路由吃掉、导致流程删不掉 → 导入侧重映射。
#[test]
fn import_remaps_reserved_route_id() {
    let dir = TempDataDir::new("flow-bundle-reserved");
    let mut svc = service(&dir);
    let mut reserved = flow("保留名", vec![graph_step("n", &[])]);
    reserved.id = "export".into();

    let report = svc
        .import_bundle(vec![reserved], Some("export"), ImportConflictMode::Rename)
        .unwrap();
    assert_eq!(report.imported, 1);
    assert_eq!(report.renamed.len(), 1, "保留段 id 必须重映射");
    assert_eq!(report.renamed[0].old_id, "export");
    let new_id = report.renamed[0].new_id.clone();
    assert_ne!(new_id, "export");
    assert!(
        svc.flow_by_id("export").is_none(),
        "库里不得出现删不掉的流程(静态路由段优先于 /{{id}})"
    );
    assert_eq!(
        svc.get_library().current_flow_id.as_deref(),
        Some(new_id.as_str()),
        "root_id 经重映射后仍选中导入的那份"
    );
}

// ==================== 导入覆盖模式(A 批 B4) ====================

/// 覆盖模式:同 id 内容不同 → **覆盖本库那份**(id 不变),库中其它流程一律不动。
///
/// 判别性:本机那份已被改过(两步),用「原内容」的包覆盖后应回到单步;
/// 若误走 rename 分支,则本机那份保持两步、库里多出一份副本 —— 两条断言同时失败。
#[test]
fn import_replace_overwrites_same_id_and_keeps_the_rest() {
    let dir = TempDataDir::new("flow-bundle-replace");
    let mut svc = service(&dir);
    let main = save_flow(&mut svc, "主", vec![graph_step("n", &[])]);
    let other = save_flow(&mut svc, "别家", vec![graph_step("x", &[])]);
    let bundle = svc.export_bundle(Some(&main)).unwrap().flows;
    // 本机改掉 main(结构变了)→ 与文件内容不同
    {
        let mut edited = svc.flow_by_id(&main).unwrap().clone();
        edited.steps.push(graph_step("n2", &["n"]));
        svc.set(edited).unwrap();
    }
    let size = svc.get_library().flows.len();

    let report = svc
        .import_bundle(bundle, Some(&main), ImportConflictMode::Replace)
        .unwrap();

    assert_eq!(report.replaced.len(), 1, "应报告覆盖了 1 份");
    assert_eq!(report.replaced[0].id, main);
    assert_eq!(report.imported, 1);
    assert!(report.renamed.is_empty(), "覆盖模式不产生新 id");
    assert_eq!(
        svc.get_library().flows.len(),
        size,
        "覆盖是原地替换,库规模不变"
    );
    assert_eq!(
        svc.flow_by_id(&main).unwrap().steps.len(),
        1,
        "本库那份应被文件内容覆盖(回到单步)"
    );
    assert!(svc.flow_by_id(&other).is_some(), "库中其它流程一律不动");
}

/// 覆盖模式下**引用不破**:被覆盖的流程 id 不变,挂载它的流程仍指向同一份。
#[test]
fn import_replace_keeps_sub_flow_references_intact() {
    let dir = TempDataDir::new("flow-bundle-replace-refs");
    let mut svc = service(&dir);
    let sub = save_flow(&mut svc, "子", vec![graph_step("n", &[])]);
    let main = save_flow(&mut svc, "主", vec![sub_step("n", &sub)]);
    let bundle = svc.export_bundle(Some(&main)).unwrap().flows;
    // 只改子流程:入口与文件逐字节相同 → 跳过;子流程内容不同 → 被覆盖
    {
        let mut edited = svc.flow_by_id(&sub).unwrap().clone();
        edited.description = Some("本机改过".into());
        svc.set(edited).unwrap();
    }

    let report = svc
        .import_bundle(bundle, Some(&main), ImportConflictMode::Replace)
        .unwrap();

    assert_eq!(report.skipped, 1, "入口内容相同 → 跳过");
    assert_eq!(report.replaced.len(), 1, "子流程被覆盖");
    assert_eq!(report.replaced[0].id, sub);
    assert_eq!(
        svc.flow_by_id(&main).unwrap().steps[0].sub_flow_ref(),
        Some(sub.as_str()),
        "覆盖保留 id → 挂载引用不受影响(这正是覆盖模式的价值)"
    );
    assert_eq!(
        svc.flow_by_id(&sub).unwrap().description,
        None,
        "子流程内容确实被文件覆盖(本机那次编辑丢失——覆盖不可逆,故请求侧须显式传参)"
    );
}

/// 覆盖模式仍不越界:① 保留段 id 照旧重命名(那类 id 没有「本库同一份」可言);
/// ② 库中未在文件里出现的流程不受影响(绝不「导入即清库」)。
#[test]
fn import_replace_does_not_touch_absent_or_reserved_flows() {
    let dir = TempDataDir::new("flow-bundle-replace-scope");
    let mut svc = service(&dir);
    let kept = save_flow(&mut svc, "本机独有", vec![graph_step("k", &[])]);
    let mut reserved = flow("保留名", vec![graph_step("n", &[])]);
    reserved.id = "export".into();

    let report = svc
        .import_bundle(vec![reserved], Some("export"), ImportConflictMode::Replace)
        .unwrap();

    assert_eq!(report.renamed.len(), 1, "保留段 id 即使覆盖模式也要重映射");
    assert!(report.replaced.is_empty());
    assert!(
        svc.flow_by_id("export").is_none(),
        "库里不得出现删不掉的流程"
    );
    assert!(
        svc.flow_by_id(&kept).is_some(),
        "文件里没有的流程一律不动(覆盖 ≠ 清库)"
    );
}

/// 搬运包的线格式键必须与常量一致(前端按 `FLOW_BUNDLE_KEY` 判定新旧格式)。
#[test]
fn flow_bundle_wire_key_matches_constant() {
    let dir = TempDataDir::new("flow-bundle-wire");
    let svc = service(&dir);
    let bundle = svc.export_bundle(None).unwrap();
    let value = serde_json::to_value(&bundle).unwrap();
    assert_eq!(
        value.get(FLOW_BUNDLE_KEY),
        Some(&serde_json::json!(FLOW_BUNDLE_VERSION)),
        "线格式键/版本号必须与常量一致(改结构体字段名会在这里变红)"
    );
    for key in ["exported_at", "root_id", "flows"] {
        assert!(value.get(key).is_some(), "线格式缺键:{key}");
    }
}

// ==================== CODE-5:编码流程预设集(包开关并入) ====================

/// 包流程可用的工具集:从**单一出处**常量派生(工作区族清单 + bash),不手抄——
/// 工具族改名时本函数自动跟随;而流程常量里写死的字面量由 `validate_flow` 兜底报错。
fn pack_tools() -> BTreeSet<String> {
    crate::tools::tool_sets::WORKSPACE_TOOLS
        .iter()
        .map(|s| (*s).to_string())
        .chain(std::iter::once(crate::tools::bash::TOOL_NAME.to_string()))
        .collect()
}

fn pack_service(dir: &TempDataDir, enabled: bool) -> AgentFlowService {
    AgentFlowService::new(
        dir.path().to_path_buf(),
        pack_tools().into_iter().collect(),
        enabled,
    )
}

/// 关包:内置集 = 协调 + 示范(FLOW-DEMO-1 起示范流程随构造期并入、与包无关);
/// 库文件只多出内置示范流程的 id 账目,包流程一条都不在。
#[test]
fn pack_flows_absent_when_disabled() {
    let dir = TempDataDir::new("pack-off");
    let svc = pack_service(&dir, false);
    assert_eq!(svc.get_library().flows.len(), 2);
    assert_eq!(svc.get_library().flows[0].id, "builtin-coordination");
    assert_eq!(svc.get_library().flows[1].id, "builtin-research-demo");
    assert_eq!(
        svc.get_library().seeded_pack_ids,
        vec!["builtin-research-demo"],
        "关包时账目只记内置示范流程,不记包 id"
    );
    let text = std::fs::read_to_string(dir.join("agent_flows.json")).unwrap();
    assert!(
        text.contains("builtin-research-demo") && !text.contains("builtin-code-review"),
        "库文件应含示范流程与其 id 账目、不得含包流程:{text}"
    );
}

/// 开包:2+2 条(协调 + 示范 + 两条包流程),且**既有条目各字段逐字不变**(按序列化
/// 形态比对,比字段级更严),`current_flow_id` 不因并入而改变(`seeded_pack_ids` 是
/// 顺序化「改过/删过不复活」的凭据)。
#[test]
fn pack_flows_merged_when_enabled_and_builtin_untouched() {
    let dir = TempDataDir::new("pack-on");
    let svc = pack_service(&dir, true);
    let lib = svc.get_library();
    assert_eq!(
        lib.flows.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        vec![
            "builtin-coordination",
            "builtin-research-demo",
            "builtin-code-review",
            "builtin-code-impl"
        ],
        "内置簇位置不变(示范排协调之后),包流程按常量表追加"
    );
    let builtin = lib
        .flows
        .iter()
        .find(|f| f.id == "builtin-coordination")
        .unwrap();
    assert_eq!(
        serde_json::to_value(builtin).unwrap(),
        serde_json::to_value(builtin_flow()).unwrap(),
        "既有内置流程必须逐字不变(并入只 push 新条目)"
    );
    assert_eq!(
        lib.seeded_pack_ids,
        vec![
            "builtin-research-demo",
            "builtin-code-review",
            "builtin-code-impl"
        ]
    );
    assert_eq!(
        lib.current_flow_id.as_deref(),
        Some("builtin-coordination"),
        "并入不得改当前选中"
    );
}

/// 升级路径:库文件已存在(旧版本时代建的)时构造 → 注入内置示范 + 包流程并落盘;
/// 重载后仍在。
#[test]
fn pack_flows_merge_into_existing_library_and_persist() {
    let dir = TempDataDir::new("pack-upgrade");
    let lib = AgentFlowLibrary {
        current_flow_id: Some("builtin-coordination".into()),
        flows: vec![builtin_flow()],
        seeded_pack_ids: Vec::new(),
    };
    lib.save(dir.path()).unwrap();

    let svc = pack_service(&dir, true);
    assert_eq!(svc.get_library().flows.len(), 4);
    // 落盘验证:重新 load 而不是只看内存
    let reloaded = AgentFlowLibrary::load(dir.path());
    assert_eq!(reloaded.flows.len(), 4, "注入结果必须已落盘");
    assert_eq!(reloaded.seeded_pack_ids.len(), 3);
}

/// 幂等 + 不复活 + 不回收:重复 sync 不重复注入;用户删过的再 sync 不复活;
/// 关包不回收已注入副本(既定边界,同 LIT-5 Q2)。
#[test]
fn pack_sync_is_idempotent_and_respects_user_changes() {
    let dir = TempDataDir::new("pack-idem");
    let mut svc = pack_service(&dir, true);
    assert_eq!(svc.get_library().flows.len(), 4);

    svc.sync_pack_flows(true);
    svc.sync_pack_flows(true);
    assert_eq!(svc.get_library().flows.len(), 4, "重复 sync 不重复注入");

    // 删过不复活
    svc.remove("builtin-code-review").unwrap();
    assert_eq!(svc.get_library().flows.len(), 3);
    svc.sync_pack_flows(true);
    assert_eq!(svc.get_library().flows.len(), 3, "用户删过的包流程不得复活");

    // 关包不回收
    svc.sync_pack_flows(false);
    assert_eq!(
        svc.get_library().flows.len(),
        3,
        "关包不回收已注入副本(既定边界);被用户删掉的那条当然也不回来"
    );
}

/// 内置**示范流程**的并入生命周期(FLOW-DEMO-1):与包流程同一账目——删过不复活
/// (重建服务不重注入)、用户改过不被覆盖(sync 只认账目)。
#[test]
fn builtin_demo_flow_respects_delete_and_edit_like_packs() {
    let dir = TempDataDir::new("demo-lifecycle");
    let mut svc = pack_service(&dir, false);
    assert!(svc.flow_by_id("builtin-research-demo").is_some());

    // 删过不复活:重建服务(构造期会再走一次并入)也不回来
    svc.remove("builtin-research-demo").unwrap();
    let svc2 = pack_service(&dir, false);
    assert!(
        svc2.flow_by_id("builtin-research-demo").is_none(),
        "用户删过的示范流程不得复活"
    );

    // 用户改过不被覆盖:改一个再重建,仍是用户版
    let dir2 = TempDataDir::new("demo-edit");
    let mut svc3 = pack_service(&dir2, false);
    let mut edited = svc3.flow_by_id("builtin-research-demo").unwrap().clone();
    edited.name = "我的调研流程".into();
    svc3.set(edited).unwrap();
    let svc4 = pack_service(&dir2, false);
    assert_eq!(
        svc4.flow_by_id("builtin-research-demo").unwrap().name,
        "我的调研流程",
        "用户改过的示范流程不得被覆盖回去"
    );
}

/// 用户改过的包流程:再 sync 不覆盖(同 id 已在 seeded 里 → 跳过)。
#[test]
fn edited_pack_flow_is_not_overwritten_by_sync() {
    let dir = TempDataDir::new("pack-edit");
    let mut svc = pack_service(&dir, true);
    let mut edited = svc.flow_by_id("builtin-code-review").unwrap().clone();
    edited.name = "我的审查流程".into();
    svc.set(edited).unwrap();

    svc.sync_pack_flows(true);
    let got = svc.flow_by_id("builtin-code-review").unwrap();
    assert_eq!(got.name, "我的审查流程", "用户改过的包流程不得被覆盖回去");
}

/// 常量自检:2 条、id 稳定、过 `validate_flow`(用真实工具集),
/// 且**工具面刻意分工**——审查流程只读(物理上改不了文件),实现流程才放开写与 bash。
#[test]
fn coding_pack_flows_pass_validate_flow() {
    let flows = coding_pack_flows();
    assert_eq!(flows.len(), 2, "首版清单是 2 条(Q5=(a) 拍板)");
    assert_eq!(
        flows.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        vec!["builtin-code-review", "builtin-code-impl"]
    );
    for f in &flows {
        validate_flow(f, &pack_tools())
            .unwrap_or_else(|e| panic!("包流程「{}」未过 validate_flow:{e}", f.name));
    }

    let readonly: Vec<&str> = crate::tools::tool_sets::WORKSPACE_READONLY_TOOLS.to_vec();
    let review = &flows[0];
    for s in &review.steps {
        let tools = s.tools.clone().unwrap_or_default();
        assert!(
            tools.iter().all(|t| readonly.contains(&t.as_str())),
            "审查流程的步骤「{}」只能用只读工具,实际:{tools:?}",
            s.name
        );
    }

    let impl_flow = &flows[1];
    let uses_bash = impl_flow.steps.iter().any(|s| {
        s.tools
            .as_ref()
            .is_some_and(|t| t.iter().any(|x| x == "bash"))
    });
    assert!(uses_bash, "实现流程应包含可执行命令的自测步骤(D4=(a) 拍板)");
}

/// FLOW-DEMO-1:内置画布示范流程「多路调研示范流程」的常量自检——真实工具集过
/// `validate_flow`、五节点坐标齐备(前端 `savedPosition` 要求 x/y 均为数字)、
/// 图语义为 分叉 → 并行两路 → 汇合(reflect)→ 严格终稿,且终稿为显式 `is_output`。
#[test]
fn builtin_demo_flow_passes_validate_flow_and_is_two_dimensional() {
    let flows = builtin_demo_flows();
    assert_eq!(flows.len(), 1, "首版示范清单是 1 条");
    let demo = &flows[0];
    assert_eq!(demo.id, "builtin-research-demo");
    assert!(demo.enabled, "示范流程开箱启用");
    validate_flow(demo, &pack_tools()).unwrap_or_else(|e| panic!("示范流程未过 validate_flow:{e}"));

    // 坐标齐备:五节点 x/y 均为数字(画布式的前提),且结构上单点层居中(134)、双点层 0/268
    for s in &demo.steps {
        assert!(
            s.x.is_some() && s.y.is_some(),
            "节点「{}」缺画布坐标",
            s.name
        );
    }
    let pos = |id: &str| {
        let s = demo.steps.iter().find(|s| s.id == id).unwrap();
        (s.x.unwrap(), s.y.unwrap())
    };
    assert_eq!(pos("decompose"), (134.0, 0.0));
    assert_eq!(pos("facts"), (0.0, 120.0));
    assert_eq!(pos("risks"), (268.0, 120.0));
    assert_eq!(pos("verify"), (134.0, 240.0));
    assert_eq!(pos("report"), (134.0, 360.0));

    // 图语义:拓扑序 = 数组序;facts/risks 各以 decompose 为唯一上游(分叉),
    // verify 汇合两路,report 接 verify;终稿由显式 is_output 选中
    let graph = resolve_graph(&demo.steps).expect("示范流程应无环");
    assert_eq!(graph.order, vec![0, 1, 2, 3, 4], "拓扑序应为数组序");
    assert_eq!(graph.inputs[0], Vec::<usize>::new(), "decompose 是源节点");
    assert_eq!(graph.inputs[1], vec![0], "facts 的上游是 decompose");
    assert_eq!(graph.inputs[2], vec![0], "risks 的上游是 decompose");
    assert_eq!(graph.inputs[3], vec![1, 2], "verify 汇合两路(下标升序)");
    assert_eq!(graph.inputs[4], vec![3], "report 接 verify");
    assert_eq!(
        output_index(&demo.steps, &graph.inputs),
        Some(4),
        "终稿应为显式 is_output 的 report"
    );
    let report = &demo.steps[4];
    assert_eq!(report.kind.as_deref(), Some("strict"), "终稿用严格档收口");
    assert!(
        report.tools.is_none(),
        "严格档终稿不下发工具(结构上也不需要)"
    );
    let verify = &demo.steps[3];
    assert_eq!(verify.action, "reflect", "核验节点应为 reflect");
    assert!(
        verify.system_prompt.is_none() && verify.generates.is_none(),
        "reflect 节点的 validate_flow 约束:无 system_prompt、不生成正文"
    );
    assert!(
        demo.steps[0].generates == Some(false),
        "拆解节点是内部规划(不产出面向用户的正文)"
    );
}
