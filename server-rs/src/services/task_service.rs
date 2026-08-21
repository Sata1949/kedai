// 任务模式(task 工作台)服务:任务主表 CRUD + 后台执行引擎。
// 执行流程:planning(LLM 规划拆解)→ running(逐步派子智能体执行)→ done(LLM 汇总);
// 出错 → error;stop → ended。子任务存 task_subtasks 表(独立于 agent_subtasks,
// 因后者的 session_id 外键指向 sessions 表,而任务 id 不在其中)。
use super::log_query_failure;
use crate::connectors::Connector;
use crate::models::db::{now_iso, Db};
use crate::models::types::{
    CharacterRecord, GenerationParams, LlmMessage, LlmStreamChunk, TaskRecord, TaskStep,
    TaskSubtaskRecord, ToolChoice,
};
use crate::parsing::world_book::WorldEntry;
use crate::services::character_service::CharacterService;
use crate::services::prompt_inject_service::{FloorRole, InjectMode, PromptInjectService};
use crate::services::settings_service::{AppMode, RuntimeSettings};
use crate::services::world_book_service::WorldBookService;
use crate::utils::logger;
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{watch, RwLock};
use uuid::Uuid;

/// 任务模式单次 LLM 调用总时长上限(涵盖共享读锁排队 + 整个流式生成)。
/// 超时报错让任务进入 error 终态,防止上游停滞或锁队列饿死导致任务永久停在
/// planning/步骤 running(层内另有 SSE 流空闲看门狗,此为兜底;正常规划/生成
/// 均在数十秒量级,5 分钟上限足够宽裕)。
const TASK_LLM_TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// 任务主表列:0 id, 1 title, 2 status, 3 plan, 4 result, 5 error, 6 character_id, 7 created_at, 8 updated_at
fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<TaskRecord> {
    let plan_str: String = row.get(3)?;
    let plan = serde_json::from_str(&plan_str).unwrap_or_default();
    Ok(TaskRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        status: row.get(2)?,
        plan,
        result: row.get(4)?,
        error: row.get(5)?,
        character_id: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

const TASK_COLS: &str =
    "id, title, status, plan, result, error, character_id, created_at, updated_at";

fn row_to_subtask(row: &rusqlite::Row) -> rusqlite::Result<TaskSubtaskRecord> {
    Ok(TaskSubtaskRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        name: row.get(2)?,
        instruction: row.get(3)?,
        status: row.get(4)?,
        result: row.get(5)?,
        error: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

pub struct TaskService {
    db: Arc<Db>,
    characters: Arc<CharacterService>,
    connector: Arc<RwLock<Connector>>,
    /// 运行期设置(读任务模式的生成参数);与 AppState 共享同一实例
    settings: Arc<Mutex<RuntimeSettings>>,
    /// 世界书(注入执行者人设的世界观设定)
    world_books: Arc<WorldBookService>,
    /// 提示词注入配置(简单合成文本 / 复杂 system 楼层)
    prompt_inject: Arc<Mutex<PromptInjectService>>,
    /// 任务 id → (取消信号, 本次执行 token)。token 用于区分同一任务的先后执行:
    /// stop 后立即重跑时,旧后台任务退出不得删除/覆盖新任务的取消条目与状态。
    cancels: Mutex<HashMap<String, (watch::Sender<bool>, u64)>>,
}

/// 每次 run 分配自增 token,识别「同一任务的当前执行」。
static NEXT_RUN_TOKEN: AtomicU64 = AtomicU64::new(1);

impl TaskService {
    pub fn new(
        db: Arc<Db>,
        characters: Arc<CharacterService>,
        connector: Arc<RwLock<Connector>>,
        settings: Arc<Mutex<RuntimeSettings>>,
        world_books: Arc<WorldBookService>,
        prompt_inject: Arc<Mutex<PromptInjectService>>,
    ) -> Self {
        TaskService {
            db,
            characters,
            connector,
            settings,
            world_books,
            prompt_inject,
            cancels: Mutex::new(HashMap::new()),
        }
    }

    /// 读取任务模式合并后的有效设置(生成参数覆盖项已应用,连接信息共享)。
    fn task_settings(&self) -> RuntimeSettings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .for_mode(AppMode::Task)
    }

    /// 世界书常驻条目文本:过滤 enabled && constant 且内容非空,按 position→order→id 排序,
    /// 拼成「世界书设定」段落。无执行者时传空 id 仅取全局世界书。
    fn world_context(&self, character_id: Option<&str>) -> String {
        let cid = character_id.unwrap_or("");
        let entries = self.world_books.collect_entries_for_character(cid);
        let mut constants: Vec<&WorldEntry> = entries
            .iter()
            .filter(|e| e.enabled && e.constant && !e.content.trim().is_empty())
            .collect();
        constants.sort_by_key(|e| (e.position, e.order, e.id));
        let texts: Vec<String> = constants
            .iter()
            .map(|e| format!("[{}]\n{}", e.comment, e.content.trim()))
            .collect();
        if texts.is_empty() {
            String::new()
        } else {
            format!("世界书设定:\n{}", texts.join("\n\n"))
        }
    }

    /// 提示词注入文本:简单模式取合成文本;复杂模式取 role=system 的启用楼层内容。
    /// 任务模式无对话历史,楼层 before/after/depth 位置语义不适用,仅注入 system 楼层。
    fn inject_text(&self) -> String {
        let guard = self.prompt_inject.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = guard.get();
        let mut parts: Vec<String> = Vec::new();
        match cfg.mode {
            InjectMode::Simple => {
                let t = cfg.simple_inject_text();
                if !t.is_empty() {
                    parts.push(t);
                }
            }
            InjectMode::Complex => {
                for floor in cfg.enabled_floors_sorted() {
                    if floor.role == FloorRole::System && !floor.content.trim().is_empty() {
                        parts.push(floor.content.trim().to_string());
                    }
                }
            }
        }
        parts.join("\n")
    }

    /// 渲染任务模式 Agent 系统提示词占位符(与角色扮演同款占位符语义):
    /// {{character_name}} / {{character_description}} / {{world_info}}。
    fn render_agent_prompt(
        &self,
        prompt: &str,
        character: Option<&CharacterRecord>,
        world_text: &str,
    ) -> String {
        let mut out = prompt.to_string();
        if let Some(c) = character {
            out = out.replace("{{character_name}}", &c.chara_name);
            out = out.replace("{{character_description}}", &c.description);
        }
        out.replace("{{world_info}}", world_text)
    }

    // ===== CRUD =====

    pub fn create(&self, title: &str, character_id: Option<&str>) -> Result<TaskRecord, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("任务目标不能为空".into());
        }
        let id = Uuid::new_v4().to_string();
        let now = now_iso();
        let cid = character_id.map(|s| s.to_string());
        let conn = self.db.write();
        conn.execute(
            "INSERT INTO tasks (id, title, status, plan, result, error, character_id, created_at, updated_at) \
             VALUES (?1, ?2, 'pending', '[]', '', '', ?3, ?4, ?4)",
            params![id, title, cid, now],
        )
        .map_err(|e| format!("创建任务失败: {e}"))?;
        Ok(TaskRecord {
            id,
            title: title.to_string(),
            status: "pending".into(),
            plan: Vec::new(),
            result: String::new(),
            error: String::new(),
            character_id: cid,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn list(&self) -> Vec<TaskRecord> {
        let conn = self.db.read().expect("获取只读连接失败");
        let mut stmt = match conn
            .prepare_cached(&format!("SELECT {TASK_COLS} FROM tasks ORDER BY created_at DESC"))
        {
            Ok(s) => s,
            Err(e) => return log_query_failure("任务列表 prepare", e),
        };
        let query = stmt.query_map([], row_to_task);
        match query {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => log_query_failure("任务列表 query_map", e),
        }
    }

    pub fn get(&self, id: &str) -> Option<TaskRecord> {
        self.db
            .read()
            .ok()?
            .query_row(
                &format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"),
                params![id],
                row_to_task,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn get_with_subtasks(&self, id: &str) -> Option<(TaskRecord, Vec<TaskSubtaskRecord>)> {
        let task = self.get(id)?;
        let subtasks = self.list_subtasks(id);
        Some((task, subtasks))
    }

    /// 删除任务(子任务经外键 ON DELETE CASCADE 一并删除);运行中则先取消。
    pub fn delete(&self, id: &str) -> bool {
        self.signal_cancel(id);
        self.remove_cancel_entry(id);
        self.db
            .write()
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    // ===== 执行 =====

    /// 启动后台执行。重跑会清空旧 plan/result/error 并重新规划。
    /// 每次 run 分配新 token 覆盖旧条目,旧后台任务退出时凭 token 判断自己是否仍是当前执行。
    pub fn run(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let task = self.get(id).ok_or("任务不存在")?;
        if task.status == "running" || task.status == "planning" {
            return Err("任务已在执行中".into());
        }
        self.reset_task(id);
        let (cancel, token) = self.register_cancel(id);
        let deps = self.clone();
        let task_id = id.to_string();
        tokio::spawn(async move {
            run_task_background(deps, task_id, cancel, token).await;
        });
        Ok(())
    }

    /// 停止:标记 ended + 发取消信号 + 结束未完成的子任务。
    /// 不再立即移除取消条目——由后台任务退出时按 token 清理,避免删掉
    /// 「stop 后立即重跑」的新执行条目。
    pub fn stop(&self, id: &str) -> bool {
        let changed = self.set_status(id, "ended");
        self.signal_cancel(id);
        for st in self.list_subtasks(id) {
            if st.status == "running" || st.status == "pending" {
                self.set_subtask_status(&st.id, "ended", None, None);
            }
        }
        changed
    }

    // ===== 状态写入(内部) =====

    fn reset_task(&self, id: &str) -> bool {
        let conn = self.db.write();
        conn.execute(
            "UPDATE tasks SET status = 'planning', plan = '[]', result = '', error = '', updated_at = ?1 WHERE id = ?2",
            params![now_iso(), id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    fn set_status(&self, id: &str, status: &str) -> bool {
        let conn = self.db.write();
        conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now_iso(), id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    fn set_plan(&self, id: &str, plan: &[TaskStep]) -> bool {
        let json = serde_json::to_string(plan).unwrap_or_else(|_| "[]".into());
        let conn = self.db.write();
        conn.execute(
            "UPDATE tasks SET plan = ?1, updated_at = ?2 WHERE id = ?3",
            params![json, now_iso(), id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    fn set_result(&self, id: &str, result: &str) -> bool {
        let conn = self.db.write();
        conn.execute(
            "UPDATE tasks SET result = ?1, status = 'done', updated_at = ?2 WHERE id = ?3",
            params![result, now_iso(), id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    fn set_error(&self, id: &str, error: &str) -> bool {
        let conn = self.db.write();
        conn.execute(
            "UPDATE tasks SET error = ?1, status = 'error', updated_at = ?2 WHERE id = ?3",
            params![error, now_iso(), id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    // ===== 子任务 =====

    fn create_subtask(&self, task_id: &str, name: &str, instruction: &str) -> Result<String, String> {
        let id = Uuid::new_v4().to_string();
        let now = now_iso();
        let conn = self.db.write();
        conn.execute(
            "INSERT INTO task_subtasks (id, task_id, name, instruction, status, result, error, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'running', '', '', ?5, ?5)",
            params![id, task_id, name, instruction, now],
        )
        .map_err(|e| format!("创建子任务失败: {e}"))?;
        Ok(id)
    }

    fn set_subtask_status(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> bool {
        let conn = self.db.write();
        // 保持未提供字段不变:先读旧值
        let existing = conn
            .query_row(
                "SELECT result, error FROM task_subtasks WHERE id = ?1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (res, err) = existing.unwrap_or((String::new(), String::new()));
        let res = result.map(|s| s.to_string()).unwrap_or(res);
        let err = error.map(|s| s.to_string()).unwrap_or(err);
        conn.execute(
            "UPDATE task_subtasks SET status = ?1, result = ?2, error = ?3, updated_at = ?4 WHERE id = ?5",
            params![status, res, err, now_iso(), id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    fn list_subtasks(&self, task_id: &str) -> Vec<TaskSubtaskRecord> {
        let conn = self.db.read().expect("获取只读连接失败");
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, name, instruction, status, result, error, created_at, updated_at \
                 FROM task_subtasks WHERE task_id = ?1 ORDER BY created_at ASC",
            )
            .unwrap();
        stmt.query_map(params![task_id], row_to_subtask)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    // ===== 取消信号 =====

    /// 注册本次执行:总是新建 watch 通道并用新 token 覆盖旧条目(重跑语义),
    /// 返回取消接收端与本次 token。旧执行持有的 Sender 随 drop 失效,不影响新通道。
    fn register_cancel(&self, id: &str) -> (watch::Receiver<bool>, u64) {
        let token = NEXT_RUN_TOKEN.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = watch::channel(false);
        self.cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), (tx, token));
        (rx, token)
    }

    fn signal_cancel(&self, id: &str) {
        if let Some((tx, _)) = self.cancels.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            let _ = tx.send(true);
        }
    }

    /// 指定 token 是否仍是该任务当前的执行(旧执行在重跑后返回 false)。
    fn is_current_run(&self, id: &str, token: u64) -> bool {
        self.cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .map(|(_, t)| *t == token)
            .unwrap_or(false)
    }

    /// 仅当条目仍归属本次 token 时移除(旧执行不得删掉新执行刚注册的条目)。
    fn remove_cancel_if(&self, id: &str, token: u64) {
        let mut cancels = self.cancels.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, t)) = cancels.get(id) {
            if *t == token {
                cancels.remove(id);
            }
        }
    }

    /// 无条件移除条目(仅任务已删除等终态路径使用)。
    fn remove_cancel_entry(&self, id: &str) {
        self.cancels.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }

    // ===== LLM 调用 =====

    /// 非流式生成:把 Token 块拼成文本返回。记录模型/参数/耗时/结果规模到日志,
    /// 便于任务运行期问题定位(llm_requests 表仅聊天引擎使用,任务模式不走该链路)。
    async fn generate_text(
        &self,
        messages: Vec<LlmMessage>,
        max_tokens: u32,
        temperature: f64,
        top_p: f64,
        cancel: watch::Receiver<bool>,
    ) -> Result<String, String> {
        let params = GenerationParams {
            temperature,
            top_p,
            max_tokens,
            stop: None,
            tools: Vec::new(),
            max_tool_rounds: None,
            tool_choice: ToolChoice::Auto,
            parallel_tool_calls: None,
        };
        let started = Instant::now();
        // 总时长看门狗:超时范围包含 connector 读锁获取(该锁与聊天引擎/设置接口共享,
        // 慢网络请求可能持锁排队)与整个流式生成;超时后 future 被 drop,读锁随之释放。
        let call = async {
            let connector = self.connector.read().await;
            let model = connector.model().to_string();
            let connector_type = connector.type_name().to_string();
            let res = connector.generate(&messages, params, cancel).await;
            (res, model, connector_type)
        };
        let (res, model, connector_type) = match tokio::time::timeout(TASK_LLM_TOTAL_TIMEOUT, call).await {
            Ok(v) => v,
            Err(_) => {
                logger::warn(
                    "任务模式 LLM 生成超时(看门狗触发)",
                    &[
                        ("timeout_s", serde_json::Value::from(TASK_LLM_TOTAL_TIMEOUT.as_secs())),
                        ("max_tokens", serde_json::Value::from(max_tokens)),
                        ("elapsed_ms", serde_json::Value::from(started.elapsed().as_millis() as u64)),
                    ],
                );
                return Err(format!(
                    "模型调用超过 {}s 未完成(上游停滞或接口占用),已中止;可重新执行任务",
                    TASK_LLM_TOTAL_TIMEOUT.as_secs()
                ));
            }
        };
        let chunks = res?;
        let mut content = String::new();
        for c in chunks {
            if let LlmStreamChunk::Token(t) = c {
                content.push_str(&t);
            }
        }
        logger::info(
            "任务模式 LLM 生成完成",
            &[
                ("model", serde_json::Value::String(model)),
                ("connector", serde_json::Value::String(connector_type)),
                ("max_tokens", serde_json::Value::from(max_tokens)),
                ("temperature", serde_json::Value::from(temperature)),
                ("top_p", serde_json::Value::from(top_p)),
                ("chars", serde_json::Value::from(content.chars().count())),
                ("elapsed_ms", serde_json::Value::from(started.elapsed().as_millis() as u64)),
            ],
        );
        Ok(content)
    }

    /// 规划:把目标拆成 2~5 个步骤(JSON 数组)。
    /// 温度/长度保持低温短输出(JSON 解析稳定是结构约束,非用户可调生成风格),top_p 读取任务模式设置。
    /// 仅注入世界书常驻设定作为背景;不注入提示词注入(输出要求会破坏 JSON)与 Agent 系统提示词。
    async fn plan_task(
        &self,
        title: &str,
        character_id: Option<&str>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Vec<TaskStep>, String> {
        let mut sys = String::from(
            "你是任务规划器。把用户目标拆解为 2~5 个可独立执行的具体步骤,每个步骤单一、明确、粒度适中(约 2~5 分钟可完成)。严格只输出 JSON 数组,不要输出任何解释或多余文字。数组元素格式:{\"name\":\"步骤名\",\"goal\":\"该步骤要完成的目标\"}",
        );
        let world = self.world_context(character_id);
        if !world.is_empty() {
            sys.push_str(&format!("\n\n{}", world));
        }
        let messages = vec![
            LlmMessage::plain("system", &sys),
            LlmMessage::plain("user", title),
        ];
        let settings = self.task_settings();
        let text = self
            .generate_text(messages, 1024, 0.3, settings.default_top_p, cancel.clone())
            .await?;
        let steps = parse_plan(&text)?;
        if steps.is_empty() {
            return Err("规划器未产出有效步骤".into());
        }
        Ok(steps)
    }

    /// 执行单个步骤:注入执行者人设 + 世界书 + 提示词注入 + Agent 系统提示词。
    /// 生成参数读任务模式设置。
    async fn generate_step(
        &self,
        task: &TaskRecord,
        step: &TaskStep,
        cancel: &watch::Receiver<bool>,
    ) -> Result<String, String> {
        let settings = self.task_settings();
        let character = task
            .character_id
            .as_deref()
            .and_then(|cid| self.characters.get(cid));

        let mut sys = String::from(
            "你是任务执行者,负责独立完成交给你的一个子任务。直接输出该子任务的最终结果:不要复述指令、不要输出计划或元文本、不要模拟对话、不要用标题包裹结果。",
        );
        if let Some(c) = &character {
            let style = persona_style(c);
            if !style.is_empty() {
                sys.push_str(&format!(
                    "\n\n写作风格参考(角色「{}」):\n{}",
                    c.chara_name, style
                ));
            }
        }
        let world = self.world_context(task.character_id.as_deref());
        if !world.is_empty() {
            sys.push_str(&format!("\n\n{}", world));
        }
        let inject = self.inject_text();
        if !inject.is_empty() {
            sys.push_str(&format!("\n\n{}", inject));
        }
        if !settings.agent_system_prompt.trim().is_empty() {
            let rendered = self.render_agent_prompt(
                &settings.agent_system_prompt,
                character.as_ref(),
                &world,
            );
            sys.push_str(&format!("\n\n{}", rendered));
        }
        let messages = vec![
            LlmMessage::plain("system", &sys),
            LlmMessage::plain("user", &step.goal),
        ];
        self.generate_text(
            messages,
            settings.default_max_tokens,
            settings.default_temperature,
            settings.default_top_p,
            cancel.clone(),
        )
        .await
    }

    /// 汇总:综合各步骤结果产出最终成果。注入世界书 + 提示词注入 + Agent 系统提示词。
    /// 生成参数读任务模式设置。
    async fn summarize_task(
        &self,
        task: &TaskRecord,
        plan: &[TaskStep],
        cancel: &watch::Receiver<bool>,
    ) -> Result<String, String> {
        let settings = self.task_settings();
        let character = task
            .character_id
            .as_deref()
            .and_then(|cid| self.characters.get(cid));

        let mut sys = String::from(
            "你是任务汇总者。下面是用户目标、执行计划与各步骤结果。请输出一份完整、有条理的最终成果,直接呈现结果本身(不要写「汇总如下」「以下是」等元文本)。",
        );
        let world = self.world_context(task.character_id.as_deref());
        if !world.is_empty() {
            sys.push_str(&format!("\n\n{}", world));
        }
        let inject = self.inject_text();
        if !inject.is_empty() {
            sys.push_str(&format!("\n\n{}", inject));
        }
        if !settings.agent_system_prompt.trim().is_empty() {
            let rendered = self.render_agent_prompt(
                &settings.agent_system_prompt,
                character.as_ref(),
                &world,
            );
            sys.push_str(&format!("\n\n{}", rendered));
        }
        let mut user = format!("用户目标:\n{}\n\n各步骤结果:\n", task.title);
        for (i, s) in plan.iter().enumerate() {
            user.push_str(&format!("{}. {}({}):\n{}\n", i + 1, s.name, s.status, s.result));
        }
        let messages = vec![
            LlmMessage::plain("system", &sys),
            LlmMessage::plain("user", &user),
        ];
        self.generate_text(
            messages,
            settings.default_max_tokens,
            settings.default_temperature,
            settings.default_top_p,
            cancel.clone(),
        )
        .await
    }
}

/// 从角色卡构建执行者人设参考文本:人设(description)+ 人格(personality)+
/// 世界观/情境(scenario)+ 文风示例(mes_example),均为 V2 角色卡 data 对象的顶层字段。
/// 任一字段为空则跳过;全部为空返回空串(此时不注入人设)。
fn persona_style(c: &CharacterRecord) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !c.description.trim().is_empty() {
        parts.push(format!("人设:\n{}", c.description.trim()));
    }
    if let Some(raw) = c.data_raw.as_ref() {
        if let Some(p) = raw.get("personality").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
            parts.push(format!("人格:\n{}", p.trim()));
        }
        if let Some(s) = raw.get("scenario").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
            parts.push(format!("世界观/情境:\n{}", s.trim()));
        }
        if let Some(m) = raw.get("mes_example").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
            parts.push(format!("文风示例:\n{}", m.trim()));
        }
    }
    parts.join("\n\n")
}

/// 从 LLM 输出解析计划 JSON 数组(容忍 markdown 代码块包裹)。
fn parse_plan(text: &str) -> Result<Vec<TaskStep>, String> {
    let t = text.trim();
    if let Ok(steps) = serde_json::from_str::<Vec<TaskStep>>(t) {
        return Ok(steps);
    }
    let stripped = t
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str::<Vec<TaskStep>>(stripped).map_err(|e| format!("解析计划失败: {e}"))
}

/// 后台执行退出收尾:仅当自己仍是该任务当前执行时才写终态(旧执行在重跑后让位,
/// 不覆盖新任务状态),并按 token 清理取消条目(不删新执行的条目)。
fn finalize_run(
    deps: &TaskService,
    task_id: &str,
    token: u64,
    ended_by_cancel: bool,
    error: Option<&str>,
) {
    if deps.is_current_run(task_id, token) {
        if ended_by_cancel {
            let _ = deps.set_status(task_id, "ended");
        } else if let Some(e) = error {
            let _ = deps.set_error(task_id, e);
        }
    }
    deps.remove_cancel_if(task_id, token);
}

/// 后台执行主体:规划 → 逐步执行 → 汇总。
async fn run_task_background(
    deps: Arc<TaskService>,
    task_id: String,
    cancel: watch::Receiver<bool>,
    token: u64,
) {
    let Some(task) = deps.get(&task_id) else {
        deps.remove_cancel_if(&task_id, token);
        return;
    };

    // 1) 规划
    let plan = match deps
        .plan_task(&task.title, task.character_id.as_deref(), &cancel)
        .await
    {
        Ok(steps) => steps,
        Err(e) => {
            finalize_run(&deps, &task_id, token, *cancel.borrow(), Some(&e));
            return;
        }
    };
    if *cancel.borrow() {
        finalize_run(&deps, &task_id, token, true, None);
        return;
    }
    let _ = deps.set_plan(&task_id, &plan);
    let _ = deps.set_status(&task_id, "running");

    // 2) 逐步执行
    let mut final_plan = plan.clone();
    for (i, step) in plan.iter().enumerate() {
        if *cancel.borrow() {
            finalize_run(&deps, &task_id, token, true, None);
            return;
        }
        final_plan[i].status = "running".to_string();
        let _ = deps.set_plan(&task_id, &final_plan);

        let subtask_id = match deps.create_subtask(&task_id, &step.name, &step.goal) {
            Ok(id) => id,
            Err(e) => {
                final_plan[i].status = "error".to_string();
                final_plan[i].result = e.clone();
                let _ = deps.set_plan(&task_id, &final_plan);
                continue;
            }
        };

        // 生成步骤:空内容自动重试一次(LLM 偶发空响应),仍空才标 error。
        let gen = match deps.generate_step(&task, step, &cancel).await {
            Ok(text) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    deps.generate_step(&task, step, &cancel).await
                } else {
                    Ok(text)
                }
            }
            Err(e) => Err(e),
        };
        match gen {
            Ok(text) if !text.trim().is_empty() => {
                let text = text.trim().to_string();
                final_plan[i].status = "done".to_string();
                final_plan[i].result = text.clone();
                deps.set_subtask_status(&subtask_id, "done", Some(&text), None);
            }
            Ok(_) => {
                final_plan[i].status = "error".to_string();
                final_plan[i].result = "子任务返回空内容".to_string();
                deps.set_subtask_status(&subtask_id, "error", None, Some("子任务返回空内容"));
            }
            Err(e) => {
                if *cancel.borrow() {
                    finalize_run(&deps, &task_id, token, true, None);
                    return;
                }
                final_plan[i].status = "error".to_string();
                final_plan[i].result = e.clone();
                deps.set_subtask_status(&subtask_id, "error", None, Some(&e));
            }
        }
        let _ = deps.set_plan(&task_id, &final_plan);
    }

    // 3) 汇总:空内容自动重试一次。
    if *cancel.borrow() {
        finalize_run(&deps, &task_id, token, true, None);
        return;
    }
    let summary = match deps.summarize_task(&task, &final_plan, &cancel).await {
        Ok(text) => {
            let text = text.trim().to_string();
            if text.is_empty() {
                deps.summarize_task(&task, &final_plan, &cancel).await
            } else {
                Ok(text)
            }
        }
        Err(e) => Err(e),
    };
    match summary {
        Ok(summary) if !summary.trim().is_empty() => {
            // 正常完成:旧执行在重跑后让位,不覆盖新任务状态。
            if deps.is_current_run(&task_id, token) {
                let _ = deps.set_result(&task_id, &summary);
            }
        }
        Ok(_) => finalize_run(&deps, &task_id, token, false, Some("汇总返回空内容")),
        Err(e) => finalize_run(&deps, &task_id, token, *cancel.borrow(), Some(&e)),
    }
    deps.remove_cancel_if(&task_id, token);
}
