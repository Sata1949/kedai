//! `team` 的单元测试(由原 `team.rs` 内的 `#[cfg(test)] mod tests` 整段搬来,原样保留)。
//!
//! `#![cfg(test)]` 必须留在**列首**且字面为 `#[cfg(test)]`:`tools/check-arch.mjs` 的
//! `productionLineCount` 靠它切出生产区,否则本文件会被当成生产代码,规则 E
//! (`services` 生产代码禁用 `.expect(`,基线为空集)会把测试里的断言全部判红。
#![cfg(test)]
use super::assemble::*;
use super::audit::*;
use super::budget::*;
use super::parallel::*;
use super::topology::*;
use super::*;
use crate::services::task_engine::wiring::extract_exports;
use std::time::{Duration, Instant};

/// 合法拓扑直接解析;过滤空分工/空子目标
#[test]
fn parse_team_topology_plain() {
    let text = r#"{"mains":[{"name":"调研","goals":[{"name":"子一","goal":"做一"},{"name":"子二","goal":"做二"}]},{"name":"写作","goals":[{"name":"子三","goal":"做三"}]}]}"#;
    let mains = parse_team_topology(text).expect("合法拓扑应解析成功");
    assert_eq!(mains.len(), 2);
    assert_eq!(mains[0].goals.len(), 2);
    assert_eq!(mains[1].name, "写作");
}

fn team_main_with_after(name: &str, after: Vec<usize>) -> TeamMain {
    TeamMain {
        name: name.into(),
        goals: vec![TaskStep {
            name: "子".into(),
            goal: String::new(),
            status: TaskStepStatus::Pending,
            result: String::new(),
            node_id: None,
            files: Vec::new(),
        }],
        after,
    }
}

fn step_with_files(files: &[&str]) -> TaskStep {
    TaskStep {
        name: "子".into(),
        goal: String::new(),
        status: TaskStepStatus::Pending,
        result: String::new(),
        node_id: None,
        files: files.iter().map(|s| s.to_string()).collect(),
    }
}

fn temp_step_ws(tag: &str) -> std::path::PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "kedai-step-outcome-{tag}-{}-{ns}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("创建临时工作区失败");
    dir
}

/// SKY-8:未声明文件时保持旧语义——守卫截断→partial,预算收尾→done。
#[test]
fn step_outcome_without_declared_files_keeps_old_semantics() {
    let step = step_with_files(&[]);
    let (status, text) = step_outcome(None, &step, "正文", Some("连续只读空转熔断"), false, None);
    assert_eq!(status, TaskStepStatus::Partial);
    assert!(text.contains("[守卫截断]"), "{text}");
    let (status, text) = step_outcome(None, &step, "正文", None, true, None);
    assert_eq!(status, TaskStepStatus::Done);
    assert_eq!(text, "正文");
}

/// SKY-8:守卫截断但声明文件齐全 → done,并留收尾说明。
#[test]
fn step_outcome_guard_stop_with_complete_file_is_done() {
    let ws = temp_step_ws("guard-complete");
    std::fs::write(ws.join("out.txt"), "hello").expect("写文件失败");
    let step = step_with_files(&["out.txt"]);
    let (status, text) = step_outcome(
        Some(&ws),
        &step,
        "正文",
        Some("声明文件无进展收尾"),
        false,
        None,
    );
    assert_eq!(status, TaskStepStatus::Done);
    assert!(text.contains("按完成计"), "{text}");
    let _ = std::fs::remove_dir_all(&ws);
}

/// SKY-8:预算收尾但声明文件缺失 → partial(不得再记 done)。
#[test]
fn step_outcome_budget_stop_with_missing_file_is_partial() {
    let ws = temp_step_ws("budget-missing");
    let step = step_with_files(&["out.txt"]);
    let (status, text) = step_outcome(Some(&ws), &step, "正文", None, true, None);
    assert_eq!(status, TaskStepStatus::Partial);
    assert!(text.contains("[预算收尾]"), "{text}");
    assert!(text.contains("out.txt:文件缺失"), "{text}");
    let _ = std::fs::remove_dir_all(&ws);
}

/// SKY-8:预算收尾但声明文件齐全 → done(旧语义不变)。
#[test]
fn step_outcome_budget_stop_with_complete_file_is_done() {
    let ws = temp_step_ws("budget-complete");
    std::fs::write(ws.join("out.txt"), "hello").expect("写文件失败");
    let step = step_with_files(&["out.txt"]);
    let (status, text) = step_outcome(Some(&ws), &step, "正文", None, true, None);
    assert_eq!(status, TaskStepStatus::Done);
    assert_eq!(text, "正文");
    let _ = std::fs::remove_dir_all(&ws);
}

/// SKY-8:守卫截断 + 声明文件含未替换槽位 → partial 并点名问题。
#[test]
fn step_outcome_guard_stop_with_placeholder_is_partial() {
    let ws = temp_step_ws("guard-placeholder");
    std::fs::write(ws.join("out.txt"), "a\n/*@@MAIN@@*/\n").expect("写文件失败");
    let step = step_with_files(&["out.txt"]);
    let (status, text) = step_outcome(Some(&ws), &step, "正文", Some("重复调用熔断"), false, None);
    assert_eq!(status, TaskStepStatus::Partial);
    assert!(text.contains("@@ 未替换槽位"), "{text}");
    let _ = std::fs::remove_dir_all(&ws);
}

/// SKY-8:单文件目标的中间模板步骤以最终入口为准——入口干净即 done,
/// 模板自身的 `@@`/缺失不再单独判死;入口缺失时仍 partial。
#[test]
fn step_outcome_intermediate_template_uses_final_entry() {
    let ws = temp_step_ws("single-entry");
    std::fs::write(ws.join("index.html"), "<html>ok</html>").expect("写入口失败");
    let step = step_with_files(&["src/shell.html"]);
    let (status, text) = step_outcome(Some(&ws), &step, "正文", None, true, Some("index.html"));
    assert_eq!(status, TaskStepStatus::Done);
    assert!(text.contains("中间模板"), "{text}");

    let ws2 = temp_step_ws("single-entry-missing");
    let (status, text) = step_outcome(Some(&ws2), &step, "正文", None, true, Some("index.html"));
    assert_eq!(status, TaskStepStatus::Partial);
    assert!(text.contains("最终入口"), "{text}");
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&ws2);
}

/// SKY-7:集成主必须是依赖 DAG 的汇点——链式拓扑只有末端总装主豁免;
/// 中段生产者(有 after、但下游还有主)仍走两道计数守卫。
#[test]
fn integration_main_is_only_dependency_sink() {
    // 1 → 2 → 3 串行链
    let chain = vec![
        team_main_with_after("生产", vec![]),
        team_main_with_after("中段", vec![0]),
        team_main_with_after("总装", vec![1]),
    ];
    assert!(!is_integration_main(&chain, 0), "无上游依赖:不豁免");
    assert!(!is_integration_main(&chain, 1), "中段生产者不是汇点:不豁免");
    assert!(is_integration_main(&chain, 2), "末端总装主是汇点:豁免");

    // 两主:分片 → 总装(汇点)
    let two = vec![
        team_main_with_after("分片", vec![]),
        team_main_with_after("总装", vec![0]),
    ];
    assert!(is_integration_main(&two, 1), "两主汇点保持 SKY-6 豁免");

    // 无依赖并行:全部不豁免
    let parallel = vec![
        team_main_with_after("甲", vec![]),
        team_main_with_after("乙", vec![]),
    ];
    assert!(!is_integration_main(&parallel, 0));
    assert!(!is_integration_main(&parallel, 1));

    // 环:每个都有下游,全部不豁免(环由 plan_waves 降级同波)
    let ring = vec![
        team_main_with_after("甲", vec![1]),
        team_main_with_after("乙", vec![0]),
    ];
    assert!(!is_integration_main(&ring, 0));
    assert!(!is_integration_main(&ring, 1));

    // 越界防御
    assert!(!is_integration_main(&two, 9));
}

/// SKY-7:预算份额按「本主步数 / 剩余未执行步数」比例分;至少 1 秒但不超池。
#[test]
fn fair_share_is_proportional_and_bounded() {
    assert_eq!(
        fair_share(Duration::from_secs(100), 3, 10),
        Duration::from_secs(30)
    );
    assert_eq!(
        fair_share(Duration::from_secs(100), 2, 4),
        Duration::from_secs(50)
    );
    assert_eq!(fair_share(Duration::ZERO, 1, 3), Duration::ZERO);
    assert_eq!(fair_share(Duration::from_secs(100), 0, 3), Duration::ZERO);
    assert_eq!(
        fair_share(Duration::from_secs(100), 1, 0),
        Duration::from_secs(100),
        "分母为 0 时防御性给满池"
    );
    // 小池:至少 1 秒的意图被池本身封顶,不会放大预算
    assert_eq!(
        fair_share(Duration::from_millis(200), 1, 10),
        Duration::from_millis(200)
    );
    // 份额不小于 1 秒(中等池的 1/10 小于 1 秒时抬到 1 秒)
    assert_eq!(
        fair_share(Duration::from_secs(5), 1, 10),
        Duration::from_secs(1)
    );
}

/// SKY-7:审计预留 <120s 不生效;120s 起 5%,夹在 10..=180 秒。
#[test]
fn post_exec_reserve_scales_and_is_capped() {
    assert_eq!(post_exec_reserve(Duration::from_secs(1)), Duration::ZERO);
    assert_eq!(post_exec_reserve(Duration::from_secs(119)), Duration::ZERO);
    assert_eq!(
        post_exec_reserve(Duration::from_secs(120)),
        Duration::from_secs(10)
    );
    assert_eq!(
        post_exec_reserve(Duration::from_secs(1200)),
        Duration::from_secs(60)
    );
    assert_eq!(
        post_exec_reserve(Duration::from_secs(3600)),
        Duration::from_secs(180)
    );
    assert_eq!(
        post_exec_reserve(Duration::from_secs(86_400)),
        Duration::from_secs(180)
    );
}

/// SKY-7:主执行池截止期 = 全局截止期 - 预留;无截止期保持 None。
#[test]
fn main_pool_deadline_reserves_for_audit() {
    assert!(main_pool_deadline(None).is_none(), "无预算不得引入新闸");
    let global = Instant::now() + Duration::from_secs(3600);
    let pool = main_pool_deadline(Some(global)).expect("应有池");
    let gap = global.saturating_duration_since(pool).as_secs();
    assert!(
        (170..=180).contains(&gap),
        "3600s 预算应预留约 180s,实际 {gap}s"
    );
}

/// TMR-4:files 声明被解析、归一化、去重;null 容错;非法路径/通配符丢弃。
#[test]
fn parse_team_topology_declares_files() {
    let text = r#"{"mains":[{"name":"甲","goals":[{"name":"a","goal":"写 a","files":["./parts/a.js","parts/a.js","../escape.js","C:/abs.js","src/*.js","parts/b.js"]},{"name":"b","goal":"无文件","files":null},{"name":"c","goal":"缺省"}]}]}"#;
    let mains = parse_team_topology(text).unwrap();
    assert_eq!(
        mains[0].goals[0].files,
        vec!["parts/a.js", "parts/b.js"],
        "files 应归一化、去重、丢弃非法项"
    );
    assert!(mains[0].goals[1].files.is_empty(), "null 应按空数组处理");
    assert!(
        mains[0].goals[2].files.is_empty(),
        "缺省 files 应按空数组处理"
    );
}

/// SKY-3:审计输入里的工作区清单必须按 TaskStep.files **全类型**枚举——
/// 空岛实跑中审计员只 glob `*.js`,据此断言「没有 index.html」,而文件已存在。
#[test]
fn workspace_manifest_lists_declared_files_of_all_types() {
    let dir = std::env::temp_dir().join(format!("kedai-manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("parts")).expect("建测试目录失败");
    std::fs::write(
        dir.join("index.html"),
        "<html>\n/*@@MAIN@@*/\n/*@@SKY@@*/\n</html>",
    )
    .expect("写 index.html 失败");
    std::fs::write(dir.join("parts/a.js"), "// a").expect("写分片失败");

    let mains = vec![TeamMain {
        name: "总装主".into(),
        goals: vec![TaskStep {
            name: "总装".into(),
            goal: "内联分片".into(),
            files: vec![
                "index.html".into(),
                "parts/a.js".into(),
                "parts/missing.js".into(),
            ],
            ..TaskStep::default()
        }],
        after: Vec::new(),
    }];
    let manifest = workspace_manifest(&dir, &mains, "");

    assert!(
        manifest.contains("`index.html`:存在("),
        "非 .js 的入口文件必须进清单: {manifest}"
    );
    assert!(
        manifest.contains("含 2 行 @@ 未替换槽位"),
        "未替换的模板槽位必须如实计数(SKY-4/5): {manifest}"
    );
    assert!(
        manifest.contains("`parts/a.js`:存在("),
        "分片文件应列出字节数: {manifest}"
    );
    assert!(
        manifest.contains("`parts/missing.js`:缺失"),
        "缺失文件必须如实标注: {manifest}"
    );
    // 清单进入审计输入时位于产出正文之前,不能被大段自述淹没
    let input = build_audit_input("目标", &mains, &[Some("自述".into())], &manifest);
    let manifest_pos = input.find("工作区声明文件实况").expect("清单应进输入");
    let outputs_pos = input.find("各主 agent 产出").expect("产出段应存在");
    assert!(manifest_pos < outputs_pos, "清单应在产出正文之前");

    // 工作区不存在时不制造假缺失
    let _ = std::fs::remove_dir_all(&dir);
    assert!(workspace_manifest(&dir, &mains, "").is_empty());
}

/// SKY-1:上游产出的接口清单必须由引擎实测,覆盖 ESM 与经典脚本两种形态。
#[test]
fn extract_exports_covers_esm_and_classic_forms() {
    let text = "export const A = 1;\nexport function buildB() {}\nexport async function c() {}\nexport { d, e as f };\nexport default init;\nwindow.MCWorldMain = MCWorldMain;\nglobal.MCTextures = MCTextures;\nconst notExported = 1;";
    let got = extract_exports(text);
    for name in [
        "A",
        "buildB",
        "c",
        "d",
        "f",
        "default",
        "MCWorldMain",
        "MCTextures",
    ] {
        assert!(
            got.iter().any(|g| g == name),
            "应提取导出名「{name}」: {got:?}"
        );
    }
    assert!(
        !got.iter().any(|g| g == "notExported"),
        "非导出标识符不得进清单: {got:?}"
    );
}

/// SKY-1:总装主的依赖摘要要附「引擎实测导出」,不能只给步骤名与文件。
#[test]
fn upstream_seed_carries_measured_exports() {
    let dir = std::env::temp_dir().join(format!("kedai-seed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("建测试目录失败");
    std::fs::write(
        dir.join("src/part-subislands.js"),
        "export function generateSubIslands() {}\nexport const createSubIslands = 1;",
    )
    .expect("写分片失败");

    let mains = vec![
        TeamMain {
            name: "分片主".into(),
            goals: Vec::new(),
            after: Vec::new(),
        },
        TeamMain {
            name: "总装主".into(),
            goals: Vec::new(),
            after: vec![0],
        },
    ];
    let step_ranges = vec![vec![0usize], vec![1usize]];
    let plan = vec![
        TaskStep {
            name: "写副岛".into(),
            files: vec!["src/part-subislands.js".into()],
            result: "已写".into(),
            ..TaskStep::default()
        },
        TaskStep {
            name: "总装".into(),
            files: vec!["index.html".into()],
            ..TaskStep::default()
        },
    ];
    let seed = upstream_seed_for(&mains, &step_ranges, &plan, 1, Some(&dir));
    assert_eq!(seed.len(), 1);
    let label = &seed[0].0;
    assert!(
        label.contains("引擎实测导出") && label.contains("generateSubIslands"),
        "依赖摘要应附实测导出名: {label}"
    );
    assert!(
        label.contains("createSubIslands"),
        "同文件多个导出都要列出: {label}"
    );
    // 无工作区时不伪造导出清单
    let bare = upstream_seed_for(&mains, &step_ranges, &plan, 1, None);
    assert!(!bare[0].0.contains("引擎实测导出"), "{}", bare[0].0);

    let _ = std::fs::remove_dir_all(&dir);
}

/// TMR3-3:after 用 1-based 主号;空名主被过滤后仍要重映射到正确的新下标。
#[test]
fn parse_team_topology_after_is_one_based_and_remapped() {
    let text = r#"{"mains":[{"name":"","goals":[{"name":"空","goal":"空"}]},{"name":"分片","after":[],"goals":[{"name":"a","goal":"写分片","files":["parts/a.js"]}]},{"name":"总装","after":[2],"goals":[{"name":"b","goal":"把 parts/a.js 引入 index.html","files":["index.html"]}]}]}"#;
    let mains = parse_team_topology(text).expect("合法拓扑应解析成功");
    assert_eq!(mains.len(), 2, "空名主应被过滤");
    assert_eq!(mains[0].name, "分片");
    assert!(
        mains[0].after.is_empty(),
        "无依赖应保持空数组: {:?}",
        mains[0].after
    );
    assert_eq!(
        mains[1].after,
        vec![0],
        "raw 主 2 → 过滤后新下标 0;1-based 解析 + 重映射"
    );
}

/// 2026-10-23 真实模型实测:after 写成主 agent 名称(或数字字符串)时也要能解析。
/// 原始失败形态:deepseek v4.1 flash 输出 `"after":["纹理与材质主"]`,旧解析器
/// 只收 usize,整份拓扑报 `invalid type: string, expected usize`。
#[test]
fn parse_team_topology_after_accepts_main_names() {
    let text = r#"{"mains":[{"name":"纹理主","goals":[{"name":"a","goal":"写纹理","files":["parts/textures.js"]}]},{"name":"世界主","after":["纹理主"],"goals":[{"name":"b","goal":"写世界","files":["parts/world.js"]}]},{"name":"总装主","after":["纹理主","2"],"goals":[{"name":"c","goal":"总装","files":["index.html"]}]}]}"#;
    let mains = parse_team_topology(text).expect("名称/数字字符串形态的 after 应解析成功");
    assert_eq!(mains[1].after, vec![0], "名称应解析为对应主下标");
    assert_eq!(mains[2].after, vec![0, 1], "名称与数字字符串混用应去重排序");
}

/// TMR3-3:Kahn 分波——依赖主在前,无依赖时单波并行。
#[test]
fn plan_waves_orders_dependents() {
    let mut mains = vec![
        test_main("甲", &[], "", vec![]),
        test_main("乙", &[], "", vec![0]),
        test_main("丙", &[], "", vec![1]),
    ];
    assert_eq!(plan_waves(&mains), vec![vec![0], vec![1], vec![2]]);
    for m in mains.iter_mut() {
        m.after.clear();
    }
    assert_eq!(
        plan_waves(&mains),
        vec![vec![0, 1, 2]],
        "无依赖必须保持单波并行(与改动前一致)"
    );
}

/// TMR3-3:成环时剩余主降级为同一波并行,保证每个主都会被执行。
#[test]
fn plan_waves_falls_back_to_parallel_on_cycle() {
    let mains = vec![
        test_main("甲", &[], "", vec![1]),
        test_main("乙", &[], "", vec![0]),
    ];
    assert_eq!(plan_waves(&mains), vec![vec![0, 1]]);
}

/// TMR3-3:目标文本提到上游声明的文件 → 推断主级依赖;反向提及成环则跳过。
#[test]
fn infer_dependencies_from_files_adds_edge_and_skips_cycle() {
    let mut mains = vec![
        test_main(
            "总装",
            &["index.html"],
            "把 parts/a.js 引入 index.html",
            vec![],
        ),
        test_main("分片", &["parts/a.js"], "写分片", vec![]),
    ];
    infer_dependencies_from_files(&mut mains);
    assert_eq!(
        mains[0].after,
        vec![1],
        "总装文本点名 parts/a.js → 依赖分片主"
    );
    assert!(mains[1].after.is_empty());

    // 反向再点名 index.html:会与 0→1 成环,必须跳过而不是制造环
    let mut cyclic = vec![
        test_main(
            "总装",
            &["index.html"],
            "把 parts/a.js 引入 index.html",
            vec![],
        ),
        test_main("分片", &["parts/a.js"], "写分片并回填 index.html", vec![]),
    ];
    infer_dependencies_from_files(&mut cyclic);
    assert_eq!(cyclic[0].after, vec![1]);
    assert!(
        cyclic[1].after.is_empty(),
        "成环边必须跳过: {:?}",
        cyclic[1].after
    );
}

/// TMR3-L3:否定语境的提及是约束而非依赖证据(「不要覆盖」不得把并行波次串行化)。
#[test]
fn infer_dependencies_skips_negated_mentions() {
    let mut mains = vec![
        test_main(
            "总装",
            &["index.html"],
            "不要覆盖 parts/a.js,只写自己的文件",
            vec![],
        ),
        test_main("分片", &["parts/a.js"], "写分片", vec![]),
    ];
    infer_dependencies_from_files(&mut mains);
    assert!(
        mains[0].after.is_empty(),
        "否定语境不应推断依赖: {:?}",
        mains[0].after
    );

    // 同一路径另有一处非否定提及:仍应推断依赖(只要有一处是消费语义)
    let mut mixed = vec![
        test_main(
            "总装",
            &["index.html"],
            "不要覆盖 parts/a.js;最终把 parts/a.js 引入 index.html",
            vec![],
        ),
        test_main("分片", &["parts/a.js"], "写分片", vec![]),
    ];
    infer_dependencies_from_files(&mut mixed);
    assert_eq!(mixed[0].after, vec![1], "存在非否定提及时仍应加边");
}

/// TMR3-L3:主自己声明的文件不是「依赖上游」的证据(它本来就要读写自己的产物)。
#[test]
fn infer_dependencies_skips_self_declared_files() {
    let mut mains = vec![
        test_main(
            "总装",
            &["index.html", "parts/a.js"],
            "把 parts/a.js 引入 index.html",
            vec![],
        ),
        test_main("分片", &["parts/a.js"], "写分片", vec![]),
    ];
    infer_dependencies_from_files(&mut mains);
    assert!(
        mains[0].after.is_empty(),
        "自己声明的文件不应推断为依赖他人: {:?}",
        mains[0].after
    );

    // 提到的是别人的另一个文件:仍要加边(守卫只排除「自己声明的那一个」)
    let mut other = vec![
        test_main(
            "总装",
            &["index.html", "parts/a.js"],
            "把 parts/b.js 引入 index.html",
            vec![],
        ),
        test_main("分片", &["parts/b.js"], "写分片", vec![]),
    ];
    infer_dependencies_from_files(&mut other);
    assert_eq!(other[0].after, vec![1]);
}

fn test_main(name: &str, files: &[&str], goal: &str, after: Vec<usize>) -> TeamMain {
    TeamMain {
        name: name.into(),
        goals: vec![TaskStep {
            name: name.into(),
            goal: goal.into(),
            files: files.iter().map(|s| (*s).to_string()).collect(),
            ..TaskStep::default()
        }],
        after,
    }
}

/// TMR-4:同一文件被多个主声明时首声明胜出;未声明文件由运行期首写者认领。
#[test]
fn build_write_ownership_keeps_first_declaration() {
    let mains = vec![
        TeamMain {
            name: "甲".into(),
            goals: vec![TaskStep {
                name: "a".into(),
                goal: "x".into(),
                files: vec!["shared.js".into()],
                ..TaskStep::default()
            }],
            after: Vec::new(),
        },
        TeamMain {
            name: "乙".into(),
            goals: vec![TaskStep {
                name: "b".into(),
                goal: "y".into(),
                files: vec!["shared.js".into(), "parts/b.js".into()],
                ..TaskStep::default()
            }],
            after: Vec::new(),
        },
    ];
    let ownership = build_write_ownership(&mains);
    assert_eq!(ownership.declared_owner("shared.js"), Some("主 Agent-1"));
    assert_eq!(ownership.declared_owner("parts/b.js"), Some("主 Agent-2"));
    assert_eq!(
        ownership.reserve("主 Agent-2", "shared.js"),
        Err("主 Agent-1".to_string()),
        "重复声明时首声明胜出"
    );
}

/// 剥 markdown 代码块 + 前言后记容忍
#[test]
fn parse_team_topology_fenced_and_prose() {
    let text = "好的,拓扑如下:\n```json\n{\"mains\":[{\"name\":\"甲\",\"goals\":[{\"name\":\"g\",\"goal\":\"x\"}]}]}\n```\n以上。";
    let mains = parse_team_topology(text).unwrap();
    assert_eq!(mains.len(), 1, "单主降级拓扑应被接受");
}

/// 超编归并与截断:6 主 → 4 主(尾部并入第 4 位),每主上限 4(归并后第 4 位
/// 9 个子目标 → 截前 4)
#[test]
fn parse_team_topology_clamps_overflow() {
    let mut text = String::from(r#"{"mains":["#);
    for i in 1..=6 {
        if i > 1 {
            text.push(',');
        }
        text.push_str(&format!(
                r#"{{"name":"主{i}","goals":[{{"name":"a","goal":"x"}},{{"name":"b","goal":"y"}},{{"name":"c","goal":"z"}}]}}"#
            ));
    }
    text.push_str("]}");
    let mains = parse_team_topology(&text).unwrap();
    assert_eq!(mains.len(), 4, "超编主 agent 应归并为 4");
    for m in &mains {
        assert!(m.goals.len() <= 4, "每主子目标应截断到 4");
    }
    assert_eq!(mains[3].goals.len(), 4, "归并后第 4 位应截断到 4");
    let total: usize = mains.iter().map(|m| m.goals.len()).sum();
    assert!(total <= 15, "全局子目标应 ≤15");
}

/// 全局 ≤15 封顶真实生效:每主上限调至 4 后 4×4=16 可超,从末位主倒序截断,
/// 每主至少保留 1 个子目标(4×2=8<15 时旧上限永远触不到,属逻辑矛盾修复)
#[test]
fn parse_team_topology_global_cap_fifteen() {
    let mut text = String::from(r#"{"mains":["#);
    for i in 1..=4 {
        if i > 1 {
            text.push(',');
        }
        text.push_str(&format!(
                r#"{{"name":"主{i}","goals":[{{"name":"a","goal":"x"}},{{"name":"b","goal":"y"}},{{"name":"c","goal":"z"}},{{"name":"d","goal":"w"}}]}}"#
            ));
    }
    text.push_str("]}");
    let mains = parse_team_topology(&text).unwrap();
    let total: usize = mains.iter().map(|m| m.goals.len()).sum();
    assert_eq!(total, 15, "全局子目标应封顶 15: {mains:?}");
    for m in &mains {
        assert!(!m.goals.is_empty(), "每主至少保留 1 个子目标: {mains:?}");
    }
    assert_eq!(mains[3].goals.len(), 3, "应从末位主截断: {mains:?}");
}

/// 空拓扑/垃圾文本报错(交给重试)
#[test]
fn parse_team_topology_rejects_empty() {
    assert!(parse_team_topology("{\"mains\":[]}").is_err());
    assert!(parse_team_topology("这不是 JSON").is_err());
    assert!(parse_team_topology("").is_err());
    // 空分工/全空子目标被过滤后为空 → 报错
    assert!(
        parse_team_topology(r#"{"mains":[{"name":"","goals":[]},{"name":"甲","goals":[]}]}"#)
            .is_err()
    );
}

/// 审计输出:通过/打回/结论;序号 1-based 转 0-based;越界条目丢弃;
/// step 可选(指明则打回粒度到子目标,缺省则整主)
#[test]
fn parse_audit_kickbacks() {
    let v = parse_audit(
        r#"{"通过":false,"打回":[{"main":2,"instruction":"补数据"},{"main":9,"instruction":"越界"},{"main":1,"instruction":""}],"结论":"主二缺数据"}"#,
        3,
    );
    assert!(!v.pass);
    assert_eq!(
        v.kickbacks,
        vec![Kickback {
            main: 1,
            step: None,
            instruction: "补数据".to_string()
        }],
        "越界与空指令条目应丢弃"
    );
    assert_eq!(v.conclusion, "主二缺数据");

    let ok = parse_audit(r#"{"通过":true,"打回":[],"结论":"覆盖完整"}"#, 2);
    assert!(ok.pass && ok.kickbacks.is_empty());

    // step 指明时打回粒度到子目标(1-based → 0-based);step 越界(0)按整主
    let scoped = parse_audit(
        r#"{"通过":false,"打回":[{"main":1,"step":2,"instruction":"只补第二个子目标"},{"main":2,"step":0,"instruction":"step=0 无效按整主"}],"结论":"x"}"#,
        2,
    );
    assert_eq!(
        scoped.kickbacks,
        vec![
            Kickback {
                main: 0,
                step: Some(1),
                instruction: "只补第二个子目标".to_string()
            },
            Kickback {
                main: 1,
                step: None,
                instruction: "step=0 无效按整主".to_string()
            },
        ]
    );
}

/// 审计输出无法解析:按未通过兜底(实跑问题 2);结论不落半截 JSON
#[test]
fn parse_audit_fallback_not_passed() {
    let v = parse_audit("看起来都不错", 2);
    assert!(!v.pass, "非 JSON 输出应兜底未通过(审计闸门不得静默失效)");
    assert!(v.kickbacks.is_empty());
    assert_eq!(v.conclusion, "看起来都不错");
}

/// 截断自愈预算决策(2026-09-03 实测:推理模型 reasoning 烧光
/// default_max_tokens=1024,审计 finish=length 产出腰斩 JSON):
/// 仅 finish=length 触发;翻倍且不低于 HEAL_BUDGET_FLOOR,封顶 TEAM_RETRY_MAX_TOKENS_CAP。
/// (2026-09-15:小预算不再「翻倍到 2048 这种仍被推理烧光」的档位,直接抬到下限。)
#[test]
fn trunc_heal_budget_only_on_length() {
    assert_eq!(
        trunc_heal_budget(Some("length"), 1024),
        Some(16_384),
        "小预算应抬到下限(而非仅翻倍到 2048)"
    );
    assert_eq!(
        trunc_heal_budget(Some("length"), 16_384),
        Some(32_768),
        "下限之上仍按翻倍"
    );
    assert_eq!(trunc_heal_budget(Some("length"), 40000), Some(80_000));
    assert_eq!(trunc_heal_budget(Some("length"), 131_072), None);
    assert_eq!(trunc_heal_budget(Some("stop"), 1024), None);
    assert_eq!(trunc_heal_budget(None, 1024), None);
}

/// 推理感知加强(2026-09-15 实录:审计 10000 预算被 reasoning 吃掉 9894、
/// 正文仅 178 字符即截断,翻倍到 20000 重发才成功):
/// 有 reasoning 观测时按「已消耗 + 原预算」一次给足;无观测时维持既有翻倍语义。
#[test]
fn trunc_heal_budget_reasoning_aware_gives_room_for_body() {
    // 实录形态:10000 预算,completion 10005(其中 reasoning 9894)
    // 目标 = 10005 + 10000 = 20005,比单纯翻倍(20000)略高,足以容下正文
    assert_eq!(
        trunc_heal_budget_reasoning_aware(Some("length"), 10_000, 10_005, 9_894),
        Some(20_005)
    );
    // 无 reasoning 观测(非思考模型):维持既有翻倍语义
    assert_eq!(
        trunc_heal_budget_reasoning_aware(Some("length"), 10_000, 10_000, 0),
        Some(20_000)
    );
    // 非 length 不触发(加强不得绕过触发条件)
    assert_eq!(
        trunc_heal_budget_reasoning_aware(Some("stop"), 10_000, 10_000, 9_000),
        None
    );
    // 推理感知结果同样受封顶约束
    assert_eq!(
        trunc_heal_budget_reasoning_aware(Some("length"), 131_072, 131_072, 130_000),
        None,
        "已达封顶:无增长即不重发"
    );
}

/// 腰斩 JSON 兜底(自愈重发仍截断的末层防线):按未通过兜底,结论段
/// 不落半截 JSON(原样进 result「## 审计结论」卡既难看又误导),给干净说明
#[test]
fn audit_truncated_json_fallback_clean_conclusion() {
    let v = parse_audit(r#"{"通过":false,"打回":[{"main":2"#, 2);
    assert!(!v.pass, "截断审计输出应兜底未通过");
    assert!(v.kickbacks.is_empty());
    assert!(
        !v.conclusion.contains('{'),
        "截断 JSON 不应原样进结论段,实际:{}",
        v.conclusion
    );
}

/// 审计打回按定位去重:同一主同一子目标重复打回只保留首条;
/// 「同主不同子目标」与「同主整主 + 具体子目标」均保留(粒度不同)
#[test]
fn parse_audit_dedups_same_target() {
    let v = parse_audit(
        r#"{"通过":false,"打回":[{"main":1,"step":1,"instruction":"首条指令"},{"main":2,"instruction":"主二补做"},{"main":1,"step":1,"instruction":"重复条目应丢弃"},{"main":1,"step":2,"instruction":"同主不同子目标保留"}],"结论":"x"}"#,
        3,
    );
    assert!(!v.pass);
    assert_eq!(
        v.kickbacks,
        vec![
            Kickback {
                main: 0,
                step: Some(0),
                instruction: "首条指令".to_string()
            },
            Kickback {
                main: 1,
                step: None,
                instruction: "主二补做".to_string()
            },
            Kickback {
                main: 0,
                step: Some(1),
                instruction: "同主不同子目标保留".to_string()
            },
        ],
        "同一主同一子目标重复打回应只保留首条"
    );
}

/// 从 plan 重建各主产出:失败子目标附错误说明;全败主为 None;
/// 【主Agent-N】前缀被剥离
#[test]
fn rebuild_outputs_from_plan() {
    let mains = vec![
        TeamMain {
            name: "甲".into(),
            goals: vec![],
            after: Vec::new(),
        },
        TeamMain {
            name: "乙".into(),
            goals: vec![],
            after: Vec::new(),
        },
    ];
    let step_ranges = vec![vec![0, 1], vec![2]];
    let plan = vec![
        TaskStep {
            name: "【主Agent-1】子一".into(),
            goal: String::new(),
            status: TaskStepStatus::Done,
            result: "产出甲一".into(),
            node_id: None,
            files: Vec::new(),
        },
        TaskStep {
            name: "【主Agent-1】子二".into(),
            goal: String::new(),
            status: TaskStepStatus::Error,
            result: "上游抖动".into(),
            node_id: None,
            files: Vec::new(),
        },
        TaskStep {
            name: "【主Agent-2】子三".into(),
            goal: String::new(),
            status: TaskStepStatus::Error,
            result: "也失败了".into(),
            node_id: None,
            files: Vec::new(),
        },
    ];
    let outputs = rebuild_outputs(&mains, &step_ranges, &plan);
    let o0 = outputs[0].as_deref().unwrap_or("");
    assert!(
        o0.contains("子目标 1「子一」"),
        "应带主内 1-based 编号且剥离前缀: {o0}"
    );
    assert!(o0.contains("产出甲一"), "应含成功产出: {o0}");
    assert!(o0.contains("执行失败"), "失败子目标应附说明: {o0}");
    assert!(outputs[1].is_none(), "全败主产出应为 None");
    assert_eq!(strip_main_prefix("【主Agent-3】写作"), "写作");
    assert_eq!(strip_main_prefix("无前缀"), "无前缀");
}

/// SKY-6 第 2 项:守卫截断的 partial 步骤要进各主产出(审计可见),
/// 并出现在 partial 终态原因里;但它不是 error,不得拖累「失败步骤」判定。
#[test]
fn partial_steps_enter_outputs_and_labels() {
    let mains = vec![TeamMain {
        name: "甲".into(),
        goals: vec![],
        after: Vec::new(),
    }];
    let step_ranges = vec![vec![0]];
    let plan = vec![TaskStep {
        name: "【主Agent-1】读分片".into(),
        goal: String::new(),
        status: TaskStepStatus::Partial,
        result: "已有正文\n\n[守卫截断] 连续只读空转熔断".into(),
        node_id: None,
        files: Vec::new(),
    }];
    let outputs = rebuild_outputs(&mains, &step_ranges, &plan);
    let out = outputs[0]
        .as_deref()
        .expect("partial 步骤有产出,不应为 None");
    assert!(out.contains("[守卫截断]"), "审计输入应看到截断标记: {out}");
    assert!(!has_error_steps(&plan), "partial 不是 error");
    assert_eq!(
        partial_step_labels(&plan),
        vec!["主 Agent-1「读分片」".to_string()],
        "原因文本应带主序号与子目标名"
    );
}

/// JoinError(panic)兜底:无法从 JoinError 反查是哪一主 panic,保守地把本轮 batch 中
/// 仍 running 的步骤统一置 error(终态至少 partial,不留永远 running 的步骤);
/// 已终态步骤与 batch 外步骤不动。
///(集成侧不可注入:mock 链路无法让某一主 panic,故抽取 fail_batch_running_steps 单测覆盖。)
#[test]
fn fail_batch_running_steps_marks_running_error() {
    fn mk(status: TaskStepStatus) -> TaskStep {
        TaskStep {
            name: "子目标".into(),
            goal: String::new(),
            status,
            result: String::new(),
            node_id: None,
            files: Vec::new(),
        }
    }
    let mut plan = vec![
        mk(TaskStepStatus::Done),
        mk(TaskStepStatus::Running),
        mk(TaskStepStatus::Running),
        mk(TaskStepStatus::Pending),
    ];
    // 4 主各领 1 步;本轮 batch = 主 0/1/3(主 2 不在本轮)
    let step_ranges = vec![vec![0], vec![1], vec![2], vec![3]];
    let failed = fail_batch_running_steps(&mut plan, &[0, 1, 3], &step_ranges);
    assert_eq!(failed, 1, "仅 batch 内仍 running 的步骤应置 error");
    assert_eq!(plan[0].status, TaskStepStatus::Done, "已终态步骤不动");
    assert_eq!(plan[1].status, TaskStepStatus::Error);
    assert!(!plan[1].result.is_empty(), "置 error 应附原因文本");
    assert_eq!(plan[2].status, TaskStepStatus::Running, "batch 外步骤不动");
    assert_eq!(
        plan[3].status,
        TaskStepStatus::Pending,
        "非 running 步骤不动"
    );

    // 幂等:无 running 步骤时不重复置位、不污染状态
    let failed2 = fail_batch_running_steps(&mut plan, &[0], &step_ranges);
    assert_eq!(failed2, 0, "无 running 步骤应返回 0");
    assert_eq!(plan[0].status, TaskStepStatus::Done);
}

/// 终态判定以 plan 终态为准:补做把步骤覆盖为 Done 后,不得再判 partial
///(旧 had_error 累加标志永不复位 → 假 partial,实跑问题 2)
#[test]
fn terminal_error_steps_drive_partial() {
    let mk = |name: &str, status: TaskStepStatus| TaskStep {
        name: name.into(),
        goal: String::new(),
        status,
        result: "r".into(),
        node_id: None,
        files: Vec::new(),
    };
    let plan = vec![
        mk("【主Agent-1】子一", TaskStepStatus::Done),
        mk("【主Agent-2】子二", TaskStepStatus::Error),
    ];
    assert!(has_error_steps(&plan), "存在 Error 步骤应判失败");
    assert_eq!(
        error_step_labels(&plan),
        vec!["主 Agent-2「子二」".to_string()],
        "原因文本应带主序号与子目标名"
    );
    // 补做成功:同一位置覆盖为 Done → 不再有失败步骤(不应判 partial)
    let mut healed = plan.clone();
    healed[1].status = TaskStepStatus::Done;
    assert!(!has_error_steps(&healed), "补做成功后不得再判 partial");
    assert!(error_step_labels(&healed).is_empty());
}

/// 补做轮 prior 种子:子目标粒度补做时,种入该主既有 Done 产出;被打回子目标
/// 的旧版本特别标注「旧版本」;未打回的同门产出原样注入;整主补做/首轮为空
/// (实跑问题 2:补做轮跳过未点名子目标导致 prior 为空、版本冲突原样复现)
#[test]
fn kickback_prior_seeds_sibling_outputs() {
    let plan = vec![
        TaskStep {
            name: "【主Agent-1】子一".into(),
            goal: String::new(),
            status: TaskStepStatus::Done,
            result: "甲产出".into(),
            node_id: None,
            files: Vec::new(),
        },
        TaskStep {
            name: "【主Agent-1】子二".into(),
            goal: String::new(),
            status: TaskStepStatus::Done,
            result: "旧乙产出".into(),
            node_id: None,
            files: Vec::new(),
        },
        TaskStep {
            name: "【主Agent-1】子三".into(),
            goal: String::new(),
            status: TaskStepStatus::Error,
            result: "失败".into(),
            node_id: None,
            files: Vec::new(),
        },
    ];
    let step_ranges = vec![vec![0, 1, 2]];
    // 只补做子目标下标 1(第 2 个):应种入子一(同门)+ 子二的旧版本;失败子三不种
    let seed = before_kickback_prior(&step_ranges, &plan, 0, Some(&[1]));
    assert_eq!(seed.len(), 2, "只种入 Done 产出: {seed:?}");
    assert_eq!(seed[0].0, "子一", "未打回同门产出原样种入");
    assert_eq!(seed[0].1, "甲产出");
    assert_eq!(seed[1].0, "子二(旧版本,本次产出将替换它)");
    assert_eq!(seed[1].1, "旧乙产出");
    // 整主补做(goal_filter=None):不种,走首轮同构路径(循环内自然累积)
    assert!(before_kickback_prior(&step_ranges, &plan, 0, None).is_empty());
}

/// 审计 JSON 合法但缺「结论」字段:不把裸 JSON 灌进「## 审计结论」卡
#[test]
fn parse_audit_missing_conclusion_uses_placeholder() {
    let v = parse_audit(r#"{"通过":true,"打回":[]}"#, 2);
    assert!(v.pass);
    assert_eq!(v.conclusion, "(审计未给出结论)");
    assert!(!v.conclusion.contains('{'), "不得回填裸 JSON");
}
