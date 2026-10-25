// 工具循环的停止判定(C-2C,自 `executor.rs::run_tool_loop` 抽出)。
//
// 设计要点:
// - **判定与副作用分离**:本模块只维护守卫计数并产出「提醒 / 停止」决策;
//   SSE 事件、消息回灌、状态机迁移仍由循环体执行。
// - **顺序是契约**:7 类停止条件的先后顺序与抽出前逐字一致——token 预算 →
//   步骤墙钟 → 轮次上限 → 连续只读 → 声明文件无进展 → 语义熔断 → 重复调用。
//   预算/轮次属于正常收尾(步骤仍可 done);后 4 类是守卫截断(任务侧降 partial)。
// - **文案是契约**:`LoopStop.message` 是任务侧写进 error 的短原因,
//   `event_title/event_detail` 是 SSE 事件原文;两者都必须保持与抽取前一致。
use std::time::Duration;

use crate::models::types::ToolCallArgs;
use crate::services::task_core::prompt_consts::{NO_PROGRESS_FIRST_NUDGE, READONLY_FIRST_NUDGE};
use crate::utils::loop_guard::DEFAULT_WINDOW;

use super::super::executor::{budget_reached, step_budget_reached};

/// 工具循环的终止类别(C-2C 单一出处)。
///
/// `TokenBudget` / `StepBudget` / `RoundLimit` 是正常收尾:到达闸门时本轮工具已执行完,
/// 带着已有产出结束,任务终态不因此降级。后 4 类是空转守卫截断:同样保留产出,
/// 但任务侧必须把步骤降 `partial`(SKY-6 第 2 项)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoopStopKind {
    TokenBudget,
    StepBudget,
    RoundLimit,
    Readonly,
    NoProgress,
    Semantic,
    RepeatedCall,
}

impl LoopStopKind {
    /// 是否属于「空转守卫」截断(决定任务终态是否降 partial)。
    pub(crate) fn is_guard(self) -> bool {
        matches!(
            self,
            Self::Readonly | Self::NoProgress | Self::Semantic | Self::RepeatedCall
        )
    }

    /// 是否属于预算类闸门(决定 `ExecutorResult.budget_stopped` 与 extra 留痕)。
    pub(crate) fn is_budget(self) -> bool {
        matches!(self, Self::TokenBudget | Self::StepBudget)
    }
}

/// 一次工具循环停止的完整记录。
pub(crate) struct LoopStop {
    pub(crate) kind: LoopStopKind,
    /// 短原因:任务侧写进 `AgentLoopOutput.guard_stopped` / error 的文案。
    pub(crate) message: String,
    /// 面向用户的 SSE 事件标题(与抽出前逐字一致)。
    pub(crate) event_title: &'static str,
    /// 面向用户的 SSE 事件明细(与抽出前逐字一致)。
    pub(crate) event_detail: String,
}

impl LoopStop {
    /// 事件载荷(标题 + 明细),循环体据此发 `step_evt`。
    pub(super) fn event(&self) -> (&'static str, &str) {
        (self.event_title, &self.event_detail)
    }
}

/// 守卫配置(循环开始前从设置/任务参数解析一次,循环内不再读锁)。
pub(super) struct GuardConfig {
    /// 已解析的轮次上限:`None` = 不限(任务模式 `Some(0)` 哨兵)。
    pub(super) max_rounds: Option<usize>,
    /// token 预算;0 = 关闭。
    pub(super) token_budget: u32,
    /// token 预算越线是停止(true)还是只提醒(warn 档)。
    pub(super) stop_on_budget: bool,
    /// 步骤墙钟预算;None = 不设(聊天路径恒 None)。
    pub(super) step_budget: Option<Duration>,
    /// 连续只读阈值;None = 本循环不启用(聊天/子 agent/审计等)。
    pub(super) readonly_limit: Option<usize>,
    /// 声明文件无进展阈值;None = 本循环不启用(无声明文件或显式关闭)。
    pub(super) no_progress_limit: Option<usize>,
    /// 语义熔断窗口(仅用于事件文案里的「最近 N 次」)。
    pub(super) semantic_window: usize,
}

/// 守卫的跨轮状态。
pub(super) struct GuardState {
    config: GuardConfig,
    /// token 预算提醒是否已发过(只提示一次)。
    budget_noticed: bool,
    /// 连续只读工具调用计数(写类/编排类工具出现即清零)。
    readonly_streak: usize,
    /// 本段只读空转是否已提醒过(写类工具出现后重置)。
    readonly_nudged: bool,
    /// 连续「没有成功写入声明文件」的轮数。
    no_progress_rounds: usize,
    /// 本段无进展是否已提醒过(有进展后重置)。
    no_progress_nudged: bool,
}

/// 轮末判定所需的当轮事实。
pub(super) struct RoundFacts {
    /// 本轮序号(从 1 开始,= 已执行完工具轮数)。
    pub(super) round: usize,
    /// 工具循环已耗时。
    pub(super) elapsed: Duration,
    /// 本 run 累计 prompt + completion。生产路径由 evaluate_token 单独接收;
    /// 本字段只服务决策表单测入口(避免把「取用量」和「判其余闸门」耦合回一次调用)。
    #[allow(dead_code)]
    pub(super) used_tokens: i64,
    /// 语义熔断命中:(工具名, 窗口内次数, 输出去重数)。
    pub(super) semantic_break: Option<(String, usize, usize)>,
    /// 重复调用熔断命中:(工具名, 窗口内同指纹次数)。
    pub(super) repeated: Option<(String, usize)>,
}

/// 非停止类提醒(发出后循环继续)。
pub(super) enum GuardNotice {
    TokenBudgetWarn {
        used_tokens: i64,
        budget: u32,
        round: usize,
    },
    ReadonlyNudge {
        limit: usize,
    },
    NoProgressNudge {
        rounds: usize,
        limit: usize,
    },
}

impl GuardNotice {
    /// SSE 事件标题 + 明细(与抽出前逐字一致)。
    pub(super) fn event(&self) -> (&'static str, String) {
        match self {
            Self::TokenBudgetWarn {
                used_tokens,
                budget,
                round,
            } => (
                "Token 预算超限",
                format!(
                    "已用 {used_tokens} token / 预算 {budget}(第 {round} 轮):仅提示(warn 档),循环继续"
                ),
            ),
            Self::ReadonlyNudge { limit } => (
                "只读探索提醒",
                format!(
                    "已连续 {limit} 次只读工具调用没有写出产物:已提醒先落盘;若任务只需调研,请直接给出结论"
                ),
            ),
            Self::NoProgressNudge { rounds, limit } => (
                "无文件进展提醒",
                format!(
                    "已连续 {rounds} 轮没有成功写入本主负责的文件:已提醒先落盘;再连续 {} 轮仍无写入将收尾",
                    limit.saturating_sub(*rounds)
                ),
            ),
        }
    }

    /// 需要回灌给模型的 user 提醒文本(只读/无进展守卫有;预算提醒无)。
    pub(super) fn nudge_text(&self) -> Option<&'static str> {
        match self {
            Self::TokenBudgetWarn { .. } => None,
            Self::ReadonlyNudge { .. } => Some(READONLY_FIRST_NUDGE),
            Self::NoProgressNudge { .. } => Some(NO_PROGRESS_FIRST_NUDGE),
        }
    }
}

/// 轮末判定结果:先按顺序发 `notices`,最后若 `stop` 非空则收尾。
pub(super) struct GuardEvaluation {
    pub(super) notices: Vec<GuardNotice>,
    pub(super) stop: Option<LoopStop>,
}

impl GuardState {
    pub(super) fn new(config: GuardConfig) -> Self {
        Self {
            config,
            budget_noticed: false,
            readonly_streak: 0,
            readonly_nudged: false,
            no_progress_rounds: 0,
            no_progress_nudged: false,
        }
    }

    /// 按本轮**模型请求**的工具调用统计只读/写类(TMR-5)。
    ///
    /// 被拒绝的写调用也算「已尝试动手」——因此看的是 `tool_calls` 而不是执行输出;
    /// 中性工具(todo/sleep 等)不增不减。
    pub(super) fn observe_tool_calls(&mut self, calls: &[ToolCallArgs]) {
        if self.config.readonly_limit.is_none() {
            return;
        }
        for call in calls {
            if crate::tools::tool_sets::is_readonly_probe_tool(&call.name) {
                self.readonly_streak += 1;
            } else if crate::tools::tool_sets::is_write_like_tool(&call.name) {
                // 一旦真的动手(或尝试派发新执行面),本次「只读空转段」结束:
                // 清空计数并允许下一次空转段重新提醒。
                self.readonly_streak = 0;
                self.readonly_nudged = false;
            }
        }
    }

    /// 登记本轮是否有「成功写入声明文件」的进展,并产出中途提醒(先提醒后硬收尾)。
    ///
    /// 提醒点 = `limit / 2`(至少 1):SKY-7 的「硬收尾之前先给一次机会」。
    pub(super) fn observe_round_progress(&mut self, round_progress: bool) -> Option<GuardNotice> {
        let limit = self.config.no_progress_limit?;
        if round_progress {
            self.no_progress_rounds = 0;
            self.no_progress_nudged = false;
            return None;
        }
        self.no_progress_rounds += 1;
        let nudge_at = (limit / 2).max(1);
        if !self.no_progress_nudged && self.no_progress_rounds >= nudge_at {
            self.no_progress_nudged = true;
            return Some(GuardNotice::NoProgressNudge {
                rounds: self.no_progress_rounds,
                limit,
            });
        }
        None
    }

    /// 第一阶段:token 预算(HB-1)。
    ///
    /// 与其余闸门分开是为了**保持抽取前的采样时点**:旧实现先发出 token warn 事件
    /// (`await`),再到步骤墙钟处取 `elapsed`;若合并成一次判定,一次慢 send 会让墙钟
    /// 少算这段耗时,预算边界上可能多跑一轮。循环体先发本阶段提醒,再调 `evaluate_rest`。
    ///
    /// `session_id` 只用于诊断日志(保持抽取前的字段与文案),不参与判定。
    pub(super) fn evaluate_token(
        &mut self,
        session_id: &str,
        used_tokens: i64,
        round: usize,
    ) -> GuardEvaluation {
        let mut notices = Vec::new();
        if self.config.token_budget > 0 && budget_reached(used_tokens, self.config.token_budget) {
            if !self.budget_noticed {
                self.budget_noticed = true;
                tracing::info!(
                    session_id = session_id.to_string(),
                    used_tokens,
                    budget = self.config.token_budget,
                    rounds = round + 1,
                    action = if self.config.stop_on_budget {
                        "stop"
                    } else {
                        "warn"
                    },
                    "单次生成 token 预算超限"
                );
                if !self.config.stop_on_budget {
                    notices.push(GuardNotice::TokenBudgetWarn {
                        used_tokens,
                        budget: self.config.token_budget,
                        round: round + 1,
                    });
                }
            }
            if self.config.stop_on_budget {
                let budget = self.config.token_budget;
                let display_round = round + 1;
                return GuardEvaluation {
                    notices,
                    stop: Some(LoopStop {
                        kind: LoopStopKind::TokenBudget,
                        message: format!(
                            "Token 预算超限:已用 {used_tokens} token / 预算 {budget}(第 {display_round} 轮)"
                        ),
                        event_title: "Token 预算超限,停止工具循环",
                        event_detail: format!(
                            "已用 {used_tokens} token / 预算 {budget}(第 {display_round} 轮):已停止工具循环,本轮产出与用量照常保留"
                        ),
                    }),
                };
            }
        }
        GuardEvaluation {
            notices,
            stop: None,
        }
    }

    /// 第二阶段:步骤墙钟 → 轮次上限 → 连续只读 → 声明文件无进展 → 语义 → 重复调用。
    ///
    /// 顺序与抽出前逐字一致;`session_id` 只用于诊断日志,不参与判定。
    pub(super) fn evaluate_rest(&mut self, session_id: &str, facts: RoundFacts) -> GuardEvaluation {
        let mut notices = Vec::new();

        // 2) 步骤墙钟预算(D3):时间闸门优先于轮数闸门。
        if facts.round > 0 && step_budget_reached(facts.elapsed, self.config.step_budget) {
            let budget_secs = self.config.step_budget.map(|b| b.as_secs()).unwrap_or(0);
            return GuardEvaluation {
                notices,
                stop: Some(LoopStop {
                    kind: LoopStopKind::StepBudget,
                    message: format!(
                        "步骤墙钟预算用尽:已用 {}s / 预算 {budget_secs}s(第 {} 轮)",
                        facts.elapsed.as_secs(),
                        facts.round
                    ),
                    event_title: "步骤墙钟预算用尽",
                    event_detail: format!(
                        "已用 {}s / 预算 {budget_secs}s(第 {} 轮):已停止工具循环,本轮产出与用量照常保留",
                        facts.elapsed.as_secs(),
                        facts.round
                    ),
                }),
            };
        }

        // 3) 轮次上限(显式配置时才判;None = 不限)。
        if self
            .config
            .max_rounds
            .is_some_and(|limit| facts.round >= limit)
        {
            return GuardEvaluation {
                notices,
                stop: Some(LoopStop {
                    kind: LoopStopKind::RoundLimit,
                    message: format!("达到工具调用轮次上限:已执行 {} 轮", facts.round),
                    event_title: "达到工具调用轮次上限",
                    event_detail: format!(
                        "已执行 {} 轮工具调用,停止继续调用工具,输出当前结果",
                        facts.round
                    ),
                }),
            };
        }

        // 4) 连续只读空转(TMR-5):先提醒一次,再次达到则中止本步。
        if let Some(limit) = self.config.readonly_limit {
            if self.readonly_streak >= limit {
                self.readonly_streak = 0;
                if !self.readonly_nudged {
                    self.readonly_nudged = true;
                    notices.push(GuardNotice::ReadonlyNudge { limit });
                } else {
                    return GuardEvaluation {
                        notices,
                        stop: Some(LoopStop {
                            kind: LoopStopKind::Readonly,
                            message: format!(
                                "连续只读空转熔断:连续 {limit} 次只读工具调用没有写出任何产物"
                            ),
                            event_title: "连续只读空转熔断",
                            event_detail: format!(
                                "已连续 {limit} 次只读工具调用没有写出任何产物,且提醒后仍未落盘,已在第 {} 轮中止;已有正文与用量照常保留",
                                facts.round
                            ),
                        }),
                    };
                }
            }
        }

        // 5) 声明文件无进展收尾(P0-2):与只读守卫互补。
        if let Some(limit) = self.config.no_progress_limit {
            if self.no_progress_rounds >= limit {
                tracing::warn!(
                    session_id = session_id.to_string(),
                    limit,
                    round = facts.round,
                    "声明文件无进展收尾"
                );
                return GuardEvaluation {
                    notices,
                    stop: Some(LoopStop {
                        kind: LoopStopKind::NoProgress,
                        message: format!(
                            "声明文件无进展收尾:连续 {limit} 轮没有成功写入本主负责的文件"
                        ),
                        event_title: "无文件进展收尾",
                        event_detail: format!(
                            "已连续 {limit} 轮没有成功写入本主负责的文件:已停止工具循环,本轮产出与用量照常保留"
                        ),
                    }),
                };
            }
        }

        // 6) 语义熔断(HB-2):参数可以变,输出实质无变化。
        if let Some((tool, calls, distinct)) = facts.semantic_break {
            let detail = format!(
                "检测到重复空转:工具 \"{tool}\" 在最近 {} 次调用中出现 {calls} 次,输出实质无变化(去重后 {distinct} 种),已在第 {} 轮中止。请调整策略后重试。",
                self.config.semantic_window, facts.round
            );
            tracing::warn!(
                session_id = session_id.to_string(),
                tool = tool.as_str(),
                calls,
                distinct,
                "工具循环重复空转熔断(HB-2)"
            );
            return GuardEvaluation {
                notices,
                stop: Some(LoopStop {
                    kind: LoopStopKind::Semantic,
                    message: format!(
                        "重复空转熔断:工具 \"{tool}\" 在最近 {} 次调用中出现 {calls} 次,输出实质无变化",
                        self.config.semantic_window
                    ),
                    event_title: "重复空转熔断",
                    event_detail: detail,
                }),
            };
        }

        // 7) 重复调用熔断(P0-2):同参数在窗口内反复出现。
        if let Some((tool, times)) = facts.repeated {
            let detail = format!(
                "检测到重复调用:工具 \"{tool}\" 以相同参数在最近 {DEFAULT_WINDOW} 轮内出现 {times} 次,已在第 {} 轮中止以避免空转。请调整策略后重试。",
                facts.round
            );
            tracing::warn!(
                session_id = session_id.to_string(),
                tool = tool.as_str(),
                repeat_count = times,
                round = facts.round,
                "工具循环重复调用熔断"
            );
            return GuardEvaluation {
                notices,
                stop: Some(LoopStop {
                    kind: LoopStopKind::RepeatedCall,
                    message: format!("重复调用熔断:工具 \"{tool}\" 以相同参数出现 {times} 次"),
                    event_title: "重复调用熔断",
                    event_detail: detail,
                }),
            };
        }

        GuardEvaluation {
            notices,
            stop: None,
        }
    }

    /// 决策表单测的便捷入口:先 token 再其余(不模拟提醒事件的发送耗时)。
    #[cfg(test)]
    fn evaluate(&mut self, session_id: &str, facts: RoundFacts) -> GuardEvaluation {
        let used_tokens = facts.used_tokens;
        let round = facts.round;
        let mut first = self.evaluate_token(session_id, used_tokens, round);
        if first.stop.is_some() {
            return first;
        }
        let mut rest = self.evaluate_rest(session_id, facts);
        first.notices.append(&mut rest.notices);
        first.stop = rest.stop;
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> GuardConfig {
        GuardConfig {
            max_rounds: Some(32),
            token_budget: 0,
            stop_on_budget: false,
            step_budget: None,
            readonly_limit: None,
            no_progress_limit: None,
            semantic_window: 8,
        }
    }

    fn facts(round: usize) -> RoundFacts {
        facts_at(round, 0, Duration::ZERO)
    }

    fn facts_at(round: usize, used_tokens: i64, elapsed: Duration) -> RoundFacts {
        RoundFacts {
            round,
            elapsed,
            used_tokens,
            semantic_break: None,
            repeated: None,
        }
    }

    fn call(name: &str) -> ToolCallArgs {
        ToolCallArgs {
            id: "c1".into(),
            name: name.into(),
            arguments: "{}".into(),
        }
    }

    /// 决策表第 1 行:token 预算 stop 优先于步骤墙钟与轮次上限,且 `is_budget()` 为真。
    #[test]
    fn token_budget_stop_wins_over_step_and_round() {
        let mut state = GuardState::new(GuardConfig {
            token_budget: 100,
            stop_on_budget: true,
            step_budget: Some(Duration::from_secs(1)),
            max_rounds: Some(1),
            ..base_config()
        });
        let out = state.evaluate("s", facts_at(1, 100, Duration::from_secs(5)));
        let stop = out.stop.expect("token 预算达到且 stop 档必须收尾");
        assert_eq!(stop.kind, LoopStopKind::TokenBudget);
        assert!(stop.kind.is_budget(), "token 预算属于预算类闸门");
        assert!(!stop.kind.is_guard(), "预算类不得被当成守卫截断");
        assert!(stop.message.contains("Token 预算超限"), "{}", stop.message);
        assert_eq!(stop.event_title, "Token 预算超限,停止工具循环");
        assert!(out.notices.is_empty(), "stop 档不再单独发 warn 提醒");
    }

    /// 决策表第 2 行:warn 档只提醒一次;同轮继续走到轮次上限时轮次上限收尾。
    #[test]
    fn token_budget_warn_notices_once_then_round_limit_stops() {
        let mut state = GuardState::new(GuardConfig {
            token_budget: 100,
            max_rounds: Some(1),
            ..base_config()
        });
        let first = state.evaluate("s", facts_at(1, 120, Duration::ZERO));
        assert_eq!(first.notices.len(), 1, "首次越线应发一次 warn 提醒");
        let stop = first.stop.expect("已达轮次上限,应继续判定到轮次收尾");
        assert_eq!(stop.kind, LoopStopKind::RoundLimit);

        let second = state.evaluate("s", facts_at(1, 130, Duration::ZERO));
        assert!(second.notices.is_empty(), "warn 提醒只发一次");
        assert_eq!(second.stop.map(|s| s.kind), Some(LoopStopKind::RoundLimit));
    }

    /// 决策表第 3 行:步骤墙钟优先于轮次上限;第 0 轮不判(与抽出前 `round > 0` 一致)。
    #[test]
    fn step_budget_wins_over_round_limit_only_after_first_round() {
        let config = GuardConfig {
            step_budget: Some(Duration::from_secs(1)),
            max_rounds: Some(1),
            ..base_config()
        };
        let mut state = GuardState::new(config);
        let zeroth = state.evaluate("s", facts_at(0, 0, Duration::from_secs(5)));
        assert!(zeroth.stop.is_none(), "第 0 轮不判步骤预算");

        let out = state.evaluate("s", facts_at(1, 0, Duration::from_secs(1)));
        let stop = out.stop.expect("到点必须收尾");
        assert_eq!(stop.kind, LoopStopKind::StepBudget);
        assert!(stop.kind.is_budget());
        assert!(
            stop.event_detail.contains("预算 1s"),
            "{}",
            stop.event_detail
        );
    }

    /// 决策表第 4 行:轮次上限是正常收尾,不属于预算/守卫。
    #[test]
    fn round_limit_stops_only_at_configured_round() {
        let mut state = GuardState::new(GuardConfig {
            max_rounds: Some(2),
            ..base_config()
        });
        assert!(
            state.evaluate("s", facts(1)).stop.is_none(),
            "未到上限不得收尾"
        );
        let stop = state.evaluate("s", facts(2)).stop.expect("达到上限应收尾");
        assert_eq!(stop.kind, LoopStopKind::RoundLimit);
        assert!(!stop.kind.is_budget() && !stop.kind.is_guard());
    }

    /// 决策表第 5 行:只读守卫先提醒、等到下一段再熔断;写类工具清空计数与提醒位。
    #[test]
    fn readonly_nudges_then_stops_and_write_resets() {
        let mut state = GuardState::new(GuardConfig {
            readonly_limit: Some(2),
            ..base_config()
        });
        state.observe_tool_calls(&[call("fs_read"), call("fs_read")]);
        let first = state.evaluate("s", facts(1));
        assert_eq!(first.notices.len(), 1, "首次达到阈值只提醒");
        assert!(first.stop.is_none(), "首次达到阈值不得熔断");
        assert!(matches!(
            &first.notices[0],
            GuardNotice::ReadonlyNudge { limit: 2 }
        ));

        state.observe_tool_calls(&[call("fs_read"), call("fs_read")]);
        let second = state.evaluate("s", facts(2));
        let stop = second.stop.expect("提醒后再次达到必须熔断");
        assert_eq!(stop.kind, LoopStopKind::Readonly);
        assert!(stop.message.contains("连续 2 次只读"), "{}", stop.message);
        assert!(stop.event_detail.contains("提醒后仍未落盘"));

        // 写类工具出现即清零,并允许下一段重新提醒。
        let mut reset = GuardState::new(GuardConfig {
            readonly_limit: Some(2),
            ..base_config()
        });
        reset.observe_tool_calls(&[call("fs_read")]);
        reset.observe_tool_calls(&[call("fs_write")]);
        reset.observe_tool_calls(&[call("fs_read")]);
        let out = reset.evaluate("s", facts(1));
        assert!(out.stop.is_none(), "写类工具应清零只读计数");
        assert!(out.notices.is_empty());
    }

    /// 决策表第 6 行:无进展守卫在第 limit/2 轮提醒、第 limit 轮硬收尾;有进展重置。
    #[test]
    fn no_progress_nudges_at_half_then_stops_at_limit() {
        let mut state = GuardState::new(GuardConfig {
            no_progress_limit: Some(4),
            ..base_config()
        });
        assert!(state.observe_round_progress(false).is_none());
        let nudge = state
            .observe_round_progress(false)
            .expect("第 2 轮(limit/2)应提醒");
        assert!(matches!(
            &nudge,
            GuardNotice::NoProgressNudge {
                rounds: 2,
                limit: 4
            }
        ));
        let (_, detail) = nudge.event();
        assert!(detail.contains("再连续 2 轮"), "{detail}");
        assert!(state.observe_round_progress(false).is_none());
        assert!(
            state.evaluate("s", facts(3)).stop.is_none(),
            "第 3 轮不得收尾"
        );
        assert!(state.observe_round_progress(false).is_none());
        let out = state.evaluate("s", facts(4));
        let stop = out.stop.expect("第 4 轮必须收尾");
        assert_eq!(stop.kind, LoopStopKind::NoProgress);
        assert!(stop.message.contains("连续 4 轮"), "{}", stop.message);

        // 有进展 = 本轮空转段结束,下一次连续无写重新获得一次提醒。
        let mut reset = GuardState::new(GuardConfig {
            no_progress_limit: Some(4),
            ..base_config()
        });
        reset.observe_round_progress(false);
        reset.observe_round_progress(false); // 提醒
        reset.observe_round_progress(true); // 有进展
        assert!(reset.observe_round_progress(false).is_none());
        let nudge_again = reset
            .observe_round_progress(false)
            .expect("重置后第 2 轮应重新提醒");
        assert!(matches!(nudge_again, GuardNotice::NoProgressNudge { .. }));
    }

    /// 决策表第 7/8 行:语义熔断先于重复调用熔断(两者同轮命中时语义胜出)。
    #[test]
    fn semantic_stops_before_repeated_call() {
        let mut state = GuardState::new(base_config());
        let mut first = facts(4);
        first.semantic_break = Some(("bash".into(), 8, 1));
        first.repeated = Some(("fs_read".into(), 3));
        let out = state.evaluate("s", first);
        let stop = out.stop.expect("语义熔断应胜出");
        assert_eq!(stop.kind, LoopStopKind::Semantic);
        assert!(stop.message.contains("bash"), "{}", stop.message);
        assert!(stop.message.contains("出现 8 次"), "{}", stop.message);
        assert!(stop.event_detail.contains("去重后 1 种"));

        let mut second = facts(5);
        second.repeated = Some(("fs_read".into(), 3));
        let out = state.evaluate("s", second);
        let stop = out.stop.expect("无语义命中时应落到重复调用熔断");
        assert_eq!(stop.kind, LoopStopKind::RepeatedCall);
        assert!(stop.message.contains("fs_read"), "{}", stop.message);
        assert!(stop.event_detail.contains(&DEFAULT_WINDOW.to_string()));
    }

    /// 判定「守卫类」只覆盖 4 个空转守卫;预算/轮次不得被任务侧降级成 partial。
    #[test]
    fn only_idle_guards_are_guard_kinds() {
        for kind in [
            LoopStopKind::Readonly,
            LoopStopKind::NoProgress,
            LoopStopKind::Semantic,
            LoopStopKind::RepeatedCall,
        ] {
            assert!(kind.is_guard(), "{kind:?} 应属于守卫类");
            assert!(!kind.is_budget(), "{kind:?} 不属于预算类");
        }
        for kind in [
            LoopStopKind::TokenBudget,
            LoopStopKind::StepBudget,
            LoopStopKind::RoundLimit,
        ] {
            assert!(!kind.is_guard(), "{kind:?} 不得被当成守卫截断");
        }
        assert!(LoopStopKind::TokenBudget.is_budget());
        assert!(LoopStopKind::StepBudget.is_budget());
    }

    /// 未达到任何闸门时既不提醒也不收尾(负样例,防判定过于激进)。
    #[test]
    fn healthy_round_produces_no_notice_or_stop() {
        let mut state = GuardState::new(GuardConfig {
            token_budget: 1000,
            step_budget: Some(Duration::from_secs(60)),
            max_rounds: Some(32),
            readonly_limit: Some(8),
            no_progress_limit: Some(16),
            ..base_config()
        });
        state.observe_tool_calls(&[call("fs_write")]);
        assert!(state.observe_round_progress(true).is_none());
        let out = state.evaluate("s", facts_at(1, 200, Duration::from_secs(2)));
        assert!(out.notices.is_empty());
        assert!(out.stop.is_none());
    }

    /// 同一轮既有提醒又有 stop 时,提醒必须随判定结果一起返回(循环体先发提醒再发
    /// stop;这是 `mod.rs` 事件顺序的可回归证据,与抽取前一致)。
    #[test]
    fn notices_are_kept_when_same_round_also_stops() {
        let mut state = GuardState::new(GuardConfig {
            readonly_limit: Some(2),
            ..base_config()
        });
        state.observe_tool_calls(&[call("fs_read"), call("fs_read")]);
        let mut round = facts(3);
        round.semantic_break = Some(("bash".into(), 8, 1));
        let out = state.evaluate("s", round);
        assert_eq!(out.notices.len(), 1, "只读提醒必须先于 stop 保留");
        assert!(matches!(
            &out.notices[0],
            GuardNotice::ReadonlyNudge { limit: 2 }
        ));
        assert_eq!(
            out.stop.expect("语义熔断应胜出").kind,
            LoopStopKind::Semantic
        );
    }
}
