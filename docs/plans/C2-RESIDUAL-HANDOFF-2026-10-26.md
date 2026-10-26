# C-2 残余处理交接文档（2026-10-26）

> 状态：**待处理**（C-2E 已收口；本文件只做交接，不改代码）
> 基线：`main` = `6ddd2cb`（PR #47 后，本文件编写时；C-2 六批 C-2-0 + C-2A~E 全部完成）
> 依据：[`CODEBASE-DEBT-AUDIT-2026-10-24.md`](CODEBASE-DEBT-AUDIT-2026-10-24.md) §四/§七/§九、[`C2-0-HANDOFF-2026-10-24.md`](C2-0-HANDOFF-2026-10-24.md)、`docs/计划.md` 的「C-2 残余」
> 范围：**五项已登记残余** + 超千行基线文件的处置边界；不是重开 C-2，也不是把 30 个文件全部拆完

---

## 一、先核对当前基线（动手前必做）

| 事实 | 当前值 | 权威来源 |
|---|---:|---|
| 超千行非测试源文件 | **30 个** | `node tools/check-file-size.mjs --verbose` |
| 文件大小基线 | `tools/file-size-baseline.json`（只降不升） | 同上 |
| 组件直改 store state | **113 处**（UI 48 + 表单级 65 + 行级 0） | `node tools/check-arch.mjs` |
| 已登记越代依赖 | **4 条**（wiring 3 / debt 1） | 同上；debt 仅 `stores/chat → characterScriptSandbox` |
| 后端测试 | **2044 静态**（1576 单测 + 468 集成 / 81 文件） | `node tools/count-tests.mjs` |
| 前端测试 | **1688 静态 / 1731 运行时 / 163 文件** | `npm test -w web` + `count-tests` |
| 门禁阶段 | **22 个 `Invoke-Stage`** + 2 审计段 | `tools/check-all.ps1` |
| 架构回边 | `composables→components` 已还；只剩 `stores/chat→characterScriptSandbox` | `tools/arch-layers.json` |

```bash
# 快速核对（在 D:\kedai）
node tools/check-file-size.mjs --verbose
node tools/check-arch.mjs
node tools/count-tests.mjs
```

行号均为 `6ddd2cb` 快照；**动手前先复核**（C-2E 后的行号很容易再漂）。

---

## 二、五项残余（按推荐顺序）

### R-1 `settings_service/mod.rs` 测试搬出（最高性价比，先做）

**现状证据**（`6ddd2cb`）：

- `server-rs/src/services/settings_service/mod.rs` 共 **2,126 行**；
- **生产代码只到第 402 行**（`RuntimeSettings` 结构体/`AppMode`/辅助函数）；
- 第 403 行起是 `#[cfg(test)] mod tests { … }`，约 **1,720 行**，34 个 `#[test]`；
- 第 58 行有一条 `#[cfg(test)] use crate::config::AppConfig;`，只被测试使用。

**目标**：`mod.rs` 降到 ~402 行；测试搬到 `server-rs/src/services/settings_service/tests.rs`。

**步骤**：

1. 新建 `settings_service/tests.rs`，**第一行**写 `#![cfg(test)]`（`check-arch` 的 `productionLineCount` 靠它切生产区，见 `team/tests.rs` 头注释）。
2. 把 `mod tests { … }` 的内容原样搬入（去掉外层 `mod tests {` 包裹）；保留 `use super::*;`、`use super::params::{…};`、`use crate::utils::test_support::TempDataDir;`。
3. `mod.rs` 里改为：
   ```rust
   #[cfg(test)]
   mod tests;
   ```
4. 把第 58 行的 `#[cfg(test)] use crate::config::AppConfig;` 从 `mod.rs` 移到 `tests.rs`（否则父模块留下未使用导入；`cargo check --tests` 会报）。
5. 运行 `cargo fmt`、`cargo test --test settings`（或全量）、`node tools/check-file-size.mjs --update`（只允许下调），确认 `settings_service/mod.rs` 条目从基线消失。

**验收**：

- `wc -l`：`mod.rs` ~402，`tests.rs` ~1,720；
- 静态测试数不变（当前 2044；`count-tests --check`）；
- `check-arch` 无规则 E 误报（`#[cfg(test)]` 在列首）；
- `check-file-size` 基线减少 1 项，无新增 >1000 文件；
- `cargo test --workspace -j 4` 全绿。

**风险**：极低（纯搬运）。唯一坑是测试文件里的 `super::params` 路径和 `AppConfig` 导入位置。

**收益**：2,126 → 402，直接消掉本项残余的绝大部分；`settings_service/params.rs`（1,235 行）仍在基线内，是否继续拆见 R-1b。

**R-1b（可选，后置）**：`settings_service/params.rs` 1,235 行，按「默认值 / 校验 / 纯归一化」拆。它不在五项残余的硬要求内；等 R-1 落地后单独评估。

---

### R-2 `agents/engine/executor.rs` 测试搬出 + 生产分区

**现状证据**（`6ddd2cb`）：

- 共 **1,852 行**；**生产代码到第 1,166 行**，第 1,167 行起 `#[cfg(test)] mod tests`（约 **685 行**，测试密集）。
- 生产区自然边界（行号为快照）：
  - `tool_origin_str` / `maybe_run_tool`：16–152（约 137 行）
  - 看门狗/传输重试：`with_call_watchdog` 153、`call_watchdog_for` 174、`resolve_call_watchdog` 185、`should_retry_transport` 215（约 78 行）
  - `execute_generation`：231–387（约 157 行）
  - `add_usage` / `process_chunk`：388–525（约 138 行）
  - `ExecutorResult` impl：526–587
  - 截断自愈：588–687（约 100 行）
  - `ToolGate` / 预算/无进展守卫：688–816（约 129 行）
  - `trim_round_history`：817–884
  - `generate_round`：885–1,120（约 236 行）
  - `impl AgentEngine::generate_text`：1,121–1,166
- **不是**「execute_generation 一千行」：真正的体量是「生产 1,166 + 测试 685」，且生产内部已按职责分节；C-2C 已把工具循环拆到 `tool_loop/`，不要重复拆那部分。

**目标**：生产文件每个 <1,000 行；`executor.rs` 本身消失或变成薄 `mod.rs`；测试 685 行搬到 `executor/tests.rs`。

**步骤（两笔提交，先搬后拆）**：

1. **提交 1（纯搬运测试）**：`executor.rs` 的 `#[cfg(test)] mod tests { … }` → `executor/tests.rs`，第一行 `#![cfg(test)]`；`executor.rs` 改 `#[cfg(test)] mod tests;`。此时 `executor.rs` ~1,166 行，仍 >1,000，但测试体量已隔离。
2. **提交 2（生产分区，纯搬运）**：
   - `executor/mod.rs`：`ExecutorResult`/`AgentEngine::generate_text` + `pub(super)` 导出；
   - `executor/generation.rs`：`execute_generation` / `process_chunk` / `add_usage` / `generate_round` / `trim_round_history`；
   - `executor/transport.rs`：看门狗 + 传输重试；
   - `executor/heal.rs`：截断自愈四函数；
   - `executor/guard.rs`：`ToolGate` / 预算 / 无进展 / 声明文件判定。
   保留 `pub(crate)` / `pub(super)` 可见性；跨文件引用改 `crate::agents::engine::executor::…` 或 `super::…`。不要改执行顺序、守卫阈值、事件文案。
3. 每步后跑 `cargo fmt`、`cargo test --workspace -j 4`、`cargo clippy --all-targets -- -D warnings`；最后 `node tools/check-file-size.mjs --update`。

**验收**：

- `wc -l`：`executor/mod.rs` <300，其余生产文件 <1,000；`executor.rs` 基线条目移除；
- 函数条目数与测试数不变（`count-tests --check`；C-2C 的 `tool_loop` 28 个守卫集成用例零修改）；
- 真实模型窄探针：solo + team 各一条（`tools/bench/task-probe.mjs`，隔离 `DATA_DIR`），确认 `done`/事件序列/终态不漂移；
- `check-arch` 无新增违规；`check-file-size` 只降。

**风险**：中（大范围改 import/可见性）。按 C-2C/C-2D 的教训：先纯搬运、测试零修改，再单独做任何去重；不要顺手改守卫逻辑。

---

### R-3 `stores/chat → characterScriptSandbox` 回边（最后一条 debt）

**现状证据**：

- `web/src/stores/chat.ts:11`：`import { broadcastCardEvent, broadcastMvuUpdate } from '../characterScriptSandbox';`
- 使用点：`chat.ts:248`、`:400`、`:463`（`broadcastMvuUpdate`），`:490`（`broadcastCardEvent`）。
- 实现在 `web/src/sandbox/iframe-lifecycle.ts`：
  - `liveSandboxes: Map<string, Window>`（:74）
  - `broadcastMvuUpdate`（:80–:107）
  - `broadcastCardEvent`（:109–:124）
  - 创建时 `liveSandboxes.set`（:396），销毁时 `delete`（:234）
- 依赖的纯常量/函数在 `web/src/sandbox/protocol.ts`：`CHANNEL`、`MAX_MESSAGE_BYTES`、`cloneData`、`utf8Bytes`（都已 export，但 `protocol.ts` 属 L3）。
- 登记：`tools/arch-layers.json` 的 `frontendRegisteredEdges`（`stores → characterScriptSandbox`，`class=debt`）。

**目标**：消除 L2（stores）→ L3（sandbox）方向，`check-arch` 已登记依赖 4→3（wiring 3 / debt 0），删除该 debt 条目。

**推荐方案（纯下沉 L1，不改事件语义）**：

1. 新建 `web/src/utils/sandboxProtocol.ts`（L1）：搬 `CHANNEL`、`MAX_MESSAGE_BYTES`、`cloneData`、`utf8Bytes`（纯函数/常量）。
2. `web/src/sandbox/protocol.ts` 改为从 `../utils/sandboxProtocol` import/re-export（保持对外导入面不变）。
3. 新建 `web/src/utils/sandboxBroadcast.ts`（L1）：把 `liveSandboxes` 注册表与 `broadcastMvuUpdate` / `broadcastCardEvent` 原样搬入；新增 `registerSandboxSink(nonce, win)` / `unregisterSandboxSink(nonce)`。
4. `sandbox/iframe-lifecycle.ts` 的 set/delete 改调 register/unregister；保留 `broadcastMvuUpdate` / `broadcastCardEvent` 的 re-export（兼容既有导入），但实现来自 L1。
5. `stores/chat.ts` 改为从 `../utils/sandboxBroadcast` 导入；`characterScriptSandbox.ts` 门面可继续 re-export（外部 API 不变）。
6. 删除 `arch-layers.json` 的 `stores → characterScriptSandbox` 条目；同步 `docs/契约-架构与数据.md` 的 4→3（wiring 3 / debt 0）。

**验收**：

- `node tools/check-arch.mjs` → 已登记依赖 3 条（wiring 3 / debt 0），无 stale；
- 前端测试：sandbox 生命周期/广播相关用例全绿；新增 `register/unregister/尺寸上限/空注册表` 用例；
- 浏览器/SSR 冒烟：切换 swipe 触发 `message_swiped`、MVU 变量更新触发状态栏重渲染（现有 `sandbox/*.test.ts` + 真实页面各一条）；
- `npm run typecheck -w web`、`npm run build -w web`、`check-test-ratchet`。

**备选方案**：事件总线（store emit → sandbox host subscribe）。审计的 remediation 提到过，但改动面更大、需要新的生命周期约定；**优先选 L1 下沉**，事件总线只在 L1 方案被证明不可行时再评估。

**风险**：低–中。必须保持：`postMessage('*')` 的 channel/nonce、1MB 尺寸守卫、iframe 销毁时 unregister、广播顺序与 payload 克隆语义。

---

### R-4 `TaskDetail` / `TaskUsageTotal` Rust 结构体（+ `ts-rs` 决定）

**现状证据**：

- 后端详情响应用 `json!` 手工拼：`server-rs/src/api/tasks.rs:263`（详情）、`:819`（全局 usage）。
- Rust 数据模型：`TaskRecord`（`models/types.rs:1600`）、`TaskSubtaskRecord`（`:1671`）、`TaskMessageRecord`（`:1717`）；**没有** `TaskDetail` / `TaskUsageTotal` 结构体。
- 前端手写：`web/src/api/types/task.ts:196`（`TaskUsageTotal`）、`:223`（`TaskDetail`）。
- `check-contract` 当前 **29 组映射**（`tools/check-contract.mjs` 的 `MAPPINGS`），未覆盖 `TaskDetail`。
- 仓库**没有** `ts-rs` 依赖。

**目标**：把线格式收进 Rust 结构体，前端类型有机器对拍；不改 JSON 字段名/可选性。

**推荐步骤（先结构体，后决定 ts-rs）**：

1. 在 `api/tasks.rs`（或 `api/tasks/detail.rs`）定义：
   ```rust
   #[derive(Serialize)]
   struct TaskUsageTotal { prompt_tokens: i64, completion_tokens: i64, reasoning_tokens: i64 }
   #[derive(Serialize)]
   struct TaskDetailResponse {
       task: TaskRecord,
       subtasks: Vec<TaskSubtaskRecord>,
       usage_total: TaskUsageTotal,
       messages: Vec<TaskMessageRecord>,
       flow_snapshot: Option<crate::services::agent_flow_service::FlowSnapshot>,
       workspace_profile: Option<crate::services::workspace_profile::WorkspaceProfile>,
       delivery: Option<crate::services::task_engine::delivery::TaskDeliverySummary>,
   }
   ```
   字段名与当前 `json!` 逐字一致；`usage_total` 全量端点单独保留（不要硬塞进同一个结构体）。
2. 用 `Json(TaskDetailResponse { … })` 替换 `json!`；现有 JSON 形状零变化。
3. 给 `tools/check-contract.mjs` 增加 `TaskDetailResponse` ⇄ `TaskDetail`、`TaskUsageTotal` ⇄ `TaskUsageTotal` 两组字段映射（30/31 组）；同步 `docs/契约.md` 的 MAPPINGS 表与计数（`check-doc-claims` 会钉住）。
4. **`ts-rs` 决策**：审计 §九.3 明确「短期不建议引入」；先做结构体 + `check-contract` 字段对拍。等 `TaskDetail` 字段继续增长、手工映射开始频繁漏改时，再单独评估 `ts-rs`（会改 Cargo.lock / npm 审计面）。

**验收**：

- `cargo test` 全绿；现有任务详情集成用例零修改；
- `check-contract` 新映射通过（29→30/31），`check-doc-claims` 计数一致；
- 前端 `vue-tsc`、`npm test -w web`；
- 手动/集成对比一份详情响应 JSON（字段名、null/缺失语义不变）。

**风险**：低（结构体化不改线格式）。坑：`flow_snapshot` 目前是 `Value`/可选，别为了类型纯洁去改生成逻辑；`workspace_profile` 是实时探测，不能挪进 DB。

---

### R-5 `check-settings-parity` 增类型/取值域

**现状证据**：

- `tools/check-settings-parity.mjs` 只比**字段名集合**与「是否可选」；文件头自述「不涉及类型」。
- 当前三组：PUT 75 / GET 77 / 连接 14；差异常量 `GET-only 4 / PUT-only 2 / 同名重定义 4`。
- 前端 `RuntimeSettingsPatch` 是派生类型：`web/src/api/types/settings.ts:271`（GET-only）、`:282`（同名重定义）、`:292`（Overrides）、`:314`（`Partial<Omit<…>>`）。
- 后端 patch 分域在 `server-rs/src/api/settings/patch/{connection,generation,agent,tools,memory,task,bundles}.rs`，每个域一个 `apply` 返回 `PatchError{field,message}`。

**目标**：字段名之后，再锁「类型类别」与「取值域」，让「后端 `u32` / 前端 `string`」「后端 `0.0..=2.0` / 前端任意 number」这类漂移在门禁转红。

**推荐分两步**：

1. **类型类别（先做，收益大）**：
   - 扩展 `rustStructFields` 同时取字段类型 token（`pub name: Type,`）；
   - 扩展 `tsInterfaceFields` 同时取 TS 类型（`name?: type;`）；
   - 归一化类别：`bool` / `number` / `string` / `string[]` / `enum(字面量联合)` / `object` / `unknown`;
   - 规则：类别不一致即 FAIL；`Option<T>` ⇄ `T | null` 视为同一类；`Option<T>` ⇄ 可选字段允许。
   - 增加自测：临时把 `web/src/api/types/settings.ts` 某字段 `boolean` 改 `string` → 门禁红；改回绿。
2. **取值域（后做，需字段表）**：
   - 在脚本内建 `PATCH_RULES` 表（字段 → 允许值/区间），来源逐条对应 `patch/*.rs` 的 `validation` 与前端 `as const` 联合（如 `COMPACTION_MODES`、`PRESET_TAIL_ROLES`）；
   - 新增字段若既不在 `PATCH_RULES` 也无可推导规则 → FAIL（强制登记），而不是默认放过；
   - 测试：把某个区间从 `0.0..=2.0` 改成 `0.0..=3.0` 或前端联合少一个值 → 门禁红。

**验收**：

- 现有 `check-settings-parity` 输出三组计数不变；
- 类型/取值域的人为改动均能转红；
- `check-doc-claims`（阶段数/映射数）与 `MAINTENANCE.md` 门禁说明同步；
- 保持纯 Node 零依赖（不要引入 TS 编译器/解析器）。

**风险**：中（脚本解析 Rust/TS 文本，边界情况多）。建议先只覆盖 `RuntimeSettings` / `UpdateSettingsBody` / `ConnectionProfile*` 三组，不追求全仓类型。

---

## 三、推荐批次与顺序

| 批次 | 内容 | 预估 | 依赖 | 回滚 |
|---|---|---|---:|---|
| **C-2F-1** | R-1 settings_service 测试搬出 | 0.5 天 | 无 | revert 单提交 |
| **C-2F-2** | R-2 executor 测试搬出 + 生产分区 | 1–2 天 | 无（可与 R-1 并行） | 两笔独立提交，先测试后分区 |
| **C-2F-3** | R-3 sandbox 广播下沉 L1 | 0.5–1 天 | 无 | revert 单提交 |
| **C-2F-4** | R-4 TaskDetail/TaskUsageTotal 结构体 + 映射 | 0.5–1 天 | 无 | revert 单提交 |
| **C-2F-5** | R-5 settings parity 类型/取值域 | 0.5–1 天 | R-1b 可选（settings 域稳定后） | revert 单提交 |

**顺序理由**：

- R-1 先做：2,126 → ~402，纯搬运，立刻把 `settings_service` 从基线里拿掉；先验证「搬测试」流程。
- R-2 紧随：executor 是任务模式核心，趁 `tool_loop` 拆分后的结构稳定，先把 685 行测试隔离，再分区生产代码；两笔提交保证可独立回滚。
- R-3 独立：纯前端分层，和 R-1/R-2 无冲突；做完后 `arch-layers.json` 的 debt 归零。
- R-4 独立：只动 API 序列化层，线格式零变化；先结构体后映射，不引 ts-rs。
- R-5 最后：它依赖 settings 域稳定（R-1/R-1b），且是工具链改动，失败不影响产品行为。

**每批一个 PR**：纯搬运与行为改动分开提交；R-2 的测试搬出/生产分区必须两笔。PR 合并后删除临时分支（本地 + 远端）。

---

## 四、验证命令与证据

### 4.1 后端

```bash
# 机器环境：vcvars64（见 AGENTS.md「本机环境」）
cargo fmt --manifest-path server-rs/Cargo.toml --all --check
cargo clippy --manifest-path server-rs/Cargo.toml --all-targets -j 4 -- -D warnings
cargo test --manifest-path server-rs/Cargo.toml --workspace -j 4
node tools/count-tests.mjs --check
```

**LNK1318 / PDB 坑（2026-10-25 实测）**：全量链接若报 `LNK1318: PDB 错误: LIMIT (12)`，先删旧 PDB 再 `--no-run`：

```bash
rm -f server-rs/target/debug/deps/*.pdb
cargo test --manifest-path server-rs/Cargo.toml --workspace -j 4 --no-run
cargo test --manifest-path server-rs/Cargo.toml --workspace -j 4
```

这是链接器/旧 PDB 环境问题，不是代码回归；不要改依赖或删测试来「修」它。

### 4.2 前端与门禁

```bash
node --test tools/check-arch.test.mjs
node tools/check-arch.mjs
node tools/check-file-size.mjs
node tools/check-contract.mjs
node tools/check-settings-parity.mjs
node tools/check-lock-sync.mjs
node tools/check-css-discipline.mjs
node tools/check-test-ratchet.mjs
node tools/check-frontend-lint.mjs
node tools/check-docs.mjs
node tools/check-doc-claims.mjs
node tools/check-tutorial.mjs
node tools/count-tests.mjs --check
npm test -w web
npm run typecheck -w web
npm run lint -w web
npm run build -w web
node tools/check-bundle.mjs
```

改完超千行文件后**必须**：

```bash
node tools/check-file-size.mjs --update
git diff -- tools/file-size-baseline.json   # 只允许向下；新增 >1000 文件会 FAIL
```

### 4.3 真实模型（任务/模型链路改动必跑）

- 隔离实例：`DATA_DIR=D:/kedai-bench-run/data-c2f-<批次>`、`PORT=30xx`；启动后**先核对日志 `"data_dir":"..."`**。
- 凭据只在仓外：`D:\kedai-bench-run\realmodel.env` / `realmodel-put.json`（禁止复制进仓库）。
- 最小连通：`KEDAI_BASE=http://127.0.0.1:30xx node D:/kedai-bench-run/realmodel-smoke.mjs`
  （期望 bootstrap 200 → PUT settings 200 → generate-raw 200，`text=连接成功`）。
- 任务窄探针：`node tools/bench/task-probe.mjs`（solo + team 各一条；R-2 必跑）。
- 证据落 `D:\kedai-bench-run\<批次>-evidence\`；提交正文写「真实模型实测通过 + 证据路径 + 关键结论」。
- 收工**必杀进程**（按端口找 PID，不要 `taskkill /IM kedai-server.exe` 误杀用户 3001 实例）。

---

## 五、明确不动的边界（审计 §七，勿在 C-2F 顺手推翻）

- **`models/types.rs` 大拆分**：已登记知情接受；传输类型混装，拆它只改 import、不改线格式，收益低、冲突面大。R-4 只**新增** `TaskDetailResponse`，不动 `types.rs` 的既有结构。
- **`TaskBackend` 的 8 个窄接口**：刻意依赖倒置，每个接口 1 个实现，不再切。
- **`wiring.rs` / `delivery.rs` / `task_service/db.rs`**：纯函数/内聚/测试密，行数是固有复杂度，不是重复。
- **EJS 解释器 / 世界书解析 / 插件 / MCP / undo / secret_store**：高风险安全面，只接受安全修复。
- **六类文档台账**：48k+ 行是刻意设计；不要以「太长」重写或合并。
- **`check-arch` / `check-contract` / `check-doc-claims`**：正在有效工作；改覆盖缺口，不要降基线/加白名单。
- **任务事件 `switch`**：有 `never` 穷尽断言，是良性复杂度。
- **前端沙箱 / mini-jQuery / 渲染 sanitize**：跨 realm 自包含副本 + 安全关键路径，不要为「统一 v-html」重写。
- **`importBundle.ts`**：纯函数、零 IO、单测充分；只做小辅助。
- **legacy / plan / custom 的 step 循环合一**：`docs/遗留.md` 已书面裁定回归风险高于收益；只抽纯函数终态，不动控制流。

---

## 六、已知坑与回滚

1. **`tests.rs` 必须 `#![cfg(test)]` 在列首**：`check-arch` 的 `productionLineCount` 靠它切生产区；否则规则 E（services 生产代码禁 `.expect`）会把测试断言全判红。参照 `team/tests.rs`、`agent_flow_service/tests.rs`。
2. **子模块路径**：`super::` 在 `tests.rs` 里指向父模块；跨目录搬函数时改成 `crate::…`，方法跨模块用 `pub(super)`/`pub(crate)`。C-2D 踩过。
3. **`count-tests` 口径**：`tests/common/` 已排除；`src/**/tests.rs` 里的 `#[test]` 仍计入静态数。搬测试**不应**改变 2044。
4. **`check-file-size --update` 只许下调**：直接改 JSON 绕过 `--update` 等于手动放宽 ratchet；提交正文必须说明每一次基线变化。
5. **`arch-allow-file` 只认文件头 20 行**；`arch-allow` 认当前/上一行；新增直写优先走 action，别加豁免。直写总量基线 113，超出即 FAIL。
6. **`check-settings-parity` 解析假设**：Rust 结构体字段 4 空格、TS 接口 2 空格、`RuntimeSettingsPatch` 的三组差异常量（GET-only / PUT-only / Overrides）必须同步；加类型解析后先跑 mutation 反证。
7. **sandbox 广播**：`postMessage('*')`、`channel`/`nonce`、1MB 上限、iframe 销毁 unregister、payload 克隆语义都不能变；R-3 是纯下沉，不是重写协议。
8. **真实模型**：coding 类任务新 DATA_DIR 的 `exec_enabled` 默认关；推理模型 `max_tokens` ≥256（32 会 200 但 content 空）；429 周额度耗尽时按「确实不可测」登记，不要误判配置。
9. **回滚**：R-1/R-2 按提交回滚（测试搬运与生产分区分开）；R-3/R-4/R-5 单提交；文档/基线最后回滚。

---

## 七、交接检查表（复制到 PR 描述）

- [ ] 已跑 `node tools/check-file-size.mjs --verbose`，确认基线 30 与本次目标文件
- [ ] R-1：`settings_service/mod.rs` ≤500，`tests.rs` 首行 `#![cfg(test)]`，基线减少 1
- [ ] R-2：`executor/tests.rs` 首行 `#![cfg(test)]`；生产文件 <1000；`executor.rs` 基线移除
- [ ] R-3：`check-arch` 4→3（wiring 3 / debt 0），无 stale；sandbox 广播用例绿
- [ ] R-4：`check-contract` 新映射通过；详情 JSON 字段名/可选性逐字不变
- [ ] R-5：类型/取值域 mutation 反证均转红；脚本仍零依赖
- [ ] `cargo fmt --check` / `clippy -D warnings` / `cargo test --workspace -j 4`
- [ ] `npm test -w web` / `vue-tsc` / ESLint 0 error / `vite build` / bundle
- [ ] 全部 Node 门禁（含 `check-docs` D7 / `check-doc-claims` / `count-tests --check`）
- [ ] 真实模型：最小连通 +（R-2）solo/team 窄探针；证据路径已写入提交正文
- [ ] `docs/计划.md` C-2 残余、`docs/契约-架构与数据.md`、`MAINTENANCE.md` 计数同步
- [ ] PR CI `check` / `check-ubuntu` 双绿后合并；临时分支已删

---

## 八、参考

- 审计报告：[`CODEBASE-DEBT-AUDIT-2026-10-24.md`](CODEBASE-DEBT-AUDIT-2026-10-24.md)（C-2E 追注 + §七 不动的边界 + §九 待拍板）
- 上一份交接：[`C2-0-HANDOFF-2026-10-24.md`](C2-0-HANDOFF-2026-10-24.md)（体例参照；其 §五「剩余事项」已被本文件取代）
- 计划台账：`docs/计划.md` 的 C-2 章（「C-2 残余」五项）
- 门禁与构建：`AGENTS.md`、`MAINTENANCE.md` §0/§2/§11
- 测试夹具：`server-rs/tests/common/mod.rs`（C-2E 收口；新增测试不要再复制夹具）
- 经验：`docs/经验.md` E134（批量删除区间重叠）、条目 29（cargo 并发/PDB）
- 证据：`D:\kedai-bench-run\c2e-evidence\`（C-2E）；本批执行后新建 `c2f-*-evidence\`
