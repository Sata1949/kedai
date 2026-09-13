# Kedai 三结合梯队架构（老中青三结合）

> 借鉴我国干部队伍/领导班子建设中长期沿用的「老中青三结合」组织原则，将其四条精神
> （优势互补 / 传帮带 / 梯队衔接 / 动态循环）映射为代码治理模型，作为 kedai 演进的总纲。
> 本文件是架构决策依据：**任何结构性改动先对照本文件判断落点与代际影响**。

---

## 1. 「老中青三结合」的含义与内容

「老中青三结合」是干部队伍与领导班子配备的指导原则：以**老（经验者）**把关定向、
**中（骨干）**承上启下、**青（后备）**注入活力，三部分按合理梯次组合，保证事业平稳
交接、后继有人、不断层、不僵化。

| 组成部分 | 特点与作用 |
|---|---|
| 老（经验者） | 经验丰富、把关定向、原则性强；「传帮带」的主要承担者，防风险的压舱石 |
| 中（骨干） | 年富力强、业务成熟，处于干事创业黄金期；承上启下、攻坚执行的中坚 |
| 青（后备） | 朝气蓬勃、学习力强、可塑性强；队伍的活水与未来，敢闯敢试 |

**四条精神内核**：
1. **优势互补**：经验 + 成熟 + 活力互补，避免「全老则僵化保守、全新则轻率失稳」的单结构缺陷；
2. **传帮带**：老带中青、经验传承，在实践中培养，防止能力断层；
3. **梯队衔接**：合理梯次、后继有人，保证事业平稳交接、不断层；
4. **动态循环**：青年成长为骨干、骨干沉淀为经验者，队伍新陈代谢、永续发展。

---

## 2. 三层映射（老 · 中 · 青 → 稳 · 干 · 活）

### L1 老层 · 稳 —— Anchored Core（基石层）
**定位**：长期验证的稳定核心与兼容契约。改动成本最高，一旦破坏影响全局。
**纪律**：不可变契约优先；任何改动须「传帮带评审」（带测试、不破坏协议、声明向后兼容影响）。

kedai 落点：
- `parsing/`（角色卡 V2/V3、世界书、mvu 协议兼容解析、EJS 渲染契约）
- `models/`（SQLite 表结构、types 契约）
- **既有测试套件**（后端 740 单元 + 183 集成 = 923,前端 697 个,2026-09-13 实测=经验库,是「传帮带」的第一载体）
- SillyTavern 生态兼容承诺（角色卡/世界书/正则脚本/酒馆助手协议）

### L2 中层 · 干 —— Orchestration（骨干层）
**定位**：承上启下、业务编排与翻译适配。是日常开发的主战场，改动频繁但受老层契约约束。
**纪律**：依赖老层契约、不得绕过；负责协议翻译与新旧字段兼容（如「未知字段无损保留」）；给青层提供受控接口。

kedai 落点：
- `agents/`（engine 编排、状态机、planner/reflector）
- `services/`（角色/会话/世界书/注入服务）
- `api/`（路由层、鉴权、SSE 流）

### L3 青层 · 活 —— Frontier（探索层）
**定位**：新能力、实验、快速迭代、失败隔离。允许「先跑起来再打磨」，但默认隔离。
**纪律**：不直接触碰老层数据契约；新能力先在此验证，成熟后按晋升通道升级。

kedai 落点：
- `tools/`（工具注册：calculator/memory/search/自定义工具插件）
- `skills/`、`examples/`（技能库、示范模板）
- 脚本沙箱（`characterScriptSandbox.ts`）、远程资源代理
- 本轮新能力：GENERATE/@INJECT 注入、@@ 装饰器、EJS 读取 API、统计变量
- **EJS 自研解释器（`parsing/assistant/ejs/`）· 冻结**：只接受安全修复，**新模板能力一律在
  `scripts/runtime.rs` 的 rquickjs 沙箱侧实现**（rquickjs 自带内存/中断/栈上限）。该解释器
  已加固「循环步数 + 墙钟预算 + 解析深度守卫」（2026-09-13），但不再扩展——自研引擎缺
  引擎级沙箱限额，继续加功能会扩大不可信输入的攻击面。详见 `MAINTENANCE.md §0` 冻结纪律。

---

## 3. 三种「结合」机制

### 3.1 传帮带 · 晋升通道（动态循环）
青层创新成熟 → 过晋升门槛 → 升入中层打磨 → 沉淀为老层契约。
**晋升门槛（三关）**：① 通过既有测试套件（经验库）；② 不破坏兼容协议；③ 经评审确认边界。
- 新能力一律先进 L3 验证（本轮 GENERATE/@INJECT/EJS API 即走此路）；
- 反例（禁止）：新代码直接改写 L1 契约而不带测试 → 破坏经验传承。

### 3.2 优势互补 · 边界
- **老层**：不可变契约，防青层误伤、防中层漂移；
- **中层**：翻译适配（V2/V3、新旧字段、协议转义），是「经验 → 实现」的桥梁；
- **青层**：隔离试探（沙箱/功能开关），防架构僵化。
三层缺一不可：全老 = 僵化不可演进；全新 = 失稳无锚点。

### 3.3 防断层 · 兼容承诺
- 兼容协议是「老层经验」的载体：演进可迁移、不破坏（数据迁移幂等、v3fix 重刷、SQLite 迁移）；
- 数据结构演进必须带迁移路径（如旧字段兼容解析、`serde(default)` 缺省）；
- 跨端协议（前后端 mvu）以协议文档锁定双端一致，防「代际交替」时行为漂移（见 `docs/mvu-protocol.md`）。

---

## 4. kedai 模块代际归属清单

| 模块 | 代际 | 说明 |
|---|---|---|
| `parsing/character_card.rs` | L1 | 角色卡 V2/V3 解析（兼容契约） |
| `parsing/world_book.rs` | L1 | 世界书解析 + 条目匹配（含 @@ 装饰器） |
| `parsing/macros.rs` / `preset.rs` / `regex_script.rs` | L1 | 宏、预设、正则脚本解析 |
| `parsing/assistant/` | L1 | mvu 变量树 / 输出协议 / EJS 解释器（酒馆助手兼容层） |
| `models/` | L1 | SQLite 表结构 + 类型契约 |
| `agents/engine/` | L2 | Agent 编排（run 流程、消息构建、mvu 应用） |
| `agents/planner.rs` / `reflector.rs` / `state_machine.rs` | L2 | 计划/反思/状态机 |
| `services/` | L2 | 业务服务（角色/会话/世界书/注入/设置/密钥） |
| `services/task_engine/` | L2 | 六模式任务引擎（EventSink/ModeExecutor 底座 + solo/multi/plan/team/custom 执行器;legacy 仍驻 task_service）——语义见 `docs/任务引擎六模式.md` |
| `api/` | L2 | 路由 + 鉴权 + SSE |
| `connectors/` | L2 | LLM 后端适配 |
| `tools/` | L3 | 工具系统（registry/calculator/memory/search） |
| `services/undo_service.rs` + `api/undo.rs`（批次 6.1） | L3 | 回退快照/undo：写工具（write/replace/create/update_variables/memory_write）执行前取逆操作负载落 `undo_snapshots` 表，`POST /api/undo/{id}/restore` 逆序恢复；默认开，`undo_enabled` 设置开关隔离（命名避开 contracts 的 checkpoint/StateCheckpoint） |
| `mcp/`（批次 6.2） | L3 | MCP stdio 客户端：JSON-RPC 2.0（NDJSON 行帧）托管子进程，工具以 `mcp_{server}_{tool}` 前缀注册进 ToolRegistry（未知工具默认 Dangerous 裁决自动覆盖）；默认关，`mcp_enabled` 设置开关 + `mcp_servers` 列表隔离，仅启动时装配（改设置需重启生效，无热重连） |
| `web/src/mvu/`（前端） | L1+L3 | 变量树展示/回放（L1 协议）+ 宿主集成（L3） |
| `web/src/components/` | L3 | UI 组件 |
| 测试套件（`tests/` + `*.test.ts`） | L1 | 经验库，晋升门槛的裁判 |

---

## 5. 代际流动规则与评审纪律

1. **改 L1**：先写失败测试 → 最小实现 → 声明兼容影响（MAINTENANCE §架构治理）→ 评审；
2. **改 L2**：不得绕过 L1 契约直接改数据层语义；协议翻译集中在中层；
3. **进 L3**：新能力自由，但必须默认隔离（沙箱/开关/注册表），且纳入晋升跟踪；
4. **晋升**：L3 → L2：能力被 2 个以上调用点复用且测试覆盖；L2 → L1：协议稳定、外部依赖成型；
5. **Mutex/并发纪律**：禁止持 `std::sync::Mutex` guard 跨 `.await`（Clippy `await_holding_lock` 防护）；
   锁中毒一律恢复（`unwrap_or_else(|e| e.into_inner())`），不 panic；
6. **文档先行**：结构性改动先更新本文件或对应协议文档，再动代码（传帮带：先稳基座再动骨干）。

---

## 6. 本轮重构的三层执行对照

| 阶段 | 层 | 内容 |
|---|---|---|
| 阶段 0 | 全局 | 本文件（定调） |
| 阶段 1 | L1 加固 | Mutex 76 处中毒恢复（经验库更可靠） |
| 阶段 2 | L2 清障 | assistant 拆 6 模块、agent_tools 拆 4 模块、engine run 拆分 |
| 阶段 3 | L3 放活 | mvu 语义对齐 + 协议文档（传帮带经验文本） |
| 阶段 4 | 落实 | EJS 加固+冻结、任务引擎依赖倒置（`task_core`/`TaskBackend`）、门禁接线（build/pre-push）、L1 渗透清理 |

---

## 7. 叙事与实现的偏差登记（2026-09-13 核查）

> 本章如实记录「本文件描述的架构」与「代码实际状态」曾经的差距,以及核查后的处置。
> **用途**:防止后续 agent 照着旧叙事改错位置;也是「传帮带」的经验留存。
> 核查方法:`grep` 交叉验证 + 全量测试(当时 975 后端 / 733 前端)+ 依赖图分析。

| # | 叙事曾声称 | 代码实际(核查时) | 处置 |
|---|---|---|---|
| 1 | L1 是「不可变契约、上层不得渗透」 | **被 L2 反向渗透**:`contracts/registry.rs:11-12` 引 `services::character_service/world_book_service`;`parsing/preset.rs:24` 引 `services::prompt_inject_service` | 已修:`PromptFloor` 等类型下沉 `models/types.rs`;registry 改为闭包注入(见 §4)。check-arch 规则 D 锁死 |
| 2 | L2 内部单向分层 | `services/task_engine/**` ↔ `services/task_service/**` **双向 import**(引擎侧 10 个文件引用 TaskService) | 已修:抽 `task_core`(共享类型/常量)+ `TaskBackend` trait(宿主能力),`task_engine` 对 `task_service` 零引用。check-arch 规则 C 锁死 |
| 3 | legacy/followup「上 ModeExecutor 缝」 | 上了缝,但经 `OnSuccess::SelfFinalized` **自行写库**,绕过统一收尾(4 个收尾入口重复同构) | 已修:终态值化(`TaskTerminal` 五变体),执行器只返回值;4 入口收敛为 `finalize_terminal` 单一出口 |
| 4 | EJS 为 L3 探索层「新能力」 | 自研解释器缺引擎级沙箱限额:循环无迭代上限、解析无深度守卫(单卡可挂死/栈溢出) | 已修:加迭代+墙钟预算与 3 处解析深度守卫;**并冻结**(新能力一律走 rquickjs 沙箱) |
| 5 | 测试套件是 L1「经验库/晋升门槛」 | 数字长期手抄(文档 923/697,实际 975/733);且**从未被任何自动化调用**(无 CI、build.ps1 不跑测试) | 已修:`build.ps1` 硬门禁 + pre-push hook;测试数改 `tools/count-tests.mjs` 自动统计 |
| 6 | 「新能力默认隔离」 | 角色卡脚本桥(`scripts/bridge.rs` 写变量/导入)**不经授权裁决**却默认自动执行 | 登记为 L12 待办(见 known-limitations),补齐路径已写明 |
| 7 | 六模式「由 ModeExecutor 统一驱动」 | 仅**派发点**统一;`multi.rs` 逐行复制 `solo.rs`;步进循环三份且取消口径不一致 | 已修:multi 委托 solo;`StateMachine` 静默吞错改为告警+迁移表修正;步进循环收敛见 §4 待办 |

**核查结论**:架构叙事本身方向正确(分层、隔离、晋升),但此前**停留在文档层**——代码里存在环依赖、重复实现与死抽象。本章第 1-5、7 项的处置使分层首次成为**结构 + 机器可验证护栏**;第 6 项与步进循环收敛仍为待办,已在对应文档登记。

---

## 8. 偏离纪律的结构性遗留(待办,知情接受)

| 遗留 | 现状 | 为何暂不动 |
|---|---|---|
| 步进循环三份 | legacy/plan/custom 各写一遍「置 running→写库→失败继续」;取消兜底三种口径(plan 扫 Pending、team 扫 Running、custom 直接 return) | 抽 `step_runner` 会同时改动三个已稳定运行的模式,收益(一致性)低于回归风险;待下次实跑暴露问题时再做 |
| `models/types.rs` 混装传输类型 | `LlmMessage`/`LlmStreamChunk`/`SseEvent`/`GenerationParams`/`ToolContext` 与数据契约同处一文件 | 拆分不改线格式但会大范围改动 import,收益仅为文件整洁;已用 check-arch 规则 D 的豁免项显式标注 |
| 契约不变量仅 mutex | range/require_if 恒判通过(见 known-limitations L2) | 依赖未实现的谓词引擎,属独立设计,不宜顺手发明语法 |
