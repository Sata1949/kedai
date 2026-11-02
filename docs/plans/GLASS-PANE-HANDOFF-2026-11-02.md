# 拼接玻璃 UI 侦查与交接（2026-11-02）

> **⚠ 本稿大部分内容已被 GLASS-PANE-1 批推翻或落实，读前先看这段。**
> 2026-11-02 晚的 GLASS-PANE-1 批按本稿开工，但**本稿推荐的 48 块随机多边形拼片实测不可用**
> （读作碎玻璃／迷彩），最终改成「石棂骨架 + 菱形小片」，手法也从「盖暗罩」改成
> `background-blend-mode: multiply`。此外本稿建议的三种空态装饰方案均被用户否掉。
> **现接口径以 `docs/计划.md` 的「落地记录之二：拼接玻璃」与 `docs/功能-变更史.md`
> 的 `2026-11-02-glass-pane-1` 一章为准**；本稿保留仅作侦查过程与失败记录。
> 仍未做：纹理配色细调、低端设备光栅化取证、任务台空态焦点（见 `docs/遗留.md` §三十六）。

> **给接手的人**：这份文档回答一个问题——**「多块玻璃拼接组成 UI」到底该怎么在 Kedai 里实现**。
> 结论来自**真实启动 + 真实浏览器逐屏截图 + 七轮注入式原型验证**，不是推演。
> 证据目录：`D:\kedai-bench-run\glass-aesthetic-evidence\`：
>
> | 文件 | 用途 |
> |---|---|
> | `panes-820-compact.svg` / `.uri.txt` | **48 块拼片源（推荐，7.8KB）** |
> | `panes-900.svg` / `panes.css.txt` | 88 块高密度版（22.9KB，备用） |
> | `shots/A1-current-empty.png` | 现状：无拼接 |
> | `shots/A3-OLD-version.png` | 旧版：壁纸式拼片 |
> | `shots/A4~A8-PROTOTYPE-*.png` | 六轮原型（含三次失败记录） |
> | `shots/A10-compact48.png` | **第 ⑦ 轮：拼片用在装饰处（推荐效果）** |
> | `shots/A2-current-settings.png` | 现状设置页（同样无拼接） |
> | `shots/COMPARE-pane-investigation.png` | 七轮对照拼图 |
> | `old-glass-backup.css` | 旧版主题全文（备查 11 处 `--kg-mosaic` 用法） |

---

## 〇、一句话结论

**你要的效果现在没有。** 当前主题（`7a01329` 之后的「石室与光」）里**拼接玻璃元素为零**——
实测全界面 8 个带背景层的元素，`pattern: false` **100%**，即没有任何重复图案或多块拼接结构。

而我上一轮重做**恰好把拼接删掉了**：旧版的 `--kg-mosaic`（384×176 可平铺菱形拼片）被整条移除，
连 11 处引用一起。旧版有拼接感但那是「壁纸」，新版质感好但丢了拼接——**两头都没做对**。

**正确方向已由六轮原型验证**（见 §四）：**拼片不能当背景铺，只能当作"一块玻璃"用在装饰性/结构性元素上**。

---

## 一、实测证据：现在到底有没有拼接玻璃

### 1.1 全界面扫描（1440×900，隔离实例 3071，release 产物）

| 元素 | 尺寸 | 背景层数 | 是否含 pattern |
|---|---|---|---|
| `html` / `.sv-frame-col` | 1440×900 | 12 | ❌ |
| `.sv-ambient`（WebGL 层） | 1440×900 | 7 | ❌ |
| `.sv-sidebar` | 296×805 | 9 | ❌ |
| `.sv-main` | 1050×805 | 15 | ❌ |
| `.sv-char-item.active` | 268×61 | 5 | ❌ |
| `.sv-btn.primary` ×2 | — | 5 | ❌ |

**`pattern: true` 的数量 = 0。** 所谓"玻璃感"全靠 `radial-gradient` 渐变 + `inset` 描边 + 外投影堆出来的。

### 1.2 肉眼可见的全部"玻璃相关"元素

把当前界面（`shots/A1-current-empty.png`）拆开看，跟拼接/彩窗沾边的只有三处：

1. **面板顶部 1~3px 彩色细带**（`::before` 的 `repeating-linear-gradient`）——太细，读作装饰线
2. **右侧 108px 玫瑰窗小图标**（`--kg-rose`）——一个图标，不是界面结构
3. **选中态按钮/角色卡**——纯色块 + 单层渐变

面板本体是一整块**素净暖象牙平面**。这就是"没有拼接玻璃相关 UI"的全部原因。

---

## 二、旧版 vs 新版：为什么两头都不对

### 2.1 旧版（`c2313a5` 及之前）：有拼接，但是"壁纸"

`--kg-mosaic` 是一张 **384×176 的可平铺瓦片**，内含 **300 个 polygon / 23 种颜色 / 菱形固定 48×44**，
在 1440×900 下**重复约 6.9 次**。它被用在 **11 处**（外壳、顶栏、侧栏头、模态头、气泡、角色卡、按钮…）。

实测旧版渲染（`shots/A3-OLD-version.png`）：
- 外壳有大片彩色菱形网格，角色卡也是彩色拼片 —— **确实"有拼接"**
- 但它是**均匀重复的**，且铺在**外壳**上、面板透出它 —— 读作「印花纹的壁纸」
- 面板仍是大面积浅色（亮部 70.7%），所以整体又吵又不像窗

**判据**：真实彩窗的拼片**不规则、大小悬殊、紧密咬合、从不重复**。均匀平铺 → 必然读作壁纸。

### 2.2 新版（`7a01329`「石室与光」）：质感对了，拼接没了

见 §一。面板变成一整块发光玻璃，可读性与护眼指标都很好（眩光 12.5%→0.3%），
**但"多块玻璃拼接"这个核心视觉被彻底删除**。

### 2.3 两版的共同问题

**都没有把"玻璃拼接"当作 UI 的构成语法。**
- 旧版：把拼片当**背景纹理**（装饰层）
- 新版：把拼片当**噪音删掉**（无）
- 你要的：拼片是**界面元素本身的材质**（结构层）

---

## 三、关键设计判断（六轮原型试出来的）

### 3.1 拼片不能当全屏背景——这是硬约束

原型 3（`shots/A6-PROTOTYPE-v3-svg.png`）用真正不规则的 SVG 拼片铺满主面板，效果是：

✅ **"多块玻璃拼接"的观感完全成立**——不规则多边形、粗细不一的铅条、大小悬殊的玻璃块
❌ **文字被铅条穿过，可读性崩溃**

这不是调参能解决的：**只要拼片是背景，文字就必然跨越铅条**。真实教堂里没人把经文印在窗上，
文字是刻在石头上或单独的面板上。

### 3.2 正确解法：拼片与文字必须分层

| 层级 | 材质 | 文字 | 例子 |
|---|---|---|---|
| **装饰层** | 拼片玻璃（可以有铅条） | ❌ 不放文字 | 顶栏横楣、面板外框、空态背景块 |
| **内容层** | 整块素玻璃（无铅条） | ✅ 文字在此 | 主面板正文、侧栏列表、输入区 |
| **强调层** | 带拼片的单色玻璃（低对比纹理） | ✅ 文字+柔光垫 | 选中态角色卡、主按钮 |

原型 5（`shots/A8-PROTOTYPE-v5-earned.png`）验证了这个分层：
顶栏变成一条真正的**横向拼片玻璃窗楣**，角色卡与主按钮是**嵌着拼片的宝石玻璃**，
内容区仍清晰可读——**这是目前最接近你要求的一版**。

### 3.3 拼片纹理的来源选择

| 方案 | 做法 | 优点 | 缺点 |
|---|---|---|---|
| **A. 生成式 SVG（推荐）** | 用脚本生成不规则多边形 SVG，作为 `background-image` 挂到目标元素 | 真正不规则、可控密度/配色；一次生成可复用 | data URI 有体积成本（见下表） |
| B. CSS conic-gradient 假拼片 | 多层 `conic-gradient` 交错 | 零资源、体积小 | 边界是直线，读作"方格纸"（原型 2 实测失败） |
| C. 复用旧 `--kg-mosaic` | 拿回旧的菱形瓦片 | 现成 | 均匀平铺 = 壁纸，且是我们已判死的方向 |

**推荐 A**。本次已生成**两份可用的拼片源**（都在证据目录）：

| 文件 | 块数 | SVG 大小 | data URI | 说明 |
|---|---|---|---|---|
| `panes-900.svg` / `panes.css.txt` | 88 | 21.8KB | 22.9KB | 密度高，适合大面板；在 300% 放大下偏碎 |
| **`panes-820-compact.svg` / `.uri.txt`** | **48** | **7.4KB** | **7.8KB** | **推荐**：密度适中、加了少量钴蓝/红宝石/金（占比 ~3%），实测顶栏效果最好 |

已验证的用法（`background-size` 放大取样、避免看出重复）：

```css
/* 顶栏：横向窗楣 */
background-image: linear-gradient(rgba(9,14,32,0.50), rgba(9,14,32,0.50)),
                  url("data:image/svg+xml,<panes-820-compact.uri.txt 内容>");
background-size: 100% 100%, 300% 620%;
background-position: 0 0, 42% 48%;

/* 角色卡/按钮：宝石玻璃（要额外压一层暗，保证文字） */
background-image: radial-gradient(ellipse 90% 120% at 40% 50%, rgba(10,22,50,0.72), rgba(10,22,50,0.50) 70%),
                  linear-gradient(rgba(8,16,38,0.30), rgba(8,16,38,0.30)),
                  url("data:image/svg+xml,...");
background-size: 100% 100%, 100% 100%, 300% 420%;
```

---

## 四、六轮原型对照（全部实测截图）

| # | 截图 | 做法 | 结论 |
|---|---|---|---|
| ① | `A3-OLD-version.png` | 旧版壁纸式拼片 | 有拼接感，但均匀重复=壁纸；面板仍浅色 |
| ② | `A1-current-empty.png` | 当前版本 | **拼接为零**（本次要修的就是它） |
| ③ | `A4-PROTOTYPE-multipane.png` | 均匀网格切分面板 | 有"格窗"感，但太机械，像方格纸 |
| ④ | `A6-PROTOTYPE-v3-svg.png` | 真不规则 SVG 铺满 | **拼接观感最好，但文字不可读** |
| ⑤ | `A7-PROTOTYPE-v4-frame.png` | 拼片做面板外框+侧栏背景 | 内容区可读，但**侧栏拼片压住文字** |
| ⑥ | `A8-PROTOTYPE-v5-earned.png` | **拼片只用在顶栏/角色卡/主按钮** | ✅ **可读性与拼接感兼得，推荐方向** |
| ⑦ | `A10-compact48.png` | 用 48 块精简版拼片重做顶栏/角色卡/按钮 | ✅ 效果更好且体积从 22.9KB→**7.8KB**；**遗留：顶栏右侧控件（AGENT/CTX）被拼片衬得难辨** |

对照拼图：`shots/COMPARE-pane-investigation.png`

### 第 ⑦ 轮的具体遗留（下一批务必处理）

顶栏铺拼片后，右侧的 **AGENT 开关、CTX/本轮计数** 落在拼片高亮处，控件边缘与背景的区分度下降。
可行的修法（未验证，供参考）：
- 给这些控件**单独加深色底**（`rgba(9,14,32,0.72)`）让它们从拼片里"浮起来"
- 或把拼片只铺在顶栏**左侧 60%**（`background-size` 改窄 + `background-position: left`），
  右侧留给控件区保持纯色
- 或提高拼片上方覆盖层的浓度（0.50 → 0.62），代价是彩色感变弱

**同类风险**：角色卡选中态叠拼片后，`.sv-char-name` 文字实测被压（原型 5）。必须给内文加柔光垫
或 `text-shadow`。这类"拼片压字"问题在每处用拼片时都要逐一验证。

---

## 五、下一步执行建议（按优先级）

### P0｜把拼片加入装饰层（低风险，见效最快）

**目标**：让界面重新有"拼接玻璃"的识别度，零可读性风险。
**已完成原型验证**（第 ⑦ 轮 `A10-compact48.png`），可直接照抄参数。

1. **顶栏 → 横向拼片窗楣**（已验证）
   ```css
   background-image: linear-gradient(rgba(9,14,32,0.50), rgba(9,14,32,0.50)),
                     url("data:image/svg+xml,<panes-820-compact>");
   background-size: 100% 100%, 300% 620%;
   background-position: 0 0, 42% 48%;
   background-repeat: no-repeat, no-repeat;
   border-bottom: 2px solid #07060c;
   ```
   ⚠️ **遗留**：右侧控件（AGENT/CTX）被拼片衬得难辨 —— 见 §四「第 ⑦ 轮的具体遗留」。

2. **角色卡选中 / 主按钮 → 带拼片的宝石玻璃**（已验证结构，字体问题待修）
   ```css
   background-image: radial-gradient(ellipse 90% 120% at 40% 50%, rgba(10,22,50,0.72), rgba(10,22,50,0.50) 70%),
                     linear-gradient(rgba(8,16,38,0.30), rgba(8,16,38,0.30)),
                     url("data:image/svg+xml,<panes-820-compact>");
   background-size: 100% 100%, 100% 100%, 300% 420%;
   ```
   ⚠️ **必须**给内文加柔光垫或 `text-shadow`（原型 5 实测文字被压）。

3. **空态 → 在玫瑰窗对面加一块纯装饰拼片玻璃**
   无文字，纯粹承担"这是一扇彩窗"的识别。**这项尚未做原型，是本批最安全的增量。**

### P1｜建立"玻璃块"的组件语法（核心）

把 §3.2 的三层模型落成语义类，而不是逐处硬编码：

```
.sw-glass-pane      装饰层：拼片玻璃，禁放文字
.sw-glass-plain     内容层：素玻璃，文字安全
.sw-glass-jewel     强调层：单色+拼片，需柔光垫
```

然后按元素归位：顶栏/外框→`pane`，面板/列表/输入→`plain`，选中态/按钮→`jewel`。

### P2｜拼片纹理本身的调优

- **块数**：**48 块是已验证的甜点值**（`panes-820-compact`）。88 块在 300% 放大下偏碎。
- **配色**：compact 版已含少量 **#143a7c 钴蓝 / #8e1b33 红宝石 / #b08a34 金**（各 3 块，opacity 0.18~0.20）。
  实测在顶栏上彩色感足够但不吵。若要更"教堂"，可把这三色各加到 5~6 块。
- **铅条**：`stroke-width` 2.2~3.0 已验证可用（**粗细要有变化**，真实铅条不均）。
- **体积**：compact 版 **7.8KB data URI**，已可接受；主题 CSS 现 5.86KB gzip，
  加 7.8KB 后预计 ~9.5KB gzip，**仍在 12KB 预算内**。

### P3｜不要做的事（已验证失败）

- ❌ **不要用 CSS `conic-gradient` 造假拼片**——边界是直线，读作方格纸（原型 2）
- ❌ **不要把拼片铺在含文字的面板上**——必然压字（原型 3、4 都失败）
- ❌ **不要用可平铺的均匀瓦片**——重复周期一眼可见（旧版的问题）
- ❌ **不要为了拼接牺牲护眼指标**——当前「石室与光」的眩光 0.3%、对比度 0 项不达标是好底子，要保住

---

## 六、当前基线（改之前请先读）

**代码位置**：`web/src/theme/stained-glass.css`（单文件，2026-11-02 重写版）+ `web/src/theme/builtin.ts`（令牌）

**当前设计模型「石室与光」三层**：
- 层 0 石室：`#0b0910` 近黑微暖 + 三束极淡彩光 + 四角暗角
- 层 1 窗：面板 = 一块发光暖象牙玻璃（主窗 `#ece5d6` / 侧窗 `#ddd5c2`），玻璃感靠 4 层描边 + inset 聚光 + 外辉光
- 层 2 石梁：顶栏/输入区近黑 `#08060f` + 暖金边

**必须保住的指标**（改完要复测）：
| 指标 | 当前值 | 测法 |
|---|---|---|
| 眩光风险（亮度>0.94 且饱和<0.06 面积占比） | 0.3% / 0.0% / 0.0% | 截图 → PIL 统计 |
| 过亮区（亮度>0.85） | 46.7% / 38.0% / 48.4% | 同上 |
| 亮度动态范围（p99−p10） | 0.963 / 0.963 / 0.967 | 同上 |
| 对比度 <3:1 的文本节点 | **0 项** | 注入式扫描脚本（见 §八） |
| 主题 CSS 体积 | 5.86KB gzip | `gzip -c dist/assets/stained-glass-*.css \| wc -c` |

**已知坑（踩过的）**：
1. **移动端布局**：主题里给 `.sv-sidebar` 写 `position` 会打穿 `mobile.css` 的 `fixed` 抽屉
   → 主区被挤到 55px。**`position` 只在 `@media (min-width:768px)` 内声明**。
2. **暗底文字**：任何改暗的容器必须同步其**全部子元素**文字色，否则成片 <3:1
   （顶栏曾一次冒出 6 处 1.2~2.94:1）。
3. **`--kg-mosaic` 已删除**，但 `UiSection.vue` 的主题缩略图曾漏改（`var(--kg-mosaic,none)` 带兜底，
   不报错但静默失效）。加新拼片资源时**全局搜一遍有没有别处引用**。

---

## 七、怎么复跑这次侦查

```bash
# 1. 启动隔离实例（release 产物即可，不必等 debug）
MSYS_NO_PATHCONV=1 DATA_DIR=D:/kedai-bench-run/data-xxx PORT=3071 \
  LOG_DIR=D:/kedai-bench-run/xxx/logs KEDAI_WEB_DIST=web/dist \
  dist/kedai-server.exe
# 启动后先核对日志里的 "data_dir" 行

# 2. 浏览器里设主题
localStorage.setItem('kedai.theme.v1','stained-glass')
localStorage.setItem('kedai.onboarding.v1', JSON.stringify({done:true,scope:'none'}))

# 3. 宿主假象处理（必做，否则截图全是空白/中间态）
#    rAF 不推进 → 注入:
#    * { animation-duration:0.001s !important; transition:none !important; }
#    并 document.getAnimations().forEach(a => a.finish())
#    再清掉 [class*="sv-fade-"] / [class*="sv-mode-"] 的过渡类

# 4. 注入式试改（只读，不写仓库）
#    把候选 CSS 写到仓外文件，再 style.textContent = css 注入；移除即恢复
```

---

## 八、可复用的检查脚本

**拼接存在性检测**（本次用的）：

```js
// 返回界面上所有"有多层背景"的元素；pattern:true 才说明有拼接/纹理
Array.from(document.querySelectorAll('html[data-theme] *')).map(el => {
  const cs = getComputedStyle(el);
  if (cs.backgroundImage === 'none') return null;
  const layers = cs.backgroundImage.split(/,(?![^(]*\))/).length;
  const hasPattern = /repeating-|conic-gradient/.test(cs.backgroundImage);
  const r = el.getBoundingClientRect();
  if (r.width < 4 || r.height < 4) return null;
  return { cls: String(el.className).slice(0,42), w: Math.round(r.width), h: Math.round(r.height), layers, pattern: hasPattern };
}).filter(Boolean);
```

**对比度扫描**（<3:1 项必须为 0）：见 `glass-aesthetic-evidence` 里历次调用，
要点是 `effBg()` 要**向上找第一个 alpha>0.72 的祖先背景**，否则半透明层会算错。

---

## 九、一句话交接

> **现在没有拼接玻璃。** 旧版是"壁纸"，新版"什么都没有"。
> 已验证的正确做法是：**拼片是装饰层与强调层的材质，内容层必须是素玻璃**。
>
> **可直接开工的三份材料**（都在 `D:\kedai-bench-run\glass-aesthetic-evidence\`）：
> 1. `panes-820-compact.svg` / `panes-820-compact.uri.txt` —— 48 块拼片源，7.8KB，已验证效果好
> 2. `shots/A10-compact48.png` —— 顶栏/角色卡/按钮的成品参数截图
> 3. `shots/COMPARE-pane-investigation.png` —— 七轮对照，一眼看清每条路的成败
>
> **两个必踩的坑**（本文档 §四末 + §六）：
> 顶栏右侧控件被拼片衬糊、角色卡文字被拼片压暗。每处用拼片都要单独验对比度。
>
> **别丢掉的底子**：「石室与光」的护眼指标（眩光 0.3%、对比度 0 项不达标、动态范围 0.963）
> 是上一轮辛苦调出来的，加拼片时不要把面板整体压暗或提亮。
