# 「极」模块系 —— 角色扮演模式模块整合策划

> 策划日期：2026-11-06
> 产出方式：本机 CodeBuddy 反代网关 + 混元4 免费模型（`hy4free`）三轮对话——①诊断与目标架构 ②资深评审红队修订 ③失败预演与批次收敛；主评审负责事实校验、坐标修正与最终收敛。
> 证据：`D:/kedai-bench-run/hunyuan-plan-evidence/`（`prompt-1/2/3*.md`、`round1b/2/3.md`、`call-hy.mjs`、原始响应 JSON）。
> 状态：**策划稿，未拍板，未改任何生产代码**。

---

## 零、一句话结论

「极」不是新建一层抽象，而是给角色模式补三条缺失的**结构性契约**：

1. **唯一提示词构建器**——消除「运行时 `finalize_messages`」与「设置页 `prompt_preview`」两套重拼的漂移；
2. **配置只读投影 + 唯一写汇聚点**——收口 `settings.json` / `prompt_floors.json` / `user_persona.json` / `regex_scripts.json` / `AGENTS_RUNTIME.md` / SQLite 六分叉；
3. **前端命令层**——止住 `ChatWindow`（997 行）与三个设置 store 的继续分叉。

但必须诚实说：**这套整合不是当前最该做的事**。它是维护性投资，收益在半年后，不产生用户可见价值。若近期有缺陷或用户任务要修，应让位。若要做，按 4 批核心（JI-1~JI-4）落地即可，不必做满 9 项。

---

## 一、现状诊断（实测）

### 1.1 规模分布

| 面 | 规模 | 关键事实 |
|---|---|---|
| 后端角色模式子系统 | 13 个扁平 service，约 7,500 行 | 世界书 1288、会话 1134、记忆 1085、运行时提示词 868、注入 787、角色卡 648、用户脚本 381、人设 346、契约变更 229、变量 233、正则 219、POV 196、快捷回复 191 |
| 共享原语 | `services/prompt_kit.rs` | 世界书过滤、注入合成、占位符渲染、untrusted 边界、预算选择 |
| 设置面 | `UpdateSettingsBody` 75 字段 | 角色模式专属 36 + 共享生成 4；另有 5 类配置不在 `settings.json` |
| 前端角色模式 | 99 文件、20,995 行 | UI 8/3317、弹窗 14/4738、store 9/2867、composable 10/1799、api 22/2391、专属引擎 36/5883 |
| 主界面单文件 | `ChatWindow.vue` 997 行 | 同时编排消息、脚本、MVU、资源卡、编辑态 |

### 1.2 七个真实断点（按「维护成本 × 扩展阻力 ÷ 迁移风险」排序）

| 序 | 断点 | 证据 | 为什么排这里 |
|---|---|---|---|
| 1 | 提示词两套构建 | `agents/engine/messages/context.rs:507` `finalize_messages` 与 `api/settings/prompt.rs:135` `prompt_preview` 各自重拼，无共享构建器（已核实） | 双轨漂移不可观测；新增素材要改两处；迁移风险最低（可影子对比） |
| 2 | 配置六分叉 | 六种存储介质；导出导入 bundle v1 六段逐个对接 | 收益最高但数据风险最高，只做只读投影与 round-trip，不搬存储 |
| 3 | 前端无统一命令层 | 设置拆 `genSettings` + `modelConn` + `uiPrefs`；`ChatWindow` 997 行 | 前端向下依赖合法，抽 composable 不动协议；能止住膨胀 |
| 4 | 脚本执行边界未契约化 | 后端 `regex_service` 只存储；前端 `render.ts`（669 行）执行；角色脚本走 `scripts/` L3 | 扩展阻力极高；先冻结清单与 trace，不搬执行点 |
| 5 | 四套上下文素材各自为政 | 世界书/记忆/注入/变量在 `context.rs` + `build.rs` 多段拼接 | 动装配顺序会改 prompt 成品，必须在前缀可观测之后 |
| 6 | 后端无角色模式聚合边界 | 13 个扁平 service；settings 36 字段平铺 | 低风险高杠杆，是后续模块的落点 |
| 7 | 记忆模块三职责混杂 | `memory_service/` 1085 行含蒸馏/召回/向量 | 局部内聚问题，涉及表语义，最后拆 |

---

## 二、设计约束（不可违反）

1. **三结合四代际**：L1 只承载兼容契约（`parsing/ models/ contracts/ migration/ utils/`）；L2 编排（`agents/ services/ api/ connectors/ slash/ tools/`）；L3 隔离（`mcp/ plugins/ scripts/` + 前端 `components/ sandbox/`）；entry 只有 `bin/`。归属 SSOT 是 `tools/arch-layers.json`，规则 I/J 强制。
2. **子域不得自称更高代际**。`services/` 整桶登记 L2，门禁看不到子域；因此 `services/roleplay/*` 一律登记为 L2，契约类型必须放真 L1 桶 `models/`。
3. **前缀逐字节稳定**。真实装配顺序为 `system → 摘要槽 → 记忆槽 → 召回槽 → 尾部历史`。现有回归测试：`appending_history_keeps_prefix_bytes_stable`、`probability_entries_do_not_perturb_system_prefix`、`appending_history_keeps_prefix_over_memory_slot`、`recall_slot_keeps_prior_prefix_stable`、`inject_into_mid_history_keeps_prior_prefix_stable`。冻结段 = system + 摘要槽 + 记忆槽（记忆集合不变时）；召回槽与历史属 tail。
4. **唯一写汇聚点**。配置写路径只能有一个 commit 入口；视图是 request-scoped 只读快照，不缓存、不双写（参照世界书 `set_write_hook` 的收敛先例）。
5. **行为与结构不同批**。每批只有一种开关或纯机械搬迁；默认关；旧路径保留。
6. **不新增顶层桶、不改 ST 线格式、不改 SQLite 既有表语义**（可加表/加列）。
7. **真实门禁命令**：`cargo test --manifest-path server-rs/Cargo.toml -j 8`、`npm test -w web`、`npm run build -w web`、`node tools/check-arch.mjs`、`node tools/check-file-size.mjs`、`node tools/check-test-ratchet.mjs`、`node tools/check-settings-parity.mjs`、`node tools/check-doc-claims.mjs`。

---

## 三、「极」模块系

### 3.1 依赖图

```text
entry   bin/（薄装配函数） + api/app_state.rs（既有进程级装配，L2，已登记 wiring）
          │
L2      services/roleplay/{config, context, prompt}  ──→  既有 13 个扁平 service（资源所有者 / adapter）
          │                                                  │
          └──────────────→  models/roleplay_contract（L1 类型 / 常量 / port）
                                        ▲
L3      scripts/ 或 web/src/sandbox/ ───┘  实现 ExecutorPort，只经 L1 类型交互（本批不建）

前端    components/ (L3) ──→ composables/roleplay/ (L2) ──→ stores/ (L2) ──→ api/contracts/mvu/utils (L1)
```

### 3.2 模块表

| 模块名 | 代码名 | 代际 | 真实落点 | 对外接口 | 明确不做 |
|---|---|---|---|---|---|
| 角色模式契约 | `roleplay_contract` | L1 | `server-rs/src/models/roleplay_contract.rs`（或 `models/roleplay/`，`models/mod.rs` 注册） | `PromptSpec` / `PromptPlan` / `ContextFragment` / `ProviderResult` / `FailurePolicy` / `ConfigRevision` / `ExecutorPort` | 不访问 DB/文件；不执行模板/正则；不装配服务；不改表/线格式 |
| 角色模式配置 | `roleplay_config` | L2 | `server-rs/src/services/roleplay/config/` | 只读 `RoleplayProfileView` + 唯一写 `RoleplayConfigCommit` + bundle 六段适配 | 不搬数据；不缓存 Profile；不让 UI 直写；不改旧字段语义 |
| 上下文素材 | `roleplay_context` | L2 | `server-rs/src/services/roleplay/context/` | `ContextProvider` 注册表 + `assemble` + source/priority/budget/failure trace | 不落库；不改既有 service 内部；不拥有向量算法；不执行 regex |
| 提示词构建 | `roleplay_prompt` | L2 | `server-rs/src/services/roleplay/prompt/` | `build_prompt(PromptSpec) -> PromptPlan { messages, prefix_hash, trace }`；send/preview 共用 | 不定义存储；不执行脚本/EJS；不替代 `engine.run` 事务收尾 |
| 脚本沙箱 | `script_sandbox` | L3 | 预留：`scripts/`（后端）与 `web/src/sandbox/`（前端）；**本批不建目录** | 实现 L1 `ExecutorPort`，只收 L1 命令、返回 L1 结果 | 不被 L2 import；不新建插件机制；不搬前端执行点 |
| 前端命令层 | `roleplay_commands` | 前端 L2 | `web/src/composables/roleplay/` | commands / selectors / view-model；只经 store action 或 API DTO | 不替代 `stores/store.ts` / `modals.ts`；不合并三 store；不执行不可信脚本 |
| 装配点 | `compose_roleplay` | entry | `server-rs/src/bin/` 薄函数 + `api/app_state.rs` 注册点 | `compose_roleplay(flags)` 装配 ports/providers/开关 | 不写业务逻辑；不解析 ST；不访问 DB 本体；不新增顶层桶 |

> 装配点不设独立模块目录：SSOT 的 entry 只有 `bin/`，业务装配实际在 `api/app_state.rs`（L2，已登记 wiring）。把它写成新 entry 模块会制造「双装配」假象。

### 3.3 关键边界决策

**在 `services/roleplay/` 下建 L2 薄聚合，不新建顶层模块。**

- 13 个扁平 service 继续是**资源所有者与 adapter**：CRUD、表、文件、缓存都留在原处。
- `roleplay/` 只做四件事：配置投影、ContextProvider 注册、Prompt 构建编排、bundle 适配。
- `engine.run` 仍是事务与收尾边界，不新增角色模式分支。
- 不选新建顶层 `roleplay/`：会直接触发规则 I 的「新增顶层模块」FAIL；若把契约/编排/隔离混在一个桶，规则 J 无法给单一归属；若拆多个顶层，又迫使 API、service、测试路径整体迁移。
- 不选扩 SSOT 到子域粒度：规则 I/J 要改成路径前缀图 + 白名单 + 依赖聚合 + 快照测试全量更新，还会把「名义越代」合法化。

---

## 四、八个关键决策的裁定

| 议题 | 混元4 初案 | 主评审裁定 | 理由 |
|---|---|---|---|
| R1 契约代际 | `services/roleplay` 下自称 L1 | **改**：契约迁入 `models/roleplay_contract.rs`；`services/roleplay/*` 一律 L2 | 按 §2.3，实测代际由依赖图决定；services 整桶 L2，门禁看不到子域 |
| R2 前缀稳定 | 未提 | **补**：冻结段 = system + 摘要槽 + 记忆槽；召回槽/历史属 tail；只从尾向前裁 | 五条既有回归测试锁定；写反会导致 provider 前缀缓存全 miss |
| R3 配置视图 | 「统一 Profile 对象」 | **改**：只读投影；唯一写汇聚点 `RoleplayConfigCommit`；revision + 原子写 + 失效传播 | 防双写、影子状态、半态 |
| R4 hash 判据 | send 与 preview 哈希 100% 一致 | **改**：只在同一 `PromptSpec` 下比较 `messages/prefix_hash/plan.hash`；输入不同只比冻结 prefix 与结构 trace | 两者输入本就不同，原判据不可达 |
| R5 素材失败语义 | 未定义 | **补**：`ProviderResult{Ready,Empty,Degraded,Unavailable}` + `FailurePolicy{Skip,Placeholder,FallbackLastKnown,AbortIfRequired}` + trace | 防把静默降级变成装配失败或空转 |
| R6 前端门面 | 再造一个设置门面 | **改**：只做 composables 命令层，状态真源仍是三 store；不合并、不新增全局 store | 现有 `store.ts` 已是 entry 门面，再造就成第四个设置入口 |
| R7 正则清单 | 「统一清单」 | **补**：真源为 `render.ts` resolver；合并顺序冻结（全局数组序 → 卡内数组序 → 规则数组序）；不去重/不重排/保留 tombstone；只影子生成 | 顺序本身就是 ST 兼容语义，改了输出就变 |
| R8 批次纪律 | 8 批混合 | **改**：6 批 + 3 推迟；每批单开关或纯机械；行为与结构分离 | 降低回归面，保留回滚粒度 |

---

## 五、批次计划

### 5.1 核心批次（建议做）

#### JI-1 契约归位（纯结构，L1）

- **改动面**：新增 `server-rs/src/models/roleplay_contract.rs`（`models/mod.rs` 注册）；同步 `docs/契约-架构与数据.md` §5 清单与 `tools/arch-layers.json` 的 `models` notes。
- **内容**：只放类型与常量——`PromptSpec`、`PromptPlan`、`ContextFragment`、`ProviderResult`、`FailurePolicy`、`ConfigRevision`、`ExecutorPort`。不写任何业务逻辑。
- **开关与回滚**：无开关，纯新增；`git revert` 即可。
- **验收**：`cargo test --manifest-path server-rs/Cargo.toml -j 8`；`node tools/check-arch.mjs`（L1 不得 `use crate::services/agents/api/tools`）；`node tools/check-file-size.mjs`（新文件 <1000 行）。
- **不可逆点**：无。
- **风险**：低。

#### JI-2 分层可观测性（纯只读，不改输出）

> 这是混元4 方案缺的一步。先让「双轨差异」可见，再谈统一构建器。

- **改动面**：`agents/engine/messages/context.rs`（`finalize_messages` 旁路产出 `PromptLayerTrace`）；`api/settings/prompt.rs`（预览复用同一 trace 结构）；新增 trace 测试。
- **内容**：给每次装配输出「层名 / 来源 id / 顺序 / 字节数 / 是否冻结 / 是否命中」的结构化 trace；不改变消息内容、顺序、裁剪。
- **开关与回滚**：trace 默认只在 debug 日志与测试中开启；无设置字段、无行为开关。
- **验收**：五条前缀回归全绿；新增 trace 快照测试；`GET /api/settings/prompt-preview` 返回结构不变（只增可选字段）；真实隔离实例冒烟对比。
- **不可逆点**：无。
- **风险**：低。

#### JI-3 单一构建器影子（结构，L2）

- **改动面**：新增 `server-rs/src/services/roleplay/prompt/`（`mod.rs` / `spec.rs` / `build.rs` / `trace.rs`）；`services/mod.rs` 注册；golden fixtures 落 `server-rs/tests/fixtures/prompt_prefix/`。
- **内容**：从现有 `build_llm_messages_with_card` + `finalize_messages` 抽共享构建器，**先影子调用**：生产路径仍走旧构建器，测试中双跑并逐字节对比。复用 `prompt_kit` 原语，不复制实现。
- **开关与回滚**：无生产开关；影子只在 `cargo test` 运行，零运行时开销。
- **验收**：同一 `PromptSpec` 的 `messages/prefix_hash/plan.hash` 新旧一致；五条前缀回归全绿；`cargo test --manifest-path server-rs/Cargo.toml -j 8 roleplay_prompt`；`node tools/check-file-size.mjs`。
- **不可逆点**：无。
- **风险**：高（踩前缀不变式）。

#### JI-4 预览单切（单行为）

- **改动面**：`api/settings/prompt.rs::prompt_preview` 改调新构建器；旧路径保留。
- **开关与回滚**：新增 `roleplay_preview_build_enabled`（默认 false），同步 `node tools/check-settings-parity.mjs` 登记与 `web/src/stores/genSettings.test.ts` 夹具；关开关即回旧路径。
- **验收**：`GET /api/settings/prompt-preview` 结构断言；`npm test -w web`；隔离实例真实请求对比切换前后层清单；`node tools/check-settings-parity.mjs`。
- **不可逆点**：无。
- **风险**：中。

### 5.2 可选批次（有信号再做）

#### JI-5 素材失败契约（结构，影子）

- **改动面**：`services/roleplay/context/`（provider 注册表 + shadow trace）。
- **内容**：接入记忆/召回 provider 时先定义失败策略与 trace；冻结段禁止实时召回；失败默认 `Skip` 或 `FallbackLastKnown`。
- **验收**：provider failure contract 测试、空召回、embedding 超时降级 trace。
- **不可逆点**：无。

#### JI-6 正则清单影子（结构，不改输出）

- **改动面**：从 `web/src/render.ts` 抽纯函数到 `web/src/utils/regexManifest.ts`；新增 ST fixture golden。
- **内容**：只生成 manifest 与 apply trace，不改变执行路径与渲染字节。
- **验收**：`npm test -w web`；渲染输出 hash 不变；`node tools/check-file-size.mjs`；新增文件登记 `frontendEntryFiles`（若是根文件）。
- **不可逆点**：无。

### 5.3 推迟批次（另立提案）

| 批次 | 为什么推迟 | 重启前置条件 |
|---|---|---|
| JI-7 配置写汇聚与六分叉迁移 | 数据风险最高；需要迁移路径、回滚预案、双库对账 | 出现「设置改了运行时不生效」或导入后不一致的真实缺陷 |
| JI-8 正式 send 切换 | 触碰主链与事务边界；收益已由影子双跑覆盖大半 | JI-3 连续两个版本零 diff，且有真实模型实测证据 |
| JI-9 前端命令层机械抽取 | 装饰层；不解决真实缺陷时只会增加一层转发 | `ChatWindow` 再逼近 1000 行或同类逻辑出现第三份拷贝 |

---

## 六、失败预演（Top 10，按概率 × 破坏力）

| # | 失败模式 | 早期信号 | 预防 |
|---|---|---|---|
| 1 | SSOT 坐标幻觉复发（子域自称 L1/L3、新 entry 模块、假路径） | `check-doc-claims` 报路径不存在；PR 出现 `services/*` 自称 L1/L3 | 只用 `models/`、`scripts/`、`bin/`、`api/app_state.rs` 这些已登记坐标 |
| 2 | 假门禁与漏登记开关 | 找不到脚本；`genSettings.test.ts` 快照变；parity 红 | 每批跑真实命令；新开关一律 `*_enabled` 并同步 parity 与夹具 |
| 3 | 前缀冻结语义写反 | 五条前缀回归任一红；provider cache hit 掉 | 冻结段严格限定 system + 摘要槽 + 记忆槽；只裁 tail |
| 4 | send/preview 哈希比较错 | 为凑一致在 `PromptSpec` 塞 preview-only 字段 | 只断言 same-spec hash；输入不同只比冻结 prefix 与 trace |
| 5 | `render.ts` 真源被抢走 | `frontendEntryFiles` 红；manifest hash 变 | 只影子生成；不排序、不去重、不后端重排 |
| 6 | L3 隔离被越代依赖污染 | 依赖图新增越代边 | L2 不 import L3；绑定只在 `api/app_state.rs` |
| 7 | 正式 send 单切打断主链 | send 前缀回归失败；`engine.run` diff | 推迟；重启时单开关 + 旧路由保留 |
| 8 | Profile 假收敛（双写/半态） | 各介质 revision 不一致；roundtrip 不稳 | 只读投影 + 唯一 commit + revision barrier；迁移另立提案 |
| 9 | Provider 失败变静默语义变更 | 冻结 prefix 随召回变；trace 缺字段 | 失败策略 + trace + 冻结段禁实时召回 |
| 10 | 命名污染与装饰层膨胀 | 新目录无状态却增文件；baseline 涨 | 代码标识符用英文机械名；「极」只留文档 |

---

## 七、命名

**「极」只是模块系名，子模块一律用常规工程名，不再派生子代号。**

| 模块名 | 代码名 | 职责一句话 |
|---|---|---|
| 角色模式契约 | `roleplay_contract` | L1 类型、常量与 port |
| 角色模式配置 | `roleplay_config` | 配置只读投影与唯一写入口 |
| 上下文素材 | `roleplay_context` | ContextProvider 注册与装配 |
| 提示词构建 | `roleplay_prompt` | send/preview 共用的构建器 |
| 脚本沙箱 | `script_sandbox` | L3 执行隔离（预留） |
| 前端命令层 | `roleplay_commands` | UI 与 store 之间的命令与选择器 |
| 装配点 | `compose_roleplay` | entry 薄装配函数 |

命名纪律：

- 「极」只出现在模块系层级：文档标题、路线图、批次代号（`JI-*`）、PR 标签；
- 子模块名不带「极」字，也不使用单字代号；中文名用完整词组，英文名用可搜索的 snake_case；
- 「极」不进代码标识符、开关、日志/metric、API 字段、测试名；调试面板只显示 revision、spec hash、policy key。

---

## 八、诚实的优先级判断

**这不是当前最该做的事。**

- 它不产生用户可见价值，收益是「以后加素材/改设置/排查 prompt 差异更快」。
- 若近期有用户任务、缺陷、发布计划，应优先做缺陷驱动的工作。
- 建议的启动信号（满足任一条再开工）：
  1. 出现「预览与实际生成不一致」的投诉或排查；
  2. 新增一个上下文素材要改 3 处以上；
  3. 设置页改了、运行时没生效；
  4. `ChatWindow.vue` 再次逼近 1000 行，或同类编排出现第三份拷贝；
  5. 需要接入第二种模型方言，prompt 结构差异开始分叉。

**如果只能做一件事**：JI-2（分层可观测性）。它零行为风险，却能让后续所有决策有证据；比直接重写构建器更划算。

---

## 九、验收与回滚纪律

- 每批：默认关或纯影子；旧路径保留；单开关可回；不与行为变更混批。
- 每批收口：全量 `npm run check` + 真实模型冒烟（碰主链时）；纯文档/影子批次按 AGENTS.md 豁免条款跑最小连通冒烟并说明。
- 证据落 `D:/kedai-bench-run/ji-<批次>-evidence/`（命令、日志、golden diff、真实模型响应摘要）。
- 提交体例：`refactor(角色模式): 极模块系 JI-x ...`，正文写「改了什么 / 为何这么改 / 验证证据」。

---

## 十、证据附录

| 项 | 内容 |
|---|---|
| 模型 | `hy4free`（网关返回 `hy4-free`），credit 0，带推理 |
| 第 1 轮 | `prompt-1.md` → `round1b.md`；161.9s；prompt 1815 / completion 13739（reasoning 11483） |
| 第 1 轮失败 | 8192 预算全烧在 reasoning，正文为空（`round1.raw.json`）；印证「推理模型 max_tokens 是预算不是长度」 |
| 第 2 轮 | `prompt-2.md` → `round2.md`；228.5s；prompt 3409 / completion 19563（reasoning 16488） |
| 第 3 轮 | `prompt-3-full.md` → `round3.md`；152.9s；prompt 4254 / completion 13721（reasoning 11198） |
| 本地校验 | `check-arch.mjs` 实跑输出（3 条 wiring）；`arch-layers.json` 后端归属实测；`build_tests.rs` 五条前缀测试；`web/src/render.ts` 669 行；`file-size-baseline.json` 阈值 1000 |

**模型输出的已知错误（已在本稿修正）**：不存在的 `<l1_top>` / `<l3_top>` 占位、`check-ratchet.mjs --rules=I,J`、`pnpm test`、`web/src/render/regex_manifest.ts`、`web/src/viewmodels/`、把 `bin/` 之外的 `main.rs/app.rs` 当 entry、send 与 preview 哈希一致判据。
