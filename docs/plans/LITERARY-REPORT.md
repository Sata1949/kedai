# 文学能力包方案调研（LITERARY-REPORT）

> **日期**：2026-09-28
> **范围**：通用 harness 定性下的文学能力增强——外部文学 / 角色扮演 harness 的文学能力优化经验、
> 仓库现状对照、官方能力包形态设计（T1~T3）与出包清单（T4）。
> **方法**：外部调研（SillyTavern / AI Dungeon / Novelcrafter / Sudowrite / NovelAI / RisuAI 等官方文档与
> 源码默认值 + 长文生成学术做法，逐条带 URL）+ 仓库三路只读侦查（逐条 `文件:行号`，行号基于 2026-09-28 的 `main`）。
> **边界**：本报告是**调研产出**，不是执行稿——开放项、验收断言与待拍板的唯一出口是
> [`计划.md`](../计划.md) 的「文学能力包（LIT-1~LIT-8）」章。包形态受 2026-09-28 定性裁定约束：
> 通用 harness、垂直能力走官方能力包、默认关、不新建插件机制。
> **生命周期**：不随代码同步（`docs/README.md` 的 `plans/` 约定）；引用其中事实前先复核现状，行号会漂移。

## 一、结论摘要

1. **仓库的文学底座比预期厚**：世界书条目字段已是 ST 同构的全字段集（含 `keys_secondary` / `regex` /
   `sticky` / `cooldown` / `probability` / `scan_depth` / `role`），角色卡兼容 V1/V2/V3，token 级裁剪、
   四级压缩水位、跨会话记忆蒸馏 + 混合召回（向量 0.7 + Jaccard 0.3）、正则脚本、6 层位置装配都在。
   外部经验里「低成本高收益」的多数条目，Kedai **已有机制、只缺用法规范与默认值**。
2. **真正的缺口集中在「提示词层的文学纪律」**：反 AI 腔清单、正例锚定、视角 / 时态锚定、
   人物声音一致性、事实不回退、叙事节拍与结尾钩子——这些**不需要动架构**，正是能力包该装的东西。
3. **最高杠杆的单点**是「Author's Note 等价段」：仓库已有位置 0 尾部注入缝（`preset_tail_prompt`，
   默认空），而外部共识是「关键约束放中段会被忽略、需头尾双写」。把它接上包的默认内容，
   存量用户与自定义提示词用户都能受益。
4. **长程一致性的外部共识是「锚点事实表只增不改 + 摘要随消息可回滚」**，反面是「逐轮重写摘要 →
   早期事实漂移」。Kedai 已有记忆蒸馏与摘要槽，包应在**策略层**（写入候选筛选、摘要增量、注入位置）
   给推荐档，而不是新增存储。
5. **文风控制上「few-shot 正例优于禁令」**是跨家共识；纯禁令需要配输出后正则兜底（Kedai 已有
   正则脚本机制与禁词库，属可复用面）。
6. **首条消息决定模型模仿的文风与长度**（ST 官方原话）——这条把「开场白」从角色卡字段提升为
   文风第一控制点，包的「开场白写作规范」因此有独立价值。
7. **流程（agent-flows）是被低估的载体**：仓库内置流程库已有一条「文学创作协调流程」，
   把「章节续写 / 润色去 AI 腔 / 一致性校对 / 人物声音校准」做成流程预设，复用既有机制、
   零架构改动，且比提示词更可组合（二维流程、子图、节点级模型都现成）。
8. **T4 类能力（世界书递归与预算、文档 RAG、群聊、Trigger 分类、stop strings、摘要回滚）都必须动
   既有实现**（worldbook 引擎 / 消息装配 / 存储），与「已有架构与功能实现不变」的定性冲突，
   故逐条出包、另行裁定，不在本包范围。
9. **两处诚实边界必须写进条目而不是藏起来**：① 位置 0 尾段**不计 `protected_tail`**，极端裁剪下可被切；
   ② 并入内置流程库的包流程，用户编辑即原地覆盖、删除不复活（沿用既有语义，不新增复活逻辑）。
10. **一处既有隐患要顺手钉住**：角色扮演侧缺省提示词存在**两份并行文本**
    （`params.rs` 宏版 / `build.rs` 分段版），包启用时必须同受一个判据函数管辖，否则必然出现第三份副本。

## 二、定性变更与包形态

**定性（2026-09-28 项目组拍板）**：Kedai 是**通用 LLM 智能体 harness**——角色扮演（文学创作）会话与
任务工作台是平级双顶层模式，共用 Agent 引擎与工具 / 授权层；垂直能力一律以**官方能力包**形态交付。
本批已同步全部文档的定位叙述（`README.md` / `MAINTENANCE.md` / `docs/功能.md` / `docs/README.md` /
`package.json` / `web/index.html` / `AGENTS.md`），**架构与功能实现未动**。

**包形态（与编码能力包同构，2026-09-28 裁定）**

| 要素 | 编码能力包（已落地先例） | 文学能力包（本方案） |
|---|---|---|
| 声明 | 单个常量 `default_coding_task_agent_prompt` | 常量集：缺省词、增强段、Author's Note 段、文风预设、流程预设、反思检查项 |
| 开关 | `task_coding_bundle_enabled`（扁平 + task 覆盖层，默认 false） | `literary_bundle_enabled`（RP 侧扁平）+ `task_literary_bundle_enabled`（扁平 + 覆盖层） |
| 生效 | **缺省值切换**（用户自定义逐字优先） | 缺省值切换 **+ 启用时追加注入段**（不写用户字段，关掉逐字还原） |
| 机制 | 不新建插件机制、只替换缺省值 | 同上；另复用内置流程库（`agent_flow_service`） |

**一处必须写进方案的技术事实**：`for_mode` 对 Roleplay 直接 `return self.clone()`
（`server-rs/src/services/settings_service/params.rs:577-580`），角色扮演**引擎侧不经 `for_mode`**——
引擎读的是启动时注入的扁平设置快照（`server-rs/src/api/app_state.rs:313-322` →
`server-rs/src/agents/engine/messages/context.rs:341`）。因此**「在 `for_mode` 里加注入」对角色扮演无效**，
包的注入段必须落在引擎装配点（候选插点见 §五）。

## 三、外部经验（机制 → 做法 → 代价/收益 → 来源）

### 3.1 世界书 / Lorebook：触发、预算与注入位置

- **触发层降噪**：主键触发以外，ST 支持**次键（AND 语义）**、正则键、大小写与全词匹配、
  触发概率（50% = 1:1）、组内权重（默认 100）。**次键 + 全词匹配是降噪第一优先级，几乎零成本**。
- **递归与预算**：条目级 `recursion` 三态（不可递归 / 阻止后续递归 / 延迟到递归后）、
  全局 `max recursion steps`（0 = 只受预算约束）、`min activations`（非 0 时突破 scan depth 反向扫全部
  聊天直到激活 N 条，且**该轮不计递归产生的条目**）。源码默认：`world_info_depth=2`、
  `world_info_budget=25%`、`min_activations=0`、`max_recursion_steps=0`、条目 `DEFAULT_DEPTH=4`、权重 100。
- **注入位置**：before/after 角色定义、before/after 示例、Author's Note 顶 / 底、`@D`（深度 0 = 提示词
  最底部）、outlet（`{{outlet::Name}}` 手动插槽，可当预留位）。
- **向量检索**：ST Data Bank 走「文件 → chunk（字符数 + 重叠%）→ 检索注入」，有 `Insert#` 上限与
  score threshold（实用 0.2~0.5），**且先为检索块预留预算**；Chat Vectorization 只向量化聊天
  （chunk 默认 400 字符、阈值 25%、query=最近 2 条、insert=3、retain=5）。代价：嵌入须与生成同模型，
  换模型需全量重算。
- **代价 / 收益**：机制全上会爆 token（递归无上限 + 大预算）；ST 的默认值是「小预算 + 浅扫描」。
  **低成本高收益子集**：次键 + 全词匹配、状态类条目用 sticky/cooldown 避免每轮重复、
  `budget=25%` cap + `max recursion steps ≤ 3`、关键约束在头尾重复（中段易被忽略）。
- **来源**：[ST World Info](https://docs.sillytavern.app/usage/core-concepts/worldinfo/)、
  [源码默认值](https://cdn.jsdelivr.net/gh/SillyTavern/SillyTavern@release/public/scripts/world-info.js)、
  [Chat Vectorization](https://docs.sillytavern.app/extensions/chat-vectorization/)、
  [Data Bank](https://docs.sillytavern.app/usage/core-concepts/data-bank/)、
  [Lost in the Middle](https://arxiv.org/abs/2307.03172)

### 3.2 长程一致性：预算分配、滚动摘要与锚点事实

- **AI Dungeon 的预算与顺序（可直接抄）**：硬性部分超 70% 时按优先序保留
  Author's Note > Plot Essentials > AI Instructions > Story Summary；动态部分按约
  **25% Story Cards / 50% History / 25% Memory Bank** 分配；最终顺序为
  Instructions → Plot Essentials → Story Cards → Story Summary → Memory Bank → History →
  Author's Note → Last Action → Front Memory。History 由新到旧填充但**最新一条必留**；
  Story Cards 按触发新近度 + 频率排序，至少回看 4 条行动。
- **分层 / 滚动摘要**：AD 每 6 轮压成 1 条 Memory（文本 + 向量入 Memory Bank，容量分层，遗忘「最少使用」）；
  Story Summary 每 15 行动滚动更新。ST Summarize 扩展的触发是「每 X 消息**或**每 X 词（先到先触发）」+
  目标词数，模板 `{{summary}}`，位置同 Author's Note；**摘要随消息写入 chat metadata，
  编辑 / 删除该消息会回滚到上一版摘要**——这是防漂移的廉价机制。
- **学术做法**：递归摘要（记忆 = 上一版记忆 + 后续上下文）[2308.15022]；RecurrentGPT 的语言化长 / 短时记忆
  （可读可编辑）[2305.13304]；MemGPT/Letta 的分层记忆（core memory blocks + archival + recall）[2310.08560]；
  DOC 的层级大纲 + controller（plot coherence +22.5%）[2212.10077]。
- **最小状态层**：定长「锚点事实表」（JSON/YAML：人物属性 / 关系 / 物品 / 时间线），**只增不改**，
  摘要只做增量；写入候选 = 发生状态变更的句子（换装 / 受伤 / 获得物品 / 关系变化），而非全量对话。
- **来源**：[AD 上下文构成](https://help.aidungeon.com/faq/what-goes-into-the-context-sent-to-the-ai)、
  [AD 记忆系统](https://help.aidungeon.com/faq/the-memory-system)、
  [ST Summarize](https://docs.sillytavern.app/extensions/summarize/)、
  [递归摘要](https://arxiv.org/abs/2308.15022)、[RecurrentGPT](https://arxiv.org/abs/2305.13304)、
  [MemGPT](https://arxiv.org/abs/2310.08560)、[Letta 概念](https://docs.letta.com/concepts/memgpt)、
  [DOC](https://arxiv.org/abs/2212.10077)

### 3.3 文风与文笔质量控制

- **反 AI 腔没有权威词表**（社区共识的常见标记：`a testament to` / `little did X know` /
  `shivers down her spine` / `the air was thick with` / `couldn't help but` / `something shifted` /
  三段排比 + 破折号总结）。工程上**few-shot 正例优于禁令**；纯禁令要配**输出后正则黑名单重写**
  （ST 正则扩展支持作用域 = 输出、捕获组、flags、min/max depth）。AI Dungeon 明确建议
  Plot Essentials「避免否定句，用 avoid」。
- **人称 / 视角 / 时态**：在 system 与 Author's Note **双写**，并用示例锚定；
  **首条消息（first_mes）决定模型模仿的文风与长度**（ST 官方原话），故它是文风第一控制点。
- **防复读与防越界**：ST 侧推荐 `Names as Stop Strings`、`Separators as Stop Strings`（防角色名复读与
  示例块复读）、模板设 Example Messages Behavior = Never include；Context Template 中**未出现的参数
  完全不发送**，可当裁剪开关。
- **来源**：[ST Context Template](https://docs.sillytavern.app/usage/prompts/context-template/)、
  [ST Regex](https://docs.sillytavern.app/extensions/regex/)、
  [ST Character Design](https://docs.sillytavern.app/usage/core-concepts/characterdesign/)、
  [AD Plot Essentials](https://help.aidungeon.com/faq/plot-essentials)、
  [ST Prompt Manager](https://docs.sillytavern.app/usage/prompts/prompt-manager/)

### 3.4 交互式叙事结构

- **Novelcrafter**：Codex 条目带 `aliases` / `mentions`（关键词召唤）与 **relations / progressions**
  （追踪元素**随时间的变化**）；`scene beats` = 场景摘要 + 节拍；Matrix 跨场景追踪人物与情节线；
  Prompt Components / Inputs（可复用指令块 + `include()` + 实时预览）；**Extract** 从聊天抽结构化数据
  回填 Codex（生成后抽取更新——值得移植的「写回」模式）。
- **Sudowrite**：Story Bible 分 Genre / Style / Synopsis / Characters / Worldbuilding / Outline；
  另有 Saliency Engine、Chapter Continuity（选取近期正文与圣经条目入 prompt，细节未公开）。
- **动作切换**：ST Prompt Manager 的每条提示词带 `Trigger = Normal / Continue / Impersonate / Swipe /
  Regenerate / Quiet`——同一后端可按「续写 / 改写 / 扮演」装配不同提示词集，这是「多动作」的低成本实现路径。
- **来源**：[Novelcrafter 文档](https://www.novelcrafter.com/help/docs)、
  [Codex 条目解剖](https://www.novelcrafter.com/help/docs/codex/anatomy-codex-entry)、
  [Prompt Components](https://www.novelcrafter.com/help/docs/prompt-components/prompt-components)、
  [Sudowrite 文档](https://docs.sudowrite.com/)

### 3.5 角色扮演工程

- **角色卡字段的分工**：name / description / personality / scenario 常驻；`first_mes` 只发一次；
  example messages 按可用空间**逐块淘汰**（须含 `<START>` 分隔符）。
- **群聊发言调度**：Manual / Natural Order（提及名字 + talkativeness + 允许自答）/ List / Pooled
  （自上次 user 发言后未回者中随机）；`Swap character cards` = 只发当前发言者卡（**防人格混淆**）；
  `Join character cards` 合并全组（官方警告可能导致人格融合）；另有 group nudge 提示词、mute、auto-mode 延时。
- **Persona**：描述可注入 Story String / Author's Note 顶底 / in-chat depth，位置随 persona 保存；
  切换 persona 不回溯历史。
- **防代答 / 防出戏**：停止串里加 `{{char}}` / `{{user}}` 名 + Always add character's name to prompt；
  正则拦截「As an AI」类元文本。
- **来源**：[ST Character Design](https://docs.sillytavern.app/usage/core-concepts/characterdesign/)、
  [ST Group Chats](https://docs.sillytavern.app/usage/core-concepts/groupchats/)、
  [ST Personas](https://docs.sillytavern.app/usage/core-concepts/personas/)

### 3.6 跨家反模式清单（写进方案的红线）

1. **递归无上限 + 大预算 → token 爆**：条目设 non-recursable、steps ≤ 3。
2. **`min activations` 反向全扫**会忽略递归条目，产生预期外激活与成本。
3. **摘要逐轮重写 → 早期事实漂移**：改「锚点表只增不改」+ 原始记忆分片（不重写）+ 随消息回滚。
4. **向量阈值过低（0.2）召回噪声污染文风**：抬阈值 / 限 Insert#。
5. **关键约束放中段被忽略**：头尾双写（Lost in the Middle）。
6. **合并角色卡导致人格融合**；长 entry 只被部分使用；示例块重复注入。

## 四、仓库现状对照矩阵

判定口径：**已有** = 机制与字段齐备，包只需给用法规范或默认值；**部分** = 机制在但缺策略 / 缺内容；
**缺** = 全仓零命中。行号基于 2026-09-28 `main`。

| # | 外部经验点 | 仓库现状（证据） | 判定 | 去向 |
|---|---|---|---|---|
| 1 | 世界书全字段（次键 / 正则 / sticky / cooldown / probability / scan_depth / role） | `server-rs/src/parsing/world_book.rs:59-118`；激活窗口 `agents/engine/worldbook.rs:43/56`（`scan_depth` 缺省 4） | 已有 | T1 用法规范（不改代码） |
| 2 | 世界书递归 / 预算 cap / min activations / 分组权重 / outlet | 全仓零命中 | 缺 | **T4 出包** |
| 3 | Author's Note（位置 0 尾部注入） | 字段 `preset_tail_prompt`（`settings_service/mod.rs:126`，缺省空）+ 注入 `agents/engine/messages/build.rs:283-288/316-321` | 部分（缝在、内容空） | **T1**（包提供默认内容） |
| 4 | 6 层位置装配 / 头尾分工 | `build.rs:81` + 位置定义注释 `:20-32` | 已有 | — |
| 5 | 「中段易被忽略」→ 头尾双写 | system 侧有（位置 4/5），尾段默认空 | 部分 | T1（增强段 + AN 段） |
| 6 | 滚动 / 分层摘要的触发与预算 | `compaction_mode/threshold/keep_recent/snip_bytes`（`params.rs:279-297`）、消费 `agents/engine/compaction.rs:211/286-289`、`messages/context.rs:93-94`；摘要槽 `messages/inject.rs:292` | 已有（缺推荐档） | T3 推荐档 |
| 7 | 摘要随消息回滚（防漂移） | 零命中 | 缺 | **T4 出包** |
| 8 | 锚点事实表「只增不改」 | MVU 变量树（会话级 `stat_data`，落库）；记忆表 `memory_entries` | 部分（有存储、无纪律与写入候选筛选） | T1 纪律 + T3 推荐档 |
| 9 | 跨会话记忆蒸馏 + 混合召回 | `services/memory_service/`（向量 0.7 + Jaccard 0.3，`mod.rs:109-111`）；注入槽 `inject.rs:322/368`；设置 `params.rs:299-312` | 已有 | T3 参数推荐 |
| 10 | 向量检索的预算 / 阈值 / insert 上限 | `memory_inject_limit`（8）/ `memory_inject_char_budget`（2000）已有；**文档级 RAG（Data Bank 等价物）零命中** | 部分 / 缺 | T3 参数；**T4 出包** |
| 11 | 反 AI 腔清单 + few-shot 正例 | 现有缺省词有基础「规避」条（禁先否后肯 / 动物比喻 / 元评论 / 夸张，`params.rs:427-457`、`build.rs:187-190`），无清单、无正例锚定 | 部分 | **T1** |
| 12 | 输出后正则黑名单重写 | `parsing/regex_script.rs:12/35/160` 机制 + 禁词库（prompt_floor） | 已有（缺可导入的文学向规则集） | T3 候选（规则集常量） |
| 13 | 首条消息决定文风与长度 | `first_mes` 支持（`parsing/character_card.rs`）、开场白编辑弹窗、`regreet` | 已有机制 | T1 写作规范 |
| 14 | 示例对话（mes_example）的效用 | 任务侧 `persona_style` 注入「文风示例」（`task_service/prompt.rs:212-245`）；RP 侧无独立开关 | 部分 | T1 提示词纪律 |
| 15 | stop strings（防代答 / 防复读） | 零命中 | 缺 | **T4 出包** |
| 16 | Trigger 分类（Continue / Impersonate / Quiet） | `agent_mode` 四模式（fast/deep/agent/custom）≠ 动作分类 | 缺 | **T4 出包** |
| 17 | 群聊 / 多角色发言调度 | 零命中 | 缺 | **T4 出包** |
| 18 | Scene beats / director（结构化剧情推进） | 无结构化载体；缺省词内有「模块化剧情」条（`build.rs:194-195`） | 部分 | T1 提示词层；结构化属 T4 |
| 19 | 流程化产出（续写 / 润色 / 校对 / 校声） | 内置流程库仅 1 条 `builtin-coordination`（`services/agent_flow_service/library.rs:112-175`）；二维流程 / 子图 / 节点级模型齐备 | 机制齐备、内容缺 | **T2** |
| 20 | 文风预设（style preset） | 无独立字段（全仓零命中） | 缺 | **T3**（新增选择型字段） |
| 21 | 人称 / 视角 / 时态锚定 | 缺省词有「视角与口吻」条（`params.rs:437-438`） | 部分 | T1 |
| 22 | 一致性维护（人物声音 / 事实不回退） | 缺省词有「逻辑一致」条（`params.rs:452`） | 部分 | T1 |
| 23 | 长篇产出纪律（篇幅 / 分段 / 结尾钩子） | 缺省词有「推进节奏」「句式」「开头结尾」条（`params.rs:442/445/449`） | 部分 | T1 |

## 五、包设计（T1~T3）

> 断言的权威版本在 [`计划.md`](../计划.md) 的 LIT-1~LIT-7；本节给设计推导与取舍理由。

### 5.1 开关与生效语义

| 开关 | 位置 | 默认 | 消费面 |
|---|---|---|---|
| `literary_bundle_enabled` | 扁平字段（`RuntimeSettings`，`#[serde(default)]`） | `false` | 角色扮演引擎侧（**不经 `for_mode`**，直接读扁平快照） |
| `task_literary_bundle_enabled` | 扁平 + `task` 覆盖层（`ModeSettings`，`Option<bool>`） | `false`；覆盖 `None` 沿用扁平 | 任务模式缺省词选择点（对称 `task_coding_bundle_enabled` 的 `for_mode` 前置合并） |

**生效语义（三处，缺省值切换 + 追加注入段）**：

1. **缺省词切换**：提示词框为空（或字段缺省）时，主提示词 / 反思提示词取包内文学增强版；
   **用户自定义值逐字优先**（与编码包同款语义）。
2. **追加「文学增强段」**：开关开启即在位置 4（system 尾）追加一段稳定文本——覆盖存量用户
   （他们已保存过提示词，缺省值切换对他们无效），关掉开关逐字还原。
3. **追加「Author's Note 段」**：位置 0（最新用户消息尾部），随 `preset_tail_role`；
   与位置 4 形成头尾双写（对应 §3.1 的中段忽略问题）。

**为什么不是「一键写入用户字段」**：写入会让包文本变成用户数据（此后包升级不传导、且程序改写用户配置
需要额外审计面）；追加注入段是**纯加性、可逐字回滚**的，符合编码包「只影响缺省值」的克制边界。

### 5.2 注入段的插点取舍（两处候选，实测证据）

| 候选 | 落点 | 位置 / 角色 | `protected_tail` | untrusted 包裹 | 代价 |
|---|---|---|---|---|---|
| **A（尾段）** | `agents/engine/messages/context.rs:402-409`（`preset_tail` 快照后并入） | 位置 0；随 `preset_tail_role`（system 钳为 user） | **不计** → 极端裁剪可被切 | 是（`untrusted_boundary("world_book")`） | 无 system 前缀变化，测试面小 |
| **B（system 尾）** | `agents/engine/messages/build.rs:212-222`（照 prompt_floors 简单注入先例） | 位置 4；system | 计入 → 不可被裁 | 否（与既有注入段同款） | 改动 system 前缀 → 需同步 `build_tests.rs:497/520` 等 5 处 |

**推荐：A + B 头尾双写**——B 承载「不可被裁的创作总纲与一致性纪律」，A 承载「本轮关键约束摘要
（含用户本轮要求的敬语 / 篇幅 / 视角）」。诚实边界：A 段在极端裁剪下可能被切（与 `preset_tail` 同命），
条目里写明，并保证「A 被切时 B 仍在」。

### 5.3 T1 提示词缺省集（零架构改动）

- **主提示词常量集**：在现有 `default_roleplay_agent_prompt()`（`params.rs:427`）基础上增量——
  视角 / 人称 / 时态锚定、反 AI 腔禁用清单 + 正例锚定、show-don't-tell、句式节奏与对话描写配比、
  防复读 / 防代答 / 防元叙事、人物声音一致性、已确立事实不回退、结尾钩子、篇幅纪律。
  **两份并行文本统一管辖**：`params.rs:427`（宏版，`from_config` 与空值回填用）与
  `build.rs:147-201`（分段版，用户清空提示词框时的兜底）必须由**同一个判据函数**决定取哪份，
  避免出现第三份副本。
- **Author's Note 段**：常量文本 + 可选「本轮用户要求」占位符（渲染机制复用既有宏或简单替换）。
- **反思检查项**：`default_reflect_prompt()`（`params.rs:469`）的文学向增补——文风漂移 / 复读 /
  代答 / 替用户决定 / 时间线矛盾；**PASS / FAIL 首行协议不得变**（防解析回归）。
- **任务侧文学变体（Q5）**：任务向缺省词**禁止含 `{{char}}` 等角色扮演宏**（既有测试锁：
  task 默认词不得继承人设词），故任务侧单独一份任务向文学变体，不逐字复用 RP 文本。

### 5.4 T2 流程预设集（复用内置流程库）

- 现状：`services/agent_flow_service/library.rs` 内置仅 1 条（`builtin-coordination`「文学创作协调流程」，
  3 步 draft/reflect/revise）；seeding 只在「库文件缺失」（`:75-82`）或「旧单流程格式且从未编辑」（`:52-54`）；
  库格式下只补 `current_flow_id`（`finalize_library`，`:100-106`）。
- 包新增常量流程：**章节续写 / 润色去 AI 腔 / 一致性校对 / 人物声音校准**；
  开关开启时并入内置集，**加性**——不动既有那条的 id 与文本。
- **门控代价（实测）**：`AgentFlowService` 构造只有 `data_dir / library / registered_tools`
  （`mod.rs:38-42`、`service.rs:14-21`），**读不到 settings** → 开关必须从构造点
  `api/app_state.rs:303` 传参，或按开关选择内置常量集合。
- **归属规则（Q2 推荐：不复活）**：用户编辑 = 原地覆盖（`service.rs:231-249` `set()`），删除后不重建——
  包流程一旦被用户改过就是用户资产，包更新不覆盖。这条是**诚实边界**：代价是包升级不传导，
  收益是「用户改过的东西不会被程序悄悄改回」。
- 消费点三处需一并回归：`api/chat.rs:187`、`api/agent.rs:49`、`task_service/backend_impl.rs:159`。

### 5.5 T3 预设档（新增选择型设置字段）

- **文风预设 `literary_style_preset`**：字符串枚举，缺省 `""` = 不注入；取值集合为常量清单
  （首版清单见 `计划.md` 待拍板 Q3）；未知取值**显式报错**，不静默回退。注入缝与 §5.2 的 A 段同处。
- **记忆 / 压缩推荐档**：不新增独立数值字段，而是给既有字段一组「档位」——缺省 = 不改变（推荐值仅在
  设置页旁注展示），用户显式选档才写入既有字段并可回到「不改变」。涉及的既有字段与钳制区间：
  `compaction_mode/threshold(0.5–0.95)/keep_recent(2–200)/snip_bytes(≤1MB)`、
  `memory_inject_limit(8)/memory_inject_char_budget(2000)/memory_max_entries(200)`、
  `memory_distill_enabled`、`embedding_*`（钳制见 `services/settings_service/secret.rs:95-104`）。
- **接线代价（实测）**：每个新设置字段 ≈ Rust 8 处（`params.rs` 覆盖层 + 默认 + `for_mode` 合并 + 消费、
  `mod.rs` 扁平字段、`api/settings.rs` 的 `UpdateSettingsBody` / 序列化投影 / `apply!`）
  + 前端 4 处（`api/types.ts` ×2、`stores/genSettings.ts` ×4、新 section 组件、
  `SettingsHub.vue:89` / `SettingsModal.vue:28/91/120` / `onboarding.ts:31-34` **三处手工对齐、无自动校验**）。

## 六、T4 出包清单（需动既有实现，逐条理由）

| 能力 | 为何出包 |
|---|---|
| 世界书递归 / 预算 cap / min activations / 分组权重 / outlet | 要改 `agents/engine/worldbook.rs` 的激活算法与 `WorldEntry` 字段面——属既有功能实现变更 |
| 文档级 RAG（Data Bank 等价物） | 需新的分块 / 向量化 / 预算预留管线（现只有会话记忆向量），触及存储与检索层 |
| 群聊 / 多角色与发言调度 | 全新会话形态：发言轮转、按发言者投卡、群提示词——架构面改动 |
| Trigger 分类（Continue / Impersonate / Quiet） | 需在消息装配处按动作切换提示词集，属装配语义变更 |
| stop strings（防代答 / 防复读） | 需连接器参数面新增字段并贯通两模式 |
| 摘要随消息回滚 | 需把摘要与消息元数据绑定（存储与回滚语义） |
| 场景节拍 / director 的结构化载体 | 需新的状态载体与渲染面；目前只能做提示词纪律（T1） |

## 七、切片顺序与工时（概估）

| 序 | 切片 | 主题 | 工时 | 风险 |
|---|---|---|---|---|
| 1 | LIT-1 | 双开关骨架与前后端接线 | 0.5d | 低（照编码包清单） |
| 2 | LIT-2 | T1 主提示词文学增强缺省集（两份并行文本统一管辖） | 0.5d | 低-中（缺省词回归面） |
| 3 | LIT-3 | T1 注入段（位置 4 增强段 + 位置 0 Author's Note） | 1d | 中（system 前缀与裁剪语义） |
| 4 | LIT-4 | T1 反思检查项扩充 | 0.25d | 低（协议不变） |
| 5 | LIT-5 | T2 流程预设集 | 1d | 中（custom / 二维流程回归） |
| 6 | LIT-6 | T3 文风预设选择器 | 1d | 低-中（新字段 12 处接线） |
| 7 | LIT-7 | T3 记忆 / 压缩推荐档 | 0.5d | 低（缺省不改变） |

合计约 **4.75d**；建议 LIT-1+2+3 一个批次（可见效）、LIT-4 搭车、LIT-5 独立、LIT-6+7 同批。

## 八、验收与风险

- **门禁**：改后端跑 `cargo test --manifest-path server-rs/Cargo.toml -j 8`；改前端跑 `npm test -w web`；
  新增测试文件必须回填 `MAINTENANCE.md` 计数（`count-tests.mjs --check` 硬红）；
  新增设置字段走文档三处同步（`契约-协议与配置.md` 回退规则表 + 口径段、`功能.md`、`功能-变更史.md`）。
- **测试锚点（改缺省词 / 注入段必然触及）**：`build.rs:471/506`（system 含「文学创作系统」）、
  `messages/build_tests.rs:198-235/237-270/273-337/340-375/377-406/497/520`（位置顺序与前缀稳定性）、
  `tests/api_misc_assets.rs:259-290`（`preset_tail_role` 往返）、`tests/api_agent_loop.rs:270-311`（反思）、
  `tests/settings_connector.rs:164-226` 与 `tests/tasks_prompt.rs:179-246`（双模式隔离）、
  `settings_service/mod.rs:1200-1230`（空值回填）、`web/src/stores/genSettings.test.ts:362-390`。
- **判据**：开关关 → 一切逐字节等同今天（回归钉子）；开关开 → 三处生效语义可分别断言；
  用户自定义值逐字优先（既有四态测试体例）。
- **主要风险**：① 缺省词与注入段改变现有观感（仅开关开启者，可关回）；② 注入段进 system 会扰动
  前缀稳定性测试与缓存假设（段文本须同轮恒定）；③ 包流程并入内置库后用户编辑不回滚（写明）；
  ④ T3 新字段的三处前端手工对齐漏一即断裂（沿用编码包的人工核对清单，CAP-2 已登记此风险）。

## 九、附录：方法局限与待核实

- **NovelAI Lorebook** 的具体机制（keys / prefix-suffix / insertion order / budget / 靠上下文末尾注入）
  本次因环境无法访问 `docs.novelai.net` 未验证，仅作**低置信参考**，不写入结论。
- 本报告的「外部默认值」多取自官方文档与社区源码（SillyTavern release 分支），**版本会变**；
  引入具体数值（如 budget 25%）时按「用法规范」而非「照抄实现」对待。
- 反 AI 腔词表无权威来源，属社区共识；包内清单应按「可编辑常量 + 可关闭」设计，不做成硬规则。
- 仓库行号基于 2026-09-28 `main`（`da56ee9` 之后本批的 `234b007`）；引用前复核。
