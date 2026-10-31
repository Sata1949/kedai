// 工具元数据:新增工具指南(「何时调用」段)与前端渲染意图。
//
// 纯静态映射,不依赖注册表状态——拆出本模块是为了让 `registry.rs` 保持在
// `check-file-size` 的只降不升基线内(计划 4.7 补登记时新增了条目)。
/// 新增工具指南(「何时调用」段)。仅供 [`tools::registry_meta_tests`]
/// 遍历注册表断言每个内置工具都有指南(计划 4.7 的漏改拦截)。
#[cfg(test)]
pub(crate) fn tool_when_for_test(name: &str) -> String {
    tool_when(name)
}

pub(crate) fn tool_when(name: &str) -> String {
    let s = match name {
        "read" => "需要读取世界书条目/角色提示词/技能库/角色文件/子任务结果时调用。",
        "role" => "需要随机数或掷骰(决定走向、概率事件、生成随机值)时调用。",
        "write" => "需要把内容写入当前对话气泡或角色文件区文件时调用。",
        "replace" => "需要修改对话气泡或角色文件区的已有内容(追加/插入/替换/删除)时调用。",
        "create" => "需要在角色文件区新建文件夹或文件时调用。",
        "search" => "需要联网查询最新信息、资料或实时内容时调用。",
        "todo" => "需要查看当前执行计划、子智能体任务状态或最近工具调用历史时调用。",
        "agentgo" => "需要把可并行的子任务(如多段资料整理、分头起草)交给子智能体后台执行时调用,返回 task_id 供后续 read/todo 轮询。",
        "agentend" => "需要取消或结束已派出的子智能体任务时调用(传入 task_ids)。",
        "sleep" => "需要等待子任务或外部过程完成时调用(毫秒,上限 60000)。",
        "calculator" => "需要进行四则运算或数值计算时调用。",
        "censor_text" => "需要把文本中的禁词替换为更得体表达时调用。",
        "memory_read" => "需要读取本会话长期记忆中的既有记录时调用。",
        "memory_write" => "需要把值得长期记住的信息写入本会话记忆时调用。",
        "update_variables" => "需要更新当前会话的 stat_data 变量树时调用。",
        "submit" => "需要把最终产物作为文件交付给用户时调用(写入设备下载位置,返回落盘路径)。",
        "run_flow" => "需要整套预置流程(而非单个工具)来完成一个子目标时调用;可用流程由本任务名单决定,成果作为本次调用结果返回。",
        // 工作区文件工具族(编码通道批次;仅任务绑定工作区时下发)
        "fs_read" => "需要读取本任务工作区里的文件内容时调用(带行号、可分页);改文件前先用它读一遍。",
        "fs_write" => "需要新建或整体重写工作区里的文件时调用;覆盖已有文件前必须先 fs_read(防盲写)。",
        "fs_edit" => "需要定点修改工作区文件里的一段文本时调用(比 fs_write 省 token);先 fs_read,且原文需能唯一匹配。",
        "fs_glob" => "需要按通配模式(如 **/*.rs)列出工作区里的文件、确认路径拼写时调用。",
        "fs_grep" => "需要按正则在工作区里检索文本、定位到具体文件与行号时调用。",
        "fs_patch" => "需要一次改动多个文件(批量重构 / 重命名 / 跨文件同步)时调用;以 *** Update File 等分块的结构化补丁给出,每段旧内容需在文件中唯一匹配,改前先 fs_read。",
        "fs_inline_html" => "需要把多个分片装配成一个自包含 HTML 时调用;引擎读分片、替换 @@NAME@@ 槽位并写入口,避免手写大段内联脚本。",
        // 视觉工具族(视觉能力包 D4;仅工作区任务且连接开启「视觉输入」时下发)
        "view_image" => "需要真的「看」工作区里的图像(截图/设计稿/照片)内容时调用;直接把图像交给模型查看,返回图像本身与尺寸信息。",
        "zoom_image" => "需要放大查看图像局部细节(小字/图标/某块区域)时调用;给出原图像素坐标裁剪并放大,看不清细节先用它。",
        "image_diff" => "需要对比两张图像(改版前后截图等)的差异时调用;返回差异像素统计与可视化差异图。",
        // 截图工具(视觉能力包 D5;仅「视觉与截图」开关开启时下发)
        "screenshot" => "需要查看当前屏幕画面时调用(全屏 / 指定显示器 / 区域 / 窗口标题);用于读屏视觉验证(UI 检查、改版前后对比)。",
        // 急停工具(CU-1;始终下发)
        "stop_computer_control" => "用户要求你停止查看/操作其电脑,或你察觉当前屏幕读取并非用户所愿时调用;置位后电脑操作类工具(当前含截图)在用户恢复前一律拒绝执行。",
        // 多步变量驱动器元工具(§10.1):只在多步工具循环里出现,不进正文默认列表
        "get_state" => "需要查看契约裁剪后的当前状态树(便于决定下一步补丁)时调用。",
        "apply_patch" => "需要按契约规则批量更新状态(number replace/delta、remove、move)时调用;修改前先 get_state 确认现状。",
        // 命令执行(阶段 B):危险工具,任务模式按名例外放行,危险命令另走命令级确认
        "bash" => "需要在用户机器上执行 shell 命令(构建、测试、脚本)时调用;工作目录受工作区闸门约束,破坏性/提权命令需显式授权。",
        // 定点修订(反思阶段工具)
        "revise_passage" => "需要按指定要求重写正文中的某一段落时调用(只改目标段落,不动其余内容)。",
        // 世界书词条同步(RPFLOW-2 归档步;仅按名单释放)
        "worldbook_update" => "需要把本轮剧情产生的新设定写回世界书词条时调用(归档步专用,由流程显式下发)。",
        _ => return String::new(),
    };
    s.to_string()
}

/// 内置工具的展示意图(render intent),供 SSE ToolCall/ToolResult 透传、前端分派渲染。
/// 取值:read(读文件/条目,代码块展示)、search(搜索结果,列表化)、image(视觉工具,
/// 缩略图渲染)、generic(其余通用)。未知名(脚本插件工具)返回 None,前端回退 JSON 直出。
pub fn render_kind_for(name: &str) -> Option<&'static str> {
    match name {
        "read" | "fs_read" | "fs_grep" => Some("read"),
        "search" => Some("search"),
        // 视觉工具族(视觉能力包 D4):前端按 image 分派渲染缩略图(输出含 images 引用);
        // 截图工具(D5)同款
        "view_image" | "zoom_image" | "image_diff" | "screenshot" => Some("image"),
        "calculator" | "role" | "write" | "replace" | "create" | "todo" | "agentgo"
        | "agentend" | "sleep" | "censor_text" | "memory_read" | "memory_write"
        | "update_variables" | "get_state" | "apply_patch" | "submit" | "run_flow"
        // 工作区写/列族(编码通道批次)走通用渲染
        | "fs_write" | "fs_edit" | "fs_glob" | "fs_patch" | "fs_inline_html"
        // 急停工具(CU-1)走通用渲染
        | "stop_computer_control"
        // 命令执行 / 定点修订 / 世界书同步:无专用渲染面,走通用。
        // 这三处是 4.7 元测试首轮抓出的真实漏登记(render_kind 缺失会让前端退化成
        // JSON 直出),补登记不改变任何既有渲染结果——此前它们同样落到 None 分支。
        | "bash" | "revise_passage" | "worldbook_update" => Some("generic"),
        _ => None,
    }
}
