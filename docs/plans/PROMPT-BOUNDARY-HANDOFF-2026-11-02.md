# 提示词双模式边界清理 · 交接文档（2026-11-02）

> **状态**：RTSPLIT-1/2（PR #86）、RTSPLIT-3/4（PR #88）**已合并**；本文档交接的是
> **排查已完成、实现未开工**的四项遗留。
> **基线**：`main` = `781cbe5`（PR #88 后；行号为此快照，动手前先复核——本仓行号漂移很快）
> **依据**：`docs/契约-协议与配置.md` 第三节（untrusted 包裹）、第五节（预览口径）、
> 「双模式提示词边界」全节；`docs/功能.md` 的 Agent 引擎章节。
> **性质**：这份文档回答一个问题——**「还有哪些跨模式共用但语义跑偏的东西，分别该怎么修」**。
> 结论来自三路并行调查 + 逐条人工复核，**已剔除误报**（见 §四，避免接手的人重复排查）。

---

## 〇、一句话交代背景

Kedai 是「通用 LLM 智能体 harness」，角色扮演与任务是**平级的双顶层模式**。两个模式共享大量
基础设施（`prompt_kit`、工具注册表、流程库、settings 覆盖层），共用的地方就容易把**一个模式的
语义泄漏到另一个**。

前两批已修掉两处最要命的：

| 批 | 问题 | 合并 |
|---|---|---|
| RTSPLIT-1/2（#86） | `AGENTS_RUNTIME.md` 单文件双模式共用，任务侧**完全不读**它 | `f405108` |
| RTSPLIT-3/4（#88） | `untrusted_boundary` 规则句双模式共用，「任务目标」被降级为「创作设定」 | `781cbe5` |

**剩下的四项按推荐顺序列在 §二**。其中 **R-1 是唯一会直接跑错内容的一项**，优先级最高。

---

## 一、动手前必做的基线核对

| 事实 | 当前值 | 权威来源 |
|---|---:|---|
| 后端测试 | **2168**（1679 单测 + 489 集成 / 84 文件） | `node tools/count-tests.mjs` |
| 前端测试 | **1839** 静态 / 1881 运行时 / 174 文件 | 同上 |
| 门禁阶段 | **19 段**（`-SkipRust` 时的段数） | `tools/check-all.ps1` |
| 契约映射 | **32 组** | `node tools/check-contract.mjs` |
| 当前 main | `781cbe5` | `git log --oneline -1` |

```bash
# 快速核对(在 D:\kedai;cargo 需先 call vcvars64.bat)
node tools/count-tests.mjs
node tools/check-contract.mjs
node tools/check-arch.mjs
cargo test --manifest-path server-rs/Cargo.toml -j 8
npm test -w web
```

**真实模型实测当前不可用**：上游 `api.commandcode.ai` 直连返回 **HTTP 401**
（`Invalid 'Authorization' header or token`），凭据在仓外 `D:\kedai-bench-run\realmodel.env`
的 `CMD_CODE_KEY`，2026-11-02 全天两次探测均 401。按仓库纪律，收口时若仍未恢复须如实登记
「确实不可测」，**不得以 mock 或确定性补证冒充真实模型验收**。

---

## 二、四项遗留（按推荐顺序）

### R-1 内置流程库把创作流程当成了任务的默认流程 ★最高优先

**为什么最优先**：这不是「提示词措辞不好」，而是**任务 custom 模式默认会跑一段创作流程**，
产出形态直接错。

**现状证据**（`781cbe5`）：

- `server-rs/src/services/agent_flow_service/library.rs:595-670` 的 `builtin_flow()`：
  - `id = "builtin-coordination"`、`name = "文学创作协调流程"`
  - 三步：`draft`（起草正文）/ `reflect`（反思质量）/ `revise`（修订输出）
  - 步骤提示词含「沿用系统提示词已确定的**人称锚定**与角色口吻」「**角色台词**保持第一人称口吻」
  - 步骤白名单 `tools: ["update_variables"]`
- `library.rs:80` + `:107-109`：全新安装 `current_flow_id = builtin-coordination`（**默认选中**）
- `server-rs/src/services/task_service/mod.rs:436-438`：
  ```rust
  let Some(flow_id) = task.and_then(|t| t.flow_id.clone()) else {
      return guard.current_snapshot(&extra_ids);   // ← 未绑定流程 = 用「当前流程」
  };
  ```
- `server-rs/src/services/task_engine/custom/step.rs::build_step_system` 把该流程的
  `system_prompt` 原样拼进任务 system

**三重语义错配**（每一重都独立成立）：

1. **人称锚定**：`pov_anchor` 只在聊天链注入（`agents/engine/messages/build.rs`），任务链
   **从不注入**——步骤里那句「沿用已确定的人称锚定」在任务侧是**悬空引用**。
2. **`update_variables` 工具**：任务侧 `tool_policy` 默认档（`deny_dangerous`）会剔除
   Dangerous 工具，`update_variables` 不在任务工具面——**点名的工具根本不下发**。
3. **`<UpdateVariable>` 协议**：任务侧不解析该文本协议，写了等于噪声。

**推荐修法**（三选一，按改动面从小到大）：

| 方案 | 做法 | 代价 |
|---|---|---|
| **A（推荐）** | `AgentFlowConfig` 加 `mode_scope: String`（`"all"` / `"roleplay"` / `"task"`，`#[serde(default)]` = `"all"`）→ `builtin-coordination` 标 `roleplay`；新增 `builtin-task-default`（任务向：调研→执行→核验）标 `task`；`resolve_task_flow` 未绑定分支校验 `mode_scope`，命中 roleplay 时改选首个 `task`/`all` 流程，找不到则返回**明确错误**（不静默跑创作流程）；`TaskFlowSelect.vue:34` 附近按 `mode_scope` 过滤 | 跨前后端 + 数据格式，需往返测试锁旧 `agent_flows.json` 兼容 |
| B | 只改 `resolve_task_flow`：未绑定时若当前流程是 `builtin-coordination` 则报错让用户选 | 最小，但用户仍能在选择器里选中创作流程 |
| C | 只在前端选择器过滤 | 最省事，但 API 直建任务仍能绕过——**不推荐单独用** |

**验收判据**：
- 全新安装建一个未绑定的 custom 任务 → 不应跑到「起草正文」流程（或明确报错）
- 旧 `agent_flows.json`（无 `mode_scope` 键）读入后行为不变（`serde(default)` = `"all"`）
- 前端任务模式的选择器不出现纯 roleplay 流程；聊天侧不出现纯 task 流程

**现有测试可参考**：`server-rs/tests/agent_flows.rs`、`server-rs/tests/task_custom_*.rs`（十余个文件）

---

### R-2 `ModeSettings` 里的「可写但任务侧永不消费」字段（静默死写）

**性质**：违反仓库自己的 TM-SET-3 纪律（「写覆盖层是『改了不生效』的静默死写」）。
不影响正确性，但会让用户「保存成功却没有效果」。

**字段清单**（`server-rs/src/services/settings_service/params.rs:60-135`，逐条 grep 确认任务链零引用）：

| 字段 | roleplay 消费点 | 任务侧 |
|---|---|---|
| `compaction_mode` / `_threshold` / `_keep_recent` / `_snip_bytes` | `agents/engine/compaction.rs`、`messages/context.rs` | **零** |
| `memory_distill_enabled` / `memory_inject_limit` / `memory_inject_char_budget` / `memory_max_entries` | `messages/context.rs` 记忆槽 | **零**（且 `memory_service` 读扁平值，绕过 `for_mode`） |
| `mvu_vars_position` | `messages/context.rs` | **零** |
| `reflect_prompt` / `reflect_advice_prompt` / `reflect_advice_role` | `reflector_integration.rs` | **零** |
| `preset_tail_prompt` / `preset_tail_role` | `build.rs` 位置 0 | **零** |
| `render_html` | 前端 `uiPrefs.ts`（后端零消费） | **零** |
| `llm_request_log` | `executor/generation.rs` → `llm_requests` 表 | **零**（任务用自己的 `task_llm_calls`） |
| `skill_progressive_disclosure` | `run_loop.rs` 【可用技能】清单 | **零** |
| `max_context_tokens` / `max_tool_rounds` / `bypass_mode` | 聊天 AGENT 路径 | **零**（文档已说明；`bypass_mode` 更是从未被任何写入点写过） |

**推荐修法**（两档，建议先做文档档）：

- **档一（零风险，建议随任意批次做）**：在 `docs/契约-协议与配置.md` 的字段回退规则表里
  逐条标注「任务侧零消费 / 仅线格式兼容保留」，让后续维护者不再误写。
- **档二（独立批次）**：从 `ModeSettings` 删除这些字段 + 删 `for_mode` 对应分支 + 前端不再发送。
  需同步 `api/settings/patch/{generation,agent,memory,tools}.rs` 与
  `web/src/composables/{useDataManager,useAgentSettings}.ts`，并跑线格式测试
  （`settings_service/tests.rs` 的 `prompt_config_serde_wire_format_unchanged`）。

**别搞混**：`max_flow_call_depth` / `max_flow_calls_per_task` / `default_node_max_context`
**只被任务侧消费**，共用扁平默认是**正确**的，别一起删。

---

### R-3 任务工具描述里的创作向措辞（随每次请求下发）

**性质**：工具 `description` 与 system 同级进入模型上下文，任务模型会看到角色扮演词汇。

| 文件:行 | 现文案片段 | 建议 |
|---|---|---|
| `server-rs/src/tools/agent_tools_read.rs:18` | 「读取上下文资料:世界书条目(**角色提示词**)、技能库、**角色文件区**文件」 | 任务侧改写为「读取资料:世界书、技能库、文件…」，或按模式下发不同 description |
| `server-rs/src/tools/memory.rs:23` / `:60` | 「读取/写入**本角色**的跨会话长期记忆」 | 任务无角色时常落空桶，措辞改「本会话/本任务的长期记忆」 |
| `server-rs/src/tools/censor.rs:36` | 「对文本进行禁词替换…保留上下文语义」 | 「禁词库」是角色扮演注入配置概念，任务侧无该配置 |
| `server-rs/src/tools/revise.rs:24` | 「定点修改**正文**中的某段文本」 | 「正文」是创作术语 → 「文本片段」 |
| `server-rs/src/tools/agent_tools_agent.rs:41` | 「带**角色设定**与常驻世界书上下文」+ 整套任务向「契约/交付物/判据/END」模板 | 两个模式都要用，但模板是任务向的；建议描述按模式分叉 |
| `server-rs/src/tools/agent_tools_shared.rs`（`role`） | 「生成随机数/掷骰子 1d20」 | 任务侧可中性化为「生成随机整数」 |

**注意**：改工具描述会动 `tool_registry` 的下发路径，影响面比看起来大——建议先确认
`ToolDefinition` 是否有「按模式生成 description」的现成接缝（目前是单一 `description: String`）。
若没有，需先加接缝再改文案。

**已经踩过的坑**（别重复）：`read` 在任务里语义错位已造成实测缺陷 D5/TM-D5，
缓解方式是给任务侧追加 `fs_read`/`fs_glob`/`fs_grep`（`tool_sets::WORKSPACE_READONLY_TOOLS`），
但 `read` 本身仍留在任务工具面与规划侦察白名单里。

---

### R-4 次要词面与过时注释（低优先，可顺手做）

| 项 | 位置 | 说明 |
|---|---|---|
| 「世界书设定:」前缀 | `prompt_kit.rs::constant_world_text` | 任务侧可换中性前缀（函数已支持 `constant_world_body` 自拼） |
| `大纲.md` / `draft.md` 例子 | `task_core/prompt_consts.rs` 的 `capability_note` | 反例举了创作文件名，可换 `config.md` / `report.md` |
| 「章节」 | `task_core/prompt_consts.rs::team_audit_tool_guidance` | 与「函数」并列，属泛称，可改「条目」 |
| 过时注释一批 | `task_engine/team/mod.rs:5,7,11,497`、`task_service/mod.rs:13,102`、`task_service/executor.rs:265,1239`、`task_core/backend.rs:144,305` | 仍在说「人设 / 升华 / 提示词注入」——TM-SET-2 已把这些退役，注释会让后来者把角色扮演段改回来 |
| `TaskComposer.vue:174` 示例 | 「帮我写一篇 2000 字的科幻短篇…」 | 只是 placeholder，可换中性示例 |

---

## 三、贯穿四项的共同纪律

1. **先问「执行期哪个模式消费它」**——这是本类问题的通用判据。新增任何进 LLM 上下文的文本、
   任何 `ModeSettings` 字段、任何预览层，都要先回答这一句。
2. **按模式分叉优先于统一中性化**——中性化会削弱另一侧既有的语义强调（RTSPLIT-3 的规则句就是
   因为角色扮演侧靠「扮演/文风」措辞做边界强调，才选择分叉而非一刀切中性）。
3. **兼容口径要逐字节锁**——`untrusted_boundary` 委托 Roleplay 变体那条单测
   （`untrusted_boundary_is_verbatim_roleplay_variant`）是范例：让「20 处调用点零改动」
   成为机器可验证的事实，而不是口头承诺。
4. **改 settings 线格式必须锁往返**——`#[serde(default)]` + 旧 JSON 往返测试（R-1 的
   `mode_scope`、R-2 的字段删除都适用）。

---

## 四、排查中被判为「不是缺陷」的（别重复劳动）

调查阶段有三处看起来很像缺陷、复核后确认是**有意设计**。再遇到请先读注释，别直接改：

| 项 | 位置 | 为什么不改 |
|---|---|---|
| `loop_guard_*` 任务侧被 `clamp_for_task` 收紧到 8/4/2 | `server-rs/src/utils/loop_guard.rs:44-65` | 注释明写「设置页那三个扁平输入对任务依然生效，只是越不过上限」；`min_calls == 0` 是「关闭本闸门」的有效值，钳制**原样保持 0**。若真要让任务侧可配，须移进 `ModeSettings` 并去掉运行期钳制，那是**产品决策**不是修 bug |
| `SUBAGENT` 白名单没有 `fs_*` | `server-rs/src/tools/tool_sets.rs:57-59` | 注释明写「子 agent 未必有工作区」，纳入会让它们看到必然报错的工具，还会扩大写入面 |
| `task_core::prompt_consts` 全部常量 | `server-rs/src/services/task_core/prompt_consts.rs` | 逐条读过：全是任务/编码语义，`EXECUTOR_PROMPT` / `TASK_INTERNAL_PLAN_PROMPT` 还显式反向禁止「模拟对话」「角色扮演口吻」——是仓库里最干净的一层 |
| `build.rs` 的【创作总纲】/【角色扮演规则】系列 | `agents/engine/messages/build.rs:305-325` | 仅角色扮演兜底模板路径消费，任务链不调用 |
| `DEFAULT_RUNTIME_PROMPT` | `runtime_prompt_service.rs:24-48` | RTSPLIT-1 已拆出 `new_task`（无内置默认 + 不迁移旧文件），已正确隔离 |

**另外两条「看起来像 bug 其实是设计」的检查方法**：`grep` 现场注释——
本仓的承重约定大多写在注释里（`docs/经验.md` E94 的「先找汇聚点再谈钩子」同族）。

---

## 五、一个已订正的误判（供参考，避免重蹈）

RTSPLIT-1 批我在提交正文里写过「`untrusted_boundary` 的规则句被引擎金样测试锁定」，
据此认为改动成本很高。RTSPLIT-3 实测 `grep` 全仓后发现：那句话只在 `prompt_kit.rs:234`
**一处定义**，**没有任何测试断言其原文**——金样
（`build_tests.rs::prompt_order_golden_with_untrusted_boundaries`）只断言
`<UNTRUSTED_PROMPT_SOURCE source="…">` 标签与包裹结构。

**教训**：说「被测试锁定」之前，先 grep 断言的内容，别看测试名推断。

---

## 六、验收清单（每项收口时逐条走）

```
[ ] cargo test --manifest-path server-rs/Cargo.toml -j 8          # 需 vcvars64
[ ] npm test -w web
[ ] npm run build -w web
[ ] node tools/count-tests.mjs --check                            # 有新测试须回填 MAINTENANCE
[ ] powershell -File tools/check-all.ps1                          # 全量 19 段
[ ] 真实模型实测（隔离实例 + 探针）；不可测则如实登记理由
[ ] docs/契约-协议与配置.md 对应节次同步（本类改动几乎必然动契约）
[ ] 提交走 PR（main 有分支保护，禁直推）
```

**隔离实例起法**（实测纪律，见 `docs/经验.md` E75）：

```bash
MSYS_NO_PATHCONV=1 DATA_DIR=D:/kedai-bench-run/data-<批次> PORT=30xx \
  LOG_DIR=D:/kedai-bench-run/<批次>-evidence/logs \
  server-rs/target/debug/kedai-server.exe
# 启动后先核对日志里的 "data_dir":"..." 行，再动手
```

探针参考：`tools/bench/e2e-probe.mjs`（角色扮演）、`tools/bench/task-probe.mjs`（任务）、
`tools/bench/rtsplit-probe.mjs`（双模式运行时提示词，本批新增）。

**探针坑**（本批实测踩到，省你时间）：

- `cargo test` 日志别用 `tail -N` 再统计——会被截断只剩尾部；重定向到文件再 `grep "^test result:"`
- mock 的 `[[floors]]` 只在 **user 消息**含该串时触发；放 `system_prompt` 里无效
- `[[reply:[{…}]]]` 里的 `[[floors]]` 会被提前截断，须写成 `\u005b\u005bfloors\u005d\u005d`
- solo 模式的回显落在 `task.result`，**不在** `subtasks`
- 任务 `/run` 等写端点须带 `content-type: application/json`（走 `JsonBody` 提取器，缺了 415）

---

## 七、参考实现（前两批怎么做的，照着走）

| 关注点 | 位置 |
|---|---|
| 按模式分叉的函数签名范例 | `prompt_kit.rs::untrusted_boundary_for(mode, …)` + 两个便利包装 |
| 兼容性断言范例 | `prompt_kit.rs::untrusted_boundary_is_verbatim_roleplay_variant` |
| 双文件服务范例（含「不迁移旧文件」的理由） | `runtime_prompt_service.rs::new_task` |
| 预览按模式取文件的单点 | `api/settings/prompt.rs::runtime_prompt_for` |
| 模式级测试范例 | `server-rs/tests/runtime_prompt_split.rs`（8 例，含 `[[floors]]` 回显装配） |
| 前端模式时效范例 | `web/src/composables/useAgentPromptEditor.ts`（序号 + 模式快照 + 服务端回显三重校验）、`stores/genSettings.ts`（`settingsMode` / `settingsError`） |

**证据目录**：`D:\kedai-bench-run\rtsplit-evidence\`（RTSPLIT-1/2）、
`D:\kedai-bench-run\rtsplit3-evidence\`（RTSPLIT-3/4，含 `SUMMARY.md` 与可复跑的 `verify.mjs`）。
