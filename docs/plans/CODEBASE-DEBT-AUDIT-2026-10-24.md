# Kedai 代码结构债审计（2026-10-24）

> 日期：2026-10-24
> 范围：`server-rs/src`（128,280 行）、`web/src`（77,235 行）、`tools/`、`launcher/`、`start.ps1` 与 `docs/` 结构；审计时 HEAD `d04ab5a`
> 方法：`churn × size` 热点、函数/文件规模、跨文件重复块、三路只读深审（后端架构 / 前端组件与状态 / 脚本文档卫生）、静态门禁实跑、git 历史考古；关键 P0/P1 均由主审逐条回代码复核
> 边界：只判「意外复杂度/维护性债」，不把领域固有复杂度（EJS 解释器、六模式语义、文档台账）算作屎山；未做安全专项，未跑全量测试与真实模型复跑
> 性质：只读审计报告；不改变任何生产代码与计划台账状态

---

## 一、结论摘要

**有屎山，但不是整仓。** 这个仓库的门禁、测试和文档台账明显高于同类个人项目：架构分层有机器门禁、跨端契约有 29 组映射、计数与文档口径有硬检查、大组件基本都有测试。真正的问题是**三个高改动核心区域在高速迭代中长成了「局部屎山」**，而且仓库自己的 `C-2` 计划已经记录过同一趋势——只是长期未启动。

另外，本次审计抓到 **2 个应先修的用户可见问题** 和 **1 个控制面不一致**；它们不是纯重构能顺手解决的，建议独立成 `C-2-0` 先行。

三条结构性根因，按危险程度排序：

1. **同一个决策有多个停止出口，且用字符串表达。** `agents/engine/executor.rs::run_tool_loop` 单函数 1,046 行、57 个 `if`、7 个循环、15 个参数；token 预算、步骤墙钟、轮次上限、连续只读、声明文件无进展、语义熔断、重复调用熔断各自 `return Ok(ExecutorResult { ... })`，`guard_stopped: Option<String>` 再把「为什么停」变成下游靠字符串判断。每加一道守卫，就要在同一函数里再插一段、再构造一次终态；SKY-6/SKY-7 的反复误伤与豁免就是这种结构的直接产物。
2. **设置 patch 是 75 字段的上帝端点。** `api/settings.rs::update_settings` 824 行（去注释约 700 行）、152 个 `if`、34 个 `return validation`；`UpdateSettingsBody` 75 个字段、`RuntimeSettings` 77 个字段、`ModeSettings` 34 个字段；前端 `RuntimeSettings` 77 / `RuntimeSettingsPatch` 75 个字段手工镜像。新增一个设置要同步后端结构、默认值、patch 分支、校验、前端类型、`genSettings` 和测试，是典型的 shotgun surgery。更糟的是：同一函数内有些非法值返回 400，另一些静默忽略并返回 200。
3. **前端任务台是单组件承载全页。** `TaskBoard.vue` 1,903 行（script 681 / template 623 / style 594），32 个 `computed`、20 个函数、44 个 `v-if`、13 个 `v-for`；任务头、状态矩阵、批准区、计划对话、步骤/团队/子任务、流式输出、成果卡、交付校验、文件变更、对话记录、追加指令全部在一个文件里。`Sidebar.vue` 988 行、`ChatWindow.vue` 999 行是同一形态的第二、第三处。

**最关键的量化事实**：仓库的 `C-2` 条目（`docs/计划.md:6027`）在 2026-09-28 记录「后端 13 + 前端 2 = 15 个超千行文件」；本次实测（排除 `*.test.ts`、`tests/`、`*_tests.rs`）已是 **33 个非测试源文件 >1,000 行**，约 +120%。目标「≤6」不仅没接近，反而在加速远离。这不是新发现，而是既有计划失守。

---

## 二、量化证据

### 2.1 规模与热点

| 指标 | 实测值 | 说明 |
|---|---:|---|
| Rust 源码 | 128,280 行 / 368 文件 | `server-rs/src` |
| 前端源码 | 77,235 行 / 292 TS + 62 Vue | `web/src` |
| 文档 | 48,973 行 / 81 文件 | 六类台账；`功能-变更史` 10,499 行、`计划` 7,671 行 |
| Rust 函数总数 | 4,107 | 其中 **71 个 ≥120 行** |
| 生产代码 `unwrap/expect` | 约 39 处 | 排除测试文件/测试辅助；多数是解析器不变量 `expect`，不是主要风险 |
| 测试文件 unwrap/expect | 1,884 处 | 测试代码；不构成生产风险 |
| `#[allow(clippy::too_many_arguments)]` | 55 处 | 参数对象缺失的横切信号 |
| 超千行非测试源文件 | **33 个** | C-2 在 2026-09-28 记 15 个 |
| 已登记架构债 | 5 条 | 3 条后端 wiring（组合根/暴露面），2 条前端真实回边 |
| 组件直改 state 白名单 | 审计时 54 处；C-2-0 后 113 处（UI 48 + 表单级 65，待修 0） | `check-arch` 通过；是显式分类接纳，不是零债务 |
| 集成测试夹具重复 | 70 个 `test_app` / 70 个 `send_json` / 35 个 `test_lock` / 35 个 `wait_terminal` | `server-rs/tests/*.rs`；估算重复 1,700+ 行 |

### 2.2 高改动 × 大体积热点

| 文件 | 行数 | 历史提交数 | 主要风险 |
|---|---:|---:|---|
| `server-rs/src/services/task_engine/team.rs` | 3,276 | 35 | 规划/执行/审计/总装/预算五阶段同文件；`run_inner` 510 行；首审/终审两段同构 |
| `server-rs/src/agents/engine/executor.rs` | 2,941 | 42 | `run_tool_loop` 1,046 行；7 类停止出口 |
| `server-rs/src/models/types.rs` | 2,327 | 50 | 数据/传输/任务类型混装；已登记知情接受项 |
| `server-rs/src/services/settings_service/mod.rs` | 2,126 | 40 | `RuntimeSettings` 77 字段 + 默认值/持久化 |
| `server-rs/src/api/settings.rs` | 1,929 | 45 | 75 字段 patch + 824 行 `update_settings` + 340 行预览 |
| `web/src/api/types.ts` | 1,489 | 61 | 97 个导出类型；跨端手工镜像 |
| `web/src/components/TaskBoard.vue` | 1,903 | 29 | 单组件承载任务详情全页 |
| `web/src/stores/task.ts` | 1,023 | 22 | 事件映射/CRUD/流程绑定/持久化同店 |

### 2.3 增长曲线（同一文件的历史快照）

| 文件 | 2026-09 基线 | 2026-10-24 | 增幅 |
|---|---:|---:|---:|
| `executor.rs` | 1,298 | 2,941 | +127% |
| `team.rs` | 1,495 | 3,276 | +119% |

同期 `executor.rs` 的提交历史里，`SKY-6`、`SKY-7`、`TMR3-L1/L2`、`TMR3-L6/L7`、`TMR` 批、`HB-1/2/3/4` 连续出现；`team.rs` 同样密集。**这不是「文件大」本身的问题，而是同一处被反复打补丁**。

### 2.4 静态门禁实跑

本次实际跑过并通过：

| 命令 | 结果 |
|---|---|
| `node tools/check-arch.mjs` | 后端/前端无新增违规；5 条已登记债务；审计时 54 处直改 state；C-2-0 后 113 处分类（48 UI + 65 表单级，0 待修） |
| `node tools/check-contract.mjs` | 29 组映射一致 |
| `node tools/check-doc-claims.mjs` | 12 项计数口径一致 |
| `node tools/check-docs.mjs` | 六类体系/索引/链接/归档一致 |
| `node tools/check-css-discipline.mjs` | 4590 行基线、10 处 `!important`、59/40 间距裸值，未超基线 |
| `node tools/count-tests.mjs --check` | 后端 1967（1507 单元 + 460 集成）/ 前端 1641，与台账一致 |

**结论**：门禁不是摆设。问题恰好落在门禁覆盖不到的地方——函数长度、控制流密度、终态分支一致性、字段级同步成本、以及少数语法形态的漏检。

---

## 三、先修项（P0 / 用户可见）

### P0-1 设置 patch 对部分非法值静默忽略，仍返回 200

**位置**：`server-rs/src/api/settings.rs:702-781`

**证据**：

- `default_temperature` 越界（不在 `0.0..=2.0`）时只是不写入（`:702-706`）；`default_top_p` 同理（`:707-711`）。
- `mvu_vars_position` 非 `system`/`user_tail` 静默忽略（`:750-755`）；`preset_tail_role`（`:765-770`）、`reflect_advice_role`（`:776-781`）同样。
- 同一函数内 `default_max_tokens`、`max_context_tokens`、`max_tool_rounds`、`tool_history_keep_rounds` 等 30+ 处返回 400。

**为什么是债**：这些字段包含任务预算、熔断窗口、守卫阈值。前端收到 200 后认为「改成功了」，实际运行时仍是旧值，故障表现为「设置不生效」，排查成本极高。这不是风格问题，是控制面契约不一致。

**修复方向**：为每个字段声明明确策略 `Reject`（400）或 `Clamp`（夹到边界并在响应里回执），禁止第三种「静默忽略」；`C-2B` 的字段表顺带承载该策略。

### P0-2 `selectTask` 先改 id 再加载，加载失败会留下旧任务详情

**位置**：`web/src/stores/task.ts:518-537`、错误分支 `:462-464`

**证据**：

- `selectTask` 先写 `currentTaskId = id`，再 `await loadTaskDetail(id)`。
- `loadTaskDetail` 的 `catch` 只 `console.error`，不重置 `currentTask`。
- 结果：`currentTaskId` 是新任务，`currentTask` 仍是旧任务；`TaskBoard` 继续渲染旧标题/旧计划，批准、追加、删除等动作却发往新 id。

**修复方向**：加载前清空详情，或等详情成功后再提交 `currentTaskId`；失败时回退 id 并给可见错误。补一条「详情 500 时不得显示旧任务」的单测。

### P0-3 「清空当前任务」三条路径已经分叉

**位置**：`task.ts:536-545`（`clearSelectedTask`）、`:737-746`（`deleted` 事件）、`:970-979`（`deleteTask`）

**证据**：

- `deleted` 事件分支没有 `persistCurrentTaskId(null)`。
- `deleteTask` 没有清 `lastAgentStatus`。
- 三份复制各自少一项；新增状态字段时必然再漏一处。

**修复方向**：抽 `resetCurrentTaskState(options)` 单一函数，三处调用；补「删除事件后 localStorage 无残留」「deleteTask 后 lastAgentStatus 为空」测试。

---

## 四、结构性 P1

### 4.1 工具循环与守卫：单函数 1,046 行，7 类停止决策

**位置**：`server-rs/src/agents/engine/executor.rs:766-1811`

**证据**：

- 函数签名 15 个参数，`#[allow(clippy::too_many_arguments)]` 就在 `:765`。
- 函数内 57 个 `if`、7 个循环、18 个 `let mut`；分节注释覆盖：工具历史回灌上限、截断自愈、重复调用熔断、本轮工具执行、连续只读检测、token 预算闸门、声明文件无进展收尾。
- 7 个停止出口：token 预算 `:1564-1574`、步骤墙钟 `:1601-1611`、轮次上限 `:1630-1640`、连续只读 `:1683-1695`、无进展 `:1724-1736`、语义熔断 `:1759-1771`、重复调用 `:1795-1808`；七段两两相似度 0.70~1.00，其中只读与无进展两段标识符集合完全一致。
- `ExecutorResult.guard_stopped: Option<String>`（`:512`）把「为什么停」变成字符串；下游 `TaskStepStatus::Partial` 的判定依赖这段文本。SKY-6 第 2/3 项要修的就是「守卫截断还记 Done」，这正是字符串控制流的典型故障面。
- 相邻的 `run_loop.rs::step_loop` 970 行、66 个 `if`、8 个 `match`，是聊天侧的第二套状态机；任务侧 `solo::run_agent_loop` 415 行、`team::run_inner` 510 行、`team::run_mains_parallel` 338 行、`custom::run_step_with_tools` 286 行，都在各自组装/解释同一组守卫与终态。

**为什么是屎山**：新增一道守卫的「改动半径」不是加一个函数，而是改一个 1,046 行函数、选一个插入点、复制一次终态构造、再确认 `guard_stopped`/`budget_stopped` 两个字段没写错。守卫顺序本身也是隐式契约（token → 墙钟 → 轮次 → 只读 → 无进展 → 语义 → 重复），没有一张可执行的决策表。

**注意**：六模式执行器**不是**六份复制。`multi.rs` 已是 37 行 newtype，`plan`/`followup` 复用 `run_agent_loop`，`team`/`custom` 直调 `run_tool_loop`。真正的重复在**终态/守卫/记账分支**，不要按「六模式重写」立项。

### 4.2 设置 patch：75 字段上帝端点 + 四处手工镜像

**位置**：`server-rs/src/api/settings.rs:43`（`UpdateSettingsBody`）、`:492-1315`（`update_settings`）、`:350-430`（`settings_json` 两段手工投影）、`services/settings_service/mod.rs:87`（`RuntimeSettings`）、`services/settings_service/params.rs:41`（`ModeSettings`）、`web/src/api/types.ts:406/605`

**证据**：

- `update_settings` 824 行、152 个 `if`、34 个 `return validation(...)`；字段校验、连接数组全量语义、密钥清空、模式覆盖、默认值归一化全在一个函数。
- 75 个 patch 字段 / 77 个运行期字段 / 34 个模式覆盖字段；前端 `RuntimeSettings` 77、`RuntimeSettingsPatch` 75，`api/types.ts` 共 97 个导出类型、1,489 行。
- `genSettings.ts` 的 `loadSettings`（`:174-248`）与 `saveSettings`（`:251-320`）是约 60 行同构回填，新增字段要改两遍。
- 新增设置的实际改动面：`UpdateSettingsBody` + `RuntimeSettings` + `params.rs` 默认值 + `update_settings` 分支 + `settings_json` 投影 + `api/types.ts` 两个接口 + `genSettings.ts` + 测试。`check-contract` 只覆盖 29 组关键映射，**没有** `RuntimeSettings`/`RuntimeSettingsPatch`。
- `update_settings` 内部在函数体中定义 `macro_rules! apply`（`:201` 附近），宏作用域只在函数内，单测无法直接覆盖。

**为什么是屎山**：这是「一个设置一个分支」的线性增长，字段越多，函数越长；漏一个字段不会编译失败，只会静默不生效。最近 `genSettings.test.ts` 被要求补夹具，正是四处镜像的成本外溢。

### 4.3 前端任务台：1,903 行单组件 + 渲染缓存被打穿 + 数据双源

**位置**：`web/src/components/TaskBoard.vue:1-1903`

**证据**：

- script 1-681、template 683-1306、style 1308-1903；32 个 `computed`、20 个函数、44 个 `v-if`、13 个 `v-for`、117 个 scoped CSS 选择器。
- 职责至少 20 项：任务头/状态矩阵/工作区与画像、流程绑定与对比模式、plan 批准/编辑/规划对话、步骤/团队分工/子任务/流式输出、成果拆分/审计/最终计划、交付校验/文件变更、对话记录/追加指令、空态。
- 同类第二、第三处：`Sidebar.vue` 988 行（角色列表/导入/建任务/工作区选择/右键菜单/提示词编辑）、`ChatWindow.vue` 999 行。
- 渲染缓存是「七套小实现」：`planRendered`、`teamGroupsRendered`、`messageHtmlCache`、`resultMain/Audit/FinalPlan`、`plannedPlanHtml` 各自调用 `renderMarkdown`。`usage` 事件触发 `loadTaskDetail`（`task.ts:686-690`），详情签名包含 `usage_total`，签名变化会替换整个 `currentTask`（`:457-460`），于是每次 LLM 调用都让所有依赖 `currentTask` 的 markdown 缓存失效。注释宣称「数据未变时零重算」，实际在长任务里不成立。
- 同一任务数据两个状态源：`tasks` 列表记录与 `currentTask.task` 详情各一份，`currentTaskUsage` 又是 `detail.usage_total` 的手工副本（`task.ts:196-201`、`:459-461`、`:666-685`、`:925-931`）；列表/详情刷新可能一半成功一半失败。
- 模板重复：团队步骤 `1039-1066` vs 普通步骤 `1070-1098`；批准区规划对话 `995-1005` vs 任务对话记录 `1185-1202`；`v-html` 路径 7 处（`955,1002,1062,1095,1152,1174,1178,1199`）。
- 有测试（`TaskBoard.test.ts` 14 例 + `TaskBoard.bind.test.ts` + `taskModeUi.test.ts` 36 例），所以不是「无测试的意大利面」；问题是测试只能覆盖一个巨型组件的局部，拆分后无法按边界独立演进。

**为什么是屎山**：新增一个任务结果卡/状态行，必须回到同一个文件里找插入点；模板 44 个 `v-if` 使「哪个状态下渲染哪块」无法从组件边界读出。拆分不是审美问题，是让状态矩阵、批准流、执行流、成果流各自可测、可回滚。

### 4.4 team 的质量闸门与重试按调用点复制

**位置**：`team.rs:1244-1753`（`run_inner` 510 行）、`:1764-2101`（`run_mains_parallel` 338 行）、`team.rs:1384-1436` 与 `:1555-1608`（首审/终审）、`team.rs:975-1067`（team 规划重试）、`retry.rs:152-259`（`plan_task_retry`/`plan_revise_retry`）、`custom.rs:748-843`（节点重试）

**证据**：

- 首审与终审结构完全同构：拼 system、`generate_with_readonly_tools`、失败回退 `generate_text_healed`、取消判断、错误 fallback；去掉注释后相似度 0.759。
- 截止期收尾两段 `:1347-1364` 与 `:1478-1493` 相似度 1.000，只有「审计/汇总」与「终审/汇总」两个词不同。
- `workspace_manifest` 在 `:1376-1380`、`:1543-1547`、`:1629-1633` 被构造三次。
- 生成重试有四套实现：`plan_task_retry` 与 `plan_revise_retry` 相似度 0.859；team 规划重试用 `TEAM_PLAN_MAX_ATTEMPTS`，custom 节点用 `max_retries`，`generate_text_healed` 又是另一套。失败 attempt 的 usage 入账位置各不相同，直接威胁「各行 token 求和 == usage_total」不变量。
- 六个执行器各自拼终态：solo `:576-594`、legacy `:151-153`、plan `:178-194`、team `:1710-1744`、custom `:1421-1425`、followup `:142-144`；`Done/Partial` 的组合靠人工。

**为什么是屎山**：team 的质量闸门（审计→打回→复查→终审→汇总）是固有复杂度；但「首审与终审写两遍」「四套重试」「六处终态拼装」是意外复杂度。改一处忘另一处会直接造成状态口径分叉。

### 4.5 架构门禁存在系统性假阴性

**位置**：`tools/check-arch.mjs:270-301`（`directWrites`）、`:999`（前端规则 J 只扫 `from '...'`）

**证据**：

- `directWrites` 只匹配 `<binding>.<field> =`，以下真实写入全部漏检：
  - `TaskModeSelect.vue:15` `v-model="taskRunMode"`；
  - `TaskFlowSelect.vue:127/136` `v-model="taskFlowMode/taskFlowId"`；
  - `TaskConnectionSelect.vue:56/87` `taskConnectionId.value = ''` 与 `v-model="taskConnectionId"`；
  - `Sidebar.vue:502` `v-model="store.searchQuery"`；
  - `VisionScreenshotSection.vue:104/111` `visionScreenshotEnabled.value = ...`。
- 前端规则 J 只扫静态 `from`，不扫动态 `import()`；`web/src/utils/flowCanvasChunk.ts:13` 动态导入 `../components/settings/AgentFlowCanvas.vue`，即 L1（`utils`）→ L3（`components`）的禁止方向，但 `--verbose` 仍报「前端跨代边 2 条（登记 2）」，完全不可见。
- 报表还把 3 条 `class=wiring`（组合根/暴露面/兼容桥）和 2 条 `class=debt` 一并打印成「待偿还债务 5 条」，口径失真；实际应分栏。

**为什么是债**：审计时的 54 条白名单只覆盖 `<binding>.<field> =` 一种语法；C-2-0 扩展后变为 113 处显式分类（48 UI 白名单 + 65 设置表单文件级 `arch-allow-file`，0 待修），才真正堵住 `v-model` / `.value =` 的绕过。

### 4.6 `start.ps1` 端口硬编码 3001，与服务端/launcher 分叉

**位置**：`start.ps1:20,134,162,203,209,246,248,252`；`server-rs/src/config.rs:208`；`launcher/src/main.rs:194-200`

**证据**：

- `start.ps1` 完全不读取 `PORT` 或根 `.env`，8 处硬编码 `3001`。
- 服务端 `PORT` 环境变量优先（`config.rs:208`），launcher 从 `PORT`/`.env` 解析（`main.rs:194-200`）。
- 用户设 `PORT=30xx` 后，`start.ps1` 会健康检查/打开错误端口，旧实例检测也失效。

**为什么是债**：启动链有三套端口来源；这是「配置源分叉」而不是大文件问题。修复时先把 `start.ps1` 的端口解析改成与 launcher 同语义，所有健康检查/提示/打开浏览器用同一个 `$Port`。

### 4.7 集成测试夹具大量复制

**位置**：`server-rs/tests/*.rs`

**证据**：

- 70 个 `test_app`（56 个完全相同 4 行体）、70 个 `send_json`（66 个完全相同 18 行体）、35 个 `test_lock`（29 个相同 4 行体）、35 个 `wait_terminal`；8 行重复块 347 处，估算重复代码 1,700+ 行。
- 例如 `agent_trace.rs:13`、`agent_flows.rs:36`、`api_agent_loop.rs:76`、`coding_pack_flow_runs.rs:49`。

**为什么是债**：不影响当前正确性，但修一个夹具约定要改几十处，构成实质维护债。建议新增 `server-rs/tests/common/mod.rs`，先迁 API 测试，再迁任务测试；保留每个集成测试 crate 的独立 `OnceLock` 语义。

---

## 五、P2 发现

### 5.1 参数膨胀：55 处 `too_many_arguments`

分布：`task_core/backend.rs` 8、`agents/engine/executor.rs` 6、`task_service/executor.rs` 4、`task_engine/custom.rs` 3、`task_engine/team.rs` 3。`run_tool_loop` 15 参、`step_loop` 13 参、`generate_text_timed` 11 参。

不建议全仓扫荡。优先给 `run_tool_loop`、`step_loop`、`generate_text_timed`、`run_step_with_tools` 这四个「调用点多、参数易错位」的函数做参数对象；其余保留 `allow` 并在注释里说明理由。

### 5.2 两个 SSE 解析器的共享骨架

`connectors/openai_compatible/sse_parser.rs`（299 行）与 `responses_parser.rs`（295 行）的 `push/finish/parse_event` 骨架高度相似；重复块扫描在 25 行窗口下命中 23 个连续窗口。两者协议语义不同（Chat SSE vs Responses API），不是简单复制，但「事件切分/缓冲区/坏 JSON 容忍/完成判定」可以下沉为共享 `SseFramer`，两边只保留 `parse_event`。风险低，收益中等。

### 5.3 仓库卫生

- **根目录三份非约定文档**：未跟踪的 `SKY7-EXEC-PLAN.md`（144 行，抬头明确写「收口时删除」）与 `更新稿-0.3.0B之后.md`（43 行），以及已跟踪的 `P0-HANDOFF.md`。`docs/README.md` 自述「根目录只留 README/AGENTS/MAINTENANCE」，但门禁只守 `docs/` 根，不守仓库根。
- **2026-10-25 C-2E 已处置**：两份过程稿已归档（`docs/archive/P0-HANDOFF-2026-10-22.md`、`docs/archive/更新说明-0.3.0B之后-2026-10-11.md`），活文档引用改指归档；`check-docs` 新增 D7 守住仓库根。
- **处置**：`SKY7-EXEC-PLAN.md` 内容已落入 `docs/计划.md:4900`、`docs/遗留.md:2766`、`docs/经验.md:2712` 等，可直接删；`更新稿-0.3.0B之后.md` 是面向用户的发布说明稿，12 条大更新与撤回记录已在 `功能-变更史`，不建议新建第七类活文档，应作为 GitHub Release 正文或并入 `功能-变更史` 短索引；`P0-HANDOFF.md` 仍被 4 份活文档引用，先把开放项并入计划/遗留、更新引用，再删除，不要盲删。
- **5 条架构债**：3 条后端 wiring（`api→mcp`、`api→plugins`、`scripts→slash`）是组合根/兼容桥，登记为可接受；2 条前端真实回边中，`composables/useMemoryPanel → components/memoryPanel` **已由 2026-10-25 C-2E 偿还**（纯函数移入 `utils/`，登记删除）；`stores/chat → characterScriptSandbox` 需要事件总线，单独评估（登记保留）。
- **组件直改 state 113 处（C-2-0 后分类计数）** 是显式接纳，不是零债务；**2026-10-25 C-2E 已加 ratchet（`BASELINE_DIRECT_WRITES=113`，只降不升）**，不要一次性清零。
- **文档台账不是屎山**：81 文件 / 48,973 行是六类体系的刻意设计，`check-docs`/`check-doc-claims` 全绿；问题是根目录草稿、少量现行态漂移与关键常量多源，不是「文档太多」。

### 5.4 CI / 门禁注释漂移

- `.github/workflows/ci-linux.yml:7` 写「全量 15 段门禁」，实际 `check-all.ps1` 已有 19 个 `Invoke-Stage` 调用。
- `ci-linux.yml:50-51` 写 `minSdk 24`，实际 `src-tauri/gen/android/app/build.gradle.kts:36` 是 30。**2026-10-25 C-2E 已修为 30。**
- `ci-linux.yml:21` 引用已删除的 `docs/plans/QUALITY-FIX-PLAN.md`。
- `tools/check-css-discipline.mjs:80-84` 注释称 `content.css` 有 12 处 `!important`，基线/实测是 10 处。
- `tools/check-frontend-lint.mjs:5-8` 头部仍写「为什么不用 ESLint」，但仓库已装 ESLint 10，`check-all.ps1:281` 每轮都跑 `npm run lint -w web`；理由已过期，两个门禁可并存。

### 5.5 关键常量多源维护

`fair_share` 公式、`post_exec_reserve` 的 120s/5%/10..=180、`TASK_NO_PROGRESS_LIMIT=16` 等常量在 4~7 份活文档复写；它们不是整段复制，但改一个值要手工改多处。建议扩展 `check-doc-claims`，从 Rust 代码/脚本派生这些值，文档里写错即红；同时把历史层豁免从「含 2026-09-15」这种弱标记改成显式 `历史值/当时/原文` 标记，section 找不到时直接 FAIL，不静默退回全文。

---

## 六、修复路线：重启 C-2，而不是新开一堆批次

`docs/计划.md:1078` 的 `PC-2` 与 `:6027` 的 `C-2` 已经覆盖「大文件拆分」，但 C-2 长期未启动，目标「≤6」已被现实甩开。本报告建议**重启 C-2 并扩展为「结构债治理」**，按六个可独立回滚的批次执行。每批一个 PR，纯搬运与行为改动分开提交。

### C-2-0 先修项（低风险，先做，1 天内）

- 设置非法值策略：为 `default_temperature`、`default_top_p`、`mvu_vars_position`、`preset_tail_role`、`reflect_advice_role` 等静默分支明确 `Reject`/`Clamp`，与 400 分支统一；补字段级测试。
- `selectTask` 原子化：加载前清空详情或成功后再提交 id；失败回退并给可见错误。
- 抽 `resetCurrentTaskState(options)`：`clearSelectedTask`/`deleted`/`deleteTask` 三处共用；补 localStorage 与 `lastAgentStatus` 断言。
- 扩展 `check-arch`：识别 `storeToRefs` 解构、`v-model`、`.value =`；识别动态 `import()`；把 `flowCanvasChunk.ts` 移到 `components/settings/` 或登记为 `class=debt`；报表按 `wiring`/`debt` 分栏。
- 修 `start.ps1` 端口：统一读取 `PORT`（其次根 `.env`），所有健康检查/提示/打开浏览器用同一 `$Port`。
- 验收：`npm test -w web`、`node tools/check-arch.mjs`（新增 fixture 能抓到 `v-model`/动态导入）、`node tools/check-contract.mjs`、`node tools/check-docs.mjs`、`node tools/check-doc-claims.mjs`、`node tools/count-tests.mjs --check`。

**2026-10-24 执行追注（已完成）**：本批已落地，未开始大文件拆分。实际改动与验证：

- 设置非法值：`default_temperature` / `default_top_p` / `mvu_vars_position` / `preset_tail_role` / `reflect_advice_role` 统一返回 400（此前静默忽略）；`settings_connector` 改为表驱动断言 5 个字段，并补「5 字段自身合法 + 非法同批时合法字段不落库」的原子性断言。
- 任务状态：`selectTask` 详情加载失败回退到上一个任务（`loadTaskDetail` 返回 boolean）并写入 `taskLoadError`，`TaskBoard` 渲染可见错误条（此前只 `console.error`，用户看到的是「点了没反应」）；同 id 在途请求改为共享同一个 Promise，`selectTask` 不再误判成功；`resetCurrentTaskState` 统一 `clearSelectedTask` / `deleted` 事件 / `deleteTask` 三处，删除事件补持久化清理、`deleteTask` 补 `lastAgentStatus` 清理。
- 门禁：`check-arch` 识别 `storeToRefs` 解构、`v-model`、`.value =`、动态 `import()`；新增 `arch-allow`（行级）与 `arch-allow-file`（文件级，仅头部 20 行，必须写理由）；`flowCanvasChunk` 移入 `components/settings` 消除 L1→L3 动态边；报表分列 UI 白名单 / arch-allow / arch-allow-file / 待修，已登记依赖按 `wiring` / `debt` 分栏（旧实现把 3 条 wiring 误报成债）；新增 `tools/check-arch.test.mjs` fixture 自测（`v-model` / 动态 `import()` / 豁免边界），并抓出 `arch-allow-file:` 空理由可跨行误判的真实缺陷。
- 启动：`start.ps1` 端口解析改为 `PORT`（合法）→ 根 `.env`（合法）→ 3001（与 launcher 同源，非法 `PORT` 也回落 `.env`），8 处硬编码 3001 全部替换；`[DryRun] port=` 可验证。
- 验证：前端 `npm test` 156 文件 / 1688 运行时（静态 1645）、`vue-tsc`、`vite build`、ESLint 0 error；后端 `cargo test --workspace -j 8` 全绿（静态 1967；首次全量在 `task_events_emitted_on_run` 出现一次已知 broadcast 丢帧抖动，隔离复跑 ×2 绿、`RUST_TEST_THREADS=4` 降载全量重跑绿）；`check-arch` / `check-arch self-test` / `check-contract` / `check-docs` / `check-doc-claims` / `check-css-discipline` / `check-test-ratchet` / `check-frontend-lint` / `check-lock-sync` / `check-bundle` / `check-tutorial` / `count-tests --check` 全绿；隔离实例真实模型 `PUT /api/settings` 200、非法 `top_p` 400、`POST /api/chat/generate-raw` 200（`text=连通正常`），证据 `D:\kedai-bench-run\c20-final-evidence\`。
- 仍存在：C-2 的 33 个超千行文件拆分未启动；本批只修前置项。

### C-2A 前端任务台拆分（低风险，1~2 天）

**目标**：`TaskBoard.vue` 降到 ≤500 行，`Sidebar.vue` 降到 ≤600 行；`api/types.ts` 按域拆分并保留 barrel。

**步骤**：

1. 先补 characterization tests：空态、运行中、plan 待批准、team 多主、成果卡、交付校验、追加指令七种状态各一条挂载断言；现有 14 例作为回归网。
2. 抽 4 个子组件：`TaskStatusHeader`（顶栏/状态矩阵/工作区/画像）、`TaskApprovalPanel`（批准/编辑/规划对话）、`TaskExecutionPanel`（进度/步骤/团队/子任务/流式）、`TaskResultPanel`（成果/交付/文件变更/对话/追加）。
3. 抽 composable：`useTaskViewState`、`useTaskMarkdown`（统一 7 个渲染缓存，按文本哈希 + LRU）、`useTaskFlowView`、`useTaskFlowBinding`、`useTaskActions`、`useTaskLiveText`、`useTaskMessages`。
4. 抽共享 `MarkdownBlock`/`useMarkdownCache`，让 `TaskBoard` 与 `ChatMessageItem` 不再各自维护渲染缓存。
5. `api/types.ts` 拆为 `types/settings.ts`、`types/task.ts`、`types/chat.ts`、`types/tools.ts`、`types/index.ts`；现有 import 路径经 barrel 保持不变。
6. 顺带消除明确重复：`WorldBooksModal` 的条目编辑器抽 `WorldBookEntryEditor`（190 行两遍）、`AgentPanel` 的 `sv-tool-io` 抽 `ToolIoPane`（6 处）、`Sidebar`/`TaskBoard` 的 `removeTask` 收敛、`.sv-task-new` 只留一个 CSS 所有者。

**验收**：`npm test -w web` 全绿；`npm run build -w web`；`node tools/check-bundle.mjs` 不破首屏预算；浏览器两态截图（空态/运行中）与拆分前一致；`check-arch` 无新增违规。

### C-2B 设置体系域化（中风险，2~3 天）

**目标**：`update_settings` 从 824 行降到 ≤200 行；字段级行为不变；线格式零变化；非法值策略统一。

**步骤**：

1. 保持 `UpdateSettingsBody` 扁平（不改 JSON 线格式），在 `api/settings/` 下拆 `patch/connection.rs`、`generation.rs`、`agent.rs`、`memory.rs`、`task.rs`、`tools.rs`、`bundles.rs`。
2. 每个域一个 `apply_*` 函数，返回 `Result<(), PatchError>`；`PatchError` 带字段名 + 用户可读文案；handler 只做锁、clone、逐域 apply、归一化、落盘、替换内存。
3. 把函数体内的 `macro_rules! apply` 提到模块级（或改成泛型 helper），让单测能直接覆盖。
4. 增加字段清单对拍：新增只读脚本 `tools/check-settings-parity.mjs`，从 Rust `RuntimeSettings`/`UpdateSettingsBody` 与 TS `RuntimeSettings`/`RuntimeSettingsPatch` 抽取字段名，做集合比对；接入 `check-all.ps1`。
5. 前端 `genSettings` 抽 `applyRuntimeSettings(s)`，`loadSettings`/`saveSettings` 共用；`RuntimeSettingsPatch` 不再手写 75 个可选字段。
6. 字段测试至少覆盖「设置后生效 / 非法值按策略处理 / 缺省不覆盖」三态；前端 `genSettings.test.ts` 夹具同步。

**验收**：现有 `settings_connector`/`api_integration` 全绿；新增字段清单测试；`check-contract`/`check-doc-claims`；真实模型最小连通（`generate-raw` 200）确认设置仍能下发。

**2026-10-25 执行追注（已完成）**：本批已落地，未开始 C-2C/C-2D/C-2E。

- 后端：`api/settings.rs`（1934 行）拆为 `api/settings/{mod,projection,probes,prompt,patch/*}`；`update_settings` **149 行**（目标 ≤200）；75 字段按七域拆分，每域一个 `apply` 返回 `PatchError{field,message}`；`apply_mode!` 提到 `patch/mod.rs` 模块级。
- 非法值策略：在 C-2-0 的 5 个字段之外，`compaction_mode` / `compaction_keep_recent` / `compaction_snip_bytes` / `memory_inject_limit` / `memory_inject_char_budget` / `memory_max_entries` 六处「静默忽略 200」改为 400。域顺序为连接→生成→Agent→工具→记忆→任务→能力包（bundles 最后，保证文学推荐档写压缩三项与默认词重物化最后执行）。
- 门禁：新增 `tools/check-settings-parity.mjs`（PUT 75 / GET 77 / 连接 14；GET 侧直接解析 `settings_json()` 投影键，覆盖「结构体加字段但投影忘加」），接入 `check-all.ps1`（阶段 20→21）；`check-contract` 的 `PromptPreviewResponse` 路径改指 `api/settings/prompt.rs`。
- 前端：`RuntimeSettingsPatch` 由 `Partial<Omit<RuntimeSettings,…>> & Overrides` 派生（GET-only / PUT-only / 同名重定义三组常量显式列出）；`genSettings` 的 load/save 回填合并为 `applyRuntimeSettings`。
- 测试：新增 6 个后端单测（生效 / 非法 / 缺省三态 + 连接清空名单 + 任务默认连接）；`settings_connector` 的 5 字段表驱动扩到 11 个并补新字段的整批原子性；`api_memory_diagnostics` 的越界忽略断言改为 400。
- 验证：后端静态 1973（1513 单测 + 460 集成）、前端静态 1686 / 运行时 1729；`check-settings-parity` 三组 OK；真实模型最小连通证据见 `D:\kedai-bench-run\c2b-evidence\`。
- 仍存在：`check-settings-parity` 只比字段名与可选性，不校验类型/取值域；`RuntimeSettingsPatch` 的派生规则靠三个常量维系，新增差异类别时需同步脚本；`settings_service/mod.rs`（2126 行）自身未拆。

### C-2C 工具循环守卫管线（高风险，3~5 天，下一次任务模式改动前必做）

**目标**：`run_tool_loop` 从 1,046 行降到 ≤300 行；守卫决策与副作用分离；行为逐字节等价。

**步骤**（必须按顺序）：

1. **先固化行为**：为 7 类停止条件写 characterization tests（给定轮次状态/用量/工具调用序列 → 期望 `LoopStop` 与终态字段），把现有顺序写成表。
2. 新增 `enum LoopStopKind { TokenBudget, StepBudget, RoundLimit, Readonly, NoProgress, Semantic, RepeatedCall }` 与 `struct LoopStop { kind, message }`；把 `ExecutorResult.guard_stopped: Option<String>` 改为 `stop: Option<LoopStop>`（或保留旧字段做兼容转换，分两步提交）。
3. 抽 `ExecutorResult::stopped(content, finish_reason, self_heals, stop)` 单点构造，消掉 6~7 处重复结构体字面量；加一条「所有停止出口都设置 kind」的单测。
4. 把「工具历史裁剪」「截断自愈」从循环体抽成纯/近纯 helper；把守卫判定抽成 `LoopGuardState::evaluate(...) -> Option<LoopStop>`，循环体只负责执行工具、发事件、匹配 Stop。
5. 最后再做参数对象 `ToolLoopRequest`，把 15 参降到 1 个结构体 + 2 个可变引用。

**验收**：全量 `cargo test --manifest-path server-rs/Cargo.toml -j 8`；守卫决策表单测；任务模式窄探针（solo + team 各一条）真实模型通过；对比拆分前后的事件序列/终态状态，不出现 `guard_stopped` 字符串漂移。

**2026-10-25 执行追注（已完成）**：本批已落地，行为逐字节等价（既有守卫集成用例零修改全绿）。

- 结构：`agents/engine/tool_loop/{mod,guard,round}.rs`。`mod.rs` 只留循环骨架（`run_tool_loop` **251 行**、单结构体入参 `ToolLoopRequest`）；`round.rs` 承载单轮裁决/分组执行/收尾与 `await_or_cancel`；`guard.rs` 承载 7 类停止条件判定与文案。
- 停止模型：`LoopStopKind{TokenBudget,StepBudget,RoundLimit,Readonly,NoProgress,Semantic,RepeatedCall}` + `LoopStop{kind,message,event_title,event_detail}`；`ExecutorResult.guard_stopped: Option<String>` 改为 `stop: Option<LoopStop>`，新增 `ExecutorResult::stopped(...)` 单点构造（`budget_stopped` 由 kind 派生）与 `guard_stopped()`（仅 4 个空转守卫透出，预算/轮次不降 partial）。
- 顺序表（`LoopGuardState::evaluate` 单测锁定，与抽取前一致）：token 预算 → 步骤墙钟（仅 `round>0`）→ 轮次上限 → 连续只读（先提醒后熔断）→ 声明文件无进展（`limit/2` 提醒、`limit` 收尾）→ 语义熔断 → 重复调用；同轮多命中时前者胜出，提醒在 stop 之前发出。
- 抽离：工具历史裁剪 → `trim_round_history`；单轮生成（看门狗/传输重试/截断自愈）→ `generate_round`；两者留在 `executor.rs`（`pub(super)`），循环体不再内联。
- 测试：新增 `guard.rs` 决策表 10 例（7 类 + 优先级/健康负样例/同轮提醒与 stop 并存）、`ExecutorResult::stopped` 旗标派生 1 例；`split_parallel_groups` 4 例与取消闸门 4 例随代码移动到 `round.rs`（数量不变）；`api_agent_loop` / `task_loop_budget` / `task_no_progress_guard` / `task_guard_truncation` / `task_team_guard_scope` / `task_team_integration_guards` 共 28 例零修改全绿。
- 验证：`cargo test --workspace -j 4`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all --check`；后端静态计数 2009（1544 单测 + 465 集成），前端不变；真实模型窄探针证据见本批提交正文与 `D:\kedai-bench-run\c2c-evidence\`。
- 仍存在：`executor.rs` 仍有 1,852 行（`execute_generation`/截断自愈/非流式生成等），属 C-2D/C-2E 的后续拆分面；本批只收了工具循环。

### C-2D team/custom 文件拆分与去重（中风险，2~3 天）

**目标**：`team.rs` 从 3,276 行降到 ≤800 行；`custom.rs` 从 1,444 行降到 ≤900 行；行为零变化。

**步骤**：

1. `team/` 目录：`mod.rs`（`TeamExecutor`/`run_inner` 编排）、`topology.rs`（`parse_team_topology`/`resolve_after`/`plan_waves`/依赖推断）、`budget.rs`（`post_exec_reserve`/`main_pool_deadline`/`fair_share`/截断预算）、`parallel.rs`（`run_mains_parallel`/补做合并/`rebuild_outputs`）、`audit.rs`（首审/终审共用 `run_audit_phase`）、`assemble.rs`（`workspace_manifest`/`build_*_input`/终态原因）、`tests.rs`。
2. `custom.rs` 按「步骤执行 / 节点图 / 子流程调用 / 变量与上下文 / 测试」拆目录模块。
3. 先纯搬运（`git mv` + `pub(super)` 调整），测试零修改；下一提交再做首审/终审去重、截止期收尾去重、重试骨架参数化（`retry_parse_loop`）。
4. 抽纯函数 `terminal_status(flags) -> (TaskStatus, Option<String>)`，只替换六处终态拼装，不动各模式控制流；`docs/遗留.md:438-446` 的 B.4b step_runner 合一维持知情接受，不推翻。

**验收**：`cargo test` 全绿且测试数不变；team/plan/custom 集成测试；真实模型窄探针；SKY-6/SKY-7 的守卫语义不变。

**2026-10-25 执行追注（已完成）**：本批分两笔提交落地（纯搬运 → 去重），未开始 C-2E。

- 目录化：`team.rs`（3,649 行）→ `team/{mod,topology,budget,parallel,audit,assemble,tests}.rs`，mod.rs **632 行**；`custom.rs`（1,446 行）→ `custom/{mod,context,step,graph,subflow}.rs`，mod.rs **316 行**；生产文件均 <1,000 行（最大 `team/parallel.rs` 981）。`TeamMain`/`Kickback`/`AuditVerdict` 与 `NodeOutcome`/`NodeInput`/`GraphCtx`/`GraphOutcome` 作为跨子模块共享词汇留在各自 `mod.rs`；39 个 team 单测搬到 `team/tests.rs`，`#![cfg(test)]` 保规则 E 生产区切分。
- 去重：首审/终审共用 `TeamExecutor::run_audit_phase`（只读工具通道 → 纯文本回退，取消/失败语义保持）；两处预算到点收尾收敛为 `finish_on_deadline`；`retry.rs` 新增 `retry_parse_loop`，统一 `plan_task_retry`/`plan_revise_retry`/team 规划三处骨架（唯一差异是 team 日志多一个 `max_tokens` 字段）；`task_engine::executor::terminal_status(partial, error)` 替换 legacy/plan/solo/team/custom/followup 六处终态拼装，`Done` 恒清空原因。
- 验证：合并 `origin/main`（PR #44 / SKY-9）后，后端静态 **2040**（1573 单测 + 467 集成，本批新增 3 例 `terminal_status`）；前端 1687 静态 / 1730 运行时；`cargo test --workspace -j 4`、`clippy --all-targets -D warnings`、`cargo fmt --check` 全绿；custom/team 18 个集成测试文件 69 例零修改全绿；函数条目数 team 88/88、custom 20/20，`allow(too_many_arguments)` 3/3、3/3 不变。
- 真实模型窄探针（隔离实例 `DATA_DIR=D:/kedai-bench-run/data-c2d`、`PORT=3018`，启动日志/health 均核对该目录）：`PUT /api/settings` 200、`generate-raw` 200（`text=连通正常`）、`task-probe solo` 8.7s `done`、`team` 22.0s `done`（planner → 主 agent → audit → summary 四段）、`custom` 3 步全 `done`；证据 `D:\kedai-bench-run\c2d-evidence\`。
- 仍存在：`executor.rs`（1,852 行）与 `settings_service` 未拆；C-2E（卫生/ratchet）未启动。

### C-2E 卫生、测试夹具与 ratchet（低风险，可并行）

1. 删除 `SKY7-EXEC-PLAN.md`；`更新稿-0.3.0B之后.md` 比对后并入/归档；`P0-HANDOFF.md` 迁移开放项与引用后删除；给 `check-docs` 加仓库根 md allowlist。
2. 新增 `check-file-size` ratchet：基线 = 当前 33 个 >1,000 行文件，只降不升；先止增，再定 ≤20、≤10 的阶段目标，不直接承诺 ≤6。
3. 新增 `server-rs/tests/common/mod.rs`，先迁 `test_app`/`send_json`/`test_lock`/`wait_terminal`，保留各 crate 独立静态锁；目标是把 70/70/35/35 收敛到 1 份。
4. 偿还 `useMemoryPanel → components` 一条前端回边（纯函数下沉）；`stores/chat → characterScriptSandbox` 单独立项。
5. 给「组件直改 state」113 处分类计数加 ratchet；不一次性清零。
6. 修 CI/注释漂移：`ci-linux.yml` 15→19 段、minSdk 24→30、删 `QUALITY-FIX-PLAN` 死引用；`check-css-discipline` 12→10；`check-frontend-lint` 头部说明改为「ESLint 管风格，本脚本管类型逃逸 ratchet」。
7. 更新 `docs/计划.md` C-2 的计数与状态（本报告不改台账；执行前由维护者拍板）。

**验收**：`check-arch`/`check-docs`/`check-doc-claims`/`count-tests --check` 全绿；ratchet 人为加一个超限文件必须转红；根目录只剩 `README.md`/`AGENTS.md`/`MAINTENANCE.md`；测试夹具 grep 只剩 1 处定义（或明确保留的少量变体）。

**2026-10-25 执行追注（已完成）**：本批已落地，C-2 六批（C-2-0 + C-2A~E）全部完成。

- 卫生：`P0-HANDOFF.md` → `docs/archive/P0-HANDOFF-2026-10-22.md`；未跟踪的
  `更新稿-0.3.0B之后.md` → `docs/archive/更新说明-0.3.0B之后-2026-10-11.md`；
  4 处活文档引用改指归档。`check-docs` 新增 D7：仓库根 `*.md` 只允许
  README/AGENTS/MAINTENANCE。
- 文件大小 ratchet：`tools/check-file-size.mjs` + `tools/file-size-baseline.json`，
  基线 30 个超千行非测试源文件（排除 tests/common、tests.rs、*_tests.rs、tests_*.rs、
  *.test.ts、*.spec.ts、*.d.ts）；基线内只降不升，基线外新超阈值即 FAIL；
  接入 `check-all`（阶段 21→22）。
- 测试夹具：`server-rs/tests/common/mod.rs` 提供 `test_app`/`send_json`/`test_lock`/
  `wait_terminal` 及显式变体；75 个测试文件删除本地定义约 2,600 行，24 个任务用例
  保留一行超时包装、2 个特殊变体原样保留；`count-tests` 排除 `tests/common/`，
  集成文件数仍 80。每个测试文件仍编译为独立二进制，进程级 app 与串行锁语义不变。
- 架构债：`memoryPanel` 纯函数移入 `utils/`，`composables→components` 回边消除；
  `arch-layers.json` 删登记，已登记依赖 5→4（wiring 3 / debt 1）。`stores/chat →
  characterScriptSandbox` 需事件总线，按审计口径单独立项，未动。
- 直写 ratchet：`check-arch` 增 `BASELINE_DIRECT_WRITES=113`，总量超出即 FAIL；
  113 处分类计数不变。
- 漂移：`ci-linux.yml` 删「15 段」硬编码与 `QUALITY-FIX-PLAN` 死引用、
  `CARGO_NDK_PLATFORM` 24→30；`check-css-discipline` 12→10；
  `check-frontend-lint` 头部改为与 ESLint 的分工。
- 台账：`docs/计划.md` C-2 状态改「部分完成」并登记 C-2 残余；本追注；
  `docs/契约-架构与数据.md` 已登记依赖/直写计数同步；`MAINTENANCE.md` 门禁清单补
  file-size 与直写 ratchet；`docs/经验.md` E134 记录批量删除的区间重叠坑。
- 验证：后端静态 2040（1573 单测 + 467 集成 / 80 文件）不变；`cargo check --tests`
  0 warning、`clippy --all-targets -D warnings`、`fmt --check` 全绿；定向集成 7 文件
  53 例全绿；前端 1687 静态 / 1730 运行时；`check-docs`（D7）/`check-arch`/
  `check-file-size`/`check-doc-claims`/`check-test-ratchet`/`check-css-discipline`/
  `check-frontend-lint` 全绿；file-size 与直写两个 ratchet 均做人为反证并转红。
- 仍存在：`executor.rs`（1,852 行）与 `settings_service/mod.rs`（2,126 行）未拆；
  `stores/chat → characterScriptSandbox` debt；`check-settings-parity` 只比字段名/
  可选性；`TaskDetail`/`TaskUsageTotal` + `ts-rs` 未做——均登记在 `docs/计划.md`
  的「C-2 残余」。

### 排序理由

- `C-2-0` 先做：修的是用户可见错误和门禁假阴性，成本低、收益立刻可见；不先修，后面的拆分会在错误基线上进行。
- `C-2A` 再做的理由：前端测试快、纯结构改动多，能快速验证「拆分流程」本身；同时降低后续任务 UI 改动的冲突面。
- `C-2B` 紧随：设置字段仍在增长，越晚拆冲突越多；且 `P0-1` 的非法值策略需要字段表承载。
- `C-2C` 放在任务模式下一次改动之前：如果接下来还要加守卫/预算，必须先把停止模型收口，否则继续在 1,046 行函数上打补丁。
- `C-2D` 最后做：team/custom 拆分是纯搬运为主，但依赖 `C-2C` 的终态模型稳定；先拆会把旧的字符串控制流搬来搬去。
- `C-2E` 可并行：卫生与 ratchet 不阻塞任何批次。

---

## 七、明确不建议动

- **`models/types.rs` 的大拆分**：`arch-layers.json` 已登记「传输类型混装」为知情接受项；拆它只改 import、不改线格式，收益低、冲突面大。先做 C-2A/C-2B/C-2C，等类型自然稳定后再评估。
- **`TaskBackend` 的 8 个窄接口**：这是批次 4.2 刻意做的依赖倒置，当前每个接口只有 1 个实现，再拆没有架构收益；不要为了「更小」继续切。
- **EJS 解释器/世界书解析/插件/MCP/undo/secret_store**：`parsing/assistant/ejs/*` 已冻结，只接受安全修复；`plugins/`、`mcp/`、`undo_service.rs`、`secret_store.rs` 安全面高、测试面足，本次不碰。
- **`wiring.rs`、`delivery.rs`、`task_service/db.rs`**：纯函数/内聚/测试密；行数大是固有复杂度，不是重复。
- **六类文档台账**：48,973 行是刻意设计，门禁全绿；不要用「文档太长」为由重写或合并六类。
- **`check-arch` / `check-contract` / `check-doc-claims`**：这些门禁正在有效工作；不要为了通过重构而放宽规则或加白名单。要改的是**覆盖缺口**（v-model/动态导入/字段 parity），不是降低基线。
- **任务事件 `switch`**：`stores/task.ts:663` 的 13 个 case 看起来长，但有 `never` 穷尽断言；这是良性复杂度，不要改成字符串映射或 `any`。
- **前端沙箱/mini-jQuery/渲染 sanitize**：`sandbox/boot-script.ts`、`dom-rpc.ts`、`iframe-lifecycle.ts`、`shared-globals.ts` 的重复是跨 realm 自包含的刻意副本；`markdown.ts`/`sanitizeMessageHtml` 是安全关键路径，不要为了「统一 v-html」重写。
- **`importBundle.ts`**：纯函数、零 IO、单测充分；只做小的 `sectionEntry` 辅助，不要重写。
- **legacy/plan/custom 的 step 循环合一**：`docs/遗留.md:438-446` 已书面裁定回归风险高于收益；只抽纯函数终态，不动控制流。

---

## 八、验收与证据纪律

- **纯搬运批**（C-2A/C-2D）：测试零修改或只改 import；`cargo test -j 8`、`npm test -w web`、`npm run build -w web`、`node tools/check-arch.mjs`、`node tools/check-contract.mjs`、`node tools/check-docs.mjs`、`node tools/check-doc-claims.mjs`、`node tools/count-tests.mjs --check`。
- **行为相关批**（C-2-0/C-2B/C-2C）：先写失败测试；按 AGENTS.md 跑真实模型窄探针，证据落 `D:\kedai-bench-run\<批次>-evidence\`；提交正文写「改了什么 / 为何这么改 / 验证证据」。
- **C-2C 额外要求**：守卫决策表 + 拆分前后事件序列/终态对比；若顺序或语义有变，单独立项拍板，不混在纯重构里。
- **每批一个 PR**：当前分支是 `feat/sky7-guard-scope-budget-share`（领先 `main` 4 笔、落后 2 笔，`main` 已有 PR #38），应先等 SKY-7 合并到 `main`，再从 `main` 切 `refactor/c2-*`；避免在 SKY-7 分支上叠大拆分。

---

## 九、待拍板

1. **先做 C-2-0 + C-2A，还是 C-2C？** 建议 `C-2-0` 先做（修用户可见错误 + 门禁假阴性）；随后若下一项工作仍是任务模式守卫/预算，则 `C-2C` 优先于 `C-2A/C-2B`，避免在 1,046 行函数上继续加补丁；否则按 C-2A → C-2B → C-2C 顺序。
2. **文件大小 ratchet 的目标**：建议先「只降不升」，稳定两批后再定 ≤20/≤10；不建议直接承诺 C-2 原目标 ≤6。
3. **是否引入类型代码生成**（`ts-rs`/OpenAPI）：短期不建议；现有 29 组契约映射 + 域拆分 + `check-settings-parity` 足够。等设置/任务类型稳定后，再评估把 `TaskDetail`/`TaskUsageTotal` 落成 Rust 结构体并生成 TS。
4. **`P0-HANDOFF.md` 的处置**：其中仍开放的 P1 项是否已全部转入 `计划.md`/`遗留.md`？若是，迁移引用后删除；若否，先补登再删。

---

## 附录：复现命令

```bash
# 规模与超千行文件（排除测试）
git ls-files 'server-rs/src/*.rs' 'web/src/*.ts' 'web/src/*.vue' | xargs wc -l | sort -rn | head -50

# 函数规模与生产 unwrap/expect（脚本见审计记录；核心口径：排除 tests/、tests.rs、*_tests.rs 与 *.test.ts）
# 结果：4,107 函数；71 个 ≥120 行；生产 unwrap/expect ≈39

# 静态门禁
node tools/check-arch.mjs
node tools/check-contract.mjs
node tools/check-doc-claims.mjs
node tools/check-docs.mjs
node tools/check-css-discipline.mjs
node tools/count-tests.mjs --check

# git 考古
git log --oneline -30 -- server-rs/src/agents/engine/executor.rs
git log --oneline -30 -- server-rs/src/services/task_engine/team.rs
git log --format= --name-only -- '*.rs' '*.ts' '*.vue' | sort | uniq -c | sort -rn | head -40
```

> 本报告未跑全量 `npm run check`、`npm test`、`cargo test` 或真实模型复跑；上述结论是结构审计结论，执行批前按仓库纪律补跑。行号为 2026-10-24 快照，执行时先复核。
