// 任务三档授权裁决（2026-11-06）：逐步 / 工作区内 / 自动。
//
// 从 `permissions.rs` 拆出的理由：三档逻辑与「聊天侧文件三档矩阵」是两套正交语义，
// 混在一个 1000+ 行文件里既难读也触发了 file-size ratchet。本模块只做**任务档位**
// 的裁决，返回 `Some(decision)` 表示已由档位裁定，`None` 表示交给 `permissions.rs`
// 的既有命令/文件矩阵。
//
// 优先级（不可调换）：
//   1. 屏幕/输入类（ScreenInput）任何任务档位都不放行（CU-1）；
//   2. 逐步：写/执行类工具一律转确认（只读工具交给既有矩阵）；
//   3. 工作区内：命令可证明在工作区内 → 放行；越界/无法证明 → 转确认；fs_* 仍由
//      `workspace_guard` 硬拒；
//   4. 自动：破坏性/提权命令放行（逐条写审计），ScreenInput 仍拒；非命令工具交给
//      既有矩阵（工具面由 `tool_policy` 的 `all` 保证）。
use crate::models::tool_policy::{TaskAuthorizationMode, ToolRisk};
use crate::models::types::ToolContext;
use crate::tools::action_class::ToolAction;
use crate::tools::command_risk::CommandRisk;
use crate::tools::permissions::PermissionDecision;
use crate::tools::workspace_check::{check_command, WorkspaceVerdict};

/// 读取当前任务档位：优先 scope（绑定工作区），回退 budget（未绑定工作区）。
pub(crate) fn task_authorization_of(ctx: &ToolContext) -> Option<TaskAuthorizationMode> {
    ctx.scope
        .as_ref()
        .and_then(|scope| scope.task_authorization())
        .or_else(|| ctx.budget.and_then(|b| b.task_authorization))
}

/// 按任务档位裁决。`Some` = 档位已裁定；`None` = 继续走既有矩阵。
pub(crate) fn decide_task_mode(
    tool: &str,
    ctx: &ToolContext,
    action: &ToolAction,
    risk: ToolRisk,
) -> Option<PermissionDecision> {
    let mode = task_authorization_of(ctx)?;
    // CU-1：屏幕/输入类任何任务档位都不放行。Step 的「逐条确认」不等于
    // 「允许操作真实设备」——它与 Destructive/Admin 不同类，后果不可撤销。
    if action.is_exec_op()
        && action.exec_risk.unwrap_or(CommandRisk::Admin) == CommandRisk::ScreenInput
    {
        return Some(PermissionDecision::denied_with_code(
            risk,
            "屏幕/输入类命令会代替用户操作真实设备/读取屏幕,任务模式任何授权档位都不放行".into(),
            "screen_input_denied",
        ));
    }
    match mode {
        TaskAuthorizationMode::Step => {
            if crate::tools::tool_sets::is_write_like_tool(tool) {
                return Some(PermissionDecision::confirm(
                    risk,
                    "逐步授权:该工具会写入或发起执行,需要你确认后才执行".into(),
                    "step_confirm",
                ));
            }
            None
        }
        TaskAuthorizationMode::Workspace => {
            if !action.is_exec_op() {
                return None;
            }
            // 未绑定工作区时没有可判定的根：一律转确认（fail-closed），
            // 不能因为「没有工作区」就当成「哪里都在工作区内」。
            let verdict = match ctx.scope.as_ref() {
                Some(scope) => check_command(
                    action.exec_command.as_deref().unwrap_or(""),
                    action.exec_cwd.as_deref(),
                    scope.workspace(),
                ),
                None => WorkspaceVerdict::Unknown(
                    "本任务未绑定工作区,无法判定命令是否在工作区内".into(),
                ),
            };
            Some(match verdict {
                WorkspaceVerdict::Inside => {
                    PermissionDecision::allowed(risk, "工作区内命令自动放行".into())
                }
                verdict => PermissionDecision::confirm(risk, verdict.reason(), "workspace_escape"),
            })
        }
        TaskAuthorizationMode::Auto => {
            // 非命令工具交给既有矩阵（危险工具的工具面由 tool_policy 的 all 负责）。
            if !action.is_exec_op() {
                return None;
            }
            let cmd_risk = action.exec_risk.unwrap_or(CommandRisk::Admin);
            if cmd_risk.requires_explicit_confirm() {
                return Some(PermissionDecision::allowed(
                    risk,
                    format!("自动授权已放行{}命令(逐条记入执行审计)", cmd_risk.label()),
                ));
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::action_class::{PathZone, ToolOp, ToolOrigin};

    fn ctx(mode: TaskAuthorizationMode, workspace: Option<&str>) -> ToolContext {
        let workspace = workspace.unwrap_or(if cfg!(windows) {
            r"C:\work\repo"
        } else {
            "/work/repo"
        });
        let scope = Some(std::sync::Arc::new(
            crate::models::types::ExecScope::new(std::path::PathBuf::from(workspace), true)
                .with_task_authorization(Some(mode)),
        ));
        ToolContext {
            session_id: "task:t1".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope,
            budget: Some(crate::models::types::ToolBudget {
                semantic_guard: None,
                readonly_streak_limit: None,
                no_progress_limit: None,
                shard_read_nudge_after: None,
                task_authorization: Some(mode),
            }),
        }
    }

    fn unbound_ctx(mode: TaskAuthorizationMode) -> ToolContext {
        ToolContext {
            session_id: "task:t1".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
            budget: Some(crate::models::types::ToolBudget {
                semantic_guard: None,
                readonly_streak_limit: None,
                no_progress_limit: None,
                shard_read_nudge_after: None,
                task_authorization: Some(mode),
            }),
        }
    }

    fn exec(cmd: &str) -> ToolAction {
        crate::tools::action_class::classify(
            "bash",
            &format!(r#"{{"command":"{cmd}"}}"#),
            ToolOrigin::Builtin,
        )
    }

    fn risk_action(risk: CommandRisk) -> ToolAction {
        ToolAction {
            op: ToolOp::Exec,
            zone: PathZone::Opaque,
            target: Some("del C:\\tmp\\a.txt".into()),
            reason: "命令用例".into(),
            exec_risk: Some(risk),
            exec_command: Some("del C:\\tmp\\a.txt".into()),
            exec_cwd: None,
        }
    }

    fn read_action() -> ToolAction {
        ToolAction {
            op: ToolOp::Other,
            zone: PathZone::Opaque,
            target: None,
            reason: String::new(),
            exec_risk: None,
            exec_command: None,
            exec_cwd: None,
        }
    }

    /// 逐步：写/执行类确认，只读放行（None = 交给既有矩阵）。
    #[test]
    fn step_confirms_write_like_only() {
        let ctx = ctx(TaskAuthorizationMode::Step, None);
        let d = decide_task_mode(
            "bash",
            &ctx,
            &risk_action(CommandRisk::Safe),
            ToolRisk::Dangerous,
        )
        .expect("bash 是写类,应被档位裁定");
        assert!(!d.allowed);
        assert_eq!(d.code.as_deref(), Some("step_confirm"));
        for tool in ["fs_read", "todo", "read", "search", "memory_read"] {
            assert!(
                decide_task_mode(tool, &ctx, &read_action(), ToolRisk::Safe).is_none(),
                "{tool} 属只读,应继续走既有矩阵"
            );
        }
    }

    /// 工作区内：区内命令放行；越界/变量/未绑定工作区转确认。
    #[test]
    fn workspace_allows_inside_and_confirms_escape() {
        let ctx = ctx(TaskAuthorizationMode::Workspace, None);
        let d = decide_task_mode("bash", &ctx, &exec("cargo test"), ToolRisk::Dangerous)
            .expect("命令应由档位裁定");
        assert!(d.allowed, "区内命令应放行;理由:{}", d.reason);

        let outside = if cfg!(windows) {
            r"type C:\Windows\win.ini"
        } else {
            "cat /etc/passwd"
        };
        let d = decide_task_mode("bash", &ctx, &exec(outside), ToolRisk::Dangerous).unwrap();
        assert!(!d.allowed);
        assert_eq!(
            d.code.as_deref(),
            Some("workspace_escape"),
            "理由:{}",
            d.reason
        );

        let d =
            decide_task_mode("bash", &ctx, &exec("del %TEMP%\\*.*"), ToolRisk::Dangerous).unwrap();
        assert!(!d.allowed);
        assert_eq!(d.code.as_deref(), Some("workspace_escape"));
    }

    /// 工作区内但未绑定工作区：fail-closed。
    #[test]
    fn workspace_without_workspace_fails_closed() {
        let ctx = unbound_ctx(TaskAuthorizationMode::Workspace);
        let d = decide_task_mode("bash", &ctx, &exec("cargo test"), ToolRisk::Dangerous).unwrap();
        assert!(!d.allowed);
        assert_eq!(d.code.as_deref(), Some("workspace_escape"));
        assert!(d.reason.contains("未绑定工作区"), "理由:{}", d.reason);
    }

    /// 自动：破坏性/提权放行；屏幕/输入仍拒。
    #[test]
    fn auto_allows_destructive_but_never_screen_input() {
        let ctx = ctx(TaskAuthorizationMode::Auto, None);
        for risk in [CommandRisk::Destructive, CommandRisk::Admin] {
            let d = decide_task_mode("bash", &ctx, &risk_action(risk), ToolRisk::Dangerous)
                .expect("自动档应裁定命令");
            assert!(d.allowed, "自动档应放行 {risk:?};理由:{}", d.reason);
            assert!(d.reason.contains("自动授权"), "理由应点名档位:{}", d.reason);
        }
        let d = decide_task_mode(
            "bash",
            &ctx,
            &risk_action(CommandRisk::ScreenInput),
            ToolRisk::Dangerous,
        )
        .unwrap();
        assert!(!d.allowed);
        assert_eq!(d.code.as_deref(), Some("screen_input_denied"));
    }

    /// 旧任务(None 档位):行为与三档上线前逐字一致(高危直接拒绝,不等待)。
    #[test]
    fn legacy_task_without_mode_keeps_old_behavior() {
        use crate::models::tool_policy::AuthorizationMode;
        use crate::tools::permissions::ToolPermissionManager;
        let m = ToolPermissionManager::in_memory();
        let ctx = ToolContext {
            session_id: "task:t1".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
            budget: None,
        };
        let d = m.decide_with_policy(
            "bash",
            &ctx,
            true,
            true,
            AuthorizationMode::Loose,
            &risk_action(CommandRisk::Destructive),
            false,
        );
        assert!(!d.allowed);
        assert_eq!(d.code.as_deref(), Some("high_risk_denied"));
        assert!(
            d.reason.contains("任务模式不支持高危命令"),
            "理由:{}",
            d.reason
        );
    }

    /// 聊天路径不受任务档位影响:即使 settings 是 Bypass,高危命令仍逐条确认。
    #[test]
    fn chat_path_ignores_task_modes() {
        use crate::models::tool_policy::AuthorizationMode;
        use crate::tools::permissions::ToolPermissionManager;
        let m = ToolPermissionManager::in_memory();
        let ctx = ToolContext {
            session_id: "chat".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
            budget: None,
        };
        for mode in [
            AuthorizationMode::Strict,
            AuthorizationMode::Loose,
            AuthorizationMode::Bypass,
        ] {
            let d = m.decide_with_policy(
                "bash",
                &ctx,
                true,
                false,
                mode,
                &risk_action(CommandRisk::Destructive),
                false,
            );
            assert!(!d.allowed, "聊天路径 {mode:?} 下不得自动放行高危命令");
        }
    }
}
