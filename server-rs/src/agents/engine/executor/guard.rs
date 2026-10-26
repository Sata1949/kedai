// 工具闸门 / 预算与无进展守卫（C-2F-2 自 executor.rs 纯搬运）。
use serde_json::Value;

/// AGENT 模式工具循环:生成 → 有 tool_calls 则逐个执行并回填消息 → 重新生成。
/// 轮次上限:None = 聊天默认 32;Some(0) = 任务模式不限;Some(n>0) = 显式上限。
/// 无 tool_calls 时返回最终正文。每轮 usage 已累加进 total_usage。
/// 截断自愈(问题①):单轮生成被 max_tokens 截断到不可用(空正文 / 半截 tool_call
/// JSON / 连接器流尾 flush 校验报错)时,本轮 max_tokens 翻倍(上限
/// TRUNCATION_HEAL_MAX_TOKENS_CAP)原样重发一次,重发仍失败才透出原结果;
/// 触发自愈的记录经 ExecutorResult.self_heals 透出(任务模式补落 task_llm_calls)。
/// SSE 语义:模型发出调用时由 execute_generation 推送 ToolCall,执行完成后此处推送 ToolResult。
/// 轮次语义(修复原 8 轮 off-by-one):第 N 轮(含 N=上限)生成的工具调用照常执行,
/// 只是执行完后停止再发起新的模型请求——工具调用不会被静默丢弃,卡片不会无终态悬挂。
/// 白名单模式:步骤配置了 tools 白名单时,名单内工具自动放行(不再弹授权框)。
/// agent_session 为 None 表示任务模式(不建影子 agent_sessions 行,
/// 工具授权闸门:替代裸白名单,把「名单内放行」与「名单外如何处理」分开表达。
/// - `whitelist`:None = 非白名单模式(未放行工具走授权等待);
///   Some(list) = 仅名单内工具放行,**空名单 = 拒绝一切**。
///   名单由「本轮下发的工具集」派生,空集即「该节点一个工具都不该被调用」;
///   旧语义「空 = 全量放行」是 fail-open 的越权面(见 `遗留.md` IFW-12 附带发现,
///   2026-09-24 修正)。
/// - `no_ui_authorization`:true = 未放行工具立即拒绝并回灌错误,不进入授权等待。
///   任务模式没有 UI 授权上下文,若走等待会空等 300 秒超时,故必须置 true。
#[derive(Clone, Copy)]
pub(crate) struct ToolGate<'a> {
    pub whitelist: Option<&'a [String]>,
    pub no_ui_authorization: bool,
}

impl<'a> ToolGate<'a> {
    /// 非白名单模式,未放行则等待授权(聊天 agent 路径)
    pub(crate) fn wait() -> Self {
        Self {
            whitelist: None,
            no_ui_authorization: false,
        }
    }

    /// 显式白名单,不等待(任务 custom 步骤 / 子 agent)
    pub(crate) fn listed(whitelist: &'a [String]) -> Self {
        Self {
            whitelist: Some(whitelist),
            no_ui_authorization: true,
        }
    }

    /// 该工具是否被名单放行。
    ///
    /// **空名单不放行任何工具**:名单由「本轮下发的工具集」派生,空集意味着该节点
    /// 一个工具都没下发,此时放行一切与下发意图相反——模型臆造的工具调用会绕过
    /// 策略剔除被执行(fail-open;任务策略 all 时下发集本身非空,不依赖该语义)。
    pub(crate) fn authorizes(&self, name: &str) -> bool {
        self.whitelist
            .is_some_and(|wl| wl.iter().any(|n| n == name))
    }

    /// 该工具是否被闸门硬性排除。仅对「有名单 + 不等待授权」的路径成立(任务模式):
    /// 名单是能力的硬边界,不在名单内的工具必须拒绝——否则文件规则可能因「宽松模式
    /// 写文件放行」而放过被任务策略排除的危险工具(模型幻觉调用即越权)。
    /// 聊天路径(no_ui_authorization=false)不硬性排除:名单外工具走授权等待,与改造前一致。
    /// 空名单同样走本判定(拒绝一切)——这正是 2026-09-24 修正的形态。
    pub(crate) fn excludes(&self, name: &str) -> bool {
        self.no_ui_authorization && self.whitelist.is_some() && !self.authorizes(name)
    }
}

/// 单次生成 token 预算是否已达上限(HB-1,纯函数便于边界单测):
/// 预算 0 = 关闭;口径 = 本 run 内工具循环累计 prompt+completion,「达到」即算超限
/// (等于预算也停,与轮次上限的 `round >= max_rounds` 同口径)。
/// token 预算是否已达(达到即算超限)。pub(super):run_loop 的归档步闸门复用同一判定
/// (预算越线后不再追加自动调用),避免两处各写一份比较逻辑而漂移。
pub(crate) fn budget_reached(used_tokens: i64, budget: u32) -> bool {
    budget > 0 && used_tokens >= budget as i64
}

/// 步骤墙钟预算是否已用尽(D3,纯函数便于边界单测):None = 不设预算(聊天路径恒此);
/// 口径 = 单次 `run_tool_loop` 的墙钟耗时,「达到」即算(与 token 预算/轮次上限同族)。
pub(crate) fn step_budget_reached(
    elapsed: std::time::Duration,
    budget: Option<std::time::Duration>,
) -> bool {
    budget.is_some_and(|b| elapsed >= b)
}

/// 预算收尾时的正文选择(纯函数便于单测):本轮正文非空即用它;本轮只有工具调用
/// (工具型模型的常见形态)则回退到最近一次非空正文——预算的承诺是「带着已有产出收尾」,
/// 回一个空串会被上游判成「返回空内容」,与承诺相反。
/// 两者都空时如实返回空串(不伪造正文,口径同提交 2 的「不伪造成果」)。
pub(crate) fn budget_stop_content(current: String, last_non_empty: &str) -> String {
    if current.trim().is_empty() {
        last_non_empty.to_string()
    } else {
        current
    }
}

/// 工具循环轮次上限解析(2026-10-19 任务模式取消轮次限制):
/// - `None` → 旧调用方/聊天路径缺省 32(行为不变);
/// - `Some(0)` → **不限轮次**(任务模式显式哨兵,不再因读文件/探索耗尽轮次);
/// - `Some(n>0)` → 显式上限(聊天设置、custom 节点级预算)。
///
/// 不限轮次不等于无界:步骤墙钟预算、空闲超时、语义/重复熔断、token 预算、
/// 任务总预算(若开启)与用户取消仍然生效。
pub(crate) fn resolve_max_rounds(configured: Option<u32>) -> Option<usize> {
    match configured {
        Some(0) => None,
        Some(n) => Some(n.max(1) as usize),
        None => Some(32),
    }
}

/// 本次工具调用是否是「成功写入本主声明文件」的候选(P0-2 纯函数)。
///
/// 只认带 `path` 参数的工作区写工具(`fs_write`/`fs_edit`)+ 路径命中
/// `scope.declared_files()`。`bash` 刻意不算:它可写可读,且自检循环正是用
/// `bash` 跑探针;临时文件工具调用也不算——它们不推进交付物。`fs_patch` 的
/// 补丁目标暂不解析(其参数是补丁文本,不是单一路径),因此不会计为进展。
/// 结果是否成功由调用方用 `output` 判定(本函数只看调用意图与目标路径)。
pub(crate) fn is_declared_file_write(name: &str, arguments: &str, declared: &[String]) -> bool {
    if !crate::tools::tool_sets::is_workspace_write_tool(name) || declared.is_empty() {
        return false;
    }
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return false;
    };
    let Some(raw) = args.get("path").and_then(|v| v.as_str()) else {
        return false;
    };
    let Some(key) = crate::models::ownership::normalize_workspace_rel(raw) else {
        return false;
    };
    declared.iter().any(|d| {
        crate::models::ownership::normalize_workspace_rel(d).as_deref() == Some(key.as_str())
    })
}

/// 台账增量里是否出现了「本主声明文件」的改动(TMR3-L6 纯函数)。
///
/// 台账路径是执行侧扫出的工作区相对路径,与声明侧同用 `normalize_workspace_rel`
/// 归一化后再比对;任一非法路径只跳过该行(不让单条脏数据否决整轮进展)。
pub(crate) fn has_declared_change(declared: &[String], changed_paths: &[String]) -> bool {
    if declared.is_empty() {
        return false;
    }
    changed_paths.iter().any(|raw| {
        let Some(key) = crate::models::ownership::normalize_workspace_rel(raw) else {
            return false;
        };
        declared.iter().any(|d| {
            crate::models::ownership::normalize_workspace_rel(d).as_deref() == Some(key.as_str())
        })
    })
}
