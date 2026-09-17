// 任务模式工具策略:把 settings 里的策略档位编译为「下发工具定义 + 放行名单」。
//
// 背景:任务模式没有 UI 授权上下文,不能走「等待授权」(会空等超时)。因此任务侧
// 一律使用 ToolGate { no_ui_authorization: true } —— 名单内工具放行,名单外立即拒绝。
// 策略档位:
//   all            → 全量(剔除正文元工具),等价改造前的全放行行为
//   deny_dangerous → 全量 − 元工具 − 危险级 + 例外 bash(默认;无人值守任务不默认放行写类操作,
//                    但保留命令执行能力,命令级硬门见下)
//   allowlist      → 仅配置白名单 ∩ 已注册(最严格)
//
// bash 例外说明:bash 在权限矩阵里恒为 Dangerous(见 tools/permissions.rs 的 default_risk),
// 若按风险级一刀切就会被默认策略整体剔除,任务模式连 `ls`/`echo` 都用不了。用户要求任务
// 模式具备命令执行能力,故这里按**工具名**开例外。注意「放行工具」≠「放行危险命令」:
// permissions::check 的命令级硬门在任何自动放行之前判定,破坏性/提权命令在任务模式下
// 仍被直接拒绝(无 UI 可确认),只有 safe/sensitive 命令经白名单授权放行。
use crate::models::types::ToolDefinition;
// 工具风险词汇已下沉 L1(2026-09-14):L2 直连 models,不经 tools 转发。
use crate::models::tool_policy::ToolRisk;
use crate::tools::registry::ToolRegistry;
use crate::tools::tool_sets;

/// 编译结果。`allowed` 为放行名单(供 ToolGate 使用),需与 `defs` 同源:
/// 名单内 = 可被模型看到并执行;名单外工具即使被模型臆造调用也会被闸门拒绝。
pub(crate) struct TaskToolPolicy {
    pub defs: Vec<ToolDefinition>,
    pub allowed: Vec<String>,
}

impl TaskToolPolicy {
    /// 本策略对应的授权闸门:任务模式恒不等待授权(无 UI 上下文)。
    /// 注意:调用方若已取走 `defs`,应改用 `ToolGate::listed(&allowed)` 以免部分移动冲突。
    #[cfg(test)]
    pub(crate) fn gate(&self) -> crate::agents::engine::executor::ToolGate<'_> {
        crate::agents::engine::executor::ToolGate {
            whitelist: Some(&self.allowed),
            no_ui_authorization: true,
        }
    }
}

/// 按策略档位编译工具集。
/// `policy`:all / deny_dangerous / allowlist(非法值按 deny_dangerous 处理,与加载侧钳制一致)。
/// `allowlist`:仅 allowlist 档位使用。
pub(crate) fn compile(
    policy: &str,
    allowlist: &[String],
    registry: &ToolRegistry,
) -> TaskToolPolicy {
    let all = platform_gate(tool_sets::exclude_meta(registry.list_definitions()));
    let selected: Vec<ToolDefinition> = match policy {
        "all" => all,
        "allowlist" => tool_sets::filter_by_names(all, allowlist),
        // 默认(含非法值回退):拒绝危险级;bash 按工具名开例外(见文件头「bash 例外说明」),
        // 危险命令仍由 permissions 的命令级硬门拦截,不因本例外放行。
        _ => {
            let risk = registry.permissions();
            all.into_iter()
                .filter(|d| {
                    d.name == crate::tools::bash::TOOL_NAME
                        || risk.risk_for(&d.name) != ToolRisk::Dangerous
                })
                .collect()
        }
    };
    let allowed = selected.iter().map(|d| d.name.clone()).collect();
    TaskToolPolicy {
        defs: selected,
        allowed,
    }
}

/// 平台/档位可见性闸门(2026-09-17)。
///
/// 与风险级过滤正交:本函数按**运行环境**决定某些工具该不该出现在模型面前。
/// 当前唯一受限的是 `submit`(产物提交):
///   - 它只在 Android 且执行档位为**沙箱**(非 ROOT / 非 Shizuku)时有意义——
///     那两档下 `bash` 本就能写任意路径,再给一个 submit 只会让模型在两条等价通道间摇摆;
///   - 非 Android 平台它必然失败(桌面有保存对话框与 write 工具),下发纯属噪声。
///
/// 为什么在策略层而不是注册层过滤:工具始终注册,权限面板与契约类型才有一致的
/// 工具面;过滤只影响「本轮下发给模型什么」。档位在**每轮编译时现探**(带进程内
/// 缓存),故用户在任务执行途中改授权档位,下一轮的工具清单即随之变化。
fn platform_gate(defs: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    if crate::services::artifact_submit::is_available() {
        return defs;
    }
    defs.into_iter()
        .filter(|d| d.name != crate::tools::submit::TOOL_NAME)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::ToolRegistry;
    use serde_json::json;

    /// 注册若干代表性工具(含元工具与各风险级),用于策略断言
    fn registry_with_tools() -> ToolRegistry {
        let reg = ToolRegistry::new();
        let noop: crate::tools::registry::ToolExecutor =
            std::sync::Arc::new(|_a, _c| Box::pin(async { Ok("{}".to_string()) }));
        for name in [
            "read",
            "search",
            "todo",
            "agentgo",
            "agentend",
            "write",
            "replace",
            "create",
            "memory_write",
            "update_variables",
            "get_state",
            "apply_patch",
            "bash",
            // 名字含 "bash" 的其它危险工具:锁死「按工具名精确匹配」——
            // 若实现改成前缀/包含匹配,这两个会被误放行,泄漏测试即失败。
            "bash2",
            "mcp_x_bash",
            // 产物提交:敏感级,但受平台/档位闸门限制(见 platform_gate)
            "submit",
        ] {
            reg.register(
                ToolDefinition {
                    name: name.into(),
                    description: name.into(),
                    parameters: json!({ "type": "object" }),
                },
                noop.clone(),
            );
        }
        reg
    }

    #[test]
    fn deny_dangerous_excludes_dangerous_and_meta() {
        let reg = registry_with_tools();
        let p = compile("deny_dangerous", &[], &reg);
        let names: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        for banned in [
            "write",
            "replace",
            "create",
            "memory_write",
            "update_variables",
        ] {
            assert!(!names.contains(&banned), "危险工具 {banned} 应被拒绝");
        }
        for meta in ["get_state", "apply_patch"] {
            assert!(!names.contains(&meta), "元工具 {meta} 不应下发");
        }
        assert!(names.contains(&"read"));
        assert!(names.contains(&"agentend"), "agentend 属敏感级,应保留");
        // bash 恒为危险级但按工具名开例外:任务模式需具备命令执行能力
        // (危险命令由 permissions 的命令级硬门拦截,与工具是否下发无关)
        assert!(
            names.contains(&"bash"),
            "bash 应在 deny_dangerous 下下发(任务模式命令执行能力)"
        );
    }

    /// bash 例外只针对 bash 本身:其它危险工具在 deny_dangerous 下仍不得下发
    #[test]
    fn deny_dangerous_bash_exception_does_not_leak_to_other_dangerous() {
        let reg = registry_with_tools();
        let p = compile("deny_dangerous", &[], &reg);
        let names: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        // 精确匹配护栏:名字包含 "bash" 的其它危险工具不得被例外带出
        // (若实现改为前缀/包含匹配,下面两条断言即失败)
        for lookalike in ["bash2", "mcp_x_bash"] {
            assert!(
                !names.contains(&lookalike),
                "{lookalike} 名字含 bash 但非 bash 本体,deny_dangerous 下不得下发"
            );
        }
        let dangerous: Vec<&str> = p
            .defs
            .iter()
            .map(|d| d.name.as_str())
            .filter(|n| *n != "bash")
            .filter(|n| reg.permissions().risk_for(n) == ToolRisk::Dangerous)
            .collect();
        assert!(
            dangerous.is_empty(),
            "除 bash 外不得放行危险工具,实际:{}",
            dangerous.join(",")
        );
    }

    #[test]
    fn allowlist_only_keeps_configured_and_registered() {
        let reg = registry_with_tools();
        let p = compile(
            "allowlist",
            &["read".to_string(), "not_registered".to_string()],
            &reg,
        );
        let names: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read"]);
    }

    #[test]
    fn all_keeps_everything_except_meta() {
        let reg = registry_with_tools();
        let p = compile("all", &[], &reg);
        let names: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"write"));
        assert!(!names.contains(&"get_state"));
    }

    #[test]
    fn allowed_names_match_defs() {
        let reg = registry_with_tools();
        let p = compile("deny_dangerous", &[], &reg);
        let defs: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        let allowed: Vec<&str> = p.allowed.iter().map(|s| s.as_str()).collect();
        assert_eq!(defs, allowed);
    }

    /// 平台/档位闸门(2026-09-17):submit 只在 Android 沙箱档可见。
    ///
    /// 单测跑在桌面,故此处断言的是「不可用环境下必须剔除」这一侧;Android 侧的
    /// 反向断言(沙箱档下发、ROOT/Shizuku 档不下发)由 services::artifact_submit
    /// 的 is_available 语义 + Kotlin 档位探测共同保证,规则 L 守桥接方法登记。
    /// 关键点:**defs 与 allowed 必须同时剔除**——只剔 defs 会让模型臆造调用时报
    /// 「未注册」而非「不可用」,只剔 allowed 则模型看得见却调不动。
    #[test]
    fn submit_filtered_out_when_unavailable() {
        let reg = registry_with_tools();
        let p = compile("all", &[], &reg);
        let defs: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            !defs.contains(&"submit"),
            "不可用环境下 submit 不得下发: {defs:?}"
        );
        assert!(
            !p.allowed.iter().any(|n| n == "submit"),
            "allowed 与 defs 必须同源剔除: {:?}",
            p.allowed
        );
        // 其它工具不受影响(闸门只作用于 submit)
        assert!(defs.contains(&"read"), "闸门不得误伤其它工具: {defs:?}");
        assert!(defs.contains(&"bash"), "闸门不得误伤 bash: {defs:?}");
    }

    #[test]
    fn gate_never_waits_for_authorization() {
        let reg = registry_with_tools();
        let p = compile("deny_dangerous", &[], &reg);
        assert!(p.gate().no_ui_authorization, "任务模式闸门不得等待授权");
    }
}
