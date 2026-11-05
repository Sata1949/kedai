---
name: kedai-batch-dev
description: Kedai 仓库的批次开发全流程执行手册。凡在本仓库（D:\kedai，Rust server-rs + Vue web + Tauri src-tauri）动手改生产代码、写新功能、修缺陷、动门禁脚本或契约文档时，先加载本技能。它规定批次怎么立、测试怎么写、验证跑到哪一步算过、台账回填哪几处、提交怎么组织。触发词：Kedai、批次、改这个功能、修这个 bug、提交、收口、回填台账、写计划条目。
---

# Kedai 批次开发全流程

本技能是本仓库的**执行顺序**。方法论层面的东西（怎么做 TDD、怎么调试）用别的技能；这里只管 Kedai 特有的那些「不做就会返工」的约束。

先认清一件事：这个仓库的纪律密度很高——`AGENTS.md`、五份台账（计划/遗留/经验/功能/契约）、17 段门禁、`tools/arch-layers.json` 分层 SSOT。**违反纪律的代价不是被骂，是门禁转红、返工重来**。所以顺序比速度重要。

## 第 0 步：先把事实查清，不要照文档改代码

`docs/` 里的行号会漂移。`经验.md` E36 记过一次教训：计划文档里 4 处坐标全错，照它改会改错位置。动手前：

1. 读 `AGENTS.md`（根目录）里与你这次改动相关的段落——**它覆盖一切默认行为**。
2. 用 `grep` / `Read` 确认要改的文件与函数**现在**在哪。文档给的行号只当线索，不当地址。
3. 若发现文档与代码冲突：以代码为准，并把偏差登记进对应台账（不是默默按代码改）。

### 子代理预算（2026-11-04 立）

派子代理很贵：一次多维度排查可派出 6 个 agent，单个最大 `model-io` 记录达 1 GB
（实测数据）。以下为硬约束：

- **单批次排查最多派 2 个**（一个后端、一个前端）。不要按「正确性/安全/性能/文档/卫生」
  分成四五个并行——那是同一批文件的重复读取，成本翻倍、信息冗余。
- **派发前先自己 Grep 定位**，把范围缩到具体文件再派。任务描述里直接给文件清单，
  不要写「审查整个后端」这种宽口径。
- **简单查找自己查**：单文件、单符号、明确的一处调用链，Grep 一下就行，不派。
- 子代理定义（`~/.zcode/agents/*.md`）已内置「禁止再派发 / 窄查询 / 只跑最小测试 /
  限时 / 输出上限」五条闸门，不要绕过。

## 第 1 步：判断这是哪一类改动，决定要不要走 PR

| 改动类型 | 流程 |
|---|---|
| 生产代码 / 门禁脚本 `tools/check-*` / `.github/workflows/` / 契约文档 | 必须走「临时分支 → PR → CI 绿 → merge」。**直推 main 会被服务端拒绝（含 admin）** |
| 纯文档 | 同样走 PR（2026-10-11 起分支保护覆盖一切改动） |
| 平台专属（Android） | 在 `kedai-Android` 分支提交；共享代码先落 main 再同步 |

分支规则：长期分支只有 `main` / `kedai-Win` / `kedai-Android`；临时分支仅限 PR 流程，**合并后立即删除**（本地与远端）。

## 第 2 步：先写失败测试

`AGENTS.md` 的硬约定：**修改核心行为先写失败测试，再做最小实现**。

写测试时有两条本仓库特有的要求，不做的话测试等于没写：

1. **变异测试**：新测试写完后，临时把代码改回缺陷形态，确认测试**真的会红**。永远通过的装饰性断言在本仓库被视为无效（`经验.md` E28、E29 都因此翻过车）。
2. **临时目录守卫**：后端任何建临时数据目录的测试，一律用 `server-rs/src/utils/test_support.rs` 的 `TempDataDir::new("<tag>")`。**不要裸写 `std::env::temp_dir().join(...)`**——历史上这曾把 `%TEMP%` 撑到 11.22GB / 14015 个目录（`经验.md` 经验 32）。
   - 析构顺序是硬约束：守卫声明在 Db/服务**之前**；解构写成 `let (guard, svc) = ...`（**不是** `let (svc, guard)`）；结构体里守卫必须是**最后一个字段**。
   - 禁止 `Db::open(&TempDataDir::new("x").path()...)` 这种临时值形态——语句结束守卫就析构，库还开着就删目录。

前端测试：Vitest，文件与源码同目录 `*.test.ts` 或 `__tests__`。**新增源码文件若没有对应测试 import，`check-test-ratchet` 会 FAIL**（零测试文件有显式清单，见 `tools/check-test-ratchet.mjs`）。

## 第 3 步：实现时守住几条不可越的线

- **不要顺手实现未要求的东西**。`AGENTS.md` 明写「不顺手实现未要求的安全/API 批次」。混批会让回滚粒度失效——这是本仓库最常见的返工原因。
- **不要新建插件机制**。垂直能力（编码/文学等）一律走「官方能力包」形态：常量声明 + 设置开关 + 缺省值切换，默认关、显式启用。
- **冻结点清单**（改了就是事故，见 `经验.md` E28）：
  - `server-rs/src/models/db/schema.rs` 的 `CREATE_TABLES` 静态文本（须与 `server-rs/src/migration/ddl.rs` 在 `normalize_sql` 后一致）
  - `SseEvent` / `TaskEventKind` 的线格式值（`check-contract.mjs` 守卫）
  - EJS 能力面（`check-arch` 规则 H 冻结，只降不升）
  - `/api/health` 的 `ok` 语义（桌面壳单实例判定依赖它）
  - SSE 重连算法（只加可观测信号，不改策略）
- **新增设置字段**：必须补 `web/src/stores/genSettings.test.ts` 的夹具（只有 `vue-tsc` 会抓漏），并在 `check-settings-parity` 的三组对拍里登记。
- **新增 SSE 事件变体**：前后端四处同步——枚举 `TaskEventKind` / 发射点 / `web/src/api/types/` 的 TS union / `web/src/stores/task.ts` 的刷新映射（末项由 `never` 穷尽断言兜底），另须同步 `docs/契约.md` 两处变体计数。
- **新增前端根文件**：登记进 `tools/arch-layers.json`，否则 `check-arch` 规则 I 直接 FAIL。
- **棘轮只降不升**：`check-arch` 基线、`file-size-baseline.json`、`check-css-discipline` 的 CSS 基线，改动时必须同步下调并记录前后值。**绝不允许为了过检查而加基线条目**。

## 第 4 步：验证，按代价从低到高

```powershell
# 改了前端
npm test -w web
npm run build -w web        # vue-tsc 是硬门禁,类型错会在这里暴露

# 改了后端(必须先加载 vcvars64,见下)
cargo test --manifest-path server-rs/Cargo.toml -j 8

# 只改文档也要跑(门禁不漂移)
node tools/check-arch.mjs
node tools/count-tests.mjs --check
```

**本机 cargo 必须先有 MSVC 环境**，否则报 `link.exe not found`。Git Bash 里最省事的形态：

```bash
cmd //c "call \"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat\" && cd /d D:\kedai\server-rs && cargo test -j 8"
```

`cargo test` 统一带 `-j 8`。历史上默认并行度下 rustc 会崩，报错伪装成「依赖 rlib 缺失」——**遇到那形态先降并发复跑，不要改依赖**（`经验.md` 条目 29）。若同时跑着重活或页面文件吃紧，降到 `-j 4`。

## 第 5 步：回填台账（这一步不能省）

批次收口时按「改到谁更新谁」回填，具体清单：

| 台账 | 什么时候动 |
|---|---|
| `docs/功能.md` | 新增了用户可见能力 |
| `docs/契约.md` / `docs/契约-协议与配置.md` | 动了端点字段、SSE 负载、线格式、配置项、平台分支规则 |
| `docs/契约-架构与数据.md` | 动了分层归属（并同步 `tools/arch-layers.json`） |
| `docs/计划.md` | 有条目状态变化（未启动→进行中→已完成） |
| `docs/遗留.md` | 发现缺陷但本批不修，或有意裁剪 |
| `docs/经验.md` | 能写全「现象 → 根因 → 做法」三段、跨任务可复用 |
| `docs/功能-变更史.md` | 每批落地都要写一章（commit hash + 命令 + 当时实测数值） |
| `MAINTENANCE.md` | 测试数字、命令、门禁清单这类**可计数事实**的唯一真值源 |

两条铁律：

- **测试数字绝不手抄**。`MAINTENANCE.md` 是唯一真值源，由 `node tools/count-tests.mjs` 自动统计。写别的文档时用「见 MAINTENANCE.md §2」指过去。
- **条目 ID 不得重编号**。`L16`、`P-8`、`E28` 这些是跨文档引用锚点，源码注释里都在用。

`docs/plans/` 只放**调查报告**（如 `FRONTEND-REPORT.md`）；计划条目的唯一出口是 `docs/计划.md`。临时执行稿在收口时并入台账并删除原稿——这是既定体例（`docs/README.md` 记了 8 次先例）。

## 第 6 步：提交

Conventional Commits 前缀 + **中文 scope 与描述**，沿用既有风格：`fix(设置): ...`、`refactor(架构治理): ...`、`perf(CI): ...`。

正文必须写清三件事：**改了什么 / 为何这么改 / 验证证据**（实际跑过的命令与结果）。涉及模型链路的还要附真实模型实测证据（见 `kedai-verify-real-model` 技能）。

- 一个逻辑批次一个提交；不相关的改动不混。工作区有多条线时按主题分开 `git add <path>`，不要 `git add -A`。
- 改完先 `cargo fmt`（本机门禁第一段就是 fmt，红了后面全跳）。
- 提交信息长时用 `git commit -F <文件>`，`-m` 会 E2BIG。
- **推送**：`pre-push` 钩子在本机会因 SIGPIPE 被杀（门禁过了但推送没完成）。可靠路径是——预跑 `tools/check-all.ps1 -Quick` 留证 → `git push --no-verify` → `git ls-remote` 验收 SHA。别读日志下结论，读远端 ref。
- 并行会话可能把 HEAD 切走。每次提交前 `git branch --show-current` 确认分支。

## 常见返工陷阱

| 症状 | 真实原因 |
|---|---|
| cargo 报「依赖 rlib 缺失」 | rustc 并行崩溃，降 `-j` 复跑，别改 Cargo.toml |
| `LNK1104 / os error 5` | 有另一个 cargo 在跑，或开发中的 server 锁住了 target 产物 |
| 门禁全红但隔离复跑绿 | 先怀疑日志来自修复前的旧构建；再怀疑 `settings_connector` 顺序竞争（TEST-ISO-1） |
| 前端类型错但测试绿 | `vitest` 不做类型检查，得跑 `vue-tsc`（`npm run build -w web`） |
| 改了设置字段没报错但没生效 | `PUT /api/settings` 曾静默拒信封，见 `遗留.md`；新字段必须过 `check-settings-parity` |
| CSS 门禁红了 | 基线 ratchet 只降不升；新增样式应走 `<style scoped>` 或工具类，别扩全局域 |

相关技能：`kedai-gate-runner`（门禁逐段与失败判别）、`kedai-backend-core`（后端落点）、`kedai-frontend-core`（前端落点）、`kedai-verify-real-model`（收口实测）。
