// 急停工具(stop_computer_control,CU-1,2026-10-06):Agent 可调的自停开关。
//
// 用途:用户在对话里说「别再操作/看我屏幕」时,模型可主动置位急停——与前端
// 「停止操作电脑」按钮写**同一后端状态**(`services::computer_use::ComputerUseControl`,
// 由 AppState/ToolDeps 共享同一实例)。
//
// 边界(有意为之):
// - 工具面**只暴露置位,不暴露解除**:解除是用户决策(前端「恢复」按钮/端点),
//   不给模型自解锁——急停若可被模型随手解除就失去了「用户在场」的意义;
// - 风险级 **Safe**(见 permissions.rs 显式登记):急停是收紧动作,任何授权模式都应能
//   立即生效;若归 Dangerous,任务默认策略(deny_dangerous)会把它整族剔掉,
//   恰恰让无人值守任务失去急停能力(与截图不同的取舍,理由记录在此);
// - 不写 cu_audit:审计记的是「读屏/输入类动作」,置位急停本身是控制动作,不属该面。
//
// 可见性:始终注册、始终下发(chat 与任务默认策略都含它)——它不依赖截图开关,
// 因为「停止操作电脑」在任何时刻都应有出口(即使截图当前是关的,模型也可响应
// 用户「刚才别看我屏幕」的指令)。

use crate::models::types::{ToolContext, ToolDefinition};
use crate::tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::Arc;

use super::agent_tools::ToolDeps;

/// 工具名(单一出处:注册、风险级登记、渲染意图与测试都以它为准)
pub const TOOL_NAME: &str = "stop_computer_control";

/// 注册急停工具(始终注册且始终可见;判据见文件头)
pub fn register_stop_control_tool(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    let definition = ToolDefinition {
        name: TOOL_NAME.into(),
        description: "立即停止一切电脑操作类能力(当前含截图取屏),直到用户在界面上恢复。\
                      当用户要求你停止查看/操作其电脑,或你察觉当前屏幕读取并非用户所愿时调用。"
            .into(),
        parameters: json!({ "type": "object", "properties": {} }),
    };
    registry.register(
        definition,
        Arc::new(move |_args: serde_json::Value, _ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move {
                deps.cu_control.stop();
                Ok(json!({
                    "text": "已停止电脑操作:屏幕/输入类工具在用户恢复前不会执行。请告知用户已停止,并等待其确认后续动作。",
                    "stopped": true
                })
                .to_string())
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::ToolRegistry;

    /// 置位即生效:调用工具后 cu_control 进入急停态、返回体带 stopped=true(幂等)
    #[tokio::test]
    async fn stop_tool_sets_control_flag() {
        let (_guard, deps) = ToolDeps::dummy_for_test();
        let deps = Arc::new(deps);
        let registry = ToolRegistry::with_permissions(
            crate::tools::permissions::ToolPermissionManager::in_memory(),
        );
        register_stop_control_tool(&registry, deps.clone());
        assert!(!deps.cu_control.is_stopped());
        let ctx = ToolContext {
            session_id: "s".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
            budget: None,
        };
        let out = registry
            .execute(TOOL_NAME, "{}", ctx.clone())
            .await
            .expect("急停工具应始终可调(风险级 Safe)");
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["stopped"], serde_json::Value::Bool(true));
        assert!(deps.cu_control.is_stopped());
        // 幂等:再调一次仍成功、仍处于急停
        let _ = registry.execute(TOOL_NAME, "{}", ctx).await.unwrap();
        assert!(deps.cu_control.is_stopped());
    }
}
