# Computer Use 能力建设 · 执行稿（2026-09-26）

> 临时执行稿：不进提交，收口时删除。口径已定项，执行时不得擅自变更（要改先改本节并说明理由）。
> 行号取自当前工作区；实施前一律先 grep 复核（`docs/经验.md` 条目 2）。
> 配套文档：`COMPUTER-USE-REPORT.md`（Kedai 现状与缺口）、`COMPUTER-USE-HARNESSES.md`（成熟实现参照与共识定律）。

## 0. 目标与评估口径

**「computer use 能力」在本稿的定义**（不是通用 RPA，也不是"能跑脚本"）：

1. **观察**：Agent 能读到"屏幕上有什么"——元素树（结构化、便宜、精确）优先，截图（像素、贵、兜底）次之；
2. **动作**：Agent 能对"具体控件/坐标"施加动作——元素语义动作优先，坐标点击兜底；
3. **闭环**：动作之后必须能再观察并自证效果（"尝试过"≠"完成"）；
4. **授权与审计**：每个动作可被批准/拒绝/中止，且留下可审计痕迹；屏幕内容有隐私边界。

### 四条验收场景（做完要能跑，不然不算完成）

| # | 场景 | 通过判据 |
|---|---|---|
| V1 | **Windows 基本操作**：打开记事本 → 新建 → 输入一段文字 → 保存到已绑定工作区 | 全程**不使用截图**；通过元素树的前后差异自证文字已进入编辑区、保存对话框已填好路径 |
| V2 | **Android 设备操作**（Shizuku 档）：打开一个应用 → 读到其元素树 → 往搜索框输入 ASCII → 截一张图并让模型读出图中文字 | 截图经**视觉通道**真实进入模型上下文（不是 base64 文本），模型能复述图中文字 |
| V3 | **安全口径**：GUI 动作在任务模式默认不可用；`input`/`powershell` 不再被 Bypass 自动放行；第二个会话拿不到控制器；急停立即生效 | 四条各有测试用例锁死（见提交 1 的失败测试清单） |
| V4 | **优雅降级**：能力开关关、helper 缺失、元素已消失、档位未就绪时，工具给出**可诊断的错误码**而不是静默失败或假装成功 | 错误码与文案逐个断言 |

**证据性质**：本稿全部结论来自前两份报告的实测与读码，以及本次对依赖/门禁的核对（§9.3 记录了核对结果）。**未跑构建与测试**。

---

## 1. 缺陷清单（CU-*，均为本稿新登记）

### CU-1 — 视觉输入通道不存在（图片附件是伪多模态）

前端把图片读成 data URL 后**拼成纯文本**塞进消息（`web/src/components/ChatInput.vue:99-110`，`:104` 那行 `[图片: name]\n<base64>`），后端连接器把 `content` **恒序列化为字符串**（`connectors/openai_compatible/mod.rs:488-518`），后端 `data:image` 处理零命中。

**后果**：模型看不到任何图像。这是 computer use 最靠前的地基——**即便加一个截屏工具，截出来的图也没有通道送进模型**。

### CU-2 — 零 GUI 原语

截屏 / 鼠标 / 键盘 / 窗口枚举 / 剪贴板 / 浏览器自动化 / Android 无障碍，在 `server-rs/src/tools/` 与 `services/exec/` 全关键词零命中；26 个内置工具里只有 `bash` 属 `ToolOp::Exec`（`tools/action_class.rs:146-176`）。

### CU-3 — 键鼠/屏幕级命令已落在"可自动放行"档（**安全前提**）

`input`、`screencap`、`uiautomator`、`dumpsys` **不在任何风险词表** → 按"未识别保守归 Sensitive"兜底（`tools/command_risk.rs:344-347`）；桌面 `powershell`/`pwsh` 同为 Sensitive（`command_risk.rs:115,148-149`）。而 Sensitive 在 **Bypass 档（= 任务白名单档）可自动放行**（`tools/permissions.rs:195-207`，回归测试 `:901-930`）。

**后果**：**在引入任何 GUI 工具之前，事实上的键鼠注入与屏幕读取通道已经打开**——Android 上 `input tap`（触摸注入）+ `screencap`（读屏）、桌面上 PowerShell 的 `SendKeys`/截屏。这不是"未来风险"，是现状。

### CU-4 — 缺"能力开关 + 单控制器 + 急停"三件套

无 `computer_use` 总开关（对比 `exec_enabled` 的存在）、无输入独占（real 键鼠是全局独占资源，对照成熟实现的 `CONTROLLER_BUSY`/controller lease）、无 Agent 可调之外的用户急停按钮。

### CU-5 — Android 端无结构化设备工具

只能让模型手写 shell 命令；`uiautomator dump` 无人封装（要落盘 + pull + 解析），`am`/`pm`/`settings` 归 Admin 被硬门拦（`command_risk.rs:76-78`）→ **连"启动目标应用"这条都走不通无人值守**。

### CU-6 — 工具结果无法携带图像

`role: tool` 消息的 `content` 恒字符串；即使捕获了截图，也没有"工具返回图像"的协议位。

### CU-7 — 任务模式无审批通道，与 GUI 动作天然冲突

任务侧 `no_ui_authorization=true`、名单外立即拒绝（`agents/engine/executor.rs:1284-1292`、`services/task_engine/tool_policy.rs:30-38`）。GUI 动作恰恰是"最需要人来点头"的一类。

### CU-8 — 无图像预算与历史裁剪策略

截图是 token 大户。没有任何"最近 N 张"或"整块删除保 prompt cache"的机制（对照 Anthropic 的 `only_n_most_recent_images` + 分块删除）。

### CU-9 — Android 中文输入不可行

`input text` 走虚拟键盘 KeyCharacterMap 映射，**无法输入非 ASCII**（中文/emoji 失败）。当前无任何方案。

### CU-10 — Windows 侧 UIA/注入缺隔离与超时

UIA 是跨进程 COM 调用，目标应用挂起时调用会长时间阻塞；在 tokio 里直接调会把服务卡住且**不可取消**。需要进程级隔离 + 硬超时（对照成熟实现把原生能力放在可 kill 的 helper 进程里）。

### 相关但已登记（引用即可，不重复登记）

`遗留.md`：T2（命令混淆可绕过分级）、L3（孙进程不保证回收）、CFG-2（新增工具 6 处登记无机检）、AND-3（ROOT/Shizuku **未真机验证**）、TM-X1（scratch 无清理入口）、GUI-1~4（自身窗口人工实测）。
`展望.md`：RD-3（工具侧沙箱隔离，未做）、REG-1（新增工具登记机检）、LH-1/LD-2。
`HARNESS-PLAN.md`：P1-6（取消不中止正在执行的工具；`services/exec/desktop.rs:4` 头注释与实现不符）、P1-8（输出**保头**截断 32768，`services/exec/mod.rs:89,98-105`）——G/U 长文本 dump 会踩同一条。

### 与既有「不做」的口径冲突（必须先裁定）

`HARNESS-PLAN.md:203` 明文列了「不做：…**图片与二进制读取**」，`:43` 亦记「`read` 只 `read_to_string`（无二进制、无图片）」。那一条是**编码 harness 批次的范围内约定**，本稿**有意重访**，裁定如下：

- **`read` 与 `fs_read` 保持纯文本、不读图片**（既有纪律不变，避免把二进制塞进文本通道）；
- 图像**只走新开的视觉通道**（提交 2），且只在"模型能力位为真 + 图像预算内"时出现。

写文档时按这条口径落，不要留下"既说不做图片、又做得正欢"的自相矛盾。

---

## 2. 需要拍板的四个点（附推荐，用户不表态则按推荐执行）

| # | 决策点 | 选项 | 推荐与理由 |
|---|---|---|---|
| 1 | **平台优先级** | (a) Windows 先行 (b) Android 先行 (c) 只做一端 | **(a) Windows 先行**：Kedai 主力平台、Agent 与目标同机、与既有 coding harness 同环境；Android 紧随（提交 4）。若只允许做一端，选 Android：Kedai 的 Android 侧已有 Shizuku 三档执行器，**同样的能力在 Android 上成本更低** |
| 2 | **是否把"操作电脑"纳入产品面** | (a) 纳入 (b) 只做地基不开放动作 (c) 不纳入 | **(a) 纳入，但按提交粒度控制暴露面**。提交 1（安全）与提交 2（视觉通道）**无论定位如何都该做**——提交 2 同时修掉"用户贴图变成 base64 垃圾"这个**现有 bug**；提交 3 起才是真正的产品面扩张 |
| 3 | **Android 中文输入方案** | (a) 仅 ASCII（`input text`）(b) **Kedai 自带输入法**（InputMethodService，Agent 直接 `commitText`）(c) 引第三方 ADBKeyBoard APK (d) 剪贴板 + `KEYCODE_PASTE` | **(a) 先落地 + (b) 作为提交 4 的可选子项**。(d) 走不通：后台应用既不能设剪贴板也不能把粘贴送进未获焦的输入框；(c) 引入第三方 APK 与供应链面，只在调研里作对照 |
| 4 | **Windows 截图路线** | (a) 本轮不做（只做元素树）(b) WGC（Windows Graphics Capture）(c) GDI/BitBlt 兜底 | **(a) 本轮不做**。理由不是我保守：**成熟实现（ZCode 0.6.3）在 Windows 上也没做截屏**，其 helper 原文写着"直到 Windows Graphics Capture 被实现"——Windows 截屏要同时处理 DPI 虚拟化、多显示器、遮挡/最小化，成本高而元素树已覆盖大部分控件类目标。(b) 排进提交 5，(c) 只作诊断用途不作为动作依据 |

---

## 3. 提交 1 — 安全前提（对应 CU-3 / CU-4 / CU-7）

目标：**把已经打开的键鼠/屏幕通道收回到可授权状态**，并给后续能力备好三件套。本提交**不含任何新能力**，纯收紧 + 基建。

### 3.1 屏幕/输入类命令提级（安全核心，先写失败测试）

现状：`input`/`screencap`/`uiautomator`/`dumpsys` 未入词表 → Sensitive；`powershell`/`pwsh`/`pwsh` 显式 Sensitive。

做法：**新增一个风险类别 `ScreenInput`（屏幕/输入类）**，语义与 Destructive/Admin 同级 —— **任何模式都不自动放行**（复用 `permissions.rs:138-158` 的硬门位置，即判定顺序的最前面），理由栏写明"会代替用户操作真实设备/读取屏幕"。归属名单：

| 平台 | 归入 `ScreenInput` 的命令 |
|---|---|
| 桌面 | `powershell`、`pwsh`（含 `-Command` 形式的键鼠/截屏能力；**不做参数级嗅探**——参数级判定必然不完备，只按命令名收紧） |
| Android | `input`、`screencap`、`uiautomator`、`dumpsys`、`ime` |

**两条纪律**：

- **不改既有回归测试的语义**：锁 Sensitive 语义的用例是 `permissions.rs:855-862`（`sensitive_command_not_treated_as_destructive`：`mkdir` 在 Strict 需授权、在 Bypass 放行），另两条是 `:871-895`（工具级授权不豁免高危硬门）与 `:900-937`（任务白名单不豁免高危硬门）。三条断言在本批**仍然全部正确**——本批是**把部分命令从 Sensitive 挪出去**，不是改 Sensitive 的语义。新增用例覆盖 `ScreenInput` 的四档全拒。
- **不在 UI 上把 `ScreenInput` 包装成"更高危"以外的含义**：确认卡展示命令原文 + 类别标签 + 一句后果说明即可，不要长篇说教。

### 3.2 能力总开关与分平台子开关

- `settings.json` 新增 `computer_use_enabled`（**默认 false**）、`cu_allow_windows`（默认 false）、`cu_allow_android`（默认 false），与 `exec_enabled` 同族落 `settings_service/params.rs`。
- **关闭时的纪律**：工具**不下发**（模型看不到，不是"调了报错"）；臆造调用返回专门错误码（比"未注册"可诊断）。这条照抄 `fs_*` 的 `workspace_gate` 手法（`task_engine/tool_policy.rs`）。
- 与 `exec_enabled` 的关系：**GUI 能力依赖 exec 开（Android 侧走 shell 档位），但 exec 开不蕴含 GUI 开**——两个开关正交，UI 文案要写清。

### 3.3 三个基建件

1. **控制器租约（单控制器互斥）**：进程内单例 `ComputerUseLease`，`{owner_session_id, acquired_at, expires_at}`；拿不到即返回 `CONTROLLER_BUSY` 并**报告 owner**（"另一个会话持有控制权，请先关闭它"），且**永不可重试**。释放时机：会话结束、急停、超时、进程退出（Drop 兜底）。
2. **急停**：`stop_computer_control`（Agent 可调）**+ 用户侧按钮**（前端一个显眼的"停止操作电脑"）；触发后：撤销租约、当前动作尽量中止、后续 GUI 调用一律 `CONTROL_STOPPED`。**注意既有缺口 P1-6（取消不中止正在执行的工具）**——GUI 动作必须有可中止路径，否则急停是假的；本提交至少保证"新动作不再发起 + 租约撤销"，动作级中止排进提交 3/4 的实现要求。
3. **审计**：复用 `exec_audit` 的形态新增 `cu_audit`（或加列），记录：时间、会话/任务、平台、动作、目标（应用名/元素 role+name/坐标）、裁决、结果、错误码。**屏幕内容一律不入库**：只记 `frame_id`、字节数、哈希，不落 base64（隐私要求，见 §9.2）。

### 3.4 任务模式 fail-closed

任务模式 **一律不下发 GUI 动作类工具**（本轮不给任何例外）。观察类（提交 3 的 `win_state`）也先不给，等提交 3 完成后再单独评估——理由是"读到屏幕"在无人值守场景下等于把用户屏幕内容交给模型，与"任务模式无审批"叠加不可接受。

### 3.5 先写的失败测试清单

1. `ScreenInput` 类命令在四档（Loose/Default/Bypass/白名单）**全部拒绝**，且拒绝理由含类别标签；
2. `ScreenInput` 命中时**不消耗** `exec_allow_*` 的档位放行；
3. 能力开关关闭 → GUI 工具**不在**下发清单里；臆造调用返回专门错误码；
4. `computer_use_enabled=false` 时 `cu_allow_*` 无论真假都不生效（防"只开子开关"的误配）；
5. 租约：同会话可重入、异会话 `CONTROLLER_BUSY`、过期后可获取、急停后不可获取；
6. 审计：每次尝试（含被拒、含 `CONTROLLER_BUSY`）**都有且只有一条**记录，且不含 base64；
7. 回归：既有 `exec_audit` 与授权卡行为不变。

### 3.6 验证与门禁同步

```powershell
cargo test --manifest-path server-rs/Cargo.toml -j 8
npm test -w web
node tools/check-doc-claims.mjs      # 新增设置项与风险类别若被文档计数，需同步断言
node tools/check-contract.mjs        # 若 settings 字段进入契约快照
node tools/count-tests.mjs --check
```

---

## 4. 提交 2 — 视觉输入通道（对应 CU-1 / CU-6 / CU-8）

目标：**让模型真的看得见图像**。这一提交同时修掉"用户贴图变 base64 垃圾"的现有 bug，是**独立于 computer use 也有价值**的地基。

### 4.1 消息可携带图像（向后兼容）

- `LlmMessage`（`models/types.rs:390-402`）新增 `images: Option<Vec<ImageRef>>`，`#[serde(default, skip_serializing_if = "Option::is_none")]`。
- **连接器侧**：`to_openai_messages`（`connectors/openai_compatible/mod.rs:480-520`）在 `images` 非空且**模型能力位为真**时，把该条消息的 `content` 序列化为 **parts 数组**（`[{type:"text",text},{type:"image_url",image_url:{url:"data:image/png;base64,..."}}]`）；为空时**逐字节保持现状**（纯字符串）。
- **不做**：不改 `content: string` 的既有形态（`docs/契约.md:110,116`），不引入 break change；图像是**并列的可选字段**，不是把 content 改成枚举——理由是本仓 message 结构被 1000+ 测试与前端契约锁着，枚举化会牵动面过大。

### 4.2 工具结果可携带图像

新增"工具返回图像"的协议位：`role: tool` 消息同样支持 `images`（同一套机制）。工具执行返回值从 `String` 扩展为 `{ text: String, images: Vec<ImageRef> }` 的可选形态，**既有工具零改动**（缺省即纯文本）。

### 4.3 图像落盘与预算（CU-8）

| 项 | 决定 | 理由 |
|---|---|---|
| 落盘位置 | `DATA_DIR/cu_frames/<session或task>/<frame_id>.png` | 不进工作区、不进仓库、不进聊天数据表；可被清理策略整体回收 |
| DB 只存引用 | 消息里存 `{frame_id, path, w, h, bytes, sha256}`，**不存 base64** | 防 SQLite 膨胀；同一张图跨轮复用不重复计费 |
| 降采样 | 长边 ≤ 1568px（等比），PNG 优先；超过单图字节上限再转 JPEG q75 | 对齐主流视觉模型的输入上限；UI 文本用 PNG 更清晰 |
| 历史裁剪 | 只保留**最近 N 张**（默认 4）图像；删除按**固定块**整块删，不做逐条删 | 与成熟实现同款：整块删除对 prompt cache 友好（Anthropic `loop.py` 注释：cached reads 是 10% 价格） |
| 清理 | 会话/任务结束 + 启动时清理超过 T 天的残留帧 | 与 TM-X1（scratch 无清理入口）同源问题，本批只做 GUI 帧的资源回收，不扩张到 scratch |

### 4.4 模型能力位与降级

- 新增设置 `supports_vision`（**默认 false**）——**保守默认**：不知道就当作看不见。
- 能力位为假时：截图工具的调用**明确报错**（文案含"当前模型未开启视觉能力，请在设置中开启或改用文本路径"），**绝不**把 base64 塞进文本。
- 前端设置页给一个开关 + 一句说明（哪些模型支持），不做模型自动探测（探测要发试探请求，成本与噪声都不划算）。

### 4.5 前端

- `ChatInput.vue` 附件路径改写：图片**不再拼进 `content`**，改走 `images` 字段（`store.sendMessage(content, images)`）；文本文件行为不变。
- 附件缩略图 + "当前模型不支持视觉"的显式提示（不要让用户以为发成功了）。
- 消息渲染：支持展示图像（含"已被裁剪"的占位说明，避免用户困惑为什么模型"忘了"那张图）。

### 4.6 安全

图像进入上下文前必须有提示：**屏幕截图可能包含聊天正文、密钥、其它应用数据**（与 `AGENTS.md`"不泄露聊天正文、API Key 或本地真实数据"的纪律直接相关）。本批落地：① 界面上首次开启时的一次性告知；② 图像只发给用户自己配置的模型端点（不新增任何第三方）；③ 审计只记元数据（§3.3）；④ 提供"不看图"的全局开关（`supports_vision=false` 即等于关闭）。

### 4.7 先写的失败测试清单

1. `images` 为空时，连接器输出与改造前**逐字节相同**（快照比对）；
2. `supports_vision=false` 时：截图工具报错、消息不产生 parts、前端提示出现；
3. 降采样：超长边图被等比缩放到 ≤1568；超字节上限转 JPEG；不放大；
4. 历史裁剪：第 N+1 张入上下文时最旧一张被移除，**且删除是按块发生**（同轮内多张一起走）；
5. `frame` 生命周期：会话结束后文件被回收；引用失效时给可读文案而非 panic；
6. 机密边界：图像**不进** `exec_audit`/`cu_audit`、不进聊天导出 JSON。

### 4.8 验证

同 §3.6，另加 `npm run build -w web`（前端改动）。

---

## 5. 提交 3 — Windows 观察与动作（元素树，对应 CU-2 / CU-10）

目标：V1 验收（记事本任务）。**本提交不做截图、不做坐标**——只做"元素树 + 元素动作"，这是成熟实现的主路径，也是成本最低的可用面。

### 5.1 架构决策：helper 进程（不是进程内调用）

**新增 `server-rs/src/bin/kedai-cu-helper.rs`（Windows only）+ `server-rs/src/services/computer_use/`（宿主侧管理）**。

- **为什么必须独立进程**：UIA 是跨进程 COM 调用，目标应用挂起时调用会长时间阻塞且**不可取消**（这是所有 UIA 实现的通病）；进程内做就等于把后端可被一个卡死的第三方应用拖垮。独立进程可**硬超时 + kill**，并可崩溃隔离。
- **复用既有经验**：Kedai 已有成熟的 stdio 子进程托管（`mcp/process.rs`：piped stdio、stderr 入日志、`kill_on_drop(true)`），helper 用同款 JSON-lines over stdio；**不要**再发明一套 IPC。
- **必须补的既有短板**：`mcp/process.rs` 注释已指出"kill 只杀直接子进程，孙进程不保证回收"——helper 若有子进程需走 Job Object（与 HARNESS P1-6 同源，建议与那批合并口径）。
- **构建**：helper 由 `build.ps1` 一并产出并随包分发（便携版目录内）；找不到 helper 时返回 `HELPER_UNAVAILABLE` 而不是静默降级。**不改 Tauri 侧**（helper 由 Rust 服务派生，不经 webview，不涉 Tauri ACL）。

### 5.2 工具族（命名 `win_*`，与 `fs_*` 同风格）

| 工具 | 风险级 | 参数 | 语义要点 |
|---|---|---|---|
| `win_list_windows` | Safe | `app?` | 列出应用与窗口（`window_id`/`title`/`bounds`/`minimized`）；**bounds 仅诊断，不可是坐标** |
| `win_state` | Safe | `window_id, include_diff?, disable_diffing?` | 返回元素树文本 + 元素表（`{index, kind, title, value, actions[], enabled, focused}`）；默认返回**相对上次的 diff** |
| `win_click` | Dangerous | `target(元素 index), mouse_button?, click_count?` | 元素为主；无元素时的坐标路径**本批不提供**（无截图即无栅格权威，见提交 5） |
| `win_set_value` | Dangerous | `element, value` | **优先于打字**：能设值就设值 |
| `win_key` | Dangerous | `text, repeat?, hold_seconds?` | 键/组合键；**必须绑定 app/window**，不允许无绑定键盘输入 |
| `win_scroll` | Dangerous | `target, direction, amount` | amount 为"页"，夹取范围 |
| `win_perform_action` | Dangerous | `element, action` | 只接受元素**自己声明**的 action（树里列出），不接受猜测 |

**元素引用纪律（照抄成熟实现，写进工具描述）**：

- 元素 index **属于最近一次观察**；观察后重新编号；
- 元素消失 → `ELEMENT_UNAVAILABLE`（fail-closed），不是"点了没反应"；
- 树被裁剪时说清裁剪并让索引跳号；被裁掉的元素可由元素表按名字过滤取回；
- 观察会等 UI 稳定再捕获 → **禁止模型用 `sleep`/轮询代替观察**；
- 一次动作后必须再观察（"尝试过"≠"完成"）。

### 5.3 错误契约（照抄成熟实现的错误学）

错误码：`ELEMENT_UNAVAILABLE`、`STALE_STATE`、`ACTION_UNAVAILABLE`、`NOT_SETTABLE`、`FOREGROUND_REQUIRED`、`CONTROLLER_BUSY`、`CONTROL_STOPPED`、`HELPER_UNAVAILABLE`、`VERSION_MISMATCH`、`TIMEOUT`、`INTERNAL`。

三个必须落对的语义：

1. **`actionSent`**：动作是否**可能已下发**；默认 `false`（保守方向——宁可让模型重试，也不要让它放弃一个从未下发的动作）。
2. **`retry`**：`reobserve`（先重新观察）/ `retry` / `never`（`CONTROLLER_BUSY`、`CONTROL_STOPPED`、权限类一律 `never`）。
3. **未知错误归 `INTERNAL`，绝不静默成功**。

### 5.4 风险级与下发

- 动作类 = `Dangerous`；观察类 = `Safe`（但仍受能力开关约束）。
- 下发闸门新增 `computer_use_gate`（仿 `workspace_gate`/`platform_gate`）：`computer_use_enabled && cu_allow_windows` 为假 → **整族剔除**；`fs_*` 的 fail-closed 纪律同样适用。
- 聊天路径需过授权卡（`permissions.rs` 既有机制）；任务模式按 §3.4 **不给**。

### 5.5 「何时调用」指南与提示词段

工具指南（`tools/registry.rs` 的 `tool_when`）+ 一段纪律文本，内容就是成熟实现的那四条（**不要自己发明措辞**）：

1. 能拿到元素就用元素，**永不因为快捷键更短就把元素动作换成键盘**；
2. 能 `setValue` 就不要打字；`paste` 只用于富文本或不可设值的目标；
3. 坐标是最后手段（本批根本没有）；
4. **不要**对同一动作同时跑元素与视觉两条路；动作后必须再观察自证。

### 5.6 先写的失败测试清单

1. 能力开关关 / 平台开关关 → 工具不下发；臆造调用给专门错误码；
2. helper 不存在 → `HELPER_UNAVAILABLE`；helper 半途崩溃 → 同码 + 租约释放；
3. 元素索引跨观察失效 → `ELEMENT_UNAVAILABLE`；
4. 观察返回 diff 且在 `disable_diffing=true` 时返回全量；
5. `win_key` 无绑定 app/window → 拒绝；
6. `win_perform_action` 传树里没声明的 action → `ACTION_UNAVAILABLE`；
7. 超时：helper 卡住 → 硬超时 `TIMEOUT`，且**进程被回收**（断言无残留子进程）；
8. 租约与急停联动（同 §3.5 第 5 条，跨工具族）。

### 5.7 依赖与门禁（**实施前已核对**）

- UIA 与 SendInput 用 `windows` crate（`Win32_UI_Accessibility` / `Win32_UI_Input_KeyboardAndMouse` / `Win32_System_Com` / `Win32_Foundation`）。**该 crate 已在 `src-tauri/Cargo.lock` 内（0.61.3）**，按 `check-lock-sync.mjs` 的规则（内嵌方版本集合须覆盖被内嵌方）**取 0.61.x 即门禁绿**。
- `windows-sys` 现有 features 只有 `Win32_Foundation`/`Win32_Security_Cryptography`/`Win32_Storage_FileSystem`（`server-rs/Cargo.toml:66-70`），**不够**，需要另加（或直接用 `windows` crate，二者可并存）。
- 图像处理（提交 2 的降采样）需要新增依赖（如 `image`，纯 Rust、无 C 依赖 → 对 Android 交叉编译友好）。**注意**：`check-lock-sync.mjs` 头注释明确「**仅存在于单侧的 crate 跳过**」，所以**门禁不会替你发现这类漂移**——新增依赖必须主动在 `src-tauri` 侧也锁定（或在变更说明里登记理由），否则测试版与便携版可能解析出不同版本。
- 新增工具触发 CFG-2 的**6 处登记**（注册定义 / `default_risk` / `tool_sets` / `tool_when` / `render_kind_for` / 前端流程白名单），当前无机检；本批必须手工过一遍并同步 `check-doc-claims.mjs` 的计数断言。

### 5.8 验证

同 §3.6；另需在真实 Windows 环境手工走一遍 V1（自动化测不了 UIA 与真实应用的交互，`docs/遗留.md` GUI-1~4 已记同款限制）。

---

## 6. 提交 4 — Android 设备操作（对应 CU-5 / CU-9）

目标：V2 验收。**成本显著低于 Windows**：执行通道（Shizuku/root 档位的 shell）已经存在，缺的是结构化工具与视觉通道。

### 6.1 工具族（命名 `dev_*`）

| 工具 | 风险级 | 参数 | 实现 |
|---|---|---|---|
| `dev_status` | Safe | — | 档位与能力位：`{tier, input_ok, ui_dump_ok, screenshot_ok}`——**先声明能力再干活** |
| `dev_ui` | Safe | `query?, max?` | `uiautomator dump` → 解析 → 元素表 `{index, text, contentDescription, resourceId, className, bounds, clickable, enabled}`（形状照 `android-emulator` 插件的 `UiElement`） |
| `dev_tap` / `dev_swipe` | Dangerous | `x,y` / `x1,y1,x2,y2,duration` | `input tap` / `input swipe`（**长按 = 同点 swipe + 长 duration**） |
| `dev_text` | Dangerous | `text` | ASCII 走 `input text`；非 ASCII 见 §6.3 |
| `dev_keyevent` | Dangerous | `key` | 白名单键位（BACK/HOME/ENTER/APP_SWITCH/...），不做任意 keycode 透传 |
| `dev_screenshot` | Sensitive | `region?` | `screencap -p` → **经视觉通道回传**（提交 2 的成果）；只回传图像，不落工作区 |
| `dev_app` | Dangerous | `action: launch/terminate/open_url, package/url` | `am start`/`am force-stop`——**本工具内部走授权门，不放松命令词表** |

**一个关键设计决定**：`am`/`pm`/`settings` 在 `command_risk.rs:76-78` 是 Admin、被硬门全拒。本批**不修改这三条词表**（一放松就同时放开了 bash 路径），而是让 `dev_app` 作为**结构化工具**走自己的风险级与授权门。这样"模型手写 `am start`"仍然被拦，"`dev_app` 启动应用"可被单独批准——**权限面更细，而不是更宽**。

### 6.2 执行通道与档位如实回报

- 复用既有 ShellTier 三档（ROOT → Shizuku → 沙箱）；`dev_*` 工具内部走 `services::exec::execute`，**在沙箱档直接判定不可用**（`input`/`screencap` 在应用自身 UID 下通常失败）。
- **顺手修一个既有缺陷**：Shizuku 执行中途失效时 Kotlin 侧会用沙箱档重跑同一命令，而回报给 Rust 的 `tier` 仍是执行前探测值（`ShellExecutorBridge.kt:219-225`）→ **审计口径失真**。本批要求"实际执行档如实回报"，否则 `dev_status` 的能力位会说谎。
- 前置：Android 上 ROOT/Shizuku 档**至今未真机验证**（`遗留.md:577-581`，AND-3）→ V2 验收**必须先有一台真机跑通 Shizuku 档**，否则本提交只能算"代码完成"。

### 6.3 中文输入（CU-9，按 §2 决策 3 执行）

- **先落地**：`dev_text` 对非 ASCII **明确报错**（文案给"当前仅支持 ASCII，中文请改用 …"），不要静默失败或发出去变成乱码。
- **可选项（推荐做）**：**Kedai 自带输入法**——`InputMethodService` 组件（属 Kedai 自己的 Android 应用），Agent 侧把文本交给自己的 IME 执行 `commitText`，用户只需一次性在系统设置里启用（并用 Shizuku 档的 `ime set` 自动切换）。优点：**零第三方依赖**、任意 Unicode、无需广播协议。
- **不采用**：第三方 ADBKeyBoard APK（供应链面 + 停更风险）、剪贴板 + `KEYCODE_PASTE`（后台应用既不能设剪贴板，也送不进未获焦的输入框）。

### 6.4 先写的失败测试清单

1. 沙箱档调用 `dev_tap`/`dev_screenshot` → 明确不可用（不是执行失败）；
2. `dev_status` 能力位与实际可执行性一致（**含档位失效后的降级如实回报**）；
3. `dev_text` 非 ASCII → 报错且**不执行**任何命令；
4. `dev_keyevent` 传白名单外键位 → 拒绝；
5. `dev_ui` 解析：fixture XML（含中文 text/contentDescription、嵌套、不可点击容器）→ 元素表正确；
6. `dev_screenshot` 产出图像**进入视觉通道**且**不进工作区**、不进审计正文；
7. `dev_app launch` 与手写 `am start` 的权限差异：前者可批准、后者恒拒（锁死"不放松词表"这条）。

### 6.5 验证

同 §3.6；另需 Android 真机（Shizuku 档）手工走 V2。

---

## 7. 提交 5（可延后）— 截图与坐标闭环

只有到这一步才引入坐标，且必须先把"栅格权威"做对：

1. **截屏原语**：Windows 走 **WGC**（Windows Graphics Capture，正解），GDI/BitBlt 只作诊断不作动作依据；Android 已有 `dev_screenshot`。
2. **栅格权威（frame）**：每次捕获签发 `frame_id`，坐标只对**那一帧**语义有效；模型只对**当前返回的图**取整数像素，且 `0 <= x < width`；**元素与窗口的 bounds 永远是诊断值，不得当坐标**。
3. **坐标变换归 harness**：模型给的是返回栅格的像素，缩放/DPI/窗口偏移由我们内部换算（成熟实现的做法：SDK 内部绑定栅格、独自承担全部变换）。
4. **DPI 纪律**：Tauri 进程声明 **Per-Monitor V2** 感知，否则 UIA 的 `BoundingRectangle` 与截图像素空间不一致（非 PMv2 线程的系统 API 返回值会被虚拟化）；坐标优先用**窗口内归一化**而非绝对屏幕坐标（一次性消掉 DPI/多显示器/窗口拖动三个问题）。
5. **"点了没反应"要能被发现**：动作后观察若发现"被接受但效果未变"，在观察结果里标注（对照成熟实现的 `[effect_evidence unchanged]`），并**禁止重复同一坐标**。

---

## 8. 提交 6（可延后 / 可替换）— 浏览器

**决策：本轮不内建浏览器。** 两条理由：① 它是另一套地基（CDP vs UIA/adb），与上面五批不同源；② Kedai 的任务模式当前拿不到 MCP 工具，而聊天模式接 MCP 可以**零代码**先验证需求。

- **验证路径（零代码）**：设置页填 `npx @playwright/mcp@latest`（`--headless --isolated`，只开 core 能力组），聊天模式试用。注意如实告知用户：需本机有 Node、每次调用要过授权、**任务模式用不了**。
- **若确定要内建**：两台可选路线——`agent-browser`（Rust 原生、可作 stdio MCP、自带 `@eN` 引用与省 token 手段）或 `chromiumoxide`/`playwright-rs`（内建，需自建快照层，成本最高）。**内建必须先做快照层**（a11y/DOM 树裁剪 + 元素编号 + 引用生命周期），那是这个提交里最贵也最核心的部分。

---

## 9. 明确不做 / 进程内做不到

### 9.1 明确不做

- **纯视觉主路径**（截图 → 模型直接吐坐标）：不做。理由有量化锚点：纯视觉裸模型点不准（OmniParser v2 博客：ScreenSpot Pro 上 GPT-4o 裸测 0.8 → 加解析器 39.6）。
- **MediaProjection**：不做（每会话授权 + Android 14 一 token 一会话，长跑 agent 不可用）。
- **Android 无障碍（AccessibilityService）档**：本轮不做（与既有明文否决一致，`docs/功能-变更史.md:3954`）；能力上它是"无 adb 通道时的上限"，等 §2 决策 1 若改为 Android 先行再评估。
- **macOS / Linux GUI**：不做。
- **内建浏览器**：本轮不做（见提交 6）。
- **屏幕录制 / 视频**：不做。
- **操作 Kedai 自己的窗口**：不做（自指混乱，且 GUI-1~4 是另一件事）。
- **Canvas / 游戏 / 自绘 UI 的保证**：不承诺（无元素树时明确报错，不假装支持）。
- **`read`/`fs_read` 读图片**：不做（图像只走视觉通道）。

### 9.2 进程内做不到（诚实边界，不要写成已解决）

1. **GUI 动作 = 把用户的全部应用纳入模型可达面**。屏幕内容与第三方应用文本是**不可信输入**，prompt injection 会从屏幕、网页、聊天窗口进来；**这在单机应用内封不死**，只能靠"授权 + 审计 + 用户在场 + 可急停"。任何声称"我们防住了注入"的写法都过不了口径检查。
2. **屏幕内容的隐私与"不泄露聊天正文"纪律天然冲突**。截图会把聊天正文/密钥/其它应用数据一起交给模型端点。本稿的处置是"默认关 + 一次性告知 + 只发用户自己的端点 + 审计只记元数据 + 可整体关闭"，**不是"解决了隐私问题"**。
3. **Windows UIA 覆盖不全**：Electron（无障碍默认关）、Chromium（按需建树）、Canvas/游戏拿不到元素 → 那些目标在提交 3 的能力范围内**做不了**，要等提交 5 的坐标路径或永远做不到。
4. **Android 的四个硬限制**：`FLAG_SECURE` 窗口截不到；`input text` 打不了非 ASCII（除非做 IME）；Shizuku 每次重启要用户手动启动；档位真机未验证（AND-3）。
5. **helper 可被 kill 不等于动作可撤销**：已经点下去的删除/发送无法回滚。授权门是唯一防线。

### 9.3 本稿对既有设施的核对结论（实施前已做）

| 核对项 | 结论 |
|---|---|
| `windows` crate | `src-tauri/Cargo.lock` 已有 0.61.3 → 取 0.61.x 可过 `check-lock-sync` |
| `windows-sys` features | 现有三项不含 UI 输入/无障碍 → 需新增（`server-rs/Cargo.toml:66-70`） |
| 新增 `image` 类依赖 | 两侧 lock 都无 → **门禁对"单侧 crate"跳过，不会报红**，需主动登记（防测试版/便携版静默漂移） |
| 新增工具 | 触发 CFG-2 的 6 处登记 + `check-doc-claims.mjs` 计数断言，均需手工过 |
| 既有回归测试 | 锁 Sensitive 语义的是 `permissions.rs:855-862`（`mkdir` 在 Bypass 放行），锁高危硬门的是 `:871-895`/`:900-937` → 本稿**只挪命令、不改 Sensitive 语义**，三条断言均不冲突 |

---

## 10. 前置条件与资源纪律

- **排序**：建议排在 `HARNESS-PLAN` 提交 3 与 `TASK-MODE-FIX-PLAN` 提交 3 之后开工——那两批都会动 `services/exec/`（取消中止工具、进程树清理、输出保尾截断），与本稿提交 3/4 的 helper/执行通道同域，同时改会互相打架。**但提交 1 与提交 2 可随时开工**（不碰 exec 层）。
- **平台顺序**：Windows（提交 3）与 Android（提交 4）可并行开发，但**验收必须各自有真机/真环境**；Android 侧的真机（Shizuku 档）依赖 AND-3 的验证结论。
- **本机 shell 无 cargo 环境** → 走 `tools/cargo-vcvars.cmd` 包装器；`-j 8`，内存紧张时按既有纪律降 `CARGO_INCREMENTAL=0 + -j 2`；**不要与 `build.ps1` 并发**。
- 改前端必须 `npm run build -w web`；交付/试用 exe 前跑一次完整 `.\build.ps1`（helper 二进制新增后，确认它确实进了便携版目录）。

---

## 11. 收口

按本仓体例收口：`功能.md`（新工具族与能力开关）、`契约-协议与配置.md`（6 处登记 + 新设置项 + 错误码表）、`契约.md`（消息 `images` 字段与端点变化）、`遗留.md`（CU-1~CU-10 的登记与状态）、`展望.md`（RD-3 与本稿的关系、《不做》清单的更新）、`经验.md`（采坑：伪多模态、UIA 阻塞需 helper、Android `input text` 非 ASCII、屏幕内容隐私边界）、`功能-变更史.md` 收口章。**删本稿。**
