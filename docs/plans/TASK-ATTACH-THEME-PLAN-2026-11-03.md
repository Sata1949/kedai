# 任务附件与双主题观感执行计划（2026-11-03）

> **状态**：执行稿，来自 `plans/TASK-BOARD-HANDOFF-2026-11-03.md` 的阶段 3 / 阶段 4。
> **基线**：`main` = `182e0aa`（阶段 0/1/2 已合并 PR #92/#93/#94）。
> **性质**：这是过程稿，收口后按体例并入 `docs/计划.md` 并删除本文件（同 RPFLOW / TMSHOW 先例）。

---

## 决策记录（2026-11-03 拍板）

| 决策点 | 选择 |
|---|---|
| 「提交 4」 | **阶段 4 = 第 4 个提交**（双主题观感） |
| 附件落点 | `<workspace>/.kedai/attachments/` + **自动生成 `.gitignore`** |
| 阶段 4 范围 | 交接文档四项 **+ 默认主题同口径复查** |

提交序列：① 后端数据层 → ② 目标注入 → ③ 前端与台账 → ④ 双主题观感。

---

## 一、提交 1：后端数据层与端点

### 1.1 新表 `task_attachments`（36 → 37 张）

```sql
CREATE TABLE IF NOT EXISTS task_attachments (
  id         TEXT PRIMARY KEY,
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  name       TEXT NOT NULL,
  rel_path   TEXT NOT NULL,
  size       INTEGER NOT NULL,
  mime       TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_attachments_task ON task_attachments(task_id);
```

**四处必须同时登记**（漏一处即被 `tests/schema_migration_meta.rs` 抓红）：
1. `models/db/schema.rs` 的 `CREATE_TABLES`；
2. `migration/ddl.rs` 的 `TASK_ATTACHMENTS_DDL` + `ensure_task_attachments_table`（照 `ensure_task_trace_table` 的 PRAGMA 探测模板）；
3. `models/db/mod.rs` 升级事务内 **+ `migration/merge.rs` 基线侧与源快照侧各一次**（漏一侧合并会报「基线缺少源表」）；
4. `migration/mod.rs` 的 `pub use ddl::{…}` 导出。

计数派生因此变化：`db-tables` 36 → 37、`ensure-in-tx` 23 → 24、`ensure-defs` 25 → 26。

### 1.2 服务层 `services/task_service/attachments.rs`

- `attachment_dir(task)`：绑定工作区 → `<workspace>/.kedai/attachments/`；未绑定 → `scratch_dir_for(id)/attachments/`；
- `store_attachment` / `list_attachments` / `delete_attachment`；
- 文件名 sanitize（拒 `/ \ : ..` NUL 与控制字符）；**展示名保留原名**，落盘名 `{seq}-{safe}`；
- 上限：单任务 10 个 / 单文件 25MB（body 上限 35MB 内留余量），校验在服务层以便单测覆盖；
- `.gitignore` 自动生成：首次建 `<workspace>/.kedai/attachments/` 时，若 `<workspace>/.kedai/.gitignore` 不存在则写 `attachments/\n`；**已存在则一字不改**；未绑定任务（scratch）不生成。

### 1.3 三个端点

`api/tasks.rs` 现 1159 行且被 `tools/file-size-baseline.json` 锁死只降不升，**先搬迁腾空间**：
把 `task_artifacts` / `task_artifact_download` / `ArtifactDownloadQuery` / `scratch_cleanup` 搬进新
`api/tasks/artifacts.rs`，用 `pub use artifacts::*;` 保持路由可见性。

新 `api/tasks/attachments.rs`；`routes/misc.rs` 加两行：
- `/api/tasks/{id}/attachments` → `get(...).post(...)`
- `/api/tasks/{id}/attachments/{att_id}` → `delete(...)`

multipart 复用 `api/characters.rs` 的 `parse_boundary` + `parse_multipart`（`pub(crate)`，返回 `Vec<MultipartPart>`，天然支持多文件），不新写解析器。

详情：`TaskDetailResponse` 加 `attachments: Vec<TaskAttachmentRecord>`。

---

## 二、提交 2：目标注入

新增 `task_engine/attachments.rs` 纯函数，在 `task_engine/mod.rs` 的 `goal_text` 组装处调用：

```
{原 goal}

## 附件
以下文件已随任务提供，路径相对本次任务的工作目录：
- `spec.txt` → `.kedai/attachments/1-spec.txt`（1.2 KB，文本；用 fs_read 读取）
- `shot.png` → `.kedai/attachments/2-shot.png`（240 KB，图像；视觉已开启，用 view_image 查看）
需要附件内容时先读，不要凭文件名猜测。
```

四条硬判定：
1. **legacy 是特例**：`legacy.rs` 用 `task.title` 而非 `ctx.goal`，必须改成 `&goal_text` 并加测试——否则附件段对 legacy 完全无效；
2. **视觉未开启**时图像只写「无法查看画面」，不谎称能看（`settings_service::vision_enabled`）；
3. **不动 `delivery.rs` 与 `ownership.rs`**：附件不进交付必需清单、不进写所有权；
4. **不把 `.kedai` 加进 `workspace_scan::EXCLUDED_SEGMENTS`**——加了 `fs_glob`/`fs_grep` 就看不到附件。副作用是 bash 前后扫描会把附件改动记进 `task_file_changes`，可接受（`change_baseline` 在 run 开始时取）。

`hygiene_issues` 只在根级 `read_dir` 且只收 `is_file()`，`.kedai` 是目录 → 不误报；仍跑一次实测确认。

---

## 三、提交 3：前端与台账

- `api/tasks.ts` 三函数（上传复用 `api/stream.ts` 的 `uploadForm`）；`api/types/task.ts` 加 `TaskAttachment` + `TaskDetail.attachments?`；
- **`stores/task.ts`（1157 行，只降不升）零增长**：新逻辑进 `composables/useTaskAttachments.ts`（模块级单例持 pending 队列）；
- `sidebar/SidebarFooter.vue` 按 `appMode` 分支：**roleplay 分支逐字不动**（`导入角色卡` 文案、`accept`、单文件、`kedai:open-character-import`）；task 分支改「导入附件」、去 `accept`、加 `multiple`；
- 新 `components/task/TaskAttachmentList.vue`（无 props，自取 composable，照 `FileChangesPanel.vue` 体例），挂进 `TaskResultPanel.vue` 最前；
- 时序：`createAndRun` 创建成功 → 上传 pending 附件 → 再 `runTask`；
- 台账：`docs/契约.md`（三端点 + 详情形状 + 映射表）、`docs/功能.md`、`docs/功能-变更史.md`、`docs/计划.md`（批次号 grep 确认未占用）、**`docs/遗留.md` VISION-L5 加追注改判、原文保留**、`MAINTENANCE.md` 计数现取回填。

真实模型实测（不可豁免）：隔离实例上传 `.txt` + `.png` → 验证目标里出现附件段、模型确实读了文件。证据落 `D:\kedai-bench-run\attach1-evidence\`。

---

## 四、提交 4：双主题观感（本批重点）

### 4.1 侧栏表单元控件主题化

`stained-glass.css:829-837` 的「玻璃内凹」只覆盖 `.sv-modal`，侧栏的模式/连接/工作区控件仍是 `rgb(253,248,236)` 白底。补侧栏选择器，并**与既有 `.sv-modal` 那条合并成一个选择器列表、只写一份声明**（同时省 gzip）。

**头号陷阱**：`stained-glass.css:302-305` 的 `.sv-side-head * { color: #f0e8d6 }` 会把侧栏头内的控件文字刷成暖白；若侧栏头有 select（须实测确认），必须照顶栏 `.sv-topbar select` 的先例回退成浅底深字，否则暖白压浅玻璃约 1.2:1。`color` 必须显式给 `var(--kg-ink)`。

### 4.2 sticky 表头与任务详情段带

`.sv-task-sticky` 在主题文件里零命中（沿用 `--sv-paper` + `--sv-line-strong`，凑巧可读）；但 `.sv-task-detail` 的 `--sv-top-stripe` **在彩窗令牌里保留了默认主题的红黑段带原值**（`builtin.ts`）——彩窗下任务详情顶端仍是红黑条，与「石室与光」调性冲突。

改动：段带改用 `--kg-ruby` / `--kg-gold` / `--kg-sapphire`；`.sv-task-sticky` 补背景与底部金线。**不得用 `--kg-panes-band`**（内容层禁拼片）。顺带复核 `.sv-transcript-modes` 的 `.active` 在彩窗下的对比度。

### 4.3 侧栏底部三按钮

`.sv-side-foot` / `.sv-side-btn` / `.sidebar-exit` 主题文件零命中。改 `border-top-color`、描边与文字色到 `--kg-*` 档。

**特异性陷阱**：`.sidebar-exit` 是 `SidebarFooter.vue` scoped（3 class + `[data-v-*]`）；主题层若用 `html[…] .sv-side-foot .sidebar-exit` 可能不够，实测；不够就提到 `.sv-side-foot .sv-btn.sidebar-exit`。**禁止 `!important`**。

### 4.4 gzip 余量：先回收再加

现 11,467 / 12,000（余 533）。顺序必须**先回收 → 再加**：删死规则/死选择器（DOM 命中实测判定）、4.1 与 `.sv-modal` 声明合并。`--kg-panes-band`（53,327 字符 data URI）是 gzip 大头但不能删（契约 1/2 条）。若仍超 12,000，须在 `tools/check-bundle.mjs` 的 `CHUNK_BUDGETS_GZ` 显式登记 `'stained-glass.css': <实测+10%>` 并在脚本头注释写理由。

### 4.5 契约对照（改之前逐条）

- 第 2 条：拼片只许 `.sv-topbar` / `.sv-char-item.active` / `.sv-btn.primary:not(:disabled)` + `.sv-btn-send:not(:disabled)`，数量**恰好等于**白名单 → 4.3 侧栏主按钮**不能引拼片令牌**；
- 第 3 条：`.sv-task-detail` 等内容层不得出现在拼片选择器 → 4.2 段带只能用纯色/渐变；
- 第 4 条：无 `!important`；第 5 条：`.sv-sidebar` 的 `position` 只在 `@media (min-width:768px)`；第 8 条：每个 `--kg-*` 声明都被 `var()` 引用、四个死令牌不得出现；
- `registry.test.ts` 彩窗 8 组令牌对 ≥4.5:1 —— 不动令牌值即不受影响；
- `check-css-discipline.mjs` **不扫 `web/src/theme/`**，主题文件不受行数/裸值 ratchet 约束。

### 4.6 默认主题同口径复查

彩窗侧改完后，用同一套量测对默认主题跑一遍：侧栏表单元控件、`.sv-task-sticky`、`.sv-task-detail` 段带、`.sv-side-foot` 三按钮、`.sv-transcript-modes`。判据同为「对比度 <3:1 为 0 项」+ 同排控件底/边/字色同源。

**只修明确缺陷**；**不推翻既有裁决**（UIPOL-2 的 1060px、UIPOL-4 的 clamp、GLASS-PANE1-L3「空态不补装饰条」一律不动）。属既有知情接受的，登记 `遗留.md` 而不就地改。

### 4.7 实测口径

隔离实例三件套（`PORT` + `DATA_DIR` + `KEDAI_WEB_DIST=web/dist`），1440×900 与 390×844 × 两主题。逐条量测对比度（<3:1 必须 0 项）、眩光/暗部/动态范围、`check-bundle --verbose` 的 gzip。证据落 `D:\kedai-bench-run\task-board-attach-theme-evidence\`。纯样式批次按 `AGENTS.md` 豁免跑一次最小连通冒烟并在提交正文说明。

---

## 五、门禁红点

| 门禁 | 触发点 | 处理 |
|---|---|---|
| `check-doc-claims` `db-tables` | 36 → 37 | `MAINTENANCE.md` + `契约-架构与数据.md` |
| 同上 `ensure-in-tx` / `ensure-defs` | 23 → 24 / 25 → 26 | `契约-架构与数据.md` 五处 / 一处 |
| 同上 `contract-mappings` | 若登记 `TaskAttachment` 则 32 → 33 | 契约.md 两处 + README + MAINTENANCE + 变更史文末 |
| 同上 `api-routes-covered` | 三新端点 | 必须登记 `docs/契约.md` |
| `check-tutorial` | `导入角色卡` 在 `SidebarFooter.vue` | 保留在 roleplay 分支即不红 |
| `check-text-punct` | 新模板文案 | 汉字后半角 `,;:?!` 全改全角 |
| `check-docs` D7 | 计划稿位置 | 落 `docs/plans/` + 登记 `docs/README.md` |
| `check-file-size` | `api/tasks.rs` 1159 / `stores/task.ts` 1157 | 靠搬迁与 composable 保持零增长 |
| `count-tests --check` | 新增用例 | 同批现取回填 `MAINTENANCE.md` |

---

## 六、执行顺序

四个提交按序做，每个自带测试 → 每步跑对应门禁 → 最后全量 `npm run check` + 真实模型实测 → 走 PR（分支保护强制）。
