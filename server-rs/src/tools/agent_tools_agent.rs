// Agent 强化工具集 · agent 域(中层 L3 青层工具域):
//   agentgo   排出子智能体(后台异步生成,read/todo 轮询结果)
//   sleep     等待
//   agentend  结束子智能体任务
//   todo      列出计划表(计划/子任务/工具调用历史)
// 子智能体后台执行(run_subtask)以角色设定 + 常驻世界书(constant 条目)构建子上下文。
use crate::models::types::{GenerationParams, LlmMessage, ToolContext, ToolDefinition};
use crate::tools::registry::ToolRegistry;
use serde_json::{json, Value};
use std::sync::Arc;

use super::agent_tools::ToolDeps;

// ==================== agentgo:排出子智能体(后台异步,read/todo 轮询) ====================
pub(super) fn register_agentgo(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: "agentgo".into(),
            description: "排出子智能体:为每个任务在后台用当前模型独立生成一段内容(带角色设定与常驻世界书上下文)。返回 task_id,后续用 read(type=subtask) 或 todo 轮询结果;用 agentend 结束任务。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "任务名" },
                                "instruction": { "type": "string", "description": "子任务指令" },
                                "max_tokens": { "type": "integer", "description": "输出上限(默认 512)" }
                            },
                            "required": ["name", "instruction"]
                        }
                    }
                },
                "required": ["tasks"]
            }),
        },
        Arc::new(move |args: Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move {
                // 读取调度限制(深度/并发):settings 已在 load 时钳制到合法区间,此处直接使用
                let (max_depth, max_concurrency) = {
                    let s = deps.settings.lock().unwrap_or_else(|e| e.into_inner());
                    (s.subagent_max_depth, s.subagent_max_concurrency)
                };
                // 深度守卫:主 Agent 为 depth 0,子任务内再派发时 +1;
                // 当前子任务生成不带工具(depth 恒 0),守卫为嵌套派发(落地项 3 预留)兜底
                if ctx.agent_depth >= max_depth {
                    return Err(format!(
                        "子智能体嵌套超过 {max_depth} 层,请让主智能体直接处理"
                    ));
                }
                let tasks = args.get("tasks").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                if tasks.is_empty() {
                    return Err("缺少 tasks 参数".into());
                }
                if tasks.len() > 5 {
                    return Err("一次最多排出 5 个子智能体".into());
                }
                // 并发守卫:统计本会话仍在 running 状态的子任务数;
                // 顺序执行(tokio::spawn 后立即返回)下通常不超限,守卫防
                // 多轮循环派发/异常重试把在跑任务堆满,拒绝时不创建新任务
                let running = deps
                    .subtasks
                    .list_by_session(&ctx.session_id)
                    .iter()
                    .filter(|t| t.status == "running")
                    .count();
                if running + tasks.len() > max_concurrency as usize {
                    return Err(format!(
                        "子智能体并发已满 {max_concurrency}(在跑 {running} 个,本次再排 {} 个),请稍后重试或先用 todo 轮询已有任务",
                        tasks.len()
                    ));
                }
                let mut launched: Vec<Value> = Vec::new();
                for t in &tasks {
                    let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let instruction = t.get("instruction").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if name.is_empty() || instruction.is_empty() {
                        return Err("子任务 name 与 instruction 不能为空".into());
                    }
                    let max_tokens = t.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(512).clamp(64, 4096) as u32;
                    let record = deps.subtasks.create(&ctx.session_id, &ctx.character_id, &name, &instruction)?;
                    let deps2 = deps.clone();
                    let task_id = record.id.clone();
                    let session_id = ctx.session_id.clone();
                    let character_id = ctx.character_id.clone();
                    tokio::spawn(async move {
                        run_subtask(deps2, task_id, session_id, character_id, instruction, max_tokens).await;
                    });
                    launched.push(json!({ "task_id": record.id, "name": name, "status": "pending" }));
                }
                Ok(json!({ "ok": true, "tasks": launched }).to_string())
            })
        }),
    );
}

/// 后台子任务执行:构建上下文 → 生成 → 写回结果
async fn run_subtask(
    deps: Arc<ToolDeps>,
    task_id: String,
    session_id: String,
    character_id: String,
    instruction: String,
    max_tokens: u32,
) {
    // 已被 agentend 提前结束 → 不再启动
    if deps.subtasks.is_ended(&task_id) {
        return;
    }
    let _ = deps.subtasks.set_running(&task_id);
    let cancel = deps.subtasks.register_cancel(&task_id);

    // 子上下文:角色设定 + 常驻世界书条目
    let mut sys = String::from("你是主 Agent 排出的子智能体,负责独立完成交给你的子任务。");
    if let Some(c) = deps.characters.get(&character_id) {
        sys.push_str(&format!(
            "\n\n角色「{}」设定:\n{}",
            c.chara_name,
            c.description.trim()
        ));
    }
    let world = collect_constant_world_text(&deps, &character_id);
    if !world.is_empty() {
        sys.push_str(&format!("\n\n常驻世界书设定:\n{world}"));
    }
    sys.push_str("\n\n请直接输出子任务的最终结果(不要模拟对话、不要重复指令)。");
    let messages = vec![
        LlmMessage::plain("system", &sys),
        LlmMessage::plain("user", &instruction),
    ];
    let params = GenerationParams {
        temperature: 0.7,
        top_p: 0.9,
        max_tokens,
        stop: None,
        tools: Vec::new(),
        max_tool_rounds: None,
        tool_choice: crate::models::types::ToolChoice::Auto,
        parallel_tool_calls: None,
    };

    let connector = deps.connector.read().await;
    let res = connector.generate(&messages, params, cancel).await;
    drop(connector);

    match res {
        Ok(chunks) => {
            let mut content = String::new();
            for c in chunks {
                if let crate::models::types::LlmStreamChunk::Token(t) = c {
                    content.push_str(&t);
                }
            }
            if deps.subtasks.is_ended(&task_id) {
                // 生成期间被 agentend 结束:保持 ended,不写结果
                let _ = session_id;
            } else if content.trim().is_empty() {
                let _ = deps.subtasks.set_error(&task_id, "子任务返回空内容");
            } else {
                let _ = deps.subtasks.set_done(&task_id, &truncate_subtask_result(&deps, content.trim()));
            }
        }
        Err(e) => {
            if !deps.subtasks.is_ended(&task_id) {
                let _ = deps.subtasks.set_error(&task_id, &e);
            }
        }
    }
    deps.subtasks.unregister_cancel(&task_id);
}

/// 子任务结果超长截断:超 subagent_result_max_chars 时保留前 N 字符并附尾注,
/// 不静默丢内容(原长写入尾注,调用方可知全貌)。按字符截断,避开 UTF-8 边界问题。
fn truncate_subtask_result(deps: &ToolDeps, content: &str) -> String {
    let max_chars = {
        let s = deps.settings.lock().unwrap_or_else(|e| e.into_inner());
        s.subagent_result_max_chars as usize
    };
    truncate_subtask_result_with_limit(content, max_chars)
}

/// 截断纯函数(限长可注入,测试用)
fn truncate_subtask_result_with_limit(content: &str, max_chars: usize) -> String {
    let total = content.chars().count();
    if total <= max_chars {
        return content.to_string();
    }
    let clipped: String = content.chars().take(max_chars).collect();
    format!("{clipped}\n[子智能体结果已截断,原长 {total} 字符]")
}

/// 测试入口:跨模块(agent_tools tests)验证截断行为
#[cfg(test)]
pub(super) fn truncate_subtask_result_for_test(deps: &ToolDeps, content: &str) -> String {
    truncate_subtask_result(deps, content)
}

/// 常驻世界书文本(子任务上下文用:constant 且启用)
fn collect_constant_world_text(deps: &ToolDeps, character_id: &str) -> String {
    let mut entries: Vec<crate::parsing::world_book::WorldEntry> = Vec::new();
    if let Some(raw) = deps.characters.get(character_id).and_then(|c| c.data_raw) {
        entries.extend(crate::parsing::world_book::character_book_entries(&raw));
    }
    entries.extend(deps.world_books.collect_entries_for_character(character_id));
    entries
        .iter()
        .filter(|e| e.enabled && e.constant && !e.content.trim().is_empty())
        .map(|e| format!("[{}]\n{}", e.comment, e.content.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ==================== sleep:等待 ====================
pub(super) fn register_sleep(registry: &ToolRegistry, _deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: "sleep".into(),
            description: "等待指定毫秒数(上限 60000ms)。需要等待子任务/外部过程完成时使用。".into(),
            parameters: json!({
                "type": "object",
                "properties": { "ms": { "type": "integer", "description": "等待毫秒数(1~60000)" } },
                "required": ["ms"]
            }),
        },
        Arc::new(|args: Value, _ctx: ToolContext| {
            Box::pin(async move {
                let ms = args
                    .get("ms")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    .clamp(1, 60_000) as u64;
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                Ok(json!({ "ok": true, "slept_ms": ms }).to_string())
            })
        }),
    );
}

// ==================== agentend:结束子智能体任务 ====================
pub(super) fn register_agentend(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: "agentend".into(),
            description: "结束(取消)已排出的子智能体任务,传入 task_ids 数组。正在生成的任务会被中断,未开始的不会启动。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "agentgo 返回的 task_id 列表"
                    }
                },
                "required": ["task_ids"]
            }),
        },
        Arc::new(move |args: Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move {
                let ids = args.get("task_ids").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                if ids.is_empty() {
                    return Err("缺少 task_ids 参数".into());
                }
                let mut results: Vec<Value> = Vec::new();
                for id in ids {
                    let id = id.as_str().unwrap_or("").to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let existed = deps.subtasks.get(&id).is_some();
                    let ended = deps.subtasks.end(&id);
                    results.push(json!({ "task_id": id, "existed": existed, "ended": ended }));
                }
                Ok(json!({ "ok": true, "results": results, "session_id": ctx.session_id }).to_string())
            })
        }),
    );
}

// ==================== todo:列出计划表 ====================
pub(super) fn register_todo(registry: &ToolRegistry, deps: Arc<ToolDeps>) {
    registry.register(
        ToolDefinition {
            name: "todo".into(),
            description: "列出当前计划表:Agent 执行计划步骤、全部子智能体任务及其状态(pending/running/done/error/ended)与结果、最近的工具调用历史。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "include_subtasks": { "type": "boolean", "description": "是否包含子任务详情(默认 true)" }
                }
            }),
        },
        Arc::new(move |_args: Value, ctx: ToolContext| {
            let deps = deps.clone();
            Box::pin(async move {
                let mut out = json!({ "ok": true });
                if let Some(agent_session) = deps.agent_sessions.find_by_session(&ctx.session_id) {
                    out["plan"] = json!(agent_session.plan);
                    out["agent_state"] = json!(agent_session.state);
                    out["step_index"] = json!(agent_session.step_index);
                    let calls: Vec<Value> = deps
                        .agent_sessions
                        .list_tool_calls(&agent_session.id)
                        .iter()
                        .rev()
                        .take(20)
                        .map(|c| json!({ "name": c.name, "input": c.input, "output": c.output, "duration_ms": c.duration_ms }))
                        .collect();
                    out["tool_calls"] = json!(calls);
                } else {
                    out["plan"] = json!([]);
                    out["tool_calls"] = json!([]);
                }
                let tasks: Vec<Value> = deps
                    .subtasks
                    .list_by_session(&ctx.session_id)
                    .iter()
                    .map(|t| {
                        json!({
                            "task_id": t.id,
                            "name": t.name,
                            "status": t.status,
                            "instruction": t.instruction,
                            "result": t.result,
                            "error": t.error,
                        })
                    })
                    .collect();
                out["subtasks"] = json!(tasks);
                Ok(out.to_string())
            })
        }),
    );
}
