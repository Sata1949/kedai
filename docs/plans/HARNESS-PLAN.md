# 编码 Harness 适配 · 执行稿（2026-09-26）

> 临时执行稿：不进提交，收口时删除。口径已定项，执行时不得擅自变更（要改先改本节并说明理由）。
> 行号取自当前工作区；文档类文件正在被 `DOC-DRIFT-PLAN` 的提交 3 修改，**实施前一律先 grep 复核**（`docs/经验.md` 条目 2）。

## 0. 评估口径

「Kedai 当 coding harness」的验收场景已存在，即 `D:\coding-bench` 的 50 道题（kedai 28 + zephyr 22）：

- 出题侧只给一份 `BENCH_TASK.md`，agent 在沙箱工作区里读代码、改文件、必要时跑测试，判分由判分器注入的隐藏测试做（`D:\coding-bench\README.md` §一、`runner/agent-driver.md`）。
- 对 agent 的要求只有两条：**能读题面 + 能在工作区里改文件**；`agent-driver.md` 给的形态是 `zcode --print $task --cwd $sandbox` 这类单次调用。

Kedai 当前的驱动通道只有一条：**HTTP API + SSE**，无单次 CLI（`server-rs/src/bin/` 只有 `kedai-data-merge.rs`）。仓库内既有范式是 `tools/bench/task-probe.mjs:15-21`：`GET /api/bootstrap` 取 token → 建任务 → 订阅 `/api/tasks/events`。

**证据性质**：本节全部结论来自静态阅读，未跑构建与测试；P0 四条已逐条打开原文复核（见各条「复核」标注）。

## 1. 缺陷清单

### P0 — 不修则不可用

| 编号 | 缺陷 | 证据 | 对 harness 的后果 |
|---|---|---|---|
| P0-1 | **文件工具触达不到工作区**：`read`/`write`/`replace`/`create` 全部收口在 `data/character_files/{character_id}/`，相对路径经 `safe_rel_path` 拒绝绝对路径/盘符/`..` | `tools/agent_tools_shared.rs:43-45`（根）、`:48-66`（校验）、`:69-77`/`:80-94`（读写）；已复核 | 沙箱里的源码**没有任何工具能读写**；且任务模式默认策略把 `write`/`replace`/`create`（Dangerous）整体剔除，只按工具名给 `bash` 开例外（`services/task_engine/tool_policy.rs:53-63`）→ **任务模式真正能触达工作区的工具只有 `bash` 一个**，「改代码」只能靠 `sed`/`echo >` 拼命令 |
| P0-2 | **没有工作区概念**：任务无 cwd/workspace 字段；`bash` 的 `cwd` 由模型逐次给定，缺省是 `DATA_DIR`；执行前只校验「路径存在且是目录」 | `models/types.rs:1196-1236`（TaskRecord 无该字段）、`tools/bash.rs:96-101`（缺省 data_dir）、`services/exec/desktop.rs:40-47`（仅 `is_dir`）；已复核 | 沙箱路径只能写进任务目标文本，模型每次调 `bash` 都要记得带 `cwd`；一旦漏带，命令在用户数据目录执行（污染真实数据）；多题并发时也没有 per-task 目录语义 |
| P0-3 | **开箱不可用 + 无单次入口**：`exec_enabled` 默认 false；无 CLI agent 入口 | `services/settings_service/params.rs:491`；已复核；`server-rs/src/bin/` 仅 `kedai-data-merge.rs` | 必须先在设置里手工打开命令执行；bench 集成只能走 HTTP + 轮询，无法像 `--print --cwd` 那样一题一进程 |
| P0-4 | **无路径边界 + 自我提权通道**：命令与 cwd 都不看路径；`GET /api/bootstrap` 免鉴权把 bearer token 交给任意本机进程，token 可改全部安全设置且立即生效 | `tools/permissions.rs:244-245`（Exec 跳过路径矩阵）、`tools/action_class.rs:145-176`（zone 恒 Opaque）、`api/static_files.rs:11-20`（bootstrap 回 token，已复核）、`api/settings.rs:610-638`（PUT settings） | 模型有 shell 即可读进程可读的任意路径（含题库 `gold.patch`/`hidden/`、`settings.json`、`kedai.db`），也可一条 `curl` 把自己升到 bypass —— **评测结论不可信**；详见 §5 的诚实边界 |

### P1 — 决定评测能否跑完 / 结论是否可审计

| 编号 | 缺陷 | 证据 | 对 harness 的后果 |
|---|---|---|---|
| P1-5 | **无变更清单 / 无 diff / bash 不在回退范围**：undo 快照只覆盖 `write/replace/create/update_variables/memory_write` 且对象是角色文件区 | `services/undo_service.rs:24-34`；`changed_files`/`watcher` 全仓零命中 | 无法回答「这一轮改了哪些文件」；沙箱跑坏只能整题重来；父任务/汇总只能信子任务的自然语言自述 |
| P1-6 | **取消不中止正在执行的工具；超时只杀直接子进程**：工具执行不接 abort token，循环 `await` 工具 future 无 `select!`；`desktop.rs` 文件头声称进程组/`taskkill /T`，实现里没有 | `tools/registry.rs:252-297`、`agents/engine/executor.rs:969-1014`、`services/exec/desktop.rs:4` 对比 `:64-92` | 点停止后沙箱仍在被改（最长到命令自身超时）；`cmd /C` 拉起的孙进程逃逸（cargo/rustc 编译树可能留下占用） |
| P1-7 | **压缩与裁剪按角色扮演设计**：摘要提示词只要求保留人物关系/事件/伏笔；run 内超 4 轮的旧工具结果与参数被占位化；工具调用记录随 `agent_sessions` 在下次 run 开头级联删除 | `agents/engine/compaction.rs:88-93`、`agents/engine/messages/trim.rs:255-279`、`models/db/schema.rs:53-61`、`agents/engine/mod.rs:438-446` | 长编码任务里模型记不住「改过哪些文件、跑过什么命令」→ 重复读同一文件、重复改同一处；崩溃/中断后无可续跑的记录 |
| P1-8 | **自测闭环被两个上限截断**：`bash` 超时上限 300s；命令输出按**字符保头**截断 32768 | `services/exec/mod.rs:89`（`MAX_TIMEOUT_MS`）、`:98-105`（保头截断）；已复核 | bench 记录单题 cargo 校验 1~16 分钟（`D:\coding-bench\README.md` §六）→ **300s 上限跑不完一次 `cargo test`**；而测试失败摘要与 `test result: FAILED` 在输出**尾部**，保头截断恰好把判据切掉 |
| P1-9 | **任务模式无审批通道，策略前端只读**：任务侧 `no_ui_authorization=true`、名单外立即拒绝、高危命令直接拒绝；`task_tool_policy` 前端只装载不可编辑 | `services/task_engine/tool_policy.rs:30-38`、`agents/engine/executor.rs:1284-1292`、`web/src/stores/genSettings.ts:104-106`、`web/src/composables/useDataManager.ts:125-158` | 对无人值守是「可接受的设计」，但意味着 `rm` 类破坏性命令在任务里恒不可用（`cargo` 系命令落在 Sensitive 可用），且无法按任务收紧/放开 |

### P2 — 体验与健壮性

| 编号 | 缺陷 | 证据 |
|---|---|---|
| P2-10 | 子代理工具白名单无写文件、无 bash（`read/search/todo/sleep/calculator/memory_read`）→ multi/team 的子 agent 只能只读侦察 | `tools/tool_sets.rs:24-31` |
| P2-11 | 无 glob/grep/LSP/符号查找；`read` 只 `read_to_string`（无二进制、无图片）、无行号分页 | `tools/agent_tools_shared.rs:76`；`walkdir`/`glob`/`ignore` 均不在依赖里（`server-rs/Cargo.toml` 只有 `regex`） |
| P2-12 | `todo.include_subtasks` 参数被声明但实现忽略；任务模式 `todo.tool_calls` 恒不可用 | `tools/agent_tools_agent.rs:790` 对比 `:794`；CHG-4 已登记 |
| P2-13 | 前端：任务模式无工具级可视化（只有一行被截断的 `agent_status`）、无 diff、审批卡入参默认折叠；后端已下发的 `render_kind` 全前端零消费 | `web/src/components/AgentPanel.vue:432-547`、`web/src/components/TaskBoard.vue:939-1025`、`web/src/sseReducer.ts:25-26` |
| P2-14 | 任务状态机无集中转移校验（`set_status` 裸 UPDATE），`stop` 无守卫可改写终态 | `services/task_service/db.rs:172-193`、`services/task_service/executor.rs:374-385` |
| P2-15 | 子进程继承全部环境变量（无 `env_clear`）；`env`/`printenv` 是 Safe 命令；命令输出进 `exec_audit` 与工具结果 | 无 `env_clear`（搜索零命中）、`config.rs:135`（dotenvy）、`tools/command_risk.rs:184-185` |
| P2-16 | 子代理并发上限按**精确 session** 计数：team 4 个主各算一份 → 实际可同时 24 个子 agent，且每个主都带 bash | `tools/agent_tools_agent.rs:86-97`、`services/task_engine/team.rs:498` |

### 相关但已登记（不重复登记，引用即可）

`遗留.md`：T2（命令混淆）、L22（空转熔断，方向已部分落地）、CHG-3、CHG-4、OBS-1、L24、L26、ENV-3、TEST-ISO-1。
`展望.md`：RD-3（工具执行沙箱隔离，未做）、LH-1（Plan Mode，暂缓)、LD-2（并行工具执行）、REG-1（新增工具登记机的机检）、LH-3（会话分层持久化/recovery）。

**新登记需求（本稿发现，`遗留.md` 与 `展望.md` 均无）**：P0-1、P0-2、P0-3、P0-4、P1-5、P1-6、P1-7、P1-8、P2-10~P2-16。文档侧没有「Kedai 不做编码 agent」的明文承诺，故这批也**没有既有的「有意裁剪」判定可援引**——是否把编码能力纳入产品面属于定位决策，见 §5。

## 2. 提交 1 — 编码通道打通（workspace 绑定 + 工作区文件工具族 + 路径闸门）

目标：让 agent 能用**结构化工具**读写沙箱里的代码，且路径被 jail 在工作区内。这是「能跑 bench」的硬前提。

### 2.1 数据模型：任务级 workspace 绑定

| 动作 | 位置 | 要点 |
|---|---|---|
| `tasks` 表加列 `workspace TEXT NOT NULL DEFAULT ''` | `migration/ddl.rs` 新增 `ensure_tasks_workspace_column`，仿 `:189`（`ensure_tasks_executor_id_column`）的幂等写法 | 空串 = 未绑定，旧行零变化；**同步 `models/db/mod.rs` 的事务内 `ensure_*` 登记与计数** |
| `TaskRecord.workspace: Option<String>` | `models/types.rs` | `#[serde(default, skip_serializing_if = "Option::is_none")]`，旧客户端零变化 |
| `CreateTaskBody.workspace: Option<String>` | `api/tasks.rs` | 创建期校验：目录存在 + canonicalize 成功 + **不得包含 DATA_DIR、也不得是 DATA_DIR 的祖先**；失败 400 并给下一步文案。创建期把 canonical 绝对路径落库（冻结） |
| 透传到工具 | `services/task_engine/*`（solo/team/custom 的 `ToolContext` 构造点）、`services/task_service/executor.rs` | 新增 `ToolContext.workspace: Option<Arc<PathBuf>>`；ledger 里未绑定即 `None` |

**为何不做全局设置级 workspace**：bench 一题一沙箱，需要并行多题 → 必须任务级。

### 2.2 路径闸门（本批安全核心，先写失败测试）

新增 `server-rs/src/tools/workspace_guard.rs`：

```
safe_workspace_path(root: &Path, raw: &str) -> Result<PathBuf, String>
```

规则（每条一个负路径测试）：

1. 空 / 含 NUL → 拒绝。
2. 绝对路径允许，但 relative 一律基于 `root` 解析；两种情况最终都要落在 root 内。
3. **逐段 symlink 检查**：从 root 起对每一段做 `symlink_metadata`，任一段是符号链接即拒绝（防「写穿」）。Windows 上 **junction 不报 `is_symlink()`**，须用 `std::os::windows::fs::MetadataExt::file_attributes()` 判 `REPARSE_POINT`——这一条必须双平台分支写，否则又是一个「只在单一平台编译的门禁盲区」（`docs/经验.md` E43）。
4. root 先 canonicalize 并缓存；目标若不存在则 canonicalize 其父目录后拼文件名；比对用 `Path::strip_prefix`（canonical 后大小写是真实的，故 Windows 大小写不敏感问题由 canonical 化解）。
5. 二次防线：结果仍不得落在 DATA_DIR 内（防 workspace 被指到 `D:\kedai` 而 `data\` 就在其下）。

### 2.3 工作区文件工具族（`fs_*`）

**命名定为 `fs_*` 而非复用 `read`/`write`/`replace`**：既有四个工具是角色扮演语义（`target=bubble|file`、`safe_rel_path` 收口在角色文件区），改它们会牵动 1000+ 既有测试与线上行为；新族只在「workspace 已绑定」时下发，角色扮演路径逐字节不变。

| 工具 | 风险级 | 参数 | 语义与硬约束 |
|---|---|---|---|
| `fs_read` | Safe | `path, offset?, limit?` | 输出带行号；默认 200 行、上限 2000 行；NUL 字节判定为二进制即拒绝（`read_to_string` 会直接报错，需先探测给可读文案）；单次 ≤48 KiB（给注册表 64 KiB 截断留余量） |
| `fs_write` | Dangerous | `path, content` | **强制先读后写**：本 run 内必须有该 path 的 `fs_read` 记录，且读时 mtime/hash 与当前一致，否则拒绝并提示「先 fs_read」；原子写复用 `utils/fs_atomic` |
| `fs_edit` | Dangerous | `path, old, new, replace_all?` | `old` 在文件内出现次数 ≠1 且未显式 `replace_all` → **失败并回报实际次数**（既有 `replace` 是无唯一性校验的 `String::replace`，全量替换误伤面大，不沿用）；同样要求先读 |
| `fs_glob` | Safe | `pattern, max?` | 零新依赖：手写 `read_dir` 递归 + 通配匹配；默认排除 `.git/ target/ node_modules/ dist/ .kedai-index/ data/`；≤200 命中 |
| `fs_grep` | Safe | `pattern, glob?, max?` | 用既有 `regex`（`server-rs/Cargo.toml:46`）；返回 `path:line:text`；≤200 命中 |

单文件读写上限 8 MiB（超限明确报错，不静默截断）。

> 代价（必读）：新增内置工具触发 `契约-协议与配置.md:1878-1902` 的**6 处登记清单**（注册/风险级/场景白名单/何时调用/渲染意图/前端流程白名单），该清单**当前无机检**（遗留 CFG-2）→ 本批必须手工过一遍，并把「机检」排进提交 2（对应展望 REG-1）。

### 2.4 策略下发（fail-closed）

`services/task_engine/tool_policy.rs::compile` 增加 `has_workspace: bool` 入参：

- 绑定了 workspace → 下发 `fs_*`；`fs_write`/`fs_edit` 属 Dangerous，按**工具名例外**纳入（与 `bash` 同款，`:59`），`fs_read/glob/grep` 天然 Safe 不受影响。
- 未绑定 → **不下发**（模型看不到），且臆造调用时返回新错误码 `workspace_not_bound`（比「未注册」可诊断）。
- 空集必须 fail-closed（`docs/经验.md` E56）。

### 2.5 `bash` 侧

- 绑定 workspace 时：`cwd` 缺省 = workspace（不再是 `data_dir`）；显式 `cwd` 必须经 `safe_workspace_path` 校验。
- 未绑定 → 保持现状，零回归。
- **不校验命令文本里的绝对路径**：那是做不到的（见 §5），不要写进注释假装做了。

### 2.6 先写的失败测试清单

1. `safe_workspace_path` 负路径：`..`、绝对路径越界、symlink 逃逸、Windows junction、UNC、`C:`、大小写变体、NUL。
2. 工具级：未绑定 workspace 时 `fs_*` 一律被拒（错误码 `workspace_not_bound`）；绑定时沙箱内正路径读写通过。
3. `fs_write` 先读后写：未读拒绝；读过但 mtime 变化拒绝。
4. `fs_edit` 唯一性：0 处 / 2 处都失败且回报计数；`replace_all=true` 时全替换。
5. `bash` cwd jail：越界 cwd 被拒；未绑定任务行为与改造前一致。
6. 回归：角色扮演路径下发给模型的工具清单与行为不变（既有测试全绿）。

### 2.7 验证

```powershell
cargo test --manifest-path server-rs/Cargo.toml -j 8
npm test -w web
node tools/check-arch.mjs
node tools/check-contract.mjs
node tools/check-doc-claims.mjs
node tools/count-tests.mjs --check
```

### 2.8 执行结果（2026-09-26 收口，提交 `d000c4d`）

**已完成**，与计划的偏离与遗留如下：

- 落地形态与设计一致：`ExecScope`（工作区根 + jail 标志 + 本 run 读记录，`Arc` 共享）承载
  在 `ToolContext.scope`；`safe_workspace_path` / `data_dir_conflict` 为创建期与工具期共用判据；
  五个工具命名 `fs_read`/`fs_write`/`fs_edit`/`fs_glob`/`fs_grep`；策略侧新增 `workspace_gate`
  （仿 `platform_gate`），未绑定即整族剔除。
- **偏离 1**：未新增错误码 `workspace_not_bound`——未绑定时工具返回中文错误（执行侧兜底），
  且下发侧已剔除，模型实际见不到该工具，故未引入新错误码。
- **偏离 2**：`tools/tool_sets.rs` 的 `WORKSPACE_TOOLS` **未**加入 `READONLY_SCOUT` / `SUBAGENT`
  / `REFLECT`（保持子 agent 与规划侦察既有能力面不变），已在常量注释写明这是有意保持及放开
  条件——即 `fs_read` 目前**不参与 plan 模式的只读侦察**，绑定工作区后仍需单独评估再放开。
- **测试期发现并修复的真实缺陷**：`fs_read` 分页 off-by-one（`limit=1` 会多返回一行），
  由分页断言捕获。
- **门禁**：`npm run check` 全量 15 段全绿；后端 1431 项（1107 单测 + 324 集成）0 失败。
- **遗留（本批未做，供后续批次决定）**：① `api/tool_permissions.rs` 的授权面板仍列出
  `fs_*`（对聊天会话是噪声，非模型可见清单）；② `settings_connector` 的顺序依赖 TEST-ISO-1
  在全并发下仍偶发（单测隔离与 `--test-threads=1` 均通过），本批未处理。

## 3. 提交 2 — 可观测与防污染（结论可信度的前提）

### 3.1 变更清单与 diff

- 新表 `task_file_changes`：`task_id, step_index, path(相对 workspace), op(create|write|edit|delete|unknown), before_hash, after_hash, bytes, source(tool|bash), created_at`。
- 写工具：直接记录（精确）。
- `bash`：执行前后对 workspace 做「mtime+size 树扫描」，变动文件再算哈希 → 同一张表。**口径如实写清是启发式**（扫描预算：≤20 万条目或 5s，超限记 `undetected` 而非假装没事）。
- 新事件 `SseEvent::FileChanged`：**三处同步**（`models/types.rs` 枚举 + 发射点 + `web/src/stores/task.ts` 映射），与既有纪律一致（`AGENTS.md` 任务模式机制要点）。
- 新端点：`GET /api/tasks/{id}/changes`、`GET /api/tasks/{id}/diff?path=`（unified diff 自实现，不必引 crate）。

### 3.2 前端

- 任务模式工具调用卡片（**消费既有 `render_kind`**，当前 0 消费）；bash 走终端式呈现、文件工具走文件式。
- 变更清单 + diff 视图 + 单文件回退（按 `before_hash` 恢复，且回退本身再记一条变更）。
- 审批卡（聊天路径）显示命令原文与写入 diff，不再只给折叠 JSON。

### 3.3 防污染（针对评测，也针对真实使用）

- `GET /api/bootstrap` 收紧：**注意 `tools/bench/*.mjs` 依赖它**（`task-probe.mjs:22-26`）——改法必须是「给同用户进程留可读通道」而不是直接删：建议进程启动生成一次性 nonce 写入 `DATA_DIR/.bootstrap-nonce`（权限同用户可读），`bootstrap` 校验 nonce 并单次失效；探针改为先读该文件。**改动牵动前端启动链路**（web 启动先取 token），前后端 + 契约同步。
- DATA_DIR 写保护：`fs_*` 拒绝 DATA_DIR；`bash` 在 jail 开启时拒绝越界 cwd。
- 启动完整性自检：`settings.json`／`tool_permissions.json` 被外部改动时告警（warn + 前端提示）。

### 3.4 门禁加固

把提交 1/2 引入的新计数（表数、`ensure_*` 数、`SseEvent` 变体数、新端点）同步进 `tools/check-doc-claims.mjs` 断言表；并补 REG-1 要求的「新增内置工具 6 处登记」机检（能自动断言的部分）。

## 4. 提交 3 — 长任务韧性 + headless 入口 + 编码提示词 + 留档

1. **取消能中止工具**：`ToolRegistry::execute*` 接 abort（或循环侧 `select!` 竞争），`bash` 走进程树清理（Windows Job Object 或 `taskkill /PID x /T /F`；Unix `process_group(0)` + `killpg`）；顺手修 `services/exec/desktop.rs:4` 那句与实现不符的文件头注释。
2. **续跑**：`agent_sessions`/`tool_calls` 不再在 run 开头无条件级联删除；新增续跑入口（聊天与任务分别决定，`docs/经验.md` E46）。
3. **压缩/裁剪为编码场景保留工作状态**：摘要提示词加「已改动文件清单 / 已执行命令及结果 / 未完成 TODO」保留要求；`trim_tool_history` 对写类工具调用不占位化。
4. **自测闭环**：`bash` 超时上限可配（任务/节点级覆盖；默认上限提到 1800s 与 bench 的 600s/check 对齐）；输出截断改「保头 + 保尾」（失败摘要通常在尾部）。
5. **headless 单次入口**：`server-rs/src/bin/kedai-agent.rs`
   `--prompt-file BENCH_TASK.md --workspace <sandbox> --mode solo --out result.json`
   输出结构化 JSON（`status/summary/files_changed[]/usage`）+ 退出码 0/1/2；**不依赖 web/dist**；workspace 走同一套闸门。
6. **编码执行者模板**：内置一份（「编码执行者」指令 + 任务级 `agent_system_prompt`），显式写工具纪律（先读后写、bash 带 cwd、失败先跑测试），并在文档登记用法。注意 `EXECUTOR_PROMPT`（`services/task_core/prompt_consts.rs:13`）不含任何工具使用指引，模板必须自己补。
7. **留档**：`功能.md`（新工具族 + workspace 契约）、`契约-协议与配置.md`（新工具 6 处登记 + workspace 字段 + 新端点）、`契约.md`（新端点/事件/表）、`遗留.md`（§1 清单的新登记项与状态）、`经验.md`（本次采坑：bash 是唯一通道、进程树清理、cwd jail 不拦命令内路径）、`功能-变更史.md` 收口章。删本稿。

## 5. 明确不做 / 进程内做不到

**做不到（诚实边界，不要写成已解决）**：模型一旦有 shell，就能读该进程有权限读的**任意**路径 —— cwd jail 只约束相对路径与缺省目录，拦不住 `type D:\coding-bench\tasks\<id>\gold.patch` 这类命令内绝对路径。因此**「防题库泄底」在进程内封不死**，只能靠 OS 级手段：用独立低权账户/容器/VM 跑被测实例，或把题库目录 ACL 收紧到被测进程不可读。Kedai 侧能做到的只有「越界读写进审计 + 变更清单只认 workspace」。

**不做**：

- worktree/容器级工作区隔离（一题一沙箱由题库侧保证）。
- LSP/符号索引、网页抓取、图片与二进制读取。
- 多任务并行 UI（任务视图的并排/聚合）。
- 角色扮演主链路任何行为变更（`read`/`write`/`replace`/`create` 对旧场景逐字节不变）。
- 自动 `git commit` / 自动建分支。

**待决策（属于定位问题，需用户拍板）**：是否把「编码能力」正式纳入产品面。文档里既没有纳入（`docs/README.md` 的能力面总览无编码类），也没有明文排除。若只要「能跑 bench」，做完提交 1 即可；要「结论可信」做到提交 2；要「长任务跑得完」做到提交 3。

## 6. 前置条件与资源纪律

- **排序**：本稿排在 `DOC-DRIFT-PLAN` 收口之后开工。该计划的提交 3 正在工作区执行（`tools/check-doc-claims.mjs` 已生成、`DOC-DRIFT-PLAN.md` 已删、`tools/{check-all.ps1,check-arch.mjs,count-tests.mjs}` 已改），否则同一批计数口径（表数/`ensure_*`/枚举变体数）与断言表会互相打架。
- 本机 shell 无 cargo 环境 → `cargo` 走 `tools/cargo-vcvars.cmd` 包装器；`-j 8`，内存紧张时按既定纪律降 `CARGO_INCREMENTAL=0 + -j 2`，不要与 `build.ps1` 并发。
- 改了前端必须 `npm run build -w web`；交付/试用 exe 前跑一次完整 `.\build.ps1`。

## 7. 收口

删本文件 + `计划.md` 记裁定 + `功能-变更史.md` 收口章 + `遗留.md` 状态变更注。
