# Kedai 文档漂移盘点报告

> 日期：2026-09-28
> 范围：`docs/` 下九份活文档（约 1.9MB 正文）+ 根 `MAINTENANCE.md` / `README.md` / `AGENTS.md`；含 `docs/plans/` 三份报告与 `docs/archive/` 索引的结构核对（归档正文只读，未纳入漂移判定）
> 方法：7 路并行审查（按文档线切分）+ 主调查员对约 20 条高危项**逐条回代码复核**（`grep` / 读码 / `git log` / 实跑 5 道机检）；标「已复核」者为主调查员亲自验证，其余条目的证据均来自并行审查的代码引用
> 边界：**行号为 2026-09-28 快照**，执行修正时逐处先复核；历史层（「原文 + 变更标注」双层体例）保留旧值**不计为矛盾**，本报告不列入
> 生命周期：工作线调查报告，不随代码同步；开放项与执行方案见 `计划.md`（文档漂移收口批），执行证据见 `功能-变更史.md`

---

## 一、结论摘要

**机检全绿，但机检有明确盲区。** 五道机检（`check-docs` / `check-contract` / `check-arch` / `check-doc-claims` / `count-tests --check`）实跑全部通过——它们只锁「21 组类型映射 + 11 项计数口径 + 分层归属 + 文档结构链接」。文档中大量**行为描述、端点方法、状态标记、跨文档一致性**不在其中，而漂移恰恰集中在那里。

**文档骨架健康，坏的是时间轴。** 六类体系、事实权威优先级、历史层体例都在真运转：`遗留.md` §九「已封堵项索引」35 行**无一条假封堵**；`功能-变更史.md` 的 **98 个 commit hash 全部有效**；`展望.md` §六 六条证伪方向的前提在今日代码上仍全部成立；`经验.md` 编号体系（1~33 含重复 11、E1~E68）连续无重号。

**问题的成因高度一致**：2026-09-17～09-27 这 11 天约 40 个批次（274 次提交）之后，写于其前的那些层没有回填。次级成因有二：① **同一事实在多份文档手抄**（`deny_dangerous` 例外、上传上限、tracing 计数都是这样分叉的）；② **`文件:行号` 引用**在高密度改动下大面积漂移（成片偏移十几到上百行）。

按修正点计（一个修正点可含多处同步），本报告共登记 **84 点**：A 契约字面 20 / B 状态过时 19 / C 文档矛盾 11 / D ID 断链 16 / E 漏登记 8 / F 其他 10。其中**高severity 约 20 点**，可分两类风险：**照文档写会出错**（A 类）与**去修一个已经不在的问题**（B 类中的 6 条）。

（提出实施计划时的「约 55 处」是当时的保守估计，以本报告的 84 点为准。）

---

## 二、机检现状（本批的起点）

| 门禁 | 实跑结果 | 覆盖什么 | 覆盖不到什么 |
|---|---|---|---|
| `tools/check-docs.mjs` | `[OK] 文档六类体系一致` | D1 九份活文档存在 / D2 `docs/` 根白名单 / D3 README 链接双向一致 / D4 相对链接目标存在（**锚点不校验**）/ D5 归档映射 / D6 ID 形态 WARN | prose 内容、锚点正确性 |
| `tools/check-contract.mjs` | `[OK] 已校验 21 组映射` | Rust ↔ TS 类型/枚举字段集合 | 端点方法/路径、文档字面、字段清单完整性 |
| `tools/check-arch.mjs` | `[OK] 无新增架构违规`（5 条已登记债务） | 分层归属（SSOT `arch-layers.json`）、规则 A–L、ratchet 基线 | 对账章叙述与 SSOT 的**一致性**（顶层目录粒度） |
| `tools/check-doc-claims.mjs` | `[OK] 11 项计数口径一致` | 11 类可派生计数 | 语义级事实（端点方法对不对、条目做完没有） |
| `tools/count-tests.mjs --check` | `[OK] 与 MAINTENANCE.md 记录一致` | 测试数（后端 1464 / 前端 1254） | 其余文档手抄的测试数 |

**结论**：本报告 A~F 全部位于机检边界之外。这不是门禁写坏了，而是「派生器 + 文档正则」这套模式天然只能锁**能数出来的**事实。

---

## 三、A 类 · 契约字面硬错误（20 点）

照文档实现会直接出错或误判的一类。**全部需走 PR**（契约文档批次）。

### A1–A3 端点方法/路径写错（已复核）

| # | 位置 | 文档写 | 代码实际 |
|---|---|---|---|
| A1 | `契约.md:203` | `POST /api/settings/prompt-preview` | **GET**（`server-rs/src/api/routes/settings.rs:36`；前端 `web/src/api/settings.ts:88` 亦为 GET） |
| A2 | `契约.md:668` | `GET / POST / **PATCH** / DELETE /api/quick-replies*` | `PUT`（`routes/content.rs:66` 注册 `put(quick_replies::update)`） |
| A3 | `契约.md:660` | `GET / PUT /api/audio` | `GET /api/audio` + `PUT /api/audio/settings` + `PUT /api/audio/playlist`（`routes/misc.rs:68-70`） |

按文档发 POST/PATCH/`PUT /api/audio` 都会得到 405。

### A4 `workspace` 字段面完全缺失（已复核）

- **代码**：`server-rs/src/api/tasks.rs:63` 建任务请求含 `pub workspace: Option<String>`；`:110-130` 的 `validate_workspace` 在创建期 canonicalize 冻结、与数据目录双向包含即 400；`models/types.rs:1356` 的 `TaskRecord` 下发该字段；前端 `web/src/api/types.ts:1032` 已同步
- **文档**：`grep -c workspace docs/契约.md` → **0**（「工作区」亦 0）。`契约.md:460` 的请求体清单与 `:994` 的 `TaskRecord` 字段清单均无此项
- **风险**：开发者按文档实现建任务客户端时，既不知道能传工作区，也不知道传错会 400；另有整批 `fs_*` 工具族依赖它

### A5–A8 数值与清单错误（已复核）

| # | 位置 | 文档写 | 代码实际 |
|---|---|---|---|
| A5 | `契约.md:247`、`:854`、`功能.md:185` | 上传上限 **20MB** | **30MB**（`api/characters.rs:37`、`world_books.rs:36`、`prompt_inject.rs:60` 三处 `MAX_*`；外层 `DefaultBodyLimit` 35MB，`api/mod.rs:192`） |
| A6 | `契约.md:704-705` | `max_tokens` 钳 `1..=65536` | **131072**（`api/chat.rs:770` `GENERATE_RAW_MAX_TOKENS_LIMIT`；2026-09-16 提交 `2c319b2` 已统一上调） |
| A7 | `契约.md:979` | SSE 对账表列 10 个变体 | **11 个**，漏 `Retry`→`retry`（`models/types.rs:664`；同文 `:905-917` 与 `:1075` 自己都列了 11 个）——**文档内部自相矛盾** |
| A8 | `契约.md:1051` | `JsonBody` 接入 `tasks(4)` | **`tasks(5)`**（`tasks.rs:133,198,311,348,415`），且漏 `task_executors.rs:42`、`agent_flows.rs:160` 两个文件 |

### A9 缺录端点 10 组（已复核）

代码已注册而 `契约.md` **零覆盖**：

| 端点 | 注册处 |
|---|---|
| `GET/PUT/DELETE /api/characters/{id}/contract` + `/contract/history` + `/contract/rollback` | `routes/content.rs:34-46` |
| `GET/PUT/DELETE /api/script-authorizations` + `/api/script-authorizations/list` | `routes/misc.rs:79-88` |
| `GET /api/agent-flows/export` + `POST /api/agent-flows/import` | `routes/agent.rs:38-46` |
| `GET /api/chat/sessions/{id}/agent/trace` | `routes/chat.rs:50-53` |
| `PUT /api/scripts/tree`（文档 `:662` 只写 GET） | `routes/misc.rs:107` |
| `GET /api/bootstrap`（下发 API token，安全相关） | `api/mod.rs:160` + `static_files.rs:11-19` |

矛盾尤其明显的是：`契约.md:1021`/`:1043` 在讨论「contract 回滚的 422 语义校验」，却从未定义这三个 contract 端点。

### A10–A14 字段清单与引用失效

- **A10** `契约.md:266` 流程步骤字段清单缺 `tool_choice` / `tool_choice_function` / `parallel_tool_calls`（`models/types.rs:235-241`），`:296` 的校验清单同缺三条 400 规则（`agent_flow_service/graph.rs:272-296`）
- **A11** `契约.md` 有 **12 处**引用 `server-rs/tests/api_integration.rs` 与 `tests/tasks.rs`，两文件已于 2026-09-27 按域拆分删除（`MAINTENANCE.md:463` 有记录）；被点名的用例仍在（如 `assert_error_shape` → `tests/api_errors_codes.rs:67`）
- **A12** `契约.md:1108` 冻结清单 #25 的机检状态过时：`outcome` 三态已有单测覆盖（`tools/agent_tools.rs:125` 起 4 例），不再只是「变更说明里的验收断言」
- **A13** `契约.md:1260` 写 `FlowSnapshot` 由「映射 16」覆盖，实为**映射 19**（`tools/check-contract.mjs:157-163`；`:1166` 自己也登记为 19）
- **A14** `契约.md:1108` 与 `:1300` 对同一事实引用的行号**互不相同且都不是**该字段（应为 `agent_tools_agent.rs:766-784` 的 `outcome` 与 `:856-858` 的 `finished_at`）

### A15–A20 协议/策略/配置字面

- **A15** `契约-协议与配置.md:56` mvu 的 `UpdateVariable` 正则欠描述：两端实现都额外容忍标签内空白（`parsing/assistant/patch.rs:60` 有 `<\s*` / `</\s*`，`web/src/mvu/parser.ts:34` 同），文档正则匹配不到 `< UpdateVariable >`
- **A16** `契约-协议与配置.md:280-281` 称 `[GENERATE:BEFORE]` 拼在「运行时主提示词**之后**」，实际执行顺序（`agents/engine/messages/context.rs:486-500` 先前置 AGENTS_RUNTIME.md、`:593-608` 再把 generate_before 前置到同一 `s0.content` 开头）使其落在**之前**；代码注释 `context.rs:593` 与文档同错
- **A17** `契约-协议与配置.md:540-542` untrusted source 清单**漏 3 多 1**：漏 `task_executor`（`task_service/prompt.rs:101`）、`task_goal`、`flow_step`（`task_engine/custom.rs:252,259`）；`preset_tail` 并未作为 source 使用（预设尾部按 `world_book` 包裹）
- **A18** `契约-协议与配置.md:710` `deny_dangerous` 例外只写 `bash`，实为 **`bash` / `fs_write` / `fs_edit`** 三个（`task_engine/tool_policy.rs:74-76`）；`功能.md` 有 5 处同样只写 `bash` 而 `:923` 又写对三个 —— **同时是 C 类矛盾**
- **A19** `契约-协议与配置.md:709` 「`all` 档等价改造前全放行」不成立：实际还叠加 `exclude_meta`（`META_TOOLS` 三员含 `run_flow`）、`platform_gate`（剔 `submit`）、`workspace_gate`（无作用域剔 `fs_*`）
- **A20** `契约-协议与配置.md:762-763` 「无角色会话隐藏『允许当前角色』按钮」**未实现**：`web/src/components/AgentPanel.vue:471-475` 的按钮无条件渲染，实际兜底是后端授权失败降级为「仅本次允许」+ warning（`executor.rs:1452-1458`）

### A21–A22 配置与登记清单

- **A21** `契约-协议与配置.md:1252-1264` 环境变量表漏 **7 项**：`DEFAULT_TOP_P`、`DEFAULT_MAX_CONTEXT_TOKENS`、`KEDAI_ALLOW_REMOTE`、`KEDAI_STRICT_CLIENT_HEADER`、`KEDAI_API_TOKEN`（`config.rs:185,192,203,207,211`）、`KEDAI_ALLOW_INSECURE_PLAINTEXT_SECRETS`（`secret_store.rs:41`）、`WEB_DEV_PORT`（`.env.example`）；其中三个是安全开关
- **A22** `契约-协议与配置.md:1258` 「`OPENAI_API_KEY` 空时自动 mock 演示」已失效：连接器类型由 `resolve_connector_target` 决定（地址与密钥**都非空**才切 `openai-compatible`，否则保持当前类型），默认 `CONNECTOR=openai-compatible` 不回落 mock（`settings_service/connection.rs:82-100`、`connectors/mod.rs:114-122`）
- **A23** `契约-协议与配置.md:1975-2003` 「新增内置工具 6 处登记清单」**不完备**：反向核对另有 2 处「漏改即静默降级」——`services/undo_service.rs:25-31` 的 `WRITE_TOOLS`（新写工具不登记则无回退快照）、`task_engine/tool_policy.rs:74-85` 的按名例外（新增「危险级但任务模式需要」的工具会被静默剔除）

### A24–A26 状态与纪律句

- **A24** `契约-协议与配置.md:1819` 「`ci.yml` 当前已落盘但**未激活**」——与同文 `:1934`「（2026-09-26 **激活**）」**自相矛盾**（已复核：`ci.yml:19-23` 已激活）
- **A25** `契约-协议与配置.md:1314` 「三个分支**源码逐字节相同**」与现状不符：平台分支落后主干 **21 个提交**（`git rev-list --count kedai-Win..main`），差异文件含源码与门禁
- **A26** `契约-协议与配置.md:1389-1390` 的同步工作流建议 `merge --ff-only` / 「已领先时改用 `rebase`」，与同文 `:1922`「**不改写历史**：不 `rebase`/`amend` 已推送的提交」**打架**；平台分支的实际做法是 `merge`（`805a950` 提交信息专门写了理由）
- **A27** `契约-协议与配置.md:1904-1922` 的「§六 Git 提交纪律」未同步 `AGENTS.md:65-69` 的「非琐碎改动走 PR（2026-09-27 起）」整条（含四类触发面、纯文档例外、Free 计划无服务端保护的诚实边界）

### A28–A33 `契约-架构与数据.md` 的对账与结论

- **A28** `:229` 称 `migration/` **零 `crate::` 出边**——已被打破（`migration/ddl.rs:581` 用 `crate::parsing`、`merge.rs:25` 用 `crate::models`）；方向合法（L1→L1）故 `check-arch` 不报，属**门禁盲区**；SSOT `tools/arch-layers.json:62` 的 `notes` 同错
- **A29** 对账章计数过期：`frontend` **8 个目录**（实为 **9**，漏 `styles/`，`arch-layers.json:132` 已登记）；`frontendEntryFiles` **30 个**（实为 **33**，漏 `renderCache.ts` / `onboarding.ts` / `globalErrorHandlers.ts`）
- **A30** `:146`、`:508` 「`store.ts` 是 **7 个 store** 的门面」——实为 **8 个**（+`executor`，`web/src/store.ts:12-19`）
- **A31** `:240`、`:244`、`:157` 把 `services/undo_service.rs`、`services/exec/` 标为 **L3**，与 SSOT（`services` = L2）冲突；且对账章对此沉默——顶层目录粒度无法表达子域
- **A32** `:649-655`、`:805-806` 「基线没锻炼到其余 `ensure_*`」**结论说反**：冻结基线 `tests/fixtures/schema_baseline_v0_3_0_beta.sql` 缺 **7 个后加列**（`tasks.executor_id/flow_id/flow_snapshot/workspace`、两张子任务表的 `finished_at`、`characters.derived_json`、`exec_audit.risk_flag`），即 **8/15 个 `ensure_*` 确被该测试锻炼**
- **A33** `:729-730` 「自动快照**升级成功时才会写下**」**说反**（`models/db/mod.rs:137-144` 在 `BEGIN IMMEDIATE` **之前**写、失败只 warn 并继续、文件保留）；`migration/backup.rs:41-62` 与测试 `schema_migration_meta.rs:491/566` 印证——这是**回退指引里少报了一条真实恢复路径**
- **A34** `:815` 的「逐条现状汇总」把**第 2 条**列进「仍然成立」（`:794` 明写「已修复，2026-09-16」）；`:617` 与 `:630` 两处都自称「2026-09-15 起」却是 **9** 与 **13** 个 `ensure_*`

---

## 四、B 类 · 状态过时（已修好却仍记待办，19 点）

这类最浪费人力：会让后续会话**去修一个已经不在的问题**。共同特征是**修复事实早已写在 `计划.md`，只是没回流到 `遗留.md`**。

### B1–B10 `遗留.md` 应迁 §九（按 R6）

| ID | 文档现判 | 代码/提交实际 |
|---|---|---|
| **L22** 同类工具不同参数空转不被熔断 | 有意裁剪 · 转展望 | **已实现**：`utils/loop_guard.rs:186` `SemanticGuard`（同工具 ≥N 次 + 输出指纹去重 ≤K），消费点 `agents/engine/executor.rs:648`；有专门测试 `semantic_guard_breaks_on_constant_output_with_varying_args`；HB-2 状态「已完成（2026-09-19，`12e7d0c`）」 |
| **PERF-1** 无端到端延迟数据 | 待验证 · 转计划 | **已补测**：`tools/perf-baseline.json` 有「2026-09-17 P-G1 补测记录」（p50/p95/rps 实测行）；P-G1 状态「已完成（`2ec7cd7`）」 |
| **SIZE-1** `modal-settings` chunk 138.5K 超标 | 缺陷 · 转计划 | **对象已消解**：P-8（`c532a11`）已拆该 chunk（产物中零 `modal-*`），且 `check-bundle.mjs:97` 的 `FORBIDDEN_PRELOAD_PREFIXES=['modal-']` 已成硬门禁 |
| **SIZE-2** 超千行文件 6 个 | 知情接受 | **数字严重过期**：实测 **15 个**（最大 `agents/engine/executor.rs` 2015 行）；且点名的 `task_executor.rs` **文件名不存在**（实为 `task_service/executor.rs`） |
| **SIZE-3** 双端验证待跑 | 待验证 · 转计划 | **已两次跑通**：`build.ps1 -Tauri → exit 0`、两版 sidecar `dist_hash` 一致（`功能-变更史.md` 2026-09-16 与 09-27 两处录档） |
| **AND-1** release 必须补 `network_security_config.xml` | 缺陷 · 转计划 | **已落地**：`src-tauri/gen/android/app/src/main/res/xml/network_security_config.xml` 存在，`AndroidManifest.xml:28` 已引用，`build.gradle.kts:31/40/63` 三档 placeholder 齐备 |
| **AND-2** Keystore 前置探针未做 | 待验证 · 转计划 | **目的已达成**：`services/keystore_android.rs` + Kotlin `KeystoreBridge.kt` 均在并被 `secret_store.rs:36,78` 消费 |
| **G.5** 弹窗注册表重构未做 | 知情接受 | **已完成**：`web/src/modals.ts` 在，`计划.md:3281` 状态「已完成」 |
| **ARCH-4.8** 待办「加注释说明窄接口理由」 | 知情接受 | **已兑现**：`task_core/backend.rs:17-28` 有整段说明；且窄接口已 8→**9**（新增 `TaskScratch`） |
| **TM-D3/D6/D7/D8** | 标题已写「已修复」但判定字段仍「缺陷」，且未进 §九 | 四者代码全部在（`task_core/prompt_consts.rs:82`、`task_service/idle.rs`、`tools/bench/task-probe.mjs` 等）；§十一 前言却称「本节只留未修项」——**三处互相矛盾** |

### B11 `TM-X1` 的论据本身错误（**最关键的一条**）

- **文档**：`遗留.md:1130-1132` 断言「`server-rs/src/lib.rs:35-45` 在非 release 构建下把 scratch 根设为系统临时目录并**在启动时 `remove_dir_all`**，即开发/试用版的用户产物会被构建档位推断悄悄清空」；`计划.md:1535-1543`（PRODCAP-5 改动点①）据此设计
- **代码实际**：`lib.rs:28` 起是 **`pub fn build_test_app()`**——`tests/` 集成测试专用构造器，**生产路径零调用**（全仓 `grep build_test_app` 仅命中定义与两处「刻意不挂」注释）；生产路径 `config.rs:153-163` 明写「**纯路径计算，不做 IO**」；全仓 `grep cfg(debug_assertions)` 于 `server-rs/src`、`src-tauri/src` **零命中**
- **处置**（已裁定）：删该论断，改成「原文把测试专用构造器误读为生产行为」；PRODCAP-5 的改动点①降级为卫生项（给 `build_test_app` 补 `#[cfg(test)]` 门控），②③保留

### B12–B17 `计划.md` 状态行需回填

| 条目 | 文档现标 | 实际 |
|---|---|---|
| **G.3** `scriptBlocksCache` 上限 | 未启动 | **已完成**：P-9（`09a9e04`）已加有界缓存（`renderCache.ts:90` `createBoundedCache`，容量 500） |
| **PC-2** 大文件拆分 | 未启动 | **部分完成**：`services/memory_service.rs` 已随 Q1 五刀拆为目录模块（`e1d4634`） |
| **D.3** 裸 `{error}` 存量 | 「存量仍 105 处」 | 基线已降至 **1**（`check-arch.mjs:417` `BASELINE_BARE_ERROR = 1`）；同页 `:3190-3192` 自己已写「已降至 1」 |
| **架构分析批次 5** | 「第 1–4 项未做」 | 第 1 项（CI 真跑）与第 2 项（`count-tests --check` 升硬 FAIL，`check-all.ps1:235`）**已达成** |
| **批次 8** | 「终验待跑」 | 已多次跑通（`:1909` 章节、CI 权威档一次通过） |
| **HARNESS3-7** | 未启动 | 第 6 项留档已随编码能力包执行（`e8e9f94`） |
| **UIFIX-1** | **双状态行并存**（`:1779` 已完成 + `:1781` 未启动） | 已完成（`e376a3e`）——同一条目 3 行内两个互斥状态 |

### B18–B19 索引类

- **B18** `功能-变更史.md:3996` 未做项索引写「批次 5–8 未启动」，与同文 `:1909` 的批次 5 专章（5.1/5.3 已落地）**直接冲突**；另 3 行过时：`memory_service.rs` 未拆（已拆）、D.3 存量 117（现 1）、0.3.0-beta 未做真机验证（后续已出双 ABI APK + 双端实测）
- **B19** `计划.md:1605` FIX-PLAN 索引行「提交 3 七项待做」与同文 `:1624` HARNESS3-6「已落地」不一致

---

## 五、C 类 · 文档之间 / 文档内部矛盾（11 点）

| # | 位置 | 矛盾 |
|---|---|---|
| C1 | `契约-协议与配置.md:1819` vs `:1934` | `ci.yml`「未激活」vs「已激活」（实际已激活） |
| C2 | `功能.md:343-348` vs `:405-406` | 闸门空白名单语义：一处「空名单 = **拒绝一切**（旧语义已删除）」，一处「空白名单 = **放行**已按策略编译的集合」——代码是 fail-closed（`agents/engine/executor.rs:505-511`），前一处对 |
| C3 | `功能.md` 5 处 vs `:923`；`契约-协议与配置.md:663/710` | `deny_dangerous` 例外只写 `bash`，而代码与 `功能.md:923` 都是 `bash`/`fs_write`/`fs_edit` 三个 |
| C4 | `契约-架构与数据.md:787` vs `:815` | 风险汇总：一处说「第 2 条已修复」，另一处把 2 列进「仍然成立」 |
| C5 | `遗留.md:448` vs `:452` | 同文件追踪计数打架（info 29/debug 0 vs info 28/debug 1）；实测现值 204 处：warn 133/error 38/info 32/debug 1 |
| C6 | `契约-架构与数据.md:617` vs `:630` | 两处都自称「2026-09-15 起」，却是 9 与 13 个 `ensure_*` |
| C7 | `README.md:9` vs `:99` | 「原有 **68** 份…原文件移入」vs「移入的 **50** 份」（归档实有 50 份） |
| C8 | `契约-协议与配置.md:1389` vs `:1922` | 建议 `rebase` vs 「不改写历史」（实际做法 `merge`） |
| C9 | `计划.md:3454` + `:707` vs PRODCAP-1 | §八「明确不做」列「事件流持久化 / 回放」，而 PRODCAP-1 正是它的实施方案（`遗留.md:165-170` L26 已记重访），§八 未交叉标注——后来者按 §八 会否掉 PRODCAP-1 |
| C10 | `展望.md:482` | §八「共同前提」称 `PlanStep` 只有 `inputs`/`action`/`kind`，与同节迁移注记及 `models/types.rs:204-330` 不符（已含 `call_timeout_secs`/`max_retries` 等，正是本节 WF-16/WF-17 落地时新增的） |
| C11 | `功能-变更史.md:3996` vs `:1909` | 未做项索引与同文专章冲突（见 B18） |

---

## 六、D 类 · 归宿 ID 断链（16 点）

`遗留.md` 的「归宿」字段是条目出口，**16 处指向不存在的 ID**，读者按图索骥会全部落空。三类成因：

### D1 编号体系错位（上游文档用了别的编号，6 处）

| 遗留.md 行 | 写 | 实际应为 |
|---|---|---|
| `:466` | `计划.md PERF-1` | `P-G1` |
| `:487` | `计划.md SIZE-1` | `C-1` |
| `:501` | `计划.md SIZE-3` | `C-3` |
| `:285` | `计划.md ARCH-4.10` | 「架构分析批次 5 第 3 项」 |
| `:609/616/623` | `计划.md AND-1/2/3` | `A5-1/A5-2/A5-3` |
| `:444/455` | `计划.md 项 12/项 13` | `6.2`/`6.3` |

### D2 指向 `展望.md` 但条目不存在（9 处）

`展望.md:573` 自己声明「已确认缺陷或有意接受的残缺 → `遗留.md`」，与这些指向前缀**直接冲突**：

- `L3` → 实际方向在 `展望.md` 的 `RD-3`（`遗留.md:63`）
- `T1` → 实际落点是 `计划.md:3374` §七 `T1`（`遗留.md:183`）
- `D3`/`D4`/`D5`（`:232/239/246`）、`PBT-1`（`:636`，实为 `PB-1~PB-5`）、`TOK-1`（`:643`）、`TRUNC-1`（`:658`）、`CHG-4`（`:673`）、`ARCH-4.11`（`:699`）→ 该 ID 在 `展望.md` 零命中
- `DB-3`/`DB-4`（`:529/536`）→ 零命中
- **`DB-7`/`DB-8` 指反**（`:557/564`）：写「转计划」而条目实际在 `展望.md:275/282`

### D3 语义错指（2 处）

- `遗留.md:253` 说的「MCP 按模式覆盖」在 `展望.md` 是 **`CFG-2`**（不是 `CFG-1`）
- `遗留.md:260` 说的「工具清单收敛」在 `展望.md` 是 **`REG-1`**（不是 `CFG-2`）

### D4 无承载条目（2 处）

- `GUI-1`（`:576`）→ `计划.md` 零命中（判定本身成立，但无计划条目可指）
- `GUI-2`（`:580-584`）的测试计数为在制品快照，已由 `count-tests` 守护

---

## 七、E 类 · 已落地但漏登记（8 点）

| # | 漏了什么 | 应登记处 | 证据 |
|---|---|---|---|
| E1 | **任务执行者库整条线**：`stores/executor.ts`、`api/executors.ts`、`TaskExecutorsModal.vue`、后端 `api/task_executors.rs` + `services/executor_service.rs`、提示词优先级 executor > 角色卡 > 通用 | `功能.md` 前端章 / Agent 引擎章（`契约.md:475` 已登记，功能面全无） | P-11 批（`ef2dfe5`） |
| E2 | **记忆召回通道**：双通道注入（记忆槽 + 召回槽，`agents/engine/messages/inject.rs:374`）、混合打分（向量×0.7 + Jaccard×0.3，`memory_service/recall.rs:49-99`）、`RECALL_LIMIT=3`、字符预算与截断提示 | `功能.md` 记忆章（现只写单通道，且排序链漏 `pinned DESC`） | — |
| E3 | **`ci-linux.yml`**（Linux 可移植性档，`check-ubuntu` + android-smoke 手动） | `功能.md:1584` 的「三处触发」（`契约-协议与配置.md:1935` 已有） | QUALITY-FIX Q4 |
| E4 | **内建工具漏 3 个**（`role` / `revise_passage` / `submit`），且写了不存在的工具名 `run_subtask`（实为 `agentgo` + `read(type=subtask)`）；`META_TOOLS` 实为三员（含 `run_flow`） | `功能.md:938-946` 内建工具枚举 | `tools/mod.rs:47-94`、`agent_tools_shared.rs:134`、`revise.rs:23`、`submit.rs:24` |
| E5 | **前端结构**：`AgentDock` 组件不存在（实为 `AgentPanel.vue`）；七子 store → 八子（漏 `executor`） | `功能.md:1231`/`:1232` | `grep -rn AgentDock web/src` 零命中 |
| E6 | **渲染白名单 33 → 48 标签** | `功能.md:1271`/`:1273` | `web/src/render.ts:166-198` |
| E7 | **经验条目描述**：`build.ps1` 并不注入 vcvars（在 `check-all.ps1:49-93` 的子进程里）；`.shakedown/krun.bat` 不在仓（被 `.gitignore:47` 忽略）；E23 可删清单含 3 个当前不存在的路径 | `经验.md:25`、`:417`、`:464-469` | 逐条 grep |
| E8 | **`README.md` 的经验锚点区间**「条目 1~32」未含已存在的**条目 33** | `README.md:42/55/139/144` | `经验.md:226`、`MAINTENANCE.md:449` |

---

## 八、F 类 · 其他漂移（10 点）

| # | 位置 | 问题 |
|---|---|---|
| F1 | `经验.md:792` | 含一个 **0x08 退格控制字符**（`sed -n '792p' \| od -c` 可见），渲染成 `.uild.ps1` |
| F2 | `MAINTENANCE.md:459`、`:172` | `cargo test` 示例不带 `-j 8`，与 `AGENTS.md:36`、`经验.md:537`、`check-all.ps1:112` 三处纪律不同步 |
| F3 | `契约.md` | 行号锚点成片漂移（文档 `:1305` 自己声明「行号随重构漂移」），且 A14 那对**连互相一致都做不到** |
| F4 | `契约-协议与配置.md:799-832` | 行号锚点漂移（`config.rs` 约 6 行、`src-tauri/src/lib.rs` 约 100 行、`merge.rs`） |
| F5 | `展望.md:39/277/322` | 三处转载行号漂移（`build.ps1` 419-427、`lib.rs` 458-462、`connectors/mod.rs:29-32`） |
| F6 | `README.md:35` | 「**全部** HTTP 端点字面契约」描述过强（A9 的 10 组端点无契约；`:1130` 的机检总表自己也写「全部端点**响应形状**不在校验范围」） |
| F7 | `功能-变更史.md` 文首章索引 | 近 6 个新章未挂索引（`#2026-09-23-node-connection`、`#2026-09-27-doc-ledger`、`#2026-09-27-uifix`、`#2026-09-28-branches`、`#2026-09-28-onboarding`、`#2026-09-28-coding-bundle`） |
| F8 | `契约.md:59` vs `:460/:516` | 章首声明「API.md 逐端点**全文移植**」又在正文逐批增补，读者无法判断哪些清单完整、哪些是快照——建议给每条清单加「完整/节选」标注 |
| F9 | `check-all.ps1:216` | `if (-not $SkipWeb -and -not $AuditOnly)` 包住 `docs: check-doc-claims`，**`-SkipWeb` 会静默跳过文档口径检查**（与 `:204` 注释声称的「与 -SkipWeb 解耦」只对 check-docs 成立）——**既有缺陷，另立条目** |
| F10 | `计划.md:3089`/`:3302` | 两处计数随代码漂移且无历史层标记：C-2「实测 12」现值 **13**（且枚举含已消失的 `memory_service.rs`）；FT-2「46 条 warn」现值 **49** |

---

## 九、根因与防复发

### 三个根因

1. **门禁只能锁「数得出来」的事实**。语义级漂移（端点方法对不对、条目标记的状态真不真、两处说法哪个是旧的）派生器写不出来。本报告 A/B/C 三类共 50 点全在机检之外。
2. **同一事实多份手抄**。`deny_dangerous` 例外在 3 份文档 7 处、上传上限在 2 份文档 3 处、tracing 计数在同一文件 2 处——手抄必然分叉。仓库已有纪律（可计数事实只写一处）但仅覆盖「由代码派生」的那类，**语义事实**（策略例外、行为描述）无此约束。
3. **`文件:行号` 引用在高密度改动下必然腐坏**。11 天 274 次提交、约 40 个批次，成片锚点偏移。文档自己已声明「行号会漂」，但仍大量使用。

### 防复发（本批已落地 / 另立条目）

- **已落地**：给 `tools/check-doc-claims.mjs` 增第 12 条断言「**路由总表穷尽性**」——Rust 侧括号配平抽 `.route(...)` 的 `(method, path)` 集合，文档侧抽三种形态并归一，差集非空即 FAIL。这把 A1–A3、A8、A9 那一类（本次最大的一类）变成硬红。
- **已落地**：同脚本补**自指断言**（`CLAIMS` 条数 == `契约.md` 声明的项数），防「11 项」这类声明静默过期。
- **另立条目**（见 `计划.md`）：① 语义事实的「单一出处」纪律（策略例外、行为描述只在契约文档写一处，功能面引用而不复述）；② 行号引用向 `文件::符号` 迁移；③ F9 的 `-SkipWeb` 缺口。

### 对「文档 vs 代码」权威顺序的确认

本批全部修正遵循 `docs/README.md` 既定纪律：**以代码实测为准**，先修文档。唯一例外是 TM-X1（`遗留.md` 的论据本身错误）——那不是「文档旧了」而是「文档当时读错了代码」，故同步收窄依赖它的 PRODCAP-5。
