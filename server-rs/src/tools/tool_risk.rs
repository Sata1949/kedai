// 内置工具的默认风险级登记表。
//
// 拆出本模块是为了让 `permissions.rs` 保持在 `check-file-size` 的只降不升基线内;
// 同时让元测试可以直接查询「是否显式登记」(见 `tools::registry_meta_tests`)。
use crate::models::tool_policy::ToolRisk;

/// 显式风险级登记表(`None` = 未登记,交由 [`default_risk`] 的通配兜底按危险处理)。
///
/// 拆出本函数是为了让元测试能断言「每个内置工具都被显式登记过」——
/// 只看 `default_risk` 的结果无法区分「显式归危险级」与「漏登记落到兜底」,
/// 而后者正是 4.7 要拦的静默降级(历史上 fs_* 与视觉族都因此被误判过一次)。
pub(crate) fn explicit_risk(tool: &str) -> Option<ToolRisk> {
    let risk = match tool {
        "calculator" | "memory_read" | "read" | "role" | "todo" | "censor_text"
        | "revise_passage" => ToolRisk::Safe,
        // agentend 是子智能体编排收尾(结束任务),不改数据,归敏感级;
        // 若归危险级,任务模式默认策略(拒绝危险工具)会打断子智能体流程。
        "search" | "sleep" | "agentgo" | "agentend" => ToolRisk::Sensitive,
        // 产物提交:目标路径由应用决定(设备下载位置),不接受任意路径参数,文件名经清洗;
        // 归敏感级而非危险级——按危险级登记会被任务模式默认策略整体剔除,
        // 而它恰恰只在任务/沙箱场景下有用(见 tools/submit.rs 顶部说明)。
        "submit" => ToolRisk::Sensitive,
        // 动态调用流程(二维批次 7b):不放宽任何权限——被调流程的节点仍走
        // task_tool_policy 编译 + 节点白名单收窄(与入口流程同一路径),它自身不写文件、
        // 不执行命令,只消耗 token;归敏感级而非危险级,否则「按危险级剔除」的策略会
        // 在对比模式下把它一并剔掉(可见性本就由 META_TOOLS 与逐节点下发决定)。
        "run_flow" => ToolRisk::Sensitive,
        "memory_write" | "update_variables" | "write" | "replace" | "create" => ToolRisk::Dangerous,
        // 世界书词条同步(RPFLOW-2 `worldbook_update`):会改写用户数据(世界书条目 /
        // 角色卡内嵌 character_book),与文件写工具同级归危险级。可见性本就由
        // 「按名单释放」(META_TOOLS + 归档步 ARCHIVE_TOOLS)决定,不看风险级;
        // 显式登记而非依赖下方通配兜底,便于后续审查(同 bash 先例)。
        "worldbook_update" => ToolRisk::Dangerous,
        // 多步变量驱动器元工具(§10.1):两者都由 app_state 单独注册、只进多步工具循环,
        // 不在任何默认下发列表(见 META_TOOLS)。显式登记为 Dangerous 是**保留既有
        // 兜底行为**(改登记不得改变授权结果);get_state 虽只读,但一并放宽会改变
        // 现网授权路径,不在 4.7 的范围内——若后续要按只读口径放行,另立批次。
        "get_state" | "apply_patch" => ToolRisk::Dangerous,
        // 工作区文件工具族(编码通道批次):读/检索三个是安全级(只读,且路径被
        // workspace_guard 收口在工作区内);写/改/补丁三个与 write/replace 同级归危险级
        // ——它们会真的改动工作区里的源码,风险面与既有写工具同类。
        // 归危险级不影响可用性:任务模式默认策略(deny_dangerous)按**工具名**开例外放行
        // 它们(见 services/task_engine/tool_policy.rs),例外的安全前提是路径被 jail 在
        // 工作区内,而不是「工具不危险」。fs_patch 还多一道编码包闸门:关包时在策略层
        // **早于例外**被整族剔除(例外只在开包时生效)。
        "fs_read" | "fs_glob" | "fs_grep" => ToolRisk::Safe,
        // SKY-8:确定性单文件装配会写工作区入口,与 fs_write 同级归危险级;
        // 任务默认策略按工具名例外放行(见 task_engine/tool_policy.rs)。
        "fs_write" | "fs_edit" | "fs_patch" | "fs_inline_html" => ToolRisk::Dangerous,
        // 视觉工具族(2026-10-02 视觉能力包 D4):只读工作区图像、图像输出重定向到
        // DATA_DIR/images,不改工作区、不执行命令 → 安全级(与 fs_read 同档)。
        // 必须显式登记:未登记会落下面的通配兜底被误判 Dangerous,任务默认策略
        // (deny_dangerous)将整族剔除,而它们正是要在默认策略下可用的。
        "view_image" | "zoom_image" | "image_diff" => ToolRisk::Safe,
        // 截图工具(视觉能力包 D5):读屏即读敏感内容(屏幕可能有用户隐私),归敏感级;
        // 不写文件系统、无破坏性动作,归危险级会被任务默认策略整体剔除。可见性另由
        // 「视觉与截图」总开关(默认关)在策略层过滤。
        "screenshot" => ToolRisk::Sensitive,
        // 急停工具(CU-1,2026-10-06):**必须是安全级**——它是收紧动作,任何授权模式下
        // (含严格/等待授权场景)都应能立即生效;若归危险级,任务默认策略(deny_dangerous)
        // 会把它剔掉,恰恰让无人值守任务失去急停能力(与 screenshot 的取舍不同)。
        // 只置位不解除(解除是用户决策,见 tools/stop_control.rs 文件头)。
        "stop_computer_control" => ToolRisk::Safe,
        // 命令执行恒危险级:与文件写工具同级,但额外走「命令级风险强制确认」
        // (tools/command_risk.rs)。显式登记而非依赖下面的通配兜底,便于后续审查。
        "bash" => ToolRisk::Dangerous,
        // 未登记(含用户插件与未来新增工具)由 default_risk 的通配兜底按危险处理。
        _ => return None,
    };
    Some(risk)
}

pub(crate) fn default_risk(tool: &str) -> ToolRisk {
    // 未知工具(包括用户插件)按危险处理,避免新增工具绕过裁决。
    explicit_risk(tool).unwrap_or(ToolRisk::Dangerous)
}
