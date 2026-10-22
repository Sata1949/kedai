# P0 任务编排修复交接（2026-10-22）

> 本文件是给下一次会话/下一位维护者的交接说明：改了什么、验证到哪一步、还剩什么。
> 对应实测报告：`D:\kedai-bench-run\mcisland2-evidence\FINAL-REPORT.md`（仓外证据）。

## 一、背景

2026-10-21 用真实模型 `deepseek/deepseek-v4.1-flash-fast` 跑「我的世界悬浮空岛」team 任务时暴露两个 P0：

1. **失败循环的 token 不记账**：主 Agent 2 在 `parts/ui.js` 子目标跑了 234 轮后因上游错误收尾，`task_llm_calls` 记 `0 + 0` token，`usage_total` 完全缺失这段消耗。
2. **自检循环没有止损**：主 Agent 3 写出 `index.html` 后继续用 `bash`/临时文件自检，单主跑到 139 轮；只读守卫把 `bash` 归为写类而清零，语义熔断又因每轮命令/输出不同抓不住；单步预算 3600 秒远未到点。

本批只修这两个 P0，不改 P1/P2（上游重试、team 依赖图、调用面板文案等）。

## 二、P0-1：失败/中止循环的 usage 记账

### 改动

- `server-rs/src/services/task_engine/solo.rs`
  - 新增 `record_failed_loop_usage()`：把 `run_agent_loop` 已累计的 `total_usage` 写入 `record_usage`。
  - 三条失败路径都补记账：
    - 空正文重试后仍空（`Ok(res)` 且 `text.is_empty()`）；
    - 用户中断（`Ok(_)`，`res.interrupted == true`）；
    - 上游/工具错误（`Err(e)`）。
  - 错误/中断的 `record_llm_call` 行改用 `row_usage`（空正文重试时是增量，避免与首轮空行双计），`usage_total` 用整轮累计。
- `server-rs/src/connectors/mock.rs`
  - 新增测试钩子 `[[tool_loop_fail:name|N args]]`：前 N 轮返回 ToolCall，第 N+1 轮返回上游错误。既有 `[[fail:]]` 首轮即失败，构造不出「先跑 N 轮再失败」的形态。

### 语义与边界

- 成功路径仍由调用方（solo/plan/followup/team）按返回的 `TokenUsage` 记一次，未改。
- 失败路径现在也进 `usage_total`；`task_llm_calls` 错误行不再是 0 token。
- **已知未覆盖**：`run_tool_loop` 返回 `Err` 时，`self_heals`（截断自愈的独立用量）随错误被丢弃，失败循环的截断尝试仍可能少记；这是 P1，不在本批。
- 调用行求和口径：空正文重试时首轮空行 + 最终行增量 = 整轮累计；`usage_total` 只记整轮一次，保持既有「各行 token 求和 == usage_total」不变量。

### 回归测试

- `server-rs/tests/task_usage_on_failure.rs`
  - 构造 solo 任务：`[[tool_loop_fail:sleep|3 {"ms":1}]]`。
  - 断言终态 `error`、`usage_total.prompt_tokens/completion_tokens > 0`、`phase=agent` 的错误调用行也非零。
  - 修复前实测红（`usage_total` 为 0，错误行 0 token）；修复后绿。

## 三、P0-2：声明文件无进展守卫

### 改动

- `server-rs/src/models/types.rs`
  - `ToolBudget` 新增 `no_progress_limit: Option<usize>`（Copy 数值，聊天路径 `None`）。
  - `ExecScope` 新增只读访问器 `declared_files()`。
- `server-rs/src/services/task_engine/mod.rs`
  - 新增 `TASK_NO_PROGRESS_LIMIT: usize = 16`（单一出处；注释写明 2026-10-21 的 234/139 轮实测）。
- `server-rs/src/services/task_engine/solo.rs`
  - 任务侧 `ToolBudget` 填 `Some(TASK_NO_PROGRESS_LIMIT)`。
- `server-rs/src/services/task_engine/custom.rs` / `team_audit.rs` / `task_service/executor.rs`
  - 显式补 `no_progress_limit: None`（custom/审计/规划侦察没有规划器声明文件，不启用）。
- `server-rs/src/agents/engine/executor.rs`
  - 新增纯函数 `is_declared_file_write()`：只认带 `path` 的 `fs_write`/`fs_edit`，路径命中 `scope.declared_files()`；`bash`、临时文件、越界路径、非法 JSON、空声明一律不算。
  - `run_tool_loop` 每轮统计 `round_progress`；连续 `TASK_NO_PROGRESS_LIMIT` 轮没有成功写入声明文件时，发 `agent_status` 事件「无文件进展收尾」并 **带已有产出返回 Ok**（步骤 done，不判失败），`budget_stopped=false`。
  - 守卫只在 `scope` 带声明文件（team 主 agent）时启用；solo/multi 未声明文件不触发。
- `server-rs/tests/task_no_progress_guard.rs`
  - 独立测试进程；先 `PUT /api/settings {"loop_guard_semantic_min_calls":0}` 关闭语义熔断，避免 mock 重复写同一临时文件先被语义熔断在第 4 轮。
  - team 规划器声明 `out.txt`，主循环 30 轮只写未声明的 `temp.js`；断言第 16 轮出现「无文件进展收尾」、`fs_write` 事件 ≤16、步骤仍 done、没有语义熔断事件。
- `server-rs/tests/tasks_modes_team.rs` 曾加过同题用例，后移到独立文件（语义熔断已关闭的测试必须独立进程，避免污染同文件其他 team 用例）。

### 语义与边界

- 守卫从第 1 轮开始计数；一个子目标若连续 16 轮还没有写声明文件，会带已有正文收尾。这与 TMR-1「最多先读 3 个直接相关文件就落盘」一致，但比只读守卫的 8 次提醒更晚。
- 只认「调用意图 + 成功结果」：`fs_write` 成功返回（非 `Value::Null`、无 `error` 键）即算进展；**不比较内容哈希**，同内容重写也算进展。
- `fs_patch` 的补丁目标暂不解析（参数是补丁文本，不是单一路径），因此不会计为进展；编码包默认关，本批实测链路未使用。
- `bash` 写文件不算进展（TMR-L1 既有边界）；如果某个 team 任务只用 bash 写声明文件，可能被守卫提前收尾。后续 P1 可改用 `task_file_changes` 台账作为进展源。
- 没有加「每主硬轮次上限」：一个模型如果每 ≤16 轮就写一次声明文件，仍可跑到单步 3600 秒预算。这是 P1 的全局预算问题，本批不做。

## 四、验证记录

### 已跑

- `cargo fmt --manifest-path server-rs/Cargo.toml --all` → EXIT 0。
- 定向回归：
  - `task_usage_on_failure`：1 passed。
  - `task_no_progress_guard`：1 passed。
  - `task_loop_budget`：6 passed（含 step budget、readonly nudge/break、workspace partial/done）。
  - `tasks_modes_team`：13 passed（team 规划/审计/打回/所有权/交付门槛/停止）。
- `node tools/count-tests.mjs --check` → EXIT 0（后端 1885 = 1445 单测 + 440 集成 / 66 文件；前端 1633 / 156）。
- `node tools/check-arch.mjs` → EXIT 0（无新增架构违规）。
- `cargo clippy --all-targets -j 8 -- -D warnings` → EXIT 0。
- **全量 `cargo test -j 8`：EXIT 0**（71 个测试二进制/文档测试全部 `test result: ok`，无 FAILED；总数 1885 = 1445 单测 + 440 集成）。

### 真实模型复跑（2026-10-22，新二进制）

- 隔离实例：`DATA_DIR=D:/kedai-bench-run/data-mcisland3`，端口 3014，模型 `deepseek/deepseek-v4.1-flash-fast`；工作区 `mcisland3-work`；任务 ID `6a90be39-42ef-499e-91ad-c7fe84d20da4`；driver 上限 30 分钟。
- **P0-1 已验证**：主 1 的两个 300s 超时失败循环在 `task_llm_calls` 里分别记 `6,149/103` 与 `6,206/158` prompt/completion；其他 error 行 `108,456/1,819`、`52,395/1,138`，**不再是 0**；任务 `usage_total` 包含这些失败循环。
- **P0-2 已验证**：主 3 子目标 3 在连续 16 轮没有写入声明文件后触发 `无文件进展收尾`（事件 seq 208，2026-10-22T02:01:53Z，循环累计 186,422 token）；同一主更早的 smoke 子目标先被语义熔断在第 33 轮（seq 138）。两条守卫互补，均按设计带已有产出收尾。
- **但整题仍未 done**：driver 到 30 分钟上限主动 stop，终态 `ended`；9 步中只有 `parts/smoke.js` 完成，其余 8 步多为 `模型调用超过 300s 未完成(上游停滞或输出过慢)`，`index.html` 未产出；最终 usage 916,370 prompt / 69,617 completion，缺失全部 8 个必需文件（smoke.js 已存在且不在缺失清单）。
- 结论：**P0 的两个目标缺陷已修复并在真实链路验证**；但「单个子目标无进展」不等于「整任务有全局预算」——上游 300s 超时 + 每子目标各自 3600s 预算仍会让团队任务跑到外部上限。P1 的全局/每主预算与流中断重试仍是下一步必需项，不能把本批当作「团队模式已稳定 done」。

### 待补

- 无（P0 定向/全量/真实模型证据已齐）。P1 见下节。

## 五、下一步（P1/P2，未做）

1. **上游流中断的子目标级重试**：连接器只在首字节前重试（`connectors/openai_compatible/mod.rs` + `retry.rs`，MAX_ATTEMPTS=3）；SSE 读取阶段 120 秒停滞/body 断开后直接失败，任务层不判断 `LlmError::retryable()`。建议对 Timeout/Upstream 做一次带退避的子目标重试。
2. **team 依赖图/总装顺序**：当前 `run_mains_parallel` 纯并行，规划器把 `index.html` 和 `parts/*` 分给不同主时，总装主无法等待 parts。建议：
   - 单文件交付时规划器不拆冗余 parts；或
   - 把总装放到同一主的最后一个子目标（串行）；或
   - 给 team 增加显式依赖图。
3. **调用记录面板文案**：`web/src/components/CallTracePanel.vue` 把每条 agent 循环汇总行标成「LLM 调用」，错误行显示 0 token（P0-1 已让新任务非零，但历史任务/失败路径仍可能显示 0）。建议改为「工具循环」/「调用汇总」，并显示「未记录」而非 0。
4. **全局预算**：`task_step_budget_secs` 默认 3600、`task_total_budget_secs` 默认 0；team 每子目标重置。可考虑 team 默认总预算或每主墙钟上限。
5. **`exec_enabled` 默认关**：编码类任务初始化脚本应显式打开，否则前几轮 bash 全被拒（本次实测前 7 分钟）。
6. **失败循环的 self-heal 用量**：`run_tool_loop` 的 Err 不带 `self_heals`；失败循环的截断尝试仍可能少记。可让 EngineError 携带已累计 usage/self_heals，或在 `run_tool_loop` 内直接落账。

## 六、文件清单

| 文件 | 作用 |
|---|---|
| `server-rs/src/services/task_engine/solo.rs` | P0-1：失败/中止 usage 记账；ToolBudget 填无进展上限 |
| `server-rs/src/agents/engine/executor.rs` | P0-2：声明文件判定、每轮进展统计、无进展收尾 |
| `server-rs/src/models/types.rs` | `ToolBudget.no_progress_limit` / `ExecScope.declared_files()` |
| `server-rs/src/services/task_engine/mod.rs` | `TASK_NO_PROGRESS_LIMIT=16` |
| `server-rs/src/services/task_engine/custom.rs` / `team_audit.rs` / `task_service/executor.rs` | 其余 `ToolBudget` 构造点显式 `None` |
| `server-rs/src/connectors/mock.rs` | `[[tool_loop_fail:...]]` 测试钩子 |
| `server-rs/tests/task_usage_on_failure.rs` | P0-1 集成回归 |
| `server-rs/tests/task_no_progress_guard.rs` | P0-2 集成回归 |
| `MAINTENANCE.md` | 测试计数更新（1885 / 1445 / 440 / 66） |

## 七、提交与分支

- 目标分支：`fix/p0-task-usage-no-progress`（从 `main` 新建；不要在 `main` 直接提交，仓保护要求 PR）。
- 代码提交：`1fd04ab`（P0-1 + P0-2 + 两个集成测试 + MAINTENANCE 计数）。
- 实际提交粒度（两个提交，保证每个提交可独立编译/过计数检查）：
  1. `fix(任务模式): P0 失败循环记账 + 声明文件无进展守卫`（代码 + 两个集成测试 + MAINTENANCE 计数）
  2. `docs(台账): P0 修复交接文档`（本文件）
- 推送前：`cargo fmt`、全量 `cargo test -j 8`、`npm run check`（或至少 `cargo test` + `count-tests` + `check-arch`），PR 等 CI 双绿后合并。
- 本批不涉及前端代码；`CallTracePanel` 文案是 P2，未改。
