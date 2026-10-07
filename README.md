<div align="center">

# KedaiAgent

**一款软件，一键出发。**

我们努力制造一个让模型更通用、更好用的 harness。

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Windows%20|%20Android-lightgrey)
![Backend](https://img.shields.io/badge/backend-Rust-orange)
![Frontend](https://img.shields.io/badge/frontend-Vue%203-42b883)

</div>

---

## 声明

- 本软件遵循 **MIT 协议** —— 全文见 [LICENSE](LICENSE)。
- 本应用**免费分发**，**不存在任何针对软件本体的收费项目**。**若你是花钱买的，那说明你上当受骗了。**
- 除用户自行配置的远程 LLM 后端外，**数据不上传任何第三方**；聊天记录、角色卡、API Key 全部只存在你自己的机器上。

## 版本后缀说明

下载时请认准版本后缀，它标明这个版本被验证到什么程度：

| 后缀 | 含义 |
|---|---|
| `Alpha` | **测试版** —— 包含最新最前沿的更新内容，**可能存在极其恶性的 BUG** |
| `Beta` | **预览版** —— 包含已经过证实的、实用性强的功能更新和合理的代码修复 |
| `A` / `B` / `C` 等大写字母 | **修复版** —— 仅在包含恶性 BUG 的稳定版的修复更新版本中使用 |
| 无后缀 | **稳定版** —— 最适合普通用户使用 |

> 例：`0.4.0-alpha` 是测试版；`0.4.0` 是稳定版；`0.4.0-A` 是对 `0.4.0` 的修复版。
> 版本号由 `tools/bump-version.ps1` 统一维护，全仓库 8 处声明 + 1 处兜底共 9 个文件同批更新。

---

## 这是什么

KedaiAgent 是一个**本地运行的通用 LLM 智能体 harness**——把大模型从「聊天框」变成能干活的智能体运行框架。它内置 **Agent 引擎**（计划 → 执行 → 反思）、完整的**工具与授权层**、**上下文工程**（缓存感知压缩、跨会话记忆），并原生兼容 **SillyTavern 生态**（角色卡 / 世界书 / 变量与输出协议）。

**双顶层模式**是它的骨架：

| 模式 | 面向 | 做什么 |
|---|---|---|
| 🎭 **角色扮演（文学创作）** | 创作与沉浸式交互 | 加载角色卡与世界书,按你设定的文风与规则持续创作长篇文本 |
| 🗂️ **任务工作台** | 让模型干活 | 下发一个目标 → 自动拆解计划 → 派子智能体执行 → 汇报结果 |

两个模式**平级**，共享同一个 Agent 引擎、连接器、工具与授权层。垂直能力（编码、文学等）以**官方能力包**形态交付——**默认关、显式启用**，不侵入内核架构。

**核心特性**

- 🏛️ **通用 harness 骨架**
  - **双顶层模式**：角色扮演 / 文学创作会话 ↔ 任务工作台，共享同一 Agent 引擎、连接器、工具与授权层
  - **官方能力包**：垂直能力以可选包形态交付（默认关、显式启用；首个为编码能力包），不新建插件机制、不侵入内核架构
  - **可扩展面**：技能库 / MCP 客户端 / 自定义工具插件 / 自定义流程（线性与二维节点图）
- 🎭 **SillyTavern 兼容**（落在解析层）
  - 角色卡：支持 V2/V3 规范（`.png` tEXt 内嵌 或 独立 `.json`），未知字段无损保留
  - 世界书（World Info）：支持独立世界书 JSON（ST 导出格式）与角色卡内嵌 `character_book`，按关键词**或正则（`regex`+`use_regex`）**命中注入
  - 正则脚本（`regex_scripts`）：AI 输出中的占位符 / 变量标记可替换为显示 HTML（如状态栏卡片），前端可开关
  - 对话：消息数组与 SillyTavern 完全一致，互相导入 / 导出无障碍
  - 后端：OpenAI 兼容接口（`/v1/chat/completions` 流式，兼容 Oobabooga --api / vLLM / LM Studio / Ollama 等）
- 🧩 **酒馆助手插件兼容**（SillyTavern-Assistant）
  - **EJS 模板渲染**：世界书条目中的 `<% %>` / `<%= %>` / `<%_ _%>` 标签按当前变量状态渲染（内建 `getvar/setvar/addvar`、`Math`、数组 / 字符串方法、if/for/箭头函数等 JS 子集），「分阶段人设」类条目开箱即用
  - **变量系统**：会话级 `stat_data` 变量树（SQLite 持久化），`[InitVar]` 条目初始化，支持 YAML / JSON / `_.set()` 三种初始格式
  - **输出协议**：模型回复中的 `<UpdateVariable><JSONPatch>` 块自动剥离并应用（`replace/delta/insert/remove/move`），同时兼容 MagVarUpdate 的 `_.set(...)` 格式；更新后的变量树经 SSE `vars` 事件推送前端，消息快照随 `extra.mvu` 落库
  - **状态注入**：`{{format_message_variable::path}}` 宏与 `<StatusPlaceHolderImpl/>` 占位符展开为当前变量状态；注入位置可在设置中切换（默认 `system`，可选「最新用户消息尾部」——变量更新不再使整个 system 前缀缓存失效，DeepSeek / Anthropic / OpenAI 等提示词缓存命中率显著提升）
  - **内嵌插件识别**：打开「插件」窗口可查看当前角色卡内嵌插件（自动检测，含特性明细：初始变量 / EJS 模板 / 变量系统 / 状态注入 / 输出协议），内置实现无需安装
- 🤖 **Agent 引擎**
  - 状态机驱动：`planning → executing ⇄ tool_call → reflecting → finished`，全程可观测、可中断
  - 四种模式：`fast`（单步直接生成）、`deep`（计划 → 执行 → 反思，质量更高）、`agent`（工具自循环）、`custom`（自定义流程）
  - 推理链通过 SSE 流式推送到前端，实时展示思考过程
- 🗂️ **任务工作台**
  - 六种运行模式（`legacy` 三段式 / `solo` / `multi` / `plan` / `team` / `custom`），状态落 SQLite 六表，全程可中断、可重跑
  - 进度经 **SSE 实时推送**（`GET /api/tasks/events`），前端事件驱动刷新、无轮询；断线指数退避重连 + 低频兜底
  - 提示词与角色扮演模式**类型级隔离、按模式独立存储互不影响**，外部文本统一 `<UNTRUSTED_PROMPT_SOURCE>` 边界包裹
- 🧠 **上下文工程**（提示词缓存友好）
  - **缓存感知压缩**：每轮 LLM usage 落库，`GET /api/diagnostics/cache` 报告命中率、费用估算与四级水位；前端「优化」弹窗内置缓存健康面板
  - **前缀分层**：消息组装固定为「system（静态）→ 摘要槽 → 记忆槽 → 尾部历史（只追加）」，同会话两次构建公共前缀逐字节一致（有回归测试护航）
  - **阶梯压缩**：LLM 摘要前先做零成本 snip（陈旧超长工具结果压占位符、错误特征保留、尾部原文保留）
  - **跨会话记忆蒸馏**：会话历史蒸馏为结构化记忆条目，按使用次数与最近使用衰减精选注入
- 🔧 **工具与授权**
  - 标准化 Tool 接口（OpenAI Function Calling 格式），内置 `calculator`（白名单解析，不使用 eval）、`memory_read/write` 等
  - **技能渐进披露**：技能清单仅预载 `name + description`，正文经 `read(type=skill)` 按需加载
  - **三档授权**：严格 / 宽松 / 放行，按「操作类型 × 路径区域」判定；授权管理面板可查看并撤销已授权限
  - **子代理调度守卫**：递归深度与全局并发可配置，子代理结果超长自动截断为摘要回传，防上下文爆炸
  - **自定义工具插件**：`<数据目录>/plugins/tools/*.json` 白名单脚本工具（不使用 eval），模板见 [`examples/`](examples/)
- ⚡ **流式体验**：token 逐字渲染 + Agent 步骤事件，首 Token 低延迟
- 🔐 **隐私优先**：数据仅存本地（SQLite + 文件），API Key 只存服务端；Windows 用 DPAPI 加密，Android 用 Keystore，**绝不明文回传前端**

## 技术栈

| 层 | 选型 |
|---|---|
| 桌面壳 | **Tauri 2**（窗口加载本地服务，NSIS 安装包，`src-tauri/`） |
| 前端 | Vue 3 + Vite + Tailwind CSS v4 + Pinia（淡粉画布 × 至上主义 / 构成主义设计系统） |
| 后端 | **Rust + axum + tokio** |
| 数据库 | SQLite（rusqlite，bundled 零原生依赖） |
| 流式 | SSE（Server-Sent Events） |
| Token 计数 | tiktoken-rs（按模型自动选分词器） |
| 测试 | cargo test（单测 + API 集成）+ Vitest（前端单测） |

> 原 Node.js + TypeScript + Fastify 后端已重写为 Rust（见 `server-rs/`）；原 C# 启动器被 Tauri 桌面壳取代。历史代码不随仓库归档，需溯源请查 git 历史。

## 快速启动

> 维护指南见 [MAINTENANCE.md](MAINTENANCE.md)（架构、构建、API 契约、数据库与踩坑记录）。

要求：

- 前端：Node.js ≥ 18（npm）
- 后端：**Rust 工具链**（`cargo`、`rustup`）
  - 未安装：`winget install Rustlang.Rustup`

### 开发模式（前后端分离）

```powershell
# 1. 按 package-lock.json 确定性安装前端依赖
npm ci

# 2. 后端编译(debug)
cd server-rs
cargo build

# 3. 两个终端分别启动
#    终端 A(后端):cargo run -p kedai-server   # 监听 127.0.0.1:3001
#    终端 B(前端):npm run dev -w web          # 监听 http://localhost:5173
```

> 开发模式无需先构建前端：Rust 服务默认使用编译期内嵌的 `web/dist`；仅显式设置环境变量 `KEDAI_WEB_DIST` 指向磁盘目录时才用磁盘版覆盖。

### Windows 便携版（正式交付）

```powershell
# 一键双端同步构建:测试版 kedai-server.exe + 便携版 Kedai.exe(发布/交付用这条)
.\build.ps1            # 等价 npm run build:all / build:rs
# 正式入口(项目内双击):Kedai.lnk → 项目根 Kedai.exe 图形启动器(自带过期/漂移检测)
# 交付产物入口:dist\Kedai-portable\Kedai.exe
```

把整个 `dist\Kedai-portable\` 目录复制给用户即可；运行时不需要 Node.js、Rust 或项目源码。系统需要 Microsoft Edge WebView2 Runtime（Windows 10/11 通常已内置）。

> ⚠️ Windows 产物**未做 Authenticode 代码签名**，从网络下载分发后首次运行可能触发 SmartScreen「Windows 已保护你的电脑」：点「更多信息」→「仍要运行」即可；操作步骤与产物来路自查（构建指纹 / 哈希 / APK 签名）见 [docs/契约-协议与配置.md](docs/契约-协议与配置.md)。

桌面壳与 Axum 后端运行在同一进程。主窗口初始隐藏，后端 `/api/health` 就绪后才导航并显示，避免启动阶段白屏；关闭窗口即退出，不残留后端进程。数据与日志统一保存到：

- 数据：`%APPDATA%\com.kedai.app\data`
- 日志：`%APPDATA%\com.kedai.app\logs`

若从项目目录运行桌面产物，首次启动会在目标数据目录尚未使用时，自动把项目 `data` 复制过去。迁移幂等且保守：不删除源数据、不覆盖已有桌面数据、跳过 SQLite `-wal`/`-shm`，并修正数据库中的旧头像绝对路径。便携目录本身不携带用户数据。

仍需安装包时可执行 `npm run build:desktop`；NSIS 不是本项目的首选交付形式。

### 浏览器模式（仅开发 / 调试）

```powershell
# 快速迭代只构建测试版(不刷新便携版,会提示未同步):
.\build.ps1 -TestOnly
# 启动服务并自动打开浏览器
.\start.ps1              # 始终使用浏览器开发/调试模式
.\start.ps1 -NoBrowser   # 加 -NoBrowser 不自动开浏览器
```

> 也可直接双击 `server-rs\target\release\kedai-server.exe` 启动；服务已在运行时再启动会自检并复用，不重复开端口。
> 测试版与便携版各带构建指纹（`<exe>.build.json` + `/api/health` 的 build_id），设置中心底部可见；两端指纹一致即同步，不一致时启动器会自动双端重建。

### 配置（可选）

复制 `.env.example` 为 `.env` 并填写 `OPENAI_API_KEY` 与 `OPENAI_BASE_URL`。
无 Key 也能跑：将 `CONNECTOR` 设为 `mock` 使用演示模式（未配置 Key 时自动进入 mock）。

> 💡 也可直接在应用内「设置」中编辑 API 地址 / Key / 模型，以及温度、Top-P、最大生成长度、最大上下文窗口；
> 保存后写入 `<数据目录>/settings.json` 并立即生效（优先级高于 `.env`，Key 仅存本地服务端、不回显明文；数据目录解析规则见「目录结构」一节）。Windows 使用当前用户 DPAPI 加密。非 Windows 在没有系统凭据后端时默认拒绝持久化非空 Key；只有明确接受明文风险并设置 `KEDAI_ALLOW_INSECURE_PLAINTEXT_SECRETS=1` 才允许写入 `plain:v1:` 值，更推荐通过环境变量提供 Key。

### 试跑演示

1. 打开应用，点左侧 📤 上传角色卡（`.png` 或 `.json`）
2. 选中角色，输入消息发送（Fast 模式）
3. 切到 **Deep** 模式，输入「帮我算 12*34」→ 观察底部 Agent 抽屉的工具调用与推理链
4. 生成中点「■ 停止」可中断；抽屉可手动收起 / 展开

## 目录结构

```
kedai/
├── src-tauri/              # 桌面壳(Tauri 2):窗口 + NSIS 安装包 + 进程内复用 Rust 服务
├── server-rs/              # 后端(Rust + axum)
│   ├── src/
│   │   ├── api/            # RESTful + SSE 路由
│   │   ├── agents/         # 状态机 / 规划器 / 执行器 / 反思器 / engine/(目录模块)
│   │   ├── connectors/     # 后端适配器(openai-compatible / mock)
│   │   ├── tools/          # 工具系统(registry / calculator / memory / agent_tools)
│   │   ├── models/         # 类型 + SQLite 表结构
│   │   ├── parsing/        # 角色卡 V2 解析 + assistant/ejs/(mvu 变量渲染)
│   │   ├── services/       # 角色/会话/Agent会话/Token/任务引擎 等业务服务
│   │   └── utils/          # 结构化 JSON 日志(按天归档)
│   └── tests/              # API 集成测试
├── web/                    # 前端(Vue 3 + Pinia + Tailwind v4)
│   └── src/
│       ├── api/            # REST + SSE 流式客户端(按域拆分)
│       ├── stores/         # Pinia 子 store,store.ts 门面聚合
│       ├── mvu/            # 变量系统 + mini-jquery 沙箱
│       └── components/     # Sidebar / ChatWindow / TaskBoard / AgentPanel + 弹窗
├── docs/                   # 协议锁定文档(活文档)与调研报告,索引见 docs/README.md
├── examples/               # Skill 与工具插件示范模板
├── launcher/               # 图形启动器源码(双击正式入口,产物为项目根 Kedai.exe)
├── Kedai.exe / Kedai.lnk   # 图形启动器与快捷方式(过期/漂移自动询问重建,由 build.ps1 维护)
├── dist/Kedai-portable/    # 正式 Windows 便携目录(build.ps1 默认同步产出)
├── start.ps1               # 启动脚本:默认浏览器测试版,-Portable 启动便携版
├── build.ps1               # 一键构建:默认双端同步(前端 + Rust release + 便携版)
├── LICENSE                 # MIT 许可
└── logs/                   # 运行日志(按天归档,自动清理 3 天前)
```

> **数据目录**（不进仓库）：优先级 `DATA_DIR` 环境变量 > `%APPDATA%\com.kedai.app\data\`（已含用户数据时，与桌面版同目录）> 项目根 `data\`。内容：SQLite（kedai.db）+ 角色卡原图 + avatars + JSON sidecar。下文凡写 `<数据目录>` 均指此。

## 日志与维护

- 运行日志双写：控制台 + `logs/kedai-YYYY-MM-DD.log`（按天归档；桌面应用场景在 `%APPDATA%\com.kedai.app\logs\`）。
- 自动清理：启动时删除 3 天前的过期日志。
- 数据库：`<数据目录>/kedai.db`（WAL 模式；桌面版与已有用户数据的场景固定在 `%APPDATA%\com.kedai.app\data\`）。

## API 概览

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | 健康检查 |
| POST | `/api/chat/send` | 发送消息 → Agent 全流程 → **SSE 流** |
| POST | `/api/chat/stop` | 中断生成 |
| GET | `/api/chat/history` | 会话消息历史 |
| GET/POST/DELETE | `/api/chat/sessions` | 会话管理 |
| PUT/DELETE | `/api/chat/messages/:id` | 编辑 / 删除消息 |
| POST | `/api/chat/clear` | 清空会话消息 |
| GET | `/api/characters` | 角色卡列表 |
| POST | `/api/characters/upload` | 上传角色卡（.png/.json，V2 解析） |
| GET/PUT/DELETE | `/api/characters/:id` | 角色卡详情 / 更新 / 删除 |
| POST | `/api/settings/connect` | 测试后端连接 |
| GET | `/api/settings/models` | 可用模型列表 |
| GET/PUT | `/api/settings/model` | 获取 / 切换当前模型 |
| POST | `/api/token/count` | 消息数组 Token 计数 |
| GET / POST | `/api/export/chat`、`/api/import/chat` | 导出 / 导入（SillyTavern 兼容） |
| GET/POST | `/api/world-books` | 世界书列表 / 上传 |
| GET | `/api/world-books/{id}/entries` | 世界书条目预览 |
| POST | `/api/agent/plan` | 预览行动计划（不执行） |
| POST | `/api/agent/interrupt` | 中断 Agent |
| GET/POST | `/api/tasks` | 任务列表 / 新建任务 |
| POST | `/api/tasks/{id}/run`\|`stop`\|`approve`\|`followup` | 运行 / 停止 / 批准 / 追问 |
| GET | `/api/tasks/events` | **任务事件 SSE 流**（取代轮询） |
| GET/POST/PATCH/DELETE | `/api/memory*` | 记忆库列表 / 蒸馏 / 检索 / 精简 / 新增 / 编辑 / 删除 |
| GET/POST/DELETE | `/api/plugins/tools*` | 自定义工具插件管理 |
| GET/POST/PUT/DELETE | `/api/skills*` | 技能库管理 |

> 完整端点清单（含请求体字段、错误码与线格式约定）以 [docs/契约.md](docs/契约.md) 为准——它是端点清单的权威来源，并有门禁断言与代码注册双向比对。

### SSE 事件格式

```text
data: {"type":"step","step":"计划中…","detail":"快速模式:直接生成回复"}

data: {"type":"token","text":"（"}

data: {"type":"tool_call","name":"calculator","input":{"expression":"12*34"}}

data: {"type":"tool_result","name":"calculator","output":{"result":408}}

data: {"type":"finish","usage":{"prompt_tokens":166,"completion_tokens":35,"total_tokens":201,"context_tokens":490},"content":"…"}
```

事件类型：`token`（文本片段）、`step`（Agent 步骤）、`tool_call` / `tool_result`（工具调用）、`tool_authorization_required`（工具未获授权，前端提供授权入口）、`vars`（变量树同步）、`interrupted`（中断）、`error`（错误终态，含 code/message/retryable）、`finish`（结束，含 usage）、`task`（任务模式事件）。

## 世界书（World Info）

- **来源**：两种——独立上传（SillyTavern 导出格式，顶层 `entries` 对象 / 数组）与角色卡内嵌 `character_book.entries`（兼容 V2 顶层与 V3 `data.character_book` 两种布局）。
- **注入规则**：常驻条目（`constant=true`，启用的）始终注入；非常驻条目按 `keys` 对最近 `depth` 条用户消息做大小写不敏感子串匹配（`depth` 缺省 4，0 = 全部历史），或按条目 `regex`+`use_regex` 做正则匹配（优先于 keys），命中才注入。
- **自动转换**：上传时自动规范化酒馆变体——关键词兼容 `keys/key/keywords/keyword` 字段名与逗号分隔字符串、`constant` 兼容字符串 / 数字形态并支持缺失时自动判定、`role` 缺失即「自动」分配。上传响应附带转换统计 `conversion`。
- **管理**：侧边栏「世界书」按钮打开管理界面——上传、查看条目（含正则标记）、绑定角色（空 = 全局）、启用 / 停用、删除。
- **持久化**：`<数据目录>/kedai.db` 的 `world_books` 表，原始 JSON 无损保留（`data_raw`）。

## 提示词注入

- **简单模式**：字数 / 转述 / 对话 / 视角 四项，启用项按可调顺序合成一条注入提示词拼入系统提示词末尾，对所有会话生效（支持酒馆宏）。
- **禁词库**（简单模式）：自由编辑「禁词 → 替换词」映射表。输出含禁词时，**所有模式**先注入自省提示词要求换用更得体表达；deep/agent/custom 模式另由引擎在生成收尾调用 `censor_text` 工具做同义替换兜底。
- **复杂模式（楼层）**：仿 SillyTavern Prompt Manager 的楼层系统——角色 / 位置 / 深度 / 拖拽排序，可导入酒馆预设（`examples/presets/` 示范）。
- **管理**：设置 → 提示词注入，或「提示词管理」弹窗；配置持久化到 `<数据目录>/prompt_floors.json`。

## 消息 HTML 渲染

- 角色卡 `extensions.regex_scripts` 中的脚本，可用于把 AI 输出的占位符 / 变量标记替换为显示 HTML（如「状态栏」卡片）。
- **安全性**：先在原文上匹配脚本、再对 AI 输出整体做 HTML 转义，最后仅把脚本命中的片段还原为脚本自带内容，避免注入；`<script>` 一律剔除。脚本受控执行：提取 `<script>` 源码后由沙箱 iframe 执行（隔离宿主 window/document，遮蔽网络与存储），并受角色卡 JS 授权门禁约束。
- **开关**：聊天窗口顶栏「HTML」开关按钮，或「设置 → 界面 → 消息 HTML 渲染」。默认关闭，开启后仅对 AI 回复生效。

## 示例模板（Skill 与工具插件）

`examples/` 提供两套扩展体系的示范模板与使用说明：

- **示范 Skill**：`examples/skills/写作风格指南.json` —— agent 可 `read(type=skill)` 加载的写作规范。
- **示范工具插件**：`examples/plugins/tools/score_eval.json`（评分判定）与 `clamp.json`（数值限幅）。
- **示范预设**：`examples/presets/可待精华增强楼层.json` —— 提炼自酒馆「可待」预设的可选增强楼层，在「设置 → 提示词注入 → 导入酒馆预设」中一键导入、按需开启。
- **说明文档**：[examples/README.md](examples/README.md) —— 导入 / 启用命令、脚本语法边界与常见陷阱。

> 工具插件脚本为白名单求值器（**不使用 eval**），语法能力有限，编写前请阅读 `examples/README.md` 的「脚本语法边界」。

## 测试与门禁

```bash
# 后端:单元测试 + API 集成测试
cargo test --manifest-path server-rs/Cargo.toml -j 8

# 前端:Vitest 单测 / 类型检查
npm test -w web
npm run typecheck -w web

# 仓库根:一键全量检查(15 段:fmt/clippy/cargo test/audit/docs/lock-sync/
#          contract/arch/count/doc-claims/type-ratchet/eslint/vue-tsc/vitest/bundle)
npm run check
```

> 测试数量以 `node tools/count-tests.mjs` 现取为准，不在此复写——它由门禁 `--check` 与 [MAINTENANCE.md](MAINTENANCE.md) 双向钉死。

> **验收基准（2026-10-06 起）**：所有批次以**真实模型实测**为唯一验收口径（commandcode 网关 +
> `deepseek/deepseek-v4.1-flash`）；上方 mock 单测 / 集成测试定位为回归网。凭据只存本机仓外，
> 不入库（本仓为公开仓）——口径正文见 [AGENTS.md](AGENTS.md) 的「真实模型实测」节。

## Roadmap

- [x] 世界书（World Info）按 key 注入
- [x] Tauri 桌面化（安装包 + 窗口 + 品牌图标）
- [ ] Oobabooga / KoboldAI 连接器适配
- [x] 智能上下文压缩（可逆投影 + LLM 摘要，manual/auto 模式）
- [x] 缓存感知压缩管线（usage 落库 + 四级水位诊断 + 摘要槽增量化 + snip 零成本裁剪）
- [x] 跨会话记忆蒸馏
- [x] 技能渐进披露与子代理调度守卫
- [x] 任务工作台全面重构（六模式 + 状态枚举化 + 事件 SSE 取代轮询 + 双模式提示词类型级隔离）
- [x] 三档授权模式（严格 / 宽松 / 放行，按「操作类型 × 路径区域」判定）
- [x] LLM 原生 function calling 全链路
- [x] 自定义工具注册（白名单脚本工具）
- [ ] 工具执行沙箱隔离
- [ ] 知识库向量检索工具

## 致谢

- **mvu 变量系统**（`web/src/mvu/` 与后端 `parsing/assistant/`）为 **MagVarUpdate** 的兼容实现：协议约定与全局脚本 API 均源自该扩展。原作者 **[MagicalAstrogy](https://github.com/MagicalAstrogy/MagVarUpdate)**（MIT License）。KedaiAgent 为独立实现，不包含原版代码、不依赖其运行时，仅保持协议与命名兼容。
- 酒馆助手（SillyTavern-Assistant）兼容层的 `<UpdateVariable><JSONPatch>` 输出协议参考 SillyTavern 社区插件生态的公开约定。

## 许可

[MIT](LICENSE) © 2026 Sata1949

本应用**免费分发**，不存在任何针对软件本体的收费项目。若你是花钱买的，那说明你上当受骗了。
