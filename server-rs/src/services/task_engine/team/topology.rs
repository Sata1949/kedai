// team 规划与拓扑:解析规划器输出、主级依赖推断、Kahn 分波、集成主判定、负责文件声明。
// 由原 `team.rs` 整段搬来(C-2D,纯搬运)。
use super::{
    TeamExecutor, TeamMain, TEAM_JSON_TEMPERATURE, TEAM_MAX_DECLARED_FILES_PER_GOAL,
    TEAM_MAX_GOALS_GLOBAL, TEAM_MAX_GOALS_PER_MAIN, TEAM_MAX_MAINS, TEAM_PLAN_INITIAL_MAX_TOKENS,
    TEAM_PLAN_MAX_ATTEMPTS, TEAM_RETRY_MAX_TOKENS_CAP,
};
use crate::models::ownership::{normalize_workspace_rel, WriteOwnership};
use crate::models::types::{LlmMessage, TaskStep};
use crate::services::prompt_kit::untrusted_boundary;
use crate::services::task_core::prompt_consts::{
    TEAM_PLANNER_PROMPT, TEAM_SINGLE_FILE_PLANNER_NOTE,
};
use crate::services::task_core::TaskGenOutput;
use crate::services::task_engine::context::TaskRunContext;
use crate::services::task_engine::wiring::single_file_requested;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

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
pub(super) fn build_write_ownership(mains: &[TeamMain]) -> Arc<WriteOwnership> {
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
pub(super) fn declared_files_of(main: &TeamMain) -> Vec<String> {
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
pub(super) fn declares_html(files: &[String]) -> bool {
    files.iter().any(|f| {
        let lower = f.to_ascii_lowercase();
        lower.ends_with(".html") || lower.ends_with(".htm")
    })
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
pub(super) fn parse_team_topology(text: &str) -> Result<Vec<TeamMain>, String> {
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
pub(super) fn infer_dependencies_from_files(mains: &mut [TeamMain]) {
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
pub(super) fn plan_waves(mains: &[TeamMain]) -> Vec<Vec<usize>> {
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
pub(super) fn is_integration_main(mains: &[TeamMain], index: usize) -> bool {
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

impl TeamExecutor {
    /// team 规划:TEAM_PLANNER_PROMPT + 世界书背景(untrusted 包裹),解析失败按
    /// finish_reason 分级重试(截断/空输出翻倍预算,最多 TEAM_PLAN_MAX_ATTEMPTS 次)。
    /// 调用追踪经 generate_text 统一出口落 task_llm_calls(phase=planner)。
    pub(super) async fn plan_team(
        &self,
        ctx: &TaskRunContext,
    ) -> Result<(Vec<TeamMain>, TaskGenOutput), String> {
        let mut sys = String::from(TEAM_PLANNER_PROMPT);
        // 执行阶段能力段(D1):各主 agent 走 run_agent_loop(有工具),工具面与
        // solo/multi 同源编译(单一出处 task_engine::tool_policy::compile)——
        // 让分工拓扑不再规划出执行者做不到的子目标(如没有工具却要求产出文件)。
        let allowed = crate::services::task_engine::tool_policy::compile(
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
}
