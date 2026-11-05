---
name: kedai-frontend-core
description: Kedai Vue 3 前端（web/）的落点地图与硬约束。当需要判断组件/状态/composable 该放哪里、要改任务事件刷新、要加设置字段到界面、要写样式或动效、要处理主题与移动端布局时使用。含前端分层（components 在 L3、stores 在 L2）、四条前端门禁规则、样式纪律 ratchet、SSE 事件驱动刷新链路、资源界面的既有裁决。触发词：web、前端、Vue、组件、store、Pinia、composable、样式、CSS、主题、动效、SSE 刷新、移动端。
---

# Kedai 前端：落点地图与硬约束

## 一、分层（与后端不是同一套模型）

`tools/arch-layers.json` 的 `frontend` 段是 SSOT：

| 代际 | 前端目录 | 含义 |
|---|---|---|
| **L1** | `api` `contracts` `mvu` `utils` `visual` `styles` | 纯契约与纯逻辑 |
| **L2** | `stores` `theme` `composables` | 状态编排 |
| **L3** | `components` `sandbox` | 展示与沙箱 |

**允许**：`L1→L1`、`L2→L1`、`L2→L2`、`L3→L1`、`L3→L2`、`L3→L3`。

**禁止**：`L2→L3`（编排不得依赖 UI/沙箱）、`L1→L2/L3`（纯逻辑不得依赖状态）。

前端是**层次模型**而不是后端的隔离模型——组件读 store、store 调 composable 是正常向下依赖，所以额外允许 `L3→L2`。真正禁止的只有上面两条。

**新增前端根文件或顶层目录必须登记进 `arch-layers.json`**，否则规则 I 直接 FAIL。

## 二、四条前端门禁（都会真的拦你）

| 规则 | 查什么 | 怎么过 |
|---|---|---|
| A | store 间顶层 import 成环 | 在 action 内运行期 `useXStore()` 是允许的；**顶层 import 成环**即 FAIL |
| B | 组件直改 store state（`store.field = ...`） | 改走 store action。存量基线 113 处，**只降不升**；确需豁免要用 `arch-allow` 并写理由 |
| type-ratchet | `as never` / `as unknown as` / 非空断言 / `any` | 只降不升，别为了过类型检查加断言 |
| test-ratchet | 新增源码文件没有测试 import | 补测试；零测试文件是显式清单（只减不增） |

`check-frontend-lint.mjs` 同时承担 eslint 之外的类型逃逸检查——**`vitest` 不做类型检查**，类型错只有 `vue-tsc`（`npm run build -w web`）会抓。

## 三、样式纪律（改了很容易踩）

`check-css-discipline.mjs` 有四条 ratchet，全部只降不升：

1. 全局域逐文件行数（`style.css` + `styles/*.css`）——新组件样式一律走 `<style scoped>` 或工具类，**全局域只应缩小**。
2. `!important`（全局域逐文件 + 组件 scoped 合计）——只降不升。
3. **裸时长归零**——`animation` / `transition` 的时长必须走令牌（`--dur-*` / `--stagger-step` / `--transition-*`）。`0ms`/`0s` 与 `reduced-motion` 的 `0.01ms` 属归零语义，白名单放行。
4. **间距裸值 ratchet**——`padding`/`margin`/`gap` 里的非 0 裸 px 只降不升，可令牌化的走 `--space-*`。

改文件行数基线的合法路径是**下调**；确需上调（如批次有意扩张全局域）要在提交信息里给理由并同步改脚本。这条和 `check-bundle` 的预算纪律同源。

**文案标点**（`check-text-punct.mjs`）：`.vue` 的 `<template>` 内、标签外、`{{ }}` 之外的文本节点里，**汉字后不得跟半角 `, ; : ? !`**。基线 0，一次清零防回流。注意它只扫模板文本节点 + 两张登记文案表（`api/labels.ts`、`utils/tutorialCopy.ts`）——**经 `{{TABLE[x]}}` 渲染的常量表会漏**，那类文案要人工核。

**移动端布局守卫**在 `styles/mobileLayout.test.ts`（与样式同域），不在组件目录。

## 四、SSE 事件驱动刷新

`web/src/stores/task.ts` 是任务事件的消费端，**无轮询**：断线指数退避重连 + 5s 兜底轮询。

链路：后端 `TaskService` 广播 → `GET /api/tasks/events` → 前端 store 按事件 kind 精确刷新。

**新增事件 kind 的四处同步**（少一处就是静默不刷新）：后端枚举 `TaskEventKind` / 发射点 / `web/src/api/types/` 的 TS union / `stores/task.ts` 的刷新映射。最后一项由 `never` 穷尽断言兜底——**只有 TS union 加了而映射没加，`vue-tsc` 才会报错**，所以务必跑一次 build。

测试注意：broadcast 压力下会丢帧，断言写「SSE ∪ REST 补拉」的并集。

## 五、设置字段进界面

新增设置字段除了后端登记，前端还必须：补 `web/src/stores/genSettings.test.ts` 的夹具。**只有 `vue-tsc` 会抓漏**，`vitest` 不会。

`api/types.ts` 现在只是门面（`export * from './types/index'`），实际按域拆在 `api/types/` 下。改线格式时去对应域文件，并同步 `tools/check-contract.mjs` 的映射。

## 六、资源界面：先读既有裁决，别当新漏洞推翻

动 `resource-frame` / `TavernHelper` / `useResourceFrames` 之前，**先读 `docs/遗留.md` 的 T1**（知情接受不修）与变更史 E.1/L12。2026-10-06 的追注明确 R1 = A+C：外联与「新窗口打开」**保留**，收紧的只是触发门槛。把这条当漏洞"修掉"会推翻一次经过裁决的决定。

## 七、主题与动效

- 主题 token 体系已经存在（含「石室与光」彩窗主题）。**不做全局 token 反转**——改底色必须同步子元素文字色（这条踩过）。主题里写 `position` 会搞崩移动端抽屉。
- 动效只动 `transform` / `opacity`，不动 `width`/`height`/`top`/`left`。
- **大面板实时 `backdrop-filter` 是性能禁区**（已在 GLASS-1 批知情不做）。
- 已有 `prefers-reduced-motion` 与 `data-perf-tier` 的既定做法，新动效要接进同一套开关，别另起。
- `TransitionGroup` 有三个坑（经验 E73）：`move` 被行内动画顶掉、scoped 覆盖过渡类、flex abspos 静态位置。

## 八、移动端与观感实测

窄屏布局有源码级守卫 `styles/mobileLayout.test.ts`（六条），改移动端布局时会被它拦——**合法改动要同步改断言**，不是删断言。

做观感实测不必重编 exe：`PORT` + 隔离 `DATA_DIR` + `KEDAI_WEB_DIST=web\dist` 三件套。真实 exe 的 API 探针要带 token（`GET /api/bootstrap` 取 Bearer，loopback 限定）。换端口 = 换 origin = localStorage 天然为空，首启类功能免手清。**收工必杀进程**（便携版常驻 3001）。

渲染相关：`rAF` 在某些宿主里不推进是**宿主假象**不是缺陷（500ms 0 帧是判别点）；注入 style 做「修前→修后」试改是有效的验证手段。

## 九、构建产物

`npm run build -w web` 产出 `web/dist`，会被编译进 exe。日常改代码用 `npm run watch`（增量，保留 target，不跑门禁）；**版本号更新后 / 交付前必须 `.\build.ps1`**——`watch` 不刷新 `dist\`、便携版与 sidecar 指纹。

相关技能：`kedai-batch-dev`（流程）、`kedai-gate-runner`（门禁）、`kedai-backend-core`（后端对应面）。
