// team 模式(批次 4.3b,首版只做自动拓扑):规划器拆目标并归并 2~4 个主 agent
//(每主 1~4 子目标,全局 ≤15 封顶)→ 各主并行跑 solo 同款工具自循环(独立 run_id/
// task:{id}:main:{n} 虚拟 session、AbortFlag、cancel 同源;可经 agentgo 派子 agent,
// 子 agent 路径与 multi 相同);主 agent 内按子目标逐个独立执行(每子目标一次调用,
// 各写各自步骤的 result/status,同一主复用同一虚拟 session 保持人设一致)→
// 审计 agent 一致性/质量/覆盖度审查(缺漏打回指定主补做,最多 1 轮;补做完成后
// 追加一次终审——只产出结论文本,不再打回)→ 升华整合输出。
// 结果契约:result = 整合文本 + "\n\n## 审计结论\n" + 审计文本(前端按此拆卡);
// 无打回时审计文本 = 首次审计结论,有打回时 = 终审结论。
// plan 步骤名 = 「【主Agent-N】子目标名」(前端分工卡按此前缀分组)。
// 手动拓扑(用户指定主 agent 数量/人设/分工)预留,待前端入口批次(docs/功能.md)。
use super::context::TaskRunContext;
use super::executor::{usage_as_output, ModeExecutor};
use super::solo::{run_agent_loop, AgentLoopCall, AgentLoopOutput};
use super::wiring::{file_exports, single_file_requested, wiring_issues};
use crate::agents::engine::AgentEngine;
use crate::models::ownership::{normalize_workspace_rel, WriteOwnership};
use crate::models::types::{
    LlmMessage, TaskEventKind, TaskStatus, TaskStep, TaskStepStatus, TokenUsage,
};
use crate::services::prompt_kit::untrusted_boundary;
use crate::services::task_core::prompt_consts::{
    team_audit_tool_guidance, SUMMARIZER_PROMPT, TEAM_ASSEMBLY_CONTRACT_NOTE, TEAM_AUDIT_PROMPT,
    TEAM_FINAL_AUDIT_PROMPT, TEAM_PLANNER_PROMPT, TEAM_SINGLE_FILE_ASSEMBLY_NOTE,
    TEAM_SINGLE_FILE_PLANNER_NOTE,
};
use crate::services::task_core::{fallback_terminal, TaskBackend, TaskGenOutput, TaskTerminal};
use futures::future::BoxFuture;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio::task::JoinSet;

/// run_mains_parallel 并行 JoinSet 的单主产出:(主序号, [(子目标序号, 子目标名, 生成结果)])
type MainJoinOutput = (usize, Vec<(usize, String, Result<AgentLoopOutput, String>)>);

/// 主 agent 分工数上限(规划器约定 2~4;超出部分归并进末位主 agent)
const TEAM_MAX_MAINS: usize = 4;
/// 每主 agent 子目标数上限(规划器约定 1~4;超出部分截断并记日志)
const TEAM_MAX_GOALS_PER_MAIN: usize = 4;
/// 全局子目标数上限(4 主 × 4 子 = 16 可超,超出部分从末位主倒序截断,每主至少留 1)
const TEAM_MAX_GOALS_GLOBAL: usize = 15;
/// 规划/审计调用的温度(结构化 JSON 输出,与 legacy planner 同口径)
const TEAM_JSON_TEMPERATURE: f64 = 0.3;
/// 规划调用初始 max_tokens(推理模型 reasoning 与正文共用预算,同 PLAN_INITIAL_MAX_TOKENS)
const TEAM_PLAN_INITIAL_MAX_TOKENS: u32 = 2048;
/// 规划解析最大尝试次数(截断/空输出翻倍预算,同 PLAN_MAX_ATTEMPTS)
const TEAM_PLAN_MAX_ATTEMPTS: u32 = 3;
/// 重试预算翻倍上限(与设置页 max_tokens 上限一致)
const TEAM_RETRY_MAX_TOKENS_CAP: u32 = 131_072;
/// 结构化/长文本阶段(审计、终审、汇总)的初始输出预算下限。
/// 2026-09-15 实录:审计 `max_tokens=10000` 被 reasoning 吃掉 9894、正文仅 178 字符
/// 即被截断,自愈翻倍到 20000 重发才成功——首发即给足可省掉这一整个 50 秒左右
/// 的空烧往返。默认 `default_max_tokens=1024` 时更小,结构化 JSON 必然腰斩。
const TEAM_STRUCTURED_MIN_TOKENS: u32 = 16384;

/// 主执行池给审计/终审/汇总预留的秒数下限/上限(SKY-7,见 `post_exec_reserve`)。
/// 下限 10s 覆盖一次快速审计/汇总往返;上限 180s 防止长任务把 5% 无界放大。
const TEAM_POST_EXEC_RESERVE_MIN_SECS: u64 = 10;
const TEAM_POST_EXEC_RESERVE_MAX_SECS: u64 = 180;

/// 每个子目标最多声明的负责文件数(TMR-4;规划器输出不可信,防超长/超量)。
const TEAM_MAX_DECLARED_FILES_PER_GOAL: usize = 32;

/// 清洗规划器给的 `files`:归一化相对路径、去重、限量;非法项静默丢弃。
///
/// 非法项不报错的原因:文件声明是**增强约束**,不是拓扑正确性的前提。规划器偶发
/// 输出通配符/绝对路径时,丢掉该项仍可用「首写者认领」兜底,不该让整个 team 任务
/// 因一个路径重跑规划。
fn sanitize_declared_files(raw: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        if out.len() >= TEAM_MAX_DECLARED_FILES_PER_GOAL {
            break;
        }
        let Some(key) = normalize_workspace_rel(&item) else {
            continue;
        };
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// 由规划器拓扑构造本次 team 运行共享的 `WriteOwnership`。
///
/// owner 取稳定的 `主 Agent-{n}`(与提示词/错误文案同一口径);同一文件被多个主
/// 声明时**首声明胜出**并记 warn——规划器提示词已禁止重叠,这里只做防御,
/// 不因冲突重新规划。
fn build_write_ownership(mains: &[TeamMain]) -> Arc<WriteOwnership> {
    let mut declarations: Vec<(String, String)> = Vec::new();
    let mut owner_of: HashMap<String, String> = HashMap::new();
    for (i, m) in mains.iter().enumerate() {
        let owner = format!("主 Agent-{}", i + 1);
        for goal in &m.goals {
            for path in &goal.files {
                if let Some(first) = owner_of.get(path) {
                    if first != &owner {
                        tracing::warn!(
                            path = path.as_str(),
                            first_owner = first.as_str(),
                            ignored_owner = owner.as_str(),
                            "team 规划器把同一文件声明给了多个主 agent,首声明胜出"
                        );
                    }
                    continue;
                }
                owner_of.insert(path.clone(), owner.clone());
                declarations.push((owner.clone(), path.clone()));
            }
        }
    }
    Arc::new(WriteOwnership::new(declarations))
}

/// 一个主 agent 的负责文件并集(去重、保持规划器顺序;提示词与 scope 共用)。
fn declared_files_of(main: &TeamMain) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for goal in &main.goals {
        for path in &goal.files {
            if !out.contains(path) {
                out.push(path.clone());
            }
        }
    }
    out
}

/// 是否声明了 HTML 入口(总装主;单文件装配纪律只对这类主注入)。
fn declares_html(files: &[String]) -> bool {
    files.iter().any(|f| {
        let lower = f.to_ascii_lowercase();
        lower.ends_with(".html") || lower.ends_with(".htm")
    })
}

/// 审计/终审/汇总输入里的「工作区声明文件实况」(SKY-3,2026-10-23)。
///
/// 为什么需要:空岛 team 实跑中,审计员用 `fs_glob("*.js")` 核对文件,只看到 9 个
/// 分片便断言「没有 index.html」——而它其实已存在(27,882B 骨架)。文件核对不能
/// 依赖模型自选通配模式。这里按规划器声明的**全类型**路径逐项 stat,把「存在/缺失 +
/// 字节数」直接写进输入,审计与汇总都以同一份事实为起点;审计若与清单冲突,以清单为准。
///
/// 文本文件另标「含 N 行 @@ 未替换槽位」(SKY-4/SKY-5):占位符是否清零不再靠审计
/// 自己 grep,引擎先给出可直接引用的计数。
///
/// SKY-1 确定性部分(2026-10-24):同一份清单后附**静态接线检查**——导入名是否真被
/// 导出、本地模块/资源是否存在、单文件目标是否仍外链本地文件。审计不必靠自由文本
/// 发现 `buildSubIslands` 与 `generateSubIslands` 这类硬错配。
///
/// 工作区不可访问时返回空串(与交付摘要同口径:不制造假缺失)。
fn workspace_manifest(workspace: &std::path::Path, mains: &[TeamMain], goal: &str) -> String {
    if !workspace.is_dir() {
        return String::new();
    }
    let mut seen: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for (i, main) in mains.iter().enumerate() {
        for path in declared_files_of(main) {
            if seen.contains(&path) {
                continue;
            }
            seen.push(path.clone());
            let abs = workspace.join(&path);
            let state = match std::fs::metadata(&abs) {
                Ok(m) if m.is_file() => {
                    let mut state = format!("存在({}B)", m.len());
                    if let Some(n) =
                        super::delivery::placeholder_line_count(&abs).filter(|n| *n > 0)
                    {
                        state.push_str(&format!(",含 {n} 行 @@ 未替换槽位"));
                    }
                    state
                }
                Ok(_) => "存在但非普通文件".to_string(),
                Err(_) => "缺失".to_string(),
            };
            lines.push(format!("- 主 Agent-{} `{path}`:{state}", i + 1));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "\n工作区声明文件实况(引擎在审计前按 TaskStep.files 全类型枚举;与主 agent 自述冲突时以本清单为准):\n{}\n",
        lines.join("\n")
    );
    let wiring = wiring_issues(workspace, &seen, goal);
    if !wiring.is_empty() {
        out.push_str(
            "\n引擎静态接线检查(导入名未导出/本地模块或资源不存在/单文件交付外链;确定性判定,与自述冲突时以本清单为准):\n",
        );
        for issue in &wiring {
            out.push_str(&format!("- {issue}\n"));
        }
    }
    out
}

/// 一个主 agent 的分工:分工名 + 子目标(1~4 个)+ 依赖的其他主 agent。
#[derive(Debug, Clone)]
struct TeamMain {
    name: String,
    goals: Vec<TaskStep>,
    /// 依赖的主 agent 下标(0-based;已去自环/越界/重复)。空 = 无依赖,按现状并行。
    after: Vec<usize>,
}

/// 审计打回条目:定位粒度到「主 agent 的某个子目标」。
/// `step` 为 1-based 的主内子目标序号(审计输出新增可选字段);缺省(None)= 审计未
/// 指明子目标,退回整主补做,保持对旧审计输出的兼容。定位到具体子目标可避免
/// 「打回一个子目标却重跑该主全部子目标、各再造一版」的版本爆炸(实跑问题 2)。
#[derive(Debug, Clone, PartialEq)]
struct Kickback {
    /// 主 agent 序号(0-based)
    main: usize,
    /// 主内子目标序号(0-based;None = 整主)
    step: Option<usize>,
    /// 补做指令
    instruction: String,
}

/// 审计结论:通过/打回列表/结论文本。
/// 解析失败或输出截断时**按未通过兜底**(实跑问题 2:旧实现默认 pass=true,审计可
/// 静默失效;审计是质量闸门,读不出明确通过就不应判定通过)——结论段给干净说明,
/// 不把半截 JSON 原样灌进 result;打回为空时由调用方映射为 partial 终态。
#[derive(Debug, Clone)]
struct AuditVerdict {
    pass: bool,
    kickbacks: Vec<Kickback>,
    conclusion: String,
}

/// 规划器输出拓扑的原始 JSON 形态
#[derive(Deserialize)]
struct TeamTopologyRaw {
    mains: Vec<TeamMainRaw>,
}

#[derive(Deserialize)]
struct TeamMainRaw {
    name: String,
    #[serde(default)]
    goals: Vec<TeamGoalRaw>,
    /// 依赖的其他主 agent(TMR3-3;规划器用 1-based 主号,可省略)。
    /// 只做主级依赖:总装主声明 after=[提供分片的主号],执行器按拓扑分波。
    /// 2026-10-23 真实模型实测:规划器会把主 agent **名称**写进 after
    /// (`["纹理与材质主"]`),整个拓扑解析失败;故用 [`AfterRef`] 宽容两种形态。
    #[serde(default)]
    after: Option<Vec<AfterRef>>,
}

/// 依赖引用的宽容形态(2026-10-23):提示词要求 1-based 数字序号,真实模型
/// (deepseek/deepseek-v4.1-flash)仍会写主 agent 名称。名称在解析期按
/// 「原始 mains 下标」解析,再随空名主过滤做重映射(与数字序号同一条路径)。
#[derive(Deserialize)]
#[serde(untagged)]
enum AfterRef {
    /// 规范形态:1-based 数字序号
    Index(usize),
    /// 宽容形态:主 agent 名称;纯数字字符串按序号处理
    Name(String),
}

/// 把规划器的 after 原始项解析为 1-based 原始主号;无法解析的名称静默丢弃
/// (依赖是排序增强,不该让一个写错的名字毁掉整份拓扑)。
fn resolve_after(raw: Vec<AfterRef>, name_to_raw: &HashMap<String, usize>) -> Vec<usize> {
    raw.into_iter()
        .filter_map(|r| match r {
            AfterRef::Index(n) => Some(n),
            AfterRef::Name(s) => {
                let s = s.trim();
                s.parse::<usize>()
                    .ok()
                    .or_else(|| name_to_raw.get(s).map(|i| i + 1))
            }
        })
        .collect()
}

/// 规划器的子目标原始形态(TMR-4:多一个可选的 `files`)。
///
/// 独立于 `TaskStep` 而不是直接复用:规划器只该输出 name/goal/files,不该把
/// `status`/`result`/`node_id` 等运行态字段带进来;`files` 用 Option 接纳模型
/// 常见的 `"files": null`(直接 `Vec` 会在 null 上整条拓扑解析失败)。
#[derive(Deserialize)]
struct TeamGoalRaw {
    #[serde(default)]
    name: String,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    files: Option<Vec<String>>,
}

/// 解析 team 规划输出:剥 markdown 代码块 → 取首个 '{' 到末个 '}' 子串 → serde 解析;
/// 过滤空分工/空子目标,归并超编主 agent(>4 并入第 4 位)、截断超编子目标(每主 >4
/// 截前 4),再做全局 ≤15 封顶(4×4=16 可超,从末位主倒序截断,每主至少留 1)。
/// mains 为空报错(交给重试);仅 1 个主 agent 时接受(降级拓扑,记日志),不强行拆分。
fn parse_team_topology(text: &str) -> Result<Vec<TeamMain>, String> {
    let t = text.trim();
    let stripped = t
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let candidate = match (stripped.find('{'), stripped.rfind('}')) {
        (Some(lo), Some(hi)) if lo < hi => &stripped[lo..=hi],
        _ => stripped,
    };
    let raw: TeamTopologyRaw =
        serde_json::from_str(candidate).map_err(|e| format!("解析团队拓扑失败: {e}"))?;
    // TMR3-3:先保留原始主号,过滤空主后重映射 after。规划器用 1-based 主号,
    // 空名/空子目标主会让后续下标整体前移;不重映射会把依赖指到错误的主。
    // 2026-10-23:after 允许写名称——先建「名称 → 原始下标」索引再消费 mains。
    let name_to_raw: HashMap<String, usize> = raw
        .mains
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let name = m.name.trim();
            (!name.is_empty()).then(|| (name.to_string(), i))
        })
        .collect();
    let mut kept: Vec<(usize, TeamMain, Vec<usize>)> = Vec::new();
    for (raw_idx, m) in raw.mains.into_iter().enumerate() {
        let TeamMainRaw { name, goals, after } = m;
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let goals: Vec<TaskStep> = goals
            .into_iter()
            .filter_map(|g| {
                let name = g.name.trim().to_string();
                let goal = g.goal.trim().to_string();
                if name.is_empty() && goal.is_empty() {
                    return None;
                }
                Some(TaskStep {
                    name,
                    goal,
                    files: sanitize_declared_files(g.files.unwrap_or_default()),
                    ..TaskStep::default()
                })
            })
            .collect();
        if goals.is_empty() {
            continue;
        }
        kept.push((
            raw_idx,
            TeamMain {
                name,
                goals,
                after: Vec::new(),
            },
            resolve_after(after.unwrap_or_default(), &name_to_raw),
        ));
    }
    if kept.is_empty() {
        return Err("解析团队拓扑失败: 规划输出不含任何有效主 agent 分工".into());
    }
    let mut remap: HashMap<usize, usize> = HashMap::new();
    for (new_idx, (raw_idx, _, _)) in kept.iter().enumerate() {
        remap.insert(*raw_idx, new_idx);
    }
    let mut mains: Vec<TeamMain> = Vec::with_capacity(kept.len());
    for (new_idx, (_, mut main, after_raw)) in kept.into_iter().enumerate() {
        main.after = after_raw
            .into_iter()
            .filter_map(|n| n.checked_sub(1)) // 规划器 1-based → 0-based
            .filter_map(|raw_ref| remap.get(&raw_ref).copied())
            .filter(|&dep| dep != new_idx)
            .collect();
        main.after.sort_unstable();
        main.after.dedup();
        mains.push(main);
    }
    // 超编归并:第 4 位之后的主 agent 子目标并入第 4 位
    if mains.len() > TEAM_MAX_MAINS {
        let tail = mains.split_off(TEAM_MAX_MAINS);
        let merged: usize = tail.iter().map(|m| m.goals.len()).sum();
        let mut tail_after: Vec<usize> = Vec::new();
        for m in tail {
            tail_after.extend(m.after);
            mains[TEAM_MAX_MAINS - 1].goals.extend(m.goals);
        }
        // 被并入主(原下标 ≥4)在所有 after 引用中统一映射到第 4 主
        for m in mains.iter_mut() {
            m.after = m
                .after
                .iter()
                .map(|&d| {
                    if d >= TEAM_MAX_MAINS {
                        TEAM_MAX_MAINS - 1
                    } else {
                        d
                    }
                })
                .collect();
        }
        mains[TEAM_MAX_MAINS - 1]
            .after
            .extend(tail_after.into_iter().map(|d| {
                if d >= TEAM_MAX_MAINS {
                    TEAM_MAX_MAINS - 1
                } else {
                    d
                }
            }));
        for (i, m) in mains.iter_mut().enumerate() {
            m.after.retain(|&d| d != i);
            m.after.sort_unstable();
            m.after.dedup();
        }
        tracing::warn!(
            merged_goals = merged,
            "team 规划主 agent 超编,已归并进末位主 agent"
        );
    }
    // 每主子目标超编截断(保留前 TEAM_MAX_GOALS_PER_MAIN)
    for (i, m) in mains.iter_mut().enumerate() {
        if m.goals.len() > TEAM_MAX_GOALS_PER_MAIN {
            tracing::warn!(
                main = i + 1,
                dropped = m.goals.len() - TEAM_MAX_GOALS_PER_MAIN,
                "team 规划子目标超编,已截断"
            );
            m.goals.truncate(TEAM_MAX_GOALS_PER_MAIN);
        }
    }
    // 全局子目标 ≤15 封顶(4 主 × 4 子 = 16 可超):从末位主倒序截断,每主至少留 1
    let mut total: usize = mains.iter().map(|m| m.goals.len()).sum();
    if total > TEAM_MAX_GOALS_GLOBAL {
        let mut dropped = 0usize;
        for m in mains.iter_mut().rev() {
            if total <= TEAM_MAX_GOALS_GLOBAL {
                break;
            }
            let overflow = total - TEAM_MAX_GOALS_GLOBAL;
            // 每主至少保留 1 个子目标(空分工已在前面过滤,此处守住不再制造空主)
            let drop = overflow.min(m.goals.len().saturating_sub(1));
            m.goals.truncate(m.goals.len() - drop);
            total -= drop;
            dropped += drop;
        }
        tracing::warn!(
            dropped = dropped,
            "team 规划子目标全局超编,已按 ≤15 封顶截断"
        );
    }
    // TMR3-3:规划器漏写 after 时的确定性兜底——若主 B 的子目标文本提到主 A
    // 声明的文件路径,则 B 依赖 A(总装经常在目标里直接点名 parts/*.js)。
    infer_dependencies_from_files(&mut mains);
    if mains.len() == 1 {
        tracing::warn!("team 规划仅产出 1 个主 agent,按降级拓扑执行");
    }
    Ok(mains)
}

/// 主 start 是否(传递地)依赖 target(TMR3-3 环检测;n≤4,DFS 足够)。
/// 返回 true 表示 `start.after` 链上存在 target。
fn depends_transitively(mains: &[TeamMain], start: usize, target: usize) -> bool {
    fn walk(mains: &[TeamMain], cur: usize, target: usize, seen: &mut [bool]) -> bool {
        if cur == target {
            return true;
        }
        if cur >= mains.len() || seen[cur] {
            return false;
        }
        seen[cur] = true;
        mains[cur]
            .after
            .iter()
            .any(|&dep| walk(mains, dep, target, seen))
    }
    if start >= mains.len() {
        return false;
    }
    let mut seen = vec![false; mains.len()];
    walk(mains, start, target, &mut seen)
}

/// 从文件声明推断主级依赖(TMR3-3):主 B 的任一子目标文本提到主 A 声明的文件
/// 路径时,B 依赖 A。
///
/// 为什么需要这层兜底:规划器提示词已经要求总装主用 `after` 声明依赖,但模型
/// 可能漏写;而「总装目标里点名 parts/a.js」是模型很自然的写法。推断是确定性的,
/// 成环则跳过该边(不重跑规划),最坏退化为现状并行。
///
/// 误报收敛(TMR3-L3):① 主自己声明的文件不算依赖证据(它本就要写/读自己的产物);
/// ② 「不要覆盖 parts/a.js」这类**否定语境**的提及是约束,不是消费关系,不计边。
/// 主级粒度保留:同一主内多个子目标仍串行,细粒度 DAG 需拆散 `run_main` 的会话
/// 连续性/上游产出注入,当前没有需求支撑(见 `docs/遗留.md` §三十 TMR3-L3)。
fn infer_dependencies_from_files(mains: &mut [TeamMain]) {
    let decls: Vec<Vec<String>> = mains.iter().map(declared_files_of).collect();
    let texts: Vec<String> = mains
        .iter()
        .map(|m| {
            m.goals
                .iter()
                .map(|g| format!("{} {}", g.name, g.goal))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    for i in 0..mains.len() {
        for (j, decl) in decls.iter().enumerate() {
            if i == j || mains[i].after.contains(&j) {
                continue;
            }
            let hit = decl.iter().find(|path| {
                path.len() >= 4
                    && (path.contains('/') || path.contains('.'))
                    && !decls[i].contains(*path)
                    && mentions_path_unnegated(&texts[i], path.as_str())
            });
            // 加边 i→j(i 依赖 j)会成环 = j 已经(传递地)依赖 i
            if let Some(path) = hit {
                if depends_transitively(mains, j, i) {
                    continue;
                }
                mains[i].after.push(j);
                tracing::info!(
                    main = i + 1,
                    depends_on = j + 1,
                    path = path.as_str(),
                    "team 依赖推断:目标文本提到上游主声明的文件"
                );
            }
        }
    }
}

/// 文本里对该路径的提及是否至少有一处**不带否定前缀**(TMR3-L3)。
///
/// 规划器的约束句常写成「不要覆盖 parts/a.js」「无需读取 index.html」——这类
/// 提及与「总装需要 parts/a.js」的消费关系相反,直接按子串判依赖会把并行波次
/// 无谓地串行化。取提及前 8 个字符做否定词检查(够覆盖常见中文否定句式,又不会
/// 把上一句的否定词误算到下一句的路径上);同一路径出现多次时,只要有一处非否定
/// 提及就仍视为依赖证据。
fn mentions_path_unnegated(text: &str, path: &str) -> bool {
    const NEGATION_WINDOW_CHARS: usize = 8;
    const NEGATIONS: &[&str] = &[
        "不要", "不得", "禁止", "避免", "无需", "不用", "切勿", "勿", "不能", "不应", "别再", "别",
    ];
    let mut from = 0usize;
    while let Some(pos) = text[from..].find(path) {
        let start = from + pos;
        // 按字符边界回退 8 个字符;不足 8 个则从头取
        let prefix_start = text[..start]
            .char_indices()
            .rev()
            .nth(NEGATION_WINDOW_CHARS - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let prefix = &text[prefix_start..start];
        if !NEGATIONS.iter().any(|n| prefix.contains(n)) {
            return true;
        }
        from = start + path.len();
    }
    false
}

/// 按主级依赖把主 agent 分成执行波次(TMR3-3;Kahn 拓扑)。
///
/// - 无依赖 = 一波,与 TMR3 之前「全部并行」逐字节等价;
/// - 有依赖 = 依赖主先进前面的波,后续波等前一波全部结束;
/// - 检测到环(规划器幻觉/推断兜底仍成环)时记 warn,把剩余主放同一波并行,
///   保证每个主都会被执行(不因依赖错误整任务失败)。
fn plan_waves(mains: &[TeamMain]) -> Vec<Vec<usize>> {
    let n = mains.len();
    let mut indegree = vec![0usize; n];
    let mut edges = vec![Vec::<usize>::new(); n];
    for (i, m) in mains.iter().enumerate() {
        for &dep in &m.after {
            if dep < n && dep != i && !edges[dep].contains(&i) {
                edges[dep].push(i);
                indegree[i] += 1;
            }
        }
    }
    let mut waves = Vec::new();
    let mut done = vec![false; n];
    let mut remaining = n;
    while remaining > 0 {
        let wave: Vec<usize> = (0..n).filter(|&i| !done[i] && indegree[i] == 0).collect();
        if wave.is_empty() {
            let rest: Vec<usize> = (0..n).filter(|&i| !done[i]).collect();
            tracing::warn!(
                mains = ?rest,
                "team 主级依赖成环,剩余主 agent 降级为同一波并行"
            );
            waves.push(rest);
            break;
        }
        for &i in &wave {
            done[i] = true;
            remaining -= 1;
            for &j in &edges[i] {
                indegree[j] = indegree[j].saturating_sub(1);
            }
        }
        waves.push(wave);
    }
    waves
}

/// 团队「集成/总装主」判定(SKY-7,2026-10-24;收窄 SKY-6 的 `after` 非空判据)。
///
/// 只有**依赖 DAG 的汇点**才享有两个计数守卫(连续只读 / 声明文件无进展)的豁免:
/// 有上游依赖,且没有任何其他主依赖它(本主之后没有下游),即它是某条执行链的终点、
/// 需要先读完全部上游产物再写最终交付物。
///
/// 为什么不能只看 `after` 非空:2026-10-24 全题复跑里规划器把三主排成 1→2→3 的串行链,
/// 主 2 有 `after` 但只是**中段生产者**(下游还有总装主),被误判为集成主后两道守卫
/// 同时关闭——342 轮零写盘、吃掉 52 分钟,总装主从未得到启动窗口。
///
/// 边界:
/// - 无依赖的并行主 → false(没有上游可读,长读不构成理由);
/// - 环 → 全部 false(环会被 `plan_waves` 降级为同一波,守卫继续兜底);
/// - 多个汇点(多个终点交付) → 各自 true(SKY-6 的「两主:分片 + 总装」形态不变)。
fn is_integration_main(mains: &[TeamMain], index: usize) -> bool {
    let Some(main) = mains.get(index) else {
        return false;
    };
    if main.after.is_empty() {
        return false;
    }
    !mains
        .iter()
        .enumerate()
        .any(|(j, other)| j != index && other.after.contains(&index))
}

/// 步骤终态 = 收尾方式 + 本步声明文件的磁盘事实(SKY-8,2026-10-24)。
///
/// 规则(与 `docs/契约-协议与配置.md` 的步骤状态语义表同源):
/// - 正常返回 → `done`;
/// - **守卫截断**:声明文件全部存在且内容体检通过 → `done`(结果留一条收尾说明);
///   否则 `partial`;
/// - **预算收尾**:声明文件有缺失/空/未替换槽位 → `partial`;否则 `done`
///   (TMR-3 既有语义不变);
/// - 未声明文件或工作区不可访问:保持旧语义(守卫→partial,预算→done),不制造假判定。
/// - **单文件目标**:声明了 HTML 但不是最终入口的**中间模板**步骤,以最终入口的
///   磁盘事实为准——最终入口干净 → done;入口缺失/不干净 → partial。模板里的 `@@`
///   不再单独把中间步骤判死(模板是中间产物,最终入口才是交付物)。
///
/// 只查本步 `TaskStep.files` 的存在/空/`@@`,不递归查导入——上游文件可能尚未落盘,
/// 递归接线留给任务终态的 `delivery.content_issues`。
fn step_outcome(
    workspace: Option<&Path>,
    step: &TaskStep,
    text: &str,
    guard_stopped: Option<&str>,
    budget_stopped: bool,
    final_entry: Option<&str>,
) -> (TaskStepStatus, String) {
    let text = text.trim_end();
    if let Some(final_rel) = final_entry {
        let step_has_html = step.files.iter().any(|file| {
            let lower = file.to_ascii_lowercase();
            lower.ends_with(".html") || lower.ends_with(".htm")
        });
        let step_has_final = step
            .files
            .iter()
            .any(|file| normalize_workspace_rel(file).as_deref() == Some(final_rel));
        if step_has_html && !step_has_final {
            let final_ok = workspace
                .and_then(|ws| super::delivery::step_files_unmet(ws, &[final_rel.to_string()]))
                .map(|issues| issues.is_empty())
                .unwrap_or(false);
            let note = if final_ok {
                "[单文件交付] 中间模板不作为交付必需项；最终入口已就绪，按完成计。"
            } else {
                "[单文件交付] 最终入口缺失或内容未通过；该中间步骤仍未完成。"
            };
            return (
                if final_ok {
                    TaskStepStatus::Done
                } else {
                    TaskStepStatus::Partial
                },
                step_result_with_note(text, note),
            );
        }
    }
    let unmet = workspace.and_then(|ws| super::delivery::step_files_unmet(ws, &step.files));
    if let Some(reason) = guard_stopped {
        match unmet {
            Some(issues) if !step.files.is_empty() && issues.is_empty() => (
                TaskStepStatus::Done,
                step_result_with_note(
                    text,
                    &format!(
                        "[收尾说明] 工具循环被守卫提前截断（{reason}），但本步声明文件已全部落盘且内容体检通过，按完成计。"
                    ),
                ),
            ),
            Some(issues) => {
                let detail = if issues.is_empty() {
                    format!("[守卫截断] {reason}；该子目标可能未完成。")
                } else {
                    format!(
                        "[守卫截断] {reason}；该子目标可能未完成。未满足:{}",
                        issues.join(";")
                    )
                };
                (
                    TaskStepStatus::Partial,
                    step_result_with_note(text, &detail),
                )
            }
            None => (
                TaskStepStatus::Partial,
                step_result_with_note(
                    text,
                    &format!("[守卫截断] {reason}；该子目标可能未完成。"),
                ),
            ),
        }
    } else if budget_stopped {
        match unmet {
            Some(issues) if !step.files.is_empty() && !issues.is_empty() => (
                TaskStepStatus::Partial,
                step_result_with_note(
                    text,
                    &format!(
                        "[预算收尾] 工具循环因预算到点停止；本步声明文件未满足:{}",
                        issues.join(";")
                    ),
                ),
            ),
            _ => (TaskStepStatus::Done, text.to_string()),
        }
    } else {
        (TaskStepStatus::Done, text.to_string())
    }
}

/// 在步骤正文后追加收尾说明;正文为空时不留下前导空行。
fn step_result_with_note(text: &str, note: &str) -> String {
    if text.is_empty() {
        note.to_string()
    } else {
        format!("{text}\n\n{note}")
    }
}

/// 团队主执行阶段给审计/终审/汇总预留的墙钟(SKY-7,2026-10-24;单一出处)。
///
/// 为什么需要:团队总预算由各主共享且此前无预留,2026-10-24 全题复跑里主 2 吃掉
/// 全部 3600s,审计与汇总直接未执行,任务只能 partial。预留让确定性交付检查与
/// 质量闸门至少有一次运行窗口。
///
/// 口径:剩余 <120s 不预留(秒级集成测试与小预算行为不变);否则取 5%,
/// 夹在 `TEAM_POST_EXEC_RESERVE_MIN_SECS..=TEAM_POST_EXEC_RESERVE_MAX_SECS`。
fn post_exec_reserve(remaining: Duration) -> Duration {
    let secs = remaining.as_secs();
    if secs < 120 {
        return Duration::ZERO;
    }
    Duration::from_secs((secs / 20).clamp(
        TEAM_POST_EXEC_RESERVE_MIN_SECS,
        TEAM_POST_EXEC_RESERVE_MAX_SECS,
    ))
}

/// 主执行阶段的预算池截止期 = 全局截止期 - 审计预留;无截止期 = `None`(不引入新闸)。
fn main_pool_deadline(deadline: Option<Instant>) -> Option<Instant> {
    let d = deadline?;
    let remaining = d.saturating_duration_since(Instant::now());
    let reserve = post_exec_reserve(remaining);
    Some(d.checked_sub(reserve).unwrap_or(d))
}

/// 按「本主实际执行步数 / 剩余未执行步数」公平分配预算池(SKY-7;纯函数)。
///
/// - `pool_remaining` 或 `units` 为 0 → 0(调用方按即时截止处理,不启动新子目标);
/// - `remaining_units == 0` → 全部剩余(防御性兜底,正常调用不会命中);
/// - 池 >0 时至少给 1 秒,但绝不超过池本身(小池按池封顶,避免份额大于总预算)。
fn fair_share(pool_remaining: Duration, units: usize, remaining_units: usize) -> Duration {
    if pool_remaining.is_zero() || units == 0 {
        return Duration::ZERO;
    }
    if remaining_units == 0 {
        return pool_remaining;
    }
    let pool_nanos = pool_remaining.as_nanos();
    let nanos = (pool_nanos * units as u128 / remaining_units as u128)
        .max(1_000_000_000)
        .min(pool_nanos);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// 依赖主已完成子目标的产出摘要(TMR3-3):总装主启动时注入目标文本。
/// 只取有 `result` 的步骤;步骤名附带声明文件与**引擎实测的导出名**
/// (SKY-1,2026-10-23:总装主不必再猜上游模块有哪些导出)。
fn upstream_seed_for(
    mains: &[TeamMain],
    step_ranges: &[Vec<usize>],
    plan: &[TaskStep],
    main_index: usize,
    workspace: Option<&std::path::Path>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(main) = mains.get(main_index) else {
        return out;
    };
    for &dep in &main.after {
        let Some(range) = step_ranges.get(dep) else {
            continue;
        };
        for &si in range {
            let Some(step) = plan.get(si) else {
                continue;
            };
            if step.result.trim().is_empty() {
                continue;
            }
            let ifaces: Vec<String> = workspace
                .map(|ws| {
                    step.files
                        .iter()
                        .filter_map(|file| {
                            let exports = file_exports(&ws.join(file));
                            (!exports.is_empty())
                                .then(|| format!("{file} → {}", exports.join(" / ")))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let iface_note = if ifaces.is_empty() {
                String::new()
            } else {
                format!(" [引擎实测导出:{}]", ifaces.join(";"))
            };
            let files = if step.files.is_empty() {
                String::new()
            } else {
                format!(" [{}]", step.files.join(", "))
            };
            out.push((
                format!("{}{}{}", step.name, files, iface_note),
                step.result.clone(),
            ));
        }
    }
    out
}

/// 依赖主声明的上游文件路径（去重、保持 plan 顺序）。供集成主预置骨架使用。
fn upstream_files_for(
    mains: &[TeamMain],
    step_ranges: &[Vec<usize>],
    plan: &[TaskStep],
    main_index: usize,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(main) = mains.get(main_index) else {
        return out;
    };
    for &dep in &main.after {
        let Some(range) = step_ranges.get(dep) else {
            continue;
        };
        for &si in range {
            let Some(step) = plan.get(si) else {
                continue;
            };
            for file in &step.files {
                if !out.iter().any(|existing| existing == file) {
                    out.push(file.clone());
                }
            }
        }
    }
    out
}

/// 集成主启动前预置单文件最终入口（SKY-8 全题复跑兜底；见 `assemble_pre_seed`）。
///
/// 只在「单文件目标 + 有上游分片 + 本主声明了 HTML 入口」时触发；已有入口不覆盖。
/// 返回给目标文本的说明（也作为 AgentStatus 事件透出）；预置失败返回 None，模型
/// 仍可用 `fs_inline_html` 自行装配。
fn pre_seed_single_file(
    ctx: &TaskRunContext,
    scope: Option<&Arc<crate::models::types::ExecScope>>,
    declared_files: &[String],
    upstream_files: &[String],
) -> Option<String> {
    if !single_file_requested(&ctx.goal) || upstream_files.is_empty() {
        return None;
    }
    // 取**最后一个** HTML：单文件目标里 `src/shell.html` 之类模板可能先声明，
    // 最终入口按 plan 顺序在后（与 `required_from_plan_for_goal` 同口径）。
    let html = declared_files
        .iter()
        .rfind(|file| {
            let lower = file.to_ascii_lowercase();
            lower.ends_with(".html") || lower.ends_with(".htm")
        })
        .cloned()?;
    // 必须用本主**派生后的 scope**（带 owner/声明文件），不能用 ctx 的 base scope：
    // 否则 reserve_write 会因「声明归属 ≠ 空 owner」拒绝预置（全题复跑前的实测）。
    let scope = scope?;
    let workspace = scope.workspace();
    let out_path = workspace.join(&html);
    if out_path.is_file() {
        return Some(format!(
            "引擎检测到最终入口 {html} 已存在:请先 fs_read 核对,再用 fs_edit 或 fs_inline_html 补全入口调用;不要重写整份内联。"
        ));
    }
    let include_three = ctx.goal.to_ascii_lowercase().contains("three");
    let (text, count) = crate::tools::agent_tools_fs_inline::assemble_pre_seed(
        workspace,
        &html,
        upstream_files,
        "Kedai 单文件交付（引擎预置骨架）",
        include_three,
    )
    .ok()?;
    if let Err(reason) = scope.reserve_write(&out_path) {
        tracing::warn!(
            path = html.as_str(),
            reason = reason.as_str(),
            "预置骨架被所有权拒绝"
        );
        return None;
    }
    if let Err(error) = crate::utils::fs_atomic::write_atomic(&out_path, text.as_bytes()) {
        tracing::warn!(path = html.as_str(), error = %error, "预置骨架写入失败");
        return None;
    }
    if let Ok(stamp) = crate::models::types::ExecScope::stamp_of(&out_path) {
        scope.note_read(&out_path, stamp);
    }
    Some(format!(
        "引擎已在工作区预置最终入口 {html}({} 字节,内联 {count} 个上游分片{}):请先 fs_read 核对,再用 fs_edit 或 fs_inline_html 补全入口调用;不要重写整份内联。",
        text.len(),
        if include_three {
            ",已带 Three.js CDN"
        } else {
            ""
        }
    ))
}

/// 解析审计输出:{"通过":bool,"打回":[{"main":1-based 序号,"step":主内 1-based 子目标
/// 序号(可选),"instruction":"..."}],"结论":"..."}。
/// 解析失败/截断一律按**未通过**兜底(见 AuditVerdict 文档;实跑问题 2 修复);
/// 打回序号越界的条目丢弃(防 LLM 幻觉引用不存在的主 agent);同一定位(主 + 子目标)
/// 重复打回只保留首条(防同一虚拟 session 在补做轮被并发 spawn 两次)。
fn parse_audit(text: &str, mains_count: usize) -> AuditVerdict {
    let t = text.trim();
    let stripped = t
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let candidate = match (stripped.find('{'), stripped.rfind('}')) {
        (Some(lo), Some(hi)) if lo < hi => &stripped[lo..=hi],
        _ => stripped,
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(candidate) else {
        // 解析失败(含推理模型烧光预算的腰斩 JSON):审计质量闸门读不出明确通过,
        // 按未通过处理;结论段给干净说明——半截 JSON 原样进 result「## 审计结论」
        // 卡既难看又误导;原文记 warn 字段备查
        tracing::warn!(
            head = stripped.chars().take(120).collect::<String>(),
            "team 审计输出无法解析,按未通过兜底"
        );
        let conclusion = if stripped.starts_with('{') {
            "(审计输出不完整,无法确认通过)".to_string()
        } else {
            t.to_string()
        };
        return AuditVerdict {
            pass: false,
            kickbacks: Vec::new(),
            conclusion,
        };
    };
    let pass = v.get("通过").and_then(|b| b.as_bool()).unwrap_or(false);
    let conclusion = v
        .get("结论")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        // JSON 合法但缺「结论」:回填固定文案,不把裸 JSON(整段原文是 `{"通过":…}`)
        // 灌进 result 的「## 审计结论」卡——那既难看又误导(实跑问题 2 连带给修复)
        .unwrap_or_else(|| "(审计未给出结论)".to_string());
    let mut kickbacks: Vec<Kickback> = Vec::new();
    if let Some(arr) = v.get("打回").and_then(|a| a.as_array()) {
        for item in arr {
            let main = item.get("main").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
            // step 为可选的主内 1-based 子目标序号;缺省 = 整主补做(兼容旧输出)
            let step = item
                .get("step")
                .and_then(|n| n.as_u64())
                .map(|n| n as usize);
            let instruction = item
                .get("instruction")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if main >= 1 && main <= mains_count && !instruction.is_empty() {
                let idx = main - 1;
                // step 越界(0)视为未指明 → 整主补做,不错失问题上报;先归一到 0-based
                // 再做去重比较(否则 1-based 与已存 0-based 不一致,去重失效)
                let step0 = step.filter(|s| *s >= 1).map(|s| s - 1);
                // 同一定位(主 + 子目标)重复打回只保留首条(补做轮每定位一个 spawn)
                if kickbacks.iter().any(|k| k.main == idx && k.step == step0) {
                    tracing::warn!(main = main, "team 审计打回条目与既有条目同定位,已保留首条");
                    continue;
                }
                kickbacks.push(Kickback {
                    main: idx,
                    step: step0,
                    instruction,
                });
            } else {
                tracing::warn!(
                    main = main,
                    "team 审计打回条目无效(序号越界或指令为空),已丢弃"
                );
            }
        }
    }
    AuditVerdict {
        pass,
        kickbacks,
        conclusion,
    }
}

/// 截断自愈预算决策(纯函数):仅 finish_reason=length 触发,预算翻倍、不低于
/// `HEAL_BUDGET_FLOOR` 并封顶。对齐 solo/custom 逐步执行的自愈语义
///(2026-09-03 实测:team 审计/终审/汇总直调 generate_text 无自愈,推理模型
/// reasoning 烧光 1024 预算产出腰斩 JSON;2026-09-15 实录审计 10000 预算被
/// reasoning 吃掉 9894、正文仅 178 字符)。
/// 算法与触发条件收敛在 `utils::retry`(2026-09-13 批次 4.1 四路合一;
/// 2026-09-15 引入下限);本路径单次重发。
fn trunc_heal_budget(finish_reason: Option<&str>, max_tokens: u32) -> Option<u32> {
    if finish_reason != Some("length") {
        return None;
    }
    crate::utils::retry::heal_budget_with_floor(
        max_tokens,
        TEAM_RETRY_MAX_TOKENS_CAP,
        crate::utils::retry::HEAL_BUDGET_FLOOR,
    )
}

/// 审计/终审/汇总的实际输出预算:在用户设置之上保证 `TEAM_STRUCTURED_MIN_TOKENS`
/// 下限(结构化 JSON 与长文本汇总在小预算下必被 reasoning 挤断),并受封顶约束。
fn structured_budget(setting: u32) -> u32 {
    // 下限与封顶都是常量,且 MIN <= CAP,故 clamp 不会 panic(见两常量定义处)
    setting.clamp(TEAM_STRUCTURED_MIN_TOKENS, TEAM_RETRY_MAX_TOKENS_CAP)
}

/// 推理感知的截断自愈预算(纯函数,2026-09-15):在 `trunc_heal_budget` 之上,
/// 若该轮推理已挤占预算(有 reasoning 观测),按「已消耗 + 原预算」给足,让正文有
/// 与原预算等宽的空间,避免翻倍后仍被推理吃掉、再烧一个完整往返。
///
/// 只在这一形态加强:无 reasoning 观测(非思考模型)或推理未挤占时,维持既有翻倍语义。
fn trunc_heal_budget_reasoning_aware(
    finish_reason: Option<&str>,
    max_tokens: u32,
    completion_tokens: i64,
    reasoning_tokens: i64,
) -> Option<u32> {
    let base = trunc_heal_budget(finish_reason, max_tokens)?;
    // 无推理观测(0 = 非思考模型或上游未下发)时不加强,保持既有语义
    if reasoning_tokens <= 0 || completion_tokens <= 0 {
        return Some(base);
    }
    // reasoning 与正文共用 completion 预算,推理占满时正文只能腰斩;
    // 目标 = 已消耗 + 原预算(正文至少还有与原预算等宽的空间)
    let target = (completion_tokens.max(0) as u32)
        .saturating_add(max_tokens)
        .min(TEAM_RETRY_MAX_TOKENS_CAP);
    Some(target.max(base))
}

/// 审计/终审/汇总共用的纯生成出口:先按原预算调 generate_text,finish=length
/// 时按「翻倍(不低于下限)+ 推理感知」重发一次(截断自愈)。两次调用均经
/// generate_text 落 task_llm_calls,调用情况面板可见「截断 → 提高预算重发」
/// 完整链路;token 用量两次都入 total,record_usage 两次都落(与 solo 自愈留痕同口径)。
#[allow(clippy::too_many_arguments)]
async fn generate_text_healed(
    svc: &Arc<dyn TaskBackend>,
    task_id: &str,
    phase: &str,
    messages: Vec<LlmMessage>,
    max_tokens: u32,
    temperature: f64,
    top_p: f64,
    cancel: &watch::Receiver<bool>,
    total: &mut TokenUsage,
) -> Result<TaskGenOutput, String> {
    let mut out = svc
        .generate_text(
            task_id,
            phase,
            None,
            messages.clone(),
            Vec::new(),
            max_tokens,
            temperature,
            top_p,
            None, // team 模式无节点级连接
            cancel.clone(),
        )
        .await?;
    svc.record_usage(task_id, phase, None, &out);
    total.prompt_tokens += out.prompt_tokens;
    total.completion_tokens += out.completion_tokens;
    total.total_tokens += out.prompt_tokens + out.completion_tokens;
    let Some(retry_budget) = trunc_heal_budget_reasoning_aware(
        out.finish_reason.as_deref(),
        max_tokens,
        out.completion_tokens,
        out.reasoning_tokens,
    ) else {
        return Ok(out);
    };
    tracing::warn!(
        phase = phase,
        max_tokens = max_tokens,
        reasoning_tokens = out.reasoning_tokens,
        completion_tokens = out.completion_tokens,
        retry_max_tokens = retry_budget,
        "team 纯生成调用截断,提高输出上限重发"
    );
    svc.emit_event(
        TaskEventKind::AgentStatus,
        task_id,
        None,
        None,
        Some(format!(
            "(截断自愈){phase} 输出截断,输出上限提高至 {retry_budget} 重发"
        )),
    );
    out = svc
        .generate_text(
            task_id,
            phase,
            None,
            messages,
            Vec::new(),
            retry_budget,
            temperature,
            top_p,
            None, // team 模式无节点级连接
            cancel.clone(),
        )
        .await?;
    svc.record_usage(task_id, phase, None, &out);
    total.prompt_tokens += out.prompt_tokens;
    total.completion_tokens += out.completion_tokens;
    total.total_tokens += out.prompt_tokens + out.completion_tokens;
    Ok(out)
}

/// team 执行器:任务后端(规划/审计/汇总纯生成出口 + 落库)+ 聊天引擎(主 agent 工具循环)。
pub(crate) struct TeamExecutor {
    svc: Arc<dyn TaskBackend>,
    engine: Arc<AgentEngine>,
}

impl TeamExecutor {
    pub(crate) fn new(svc: Arc<dyn TaskBackend>, engine: Arc<AgentEngine>) -> Self {
        TeamExecutor { svc, engine }
    }

    /// team 规划:TEAM_PLANNER_PROMPT + 世界书背景(untrusted 包裹),解析失败按
    /// finish_reason 分级重试(截断/空输出翻倍预算,最多 TEAM_PLAN_MAX_ATTEMPTS 次)。
    /// 调用追踪经 generate_text 统一出口落 task_llm_calls(phase=planner)。
    async fn plan_team(
        &self,
        ctx: &TaskRunContext,
    ) -> Result<(Vec<TeamMain>, TaskGenOutput), String> {
        let mut sys = String::from(TEAM_PLANNER_PROMPT);
        // 执行阶段能力段(D1):各主 agent 走 run_agent_loop(有工具),工具面与
        // solo/multi 同源编译(单一出处 task_engine::tool_policy::compile)——
        // 让分工拓扑不再规划出执行者做不到的子目标(如没有工具却要求产出文件)。
        let allowed = super::tool_policy::compile(
            &ctx.settings.task_tool_policy,
            &ctx.settings.task_tool_allowlist,
            &self.engine.tool_registry(),
            ctx.scope.is_some(),
            // 包专属工具随编码能力包开关(口径同 solo.rs)
            ctx.settings.task_coding_bundle_enabled,
            // 视觉三件随生效连接的「视觉输入」能力位(口径单一出处)
            crate::services::settings_service::vision_enabled(&ctx.settings),
            // 截图工具随「视觉与截图」总开关(默认关)
            ctx.settings.vision_screenshot_enabled,
        )
        .allowed;
        sys.push_str(&format!(
            "\n\n{}",
            crate::services::task_core::prompt_consts::capability_note(
                crate::services::task_core::prompt_consts::StepCapability::ToolLoop,
                &allowed,
                ctx.scope.as_deref().map(|s| s.workspace()),
            )
        ));
        let world = self.svc.world_context(ctx.character_id.as_deref());
        if !world.is_empty() {
            sys.push_str(&format!("\n\n{}", untrusted_boundary("world_book", &world)));
        }
        // SKY-4:单文件目标在规划期就把约束说清——分片只作中间产物,必须有唯一
        // 子目标负责自包含总装;否则规划器可能把最终入口拆成多个外链本地文件。
        let mut user_goal = ctx.goal.clone();
        if single_file_requested(&ctx.goal) {
            user_goal.push_str("\n\n");
            user_goal.push_str(TEAM_SINGLE_FILE_PLANNER_NOTE);
        }
        let messages = vec![
            LlmMessage::plain("system", &sys),
            LlmMessage::plain("user", &user_goal),
        ];
        let mut max_tokens = TEAM_PLAN_INITIAL_MAX_TOKENS;
        let mut last_err = String::from("规划器未产出有效拓扑");
        for attempt in 1..=TEAM_PLAN_MAX_ATTEMPTS {
            if attempt > 1 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if *ctx.cancel.borrow() {
                    return Err("任务已停止".into());
                }
            }
            let out = self
                .svc
                .generate_text(
                    &ctx.task_id,
                    "planner",
                    None,
                    messages.clone(),
                    Vec::new(),
                    max_tokens,
                    TEAM_JSON_TEMPERATURE,
                    ctx.settings.default_top_p,
                    None, // team 模式无节点级连接
                    ctx.cancel.clone(),
                )
                .await?;
            match parse_team_topology(&out.text) {
                Ok(mains) => return Ok((mains, out)),
                Err(e) => last_err = e,
            }
            // 失败 attempt 同步入账 usage(口径同 plan_task_retry;2026-09-10 实测修复)
            self.svc.record_usage(&ctx.task_id, "planner", None, &out);
            let reason = out.finish_reason.as_deref().unwrap_or("");
            tracing::warn!(
                attempt = attempt,
                finish_reason = reason,
                error = last_err.clone(),
                "team 规划输出解析失败,准备重试"
            );
            if reason == "length" || out.text.trim().is_empty() {
                // 与 trunc_heal_budget 同源(批次 4.1 收敛后的补充):翻倍+封顶,达上限保持原值
                max_tokens =
                    crate::utils::retry::doubled_heal_budget(max_tokens, TEAM_RETRY_MAX_TOKENS_CAP)
                        .unwrap_or(max_tokens);
            }
        }
        Err(format!(
            "{last_err}(已重试 {} 次)",
            TEAM_PLAN_MAX_ATTEMPTS - 1
        ))
    }

    /// 一个主 agent 的完整执行:按子目标逐个独立调用 run_agent_loop(每子目标一次
    /// agent 调用,同一主的多个子目标复用同一虚拟 session task:{id}:main:{n} 保持
    /// 人设一致)。返回 (主序号 0-based, 各子目标结果[全局步骤下标, 子目标名, 结果]),
    /// 由调用方逐个回写 plan 步骤并聚合进审计/汇总输入。
    #[allow(clippy::too_many_arguments)]
    async fn run_main(
        &self,
        ctx: &TaskRunContext,
        main_index: usize,
        main: &TeamMain,
        step_idxs: &[usize],
        extra_instruction: Option<&str>,
        goal_filter: Option<&[usize]>,
        prior_seed: Vec<(String, String)>,
        upstream_seed: Vec<(String, String)>,
        // SKY-8:依赖主声明的上游文件路径(供集成主启动前预置单文件骨架)。
        upstream_files: Vec<String>,
        deadline_reason: &'static str,
        // SKY-7:是否豁免两个计数守卫。判定由调用方按「依赖 DAG 汇点」算出
        // (见 `is_integration_main`);不要在本函数里按 after 非空自行判定。
        integration_main: bool,
        ownership: Arc<WriteOwnership>,
    ) -> (usize, Vec<(usize, String, Result<AgentLoopOutput, String>)>) {
        let n = main_index + 1;
        let session_id = format!("task:{}:main:{}", ctx.task_id, n);
        // TMR-4:该主自己的负责文件(并集)与带 owner 的 scope。声明只用于所有权
        // 拦截与提示词;同主多个子目标共用一份声明,避免「子目标 A 写不了自己
        // 在子目标 B 里声明的总装文件」这种主内摩擦。
        let declared_files = declared_files_of(main);
        let scope = ctx.scope.as_ref().map(|base| {
            base.with_write_ownership(ownership, format!("主 Agent-{n}"), declared_files.clone())
        });
        // SKY-6/SKY-7:集成/总装主(依赖汇点)先读完全部分片再写大文件是合法前置,
        // 不能按「探索空转」截断;只关只读计数与无进展计数,语义熔断/步骤墙钟/
        // 团队总预算仍是上限。判定收窄为依赖 DAG 汇点:中段生产者(还有下游主)
        // 不再豁免(2026-10-24 全题复跑:主 2 有 after 无下游豁免后 342 轮零写盘)。
        let mut results = Vec::with_capacity(main.goals.len());
        // 本主已完成子目标的产出:(子目标名, 产出) —— 注入后续子目标,让同一主内各
        // 子目标在彼此成果上衔接,而非各自重造一整份交付物(实跑问题 2 结构性根因)。
        // 首轮为空、循环内自然累积;补做轮因跳过未点名子目标(见下方 continue),必须由
        // 调用方经 prior_seed 显式种入该主既有产出,否则补做子目标既看不到同主其他成果,
        // 也看不到自己被替换的旧版本,版本冲突会在打回场景原样复现。
        let mut prior: Vec<(String, String)> = prior_seed;
        // SKY-8:集成主启动前用上游分片预置最终入口。全题复跑里总装主把 24.5 分钟
        // 花在读分片/写扫描脚本上，最后只交 `src/shell.html`（12 处 @@），
        // `index.html` 从未落盘；预置让「入口存在」不再依赖模型的长尾行为。
        // 已有入口时不覆盖（补做轮保留模型自己的版本），只把「核对/补全」写进目标。
        let pre_seed_note = if integration_main {
            pre_seed_single_file(ctx, scope.as_ref(), &declared_files, &upstream_files)
        } else {
            None
        };
        if let Some(note) = &pre_seed_note {
            self.svc.emit_event(
                TaskEventKind::AgentStatus,
                &ctx.task_id,
                None,
                None,
                Some(note.clone()),
            );
        }
        for (k, g) in main.goals.iter().enumerate() {
            // 子目标粒度补做:仅重跑被点名的子目标(其余保留首轮产出);
            // 被跳过的子目标仍要入 results(带既有产出)以便调用方统一回写/重建
            if let Some(only) = goal_filter {
                if !only.contains(&k) {
                    continue;
                }
            }
            // 取消(含上一子目标被中断后):剩余子目标统一标记中断,保证每步都有终态
            if *ctx.cancel.borrow() {
                for (k2, g2) in main.goals.iter().enumerate().skip(k) {
                    if goal_filter.is_some_and(|only| !only.contains(&k2)) {
                        continue;
                    }
                    results.push((step_idxs[k2], g2.name.clone(), Err("任务已停止".into())));
                }
                break;
            }
            // 任务总预算用尽(PRODCAP-2):剩余子目标不再启动,统一标错(与取消同款
            // 「每步都有终态」纪律);已产出的子目标结果照常聚合。
            if super::deadline_exhausted(ctx.deadline) {
                for (k2, g2) in main.goals.iter().enumerate().skip(k) {
                    if goal_filter.is_some_and(|only| !only.contains(&k2)) {
                        continue;
                    }
                    results.push((step_idxs[k2], g2.name.clone(), Err(deadline_reason.into())));
                }
                break;
            }
            let mut goal = format!(
                "总体目标:\n{}\n\n你是主 Agent-{n},负责分工「{}」。当前子目标(第 {}/{} 个):{}\n{}\n",
                ctx.goal,
                main.name,
                k + 1,
                main.goals.len(),
                g.name,
                g.goal
            );
            // TMR-4:把「你负责哪些文件」写进目标文本,让模型在动手前就知道边界;
            // 运行期还有一道硬拦截(fs_* 调用被拒绝并回灌同一份清单)。
            if !declared_files.is_empty() {
                goal.push_str(
                    "\n你负责的文件(工作区相对路径;只能写这些,其他文件由别的主 agent 负责):\n",
                );
                for path in &declared_files {
                    goal.push_str(&format!("- {path}\n"));
                }
                goal.push_str("若必须改动未列出的文件,请在结果正文说明原因与建议,不要直接覆盖。\n");
            }
            // 同主前序产出:仅供衔接与保持一致,明确禁止整篇复述或另造版本
            if !prior.is_empty() {
                goal.push_str(
                    "\n本主此前已完成子目标的产出(仅供衔接与保持一致;不要在本次产出中整篇复述或另起一版):\n",
                );
                for (name, text) in &prior {
                    let brief: String = text.trim().chars().take(1200).collect();
                    goal.push_str(&format!("- 「{name}」:{brief}\n"));
                }
            }
            // TMR3-3:依赖主已完成——把其产出摘要带给本主,并提示文件已落盘。
            // 这是「总装主等 parts 主」的可见依据;正文摘要只作衔接,交付以文件为准。
            if !upstream_seed.is_empty() {
                goal.push_str(
                    "\n依赖的上游主 agent 产出(相关文件已落盘;请用 fs_read 核对后再总装/衔接,不要整篇复述):\n",
                );
                for (name, text) in upstream_seed.iter().take(8) {
                    let brief: String = text.trim().chars().take(1200).collect();
                    goal.push_str(&format!("- 「{name}」:{brief}\n"));
                }
                // SKY-1:总装/衔接形态——签名以文件为准,写后必须实际运行自检
                goal.push('\n');
                goal.push_str(TEAM_ASSEMBLY_CONTRACT_NOTE);
                goal.push('\n');
            }
            // SKY-4/SKY-1 单文件装配纪律:目标要求唯一单文件、且本主声明了 HTML 入口时,
            // 把「必须自包含、不得外链本地文件、不得残留槽位」写成装配边界;引擎在交付
            // 门槛对本地外链与未导出导入做确定性判定(见 wiring 模块)。
            if single_file_requested(&ctx.goal) && declares_html(&declared_files) {
                goal.push('\n');
                goal.push_str(TEAM_SINGLE_FILE_ASSEMBLY_NOTE);
                goal.push('\n');
            }
            if let Some(note) = &pre_seed_note {
                goal.push('\n');
                goal.push_str(note);
                goal.push('\n');
            }
            if let Some(extra) = extra_instruction {
                goal.push_str(&format!("\n审计补做指令:\n{extra}\n"));
            }
            goal.push_str("请完成该子目标并直接产出最终结果。");
            let call = AgentLoopCall {
                task_id: ctx.task_id.clone(),
                session_id: session_id.clone(),
                goal,
                settings: ctx.settings.clone(),
                executor_id: ctx.executor_id.clone(),
                character_id: ctx.character_id.clone(),
                phase: "agent",
                step_index: Some(step_idxs[k]),
                label: format!("主 agent {n} 子目标 {}", k + 1),
                connection_id: ctx.connection_id.clone(),
                // 工作区作用域(编码通道批次 1):各主共享同一工作区;
                // TMR-4 起再带本主的写入者身份与负责文件(所有权拦截用)。
                scope: scope.clone(),
                // 任务级总预算(PRODCAP-2):各主 agent 的循环同受截止期约束
                deadline: ctx.deadline,
                // SKY-6:集成主(有上游依赖)读全部分片是合法前置,两个计数闸门都关;
                // 非集成主维持默认,继续保护「只读探索 / 只说不写」。
                readonly_guard: !integration_main,
                no_progress_guard: !integration_main,
                // SKY-8:集成主读满 8 次 fs_read 后提醒一次「先写入口骨架」;
                // 中段生产者/普通主走既有的只读与无进展守卫,不需要这条软提醒。
                shard_read_nudge_after: integration_main.then_some(super::TASK_SHARD_READ_NUDGE),
            };
            let result = run_agent_loop(
                self.svc.clone(),
                self.engine.clone(),
                call,
                ctx.cancel.clone(),
            )
            .await;
            // TMR3-L7:成功子目标的 usage 在**完成即入账**,不等 run_main 返回后由
            // 并行收集方统一记。这样主 agent 若在后续子目标 panic(JoinError),
            // 已完成部分的 token 不会随任务一起消失;汇入 `total` 仍由收集方按返回值做。
            if let Ok(out) = &result {
                self.svc.record_usage(
                    &ctx.task_id,
                    "agent",
                    Some(step_idxs[k]),
                    &usage_as_output(&out.usage),
                );
            }
            // 成功产出进 prior 供后续子目标衔接(失败不注入,避免把错误文本当参考)
            if let Ok(out) = &result {
                prior.push((g.name.clone(), out.text.clone()));
            }
            results.push((step_idxs[k], g.name.clone(), result));
        }
        (main_index, results)
    }

    async fn run_inner(
        &self,
        mut ctx: TaskRunContext,
    ) -> Result<(TaskTerminal, TokenUsage), String> {
        let svc = &self.svc;
        let mut total = TokenUsage::default();

        // ===== a. 规划器:自动拓扑(reset_task 已置 planning;规划阶段归执行器所有) =====
        svc.set_status(&ctx.task_id, TaskStatus::Planning);
        let (mains, plan_out) = self.plan_team(&ctx).await?;
        // TMR-4:声明表只构造一次,首轮与补做轮共用(认领记录也随之延续)。
        let ownership = build_write_ownership(&mains);
        svc.record_usage(&ctx.task_id, "planner", None, &plan_out);
        total.prompt_tokens += plan_out.prompt_tokens;
        total.completion_tokens += plan_out.completion_tokens;
        total.total_tokens += plan_out.prompt_tokens + plan_out.completion_tokens;
        if *ctx.cancel.borrow() {
            return Err("任务已停止".into());
        }

        // TMR3-4:团队总预算从**规划完成后**起算(规划自身有 3×300s 的重试上限);
        // 与任务总预算取小,团队预算更早时标记绑定,用于区分收尾文案。
        let (deadline, team_binding) =
            super::team_deadline(ctx.deadline, ctx.settings.task_team_budget_secs);
        ctx.deadline = deadline;
        // SKY-7:主执行池 = 截止期 - 审计/汇总预留。主 agent 只能分到池内份额,
        // 审计/终审/汇总继续用 ctx.deadline——主执行再也吃不掉质量闸门的最后窗口。
        let main_pool = main_pool_deadline(deadline);
        let share_reason = if team_binding {
            "团队预算份额用尽(未执行)"
        } else {
            "任务预算份额用尽(未执行)"
        };
        // TMR3-L4:默认 1200s 是**安全阀**而不是目标值,但它是 TMR3 之后的新默认行为
        // ——团队任务不再无限跑。开工时先透出一条预算事实,让用户在看到「团队总预算
        // 用尽」之前就知道有这条闸门、以及去哪里调(设置页;0 = 关闭)。
        // SKY-7:同时说明按主/步分配与审计预留,避免用户误以为每主都能用满总预算。
        if team_binding {
            let reserve_secs = deadline
                .map(|d| post_exec_reserve(d.saturating_duration_since(Instant::now())).as_secs())
                .unwrap_or(0);
            let mut msg = format!(
                "团队执行预算:{} 秒(规划完成后起算;设置页可调大或设 0 关闭)",
                ctx.settings.task_team_budget_secs
            );
            if reserve_secs > 0 {
                msg.push_str(&format!(";按主/步分配,审计/汇总预留 {reserve_secs} 秒"));
            }
            svc.emit_event(
                TaskEventKind::AgentStatus,
                &ctx.task_id,
                None,
                None,
                Some(msg),
            );
        }
        let deadline_reason = if team_binding {
            "团队总预算用尽(未执行)"
        } else {
            "任务总预算用尽(未执行)"
        };

        // plan 步骤:「【主Agent-N】子目标名」(前端分工卡分组契约);
        // step_ranges[n] = 主 agent n 在 plan 中的步骤下标区间
        let mut plan: Vec<TaskStep> = Vec::new();
        let mut step_ranges: Vec<Vec<usize>> = Vec::new();
        for (i, m) in mains.iter().enumerate() {
            let mut idxs = Vec::new();
            for g in &m.goals {
                idxs.push(plan.len());
                plan.push(TaskStep {
                    name: format!("【主Agent-{}】{}", i + 1, g.name),
                    goal: g.goal.clone(),
                    status: TaskStepStatus::Pending,
                    result: String::new(),
                    node_id: None,
                    files: g.files.clone(),
                });
            }
            step_ranges.push(idxs);
        }
        svc.set_plan(&ctx.task_id, &plan);
        svc.set_status(&ctx.task_id, TaskStatus::Running);

        // ===== b. 各主 agent 并行(JoinSet;独立 run_id/虚拟 session,cancel 同源) =====
        let (mut outputs, cancelled) = self
            .run_mains_parallel(
                &ctx,
                &mains,
                &step_ranges,
                &mut plan,
                &mut total,
                None,
                &ownership,
                deadline_reason,
                // SKY-7:首轮用「全局截止期 - 审计预留」作为主执行池
                main_pool,
                share_reason,
            )
            .await;
        if cancelled {
            return Err("任务已停止".into());
        }
        // 任务总预算用尽(PRODCAP-2):各主 agent 的循环已被截止期收窄的步骤预算收口;
        // 截止期已过则审计/汇总不再发起新调用(总预算的目的就是停止继续开销),
        // 以已完成部分收尾(无产出时 fallback_terminal 自然回落 Failed)。
        if super::deadline_exhausted(ctx.deadline) {
            let msg = if team_binding {
                "团队总预算用尽:审计与汇总未执行,以已完成部分收尾"
            } else {
                "任务总预算用尽:审计与汇总未执行,以已完成部分收尾"
            };
            svc.emit_event(
                TaskEventKind::Status,
                &ctx.task_id,
                None,
                None,
                Some(msg.to_string()),
            );
            return Ok((fallback_terminal(&plan, msg.to_string()), total));
        }
        if outputs.iter().all(|o| o.is_none()) {
            // 全灭时 helper 自然回落 Failed(无 done 步骤可拼),措辞与旧实现一致
            return Ok((
                fallback_terminal(&plan, "全部主 agent 执行失败,团队无产出".into()),
                total,
            ));
        }

        // ===== c. 审计 agent:一致性/质量/覆盖度审查;缺漏打回指定主补做(最多 1 轮) =====
        // SKY-3:审计前按 TaskStep.files 的**全类型**路径 stat 一次,把文件实况写进输入。
        // 审计员自选的 fs_glob 模式(如 "*.js")不能作为存在性判据。
        let manifest = ctx
            .scope
            .as_ref()
            .map(|s| workspace_manifest(s.workspace(), &mains, &ctx.goal))
            .unwrap_or_default();
        let audit_input = build_audit_input(&ctx.goal, &mains, &outputs, &manifest);
        // TMR-2:审计先走「只读工作区工具」通道,让审计员能核对文件真实存在与内容;
        // 工具不可用/循环失败/空输出时回退既有纯文本审计,保证审计整体不会因新能力失效。
        let mut audit_system = String::from(TEAM_AUDIT_PROMPT);
        audit_system.push_str("\n\n");
        audit_system.push_str(team_audit_tool_guidance(ctx.scope.is_some()));
        let audit_out = match super::team_audit::generate_with_readonly_tools(
            svc,
            &self.engine,
            &ctx,
            "audit",
            "审计 agent",
            &audit_system,
            &audit_input,
            super::team_audit::AUDIT_MAX_ROUNDS,
            structured_budget(ctx.settings.default_max_tokens),
            TEAM_JSON_TEMPERATURE,
            &mut total,
        )
        .await
        {
            Ok(o) => o,
            // 取消:保持 Err,由引擎按 ended 收尾(终态兜底从库里的 plan 补写已完成产出)
            Err(e) if *ctx.cancel.borrow() => return Err(e),
            Err(tool_err) => {
                tracing::warn!(
                    error = tool_err.as_str(),
                    "team 审计只读工具通道不可用,回退纯文本审计"
                );
                let audit_messages = vec![
                    LlmMessage::plain("system", &audit_system),
                    LlmMessage::plain("user", &audit_input),
                ];
                // 截断自愈:审计是结构化 JSON 输出,推理模型烧光预算会腰斩 JSON(实测)
                match generate_text_healed(
                    svc,
                    &ctx.task_id,
                    "audit",
                    audit_messages,
                    structured_budget(ctx.settings.default_max_tokens),
                    TEAM_JSON_TEMPERATURE,
                    ctx.settings.default_top_p,
                    &ctx.cancel,
                    &mut total,
                )
                .await
                {
                    Ok(o) => o,
                    Err(e) if *ctx.cancel.borrow() => return Err(e),
                    // 审计调用失败但各主已产出(提交 2 部分成果兜底):不再把成果整体丢掉
                    Err(e) => {
                        return Ok((fallback_terminal(&plan, format!("审计调用失败:{e}")), total))
                    }
                }
            }
        };
        if *ctx.cancel.borrow() {
            return Err("任务已停止".into());
        }
        let verdict = parse_audit(&audit_out.text, mains.len());
        // 进入最终 result「## 审计结论」段的文本:无打回 = 首次审计结论;
        // 有打回 = 补做后的终审结论(修复:旧实现永远挂首次打回原文,任务 done
        // 而 result 结尾仍显示「需要补全」)
        let mut final_conclusion = verdict.conclusion.clone();
        // 终态闸门(实跑问题 2):审计/终审未通过则任务不得判 done。
        // 无打回时即首次审计结论;有打回时以补做后的终审裁定覆盖。
        let mut final_pass = verdict.pass;

        // 打回补做:仅一轮,补做完成后追加终审(只产出结论文本,不再打回)
        if !verdict.pass && !verdict.kickbacks.is_empty() {
            svc.emit_event(
                TaskEventKind::AgentStatus,
                &ctx.task_id,
                None,
                None,
                Some(format!("审计打回 {} 处补做", verdict.kickbacks.len())),
            );
            let (new_outputs, cancelled) = self
                .run_mains_parallel(
                    &ctx,
                    &mains,
                    &step_ranges,
                    &mut plan,
                    &mut total,
                    Some(&verdict.kickbacks),
                    &ownership,
                    deadline_reason,
                    // SKY-7:补做轮不再二次预留(补做优先),但仍在全局截止期内
                    // 按被打回主的子目标数分配,避免一个补做主吃光终审窗口。
                    ctx.deadline,
                    share_reason,
                )
                .await;
            outputs = new_outputs;
            if cancelled {
                return Err("任务已停止".into());
            }
            // TMR3-4:补做轮可能把团队预算用尽;到点后不再发起终审/汇总。
            if super::deadline_exhausted(ctx.deadline) {
                let msg = if team_binding {
                    "团队总预算用尽:终审与汇总未执行,以已完成部分收尾"
                } else {
                    "任务总预算用尽:终审与汇总未执行,以已完成部分收尾"
                };
                svc.emit_event(
                    TaskEventKind::Status,
                    &ctx.task_id,
                    None,
                    None,
                    Some(msg.to_string()),
                );
                return Ok((fallback_terminal(&plan, msg.to_string()), total));
            }
            if outputs.iter().all(|o| o.is_none()) {
                // 补做后全灭:首轮通过的主产出仍可能已在 plan 里(helper 会捞回来),
                // 一步都没有才回落 Failed
                return Ok((
                    fallback_terminal(&plan, "补做后全部主 agent 无产出".into()),
                    total,
                ));
            }

            // SKY-5:补做轮末执行器复查(2026-10-24)。补做是否真的修好了,不再等终审:
            // 先按最终 plan 重算「必需文件缺失 + 内容体检 + 静态接线 + 单文件外链」。
            // 仍有确定性问题时直接跳过终审——任务终态必然是 partial(交付门槛同源),
            // 再花一次 16K+ 的结构化调用没有信息增量。无问题时维持既有终审。
            let recheck_required = super::delivery::required_from_plan_for_goal(&plan, &ctx.goal);
            let recheck_problems: Vec<String> = match ctx.scope.as_ref() {
                Some(scope) => {
                    let ws = scope.workspace();
                    let mut problems: Vec<String> =
                        super::delivery::missing_files(ws, &recheck_required)
                            .into_iter()
                            .map(|path| format!("缺少必需文件:{path}"))
                            .collect();
                    problems.extend(super::delivery::content_issues(
                        ws,
                        &recheck_required,
                        &ctx.goal,
                    ));
                    problems
                }
                None => Vec::new(),
            };
            if !recheck_problems.is_empty() {
                let detail = recheck_problems.join(";");
                svc.emit_event(
                    TaskEventKind::Status,
                    &ctx.task_id,
                    None,
                    None,
                    Some(format!("补做后交付内容复查未通过:{detail}")),
                );
                final_pass = false;
                final_conclusion =
                    format!("补做后交付内容复查未通过:{detail};已跳过终审,任务以 partial 交付。");
            }
            if recheck_problems.is_empty() {
                // 终审:基于补做后的最终产出给出结论文本(空输出兜底回退首次审计结论,
                // 与审计「增强环节异常不沉任务」同口径)。TMR-2:与首审同款只读工具通道,
                // 失败回退纯文本。SKY-3:补做可能改动文件,终审前**重新**枚举工作区实况,
                // 不复用首审时点的清单。
                let review_manifest = ctx
                    .scope
                    .as_ref()
                    .map(|s| workspace_manifest(s.workspace(), &mains, &ctx.goal))
                    .unwrap_or_default();
                let review_input = build_final_review_input(
                    &ctx.goal,
                    &mains,
                    &outputs,
                    &verdict,
                    &review_manifest,
                );
                let mut review_system = String::from(TEAM_FINAL_AUDIT_PROMPT);
                review_system.push_str("\n\n");
                review_system.push_str(team_audit_tool_guidance(ctx.scope.is_some()));
                let review_out = match super::team_audit::generate_with_readonly_tools(
                    svc,
                    &self.engine,
                    &ctx,
                    "final_audit",
                    "终审 agent",
                    &review_system,
                    &review_input,
                    super::team_audit::FINAL_AUDIT_MAX_ROUNDS,
                    structured_budget(ctx.settings.default_max_tokens),
                    TEAM_JSON_TEMPERATURE,
                    &mut total,
                )
                .await
                {
                    Ok(o) => o,
                    Err(e) if *ctx.cancel.borrow() => return Err(e),
                    Err(tool_err) => {
                        tracing::warn!(
                            error = tool_err.as_str(),
                            "team 终审只读工具通道不可用,回退纯文本终审"
                        );
                        let review_messages = vec![
                            LlmMessage::plain("system", &review_system),
                            LlmMessage::plain("user", &review_input),
                        ];
                        match generate_text_healed(
                            svc,
                            &ctx.task_id,
                            "final_audit",
                            review_messages,
                            structured_budget(ctx.settings.default_max_tokens),
                            TEAM_JSON_TEMPERATURE,
                            ctx.settings.default_top_p,
                            &ctx.cancel,
                            &mut total,
                        )
                        .await
                        {
                            Ok(o) => o,
                            Err(e) if *ctx.cancel.borrow() => return Err(e),
                            // 终审调用失败但补做产出已在手(提交 2 部分成果兜底):不再整体丢成果
                            Err(e) => {
                                return Ok((
                                    fallback_terminal(&plan, format!("终审调用失败:{e}")),
                                    total,
                                ))
                            }
                        }
                    }
                };
                if *ctx.cancel.borrow() {
                    return Err("任务已停止".into());
                }
                // 终审:基于补做后的最终产出做裁定(结构化 JSON);解析失败/空输出按未通过
                // 兜底(终审是终态闸门,读不出明确通过就不判 done);结论段空输出时回退首次
                // 审计结论,避免「## 审计结论」段空白
                let review_verdict = parse_audit(&review_out.text, mains.len());
                final_pass = review_verdict.pass;
                if review_out.text.trim().is_empty() {
                    tracing::warn!("team 终审返回空内容,审计结论段回退为首次审计结论");
                } else {
                    final_conclusion = review_verdict.conclusion;
                }
            }
        }

        // ===== d. 升华整合:整合文本 + 审计结论段(前端拆卡契约) =====
        // 汇总输入携带审计结论与已选定版本,并要求「只整合、不另起一版」——旧实现
        // 汇总与审计互不可见,汇总自由再生产一版,导致正文与审计结论不同源
        //(实跑问题 2)。SKY-3:汇总同样带**实时**工作区清单,不复述旧文本。
        let summary_manifest = ctx
            .scope
            .as_ref()
            .map(|s| workspace_manifest(s.workspace(), &mains, &ctx.goal))
            .unwrap_or_default();
        let summary_input = build_summary_input(
            &ctx.goal,
            &mains,
            &outputs,
            &final_conclusion,
            &summary_manifest,
        );
        let mut sum_sys = String::from(SUMMARIZER_PROMPT);
        let world = svc.world_context(ctx.character_id.as_deref());
        if !world.is_empty() {
            sum_sys.push_str(&format!("\n\n{}", untrusted_boundary("world_book", &world)));
        }
        let summary_messages = vec![
            LlmMessage::plain("system", &sum_sys),
            LlmMessage::plain("user", &summary_input),
        ];
        // 空输出重试一次(对齐 legacy 汇总空输出语义;此处简化为同参数单重重试)
        // 截断自愈与 record_usage/total 入账由 generate_text_healed 承担
        let mut summary_out = match generate_text_healed(
            svc,
            &ctx.task_id,
            "summary",
            summary_messages.clone(),
            structured_budget(ctx.settings.default_max_tokens),
            ctx.settings.default_temperature,
            ctx.settings.default_top_p,
            &ctx.cancel,
            &mut total,
        )
        .await
        {
            Ok(o) => o,
            Err(e) if *ctx.cancel.borrow() => return Err(e),
            // 汇总调用失败但各主已产出(提交 2 部分成果兜底):不再整体丢成果
            Err(e) => return Ok((fallback_terminal(&plan, format!("团队汇总失败:{e}")), total)),
        };
        if summary_out.text.trim().is_empty() && !*ctx.cancel.borrow() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            summary_out = match svc
                .generate_text(
                    &ctx.task_id,
                    "summary",
                    None,
                    summary_messages,
                    Vec::new(),
                    structured_budget(ctx.settings.default_max_tokens),
                    ctx.settings.default_temperature,
                    ctx.settings.default_top_p,
                    None, // team 模式无节点级连接
                    ctx.cancel.clone(),
                )
                .await
            {
                Ok(o) => o,
                Err(e) if *ctx.cancel.borrow() => return Err(e),
                Err(e) => {
                    return Ok((fallback_terminal(&plan, format!("团队汇总失败:{e}")), total))
                }
            };
            svc.record_usage(&ctx.task_id, "summary", None, &summary_out);
            total.prompt_tokens += summary_out.prompt_tokens;
            total.completion_tokens += summary_out.completion_tokens;
            total.total_tokens += summary_out.prompt_tokens + summary_out.completion_tokens;
        }
        if summary_out.text.trim().is_empty() {
            // 汇总空产出同样降级(提交 2):各主成果保留
            return Ok((fallback_terminal(&plan, "团队汇总返回空内容".into()), total));
        }

        // ===== e. 收尾:终态失败步骤或审计/终审未通过 → partial;全部通过 → done =====
        // 实跑问题 2:审计不通过(含解析失败兜底未通过、补做后终审仍不过)不得判定
        // done——审计是质量闸门,其结论必须反映到终态;未通过时任务以 partial 交付,
        // 由用户决定是否追加指令继续修。
        // 失败步骤判定以 plan 终态为准(而非累加式 had_error):补做成功的步骤已被覆盖为
        // Done,不应再拖累终态——旧实现 had_error 一旦置位永不复位,导致「失败 → 打回补做
        // 成功 → 终审通过」仍判 partial 且 error 为空、用户无从解释(实跑问题 2)。
        let failed_steps = error_step_labels(&plan);
        let failed = has_error_steps(&plan);
        // SKY-6 第 2 项/SKY-8:未完成(partial)步骤即使审计通过也不得让任务记 done
        // ——步骤没跑完就是没跑完。SKY-8 起 partial 有两个来源:空转守卫截断,以及
        // 预算收尾时声明文件缺失;补做成功的步骤已被覆盖回 done,不会拖累终态。
        let truncated_steps = partial_step_labels(&plan);
        let text = format!(
            "{}\n\n## 审计结论\n{}",
            summary_out.text.trim(),
            final_conclusion
        );
        // partial 原因:失败子目标 > 审计/终审未通过;守卫截断原因**独立追加**
        // (不是二选一——两者可能同时存在,用户需要知道哪些子目标没跑完)。
        // Done 时为 None(complete_mode_run 写空串清掉上一轮残留 error)
        let mut reasons: Vec<String> = Vec::new();
        if failed {
            reasons.push(format!(
                "以下子目标执行失败,成果不完整:{}",
                failed_steps.join("、")
            ));
        } else if !final_pass {
            reasons.push("审计/终审未通过,未达交付标准(详见「审计结论」)".to_string());
        }
        if !truncated_steps.is_empty() {
            reasons.push(format!(
                "以下子目标未完成(守卫截断或预算收尾),成果可能不完整:{};可追加指令继续完成",
                truncated_steps.join("、")
            ));
        }
        let error = (!reasons.is_empty()).then(|| reasons.join("\n"));
        // 有可解释 error(失败子目标 / 审计未通过 / 未完成步骤)即 partial,否则 done
        let status = if error.is_some() {
            TaskStatus::Partial
        } else {
            TaskStatus::Done
        };
        Ok((
            TaskTerminal::Complete {
                result: text,
                status,
                error,
            },
            total,
        ))
    }

    /// 并行跑一批主 agent(首轮 = 全部;补做轮 = 打回指定子集/子目标,目标文本附补做指令)。
    /// 主 agent 内按子目标逐个独立执行,每子目标完成即推进其 plan 步骤
    ///(running→done/error,写库成功才发事件由 set_plan 保证)。
    /// 各主产出**在收尾时从 plan 统一重建**(而非就地拼接本轮 sections):plan 是子目标
    /// 结果的唯一真相源,重建保证补做替换旧版本、未打回主保留原产出、不出现新旧并存
    ///(实跑问题 2)。
    /// 返回 (各主产出, 是否被任务取消中断);某主部分子目标失败时产出保留成功部分
    ///(附失败说明供审计覆盖度判定),全部子目标失败才记 None。
    #[allow(clippy::too_many_arguments)]
    async fn run_mains_parallel(
        &self,
        ctx: &TaskRunContext,
        mains: &[TeamMain],
        step_ranges: &[Vec<usize>],
        plan: &mut [TaskStep],
        total: &mut TokenUsage,
        kickbacks: Option<&[Kickback]>,
        ownership: &Arc<WriteOwnership>,
        // 波间截止期收尾文案:团队总预算与任务总预算要能区分(TMR3-4)
        deadline_reason: &'static str,
        // SKY-7:本轮的预算池截止期(初始执行 = 全局截止期 - 审计预留;
        // 补做轮 = 全局截止期)。None = 无墙钟预算,行为与旧实现一致。
        share_pool: Option<Instant>,
        // SKY-7:被份额收窄的主在启动下一子目标前使用的原因文案
        // (与 `deadline_reason` 区分「团队/任务总预算用尽」与「份额用尽」)。
        share_reason: &'static str,
    ) -> (Vec<Option<String>>, bool) {
        let svc = &self.svc;
        // SKY-8:单文件目标的最终入口(plan 里最后一个 HTML)。中间模板步骤的终态
        // 以它为准,不因模板里的 `@@` 单独判 partial。
        let final_entry: Option<String> = if single_file_requested(&ctx.goal) {
            super::delivery::required_from_plan_for_goal(plan, &ctx.goal)
                .into_iter()
                .next()
                .map(|(path, _)| path)
                .filter(|path| {
                    let lower = path.to_ascii_lowercase();
                    lower.ends_with(".html") || lower.ends_with(".htm")
                })
        } else {
            None
        };
        // 确定本轮要跑的主 agent 子集:(主序号, 补做指令, 限定的子目标下标集)。
        // 子目标下标先按该主实际子目标数过滤:越界/为空 → 退回整主(审计 step 越界
        // 或该主全部被点名时都不至于漏跑或空跑)。
        let batch: Vec<(usize, Option<String>, Option<Vec<usize>>)> = match kickbacks {
            None => (0..mains.len()).map(|i| (i, None, None)).collect(),
            Some(list) => {
                // 同一主的多个打回条目合并为一轮 spawn:step=None 视为整主(不设过滤),
                // 有具体 step 的取并集(指令合并);避免同一虚拟 session 被并发 spawn。
                let mut merged: Vec<(usize, Option<Vec<usize>>, Vec<String>)> = Vec::new();
                for k in list {
                    match merged.iter_mut().find(|(m, _, _)| *m == k.main) {
                        Some((_, steps, ins)) => {
                            match (&mut *steps, k.step) {
                                // 任一条目未指明子目标 = 整主补做(放宽为不过滤)
                                (s @ Some(_), None) => *s = None,
                                (Some(s), Some(step)) => {
                                    if !s.contains(&step) {
                                        s.push(step);
                                    }
                                }
                                (None, _) => {}
                            }
                            ins.push(k.instruction.clone());
                        }
                        None => merged.push((
                            k.main,
                            k.step.map(|s| vec![s]),
                            vec![k.instruction.clone()],
                        )),
                    }
                }
                merged
                    .into_iter()
                    .map(|(i, steps, ins)| {
                        // 过滤越界子目标:全部越界 → 整主(不设过滤),否则只跑命中项
                        let filtered = steps.and_then(|s| {
                            let max = mains[i].goals.len();
                            let kept: Vec<usize> = s.into_iter().filter(|x| *x < max).collect();
                            if kept.is_empty() {
                                None
                            } else {
                                Some(kept)
                            }
                        });
                        (i, Some(ins.join("\n")), filtered)
                    })
                    .collect()
            }
        };
        // 补做轮的 prior 种子在此快照:必须早于下方「置 running」——否则被打回子目标
        // 自身已被置为 Running,会被 before_kickback_prior 的 Done 过滤跳过,补做者
        // 看不到自己的旧版本(实测踩坑:只剩同门未打回子目标的产出)。
        // 按 batch 顺序与 spawn 一一对应;整主/首轮为空 vec(首轮走循环内自然累积)。
        let prior_seeds: Vec<Vec<(String, String)>> = batch
            .iter()
            .map(|(i, _, filter)| before_kickback_prior(step_ranges, plan, *i, filter.as_deref()))
            .collect();
        // SKY-7:每个主本轮**实际执行**的子目标数 = 预算份额的权重。补做轮只算
        // 被点名的子目标(被跳过的步骤不参与分配);首轮 = 该主全部子目标。
        let units_by_pos: Vec<usize> = batch
            .iter()
            .map(|(i, _, filter)| match filter {
                None => mains[*i].goals.len(),
                Some(steps) => steps.len(),
            })
            .collect();
        // TMR3-3:按主级依赖分波。首轮按 `plan_waves`;补做轮依赖已在首轮满足,
        // 保持单波(与 TMR3 之前一致)。无依赖时 waves = 一波,行为逐字节不变。
        let waves: Vec<Vec<usize>> = if kickbacks.is_none() {
            plan_waves(mains)
                .into_iter()
                .map(|wave| {
                    wave.into_iter()
                        .filter_map(|mi| batch.iter().position(|(i, _, _)| *i == mi))
                        .collect::<Vec<usize>>()
                })
                .filter(|w| !w.is_empty())
                .collect()
        } else {
            vec![(0..batch.len()).collect()]
        };
        if waves.len() > 1 {
            let desc = waves
                .iter()
                .enumerate()
                .map(|(n, w)| {
                    let names: Vec<String> = w
                        .iter()
                        .map(|pos| format!("主Agent-{}", batch[*pos].0 + 1))
                        .collect();
                    format!("第{}波:{}", n + 1, names.join(","))
                })
                .collect::<Vec<_>>()
                .join(";");
            svc.emit_event(
                TaskEventKind::Status,
                &ctx.task_id,
                None,
                None,
                Some(format!("团队依赖调度:{desc}")),
            );
        }
        let mut cancelled = false;
        for (wave_no, wave) in waves.iter().enumerate() {
            // 波间取消/截止期:未启动的条目统一标终态,不留 pending/running
            if *ctx.cancel.borrow() || super::deadline_exhausted(ctx.deadline) {
                let reason = if *ctx.cancel.borrow() {
                    cancelled = true;
                    "任务已停止"
                } else {
                    deadline_reason
                };
                for pos in waves[wave_no..].iter().flat_map(|w| w.iter().copied()) {
                    let (i, _, filter) = &batch[pos];
                    for &si in &step_ranges[*i] {
                        let will_run = match filter {
                            None => true,
                            Some(steps) => main_goal_index(step_ranges, *i, si)
                                .is_some_and(|k| steps.contains(&k)),
                        };
                        if will_run {
                            plan[si].status = TaskStepStatus::Error;
                            plan[si].result = reason.to_string();
                        }
                    }
                }
                svc.set_plan(&ctx.task_id, plan);
                break;
            }
            // 步骤置 running 并落库(事件随 set_plan 发射):只置本波真正会跑的步骤——
            // 子目标粒度补做时被跳过的步骤保持原终态,不得留 running(实跑问题 2)。
            for &pos in wave {
                let (i, _, filter) = &batch[pos];
                for &si in &step_ranges[*i] {
                    let will_run = match filter {
                        None => true,
                        Some(steps) => {
                            // filter 存的是主内子目标下标;映射回全局步骤下标比对
                            main_goal_index(step_ranges, *i, si).is_some_and(|k| steps.contains(&k))
                        }
                    };
                    if will_run {
                        plan[si].status = TaskStepStatus::Running;
                    }
                }
            }
            svc.set_plan(&ctx.task_id, plan);

            // SKY-7:按「本波 + 后续未启动波」的实际执行子目标数分配预算池。
            // 前波提前结束的余额会自然回流后续波(每波从当前剩余重新计算);
            // 同一波内各主按步数比例分,主 1 再也不能吃掉后续波的全部预算。
            let remaining_units: usize = waves[wave_no..]
                .iter()
                .flat_map(|w| w.iter())
                .map(|pos| units_by_pos[*pos])
                .sum();
            // pos → 本主本轮的截止期;None = 不设额外闸门(仅在无预算池时)。
            let mut allocations: Vec<Option<Instant>> = vec![None; batch.len()];
            if let Some(pool) = share_pool {
                let now = Instant::now();
                let pool_remaining = pool.saturating_duration_since(now);
                for &pos in wave {
                    let share = fair_share(pool_remaining, units_by_pos[pos], remaining_units);
                    allocations[pos] = Some(pool.min(now.checked_add(share).unwrap_or(pool)));
                }
                // 多主同波或仍有后续波时把分配事实透给用户/证据;单主单波不刷屏。
                let has_future = waves[wave_no + 1..].iter().any(|w| !w.is_empty());
                if wave.len() > 1 || has_future {
                    let parts: Vec<String> = wave
                        .iter()
                        .map(|pos| {
                            format!(
                                "主Agent-{} {} 秒({} 步)",
                                batch[*pos].0 + 1,
                                allocations[*pos]
                                    .map(|d| d.saturating_duration_since(now).as_secs())
                                    .unwrap_or(0),
                                units_by_pos[*pos]
                            )
                        })
                        .collect();
                    svc.emit_event(
                        TaskEventKind::AgentStatus,
                        &ctx.task_id,
                        None,
                        None,
                        Some(format!(
                            "团队预算分配(第{}波):{}(预算池剩余 {} 秒)",
                            wave_no + 1,
                            parts.join(";"),
                            pool_remaining.as_secs()
                        )),
                    );
                }
            }

            let mut join: JoinSet<MainJoinOutput> = JoinSet::new();
            for &pos in wave {
                let (i, extra, goal_filter) = batch[pos].clone();
                // SKY-7:本主的有效截止期(份额优先,无池时回退全局截止期);
                // 份额早于全局时用「份额用尽」文案,否则沿用团队/任务总预算文案。
                let main_deadline = allocations[pos].or(ctx.deadline);
                let main_reason = match (main_deadline, ctx.deadline) {
                    (Some(md), Some(gd)) if md < gd => share_reason,
                    _ => deadline_reason,
                };
                let prior_seed = prior_seeds[pos].clone();
                // 依赖主已完成子目标的产出摘要:让总装主在动手前看到上游成果;
                // 文件本身已落盘,提示模型用 fs_read 核对(不是只凭摘要复述)。
                let upstream_seed = if kickbacks.is_none() {
                    upstream_seed_for(
                        mains,
                        step_ranges,
                        plan,
                        i,
                        ctx.scope.as_ref().map(|s| s.workspace()),
                    )
                } else {
                    Vec::new()
                };
                // SKY-8:依赖主声明的上游文件(补做轮沿用首轮已落盘的产物,同样可取;
                // 这里按 plan 现况重算,不依赖 kickbacks)。
                let upstream_files = upstream_files_for(mains, step_ranges, plan, i);
                let this = Self {
                    svc: self.svc.clone(),
                    engine: self.engine.clone(),
                };
                let ctx2 = TaskRunContext {
                    task_id: ctx.task_id.clone(),
                    token: ctx.token,
                    goal: ctx.goal.clone(),
                    settings: ctx.settings.clone(),
                    executor_id: ctx.executor_id.clone(),
                    character_id: ctx.character_id.clone(),
                    cancel: ctx.cancel.clone(),
                    connection_id: ctx.connection_id.clone(),
                    // 工作区作用域(编码通道批次 1):打回补做轮沿用同一工作区
                    scope: ctx.scope.clone(),
                    // SKY-7:本主的份额截止期(无预算池时回退全局截止期)
                    deadline: main_deadline,
                };
                let main = mains[i].clone();
                // SKY-7:只有依赖 DAG 汇点(总装终点)豁免两个计数守卫
                let integration_main = is_integration_main(mains, i);
                let step_idxs = step_ranges[i].clone();
                let ownership = ownership.clone();
                join.spawn(async move {
                    this.run_main(
                        &ctx2,
                        i,
                        &main,
                        &step_idxs,
                        extra.as_deref(),
                        goal_filter.as_deref(),
                        prior_seed,
                        upstream_seed,
                        upstream_files,
                        main_reason,
                        integration_main,
                        ownership,
                    )
                    .await
                });
            }

            // 本波的主序号留底:JoinError 兜底置 error 时用(spawn 循环已消耗 wave 的引用)
            let wave_batch_idx: Vec<usize> = wave.iter().map(|pos| batch[*pos].0).collect();
            while let Some(res) = join.join_next().await {
                // 主序号不再参与聚合(产出统一从 plan 重建),仅供 JoinError 分支追溯留底
                let (_i, sub_results) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        // JoinError(panic 等):按该主失败处理,不中断其余主;JoinError 不携带
                        // 业务下标,无法反查是哪一主 panic——保守地把本波中仍 running 的
                        // 步骤统一置 error(终态至少 partial,不留永远 running)。
                        // TMR3-L7:该主**已完成**子目标的 usage 已在 run_main 内完成即入账,
                        // 不随 panic 丢失;panic 发生在进行中的调用时,那一次的用量仍不可得
                        // (同 TMR3-L1 的 future drop 边界)。
                        tracing::warn!(error = e.to_string(), "team 主 agent 任务异常终止");
                        fail_batch_running_steps(plan, &wave_batch_idx, step_ranges);
                        svc.set_plan(&ctx.task_id, plan);
                        continue;
                    }
                };
                // 逐子目标回写各自步骤(未在本轮 results 中的子目标保持原状态/产出)
                for (si, _goal_name, result) in sub_results {
                    match result {
                        Ok(out) => {
                            let usage = out.usage;
                            total.prompt_tokens += usage.prompt_tokens;
                            total.completion_tokens += usage.completion_tokens;
                            total.total_tokens += usage.total_tokens;
                            // usage 行已由 run_main 在子目标完成时落库(TMR3-L7);
                            // 这里只汇入返回值,不再重复 record_usage(否则会双计)。
                            // SKY-8:终态由「收尾方式 + 本步声明文件磁盘事实」共同决定
                            // ——守卫截断但文件齐全可记 done,预算收尾但文件缺失必须记
                            // partial;原因写进步骤结果,审计/前端都能看到。
                            let (status, result) = step_outcome(
                                ctx.scope.as_ref().map(|s| s.workspace()),
                                &plan[si],
                                &out.text,
                                out.guard_stopped.as_deref(),
                                out.budget_stopped,
                                final_entry.as_deref(),
                            );
                            plan[si].status = status;
                            plan[si].result = result;
                        }
                        Err(e) => {
                            if *ctx.cancel.borrow() {
                                cancelled = true;
                            }
                            plan[si].status = TaskStepStatus::Error;
                            plan[si].result = e.clone();
                        }
                    }
                }
                svc.set_plan(&ctx.task_id, plan);
            }
        }
        // 统一从 plan 重建各主产出(唯一真相源;补做已替换旧版本,未打回主保留原产出)
        (rebuild_outputs(mains, step_ranges, plan), cancelled)
    }
}

/// 补做轮的 prior 种子:从 plan 取该主既有子目标产出,供补做子目标衔接并保持版本一致。
/// 首轮 prior 为空、由 run_main 循环内自然累积;补做轮会跳过未点名子目标(goal_filter),
/// 循环内累积不到任何东西 —— 必须在此显式种入,否则补做子目标既看不到同主其他成果,
/// 也看不到自己被替换的旧版本,「多版本并存」会在打回场景原样复现(实跑问题 2)。
/// 被打回子目标的旧版本特别标注「旧版本(本次产出将替换它)」,让模型明确是替换而非新增。
/// 仅当本轮确为子目标粒度补做(goal_filter 有值)时种入;整主补做走首轮同构路径。
fn before_kickback_prior(
    step_ranges: &[Vec<usize>],
    plan: &[TaskStep],
    main_index: usize,
    goal_filter: Option<&[usize]>,
) -> Vec<(String, String)> {
    let Some(only) = goal_filter else {
        return Vec::new();
    };
    let Some(range) = step_ranges.get(main_index) else {
        return Vec::new();
    };
    let mut seed = Vec::new();
    for (k, &si) in range.iter().enumerate() {
        let step = &plan[si];
        // 只种入有产出的终态子目标(失败/未跑的没有可衔接内容);守卫截断的步骤
        // 也有产出,补做时要让模型看到旧版本内容。
        if !matches!(step.status, TaskStepStatus::Done | TaskStepStatus::Partial) {
            continue;
        }
        let name = strip_main_prefix(&step.name).to_string();
        if only.contains(&k) {
            seed.push((
                format!("{name}(旧版本,本次产出将替换它)"),
                step.result.clone(),
            ));
        } else {
            seed.push((name, step.result.clone()));
        }
    }
    seed
}

/// 从 plan 重建各主 agent 产出:plan 是子目标结果唯一真相源。每主按子目标顺序拼接
/// 「子目标 N「名」:产出」(N 为主内 1-based 序号)——编号是审计/终审输出 `step`
/// 字段的定位契约:审计提示词要求回填「该主第几个子目标」,输入里显式编号模型才能
/// 数对,避免靠猜序号导致打回重跑错子目标或误退整主(实跑问题 2)。
/// 失败子目标附错误说明供审计判定覆盖度。**该主无任何 Done 子目标时返回 None**
///(全败主不产生产出,维持「全部主失败 → 团队无产出」语义)。
fn rebuild_outputs(
    mains: &[TeamMain],
    step_ranges: &[Vec<usize>],
    plan: &[TaskStep],
) -> Vec<Option<String>> {
    mains
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let mut sections: Vec<String> = Vec::new();
            let mut any_done = false;
            for (k, &si) in step_ranges[i].iter().enumerate() {
                let step = &plan[si];
                let name = strip_main_prefix(&step.name);
                match step.status {
                    // 守卫截断的步骤带已有产出(和 done 一样进审计/汇总),截断标记
                    // 已由执行器写进 result(SKY-6 第 2 项)。
                    TaskStepStatus::Done | TaskStepStatus::Partial => {
                        any_done = true;
                        sections.push(format!("子目标 {}「{name}」:\n{}", k + 1, step.result));
                    }
                    TaskStepStatus::Error => {
                        sections.push(format!(
                            "子目标 {}「{name}」:(执行失败:{})",
                            k + 1,
                            step.result
                        ));
                    }
                    _ => {}
                }
            }
            if any_done {
                Some(sections.join("\n\n"))
            } else {
                None
            }
        })
        .collect()
}

/// 去掉 plan 步骤名的「【主Agent-N】」前缀(前端分工卡分组契约的前缀由执行器添加)
fn strip_main_prefix(name: &str) -> &str {
    match name.find('】') {
        Some(i) => name[i + '】'.len_utf8()..].trim_start(),
        None => name,
    }
}

/// 全局步骤下标 → 该主内子目标下标(反查;不在该主范围内返回 None)
fn main_goal_index(step_ranges: &[Vec<usize>], main: usize, step: usize) -> Option<usize> {
    step_ranges.get(main)?.iter().position(|s| *s == step)
}

/// JoinError(panic 等)兜底:无法从 JoinError 反查是哪一主 panic(不携带业务下标),
/// 保守地把本轮 batch 中仍 running 的步骤统一置 error(终态至少 partial,不留永远
/// running 的步骤)。其余主若仍在跑,其步骤会在完成回写时覆盖为最终态,此处写的只是
/// 瞬时兜底;已终态步骤与 batch 外步骤不动。返回置 error 的步骤数。
/// 终态 partial 判定由收尾处扫描 plan 中残留的 Error 步骤承担(不再单独记 had_error,
/// 避免「首轮失败 → 打回补做成功 → had_error 未复位 → 仍判 partial」的假阴性)。
fn fail_batch_running_steps(
    plan: &mut [TaskStep],
    batch_idx: &[usize],
    step_ranges: &[Vec<usize>],
) -> usize {
    let mut failed = 0usize;
    for &i in batch_idx {
        for &si in &step_ranges[i] {
            if plan[si].status == TaskStepStatus::Running {
                plan[si].status = TaskStepStatus::Error;
                plan[si].result = "主 agent 任务异常终止".into();
                failed += 1;
            }
        }
    }
    failed
}

/// 收尾判定:plan 中是否残留终态 Error 的子目标步骤(补做成功的步骤已被覆盖为 Done,
/// 不再拖累终态)→ 决定任务落 done 还是 partial。
fn has_error_steps(plan: &[TaskStep]) -> bool {
    plan.iter().any(|s| s.status == TaskStepStatus::Error)
}

/// 收集 plan 中终态 Error 的子目标名(带主 agent 序号,便于用户定位是哪个分工),
/// 用于 partial 终态的原因文本。空 = 无失败步骤。
fn error_step_labels(plan: &[TaskStep]) -> Vec<String> {
    plan.iter()
        .filter(|s| s.status == TaskStepStatus::Error)
        .map(step_label)
        .collect()
}

/// 收集被守卫截断(partial)的子目标名,用于 partial 终态的原因文本。
/// 空 = 没有步骤被截断。
fn partial_step_labels(plan: &[TaskStep]) -> Vec<String> {
    plan.iter()
        .filter(|s| s.status == TaskStepStatus::Partial)
        .map(step_label)
        .collect()
}

/// 步骤名的用户可见定位(带「主 Agent-N」前缀时拆出来重排)。
fn step_label(s: &TaskStep) -> String {
    // plan 步骤名前缀「【主Agent-N】」由执行器构造(前端分工卡分组契约),可靠
    match s
        .name
        .strip_prefix("【主Agent-")
        .and_then(|r| r.split('】').next())
        .and_then(|n| n.parse::<usize>().ok())
    {
        Some(n) => format!("主 Agent-{n}「{}」", strip_main_prefix(&s.name)),
        None => strip_main_prefix(&s.name).to_string(),
    }
}

/// 审计/汇总的输入文本:总体目标 + 工作区声明文件实况 + 各主产出
/// (失败主保留错误文本,供覆盖度判定)。
fn build_audit_input(
    goal: &str,
    mains: &[TeamMain],
    outputs: &[Option<String>],
    manifest: &str,
) -> String {
    let mut user = format!("用户目标:\n{goal}\n");
    user.push_str(manifest);
    user.push_str("\n各主 agent 产出:\n");
    for (i, m) in mains.iter().enumerate() {
        let body = match &outputs[i] {
            Some(text) => text.clone(),
            None => "(该主 agent 执行失败,无产出)".to_string(),
        };
        user.push_str(&format!(
            "### 主 Agent-{}「{}」:\n{}\n\n",
            i + 1,
            m.name,
            body
        ));
    }
    user
}

/// 汇总的输入文本:总体目标 + 各主产出 + 审计结论,并要求「只整合已选定版本、
/// 不得另起一版」。旧实现汇总输入不含审计结论,汇总者对审计认定的不一致毫不知情,
/// 自由再生产一版,最终 result 正文与「## 审计结论」段不同源(实跑问题 2)。
fn build_summary_input(
    goal: &str,
    mains: &[TeamMain],
    outputs: &[Option<String>],
    audit_conclusion: &str,
    manifest: &str,
) -> String {
    let mut user = build_audit_input(goal, mains, outputs, manifest);
    user.push_str("审计结论(必须遵守;若指出某主产出为旧版/有缺漏,以补做后的最终版本为准):\n");
    user.push_str(audit_conclusion);
    user.push_str(
        "\n\n请只整合上述各主 agent 的最终产出,输出一份统一的最终成果:\
         不要另起一版、不要改写或替换已通过审计的产出内容、不要新增未被要求的内容。\
         若各主产出存在重复或前后不一致,以审计结论所指的最终版本为准。",
    );
    user
}

/// 终审的输入文本:补做后的各主产出 + 首轮审计意见(结论与打回清单),
/// 供终审员判断首轮指出的问题是否已解决(终审只产出结论文本,不再打回)。
fn build_final_review_input(
    goal: &str,
    mains: &[TeamMain],
    outputs: &[Option<String>],
    verdict: &AuditVerdict,
    manifest: &str,
) -> String {
    let mut user = build_audit_input(goal, mains, outputs, manifest);
    user.push_str("首轮审计结论:\n");
    user.push_str(&verdict.conclusion);
    user.push_str("\n\n首轮打回(均已完成补做,补做产出已替换旧版本):\n");
    for k in &verdict.kickbacks {
        let target = match k.step {
            Some(s) => format!("主 Agent-{} 第 {} 个子目标", k.main + 1, s + 1),
            None => format!("主 Agent-{}", k.main + 1),
        };
        user.push_str(&format!("- {target}:{}\n", k.instruction));
    }
    user
}

impl ModeExecutor for TeamExecutor {
    fn run<'a>(
        &'a self,
        ctx: TaskRunContext,
    ) -> BoxFuture<'a, Result<(TaskTerminal, TokenUsage), String>> {
        Box::pin(async move { self.run_inner(ctx).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::task_engine::wiring::extract_exports;

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
        let (status, text) =
            step_outcome(None, &step, "正文", Some("连续只读空转熔断"), false, None);
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
        let (status, text) =
            step_outcome(Some(&ws), &step, "正文", Some("重复调用熔断"), false, None);
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
        let (status, text) =
            step_outcome(Some(&ws2), &step, "正文", None, true, Some("index.html"));
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
        assert!(parse_team_topology(
            r#"{"mains":[{"name":"","goals":[]},{"name":"甲","goals":[]}]}"#
        )
        .is_err());
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
}
