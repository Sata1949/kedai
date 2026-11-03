# 任务台观感与附件导入 · 交接文档（2026-11-03）

> **状态**：阶段 0/1/2 **已全部合并**（PR #92 / #93 / #94）；**阶段 3（任务附件）与阶段 4（双主题观感）未开工**。
> **基线**：`main` = `534ec93`。本文行号与实测值都是该快照，动手前先复核——本仓行号漂移很快。
> **性质**：这份文档回答三个问题——**「上一轮到底改了什么、为什么改」、「两项未开工的活该怎么落」、
> 「哪些坑踩过一次别再踩」**。结论来自本机真实页面实测（隔离实例 + 1440×900 IAB）与逐文件复核。
> **证据**：`D:\kedai-bench-run\task-rhythm-evidence\EVIDENCE.md`（含修前/修后逐项数字与反例记录）。

---

## 〇、一句话交代背景

用户 2026-11-03 提了四件事：①收敛本地分支；②图一「模式框比别的框小」；③图二「部分太集中，
分散一下」；④任务模式左下角「导入角色卡」改成「导入附件（任何文件）」；外带「用设计 skill 优化
两主题下前端观感」。

四项决策用户已拍板（AskUserQuestion 记录）：

| 决策点 | 选择 |
|---|---|
| 分支收敛 | **两条都合 + 删残留** |
| 附件深度 | **落盘 + 目标注入**（不是纯前端内联，也不是只改入口） |
| 空态版式 | **纵向节奏 + 两栏呼吸**（不动列宽与 1060px 上限，不推翻 UIPOL-2 裁决） |
| 主题范围 | **两主题都调** |

本轮做完 ①②③ + 分支收敛。④ 是阶段 3，与阶段 4 一起留在这里。

---

## 一、动手前必做的基线核对

| 事实 | 当前值 | 权威来源 |
|---|---:|---|
| `main` | `534ec93` | `git log --oneline -1` |
| 后端测试 | **2185**（1695 单测 + 490 集成 / 85 文件） | `node tools/count-tests.mjs` |
| 前端测试 | **1903** 静态 / **1947** 运行时 / 178 文件 | 同上 |
| 门禁阶段 | **24 段**（`-Perf` 关）/ 25 段（开） | `grep -c "Invoke-Stage " tools/check-all.ps1` |
| 契约映射 | **32 组** | `node tools/check-contract.mjs` |
| 全局域 CSS | **4669 行**（基线 4669） | `node tools/check-css-discipline.mjs` |
| 彩窗主题 gzip | **11,400** / 12,000（**余 ~600**） | `node tools/check-bundle.mjs` |

```bash
# 快速核对(在 D:\kedai;cargo 需先 call vcvars64.bat,见 AGENTS.md「本机环境」)
node tools/count-tests.mjs
node tools/check-contract.mjs && node tools/check-arch.mjs && node tools/check-css-discipline.mjs
npm test -w web && npm run build -w web
cargo test --manifest-path server-rs/Cargo.toml -j 8
```

**分支现状**：本地只有 `main` / `kedai-Android` / `kedai-Win`，远端同。三条待收敛分支
（`fix/uifix8-task-head`、`feat/tmshow-3-preference`、`fix/dc3-classifications`）已全部清理，
其中 DC-3 的内容早已全在 main（内容差异为零），另两条经 PR 合入。

---

## 二、上一轮改了什么（三个阶段，可直接查阅）

### 阶段 0-A：UIFIX-8 并入（PR #92，squash `334b58b`）

原分支的三笔提交（档位条提为常驻表头 / 工具参数换行 / 执行设置四行等高）rebase 到 main。
**冲突全在 `MAINTENANCE.md` 的计数行**，两侧数字都已过期——处理口径是「取 main 侧表述，
再用 `node tools/count-tests.mjs` 现取真值回填」，**不要手工协商数字**。
另补一笔 fmt 归一（`sink.rs` 两处断言拆行，分支带入的格式偏差，无语义变化）。

### 阶段 0-B：TMSHOW-3 并入（PR #93，squash `e1a8b2c`）——本批最费力处

UIFIX-8 已把档位条从 `TaskTranscript.vue` 迁到 `TaskStatusPanel.vue` 的 sticky 表头
（新组件 `task/TaskTranscriptModes.vue`），而 TMSHOW-3 改的正是 `TaskTranscript` 里那条控件。

**处理口径：偏好逻辑跟着控件走。**

| 逻辑 | 改前位置 | 改后位置 |
|---|---|---|
| `pickMode`（写任务级覆盖） | `TaskTranscript.vue` | `TaskTranscriptModes.vue` |
| `requestedMode`（高亮用） | `TaskTranscript.vue` | `TaskTranscriptModes.vue` |
| `mode` / `degraded`（实际渲染哪一档） | `TaskTranscript.vue` | **留在原地**（它才是渲染方） |

搬迁后 `TaskTranscript.vue` 删掉了档位条模板、`pickMode`、`totalBadge` 与对应样式；
三处测试断言改为「高亮从 `TaskTranscriptModes` 读、渲染从 `TaskTranscript` 的 `data-mode` 读」。

**不变量仍然成立**：控件显示 `requestedTranscriptMode(override, default)`，
渲染用 `resolveTranscriptMode(override, default, hasTrace)`——
降级只改「实际渲染哪一档」，不回写用户选择。

另解决一处**文档章号撞车**：TMSHOW 章与 GLASS-PANE1 章都占了「三十六」。
`功能-变更史.md:12208` 引用 `§三十六` 指的是 GLASS-PANE1，故保留它为三十六，
TMSHOW **顺延到三十八**（三十七 = RTSPLIT）。现在 `docs/遗留.md` 的三十/三十七/三十八
分别是 拼接玻璃 / 双模式边界 / 三档显示。

### 阶段 1：图一「模式框比别的框小」——确认已由 UIFIX-8 修好，**本批无需改动**

实测（1440×900，默认与彩窗主题一致）：

| 行 | class | 高 | 宽 | 左缘 x |
|---|---|---:|---:|---:|
| 模式 | `select.sv-select` | 40 | 288 | 37.3 / 38 |
| 连接 | `div.sv-inp-row` | 40 | 288 | 37.3 / 38 |
| 执行者 | `div.sv-inp-row`（select 224.7 + 「管理」53.3） | 40 | 288 | 37.3 / 38 |
| 工作区 | `div.sv-inp-row.sv-ws-row` | 40 | 288 | 37.3 / 38 |

根因是 `.sv-select` 带 `flex: 1`（`styles/panels.css:181`），而模式下拉是
`.sv-task-advanced-body`（`TaskComposer.vue` 的 column flex）的**唯一裸 flex 子项**，
`flex-basis: 0%` 在纵轴上压过 `height: var(--control-h)`，高度只剩内容最小高（约 20px）。
UIFIX-8 的 `.sv-task-advanced-body > * { flex: none; }` 把它钉住了。

**注意一个陷阱**：曾尝试给模式下拉包一层 `.sv-inp-row`（与其余三行「同构」），
**这是错的**——行内规则 `.sv-task-new .sv-inp-row .sv-select { width: auto }`
（`task.css:867-873`）会把它从 288px 全宽压成自然宽。包装与等高是两件事，
UIFIX-8 已经用 flex 规则解决了等高，不需要改结构。

「管理」按钮的 2px 黑边 + 硬投影是全局 `.sv-btn` 的硬边教义，与同区「选择…」「清空」一致，
**不是缺陷，别去「修」它**。

### 阶段 2：图二「太集中」——已修（PR #94，squash `534ec93`）

**实测修前**（1440×900，默认主题）：空态容器高 759、内容只有 361；所有间距都是固定档
（模式清单 16px、页脚 12px、四行行距 20px）。纵向分散当时**只写在彩窗主题**
`stained-glass.css`，默认主题完全没有。

**改法**（全部落在 `task/TaskBoardEmpty.vue` 的 scoped 块，不扩全局域行数）：

| 位置 | 改前 | 改后 |
|---|---|---|
| 步骤表 → 模式清单 | `var(--space-4)` 16px | `clamp(var(--space-6), 4vh, 40px)`（实测 36px） |
| 清单 → 页脚提示 | `var(--space-3)` 12px | `clamp(var(--space-5), 3.5vh, 48px)` + 发丝分隔线 |
| 四行行距 | 20px | 24px（`var(--space-3)`） |
| 模式 chips 间距 | 6px | 8px |
| 底部留白 | 337px | **198px** |

窄屏（≤767px）在媒体查询里**还原全部紧凑值**——clamp 的下限在手机上仍会撑出空缝。

**★ 这次踩过又退回来的坑（务必读）**：第一版用 `margin-top: auto` 把模式清单压到容器底部，
实测步骤表（底 544）到模式清单（顶 758）之间出现 **~214px 空缝**，整块读作「上下两段无关内容」，
**比原问题更糟**。故默认主题必须用 clamp 封顶。

**彩窗主题为何保留 `margin-top: auto`**：它的版心是满幅画布、可用高度约多一倍
（grid 高 678 vs 默认 381），两栏撑满 + 两端分布在那里才是对的
（实测模式清单 y=732.8 / 页脚 y=790.2 / 按钮 y=781.3）。**差异由主题可用高度决定，不是漏改**——
两处代码注释与测试注释都写明了这一点，改之前先读。

验收断言在 `components/uipolVisual.test.ts` 的 **A5** 一条源码契约里（两处 clamp、行距、
窄屏还原值、彩窗不得重复 clamp 但必须保留 auto）。

**批次号注意**：本批用 **UIPOL-4**。原提交正文写的是 UIPOL-3，但该号已被 2026-10-31 的
「界面一致性与排版治理」章占用（发送键禁用态 / 导航序号 / 按钮主次三项），
故按「先到先得、不重编号」改用全库确认未占用的 UIPOL-4。

---

## 三、阶段 3：任务附件（未开工，本包最大一块）

### 3.1 现状：为什么不能「只改入口」

| 事实 | 位置 | 含义 |
|---|---|---|
| 任务接口**没有附件字段** | `api/tasks.rs` 的 `CreateTaskBody`（只有 title / executor_id / task_mode / flow_id / workspace 等） | 传附件无处可放 |
| 四个 multipart 端点**都是资产专用解析器** | `/characters/upload`、`/world-books/upload`、`/plugins/tools/upload`、`/prompt-inject/import` | 传任意文件会在解析阶段 400 |
| 任务表无附件列 | `models/db/schema.rs` 的 `CREATE TABLE tasks` | 详情无法返回清单 |
| 聊天附件**不可复用** | `api/chat.rs:264-300` | mime 白名单 + 魔数校验 + 视觉能力位闸门，且只走 `chat/send` |
| 综合导入**不承载二进制** | `ImportBundleModal.vue:58-88` | 只吃 `.json/.md/.txt`（`file.text()` + `JSON.parse`） |
| **没有可复用的「任意文件上传」端点** | 全仓 | 必须新增 |

`遗留.md` 的 **VISION-L5**（任务附件未做）登记至今，判定是「有意不做，待独立批次登记后再做」——
本阶段就是那个独立批次。做完后该条目要**加追注改判**，**不要删除**（仓库纪律：遗留条目保留原文 + 追注）。

### 3.2 落盘位置——这是整块的关键约束

附件必须落在**任务的 fs 作用域内**，否则模型读不到，且不能靠改 `workspace_guard` 绕过
（那是安全判定面，风险最高的一类改动）：

| 任务类型 | 落点 | 依据 |
|---|---|---|
| 绑定工作区 | `<workspace>/.kedai/attachments/` | 作用域根 = 工作区（`task_engine/mod.rs:316-330`） |
| 未绑定 | `<scratch_root>/<task_id>/attachments/` | 未绑定任务的 `scope` 就是 scratch 目录（同处，注释写得很清楚） |

`scratch_root` 来自 `KEDAI_TASK_SCRATCH_DIR` 或数据目录同级 `task_scratch/`
（`config.rs:226-232`）；`scratch_dir_for` 在 `services/task_service/mod.rs:177`，首次使用时创建。

**两个必须自己核实的副作用**：

1. 往用户工作区写 `.kedai/` 目录——若产品口径不接受，唯一替代是改 `workspace_guard` 做
   「附件目录只读放行」，那属于安全判定面改动，**动手前先问**。
2. 交付卫生检查（`task_engine/delivery.rs:226` 的 `hygiene_issues`）**只看工作区根级文件**、
   只认 `_` 前缀与 `.orig/.bak/.tmp/.log/.rej` 后缀——`.kedai/attachments/` 在子目录里、
   不在根级，不会误报。**但改完要跑一次确认**，别凭推理下结论。

### 3.3 建议实现顺序（每步都能独立验证）

1. **表**：`models/db/schema.rs` 加 `task_attachments(id, task_id, name, rel_path, size, mime, created_at)`
   + `idx_task_attachments_task`。旧库走 `CREATE TABLE IF NOT EXISTS`（启动时执行），
   或按既有 `ensure_*` 体例补迁移 + `schema_migration_meta` 元测试——**先确认走哪条路径**，
   补列必须追加在表尾（列序与 `ALTER ADD COLUMN` 逐位一致，否则迁移元测试会抓）。
2. **服务**：新建 `services/task_service/attachments.rs`——`store_attachment` / `list_attachments` /
   `delete_attachment`。文件名 sanitize（拒分隔符、`..`、冒号），**展示名保留原名**，
   落盘名用 `{seq}-{safe_name}` 防覆盖。上限建议 10 个 / 单文件 25MB
   （body 全局上限是 35MB，见 `api/mod.rs:200`）。
3. **端点**（`api/tasks.rs` + `routes/misc.rs`）：
   - `POST /api/tasks/{id}/attachments`（multipart，复用 `api/characters.rs:75` 的
     `parse_boundary` 与 `:99` 的 `parse_multipart`——它是 `pub(crate)`，world_books / plugins /
     prompt_inject 已在复用）
   - `GET /api/tasks/{id}/attachments`
   - `DELETE /api/tasks/{id}/attachments/{att_id}`
   - `GET /api/tasks/{id}` 详情带 `attachments`
4. **目标注入**：`task_engine/mod.rs:377` 的 `goal_text` 组装处（六模式共用同一入口）。
   有附件时追加「## 附件」段，列 `名称 → 相对路径（大小）` 并说明读取方式
   （文本 `fs_read`；图片在视觉开启时 `view_image`，未开启如实说明）。
   **附件不参与交付门槛、不参与写所有权**（`delivery.rs` 与 `models/ownership.rs` 都不要动）。
5. **前端**：`api/tasks.ts` + `api/types.ts` 三个函数 + `TaskAttachment` 类型；
   `sidebar/SidebarFooter.vue` 按 `appMode` 分支（**角色扮演路径必须逐字不动**，
   它挂着 `kedai:open-character-import` 事件与自动选中行为）；任务模式改「导入附件」，
   去掉 `accept`、加 `multiple`；任务详情新增附件清单组件（可删）。
6. **时序**：选中任务时直接上传；无选中任务时先入 store 的 `pendingAttachments`，
   在 `TaskComposer.createAndRun` 创建成功后逐个上传，失败落错误条。

### 3.4 会红的门禁（清单）

| 门禁 | 为什么 |
|---|---|
| `check-doc-claims` 的 `api-routes-covered` | 三个新端点必须登记进 `docs/契约.md`，否则「后端注册 ↔ 契约收录」集合差集直接 FAIL |
| `check-tutorial.mjs:72` | 钉了 `导入角色卡` 字面，但它现在只在角色扮演分支里——**保留在不断言就不红**，别把它从 SidebarFooter 整个删掉 |
| `check-test-ratchet.mjs` | 新增源码文件（`attachments.rs`、附件清单组件）必须带测试 import |
| `count-tests.mjs --check` | 加测试后**同批回填** `MAINTENANCE.md`（数字现取，不背） |
| `check-file-size.mjs` | `api/tasks.rs` 已超千行、在基线里，**只降不升**；新逻辑尽量落进新文件 |

### 3.5 真实模型实测（本阶段必做，不可豁免）

这是**模型链路改动**（附件路径要进模型可见的目标文本），按 `AGENTS.md` 不能走纯样式豁免。
口径：commandcode 网关 + `deepseek/deepseek-v4.1-flash`，隔离实例上传一个 `.txt` + 一张 `.png`，
验证①目标里出现附件段、②模型确实调了该文件（不是幻觉）。证据落
`D:\kedai-bench-run\attach1-evidence\`。凭据在仓外 `D:\kedai-bench-run\realmodel.env`
（`config.rs` 的回退链：目标 `DATA_DIR` 已有 `settings.json` 时 `.env` 的 `OPENAI_*` 不生效，
见 `经验.md` E76）。

---

## 四、阶段 4：双主题观感（未开工）

只做可验证的具体项，**不推翻既有裁决**。已定位的候选：

1. **彩窗主题下表单元控件没有主题化**：`stained-glass.css:829-837` 的「玻璃内凹」
   只覆盖 `.sv-modal input/select/textarea`，侧栏的模式下拉、连接下拉、工作区输入框
   在彩窗主题下仍是默认白底（实测 `rgb(253,248,236)`）——与周围素玻璃面板不同源。
2. **UIFIX-8 引入的 sticky 表头在彩窗主题下的对比度与分隔线**未复核。
3. **侧栏底部三按钮**（导入附件 / 综合设置 / 退出）在彩窗主题下的描边与文字色对齐 `--kg-*`。
4. 空态节奏下沉后，`stained-glass.css` 的重复规则已清理，可继续回收 gzip 余量。

**硬约束（改之前逐条对照）**：

- 不做全局 token 反转 / 整屏暗场；要暗只能局部暗，且改任何背景色前先补 `--sv-on-dark`
  语义变量（**现状仓内确无此变量**，GLASS-C2）。
- 玻璃不用纯白，用暖象牙 `#ece5d6` 一族；对比度 ≥3:1 必须 0 项不达标。
- 拼片只许三处：顶栏窗楣、角色卡选中、主按钮；**内容层（主面板/侧栏/气泡/输入框/模态/hub）
  一律素玻璃**——由 `stainedGlassCss.test.ts` 白名单机检。
- 主题里禁 `!important`；所有规则必须前缀 `html[data-theme='stained-glass']`。
- `.sv-sidebar` 的 `position` 只能写在 `@media (min-width:768px)` 内（否则打穿移动抽屉）。
- 大面板实时 `backdrop-filter` 是**知情不做**（GLASS1-L1）。
- 彩窗 CSS gzip 上限走默认 12,000（现 **11,400**，余 ~600）——加材质前先在
  `tools/check-bundle.mjs` 的 `CHUNK_BUDGETS_GZ` 登记预算。
- 任务台空态**不要再补装饰条**（GLASS-PANE1-L3 + `经验.md` E145），留白用版式解决。

---

## 五、坑清单（踩过一次，别踩第二次）

1. **`MAINTENANCE.md` 的计数冲突不要手工协商**——两侧通常都已过期。取一侧表述后
   用 `node tools/count-tests.mjs` 现取回填，再 `--check` 验证。
2. **`margin-top: auto` 在内容不满高的容器里会拉出巨缝**（实测 214px / 440px 两处）。
   分散一律加 clamp 上限；只有「满幅版心」的主题才配用 auto。
3. **给模式下拉包 `.sv-inp-row` 会把它压窄**（行内 `width:auto` 豁免）。等高问题
   已由 flex 规则解决，不需要改结构。
4. **rebase 已 squash 合并的分支要用 `--onto`**；本批是 rebase 到 origin/main 后强推
   （`--force-with-lease`），推送后一律 `git ls-remote` 验收。
5. **`git push` 带 pre-push hook 在本宿主会被 SIGPIPE 杀**（`经验.md` E69）——
   预跑 `tools/check-all.ps1 -Quick` 留证后 `git push --no-verify`，再 `git ls-remote` 验收。
6. **长提交信息用 `git commit -F <文件>`**，`-m` 会 E2BIG。
7. **Python 改 CRLF 文件会静默失配**——本仓多数 md 是 CRLF，`str.replace` 带 `\n` 会
   匹配不到。要么按行处理、要么先统一换行符，改完 `grep -c "^<<<<<<<"` 复核。
8. **直接 `npx vitest` 在仓库根跑不动 `.vue`**（缺 plugin）——必须在 `web/` 下跑，
   或直接用 `npm test -w web`。
9. **隔离实例启动后先核对日志 `"data_dir":"..."` 行**再写任何东西（`经验.md` E75）。
   收工必杀进程，别碰用户占 3001 的那个。
10. **`check-docs` 的 D3 规则**：新增的 `docs/plans/*.md` 必须登记进 `docs/README.md`
    的 `plans/` 表，否则直接 FAIL。

---

## 六、验收清单（接手后逐条打勾）

- [ ] `main` 与 `origin/main` 同 SHA，本地无游离分支
- [ ] `node tools/count-tests.mjs --check` → OK（数字与 `MAINTENANCE.md` / `README.md` 一致）
- [ ] `cargo test --manifest-path server-rs/Cargo.toml -j 8` → EXIT 0（含 `cargo fmt --check`）
- [ ] `npm test -w web` → 全绿；`npm run build -w web` → 通过
- [ ] `npm run check` 全量绿（24 段；`-LooseAudit` 处理 audit 抓不到 advisory DB 的既有情况）
- [ ] 阶段 3 完成时：三个新端点已登记 `docs/契约.md`；`遗留.md` VISION-L5 加追注；真实模型证据落盘
- [ ] 阶段 4 完成时：`stainedGlassCss.test.ts` 八条契约全绿；彩窗 gzip 未超预算；对比度扫描 0 项不达标

---

## 七、附：本轮 PR 一览

| PR | 标题 | squash |
|---|---|---|
| #92 | fix(界面): UIFIX-8 三项界面改动并入 main（rebase 后） | `334b58b` |
| #93 | feat(任务模式): TMSHOW-3 三档显示档位偏好并入 main（按 UIFIX-8 表头结构重建） | `e1a8b2c` |
| #94 | fix(任务台): 空态纵向节奏下沉——默认主题不再堆成窄带（UIPOL-4） | `534ec93` |

三个 PR 的云端 CI（`check` + `check-ubuntu`）均双绿后才合并。
