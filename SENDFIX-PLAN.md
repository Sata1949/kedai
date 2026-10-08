# SENDFIX 执行稿 —— 发送失败可见性与停止竞态修复（0.4.1 撤回后的修复批）

> **性质**：批次**执行稿**（临时件，执行完删除；durable 条目见 [`docs/计划.md`](docs/计划.md) 的
> SENDFIX 章，二者内容以计划条目为准）。
> **登记**：2026-10-08。**状态**：待用户确认后开工（未启动）。
> **发布处置**：修复收口后以 **0.4.2** 重新发布（0.4.1 的 GitHub Release 与 tag 已于 2026-10-08 撤回）。

## 一、背景与结论摘要

0.4.1 正式版（2026-10-07 发布、2026-10-08 撤回）发布后同日发现「发送/生成失败**无任何可见反馈**」
缺陷族。2026-10-08 在 **0.4.1 同代码**（`820744b` 之后构建的 debug server；前端 `web/dist` 指纹
`889974e238c8…` 与发布版 sidecar 逐字节一致）上做了**真实模型实测 + 内置浏览器端到端驱动**，结论：

1. **基础发送链本身完好**（三次实测全过）：
   - `POST /api/chat/send` → 上游 `provider_round_start` → `token×n` → `finish`（fast 模式，新会话）；
   - 同一会话连续第二条消息 → `finish`；
   - AGENT 模式 → `calculator(12*34)` 工具调用 → 最终回复落库并渲染。
2. **缺陷在失败路径**：上游失败（429 / 网络 / 鉴权）与 HTTP 层拒绝（400/404/409）**全部表现为「什么都没发生」**：
   消息看似发出、没有回复、没有任何错误文案、没有重试入口——用户视角即「无法正常发送消息」。
3. 另有**停止竞态**：点「停止生成」后约 2 秒窗口内重发 → 后端 409 → 前端把本地草稿与输入文本一起
   静默吞掉（消息闪一下即消失）。

> 触发本次排查的现实链路：发布当日上游网关周额度耗尽（429，重置 2026-10-12T04:16:05Z），
> 任何人用该额度发送都会命中「失败无反馈」，从界面上完全无法区分「额度问题」与「应用坏了」。

## 二、复现证据（0.4.1 同代码，真实模型 + 本地 mock）

隔离实例：`DATA_DIR=D:\kedai-bench-run\v041-repro\data-src`、`PORT=3061`、`LOG_DIR=…\logs-src`，
启动先核日志 `"data_dir"` 行；上游为 commandcode 网关（新密钥，`deepseek/deepseek-v4.1-flash`）。

| # | 场景 | 结果 | 证据 |
|---|---|---|---|
| 1 | fast 首条（API 探针） | ✅ `token×7 → finish`，5.4s，落库正常 | `v041-repro/probe-run2.log` |
| 2 | 浏览器 UI 发首条 | ✅ 用户气泡→回复渲染→计数更新，无 JS 错误 | `logs-src/kedai-2026-10-08.log` 10:45 段 |
| 3 | 同会话连续第二条 | ✅ `finish` 正常 | 同上 10:45:46–51 |
| 4 | AGENT 模式 + 工具 | ✅ `calculator "12*34" → 408` → 最终回复 | 同上 10:49:56 段 |
| 5 | **上游 429（mock）** | ❌ **零反馈**：服务端 `[agent] error`（429 详情）后，界面 12 秒高频采样无任何错误字样；AgentPanel 停在「阶段：就绪 / 推理链(0)」 | `logs-src` 10:50:46 段 + `mock-429.mjs` 记录 |
| 6 | **停止后立即重发** | ❌ `10:55:12.659 stop → 10:55:13.132 send 409`；消息与输入文本一起消失，零提示；约 1.8s 后 `[agent] interrupted` | `logs-src` 10:55 段 |
| 7 | 恢复验证（窗口后发送） | ✅ 正常 `finish`（缺陷是窗口 + 静默，不是永久锁死） | 同上 10:55:30 后 |

## 三、缺陷与修复方案

### SENDFIX-1（P0）生成失败零反馈：错误态被历史恢复擦除

**机制链**（实测 + 读码）：

1. 服务端 → SSE `error` 事件（`server-rs` 侧一切正常，错误已落 `agent_sessions.state = "error"`；
   `GET /api/chat/sessions/{id}/agent/trace` 实测返回 `{"state":"error","tool_calls":[]}`）。
2. `web/src/sseReducer.ts:215-242` `error` 分支 → 写 agent 面板（`phase='error'`、`stepText='执行出错'`、
   推理链、`retryable`），并返回 `reloadHistory`。
3. `web/src/stores/chat.ts:129` `restoreAgentTrace` → **`:134` 判据 `!trace || trace.tool_calls.length === 0`
   → `:135` `agent.value = idleAgent()` 无条件重置**，把第 2 步写好的错误态擦掉。
4. 错误唯一渲染点 `web/src/components/AgentPanel.vue:483-495`（右栏「操作」行的「重试」按钮，
   需 `phase==='error'`）此后永不满足；对话消息列表**没有任何错误渲染**。

**改动点**：

- [ ] `restoreAgentTrace` 尊重 `trace.state`：仅当「trace 缺失」或「state===idle 且无工具调用」才空闲化；
      `state==='error'` 时还原错误态（phase/文案）。
- [ ] 错误落到**消息流可见层**：`sseReducer` 的 `error` 分支在对话区追加一条错误卡（code + 详情 + 「重试」），
      重试复用 `store.resendMessage(lastUserMessageId)`；不再依赖 AgentPanel 瞬态。
- [ ] `web/src/api/chat.ts:118-126` 非 2xx 合成路径：`step('请求失败')+finish` 改为发 `error` 事件
      （携带服务端文案/码/retryable），避免 `finish` 分支把阶段改写为「完成」。

### SENDFIX-2（P0）停止后竞态窗口：重发被 409 静默吞掉

**机制链**：`chat.ts:464 stop()` 本地立即 `generating=false`（乐观中断事件），但后端要 ~1.8s 才真正收尾
（`[agent] interrupted` 才落日志）；窗口内 `server-rs/src/api/chat.rs:132-134` 因 `engine.is_active` 直接
409「该会话正在生成中」；前端非 2xx → 合成 finish → `reloadHistory` 用服务端历史整表替换 → 本地草稿气泡
（负 id）被替换掉，而 `web/src/components/ChatInput.vue:152` 发送前已 `text.value=''` 清空输入 → **输入丢失**。

**改动点**：

- [ ] 前端「停止中」过渡态：`stop()` 后保持 `generating=true` 直到收到 `interrupted` 终态（带超时兜底），
      发送入口禁用并显示「正在停止…」——把可发送窗口消掉。
- [ ] 发送失败保留草稿：`ChatInput.send` 改为按结果清空（或失败回填文本与附件）。
- [ ] （需裁定）后端放宽 `chat.rs:132` 的 409 早退，交由引擎既有「同会话抢占（abort 旧 run + 覆盖 runs 表）」
      语义处理；若采纳，窗口内发送 = 旧生成中止 + 新生成开始，彻底不丢消息。

### SENDFIX-3（P1）失败路径回归网

- [ ] vitest：reducer error 分支；`restoreAgentTrace`（空 tool_calls + state='error'）；非 2xx 合成 error；
      ChatInput 草稿保留；stop 后 generating 保持。
- [ ] 若采纳后端改动：Rust 集成用例「stop 后立即 send 的行为」断言。

## 四、验收断言

**真实模型实测**（新密钥已可用；证据落 `D:\kedai-bench-run\sendfix-evidence\`）：

1. 回归：fast 首条 / 连续第二条 / AGENT 工具调用 → 均 `finish`（防止修缺陷伤正常路径）。
2. 429 mock：界面出现错误卡 + 「重试」；切回真实网关点「重试」→ 正常 `finish`。
3. 停止窗口：发送 → 1s 停止 → 0.25s 重发 → **不丢消息**（被明确阻止并提示，或正常开始新一轮）。
4. HTTP 400 路径（如带图 + 未开视觉能力）→ 错误可见且输入文本保留。

**门禁**：`npm run check` 全量 18 段 EXIT 0；`.\build.ps1` 双端同步（0.4.2 发布前）。

## 五、版本与发布处置

- 0.4.1：Release 与 tag 已撤回（见 `docs/功能-变更史.md`「2026-10-08 0.4.1 撤回」章）。
- 修复后：**0.4.2** 重新发布（口径：版本号是公开标识不复用；`versionCode` 派生 4002，
  顺带消除与已分发 alpha 同号 4001 的覆盖安装歧义）。
- 发布步骤复用 0.4.1 章的全套流水线（bump-version / 双端构建 / APK / 7z / Release 建档 / 摘要双验收）。

## 六、台账同步清单（收口时）

- [ ] `docs/功能-变更史.md`：修复批次章（提交 hash + 证据）+ 0.4.2 发布章 + 索引补挂。
- [ ] `docs/计划.md`：SENDFIX 章状态行更新（未启动 → 已完成，写实测结论与证据路径）。
- [ ] `docs/经验.md`：两条新坑（错误态被 trace 恢复覆盖 / stop 后窗口期 409 静默吞消息）。
- [ ] `docs/遗留.md`：若存在未修边缘（如后端 409 语义未采纳），登记诚实边界。
- [ ] 计数回填：`node tools/count-tests.mjs`（新增测试后）。
- [ ] 删除本执行稿。

## 七、风险与回滚

- 前端改动集中在 `stores/chat.ts` / `sseReducer.ts` / `api/chat.ts` / `ChatInput.vue` / `AgentPanel.vue`：
  均为错误路径增强，正常路径断言同批回归。
- 后端可选改动（SENDFIX-2 第 3 点）触碰发送入口语义，风险最高——**需用户裁定**后再动；
  不采纳也不阻塞前两点修复。
- 回滚粒度：按提交拆分（见下），任一提交可独立 revert。

## 八、提交拆分（待用户确认后执行）

1. `fix(前端): SENDFIX-1 错误可见性——错误卡落地对话区 + trace 恢复尊重 error 态 + 非 2xx 走 error 通道`
2. `fix(前端): SENDFIX-2 停止竞态——停止中过渡态 + 发送失败保留草稿`
3. `docs(台账): SENDFIX 收口——变更史/计划/经验/计数 + 实测证据`（含 SENDFIX-3 测试与可选后端项的最终口径）
