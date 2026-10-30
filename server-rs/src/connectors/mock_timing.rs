// Mock 连接器的逐字流式节流(2026-10-30 提效批)。
//
// 拆成独立模块的理由有两个,都不是纯粹的尺寸洁癖:
//   1. mock.rs 已顶在文件大小 ratchet 基线(1270 行)上,这段逻辑与「回复内容编排」
//      无耦合,抽出来才能让主文件继续下降而不欠技术债;
//   2. 这里的判据是**构建配置 × 环境变量**的复合,事故面在「测试为什么突然又慢了」,
//      单独成文比塞在两千行文件中部更容易被找到。
use std::sync::OnceLock;
use std::time::Duration;

use crate::models::types::LlmMessage;

/// 慢流哨兵:消息含该子串时,该轮生成强制走 8ms/字符(测试构建默认零延时)。
///
/// 用无括号形式是因为它常嵌进计划 JSON 的 goal 字符串,而 `reply_if` 等钩子按
/// 「首个 `]]`」截断内容——带括号的标记会被当成结束符,把整份计划截断。
pub(crate) const SLOW_STREAM_SENTINEL: &str = "__KEDAI_SLOW_STREAM__";

/// 本轮该用的字符间延时(mock 生成循环的唯一入口)。
///
/// 判据扫**全部消息**而非只扫最后一条 user:custom 流程把任务 goal 渲染进 system
/// 提示词(见 `task_engine/custom/step.rs` 的 `build_step_system`),只看 user 会漏。
pub(crate) fn char_delay_for(messages: &[LlmMessage]) -> Duration {
    if messages
        .iter()
        .any(|m| m.content.contains(SLOW_STREAM_SENTINEL))
    {
        Duration::from_millis(8)
    } else {
        mock_char_delay()
    }
}

/// 逐字流式的字符间延时(生产/演示默认 8ms;测试构建默认零延时)。
///
/// **零延时必须跳过 sleep 调用本身**:Windows 上 `sleep(Duration::ZERO)` 仍耗一个
/// 约 8ms 的定时器周期。改前 `tests\undo.rs` 单文件 489s(占全量门禁 21 分钟的 36%),
/// 单条 chat/send 的 22s 全花在 330 字符 × ~8ms 的纯等待上,与「测什么」无关。
///
/// 需要「生成仍在进行中」窗口的用例不要依赖本延时(那是墙钟赌博),应在 goal / 消息里
/// 带 [`SLOW_STREAM_SENTINEL`] 显式申请——窗口由用例自己声明,与其他用例的默认值解耦。
pub(crate) fn mock_char_delay() -> Duration {
    static DELAY: OnceLock<Duration> = OnceLock::new();
    *DELAY.get_or_init(|| {
        // 测试进程默认零延时:cfg(test) 覆盖 lib 单元测试,`feature = "test-support"`
        // 覆盖 tests/** 集成测试(独立 crate,看不到 cfg(test))。两处都不命中即
        // 生产/演示构建 → 保留 8ms 手感。环境变量始终可覆盖,便于排查慢流问题。
        let in_test_build = cfg!(any(test, feature = "test-support"));
        if in_test_build && std::env::var_os("KEDAI_MOCK_CHAR_DELAY_MS").is_none() {
            return Duration::ZERO;
        }
        std::env::var("KEDAI_MOCK_CHAR_DELAY_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(8))
    })
}
