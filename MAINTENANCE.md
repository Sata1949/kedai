# Kedai 维护指南(MAINTENANCE)

> 面向后续维护者的技术文档。涵盖架构、构建、启动、API 契约、数据库、日志与已知坑位。
> 版本:v0.3.0-A-beta(前端 Vue3 + 后端 Rust + Tauri 桌面壳) 最后更新:2026-09-13

---

## 0. 架构治理(三结合梯队架构)

**总纲**:借鉴「老中青三结合」组织原则,代码分三层——L1 老层·稳(Anchored Core:parsing/ 兼容解析、models 数据契约、测试套件、SillyTavern 兼容承诺)、L2 中层·干(Orchestration:agents/、services/、api/)、L3 青层·活(Frontier:tools/、skills/、沙箱、新能力)。四种机制:优势互补、传帮带晋升、梯队衔接、动态循环。**详见 `docs/ARCHITECTURE-3H.md`(结构性改动先读它再动代码)**。

关键纪律:
- **改 L1 契约**须先写失败测试、声明兼容影响、过评审;协议翻译集中在中层。
- **Mutex 纪律**:全项目锁中毒一律恢复(`.lock().unwrap_or_else(|e| e.into_inner())`),不 panic;**禁止持 std::sync::Mutex guard 跨 `.await`**(Clippy `await_holding_lock` 防护)。
- **DB 并发纪律**:api handler 的同步 DB 调用必须经 `db_call/db_read/db_write`(spawn_blocking),禁止 async 上下文直接持锁;细则见 §7「DB 并发纪律」。
- **渲染性能纪律(前端)**:消息渲染必须走 ChatMessageItem 的缓存 computed,禁止在 v-for 里直接调渲染方法。
- **样式分层纪律(前端,2026-09 D-5 起)**:`web/src/style.css` 只保留层① 设计变量(:root 令牌)与层② 全局基础层;新增组件样式一律 `<style scoped>` 或 Tailwind 工具类,禁止再写入 style.css;修改存量组件时顺手把该组件样式搬进 scoped(「改到谁拆谁」,文件头分层约定注释为准);新增样式不得引入 `!important`(存量 11 处见 style.css)。
- **跨端协议**(前后端 mvu)以 `docs/mvu-protocol.md` 锁定双端一致,防行为漂移。
- **EJS 自研解释器冻结纪律**:`parsing/assistant/ejs/`(自研迷你 JS 引擎,约 4000 行)**只接受安全修复,不再扩展新能力**。任何新模板能力必须在 `scripts/runtime.rs` 的 rquickjs 沙箱侧实现(rquickjs 自带内存/中断/栈上限,见该文件 `set_memory_limit`/`set_interrupt_handler`)。理由:自研解释器缺引擎级沙箱限额,长期维护成本与风险高于复用;**已加固**(循环步数+墙钟预算、解析深度守卫,见 §10 踩坑记录),但债务不再增长。
- **门禁纪律(2026-09-13 起)**:`tools/check-all.ps1` 是唯一的本地 CI 入口,现已接三处触发——
  ① `build.ps1` 在构建前跑 `check-all -Quick`,失败即中止构建(`-SkipChecks` 仅限本地应急,
  **交付/试用前必须补跑一次完整 `check-all`**);② `tools/hooks/pre-push` 在推送前跑同一检查
  (装一次:`npm run hooks:install`;紧急可 `git push --no-verify`,同样须事后补跑);
  ③ `.github/workflows/ci.yml`(**纸面 CI,从未运行**):2026-09-13 落盘,但仓库
  **始终未配置 git 远端**(`git remote -v` 为空),故从未触发。**当前实际生效的闸门
  只有前两条(build.ps1 与 pre-push)**——不要依赖 CI 兜底。配置远端并推送后它会自动生效,
  届时本节应更新为「三处触发均实测生效」。
  变更 `tools/check-*.mjs` 的检查规则时,同步更新本节与本文件的检查项清单。
- **性能门禁(2026-09-14 起,可选)**:`tools/perf-baseline.mjs` 支持 p95 阈值判定
  (`--max-p95-factor`,默认 1.25),基线值存 `tools/perf-baseline.json`;
  经 `check-all.ps1 -Perf` 并入总门禁(**默认关闭**——需服务已在运行,
  避免把「没起服务」误报成门禁失败;服务不可用时 fail-closed 而非静默跳过)。
  判定口径 `p95 <= 基线 × 倍数` 且 `errors == 0`,`p95 < 5ms` 的端点跳过。
  基线是**本机 release 口径**,只作回归对比基准、不是跨机 SLA;换机器或数据量
  变化后须重采样。数据与用法见 `docs/perf-baseline.md` 的「2026-09-14 实测」小节。
- **代际归属与跨代依赖门禁(规则 I/J,2026-09-14 起)**:`tools/check-arch.mjs` 新增两条护栏,
  其唯一机器可读事实源(SSOT)是 **`tools/arch-layers.json`**——
  - **规则 I(代际归属完整性)**:`server-rs/src` 的每个顶层模块/根文件、`web/src` 的每个顶层目录/根文件
    **必须**在 `arch-layers.json` 登记代际(L1/L2/L3/entry)与职责,**未登记即 FAIL**。
    新增或改名顶层模块时先补登记再动代码——没有代际归属的模块无从按分层纪律评审,是「分层叙事的静默失效点」。
  - **规则 J(跨代依赖方向)**:按登记代际与允许方向校验实测 import 图。**新增未登记的越代边即 FAIL**;
    存量越代边必须在 `arch-layers.json` 的 `registeredEdges`(后端)/`frontendRegisteredEdges`(前端)登记,
    写明 `reason`(为何存在)与 `remediation`(收口路径)。登记表**只减不增**:还清一条删一条,
    **禁止为过检查而加条目**(那等同于关掉护栏)。
  - **前后端方向规则不同(务必区分)**:后端是**隔离模型**——L3(工具/沙箱)与 L2(编排)双向互斥,
    青层反向依赖骨干层会让「隔离」名存实亡;前端是**层次模型**——L3(展示)→ L2(状态编排)→ L1(纯契约)
    是严格向下的正常依赖(组件读 store、store 调 composable),故前端**额外允许 L3→L2**。
    配置分别见 `arch-layers.json` 的 `dependencyRules.backendAllowed` / `frontendAllowed`。
  - **修复纪律(2026-09-14 教训)**:规则 I 的前端分支曾误写入后端失败桶,而汇总在后端违规时立即 `exit(1)`,
    导致**前端规则 J 的结论永不打印**(护栏静默失效,且当时 26 项未登记使构建被阻断)。
    现已改为两个失败桶都打印后再统一退出。修改任何门禁脚本的失败聚合逻辑时,
    **必须验证两侧报告都能输出**,并确认退出码仍非 0。
- **依赖供应链纪律(2026-09-13 批次 1 起)**:三把闸已进 `check-all`——
  ① `deps: check-lock-sync`(双 Cargo.lock 漂移检查:src-tauri 内嵌 server-rs 时 cargo 会
  重新解析依赖树,同一后端源码可能编出不同版本依赖;**0 漂移为基线**,新增即 FAIL,
  例外登记在 `tools/lock-sync-baseline.json`);
  ② `cargo audit` 默认**硬门禁**(server-rs 锁历史 0 洞;仅 advisory DB 拉取失败时降级 WARN,
  `-LooseAudit` 可临时降档);
  ③ `npm audit --omit=dev` 警告档(历史 2 洞——sanitize-html 存储型 XSS / nanoid——已修复)。
- 新能力默认进 L3 隔离验证,成熟后按晋升通道(测试通过 + 不破坏协议 + 评审)升级。

### 新能力晋升状态(三结合梯队)

| 能力 | 当前代际 | 状态 |
|---|---|---|
| GENERATE/RENDER 内容注入([GENERATE:BEFORE/AFTER]、{idx}、REGEX) | L3 → 拟 L2 | 已在 engine 挂载,测试覆盖;协议文档见 docs/generate-render-protocol.md |
| @INJECT 精确消息插入(pos/target/regex) | L3 | messages/inject.rs 测试覆盖;协议文档见 docs/inject-protocol.md |
| @@ 装饰器解析(parse_decorators/EntryDecorators) | L1 | 已入 world_book.rs 兼容解析 |
| EJS 读取 API(getwi/getchar/injectPrompt 等 15 个) | L3 | ejs 层 10 个测试;injectPrompt 为兼容占位(engine 注入清单未接入) |
| **EJS 自研解释器本体** | **L3·冻结** | **仅接受安全修复,新能力一律走 rquickjs 沙箱**;已加固循环步数/墙钟预算 + 解析深度守卫(5 个新测试) |
| LAST_SEND/LAST_RECEIVE 统计变量 | L2 | engine 收尾写入会话宏,测试覆盖 |
| <#escape-ejs> 作用域转义 | L1 | ejs/mod.rs 占位符方案 + 测试 |
| PatchOp.reason / 转义还原 / delta 无效跳过 / Insert 并入 Replace | L1 | mvu 协议对齐,见 docs/mvu-protocol.md |

---

## 1. 项目概览与架构

**定位**:本地运行的 AI 角色扮演 / 文学创作客户端,SillyTavern 生态原生兼容,内置 Agent 引擎(计划 → 执行 → 反思)。

```
kedai/
├── server-rs/                  # 后端:Rust + axum + tokio + rusqlite(核心,可单独交付)
│   ├── src/
│   │   ├── main.rs             # 命令行入口(健康自检 + run_server)
│   │   ├── lib.rs              # 库入口(run_server 供 main/Tauri 复用,build_test_app)
│   │   ├── config.rs           # 配置加载(环境变量 + .env,支持 DATA_DIR/LOG_DIR 注入)
│   │   ├── api/                # 路由层:mod.rs(组装/CORS/SPA 回退)+ routes/ 五域(chat/settings/agent/content/misc)
│   │   │   │                   #   + static_files.rs(静态文档/SPA 回退)+ util.rs(WithStatus/db_err)
│   │   │   │                   #   + errors.rs(结构化错误码 ErrorCode/err_with_code)
│   │   ├── agents/
│   │   │   ├── engine/         # 目录模块(拆分自原 engine.rs):mod.rs(主流程)+ run_loop/run_finish/run_scripts
│   │   │   │                   #   + messages/ 目录(build/inject/trim)+ worldbook/executor/mvu/compaction/reflector_integration
│   │   │   ├── state_machine.rs# 8 状态 + 迁移表(幂等迁移)
│   │   │   ├── planner.rs      # fast/deep 计划 + 算式识别
│   │   │   └── reflector.rs    # 质量反思(空/截断/未答疑问 3 规则)
│   │   ├── connectors/         # LLM 后端适配(openai_compatible/ 目录模块 + mock)
│   │   ├── tools/              # 工具系统(registry / calculator / memory / agent_tools)
│   │   ├── models/             # db/ 目录(schema 建表/backfill 迁移)/ types.rs(契约类型)
│   │   ├── parsing/            # character_card.rs + assistant/(ejs/ 目录模块:mvu 变量渲染)
│   │   ├── services/           # character / session / agent_session / token 服务
│   │   └── utils/logging.rs    # tracing 日志:PinoFormat JSON + non-blocking 双 channel(按天归档)
│   └── tests/                  # API 集成测试(api_integration/assistant/agent_flows/prompt_inject/settings_connector/security/world_books)
├── web/                        # 前端:Vue 3 + Vite + Tailwind v4 + Pinia
│   └── src/
│       ├── api/                # 目录模块(拆分自 api.ts):client/types/characters/sessions/worldbooks/... + index 聚合
│       ├── stores/             # Pinia 七子 store(character/chat/genSettings/modelConn/resources/task/uiPrefs)
│       ├── store.ts            # 全局状态门面(facade,聚合七子 store 保持原引用路径)
│       ├── sseReducer.ts       # SSE 事件纯函数(拆分自 store)
│       ├── mvu/                # 前端 mvu 变量系统(parser/variables/host/mvuStore)
│       └── components/         # Sidebar / ChatWindow / ChatInput / AgentDock / SettingsModal
│           └── settings/       # 设置弹窗十一 section(Api/Connection/GenParams/PromptInject/Agent/AgentFlow/PresetImportExport/DataManagement/Ui/Mcp/Embedding)
├── src-tauri/                  # Tauri 2 桌面壳:窗口加载 http://127.0.0.1:3001,进程内复用 run_server
│   ├── src/lib.rs              # 数据目录注入(%APPDATA%\com.kedai.app)+ 服务自检/启动
│   └── tauri.conf.json         # 窗口 1280×800、NSIS 打包、图标
├── tools/make-icons.ps1        # 品牌图标生成脚本(圆角 + 透明背景,零依赖)
├── start.ps1                   # 智能启动:默认测试版(浏览器模式),-Portable 启动便携版;过期/漂移自动双端重建
├── build.ps1                   # 一键构建(默认双端同步:前端 + Rust release + 便携版;-TestOnly 仅测试版;-Tauri 追加 NSIS)
├── Kedai.exe / Kedai.lnk       # 图形启动器(双击正式入口;源码在 launcher/,由 build.ps1 幂等维护)
├── logs/                       # 运行日志(自动清理 3 天前;桌面场景在 %APPDATA%\com.kedai.app\logs)
├── data/                       # SQLite + 角色卡原图 + avatars(勿删)
└── docs/                       # 技术文档(活文档 + archive/ 历史归档;索引见 docs/README.md)
```

### 请求数据流

```
浏览器 → /api/xxx → api/mod.rs 路由 → services → SQLite(rusqlite)
                                        └→ connectors(LLM)→ SSE 流回推
```

### Agent 引擎状态机

`idle → planning → executing ⇄ tool_call → reflecting → finished`,可 `interrupted`/`error`。
中断通过 `watch::channel<bool>` 中止标志贯穿全链路。

---

## 2. 快速命令速查

| 操作 | 命令 | 说明 |
|---|---|---|
| 一键启动(桌面) | 双击 `Kedai.lnk` 或 `.\start.ps1 -Portable` | lnk 指向项目根 `Kedai.exe` 图形启动器(过期/漂移自动询问重建);`.\start.ps1` 默认启动测试版浏览器模式 |
| 桌面安装包 | `.\build.ps1 -Tauri` | 双端同步之外追加 NSIS 安装程序(需 `@tauri-apps/cli`);安装后从开始菜单启动 |
| 一键构建 | `.\build.ps1` | **默认双端同步产出**:前端 web/dist + Rust release(测试版)+ 便携版;`-TestOnly` 仅测试版快速通道 `-Dev` debug 构建 `-NoWeb` 仅 Rust `-Tauri` 追加 NSIS 打包 |
| 兼容别名 | `npm run build:all` / `npm run build:rs` | 均等价 `.\build.ps1`(双端同步);`npm run build:test` 等价 `.\build.ps1 -TestOnly` |
| 统一改版本号 | `npm run version:bump -- x.y.z` | 7 处版本号一次改全(2 个 package.json、3 个 Cargo.toml、tauri.conf.json、本文档版本行);支持 `-DryRun` 预览 |
| 后端测试 | `cd server-rs && cargo test` | **1108 个测试(891 单测 + 217 集成,22 个集成文件)**,**需在 vcvars64 环境**;前端 `npm test -w web` **823 个(86 文件;静态计数;vitest 运行时为 841,差值 18 来自 `parser.contract.test.ts` 循环生成的 fixture 用例)**。数字由 `node tools/count-tests.mjs` 自动统计,勿手抄——`npm run count:tests` 查看当前值,`npm run check:tests` 校验文档是否漂移 |
| 全量检查(本地 CI) | `npm run check` | `tools/check-all.ps1`:fmt → clippy → cargo test → cargo audit(**硬门禁**)→ lock-sync(双锁漂移)→ contract → arch(C/D/E 分层)→ 类型 ratchet → npm audit(警告)→ vue-tsc(**硬门禁**)→ vitest → vite build。**已接入 build.ps1 与 pre-push hook**(CI 工作流为纸面、未运行,见 §0 门禁纪律)。
`check-arch` 规则自 2026-09-14 起含 C/D/E/G/H/I/J(代际归属与跨代方向以 `tools/arch-layers.json` 为 SSOT) |
| 开发模式 | `cd server-rs && cargo run` + `npm run dev -w web` | 后端 3001 / 前端 5173(代理到 3001) |
| 前端构建 | `npm run build -w web` | 产出 web/dist(编译进 exe 用) |

> **⚠️ 关键**:本机 cargo 编译必须先加载 VS 环境,否则报 `link.exe not found`:
> ```
> cmd /c "call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" && cd server-rs && cargo build --release"
> ```
> `build.ps1` 内部已做此处理,直接 `.\build.ps1` 最省事。

---

## 3. 环境依赖(本机已配置,新机器需重装)

| 依赖 | 版本 | 安装方式 | 备注 |
|---|---|---|---|
| Rust 工具链 | 1.97.1 stable | `rustup`(本机经清华镜像安装) | rustup/cargo/rustc 在 `~/.cargo/bin`(可能不在 PATH,用全路径或加 PATH) |
| VS2022 Build Tools | 17.14(C++ 工作负载) | winget | 提供 MSVC 链接器 `link.exe` 与 cl.exe(必需) |
| Node.js | ≥ 18(验证 24) | 已有 | 仅前端构建需要;发布 exe 运行时**不需要** Node |
| cargo-audit | 0.22.2 | `cargo install cargo-audit --locked` | `check-all` 的依赖审计阶段(**硬门禁**;未安装时该阶段提示跳过)。advisory DB 当前经 Gitee 镜像拉取 |

### 原生依赖说明(Phase 3 起)

| 依赖 | 形式 | 备注 |
|---|---|---|
| `sqlite-vec` 0.1.9 | C 源码静态编译(`cc` + `cl.exe`) | 记忆库向量检索的 `vec0` 虚拟表扩展;走 `sqlite3_auto_extension` 全局注册(`models/db/mod.rs::register_sqlite_vec`),产物仍为**单 exe**,便携版无需附带 dll。仅需已有 MSVC 工具链,不引入 cmake/clang/nasm。 |

### crates 国内镜像(已配置 `~/.cargo/config.toml`)

```toml
[source.crates-io]
replace-with = "ustc"
[source.ustc]
registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"
[net]
git-fetch-with-cli = true
```

若官方源可用,可改回 `crates-io` 直连。

---

## 4. 构建与发布

### 单二进制原理

`kedai-server.exe` 通过 `include_dir!` **内嵌** `web/dist`(编译期快照,见 `server-rs/src/api/static_files.rs`)。
运行时默认只服务内嵌版本;仅显式设置环境变量 `KEDAI_WEB_DIST` 指向磁盘目录时才用磁盘版覆盖
(开发调试用,见 `server-rs/src/config.rs`)。

因此**每次改前端后必须重新构建 exe** 才会带上新界面;`server-rs/build.rs` 的
`rerun-if-changed=../web/dist` 保证 cargo 感知前端变化,构建脚本另有 mtime 兜底检测。

### 构建流程

```powershell
.\build.ps1          # 默认双端同步:前端 + cargo build --release + 便携版
.\build.ps1 -TestOnly  # 快速迭代:只产出测试版(结尾会警告便携版未同步)
```

> **本机环境提示(2026-08 批次构建时验证)**:裸 shell 里没有 cargo——MSVC toolchain 需先
> `call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"`,
> 再在同一 shell 里执行 build.ps1(脚本内部直接调 cargo/npm,继承环境)。
> Git Bash 里用 `cmd //c "<包装.bat>"` 嵌套调用。另外:构建只跑 debug 不代表 exe 版已更新,
> 交付前必须跑 `.\build.ps1`(详见 AGENTS.md「构建提醒」)。

产物:`dist\kedai-server.exe`(测试版)+ `dist\Kedai-portable\Kedai.exe`(便携版),各自约 20~30MB。
双端全量构建约 5~10 分钟(便携版含 Tauri 全量编译);快速迭代用 `-TestOnly` 或 `-Dev`。

### 两版同步机制(构建指纹)

测试版与便携版是两条独立编译链,各自把编译那一刻的 `web/dist` 冻结进二进制——分开构建必然漂移。
根治手段是「默认双端同步 + 指纹可查 + 启动对齐」三层:

1. **构建层**:`build.ps1` 默认一次产出两端;每个 dist 产物旁边写 `<exe>.build.json`
   (`{version, build_time, dist_hash}`,算法见 `tools/Write-BuildStamp.ps1`)。
2. **运行时层**:`server-rs/build.rs` 把 `KEDAI_DIST_HASH`/`KEDAI_BUILD_TIME` 编进二进制,
   `GET /api/health` 返回 `{ok, ts, version, build_id, build_time, data_dir, dependencies:{db}}`;设置中心底部常驻显示
   「版本 · 构建时间 · 指纹前 8 位」,两端各开一次对比即可肉眼确认同步。
3. **启动层**:`start.ps1` 与图形启动器(`Kedai.exe`,源码 `launcher/`)启动前比对两端
   sidecar 的 `dist_hash`,不一致自动执行 `build.ps1` 双端重建;`Kedai.lnk` 指向图形启动器,
   双击自带过期/漂移检测(直接双击 dist 里的裸便携版 exe 没有这层保障)。

### 部署方式

把整个 `kedai/` 目录(或仅 `exe + data + logs`)拷到目标机即可。exe 通过向上遍历定位项目根(找 `web` 目录),因此 **exe 需放在 `server-rs/target/release/` 原路径,或保证其上级存在 `web` 目录**。若想单独分发,保持目录结构即可。

---

## 5. 启动与配置

### 启动器(start.ps1,推荐)

统一 PowerShell 启动脚本(旧 C# `kedai.exe` / `启动Kedai.bat` 已废弃删除):
- 测试版(默认 `.\start.ps1`):启动 `dist\kedai-server.exe` 并自动打开浏览器;`-NoBrowser` 不开浏览器,`-Build` 强制先执行 `build.ps1`。
- 正式版(`.\start.ps1 -Portable`):启动 `dist\Kedai-portable\Kedai.exe` 便携版桌面应用(数据存 `%APPDATA%\com.kedai.app`);双击入口是项目根 `Kedai.lnk`(指向 `Kedai.exe` 图形启动器,自带过期/漂移询问重建)。
- 自动重建:启动前检测产品源码(web/src、server-rs/src、src-tauri/src 等)是否比可执行产物新,过期则自动执行 `build.ps1`(默认双端同步);`-NoRebuild` 跳过。
- **同步纪律**:测试版与便携版是两条独立编译链的产物,`build.ps1` 默认双端同步产出;只有 `-TestOnly`/`-Dev` 快速通道会只刷新测试版。两端 sidecar 指纹(`<exe>.build.json`)不一致时,启动脚本与图形启动器都会自动触发双端重建,无需人工记忆。
- 两版共用端口 3001 与同一数据目录,勿同时运行。

### 配置(.env,复制自 `.env.example`)

| 变量 | 默认 | 说明 |
|---|---|---|
| HOST / PORT | 127.0.0.1 / 3001 | 监听地址(3001 是前端代理硬契约) |
| DATA_DIR | ./data | SQLite + 角色卡 + avatars |
| CONNECTOR | openai-compatible | `openai-compatible` \| `mock`(未知回退 mock) |
| OPENAI_BASE_URL | https://api.openai.com/v1 | 兼容 Oobabooga/vLLM/Ollama 等 |
| OPENAI_API_KEY | (空) | 无 Key 时自动 mock 演示 |
| OPENAI_MODEL | gpt-4o-mini | 默认模型(可在设置中运行时切换) |
| DEFAULT_TEMPERATURE / DEFAULT_MAX_TOKENS | 0.8 / 1024 | 生成默认参数 |
| LOG_LEVEL | info | debug/info/warn/error |

> `.env` 加载:`dotenvy` 从当前工作目录向上搜索。启动器/直接运行 exe 时 cwd 为项目根,配置正常。

---

## 6. API 契约(维护红线)

> 前端 `api.ts` 与后端 `types.rs` 的字段必须保持逐一对应;改任何响应结构需两端同步,否则前端渲染异常。

### 路由总表

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | /api/health | `{ok, ts, version, build_id, build_time, data_dir, dependencies:{db}}`(`ok` 恒 true 为 liveness;`?deep=1` 触发 quick_check) |
| POST | /api/chat/send | SSE 流(见下) |
| POST | /api/chat/stop | `{session_id}` → `{ok:true}` |
| GET | /api/chat/sessions?character_id= | `{sessions:[...]}` |
| POST | /api/chat/sessions | `{character_id, title?}` → 201 session |
| DELETE | /api/chat/sessions/{id} | 204 |
| GET | /api/chat/history?session_id= | `{messages:[...]}`(id 自增数字) |
| PUT/DELETE | /api/chat/messages/{id}?session_id= | 编辑(extra 覆写为 `{edited:true}`)/ 删除(204) |
| POST | /api/chat/clear | `{session_id}` → `{ok:true}`(清消息保留会话) |
| GET | /api/characters | `{characters:[...]}`(列表**不含** data_raw) |
| POST | /api/characters/upload | multipart 字段 `file`,20MB,→ 201 完整记录 |
| GET/PUT/DELETE | /api/characters/{id} | 详情(含 data_raw)/ 更新 / 删除(204,级联) |
| POST | /api/settings/connect | `{ok, message, models}` |
| GET | /api/settings/models | `{models:[...]}` |
| GET | /api/settings/info | `{connector, model, models, availableConnectors}` |
| GET/PUT | /api/settings/model | 获取 `{model}` / 切换 `{model}` → `{ok, model, changed}` |
| POST | /api/token/count | `{messages, model?}` → `{total, model}`(每条+4,末尾+2) |
| GET | /api/export/chat?session_id= | `{session_id, messages:[StMessage]}` |
| POST | /api/import/chat | `{session_id, messages}` → `{ok, imported}`(先清空) |
| POST | /api/agent/plan | `{message, agent_mode?, session_id?}` → `{plan, summary, tools, history?}` |
| POST | /api/agent/execute | 固定桩(引擎自动编排,不真正执行) |
| POST | /api/agent/interrupt | 等价 /api/chat/stop |
| GET | /api/avatars/{file} | 头像静态文件(数据目录 avatars) |

### SSE 事件格式(chat/send)

```
data: {"type":"step","step":"计划中…","detail":"..."}\n\n
data: {"type":"token","text":"片"}\n\n
data: {"type":"tool_call","name":"calculator","input":{...}}\n\n
data: {"type":"tool_result","name":"calculator","output":{...}}\n\n
data: {"type":"interrupted"}\n\n
data: {"type":"finish","usage":{prompt_tokens,completion_tokens,total_tokens,context_tokens},"content":"..."}\n\n
```

- 格式:`data: {json}` + 空行,`type` 编码在 JSON 内,无 `event:` 字段
- 事件类型:`token` `step` `tool_call` `tool_result` `interrupted` `finish`
- `finish.usage.context_tokens` 为单独计算的上下文总 token
- 客户端断开时自动中断生成(engine 检测 mpsc send 失败)

### 约定

- 错误响应统一 `{"error":"消息"}`;已接入结构化错误码的端点附带 `"code"`(如 `{"error":"会话不存在","code":"NOT_FOUND"}`,码表见 `server-rs/src/api/errors.rs`:VALIDATION/UNAUTHORIZED/NOT_FOUND/CONFLICT/DB/UPSTREAM/INTERNAL,前端按 code 分类提示)
- 400 缺参/校验失败,404 资源不存在,409 会话生成中,204 删除成功,201 创建成功
- 全部时间字段为 ISO 8601 字符串

---

## 7. 数据库(data/kedai.db)

rusqlite(bundled,零原生依赖),**WAL 模式 + foreign_keys ON**。共 28 张表(定义见 `server-rs/src/models/db/schema.rs`),核心表:

| 表 | 关键字段/约束 |
|---|---|
| characters | id PK, name, chara_name, description, file_path, avatar_path, data_raw(JSON 字符串), created_at |
| sessions | id PK, character_id **FK→characters ON DELETE CASCADE**, title, created_at, updated_at |
| messages | id INTEGER PK AUTOINCREMENT, session_id FK→sessions CASCADE, role CHECK(user/assistant/system), content, extra(JSON), created_at;索引(session_id, id) |
| agent_sessions | id PK, session_id **UNIQUE** FK→sessions CASCADE, state, plan/steps(JSON), step_index, agent_mode, started_at, updated_at |
| tool_calls | id PK, agent_session_id FK→agent_sessions CASCADE, name, input/output(JSON), duration_ms, created_at;索引(agent_session_id) |
| world_books | 独立世界书:原始 JSON(data_raw)、绑定角色、启用状态、条目统计 |
| skills / agent_subtasks | 技能库 / Agent 子任务 |
| session_vars / session_assistant_vars | 会话宏变量 / 酒馆助手变量树(stat_data) |
| session_usage / global_usage | 会话级 / 全局 token 用量统计 |
| tasks / task_subtasks / task_usage / task_llm_calls / task_messages | 任务模式:任务/子任务/token 用量/LLM 调用追踪/阶段消息 |
| memory_entries | 跨会话记忆蒸馏条目(按角色维度,含 selected/pinned) |
| scope_variables / backfill_meta | 7 作用域变量 / 增量回填游标 |
| contract_changelog / kaleido_state / kaleido_changelog | 契约变更历史 / kaleido 状态与历史 |
| llm_requests / session_compactions / undo_snapshots / user_scripts / quick_replies | LLM 请求缓存诊断 / 压缩记录 / 撤销快照 / 用户脚本 / 快速回复 |

> 迁移逻辑见 `server-rs/src/migration/`(backup/conflict/ddl/merge/mod 目录模块),表结构定义见 `server-rs/src/models/db/`(mod/schema/backfill)。

### 维护注意

- **数据兼容**:表结构与原 Node 版完全一致,旧 `kedai.db` 可直接使用(已验证)
- **备份**:复制 `data/` 目录即可(SQLite WAL 模式建议先停服再拷,或同时拷 `-wal`/`-shm`)
- **⭐ 代码约定**:`Db::write()` 返回 `MutexGuard<Connection>`,**必须在其作用域结束后再调用其他取写锁方法**,否则 std::sync::Mutex 重入死锁(曾踩坑,详见 §10);只读查询用 `Db::read()`(连接池,无此约束)

### DB 并发纪律(2026-08 改造)

- **两类句柄**:`Db::read()` 返回只读连接池句柄(`PooledRead`),**SELECT 专用**,可并发多连接;`Db::write()` 返回全局唯一写连接的 `MutexGuard`,一切写 SQL 与「读+写同事务」的混合场景走它。
- **handler 必须过阻塞池**:api handler 禁止在 async 上下文直接执行同步 DB 调用(会卡住 tokio worker);一律经 `AppState::db_call`(任意 services 同步方法)/ `db_read`(只读闭包)/ `db_write`(写闭包),三者内部均为 `spawn_blocking`。
- **db_write 非重入**:闭包执行期间已持有唯一写连接,**闭包内不得再调用会重新获取写锁的 services 写方法**(Mutex 非重入 → 自死锁);需要走 services 写方法时用 `db_call`(连接由服务内部按需获取)。
- **失败出口**:阻塞任务 JoinError / 连接池错误 / 服务内 String 错误统一由 api 层 `db_err` 转 500 + `code: "DB"`;锁中毒一律恢复(`.lock().unwrap_or_else(|e| e.into_inner())`),不 panic。

---

## 8. 日志

- 位置:`logs/kedai-YYYY-MM-DD.log`,按天归档(自研 DailyFileWriter,命名与旧 logger 时代一致)
- 格式:pino 风格 JSON 行:`{"level":"info","time":"...","msg":"...",...自定义字段}`(2026-09 D-4 起键序非字母序:level/time/msg 在前;离线比对脚本注意)
- 实现:tracing + tracing-subscriber(PinoFormat)+ non-blocking 双 channel(stdout/文件),热路径无锁;`RUST_LOG` 环境变量覆盖级别(默认取 LOG_LEVEL/info,白名单压制第三方库 debug 噪音)
- 双写:控制台 + 文件;启动时清理 3 天前的旧日志
- 新增日志字段约定(见 utils/logging.rs 头部):字符串走 record_str 绝不回解析;复合 JSON 值经 `%JsonField(v)` 透传
- 排查问题先看 `logs/` 当天文件

---

## 9. 常见维护操作

| 场景 | 操作 |
|---|---|
| 换 LLM 后端 | 改 `.env` 的 OPENAI_BASE_URL / API_KEY,重启 |
| 换默认模型 | 改 OPENAI_MODEL;或运行中在设置界面切换(持久化于内存,重启失效) |
| 改默认系统提示词 | 改 `data/settings.json` 的 `agent_system_prompt`(优先级最高,保存即生效);运行中的服务需重启才读取;代码内置默认在 `server-rs/src/services/settings_service/params.rs`(default_roleplay_agent_prompt,新装/空值回填用);显式清空时另由引擎兜底模板兜底:`server-rs/src/agents/engine/messages/build.rs`(build_llm_messages_with_position),均需重编译 |
| 演示模式 | CONNECTOR=mock 或清空 API Key,启动即演示 |
| 端口被占 | `netstat -ano \| findstr ":3001"` → `taskkill /f /pid <pid>`;注意可能残留 Node 旧服务 |
| 前端改了没生效 | 需重新 `.\build.ps1`(exe 内嵌的是编译期快照;开发期则靠 web/dist 磁盘优先) |
| 数据迁移/备份 | 停服后复制 `data/` |
| 磁盘瘦身 | 见下方「空间清理与数据布局」:可安全删除 `server-rs/target`、`node_modules`、`sandbox`、`dist`、`logs` 等可再生目录 |
| 跑测试 | vcvars64 环境 + `cargo test` |
| 深链设置 | 浏览器访问 `http://127.0.0.1:3001/#settings` 直接开设置 |

### 空间清理与数据布局(磁盘瘦身)

> 项目主要占用来自**构建缓存**(可安全删除、随构建自动重建);用户数据集中在 `data/`,**切勿删除**。

| 目录 | 典型大小 | 可否删 | 恢复方式 |
|---|---|---|---|
| `server-rs/target/` | 数 GB~11GB | ✅ | `cargo build/test`(vcvars64 环境,全量重编) |
| `src-tauri/target/` | ~2.3GB | ✅ | `build.ps1 -Tauri` 重打桌面包 |
| `node_modules/`(根) | ~112MB | ✅ | `npm install`(根目录,npm workspace 自动装 `web/`) |
| `sandbox/` | ~65MB | ✅ | 需参考插件源码时从 GitHub 重新下载 |
| `dist/` | ~28MB | ✅ | `build.ps1 -Tauri` 重建便携版 |
| `data-backup-before-v3fix/` | ~20MB | ✅ | 一次性旧备份,删后不可恢复 |
| `launcher/target/` | ~10MB | ✅ | 编译启动器时重建 |
| `logs/` | ~1MB | ✅ | 运行日志,启动时自动重建(桌面场景在 `%APPDATA%\com.kedai.app\logs`) |
| **`data/`** | — | ❌ | **用户数据**:`kedai.db`(SQLite)、`characters/`(角色卡原图+JSON)、`avatars/`、`settings.json`、`prompt_floors.json`、`agent_flows.json`、`AGENTS_RUNTIME.md` |
| `web/dist/` | — | ❌(建议保留) | 前端产物,exe 编译期内嵌;删除后仅影响「磁盘优先」调试路径,内嵌版本仍可用 |
| `.env` / `src/` / 文档 | — | ❌ | 源码与配置 |

- **清理命令**(PowerShell,应用停止时执行):
  ```powershell
  Remove-Item server-rs\target, src-tauri\target, launcher\target, node_modules, sandbox, dist, logs, data-backup-before-v3fix -Recurse -Force
  npm install   # 恢复依赖(如不删除 node_modules 可跳过)
  ```
- **实测记录(2026-08-12)**:删除上述全部目录回收约 **13.5GB**(server-rs/target 11.3G + src-tauri/target 2.3G + node_modules 112M + sandbox 65M + dist 28M + data-backup 20M + launcher/target 10M + logs 1.2M)。`data/`(角色卡、数据库、设置)与 `web/dist` 未动,应用数据完整。
- **注意**:删除 `server-rs/target` 后,首次 `cargo build/test` 需全量重编(集成测试编译约 2~5 分钟量级);删除 `node_modules` 后必须 `npm install` 才能构建/开发前端。

---

## 10. 已知问题与注意事项(踩坑记录)

1. **cargo 必须 vcvars64 环境**:新终端直接 `cargo build` 会报 `link.exe not found`。用 `build.ps1` 或先 `call vcvars64.bat`。
2. **std::sync::Mutex 不可重入**:所有 `services` 层 DB 操作持锁期间禁止再调用同类方法(会死锁)。目前代码已按"先释放锁再查"处理,新增代码须遵守。
3. **PowerShell 5.1 中文乱码**:`start.ps1` 需 UTF-8 with BOM;`Get-Content` 读中文文件建议 `-Encoding UTF8`。`Write-Content` 默认 UTF-16,写 JSON 给 curl 会带 BOM 导致解析失败(冒烟曾踩坑)。
4. **PowerShell 里 `curl` 是别名**:实际是 `Invoke-WebRequest`,用 `curl.exe` 才是真 curl。
5. **axum 0.8 路由语法**:路径参数用 `{id}` 而非 `:id`;multipart 由 `axum::extract::Multipart` 提供(multer 在 one-shot 测试流下会挂起,故 upload 用了手动字节解析)。
6. **端口残留**:后台/被 kill 的旧实例会短暂占用 exe 句柄,重新编译前确保无 `kedai-server.exe` 进程(`Get-Process kedai-server`)。
7. **tiktoken-rs 首次运行需联网下载 BPE 词表**(与 js-tiktoken 一致);token 计数按模型映射 o200k/cl100k/p50k,未知模型回退 cl100k。
8. **history 注入**:engine 构造 LLM 消息时,`system` 历史消息被跳过(与 Node 版一致,因历史不带 extra),`memory` 消息的注入逻辑在 Node 版存在但当前调用路径不传入 extra,保持行为一致。
9. **`.env` 修改即时性**:配置在启动时读取,改后需重启。
10. **日志编码**:Windows 控制台显示日志中文可能乱码(GDK/UTF-8 混排),不影响落盘内容,可用编辑器以 UTF-8 打开 log 查看。
11. **便携版 README.txt 保留 UTF-8 BOM(有意)**:`dist\Kedai-portable\README.txt` 带 EF BB BF 头——Windows 记事本对无 BOM 的 UTF-8 中文按 ANSI 猜解会乱码,BOM 是兼容性刻意选择,不要在构建脚本里「修复」掉它(2026-09 D-8 注明)。
11. **mvu 系统来源澄清(重要)**:本项目的 mvu 变量系统位于 `web/src/mvu/`(前端)+ `server-rs/src/parsing/assistant/`(后端),是 **MagVarUpdate**(原作者 MagicalAstrogy,`github.com/MagicalAstrogy/MagVarUpdate`,MIT)的独立兼容实现。曾有一份外部 AI 分析以某 SillyTavern 扩展的 `bundle.js`(含 `generation_id` 随机 UUID、`遵循<must>指令`、`兼容假流式`、`额外模型解析配置`、`ordered_prompts` 数组)为依据得出「缓存命中率极低 5 因素」结论——**这些代码在本项目全部不存在**,该分析不适用于 Kedai。Kedai 真实的缓存注意点:变量树经 `{{format_message_variable}}`/`{{getvar::stat_data…}}`/EJS 宏展开进 system 提示词,变量更新会使 system 前缀变化、前缀缓存失效;可用设置 `mvu_vars_position=user_tail` 把变量块挪到最新用户消息尾部以提升命中率(见 API 契约节)。
12. **导入 ST 预设的空楼层过滤**:导入酒馆预设后,纯 `{{addvar}}` 累积宏的楼层展开为空字符串(宏自身输出空),engine 组装消息时已跳过空楼层/空注入(`agents/engine/messages/build.rs` build_llm_messages_with_position),不会产生空 user/assistant 消息;但这类楼层没有可注入的实质内容,若希望「累积宏 + 末尾 getvar 输出」的拼接语义生效,需把实际内容放在带 `{{getvar}}` 输出的楼层里(如「可待精华增强楼层」的写法)。
13. **变量协议示例不要写进恒存在的 system 提示词**:`<UpdateVariable>` 输出协议示例只应由 `make_state_block` 在角色卡有变量树时注入。若把示例硬编码进默认/自定义 system 提示词,无变量树的普通卡也会看到协议:mock `[[floors]]` 回显 system 时示例会被 `parse_update_variable` 解析成真实补丁、空树建树并推送 vars 事件(曾有集成测试全红);真实场景下模型也可能模仿示例输出补丁、误激活变量系统。注意 `apply_mvu_patches` 允许空树建树(用户/模型显式输出补丁是合法语义,`pure_mvu_*` 测试依赖此行为),隔离靠「不暴露协议」而非「禁止空树应用」。
14. **沙箱 realm 与酒馆不同:同卡脚本共享 window 需显式机制**(2026-09 修复记录,commit f2c10d1)。Kedai 每脚本一个不透明源 iframe,同卡多个 tavern_helper 脚本**互不可见 window 全局**——ST 生态脚本「th-A 挂 window.WuWaShared → th-B 读」的写法在这里会失效,直接裸读 `top`/`parent` 还抛 SecurityError(WuWa Solaris-3 卡崩溃根因)。两条通道(按需二选一,勿混):① **同卡同 realm**:卡级脚本已合并进单 iframe,靠逐 `<script>` 注入共享 window(web/src/cardScriptHost.ts + boot-script.ts 多段形态),同段组内的脚本可互读全局;② **跨 realm 共享桥**:不同沙箱(消息级脚本、不同时机起的卡级组)之间靠 shared-globals 快照桥(`web/src/sandbox/shared-globals.ts`),只同步合法键 + JSON 可序列化 ≤256KB 的 window 自有属性,函数/DOM 引用不共享(见 docs/known-limitations.md L5)。另:**运行期注入 `<style>` 会触发 dom-rpc 的声明级清洗 + 容器作用域化**——容器必须带 `data-kd-scope`(渲染块自带,卡级容器由 cardScriptHost 设 `cardScopeId`),否则样式段退回纯白名单仍被剥,界面裸渲染。
15. **悬浮窗脚本依赖的 jQuery 面比想象宽**(2026-09 修复记录,commit 280e1cf)。WuWa 卡三个悬浮窗暴露了三类缺口,新增卡脚本报「悬浮球不显示/拖不动/切卡残留」时按此排查:① **jQuery UI `.draggable()` 沙箱没有**——th-5 无 `typeof` 守卫会直接 TypeError 中断整段脚本(连悬浮球都不挂);现在宿主侧 `web/src/sandbox/draggable.ts` 提供最小实现(handle/cancel/containment/distance + start/drag/stop 回调),能力边界见 known-limitations L6。② **`$('head')` 与 `.css({...})`**:`queryScoped` 只特判 body/html 时,`$('head').append('<style>…')` 零匹配静默丢失(面板失去 position:fixed/display:none → 落进消息流);`.css({k:v})` 对象形式若被当 getter 吞掉,`$('<div>').css({position:'fixed'})` 全部丢失;created 元素 `.attr('id',…)` 同理。③ **注入的 DOM 无人回收**:cleanup 原本只销毁 iframe/监听器/订阅,脚本 `$(window).on('unload')` 钩子在沙箱内静默失效 → 切卡后幽灵悬浮窗。现由 `data-kd-injected` 标记 + `data-kd-overlay-root` 覆层根统一清理;**capture 登记的监听器必须带 capture 摘除**(`removeEventListener` 的 capture 不匹配会摘不掉,拖拽中切卡残留 document 监听)。
16. **改完必须重跑 `build.ps1` 才进 exe**(2026-09 实测八项修复复核)。`cargo test`/`npm run build` 只更新 `server-rs/target/` 与 `web/dist/`;`dist\kedai-server.exe`、`dist\Kedai-portable\Kedai.exe` 与项目根 `Kedai.exe` 都不会自动更新,交付前必须跑一次完整 `.\build.ps1`(前端 + release + 便携版 + 指纹 sidecar)。
17. **16GB 内存 + 11GB 页面文件下禁止全并行 debug 链接**(2026-09 实测踩坑)。`cargo test`/`cargo build`(debug)全并行链接 `libkedai_server-*.rlib`(带 debuginfo 约 665MB)会触发 `os error 1455 页面文件太小`,rlib 被写坏后**后续所有编译持续报 E0786「invalid metadata files」**,表现为莫名其妙的全量失败。规避:`.shakedown/krun.bat` 已封装 `vcvars64 + CARGO_BUILD_JOBS=4 + debuginfo=0`;遇到 E0786 先删坏 rlib(`rm server-rs/target/debug/deps/libkedai_server-*.rlib`)再重建。**注意 release 构建(build.ps1)不受影响**(LTO + strip,产物小),但构建期间不要与前端 vitest/cargo test 并发跑,内存争用会让链接器崩(0xc0000409)。**另见条目 29**:`rustc` 自身崩溃(`STATUS_STACK_BUFFER_OVERRUN`)同样会伪装成「rlib 缺失」,那是另一诱因,处置也不同。
18. **世界书条目的 depth/role 在角色卡里常放在 `extensions`**(2026-09 第四轮实测踩坑)。顶层 `depth` 缺省 4;而真实卡(赛马娘 95 条、爱托邦 109 条、吸血鬼 54 条)**全部**把 `depth` 写在 `extensions`,顶层没有。解析必须两侧都读,否则作者设的 1/2/5 一律退化到 4:depth=1/2 被放宽 → 关键词在窗口外仍命中、污染提示词;depth=5 被收窄 → 漏注入。`probability`/`use_probability` 早有 extensions 回退,`role` 还须兼容 ST 数字(`0/1/2` → system/user/assistant)。改这条解析时对照 `parsing/world_book.rs` 的 `read_role`/depth 段与 `probability` 段保持同一写法;测试要覆盖「顶层优先 / 仅 extensions / 两者都缺省」三态(第四轮修复记录见此文 §10 之后的 `docs/实测四轮修复-变更说明.md` F1)。
19. **卡内 CSS 注释会吞掉紧随的声明**(2026-09 第四轮实测踩坑)。`/* 说明 */\n background-image: …` 的属性名会被声明切分当成 `/* 说明 */ background-image`,不匹配属性白名单 → **整条声明被丢弃**。赛马娘卡因此丢了 `body` 的纸纹背景、`.select-item` 的 `gap`、`.description-box` 的 `padding`。修法是切分前先剥注释(`cssSanitize.ts` 的 `stripCssComments`,引号感知以免误伤 `url("https://a/*.png")`);**必须在 `splitCssBlocks` 之前剥**——注释里含 `{}` 会先破坏顶层块切分。改 CSS 清洗管线时注意 `sanitizeScopedCss`(规则体)与 `sanitizeCssDeclarations`(规则体 + style 属性)两个入口都要经过剥注释。
20. **压缩 / 备份仓库报「系统找不到指定的路径」= jniLibs 里的 .so 符号链接悬空**(2026-09 实测踩坑)。`tauri android build` 为省空间不在 `gen\android\app\src\main\jniLibs\<abi>\` 存真文件,而是**建符号链接**指向 `src-tauri\target\<triple>\release\libkedai_desktop_lib.so`;构建收尾又会删除 `src-tauri\target` 回收磁盘 → 链接悬空。此后用资源管理器 / 7-Zip / HaoZip 压缩或备份仓库,工具跟随不到目标即报 `...libkedai_desktop_lib.so: 系统找不到指定的路径` 并中断,产物不完整。**判定陷阱**:悬空链接上 `Test-Path` 仍返回 `True`(PS 5.1 不穿透解析),必须比对 `(Get-Item -Force).Target` 是否存在;只看 `Test-Path` 会漏掉。现已自动化:`tools\Write-BuildStamp.ps1` 的 `Clear-KedaiDanglingJniLibs` 在删除 target 后清理悬空链接(`build.ps1` 的 `-Tauri` 分支与 `tools\build-portable.ps1` 收尾均已接入,幂等,只删 SymbolicLink 不碰真实文件)。这些链接与 .so 本就是派生文件(`gen/android/app/.gitignore` 已忽略),下次 Android 构建自动重建。
21. **压缩工具会「跟随」junction,把外置目录的体积一起装进压缩包**(2026-09 实测确认:HaoZip 对 junction 是跟随而非跳过)。若把构建产物用 junction 外置(如 `src-tauri\gen\android\app\build` → `D:\kedai-build\...`,1.4GB),则压缩 `D:\kedai` 得到的包会明显大于目录本身——实测 218MB 的目录压出 417MB 包。需要「压缩包 ≙ 目录可见体积」时,压缩前应删除或排除 junction 目标,或改用 7-Zip 的 `-xr!` 排除;另注意**悬空 junction 会被静默跳过、不报错**,与悬空 symlink(条目 20 的报错)行为不同。
22. **`server-rs\target` 下新建的 exe 可能被安全软件拦截执行**(2026-09-13 实测踩坑,os error 5)。现象:`cargo build/check/test` 在**默认 target 路径**下报
    `failed to run custom build command for native-tls … 拒绝访问。 (os error 5)`,失败在这个
    build script 上。逐步排查结论:
    - **不是目录权限**:`icacls` 显示 `server-rs\target` 与可用的外部目录 ACL 完全一致;
    - **不是残留产物**:`rm -rf target` 全量清空后仍在同一位置失败;
    - **不是目录性质**:`dir /AL` 与 `Get-Item … -Force` 确认它不是 junction/符号链接;
    - **是关键线索**:把一个**既有**的 `cmd.exe` 复制进 `server-rs\target\`,能正常执行;
      而 cargo **刚编译出来的** `build-script-build.exe` 一执行就「拒绝访问」——
      即拦截针对「在该目录下新建的可执行文件」,典型的实时防护(杀软)行为。
    **规避**:把后端产物外置到白名单目录,两种等价写法——
    - `.\build.ps1 -RustTargetDir D:\kedai-build`(**推荐给构建脚本**,只作用于 server-rs);
    - 或设环境变量 `CARGO_TARGET_DIR=D:\kedai-build`(作用于所有 cargo 调用,含 src-tauri;
      注意 `build-portable.ps1` 期望 `src-tauri\target\release\kedai-portable.exe`,用此变量会
      让便携版产物落到别处,故优先用 `-RustTargetDir`)。
    `build.ps1` 已支持该参数,并把「后端产物目录」统一解析给新鲜度检测与 exe 校验使用;
    以前它硬编码 `server-rs\target`,外置产物时会误报「Rust 编译失败:未生成 …」
    (2026-09-13 批次 1 起**清理路径也跟随该参数**,外置目录不再漏清);
    同一现象在 `docs/优化实施方案-2026-09.md` 亦有历史记录(当时以 `CARGO_TARGET_DIR` 降级处置)。
23. **src-tauri 内嵌 server-rs 时,两份 Cargo.lock 会静默漂移**(2026-09-13 批次 1 实测并清零)。
    `src-tauri/Cargo.toml` 以 `kedai-server = { path = "../server-rs" }` 内嵌后端,构建便携版时
    cargo **完全忽略 `server-rs/Cargo.lock`**,在 `src-tauri/Cargo.lock` 里重新解析整棵依赖树——
    同一份后端源码,「测试版 `kedai-server.exe`」与「便携版 `Kedai.exe`」可能编译出**不同版本**
    的依赖;跨 lock 的 `links` 冲突不会报错,分歧完全静默。实测两锁曾有 **17 个 crate 分歧**,含:
    - `chacha20 0.10.2` vs `0.10.1`(**0.10.1 已被 yank**,密码学实现版本不同,最危险);
    - `cc 1.4.0` vs `1.4.2`、`find-msvc-tools`(编译 C 依赖的工具链,影响生成的 native 代码);
    - `thiserror`、`crossbeam-channel/-utils`、`combine`、`indexmap`、`tinyvec`、
      `wasm-bindgen` 家族(0.2.126 vs 0.2.127,wasm 目标专用,影响面小)。
    **纪律**:① `node tools/check-lock-sync.mjs`(已进 `check-all` 阶段 `deps: check-lock-sync`,
    npm `check:lock`)以 **0 漂移**为基线,新增即 FAIL;② 新增依赖后两边各构建一次
    (便携版构建会自动带上 src-tauri 锁,测试版锁不会);③ 对齐手法:
    `cargo update -p <crate> --precise <ver>`(**不支持一次多条 `--precise`**,逐条执行),
    或把落后一侧整体更新到较新版本;④ 不可避免的例外登记 `tools/lock-sync-baseline.json`;
    ⑤ **本仓自身包**(`kedai-server`/`kedai-desktop`/`kedai-web`/`kedai`/`launcher`)已从比对中排除
    —— 版本号由 `bump-version.ps1` 变更后,两锁的自身版本会暂时不一致(各自构建时才更新),
    属发版的正常中间态而非依赖漂移(2026-09-13 实测:发版时该门禁曾误报,已加白名单)。
    根治途径是 Cargo workspace 化(未做,列入批次 4 可选项)。**注意**:对齐 `cc`/`thiserror`
    等版本后须重跑完整 `cargo test`(本批次已跑,全绿)。
24. **API Key 解密失败曾在一次保存后被静默清空**(2026-09-13 批次 2 修复)。`secret_store::unprotect`
    解密失败返回空串(视为未配置,避免把密文当 Key 发给上游),而 `save` 直接 `protect(内存值)` ——
    换用户/换机器/密文损坏后,用户下一次保存设置(改个温度也算)就会用空串覆盖磁盘密文,
    密钥永久丢失且只有一行 stderr。**修法**:`settings_service/secret.rs::save` 先读盘,
    「内存为空而磁盘仍是非空 `enc:v1:` 密文」时原样回写密文并留痕。**不会误伤清空操作**:
    `PUT /api/settings` 对空值直接忽略(`api/settings.rs:300-305`),接口层面无法把已配置 Key 改为空。
    改密钥相关代码时保持该不变式,并跑 `secret.rs` 的 3 个保留测试。
25. **schema.rs 加列漏写 `ensure_*` 迁移只在老用户机器上炸**(2026-09-13 批次 2 加元测试)。
    新装机走 `CREATE TABLE` 全量建表长得一切正常,全量测试也发现不了;而老库升级后访问该列直接
    `no such column`。**守卫**:`tests/schema_migration_meta.rs` 用冻结基线
    (`tests/fixtures/schema_baseline_v0_3_0_beta.sql`,0.3.0-beta 发布时的建表 SQL)建「老库」→
    `Db::open` 升级 → 与全新库逐表比对 `PRAGMA table_info` + 索引;新增列/表/索引漏迁移即失败
    (已用注入探针验证)。**纪律**:改 `schema.rs` 的建表结构必须同步写 `migration/ddl.rs` 的
    `ensure_*` 迁移;**不要更新基线文件**(它是历史快照,更新它等于关掉守卫)。
26. **外置产物目录不能整删:会连带删掉同目录下的 Android 构建目录**(2026-09-13 实测踩坑)。
    `build.ps1` 的收尾清理在批次 1 改为跟随 `-RustTargetDir` 后**整删外置目录** —— 而本机
    `D:\kedai-build` 下还挂着 `src-tauri\gen\android\app\build` 的 **junction 目标**
    `android-app-build`。整删后 APK 构建在 `:app:mergeUniversalReleaseJniLibsFolders` 报
    `Failed to create parent directory ...app\build`(悬空 junction 无法再被当作目录创建),
    报错文案**不指向真因**,排查成本高。**纪律**:默认路径 `server-rs\target` 可整删;
    **外置目录只清 cargo 自己的子目录**(`debug`/`release`/`tmp`/`CACHEDIR.TAG`/`.rustc_info.json`),
    保留同目录其它数据(`build.ps1` 与 `tools/build-portable.ps1` 已按此修正)。
    修复手法:重建 `D:\kedai-build\android-app-build`(空目录即可,gradle 会重新生成内容)。
    另注:压缩/备份工具会**跟随** junction(踩坑 21),与本次「悬空 junction 无法创建目录」
    是两个不同现象,勿混。
27. **构建/门禁报 `failed to remove file ... os error 5`,先查是不是 exe 还在跑**(2026-09-13 实测)。
    现象:开发时用 `cargo run` 或 `start.ps1` 起了 `kedai-server`,随后跑 `build.ps1` /
    `check-all.ps1`,`cargo` 重新链接时报:
    ```
    error: failed to remove file `...\target\debug\kedai-server.exe`
    Caused by:
      拒绝访问。 (os error 5)
    ```
    由 `check-all` 的 `cargo test --workspace` 阶段抛出时,汇总表只显示 `FAIL`,文案像权限/杀软问题,
    **实际根因是 Windows 不允许删除 / 覆盖正在运行的 exe**(错误发生在「删除」),与踩坑 22
    (杀软拦截**新建 exe 的执行**,错误发生在「执行」)是两回事,两者共用 `os error 5` 这个码,极易混淆。
    已验证的判定手法:用 `FileShare.None` 独占打开产物,成功=未被占用、失败=被占用
    (注意不能只看「能不能删」:Windows 对运行中的 exe **有时允许删除**(删除挂起,名字立即释放、
    进程继续跑),此时 `Remove-Item` 会成功但产物已消失,症状反变成「未生成 exe」)。
    **规避**:编译前跑 `Clear-KedaiLockedServerArtifacts`(`tools/Write-BuildStamp.ps1`,已接入
    `build.ps1` / `check-all.ps1` / `build-portable.ps1`)——把被占用的产物改名让位为 `<exe>.old`,
    cargo 随即写入同名新文件,**无需杀进程**,旧进程继续跑旧代码;未被占用的产物一律不动(不误伤增量编译)。
    另注:`cargo test` 期间若有 `kedai-server` 在跑,个别用例(如 `settings_connector` 的
    `mock_auto_switches_to_openai_on_save`)可能因占用 `settings.json` 报 `Os { code: 32 }`
    而假红;跑门禁前先关掉手动起的服务实例。
28. **世界书 `depth` 是「插入深度」不是「关键词扫描窗口」,两者混用会让条目永不触发**(2026-09-14 实测踩坑,吸血鬼卡「修复后仍丢格式」根因)。
    SillyTavern 语义(`public/scripts/world-info.js`):`entry.depth` ↔ `originalData.extensions.depth`,
    只在 `position=4`(atDepth)时决定注入插入到倒数第几条;关键词**扫描窗口**是**另一个字段**
    `entry.scanDepth` ↔ `extensions.scan_depth`(取值 `entry.scanDepth ?? 全局扫描深度`,从不读 `depth`)。
    踩坑 18 给 `depth` 补 `extensions` 回退是对的,但第四轮修复同时把它**当扫描窗口**用;
    吸血鬼卡格式条目 `extensions.depth=1` 因此被读成「只扫最近 1 条消息」,而作者页把整段
    `<chat_history>` 压成**一条** user 消息、触发词 `system log` 只在开头的 `/* system log … */` 注释行 →
    窗口只剩尾部 `<user_input>` 那条 → 格式规范永不注入 → 卡片报「丢格式」。
    现行实现:`WorldEntry.scan_depth` 独立字段(解析 `scan_depth`/`scanDepth`/`extensions.scan_depth`),
    两条路径统一经 `parsing::world_book::scan_window_len` 取窗口——**卡片生成(generate-raw)未声明则扫全部**
    (作者页自组的是扁平上下文,没有"最近聊天"概念),**角色扮演引擎未声明则沿用 depth 兜底**
    (存量卡行为不变)。改这里必须同时看这两个调用点,别再各写一份。
    另一处陷阱:`merge_entries_into` 曾是**整体替换**条目对象,而 `extensions` 不在前端视图里 →
    任何一次条目编辑保存都会静默抹掉 `extensions`(depth/scan_depth/role 全退化),已改为字段叠加
    (`overlay_view`)。改世界书写回时注意:视图为空的正则/角色要写显式 `null`,否则旧值"复活"。
29. **见到「依赖 rlib 缺失」先降并发,不要改依赖**(2026-09-15 实测,批次 0 入档)。现象文案:
    `error: crate <name> required to be available in rlib format, but was not found in this form`,
    或 `error[E0463]: can't find crate for kedai_server`。**真实原因不是代码或依赖问题**:
    默认并行度下 **rustc 进程自身崩溃**(退出码 `0xc0000409 STATUS_STACK_BUFFER_OVERRUN`),
    崩溃信息在日志末尾可见;进程被杀导致依赖产物缺失,才表现为「找不到 rlib」。
    **处置**:改用 `-j 2` 复跑(**实测一次通过、0 编译错误**),**不要改依赖**——照报错去动 `Cargo.toml`
    只会越改越偏。`tools/check-all.ps1` 的 `cargo test --workspace` 步骤已带 `-j 2`
    (注释记录了本机并行链接的历史故障),但该规避**只覆盖脚本内部**:直接调用裸 `cargo test` 仍会撞上,
    故 `AGENTS.md` 的验证命令也已补 `-j 2`。
    **与条目 17 相关,此为另一诱因**:条目 17 是 16GB 内存 / 11GB 页面文件下全并行链接的**内存耗尽**
    (表现为 `os error 1455` / `E0786 invalid metadata files`),本条是 **rustc 自身栈溢出崩溃**;
    两者都表现为「rlib 异常」,但诱因与处置不同,勿混。
30. **`target\debug\deps\` 里只有 `.rmeta` 没有同名 `.rlib` 的孤立产物**(2026-09-15 实测 **183 个**)。
    `cargo check` / `cargo clippy` 只产元数据不产 rlib,与 `cargo test` / `cargo build` 交替执行时便会残留
    ——它正是上一条「找不到 rlib」的**直接来源**。**处置**:清掉孤立 `.rmeta` 及与之对应的
    `.fingerprint\*-<hash>` 目录(**成对删除**:只删 `.rmeta` 会留下失配指纹,反而引出新怪象)。
    此类残留会随 `check`/`clippy` 与 `test` 交替执行而**持续累积**,故每次撞上「找不到 rlib」
    先看这里,再按条目 29 降并发复跑。
31. **360 安全卫士拦截 cargo 新生成的 build script 可执行文件**(2026-09-15 实测,os error 5)。
    **现象**:release 构建在 `icu_normalizer_data` / `icu_properties_data` 等 build script 处失败:
    `error: failed to run custom build command for <crate>`,
    细节为 `could not execute process ...\build-script-build (never executed)` +
    `拒绝访问。 (os error 5)`。**注意它看起来像依赖或权限问题,实际都不是**。
    **逐步排除(照此自查,别绕远路)**:
    - `Get-MpComputerStatus` 显示 Defender `RealTimeProtectionEnabled=False` → **不是 Defender**;
    - SAC(`HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy` 的 `VerifiedAndReputablePolicyState`)
      为 `0` → **不是 Smart App Control**;
    - **决定性证据**:把一个**既有**的 `cmd.exe` 复制进同一目录能正常执行,而 cargo
      **刚编译出来**的 build script exe 一执行就「拒绝访问」,且随后**被安全软件删除**
      (手动补放同名文件,重跑后仍消失)→ 拦截针对「新生成的可执行文件」,
      **与条目 22 同源**(同一台机器上 360 安全卫士的实时防护;
      `Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntiVirusProduct` 可确认其存在)。
    **处置**:临时关闭 360 实时防护,或把 `server-rs\target` 与 `src-tauri\target` 加入信任区,
    再重跑 `.\build.ps1`。
    **实测无效的绕法(别试)**:`-j 1` 降并发、外置 `CARGO_TARGET_DIR` / `-RustTargetDir`(条目 22 的
    规避对本例**无效**——拦截对象是 cargo 的 build script,不是最终链接出的 rlib/exe)、
    清理 `.fingerprint` 与新产物。以上均复测仍被拦。
    **影响面提示**:本例中**测试版 `dist\kedai-server.exe` 已正常产出**,只有 `src-tauri`
    便携版链路被阻断——故「便携版缺失」时先按本条查杀软,不要怀疑 Rust 代码。
32. **测试临时数据目录从不清理,会把 %TEMP% 撑爆**(2026-09-15 实测并修复,测试基建批次 R5)。
    **现象**:测试套件到处 `std::env::temp_dir().join(format!("kedai-..."))` 建数据目录
    (DB/角色文件/settings.json 等),但**从不删除**;即使少数用例写了 `remove_dir_all`,
    也在 `unwrap()` 断言之后,断言失败或 panic 时照样残留。
    **实测数字**:清理前 `%TEMP%` 累计 **14015 个** `kedai-*` 目录 / **11.22 GB**(最早 09-11,
    最新 09-15),前缀十几种:`kedai-tool-test-*`(2418)、`kedai-memory-*`(1804)、
    `kedai-contract-history-*`(1148)、`kedai-session-*`(984) 等;单跑一次全量
    `cargo test` 就新增 **105 个目录 / 17 种前缀**。这是压满 C 盘的主力之一。
    **约定(新增测试必须遵守)**:
    - 一律用 `server-rs/src/utils/test_support.rs` 的 `TempDataDir::new("<tag>")`
      (目录名 `kedai-<tag>-<uuid-v4>`,作用域结束 best-effort 递归删除,失败静默忽略且绝不 panic);
      **不要再裸写 `std::env::temp_dir().join(...)` 建目录**。模块门控
      `cfg(any(test, feature = "test-support"))`,集成测试经 `Cargo.toml` 的自引用
      dev-dependency(`kedai-server = { path = ".", features = ["test-support"] }`)打开。
    - **守卫的析构顺序是硬约束**(2026-09-15 rustc 1.97 实测,写反了就是「句柄还开着 → 删除失败 →
      静默残留」,不报错但没修好):局部变量按声明逆序析构(守卫声明在 Db/服务**之前**);
      解构绑定按绑定逆序析构(故 `let (guard, svc) = service();`,**不是** `let (svc, guard)`);
      结构体字段按声明顺序析构(守卫必须是**最后一个字段**);整个元组不解构时按字段顺序。
    - 禁止临时值形态 `Db::open(&TempDataDir::new("x").path().join("kedai.db"), ..)`——
      语句末守卫即析构,库还开着就删目录。必须先 `let dir = TempDataDir::new("x");`。
    - **不要**给 `lib.rs::build_test_app` 的目录套守卫:它是 `OnceLock` 进程级单例,
      生命周期等于测试进程,套上会在第一个用例结束就删掉后续所有用例要用的库。
      个别「按需构造路径」的读取型代码(`tests/undo.rs`、`tests/agent_trace.rs` 等)同理保持现状。
    - 少量确实需要「Drop 后仍检查目录」的场景用逃逸阀 `TempDataDir::keep() -> PathBuf`(极少用)。
    **本批次后的实测残留**(2026-09-15,清空 %TEMP% 后单跑一次全量 `cargo test`):
    迁移前 **105 个**目录 / 17 种前缀 → 迁移后 **约 30 个**,且全部属「有意不改」类别:
    `kedai-test-<pid>`(≈19,`build_test_app` 进程级共享)、`kedai-secure-test-<uuid>`(7,
    `build_secure_test_app` 每次调用新建)、`kedai-tool-test-<uuid>`(3)。
    最后 3 个是**已知有界残留**:`tools/agent_tools.rs` 的 `agentgo_*` 用例
    `tokio::spawn` 的后台子任务持有 `Arc<ToolDeps>`(内含 `Arc<Db>`),测试函数返回后仍存活
    占用目录——实测把 Drop 重试拉长到 500ms 仍清不掉,属结构性的「句柄生命周期长于守卫」。
    量级从「每次全量 105 个 / 十几种前缀、无上限增长」降到「约 30 个固定项」,
    增长速率已归零(反复跑不再累积新前缀)。

---

## 11. 测试

```bash
cd server-rs && cargo test
```

- 单元测试(源文件内 `#[test]`/`#[tokio::test]`,**827 个**,以 `tools/count-tests.mjs` 为准):状态机迁移、planner(fast/deep/算式识别)、reflector(3 规则)、calculator(白名单解析)、censor(禁词同义替换)、token 编码映射与估算、工具注册表、世界书转换、世界书注入(`scan_depth` 扫描窗口:`scan_window_len` 优先/兜底/0/封顶、`scan_depth` 三来源解析、显式 scanDepth 覆盖 depth 兜底)、提示词注入(含禁词库)、mvu 变量系统(含 JSONPatch 转义/reason/delta 容错)、EJS 渲染器(含读取 API、escape-ejs 与**循环预算/解析深度守卫 5 例**)、角色卡解析、正则脚本、@INJECT 解析/应用、GENERATE 注入、运行时提示词内置默认回退、角色扮演默认提示词内置(from_config + load 空值回填)、结构化错误码(api/errors.rs)、任务编排工具契约(agentgo 逐项校验/子任务截断判失败/read subtask 多键命中/todo 跨 agent 可见/agentend interrupted)、任务工具策略(deny_dangerous 的 bash 例外不外溢:含 `bash2`/`mcp_x_bash` 精确匹配护栏)、任务白名单不豁免命令级高危硬门(custom_authorized + rm -rf/sudo 必须拒绝)、generate-raw 结构化预算下限与截断自愈(含显式值钳制)、**generate-raw 世界书窗口不认条目 depth(回归护栏)+ 吸血鬼卡真实形状格式条目必注入**、**密钥保留策略(批次 2:解密失败不覆盖磁盘密文 3 例)**、**pending_runs RAII 守卫(drop 与 panic 路径)**、**db writer 锁中毒回滚(未完成事务 ROLLBACK)**、**世界书概率门控不变式(常驻条目不参与概率 / 触发条目参与,2 例)**、**system 前缀不被概率扰动(构建侧锁定)**、**截断自愈预算四路合一(utils::retry 纯函数 7 例:翻倍/精确命中封顶/已封顶返回 None/饱和不溢出/零边界/仅 length 且轮次内触发/单轮限制;task_service 封顶回退 1 例见 [抽象收敛与残余修复-变更说明.md](docs/抽象收敛与残余修复-变更说明.md))**、**用量落库事务化(global_usage 写入失败回滚 1 例、会话与全局同进同退 1 例)**、**消息删除的 message 作用域变量清理(单条/截断/清空 3 例)**、mvu 变量系统(含 JSONPatch 转义/reason/delta 容错)、EJS 渲染器(含读取 API、escape-ejs 与**循环预算/解析深度守卫 5 例**)、角色卡解析、正则脚本、@INJECT 解析/应用、GENERATE 注入、运行时提示词内置默认回退、角色扮演默认提示词内置(from_config + load 空值回填)、结构化错误码(api/errors.rs)、任务编排工具契约(agentgo 逐项校验/子任务截断判失败/read subtask 多键命中/todo 跨 agent 可见/agentend interrupted)、任务工具策略(deny_dangerous 的 bash 例外不外溢:含 `bash2`/`mcp_x_bash` 精确匹配护栏)、任务白名单不豁免命令级高危硬门(custom_authorized + rm -rf/sudo 必须拒绝)、generate-raw 结构化预算下限与截断自愈(含显式值钳制)、**密钥保留策略(批次 2:解密失败不覆盖磁盘密文 3 例)**、**pending_runs RAII 守卫(drop 与 panic 路径)**、**db writer 锁中毒回滚(未完成事务 ROLLBACK)**、**世界书概率门控不变式(常驻条目不参与概率 / 触发条目参与,2 例)**、**system 前缀不被概率扰动(构建侧锁定)**、**截断自愈预算四路合一(utils::retry 纯函数 7 例:翻倍/精确命中封顶/已封顶返回 None/饱和不溢出/零边界/仅 length 且轮次内触发/单轮限制;task_service 封顶回退 1 例见 [抽象收敛与残余修复-变更说明.md](docs/抽象收敛与残余修复-变更说明.md))**、**用量落库事务化(global_usage 写入失败回滚 1 例、会话与全局同进同退 1 例)**、**消息删除的 message 作用域变量清理(单条/截断/清空 3 例)**
- API 集成测试(`tests/` 21 个文件,**190 个**,以 `tools/count-tests.mjs` 为准,mock 连接器 + 临时数据目录):api_integration、assistant、agent_flows、tasks、task_events(含 `task_solo_offers_bash_tool`:任务模式确实下发 bash)、generate_raw(自愈成功且 injected 保留 / 重发失败回退半截 / 自愈用尽返回末次文本 / max_tokens=0 400)、**schema_migration_meta(老库升级结构一致性,冻结基线见 `tests/fixtures/schema_baseline_v0_3_0_beta.sql`)**、prompt_inject、world_books、settings_connector、security、contracts_e2e、scripts_e2e、scripts_import、swipe_regenerate、undo、user_scripts、variables_scopes、db_concurrency、macros、repo_index——health、角色 CRUD(multipart 上传)、会话/消息/导入导出、设置与 token、agent plan、SSE 聊天流、任务引擎六模式、计算器工具 SSE、世界书/角色卡、提示词注入与酒馆预设导入、鉴权
  - **已知 flaky**:`settings_connector::mock_auto_switches_to_openai_on_save` 偶发因 Windows 文件占用失败
    (`settings.json 应已持久化: Os { code: 32 }`;另实测全量负载下的 `Os { code: 2 } NotFound` 变体),
    隔离重跑即通过——非代码缺陷:PUT 保存是 `db_call` 同步 await(`api/settings.rs:657-662`),返回 200 时
    文件必已落盘,失败属环境文件锁/时序竞争。CI 接入时给该断言加重试或改经 API 校验。
- 前端 `npm test -w web`(Vitest,**768 个 / 83 文件**,数字以 `tools/count-tests.mjs` 为准;vitest 实际输出为 **786**——差值 18 来自 `parser.contract.test.ts:68` 对 `mvu_patch_cases.json` 的 19 个 fixture 用例循环生成,静态计数把该 `it(` 计为 1):stores(**storeBridge 注册/降级/owner 诊断 14 例**)、**弹窗注册表单点派生三方一致(flag 无重复/label/组件已定义/MODAL_FLAGS 对应;registry ↔ uiPrefs 双向;App.vue v-for 派生且无硬编码残留,共 9 例)**、api client(含 ApiError 错误码分类)、组件与 composables、CSS 清洗(含注释处理)与沙箱回归;类型门禁 `npm run typecheck -w web`(vue-tsc,**硬门禁**,存量 168 已于 2026-09-08 清偿归零,清偿记录见 docs/优化实施方案-2026-09.md 附录 D);类型逃逸 ratchet `node tools/check-frontend-lint.mjs`(as never / as unknown as / 非空断言 / any,**只降不升**,基线见脚本内 BASELINE;2026-09-13 批次 5.2 后 as unknown as 71→70)
- 新增接口建议同步补集成测试;测试环境变量 `CONNECTOR=mock` 强制隔离

---

## 12. Roadmap(来自 README,未实现)

- Oobabooga / KoboldAI 连接器适配(世界书按 key 注入、Tauri 桌面化已完成)
- 工具执行沙箱隔离(角色卡脚本已有 iframe 沙箱,工具侧未做)
- 知识库向量检索工具
- (2026-08 已完成项移出:智能上下文压缩、自定义工具注册、缓存感知压缩管线、跨会话记忆蒸馏、技能渐进披露与子代理调度守卫——见第 14 章)
- (2026-09 已完成项移出:LLM 原生 function calling 全链路——下发 `tools`/`tool_choice` 并按 index 聚合 `delta.tool_calls`,见 `connectors/openai_compatible/mod.rs`)

---

## 13. 相关文件对照

| 关注点 | 文件 |
|---|---|
| 启动 | `start.ps1`(统一启动器;Rust 启动器工程见 `launcher/`) |
| 构建 | `build.ps1` |
| 配置示例 | `.env.example` |
| API 文档 | `API.md`(与代码基本一致;`/api/settings/info` 实际含 `models` 字段) |
| 用户指南 | `README.md` |
| 原 Node 后端(参照) | 已归档删除(历史版本存于代码历史,不在仓库内) |

---

## 14. 新模块速览(2026-08 补记)

> 以下模块晚于本文档上次整理(2026-08-12)落地,此处补记维护入口;详细设计见 `docs/`。

### 万花筒契约 DSL(contracts)

- 位置:`server-rs/src/contracts/`(op / field / due_fields / observe / changelog / invariant / multi_step / render / state / validation / registry / extract / meta)。
- 职责:变量系统唯一事实源——字段定义、更新策略、护栏、不变量、置信度门控、熔断指纹;HTTP 出口 `/api/variable/update|state|changelog`。
- 配套服务:`services/kaleido_state_service.rs`(注意其中 FNV-64 为 SHA-256 占位,尚未兑现)、`services/variable_apply.rs`(统一写入出口)。
- 前端:`components/ContractsModal.vue` + `contracts/contractDiff.ts`。
- 预留未实现:M10+(achievements/ejs/runBoundary 等 Option 字段)、M13/M15/M16(plot/dice/memory 写者),见 `contracts/mod.rs`。

### 任务工作台(task)

- 位置:`services/task_service/`(目录模块:mod/db/cancel/events/executor/parse/prompt,legacy 三段式 planning → running(逐步派子智能体)→ done)+ `services/task_engine/`(六模式底座:solo/multi/plan/team/custom,详见 [docs/任务引擎六模式.md](docs/任务引擎六模式.md))+ `api/` 任务路由 + `web/src/components/TaskBoard.vue` + `api/tasks.ts`。

### 上下文压缩(compaction)

- 位置:`agents/engine/compaction.rs`(可逆投影 + LLM 摘要;原文消息永不删除,摘要存 `session_compactions` 表,删摘要行即恢复完整历史)。
- 触发:manual(`/api/chat/compact`)或 auto(历史 token 超阈值);设置项 `compaction_mode` / `compaction_threshold`。
- 2026-08 起配合「缓存感知压缩管线」升级(usage 缓存落库、四级水位、摘要槽增量式),设计见 `docs/archive/learn-harness-2026-08.md`。

### 缓存感知压缩管线(2026-08 新增)

- 位置:`services/cache_diagnostics.rs`(命中率/费用/水位汇总)+ `api/diagnostics.rs`(`GET /api/diagnostics/cache`)+ `connectors/openai_compatible/`(目录模块:mod/retry/sse_parser/tests;usage 5 元组解析,DeepSeek `prompt_cache_hit_tokens` 优先、OpenAI `cached_tokens` 回退)。
- 落库:`llm_requests` 表新增 usage 列(幂等迁移 `ensure_llm_requests_usage_columns`);轻量 usage 行恒落库,与请求快照开关解耦。
- 前端:`components/CacheHealthPanel.vue` + `cacheHealth.ts`(「优化」弹窗内,缓存健康面板)。
- 压缩升级:摘要槽独立(system → 摘要槽 → 记忆槽 → 历史,`messages/inject.rs` 的 `insert_summary_slot` / `insert_memory_slot`、`messages/trim.rs` 的 `protected_head_len`);摘要改追加式增量(旧段字节冻结);LLM 摘要前先 snip 超长陈旧工具结果(`compaction.rs` 的 `snip_tuples` / `should_snip`,错误特征保留、尾部 2 条原文保留);`compaction_keep_recent`(默认 4)与 `compaction_snip_bytes`(默认 8192)可配置。

### 跨会话记忆蒸馏(2026-08 新增)

- 位置:`services/memory_service.rs`(distill_session 以闭包注入 LLM,mock 可测)+ `api/memory.rs`(distill/search/prune/list/create/update/delete 七端点)+ `tools/memory.rs`(agent 主动写记忆落同表)。
- 表:`memory_entries`(character_id 维度、kind CHECK distilled|tool|manual、usage_count、last_usage、selected、pinned、索引)。
- 注入:精选排序 usage_count DESC → last_usage DESC → id DESC,注入摘要槽之后的记忆槽;使用后在 step_loop 成功路径 touch。
- 前端:`components/MemoryPanel.vue` + `memoryPanel.ts` + `composables/useMemoryPanel.ts`。

### 技能渐进披露与子代理守卫(2026-08 新增)

- 位置:`services/skill_service.rs`(manifest 固定格式清单,按 name 字节序稳定)+ skills 表新增 `allowed_tools` / `run_as_subagent` / `model` 三列。
- 预载仅 name+description(设置 `skill_progressive_disclosure`,默认开;旧技能正文从未预载,关闭即回退零注入)。
- 子代理:`tools/agent_tools_agent.rs` 深度守卫(`subagent_max_depth` 默认 2)、并发守卫(`subagent_max_concurrency` 默认 6)、结果截断(`subagent_result_max_chars` 默认 2000,截断附尾注)。
- 工具治理:`tools/registry.rs` 错误文案带「下一步怎么做」指引;定义顺序按 name 稳定(有跨构建序列化一致性测试,保前缀缓存)。

### 可观测性与错误面收口(2026-09-15 新增)

- 请求关联 ID:`server-rs/src/api/request_id.rs`——读/生成 `X-Request-Id`、回写响应头、
  建 `http_request` span、请求结束打 1 行 INFO 访问日志(`method`/`path`/`status`/`duration_ms`)。
  **注册在 `build_router` 链尾(CORS 之后)成为最外层**,否则 `security::guard` 的 401/403
  早退响应拿不到 requestId(回归护栏:`tests/security.rs::early_return_responses_carry_request_id`)。
- span 穿透:`utils/logging.rs` 的 `RequestIdLayer`(`on_new_span` 存 span extension)+
  `PinoFormat` 经 `event_scope()` 合并。**为何必须 extension**:`FormattedFields` 是格式化后的
  字符串,拿不到结构化值。挂上后请求生命周期内所有日志(含 service 层、spawn 出去的引擎/任务)
  自动带 `requestId`,无需逐处传参。改 span 字段名需两处同步(`api/request_id.rs` 与
  `utils/logging.rs` 的 `REQUEST_ID_SPAN_FIELD`)。
- JSON 体提取器:`api/json_body.rs` 的 `JsonBody<T>`(把 `JsonRejection` 收口为
  `{error, code: VALIDATION}` + 400)。**新 handler 应优先用它**,规则 K 会拦住直接写
  `Json(x): Json<T>` 的新增点(基线见 `tools/check-arch.mjs`,只降不升)。413 是已登记的例外。
- 错误分类(L1):`models/llm_error.rs`——`LlmErrorKind`(`Timeout`/`RateLimited`/`AuthFailed`/
  `Upstream`/`Generation`)+ 纯映射(`from_http_status` / `from_transport`),`code()` 返回值即
  SSE `Error.code` 线格式(**五个值冻结,改动需两端同步 + `check-contract`**)。
  `connectors/openai_compatible` 是**唯一分类边界**(生产者侧);引擎侧
  `agents/engine/types.rs` 的 `EngineError` 消费它。**禁止**回退到「扫错误文案子串」判分类。
- 前端响应形状闸门:`web/src/api/shape.ts` 的形状校验原语(`request<T>()` 后直接解构的封装配用);
  仅做最小存在性校验,避免把「后端加字段」误判为故障。同族先例:`api/diagnostics.ts` 的
  `assertDiagnostics`。
- DB schema 版本:`models/db/schema.rs` 的 `SCHEMA_VERSION`(当前 `1`)+ `Db::open` 的
  `user_version` 读写与「库比代码新则拒绝启动」。**升级一处 schema 必须同步**:
  ① `schema.rs` 的 `CREATE_TABLES`;② `migration/ddl.rs` 的对应 `ensure_*`(两处文本须在
  `normalize_sql` 后一致);③ 若属「表/列结构变化」,提升 `SCHEMA_VERSION`;
  ④ `tests/schema_migration_meta.rs` 的 7 条验收保持通过。
