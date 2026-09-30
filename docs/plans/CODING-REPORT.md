# 编码能力包功能扩展调研（CODING-REPORT）

> **文档性质**：工作线**调研产出**（`plans/` 目录，非六类活文档；引用其中事实时先复核现状）。
> **用途**：供 `计划.md` 的「编码能力包功能扩展（CODE-1~CODE-5，2026-09-30 新登记）」章使用——
> 报告只做「能不能入包、怎么入包」的推导与取证，条目细节（改动点/断言行号）以该章为准。
> **起草于 2026-09-30，基线提交 `2063486`**（行号均为该基线的实测值，改动前复核）。
> **来源动因**：用户 2026-09-30 提出「编码扩展包目前只有提示词优化，能否包含编码任务中需要用到的功能扩展（如工作区）——
> 先调研主流编码 harness（Codex / Claude Code / DSH / opencode / ZCode）的编码专属功能，再制定计划」。
> **取证口径**：以官方文档 / 官方仓库源码为主；Codex 官方站（developers.openai.com）对本环境 403，
> 其机制以 `openai/codex` 仓库源码为准；DSH/ZCode 部分信息为二手报道，逐条标注「未证实/二手」；
> 仓库侧事实均以 `文件:行号` 给出并已实测。**去重基线**：仓库既有两轮 harness 借鉴
> （`archive/learn-harness-2026-08.md` / `archive/learn-deepseek-harness.md`）与 `展望.md` 的 LH-1~LH-6、LD-1~LD-2
> ——本报告只收「它们没登记过的编码专属面」。

---

## 一、结论摘要

1. 五家的编码专属能力可归为 **7 类**：项目约定文件、工作区/隔离/沙箱、变更审计与回滚、权限与审批、
   工具面（编辑原语 / LSP / formatter / 后台作业）、上下文与记忆、扩展机制（hooks/plugins/子代理）。
2. **Kedai 的缺口集中在「编码工作流的头尾两端」**：
   - **头**：绑工作区的**前端入口缺失**——后端已完整支持（创建期校验 + 冻结 + 闸门），前端全链路
     （Sidebar → store → api）没有该参数，而 Onboarding 与关于页**already 在向用户承诺**这件事；
   - **尾**：**整任务级回滚与 patch 导出缺失**——现只有单文件回滚与逐文件 diff。
3. 两个低成本、天然贴「包范式」（常量/加性、默认关、可撤下）的增量：执行者模板补
   「**项目约定文件优先读**」纪律（CODE-3）；**工作区画像**（项目类型探测，仅展示，CODE-4）。
4. 一个与 LIT-5 同缝的可选增量：**编码流程预设集**（内置流程库加性并入，CODE-5）。
5. 需**动既有实现或引入新机制**的一律出包：OS 沙箱（Windows 无成熟原语）、git worktree、LSP 底座、
   Skills/hooks/插件机制、后台作业注册表、云端/分享/IDE 集成等（逐条理由见 §六）。
6. 与本报告相关的**既有登记**（不重复）：对话侧回退 = `HARNESS3-2`（续跑，未启动）；
   scratch 产物回收 = `PRODCAP-5`（未启动）；「规划只读侦察」= `展望.md` LH-1 已兑现部分；
   工具执行 post-execute 管道 = LH-4；apply_patch 类结构化补丁工具是否引入见拍板 Q6。

---

## 二、调研范围与方法

| 对象 | 形态 | 取证路径 |
|---|---|---|
| Claude Code | 官方 CLI/IDE/桌面 | 官方文档 `code.claude.com/docs`（2026-09 起为唯一正式站） |
| OpenAI Codex | CLI + 云端 | `openai/codex` 仓库源码（官方站 403 不可达） |
| DeepSeek Harness（DSH） | 本地服务 + Web/Electron | `deepseek-ai/deepseek-harness` 官方仓（TOML/TypeScript，MIT） |
| opencode（sst） | TUI/服务端分离 | `opencode.ai/docs` 官方文档 |
| 智谱 ZCode | 桌面 + Web + CLI | `zai-org/ZCode` 官方仓 + 官方文档站；本机有实物可复核（见 `plans/COMPUTER-USE-HARNESSES.md` §7） |

方法：每家在「机制 → 做法 → 代价/收益 → 来源」四段下取证；只收集**编码任务专属或对编码工作流直接相关**的功能，
排除与既有 LH/LD 重叠的通用项；每项给「对 Kedai 的判定」（入包 / 拍板 / 出包 / 已具备）。

---

## 三、五家 harness 的编码专属机制

### 3.1 项目约定文件与技能（最普遍的基线能力）

| 家 | 机制 | 关键做法 |
|---|---|---|
| Claude Code | `CLAUDE.md` 四层（企业/用户/项目/local） | 会话启动时从 cwd 向上扫描合并（宽泛在前、局部在后）；`@import` 递归 ≤4 层；无 CLAUDE.md 时 AGENTS.md 自动替代 |
| Codex | `AGENTS.md` | 项目根定位后自根向下逐层拼接；`AGENTS.override.md` 可覆盖；累计字节达 `project_doc_max_bytes` 即停 |
| opencode | `AGENTS.md`（兼容 `CLAUDE.md`） | 启动向上查找 + 全局；`instructions` 支持 glob/HTTP 远程规则；`/init` 可生成 |
| ZCode | `SKILL.md` 多级根 | 用户级 `~/.zcode/skills` + 项目级逐级上溯（`.zcode` 优先、**合并而非 fallback**）、诊断码 + lock 文件 |
| DSH | Skills/插件声明 | 「一切皆插件」下的指令包机制，能力以 `package.json` 的 `dsh` 字段声明 |

**共性**：模型上手项目前先读「项目自己写的约定文件」是各家主路径（而非只靠通用提示词泛指引）。
**对 Kedai 的启示**：Kedai 的编码执行者模板已有「先读后写：摸清相关文件、项目结构与既有约定」的**泛指**表述
（`server-rs/src/services/settings_service/params.rs:414-434`），缺的是**点名**（AGENTS.md/CLAUDE.md 等）
——零架构成本的文本增补（CODE-3）。

### 3.2 工作区、隔离与沙箱

- **Claude Code**：git worktree 隔离（`--worktree`，四道越界校验）；OS 沙箱走 Seatbelt / bubblewrap，
  **官方明言 "Native Windows is not supported"**。
- **Codex**：`read-only / workspace-write / danger-full-access` 三档 + 可拼装权限档案；Windows 用**受限令牌**
  （级别 elevated/restricted/disabled）；越界时 `require_escalated` + `justification` + `prefix_rule` 申请提权。
- **DSH**：同款三档；后端 bwrap/Landlock/Seatbelt/Windows ACL，并**诚实上报 `full|partial`**（Windows ACL 场景 = partial）。
  工作区以 `WorkspaceId`（uuid）与路径分离，**对模型完全不可见**。
- **opencode**：`external_directory` 规则——工作区外路径需显式授权。
- **ZCode**：无 OS 沙箱叙事（桌面产品），隔离靠审批档位 + 检查点。

**对 Kedai 的启示**：① OS 沙箱出包（Windows 无成熟原语，三家的 Windows 实现或缺失或 partial，工程量与收益不成比）；
② 「工作区 id ≠ 路径、对模型不可见」与 Kedai `workspace_guard` 的收口方向一致，无需改；
③ 真正的**产品缺口是「绑工作区」的入口本身**（§四）。

### 3.3 变更审计、检查点与回滚（Kedai 的强项线，缺口在「整任务」）

| 家 | 机制 | 关键语义 |
|---|---|---|
| Claude Code | **每轮前自动 checkpoint**（保留 100 个）；`/rewind` / 连按 Esc | 六项恢复组合（代码与对话 / 只对话 / 只代码 / 摘要）；**明确不覆盖**终端命令、后台进程、外部手改 |
| Codex | `turn_diff_tracker` 逐轮记账；`/diff` | 会话 JSONL rollout 落盘；云端任务 `apply/diff` 把云端产出打到本地 |
| DSH | 逐轮 **git tree-id 比对** + 非 git 路径整文件补齐 | 快照走私有 index + 只读 alternate，不动用户仓库；产出 unified diff hunk |
| ZCode | **每轮检查点** | 「对话+文件」或「仅对话」两档；**要么全撤要么都不撤**；只覆盖 agent 自改文件 |
| opencode | `/undo` 删最近一条用户消息**并回滚文件改动**（依赖 Git 仓库）；`/redo` | 文件回滚与对话回退**同一动作** |

**对 Kedai 的启示**：Kedai 的文件侧台账（`task_file_changes` + diff + 单文件回滚 + bash 前后扫描）已不弱于任何一家
的单文件粒度；缺口是**整任务一键回滚**与**patch 导出**（CODE-2）。对话侧回退不要在本线做（`HARNESS3-2` 已登记）。

### 3.4 权限与审批

- **Claude Code**：`Tool(specifier)` 规则语法（路径用 gitignore 语法）+ 5 层 settings 合并（安全取最严）；模式
  `default/acceptEdits/plan/bypassPermissions/auto`。
- **Codex**：`AskForApproval ∈ {Never, OnRequest, UnlessTrusted, Granular}`；execpolicy `.rules`（Starlark）
  三态 `Allow/Prompt/Forbidden`，先规则后启发式。
- **opencode**：`allow/ask/deny` 三态 + 通配（最后匹配优先）；`--auto` 跳过询问但 deny 仍生效；三次相同调用触发 `doom_loop`。
- **DSH**：`ask|never` 会话策略；**无应答者一律 fail-closed**；审批结果四态 `allowed-once/rejected/cancelled/unavailable`。
- **ZCode**：四档（变更前确认/自动编辑/计划模式/完全访问）；决策可持久化（单次/本会话/始终本项目）。

**对 Kedai 的启示**：Kedai 已有三档策略 + 风险级 + 两道闸门 + 逐工具授权，主结构不缺；「审批持久化」与
「规则文件」属内核面，出包（另立批次）。**一处待拍板的既有悬置**：`deny_dangerous` 档按名例外
（`bash`/`fs_write`/`fs_edit`）收编到包声明（`功能.md` §十六 原文「单列待明确同意」）——见 `计划.md` 本批拍板 Q6 讨论。

### 3.5 工具面（编辑原语、LSP、formatter、后台作业）

| 能力 | 家 | 做法 | 对 Kedai |
|---|---|---|---|
| 精确串替换编辑 | Claude Code / opencode | 唯一匹配 + 必要时 replace_all | **已具备**：`fs_edit` 同款语义（`agent_tools_fs.rs:253-325`） |
| 结构化补丁 `apply_patch` | Codex / ZCode | FREEFORM 补丁语法：多 hunk / Add/Update/Delete/Move；先校验后落盘；部分失败语义 | 拍板 Q6（增量在「多文件批量 + 移动/删除」；成本 = 新工具 8 处登记 + 台账接线） |
| 行号读取 + PARTIAL | Claude Code | Read 带行号、分页、接近上限给部分视图而非失败 | Kedai `fs_read` 已有行号分页（`agent_tools_fs.rs:733` 测试锁）——**已具备** |
| 大输出落盘 | Claude Code | Bash 内联约 30k 字符，超出落临时文件回传指针 | 出包/后续：Kedai 现为头+尾截断（HARNESS3-4），审计侧全量；「落盘回读」另立评估 |
| **LSP 诊断回环** | opencode / DSH | 语言服务器诊断回喂模型（编辑后自动），实验性 `lsp` 工具查定义/引用 | **出包**：需 LSP 管理底座，Windows 桌面成本高（opencode 自己都建议优先 CLI lint） |
| **formatter 自动执行** | opencode | 写/编辑后按扩展名异步跑 gofmt/prettier 等 | **出包**：按项目配置执行外部命令（供应链面 + 触发时机争议），先以模板纪律代替 |
| **后台作业注册表** | DSH / Claude Code | 长命令注册 `job_*` 可查/可停；Ctrl+B 后台化、输出落文件 | **出包（另立内核批次）**：需动 exec/引擎（超时、取消、记账全交织） |
| 持久 PTY / 交互式续写 | Codex / DSH | `exec_command` 返回 session id，`write_stdin` 续交互 | 出包：桌面场景多由 bash 单次执行覆盖 |

### 3.6 上下文与记忆

- **Claude Code**：两阶段压缩（先 microcompact 清旧工具输出、再摘要）；`/compact <指令>`；**模型自记 memory**
  （纠错/偏好，索引常驻、细节按需）；Todo 面板**抗压缩存活**。
- **Codex**：compact 系列 + memories；`get_context_remaining`/`new_context_window` 显式暴露预算。
- **DSH**：智能压缩（既有借鉴线，见 `learn-deepseek-harness.md` 借鉴点 1，已落地）。
- **对 Kedai 的启示**：压缩保留工作锚点已落地（`HARNESS3-3`，2026-09-30）；microcompact 式「先清工具输出再摘要」
  与「memory 自记」属通用内核线，不在本包范围（如需，另立架构批次）。

### 3.7 扩展机制与流程

- **Claude Code**：Hooks（30+ 事件、4 类 handler、退出码门禁）、Skills（SKILL.md 渐进披露）、Subagents
  （frontmatter 限工具/模型、独立上下文、**可按 ID 续跑**）、Plugins + marketplace、MCP + Tool Search。
- **ZCode**：plugin.json + 内置插件市场（启用插件 = 授予代码执行信任）、Hooks、子代理、自动化。
- **DSH**：一切皆插件（连 agent loop 都可替换）；有 `subagent-codex` / `subagent-claude-code` 等**外部 harness 适配器**。
- **Codex**：`update_plan`（至多一个 in_progress）、`spawn_agent` 子代理、review rubric 模板（云端 code review）。
- **opencode**：自定义命令（`.opencode/commands/*.md`）、插件 hook（tool.execute.before/after）、**流程化 agent
  （build/plan/general + 自定义）**。
- **对 Kedai 的启示**：插件/hooks/Skills **出包**（违「不新建插件机制」定性）；子代理已有（`agentgo/agentend`），
  「可续跑 + 工具白名单」在 `HARNESS3-2`/既有 `tool_sets` 线上；**可借的加性项 = 内置流程库的编码流程预设**（CODE-5，
  与 `LIT-5` 同缝）；Codex 的 review rubric 可作为「代码审查」流程的内容素材。

---

## 四、仓库现状对照矩阵（实测）

| # | 能力 | Kedai 现状（证据） | 五家做法 | 判定 |
|---|---|---|---|---|
| 1 | 工作区绑定 | **后端已全**：创建期校验 + 冻结（`server-rs/src/api/tasks.rs:65,112-127`；`task_service` 落库）；闸门 `tools/workspace_guard.rs`；scratch 兜底 `<scratch_root>/<task_id>` | 各家皆「会话/任务绑定一个目录」为默认入口 | **入包（CODE-1）**：前端全链路缺失（`Sidebar.vue:251` → `stores/task.ts:756` → `api/tasks.ts:33-59`），且 Onboarding/关于页已承诺（`OnboardingModal.vue:143-148`、`AboutSection.vue:50,109`） |
| 2 | 原生目录选择 | Tauri `plugin-dialog` 已是前端依赖（`web/package.json:24`），权限仅 `dialog:allow-save`（`src-tauri/capabilities/default.json:11`） | CLI 家由 shell 补全；桌面家（ZCode）原生选择器 | **入包（CODE-1）**：补 `dialog:allow-open`；浏览器形态手填兜底 |
| 3 | 变更台账 / diff | **已具备**：`task_file_changes` + 三端点 + `FileChangesPanel`（PRODCAP-4 已完，含 bash 侧扫描） | §3.3 各家 | 已具备（无需动） |
| 4 | 回滚 | **单文件**已闭环（`api/tasks.rs:405-414`；无基线不动手；回滚再记一条） | 各家均有整轮/整会话级回退 | **入包（CODE-2）**：整任务回滚 + patch 导出 |
| 5 | patch 交付 | 无（diff 仅逐文件查看，不可导出） | Codex 云端 `apply` 落本地、ZCode 编辑历史 | **入包（CODE-2）** |
| 6 | 项目约定文件 | 模板仅**泛指**约定（`params.rs:414-434`） | 点名机制为各家主路径（§3.1） | **入包（CODE-3）**：文本增补一句 |
| 7 | 项目类型感知 | 无探测（全仓 `Cargo.toml`/`package.json` 探测零命中，实现时复核） | 无直接对标（各家家主靠模型自行侦察） | **入包（CODE-4）**：只读探测 + 展示（是否注入见拍板 Q4） |
| 8 | 编码流程 | 内置流程库 1 条（文学协调流程） | opencode 流程化 agent、Codex review rubric | **入包（CODE-5）**：加性并入编码流程预设 |
| 9 | 执行者姿态 | **已具备**：`task_coding_bundle_enabled` 缺省词切换（HARNESS3-6） | — | 已具备 |
| 10 | 自测闭环 | **已具备**：上限 1800s + 输出保头保尾（HARNESS3-4） | — | 已具备 |
| 11 | 压缩保工作锚点 | **已具备**（HARNESS3-3） | — | 已具备 |
| 12 | 先读后写 / 新鲜度 | **已具备**：`fs_*` 读记录 + 写前校验（`agent_tools_fs.rs:11,158,235`） | DSH `fs-observation-policy` 同向 | 已具备 |
| 13 | OS 沙箱 | 路径闸门 + JobTree 终止（Windows 已做，Unix 未做：`遗留.md` EXEC-TREE-1） | §3.2 | **出包** |
| 14 | worktree / git 集成 | 无（自动 git 提交已裁「仍在原线」） | Claude Code worktree、opencode undo 依赖 Git | **出包** |
| 15 | LSP / 诊断 | 无 | opencode/DSH | **出包** |
| 16 | Skills / hooks / plugins | 无（定性禁新建插件机制） | 各家皆有 | **出包** |
| 17 | 后台作业注册表 | JobTree 可终止，无后台注册表 | DSH `job_*`、Claude Ctrl+B | **出包（内核批次评估）** |
| 18 | 对话侧回退 | 无 | Claude checkpoint、opencode undo | 已登记 `HARNESS3-2`（不重复） |

---

## 五、包设计（CODE-1~CODE-5）

### 5.1 包范式与边界（本批怎么挂进包）

- 包现状 = 单开关 + 缺省词切换（HARNESS3-6）。本批补「功能面」，纪律与前批一致：
  **加性、默认关、可独立撤下、不新建插件机制、不侵入内核**。
- **开关归属逐项定**（不搞「一律进包」）：只有**执行姿态类**（CODE-3 的模板文本、CODE-5 的流程并入）
  在包开关语义内；**通用交付面**（CODE-1 工作区入口、CODE-2 回滚/导出、CODE-4 画像展示）**不接包开关**
  ——与 D1/PRODCAP-4 的既有交付口径一致（工作区、工具族、台账都不接包开关），避免造出
  「关了包就不能绑工作区/不能回滚」的断裂。CODE-1 的开关归属留拍板 Q1。
- **动态内容不进内核**：CODE-4 若选注入（Q4），段文本经 `task_service` 拼装缝，内核装配/压缩路径不读包开关
  （承 `HARNESS3-3` 纪律）。
- **与既有台账互不重复**：CODE-2 = PRODCAP-4 的延伸；对话侧回退 = `HARNESS3-2`；scratch 回收 = `PRODCAP-5`。

### 5.2 工作区入口（CODE-1）

推导：后端创建期校验/冻结/闸门已全（§四 #1），前端三处链路无该参数，而 Onboarding/关于页已在承诺——
这是**断链补齐**而非新机制。设计：`createTask` 增可选 `workspace`（**仅非空下发**，旧请求体逐字节不变，
照 `flowId` 口径）；表单加「工作区」行（Tauri 原生选择 / 浏览器手填 + 清空 + 最近列表）；任务详情展示 +
复制；关包场景零影响。Tauri 权限补 `dialog:allow-open`；壳侧改动需重跑 `build.ps1` 验证 exe。

### 5.3 交付闭环（CODE-2）

推导：单文件回滚已闭环、语义既定（无基线不动手、回滚再记、running 409）；缺「整任务」与「导出」。
设计：`POST .../changes/rollback-all`（逐路径按既有单文件语义、**逐项报告**、可逆性显式断言）+
`GET .../changes/patch`（unified patch 文本，无基线文件头部注释列明、超预算沿用「改动过大」口径）。
失败口径（部分成功 vs 全或无）见拍板 Q2。

### 5.4 认知增强（CODE-3 / CODE-4）

- CODE-3：模板增补一句「项目约定文件优先读（AGENTS.md/CLAUDE.md/README 等）」——**修改既有常量文本**，
  四态测试与字面约束照旧（含「任务执行智能体」、无 `{{char}}`）。
- CODE-4：只读存在性探测（常量映射表：Cargo.toml→`cargo test`、package.json→`npm test`、
  pyproject.toml→`pytest`、go.mod→`go test`，首版 4~6 项），任务详情加性字段展示；**不执行任何命令**；
  是否注入提示词见 Q4（默认仅展示）。

### 5.5 流程预设（CODE-5）

推导：与 `LIT-5` 同缝（内置流程库加性并入、既有条目逐字不动、已注入用户库不重复注入、用户改过不复活）。
首版清单见 Q5。**注意与 LIT-5 的互斥面**：两批改同一段代码，建议同批或紧邻落地。

### 5.6 待拍板六项（决议入 `计划.md` 本批「本轮待拍板」表）

Q1 工作区入口开关归属；Q2 整任务回滚失败口径；Q3 模板是否增补约定文件条款；Q4 画像是否注入；
Q5 流程首版清单；Q6 apply_patch 是否作为「首个包专属工具」引入（含 §十六 悬置的 `deny_dangerous` 收编问题一并讨论）。

---

## 六、出包清单（需动既有实现或引入新机制，逐条理由）

| 能力 | 为何出包 |
|---|---|
| OS 级沙箱（Seatbelt/Landlock/受限令牌） | Windows 无成熟原语：Claude Code 明言不支持原生 Windows；DSH 在 Windows 只到 ACL partial；Codex 受限令牌工程量大。Kedai 现有「路径闸门 + JobTree」是诚实边界（`遗留.md` EXEC-TREE-1）。要做另立内核批次 |
| git worktree 隔离 | 需 git 集成与「任务=分支」语义；自动 git 提交已裁「仍在原线」（PRODCAP 章定位边界） |
| LSP 诊断回路 | 需语言服务器管理底座（拉起/版本/内存），Windows 桌面收益成本比低（opencode 亦建议优先 CLI lint） |
| Skills / hooks / plugins 机制 | 违 `AGENTS.md`「不新建插件机制」定性；包以开关 + 常量交付 |
| 后台作业注册表（`job_*`） | 需动 exec/引擎内核（超时、取消、记账口径全交织）；另立 HARNESS 批次评估 |
| 云端任务 / 会话分享 | 与单机、隐私优先定性冲突（opencode `/share` 明确不支持方向） |
| IDE 集成 / 行区间引用 / 编辑器内嵌 diff | 依赖编辑器宿主，桌面 harness 不在其位 |
| 自动 git 提交 / 提交前检查 | 定位边界已裁「仍在原线」，不回流 |
| formatter 自动执行 | 需按项目配置执行外部命令（供应链面 + 触发时机争议）；先以模板纪律代替 |
| 对话侧回退 | 已登记 `HARNESS3-2`（续跑，未启动）——不重复登记；CODE-2 只补文件侧 |
| 大输出落盘回读 | Kedai 现为头+尾截断 + 审计全量（HARNESS3-4）；「落盘 + 指针回读」需动 exec 输出通道，另行评估 |

---

## 七、切片顺序与工时（概估）

| 序 | 条目 | 工时 | 风险 | 依赖 |
|---|---|---|---|---|
| 1 | CODE-1 工作区入口 | 1d | 低-中（前端三链路 + Tauri 权限） | — |
| 2 | CODE-3 模板约定文件条款 | 0.25d | 低 | — |
| 3 | CODE-2 整任务回滚 + patch 导出 | 1.5~2d | 中（回滚语义 / 409 / 无基线） | PRODCAP-4（已完） |
| 4 | CODE-4 工作区画像（展示） | 0.5~1d | 低 | CODE-1 |
| 5 | CODE-5 流程预设集 | 1d | 中（custom 回归） | LIT-5 同缝 |

建议：**CODE-1 + CODE-3 一个批次**（开箱可见 + 零风险文本）→ **CODE-2** 独立批次（后端为主）→
CODE-4 随 CODE-1 或紧随 → CODE-5 与 LIT-5 同批或紧邻。每批次独立可交付、独立可回滚。

---

## 八、验收与风险（报告层；条目层断言见 `计划.md`）

- 共同底线：**关包/缺省形态逐字节不变**（回归钉子）；新增字段/端点全部加性；前端手工核对清单写进提交信息。
- 风险热点：① CODE-2 的批量回滚会一次改多文件 → 二次确认 + 逐项报告 + 可逆性显式断言；
  ② CODE-1 的 Tauri 权限属壳侧改动 → 收口跑 `build.ps1`；③ CODE-5 与 LIT-5 同缝 → 防两次改同一函数漂移。
- 明确不做（防止实施时顺手加）：不新增工具（apply_patch 见 Q6）；不把通用交付面收进包开关；
  不把 scratch 与工作区混同（未绑定任务仍走 `scratch_root/<task_id>`，PRODCAP-5 管回收）；
  不改既有单文件回滚语义；不实现出包清单任何一项。

---

## 九、附录：来源索引与方法局限

**来源索引**（详链接见各调研 agent 原始产出与本节列示）：
- Claude Code：Tools / Memory / Checkpointing / Permissions / Sandboxing / Hooks / Skills / Sub-agents /
  Worktrees / Sessions（`code.claude.com/docs`）。
- Codex：`openai/codex` 的 `shell_spec.rs`、`apply-patch/src/lib.rs`、`sandboxing/mod.rs`、`exec_policy.rs`、
  `agents_md.rs`、`plan_spec.rs`、`cloud-tasks/`、`rollout.rs`。
- DSH：官方仓 `docs/tool-catalog.zh.md`、`docs/subsystems/{sandbox,approval,workspace,deliverables}.zh.md`、
  `packages/boot/plugin-manager/README.zh.md`。
- opencode：`opencode.ai/docs` 的 lsp / permissions / tools / agents / formatters / server / tui。
- ZCode：官方仓 `zai-org/ZCode`（`packages/contracts/src/tools`、`adapters/src/skills/roots.ts`、
  `workspace-checkpoints.ts`）+ 官方文档站；本机实物复核见 `plans/COMPUTER-USE-HARNESSES.md` §7。

**方法局限**：
1. Codex 官方产品文档不可达（403），云端/产品面描述部分取自仓库与二手，实施前如需引用云端行为应再核；
2. DSH/ZCode 的「插件市场」「整改细节」等为二手报道（已标注），本报告未将其作为入包依据；
3. 五家版本演进快，机制细节（默认值、事件名）以实施时点为准；
4. 本报告刻意**不**展开与 LH/LD 重叠的通用项（压缩、记忆、hooks 管道等），那些进内核批次而非能力包。
