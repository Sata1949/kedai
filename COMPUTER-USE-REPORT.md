# Kedai「computer use / 电脑操作」能力调查报告

> 调查日期：2026-09-26　范围：`D:\kedai` 全仓（`server-rs/`、`web/`、`src-tauri/`、`docs/`、仓库根计划稿）
> 性质：**只读调查**，未修改任何代码或配置。本文为一次性调查产出，不属于产品文档体系（`docs/` 下无对应条目）。
> 证据口径：每条结论附 `文件:行号`；标「**已复核**」的是本次调查亲自读码确认，其余为子代理探查并经抽样验证。

---

## 0. 结论摘要

**一句话：Kedai 当前不具备 computer use 能力——既没有截屏/键鼠/窗口/浏览器自动化原语，也没有 computer use 所依赖的视觉输入通道，官方路线图里也从未出现这个方向。它能"操作电脑"的部分只有 `bash` 命令执行一条路，而且那是命令行级操作，不是 GUI 级操作。**

| 判定维度 | 结论 | 关键证据 |
|---|---|---|
| 原生 GUI 操作工具（截屏/鼠标/键盘/窗口） | **0 个** | `server-rs/src/tools/` + `services/exec/` 全关键词零命中（已复核） |
| 浏览器自动化（Playwright/Selenium/CDP/Puppeteer） | **0** | 无依赖、无代码；`docs/计划.md:806-821` 的 Playwright 是"测自己"的 E2E 待办且未启动 |
| 视觉输入（模型能否看到图像） | **不支持**（伪多模态） | 前端把图片读成 base64 塞进纯文本 content（`web/src/components/ChatInput.vue:80-115`，已复核）；连接器 content 恒为字符串 |
| 命令行执行 | **有，且是唯一执行原语** | `bash`（`server-rs/src/tools/bash.rs:36`）；桌面 `cmd /C`，Android 三档 |
| 文件操作 | **有，两层 jail** | 工作区 `fs_*` 五件 + 角色文件区 `read/write/replace/create` |
| 可扩展接入（MCP） | **有，唯一现实路径** | stdio only、默认关、仅启动时装配（`server-rs/src/mcp/mod.rs:1-8,36`，已复核） |
| 官方规划 | **全库无记载** | `docs/功能.md`/`展望.md`/`计划.md`/`遗留.md` 关键词全零命中；唯一沾边是明确否决 Android 无障碍档 |
| 成熟度评级 | **原生 0/5；叠加 `bash` 间接触达约 2/5** | 无结构化 API、无感知闭环、默认开关全关 |

**给产品决策的直白表述**：如果 computer use 定义为"观察屏幕并操作 UI"，Kedai 现在**不支持**，且**不存在可直接拼装出该能力的现成构件**——缺的不只是一个截图工具，而是**视觉回传通道**和**观察-动作循环**这两块地基。

---

## 1. 调查方法与复现

调查分四路并行（工具系统 / 扩展面 / 执行层与平台桥 / 文档规划），再对承重结论做亲自复核。复现命令：

```bash
# 1. GUI 原语是否存在（应为空）
grep -rn -i -E "screenshot|截屏|mouse_|keyboard|sendkeys|inject_input|window_activate" \
  server-rs/src/tools/ server-rs/src/services/exec/

# 2. 多模态输入是否存在（应为空）
grep -rn -i -E "image_url|multimodal|content_parts|vision_|image_base64" \
  server-rs/src/connectors/ server-rs/src/models/types.rs server-rs/src/api/chat.rs

# 3. 执行总开关默认值
grep -n "exec_enabled\|exec_allow_" server-rs/src/services/settings_service/params.rs

# 4. Tauri 自定义命令（实际为 0，lib.rs:148-160 解释了为何改用事件）
grep -rn "tauri::command" src-tauri/src/

# 5. 命令风险分级归属
grep -n '"powershell"\|"input"\|"screencap"\|"am"\|"pm"' server-rs/src/tools/command_risk.rs
```

---

## 2. 现状：工具面就是能力面

`docs/功能.md:843` 自己写明了设计口径：**「工具系统是「模型能做什么」的唯一出口，授权层是「允许它做什么」的唯一闸门」**。因此判断 computer use 能力，本质就是查工具面。

### 2.1 内置工具全景（26 个，已复核）

注册入口 `server-rs/src/tools/mod.rs:47-94`（24 个）+ `multistep.rs` 的 2 个元工具。

| 类别 | 工具 | 与"操作电脑"的关系 |
|---|---|---|
| 计算/文本 | `calculator`、`censor_text`、`revise_passage`、`role` | 无关 |
| 记忆/变量 | `memory_read`、`memory_write`、`update_variables`、`get_state`、`apply_patch` | 无关 |
| **命令执行** | **`bash`** | **唯一执行原语**（见 §2.3） |
| 工作区文件 | `fs_read`、`fs_write`、`fs_edit`、`fs_glob`、`fs_grep` | 受 jail 的文件读写 |
| 角色文件区 | `read`、`write`、`replace`、`create` | 聊天路径的文件区读写 |
| 联网检索 | `search` | 一次 GET + 正则解析，非浏览器（见 §2.5） |
| 编排 | `agentgo`、`agentend`、`todo`、`sleep`、`run_flow` | 子代理/流程编排 |
| 产物交付 | `submit` | 仅 Android 沙箱档，写设备下载目录 |
| 动态来源 | `mcp_{server}_{tool}`、插件工具 | 运行期注册，见 §4 |

### 2.2 GUI / 桌面原语：逐项为零（已复核）

| 能力 | 结论 | 验证方式 |
|---|---|---|
| 截屏 / 屏幕读取 | **不存在** | `screenshot`、`截屏`、`screen_capture` 在 `tools/` 与 `services/exec/` 零命中 |
| 鼠标移动/点击 | **不存在** | `mouse_` 零命中；全仓唯一的 `click` 是前端 WebView DOM 沙箱（`web/src/sandbox/dom-rpc.ts`），非 OS 鼠标 |
| 键盘输入注入 | **不存在** | `keyboard`、`sendkeys`、`inject_input` 零命中 |
| 窗口枚举/激活/焦点 | **不存在** | 零命中；Tauri 侧仅授权 `core:window:allow-close` |
| 剪贴板（模型可调） | **不存在** | 前端仅有用户点击触发的复制按钮与沙箱 RPC，非工具 |
| 浏览器自动化 | **不存在** | 无 Playwright/Selenium/Puppeteer/CDP 依赖与代码 |
| Android 无障碍服务 | **不存在且明确否决** | Manifest 无 `BIND_ACCESSIBILITY_SERVICE`；`docs/功能-变更史.md:3954` 列"不做无障碍（Accessibility）执行档" |
| Android 截屏/触摸注入 | **不存在**（仅可经 shell 手写命令） | 无 `MediaProjection`/`input tap`/`injectInputEvent` 调用 |

### 2.3 唯一的执行原语：`bash`

- 工具名常量注册于 `server-rs/src/tools/bash.rs:36`；`action_class.rs` 中**只有 `bash` 归入 `ToolOp::Exec`**（已复核 `action_class.rs:146-176`），即它是系统里唯一的"执行"类工具。
- **桌面**：`tokio::process::Command::new("cmd").arg("/C").arg(command)`（`services/exec/desktop.rs:29-38`），以当前用户权限运行，无提权语义。
- **Android**：三档探测 `ROOT(su) → Shizuku(ADB shell UID 2000) → 沙箱(sh)`，需用户逐档放行（`services/exec/mod.rs`、`ShellExecutorBridge.kt`）。
- **四道闸**（`docs/功能.md:873-884`）：
  1. 总开关 `exec_enabled` —— **默认 false**（`settings_service/params.rs:491`，已复核：连带 `exec_allow_root/shizuku/sandbox` 三项全 false）；
  2. 工具级风险恒 `Dangerous`；
  3. 命令级四级风险 `Safe < Sensitive < Destructive < Admin`，后两级**逐条确认、不受任何豁免**（`permissions.rs:138-158`，已复核：硬门判定位于所有自动放行之前）；
  4. 执行级强制超时（默认 60s、上限 300s），每次尝试（含被拒）落 `exec_audit`。

> **诚实边界**（项目自己的口径）：`command_risk.rs:1-7` 明确自述"本模块只做**语义标注**，**不构成安全边界**——命令字符串可被混淆/拼接/编码绕过，静态匹配天然不完备"；`HARNESS-PLAN.md:198` 亦称"模型一旦有 shell，就能读该进程有权限读的**任意**路径"，cwd jail 拦不住命令内的绝对路径。这是"防题库泄底进程内封不死"的同一根因。

### 2.4 文件面：两层 jail

- **任务模式**：任务级 workspace 绑定（`2026-09-26` 编码通道批次）；未绑定时自动落到任务级 scratch（`<DATA_DIR 同级>/task_scratch/<task_id>`），仍恒有作用域（`docs/功能.md:758-764`）。
- **聊天路径**：`Data_dir` / `data/character_files/{角色}/`，`fs_*` 族被整族剔除（`api/chat.rs:176-180`）。
- **闸门**：`tools/workspace_guard.rs::safe_workspace_path` 逐段拒符号链接与 Windows junction、前缀比对拒越界、拒 UNC、并对 `DATA_DIR` 做双向包含的二次防线。

### 2.5 网络面：`search` 不是浏览器

`search` 只对**配置的搜索端点**发一次 GET（`agent_tools_search.rs:38-46`），带 SSRF 防护（`resolve_public_http_url` 拒私网/localhost/元数据地址）、**禁用重定向**、用正则解析 HTML 抽 `{title,url,snippet}`（`:162-196`）。

**它既能搜也不能"打开网页"**：模型拿到的是搜索结果三元组，无法读取任意 URL 的正文。真正的网页读取只能靠 `bash` + `curl`（需开启 exec，且 `curl` 归 Sensitive）——这是"网页使用"能力的一个实际断层。

---

## 3. 前置能力缺口：为什么"加一个截图工具"也不够

### 3.1 视觉输入是伪多模态（本次调查的关键发现，已复核）

computer use 的闭环是「**看屏幕 → 决策 → 操作**」。Kedai 的前两环都缺，而且缺口位置比预期更靠前：

**（a）前端有选图入口，但图片是当纯文本发出去的。** `web/src/components/ChatInput.vue`：

- `:384` 文件选择器 `accept="image/*,.txt,.md,.json,.js,.ts,.py,.html,.css,.pdf,.doc,.docx"`；
- `:80-91` `readFileContent`：图片走 `readAsDataURL`（base64），其余走 `readAsText`；
- `:99-110` 关键一步——把结果拼成**纯文本**：

```js
if (isImage(f)) parts.push(`[图片: ${f.name}]\n${data}`);   // data 是 data:image/png;base64,... 长串
content = content ? `${content}\n\n${parts.join('\n\n')}` : parts.join('\n\n');
```

**（b）后端没有多模态通道。** 连接器序列化消息时 `content` 恒为 JSON 字符串（`connectors/openai_compatible/mod.rs:486,492,517`，已复核），不存在 OpenAI 风格的 content parts 数组；后端 `data:image` 处理零命中（全仓命中都在前端 CSS 净化器，与图像无关）。

**结论**：图片附件实际上把一坨 base64 文本喂给模型。模型**看不见图像**，只看到一长串字符。所以即便今天给 Agent 加一个"截屏工具"，截出来的图也**没有任何通道能送进模型**——这才是 computer use 的第一块地基缺失。（`HARNESS-PLAN.md:203` 把"图片与二进制读取"列入**明确不做**，说明这是有意为之而非遗漏。）

### 3.2 无观察-动作闭环

- `bash` **不支持后台执行**：执行器 await 完成即返回，`kill_on_drop(true)`，超时强杀（`services/exec/desktop.rs:57`）。工具描述在超时后建议"改用 `agentgo` 后台执行"（`registry.rs:341-344`）。
- 因此无法"启动一个 GUI 程序 → 一边跑一边看"；只能"跑完拿一段文本输出"。
- `sleep` 存在（1~60000ms），但没有"观察"可接——它是给子代理轮询用的，不是给屏幕轮询用的。
- 输出为**纯文本**且按字符截断 32768（`services/exec/mod.rs:83-105`），二进制/图像无通道（`fs_read` 遇 NUL 字节直接判二进制拒绝，`agent_tools_fs.rs:137-148`）。

### 3.3 无元素定位/坐标原语

没有 `uiautomator`/`dumpsys` 封装、没有窗口句柄枚举、没有 DOM 查询能力（`search` 只解析搜索结果页）。要让模型点击某个按钮，它既拿不到元素树，也拿不到坐标——只能靠猜。

---

## 4. 扩展面五条口子的可行性

| 口子 | 真实形态 | 能否注入 computer use 能力 | 证据 |
|---|---|---|---|
| **MCP 客户端** | 已实现 v1：**仅 stdio**，仅 tools，仅启动时装配，单工具超时 120s，默认关 | **能，且是唯一现实路径** | `mcp/mod.rs:1-8,36`（已复核）；工具名 `mcp_{server}_{tool}` 注册 |
| 技能 skill | SQLite 里的纯文本条文 + 渐进披露清单 | **不能**（只影响提示词） | `services/skill_service.rs`；`allowed_tools` 等字段**存而无消费点**（`docs/遗留.md:246-251`） |
| 用户脚本 | QuickJS（rquickjs），宿主 API 仅"变量读写 / 导入 / 一次 LLM 生成 / slash" | **不能**（无网络、无文件、无进程） | `scripts/bridge.rs`、`scripts/runtime.rs:154-213`；生成后才执行，不能驱动工具轮 |
| 插件 | JSON + 自研白名单**表达式求值器**（无 eval、无 IO） | **不能**（连一次 HTTP 都发不出） | `plugins/mod.rs:180-404` |
| 子代理 | `agentgo`，工具名单硬锁 | **不能**（白名单无写/执行类，且不可嵌套） | `tool_sets.rs:24-31`（已复核） |

**MCP 这条路的实际约束（重要）**：

1. **默认关闭**：`mcp_enabled=false`，开启后需**重启**才装配，运行期改设置不回溯重连（`mcp/mod.rs:4-5`）。
2. **仅桌面端**：Android 上 `spawn` 直接返回"不支持"（无 npx/node 运行时；`docs/计划.md:2849` 明确不做 Android 的 Node 运行时）。
3. **只能走聊天模式**：MCP 工具名不命中任何内建分类 → 按"未知工具一律 `Dangerous`"兜底（`permissions.rs:505-506`）→ **任务模式的 `deny_dangerous` 策略会把 MCP 工具整体剔除**（`task_engine/tool_policy.rs:187-225`，含回归测试锁死 `mcp_x_bash` 不外溢）。也就是说：**无人值守的任务模式用不了 MCP 的 computer use 能力，只能在有用户逐次确认的聊天模式用。**
4. **custom 白名单直通车道暂时走不通**：流程白名单必须命中**启动时**的注册表快照，而 MCP 是启动后才注册，保存流程时会报"引用了未注册工具"。
5. **已知缺陷**：服务器死亡后工具不注销、无热重连（`docs/遗留.md:121-127`）；孙进程不保证回收（`docs/遗留.md:58-64`，Windows 侧无 Job Object）。

---

## 5. 安全视角：`bash` 已经是"事实上的电脑操作原语"

这是本报告最需要在决策时被看见的一段：**虽然没有任何 GUI 工具，`bash` 在权限足够时已经能触达键鼠/屏幕级操作**——因为系统里没有对应的风险词条，它们落在"未识别 → Sensitive"档。

### 5.1 桌面（Windows）

`powershell` / `pwsh` 归 **Sensitive**（`command_risk.rs:148-149`，已复核属于 `SENSITIVE_COMMANDS`）。而 Sensitive 命令在 **Bypass 档（任务白名单）可自动放行**（`permissions.rs:195-207`）。PowerShell 能做的事包括截屏、`SendKeys` 键鼠模拟、窗口操作——**即"无专用工具但事实可达"**。

### 5.2 Android

| 命令 | 分级 | 后果 |
|---|---|---|
| `input`（tap/text/swipe） | **未列入任何词表 → 兜底 Sensitive**（`command_risk.rs:344-347`） | 任务模式可放行 → **等价 ADB 级触摸/文本注入** |
| `screencap` | **同上 → Sensitive** | 可拍屏，但没有回传通道（§3.1）→ 拍完看不见 |
| `am` / `pm` / `settings` | **Admin**（`command_risk.rs:76-78`，已复核） | 任何模式都不自动放行；任务模式直接拒绝 → **启动 Activity/改系统设置走不通无人值守** |

**净结论**：Android 上"能点不能看、能拍不能用"，`am start` 这条"打开目标 App"的路被硬门堵死，因此**"截图→识别→点击"的 computer-use 循环在 Kedai 内无法成立**。这与"可开关、可审计、需逐条确认的 shell 执行能力（最高到 ADB shell/root）"是两件事，不要混为一谈。

### 5.3 其他需要留意的面

- **命令分级不是边界**：见 §2.3 引用的自述。混淆/拼接/编码可绕过静态匹配。
- **Tauri 壳**：`#[tauri::command]` **数量为 0**（已复核；`lib.rs:148-160` 解释：remote origin 下自定义命令被 ACL 拒，故改用事件系统）。`capabilities/default.json` 仅四项：`core:default`、`dialog:allow-save`、`fs:allow-write-text-file`（path `**`）、`core:window:allow-close`——**无 shell 插件、无 global-shortcut**。前端无法通过 invoke 间接执行系统命令。
- **一处理论注入面**：`kedai://open-external` 在桌面走 `cmd /C start "" <url>`，校验只要求 `http(s)://` 前缀，未过滤 `&`/`|`/`"` 等 cmd 元字符（`src-tauri/src/native_bridge.rs:39-46,53-62`，已复核）。该事件只由前端页面发出、模型不可直接触发，属"需前端被注入才可利用"的低危面，但若要收口，值得在 URL 转发前做一次 shell 元字符校验。
- **HARNESS-PLAN 记录的既有缺口（P0-4）**：`GET /api/bootstrap` 免鉴权把 bearer token 交给任意本机进程 → 模型有 shell 即可一条 `curl` 把自己升到 bypass（`HARNESS-PLAN.md:26`）。**这意味着"命令分级 + 授权"在本地进程面前并非不可逾越**——评估 computer use 引入风险时必须把这条算进去。

---

## 6. 文档与路线图：全库无 computer use 记载

| 检查项 | 结论 | 证据 |
|---|---|---|
| `computer use`/`桌面自动化`/`GUI 操作`/`浏览器控制`/`鼠标`/`截屏`/`screenshot`/`selenium`/`puppeteer`/`RPA`/`VNC`/`多模态`/`image_url` | **功能、规划、展望、遗留、变更史全部未提及**（零命中） | 逐文件关键词排查 |
| `docs/展望.md` 全部条目（L / D / AND / WF / T / CFG / RD / ARCH / DB / GPU / PB / LH / LD） | **无 computer use 类目** | — |
| `docs/计划.md` 状态汇总 | 已完成/部分完成/已放弃/未启动四类中**均无**此类条目 | `docs/计划.md:3929-3933` |
| Playwright | 是"**测 Kedai 自己**"的 E2E 测试基建计划，状态**未启动**，`playwright.config.ts`/`e2e/` 均不存在 | `docs/计划.md:806-821` |
| CDP | 仅出现在**开发期**调试/量测 Android WebView 的归档记录 | `docs/archive/.../android-port-plan.md` |
| 「截图」 | 仅指开发期**人工/模拟器**截图做视觉复核 | `docs/功能-变更史.md:403,669` |
| GUI-1~4（遗留） | 指 **Kedai 自身窗口**的退出确认/黑窗待人工实测，非"Agent 操作 GUI" | `docs/遗留.md:529-562` |
| **唯一沾边的明确否决** | **不做 Android 无障碍（Accessibility）执行档** | `docs/功能-变更史.md:3954`；归档方案比较中 Kedai 主动去掉 `ACCESSIBILITY` 档，只保留 `ROOT → Shizuku → STANDARD` |
| 其他明文「不做」 | 网页抓取、图片与二进制读取、Android 的 npx/uvx（Node 运行时）、proot/busybox 打包 | `HARNESS-PLAN.md:203`、`docs/计划.md:2849` |
| 产品定位 | 「**文学创作 / 角色扮演交互工具**」，SillyTavern 生态兼容 + Agent 引擎；README 的 Roadmap 无 computer use | `README.md:1-3,279-294` |

**两条活跃工作线也与 computer use 无关**：`HARNESS-PLAN.md`（编码 Harness 适配，提交 1 已完成）与 `TASK-MODE-FIX-PLAN.md`（任务模式实测缺陷 D1~D8，提交 0/1/2 已完成）都在解决"让 Kedai 能当 coding harness 跑沙箱代码题"，方向是**工作区文件读写 + 命令执行**，不是 GUI 操作。

---

## 7. 若要建 computer use：缺口清单与建议顺序

### 7.1 必须补的构件（按依赖顺序，缺一不可）

| # | 构件 | 现状 | 说明 |
|---|---|---|---|
| 1 | **视觉输入通道（content parts）** | 缺失 | 连接器的 `content` 从"字符串"升级为"字符串 \| parts 数组"，并在前端附件路径真正构造 `image_url`。**这是地基**——不做这步，后面全白做。同时要处理模型能力探测（不支持视觉的模型要降级/拒绝，而非静默发 base64 垃圾）。 |
| 2 | **截屏原语** | 缺失 | 桌面：Windows 走 GDI/DXGI，或先落一个最小可用路径；Android：`MediaProjection`（需前台服务+用户授权）而非 `screencap`（沙箱档通常失败）。 |
| 3 | **输入注入原语** | 缺失 | 桌面 `SendInput`；Android `InputManager.injectInputEvent`（需系统权限）或无 ADB 不可行。**这是权限门槛最高的一块**——桌面需无提权可用，Android 基本只能依赖 root/Shizuku 档。 |
| 4 | **观察-动作循环** | 缺失 | 需要"截屏→送模型→操作→再截屏"的闭环。现有 `bash` 不支持后台、无流式感知，需新增会话式的 GUI 会话抽象（或改成"每步一次截屏工具调用"的无状态循环）。 |
| 5 | **元素定位** | 缺失 | 至少要有窗口枚举 + 坐标空间约定；更强则接 UI Automation（桌面的 UIA / Android 的 accessibility tree）。 |
| 6 | **授权与审计扩展** | 部分具备 | 现有 `ToolOp`/`PathZone`/四级命令风险/审计表可复用，但需要为"截屏=读屏幕内容"新增隐私维度（屏幕可能含密码、聊天正文——与本项目"不泄露聊天正文"纪律直接冲突）。 |

### 7.2 两条路线对比

| 路线 | 做法 | 优点 | 代价 |
|---|---|---|---|
| **A. MCP 接入（低投入）** | 让用户自行配置 Playwright MCP / 桌面自动化 MCP server | **零代码**，今天就能试；工具自动按 `Dangerous` 纳管 | 仅桌面、仅聊天模式（任务模式剔除 MCP）、默认关、需重启、每次调用弹授权；受 §4 的 5 条约束限制；Android 完全不可用 |
| **B. 自建原语 + 通道（高投入）** | 按 §7.1 补齐 6 件构件 | 可进任务模式（无人值守）、可跨平台设计、可复用现有授权/审计 | 工作量大（估计跨多个批次）；Android 受系统权限天花板限制；安全与隐私面显著扩大，需先解决 §5.3 的既有逃逸面 |

**建议**：先做**路线 A 验证需求**（成本近零，能立刻回答"用户到底要不要"），把**构件 1（视觉输入通道）**作为独立的、无论做不做 computer use 都值得做的基础设施先行——它同时是"用户能不能贴图问问题"这个独立产品价值点。**构件 2/3 之后再谈**，且在动工前应先处置 §5.2 与 §5.3 的既有风险（`input`/`powershell` 的 Sensitive 档位是否应该提级）。

### 7.3 若真要落地，几条安全建议（不替代威胁建模）

1. **MCP/插件工具默认档位**：当前"未知工具一律 Dangerous"是合理保守默认，引入 computer use 后建议保留（不要为体验把它降级）。
2. **Sensitive 档位复查**：`powershell`（桌面）与 `input`/`screencap`（Android）事实上是键鼠/屏幕级原语，按现有分级在 Bypass 档可自动放行。引入 GUI 能力前，建议把它们提级到"需显式确认"或单列"屏幕/输入类"类别。
3. **屏幕内容的隐私等级**：截屏等于把用户屏幕全文喂给外部模型（可能含聊天正文、密钥、其他应用数据），与 `AGENTS.md` 的"不泄露聊天正文、API Key 或本地真实数据"纪律正面冲突。**必须**有明确的用户手势授权 + 可审计，且默认关闭。
4. **收口 `open-external` 的 cmd 元字符**（§5.3），成本极低。

---

## 8. 判定矩阵（可直接引用的答复）

| 问题 | 答复 |
|---|---|
| Kedai 有 computer use 吗？ | **没有。** 零 GUI 原语、零视觉输入通道、零相关规划。 |
| 能让 Agent 操作电脑吗？ | **只能命令行级**：`bash`（默认关闭，四级风险闸门 + 审计）。GUI 级不行。 |
| 能操作浏览器吗？ | **不能**。只有 `search`（一次 GET + 正则解析），连任意 URL 正文都读不到（除非开 exec 用 `curl`）。 |
| 模型能看到屏幕/图片吗？ | **不能。** 前端图片附件被拼成 base64 纯文本，模型看到的是字符串。这是最靠前的地基缺口。 |
| 有没有现成口子可以快速接上？ | **有且只有 MCP（stdio）**：桌面、聊天模式、默认关、每次调用需授权。任务模式被 `deny_dangerous` 剔除。 |
| 能靠脚本/技能/插件自己长出这个能力吗？ | **不能。** 三条扩展面分别缺 IO、缺执行语义、缺消费点。 |
| 官方打算做吗？ | **无任何记载**；唯一相关表态是否决 Android 无障碍档。 |
| 成熟度打分 | **原生 0/5；叠加 `bash` 间接触达 ≈2/5。** |

---

## 附录 A：证据索引（承重结论 → 文件:行号）

| 结论 | 证据 |
|---|---|
| 26 个内置工具的注册全集 | `server-rs/src/tools/mod.rs:47-94`；`agent_tools.rs::register_agent_tools`；`agent_tools_fs.rs:27-31`；`multistep.rs:52,111` |
| `bash` 是唯一 Exec 类工具 | `tools/action_class.rs:146-176` |
| 快照/键鼠/窗口原语零命中 | `tools/`、`services/exec/` 关键词扫描（附录命令 1） |
| 图片附件退化为 base64 文本 | `web/src/components/ChatInput.vue:80-91,99-110`（尤其 `:104`） |
| 连接器 content 恒为字符串 | `connectors/openai_compatible/mod.rs:486,492,517` |
| 无多模态/图像处理 | 附录命令 2 全零命中 |
| `exec_enabled` 及三档默认全 false | `services/settings_service/params.rs:491-494` |
| Destructive/Admin 硬门位于所有放行之前 | `tools/permissions.rs:138-158` |
| 未知工具（含 MCP）按 Dangerous 兜底 | `tools/permissions.rs:505-506` |
| `powershell`/`pwsh` 归 Sensitive | `tools/command_risk.rs:115,148-149` |
| `am`/`pm`/`settings` 归 Admin | `tools/command_risk.rs:76-78` |
| 风险分级自述"不构成安全边界" | `tools/command_risk.rs:1-7` |
| `search` 仅一次 GET + SSRF 防护 + 禁重定向 | `tools/agent_tools_search.rs:38-46,98` |
| MCP 仅 stdio、仅启动装配、120s 超时 | `mcp/mod.rs:1-8,36` |
| 子代理白名单（无写/执行类） | `tools/tool_sets.rs:24-31` |
| MCP 工具被任务模式剔除 | `services/task_engine/tool_policy.rs:187-225` |
| Tauri 无自定义命令及其原因 | `src-tauri/src/lib.rs:148-160` |
| Tauri 能力白名单四项 | `src-tauri/capabilities/default.json` |
| `open-external` 只校验 http(s) 前缀 | `src-tauri/src/native_bridge.rs:39-46,53-62` |
| Android 无障碍档明确不做 | `docs/功能-变更史.md:3954`；`docs/archive/2026-09-16-consolidation/命令执行层与Android三档执行器-变更说明.md:128` |
| 不做网页抓取/图片与二进制读取 | `HARNESS-PLAN.md:200-205`（`:203`） |
| shell 可读任意进程可读路径 | `HARNESS-PLAN.md:198`；`docs/功能.md:779-781` |
| bootstrap 免鉴权自我提权缺口 | `HARNESS-PLAN.md:26`（P0-4） |
| MCP 遗留缺口（不注销/无热重连/孙进程） | `docs/遗留.md:58-64,121-127` |
| 技能元数据存而无消费点 | `docs/遗留.md:246-251` |
| Playwright 仅未启动的 E2E 计划 | `docs/计划.md:806-821` |
| ROOT/Shizuku 未真机验证 | `docs/遗留.md:577-581` |
| 产品定位 | `README.md:1-3`；Roadmap `README.md:279-294` |

## 附录 B：本次调查中发现的、值得单独立项的既有问题（与 computer use 无关但同源）

1. **`bash` 超时不回收孙进程**：`services/exec/desktop.rs:4` 的头注释宣称"含子孙进程，Windows 用 taskkill /T"，但实现处自认只杀直接子进程（`desktop.rs:65-67`），全仓 exec 路径无 `taskkill /T`/`process_group`。注释与实现不一致，属文档-代码漂移。
2. **Windows 输出编码**：`String::from_utf8_lossy`（`desktop.rs:96-104`）在中文 Windows 上会因 cmd 的 GBK 输出产生替换字符，且未做编码转换后回灌模型。
3. **Android 静默降级导致档位失真**：Shizuku 执行中途失效时 Kotlin 直接用沙箱档重跑，但回传给 Rust 的 `tier` 仍是执行前探测的 `Shizuku`（`ShellExecutorBridge.kt:219-225`）——**审计记录的执行档位可能与实际不符**。
4. **`ExecResult.timed_out` 是死字段**（恒 false）。
5. **Android 输出无流式上限**：Kotlin 侧 `readText()` 全量读入内存后才截断，巨量输出有 OOM 风险（`ShellExecutorBridge.kt:243-246`）。
