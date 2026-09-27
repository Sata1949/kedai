# 任务模式实测缺陷 · 修复计划（2026-09-26）

> 临时执行稿：不进提交，收口时删除。口径已定项执行时不得擅自变更（要改先改本节并说明理由）。
> 行号取自当前工作区（HEAD `d000c4d` 之后未改代码）；**实施前一律先 grep 复核**（`docs/经验.md` 条目 2）。

## 0. 评估口径与证据来源

计划对象是「真实模型 12 轮实测」暴露的缺陷：6 模式 × 写作/编程，模型 `deepseek/deepseek-v4.1-flash`（经 commandcode 网关），隔离数据目录 + 六份同起点沙箱（含 3 个植入缺陷的 Node 小项目 + 14 条断言）。

**证据索引（全部可复现）**

| 类别 | 位置 |
|---|---|
| 12 份结果 JSON | `D:\kedai-bench-run\result-<mode>-<cap>.json` |
| 服务端日志（含每轮 provider 往返） | `D:\kedai-bench-run\logs\server-v41\kedai-2026-09-26.log` |
| 命令审计（92 条，含退出码） | `GET /api/exec/audit`（`exec_audit` 表） |
| 沙箱与机判 | `D:\kedai-bench-run\sandbox\<mode>\`、`verify-prog.mjs`、`judge-writing.mjs` |
| 探针/套件 | `D:\kedai-bench-run\probe.mjs`、`run-suite.mjs` |

**实测结果基线（修复前的对照值）**

| 模式 | 写作 | 编程（独立复跑沙箱） |
|---|---|---|
| legacy | error / 478s / 结果 0 字（4 步全 done） | partial / 830s / 9-14 未改（无工具，模型拒绝编造） |
| solo | done / 15s / 565 字，机判全过 | 未收敛（903s 截断）/ **14-14 OK** |
| multi | done / 17s / 651 字，机判全过 | 未收敛（903s 截断）/ 14-14 OK |
| plan | 未收敛（901s 截断）/ 无正文 | 未收敛（903s 截断）/ 14-14 OK |
| team | 未收敛（901s 截断）/ 无结果 | 未收敛（903s 截断）/ 14-14 OK |
| custom | done / 74s / 566 字，机判全过 | done / 256s / 14-14 OK |

**已经能用的部分（本计划不得回退）**：`fs_*` 工作区闸门（绝对/相对越界均拒绝）、绑定工作区后 bash 缺省 cwd=工作区、17 个工具的 deny_dangerous 编译结果、五个模式真能改对代码、legacy 的诚实失败、custom 的可核验结论。

## 1. 缺陷清单

### D1 — 未绑定工作区的任务把数据目录当工作目录（**会脏真实数据，最高优先**）

**现象**：plan/writing 一轮跑了 **77 条 shell 命令**、在 `data-v41\` 留下 12 个垃圾文件（`a.txt cn.txt hh.txt hh2.txt out.txt s.py s2.py s3.txt t.txt z.txt zz.txt zz2.txt`），还写 python 脚本去 `open('kedai.db-wal','rb')` 搜字符串、`Get-ChildItem -Recurse` 扫 `characters/avatars`；15 分钟不收敛。team/writing 同型。

**机理（三段成因，缺一不成灾）**

1. **规划器不知道执行者没有文件工具**：`plan_task` 的 system 只有 `PLANNER_PROMPT` + 世界书（`services/task_service/executor.rs:703-726`），于是模型把交付物规划成 `constraints.md`/`draft.md`/`check.md`——实测它的计划原文就是这么写的。
2. **没绑工作区就没有文件工具**：`tool_policy.rs::workspace_gate` 整族剔除 `fs_*`（`tools/tool_sets.rs:39-43` 的注释即此契约，常量在 `:44`），执行阶段只剩 `bash`。
3. **bash 缺省 cwd = 数据目录**：`tools/bash.rs:88-106` 的 `resolve_cwd`，`(None, None) => data_dir`（`:105`）。模型只能 `echo 中文 > a.txt`，cmd 按 ANSI 落盘、`cat` 回来是乱码，于是换 `chcp 65001`、换 python、换 `-X utf8` 无限重试。

**为什么算缺陷而不是「模型乱来」**：三段都是产品侧的可控量——计划器不给能力信息、工具面在无工作区时为空、缺省目录指向用户数据。用户只要提「产出一份文档」，就会命中。

### D2 — 已完成的工作被丢弃

**现象**：legacy/writing 的 4 个步骤全部 `done`（`plan[].result` 累计约 3900 字），最后 summarize 撞 300s 看门狗 → 任务 `error`、`tasks.result` 为空。team/writing 9 个子目标完成 6 个（2 个 300s 超时）→ 无最终结果。

**机理**：

- `TaskTerminal::Failed { error }` **没有部分成果字段**（`services/task_core/terminal.rs:13-33`）；`legacy.rs:134-159` 汇总失败即 `Failed`。
- 落库侧 `finalize_run` 的 error 分支只 `set_error`（`task_service/executor.rs:1090-1122` → `db.rs:356-378`，SQL 只写 `error`/`status`，**不碰 result**）。
- plan/team/custom 都有 `Partial` 分支（`plan.rs:137-142`、`team.rs:776-800`、`custom.rs:1272-1280`），但**都要求汇总调用先成功**；汇总一失败就掉进 `Failed`。
- 前端成果卡只对 `done/partial` 渲染（`web/src/components/TaskBoard.vue:508-511`），error 态即便有 result 也不展示。

### D3 — 任务在时限内不收敛

**现象**：6 轮编程里 4 轮、6 轮写作里 2 轮在 15 分钟内没结束。solo/编程 2 分钟就改对了，之后 10 条命令全是反复自检（重跑测试、`pwd`、`ls`、`git status`、`git diff`、`md5sum`），单轮 LLM 往返 1.5~3 分钟。

**机理**：

- 收敛手段只有两个熔断 + 轮次上限：指纹熔断窗口 8/阈值 3（`utils/loop_guard.rs:15-17`）只看**参数完全相同**的调用，模型每次换个命令就绕过；语义熔断（HB-2，`loop_guard.rs:148-215`）阈值 16/12/2 对任务模式的短循环太松，且它按「同一工具出现次数」计数，模型 bash/fs_read/ls 交替就凑不满 12 次。
- **没有整步墙钟预算**：`max_tool_rounds` 默认 32（`agents/engine/executor.rs:586`）且每个 agent 循环各自重置（plan 每步、team 每子目标），64 轮的设置值足以撑出十几分钟。
- 执行者指令（`EXECUTOR_PROMPT`）没有任何工具使用/收尾纪律，模型不知道「自测通过就该收尾」。

### D4 — `bash` 的名字与实际 shell 不符

**现象**：模型按 bash 语法写命令，实测失败例（`exec_audit` 可查）：`node test/vec2.test.mjs; echo "EXIT=$?"`（cmd 不认 `;`，exit 1）、`pwd; ls -la; md5sum ...`（exit 1）、`cd /d/kedai-bench-run/sandbox/solo && ...`（cmd 不认 `/d/` 路径，exit 1）、`git status --porcelain`（exit 128）。每条白烧一轮 1~3 分钟。

**机理**：工具名 `bash`，参数描述里才有「Windows=cmd」（`tools/bash.rs:37-45`）；工具结果头只回「执行器档位 + 退出码」（`bash.rs:249-252`），不回 shell 与 cwd，模型拿不到反馈去纠正假设。

### D5 — 规划侦察看不到工作区

**现象**：plan/编程 的 planner 侦察轮里 `read` 报 `读取失败: 读取文件 package.json 失败: 系统找不到指定的路径`（详见 `result-plan-coding.json` 的 `prompt_summary`）。

**机理**：`READONLY_SCOUT = ["read","search","memory_read","calculator"]`（`tools/tool_sets.rs:20`）**有意不含 `fs_*`**（同文件 39-43 行注释写明），而 `read` 走的是角色文件区语义——于是**即便绑定了工作区，规划器也看不见工作区里的代码**，只能盲规划。

### D6 — reasoning 模型的输出预算被思考吃光

**现象**：该模型约 90% completion token 是 reasoning（实测 78/100、1835/2000），`default_max_tokens=10000` 下出现 `finish=length` 且 content 为空（legacy 有一整步如此），任务侧文案只说「返回空内容(finish_reason=length)」（`task_engine/solo.rs:198-206`），用户无从知道该调什么。

### D7 — 客户端放弃 ≠ 任务停止

**现象**：探针 15 分钟到点退出后，任务仍在服务端跑（继续烧 token、与后续轮次抢 provider），我手动 stop 过两个孤儿任务。产品侧同理：关掉 UI 不停止任务。

**机理**：任务的存活只由 `TaskService.cancels` 的 watch 通道决定（`task_service/cancel.rs:15-34`），与任何客户端连接无关；`recover_orphan_tasks` 只在**服务重启**时把孤儿置 `ended`（`db.rs:14-47`）。

### D8 — 兜底批（小、独立、已知）

1. **`settings_connector` 顺序竞争**：文件级连跑 6 次复现 3 次失败（约 50%），会让 `build.ps1` 门禁随机中止。根因：该 binary 共享进程级 app 与临时数据目录，`task_overlay_does_not_leak_into_roleplay_settings` 结尾把 task 覆盖层 `agent_system_prompt` 写成空串（`server-rs/tests/settings_connector.rs:202-208`），破坏了 `roleplay_default_prompt_visible_on_fresh_install`（同文件 382+）依赖的「首装=内置默认词」前提。
2. **`tools/bench/task-probe.mjs` 一直在跑错误题面**：`POST /api/tasks/{id}/run` 的 handler 不接 body，探针发的 `{input: ...}` 被静默忽略；任务目标其实是 `title`（`services/task_engine/mod.rs:201`）。探针又不做超时取消，会产生孤儿任务。
3. **模式脚手架混进交付物**：plan 的 result 尾部拼了 `## 最终计划`（`plan.rs:132-136`）、team 拼了 `## 审计结论`（`team.rs:778-782`），写小说的场景下用户拿到的是「正文 + 模式记账」。

## 2. 需要拍板的三个点（附推荐）

| # | 决策 | 选项 | 推荐 |
|---|---|---|---|
| Q1 | 未绑定工作区的任务，可写面怎么给？ | (a) 每任务建 **scratch 目录**并作为 bash 缺省 cwd，同时下发 `fs_*`；(b) 只把 bash 缺省 cwd 移到 scratch，不给 `fs_*`；(c) 不建目录，只改规划器提示词 | **(a)**：一次解决 D1 的三段成因——模型有正规文件工具就不会去用 `echo` 拼文件，且交付物可被结构化记录（与 HARNESS-PLAN 提交 2 的变更清单/diff 天然衔接） |
| Q2 | scratch 根放哪？ | (a) `<DATA_DIR>/../task_scratch/`（数据目录的兄弟目录）；(b) `DATA_DIR/task_scratch/`（需给 `data_dir_conflict` 开例外）；(c) 系统临时目录 | **(a)**：不削弱「`fs_*` 不得落在数据目录」这条既有不变量（(b) 要开例外），又比 (c) 可找回、可清理。新增 `KEDAI_TASK_SCRATCH_DIR` 环境变量覆盖 |
| Q3 | `bash` 是否改名 `shell`？ | (a) 保留名字，只把 shell/cwd 写进描述首行与每次结果头；(b) 改名并做迁移 | **先 (a)**：改名牵动注册表、风险级、白名单、前端白名单、文档 6 处登记与大量测试，收益仅是名字语义；(a) 已足够让模型一轮内纠正假设 |

## 3. 提交 1 — 不污染：可写面收敛（对应 D1/D4/D5）

**目标**：任务在「没绑工作区」时也不再把用户数据目录当草稿纸；同时让模型知道自己在哪、能用什么。

| 动作 | 位置 | 要点 |
|---|---|---|
| 新增 scratch 根解析 | `server-rs/src/config.rs`（`env_str("KEDAI_TASK_SCRATCH_DIR")`，默认 `data_dir.parent()/task_scratch`）、`api/state` 透传 | 与 `data_dir` 同级；启动时不做 IO，首次使用时 `create_dir_all` |
| 任务级 scratch 绑定 | `services/task_engine/mod.rs:182-193`（现 `scope_for_task(task.workspace)` 处） | 未绑定工作区时改为 `scope_for_task(scratch_dir(task_id))`，`jail=true`；scratch 建不出来 → **明确报错终止**，不静默回退数据目录（照抄现有「目录失效不降级」的注释口径） |
| `fs_*` 随 scratch 下发 | `services/task_engine/solo.rs:87-92`、`custom.rs:288`（`call.scope.is_some()` 处） | 语义不变（有 scope 就下发），但需复核 `tool_policy` 注释：从此「未绑定」与「无 scope」不再等价，注释要写清 |
| bash 缺省 cwd | `tools/bash.rs:88-106` `resolve_cwd` | 逻辑不变（有 scope 即用 scope），确认 scratch 路径经 `safe_workspace_path` 后落位正确；`(None, None)` 分支保持数据目录＝**只服务聊天路径** |
| 结果头自描述 | `tools/bash.rs:249-252` | 头部改为 `$ {command}\n(执行器:{档位};shell:{cmd|sh};cwd:{实际 cwd};退出码:{})`，并在 description 首行写明「Windows 下由 cmd /C 解释：不支持 `;` 分隔、`/d/` 路径、`md5sum` 等 GNU 工具」 |
| 规划器能力感知 | `services/task_service/executor.rs:703-726`（`plan_task`）、`services/task_engine/team.rs:420` | 追加一段内置「本轮可用能力」：由 `tool_policy::compile(...)` 的结果归纳成三类事实（可写文件/可执行命令/两者皆无）。无文件工具时明确写「交付物必须是正文，不得规划成文件」 |
| 侦察轮放开只读工作区工具 | `tools/tool_sets.rs:39-43` 注释 + 侦察装配点（`plan_scout_loop` 附近） | 绑定工作区时把 `fs_read`/`fs_glob`/`fs_grep` 并入侦察白名单（仍禁写）；同步改 39-43 行那段「有意不进」的注释 |
| 命令审计增强 | `services/exec/audit.rs` / `tools/bash.rs` | 命令文本命中 DATA_DIR 绝对路径（或 `..` 上溯）时，审计行标 `risk_flag=data_dir_touch`。**只标记不拦截**（理由见 §7）|

**先写的失败测试**

1. `resolve_cwd`：无 scope 且给定 scratch scope 时，缺省 cwd = scratch；scratch 不存在 → 报错而非回退数据目录。
2. scratch 越界：`fs_write('../settings.json')` 被拒；`bash` 显式 `cwd=DATA_DIR` 被 jail 拒。
3. 规划器提示词：绑定工作区但未绑 scratch 的用例断言「无文件工具」段存在；绑定工作区的用例断言能力段含 `fs_read`。
4. 结果头：断言输出含 `shell:` 与 `cwd:` 两个字段（防回退）。
5. 回归：聊天路径（无 scope）缺省 cwd 仍为数据目录，逐字节不变。
6. 回归：角色扮演链路的工具清单与行为不变（既有 1400+ 测试全绿）。

**验证**：`cargo test --manifest-path server-rs/Cargo.toml -j 8`；`npm test -w web`；`node tools/check-arch.mjs` + `check-contract.mjs` + `check-doc-claims.mjs` + `count-tests.mjs --check`；改前端才需 `npm run build -w web`。

## 4. 提交 2 — 不丢成果：部分成果兜底落库（对应 D2）

**目标**：任何终态（含 error/ended）下，已完成步骤的产出都必须可被用户看到，且状态诚实（partial 而非 error）。

| 动作 | 位置 | 要点 |
|---|---|---|
| 新增确定性拼装器 | `services/task_core/`（新文件，如 `assemble.rs`） | `assemble_from_plan(&[TaskStep]) -> Option<String>`：只做「## 步骤名」+ result 的确定性拼接，不调 LLM；四模式共用，**禁止各写一份**（与 `prompt_kit` 单一出处同一纪律）|
| 汇总失败的降级 | `task_engine/legacy.rs:134-159`、`plan.rs:118-136`、`team.rs:743-782`、`custom.rs:1265-1288` | 汇总/审计调用失败时：若 `plan` 中存在 done 步骤 → 返回 `Complete{ status: Partial, result: 拼装文本, error: Some(汇总失败原因) }`；无任何成果才 `Failed` |
| 取消路径同样兜底 | `task_service/executor.rs:1090-1122` `finalize_run` 的 ended 分支 | 置 `ended` 前，若 result 为空且 plan 有 done 步骤 → 先 `set_result_with_error(..., Partial, ...)`；`TaskTerminal::Failed` 增加 `partial: Option<String>` 或改由 `finalize_mode_run` 读 plan 兜底（二选一，取实现更小的那个）|
| 前端展示 | `web/src/components/TaskBoard.vue:508-511` | 成果卡条件放宽为「result 非空即展示」，error 态额外标注「任务未完成，以下为已完成部分的成果」 |

**先写的失败测试**

1. legacy 汇总失败（mock 让 summarize 返回超时错误）→ 任务终态 `partial`、`result` 含各步骤产出、`error` 含原因。
2. plan 汇总失败 → 同上；且 `## 最终计划` 段不重复出现（拼装器与模式脚手架去重）。
3. 取消（stop）时若有 done 步骤 → 终态 `ended` 且 result 非空。
4. 无任何 done 步骤时仍为 `error`/`ended` 且 result 为空（不得伪造成果）。
5. 前端单测：error 态且有 result 时渲染成果卡。

**验证**：同上；另外必跑一次实测回归——用本计划的实测环境重跑 legacy/写作，确认从 `error/0 字` 变成 `partial/≈3900 字`。

## 5. 提交 3 — 能收敛：预算、熔断与编码纪律（对应 D3/D6/D7）

**目标**：把「15 分钟不收敛」变成「在合理时限内带着产出收尾」。

| 动作 | 位置 | 要点 |
|---|---|---|
| 步骤墙钟预算 | `agents/engine/executor.rs:586`（`max_rounds` 处）与 `:1139-1164`（轮次上限收口处） | 新增 `GenerationParams.step_budget: Option<Duration>`，到点即**带着已有 content 收尾**（返回 `Ok`，步骤记 `done` 并发事件「步骤墙钟预算用尽，已带着现有产出收尾」），不制造失败。预算来源：settings `task_step_budget_secs`（0=关，默认建议 900） |
| 语义熔断对任务模式收紧 | `utils/loop_guard.rs:21-25`（默认 16/12/2）+ `executor.rs:601-609`（取值处） | 任务模式（`session_id` 以 `task:` 开头）使用更紧的默认（建议 8/4/2），并让这三个值进 task 覆盖层（现为扁平全局，`api/settings.rs:834-852`）|
| 编码纪律提示词 | `services/task_core/prompt_consts.rs`（新增内置模板） | 「自测通过即收尾、不得重复验证同一事实；命令用本机 shell 语法；改文件优先用 `fs_*` 而非 `echo` 重定向；每轮只做一个动作」。与 HARNESS-PLAN 提交 3 第 6 项同源，**合并做一次**，别再写第二份 |
| `finish=length` 可操作报错 | `task_engine/solo.rs:198-206` 与同类收口点 | 文案带上「输出上限被思考占满」的判据与建议（提高 `max_tokens`），并在 `task_llm_calls` 记 reasoning 占比 |
| 任务级空闲自愈 | `services/task_service/` 新增看守任务 | 周期扫描 running/planning 且「最近 N 分钟无新 `task_llm_calls` 行、无 cancel」的任务 → 走与 stop 同一条收尾路径并标注「空闲超时自动收尾」。**这是 D7 的产品侧答案**（客户端放弃不等于任务永生），阈值可配 |

**先写的失败测试**

1. 步骤预算：mock 让循环持续返回工具调用 → 到点步骤 `done`、content 非空、事件文案正确。
2. 语义熔断：交替工具但输出实质相同的序列（bash→ls→bash，输出不变）在 8 次内触发。
3. 提示词模板存在性 + 契约登记（该模板与 `EXECUTOR_PROMPT` 的差别必须被测试钉住，防止被后续改动抹平）。
4. `finish=length` 且 content 空 → 错误文案含 `max_tokens` 建议。
5. 空闲自愈：注入一条「running 但无活动」的任务 → 看守将其收尾且不发重复事件。

**验证**：同上；实测回归：solo/编程应在预算内 `done`（对照本轮基线：903s 截断、无结论）。

## 6. 附录批 — 门禁与工具修正（可并入提交 3）

| 动作 | 位置 | 要点 |
|---|---|---|
| 修 `settings_connector` 顺序竞争（D8-1） | `server-rs/tests/settings_connector.rs:202-208` | 用例结束时把 task 覆盖层恢复成**内置默认词**（或删除覆盖层键）而不是空串；或把「首装」用例拆到独立 binary（共享状态天然隔离）。判据：连跑 6 次全绿 |
| 修任务探针（D8-2） | `tools/bench/task-probe.mjs` | 题面写进 `title`；到点或终态即 `POST /stop`；结果 JSON 里记 `timed_out` |
| 交付物与脚手架分离（D8-3） | `task_engine/plan.rs:132-136`、`team.rs:778-782` + `TaskBoard.vue:457-522` | 二选一：(a) result 只放成果，`## 最终计划`/`## 审计结论` 由前端用 `plan` 数据渲染；(b) 保留文本但加 settings 开关。**推荐 (a)**，理由：同一份信息不该在文本与结构里各存一份 |

## 7. 明确不做 / 进程内做不到

- **不做命令文本的路径分析来「封堵」数据目录**：模型有 shell 就能读进程可读的任意路径（本轮实测 `type D:\kedai-bench-run\data\settings.deepseek.json` 退出码 0）。审计标记可以做，「拦住」是假的——HARNESS-PLAN §5 的判定不变。
- **不做**工作区/容器级隔离、OS 账户隔离（题库侧保证）。
- **不改**角色扮演主链路行为（`read`/`write`/`replace`/`create` 与聊天路径的 bash 缺省 cwd 逐字节不变）。
- **不动**六模式的编排语义（除交付物拼接）；team/plan 的审计与打回逻辑不碰。
- **不引**新依赖（LSP/符号索引/网页抓取）。
- Q3 的改名不做（见 §2）。

## 8. 前置条件与纪律

- 本计划的提交 3 与 `HARNESS-PLAN.md` 提交 2/3 有重叠（变更清单/diff、编码模板、headless 入口、取消中止工具）：**先合并口径再动手**，避免同一件事两个批次各写一遍。
- 排序建议：**提交 1 → 提交 2 → 提交 3**。提交 1 是数据安全，提交 2 让失败可诊断，提交 3 依赖前两者的观测面。
- 每个提交独立过门禁；`settings_connector` 的竞争未修前，门禁有约 50% 概率误红——先跑附录批那一项再开后续提交（或每次红先隔离复跑判别）。
- 本机 shell 无 cargo 环境：`cargo` 走 `tools/cargo-vcvars.cmd` 包装器，测试统一 `-j 8`；不要与 `build.ps1` 并发。
- 改了前端必须 `npm run build -w web`；交付/试用 exe 前跑一次完整 `.\build.ps1`。
- 实测重跑用 `D:\kedai-bench-run\`（`provider.key` + `data-v41`），命令与探针见该目录；重跑前把 `data-v41` 里那 12 个垃圾文件留档为 D1 的证据。

## 9. 收口

- 三个提交各自跑完门禁并留档（`功能-变更史.md` 收口章），`遗留.md` 登记 D1~D8 的新增项与状态。
- 文档侧：`功能.md`（scratch 工作区契约、步骤墙钟预算、任务级空闲自愈）、`契约-协议与配置.md`（新设置项、bash 结果头、规划器能力段）、`经验.md`（本轮三个采坑：无工作区时模型往数据目录写文件、cmd 与 bash 语法错配、客户端放弃≠任务停止）。
- 收口后删本文件；重跑 12 组实测做前后对比，把对照值写进 `功能-变更史.md`。

## 10. 复核结论与口径裁定（2026-09-26 实施前，逐条 grep/读码核实）

**用户裁定**：先做**提交 0+1** 后停下汇报，提交 2/3 下一轮；步骤墙钟预算默认 **1200 秒**（0=关）；提交 0+1 落地后跑**最小两组实测**。

**复核结论（本文档行号多为旧值，实施以本节为准）**

1. `api/state` 不存在（实为 `api/app_state.rs`）；`TaskService::new`（app_state.rs:351，全仓唯一调用点）**不接收 data_dir/config** → scratch 根要新增构造参数 + 字段（测试走 `lib.rs::build_test_app`，无需逐改）。
2. `scope_for_task` 定义在 `tools/workspace_guard.rs:116-130`（未绑定→`Ok(None)`、目录失效→`Err` 不降级、jail 恒 true），调用点 `task_engine/mod.rs:186`（183-197 块）。
3. **legacy 的步骤无工具**（`legacy.rs:97` → `generate_step_retry` → generate_text），plan/team/custom 的步骤才有工具（`run_agent_loop`）→「可用能力」提示段按模式分两套文案，不能统一按策略编译结果一刀切。
4. 侦察轮 `plan_scout_loop`（`task_service/executor.rs:796-923`）硬编码 `scope: None`；白名单 `PLANNER_SCOUT_TOOLS = READONLY_SCOUT`（`task_service/mod.rs:64-68`）；调用链 `plan_task`/`plan_revise`（executor.rs:703/733）← `plan_task_retry`/`plan_revise_retry`（`task_engine/retry.rs:128/175`）← `legacy.rs:53`、`plan.rs:227`（两处调用方都已有 `ctx.scope`）→ 放开 `fs_read/fs_glob/fs_grep` 要改这条签名链。
5. `exec_audit` **无 `risk_flag` 列** → 需加列：`models/db/schema.rs:221-236` + `migration/ddl.rs:643-669`（DDL/ensure/事务登记）+ `services/exec/audit.rs`（`ExecAuditEntry`/`AuditRecord`/INSERT/SELECT）+ `tests/schema_migration_meta.rs` 元测试 + `check-doc-claims` 两处计数（事务内 ensure_* 14→15、ddl.rs 定义 16→17）与文档同步。原文低估了这一项。
6. `bash.rs`：`resolve_cwd` `:88-107`，`(None,None)→data_dir`（`:105`）保持不变（只服务聊天路径）；`format_result` `:248-253` 拿不到实际 cwd，需改签名（description 在 `:35-39`）。
7. `settings_connector` 竞争真实机理：`task_overlay_does_not_leak_into_roleplay_settings`（tests/settings_connector.rs:168-212）结尾把 task 覆盖层写成 `""`，而 `for_mode(Task)` 对 `Some("")` 是「显式清空」语义（不回退内置默认词，params.rs:528-535）→ 同 binary 的 `roleplay_default_prompt_visible_on_fresh_install`（:385-409）第二条断言（task 视角非空）按运行顺序红（约 50%）。**修法：该用例开头捕获 task 视角原值、结尾复位为原值**（不是写回默认词，也不是拆 binary）。
8. 顺带项：`docs/契约.md:952` 的 `TaskEventKind` 表缺 `flow_bound` 行（计数门禁只查数字，查不出）。

**口径裁定（偏离原文并说明理由）**

- **C1** 提交 0（settings_connector 竞争）独立成一个 commit：Git 纪律「一个逻辑批次一个提交」，且它是门禁可靠性的前置。
- **C2**（提交 3 用）语义熔断收紧改为「任务侧上限钳制 + 经 `GenerationParams` 显式传值」，**不新增覆盖层字段**：引擎读的是扁平值（`engine.settings_snapshot()`），改成按 `task:` 前缀嗅探模式会引入新耦合；若进覆盖层，设置面板上那三个扁平输入会变成对任务无效的假控件。上限常量落 `utils/loop_guard.rs`（窗口≤8 / 次数≤4 / 去重≤2，`0=关闭` 保持 0），任务侧 solo.rs / custom.rs 两处参数装配显式传入。
- **C3**（提交 2 用）D8-3 并入提交 2 且**只做 plan 侧**：`TaskBoard.vue:939-964` 已渲染 plan 步骤列表（含每步 result），`## 最终计划` 是真重复；team 的 `## 审计结论` **保留文本**——`final_conclusion` 只存在于 team.rs:650-712 的局部变量，无结构化副本，彻底分离要为观感项新增 `tasks.audit_conclusion` 列（约 10 文件），收益不抵成本；理由写进 `遗留.md`。
- **C4**（提交 3 用）新设置项收敛为 2 个：`task_step_budget_secs`（**默认 1200=开**，0=关）、`task_idle_timeout_secs`（默认 900；最坏合法静默 = bash 单命令 300s 上限 + 单次模型调用 300s 上限 = 600s，取 600 会误杀）。均扁平字段、任务侧消费、全链路含设置面板 UI。
- **C5**（提交 1 实施）「可用能力」段两套文案：legacy = 无工具，交付物必须是正文、禁止规划成文件；plan/team = 有文件工具 + shell + 工作区根路径。
- **C6**（提交 1 实施）`EXECUTOR_PROMPT` 补一行「交付物正文必须完整给出，写入文件不替代正文」——提交 1 之后所有任务都会拿到 `fs_*`，新增「成果只落文件里」的风险；该行与提交 3 的编码纪律段同源，不写第二份。

**实测回归判据（提交 0+1 落地后，最小两组）**

- solo/编程（`verify-prog.mjs` 14 条断言）：断言 14-14 不回退；`exec_audit` 中 `;` 分隔、`/d/` 路径、`md5sum`、`git status` 一类语法失败显著下降（D4）；侦察轮不再出现「找不到路径」（D5）。
- plan/写作（基线 77 条命令 / 12 个垃圾文件 / 未收敛）：`data-v41` **零新增垃圾文件**；交付物落在 `task_scratch\<task_id>\` 或正文；不再反复 `chcp`/python 重试写文件（D1）。
- 跑不了（额度/环境不可用）→ 如实登记「本批未跑」并写明原因，不伪报。D2/D3 前后对照推到提交 2/3 那轮。

## 11. 本轮（提交 0+1）执行结果（2026-09-26 收口）

**提交**：`4e8346a`（提交 0，门禁竞争）+ `590af4a`（提交 1，D1/D4/D5）。

**自动化门禁（全绿）**：cargo test 全量 42 个测试二进制 0 失败（1436 = 1111 单测 + 325 集成）；
`npm test -w web` 113 文件 / 1206 用例；`npm run check` 15 段全绿。

**真模型实测回归（最小两组 + 一组补验）**

| 组 | 基线（修复前） | 本轮 |
|---|---|---|
| solo/编程 | 未收敛（903s 截断）/ 14-14 OK | **done / 215.4s**，`verify-prog` PASS（14/14、test 未改、导出完整） |
| plan/写作 | 未收敛（901s 截断）/ 无正文 / 数据目录 12 个垃圾文件 | **partial / 634.7s** / `data-v41` 零新增 / 交付物落 `task_scratch/<id>/` |
| plan/编程（补验 D5） | 规划侦察报「读取文件 package.json 失败」/ 903s 截断 | **done / 450.2s**，`verify-prog` PASS；planner 经 `fs_read` 读到带行号的工作区源码 |

命令面：三轮新审计行共 14 条（基线单轮 77 条），`;`/`/d/`/`md5sum` 零出现，`risk_flag` 全为空。
证据留档 `D:/kedai-bench-run/d1-evidence/`；逐条叙述见 `docs/功能-变更史.md` 的「2026-09-26 任务模式实测缺陷修复 · 提交 0+1」章。

**与本稿的偏离**（复核后必要修正，已并入 §10 口径裁定）：C1 提交 0 独立；C3 报告把 D8-3 拆到提交 2/3；
C2/C4 为提交 3 的既定口径（尚未实施）。**额外**：`tools/bench/task-probe.mjs` 的 D8-2 修法在下一轮实施时，
应先看本机 `D:/kedai-bench-run/probe.mjs`——它已实现「超时主动 stop + 记 `timed_out`」，
值得直接移植回仓库版，而不是另写一份。

**下一轮起点**：提交 2（部分成果兜底：`task_core::assemble_from_plan` + 四模式降级 + `finalize_run`
的 ended 兜底 + 前端成果卡放宽 + plan 去 `## 最终计划` 标记）→ 提交 3（步骤墙钟预算默认 **1200s**、
语义熔断任务侧钳制、编码纪律提示段、`finish=length` 报错、空闲自愈、探针修）。

## 12. 提交 2 详细执行稿（不丢成果 · D2 + D8-3 plan 侧，2026-09-26）

> 基线：HEAD `590af4a`（提交 0+1 已入库）。本节是**执行与验收两用稿**：除动作清单外，
> 还带「在飞实现对照」与「审查要点」，可直接当作那批改动的验收清单。
>
> ⚠️ **并发提示（2026-09-26 22:41 核实，22:5x 更新为「归属已定」）**：本工作区**另有一个会话正在实现提交 2**
> （21:21~22:23 的**未提交**改动：新增 `task_core/assemble.rs`，改四模式 + `task_service/{db,executor}.rs`
> + `tests/{tasks,task_events}.rs` + `web/src/{components/TaskBoard.vue,taskResult.ts,style.css}` +
> 四份 docs，另有一份 `COMPUTER-USE-REPORT.md`）。
> **归属已定（本会话处理）**：该在飞实现已按 §12.4 逐条验收、并提交为 `2bd144c`（代码+测试+计数）
> 与 `d215c69`（文档收口）；实现要点与 §12.1/§12.2/§12.3 的对照见各表「在飞实现对照」列，
> 验收结论与实测数值见文末 §12.7。**不必重做**；`COMPUTER-USE-REPORT.md` 属另一条线的产出，未纳入本批提交。
> 若后续会话要继续改提交 2 的代码，请从 `d215c69` 起（工作区已干净）。

### 12.1 目标与判据（D2）

任何终态下，「已完成且产出非空」的步骤内容都必须能被用户看到，且状态诚实：

- 汇总/审计调用失败但已有产出 → 终态 `partial`（`result` = 拼装文本、`error` 含失败原因）；
- 取消（stop）或引擎 Err 兜底时 result 仍为空 → 从库里的 `plan` 拼装补写 result，
  **状态与错误文本一字不改**；
- 一步都没完成 → 维持原失败语义（`error`/`ended` + 空 result），**不得伪造**
  （空串、占位文本、未完成步骤的内容都不算成果）；
- 前端：`error`/`ended` 且 result 非空 → 渲染成果卡 + 标注「任务未完成，以下为已完成部分的成果」；
  `pending/planning/running/planned` 沿用批次 R1 门控（防止把批准前的计划清单当成果展示）。

### 12.2 动作清单（文件级，以符号引用为准）

| # | 动作 | 落点 | 要点 | 在飞实现对照 |
|---|---|---|---|---|
| 2.1 | 确定性拼装器（新文件，单一出处） | `services/task_core/assemble.rs` + `task_core/mod.rs` 再导出 | `assemble_from_plan(&[TaskStep]) -> Option<String>`：只拼「## 名称 + 正文」，纯函数、不调 LLM、不读库；无可用步骤返回 `None`；另收口一处判定助手 `fallback_terminal(plan, reason) -> TaskTerminal`（有成果→`Complete{Partial}`、无成果→`Failed`） | ✅ 已存在，含纪律注释、正反两面单测 |
| 2.2 | 四模式失败降级 | `task_engine/{legacy,plan,team,custom}.rs` | 汇总/审计/终审失败且能拼出产出 → 经 `fallback_terminal` 走 partial；无可拼产出 → 原 `Failed`；取消保持 `Err`（终态由引擎按 ended 收尾并从库里 plan 兜底） | ✅ 四模式均已接；team 的审计与终审两条失败路径也覆盖（超出原稿范围） |
| 2.3 | 终态兜底写 result | `task_service/executor.rs` 新增 `salvage_partial_result`（`finalize_run` 与 `stop()` 各调一次）+ `db.rs` 新增 `set_result_only` | 判据「result 为空才补」→ 天然幂等；只写 result 列，status/error 由既有写入决定；写库成功后发既有 `status` 事件（**不引新 kind**） | ✅ 已实现，并顺带覆盖 `recover_orphan_tasks` 的重启孤儿兜底 |
| 2.4 | 批准时清掉 planned 态预览 result | `executor.rs` 的 approve 分支 + `db.rs::clear_result` | planned 态 result 是「待批准计划清单」，留着会让 2.3 的判据整条跳过（2026-09-26 实测命中：某步已完成 560 字，成果卡里仍是计划清单）；只清 result 列、不发事件（随后 status/plan 写入各自发事件） | ✅ 已实现（原稿未预见，属必要补充，登记为口径增强） |
| 2.5 | D8-3 plan 侧：result 只放成果 | `plan.rs`（删 `format_final_plan_section` 与「## 最终计划」拼接）、前端 `taskResult.ts` | plan 续跑完成 result = 汇总文本；各步名称/状态/产出由 `TaskBoard` 既有「计划步骤」区按 `task.plan` 渲染，同一份信息不再两处存 | ✅ 后端已删；前端**有意保留** `FINAL_PLAN_MARK` 拆段，只为兼容库里旧任务（配套一条向后兼容测试）——本稿接受该偏离 |
| 2.6 | 前端成果卡放宽 | `TaskBoard.vue`（`resultFinal` / `resultIncompleteNote` / 模板注释）、`style.css` | `error`/`ended` 且 result 非空 → 渲染 + 标注；过渡态门控逐字保留 | ✅ 已实现 |
| 2.7 | 文档收口 | `功能.md`、`契约-协议与配置.md`、`契约.md`、`经验.md`、`遗留.md`、`功能-变更史.md`、`MAINTENANCE.md` | D2 的终态语义与「不伪造」纪律、assemble 单一出处、前端门控口径；测试计数用 `node tools/count-tests.mjs` 现取值 | ✅ 已改（`经验.md` 另增 E58「三层各让一步」采坑条目） |

### 12.3 先写的失败测试（本稿口径 ↔ 在飞实现的用例名）

| 本稿要求的用例 | 在飞实现对照（`server-rs/tests/tasks.rs`） |
|---|---|
| ① legacy 汇总失败 → `partial`、result 含各步产出、error 含原因 | `task_summary_failure_falls_back_to_step_outputs` |
| ② 一步都没成 → 仍 `error` 且 result 为空（不伪造） | `task_summary_failure_without_step_outputs_stays_error` |
| ③ stop 时有 done 步骤 → `ended` 且 result 非空 | `task_stop_running_keeps_done_step_output` |
| ④ plan 续跑 result 不含「## 最终计划」 | `task_plan_resume_result_is_summary_only` |
| ④′ plan 续跑：汇总失败 / 含 error 步骤 / 取消三条 | `task_plan_resume_summary_failure_falls_back_to_step_outputs`、`task_plan_resume_error_step_marks_partial_in_plan`、`task_plan_resume_stop_keeps_done_step_output` |
| ⑤ team 汇总失败 → 各主产出 | `task_team_summary_failure_falls_back_to_main_outputs` |
| ⑥ 前端四例（error/ended 渲染并标注、error 空 result 不渲染、planned 门控未放宽） | `web/src/components/taskModeUi.test.ts` 新增 `describe` 的四个 `it` + 一条「旧任务带「## 最终计划」仍拆卡」向后兼容用例 |

拼装器与判定助手的**单元**正反两面见 `task_core/assemble.rs` 的 `#[cfg(test)]`。

### 12.4 审查要点（验收在飞实现时逐条确认）

1. **不伪造**：`assemble_from_plan` 只收「`Done` 且 result 非空」的步骤；无可用步骤即 `None`，
   终态与空 result 与旧行为逐字一致（已由正反用例钉住）；
2. **幂等**：`salvage_partial_result` 以「result 非空即跳过」为判据——`stop()` 与执行器收尾
   两次调用只写一次、只发一次事件；
3. **不串状态**：`set_result_only` / `clear_result` 只动 result 列；`status`/`error` 仍由
   `set_status`/`set_error`/`set_result_with_error` 决定（模式收尾路径不变）；
4. **旧数据兼容**：`## 最终计划` 拆段保留仅为存量旧任务；team 的 `## 审计结论` 照旧保留
   （§10 C3 的裁定不变）；
5. **重启孤儿**：`recover_orphan_tasks` 的兜底同走 assemble、同样不伪造（顺带覆盖，非本稿要求）；
6. **口径一致**：`经验.md` E58 与 `功能.md` 的终态表述不得互相矛盾；测试计数用现取值
   （`node tools/count-tests.mjs` 的 `--check` 必须过）。

### 12.5 验证（与提交 1 同规格）

```
cargo fmt                                     # 改完先格式化(否则 fmt --check 段红)
MSYS_NO_PATHCONV=1 cmd /c "tools\cargo-vcvars.cmd cargo test -j 8"
npm test -w web
node tools/count-tests.mjs                    # 取真值更新 MAINTENANCE.md / README.md
npm run check                                 # 全量 15 段
```

- **实测回归（提交 2 的验收值）**：用 `D:/kedai-bench-run/`（`provider.key` + `data-v41`，
  起服务见 `run-server-d1.cmd` 的教训：**环境变量要直接给 exe，别写带中文注释的 .cmd**）
  重跑 **legacy/写作**，期望从基线的 `error / 478s / 结果 0 字` 变为
  `partial / ≈3900 字`（拼装文本）。跑不了（额度/环境不可用）就如实登记「本批未跑」，不伪报。
- 提交信息体例：`fix(任务模式): 不丢成果——部分成果兜底落库 + 结果卡放宽 + plan 去「最终计划」段`，
  正文写清「改了什么 / 为何这么改 / 验证证据（命令与实测数值）」。

### 12.6 与 §10 口径的关系

- C3（D8-3 只做 plan 侧、team 审计段保留）**不变**；前端保留旧标记拆段属 C3 的兼容补充；
- C2/C4（提交 3 的步骤预算 1200s、语义熔断钳制）不受本批影响；
- 本批不碰角色扮演主链路，不动六模式编排语义（只加失败降级与交付拼接）。

### 12.7 验收与实测结果（2026-09-26 收口，提交 `2bd144c` + `d215c69`）

**§12.4 六条审查要点逐条确认**：①不伪造——正反用例 `task_summary_failure_falls_back_to_step_outputs`
/ `..._without_step_outputs_stays_error` 双向钉住；②幂等——`salvage_partial_result` 以「result 非空即跳过」
为判据，`stop()` 与执行器收尾两次调用只写一次；③不串状态——`set_result_only`/`clear_result` 只动
result 列（`status`/`error` 仍由既有写入决定）；④旧数据兼容——`taskResult.ts` 的 `FINAL_PLAN_MARK`
拆段保留并改注为「存量旧任务兼容」；⑤重启孤儿——`recover_orphan_tasks` 同走 assemble、单测扩三态
（有产出补写 / 无产出不伪造 / 已有 result 不覆盖）；⑥口径一致——`经验.md` E58 与 `功能.md` §十四
表述同源，`count-tests`/`check-doc-claims`/`check-arch` 同步后全绿。

**自动化门禁**：`npm run check` 全量 **15 段全绿**；后端 42 个测试二进制 **1431 通过 0 失败**
（静态 1443 = 1113 单测 + 330 集成 / 38 文件）；前端 **113 文件 / 1210 用例**（静态 1192）。

**真模型实测**（`D:/kedai-bench-run/`，commandcode 网关 + `deepseek/deepseek-v4.1-flash`，未绑定工作区）：

| 组 | 基线（12 轮实测 / 提交 1 轮） | 本轮 |
|---|---|---|
| legacy/写作 | error / 478s / 结果 0 字 | **done / 116.3s / 560 字**（本轮汇总成功，未走降级路径，如实记录）；`data-v41` 零新增 |
| 取消路径（补测 D2 核心判据） | 旧行为：取消连 result 都不写 | 首步 done、第 2 步 running 时 stop → **ended + result 1069 字**（已完成步骤拼装） |
| plan/写作（两轮） | 901s 截断 / 无正文 | 两轮均到点 stop → **ended**（provider 本轮劣化：4~5 次调用里 3 次撞 300s 看门狗）；result **不含**「## 最终计划」段、无假成果、`data-v41` 零新增 |

**本轮新增的口径增强（原稿未预见，已在 §12.2 #2.4 登记）**：`approve` 置 planning 时清空 planned 态的
预览 result（`db::clear_result`）——真模型 plan/写作 实测发现续跑期间 result 一直非空（计划清单），
使「result 为空才兜底」整条跳过、成果卡把计划当成成果；用 `task_plan_resume_stop_keeps_done_step_output`
锁定。**另**：`tests/task_events.rs` 两处瞬时事件断言改取并集（broadcast `Lagged` 允许丢，实测约 1/3
概率误红；**暂存本批 src 改动的对照组同样复现**，确认为既有抖动，按经验 E38 顺带修并写进提交信息）。

**下一轮起点**：提交 3（步骤墙钟预算默认 **1200s**、语义熔断任务侧钳制、编码纪律提示段、
`finish=length` 可操作报错、任务级空闲自愈、`task-probe.mjs` 移植修复 → 见 §5 与 §10 的 C2/C4）。

## 13. 提交 3 详细执行稿（能收敛 · D3 / D6 / D7 + D8-2 探针）

> 基线：HEAD `d215c69`（提交 0/1/2 已入库）。口径来源：§5（动作清单）、§10 的 C2/C4（用户裁定）、
> §12.7 的下一轮起点。行号为本会话 2026-09-26 晚逐处复核；标「探查」的来自三个只读子代理的
> 全仓扫描（**实施前按 `docs/经验.md` 条目 2 再 grep 一次**）。状态：**待实施**。

### 13.0 前置：口径与边界

- **C2（语义熔断收紧）**：不新增覆盖层字段、不做 `task:` 前缀嗅探（引擎读的是**扁平**快照，
  `agents/engine/executor.rs:601-610`；嗅探会让设置面板上那三个扁平输入对任务变成假控件）。
  改为：钳制逻辑落 `utils/loop_guard.rs` 的**纯函数**（窗口 ≤8 / 次数 ≤4 / 去重 ≤2），
  任务侧装配 `GenerationParams` 时**显式传值**；`0=关闭` 的输入保持 0。聊天路径行为逐字节不变。
- **C4（两个新设置项）**：均**扁平字段、任务侧消费**、全链路含设置面板 UI：
  `task_step_budget_secs`（**默认 1200 = 开**，0 = 关）、`task_idle_timeout_secs`（默认 900，0 = 关，
  下限见 §13.5.4）。
- **与 HARNESS-PLAN 的合并（§8 前置条件）**：「编码纪律提示段」= 该稿提交 3 第 6 项的**同源部分**
  ——本批只做「内置提示段常量 + 注入点」这一半（单一出处 `task_core/prompt_consts.rs`，
  HARNESS 线将来直接引用，不写第二份）；任务级 `agent_system_prompt` 编码模板与
  `src/bin/kedai-agent.rs` headless 入口**不在本批**。
- **本批不需要**：DDL/迁移（无 schema 变更）、新事件 kind（空闲收尾复用 `status`）、新依赖。
  两个新设置字段**没有机检守护**（`tools/` 下无「设置字段数」断言，探查结论）→ 靠 §13.3 清单人工核对。

### 13.1 目标与判据（可机判）

| # | 判据 | 判据形式 |
|---|---|---|
| D3-a | **整步墙钟预算**：单次工具循环超过 `task_step_budget_secs` → **带着已有产出收尾**，不制造失败；事件文案含「墙钟预算用尽」 | 集成用例（sleep 工具拉长轮次 + 预算 1~2s） |
| D3-b | **语义熔断任务侧收紧**：同工具 ≤8 轮窗口内 ≥4 次调用且输出实质种类 ≤2 → 熔断；对照：聊天侧同设置不熔断 | 纯函数单测 + 集成用例（`[[tool_loop_repeat:…]]`） |
| D3-c | **编码纪律段**：工具档执行者 system 含纪律句；legacy（无工具）档**不含** | 常量存在性 + 两条注入断言（防回退） |
| D6 | **`finish=length` 空产出可操作报错**：文案含 `finish_reason`、reasoning 占比与「提高输出上限 / 改非推理模型」建议 | 集成用例断言文案片段 |
| D7 | **任务级空闲自愈**：running/planning 且最近活动早于阈值 → 与 `stop` 同源收尾，终态 `ended` 且原因可见（`error` 文本 + 事件 detail） | 专门测试二进制（自建 AppState 直调 `scan_idle_tasks`）+ 纯判据单测 |
| D8-2 | **探针修**：题面写进 `title`、`/run` 不带 body、到点/终态 `POST /stop`、结果 JSON 含 `timed_out` | 脚本自查 + 实测回归时顺带验证 |

### 13.2 动作清单

| # | 动作 | 落点 | 要点 |
|---|---|---|---|
| 3.1 | `GenerationParams` 增两字段 | `models/types.rs:501-520` | `step_budget: Option<Duration>`、`semantic_guard: Option<(usize,usize,usize)>`。该结构**无 Default / 无 Serialize**（注释已声明「新增字段不动线格式」）→ 安全。**全仓 22 处构造点由编译器点名**，绝大多数补 `None`，只有任务侧两处传真值（3.4） |
| 3.2 | 引擎：墙钟闸门 + 语义参数覆盖 | `agents/engine/executor.rs` | ① `run_tool_loop` 入口记 `loop_started = Instant::now()`（`:586` 行 `max_rounds` 附近）；② 轮末闸门与 HB-1 token 闸门**并列同构**（token 闸门 `:1086-1138`、轮次上限 `:1140-1165`）——置于 token 闸门之后、`last_round` 之前（时间/成本闸门优先于轮数）；到点发 `step_evt("步骤墙钟预算用尽", …)` 并 `return Ok(ExecutorResult{ content, …, budget_stopped: true })`（照抄 `:1155-1164` 的字段形状；`budget_stopped` 复用既有「预算用尽」语义，聊天路径不可能触发）；③ 语义三值改 `params.semantic_guard.unwrap_or(设置快照)`（`:601-618`，快照读取保留为兜底） |
| 3.3 | 钳制纯函数 | `utils/loop_guard.rs` | 三个上限常量（`TASK_MAX_SEMANTIC_WINDOW / _MIN_CALLS / _MAX_DISTINCT` = 8 / 4 / 2）+ `pub fn clamp_for_task(window, min_calls, max_distinct) -> (usize, usize, usize)`：`min_calls == 0` 保持 0，其余 `min(v, 上限)` |
| 3.4 | 任务侧装配两处 | `services/task_engine/solo.rs:99-111`（`run_agent_loop`：solo/multi/team/plan/followup 共用）与 `services/task_engine/custom.rs:338-357`（工具步骤） | 两处都算：`step_budget = (settings.task_step_budget_secs > 0).then(|| Duration::from_secs(..))`；`semantic_guard = Some(clamp_for_task(settings.loop_guard_semantic_*))`。**映射抽成纯函数**（如 `task_loop_limits(&RuntimeSettings) -> (Option<Duration>, Option<(usize,usize,usize)>)`）供单测。legacy 与纯生成路径（`task_service/executor.rs:482`）不动 |
| 3.5 | 编码纪律段 | `task_core/prompt_consts.rs` 新常量 + `task_service/prompt.rs:68-146` 注入 | 常量如 `EXECUTOR_TOOL_DISCIPLINE`（自测通过即收尾、不得重复验证同一事实；命令用本机 shell 语法（Windows=cmd，多命令 `&&`）；改文件优先 `fs_write`/`fs_edit` 而非 `echo >`；每轮只做一个动作）。注入条件 = **该执行者本轮有工具**：`TaskPromptKit::assemble_executor_system_prompt`（`task_core/backend.rs:135` 声明 / `prompt.rs:68` 实现 / `backend_impl.rs:124` 桥）签名加 `has_tools: bool`——`solo.rs:72` 传 true、`executor.rs:1036`（legacy 步骤）传 false；`custom.rs::build_step_system`（`:234-262`）按档位追加（严格档不下发工具 → 不追加）。预览层（`api/settings.rs:1175` 附近）若复用该函数同批传参（探查） |
| 3.6 | `finish=length` 文案 | `task_engine/retry.rs:87-90`、`task_engine/solo.rs:208-209`、`task_engine/custom.rs:1115-1120` | 抽共享助手（`prompt_consts` 或 `task_core::types` 旁）产出统一文案：`"{label}返回空内容(finish_reason=length,思考占输出 {p}%:{r}/{c} token;当前输出上限 {max_tokens})。若反复出现:提高单次生成上限或改用非推理模型"`。数据齐备：`TaskGenOutput{finish_reason, reasoning_tokens}`（`task_core/types.rs:26-39`），`task_llm_calls` 已有 `reasoning_tokens/finish_reason` 列（**不改表**）；custom 那处当前**缺 finish_reason**（漏点），一并补齐 |
| 3.7 | 空闲自愈 | 新 `services/task_service/idle.rs`（或并入 db.rs/executor.rs）+ `run_server` 挂载 | ① 纯判据 `is_idle(status, cancelled, last_activity, now, timeout_secs) -> bool`；② 候选查询：`status ∈ {running, planning}` 的任务取 `MAX(task_llm_calls.created_at)`，**无调用行回退 `tasks.updated_at`**（`now_iso()` = UTC 毫秒 ISO，`chrono` 已在依赖 `Cargo.toml:41`，用 `parse_from_rfc3339`）；③ 收尾动作：把现有 `stop()`（`executor.rs:391-406`）抽成 `finish_ended(&self, id, reason: Option<&str>)`，`stop` 委托之、看守传 reason（写 `error` 列需新增 `db::set_error_only`，与 `set_result_only` 同形，**不动 status 语义**）；④ `pub fn scan_idle_tasks(&self, timeout_secs: u64) -> Vec<String>`（返回被收尾 id，便于断言）+ `pub fn spawn_idle_watchdog(self: &Arc<Self>, tick: Duration)`（tick 60s，每轮读设置；`0` = 关）；挂载点 `lib.rs:117`（`state.start_mcp().await;` 之后一行）——**刻意不挂 `build_test_app`**，避免给每个测试 app 挂常驻定时器 |
| 3.8 | 探针 D8-2 | `tools/bench/task-probe.mjs` | 按 `D:/kedai-bench-run/probe.mjs` 移植：题面进 `title`（现状 `:104` 是时间戳名、真题面在 `:122-125` 的 `{input}` 里被 handler 静默忽略）、`/run` 不带 body、到点或终态 `POST /stop`、结果 JSON 落文件并记 `timed_out` |
| 3.9 | 文档与计数 | §13.6 | 功能.md（任务侧四道闸：轮次 / 预算 / 熔断 / 空闲）、契约-协议与配置.md（两个设置项 × 2 张表 + 任务侧专节 + 顺手修 `:377` 的提示词锚点漂移，探查）、遗留.md（TM-D3 / TM-D6 / TM-D7 状态）、变更史新章、MAINTENANCE 计数 |

### 13.3 新增扁平设置字段的登记清单（13 处，**无机检守护 → 人工逐处核对**，行号来自探查）

**取值区间（先定死，校验与测试都依赖它）**：
- `task_step_budget_secs`：`0`（关）或 `1..=86400`。**不设下限**——`1` 合法是刻意的：低于单次调用
  300s 看门狗的值等于「第一轮结束就收尾」，属合法的「别用工具循环」语义；文档/UI 注「建议 ≥300」。
  测试要用 `1s` 触发预算路径，故不能设 60s 之类的下限。
- `task_idle_timeout_secs`：`0`（关）或 `601..=86400`（下限依据 §13.5.4）；真机验证 D7 不能用小阈值，
  改走测试缝直调 `scan_idle_tasks(timeout)`（§13.4 第 7 条）。

| # | 位置 | 样板 |
|---|---|---|
| 1 | `services/settings_service/mod.rs:79` `RuntimeSettings` 字段 | `:172-173` `#[serde(default = "default_…")] pub …: u32` |
| 2 | 同文件 `use super::params::{…}` 导入清单 `:38-50` | 新 `default_*` 名字必须进这里 |
| 3 | `services/settings_service/params.rs:161-215` `default_*()` | `:179-181` `default_tool_history_keep_rounds()` |
| 4 | 同文件 `from_config` 的 `RuntimeSettings { … }` 字面量 `:458-467` | `:461` |
| 5 | `services/settings_service/secret.rs` load 侧钳制（`:25` 内联）+ 第二份 `default_*` 导入（`:9-19`） | `:133-137`（1..=32）、`:163-166`（4..=64）；语义 = 越界**静默回退默认** |
| 6 | `api/settings.rs:41` `UpdateSettingsBody` | `:205`（keep_rounds）、`:217`（window） |
| 7 | 同文件 GET `settings_json()` 第二段 `:296-326` | `:316` / `:320`（段已很长，新字段插第二段） |
| 8 | 同文件 PUT 应用 + 校验 `:373-950` | 扁平直写样板 `:812` / `:839`（**不要**用 `apply!`）；校验取 400 拒绝（`:836-837`）或越界忽略（`:723-725`） |
| 9 | `web/src/api/types.ts:356` `RuntimeSettings` | `:401-408`（注释标「任务侧设置」） |
| 10 | 同文件 `RuntimeSettingsPatch:512` | `:546-560` |
| 11 | `web/src/stores/genSettings.ts` ref + JSDoc（`:40-56`）、load 回填（`:147-156`）、save 回填（`:212-219`）、return 导出（`:362-407`） | `ref(默认值)` 必须与后端缺省逐字一致 |
| 12 | `web/src/components/settings/GenParamsSection.vue`（`:107-149` 之后插入两位 `<div class="sv-inp-row">`，`<span class="sv-note">任务侧:…</span>`）+ `composables/useDataManager.ts:125-166` `saveParamsNow` 提交 patch | 现有三个任务侧输入即样板 |
| 13 | `docs/契约-协议与配置.md`：§一 回退规则表 `:383-400`、§五 设置字段表 `:704-724`、任务侧设置项专节 `:1195-1205` | 三处都要落行（含默认值与范围） |

### 13.4 先写的失败测试

1. `loop_guard::clamp_for_task`：三值上限钳制、`0` 保持关闭、窗口/去重的 `max(1)` 边界（照 `semantic_guard_disabled_by_zero_min_calls:421` 的写法）。
2. `task_loop_limits(&RuntimeSettings)` 纯映射：`1200 → Some(1200s)`、`0 → None`；三值进钳制（含用户把 window 设 64 仍得 8）。
3. **墙钟预算集成用例**（`tests/api_integration.rs` 或 `tests/tasks.rs`，需 `test_lock()` 串行）：`PUT` 设 `task_step_budget_secs: 1`；标题 `[[tool_loop_text:sleep|6 {"ms":1200}]]`（每轮真睡 1.2s，`[[tool_loop_text:]]` 保证循环被收掉时仍有正文落库）→ 断言：终态 `done`（**不是** error）、`result` 非空、事件 detail 含「墙钟预算用尽」。sleep 工具确凿可用：`{"ms": 1..=60000}` 真睡（`tools/agent_tools_agent.rs:685-701`），风险级 Sensitive → 任务策略内。
4. **语义钳制集成用例**：`PUT` 把三值设成宽松（window 64 / min_calls 12 / max_distinct 4）→ 任务标题 `[[tool_loop_repeat:read|6 …]]`（参数逐字相同）→ 断言「同工具调用次数 ≤ 5 即熔断」（对照：不钳制时为 12 才熔断）；同时断言终态正常收尾而非 error。
5. **纪律段两条断言**：`prompt_consts` 常量存在（含关键词）；工具档 system 含纪律句、legacy 档不含（照 `prompt.rs:320-349` 的回归锁写法）。
6. **`finish=length` 文案**：改写既有 `task_step_empty_output_retries_then_errors`（`tests/tasks.rs:340`）或新增一例，断言 `error`/步骤 result 含「思考占输出」「提高单次生成上限」；并补一条 custom 漏点的断言。
7. **空闲自愈**（新 `tests/task_idle_watchdog.rs`，独立进程 = 独立 DATA_DIR，避免与 `tests/tasks.rs` 的进程级共享 app 冲突）：
   - 直插一行 `status='running'` + 旧 `updated_at` 的任务 → `state.tasks.scan_idle_tasks(60)` 返回该 id、状态 `ended`、`error` 含「空闲超时自动收尾」；`AppState` 的 `new`/`tasks` 与 `api::build_router` 均 `pub`（已核）→ 测试可自建 app 实例。
   - 反面：`updated_at` 很新 → 不收尾；终态任务 → 不收尾；`timeout_secs` 边界（等于阈值算空闲？**取「严格大于」**，与轮次上限同族口径写进注释）。
   - 纯判据 `is_idle` 的组合表（状态 × 取消 × 时间 × 阈值边界）单测。
8. 探针无自动化测试（手工脚本）：验证 = 自查 diff + §13.6 的实测顺带跑一次。

### 13.5 关键技术裁定（附理由）

1. **预算计时范围 = 单次 `run_tool_loop`**（plan 每步、team 每主每子目标、custom 每节点各一份预算），**不做跨步累计**。理由：与既有 `max_tool_rounds` 同级同解；跨步累计要把剩余量经 `AgentLoopCall`/`TaskRunContext` 下传，与「每步独立调用」的现有语义冲突，且受益不明确。**重新评估的触发条件**：实测出现「单步都不超预算、整任务仍十几分钟」。
2. **不按 `task:` 前缀嗅探**（C2）：见 §13.0；`call_watchdog_for` 的前缀嗅探是**超时看门狗**的历史实现，不构成对本项的授权。
3. **钳制只收不放**：`min(v, 上限)`，`0=关` 保持 → 任务侧只会比聊天侧更早熔断，绝不把聊天侧的宽松值当许可。
4. **空闲阈值下限 601s**（0=关）：最坏合法静默 = bash 单命令 300s + 单次模型调用 300s = 600s，取 600 会误杀（C4 依据）；默认 900。**注意**：这同时意味着真机验证 D7 时不能把阈值调到秒级（除非走测试缝直调 `scan_idle_tasks`）。
5. **空闲收尾的状态与原因面**：终态 `ended`（与 stop 同源），原因写 `error` 列 + 事件 detail「空闲超时自动收尾:最近 {n}s 无模型调用」。`ended` + 非空 `error` 是提交 2 已确立口径的延伸（终态允许携带可解释原因）。
6. **`scan_idle_tasks` / `spawn_idle_watchdog` 为 `pub`**：这是**有意的测试缝**（`tests/` 是外部 crate，`pub(crate)` 不可达），`TaskService::stop` 已是 `pub` 先例；文档注释写明用途。

### 13.6 验证与实测回归

- 门禁：`cargo fmt`（改完先格式化）→ `tools/cargo-vcvars.cmd cargo test -j 8` → `npm test -w web` →
  `node tools/count-tests.mjs` 同步 `MAINTENANCE.md`/`README.md` → `npm run check` **全量 15 段**。
- 真模型实测（`D:/kedai-bench-run/`，commandcode 网关 + `deepseek/deepseek-v4.1-flash`；
  起服务用 `DATA_DIR=… LOG_DIR=… <debug exe>` 直给环境变量，启动后先看 `/api/health` 的 `data_dir`）：
  1. **solo/编程**（沙箱绑定 workspace，`verify-prog.mjs` 14 条断言）：期望在预算内 `done` 且断言不回退；
     **新增观察项**：`task_llm_calls` 里「改对之后的重复自检」条数应显著低于基线（基线：2 分钟改对后
     仍有 10 条反复自检、单轮往返 1.5~3 分钟）。对照值写进变更史。
  2. **预算路径验证**：临时把 `task_step_budget_secs` 调小（如 120）跑一轮 → 断言终态 `done`/`partial`、
     `result` 非空、事件含「墙钟预算用尽」（验证后复位 1200）——这是 D3-a 的真模型证据。
  3. **D7 观察**：起一轮任务后客户端放弃（探针提前退出、不再轮询）→ 看守在阈值后自动收尾；
     为控制时长可临时把 `task_idle_timeout_secs` 设为下限 601，或用测试缝的短阈值版本先证机制。
  4. 跑不了（额度/环境不可用）→ 如实登记「本批未跑」及原因，不伪报。

### 13.7 风险、边界与不做

- **不做**：跨步累计预算；`max_tool_rounds` 默认值变更；聊天路径任何行为变更；角色扮演链路；
  HARNESS 的 headless 入口 / 续跑 / 截断保尾 / 取消中止工具；新依赖。
- **已知不覆盖**（写进提交信息与遗留）：看守的 **tick 循环与 `run_server` 挂载**无自动化断言
  （决策与查询有单测、动作复用 `stop` 路径），靠一次实测观察 + 代码复核；设置字段的
  **前端漏加不会被门禁拦住**（§13.3 的 13 处靠人工核对，核对结果写进提交信息）。
- **顺序**：3.1 → 3.2/3.3 → 3.4 →（先红后绿）3.5/3.6 → 3.7 → 3.8 → 3.9 文档与计数。
- **提交粒度**：一个逻辑批次一个提交（建议：代码+测试+计数 一个、文档收口 一个），
  提交信息按仓库体例写清「改了什么 / 为何这么改 / 验证证据（命令与实测数值）」；
  完成并过全量门禁后再提交（`AGENTS.md` Git 纪律）。

## 14. 提交 3 执行结果与实测（2026-09-27 收口）

**自动化门禁**：`npm run check` **全量 16 段全绿**（fmt / clippy `-D warnings` / cargo test
--workspace / cargo+npm audit / docs / lock-sync / contract / arch / count / doc-claims /
type-ratchet / eslint / vue-tsc / vitest / vite build / bundle budget）。
后端 44 个测试二进制 **1450 通过 0 失败**；前端 vitest 运行时 **1213 / 113 文件**
（静态 `node tools/count-tests.mjs` = 1462 = 1124 单测 + 338 集成/40 文件；前端 1195）。

**真模型实测**（`D:/kedai-bench-run/`，commandcode 网关 + `deepseek/deepseek-v4.1-flash`）：

| 轮 | 设置 | 结果 |
|---|---|---|
| solo/编程 | 默认预算 1200s / 等 900s | 两轮均未收敛（provider 本轮劣化：单轮 LLM 往返 15~190s，任务持续做工具调用、无 300s 看门狗）；探针按新逻辑主动 stop → `ended`。**拿不到「预算内 done」对照值，如实登记** |
| solo/编程 | **预算临时 120s** | **done / 152.8s / result 65 字**；事件「步骤墙钟预算用尽:已用 150s / 预算 120s(第 16 轮)」；usage 90172+4854（D3-a 真模型证据） |
| D7 真机观察 | — | **未做**：601s 下限的设计前提是「最坏合法静默 600s < 下限」，即活着的任务不可能被判空闲；机制由 `tests/task_idle_watchdog.rs` 4 条双向覆盖 |

**与 §13 的口径偏离（实施中实测/复核驱动，均已并入代码与文档）**

1. **空闲判据加内存心跳**（§13.2 3.7 只写「看 `task_llm_calls` 行」）：该行是整轮工具循环结束后
   才落的，只看库会把正在干活的十分钟循环误杀。最终 = 心跳优先（`register_cancel` 登记、
   `emit_event`/`emit_llm_call` 刷新）+ 库时间戳兜底；双向用例钉住。见 `经验.md` E59。
2. **预算收尾回退最近非空正文**（新发现）：工具型模型常见「只发工具调用、不带正文」的轮，
   预算恰好停在这样一轮会返回空串 → 上游判「返回空内容」→ **error**，与「带着产出收尾」相反。
   实测第二轮正是该形态（`finish=tool_calls`）。加纯函数 `budget_stop_content`（四态单测）。
3. **语义熔断钳制与两条既有用例冲突**（口径不变、用例适配）：`task_custom_compare` 的
   「同一流程连调 10 次」与 `tasks` 的「同工具 6 轮测历史裁剪」都会在第 4 轮被新钳制先收掉
   （且工具轮无正文 → 误报空内容）。两例各自**显式关闭无关的那道闸**（`loop_guard_semantic_min_calls: 0`
   + 复位，与 api_integration 的 token 预算用例同款做法），闸门自身由 `tests/task_loop_budget.rs` 覆盖。
4. **`GenerationParams` 真构造点 18 处**（§13.2 写的 22 含 struct 定义与 3 处返回类型行）；
   `custom` 档位判定在 `run_node_attempt` 而非 `build_step_system`；设置页预览推常量而非组装函数
   ——三处已在实施中按实际结构落地（纪律段注入点选在 `run_node_attempt`，预览单推一层）。
5. **子 agent（agentgo）工具循环不套预算**：按用户裁定登记 `遗留.md` TM-X2。

**下一轮起点**：本计划（D1~D8）与 HARNESS/COMPUTER-USE/FRONTEND 三条线的后续见各自稿；
`剩余可做`：agentgo 子 agent 预算穿透（TM-X2）、scratch 目录的浏览/回收（TM-X1）、
12 组全量实测对照（本轮只跑最小两组）。
