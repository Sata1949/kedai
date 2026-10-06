# 文学能力包收尾执行稿（LIT-4 ~ LIT-7）

> **过程稿**：本文件是工作线执行稿，**收口后删除**——条目状态与口径并入 `计划.md` 的
> 「文学能力包（LIT-1~LIT-8）」章，验证证据并入 `功能-变更史.md`。所有行号为 2026-10-06 现读，
> 实施时以现读为准（首批 LIT 落地后旧行号已漂移）。
>
> 上游依据：`docs/计划.md:2421-2502`（LIT-4~LIT-7 条目）、`docs/plans/LITERARY-REPORT.md`
> （T1~T3 设计）、`docs/契约-协议与配置.md:1335`（pack_flows 单一映射点）。

## 〇、已定项（本次拍板，实施期不得擅自变更）

| # | 决策 | 取值 |
|---|---|---|
| Q2 | 包流程被用户编辑 / 删除后是否复活 | **(a) 不复活**——沿用 `seeded_pack_ids` 现状语义，改过 / 删过就是用户资产 |
| Q3 | 文风预设首版清单 | **(a) 4 档**——白描冷峻 / 古典雅致 / 轻小说 / 悬疑冷硬 |
| Q4 | LIT-7 推荐档是否写入既有数值字段 | **(a) 显式选档才写入、可回到「不改变」** |
| Q6 | LIT-7 推荐档数值 | **两档**：中篇 / 长篇（数值见 §2.4） |

## 一、五项实现裁定（新发现口径，随计划一并确认）

1. **LIT-4 的「未自定义」判据 = 逐字等于内置默认（现行版或文学版）**。
   反思提示词的空串是**显式关闭反思**的有效值（`api/settings.rs:729` 注释「空 = 回退机械规则检查」），
   故不能照抄 LIT-2 的「空串即未自定义」回填。若不改判据：`from_config`（`params.rs:650`）
   物化的是非空现行版文本，开包后永不生效（存量用户与新装用户都命中）。
   规则：
   - `reflect_prompt == default_reflect_prompt()` → 按当前开关重物化（关 → 逐字不变）；
   - `reflect_prompt == default_literary_reflect_prompt()` 且开关关 → 回落现行版；
   - **空串保持空**（不回填，保「关闭反思」语义）；
   - 自定义文本两侧都不动。
2. **LIT-5 开关取 OR**：`literary_bundle_enabled || for_mode(AppMode::Task).task_literary_bundle_enabled`。
   依据：流程库是双模式共用设施（消费点 `api/chat.rs`、`api/agent.rs`、`task_service`），
   四条流程本身是创作场景；只按任务侧会漏掉「只想在聊天里用预设流程」的用户。
   加性 + 幂等，开关关者零变化（新增条目不进库）。
3. **LIT-7 不随包开关门控**：该档只写通用字段（压缩/记忆），若门控则「关包后回退入口不可达」。
   行常显、缺省「不改变」；**关包不自动回退数值**（避免悄悄改用户字段）。
4. **UI 落点**：LIT-6 选择器与 LIT-7 推荐档都加在 `web/src/components/settings/LiteraryBundleSection.vue`
   （与 LIT-1 同分区）。
5. **位置 0 拼装顺序钉死**：`激发 → 反思建议 → 文风段 → AN → 预设尾部`。
   文风段插在 AN **之前**，AN 保持最后（LIT-3 既有顺序不动）。

## 二、逐项改法

### 2.1 LIT-4 反思检查项文学扩充（0.25d，低风险）

**落点**
- `server-rs/src/services/settings_service/params.rs:569-599`：`default_reflect_prompt()` 现行文本（五组检查）。
- 新增常量 `LITERARY_REFLECT_ENHANCEMENT`（4 项新检查，与 LIT-2 的段风格一致）：
  - **文风漂移**：词汇 / 句式 / 人称与上文不一致；
  - **复读**：与上一轮开头 / 结尾 / 比喻雷同；
  - **代答**：替用户角色说话或替用户决定；
  - **时间线矛盾**：与前文已发生的事件、时间顺序冲突。
- 新增 `default_literary_reflect_prompt()`：沿用 `default_literary_roleplay_agent_prompt`
  （`params.rs:498-503`）的 `format!` 形状——**现行文本前缀逐字节相同** + 新检查项段。
- 新增判据入口 `RuntimeSettings::reflect_default_prompt(&self) -> String`（挂在
  `roleplay_default_prompt` 旁，`params.rs:603` 之后）。

**接线（两处消费点）**
- `params.rs:650`（`from_config` 物化）：改为先留空、在函数尾按判据入口物化
  （照 `agent_system_prompt` 的 `params.rs:719` 先例，避免引用未初始化的开关值）。
- `secret.rs` load 区（在 `agent_system_prompt` 回填 `:76-78` 之后）：按裁定 1 的逐字比对规则接线，
  注释写明「空串不得回填」的理由。

**协议不动**：`PASS / 通过 / FAIL / 不通过` 首行四值、「只判定不重写」逐字保留（解析侧既有断言不破）。

### 2.2 LIT-5 四条流程预设加性并入（1d，中风险）

**常量（`server-rs/src/services/agent_flow_service/library.rs`，形状照 `coding_pack_flows()` `:168`）**

| id | 名称 | 步骤（action） |
|---|---|---|
| `builtin-lit-chapter` | 章节续写流程 | 列推进要点(direct) → 成文(direct,generates) → 反思(reflect) → 修订(direct,generates) |
| `builtin-lit-polish` | 润色去 AI 腔流程 | 标 AI 腔重灾区(direct) → 重写(direct,generates) → 反思(reflect) → 定稿(direct,generates) |
| `builtin-lit-consistency` | 一致性校对流程 | 提取事实(direct) → 比对矛盾(direct) → 出具问题清单(direct,generates) |
| `builtin-lit-voice` | 人物声音校准流程 | 摘既有台词特征(direct) → 形成声音卡(direct) → 按卡改写(direct,generates) → 反思(reflect) |

- 每条按 `validate_flow` 自检：步骤非空、**至少一个 `action=direct && generates=true`**、
  reflect 步骤不带 `generates`/`system_prompt`/子流程。
- **文学流程不下发任何工具**（`tools: vec![]`）——与编码包「工具面刻意不同」同思路（内容级安全）。

**并入缝（不新开缝）**
- `library.rs:128-135` `pack_flows(coding: bool, literary: bool)`：在 `:133` 占位注释处加
  `if literary { packs.extend(literary_pack_flows()); }`。
- `service.rs:21-40` `new(..., coding_bundle_enabled, literary_bundle_enabled)`；
  `service.rs:47-53` `sync_pack_flows(coding, literary)`。
- `api/app_state.rs:331-340`：新增 `literary_bundle_enabled` 读值（OR 口径，见裁定 2）。
- `api/settings.rs:1190-1209` 写入钩子：同款读值补 literary。
- `mod.rs:37` 加 `#[cfg(test)] pub(crate) use library::literary_pack_flows;`（生产只经 `pack_flows` 消费，
  避免 unused 告警，照 `coding_pack_flows` 先例）。
- **测试调用点 4 处必须同改**：`tests.rs:199 / :231 / :625 / :1746`（漏改编译失败）。

**归属规则（Q2=(a)）**：`merge_pack_flows` / `seeded_pack_ids` 语义**不动**——删过改过不复活；
关包不回收已注入副本（边界写进文档）。

### 2.3 LIT-6 文风预设选择器（1d，低-中风险）

**字段与常量**
- 新字段 `literary_style_preset: String`（扁平，`#[serde(default)]` = `""`），
  取值键：`plain`（白描冷峻）/ `classical`（古典雅致）/ `lightnovel`（轻小说）/ `hardboiled`（悬疑冷硬）。
- 四档文本为**可编辑常量**（`LITERARY_STYLE_PLAIN` 等），写法原则（报告 §3.3）：
  人称 / 视角 / 时态锚定 + **正例锚定**（few-shot 正例优于禁令）+ 禁用清单克制。

**注入缝（位置 0，与 AN 同处）**
- 判据入口 `literary_style_text(&self) -> Option<&'static str>`：开关关或预设空 → `None`。
- `build.rs:92-96` `LiteraryTexts` 加第 4 字段 `style: Option<&'a str>`（`Default` 全 `None`，
  保持「全 None 时与加参数前逐字节一致」）。
- `build.rs:154-157` 处新增 `untrusted_boundary("literary_style", n)` 包裹；
  三个落点（`:177-184` 退化分支 / `:347-352` user 尾 / `:383-388` assistant 尾）按裁定 5 定序。
- **不计 `protected_tail`**（可被裁，诚实边界写成机器可见断言）。
- 装配层 `context.rs:402-406 / :445-447 / :497-501` 补第 4 段取值与传参。
- 预览同步 `api/settings.rs:1729-1736`（仅 `AppMode::Roleplay`），层序与 build 一致。

**校验**
- PUT 未知取值 → 400（照 `task_tool_policy` 先例 `api/settings.rs:789-793`），文案「文学文风预设仅支持
  plain / classical / lightnovel / hardboiled（空 = 不注入）」。
- load 未知值 → 归一化为 `""` 并留注释（对齐 `secret.rs:55-61` 的存量配置回退先例）。

### 2.4 LIT-7 长程一致性推荐档（0.5d，低风险）

**字段**
- 新字段 `literary_recommend_preset: String`（`""` / `medium` / `long`；`#[serde(default)]`）。
- 新字段 `literary_recommend_snapshot: Option<LiteraryRecommendSnapshot>`
  （`#[serde(default, skip_serializing_if = "Option::is_none")]`；3 字段：压缩模式 / 阈值 / 保留条数，
  类型以现读为准）。

**档值（只动三项）**

| 档 | compaction_mode | compaction_threshold | compaction_keep_recent |
|---|---|---|---|
| `medium` 中篇 | `auto` | 0.75 | 6 |
| `long` 长篇 | `auto` | 0.7 | 8 |

依据：外部经验「硬性超 70% 按优先序保留」（阈值 0.7）+「至少回看 4 条行动」（保留条数给余量）；
其余字段（snip / 记忆注入 / 容量 / 蒸馏）**缺可靠数字依据，本轮不动**。

**PUT 语义**
1. 首次选档（快照为空）→ **先存写入前三值**再写档值；
2. 档间切换（`medium` ↔ `long`）→ **不重拍快照**（快照始终是「采纳前」原值）；
3. 选回 `""` → 有快照则恢复**写入前原值**（≠ 恢复默认值），并清空快照；
4. 未知取值 → 400；越界仍由 `secret.rs:95-104` 既有钳制兜底（不新增校验口径）。

**文案**：必须写明「采纳推荐档会修改 压缩模式 / 压缩阈值 / 压缩保留条数 三项，可一键恢复原值」。

## 三、接线清单（每个新字段 12 处）

**Rust 5**：① `settings_service/mod.rs` 扁平字段 + serde 默认；② `params.rs` `from_config` 默认物化 +
判据入口；③ `secret.rs` load 归一化；④ `api/settings.rs` `UpdateSettingsBody` + PUT 校验 + `settings_json`
投影；⑤ 判据消费点（LIT-6：装配层 / LIT-7：自身语义）。

**前端 4**：⑥ `web/src/api/types.ts`（Settings 响应 + `UpdateSettingsBody`）；
⑦ `web/src/stores/genSettings.ts`（ref + load 回填 + save 响应回填 + 对外导出）；
⑧ `LiteraryBundleSection.vue`（新增行，沿用 `queueSettingsSave` 串行保存与失败回滚体例）；
⑨ 测试（`LiteraryBundleSection.test.ts` / `genSettings.test.ts`）。

**文档 3**：⑩ `docs/契约-协议与配置.md`（回退规则表 + 预览层清单 + untrusted source 清单新增
`literary_style`）；⑪ `docs/功能.md`「三十、文学能力包」；⑫ `docs/功能-变更史.md` 新章。

> 导航三处（`SettingsHub.vue` / `SettingsModal.vue` / `onboarding.ts` 的 `SettingsSectionKey`）
> **本轮不动**——两个新控件进既有「文学能力包」分区，不新增分区。

## 四、提交结构（4 个提交，每项独立可回滚）

1. `feat(文学包): 反思检查项文学扩充（LIT-4）`
2. `feat(文学包): 四条流程预设加性并入（LIT-5）`
3. `feat(文学包): 文风预设选择器与长程一致性推荐档（LIT-6+LIT-7）`
4. `docs(台账): LIT-4~7 收口（计划/功能/契约/变更史/计数同步）`

**纪律**：每项**先写失败测试再做最小实现**（`AGENTS.md`）；互不相关的改动不混进同一提交。

## 五、测试清单

**LIT-4**
- 判据入口两态：开关关 → 逐字等于 `default_reflect_prompt()`；开关开 → 含 4 项新检查且首行协议仍四值。
- load 四例：默认文本 ± 开关（重物化 / 逐字不变）、用户在自定义文本（两侧不动）、空串保持空。
- `tests/api_agent_loop.rs:270-311` 既有反思用例不破（首行解析协议）。

**LIT-5**
- `literary_pack_flows()` 过 `validate_flow`（**真实注册工具集**，照 `tests.rs:1911` 体例）。
- 开关关 → 库里无 4 条；开关开 → 1+N，且既有条目（`builtin-coordination` / `builtin-research-demo` /
  `builtin-code-*`）各字段逐字不变；`current_flow_id` 不变。
- 幂等 + 删过不复活（照 `tests.rs:1867 builtin_demo_flow_respects_delete_and_edit_like_packs` 体例）。
- 导出 / 导入闭包往返含包流程（照 `tests.rs:1400` 体例）。
- 集成测试新增 `server-rs/tests/literary_pack_flows.rs`（照 `tests/coding_pack_flows.rs` 体例：
  独立 DATA_DIR、开关三态、关包不回收）。

**LIT-6**
- 判据入口三态（开关关 / 预设空 / 选档 → 文本）。
- 位置 0 顺序断言（激发 → 反思建议 → 文风 → AN → 预设尾部）+ 极端裁剪边界（文风段可被切、
  不进 `protected_tail`，照 `build_tests.rs:1212` 体例）。
- PUT 未知取值 400；设置往返后生效值不变；预览层含文风段且仅角色扮演。

**LIT-7**
- 选档落位（三字段值正确）+ 快照生成。
- 回退恢复**写入前原值**（测例必须构造非默认原值，与「恢复默认值」相区分）。
- 档间切换不重拍快照；关包不自动回退（边界断言）；未知取值 400。

**前端**
- `LiteraryBundleSection.test.ts`：两行新增控件的渲染 / 补丁字段名逐字 / 失败回滚不波及其他控件。
- `genSettings.test.ts`：两字段回填与缺字段兜底。

## 六、验证命令与真实模型实测

```powershell
cargo test --manifest-path server-rs/Cargo.toml -j 8
npm test -w web
npm run build -w web
node tools/count-tests.mjs --check      # 计数同步后
node tools/check-bundle.mjs             # 体积余量仅 1,212 gz（0.42%），动手前后各跑一次
npm run check                           # 全量 17 段（收口判据）
```

**真实模型实测**（`AGENTS.md` 纪律；证据落 `D:\kedai-bench-run\lit-4-7-evidence\`）
- LIT-4：开包后 `GET /api/settings` 看反思提示词含新增检查项；角色扮演真实一轮走完反思步骤不崩。
- LIT-5：开包后流程库含 4 条；真实任务跑一条包流程（custom 模式）走通。
- LIT-6：选档后预览含文风段且顺序正确；真实角色扮演一轮输出风格可辨。
- LIT-7：选档后三字段值变化；回「不改变」后恢复写入前原值。

隔离实例启动（先核对日志 `"data_dir"` 行）：
`MSYS_NO_PATHCONV=1 DATA_DIR=D:/kedai-bench-run/data-lit47 PORT=30xx LOG_DIR=... server-rs/target/debug/kedai-server.exe`

## 七、边界与风险

- **体积**：前端余量 0.42%；新控件在懒加载设置分区 chunk 内（不占首屏），超预算按 LIT-3 先例
  （506,000 → 557,300）调 `check-bundle` base 并在台账说明。
- **`pack_flows` 签名变更**：生产 2 处 + 测试 4 处必须同改（漏改编译失败，安全）。
- **不写用户字段**：LIT-4 / LIT-6 关包逐字还原；LIT-7 的写入是**显式动作 + 可回退**，
  属例外并写进文案与文档。
- **不顺手做**：LIT-8 出包清单任何一项、运行时提示词文本改写、插件机制、双模式隔离
  （任务侧缺省词禁角色扮演宏）。
- **双模式隔离**：位置 0 / 位置 4 段仅角色扮演；任务侧只受 `task_literary_bundle_enabled` 影响。
- **文档漂移**：条目内引用的旧行号（2026-09-28/10-05 基线）落地时以现读为准。

## 八、LIT-8 出包清单（本包不做，防顺手加）

世界书递归 / 预算 cap / `min activations` / 分组权重 / outlet；文档级 RAG；群聊与发言调度；
Trigger 分类；stop strings；摘要随消息回滚；场景节拍 / director 结构化载体。
理由与逐条边界见 `计划.md:2504-2517`。
