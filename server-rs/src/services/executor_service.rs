// 任务执行者库服务(需求:任务模式不再复用角色扮演角色卡,改用独立执行者)。
//
// 背景:此前任务创建下拉直接列出角色扮演角色卡,把「执行者」选定为一张人设卡,
// 造成两个模式的角色资产互相污染——角色卡的写作风格段被注入任务执行提示词,
// 用户也无法为任务单独描述「这个执行者该干什么」。本服务提供与角色卡解耦的
// 执行者实体:只有名称 + 执行者指令(+ 可选温度),持久化到 data/task_executors.json。
//
// 与 agent_flow_service 同构(全局作用域、内存缓存、全量读写、原子落盘);
// 差异:执行者按**任务**选用(tasks.executor_id),故库内无「当前选中」概念,
// save 即 upsert。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// 执行者指令长度上限(字符):执行者指令会整段注入 system 提示词,不设上限时
/// 一张超长卡会直接吃满上下文(角色卡人设段有 persona_style 精简开关兜底,
/// 执行者指令没有,故在写入侧硬限)。
pub const INSTRUCTION_MAX_CHARS: usize = 8_000;

/// 任务执行者服务:持有 data_dir,内存缓存执行者库,读写 data/task_executors.json
pub struct ExecutorService {
    data_dir: PathBuf,
    executors: Vec<TaskExecutorConfig>,
}

impl ExecutorService {
    pub fn new(data_dir: PathBuf) -> Self {
        let executors = load_library(&data_dir);
        ExecutorService {
            data_dir,
            executors,
        }
    }

    /// 全部执行者(按数组顺序;列表展示用)
    pub fn list(&self) -> &[TaskExecutorConfig] {
        &self.executors
    }

    /// 按 id 取执行者(任务执行时按 tasks.executor_id 查配置)
    pub fn get(&self, id: &str) -> Option<&TaskExecutorConfig> {
        self.executors.iter().find(|e| e.id == id)
    }

    /// 保存(创建或更新)执行者;id 为空时自动生成(新建)。
    /// 校验失败返回 Err(不落盘)。
    pub fn save(&mut self, mut config: TaskExecutorConfig) -> Result<TaskExecutorConfig, String> {
        validate_executor(&config)?;
        let now = crate::models::db::now_iso();
        if config.id.trim().is_empty() {
            config.id = Uuid::new_v4().to_string();
        }
        config.name = config.name.trim().to_string();
        config.instruction = config.instruction.trim().to_string();
        config.updated_at = now.clone();
        match self.executors.iter_mut().find(|e| e.id == config.id) {
            Some(existing) => {
                // 更新:保留创建时间(前端按 created_at 展示排序依据)
                config.created_at = existing.created_at.clone();
                *existing = config.clone();
            }
            None => {
                if config.created_at.trim().is_empty() {
                    config.created_at = now;
                }
                self.executors.push(config.clone());
            }
        }
        self.save_library()?;
        Ok(config)
    }

    /// 删除执行者;不存在返回 Err。已引用该执行者的任务不受影响:
    /// 任务执行时查不到配置即回退通用执行者(见 TaskService::character_for 同款兜底语义)
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let before = self.executors.len();
        self.executors.retain(|e| e.id != id);
        if self.executors.len() == before {
            return Err(format!("执行者不存在:{id}"));
        }
        self.save_library()
    }

    /// 持久化到 data/task_executors.json(原子写:崩溃不留半截 JSON)
    fn save_library(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&self.executors).map_err(|e| e.to_string())?;
        crate::utils::fs_atomic::write_atomic(
            &self.data_dir.join("task_executors.json"),
            text.as_bytes(),
        )
        .map_err(|e| e.to_string())
    }
}

/// 任务执行者配置(单个执行者)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskExecutorConfig {
    /// 执行者 id(库内唯一;空表示新建,由服务分配)
    #[serde(default)]
    pub id: String,
    /// 执行者名称(任务创建下拉与库列表展示)
    #[serde(default)]
    pub name: String,
    /// 执行者指令:该执行者的职责/工作方式/身份描述,整段注入执行者 system 提示词
    #[serde(default)]
    pub instruction: String,
    /// 该执行者的建议温度(可选;None = 沿用任务模式默认温度)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// 校验执行者(失败返回中文错误,API 层 400 给用户)
fn validate_executor(config: &TaskExecutorConfig) -> Result<(), String> {
    if config.name.trim().is_empty() {
        return Err("执行者名称不能为空".into());
    }
    if config.instruction.trim().is_empty() {
        return Err("执行者指令不能为空:请描述该执行者的职责与工作方式".into());
    }
    let chars = config.instruction.trim().chars().count();
    if chars > INSTRUCTION_MAX_CHARS {
        return Err(format!(
            "执行者指令过长:{chars} 字符,上限 {INSTRUCTION_MAX_CHARS}"
        ));
    }
    if let Some(t) = config.temperature {
        if !(0.0..=2.0).contains(&t) {
            return Err("执行者温度需在 0.0-2.0 之间".into());
        }
    }
    Ok(())
}

/// 从 data/task_executors.json 加载;缺失返回空库(与流程库不同:执行者库无内置项,
/// 空即「只有通用执行者」,属正常初始态,不注入任何默认执行者);
/// 损坏记录日志后回退空(不静默吞掉)。
fn load_library(data_dir: &Path) -> Vec<TaskExecutorConfig> {
    let path = data_dir.join("task_executors.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Vec<TaskExecutorConfig>>(&text) {
            Ok(list) => list,
            Err(e) => {
                tracing::error!(
                    error = e.to_string(),
                    "任务执行者库解析失败,已回退空库(仅通用执行者可用)"
                );
                Vec::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::error!(
                error = e.to_string(),
                "任务执行者库读取失败,已回退空库(仅通用执行者可用)"
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    fn executor(name: &str, instruction: &str) -> TaskExecutorConfig {
        TaskExecutorConfig {
            id: String::new(),
            name: name.into(),
            instruction: instruction.into(),
            temperature: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn missing_file_yields_empty_library() {
        let dir = TempDataDir::new("executor-empty");
        let svc = ExecutorService::new(dir.path().to_path_buf());
        assert!(
            svc.list().is_empty(),
            "执行者库无内置项:空库即「只有通用执行者」"
        );
    }

    #[test]
    fn save_assigns_id_and_persists() {
        let dir = TempDataDir::new("executor-save");
        let mut svc = ExecutorService::new(dir.path().to_path_buf());
        let saved = svc
            .save(executor("审稿员", "你是严苛的审稿人,逐条指出问题"))
            .unwrap();
        assert!(!saved.id.is_empty(), "空 id 应自动分配");
        assert_eq!(svc.list().len(), 1);

        // 重新加载:落盘生效
        let svc2 = ExecutorService::new(dir.path().to_path_buf());
        assert_eq!(svc2.list().len(), 1);
        assert_eq!(svc2.list()[0].name, "审稿员");
    }

    #[test]
    fn save_with_existing_id_updates_in_place_and_keeps_created_at() {
        let dir = TempDataDir::new("executor-update");
        let mut svc = ExecutorService::new(dir.path().to_path_buf());
        let saved = svc.save(executor("A", "指令一")).unwrap();
        let created = saved.created_at.clone();

        let mut edit = saved.clone();
        edit.name = "A2".into();
        edit.instruction = "指令二".into();
        svc.save(edit).unwrap();

        assert_eq!(svc.list().len(), 1, "同 id 应就地更新而非追加");
        assert_eq!(svc.list()[0].name, "A2");
        assert_eq!(
            svc.list()[0].created_at,
            created,
            "更新应保留创建时间(列表排序依据)"
        );
    }

    #[test]
    fn blank_name_or_instruction_rejected() {
        let dir = TempDataDir::new("executor-validate");
        let mut svc = ExecutorService::new(dir.path().to_path_buf());
        assert!(svc.save(executor("  ", "指令")).is_err(), "空名称应拒绝");
        assert!(svc.save(executor("名", "   ")).is_err(), "空指令应拒绝");
        assert!(svc.list().is_empty(), "校验失败不得落库");
    }

    #[test]
    fn out_of_range_temperature_rejected() {
        let dir = TempDataDir::new("executor-temp");
        let mut svc = ExecutorService::new(dir.path().to_path_buf());
        let mut cfg = executor("名", "指令");
        cfg.temperature = Some(2.5);
        assert!(svc.save(cfg).is_err(), "温度超上限应拒绝");
    }

    #[test]
    fn remove_missing_is_error_and_existing_persists() {
        let dir = TempDataDir::new("executor-remove");
        let mut svc = ExecutorService::new(dir.path().to_path_buf());
        let saved = svc.save(executor("A", "指令")).unwrap();
        assert!(svc.remove("nope").is_err(), "删除不存在的执行者应报错");
        svc.remove(&saved.id).unwrap();
        assert!(svc.list().is_empty());
        let svc2 = ExecutorService::new(dir.path().to_path_buf());
        assert!(svc2.list().is_empty(), "删除应已落盘");
    }

    #[test]
    fn corrupted_file_falls_back_to_empty() {
        let dir = TempDataDir::new("executor-corrupt");
        std::fs::write(dir.join("task_executors.json"), "{not json").unwrap();
        let svc = ExecutorService::new(dir.path().to_path_buf());
        assert!(svc.list().is_empty());
    }

    #[test]
    fn get_returns_none_for_unknown_id() {
        let dir = TempDataDir::new("executor-get");
        let mut svc = ExecutorService::new(dir.path().to_path_buf());
        let saved = svc.save(executor("A", "指令")).unwrap();
        assert!(svc.get(&saved.id).is_some());
        // 任务引用了已删除/不存在的执行者 → None,由调用方回退通用执行者
        assert!(svc.get("deleted-id").is_none());
    }
}
