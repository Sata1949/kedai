// 工具系统:注册表 + 内置工具(calculator / censor / memory / agent 强化工具集)
//
// 代际: L2(中层·干 / Orchestration)——**2026-09-14 由 L3 修正为 L2**。
// 判据: ① P3 隔离性不满足——本目录内无沙箱/无设置开关/无实验隔离,注册表是 L2 骨干设施;
//       ② P5 复用度命中——被 agents/services/api 共 32 处复用,属核心必需设施
//       (它失败则整个应用不可用,与 L3「失败必须被隔离」语义相反)。
// 纪律: 可依赖 L1/L2;不得依赖 L3(scripts/mcp/plugins/exec)与 entry。
// 真 L3 隔离能力在相邻模块:scripts/(rquickjs 沙箱)、mcp/(默认关)、plugins/(受控求值器)、
// services/exec/(默认关)。详见 docs/契约-架构与数据.md §2.5 末段与晋升台账。
pub mod action_class;
pub mod agent_tools;
pub mod bash;
// agent 强化工具集拆分(按功能域分文件;agent_tools.rs 为聚合入口)。
// 代际归属见本文件头部:tools/ 整体为 L2(2026-09-14 修正),不再是「青层工具域」。
mod agent_tools_agent;
// 工作区文件工具族(编码通道批次):工具**定义**在此,可见性由任务工具策略按
// 「本任务是否绑定工作区」过滤(见 task_engine/tool_policy.rs 与 tool_sets::exclude_workspace)
pub mod agent_tools_fs;
pub(crate) mod agent_tools_fs_patch;
mod agent_tools_read;
mod agent_tools_search;
mod agent_tools_shared;
mod agent_tools_write;
pub mod calculator;
pub mod censor;
pub mod command_risk;
pub mod memory;
pub mod multistep;
pub mod permissions;
pub mod registry;
pub mod revise;
// 截图工具(视觉能力包 D5;Windows 原生 GDI / 非 Windows 明示不支持):
// 可见性由「视觉与截图」总开关过滤(见 tool_policy::screenshot_gate 与 tool_sets::filter_screenshot)
pub mod screenshot;
// 急停工具(CU-1,2026-10-06):Agent 可调的自停开关,与前端「停止操作电脑」按钮写同一
// 后端状态(services::computer_use::ComputerUseControl);始终注册且始终可见(风险级 Safe)
pub mod stop_control;
// 产物提交(仅 Android 沙箱档下发;可见性过滤见 task_engine/tool_policy.rs)
pub mod submit;
// 动态调用名单内流程(二维批次 7b):工具**定义**在此,「谁可被调用」由 custom 执行器
// 逐节点下发描述表达;名单/预算/环与深度守卫在 task_engine/flow_call.rs
pub mod run_flow;
pub mod tool_sets;
pub mod variables;
// 视觉工具三件(视觉能力包 D4):工具**定义**在此,可见性 = 工作区族闸门 ∩ 视觉闸门
// (见 task_engine/tool_policy.rs 的 workspace_gate / vision_gate)
pub mod vision_tools;
// 世界书词条同步(RPFLOW-2):按名单释放的元工具(deep/agent 归档步经 ARCHIVE_TOOLS 下发,
// 见 tool_sets.rs);**不进默认工具列表**,避免正文生成轮被诱导改写用户的世界书设定
pub mod worldbook;
// 工作区路径闸门(编码通道批次,本批安全核心):fs_* 工具与 bash 的 cwd 校验共用;
// 创建期的工作区校验也复用其判据(见 agent_tools_fs.rs / bash.rs / api/tasks.rs)
pub mod workspace_guard;
// 工作区树扫描(批次 4b):bash 侧命令前后各扫一次,供文件变更台账做启发式检出;
// 遍历实现同时被 agent_tools_fs::walk_files(fs_glob/fs_grep)复用
mod workspace_scan;
// 遍历忽略集的**单一出处**(`.git`/`target`/`node_modules`/`dist`/`.kedai-index`/`data`):
// 工作区画像探测(services/workspace_profile)沿用同一份,不复制第二份(复制必然漂移)
use agent_tools::ToolDeps;
use registry::ToolRegistry;
use std::sync::Arc;
pub(crate) use workspace_scan::EXCLUDED_SEGMENTS;

/// 启动时注册全部内置工具(calculator / censor_text / memory_read / memory_write / agent 强化工具集)
pub fn register_builtin_tools(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    // calculator
    registry.register(
        crate::models::types::ToolDefinition {
            name: "calculator".into(),
            description: "执行四则运算(白名单解析,安全)".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "expression": { "type": "string", "description": "如 12*34" } },
                "required": ["expression"]
            }),
        },
        Arc::new(
            |args: serde_json::Value, _ctx: crate::models::types::ToolContext| {
                Box::pin(async move {
                    let expr = args
                        .get("expression")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let result = calculator::calculate(&expr)?;
                    Ok(result.to_string())
                })
            },
        ),
    );
    memory::register_memory_tools(registry, deps.sessions.clone(), deps.memory.clone());
    variables::register_update_variables_tool(registry, deps.sessions.clone());
    censor::register_censor_tool(registry);
    revise::register_revise_passage_tool(registry);
    // 命令执行(阶段 B):危险工具,任务模式默认策略不下发;授权与命令级风险确认见
    // tools/command_risk.rs 与 tools/permissions.rs(破坏性/提权命令任何模式都不自动放行)。
    bash::register_bash_tool(
        registry,
        deps.db.clone(),
        deps.settings.clone(),
        deps.data_dir.clone(),
    );
    // 产物提交(仅 Android 沙箱档):始终注册,可见性由任务工具策略按平台+档位过滤
    submit::register_submit_tool(registry);
    // 动态调用流程(二维批次 7b):始终注册但**永不进正文列表**(见 tool_sets::META_TOOLS),
    // 只由 custom 执行器在对比模式的宽松节点上显式下发
    run_flow::register_run_flow_tool(registry);
    // 工作区文件工具族(编码通道批次):始终注册,可见性由任务工具策略按工作区绑定过滤
    // (未绑定任务与聊天路径都看不到它们——调了必然报「未绑定工作区」)
    agent_tools_fs::register_fs_tools(registry, deps.clone());
    // 视觉工具三件(视觉能力包 D4):始终注册;可见性 = 工作区族闸门 ∩ 视觉能力位闸门
    vision_tools::register_vision_tools(registry, deps.clone());
    // 截图工具(视觉能力包 D5):始终注册;可见性由「视觉与截图」总开关过滤(默认关)
    screenshot::register_screenshot_tool(registry, deps.clone());
    // 急停工具(CU-1):始终注册且始终可见(不随截图开关隐藏——「停止操作电脑」任何时刻都要有出口)
    stop_control::register_stop_control_tool(registry, deps.clone());
    // 世界书词条同步(RPFLOW-2 归档步):始终注册,可见性由 `tool_sets::META_TOOLS` 与
    // 归档名单(ARCHIVE_TOOLS)决定——默认不下发,只由归档步显式下发
    worldbook::register_worldbook_tool(registry, deps.clone());
    agent_tools::register_agent_tools(registry, deps);
}
