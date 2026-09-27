// 跨会话记忆蒸馏服务(落地项 2,借鉴 Codex memories stage1_outputs 与 Reasonix 记忆修订):
// 记忆按 character_id 维度跨会话共享,解决长对话人设漂移。
//   - 蒸馏(distill):把会话历史交给 LLM 提取多条短记忆,每行一条落库(kind='distilled');
//   - 精选(select_for_injection):selected=1 的按 (pinned DESC, usage_count DESC,
//     last_usage DESC, id DESC) 排序取前 limit 条注入;排序键确定性(tie-break 用 id),
//     保证记忆集合未变时槽内容逐字节稳定;
//   - 召回(select_recall,升级工作流 B2 通道 2):按当前用户输入做 token 相关性召回;
//   - 衰减(touch):注入完成后批量回写 usage_count+1 与 last_usage,只动计数不改已注入内容;
//   - 淘汰(evict_over_capacity,升级工作流 B3):每角色 selected 条目超上限时把最低分
//     条目置 selected=0(只归档不删除,硬删走 prune);
//   - 去重(升级工作流 B4):insert 精确去重(content 相同则计数 +1),蒸馏时 Jaccard 近似去重。
// 工具写入(tools/memory.rs)与手动添加(kind='manual')共用同一张 memory_entries 表。
//
// QUALITY-FIX Q1-5(2026-09-27):本模块由单文件 1662 行拆成目录模块——`mod.rs` 只留
// **公共词汇**(pub 类型/常量 + 私有 ENTRY_COLUMNS)与 re-export,实现分 service / recall /
// distill / vectors 四个子模块,单元测试搬到 `tests.rs`。纯移动、不改行为;
// 外部 `crate::services::memory_service::<名>` 路径因此零改动。

use crate::models::db::Db;
use crate::services::settings_service::RuntimeSettings;
use serde::Serialize;
use std::sync::{Arc, Mutex};

mod distill;
mod recall;
mod service;
mod vectors;

// 子模块内定义的公共项在此 re-export:外部 `crate::services::memory_service::<名>` 路径零改动。
pub use distill::{distill_system_prompt, distill_user_text, parse_distilled_lines};
pub use recall::{
    jaccard_similarity, select_for_injection, select_recall, select_recall_hybrid,
    tokenize_for_similarity,
};

#[cfg(test)]
mod tests;

/// 记忆条目(memory_entries 表行)
#[derive(Debug, Clone, Serialize)]
pub struct MemoryEntry {
    pub id: i64,
    pub character_id: String,
    pub source_session_id: Option<String>,
    /// distilled | tool | manual
    pub kind: String,
    pub content: String,
    pub usage_count: i64,
    pub last_usage: Option<String>,
    pub selected: bool,
    pub created_at: String,
    pub updated_at: String,
    /// 分层注入最高优先级(1 = 常驻置顶)
    pub pinned: bool,
}

/// 蒸馏结果摘要
#[derive(Debug, Serialize)]
pub struct DistillOutcome {
    pub character_id: String,
    pub inserted: usize,
    /// 近似去重跳过的条数(与已有记忆或本批已接受行重复)
    pub skipped: usize,
    /// 本次新增记忆的 id(供调用方按需补向量,免去重扫全表)
    pub new_ids: Vec<i64>,
}

/// 向量索引状态(Phase 3;供设置界面展示)
#[derive(Debug, Default, Serialize)]
pub struct VectorStatus {
    /// 记忆总条数
    pub total: i64,
    /// 已生成向量的条数
    pub embedded: i64,
    /// 当前向量表维度(None = 表未建)
    pub dim: Option<u32>,
    /// 维度漂移记录(如 "1536->1024"),非 None 表示需重建索引
    pub dim_mismatch: Option<String>,
}

pub struct MemoryService {
    db: Arc<Db>,
    /// 运行期设置句柄(淘汰/预算等阈值来源)。由 AppState 在设置就绪后 attach;
    /// 未 attach(单测/工具单测)时按默认值工作。
    settings: std::sync::OnceLock<Arc<Mutex<RuntimeSettings>>>,
}

/// 单次蒸馏最多落库的记忆条数(防 LLM 长输出灌爆记忆库)
pub const MAX_DISTILLED_LINES: usize = 64;

/// 蒸馏近似去重的 Jaccard 相似度阈值(≥ 视为重复跳过;调此常量即可收紧/放宽)
pub const DISTILL_DEDUP_JACCARD_THRESHOLD: f64 = 0.8;

/// 通道 2 检索召回条数上限(升级工作流 B2)
pub const RECALL_LIMIT: usize = 3;

/// memory_read 工具读取上限(精选排序后截断,防无界返回)
pub const MEMORY_READ_LIMIT: usize = 50;

/// 每角色记忆容量上限默认值(升级工作流 B3;0 = 不限制淘汰)
pub const DEFAULT_MEMORY_MAX_ENTRIES: u32 = 200;

/// 记忆槽字符预算默认值(升级工作流 B2;0 = 不限制)
pub const DEFAULT_MEMORY_INJECT_CHAR_BUDGET: u32 = 2000;

const ENTRY_COLUMNS: &str =
    "id, character_id, source_session_id, kind, content, usage_count, last_usage, selected, created_at, updated_at, pinned";

/// 混合召回的打分权重:向量(语义)0.7 + Jaccard(词面)0.3。
/// 向量对同义改写敏感、Jaccard 对精确术语敏感,两路互补;
/// 任一不可用时退化为单路(纯 Jaccard 或纯向量)。
pub const RECALL_VEC_WEIGHT: f64 = 0.7;

pub const RECALL_JACCARD_WEIGHT: f64 = 0.3;
