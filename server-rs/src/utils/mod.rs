// 工具模块:结构化 JSON 日志(tracing,D-4 迁移)、原子文件写入、截断自愈预算决策、循环熔断守卫
//
// 代际: L1(老层·稳 / Anchored Core)——基础纯函数,零 `crate::` 出边。
// 判据: 被各层普遍依赖的底层能力;`retry.rs` 的截断重试决策是**纯函数**,
//       其行为被多处共用(四份重复实现已于 2026-09-13 收敛至此);
//       `loop_guard.rs` 的重复指纹熔断同为通用纯逻辑(2026-09-14 抽出)。
// 纪律: 保持无状态、无业务语义依赖。
pub mod fs_atomic;
pub mod logging;
pub mod loop_guard;
pub mod retry;
