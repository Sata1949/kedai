---
name: kedai-backend-core
description: Kedai Rust 后端（server-rs/）的落点地图与硬约束。当需要判断一段后端逻辑该写在哪个模块、改动是否越了分层、要新增内置工具、要动任务引擎或提示词注入、要加 SSE 事件或设置字段时使用。含三结合分层的 L1/L2/L3 归属、越代依赖禁令、prompt_kit 共享原语、任务引擎六模式与守卫、新增工具的 8 处登记清单。触发词：server-rs、后端、services、api、models、新增工具、任务引擎、SSE 事件、设置字段、分层、L1、L2、L3。
---

# Kedai 后端：落点地图与硬约束

## 一、先定位：这段代码属于哪一层

`tools/arch-layers.json` 是**机器可读的 SSOT**，`check-arch` 规则 I/J 守着它。改动前后端模块前先在这里查归属：

| 代际 | 后端模块 | 含义 |
|---|---|---|
| **L1 老层 · 稳** | `parsing` `models` `contracts` `migration` `utils` | 长期验证的兼容契约。数据/协议形状一变就破坏既有用户数据或外部生态 |
| **L2 中层 · 干** | `agents` `services` `api` `connectors` `slash` `tools` | 业务编排与协议翻译 |
| **L3 青层 · 活** | `mcp` `plugins` `scripts` | 新能力、实验、隔离执行 |
| **entry 入口** | `bin` | 只做装配与配置解析 |

**允许的依赖方向（后端）**：`L1→L1`、`L2→L1`、`L2→L2`、`L3→L1`、`L3→L3`。

**禁止**：`L1→L2`、`L1→L3`、`L3→L2`、`L2→L3`。

后端是**隔离模型**：L2 与 L3 双向互斥——青层反向依赖骨干层会让「隔离」名存实亡。前端是另一套（层次模型，额外允许 `L3→L2`，因为组件读 store 是正常方向）。

判断口诀：**写业务编排进 L2，写新能力/沙箱进 L3，改契约形状进 L1（且要极其谨慎），只装配进 entry。**

越代依赖不是绝对不能有，但必须登记进 `arch-layers.json` 的 `registeredEdges`，而且那是**债务清单——只减不增**。为了过检查去加条目是明令禁止的。

## 二、提示词相关的一切都走 prompt_kit

`server-rs/src/services/prompt_kit.rs` 是提示词共享原语的**唯一出处**：世界书过滤、注入合成、占位符渲染、`untrusted_boundary`。引擎侧与任务侧都调它。

**不要在任一侧复制实现。** 这是本仓库反复强调的一条——复制出去之后两边会各自漂移，而提示词漂移不会报错，只会让模型行为莫名其妙地变差。

主要原语：`constant_world_body`、`triggered_entry_hits(_scoped)`、`system_inject_text`、`render_character_placeholders`、`untrusted_boundary`、`select_segments_within_budget`、`append_vision_discipline_if_needed`、`append_write_first_discipline_if_needed`。

## 三、任务引擎：六模式的统一派发点

`server-rs/src/services/task_engine/mod.rs` 是**唯一派发点**，按 `task.task_mode` 构造对应的 `ModeExecutor`：legacy / solo / plan / multi / team / custom。各执行器在自己的子模块里。

改任务行为前必须知道这几条（都是实测踩出来的）：

- **轮次默认不限**：任务主循环用 `Some(0)` 哨兵；聊天 AGENT 仍用 `settings.max_tool_rounds`（默认 32）。两者不要互相污染。
- **连续只读守卫**：`ToolBudget.readonly_streak_limit = 8`，先提醒再熔断。纯读工具（`fs_read`/`fs_glob`/`fs_grep`）用**整窗阈值**（7 次不熔断、第 8 次才熔断）。
- **`guard_stopped()` 才是守卫截断的判据**。别用别的字段判断守卫是否生效——这是 C-2C 批记下的坑。
- **team 集成主的豁免**：`TeamMain.after` 非空即识别为集成主，此时**关闭**连续只读与无进展两个计数闸门。审计/终审的语义熔断用 `min_calls=0` 显式关闭（`semantic_guard=None` 是「回退聊天侧值」，**不是关闭**——这两个语义不要混淆）。
- **文件交付门槛**：`task_engine/delivery.rs`。绑定工作区 + 写工具 + Solo/Multi/Team；声明了必需文件却缺失 → Done 降 Partial；没声明时零文件变更也降级。
- **写所有权**：`models/ownership.rs` 的 `WriteOwnership`（声明归属 + 首写者认领）拦截 `fs_write`/`fs_edit`/`fs_patch` 的跨主写。**`bash` 写文件仍不受约束**（已知边界 TMR-L1，别当缺陷改）。
- **拆分子模块的坑**：`tests.rs` 必须以 `#![cfg(test)]` 开头；子模块要用 `crate::` 绝对路径而不是 `super::`；跨模块调方法要标 `pub(super)`。

## 四、新增内置工具：8 处登记（漏改会静默降级）

权威清单在 `docs/契约-协议与配置.md` 的「新增内置工具登记清单」一节。**漏改不报错，只是行为悄悄变弱**，所以必须逐处走完：

| # | 位置 | 必/条件 |
|---|---|---|
| 1 | `tools/mod.rs::register_builtin_tools` | 必改 |
| 2 | `tools/permissions.rs::default_risk` | **必改**——未登记一律落 `_ => Dangerous` |
| 3 | `tools/tool_sets.rs` | 只读工具要进 `READONLY_SCOUT`/`SUBAGENT`/`REFLECT`，否则规划侦察/子 agent/反思看不到它 |
| 4 | `tools/registry.rs::tool_when` | 漏改则回退 description |
| 5 | `tools/registry.rs::render_kind_for` | 前端渲染意图 |
| 6 | 前端 `utils/agentFlowTools.ts` / `AgentFlowSection.vue` | 流程白名单 |
| 7 | `services/undo_service.rs` 的 `WRITE_TOOLS` | **条件性**：仅新写类工具。不登记则**静默没有**「回退到此处」快照 |
| 8 | `services/task_engine/tool_policy.rs` 的按名例外 | **条件性**：仅「危险级但任务模式需要」的工具。不登记会被 `deny_dangerous` **静默剔除** |

第 2 与第 3 处最易漏：漏第 2 会落 `Dangerous` 兜底（策略把它剔掉、风险面板显示错级别）；漏第 3 会让所有模式都看到本该受限的工具。

注意还有个 `state_anchor_arg`：写类工具的锚点参数走**键制**（如 `fs_write` → `path`），但像 `fs_patch` 这种补丁目标是一组路径的走**值制**，不进键制——误补会让压缩后的上下文指错文件。

**这份清单目前无机检**（`遗留.md` CFG-2 记着）。收敛方向是把 1–5 合并为单一工具清单结构，或用元测试做穷尽检查。在那之前只能靠纪律。

## 五、SSE 与事件

数据流：DB 写入成功后 `TaskService` 经 broadcast 发射 `SseEvent::Task` → `GET /api/tasks/events` 转发为 SSE → 前端 `web/src/stores/task.ts` 事件驱动精确刷新（无轮询；断线指数退避 + 5s 兜底轮询）。

**新增事件 kind 要前后端四处同步**：枚举 `TaskEventKind` / 发射点 / `web/src/api/types/` 的 TS union / `web/src/stores/task.ts` 的刷新映射（末项由 `never` 穷尽断言兜底）。另须同步 `docs/契约.md` 两处变体计数，否则 `check-doc-claims` 红。

broadcast 在压力下会 `Lagged` 丢帧——**测试断言不能要求「某帧必现」**，要取「SSE ∪ REST 补拉」的并集。

## 六、配置与数据目录

`server-rs/src/config.rs` 解析 `DATA_DIR`，三级优先级：环境变量 > `%APPDATA%\com.kedai.app\data\`（已含用户数据时） > 项目根 `data\`。

**隔离实例做实测时的前置纪律**：启动后**先核对日志里的 `"data_dir":"..."` 行**，再做任何写操作。曾经因为 `.env` 的 `OPENAI_*` 不生效而对着错误的库跑了半天——机制是「目标 DATA_DIR 已有 `settings.json` 时 `.env` 不生效」（`经验.md` E76）。

## 七、设置的形状

- 双模式提示词隔离由**类型**承载（`RoleplayPromptConfig` / `TaskPromptConfig`），settings.json 线格式不变。
- 新增设置字段：补 `web/src/stores/genSettings.test.ts` 夹具，并过 `check-settings-parity` 的三组对拍（PUT 请求体 / GET 投影 / 连接元素）。**GET 侧比对的是 `settings_json()` 的手工投影键**——只比结构体会漏掉「加了字段忘投影」。
- 新增受限字段必须登记 `patch/*.rs` 派生的取值域规则，否则 FAIL。

## 八、测试落点

- 单元测试与源码同文件 `#[cfg(test)] mod tests`；集成测试在 `server-rs/tests/`。
- 临时目录一律 `utils/test_support.rs` 的 `TempDataDir::new("<tag>")`——详见 `kedai-batch-dev` 的析构顺序约束。
- `build_test_app` 是 `OnceLock` 进程级单例，**不要**给它套临时目录守卫（第一个用例结束就把后续要用的库删了）。
- 规则 M 守着「`build_test_app` 不得被生产入口引用」——集成测试链接的是非 test 构建的 lib，`#[cfg(test)]` 门控在这里不可行。

相关技能：`kedai-batch-dev`（流程）、`kedai-gate-runner`（门禁）、`kedai-frontend-core`（前端对应面）。
