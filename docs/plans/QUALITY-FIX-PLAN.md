# Kedai 代码质量修复计划（QUALITY-FIX-PLAN）

> 依据：2026-09-27 对仓库 `main`（HEAD `92b09d2`）的代码质量评审。
> 定位：本稿是**执行稿**——每项含「改了什么 / 为何这么改 / 验证证据（如何验收）」三要素，
> 与仓库既有《FRONTEND-FIX-PLAN.md》《TASK-MODE-FIX-PLAN.md》同体例。
> 批次间无强依赖的可并行；有依赖的在条目内注明。

## 评审结论摘要（修复对象）

| 编号 | 缺陷 | 严重度 | 批次 |
|---|---|---|---|
| Q1 | 巨型源码/测试文件 5 个（148KB / 176KB / 137KB / 129KB / 66KB） | 高 | 批次 1 |
| Q2 | 根目录过程稿 ~280KB 堆积，入口不清晰 | 中 | 批次 2 |
| Q3 | 缺 `rust-toolchain.toml`，工具链版本只锁在 CI 注释与文档里 | 高 | 批次 3 |
| Q4 | CI 单平台（windows-latest），Android 分支无覆盖 | 中 | 批次 4 |
| Q5 | 技术债只登记在文档（docs/经验.md、docs/遗留.md），无 issue 跟踪 | 低 | 批次 5 |
| Q6 | 单人直推 main，无 PR 状态检查兜底 | 低 | 批次 6 |

---

## 批次 1：拆分巨型文件（Q1）

### 现状证据

| 文件 | 大小 | 性质 |
|---|---|---|
| `server-rs/src/services/agent_flow_service.rs` | 148 KB | 生产代码 |
| `server-rs/tests/tasks.rs` | 176 KB | 集成测试 |
| `server-rs/tests/api_integration.rs` | 137 KB | 集成测试 |
| `web/src/style.css` | 129 KB | 全局样式 |
| `server-rs/src/services/memory_service.rs` | 66 KB | 生产代码 |

### 原则

- **纯移动，不改行为**：每刀只移动代码、改 `mod` 声明与 `use` 路径，不改任何函数体；
  若拆分中发现死代码或重复，**另起提交**处理，不与移动混在一起（保持 diff 可 review）。
- **一刀一提交**：每个文件拆分为独立提交，提交信息按体例写明映射表（旧位置 → 新位置）。
- **门禁兜底**：拆分只靠「全量测试 + clippy + count-tests --check 计数不变」验收；
  测试文件拆分后 `count-tests.mjs` 的文件数口径会变化，需同提交更新文档计数
  （这正是既有门禁设计来抓的事，让它正常工作，不要绕过）。

### Q1-1 `tests/tasks.rs`（176KB）→ 按 API 域拆分

- **改了什么**：拆为 `tests/tasks/` 目录（Rust 集成测试支持目录入口 `tests/tasks/main.rs`，
  但更推荐平铺多个文件）：`tasks_crud.rs` / `tasks_events_flow.rs` / `tasks_terminal.rs` /
  `tasks_modes_legacy_solo.rs` / `tasks_modes_plan_team.rs` / `tasks_budget_idle.rs`。
  共享辅助函数收敛到 `tests/macros.rs` 已有的公共层或新建 `tests/common/mod.rs`。
- **为何这么改**：17 万字节单文件已无法导航；按域拆分后新增测试有明确落点，
  也降低多会话并行写测试时的冲突面（本仓库已实测存在多会话并行推送）。
- **验证证据**：`cargo test --manifest-path server-rs/Cargo.toml -j 8` 全绿；
  `node tools/count-tests.mjs --check` 通过（用例总数不变、文件数口径同提交更新）；
  `git diff --stat` 确认除移动外无逻辑改动。

### Q1-2 `tests/api_integration.rs`（137KB）→ 按路由域拆分

- 同 Q1-1 手法，按 `/api/*` 路由域切 4~6 个文件。
- 验收同 Q1-1。

### Q1-3 `agent_flow_service.rs`（148KB）→ 按流程阶段抽模块

- **改了什么**：转为 `services/agent_flow_service/` 目录模块：`mod.rs`（对外 re-export，
  **公共 API 签名一个不动**）+ 按职责切分内部实现（编排/事件/落库/拼装等，以实际代码
  内聚性为准，切分前先在提交信息里给出映射表草案）。
- **为何这么改**：这是全仓最大生产文件，且是 agent 流程核心，改它的人最多；
  保持 `mod.rs` re-export 可让所有调用点零改动。
- **验证证据**：`cargo test -j 8` + `npm run check` 全量 16 段全绿；
  `check-arch.mjs` 无新增违规；`git grep` 确认外部 `use` 路径全部不变。

### Q1-4 `style.css`（129KB）→ 按组件域拆分

- **改了什么**：拆为 `web/src/styles/` 下按域分文件（base/layout/chat/settings/task 等），
  入口 `style.css` 只留 `@import` 或改由 `main.ts` 逐个 import；构建产物由 vite 合并，
  **bundle 体积不变**。
- **为何这么改**：129KB 单 CSS 文件冲突面大、无树摇可言；按域拆分与 Vue 组件边界对齐。
- **验证证据**：`npm run check` 全绿（含 bundle budget 段——体积不得超出现有预算）；
  `npm test -w web` 全绿；构建产物 diff 抽查关键选择器仍在。

### Q1-5 `memory_service.rs`（66KB）→ 视前三刀反馈决定

- 优先级最低；若 Q1-1~Q1-4 落地顺利且该文件近期改动频繁，再按「向量召回 / 记忆 CRUD /
  blob 工具」切分。可延后，不阻塞本批收口。

---

## 批次 2：根目录过程稿归位（Q2）

### 现状证据

根目录 14 个 md，其中过程稿约 280KB：
`COMPUTER-USE-REPORT.md` / `COMPUTER-USE-HARNESSES.md` / `COMPUTER-USE-PLAN.md`（~109KB）、
`FRONTEND-REPORT.md` / `FRONTEND-FIX-PLAN.md`（~85KB）、
`HARNESS-PLAN.md`（23KB）、`TASK-MODE-FIX-PLAN.md`（62KB）。

### 改动

- **改了什么**：7 份过程稿整体 `git mv` 进 `docs/plans/`；
  根目录只留 `README.md` / `AGENTS.md` / `MAINTENANCE.md`。
  同步修正三类引用：① 各稿内部互链；② `docs/` 体系中对根目录稿的链接；
  ③ 本稿（QUALITY-FIX-PLAN.md）自身已放 `docs/plans/`。
- **为何这么改**：根目录是新会话/新贡献者的第一入口，14 个 md 淹没真正的入口文件；
  过程稿价值在「可追溯」而非「在根目录」。用 `git mv` 保留历史。
  **注意**：移动前跑一遍 `git grep -n 'FRONTEND-REPORT\|COMPUTER-USE-PLAN\|TASK-MODE-FIX-PLAN\|HARNESS-PLAN'`，
  所有引用同提交修掉——**先确认 `check-docs.mjs` / `check-doc-claims.mjs` 的扫描面**：
  前者只管 `docs/`、后者只扫 7 个指定文件（2026-09-26 提交记录已核实），
  但 `docs/**` 内部若引用了根目录稿，移动后链接即死，必须同提交修正。
- **验证证据**：`node tools/check-docs.mjs` / `check-doc-claims.mjs` / `check-arch.mjs` 三项 [OK]；
  `git grep` 复核无残留旧路径引用。

---

## 批次 3：`rust-toolchain.toml` 落地（Q3）

### 现状证据

- CI 锁 `dtolnay/rust-toolchain@1.97.1`（ci.yml 注释写明「锁到与本地一致」）；
- 仓库**无任何 `rust-toolchain.toml`**（2026-09-26 提交记录已 grep 核实）；
- E60 经验条目：工具链漂移导致「本地绿 / CI 红」已实际炸过一次。

### 改动

- **改了什么**：根目录新增 `rust-toolchain.toml`：
  ```toml
  [toolchain]
  channel = "1.97.1"
  components = ["rustfmt", "clippy"]
  ```
  CI 的 `dtolnay/rust-toolchain@1.97.1` 改为 `dtolnay/rust-toolchain@master`
  + `with: toolchain-file: rust-toolchain.toml`（或保留显式版本但注明「以 toml 为准」，
  二选一，推荐前者——单一出处）。E60 条目与 MAINTENANCE.md 门禁行的「工具链口径」
  同步指向 toml 文件。升版流程改为：改 toml 一行 → 本地 `npm run check` 复现修 lint →
  一起提交（CI 自动跟随，不再有两处版本号）。
- **为何这么改**：E60 的根因是「版本号存在两个地方（CI yaml + 人的记忆）」，
  文档记录教训只是缓解，`rust-toolchain.toml` 是制度化解法——rustup 本地自动跟随，
  CI 读同一文件，单一出处消灭漂移。
- **验证证据**：全新 clone 上 `cargo --version` 自动解析为 1.97.1；
  CI run 的 Setup Rust 步输出版本与 toml 一致；`npm run check` 全绿。
  **顺手结清欠账**：升版时处理 `memory_service.rs::blob_to_vec` 的
  `chunks_exact_to_as_chunks`（CI 首跑已暴露、现被版本锁掩盖）——单独一个提交。

---

## 批次 4：CI 覆盖补强（Q4）

- **改了什么**：
  1. `ci.yml` jobs 增加 `check-ubuntu`（同一套门禁中平台无关的部分：
     fmt / clippy / cargo test / 前端三段），捕获平台相关代码（`cfg(windows)` 之外）的可移植性问题；
     Windows 全量门禁保持现状不动（它是唯一权威档）。
  2. `workflow_dispatch` 增加 `target: [all, android]` 输入，手动触发时可对
     `kedai-Android` 分支跑 Android 交叉编译冒烟（仅 `cargo build --target aarch64-linux-android`，
     不跑测试），验证 rustls/bindgen/JNI 路径仍可编译。
- **为何这么改**：Android 分支「逐字节同源」契约目前靠文档声明和人工 merge 维持，
  无机器验证；一个廉价冒烟编译即可把「移植不改桌面行为」的硬要求从口头变成门禁。
  ubuntu job 成本极低（Linux runner 按 1 倍计费且更快）。
- **验证证据**：手动触发一次 `target: android` run 全绿；ubuntu job 出现在 PR 状态检查中。
- **优先级说明**：本批价值真实但非紧急（Android 分支活跃度低），可排在批次 1~3 之后。

---

## 批次 5：技术债迁移到 GitHub Issues（Q5）

- **改了什么**：把散落在 `docs/经验.md`（E60 升版欠账）、`docs/遗留.md`（TM-D6/D7/D8、TM-X2 等）
  中「未修复/知情接受」的条目逐条开 issue，打标签 `tech-debt` / `known-limitation`；
  文档原条目**不删**，只加一行「→ 跟踪：#issue 号」。同时把本计划的 Q1~Q6 也各开一个 issue。
- **为何这么改**：文档是叙述性载体，不适合做任务跟踪；issue 有可指派、可关闭、
  可被 PR `Closes #n` 关联的能力。文档保留是为了维持「变更史即证据链」的既有体系，
  两者职责不同不冲突。
- **验证证据**：issue 列表与文档条目一一对应；文档中新增链接后 `check-docs` 通过。

---

## 批次 6：给自己的 PR 流程（Q6）

- **改了什么**：此后非琐碎改动走「功能分支 → PR → CI 状态检查绿 → merge」；
  在仓库 Settings 给 `main` 加分支保护：要求 `ci / check` 状态检查通过
  （单人项目**不启用** required review，只启用状态检查）。
- **为何这么改**：CI 已激活且零边际成本，分支保护把「本地门禁可被 `--no-verify` 绕过」
  的最后缺口堵上；PR 同时天然成为 issue 的关闭锚点（配合批次 5）。
- **验证证据**：推一个测试分支开 PR，确认状态检查出现且为必选项；
  直推 main 被拦（或按保护规则如实记录允许直推的例外）。

---

## 执行顺序与排期建议

| 顺序 | 批次 | 预估工作量 | 理由 |
|---|---|---|---|
| 1 | 批次 3（rust-toolchain.toml） | 半天 | 最小改动、消除已炸过一次的失败面 |
| 2 | 批次 2（过程稿归位） | 半天 | 纯移动，风险低，先清场 |
| 3 | 批次 5（issue 迁移） | 半天 | 为后续批次的 PR 提供关闭锚点 |
| 4 | 批次 6（分支保护） | 1 小时 | 设置项操作，即刻生效 |
| 5 | 批次 1（巨型文件拆分） | 2~4 天 | 收益最大但动代码最多，放最后稳妥推进 |
| 6 | 批次 4（CI 补强） | 1 天 | 可与批次 1 并行，互不相干 |

## 不做的项（刻意排除）

- **不引入新语言/新框架**：所有修复在现有技术栈内完成。
- **不做行为性重构**：本计划只动结构、工具链与流程；任何「顺手优化逻辑」另立批次。
- **不改 `docs/archive/**`**：按仓库纪律，归档引文停留在归档时状态。
- **不追求 CSS 树摇/原子化重写**：style.css 只做按域拆分，不换方法论。
