# 主题插件与字体

Kedai 的前端主题是**声明式**扩展点：主题只能覆盖 CSS 自定义属性、根 class、`@font-face` 与主题作用域 CSS，宿主不向插件暴露 DOM 写入口。内置的“教堂拼接玻璃”与默认“纸面 · 至上主义”走同一注册表，外部插件用同一套 API 注册。

## 内置主题

| id | 名称 | 说明 |
|---|---|---|
| `default` | 纸面 · 至上主义 | 淡粉纸面、黑方块与红斜线；默认主题，无额外 CSS/字体 |
| `stained-glass` | 教堂拼接玻璃 | 暗石室中一扇从背后发光的乳白玻璃：暖象牙不透光面、石框与窗棂、拼片只镶在窗楣与选中态；主题 CSS 与字体按需加载 |

切换入口：**综合设置 → 界面与工具 → 界面 → 外观主题**。选择写入 `localStorage["kedai.theme.v1"]`，不写 `settings.json`。启动时 `main.ts` 先同步应用已保存的令牌；主题 CSS 由宿主在启动期/空闲期预载，Splash 等待 CSS 就绪（字体后台加载），避免揭幕后再二次重绘。

### 彩窗主题的视觉依据（2026-11-02「石室与光」）

参照 15 张真实教堂彩窗照片提炼，而不是把六种宝石色等量铺开：

- **沙特尔「蓝圣母」窗 / York Minster**：连白玻璃底也是许多小菱形拼成，不是平涂；彩色圆章嵌在白色菱格场里，铅条形成密网。
- **Canterbury 特写 / Cologne 人物**：每块蓝底、白袍、绿袍都由许多不规则小片拼成，片与片之间有深浅差；粗黑铅条把画面切成网。
- **圣礼拜堂**：石构近黑，玻璃自发光；彩色光会溢出到相邻玻璃和石面上。
- **Laon / 巴黎圣母院玫瑰窗**：不是 12 瓣平涂，而是三层放射扇片 + 一圈圈白环小圆章 + 很粗的石棂；蓝场为主，红金只作圆章。

落成规则（2026-11-02 重做为「石室与光」，同日 GLASS-PANE-1 补回拼接层）：

- **三层模型**：层 0 石室近黑微暖承载对比；层 1 窗是**一整块不透明暖象牙玻璃**
  （主窗 `#ece5d6` / 侧窗 `#ddd5c2`），玻璃感由 4 层石框描边 + inset 聚光 + 外辉光承担；
  层 2 石梁/窗台近黑，只留一条暖金边。
- **不再有半透明面板**：早期版本用 `rgba(250,246,232,.58)` 让拼片从背后透上来，结果面板读作
  「印着花纹的纸」。现在面板不透光，拼片改用在**装饰层**。
- **拼接玻璃的分层铁律**（`--kg-panes-band`，560×80 的横向窗楣条带）：
  参照真实彩窗的构造语法——**大尺度是刚性石棂**（等距竖棂 lancet + 上下窗框），
  **小尺度才是玻璃片**（格内错缝咬合的菱形 quarry）。只有「石棂 + 规则小片」
  才读作彩窗；纯随机多边形会读作碎玻璃/迷彩（第一版就是这么失败的）。
  - 装饰层可用：顶栏横向窗楣、空态右栏装饰带。
  - 强调层可用：角色卡选中态、主按钮——**必须先染色**（`background-blend-mode: multiply`
    与深钴蓝/酒红相乘），否则 SVG 的不透明浅底会把深底翻成浅底、白字掉到 1.x:1。
  - **内容层禁用**：主面板 / 侧栏 / 消息气泡 / 输入框 / 模态 / hub 内容区一律素玻璃。
    只要拼片是背景，文字就必然跨越铅条，可读性直接崩（七轮原型里两轮就是这么失败的）。
  - 透明感与光泽靠分层叠加而非加白：玻璃片自身 `fill-opacity` 0.6~0.7 让底色透上来，
    片内有沿斜边的高光与右下暗部，条带嵌贯穿式斜向反光，CSS 再补顶部天光与上下亮边。
  - 这条边界由 `stainedGlassCss.test.ts` 机检，新增用点要先想清属于哪一层再扩白名单。
- 玻璃主题在 App 级挂**唯一一层整窗环境层**，`ChatWindow` / `TaskBoard` 不再各挂 stage canvas；
  大面积面板不做实时 `backdrop-filter`（动画背景上的全屏模糊是卡顿源）。
- 空态右侧是一扇 108px 密窗玫瑰，下方（流内）一条 64px 拼片装饰带。
- 低档设备（`html[data-perf-tier='low']`）关闭全部实时模糊，环境层降到 20fps/0.45 倍。
- 所有规则限定在 `html[data-theme='stained-glass']` 下，不使用 `!important`。

参考照片（Wikimedia Commons，仅作研究，不随仓库分发；本机 15 张在
`D:\kedai-bench-run\stained-glass-ref\ref-01..15`）：
[Sainte-Chapelle windows](https://commons.wikimedia.org/wiki/File:Sainte-Chapelle,_Paris,_upper_chapel_windows.jpg)、
[Chartres Notre-Dame de la Belle Verrière](https://commons.wikimedia.org/wiki/File:Chartres_-_cath%C3%A9drale_-_ND_de_la_belle_verri%C3%A8re.JPG)、
[York Minster](https://commons.wikimedia.org/wiki/File:Stained_glass_window,_sXXXIII,_York_Minster_-_geograph.org.uk_-_3948899.jpg)、
[Canterbury close-up](https://commons.wikimedia.org/wiki/File:Stained_glass_window_(S.XII),_Canterbury_Cathedral_-_geograph.org.uk_-_3864990.jpg)、
[Notre-Dame north rose (interior)](https://commons.wikimedia.org/wiki/File:North_rose_window_of_Notre-Dame_de_Paris_(interior),_December_2024_(3).jpg)、
[Laon east rose](https://commons.wikimedia.org/wiki/File:Laon_Cathedral_East_Rose_Window_01.JPG)。

## `window.KedaiUI` v1

宿主在 `app.mount()` 前安装 API，挂载后派发 `kedai:ui-ready` 事件。外部插件可以监听该事件，也可以直接读取 `window.KedaiUI`。

```ts
interface ThemeDefinition {
  id: string;                 // ^[a-z][a-z0-9-]{1,63}$
  name: string;
  description?: string;
  tokens?: Record<string, string>; // 键必须是 --*，值写入 <html> 的行内样式
  className?: string;         // 单个 class，应用到 <html>
  fonts?: Array<{ family: string; src: string; weight?: string; style?: string; display?: string }>;
  css?: string;               // 请自行以 [data-theme="<id>"] 为前缀
  load?: () => Promise<unknown>; // 首次激活或宿主预载时调用一次；应只等 CSS 等关键资源
}

interface KedaiUiApi {
  readonly version: number;   // 当前为 1
  registerTheme(theme: ThemeDefinition): boolean;
  unregisterTheme(id: string): boolean;
  listThemes(): ThemeSummary[];
  getActiveTheme(): string;
  applyTheme(id: string): Promise<boolean>;
  onChange(listener: (snapshot: ThemeSnapshot) => void): () => void;
}
```

示例：

```ts
window.addEventListener('kedai:ui-ready', () => {
  window.KedaiUI.registerTheme({
    id: 'my-theme',
    name: '我的主题',
    description: '示例：只改令牌与一条作用域 CSS',
    tokens: {
      '--sv-paper': '#f4f6f5',
      '--sv-ink': '#10201c',
      '--sv-pink-deep': '#1f6b57',
    },
    className: 'my-theme',
    css: '[data-theme="my-theme"] .sv-topbar { box-shadow: 0 3px 0 0 #1f6b57; }',
  });
  void window.KedaiUI.applyTheme('my-theme');
});
```

约束与回退：

- `id` 唯一；重复、非法 id、非 `--*` 令牌会被拒绝并返回 `false`。
- 内置主题受保护，`unregisterTheme('default' | 'stained-glass')` 返回 `false`。
- 插件主题在启动时必须重新注册；若 `localStorage` 中保存的 id 尚未注册，启动回退 `default`，注册后可再次切换。
- `css` 由本地插件提供，宿主按可信代码处理；插件必须把选择器限定在 `[data-theme="<id>"]` 下，避免污染其它主题。
- API 不提供 DOM 写入口，也不执行生命周期回调；需要复杂交互时应在插件自己的脚本里完成，再通过令牌/作用域 CSS 表达。
- 主题资源只加载一次；切回默认不会卸载已加载的 CSS/字体（它们都带主题作用域）。
- 宿主内部提供 `preloadTheme(id)`：只预载资源，不改变 activeId / data-theme / 持久化；内置彩窗在
  high 档启动空闲时预载 CSS 与环境层 chunk。字体与 CSS 拆开，`font-display: swap` 接管字形回退。
- `load()` 应只等决定“主题是否完整”的 CSS；字体等非关键资源在后台加载，避免拖住主题切换。

## 字体与许可

| 字体 | 用途 | 许可 | 来源 |
|---|---|---|---|
| Cinzel | 彩窗主题的拉丁展示字 | SIL Open Font License 1.1 | [Google Fonts](https://fonts.google.com/specimen/Cinzel) / `@fontsource/cinzel` |
| Cormorant Garamond | 彩窗主题的拉丁正文 | SIL Open Font License 1.1 | [Google Fonts](https://fonts.google.com/specimen/Cormorant+Garamond) / `@fontsource/cormorant-garamond` |
| Archivo Black | 默认主题的拉丁字标/数字（既有） | SIL Open Font License 1.1 | [Google Fonts](https://fonts.google.com/specimen/Archivo+Black) / `@fontsource/archivo-black` |

彩窗主题只打包拉丁子集：`@fontsource/cinzel/latin-600.css` 与 `@fontsource/cormorant-garamond/latin-400.css`，并按需加载；中文回退 `"Noto Serif SC" → "Songti SC" → "STSong" → "SimSun" → serif`，不额外分发中文字体文件。字体的 `LICENSE` 随 npm 包保留在 `node_modules/@fontsource/*/LICENSE`。

## 新增内置主题

1. 在 `builtin.ts` 里添加 `ThemeDefinition`，放进 `BUILTIN_THEMES`。
2. 需要额外视觉层时新建 `web/src/theme/<id>.css`，所有规则以 `html[data-theme="<id>"]` 开头，并在 `load` 里动态 `import()`。
3. 在 `registry.test.ts` 补令牌对比度/注册/回退断言。
4. 运行 `npm run build -w web`、`node tools/check-bundle.mjs`、`node tools/check-test-ratchet.mjs`、`node tools/count-tests.mjs --check`。
