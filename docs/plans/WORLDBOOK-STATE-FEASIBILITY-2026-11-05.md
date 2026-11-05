# 世界书变化记忆态系统 · 可行性与实施方案

> 日期：2026-11-05  
> 状态：**待审阅，未开工**（本文件是可行性报告，不是计划台账；用户拍板后再把批次写入 `docs/计划/`）  
> 范围：角色扮演模式 / 独立世界书优先 / 会话级覆盖层  
> 性质：可行性判断 + 目标架构 + 分批实施草案 + 待拍板决策  
> 取证方式：全仓代码勘察 + 文档台账核对（未运行真实模型、未改生产代码）  
> 现场：分支 `fix/attfix-1-backend-attachments`，工作区有并行会话的文档拆分暂存；本报告未提交、未改动任何已暂存文件。

---

## 〇、结论摘要（先看这一节）

**技术可行，但不能按“给世界书条目加个版本号/时间戳”来做。** 正确的形态是三层：

1. **基础层（Base）**：现有 `world_books.data_raw` 与角色卡内嵌 `character_book` 保持只读、跨会话共享；
2. **会话覆盖层（Session Overlay）**：按会话保存“条目当前状态”的增量覆盖，不写回基础书；
3. **变更日志（Change Log）**：每次变化记录 `before/after + 来源 + run_id + message_id + seq`，撤销靠逆操作，不靠重算。

三条硬结论：

- **世界书现在是角色/全局资产，不是会话资产。** 直接让条目“随消息变化”会串会话——同一角色的所有会话共享同一本书。必须新建会话维度覆盖层，不能改 `data_raw`。
- **“可撤销”不能靠重新计算。** 世界书命中概率用纳秒时间做随机源（`prompt_kit::world_entry_roll`），EJS/`@@var` 在渲染期有真实变量写副作用；同一历史两次装配可能得到不同结果。撤销只能靠“记录当时改了什么 + 逆操作”。
- **“与变量系统深度联动”不应把世界书状态塞进 `stat_data`。** 推荐“显式绑定 + 统一写入口”：条目声明读取哪个变量路径、变更时声明写哪些变量补丁；所有变量写入走 `VariableApplyService`，不直接改树。

建议分 6 批落地（`WBMEM-0` ~ `WBMEM-5`），其中 **WBMEM-0 是必须先做的前置修复**（一个已存在但未登记的数据丢失缺陷）。真正决定成败的是**语义裁定**（消息与记忆的归属、中间消息删除、跨会话记忆），不是代码工程量。

需要用户拍板的 7 个决策点见 §八。默认推荐：**开关默认关 + 单书显式开启；v1 只允许尾部连续区间撤销；变量联动先做只读投影 + 声明式补丁，不做整树镜像。**

---

## 一、把需求拆成可验收的命题

用户原话两条：

> 1. 深化随消息而变化的记忆系统，随消息可撤销可修改  
> 2. 实现记忆系统与变量系统的深度联动

“世界书变化记忆态”在本报告中定义为：

> **同一个聊天会话内，世界书条目的内容/启用/关键词等状态可以随消息推进而变化；每次变化有明确来源与消息归属；用户可以对某条消息引发的全部变化做撤销或修改；这些状态与 mvu 变量树之间可以按声明双向读写。**

拆成四个可验收命题：

| # | 命题 | 验收含义 |
|---|---|---|
| P1 | 会话内状态可变 | 同一角色两个会话，A 会话的条目变化不影响 B；基础书不被修改 |
| P2 | 变化可归属 | 任意一次条目变化能回答：哪个会话、哪次 run、哪条消息（或“未归属”）、什么来源 |
| P3 | 变化可撤销/可修改 | 撤销到某条消息后，后续注入内容与状态视图都回到该边界；编辑/删除消息不产生“状态与正文打架” |
| P4 | 与变量双向联动 | 变量变化能改变条目渲染/启用；条目变化能按声明写变量，且变量 changelog 记录来源；双向都可审计 |

明确**不在本方案**的范围：

- 任务模式（任务侧没有变量树、没有 EJS、没有记忆注入；本功能不得改变任务侧行为）；
- 世界书全文版本管理 / Git 式分支合并（只做会话级变化日志）；
- 把 `memory_entries`（跨会话长期记忆）改造成世界书状态（两者是不同数据，见 §4.8）；
- 新建插件机制 / 脚本沙箱 / 任意表达式 eval（复用现有 EJS 与声明式 JSON Patch）；
- 跨设备同步、多人协作。

---

## 二、现状事实（带证据）

### 2.1 世界书的数据模型与作用域

- 只有一张 `world_books` 表：`id / name / character_id / enabled / source / entry_count / data_raw / created_at`；**条目不是独立表**，全部在 `data_raw` JSON 里（`server-rs/src/models/db/schema.rs:66-76`）。
- `character_id` 为空 = 全局书；非空 = 绑定角色。**没有会话绑定列，也没有“会话 → 书”关联表**。一个会话通过角色间接获得“该角色绑定的所有书 + 所有全局书”（`server-rs/src/services/world_book_service.rs:260-279`），因此一个会话可以同时有多本书。
- 角色卡内嵌世界书在 `characters.data_raw.character_book`（V2 顶层 / V3 `data.character_book`），**不是 `world_books` 行**（`server-rs/src/parsing/world_book.rs:315-329`）。
- 条目字段：`id/uid、comment、keys、keys_secondary、regex、use_regex、content、constant、enabled、position、depth、scan_depth、order、case_sensitive、sticky、cooldown、probability、use_probability、role、decorators`（`server-rs/src/parsing/world_book.rs:57-118`；编辑视图 `server-rs/src/models/types.rs:98-160`）。
- **没有版本/更新时间/作者/消息号**：书级只有 `created_at`，条目级没有 `version / updated_at / message_id / seq`。
- `position` 在本项目不实现真实 atDepth 插入，只作排序键；`depth` 不是注入位置，只在条目未声明 `scan_depth` 时充当扫描窗口兜底；`sticky/cooldown` 只解析与回写，**不参与注入判定**（`docs/遗留.md` L1）。
- **条目没有跨书稳定标识**：`uid` 只在单本书内唯一；`collect_entries` 把多本书扁平成 `Vec<WorldEntry>`，两本书都可能有 `uid=0`。任何“按 uid 记账/撤销/合并”的设计必须先引入 `(book_id, uid)` 或来源标记（`docs/计划/02-批次记录.md:1600` WB-B 已指出）。

### 2.2 写路径、缓存与并发

- 所有 `world_books` 表 SQL 写集中在 `WorldBookService` 一个文件内（INSERT/UPDATE/DELETE 共 5 处）；**唯一失效汇聚点**是 `invalidate_entries_cache()`（`server-rs/src/services/world_book_service.rs:84-99`），它在递增全局代际、清空条目缓存后触发 `write_hook`。
- `set_write_hook` 是 `OnceLock<Arc<dyn Fn()>>`，生产接线只注入一次（`server-rs/src/api/app_state.rs:286-294`，回调 = 清契约缓存）。
- 独立书读改写走 `update_raw_with` 的 SQLite 事务（`world_book_service.rs:639-678`）：`BEGIN → SELECT → 闭包 → 序列化 → UPDATE + 重算 entry_count → COMMIT`，提交成功后才失效缓存。
- 并发修复 WBCONC：条目缓存按 `character_id`，写时先递增 `entries_gen` 再清空；填缓存前比对代际，变化则拒绝插入并重试；`save_data_raw` 检查 UPDATE 影响行数；`update()` 检查 0 行（`world_book_service.rs:28-46,288-340`）。
- “叠加保存 + 显式 removed + added 重分配”在 `save_entries_merged`：`removed` 明确列出的 uid 才删，未在视图且未 removed 的条目保留，命中条目做字段叠加；前端保存时算基线差集（`server-rs/src/parsing/world_book.rs:697-836`；前端 `web/src/utils/worldBookEntryDiff.ts`）。
- **内嵌书写入有意不触发写钩子、不失效条目缓存**（写的是 `characters.data_raw`，而条目缓存只覆盖独立书；`server-rs/src/api/world_books.rs:206-209`）。
- 同一条目上“用户字段编辑”与“模型 `worldbook_update` 追加 content”是**条目级 LWW**，没有版本号/三向合并（`docs/遗留.md` §二十七 WBCONC-L3）。

### 2.3 读取与注入链路

- **唯一读汇聚点**：`WorldBookService::collect_entries_for_character(character_id)`（`world_book_service.rs:288-310`）。调用方至少 12 处：引擎上下文（`agents/engine/messages/context.rs:160`）、`generate-raw`（`api/chat.rs:704`）、会话预览（`api/sessions.rs:615`）、提示词预览（`api/settings/prompt.rs:323`）、契约注册表（`api/app_state.rs:283`）、`read` 工具（`tools/agent_tools_read.rs:135`）、子 agent（`tools/agent_tools_agent.rs:758`）、任务 system（`services/task_service/prompt.rs:211`）、工具装配等。
- 引擎侧分组与渲染：`collect_world_text_grouped_with`（`server-rs/src/agents/engine/worldbook.rs:82-157`）：
  1. 扫描池 = 历史中 `user`/`assistant` 文本从旧到新，`system` 剔除；
  2. 逐条目：`enabled` 跳过 → `@@` 装饰器（`@@if/@@unless/@@var/@@set`）→ `constant` 直接进常驻组 → 否则 `triggered_entry_hits_scoped`（窗口按最近 N 条 **user** 消息计、匹配池取跨度内全部消息、最后过概率）；
  3. 命中条目经 EJS 渲染，空内容不注入。
- 提示词共享原语在 `server-rs/src/services/prompt_kit.rs`：`constant_world_body` / `triggered_entry_hits(_scoped)` / `world_entry_text` / `system_inject_text` / `render_character_placeholders` / `untrusted_boundary_for`（`:26-61,89-104,106-154,156-183,187-205,211-246,250-294`）。
- 注入位置：常驻进 `system`（`{{world_info}}` 占位符或内置模板追加），激发默认追加到最新 user 消息尾部；激发组统一说明头 + 整组 `untrusted_boundary("world_book")`；状态块不参与包裹（`server-rs/src/agents/engine/messages/build.rs:203-247,430-511`）。
- **任务链只取常驻段**，没有激发/扫描/概率语义（`docs/契约-协议与配置.md:673-674`；`task_service/prompt.rs:159-213`）。
- 契约缓存 `ContractRegistry` 按 `character_id` 惰性加载；来源优先角色卡 `extensions.nlkaleido`，兜底独立世界书的 `[nlkaleido_contract]` 条目；**内嵌书不是契约来源**（WBC-L1，`docs/遗留.md` §二十）。
- **概率不可复现**：`world_entry_roll` 用 `SystemTime::subsec_nanos() % 100`（`prompt_kit.rs:156-168`），同一历史两次装配可能不同。
- **渲染读路径有写副作用**：`@@var/@@set` 在装配期写变量（`parsing/assistant/ctx.rs:104-125,142-178`）；EJS `setvar/addvar/incvar/decvar` 在条目渲染时真实写树（`parsing/assistant/ejs/exec.rs:394-424`）。

### 2.4 变量系统的真实形态

- 本仓库的“变量系统”事实上一套 **mvu 变量树 `stat_data`** 加多层存储/治理，不是七套并列系统：
  - `session_assistant_vars`：会话级权威树（`schema.rs:257-261`）；
  - `session_vars`：会话级扁平字符串层（`schema.rs:250-256`），`{{getvar/setvar/addvar}}`；
  - `scope_variables`：7 作用域通用表（global/chat/character/preset/message/script/extension；`schema.rs:262-271`）；
  - `messages.extra.mvu`：每条 assistant 消息的整树快照（`agents/engine/mod.rs:770-775`）；
  - `kaleido_state` / `kaleido_changelog`：契约运行态 + 逐 op 流水（`schema.rs:351-369`）。
- 读取顺序：加载树 → `[InitVar]/[InitialVariables]` 初始化 → 世界书条目渲染（EJS/`@@` 可读可写）→ 记录初始树 → 生成状态块 → 消息构建期宏展开 → 生成 → 解析 `<UpdateVariable>` 并应用 → 两步生成状态栏 → 落库 + 写 message 作用域镜像（`agents/engine/messages/context.rs:150-202,366-407`；`agents/engine/mod.rs:489-525,651-823`）。
- 变量写入路径不止一条：正文 JSON Patch、两步生成工具、custom 每步补丁、`update_variables` 工具、HTTP `/api/variable/update`、`PUT/PATCH /api/variables`、`[InitVar]`、`@@var`、EJS、宏、角色卡脚本、Undo 恢复。**只有部分路径经过 `VariableApplyService` 的契约门控与 changelog**；`update_variables` 工具和 REST 直写不产 changelog，`@@var`/EJS 每轮渲染都写且无门控（`services/variable_apply.rs:85-232`；`tools/variables.rs:9-51`；`api/variables.rs:100-201`）。
- `VariableApplyService::apply` 已预留 `writer` 参数，但 `ChangelogSource` 只有 `Agent/Manual/ContractInit/Rollback/Import/Repair`（`contracts/changelog.rs:13-22`），且 `entries_from_applied` 硬编码 `ChangelogSource::Agent`（`variable_apply.rs:181-187`）。
- 记忆 `memory_entries` 是 **角色级、跨会话**（`schema.rs:376-388`；`source_session_id` 有、`source_message_id` 无；删角色不级联清记忆）。它与世界书是两套数据。

### 2.5 消息与撤销/回退

- `messages` 表：`id INTEGER PK AUTOINCREMENT / session_id / role / content / extra JSON / created_at`；**没有 seq / parent_id / branch_id / version**（`schema.rs:33-41`）。消息是线性列表。
- `extra` 里已挂：`ts`、token 用量、`image_refs`、`first_mes`、`mvu` 快照、`status_bar`、`status_slot.last`、`truncated`、`budget_exceeded`、`interrupted`、`swipes`/`swipe_id`、`edited`、`kind`。
- assistant 的“版本”只有 `extra.swipes = [{swipe_id, content, ts}]`，**没有每版本的变量/状态/记忆快照**；编辑 assistant 不更新 `swipes`（`agents/engine/run_finish.rs:47-82`；`session_service.rs:446-472`）。
- 消息写路径：发送/重发/重生成/编辑/删除/截断/swipe 各有端点（`api/routes/chat.rs`、`api/sessions.rs`）。**编辑/删除消息不会重算后续派生状态**；唯一显式重算的是重发/重生成时把变量树回滚到锚点前最近一条 assistant 的 `extra.mvu`（`web/src/stores/chat.ts:350-470`）。
- `undo_snapshots`：一次成功的写工具调用一条快照，工具名固定 `write/replace/create/update_variables/memory_write`；执行前 `snapshot_before` 取逆操作，成功后 `commit_staged` 落库，失败 `discard`；`seq` 持久排序，restore 应用“目标及其后续”的快照逆序；严格删除 + 失败保留 + 成功精确清理（`services/undo_service.rs:26-32,205-233,489-600,647-833`）。
- **快照没有消息归属**：`anchor_message_id` 只是快照时 `MAX(messages.id)`，不参与 restore 的批次界定（`schema.rs:425-445`；`undo_service.rs:190-200`）。`tool_calls` 只有 `agent_session_id`，没有 `message_id`。
- `memory_write` 已做精确绑定：`insert_with_status` 返回 `(entry, created)`，工具输出 `id/created`；`created=false`（去重命中）直接丢弃快照、零快照（`services/memory_service/service.rs:455-497`；`tools/memory.rs:106-113`；`undo_service.rs:515-539`）。
- 会话级变量删除已有事务模板 `try_delete_message`（message 作用域变量 + 消息同事务；`session_service.rs:582-602`），可扩展为“消息 + 状态增量”同事务。

### 2.6 文档既有裁决（必须对齐，不能推翻）

| 条目 | 位置 | 裁决 | 对本方案的含义 |
|---|---|---|---|
| L1 sticky/cooldown 无跨轮状态 | `docs/遗留.md` §一 L1 | 有意裁剪，接受降级 | 本方案就是“世界书跨轮状态”的完整形态；必须说明与 L1/WB-B 的关系 |
| WB-B 会话台账 | `docs/计划/02-批次记录.md:1593-1610` | 未启动；D3 载体未拍板、D6 刚需证据未出现 | 本方案的变更日志是 WB-B 的上位形态；可把激活台账作为其派生视图 |
| WBC-L1 内嵌书非契约来源 | `docs/遗留.md` §二十 | 维持现状，待产品裁决 | v1 先只做独立书；内嵌书后置 |
| WBCONC-L1 同轮契约双版本 | `docs/遗留.md` §二十七 | 知情接受 | 不要承诺“工具写后本轮注入立即可见” |
| WBCONC-L2 内嵌书 DB/磁盘双份 | 同上 | 潜在隐患 | 若纳入内嵌书，必须先定义“哪份是源” |
| WBCONC-L3 同 uid LWW | 同上 | 知情接受 | 本方案要提供“写前 before-image”，但不要假装已解决并发合并 |
| UNDOFIX2-L5 回退后变量树展示 | `docs/遗留.md` §二十四 | 产品口径待定 | 本方案不要声称“回退后展示层必然一致” |
| PAD-1/2/3 | `docs/遗留.md` §十五 | PAD-1 发送层清洗未做；PAD-2 只显示层；PAD-3 沙箱是宿主镜像声明式面 | 世界书状态不能假设正则/悬浮窗有完整 DOM/消息写入能力 |
| T1 资源界面外联 | `docs/遗留.md` §一 | 知情接受不修 | 不要重开 |
| L7 记忆向量检索无 ANN | `docs/遗留.md` §一 | 有意裁剪 | 不要把 `memory_entries` 与世界书状态混为一谈 |
| L26/PRODCAP-1 任务事件持久化 | `docs/遗留.md` §一 | 任务事件有 seq + 补拉 + 回放；聊天 SSE 没有 | 可借鉴范式，但不要声称聊天已有回放 |
| 双模式隔离 | `docs/契约-协议与配置.md:391-420,826-845` | 类型承载；任务侧零消费字段直写扁平 | 本功能只进角色扮演路径；任务侧必须逐字节不变 |
| 文档门禁 | `tools/check-docs.mjs` D1–D8 | 新增 `docs/plans/*.md` 不会被逐项检查 | 报告可放 plans；但不要放 docs 根/仓库根 |

---

## 三、可行性判定

### 3.1 可以直接复用的地基

| 地基 | 位置 | 复用方式 |
|---|---|---|
| 世界书读汇聚点 | `collect_entries_for_character` | 新增 `collect_entries_for_session(character_id, session_id)`，在基础条目上合并覆盖层；旧方法保留给任务/契约/预览 |
| 世界书写失效汇聚点 | `invalidate_entries_cache` | 覆盖层写不碰基础缓存；基础书写入照旧失效 |
| 条目编辑视图与差集 | `WorldBookEntryView` + `worldBookEntryDiff` | 覆盖层编辑复用“基线差集”思路，但落点是新表 |
| 消息随身派生状态 | `messages.extra.mvu` + `replayMvuVariables` | 本方案在 `extra.worldbook_state` 放**紧凑指针**（seq/revision/hash），完整数据在变更表 |
| 工具快照两段式 | `undo_snapshots` / `SnapshotStager` | 撤销状态变化沿用“执行前取逆操作、成功后提交、失败保留” |
| 事务模板 | `try_delete_message` | 扩展为“消息删除 + 覆盖层回滚 + 变更日志清理”同事务 |
| 变量统一写入口 | `VariableApplyService::apply(writer)` | 世界书副作用作为 `writer="worldbook"` 的补丁；补 `ChangelogSource` 变体 |
| 逐 op 变更日志 | `kaleido_changelog` / `contract_changelog` | 变更表结构可直接借鉴（seq、turn_id、before/after、source） |
| 事件流范式 | `task_events` / `task_trace` | 若状态时间线需要断线补拉，复用“持久化 + seq + 补拉”；v1 先 REST |

### 3.2 必须新建的东西

1. **条目稳定身份**：`entry_key`（`book:{book_id}:{uid}` / `card:{character_id}:{uid}` / `session:{session_id}:new:{uuid}`）。现状 `WorldEntry` 没有来源标记，`collect_entries` 会丢书 id。
2. **会话覆盖层存储**：`worldbook_state_entries`（每会话每 entry_key 一行，存覆盖字段 + revision + base 哈希）。
3. **变更日志**：`worldbook_state_changes`（append-only，`seq / message_id / run_id / call_id / entry_key / op / field / before / after / source / swipe_id`）。
4. **消息归属链**：在工具执行期拿 `run_id`，在 assistant 消息落库后把该 run 的变化绑定到 `message_id`；现有 `ToolContext` 只有 `session_id/character_id`，没有 run 身份。
5. **撤销/重放服务**：`revert_to_message` / `revert_after_seq`，以及消息删除/编辑/重发/重生成/swipe 的接线。
6. **变量绑定声明与执行器**：条目级的 `state_bindings` / `effects`，以及 `WorldbookStateService` 与 `VariableApplyService` 的桥。
7. **前端状态视图**：会话内的“记忆态时间线”、条目 diff、撤销按钮、可选“固化到书”。

### 3.3 五个硬风险（按严重度）

1. **跨会话串味（最高风险）**：世界书是角色/全局资产，条目缓存按 `character_id`。若把会话状态塞进 `collect_entries_for_character` 而不改缓存 key/代际，A 会话的变化会出现在 B 会话。必须“基础缓存不动 + 覆盖层按会话单独缓存”，且覆盖层缓存 key 含 revision。
2. **条目身份与写路径碎片化**：`uid` 跨书可冲突；写面至少五条（独立书事务、内嵌书事务、`worldbook_update` 工具、脚本导入、前端卡脚本 `lorebook-set`）。覆盖层必须用 `entry_key`，且明确 v1 只接管 `worldbook_update` 与用户覆盖层编辑；其余写路径写基础书，不参与会话状态。
3. **非确定性与副作用**：概率 roll 不可复现；`@@var`/EJS 在渲染期写变量。撤销不能“重跑一遍”，必须记录变更；变量副作用要么纳入变更日志（审计），要么明确“不归本系统管”。
4. **消息归属时序**：工具快照发生在 assistant 消息落库之前（引擎收尾才 `add_message`）。必须在 run 期记 `run_id`，收尾后绑定 `message_id`；中断/无正文的 run 要有 orphan 归属与 UI 提示。
5. **信任边界与双模式**：覆盖层内容仍来自模型/用户，必须继续走 `untrusted_boundary("world_book")`；不能因为“记忆态”就提升为 system 指令。任务模式没有变量/EJS/记忆，任何共享函数改动都必须在任务侧逐字节 no-op。

### 3.4 与 WB-B / L1 的关系

WB-B 是“sticky/cooldown 会话激活台账”，解决的是**读侧**“这条上一轮命中了，下一轮是否继续注入”。本方案解决的是**写侧**“这条的内容/启用被谁改成了什么，如何撤销”。两者共享同一套 `entry_key` 与会话维度，但表可以分开：

- `worldbook_activations`（WB-B 推荐的新表）：`(session_id, entry_key) → last_hit_message_seq`，派生读侧状态；
- `worldbook_state_changes`（本方案）：append-only 写侧变更。

建议把 WB-B 的 D3 载体裁定并入本方案的 WBMEM-2（一次把身份与台账基础设施建好），sticky/cooldown 的判定接线可在 WBMEM-3 或单独小批。这样既不重复造轮子，也不把两件事混成一个巨大批次。

---

## 四、目标架构

### 4.1 分层模型

```
┌─────────────────────────────────────────────────────────────┐
│ 基础层 Base（只读、跨会话共享）                              │
│ world_books.data_raw / characters.data_raw.character_book    │
│  条目身份：book:{book_id}:{uid} / card:{character_id}:{uid}  │
└─────────────────────────────────────────────────────────────┘
                              │
                              │ collect_entries_for_session()
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ 会话覆盖层 Session Overlay（每会话）                         │
│ worldbook_state_entries                                      │
│   (session_id, entry_key) → 覆盖字段 + revision + base_hash  │
│  - 只存“被改过的字段”，不复制整本书                          │
│  - 可新建会话内条目 session:{sid}:new:{uuid}                 │
└─────────────────────────────────────────────────────────────┘
                              │
                              │ 合并 + 绑定求值 + 渲染
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ 变更日志 Change Log（append-only）                           │
│ worldbook_state_changes                                      │
│   seq | message_id | run_id | call_id | entry_key | op |      │
│   field | before_json | after_json | source | swipe_id       │
│  - 撤销 = 逆序应用逆操作                                     │
│  - 重放 = 从基础 + 变更重建（可选 checkpoint）               │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ 消息锚点 / 变量联动                                          │
│ messages.extra.worldbook_state = { seq_to, revision, hash }  │
│ scope_variables('message', msg_id) 镜像（可选）              │
│ VariableApplyService(writer="worldbook") + changelog source  │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 条目身份

`entry_key` 规则（写入即冻结，不随书改名/角色迁移变化）：

| 来源 | entry_key | 说明 |
|---|---|---|
| 独立书 | `book:{book_id}:{uid}` | `uid` 用条目规范化后的 `id`；书删除时会话覆盖层级联清理（或保留为 orphan 并在 UI 提示） |
| 角色卡内嵌书 | `card:{character_id}:{uid}` | v1 先不接管；v2 再纳入（WBC-L1/L2 需先裁定） |
| 会话内新建 | `session:{session_id}:new:{uuid}` | 不属于任何基础书；可“固化”时再选择目标书 |

基础条目被用户从书中删除后，覆盖层条目变成 orphan：读取时跳过 + UI 显示“基础条目已删除”，不静默丢弃变更日志。

### 4.3 覆盖层字段

覆盖层只存**当前值**，不存完整条目副本。可覆盖字段与 `WorldBookEntryView` 对齐：

```
content, comment, keys, keys_secondary, regex, use_regex,
constant, enabled, position, depth, scan_depth, order,
case_sensitive, probability, use_probability, role,
decorators, state_bindings, effects
```

合并规则：`base` 为底，覆盖层字段逐个覆盖；数组字段整体替换（不做元素合并，避免语义歧义）；`state_bindings/effects` 不参与注入，只参与绑定求值。

### 4.4 变更日志

```sql
CREATE TABLE worldbook_state_changes (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id   TEXT NOT NULL,
  message_id   INTEGER,              -- 收尾绑定；未绑定为 NULL
  run_id       TEXT,
  call_id      TEXT,
  entry_key    TEXT NOT NULL,
  op           TEXT NOT NULL,        -- create/delete/set_field/append_content/replace_content/enable/disable
  field        TEXT NOT NULL DEFAULT '',
  before_json  TEXT,
  after_json   TEXT,
  source       TEXT NOT NULL,        -- user/model/script/import/variable/undo
  swipe_id     INTEGER,              -- 仅重生成/多版本消息
  created_at   TEXT NOT NULL
);
CREATE INDEX idx_wb_changes_session_seq ON worldbook_state_changes(session_id, seq);
CREATE INDEX idx_wb_changes_session_msg ON worldbook_state_changes(session_id, message_id);
```

关键纪律：

- **只增不改**：撤销也是追加一条 `op=revert`（或直接删除被撤销的变更行？推荐追加 + 标记 `reverted_by_seq`，保留审计；若担心膨胀，另设 `reverted` 列）。
- **幂等**：写变更时用 `run_id + call_id + entry_key + field + op` 做唯一键或幂等检查；工具重试不重复落账。
- **失败保留**：撤销中途失败时，已应用的逆操作不能丢；沿用 undo 的“严格删除 + 失败保留 + 成功精确清理”。
- **容量**：每会话默认上限（如 2000 条，参照 kaleido changelog），超限时先做 checkpoint（全量覆盖层快照）再裁旧；v1 可只告警不裁。

### 4.5 消息归属

时序：

```
run 开始（run_id）
  ├─ 工具调用 worldbook_update → 写覆盖层 + 变更日志（message_id=NULL, run_id=X）
  ├─ 其他状态写入（用户面板/脚本）→ 同上
  └─ 收尾：
       add_message(assistant) → message_id=A
       bind_run_to_message(run_id=X, message_id=A)   ← 更新该 run 的 NULL 行
       写 messages.extra.worldbook_state = {seq_to, revision, hash}
```

- **重生成**：锚点 message_id 已知，写入时直接带 `message_id=regenerate_id` + `swipe_id=新版本`；旧版本的变更集用 `state_set_id` 区分（见 §4.6）。
- **中断**：引擎落部分正文并带 `extra.interrupted`，有 message_id 则绑定；无正文则变更保持 `message_id=NULL`，UI 显示“未归属本轮”，允许手动撤销。
- **并发**：`add_message` 与 `bind_run_to_message` 不是一个事务。绑定失败不丢变更（仍可按 run_id 查），下次 `loadHistory` 做一次补偿绑定（按 `run_id` + 会话 + `turn_index` 匹配）。
- **不伪造**：旧数据没有 run 身份时，不按工具名/时间猜归属（沿用 UNDOFIX-L2 的纪律）。

### 4.6 撤销 / 编辑 / 删除 / 版本语义

**v1 只允许“尾部连续区间”撤销**，理由：中间消息的状态变化是后续消息状态变化的前置条件；单删中间消息后，后续状态无法在不重跑模型的前提下重算（工具副作用不可重放）。

| 操作 | v1 语义 |
|---|---|
| 撤销到某条消息 | 取该消息绑定后的 `seq` 边界，逆序应用其后的全部变更；成功后写 `messages.extra.worldbook_state`；失败保留并提示 |
| 删除尾部消息 | 先回滚该消息的变更，再删消息（同一会话锁内；借鉴 `try_delete_message` 事务） |
| 删除中间消息 | **拒绝**（409 + 说明“请先截断到该消息或重新生成”）；或提供显式“截断并回滚”按钮 |
| 编辑消息正文 | v1 不自动重算状态；UI 标注“本条记忆变更未随编辑重算”。需要重算时走“重新生成”（先回滚该消息状态集，再生成） |
| 重发 / 重生成 | 先回滚锚点及其后状态，再生成；新 run 的变更绑定到新 message_id/新 swipe |
| swipe 切换 | 每个 swipe 版本绑定一个 `state_set_id`；切换时应用目标版本的变更集（若未记录则提示“该版本无状态快照”） |
| 恢复（undo_snapshots） | 现有工具级回退不受影响；若回退涉及 `worldbook_update`，在快照 payload 增加反向变更（WBMEM-3） |

**为什么不能“重算”**：概率 roll 用纳秒、EJS/`@@` 渲染期写变量、模型输出不可复现。重算会得到不同条目集合，用户看到的“撤销”就不是撤销。

### 4.7 变量系统深度联动

分三层，按风险从低到高：

**L1 只读投影（推荐 v1 必做）**

- `WorldbookStateService` 向渲染上下文暴露只读视图：`getWorldbookEntry(entry_key, field)` / `getWorldbookState()`。
- EJS 内置函数或宏可读；不新增存储、不新增 scope。
- 条目内容仍可用现有 EJS `getvar` 读变量——**变量 → 条目渲染**已经是既有能力，不需要新机制。

**L2 声明式副作用（推荐 v1 做最小集）**

条目覆盖层增加：

```json
{
  "state_bindings": [
    { "field": "enabled", "when": { "var": "stat_data.flags.met_hero", "eq": true } }
  ],
  "effects": [
    { "on": "activate", "patches": [
      { "op": "replace", "path": "/flags/met_hero", "value": true }
    ] }
  ]
}
```

- `when` 只支持受限比较（`eq/ne/gt/gte/lt/lte/in`），**不引入任意表达式 eval**；解析在 Rust 侧，失败按“不限制”降级并 warn。
- `patches` 是标准 JSON Patch 数组，走 `VariableApplyService::apply(..., writer="worldbook")`。
- 契约门控沿用现有逻辑；`ChangelogSource` 增加 `Worldbook`（以及可选的 `Memory`），`entries_from_applied` 改为接收 source 参数。
- 任务模式：这些字段不解析、不消费。

**L3 双向镜像（可选，后置）**

- 变量路径 ↔ 条目字段的镜像声明，用于“状态卡”型条目（条目内容 = 变量子树格式化）。
- 不建议把整个 `stat_data` 镜像进条目，也不建议把条目状态写进 `stat_data` 的可注入区域；token 与信任边界都会失控。

**联动边界（必须写死）**：

- 变量 → 条目：只影响**下一次装配**，不保证同轮强一致（与 WBCONC-L1 同口径）。
- 条目 → 变量：只在声明了 `effects` 时写；写入失败不阻断条目变化，但要记 warn + 变更日志标 `effect_failed`。
- 渲染期 `@@var`/EJS 的既有变量写：v1 **不拦截、不改语义**，但可在变更日志里记录“渲染期写变量”审计项（source=render），避免用户误以为状态系统能解释一切变量变化。

### 4.8 与 `memory_entries` 的关系

- `memory_entries` 是角色级、跨会话的长期记忆；本方案是世界书条目的会话级状态。二者不合并。
- 可选的桥：一个“记忆投影书”（virtual book）把 `memory_entries` 投影为只读条目，随消息插入/删除；这属于后续独立批次，不在 v1。
- **不要**为了“记忆态”给 `memory_entries` 加 `source_message_id` 然后按消息删除——那会把跨会话记忆变成会话私有，破坏现有产品语义（L7 的 ANN 裁剪也基于角色维度）。

### 4.9 信任边界与双模式

- 覆盖层内容仍按世界书内容处理：激发组整组 `untrusted_boundary("world_book")`；常驻进 system 的部分沿用现有说明头 + 边界。
- 模型通过 `worldbook_update` 写入的内容来源标记 `source=model`，UI 显示“模型写入”；用户编辑 `source=user`。
- 任务模式：`collect_entries_for_character` 旧路径保持；任务 system 只取基础常驻段；`WorldbookStateService` 在任务侧不构造/不读取。
- 新设置字段：`worldbook_state_enabled` 是角色扮演专属、任务侧零消费，按契约应**直写扁平字段**，不进 `TaskPromptConfig`/`ModeSettings`（`docs/契约-协议与配置.md:826-845`）。

### 4.10 缓存与性能

- 基础条目缓存保持 `character_id` key，不动代际语义。
- 覆盖层缓存按 `(session_id, revision)`；`revision` 每次写入自增。合并时只遍历覆盖 map，不复制整本书。
- 读取路径：`collect_entries_for_session` = 基础条目（缓存克隆）+ 覆盖层（按 entry_key 应用字段）。复杂度 O(基础条目数 + 覆盖条目数)，与现状同量级。
- 前缀缓存：常驻/ system 段若被状态影响，会打穿 provider 前缀缓存。**建议状态化条目默认只作用于激发组**；若用户显式把条目设为常驻，UI 提示“每轮状态变化会导致前缀缓存失效”。变量状态块位置（`mvu_vars_position`）保持现状。
- 变更日志写入是单写连接上的小事务；不要在同事务里做文件 IO 或模型调用。

### 4.11 API 与前端

**REST（v1）**

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/api/chat/sessions/{id}/worldbook-state` | 当前覆盖层摘要 + revision + orphan 列表 |
| GET | `/api/chat/sessions/{id}/worldbook-state/changes?after_seq=&limit=` | 时间线（按消息/seq） |
| GET | `/api/chat/sessions/{id}/worldbook-state/entries/{entry_key}` | 单条目 base vs overlay diff |
| PATCH | `/api/chat/sessions/{id}/worldbook-state/entries/{entry_key}` | 用户编辑覆盖层 |
| POST | `/api/chat/sessions/{id}/worldbook-state/revert` | `{to_message_id}` 或 `{to_seq}` |
| POST | `/api/chat/sessions/{id}/worldbook-state/commit` | 可选“固化到基础书”（带 diff 预览/确认） |

**SSE**：v1 不新增 `SseEvent` 变体；状态变化通过消息 `finish` 后的 `loadHistory` + REST 拉取刷新。若后续需要实时时间线，再按“四处同步 + 契约计数”加事件（`TaskEventKind` 不是聊天事件，别混）。

**前端 UI**

- 世界书面板：每本书显示“状态模式”（关闭 / 会话覆盖），会话内条目行显示“已被本轮修改”标记。
- 聊天侧：消息气泡/时间线显示“本轮记忆变更 N 项”，展开看 diff；提供“撤销到此处”。
- 条目编辑：覆盖层字段编辑 + “恢复为基础值”；不直接写基础书。
- 空态/降级：状态服务读失败 → 按基础书渲染 + 顶部提示，不阻塞聊天。

### 4.12 schema / 迁移 / 导入导出

- 全部新表走 `ensure_*` 升级事务；`schema.rs` 的 `CREATE_TABLES` 与 `migration/ddl.rs` 的补表文本必须 `normalize_sql` 后逐字节一致（`docs/契约-架构与数据.md:30-33`）。
- 表数变化会触发 `check-doc-claims` 的数据库表计数；同批更新 `MAINTENANCE.md` / 契约相关计数。
- 聊天导入会**重排消息 id**（`docs/契约.md:157`）；覆盖层与变更日志若按 message_id 关联，导入时必须：
  - 方案 A（推荐 v1）：导入**不携带**会话覆盖层/变更日志，导入后状态归零（与 D1 的 mvu 清理同策略）；
  - 方案 B：导入时按 entry_key 重绑，message_id 重新映射；复杂且易错，后置。
- DB 合并（跨库）会重映射主键；新表若含 `session_id` 逻辑引用，需登记 `LOGICAL_REFERENCES`（参照 UNDOFIX2-L2 的修法）。
- 删除会话：FK 级联清理覆盖层与变更日志；删除角色：会话级联后孤儿书可能保留，需显式清理或标记 orphan。
- 删除书：覆盖层条目变 orphan；不要静默删变更日志（审计），在 UI 提示并可“清理 orphan”。

---

## 五、分批实施计划（WBMEM-0 ~ WBMEM-5）

> 批次 ID 使用 `WBMEM-`（2026-11-05 全库 grep 无占用；不复用 `WB-*` / `WBC-*`）。  
> 每批都要求：失败测试先行、真实模型实测（模型链路）、门禁全绿、台账回填；纯内部批次可按真实模型豁免条款执行。

### WBMEM-0 前置修复：装饰器往返 + 条目来源身份（S）

**目标**：在动状态系统之前，先修掉两个会静默破坏数据的地基问题。

**改动点**：

1. **`@@` 装饰器往返丢失（真实缺陷）**：`entry_to_view` 不返回 `decorators`，`view_to_value` 也不重放 `@@` 行（`parsing/world_book.rs:611-683`）。由于 `normalize_entry` 解析时已把 `@@` 行从 `content` 剥离，前端 GET 到的是清洗后的 content；整组 PUT 保存会把条目开头的 `@@if/@@unless/@@var/@@set` 行从 `data_raw` 抹掉。服务层只给 `worldbook_update` 工具规避了这一点（基于原始 content 追加），PUT/UI 路径无保护、无测试。
   - 修法推荐：`WorldBookEntryView` 增 `decorators`（`skip_serializing_if`），前端类型同步；`entry_to_view` 带出，`view_to_value` 重新前置；编辑 UI 显示只读/可编辑的装饰器区。
   - 备选：`entry_to_view` 直接把装饰器拼回 content，`view_to_value` 再解析。改动小但会让 UI 正文多出 `@@` 行，需产品确认。
2. **条目来源身份**：新增 `SourcedWorldEntry { entry_key, source, entry: WorldEntry }` 或等价的 `collect_sourced_entries_for_character`；不改旧 `collect_entries_for_character` 的返回类型，避免 12 处调用方一起改。
3. **测试**：装饰器 roundtrip（GET→PUT→GET）；两本书同 uid 不冲突；现有世界书测试全绿。

**验收断言**：`@@` 行在 UI 保存后仍存在；`entry_key` 在独立书/内嵌书/会话新建三种来源下稳定；状态开关关闭时逐字节无行为变化。  
**风险/回滚**：`WorldBookEntryView` 是 `check-contract` 第 26 组比对对象，双侧必须同步；回滚 = 移除字段与 UI。  
**依赖**：无，先做。

### WBMEM-1 会话覆盖层与读路径（M）

**目标**：状态开关打开时，条目可在会话内被覆盖；关闭时行为与现在完全一致。

**改动点**：

- 新表 `worldbook_state_entries` + `ensure_*` 迁移；`world_books` 增 `state_mode`（`off/session`，默认 `off`）或先只用全局开关（见 D5）。
- `WorldbookStateService`：`load_overlay` / `patch_entry` / `reset_entry` / `merge_entries` / `snapshot`。
- `collect_entries_for_session(character_id, session_id)`：基础条目 + 覆盖层合并；覆盖层缓存按 `(session_id, revision)`。
- 引擎 `context.rs:150-165` 改调新方法；任务、契约、预览、`read` 工具仍用旧方法。
- 设置 `worldbook_state_enabled`（默认 `false`，扁平字段，任务侧零消费）。

**验收断言**：

- 开关关：全部现有测试不改、注入字节不变；
- 开关开 + 覆盖层空：条目与基础完全一致；
- 覆盖 `content/enabled/keys` 后：下一轮注入变化；A 会话变化不影响 B 会话；基础 `data_raw` 不变；
- 并发：两个 run 同时写覆盖层不丢（单写连接 + revision 校验）；
- 缓存：基础书写入后基础缓存失效，但覆盖层不串味。

**风险/回滚**：签名调用面；覆盖层缓存 key 漏 session 就会串味——这是本批的第一验收项。回滚 = 开关关闭 + 移除新表读取。  
**依赖**：WBMEM-0。

### WBMEM-2 变更日志与写入归属（L）

**目标**：所有“会话内状态变化”都有来源与消息归属；模型可写，但写的是覆盖层而不是基础书。

**改动点**：

- 新表 `worldbook_state_changes`；`WorldbookStateService::record_change` 与幂等键。
- `ToolContext` 增 `run_id: Option<String>`、`call_id: Option<String>`（或新增 `ToolCallScope`），更新全部构造点；工具执行时把身份透传到 `worldbook_update`。
- `worldbook_update`：当会话 `state_mode=session` 时改为写覆盖层 + 变更日志；否则维持现状写基础书。蓝绿灯语义不变（constant 不改写、新建绿灯）。
- 收尾 `bind_run_to_message(run_id, message_id)`；`messages.extra.worldbook_state = {seq_to, revision, hash}`；中断/无正文的 orphan 处理。
- 前端：消息级只读时间线（“本轮记忆变更 N 项” + 展开 diff）。
- 可选：WB-B 的 `worldbook_activations` 同批建表（读侧台账）。

**验收断言**：

- `worldbook_update` 在状态模式下不修改基础 `data_raw`；变更日志有 `source=model`、`run_id`、`message_id`；
- 绑定失败时可按 run_id 查到，不丢变更；旧数据不伪造归属；
- 两会话隔离；任务模式 `worldbook_update` 行为不变（任务工具策略仍剔除/或写基础书，按现状）；
- `created=false` 类去重语义（若复用 memory 思路）不产生重复账。

**风险/回滚**：消息归属时序；`ToolContext` 构造点较多（约 10 处），漏改编译会红，不算隐蔽。回滚 = 工具分支回到基础书写入 + 关开关。  
**依赖**：WBMEM-1；D6（WB-B 是否同批）。

### WBMEM-3 撤销 / 编辑 / 删除 / swipe（L）

**目标**：P3 可验收——撤销到某条消息后，后续注入与状态视图都回到边界；中间消息删除有明确拒绝语义。

**改动点**：

- `WorldbookStateService::revert_to_message` / `revert_after_seq`：逆序应用逆操作；严格失败保留；成功后精确清理/标记。
- 接线：`delete_message`、`truncate_messages_after`、`resend`、`regenerate`、`swipe`。
- 会话锁：复用 `undo.session_lock` 或新增 `worldbook_session_locks`；生成中禁止编辑/删除状态（409）。
- `swipes` 每版本增 `state_set_id`；切换 swipe 应用对应状态集；无状态集的旧版本提示降级。
- 前端：撤销按钮、diff 视图、中间删除 409 文案。
- 可选：checkpoint 表（全量覆盖层快照），超容量时使用。

**验收断言**：

- 尾部撤销：注入内容、覆盖层视图、变更日志 seq 边界三者一致；
- 中间删除：拒绝并给出可操作文案；截断删除：先回滚再删；
- 重发/重生成：旧状态集回滚、新 run 绑定新 message_id/swipe；
- 失败注入（模拟 DB 错）：逆操作不丢、快照保留、UI 显示可重试；
- 并发：生成与撤销同时发生，只有一个成功（会话锁）；重复撤销幂等。

**风险/回滚**：语义最复杂的一批；务必先写失败的集成测试再实现。回滚 = 关闭撤销入口，保留只读时间线。  
**依赖**：WBMEM-2。

### WBMEM-4 变量深度联动（L）

**目标**：P4 可验收——变量变化能影响条目；条目变化能按声明写变量，且变量 changelog 有来源。

**改动点**：

- 条目覆盖层增 `state_bindings` / `effects` 解析与校验（受限比较，无 eval）。
- `WorldbookStateService` 在合并/渲染前求值 `when`；在变更后执行 `effects.patches`。
- `VariableApplyService::apply` 增 `source: ChangelogSource` 参数（或新方法），`entries_from_applied` 不再硬编码 `Agent`；`ChangelogSource` 增 `Worldbook`（必要时 `Memory`）。
- 只读投影：EJS/宏可读覆盖层字段（不新增 scope；若后续需要持久 scope，另批）。
- 任务模式：`state_bindings/effects` 不解析、不消费；新增字段不进 `ModeSettings`。
- 测试：绑定命中/不命中/降级；effect 补丁被契约拒绝；变量 changelog source；双模式隔离。

**验收断言**：

- 变量 `stat_data.flags.met_hero=true` → 条目 `enabled` 生效/失效（按声明）；撤销变量消息后条目视图恢复；
- 条目变更触发 effect → 变量树出现预期补丁，`kaleido_changelog` 的 source 为 `worldbook`；
- effect 补丁被契约拒绝时，条目变更仍成功，效果失败有可见提示；
- 任务模式注入逐字节不变；`check-settings-parity` 绿。

**风险/回滚**：信任边界（变量内容进入条目后仍须包裹）、双写顺序、契约拒绝。回滚 = 移除 bindings/effects 解析，只保留只读投影。  
**依赖**：WBMEM-3；D4。

### WBMEM-5 UI 收口与真实实测（M）

**目标**：用户可用、可审计、可回滚；真实模型证据齐备。

**改动点**：

- 世界书面板状态视图（每会话/每书/每条目 diff）；聊天侧时间线；撤销/恢复基础值/固化到书。
- 空态、失败态、orphan 提示；移动端窄屏布局（遵循 ANDUI 的移动约束）。
- 导入/导出策略（v1 不导出会话状态；导入清空，文档写明）。
- 真实模型实测 + 门禁 + 台账。

**验收断言**：UI 用例；真实模型四场景（见 §六）；全量 `npm run check`、`cargo test -j 8`、`npm test -w web`、`npm run build -w web`；证据落盘。

**风险/回滚**：UI 面较大，但可分批隐藏（功能开关关闭即不可见）。  
**依赖**：WBMEM-1~4。

### 工作量级（单人、不含真实模型反复调试）

| 批次 | 量级 | 主要不确定性 |
|---|---|---|
| WBMEM-0 | S（0.5~1 天） | `check-contract` 字段同步 |
| WBMEM-1 | M（3~5 天） | 缓存维度、调用面 |
| WBMEM-2 | L（1.5~2.5 周） | 消息归属、工具身份 |
| WBMEM-3 | L（1.5~2.5 周） | 撤销语义、并发 |
| WBMEM-4 | L（1.5~2.5 周） | 变量边界、契约 |
| WBMEM-5 | M（约 1 周） | UI/实测 |

总计约 **6~9 周**（单人）。若只做 P1+P2（覆盖层 + 变更日志，不做撤销/变量），约 2~3 周可出可演示版本。

---

## 六、验收策略（真实模型是唯一基准）

### 6.1 确定性测试（回归网）

- 覆盖层：空覆盖 = 原样；字段覆盖；跨会话隔离；缓存代际；并发写。
- 身份：两本书同 uid；内嵌书/独立书/会话新建；orphan。
- 变更日志：seq 单调、幂等、失败保留、revert 逆序。
- 消息：绑定成功/失败补偿；中断 orphan；swipe 版本；中间删除拒绝。
- 变量：bindings 求值；effects 补丁；契约拒绝；changelog source；任务模式零变化。
- 门禁：`check-contract`（新 DTO）、`check-settings-parity`（新字段）、`check-doc-claims`（表数）、`check-file-size`（新模块行数）。

### 6.2 真实模型实测（隔离实例）

按 `kedai-verify-real-model` 纪律：`DATA_DIR` 隔离、`PORT` 独立、启动后先核对日志 `data_dir`；provider 用 commandcode 网关 + `deepseek/deepseek-v4.1-flash`（或本机 CodeBuddy 反代，按当批可用性）。

四个场景：

1. **状态变化 + 撤销**：一个带世界书的角色，模型在归档步 `worldbook_update` 追加“城门已烧毁”；下一轮注入包含更新；撤销到上一轮后注入恢复；基础书未变。
2. **变量绑定**：条目 `enabled` 绑定 `stat_data.flags.met_hero`；触发变量更新后条目启停变化；撤销变量消息后恢复。
3. **跨会话隔离**：同角色两个会话，A 改 B 不变；基础书不变。
4. **消息编辑/删除/重生成**：尾部删除回滚；中间删除 409；重生成替换状态集；swipe 切换状态一致。

证据：命令 + JSON/日志 + 页面截图落 `D:\kedai-bench-run\wbmem-evidence\`；提交正文写“真实模型实测通过 + 证据路径 + 关键结论”。

### 6.3 不可测时的登记

若 provider 额度/网络不可用，按「确实不可测」在提交正文登记，并补确定性用例 + 真实页面（非 mock 模型链路）证据；不要用 mock 冒充验收。

---

## 七、风险登记与缓解

| # | 风险 | 级别 | 缓解 |
|---|---|---|---|
| R1 | 跨会话串味 | 高 | 基础缓存 + 覆盖层缓存分离；覆盖层 key 含 session + revision；首条验收即双会话隔离 |
| R2 | 条目身份冲突 | 高 | `entry_key` 含来源；v1 只接管独立书；两本书同 uid 测试 |
| R3 | 撤销靠重算导致不一致 | 高 | 只记录变更 + 逆操作；不重跑概率；UI 明确“重算需重新生成” |
| R4 | 消息归属丢失 | 中高 | run_id 先记、收尾绑定、补偿绑定、orphan UI；不伪造旧数据 |
| R5 | 中间消息删除导致后续状态断裂 | 中高 | v1 尾部连续区间；中间删除 409；截断按钮 |
| R6 | 前缀缓存被打穿 | 中 | 状态化条目默认只作用激发组；常驻变更给提示 |
| R7 | 信任边界被突破 | 中高 | 覆盖层仍走 `untrusted_boundary`；变量副作用经 `VariableApplyService`；不做任意 eval |
| R8 | 任务模式被污染 | 中 | 新方法只在角色扮演路径调用；任务侧旧方法 + 零消费字段；双模式测试 |
| R9 | 变更日志膨胀 | 低中 | 每会话上限 + checkpoint + 归档策略 |
| R10 | 导入/合并 id 重排 | 中 | v1 导入清空状态；合并登记逻辑引用；文档写明 |
| R11 | UI 把覆盖层当基础书编辑 | 中 | 面板明确“会话状态”vs“基础书”；恢复基础值按钮；写基础书需显式确认 |
| R12 | 现有装饰器往返缺陷被状态系统放大 | 高（前置） | WBMEM-0 先修 + roundtrip 测试 |

---

## 八、需要用户拍板的决策点

| # | 决策 | 推荐 | 备选 | 影响 |
|---|---|---|---|---|
| D1 | 状态落地形态 | **会话覆盖层，基础书只读；另提供显式“固化到书”** | 直接写基础书（简单但串会话） | 全局架构 |
| D2 | 变更记录形态 | **增量日志 + 每消息紧凑指针；checkpoint 可后置** | 每消息全量条目快照（简单但体积大、diff 难） | 存储/撤销 |
| D3 | 中间消息删除/编辑 | **v1 只允许尾部连续区间；中间删除 409；编辑不自动重算** | 支持任意位置回滚/重放（需重跑工具副作用，不可靠） | 产品语义 |
| D4 | 变量联动深度 | **只读投影 + 声明式 `when/effects`（受限比较 + JSON Patch），不做 stat_data 整树镜像** | 世界书状态全部塞进 stat_data（复用变量日志但 token/信任边界失控） | 变量架构 |
| D5 | 开关与范围 | **全局开关默认关 + 单书 `state_mode` 显式开启（off/session）** | 默认全开（改变存量行为） | 兼容性 |
| D6 | 与 WB-B 关系 | **共享 `entry_key` 与台账基础设施；WB-B 的 sticky/cooldown 作为读侧派生台账，可在 WBMEM-2/3 一并做** | 完全独立两套表（重复身份逻辑） | 排期 |
| D7 | 内嵌书是否纳入 | **v1 只做独立书；内嵌书 v2（先裁定 WBC-L1/L2/L3）** | v1 就纳入（需先定义 DB/磁盘哪份是源） | 范围 |

另外两个**产品文案/默认值**问题，可随批确认：

- 状态化条目默认只作用于激发组，还是允许常驻？（推荐前者，保护前缀缓存）
- 撤销入口放在消息气泡还是独立“记忆态”面板？（推荐先放独立面板，稳定后再加消息级快捷入口）

---

## 九、必须避开的既有裁决（写入计划时的红线）

1. 不要把本方案写成“WB-B 的替代”——应写“共享身份/台账基础设施，WB-B 是读侧派生视图”。
2. 不要声称“同轮强一致”（WBCONC-L1 知情接受）。
3. 不要把内嵌书当契约来源（WBC-L1 待裁决）。
4. 不要承诺“回退后变量树展示必然一致”（UNDOFIX2-L5 待产品口径）。
5. 不要把 PAD-2 当成 PAD-1 已解决；状态系统不碰发送层正则。
6. 不要用楼层桥（`window.Kedai`）做世界书/消息读写——它只给楼层摘要与 `enabled` 写入。
7. 不要新建插件机制；这是核心角色扮演功能 + 设置开关，不放进 `data/plugins/`。
8. 不要让任务模式看到变量/EJS/记忆/覆盖层；共享函数改动必须在任务侧逐字节 no-op。
9. 不要声称旧快照可精确回退（UNDOFIX-L2）。
10. 不要声称回退锁覆盖全部写路径（UNDOFIX-L1）。
11. 不要照抄测试数/SSE 变体数；引用 `MAINTENANCE.md` / `check-doc-claims`。
12. 不要重开 T1 / FE-5 B；不要动 `docs/archive/`。

---

## 十、最终判断

**可行，但应按“会话状态层 + 变更日志 + 显式绑定”做，而不是“世界书版本化”。**

最自然的接缝已经存在：

- 读：`collect_entries_for_character` 是唯一读汇聚点，新增会话版包装即可让引擎/工具/预览同源；
- 写：`invalidate_entries_cache` 是唯一失效汇聚点，基础书写入不用改；
- 撤销：`undo_snapshots` 的两段式/seq/严格恢复是成熟模板；
- 消息携带：`messages.extra.mvu` 证明了“每消息快照 + 前端回放”可行；
- 变量：`VariableApplyService` 已预留 `writer`，补 source 即可把世界书变成一等写者。

最大风险不是代码，而是**“这条消息拥有这份记忆”与“记忆按角色跨会话共享”之间的语义冲突**，以及**中间消息删除后后续状态无法可靠重算**。这两点必须先由用户裁定（D1/D3/D7），否则实现会掉进“回退删错全局状态 / 回退不完整 / 跨会话串味”的坑。

建议启动顺序：**WBMEM-0 → WBMEM-1 → 真实模型验证 P1/P2 → 再决定是否继续 WBMEM-2~5**。如果只想要“可演示的世界书会话状态”，WBMEM-0 + WBMEM-1 + 最小 UI 就够；如果要做“可撤销 + 变量联动”，必须走完整六批。

---

## 附录 A：证据索引（本次勘察核对的关键位置）

| 主题 | 位置 |
|---|---|
| 世界书建表 / 字段 | `server-rs/src/models/db/schema.rs:66-76`；`server-rs/src/parsing/world_book.rs:57-118` |
| 会话/消息表 | `server-rs/src/models/db/schema.rs:23-41` |
| 变量表 / 作用域 | `server-rs/src/models/db/schema.rs:250-271` |
| undo 快照表 | `server-rs/src/models/db/schema.rs:425-445` |
| memory_entries | `server-rs/src/models/db/schema.rs:376-388` |
| 世界书写失效 / 缓存 | `server-rs/src/services/world_book_service.rs:28-46,78-99,288-340` |
| 世界书写事务 | `server-rs/src/services/world_book_service.rs:639-678` |
| 条目视图 / 写回 | `server-rs/src/parsing/world_book.rs:611-683` |
| 引擎世界书收集 | `server-rs/src/agents/engine/worldbook.rs:82-157` |
| 引擎上下文 / 变量初始化 | `server-rs/src/agents/engine/messages/context.rs:150-202,366-407` |
| 引擎收尾落库 / mvu 快照 | `server-rs/src/agents/engine/mod.rs:757-823,844-889` |
| run_id | `server-rs/src/agents/engine/mod.rs:441` |
| ToolContext | `server-rs/src/models/types.rs:965-979` |
| worldbook_update 工具 | `server-rs/src/tools/worldbook.rs:110-405` |
| 工具快照两段式 | `server-rs/src/tools/registry.rs:322-440`；`server-rs/src/services/undo_service.rs:26-32,205-233,489-600,647-833` |
| 变量统一写入口 | `server-rs/src/services/variable_apply.rs:85-232` |
| ChangelogSource | `server-rs/src/contracts/changelog.rs:13-44` |
| 装饰器 / EJS 写变量 | `server-rs/src/parsing/assistant/ctx.rs:104-178`；`server-rs/src/parsing/assistant/ejs/exec.rs:394-424` |
| 概率 roll | `server-rs/src/services/prompt_kit.rs:156-168` |
| 任务侧世界书 | `server-rs/src/services/task_service/prompt.rs:159-213` |
| WB-B 计划 | `docs/计划/02-批次记录.md:1593-1610` |
| 既有裁决 | `docs/遗留.md` L1 / WBC-L1 / WBCONC-L1~L3 / UNDOFIX2-L5 / PAD-1~3 / T1 |
| 双模式设置 | `docs/契约-协议与配置.md:391-420,826-845` |
| 文档门禁 | `tools/check-docs.mjs` D1–D8；`tools/check-doc-claims.mjs` 12 项 |

## 附录 B：术语

- **Base（基础书）**：`world_books.data_raw` 或角色卡内嵌书，跨会话共享。
- **Overlay（覆盖层）**：会话内对条目的字段覆盖，按 `entry_key` 索引。
- **Change（变更）**：一次可审计的字段变化，含 before/after、来源、run/message 归属。
- **entry_key**：跨书稳定的条目身份，含来源类型 + 书/角色 id + uid。
- **state_set**：一条消息（或一个 swipe 版本）产生的变更集合，用于版本切换与撤销。
- **固化（commit）**：把会话覆盖层显式写回基础书的动作，默认不自动发生。
