//! 主 Agent 提示词读写与分层预览端点。
use crate::api::app_state::AppState;
use crate::api::json_body::JsonBody;
use crate::api::{internal, not_found, validation};
use crate::services::pov::{parse_pov_mode, pov_anchor};
use crate::services::settings_service::AppMode;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// GET /api/settings/agent-prompt:读取 DATA_DIR 中的运行时主 Agent 提示词。
/// 新文件缺失时由共享文件服务兼容读取/迁移旧 AGENTS_RUNTIME.md。
pub async fn get_agent_prompt(State(state): State<Arc<AppState>>) -> Response {
    let path = state.runtime_prompt.path().display().to_string();
    match state.runtime_prompt.read() {
        Ok(content) => {
            Json(json!({ "ok": true, "path": path, "content": content })).into_response()
        }
        Err(e) => not_found(e),
    }
}

#[derive(Deserialize)]
pub struct SaveAgentPromptBody {
    pub content: String,
}

#[derive(Deserialize, Default)]
pub struct PromptPreviewQuery {
    pub session_id: Option<String>,
    pub character_id: Option<String>,
    /// 预览按哪个模式的合并设置:roleplay(缺省,兼容旧客户端)| task
    pub mode: Option<String>,
}

#[derive(Serialize)]
struct PromptPreviewLayer {
    source: String,
    role: String,
    layer: u8,
    order: usize,
    content: String,
}

/// 预览响应的线格式载体(WB-E 批 2,遗留 L31):此前响应由 `json!` 手工拼,字段集合与
/// `web/src/api/types.ts` 的 `PromptPreview` 无机器比对——后端加字段、前端漏改不报错。
/// 改为结构体后即可登记进 `tools/check-contract.mjs`(该脚本按 `pub struct` 取字段集合)。
/// `warnings` 恒下发数组(空集也带),内容是**结构化码**,中文文案在
/// `web/src/utils/promptWarnings.ts` 单一出处映射。
#[derive(Serialize)]
pub struct PromptPreviewResponse {
    ok: bool,
    note: &'static str,
    layers: Vec<PromptPreviewLayer>,
    warnings: Vec<&'static str>,
}

fn push_preview_layer(
    layers: &mut Vec<PromptPreviewLayer>,
    source: impl Into<String>,
    role: impl Into<String>,
    layer: u8,
    content: impl Into<String>,
) {
    let content = content.into();
    if content.trim().is_empty() {
        return;
    }
    layers.push(PromptPreviewLayer {
        source: source.into(),
        role: role.into(),
        layer,
        order: layers.len(),
        content,
    });
}

fn stable_history_hash(text: &str) -> String {
    // 非加密用途：只用于预览中识别“内容是否变化”，不返回原文。
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// GET /api/settings/prompt-preview：按最终注入顺序输出来源层。
/// 历史层仅含 role/字符长度/稳定哈希，绝不返回聊天正文或连接密钥。
pub async fn prompt_preview(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PromptPreviewQuery>,
) -> Response {
    let mut layers = Vec::new();
    // WB-E 提示层(遗留 L31):告警以**结构化码**输出,中文文案在前端单一出处映射
    // (`web/src/utils/promptWarnings.ts`),避免提示词面文案在两侧分叉。
    let mut warnings: Vec<&'static str> = Vec::new();
    // 角色卡里是否有「会进 system 的常驻条目」:None = 本次预览没带角色卡上下文(无从判定)。
    let mut constant_system_entry: Option<bool> = None;
    match state.runtime_prompt.read_optional() {
        Ok(Some(runtime)) => {
            push_preview_layer(&mut layers, "runtime_prompt", "system", 5, runtime)
        }
        Ok(None) => {}
        Err(error) => return internal(error),
    }

    // 预览按模式走 for_mode 合并值(docs/契约-协议与配置.md 第五节):
    // 缺省/未知值按 roleplay(旧客户端零变化);task 为覆盖层合并后的有效设置,
    // agent_system_prompt 经类型级隔离转换(None 已注入内置任务默认词)。
    let mode = match query.mode.as_deref() {
        Some("task") => AppMode::Task,
        _ => AppMode::Roleplay,
    };
    // 设置快照:不留锁跨 await(for_mode 为纯计算,锁在 snapshot 内即释放)
    let settings = state.settings_snapshot().for_mode(mode);
    // 文学能力包(LIT-3):两段值取自设置层方法(开关判定 + 文本单一出处)。此处先取好——
    // 下方若干 push 会按值移动 settings 的字段(如 reflect_advice_prompt),之后再借用会编译错。
    let literary_tail = settings.literary_system_tail();
    let literary_note = settings.literary_user_note();
    // 文学能力包(LIT-6):位置 0 文风素材段(开关关或预设为空 → None)
    let literary_style = settings.literary_style_note();
    // 自定义模板缺 `{{world_info}}` → 角色卡常驻条目按「Custom Prompt 完整替换」契约不注入
    // (引擎状态块除外)。判据与引擎侧 `messages/build.rs` 的 `has_world_placeholder` 同一占位符字面;
    // 契约不改,只补可见性。任务模式不适用(世界书由引擎拼装,不经模板占位符)。
    let template_missing_world_info = matches!(mode, AppMode::Roleplay)
        && !settings.agent_system_prompt.0.trim().is_empty()
        && !settings.agent_system_prompt.0.contains("{{world_info}}");

    // agent_system_prompt 为 RoleplayPromptConfig(WP7),.0 取字符串
    if settings.agent_system_prompt.0.trim().is_empty() {
        // 空值回退文案按模式区分:roleplay 空 = 用内置人设模板;
        // task 走到这里 = 覆盖层 Some("") 显式清空(None 已被 for_mode 注入任务默认词)
        let fallback = match mode {
            AppMode::Task => "任务模式 Agent 系统提示词已显式清空,执行时仅注入下方三层固定提示词",
            AppMode::Roleplay => "内置默认角色扮演 / 文学创作模板（运行时按角色展开）",
        };
        push_preview_layer(&mut layers, "default_template", "system", 5, fallback);
    } else {
        push_preview_layer(
            &mut layers,
            "custom_template",
            "system",
            5,
            settings.agent_system_prompt.0.clone(),
        );
    }

    // task 模式追加规划器/执行者/汇总者三层固定提示词(单一来源 task_service/prompt.rs)
    if matches!(mode, AppMode::Task) {
        use crate::services::task_service::prompt as task_prompts;
        push_preview_layer(
            &mut layers,
            "task_planner_prompt",
            "system",
            5,
            task_prompts::PLANNER_PROMPT,
        );
        push_preview_layer(
            &mut layers,
            "task_executor_prompt",
            "system",
            5,
            task_prompts::EXECUTOR_PROMPT,
        );
        // 执行者工具纪律段(提交 3 · D3-c):**条件注入**——只有本轮真的下发了工具时
        // 才追加在 EXECUTOR_PROMPT 之后(legacy 的步骤没有工具,不发这一段)。
        // 预览无「本轮有没有工具」的概念,故按「工具档」形态展示;口径写进
        // docs/契约-协议与配置.md 第五节,避免读者以为它恒在。
        push_preview_layer(
            &mut layers,
            "task_executor_tool_discipline",
            "system",
            5,
            task_prompts::EXECUTOR_TOOL_DISCIPLINE,
        );
        // 写盘优先纪律段(TMR-1):同样是**条件注入**——仅当本轮实际下发工作区写工具
        // (`fs_write`/`fs_edit`/`fs_patch`)时追加。预览按「工具档」形态展示,与上一条
        // 同口径(见 docs/契约-协议与配置.md 第五节)。
        push_preview_layer(
            &mut layers,
            "task_executor_write_first_discipline",
            "system",
            5,
            task_prompts::EXECUTOR_WRITE_FIRST_DISCIPLINE,
        );
        // 视觉验证纪律段(视觉能力包 D4;修复批次扩充到「任一图像工具」):同样是
        // **条件注入**——仅当本轮工具面含图像工具(视觉三件 ∪ screenshot)时追加在
        // 工具纪律之后。预览按「工具档」形态展示(与上一条同口径)。
        push_preview_layer(
            &mut layers,
            "task_executor_vision_discipline",
            "system",
            5,
            task_prompts::VISION_VERIFY_DISCIPLINE,
        );
        push_preview_layer(
            &mut layers,
            "task_summarizer_prompt",
            "system",
            5,
            task_prompts::SUMMARIZER_PROMPT,
        );
    }

    // 任务模式注入**恒隔离**(TM-SET-2):task 模式一律不推送注入层——预览必须与真实
    // 下发一致(docs/契约-协议与配置.md 第五节)。该行为原由 task_prompt_inject_enabled
    // 门控(2026-09-10 F2 实跑修复的默认侧);开关退役后固定为「不注入」,无恢复通道。
    let inject_gated = matches!(mode, AppMode::Task);
    if !inject_gated {
        let inject = state
            .prompt_inject
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get()
            .clone();
        match inject.mode {
            crate::services::prompt_inject_service::InjectMode::Simple => push_preview_layer(
                &mut layers,
                "simple_inject",
                "system",
                4,
                inject.simple_inject_text(),
            ),
            crate::services::prompt_inject_service::InjectMode::Complex => {
                for floor in inject.enabled_floors_sorted() {
                    push_preview_layer(
                        &mut layers,
                        format!("complex_floor:{}", floor.name),
                        floor.role.as_str(),
                        4,
                        floor.content.clone(),
                    );
                }
            }
        }
    }

    // 文学能力包(LIT-3):位置 4(system 尾)增强段——在注入层之后(真实下发顺序:
    // 简单注入/楼层 → 禁词提示 → 文学增强段 → system 压入消息)。判据与文本单一出处
    // 在设置层方法;角色扮演侧专属(task 模式不经引擎装配点,不推送)。
    if matches!(mode, AppMode::Roleplay) {
        if let Some(tail) = literary_tail {
            push_preview_layer(&mut layers, "literary_enhancement", "system", 4, tail);
        }
    }

    // POV-1:角色卡上下文提前读取——锚定文案需要角色名,而锚定层必须排在角色卡层之前;
    // 推送顺序保持 runtime → 模板 → 注入 → 文学 → 锚定 → 角色卡 → 世界书。
    let (preview_character, preview_entries) = match query.character_id.as_deref() {
        Some(character_id) => {
            // 角色卡 + 世界书条目读取(同步 SQLite)合并进同一阻塞任务(DB 并发改造)
            let cid = character_id.to_string();
            let characters = state.characters.clone();
            let world_books = state.world_books.clone();
            let (character, wb_entries) = state
                .db_call(move || {
                    let character = characters.get(&cid);
                    let entries = character
                        .as_ref()
                        .map(|_| world_books.collect_entries_for_character(&cid))
                        .unwrap_or_default();
                    (character, entries)
                })
                .await
                .unwrap_or((None, Vec::new()));
            match character {
                Some(character) => {
                    let mut entries = character
                        .data_raw
                        .as_ref()
                        .map(crate::parsing::world_book::character_book_entries)
                        .unwrap_or_default();
                    entries.extend(wb_entries);
                    (Some(character), entries)
                }
                None => (None, Vec::new()),
            }
        }
        None => (None, Vec::new()),
    };

    // 人称锚定(POV-1):与引擎 build 层同判据、同 helper;简单/复杂模式都展示。
    // 未知视角值同时给结构化告警码(前端文案映射在 utils/promptWarnings.ts)。
    if matches!(mode, AppMode::Roleplay) {
        let pov_inject = state
            .prompt_inject
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get()
            .clone();
        let pov_mode = if pov_inject.simple.perspective_enabled {
            parse_pov_mode(&pov_inject.simple.perspective)
        } else {
            None
        };
        if pov_inject.simple.perspective_enabled && pov_mode.is_none() {
            warnings.push("perspective_unknown");
        }
        let char_name = preview_character
            .as_ref()
            .map(|c| c.chara_name.as_str())
            .unwrap_or("角色");
        push_preview_layer(
            &mut layers,
            "pov_anchor",
            "system",
            4,
            pov_anchor(pov_mode, char_name, None),
        );
    }

    // 角色卡层(位置 3):仅在带角色卡上下文时展示。
    if let Some(character) = &preview_character {
        // 常驻条目中真正会被 {{world_info}} 带走的子集(role 缺省即 system,与引擎 sys_world 同判据)
        constant_system_entry = Some(preview_entries.iter().any(|entry| {
            entry.enabled && entry.constant && entry.role.as_deref().unwrap_or("system") == "system"
        }));
        push_preview_layer(
            &mut layers,
            "character_card",
            "system",
            3,
            format!(
                "角色名：{}\n角色描述：{}",
                character.chara_name, character.description
            ),
        );
        let summary = preview_entries
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| {
                format!(
                    "[{}] kind={} role={} length={} hash={}",
                    entry.comment,
                    if entry.constant {
                        "constant"
                    } else {
                        "triggered"
                    },
                    entry
                        .role
                        .as_deref()
                        .unwrap_or(if entry.constant { "system" } else { "user" }),
                    entry.content.chars().count(),
                    stable_history_hash(&entry.content),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        push_preview_layer(&mut layers, "world_book", "metadata", 3, summary);
    }

    // 带角色卡上下文时以「确有常驻条目会被静默丢弃」为准,避免在全激发卡上报无信号的警;
    // 裸预览(设置页不选角色)无从判定条目集合,保守告警。
    if template_missing_world_info && constant_system_entry.unwrap_or(true) {
        warnings.push("template_missing_world_info");
    }

    if let Some(session_id) = query.session_id.as_deref() {
        // 历史摘要读取(同步 SQLite)挪进阻塞线程池(DB 并发改造)
        let sessions = state.sessions.clone();
        let sid = session_id.to_string();
        let history = state
            .db_call(move || sessions.get_messages(&sid))
            .await
            .unwrap_or_default();
        let summary = history
            .iter()
            .map(|message| {
                format!(
                    "role={} length={} hash={}",
                    message.role,
                    message.content.chars().count(),
                    stable_history_hash(&message.content)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        push_preview_layer(&mut layers, "history_summary", "metadata", 2, summary);
    }

    if let Some(flow) = state
        .flow
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get()
        .filter(|flow| flow.enabled)
    {
        for (index, step) in flow.steps.iter().filter(|step| step.enabled).enumerate() {
            if let Some(prompt) = step
                .system_prompt
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            {
                push_preview_layer(
                    &mut layers,
                    format!("step_instruction:{}", index + 1),
                    "system",
                    0,
                    prompt,
                );
            }
        }
    }

    if !settings.reflect_prompt.trim().is_empty() {
        push_preview_layer(
            &mut layers,
            "reflection",
            "system",
            0,
            settings.reflect_prompt,
        );
    }
    if !settings.reflect_advice_prompt.trim().is_empty() {
        push_preview_layer(
            &mut layers,
            "reflection_advice",
            settings.reflect_advice_role,
            0,
            settings.reflect_advice_prompt,
        );
    }
    // 文学能力包(LIT-3/LIT-6):位置 0 文风段与 Author's Note 段——都在反思建议之后、
    // 预设尾部之前,且**文风段在 AN 段之前**(与真实拼装同序);随尾部角色
    // (preset_tail_role,与真实下发同判据);按 untrusted_boundary 包裹展示
    // (「预览即真实下发」:真实拼装侧即按此包裹)。
    if matches!(mode, AppMode::Roleplay) {
        if let Some(style) = literary_style {
            push_preview_layer(
                &mut layers,
                "literary_style",
                settings.preset_tail_role.clone(),
                0,
                crate::services::prompt_kit::untrusted_boundary("literary_style", style),
            );
        }
        if let Some(note) = literary_note {
            push_preview_layer(
                &mut layers,
                "literary_note",
                settings.preset_tail_role.clone(),
                0,
                crate::services::prompt_kit::untrusted_boundary("literary_note", note),
            );
        }
    }
    if !settings.preset_tail_prompt.trim().is_empty() {
        push_preview_layer(
            &mut layers,
            "preset_tail",
            settings.preset_tail_role,
            0,
            settings.preset_tail_prompt,
        );
    }

    Json(PromptPreviewResponse {
        ok: true,
        note: "历史仅显示角色、长度与哈希；预览不包含聊天正文或 API Key。步骤指令与工具指南在具体运行步骤确定后追加。",
        layers,
        warnings,
    })
    .into_response()
}

/// PUT /api/settings/agent-prompt:原子保存主 Agent 提示词到 DATA_DIR。
pub async fn save_agent_prompt(
    State(state): State<Arc<AppState>>,
    JsonBody(body): JsonBody<SaveAgentPromptBody>,
) -> Response {
    let path = state.runtime_prompt.path().display().to_string();
    match state.runtime_prompt.write(&body.content) {
        Ok(_) => Json(json!({ "ok": true, "path": path })).into_response(),
        Err(e) => validation(e),
    }
}
