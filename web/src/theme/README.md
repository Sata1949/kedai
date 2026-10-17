# 主题插件与字体

Kedai 的前端主题是**声明式**扩展点：主题只能覆盖 CSS 自定义属性、根 class、`@font-face` 与主题作用域 CSS，宿主不向插件暴露 DOM 写入口。内置的“教堂拼接玻璃”与默认“纸面 · 至上主义”走同一注册表，外部插件用同一套 API 注册。

## 内置主题

| id | 名称 | 说明 |
|---|---|---|
| `default` | 纸面 · 至上主义 | 淡粉纸面、黑方块与红斜线；默认主题，无额外 CSS/字体 |
| `stained-glass` | 教堂拼接玻璃 | 暗石外壳、白玻璃菱格、彩色玻璃拼片与密窗玫瑰；主题 CSS 与字体按需加载 |

切换入口：**综合设置 → 界面与工具 → 界面 → 外观主题**。选择写入 `localStorage["kedai.theme.v1"]`，不写 `settings.json`。启动时 `main.ts` 先同步应用已保存的令牌，再按需加载主题 CSS/字体。

### 彩窗主题的视觉依据（2026-10-17 重做）

参照 15 张真实教堂彩窗照片提炼，而不是把六种宝石色等量铺开：

- **沙特尔「蓝圣母」窗 / York Minster**：连白玻璃底也是许多小菱形拼成，不是平涂；彩色圆章嵌在白色菱格场里，铅条形成密网。
- **Canterbury 特写 / Cologne 人物**：每块蓝底、白袍、绿袍都由许多不规则小片拼成，片与片之间有深浅差；粗黑铅条把画面切成网。
- **圣礼拜堂**：石构近黑，玻璃自发光；彩色光会溢出到相邻玻璃和石面上。
- **Laon / 巴黎圣母院玫瑰窗**：不是 12 瓣平涂，而是三层放射扇片 + 一圈圈白环小圆章 + 很粗的石棂；蓝场为主，红金只作圆章。

落成规则：外壳用暗石、彩色光晕与一层拼片；面板是半透明白玻璃（grisaille 细菱格）
叠在彩色拼片上，两种拼片使用同一套 48×44 菱形与 384×176 平铺尺寸（铅条严格对齐），
中央白玻璃罩保证文字可读，最外缘露出彩色拼片；顶栏/侧栏顶/模态头/选中态是
「拼片 + 深蓝纱 + 金色背光」的玻璃横楣；主按钮与授权档是拼片 + 红宝石纱；
综合设置的左栏与内容区也铺同一扇玻璃，二级选中为钴蓝拼片；
空态右侧是一扇 150px 密窗玫瑰（三层扇片 + 白环圆章 + 粗石棂）。所有规则限定在
`html[data-theme='stained-glass']` 下，不使用 `!important`。

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
  load?: () => Promise<unknown>; // 首次激活时调用一次（动态 CSS/字体 chunk）
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
- 主题资源只在首次激活时加载一次；切回默认不会卸载已加载的 CSS/字体（它们都带主题作用域）。

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
