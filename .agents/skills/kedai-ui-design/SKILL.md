---
name: kedai-ui-design
description: Kedai 界面设计与视觉改动的专用入口。当任务涉及改界面观感、做新页面/新组件、调排版与配色、带动效、判断「这个设计是否该按通用审美建议改」时使用。它把通用设计技能（design-taste / tufte / motion-design / canvas-design）接到本仓库的真实约束上：既有主题 token 体系、样式纪律 ratchet、性能禁区、资源界面的知情接受裁决、以及 UI 改动的取证方式。触发词：界面、观感、样式、排版、配色、主题、动效、组件设计、UI 美化、截图、视觉评审。
---

# Kedai 界面设计：通用审美要服从既有裁决

## 一、先认清一件事

通用设计技能（`design-taste`、`tufte`、`motion-design`、`canvas-design`）给的是**审美方向**。但本仓库的界面已经有一套成型体系与若干**知情接受**的裁决。直接照通用建议改，最常见的两种翻车是：

1. 推翻一次经过裁决的决定（把「有意裁剪」当漏洞修）。
2. 绕过 token 与门禁，触发样式纪律 ratchet。

**顺序永远是：先读既有裁决 → 再谈审美。**

## 二、动手前必读的三处

| 要看什么 | 在哪 |
|---|---|
| 资源界面的既有裁决 | `docs/遗留.md` 的 T1（知情接受不修）+ 变更史 E.1/L12 |
| 主题与彩窗的既有做法 | `docs/经验.md` 里彩窗主题相关条目 + `docs/plans/GLASS-AESTHETIC-PLAN-2026-11-02.md` |
| 样式纪律的四条 ratchet | `tools/check-css-discipline.mjs` 文件头注释 |

**T1 的具体内容**：2026-10-06 追注明确 R1 = A+C——外联与「新窗口打开」**保留**，收紧的只是触发门槛。把这条当新漏洞"修掉"会推翻一次经过裁决的决定，先与用户确认。

## 三、token 与样式纪律（硬的）

- 全局域 `style.css` + `styles/*.css` 的行数 **只降不升**。新组件样式一律 `<style scoped>` 或工具类。全局域只应缩小。
- `!important` 只降不升（全局域逐文件 + 组件 scoped 合计）。
- **时长必须走令牌**：`--dur-*` / `--stagger-step` / `--transition-*`。裸写 `transition: all .3s` 会被拦（`0ms`/`0s` 与 reduced-motion 的 `0.01ms` 除外）。
- **间距裸值只降不升**：`padding`/`margin`/`gap` 的非 0 裸 px 要令牌化走 `--space-*`。
- **不做全局 token 反转**（这条踩过）。改底色必须同步检查子元素文字色。

## 四、性能禁区

- **大面板实时 `backdrop-filter` 是禁区**（GLASS-1 批已认定为知情不做）。整窗环境层 + 半透明面板的方案是有意设计，不要用"更真实的模糊"去替换它。
- 只动 `transform` / `opacity`；不动 `width`/`height`/`top`/`left`。
- 新动效必须接进既有的 `prefers-reduced-motion` 与 `data-perf-tier` 开关，**不要另起一套**。
- 主题里写 `position` 会搞崩移动端抽屉。

## 五、怎么用那四个设计技能

| 要解决的问题 | 加载 | 注意 |
|---|---|---|
| 「界面看着像 AI 生成的模板」 | `design-taste` | 它的反套路规则（禁 Inter、禁纯黑、禁 AI 紫蓝）在 Kedai 已经部分内化；先看现有 token 再决定要不要改 |
| 数据展示（仪表盘、任务状态、统计卡片） | `tufte` | 先跑它的 12 问总审。Kedai 任务台的状态矩阵用得上「数据墨水比」与「小多图」 |
| 动效设计 | `motion-design` | 时长表与缓动选择直接可用，但**时长必须落成令牌** |
| 一张平面作品（海报/封面/图标构图） | `canvas-design` | 与产品 UI 无关，别用在工作台界面上 |

**四个技能都不能覆盖本仓库的门禁与裁决**——它们说"这里该加个渐变"，样式纪律可能不允许；它们说"动效要更丰富"，性能禁区可能不允许。**仓库约束优先。**

## 六、UI 改动的取证方式

界面改动**必须真渲染取证**，探针看不见视觉：

```bash
# 不重编 exe 也能看新前端
MSYS_NO_PATHCONV=1 DATA_DIR=D:/kedai-bench-run/data-<批次> PORT=30xx \
  KEDAI_WEB_DIST=web/dist server-rs/target/debug/kedai-server.exe
```

- 真实 exe 的 API 探针**要带 token**（`GET /api/bootstrap` 取 Bearer，loopback 限定）。
- 换端口 = 换 origin = localStorage 天然为空，首启类功能免手清。
- 截图超时是宿主问题，可退回 DOM 断言；必要时用确定性补证。
- **收工必杀进程**（便携版常驻 3001，别杀用户那个）。

已知的观测陷阱：

- 全屏 `mix-blend-mode` 层曾是滚动闪烁的元凶（改纯黑 + 亮度取反作 alpha）。
- IAB 宿主约 88.9% 缩放会让 3px 边框显示成 2.66667px——是假象，不是 CSS 错。
- `rAF` 在某些宿主里不推进是宿主问题，不是缺陷（500ms 0 帧是判别点）。
- 注入 style 做「修前 → 修后」对比是有效的验证手段。

## 七、移动端

窄屏布局有源码级守卫 `web/src/styles/mobileLayout.test.ts`（六条）。改移动端布局会被它拦——**合法改动要同步改断言并说明理由**，不是删断言。

## 八、交付前自检

- [ ] 没推翻 `遗留.md` T1 那类知情接受裁决
- [ ] 没扩全局域 CSS 行数 / `!important` / 裸时长 / 裸间距
- [ ] 动效接了既有开关，没用大面板实时光模糊
- [ ] 真渲染取过证（截图或 DOM 断言），不是只跑了单测
- [ ] 移动端守卫过了
- [ ] 相关进程已杀

相关技能：`kedai-frontend-core`（前端落点与门禁）、`kedai-verify-real-model`（取证流程）、`design-taste` / `tufte` / `motion-design` / `canvas-design`（通用设计方法）。
