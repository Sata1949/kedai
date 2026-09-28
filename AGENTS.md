# AGENTS.md — Kedai 开发说明

> 本文件仅供开发 agent 阅读，用于代码维护、测试与构建；它不是产品运行时提示词，禁止注入聊天模型。
> 产品运行时主 Agent 提示词统一保存在 `DATA_DIR/AGENTS_RUNTIME.md`，由后端文件服务、Engine 与设置页共用同一路径。

## 项目范围

- Rust 后端：`server-rs/`
- Vue 前端：`web/`
- 桌面壳：`src-tauri/`
- 数据目录(运行时,不进仓库)：`DATA_DIR` 环境变量 > `%APPDATA%\com.kedai.app\data\`(已含用户数据时) > 项目根 `data\`；解析逻辑见 `server-rs/src/config.rs`。
- **产品定性(2026-09-28 拍板)**：Kedai 是**通用 LLM 智能体 harness**，界面上的角色扮演 / 任务工作台是**平级的双顶层模式**；垂直能力(编码 / 文学等)一律以**官方能力包**形态交付——常量声明 + 设置开关 + 缺省值切换(按需再叠加「启用时追加注入段」)，**默认关、显式启用**，不新建插件机制、不侵入内核架构。产品定性叙述的三处同源：`README.md` 首段、`MAINTENANCE.md` §1 定位句、`docs/功能.md` 抬头定性段——改一处必须三处同步。

## 实现约定

- 面向用户的提示、错误和代码注释使用简体中文。
- 修改核心行为先写失败测试，再做最小实现。
- 不泄露聊天正文、API Key 或本地真实数据。
- 不顺手实现未要求的安全/API 批次。**Git 提交按下方「Git 提交纪律」执行（2026-09-15 起由「一律不提交」改为「完成并验证后按体例提交」）。**

## 任务模式机制要点(2026-08-28 重构后)

- 后端任务引擎分两层:`server-rs/src/services/task_service/`(任务 CRUD/状态/取消/事件)与 `server-rs/src/services/task_engine/`(批次 4 六模式底座:ModeExecutor/TaskRunContext/事件桥;legacy/solo/multi/plan/team/custom 六个执行器均在此,统一派发点见 `task_engine/mod.rs`);任务状态为枚举 `TaskStatus`/`TaskStepStatus`/`TaskSubtaskStatus`,运行模式为 `TaskRunMode`(`models/types.rs`,serde snake_case 字符串,落盘与 API 线格式不变)。六模式语义见 `docs/功能.md`。
- 数据流:DB 写入成功后 `TaskService` 经 broadcast 通道发射 `SseEvent::Task`,`GET /api/tasks/events` 转发为 SSE;前端 `web/src/stores/task.ts` 事件驱动精确刷新(无轮询;断线指数退避重连 + 5s 兜底轮询)。新增事件 kind 需前后端四处同步(枚举 `TaskEventKind`/发射点/`web/src/api/types.ts` 的 TS union/store 刷新映射;末项由 `stores/task.ts` 的 `never` 穷尽断言兜底)。
- 提示词共享原语在 `server-rs/src/services/prompt_kit.rs`(世界书过滤/注入合成/占位符渲染/`untrusted_boundary`),引擎与任务双侧调用,勿在任一侧复制实现。
- 双模式提示词隔离由类型承载(`RoleplayPromptConfig`/`TaskPromptConfig`,settings.json 线格式不变);新增涉及双模式的设置字段时,继承/隔离规则先读 `docs/契约-协议与配置.md`。

## 验证命令

```powershell
cargo test --manifest-path server-rs/Cargo.toml -j 8
npm test -w web
npm run build -w web
cargo build --manifest-path server-rs/Cargo.toml
```

- **`cargo test` 统一带 `-j 8`**:本机历史上默认并行度下 rustc 自身可能崩溃(`STATUS_STACK_BUFFER_OVERRUN`),报错却伪装成「依赖 rlib 缺失」,照它改依赖只会越改越偏;遇到该形态先降并发复跑,不要改依赖(详见 `docs/经验.md` 条目 29)。**2026-09-17 P-13 实测**:冷 target 全并行(16 jobs)与 `-j 4/6/8` 五轮均 1157 passed / 0 failed,历史故障**未复现**,故上限由 2 放宽到 8(取 8 而非不限制,是为与 `build.ps1` 等重活并发时留内存余量);矩阵与依据见 `docs/经验.md` 条目 29。

## 构建提醒(后续模型必读)

- **改完任何前后端代码后,`dist\` 下的 exe 与项目根 `Kedai.exe` 不会自动更新。** 用 `target\debug\` 二进制验证通过后,交付/试用 exe 版前必须跑一次 `.\build.ps1`(默认双端同步:前端 + release + 便携版 + 构建指纹 sidecar;详见 MAINTENANCE.md §4)。只改了代码只跑过 debug,不等于 exe 版已更新。
- 快速迭代可用 `.\build.ps1 -TestOnly`(只产测试版),但记住便携版会因此未同步,交付前补一次完整 `.\build.ps1`。
- **本机环境**:裸 PowerShell/cmd/Git Bash 里可能没有 cargo 环境(MSVC toolchain 未注入)。构建/测试前先执行
  `call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"`,
  或在已加载 vcvars64 的 shell 里再调用 build.ps1 / cargo;Git Bash 中可用 `cmd //c "<bat 包装>"` 形式嵌套。
- Windows 上 curl 直接 `-d` 中文 JSON 会被编码破坏(invalid unicode code point);中文请求体一律写 UTF-8 文件后用 `--data-binary "@file"`。

## Git 提交纪律(2026-09-15 修订:此前为「不提交 Git」)

- **完成一批改动并验证通过后,按仓库体例提交。** 不再要求「一律不提交」——旧纪律导致改动长期滞留工作区,
  无法用 `git log` 追溯「哪次改动为哪批需求服务」,也让回滚失去粒度。
- **提交信息体例**:Conventional Commits 前缀 + 中文 scope 与描述,沿用既有 commit 风格。
  例:`refactor(架构治理): 三结合彻底落地——…`、`fix(可观测性): …`。
  正文写清三件事:**改了什么 / 为何这么改 / 验证证据(实际跑过的命令与结果)**。
- **粒度**:一个逻辑批次一个提交;互不相关的改动不混进同一提交。工作区同时存在多条线时,
  按主题分多次提交(必要时用 `git add <path>` 逐个指定),不要一把 `git add -A` 了事。
- **不提交**:构建产物与其 sidecar(`dist/`、`target/`、`Kedai.exe`、`*.lnk` 等,`.gitignore` 已覆盖)、
  用户数据(`data/`)、密钥(`.env`)、开发 agent 的会话目录(`.zcode/`)。
- **提交前的最低验证**:改了后端跑 `cargo test --manifest-path server-rs/Cargo.toml -j 8`;
  改了前端跑 `npm test -w web`;只改文档可跳过测试,但仍要跑 `node tools/check-arch.mjs` 与
  `node tools/count-tests.mjs --check` 确认门禁不漂移。
- **分支**:主干 `main` + 平台分支 `kedai-Win` / `kedai-Android`(模型见 `docs/契约-协议与配置.md`)。
  远端 `origin` 为 **GitHub 私有仓**(2026-09-26 配置);推送前先确认当前分支,不要在平台分支上提交共享代码。
- **分支创建**:不得随意新建分支;长期分支只有 `main` + 两个平台分支,临时分支仅限 PR 流程,
  合并后即刻删除(本地与远端)——规则见 `docs/契约-协议与配置.md` 的「平台分支契约」§三。
- **非琐碎改动走 PR(2026-09-27 起)**:动生产代码、门禁脚本(`tools/check-*`)、`.github/workflows/`、
  契约文档的批次,走「功能分支 → PR → **CI 状态检查绿** → merge」(单人项目不要求 review,看检查即可);
  纯文档与注释批次可直接推 `main`。
  **诚实边界**:本仓是 GitHub 私有仓 + Free 计划,服务端分支保护与 rulesets **不可用**
  (实测 403 `Upgrade to GitHub Pro`,见 `docs/遗留.md` CI-PROT-1)——所以这条是**流程纪律,不是机器强制**:
  直推 `main` 不会被服务端拦下,本地 `pre-push` 也仍可 `--no-verify` 绕过,唯一的机器信号是 CI 事后转红。
- **不改写历史**:不 `rebase`/`amend` 已推送的提交;`docs/archive/` 下的历史文档只读不改。
