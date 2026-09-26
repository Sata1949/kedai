// submit 工具:把最终产物作为文件提交到设备下载位置(2026-09-17)。
//
// 使用场景(单一):Android **非 root / 非 Shizuku**(沙箱档)下,模型需要把最终
// 产物交付成用户能拿到的文件——沙箱档的 `bash` 够不到 Download,`write` 只作用于
// 角色文件区,故需要本工具这条专用通道。落点判定与平台分派见
// services::artifact_submit(本文件只做工具定义、参数解析与错误包装)。
//
// 可见性(重要):工具**始终注册**,但只在 Android 沙箱档下端给模型——
// 过滤点有两处,清单必须同步(task_engine/tool_policy.rs):
//   ① ToolPolicy::compile 剔除 defs/allowed(任务模式);
//   本工具不进 chat 侧下发路径(tool_sets 的三个白名单都不含它),
//   也不进规划侦察/子 agent/反思白名单。
//
// 风险级:Sensitive(不是 Dangerous)。理由:目标路径由应用决定、不接受任意路径参数,
// 文件名经清洗;若按 Dangerous 登记,任务模式默认策略(deny_dangerous)会把它整体
// 剔除,工具形同不存在(与 bash 的「按名开例外」是同一类问题,但这里更好的解法是
// 按真实风险级登记——它确实不危险)。
use crate::models::types::{ToolContext, ToolDefinition};
use crate::tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::Arc;

/// 工具名(权限矩阵、前端渲染与工具集合清单均以此为准)
pub const TOOL_NAME: &str = "submit";

/// 注册 submit 工具。无外部依赖:平台分派在 services::artifact_submit 内完成。
pub fn register_submit_tool(registry: &ToolRegistry) {
    let definition = ToolDefinition {
        name: TOOL_NAME.into(),
        description: "把最终产物作为文件提交到设备下载位置(仅 Android 沙箱档可用)。\
                      需要交付文件给用户时调用;返回实际落盘位置。"
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "filename": {
                    "type": "string",
                    "description": "文件名(含扩展名,如 报告.md);不含路径,路径分隔符会被拍平"
                },
                "content": {
                    "type": "string",
                    "description": "文件完整内容(上限 8MB;过大请拆分交付)"
                }
            },
            "required": ["filename", "content"]
        }),
    };

    registry.register(
        definition,
        Arc::new(move |args: serde_json::Value, _ctx: ToolContext| {
            Box::pin(async move { run(&args) })
        }),
    );
}

/// 工具主体:参数解析 → 可用性判定 → 提交 → 结果文案。
///
/// 可用性在**执行期**再判一次(而非仅靠下发过滤):用户可能在任务执行途中改了
/// 授权档位(例如把沙箱档关掉、Shizuku 中途生效),那时模型手上的工具清单已过期,
/// 直接拒绝并说明原因,比尝试写文件后报一个语焉不详的系统错误更清楚。
fn run(args: &serde_json::Value) -> Result<String, String> {
    let filename = args
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if filename.is_empty() {
        return Err("filename 不能为空:请给出文件名(如 报告.md)".into());
    }
    if !crate::services::artifact_submit::is_available() {
        return Err(
            "submit 当前不可用:仅 Android 非 root / 非 Shizuku(沙箱档)下提供本工具;\
             其它环境请用 write 工具或应用的导出功能交付产物"
                .into(),
        );
    }
    let location = crate::services::artifact_submit::submit(filename, content)?;
    Ok(format!("已提交产物:{location}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(args: serde_json::Value) -> Result<String, String> {
        run(&args)
    }

    #[test]
    fn registered_with_expected_name_and_schema() {
        let reg = ToolRegistry::new();
        register_submit_tool(&reg);
        let defs = reg.list_definitions();
        let def = defs
            .iter()
            .find(|d| d.name == TOOL_NAME)
            .expect("submit 应已注册");
        assert!(
            def.description.contains("下载"),
            "描述应说明落点在下载位置: {}",
            def.description
        );
        let required = def.parameters["required"]
            .as_array()
            .expect("应声明 required");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"filename"), "filename 应为必填: {names:?}");
        assert!(names.contains(&"content"), "content 应为必填: {names:?}");
    }

    #[test]
    fn missing_filename_rejected() {
        let err = exec(json!({ "content": "正文" })).unwrap_err();
        assert!(err.contains("filename"), "{err}");
    }

    #[test]
    fn availability_checked_before_submit() {
        // 桌面(单测环境)不可用:应在进入提交前给出可操作的错误文案
        let err = exec(json!({ "filename": "a.md", "content": "正文" })).unwrap_err();
        assert!(
            err.contains("沙箱档") || err.contains("不可用"),
            "应说明可用条件: {err}"
        );
    }
}
