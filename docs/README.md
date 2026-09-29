# docs/ 索引 —— 六类文档体系

> 本目录分两类:**六类活文档**(契约/功能/计划/展望/经验/遗留,改代码时需同步)与
> **归档**(已整合的历史文档,仅供溯源,不再更新)。
>
> **物理约定:`docs/` 根下只允许六类活文档 + 本索引 + `fixtures/` + `archive/` + `plans/`。**
> 该约定由 `tools/check-docs.mjs` 强制(规则 D2),新增文档若不属于六类,须先在本文登记其分类。
>
> **2026-09-16 整合说明**:本目录与仓库根原有 **68 份**过程文档(含根目录 `API.md`),其中 **50 份**
> 内容按六类**移植**到下列九份文档中、原文件移入 `archive/2026-09-16-consolidation/`
> (其余 13 份为更早的过程稿,详见文末「归档」章)。
> 旧路径 → 新章节的映射表见 [archive/2026-09-16-consolidation/README.md](archive/2026-09-16-consolidation/README.md)。

## 六类的定义与判定规则

新内容该写进哪份文档,按下列判据**逐条套用**(先命中先定):

| 类 | 判据 | 判据的反面 |
|---|---|---|
| **契约** | 改了它,旧 exe / 旧数据 / 旧前端会坏。字面规定,不得随手改 | 只是实现细节(改了对使用方无感)→ 功能 |
| **功能** | 代码里已跑通、有实测证据。判据是「现在能用」 | 只有方案没落地 → 计划;有缺陷 → 遗留 |
| **计划** | 有批次号 / 有改动点 / 有验收断言 / 有人在跟踪 | 只有方向、无改动点 → 展望;条目本身是问题 → 遗留 |
| **展望** | 有方向判断,但无改动点、无验收断言、需外部触发条件 | 有验收断言 → 计划 |
| **经验** | 能写全「现象 → 根因 → 做法」三段,跨任务可复用 | 只是「某功能没做」→ 计划/遗留 |
| **遗留** | 现状行为 + 判定(缺陷/有意裁剪/知情接受/待验证) + 归宿 | 有改动方案 → 计划(并在此双向引用) |

两条最容易混的边界:

- **遗留 vs 计划**:条目本身是**问题** → 遗留;条目是**改动方案** → 计划。两边用同一 ID 互相引用。
- **遗留 vs 展望**:「有意裁剪」类若无方向 → 只留遗留(判定「接受」);若有方向与触发条件 → 遗留记现状、展望记方向。

## 六类视图(本目录全部活文档)

| 文档 | 类 | 承载内容 |
|---|---|---|
| [契约.md](契约.md) | 契约 | 契约登记表、**已移植的 HTTP 端点字面契约**（覆盖面以该文「机检状态总表」第 6 节的缺口登记为准）、SSE 线格式、错误体与状态码、线格式冻结清单、机检状态总表 |
| [契约-协议与配置.md](契约-协议与配置.md) | 契约 | mvu / @INJECT / GENERATE-RENDER 三协议、双模式提示词边界、授权契约、配置与数据目录契约(`DATA_DIR` 三级优先级、3001 代理硬契约)、平台分支契约、构建与发布契约(指纹 sidecar、版本 8 处单源、签名现状)、文档与提交纪律、新增内置工具 6 处登记清单 |
| [契约-架构与数据.md](契约-架构与数据.md) | 契约 · **架构叙述权威** | 三结合分层架构(四代际判据、判定程序、允许出边、晋升规则、偏差登记、结构性遗留)、代际归属事实源对账、数据库 schema 与降级契约 |
| [功能.md](功能.md) | 功能 | 能力面总览(双顶层模式:角色扮演会话 / 任务工作台):聊天主链路 / 上下文与压缩 / 世界书与注入 / 角色卡兼容 / Agent 引擎 / 六模式任务引擎(含编码能力包) / 工具与授权 / 变量与契约引擎 / 记忆 / 技能与 MCP / 可观测性 / 前端 / 质量基线 |
| [功能-变更史.md](功能-变更史.md) | 功能 | 逐轮已实现条目的证据链(23 份变更说明 + 已完成计划批次),含 commit hash、验收命令与当时实测数值;另设**未做项索引**章(交接清单,置于 2026-09-26 之前、非文末) |
| [计划.md](计划.md) | 计划 | 未实现但有明确改动方案的条目(活跃性能批次 P-8~P-13、工程韧性批次 5~8、残余任务、二维自定义工作流、Android 阶段 5、架构欠账、安全残余);含「明确不做」与**状态漂移裁定登记** |
| [展望.md](展望.md) | 展望 | 有方向、无方案的长线议题(产品与分发、有意裁剪的补齐路径、**自定义流程的编排表达力**、平台与连接器、架构晋升、本机算力与 GPU、**已实测证伪的方向**、登记备查的未采纳方向) |
| [经验.md](经验.md) | 经验 | 开发踩坑与可复用教训,按环境/构建/测量/架构/并发/测试/文档/协作八组;**原 `MAINTENANCE.md` §10 条目 1~32 已整体迁入,编号为全仓引用锚点**(此后新增条目 33 与 E1~E69 系列沿用同一编号纪律) |
| [遗留.md](遗留.md) | 遗留 | 已确认缺陷与知情接受的残缺(L1~L27、T1~T5、D1~D5、测试空洞、性能未达项、平台待验证、接受不修的设计边界);含**已封堵项索引**留溯源 |

## 事实权威优先级(2026-09-14 确立,2026-09-16 随整合改指)

同一件事实在多个文档出现时,以下列顺序为准(**冲突时高优先级胜出,并须回填低优先级**):

| 权威 | 管什么 | 位置 | 守护机制 |
|---|---|---|---|
| **代际归属权威** | 模块属 L1/L2/L3/entry、允许的跨代方向、已登记的越代债务 | `tools/arch-layers.json` | `tools/check-arch.mjs` 规则 I/J(**未登记即 FAIL**) |
| **数字权威** | 测试数量、构建步骤、命令、门禁清单等一切可计数事实 | `../MAINTENANCE.md` | `tools/count-tests.mjs --check`（测试数）+ `tools/check-doc-claims.mjs`（其余由代码/脚本派生的计数） |
| **叙述权威** | 为什么这样分层、判据、纪律与晋升规则 | [契约-架构与数据.md](契约-架构与数据.md) §三结合分层架构(原 `docs/ARCHITECTURE-3H.md`,已归档) | 须向 `arch-layers.json` 对账 |
| **字面契约权威** | HTTP 端点字段、SSE 负载、线格式冻结值 | [契约.md](契约.md) / [契约-协议与配置.md](契约-协议与配置.md) | `tools/check-contract.mjs`(21 组 MAPPINGS,未登记不校验) |
| **条目编号权威** | 遗留 L/T/D 编号、计划批次号、经验条目 1~33（含重复的 11）与 E1~E69 | [遗留.md](遗留.md) / [计划.md](计划.md) / [经验.md](经验.md) | **编号不得重编** —— 源码注释按编号引用 |

**CI 现状(2026-09-29)**:两个 workflow 文件在位(`ci.yml` 权威档 / `ci-linux.yml` 平台可移植性档;
触发面限 `push` 到 `main` + PR + 手动 `workflow_dispatch`,平台分支刻意不触发),但**服务端已停用**
(`gh workflow disable`,状态 `disabled_manually`;缘见 `遗留.md` CI-BILL-1)、**提交暂转本地**——
恢复用 `gh workflow enable ci|ci-linux -R Sata1949/kedai`,完整清单见 CI-BILL-1 追注。
生效的闸门是 `build.ps1` 的 `[0]` 阶段、`tools/check-all.ps1` 与 `tools/hooks/pre-push`(详见 `../MAINTENANCE.md` §0)。

**纪律**:架构文档不再硬编码测试数字(那类事实一律引用 `../MAINTENANCE.md`);若与代码实测不符,
以代码为准并**先修文档再动代码**。

## 未迁入本目录的根文档(原位保留,不在归档范围)

| 文档 | 为何留原位 |
|---|---|
| [../AGENTS.md](../AGENTS.md) | 宿主读取的开发说明(开发 agent 契约),须在仓库根 |
| [../README.md](../README.md) | 对外产品说明,读者在仓库根找入口 |
| [../MAINTENANCE.md](../MAINTENANCE.md) | **数字权威**;被 `tools/count-tests.mjs` 与 `tools/bump-version.ps1` 按路径硬读取,移动即门禁失效。§6/§10/§12/§14 的正文已迁入六类文档,原位保留小节标题 + 条目号索引 |
| `../API.md`(已归档) | 端点字面契约全部移植到 [契约.md](契约.md),原文件移入 `archive/2026-09-16-consolidation/API.md` |

## 过程稿目录(`plans/`)

`plans/` 收纳**工作线**过程稿——某条线的**调查报告**与**方案调研**。它们**不是六类活文档**:
生命周期由各自抬头声明,不要求随代码同步,归档判据也不适用。
2026-09-27 从仓库根移入本目录,理由是入口清晰(根目录只留 `README.md` / `AGENTS.md` /
`MAINTENANCE.md`),**不是**把它们升格为活文档。`tools/check-docs.mjs` 的 D2 只登记本目录白名单,
不校验其内部链接与内容时效——引用其中的事实时先复核现状。

> **2026-09-27 台账收口**:原先各工作线的**执行计划稿**(`QUALITY-FIX-PLAN` / `HARNESS-PLAN` /
> `TASK-MODE-FIX-PLAN` / `PRODUCT-CAPABILITY-PLAN` / `FRONTEND-FIX-PLAN` / `COMPUTER-USE-PLAN`)
> 已把**开放项与待拍板决策**并入 [`计划.md`](计划.md) 的「产品能力批次(PRODCAP-1~5)」与
> 「其余临时执行稿」两章,原稿删除;已完成部分见 `功能-变更史.md` 与 `遗留.md`。
> **本目录自此只留调研产出**,不再放执行稿——执行稿的台账一律以 `计划.md` 为唯一出口。

| 文档 | 性质 |
|---|---|
| [plans/FRONTEND-REPORT.md](plans/FRONTEND-REPORT.md) | 前端调查报告(三批修复的依据;开放项已并入 `计划.md` 的 FE-1~FE-12) |
| [plans/COMPUTER-USE-REPORT.md](plans/COMPUTER-USE-REPORT.md) | computer use 能力调查报告(提交 1~6 的依据;缺陷清单 CU-1~CU-10 见 `计划.md`) |
| [plans/COMPUTER-USE-HARNESSES.md](plans/COMPUTER-USE-HARNESSES.md) | 成熟 harness 的 computer use 方案调研(ZCode 本机实物 + 跨方案共识定律) |
| [plans/DOC-DRIFT-REPORT.md](plans/DOC-DRIFT-REPORT.md) | 文档漂移盘点报告(六类 84 点:契约字面/状态过时/文档矛盾/ID 断链/漏登记/其他;修正项见 `计划.md` 文档漂移收口批) |

> **同名撞车提醒**:[archive/TASK-MODE-FIX-PLAN.md](archive/TASK-MODE-FIX-PLAN.md) 是更早的
> 「Kimi 续做」交接稿(归档,只读);其同名的工作区执行稿已于 2026-09-27 并入 `计划.md` 并删除。
> 在六类文档里引用时写全路径,别只写文件名。

## 归档

- [archive/2026-09-16-consolidation/README.md](archive/2026-09-16-consolidation/README.md) —— 本次整合移入的 50 份原文档(含根目录 `API.md`)的索引与「旧路径 → 新章节」映射表。
- `archive/` 下**其余 13 份**为更早的过程交接类文档(2026-09 上旬的修复计划、学习笔记、批次设计稿),
  **只读不改**,索引见本文末节。
- 归档目录内文档的 `文件:行号` 引文停留在归档时的代码状态,不再复核;需要现行事实时以
  六类活文档与代码为准。

### 早期归档(2026-09-16 之前)

| 文档 | 说明 |
|---|---|
| [archive/CHANGELOG-role-mode-fixes.md](archive/CHANGELOG-role-mode-fixes.md) | 2026-09-06 角色模式 5+3 个 bug 的修复记录(历史快照,测试计数为当时数值) |
| [archive/TASK-MODE-FIX-PLAN.md](archive/TASK-MODE-FIX-PLAN.md) | 任务模式修复计划(「Kimi 续做」交接) |
| [archive/任务模式修复-变更说明.md](archive/任务模式修复-变更说明.md) | 上一轮任务模式修复的变更记录(被任务模式重构取代) |
| [archive/任务模式重构-实施计划-v2.md](archive/任务模式重构-实施计划-v2.md) | 重构实施计划 v2 |
| [archive/plan2-variable-scopes.md](archive/plan2-variable-scopes.md) | 变量作用域批次计划 |
| [archive/plan4-slash-quick-replies.md](archive/plan4-slash-quick-replies.md) | slash 命令与快速回复批次计划 |
| [archive/plan5-audio-render-panels.md](archive/plan5-audio-render-panels.md) | 音频与渲染面板批次计划 |
| [archive/plan6-ecosystem-devtools.md](archive/plan6-ecosystem-devtools.md) | 生态与开发者工具批次计划 |
| [archive/learn-harness-2026-08.md](archive/learn-harness-2026-08.md) | harness 学习笔记(未采纳方向已登记进 [展望.md](展望.md) §七) |
| [archive/learn-deepseek-harness.md](archive/learn-deepseek-harness.md) | deepseek-harness 借鉴建议(同上) |
| [archive/kedai-agent-coordination.md](archive/kedai-agent-coordination.md) | 多 agent 协作约定 |
| [archive/context-optimization-brief.md](archive/context-optimization-brief.md) | 上下文优化简报(其中「1 秒轮询/不走 SSE」已被任务事件 SSE 取代) |
| [archive/contract-drift-2026-09-09.md](archive/contract-drift-2026-09-09.md) | 前后端契约漂移登记(#1-#12 全数修复,漂移检查已由 `tools/check-contract.mjs` 自动执行) |

## 常见任务路由(我要做 X,该读哪份)

| 我要… | 先读 |
|---|---|
| 改 HTTP 接口 / 加字段 | [契约.md](契约.md) §HTTP API + §线格式冻结清单,再跑 `node tools/check-contract.mjs` |
| 改一个枚举 / 事件 kind / 状态串 | [契约.md](契约.md) §SSE 线格式(枚举 ↔ 线格式对账表) + `server-rs/src/models/types.rs` |
| 加双模式设置字段 | [契约-协议与配置.md](契约-协议与配置.md) §双模式提示词边界(继承/隔离判定口诀) |
| 改 schema / 加列 / 加表 | [契约-架构与数据.md](契约-架构与数据.md) §数据库 schema 与降级契约(必读「只增不改」不变式与冻结基线纪律) |
| 判断某模块属于哪一代 | `tools/arch-layers.json`(归属 SSOT),叙述见 [契约-架构与数据.md](契约-架构与数据.md) §三结合分层架构 |
| 新增内置工具 | [契约-协议与配置.md](契约-协议与配置.md) §新增内置工具登记清单(6 处,漏改静默降级) |
| 发布 / 打包 / 版本号 | [契约-协议与配置.md](契约-协议与配置.md) §构建与发布契约 + `../MAINTENANCE.md` §4 |
| 接手一段陌生代码 | [功能.md](功能.md)(能力面)→ [契约.md](契约.md)(接口)→ [经验.md](经验.md)(该领域的坑) |
| 判断「这是 bug 还是有意为之」 | [遗留.md](遗留.md)(含「接受不修的设计边界」一节) |
| 想知道某功能什么时候改的、怎么验的 | [功能-变更史.md](功能-变更史.md) |
| 找下一批要做什么 | [计划.md](计划.md)(含「明确不做」,先看它免得重复提议) |
| 判断某想法值不值得重新提 | [展望.md](展望.md) §六「已实测证伪的方向」 |
| 排查环境 / 构建 / 并发的怪问题 | [经验.md](经验.md)(条目 1~33 与 E1~E69 为历史踩坑,编号可被代码注释引用) |

## 维护纪律

1. **新增文档前先读本文的分类判据**;不属于六类的,先改本文登记分类,否则 `check-docs.mjs` 会 FAIL。
2. **编号不得重编**:遗留 L/T/D、计划批次号、经验条目 1~33(数字系列,含重复的 11)与 E1~E69 是全仓引用锚点(源码注释按编号引用)。
3. **可计数事实只写一处**:测试数量/构建步骤引用 `../MAINTENANCE.md`,不要在各文档手抄。
   其余**由代码或脚本派生**的计数(数据库表数、`ensure_*` 个数、`check-contract` 映射组数、
   `check-arch` 规则字母与两个 ratchet 基线、`SseEvent`/`TaskEventKind` 变体数、`check-all` 阶段数)
   受 `tools/check-doc-claims.mjs` 守护——**可以写,但必须写对**;改了对应代码/脚本要同步文档,
   历史层(「原文 + 变更标注」)保留旧值并加日期化复核注,该脚本不判失配。
4. **改门禁要验证两侧报告都输出**:`tools/check-arch.mjs` 曾因前端分支写进后端失败桶导致报告段被静默吞掉(见 `../MAINTENANCE.md` §0 修复纪律与 [经验.md](经验.md))。
5. **归档不改写历史**:`archive/` 下文档只读不改。
