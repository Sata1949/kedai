//! 工具授权、MCP/执行开关、子代理与工具循环预算(多数为全局扁平字段)。
use crate::services::settings_service::RuntimeSettings;
use crate::tools::permissions::AuthorizationMode;

use super::super::UpdateSettingsBody;
use super::apply_mode;
use super::PatchError;

pub(super) fn apply(
    s: &mut RuntimeSettings,
    body: &UpdateSettingsBody,
    is_task: bool,
) -> Result<(), PatchError> {
    // 授权模式(三档;旧 bypass_mode 仍接受并映射为对应档位)。
    // TM-SET-3:三档/黑名单/等待超时直写扁平(全局字段)——工具授权裁决不经
    // for_mode,写覆盖层是静默死写;任务工具循环同样读全局矩阵(名单外直接拒绝,
    // 不空等),故「两模式共用一份」就是运行期事实,写侧与之对齐。
    if let Some(v) = &body.authorization_mode {
        let parsed = match AuthorizationMode::parse(v.trim()) {
            Some(m) => m,
            None => {
                return Err(PatchError::new(
                    "authorization_mode",
                    "authorization_mode 仅支持 strict / loose / bypass",
                ));
            }
        };
        s.authorization_mode = parsed;
    }
    // 旧字段兼容:bypass_mode=true → bypass,false → strict
    if let Some(v) = body.bypass_mode {
        let mapped = if v {
            AuthorizationMode::Bypass
        } else {
            AuthorizationMode::Strict
        };
        s.authorization_mode = mapped;
    }
    // 「始终需授权」清单:校验工具名存在性,未知名返回 warning(不阻塞保存)
    if let Some(v) = &body.bypass_blacklist {
        s.bypass_blacklist = v.clone();
    }
    // 授权等待超时(秒):30..=1800
    if let Some(v) = body.tool_authorization_timeout_secs {
        if !(30..=1800).contains(&v) {
            return Err(PatchError::new(
                "tool_authorization_timeout_secs",
                "tool_authorization_timeout_secs 必须在 30..=1800",
            ));
        }
        s.tool_authorization_timeout_secs = v;
    }
    // 任务模式工具策略:all / deny_dangerous / allowlist
    if let Some(v) = &body.task_tool_policy {
        if !matches!(v.as_str(), "all" | "deny_dangerous" | "allowlist") {
            return Err(PatchError::new(
                "task_tool_policy",
                "task_tool_policy 仅支持 all / deny_dangerous / allowlist",
            ));
        }
        apply_mode!(s, is_task, task_tool_policy, v.clone());
    }
    if let Some(v) = &body.task_tool_allowlist {
        apply_mode!(s, is_task, task_tool_allowlist, v.clone());
    }
    // 技能渐进披露开关(落地项 3)
    if let Some(v) = body.skill_progressive_disclosure {
        apply_mode!(s, is_task, skill_progressive_disclosure, v);
    }
    // 回退快照开关(批次 6.1)。TM-SET-3:直写扁平(全局字段)——UndoService
    // 直接读基础值,模式覆盖永不生效。
    if let Some(v) = body.undo_enabled {
        s.undo_enabled = v;
    }
    // 子智能体嵌套深度上限(1..=4,越界拒绝)。TM-SET-3:直写扁平(全局字段)——
    // agentgo 工具运行期读全局快照,模式覆盖永不生效。
    if let Some(v) = body.subagent_max_depth {
        if !(1..=4).contains(&v) {
            return Err(PatchError::new(
                "subagent_max_depth",
                "subagent_max_depth 必须在 1..=4",
            ));
        }
        s.subagent_max_depth = v;
    }
    // 子智能体并发上限(1..=16,越界拒绝;同直写扁平口径)
    if let Some(v) = body.subagent_max_concurrency {
        if !(1..=16).contains(&v) {
            return Err(PatchError::new(
                "subagent_max_concurrency",
                "subagent_max_concurrency 必须在 1..=16",
            ));
        }
        s.subagent_max_concurrency = v;
    }
    // 子智能体结果字符上限(500..=8000,越界拒绝;同直写扁平口径)
    if let Some(v) = body.subagent_result_max_chars {
        if !(500..=8000).contains(&v) {
            return Err(PatchError::new(
                "subagent_result_max_chars",
                "subagent_result_max_chars 必须在 500..=8000",
            ));
        }
        s.subagent_result_max_chars = v;
    }
    // MCP 总开关(批次 6.2;PLGM 3.4 起**直写扁平**):MCP 是进程级全局能力
    // (装配与 restart 端点读扁平权威值),不走模式覆盖层——原 `apply!` 的覆盖语义
    // 只会制造「看似可配、实际无效」的字段,DC-2/CFG-2 收口时删除。
    // 运行期改动经 POST /api/mcp/servers/{name}/restart 一键生效。
    if let Some(v) = body.mcp_enabled {
        s.mcp_enabled = v;
    }
    // MCP 服务器列表(全量替换):卫生清理同 load(trim 名称/命令,丢弃不可用条目)
    if let Some(v) = &body.mcp_servers {
        let mut servers = v.clone();
        servers.retain_mut(|srv| {
            srv.name = srv.name.trim().to_string();
            srv.command = srv.command.trim().to_string();
            !srv.name.is_empty() && !srv.command.is_empty()
        });
        s.mcp_servers = servers;
    }
    // 命令执行开关(阶段 E):bool 免校验。这些是**全局**能力开关,
    // 不随 roleplay/task 覆盖层分叉(exec: 权限是进程级事实,不是模式偏好),
    // 故直接写扁平字段而非走 apply!(is_task,...)。
    if let Some(v) = body.exec_enabled {
        s.exec_enabled = v;
    }
    if let Some(v) = body.exec_allow_root {
        s.exec_allow_root = v;
    }
    if let Some(v) = body.exec_allow_shizuku {
        s.exec_allow_shizuku = v;
    }
    if let Some(v) = body.exec_allow_sandbox {
        s.exec_allow_sandbox = v;
    }
    // 视觉与截图总开关(视觉能力包 D5):全局能力开关,直写扁平(口径同 exec_*)。
    if let Some(v) = body.vision_screenshot_enabled {
        s.vision_screenshot_enabled = v;
    }
    // 工具历史回灌上限(R3b):扁平全局字段(引擎 run_tool_loop 直读扁平值,
    // 不入模式覆盖层——任务/聊天工具循环共用同一上限,与 subagent 参数的
    // 引擎侧消费口径一致);越界拒绝,与 load 钳制区间一致
    if let Some(v) = body.tool_history_keep_rounds {
        if !(1..=32).contains(&v) {
            return Err(PatchError::new(
                "tool_history_keep_rounds",
                "tool_history_keep_rounds 必须在 1..=32",
            ));
        }
        s.tool_history_keep_rounds = v;
    }
    if let Some(v) = body.tool_history_budget_tokens {
        if v != 0 && !(1024..=1_048_576).contains(&v) {
            return Err(PatchError::new(
                "tool_history_budget_tokens",
                "tool_history_budget_tokens 须为 0(禁用)或 1024..=1048576",
            ));
        }
        s.tool_history_budget_tokens = v;
    }
    // HB-1 成本护栏:扁平全局字段(引擎 run_tool_loop 直读扁平值,与工具历史
    // 预算同款——聊天与任务工具循环共用同一上限);越界拒绝,与 load 钳制同区间
    if let Some(v) = body.session_token_budget {
        if v != 0 && !(1024..=1_000_000_000).contains(&v) {
            return Err(PatchError::new(
                "session_token_budget",
                "session_token_budget 须为 0(关闭)或 1024..=1000000000",
            ));
        }
        s.session_token_budget = v;
    }
    if let Some(v) = &body.session_budget_action {
        if !matches!(v.as_str(), "warn" | "stop") {
            return Err(PatchError::new(
                "session_budget_action",
                "session_budget_action 须为 warn 或 stop",
            ));
        }
        s.session_budget_action = v.clone();
    }
    // HB-2 语义熔断参数(扁平全局:引擎 run_tool_loop 直读);越界拒绝,与 load 同区间
    if let Some(v) = body.loop_guard_semantic_window {
        if !(4..=64).contains(&v) {
            return Err(PatchError::new(
                "loop_guard_semantic_window",
                "loop_guard_semantic_window 必须在 4..=64",
            ));
        }
        s.loop_guard_semantic_window = v;
    }
    if let Some(v) = body.loop_guard_semantic_min_calls {
        if v != 0 && !(4..=64).contains(&v) {
            return Err(PatchError::new(
                "loop_guard_semantic_min_calls",
                "loop_guard_semantic_min_calls 须为 0(关闭)或 4..=64",
            ));
        }
        s.loop_guard_semantic_min_calls = v;
    }
    if let Some(v) = body.loop_guard_semantic_max_distinct {
        if !(1..=8).contains(&v) {
            return Err(PatchError::new(
                "loop_guard_semantic_max_distinct",
                "loop_guard_semantic_max_distinct 必须在 1..=8",
            ));
        }
        s.loop_guard_semantic_max_distinct = v;
    }
    Ok(())
}
