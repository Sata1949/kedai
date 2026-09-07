// 角色脚本执行:generate/import 处理器构建与后端脚本串行执行(阶段三 3b-3、
// 阶段六 6g)。自 engine/mod.rs 拆分迁入,纯代码移动,逻辑不变。
// 依赖经 `use super::*` 取自 engine/mod.rs(与 executor/mvu 等子模块同一约定)。
use super::*;

impl AgentEngine {
    /// 阶段六 6g-1:构建 generate 处理器(捕获 connector 的 Arc clone,不引用引擎)。
    /// 同步签名,内部经 Handle::current().block_on 完成异步生成(调用方须位于
    /// tokio 运行时线程内,如 run_character_scripts 的 spawn_blocking)。
    fn make_generate_handler(&self) -> Arc<crate::scripts::bridge::GenerateHandler> {
        let connector = self.connector.clone();
        Arc::new(move |messages, params, abort| {
            let connector = connector.clone();
            tokio::runtime::Handle::current().block_on(async move {
                let conn = connector.read().await;
                let chunks = conn.generate(&messages, params, abort).await?;
                drop(conn);
                let mut out = String::new();
                let mut usage = TokenUsage::default();
                for chunk in chunks {
                    match chunk {
                        LlmStreamChunk::Token(t) => out.push_str(&t),
                        LlmStreamChunk::Usage {
                            prompt_tokens,
                            completion_tokens,
                            total_tokens,
                            prompt_cache_hit_tokens,
                            prompt_cache_miss_tokens,
                            ..
                        } => {
                            usage.prompt_tokens += prompt_tokens;
                            usage.completion_tokens += completion_tokens;
                            usage.total_tokens += total_tokens;
                            usage.prompt_cache_hit_tokens += prompt_cache_hit_tokens;
                            usage.prompt_cache_miss_tokens += prompt_cache_miss_tokens;
                        }
                        _ => {}
                    }
                }
                Ok((out, usage))
            })
        })
    }

    /// 阶段六 6g-2:构建导入处理器(捕获各 service 的 Arc clone,绕过 HTTP 直接调用)。
    /// 五类导入:character/worldbook/preset/chat/regex(暂不支持)。
    fn make_import_handler(&self) -> Arc<crate::scripts::bridge::ImportHandler> {
        let characters = self.characters.clone();
        let world_books = self.world_books.clone();
        let prompt_inject = self.prompt_inject.clone();
        let sessions = self.sessions.clone();
        Arc::new(
            move |kind: String, filename: String, content: String, session_id: String| match kind
                .as_str()
            {
                "character" => {
                    let rec = characters.upload(content.as_bytes(), &filename)?;
                    Ok(format!("已导入角色:{}", rec.id))
                }
                "worldbook" => {
                    let rec = world_books.upload(content.as_bytes(), &filename, None)?;
                    Ok(format!("已导入世界书:{}", rec.id))
                }
                "preset" => {
                    let floors = crate::parsing::preset::parse_st_preset(&content)?;
                    let mut svc = prompt_inject.lock().unwrap_or_else(|e| e.into_inner());
                    let mut cfg = svc.get().clone();
                    cfg.floors = floors.clone();
                    svc.set(cfg)?;
                    Ok(format!("已导入预设:{} 个楼层", floors.len()))
                }
                "chat" => {
                    if session_id.trim().is_empty() {
                        return Err("importRawChat 需指定会话(sessionId)".to_string());
                    }
                    let messages: Vec<crate::models::types::StMessage> =
                        serde_json::from_str(&content)
                            .map_err(|e| format!("解析聊天消息失败: {e}"))?;
                    let n = sessions.import_chat(&session_id, messages)?;
                    Ok(format!("已导入会话:{n} 条消息"))
                }
                "regex" => Err("importRawTavernRegex 暂不支持".to_string()),
                other => Err(format!("未知导入类型:{other}")),
            },
        )
    }

    /// 阶段三 3b-3:执行后端脚本(消息生成完成后)。
    /// 依次收集「全局脚本」与「角色卡脚本」的启用脚本,串行执行;
    /// 脚本经 TavernHelper 兼容桥写回共享 scopes(global/character/script 等),
    /// 由调用方收尾 take_others 统一落库。执行失败仅记日志,不影响主流程。
    pub(super) async fn run_character_scripts(
        &self,
        character_id: &str,
        scopes: &Arc<Mutex<crate::parsing::scopes::ScopeVars>>,
    ) {
        // 全局脚本(scope=global,owner 恒为空)
        let mut all: Vec<crate::scripts::loader::LoadedScript> = Vec::new();
        if let Ok(g) = self.user_scripts.get_tree("global", "") {
            all.extend(crate::scripts::loader::collect_enabled_scripts(&g));
        }
        // 角色卡脚本(含旧字段迁移;角色不存在/无脚本树 → 跳过)
        if let Ok(t) = self.user_scripts.get_character(character_id) {
            all.extend(crate::scripts::loader::collect_enabled_scripts(&t));
        }
        if all.is_empty() {
            return;
        }
        let opts = crate::scripts::runtime::EvalOptions::default();
        // 阶段六 6g:生成/导入处理器在循环外构建一次(捕获最小依赖:connector 与各
        // service 的 Arc clone),所有脚本共享同一份接线。
        let generate_handler = self.make_generate_handler();
        let import_handler = self.make_import_handler();
        for script in all {
            // 每次执行新建 EvalBridge(共享 scopes + 角色 id);运行时为同步阻塞,
            // 经 spawn_blocking 隔离,避免阻塞 tokio 运行时(quickjs 非线程安全,
            // 每次执行独立 Runtime,天然无跨线程共享)。
            let bridge = crate::scripts::bridge::EvalBridge::new(scopes.clone(), script.clone())
                .with_character(character_id.to_string())
                .with_registry(self.slash.clone())
                .with_generate(generate_handler.clone())
                .with_imports(import_handler.clone());
            let source = script.content.clone();
            let opts = opts.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                crate::scripts::runtime::eval_with_bridge(&source, &opts, &bridge)
            })
            .await
            .unwrap_or_else(|_| {
                crate::scripts::runtime::EvalOutcome::Error("脚本执行任务被取消".into())
            });
            if let crate::scripts::runtime::EvalOutcome::Error(msg) = outcome {
                logger::warn(
                    "角色脚本执行失败",
                    &[
                        ("character_id", Value::String(character_id.to_string())),
                        ("script", Value::String(script.name)),
                        ("error", Value::String(msg)),
                    ],
                );
            }
        }
    }
}
