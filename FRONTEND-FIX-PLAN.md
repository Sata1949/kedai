# Kedai 前端修复计划

> 依据：[`FRONTEND-REPORT.md`](./FRONTEND-REPORT.md)（2026-09-26 前端调查报告）
> 范围：`web/src` 前端。**不含** `server-rs` 后端改动（唯一例外见 §4.1 的负向验证）。
> 口径：分三批提交，每批独立可验证、独立可回滚。每批"完成"的定义是——失败测试先红、实现后转绿、全量门禁绿、文档同步、按体例提交。

---

## 0. 前置：开工前必须对齐的四件事

### 0.1 与既有治理的重叠（**先读，避免另起一套口径**）

调查报告的 P1-3（资源卡触发门槛）与仓库**已登记的既有事项**相邻，必须先分清关系再动手：

| 既有登记 | 位置 | 它覆盖什么 | 它**不**覆盖什么 |
|---|---|---|---|
| **T1** `resource-frame` 允许外联 + TavernHelper RPC 授予生成能力 | `docs/遗留.md:176`、`docs/计划.md:897`（3.6）、`docs/计划.md:2751`（§七 T1） | 加载**之后**的能力：CSP 的 `connect-src`、作者页 RPC。判定为**知情接受（不修）**，待办是"设置项：禁用作者页外联（**默认保持现状=开**）" | **触发门槛**——页面为什么会被加载、为什么绕过 `renderHtml` 开关与逐卡 SHA-256 授权 |
| **E.1** 脚本桥纳入授权裁决（已知限制 L12，待办） | `docs/功能-变更史.md:1106` | 授权台账与后端执行路径（`agents/engine/mod.rs`）未打通 | 资源卡通道的授权语义 |

**结论**：本计划的 §3.1 **不推翻 T1 的裁决**（"作者页可外联"仍是知情接受），只补上它未覆盖的一个维度：*加载动作本身没有任何用户知情环节，而同一份作者 JS 走卡脚本通道需要 `renderHtml` + 逐卡哈希授权两道闸门*。§3.1 的选项 B 若被采纳，则应与 E.1 合并口径登记。

### 0.2 需裁决项 R1：资源卡触发门槛的处置口径

三个选项，默认执行 **A + C**（见 §3.1 详述）：

1. **(推荐) A：首次遇到新域名时一次性确认，按域名记忆** —— 保住"资源界面可用"，把"装载并执行第三方 JS"变成用户知情动作。
2. **B：纳入逐卡授权** —— 与卡脚本通道完全对齐，但会**直接破坏**当前依赖该通道且未开 HTML 渲染的既有角色卡（仓库文档点名的 `files.yuzuki-rii.xyz/bby/v2.1.html` 即属此类），属产品能力收缩，需明确同意。
3. **C（不论选 A/B 都建议叠加）：收敛能力** —— `TavernHelper.generate` 加节流与上限；"未授权/找不到 entry"从**静默挂起**改为**回 error**（当前作者页 `await` 会永久悬挂，**这是与安全无关的独立缺陷**）。

### 0.3 明确不做（附理由，避免范围蔓延）

| 不做项 | 理由 |
|---|---|
| `TaskBoard.vue`（1240 行 / 27 computed）拆分 | 是重构不是修复，改动面覆盖任务模式全部 6 种执行分支，风险与收益都远超本计划。建议另立专项（报告 §七-7） |
| 16 个弹窗的 ESC / 焦点陷阱 / `role="dialog"` | 属交互与可访问性设计变更，需先定统一范式（共享 `useModalA11y`），且要做人工可用性验收。建议另立专项 |
| 覆盖率工具链（`@vitest/coverage-v8` + 阈值） | **环境阻塞**：本机网络拉不到新依赖（`web/src/platform.ts:3-5` 记录了同因导致的 `plugin-os` 降级）。§4.3 用**零依赖替代方案**覆盖同一盲区 |
| P2 中的性能类（`contentSignature` 全量 stringify、`utf8Bytes` 双份序列化、`isWithinRowWindow` O(n²)） | 均为**代码结构推断**、无实测数据。按仓库纪律应先量化再优化（`tools/perf-baseline.mjs` 只有后端端点，需先补测量手段）。列为后续 |
| WebView2 `event.source` 假设冲突（报告附录 B-1） | 结论未定，需桌面 exe 实测才能判定。**若是缺陷则是 P2**，届时另开提交 |

### 0.4 环境与纪律前置

- **构建/测试命令**：前端不涉及 vcvars（那是 cargo 的事）。全部命令在 `D:\kedai` 根执行。
- **AGENTS.md 硬纪律**：修改核心行为**先写失败测试**，再做最小实现。本计划每一项都给出"失败测试"落点。
- **新增 `web/src` 顶层**非测试 **源文件须登记 `tools/arch-layers.json`**（`check-arch.mjs` 规则 I 会 FAIL）。测试文件不受此约束（`render.test.ts` 等根目录测试文件未登记且门禁通过），因此本计划的测试文件可自由放置。
- **类型逃逸 ratchet 无余量**：`check-frontend-lint.mjs` 五项实测**恰好等于基线**（`asNever:31 / asUnknownAs:69 / tsExpectError:6 / nonNull:33`）。新增代码与新增测试**不得**引入任何一个。构造 fixture 时优先用类型正确的写法（如 `satisfies`），不要沿用 `store.characters = [...] as never` 这类既有写法。

---

## 1. 批次总览

| 批次 | 主题 | 项数 | 产品语义变化 | 建议提交信息 |
|---|---|---|---|---|
| **提交 1** | 正确性：迟到响应与资源生命周期 | 4 | 无 | `fix(前端): 迟到响应守卫 + 图片预览 blob URL 回收 + 资源快照竞态 + 流式渲染缓存绕过` |
| **提交 2** | 安全口径收敛 | 3(+1 可选) | §3.1 有（按 R1 裁决） | `fix(安全): 资源卡加载门槛与生成桥收敛 + 沙箱 style 属性补声明级清洗 + CSS 作用域越界` |
| **提交 3** | 门禁与债务收口 | 4 | 无 | `chore(门禁): TaskEvent 载荷字段契约校验 + 体积预算纳入 pre-push + 零测试文件 ratchet + 样式纪律计数` |

**依赖关系**：提交 1、3 相互独立；提交 2 的 §3.1 依赖 R1 裁决，但 §3.2/§3.3 不依赖，**若 R1 未裁决则先做 §3.2/§3.3，把 §3.1 单独留一个提交**（不要为等裁决阻塞整批）。

---

## 2. 提交 1：正确性

### 2.1 四处"迟到响应覆盖新选择"守卫

#### 2.1.1 `loadHistory` 缺时效守卫

- **现象**：`web/src/stores/chat.ts:99-116` 拿到 `api.fetchHistory(sessionId)` 的响应后**不校验** `currentSessionId` 是否仍等于 `sessionId`，直接 `messages.value = msgs`（`:102`）。
- **同文件反例（说明这是漏配而非设计）**：`restoreAgentTrace`（`:127`）有该守卫，且注释写着"会话已切换：迟到的响应不得覆盖当前会话（loadHistory 并发可重入）"。
- **调用方复核（已逐个查证，10 处全部是"当前会话"语义，故加守卫不误伤）**：
  - `chat.ts:75`（newSession，先设 `currentSessionId`）、`:86`（switchGreeting，取 `currentSessionId.value`）、`:96`（switchSession，先设）、`:251`/`:316`（resend/regenerate）、`:441`（SSE `reloadHistory` 用 `currentSessionId.value`）、`:461`/`:474`（删除/编辑后刷新）
  - `character.ts:206`（先 `chat.currentSessionId = target.id`）
  - `components/AgentPanel.vue:175`（回退快照后刷新，`sid` 为当前会话）
  - `composables/useDataManager.ts:60`（导入聊天记录后刷新，`sid = currentSessionId.value`）
- **失败测试**：`web/src/stores/chat.test.ts` 新增用例——mock `fetchHistory` 返回一个**受控 Promise**，先 `switchSession('A')`（不 await 完），再 `switchSession('B')` 并完成 B，然后 resolve A；断言 `messages` 为 B 的消息、`currentSessionId === 'B'`。**当前实现下该断言失败**。
- **最小实现**（`chat.ts:101-102` 之间）：
  ```ts
  const msgs = await api.fetchHistory(sessionId);
  // 迟到响应:请求落地时会话已切换则丢弃(否则旧会话消息覆盖新会话)
  if (currentSessionId.value !== sessionId) return;
  messages.value = msgs.map((m) => ({ ...m, streaming: false }));
  ```
- **验收**：新用例转绿；`stores/chat.test.ts` 原有用例全绿（尤其 `reloadHistory` 路径）。

#### 2.1.2 `selectCharacter` 尾段缺角色守卫 + `loadSessions` 无乱序保护

- **现象 A**：`web/src/stores/character.ts:198-206` 在 `await chat.loadSessions(id)` / `loadHistory` / `newSession` 之后**无** `currentCharacterId.value !== id` 检查，而同函数内 `:157-158`、`:162` 两处**都有**守卫。
- **现象 B（更根本）**：`chat.ts:59-61` 的 `loadSessions` 就一行 `sessions.value = await api.listSessions(characterId)`——**陈旧覆盖发生在函数内部**，仅在外层加守卫挡不住 `sessions` 数组被写错。
- **失败测试**：`web/src/stores/character.test.ts` 新增用例——mock `listSessions` 为受控 Promise，快速连续 `selectCharacter('A')` → `selectCharacter('B')`，断言最终 `chat.sessions` 是 B 的列表、`currentSessionId` 属于 B、`lastSessionId` 记忆写入的是 B 的会话。（注意：`chat.test.ts` 的 `vi.mock('../api')` 工厂未列 `listSessions`，需在 `character.test.ts` 自己的 mock 中补上。）
- **最小实现（两处，都要）**：
  1. `chat.ts` 的 `loadSessions` 加**请求序号**（不依赖 `storeBridge` 的 `characterIdValue`——那个在未注册时降级为 `() => null`，会让守卫在单测中恒真，反而破坏既有测试）：
     ```ts
     let sessionsSeq = 0;
     async function loadSessions(characterId: string): Promise<void> {
       const seq = ++sessionsSeq;
       const list = await api.listSessions(characterId);
       if (seq !== sessionsSeq) return;   // 已有更新的请求发出:丢弃本次(乱序保护)
       sessions.value = list;
     }
     ```
  2. `character.ts:199` 之后插入外层守卫，挡住 `currentSessionId` / `writeLastSessionId` / `loadHistory` 的错写：
     ```ts
     await chat.loadSessions(id);
     if (currentCharacterId.value !== id) return;   // 迟到:不写会话记忆、不载历史
     ```
- **验收**：新用例转绿；`stores/character.test.ts` 的"重进恢复上次角色/会话"4 条分支（`:62-106`）全绿。

#### 2.1.3 `loadTaskDetail` 守卫方向写反

- **现象**：`web/src/stores/task.ts:415` —— `if (sig !== detailSignature || currentTask.value?.task.id !== id)`。注释（`:414`）的自述意图是"切任务后旧签名残留时也要替换"，但缺的正是 `id === currentTaskId.value` 这一半。A 的响应迟到时 `currentTask?.task.id` 是 `'B' !== 'A'` → 条件恒真 → **强制覆盖为 A 的详情**，于是面板显示 A 的步骤/结果而 `currentTaskId` 是 B。
- **调用方复核（已逐个查证，守卫安全）**：`:447` `selectTask` **先**设 `currentTaskId.value = id` 再在 `:457` 调详情；`:521`/`:528`/`:555`/`:562` 事件驱动处**调用时**已判 `ev.task_id === currentTaskId.value`；`:606`/`:661` 显式传 `currentTaskId.value`；`:713`(runTask)/`:718`(stopTask)/`:753`(approveTask)/`:765`(followupTask)/`:772`(planChatTask) 的目标 id 均来自 `TaskBoard` 对当前显示任务的按钮动作。故 `id !== currentTaskId.value` 时跳过详情刷新是**正确**行为（不把非选中任务画进面板）。
- **失败测试**：`web/src/stores/task.test.ts` 新增用例——`selectTask('A')` 与 `selectTask('B')` 交错，令 A 的 `getTask` 后到；断言 `currentTask.value!.task.id === 'B'`。**当前实现下失败**。
- **最小实现**：把 `:415` 的条件拆成前置守卫（保持"内容未变则保引用"的既有优化不变）：
  ```ts
  const detail = await api.getTask(id);
  // 迟到响应:目标已不是当前选中任务则丢弃(否则旧任务详情覆盖面板)
  if (id !== currentTaskId.value) return;
  const sig = contentSignature(detail);
  if (sig !== detailSignature) { detailSignature = sig; currentTask.value = detail; currentTaskUsage.value = detail.usage_total ?? null; }
  ```
  `:414` 的注释需同步改写（它描述的是被替换掉的旧逻辑）。
- **验收**：新用例转绿；`task.test.ts` 的"零冗余重拉"（`:490-521`）与引用相等断言全绿。

#### 2.1.4 `loadTaskCalls` 同类缺口

- **现象**：`web/src/stores/task.ts:433-444` 只有内容签名去重，无 taskId 时效校验。签名**不能替代** id 守卫——A/B 内容不同时签名必然通过，`taskCalls.value = calls` 照写。
- **失败测试**：并入 2.1.3 的用例（同一竞态窗口内断言 `taskCalls` 归属 B）。
- **最小实现**：`const calls = await api.getTaskCalls(taskId); if (taskId !== currentTaskId.value) return;`
- **验收**：同上。

### 2.2 `ChatInput` 图片预览的 blob URL 生命周期

- **现象**：`web/src/components/ChatInput.vue:72-77` 的 `getFilePreview()` **每次调用**都 `URL.createObjectURL(file)`；`:238` 的模板在同一次渲染里调它**两次**（`v-if` 一次、`:src` 一次）：
  ```html
  <img v-if="getFilePreview(f)" :src="getFilePreview(f)!" alt="" />
  ```
  `text` 是模板直接依赖（`:disabled` 绑定用到），**每次击键都重渲染整组件** → 每个附件每击键新建 2 个 blob URL。全仓 `revokeObjectURL` 只有 `exportFile.ts:32` 一处，与此无关。
- **影响**：附加一张图后输入 200 字 ≈ 400 个存活 URL，每个 pin 住原始 `File` 字节（单文件上限 10MB，`:56`）；`:src` 每帧变化还会让浏览器反复重新解码图片（闪烁 + CPU）。Android 端 `ChatInput` 是主输入区，有 OOM 实感。
- **失败测试**：`web/src/components/ChatInput.test.ts`（已存在，需确认其为 jsdom 环境；无则新文件加 `// @vitest-environment jsdom`）——stub `URL.createObjectURL` 为计数器、`URL.revokeObjectURL` 为计数器；① 添加一张图片附件后断言 `createObjectURL` 只被调用 1 次（当前 2 次）；② 连续触发文本输入 N 次后断言创建次数不增长；③ `removeAttachment(0)` 与 `wrapper.unmount()` 后断言 `revokeObjectURL` 覆盖了全部已创建的 URL。
- **最小实现**：
  - 用 `Map<File, string>` 缓存预览 URL，**创建时去重**；
  - `removeAttachment` 里 `revokeObjectURL` 并删除缓存项；
  - `onBeforeUnmount` 里清掉剩余全部；
  - 模板改为读缓存（`:src="previewOf(f)"`），或改为 `computed` 的附件视图列表（`v-for` 一次算好），**彻底去掉模板内的函数调用**。
  - 注意 `readFileContent`（`:80-95`）走的是 `FileReader`，与预览 URL 无关，不要一起改。
- **验收**：新断言全绿；`ChatInput` 既有测试全绿。

### 2.3 `resourceStore` 快照竞态（双快照分裂 + 丢失更新）

- **现象**：`web/src/resourceStore.ts:218-244` 的 `loadResourceSnapshot` 在 `await idbGet(url)`（`:227`）**之前**就 `loaded.add(url)`（`:223`），在 `await` **之后**才 `memory.set(url, snap)`（`:242`）。
- **竞态窗口机制**：窗口内 `memory.get(url)` 返回 `undefined` → 并发的 `applyResourceSync`（`:247-254`）新建**第二份**空快照、写入 `memory`、再调 `loadResourceSnapshot`（因 `loaded` 已置位而**立即返回新快照、不读 IndexedDB**）→ 随后第一次调用恢复执行，把 IDB 内容 merge 进**旧快照**并用 `memory.set` **覆盖掉刚同步进来的数据**。
- **失败测试**：`web/src/resourceStore.test.ts`（已存在）新增用例——让 `idbGet` 返回受控 Promise；在 pending 期间调 `applyResourceSync(url, {已下载成果})`；resolve `idbGet({旧的持久化缓存})`；断言 `loadResourceSnapshot(url)` 返回的快照**同时**含刚同步的成果与持久化缓存，且成果不被旧值覆盖。**当前实现下成果丢失**。
- **最小实现**（不变量：`loaded` 表示"持久化部分已合入 `memory` 中**当前那份**快照"；`memory.set` 只能写回"本次仍在 `memory` 中的那份"）：
  1. 把 `loaded.add(url)` 移到 `await` **之后**；
  2. `memory.set` 之前比对 `memory.get(url)`：若已不是本次的 `snap`，说明并发方建立了新快照，则把 `caches` **只补缺失键**（不覆盖新值）合入**当前那份**并返回它，放弃回写 `snap`。
- **冲突语义需明确**：新同步的下载成果**优先于**持久化旧值（它更近），故合并只补缺失键。实现时在该分支写一行注释说明这个优先级，避免后来者改成"后者覆盖"。
- **验收**：新用例转绿；`resourceStore.test.ts` 既有用例全绿。

### 2.4 `renderCache` 被流式输出逐帧冲掉

- **现象**：`web/src/components/ChatMessageItem.vue:220-240` 的 `html` computed 以 `paintText` + 全量文本构造 key（`renderCache.ts:115-116` 的 key 末尾是整段文本），**不区分是否流式**。流式期间每帧文本都变 → 每帧 `set` 一个新条目 → 一条 8 秒回复（约 480 帧）就把 500 容量的 FIFO 缓存**全部替换为该消息的历史前缀**，稳定历史条目被挤光，P-9 优化（`renderCache.ts` 头注释自陈省掉 3.057ms/16.8k 字符）在生成时段完全失效。
- **可行性已确认**：`props.m.streaming` 在组件内可用（`ChatMessageItem.vue:189,321,324,357,375` 已在用）。
- **失败测试**：`web/src/renderCache.test.ts`（已存在）或 `ChatMessageItem.renderCache.test.ts`（已存在）——构造一条 20 帧演进的流式消息，断言其**未写入**模块级缓存（缓存内非流式条目的数量/内容不受影响）。当前实现下断言失败。
- **最小实现**（`ChatMessageItem.vue:220` 起）：
  ```ts
  const html = computed<string>(() => {
    // 流式消息不走缓存:每帧文本都变,写入会把容量占满并挤掉稳定历史条目
    if (props.m.streaming) return renderMessageHtml(renderTextFor(/* 保持与缓存路径同一入参 */), scopeId);
    ... // 原缓存路径
  });
  ```
  注意保持两条路径的**入参完全一致**（同一 `paintText`、同一 `scopeId`），否则会出现"流式结束前后 HTML 不一致"的视觉跳变——这是本项最主要的回归风险，测试里要断言"流式最后一帧的 HTML 与非流式重算的 HTML 相等"。
- **验收**：新断言全绿；`ChatMessageItem.renderCache.test.ts` 既有断言全绿；`renderCache.test.ts` 全绿。

---

## 3. 提交 2：安全口径收敛

### 3.1 资源卡加载门槛（**依赖 R1 裁决**，默认 A+C）

**已复核的三层事实**：

1. `web/src/render.ts:534-543` 的 `extractBodyLoadUrl` 用 `/\.load\s*\(\s*['"]([^'"]+)['"]\s*\)/i` 匹配**任意消息文本**，只校验 `https:` + 无凭据（`isSafeResourceUrl`，`:545-553`）。而 `ChatWindow.vue:58-60` 的 `renderTextFor` 取的是 `content_display ?? content`——即**模型输出文本**，故提示词注入可产生它。
2. `web/src/components/ChatMessageItem.vue:246-264` 把它放在**分支优先级第一位**（`:247-250`），**早于** `props.renderHtml && props.scripts.length > 0`（`:254`）与脚本授权门禁。
3. `web/src/composables/useResourceFrames.ts:311-353` 的 `hydrate()` 内**无任何开关/授权检查**（`grep authoriz|renderHtml` 零命中），且 `handleTavernCall`（`:216-245`）**无节流**、`messages` 由**无上限**的 `prompts[]` 组装。

**做 A（推荐）**：

- **失败测试**：新增 `web/src/composables/useResourceFrames.gate.test.ts`（jsdom）或把判定抽成纯函数后单测——① 首次遇到新域名：不自动投递、产生"待确认"状态；② 用户确认后：投递，且该域名进入已同意集合；③ 第二次遇到同域名：直接投递（不重复询问）；④ 非 https / 带凭据 URL：任何情况下都不投递。
- **实现要点**：
  - 判定逻辑抽成**纯函数**（`needsResourceConsent(url, consentedDomains)` 之类）放 `utils/` 或与 `render.ts` 同层，便于零 DOM 单测（仓库既有范式：`useFlowCanvas.ts:3-4` 明说"不 import 画布库，于是能在 node 环境直接单测"）。
  - 已同意域名集合持久化：**参考 `stores/uiPrefs.ts:134-135` 的 `renderHtmlOverrides` 范式**（按 key 记忆 + localStorage），不要新造存储机制。
  - 卡片壳（`render.ts:594-617` 的 `buildRemoteResourceHtml`）增加一个确认条分支；确认动作走消息画布上的**事件委托**（与既有 `data-kd-resource-wide` 的宽屏按钮同一手法，`useResourceFrames.ts` 的 `onWideClick` 是现成样板），避免 v-html 重渲染丢事件。
  - `hydrate()` 前置检查：未同意的 frame **不注册 entry、不预拉取、不启动 12s 兜底投递**。
- **风险**：既有用户体验多一次点击（每域名一次）。**回滚**：revert `hydrate` 的前置检查即可恢复现状。

**做 C（叠加，独立于 A/B）**：

- **C-1 生成桥节流与上限**：给 `handleTavernCall` 增加单会话调用计数与最小间隔（取值建议：**60s 内 ≤ 5 次、单会话 ≤ 50 次**，超限回 error 而非静默），并给组装的 `messages` 加长度上限（建议 64 条 / 单条 32KB）。**取值需你确认**——这是产品参数，不是技术常量。
- **C-2 静默挂起改回 error（独立的缺陷修复，与安全无关也该修）**：当前 `useResourceFrames.ts:228` 在 `!win || !m.callId` 时**直接 return**；`:188-189` 在 `resourceFrames.get(ev.source)` 未命中时也**直接 return**。两种情况作者页的 `await TavernHelper.generate(...)` 会**永久悬挂**且无任何反馈。改为：能回即回 `reply(false, undefined, '资源页未就绪/未授权')`，不能回时至少 `logTavernCall` 记一条。
- **失败测试**：`TavernHelper` 相关用例——① 超限的 generate 调用收到 `ok:false` 且错误文案明确；② 未注册 entry 时不悬挂（有 error 回包或日志）。
- **验收**：新用例全绿；`render.test.ts:516-570` 的资源卡既有断言全绿（若改动了 `buildRemoteResourceHtml` 的输出结构，需按新语义更新，并在提交信息中说明）。

**若 R1 选 B**：把 §3.1 的实现改为在 `hydrate` 前置检查 `renderHtml` 与 `currentScriptAuthorized`，并把该语义与 `docs/功能-变更史.md:1106` 的 E.1（L12 脚本桥授权）合并口径登记；同时必须在 `docs/功能.md` 显著位置说明"资源界面需要开启 HTML 渲染并授权"，因为这会让部分既有卡**直接失效**。

### 3.2 `sandbox/sanitize.ts` 的 `style` 属性补声明级清洗

- **现象**：`web/src/sandbox/sanitize.ts:16` 的白名单把 `style` 放进 `'*'`，而该文件**没有 `transformTags`**（`grep` 零命中）；静态通道 `web/src/render.ts:206-211` **有**（调 `sanitizeStyleAttribute`）。实测：`<div style="behavior:url(x.htc);-moz-binding:url(https://e/x)">` 在运行期通道**原样保留**。
- **影响**：同一份作者 CSS，写在 `<style>` 里被声明级清洗（`:33-39` 的注释说明了这条路径），写在 `style=""` 里不清洗 → **两通道口径不一致**。可写入 `background:url(https://…)` 做网络信标、`position:fixed` 覆盖宿主 UI，且**无长度上限**。Chromium 下 `expression`/`behavior` 无效，故不构成 XSS（这点要在测试注释里写清，避免被误判为高危）。
- **失败测试**：`web/src/sandbox/sanitize.test.ts`（已存在，强行为断言风格）——① `style` 值含 `behavior:`/`-moz-binding:`/`expression(` 时被剥除；② 合法的 `position:fixed`/`color:red` 保留（**这是保真基线，不能被顺手收紧**，见 `cssSanitize.ts:9-17` 的"保真优先"说明）；③ `style` 值超长（如 > 4KB）被截断/拒绝。
- **最小实现**：给 `SCRIPT_HTML_WHITELIST` 补 `transformTags`，复用 `sanitizeStyleAttribute`（与 `render.ts:206-211` 同一实现，**不要再抄一份**——仓库对"同一逻辑两份实现"有明确纪律，`api/stream.ts:3-8` 记录了收口前的重复清单）。
- **验收**：新断言全绿；`sandbox/sanitize.test.ts`、`render.test.ts` 全绿（后者覆盖静态通道，确认未回归）。

### 3.3 `cssSanitize` 作用域选择器可越出容器

- **现象**：`web/src/cssSanitize.ts:148-153` 的 `scopeSelector` 对以组合器开头的选择器只做前缀拼接，实测：
  - `~ .sv-msg` → `[data-kd-scope="x"] ~ .sv-msg`
  - `+ div` → `[data-kd-scope="x"] + div`
  - `> .sv-inputbar` → `[data-kd-scope="x"] > .sv-inputbar`
  即卡片 CSS 可选中并改写**作用域容器之外**的任意元素（聊天区、输入栏、设置弹窗的祖先/兄弟）。
- **影响**：条件为"用户对该卡开启 HTML 渲染"（逐卡主动授权）。配合白名单放行的 `position:fixed`/`z-index` 可做界面遮挡与点击劫持（例：盖住"停止生成"）。已超出"卡片样式只影响自己"的隐含承诺。
- **失败测试**：新增 `web/src/cssSanitize.test.ts`（**该文件目前零测试**，属报告 §6.2 列出的安全关键缺口；放在 `web/src/` 顶层是安全的——根目录测试文件不受 `arch-layers.json` 规则 I 约束，已确认）——① 以 `>`/`+`/`~`/`||` 开头的选择器被拒绝或改写成带 `:is()` 的受限形式；② 合法选择器（`.a .b`、`body.theme .card`、`[data-x] > .y`）行为与现在**逐字节一致**（`body` 前导重写逻辑不能回归——它支撑 wuwa 状态栏 35 处用法，见 `:143-147` 注释）；③ `@media`/`@supports` 内部规则同样受限。
- **最小实现**：在 `scopeSelector` 入口拒绝/改写以组合器开头的选择器。**建议拒绝**（`return ''` 并跳过该规则）而非改写：改写会引入与作者原意不符的语义，且需要更复杂的正确性论证；拒绝的可见后果仅是"该条样式不生效"，与"只有文字没有界面"的既有容忍度一致。
- **验收**：新断言全绿；`render.test.ts` 中所有 CSS 作用域/`@keyframes` 相关断言全绿（这些是 `scopeCss` 的既有回归网）。

### 3.4（可选，同文件顺手）`image-set()` 的 URL 未经白名单

- **现象**：`cssSanitize.ts:82-90` 的 `cssUrlsSafe` 只逐条检查 `url()`，未覆盖 `image-set()`/`-webkit-image-set()`。实测 `background:image-set("http://e/x")` **被保留**。
- **影响**：仅产生网络请求（图片上下文不执行脚本），后果有限；但与白名单口径不一致。
- **最小实现**：把检查扩展为"值内**所有** `image-set(`/`-webkit-image-set(` 的字符串实参都必须通过 https/data:image 校验"。
- **失败测试**：并入 §3.3 的新测试文件。
- **取舍**：若 §3.3 已做完而时间紧，此项可延后——它是 P3 级，不影响 §3.3 的验收。

---

## 4. 提交 3：门禁与债务收口

### 4.1 `TaskEvent` 载荷字段纳入契约校验（**需先扩展校验器**）

- **现状（已复核）**：`TaskEvent` 在 Rust 侧不是独立 struct，而是 **`enum SseEvent` 的变体** `Task { task_id, kind, title, status, detail, finish_reason, phase, step_index }`（`server-rs/src/models/types.rs:684-714`）；TS 侧是 `type TaskEvent = { type:'task'; ... }`（`web/src/api/types.ts:97-114`）。
- **为什么现在守不住**：`tools/check-contract.mjs` 的 `tagged-union` 分支（`:374-388`）**只比对变体名**，`rustFields`（`:180-250`）**只解析 `pub struct`**。因此后端改 `phase`→`stage` 时：`TaskEventKind` 检查全绿、`vue-tsc` 全绿、前端**静默读到 `undefined`**——而 `phase`/`step_index` 正是 `stores/task.ts:199-213` 流式缓冲 key 的组成，漏字段会让缓冲全落进 `"undefined:undefined"` 桶。
- **改动点（两处）**：
  1. `tools/check-contract.mjs`：新增"**enum 变体字段提取**"能力，并把 `TaskEvent` 登记进 `MAPPINGS`（`rust` 指向 `SseEvent` + 变体名 `Task`，`ts` 指向 `types.ts` 的 `TaskEvent` 且用 **interface/type 的字段集合**口径——注意 TS 侧的 `type:` 判别字段要列入 `tsIgnore`，它对应 Rust 的变体名而非字段）。
  2. 判定方向沿用既有规则（后端有前端缺 = FAIL；前端有后端无 = FAIL；后端可选前端非可选 = WARN）。
- **验收（含负向验证，这是本项的关键证据）**：
  1. 正跑：`node tools/check-contract.mjs --verbose` → `[OK] 任务事件载荷字段:8 个字段对齐`（示例文案），退出 0；
  2. **负向验证**：临时把 Rust 侧 `phase: Option<String>` 改名为 `stage`，跑校验器 → **必须 FAIL 且点名 `phase`/`stage`**；改回后复跑 → 退出 0。**这一步的输出要粘进提交正文**，否则无法证明新守卫真的生效（仓库对"门禁是否真在跑"有历史教训，见 `tools/check-bundle.mjs:55-59`）。
  3. `node tools/check-arch.mjs` 与 `node tools/check-doc-claims.mjs` 全绿——后者派生 `MAPPINGS` 组数（`check-doc-claims.mjs:5-12` 明确列了它），**登记新映射后文档里的"MAPPINGS N 组"必须同步**，否则该门禁会红。这是本项最容易漏的一步。

### 4.2 体积预算纳入 `pre-push` 路径

- **现状（已复核）**：`tools/check-all.ps1:255-261` 的 `if (-not $Quick)` 同时包住 `vite build` 与 `check-bundle.mjs`；而 `tools/hooks/pre-push:34` 传的正是 `-Quick`；`.github/workflows/ci.yml:10-11` 自述"本机当前未配置 git 远端"。三者叠加 → **体积预算当前没有任何自动执行路径**。
- **改动点（建议按此顺序）**：
  1. 让 `-Quick` **保留** `vite build` + `bundle budget`（把它们移出 `if (-not $Quick)`），并新引入一个更快的档（如 `-Minimal`）给"只想跑 lint+test"的场景；
  2. 若 §4.2.1 让 pre-push 变得太慢，则改为"`-Quick` 跳过 build 但**打印醒目提示**说明体积未被验证"，并把这行提示写进 hook 输出。
- **验收**：
  - `powershell -File tools/check-all.ps1 -Quick` 跑完后，输出中**必须出现** `web: bundle budget` 阶段行与余量数字；
  - 故意把 `web/dist` 改名或删除后重跑 → **必须 FAIL**（验证 fail-closed 前置真的生效：`check-bundle.mjs:102-106`）；
  - 恢复 `web/dist` 后 `npm run build -w web` 再跑 → 通过。
- **风险**：pre-push 变慢约 = 一次 `vite build`（实测 **5.34s**，见报告 §二）+ 一次 gzip 统计。代价极小，**建议直接做**。

### 4.3 零测试文件 ratchet（零依赖替代覆盖率）

- **为什么不用覆盖率**：`@vitest/coverage-v8` 需装新依赖，本机网络受限（§0.3）。而且报告 §6.4-2 指出的更基础的问题是"数量快照锁不住测试有效性"。
- **改动点**：新增 `tools/check-frontend-untested.mjs`（零依赖，风格对齐 `check-frontend-lint.mjs`）：
  - 扫描 `web/src/**/*.{ts,vue}`（排除 `*.test.ts`），对每个文件求"同目录同名 `.test.ts` 或**显式登记**的测试归属"；
  - 输出**当前零测试文件清单与总数**，与脚本内的 `BASELINE`（初始值 = 本报告 §6.2 的实测清单，10 个）比对：**超过基线即 FAIL**（新增无人测试的文件要显式登记或写测试），低于基线则提示"可下调基线"；
  - 纯类型/纯常量文件用**显式豁免表**排除（`api/types.ts`、`api/labels.ts`、`sandbox/protocol.ts`），并在脚本头注释写明豁免理由。
- **接入**：`tools/check-all.ps1` 的前端段（与 `check-frontend-lint` 相邻），以及 `package.json` 的 `check:*` 系列。
- **验收**：① 首次跑输出 10 个零测试文件、退出 0；② 临时新建一个无测试的 `web/src/_probe.ts` → **必须 FAIL 并点名它**；删除后复跑 → 退出 0（负向验证，同 §4.1 的口径）。
- **不做**：不追求覆盖率百分比，不设阈值线——那需要真覆盖率工具（网络阻塞）。

### 4.4 `style.css` 纪律计数进门禁

- **现状（已复核）**：`style.css:32` 的注释写"`!important` 存量 11 处"，`grep -c` 实测 **13** 处。同类还有 `TaskBoard.vue:1148` 的 `var(--sv-text-dim, #999)`——`--sv-text-dim` 在 `style.css` 中**不存在**（`grep -c` = 0），永远落到硬编码 `#999`；`:1160` 的 `var(--sv-accent, #4ea1ff)` 里 `--sv-accent` 指向粉色（`style.css:80`），蓝色回退值掩盖了作者本意。
- **改动点（三小项，一并提交）**：
  1. **修死令牌**：`TaskBoard.vue:1148` 改用真实存在的令牌（建议 `--sv-ink-faint` 或按视觉取最接近者），`:1160` 显式改用蓝色令牌而非靠回退值掩盖；顺带全仓 grep `var(--sv-` 与 `style.css` 的 `:root` 定义做一次差集，把同类死令牌一次清掉。
  2. **`!important` 计数进门禁**：并入 §4.3 的脚本或 `check-frontend-lint.mjs`，把"存量 N 处"从注释变成断言（基线 13，**只降不升**，与既有 ratchet 同一体例）。
  3. **注释与事实对齐**：`style.css:31` 的"11 处"改写为引用门禁而非复写数字（仓库已有此体例：`count-tests.mjs` 的失败文案就建议"改为引用而不复写"）。
- **验收**：① 死令牌差集为空；② 门禁脚本正跑退出 0、临时新增一处 `!important` 后 FAIL（负向验证）；③ `npm run build -w web` 后**视觉无变化**（本项只改令牌名与门禁，不改视觉；若 `--sv-ink-faint` 与 `#999` 视觉差异明显，需在提交信息中说明并附对比）。

---

## 5. 横切：每批都必须做的收尾

### 5.1 测试计数同步（**每批都会触发，最易漏**）

新增测试文件与用例会改变静态计数，而 `count-tests.mjs --check` 是**硬门禁**（`check-all.ps1:235`）且是**反规避式**的（未匹配到句式也 FAIL，`count-tests.mjs:147-148`）。流程：

1. 跑 `node tools/count-tests.mjs`（不加 `--check`）取**新数字**；
2. 更新**两处**文档：`MAINTENANCE.md`（§2 与 §11，`count-tests.mjs` 会逐行扫描这两个位置）与根 `README.md`；
3. 跑 `node tools/count-tests.mjs --check` 确认退出 0；
4. **不要**把新数字写进其他文档（会多出未受守护的复写处）。

参考基线（2026-09-26 实测）：**113 文件 / 1192 静态用例**（运行时 1210，差值 18 来自 `mvu/parser.contract.test.ts:67-73` 的 fixture 循环生成，该说明已在文档中，**新数字仍要保留这条说明**）。

### 5.2 文档更新清单（按改动性质取用）

| 文档 | 何时更新 |
|---|---|
| `docs/功能.md` | 有**行为**变化时（§3.1 资源卡门槛必改；§2.2 图片预览回收若用户可感则补一句） |
| `docs/功能-变更史.md` | **每批都要**：按既有体例写"改了什么 / 为何这么改 / 验证证据（实跑命令与结果）" |
| `MAINTENANCE.md` | 测试计数（§5.1）；若新增门禁脚本，补进门禁清单 |
| `README.md` | 测试计数（§5.1） |
| `docs/遗留.md` | §3.1 若落地：新增"T1-b 资源卡触发门槛"条目（沿用 T1 的"现状行为/判定/影响面/归宿/来源"结构），并注明与 T1 的关系；**不要修改或删除 T1 原文** |
| `docs/计划.md` | §3.1 若落地：翻转 3.6/§七 T1 的"状态"字段（或加一行迁移注记）；§4.1 若扩展了校验器能力，补进对应工程化条目 |
| `docs/经验.md` | 出现**可复用的教训**时才写。本计划至少有一条候选：**"`await` 之后必须校验请求键"**（4 处同构缺口，且同文件内已有正例——这种"同一文件里一半有守卫一半没有"的形态值得固化成检查项）；另一条候选是 **"同一平台事实在两处被写成相反的假设"**（WebView2 `event.source`，报告附录 B-1） |
| `tools/arch-layers.json` | 新增 `web/src` 顶层**非测试**源文件时（规则 I） |
| `docs/契约-架构与数据.md` | 新增前端模块或层级归属变化时（与上一条配套） |

### 5.3 体积预算 ratchet 协议（改完必查）

修复本身会加代码，且首屏余量只剩 **4.5%**（12,830 gz）。流程：

1. `npm run build -w web && node tools/check-bundle.mjs`；
2. 若超预算：**先量实测值确认增长有正当理由**，再上调到"实测值 + 10%"，在 `tools/check-bundle.mjs:80-85` 的预算表登记并**写明理由**；若增长无正当理由，**改代码而不是调预算**；
3. **禁止**删除检查项或把预算调到明显失效的量级（`check-bundle.mjs:34-36` 的明文纪律）；
4. 特别留意 `AgentFlowSection.js`（余 **2.9%**，`check-bundle.mjs:88-93` 已预警）与 `index.css`（余 7.0%）——本计划**不动** `AgentFlowSection`，若它因共享模块变动而涨，要在提交信息里说明原因。

### 5.4 类型逃逸 ratchet（改完必查）

`node tools/check-frontend-lint.mjs` **五项必须仍等于基线**（无余量）。新增测试构造 fixture 时最容易踩，写测试前先看基线数字。

### 5.5 提交体例（AGENTS.md 明文）

- Conventional Commits 前缀 + 中文 scope 与描述；
- 正文三件事：**改了什么 / 为何这么改 / 验证证据（实际跑过的命令与结果）**；
- **一个逻辑批次一个提交**，不 `git add -A` 混多线；
- 不提交构建产物与 sidecar；仓库未配置远端，推送前先确认分支。

---

## 6. 验证矩阵（每批收尾统一执行）

| 命令 | 期望 |
|---|---|
| `npm test -w web` | 全绿；用例数 = 更新后的文档数字（运行时口径） |
| `npm run typecheck -w web` | 0 error |
| `npm run lint -w web` | 0 error（warn 允许，当前 49 条） |
| `npm run build -w web` | 成功；记录 chunk 体积变化 |
| `node tools/check-bundle.mjs` | `[OK]`，余量数字记入提交正文 |
| `node tools/check-frontend-lint.mjs` | 五项 = 基线，`[OK] 未超基线` |
| `node tools/check-arch.mjs` | 前端 A/B/I/J 无违规；`store 循环依赖:0`；新增项不得增加债务条数 |
| `node tools/check-contract.mjs --verbose` | `[OK]`；§4.1 后含新映射的 `[OK]` 行 |
| `node tools/count-tests.mjs --check` | `[OK] 与 MAINTENANCE.md / README.md 记录一致` |
| `node tools/check-docs.mjs` / `check-doc-claims.mjs` | 退出 0（§4.1 后尤其要看 doc-claims） |
| （§4.3 后）`node tools/check-frontend-untested.mjs` | 退出 0 |
| （§4.2 后）`powershell -File tools/check-all.ps1 -Quick` | 输出中含 `web: bundle budget` 阶段 |

---

## 7. 执行顺序与依赖

```
提交 1（正确性，4 项）          ← 可立即开始，无外部依赖
  └─ 2.1 守卫 ×4  →  2.2 blob URL  →  2.3 resourceStore  →  2.4 renderCache
提交 3（门禁，4 项）            ← 与提交 1、2 无依赖，可并行/提前
  └─ 4.2 体积预算路径（最小改动、收益最高，建议最先做）
     4.4 死令牌修复（顺手）
     4.3 零测试 ratchet
     4.1 TaskEvent 契约（依赖扩展校验器，最重）
提交 2（安全，3+1 项）          ← §3.2/§3.3 可立即做；§3.1 等 R1 裁决
  └─ 3.3 CSS 作用域越界（+3.4 image-set）
     3.2 style 属性对齐
     3.1 资源卡门槛（A+C 默认；B 需明确同意）
```

**建议实际执行顺序**：`4.2 → 提交 1 → 3.3+3.2 → 4.4+4.3 → 3.1 → 4.1`。
理由：先把"验证手段"修好（4.2 让体积预算真的会被跑），再做修复；`3.1` 与 `4.1` 分别依赖裁决与校验器扩展，放最后。

---

## 8. 风险与回滚

| 项 | 风险 | 回滚 |
|---|---|---|
| 2.1 守卫 ×4 | **中**：守卫若误判会"该刷新时不刷新"（表现为切回会话/任务时面板不更新）。已逐个复核 10 + 10 处调用方的语义，但**每条守卫都必须配一条"正常路径仍然刷新"的用例**，不只测竞态 | 单处 revert 即可，四处互不影响 |
| 2.2 blob URL | 低：只影响预览显示 | revert |
| 2.3 resourceStore | **中**：合并优先级写错会让刚下载的资源被旧值覆盖（比现状更糟） | revert；测试里必须有"新旧键冲突时新值胜出"的断言 |
| 2.4 renderCache | **中**：两条路径入参不一致会导致流式结束前后 HTML 跳变 | revert |
| 3.1 A | 中：用户可感的交互变化（每域名一次确认） | revert `hydrate` 前置检查 |
| 3.1 B | **高**：会让既有卡**直接失效** | 需明确同意后才做；revert |
| 3.1 C-1 | 中：节流参数若偏紧会打断正常卡 | 参数集中为具名常量（对齐 `stores/task.ts:52-62` 的体例），便于调整 |
| 3.2 / 3.3 / 3.4 | 中：清洗收紧可能让既有卡"只有文字没有界面"（仓库有此类历史事故） | 保真基线用例（合法 CSS 逐字节不变）必须先绿；revert |
| 4.1 | 中：改校验器可能影响既有 21 组映射 | 正跑 + 负向验证 + 全量门禁；revert |
| 4.2 | 低 | revert hook/参数 |
| 4.3 / 4.4 | 低 | 基线可下调；revert |

---

## 9. 本计划不覆盖（移交后续）

1. **报告 §五 的其余 P2**：`genSettings` 51 行重复、`characterDetails` 缓存不失效、22/29 模块无形状闸门、`contextTokens` 双权威、`scriptRunner` 强引用键、`RenderPanelHost` 监听累积、`utf8Bytes` 双份序列化、`contentSignature` 全量 stringify、`isWithinRowWindow`、`api/client` 的 `undefined as T` 与英文解析器报错——**均未列入三批**。建议按报告 §七 顺序另立一批（"P2 收口"），其中"22/29 无形状闸门"与"`api/client` 错误包装"应优先（改动小、收益直接）。
2. **弹窗可访问性专项**（ESC / 焦点陷阱 / `role="dialog"` / `aria-labelledby`）。
3. **`TaskBoard.vue` 拆分专项**（顺带压缩 `index.js`，缓解体积预算压力）。
4. **前端性能测量手段**（`perf-baseline.mjs` 目前只有 2 个后端端点；需要 FCP/交互延迟的采集才能支撑 P2 性能项）。
5. **覆盖率工具链**（需在有网环境安装 `@vitest/coverage-v8`）。
6. **报告附录 B 的 5 项待验证**（尤其 B-1 的 WebView2 `event.source`——需桌面 exe 实测）。

---

*计划制定时间：2026-09-26；依据报告 [`FRONTEND-REPORT.md`](./FRONTEND-REPORT.md)；对应 HEAD `d215c69`。*
