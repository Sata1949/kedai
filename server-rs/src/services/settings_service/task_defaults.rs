// 任务向缺省生成参数与长程一致性推荐档快照（CARD-1 文件大小治理：自 params.rs 迁入）。
// 纯代码搬移，文本与数值不变；settings_service/mod.rs 再导出保持既有路径。
use serde::{Deserialize, Serialize};

/// 采纳推荐档**之前**的压缩三项快照(LIT-7):只在用户显式选档时拍摄一次,选回「不改变」时
/// 按它恢复到**写入前原值**——与「恢复默认值」是两回事(有测试锁,两者必须区分)。
/// 档间切换**不重拍**:快照始终是「采纳前」的状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiteraryRecommendSnapshot {
    pub compaction_mode: String,
    pub compaction_threshold: f32,
    pub compaction_keep_recent: u32,
}

/// 任务向缺省生成参数(TM-SET-1,2026-10-01;TPROF-1,2026-10-27 抬档):任务覆盖层未显式
/// 配置时,任务模式的有效温度 / Top-P / 单次输出上限取本组值。任务执行以工具循环与多步
/// 执行为主,低温 + 不截断采样 + 足够产出预算是更稳的档位;角色扮演侧的扁平缺省
/// (0.8 / 0.9 / 1024)不受影响。
///
/// `TASK_DEFAULT_OUTPUT_TOKENS` 沿用 TM-GEN-1 的「只抬不压」语义:None 时任务有效值取
/// `max(扁平值, 本值)` 而非直接替换——扁平值更大时保持更大,不被本值压低(理由:带工具/
/// 推理的 agent 轮会被推理 token 吃光,TM-D6 实测 1024 曾被 reasoning 整体烧尽导致正文零输出)。
/// 任务覆盖层的显式值(`Some`)一律逐字优先。
///
/// 2026-10-27 TPROF-1 起缺省输出上限抬到 32768:代码类单文件产出(HTML/TS)常超过 8K,
/// 推理模型的 reasoning 也计入该预算;32768 让首轮尽量不截断,截断自愈仍保留。
pub const TASK_DEFAULT_TEMPERATURE: f64 = 0.3;
pub const TASK_DEFAULT_TOP_P: f64 = 1.0;
pub const TASK_DEFAULT_OUTPUT_TOKENS: u32 = 32768;
