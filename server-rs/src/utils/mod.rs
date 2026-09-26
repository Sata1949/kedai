// 工具模块:结构化 JSON 日志(tracing,D-4 迁移)、原子文件写入、截断自愈预算决策、循环熔断守卫、Windows 进程派生常量、测试临时目录守卫
//
// 代际: L1(老层·稳 / Anchored Core)——基础纯函数,零 `crate::` 出边。
// 判据: 被各层普遍依赖的底层能力;`retry.rs` 的截断重试决策是**纯函数**,
//       其行为被多处共用(四份重复实现已于 2026-09-13 收敛至此);
//       `loop_guard.rs` 的重复指纹熔断同为通用纯逻辑(2026-09-14 抽出);
//       `win.rs` 的 CREATE_NO_WINDOW 是跨 crate 共用的进程派生常量(2026-09-18),
//       仅 windows 编译;`test_support.rs` 的 TempDataDir 是测试专用 RAII 守卫
//       (2026-09-15),仅 cfg(test)/test-support feature 编译,不进生产产物。
// 纪律: 保持无状态、无业务语义依赖。
pub mod blocking;
pub mod fs_atomic;
pub mod logging;
pub mod loop_guard;
pub mod retry;
// Windows 进程派生常量:仅 windows 编译(引用方须在 #[cfg(windows)] 块内使用)
#[cfg(windows)]
pub mod win;
// 测试临时目录守卫:单元测试走 cfg(test),集成测试经 test-support feature(Cargo.toml 自引用 dev-dependency)
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
