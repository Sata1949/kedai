// UIPOL 批源码级视觉契约(2026-10-31)。与 UIP/mobileLayout 两批同一口径:
// jsdom 无布局引擎、vitest `css:false` 不加载样式,尺寸/对齐类回归无法用组件测试断言,
// 改用源码文本守住「一改就悄悄回退」的七条。`.css` 的 `?raw` 在本仓返回空串,故走 node:fs。
// 每条都对应实测数字(见 D:/kedai-bench-run/uipol-evidence/README.md),注释里保留「修前的值」,
// 因为下一轮最容易被反问的就是「原来是多少」。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

/** 读取相对本测试文件的源码文本(readFileSync 直接接受 file: URL)。 */
const readText = (rel: string): string => readFileSync(new URL(rel, import.meta.url), 'utf8');

const shellCss = readText('../styles/shell.css');
const panelsCss = readText('../styles/panels.css');
const contentCss = readText('../styles/content.css');
const topbarSource = readText('ChatTopbar.vue');
const taskEmptySource = readText('task/TaskBoardEmpty.vue');
const charListSource = readText('sidebar/CharacterListPanel.vue');

describe('UIPOL-1 对齐与规格', () => {
  it('A1:顶栏 JS 授权与 HTML/AGENT 同规格(22px 裸开关,不再是 34px 带框按钮)', () => {
    // 修前:JS 是 .sv-btn(高 34px、2px 黑框、白底),与两侧 22px 的拨杆形成 6px 鼓包。
    // 拉平规则落在 ChatTopbar 的 scoped 块(组件样式不进全局域,D-5 纪律)。
    expect(topbarSource).toContain('sv-topbar-js');
    expect(topbarSource).toMatch(/\.sv-topbar-render \.sv-topbar-js \{[^}]*height: 22px;/);
    // 标签字号必须与 .toggle-label 的 11px 同档,否则同一排字会有两种大小
    expect(topbarSource).toMatch(/\.sv-topbar-render \.sv-topbar-js \{[\s\S]*?font-size: 11px;/);
    expect(shellCss).toMatch(/\.sv-render-toggle-v2 \.toggle-label \{\s*font-size: 11px;/);
    // 不得残留按钮形态的描边/底色
    expect(topbarSource).toMatch(/\.sv-topbar-render \.sv-topbar-js \{[\s\S]*?border: 0;/);
  });

  it('A2:表单标签列定宽 96px 且超长走省略号(不再内容自适应)', () => {
    // 修前:min-width:72px + width:auto → 控件左缘 {542,535} 两种取值、BASE URL 折行
    const tagRule = panelsCss.match(/\.sv-inp-tag \{[^}]*\}/)?.[0] ?? '';
    expect(tagRule).toContain('flex: 0 0 96px;');
    expect(tagRule).toContain('text-overflow: ellipsis;');
    // min-width:auto 是 flex 默认值,等于 min-content,会让 nowrap 长标签顶破定宽
    expect(tagRule).toContain('min-width: 0;');
    expect(tagRule).not.toContain('min-width: 72px');
  });

  it('A2:被截断的标签逐条补了 title(长标签最集中的两处抽样)', () => {
    // 「模型与生成」页 11 个截断标签全部有 title;这里锚住最长的那条与一个中等的
    expect(readText('settings/GenParamsSection.vue')).toContain(
      'title="空转熔断窗口 / 输出去重上限"',
    );
    expect(readText('settings/GenParamsSection.vue')).toContain('title="剧情推演同步全局词条"');
    expect(readText('settings/AgentSettingsSection.vue')).toContain('title="变量状态注入位置"');
  });

  it('B3:输入区右侧给音频浮钮留位(64px,重叠归零)', () => {
    // 修前 padding-right 16px → 浮钮压住「选择文件」10×17px,elementFromPoint 2/6 抢点击
    const inputbarRule = shellCss.match(/\.sv-inputbar \{[^}]*\}/)?.[0] ?? '';
    expect(inputbarRule).toContain('calc(var(--space-4) + 48px)');
    // 窄屏浮钮隐藏且用 padding 简写整段复写,必须保留否则手机端会白少 64px
    expect(readText('../styles/mobile.css')).toMatch(
      /\.sv-inputbar \{ padding: var\(--space-2\) var\(--space-3\) var\(--space-2-5\); \}/,
    );
  });

  it('C2:气泡内边距回到 --space-* 档(18px 不在令牌里)', () => {
    const bubbleRule = shellCss.match(/\.sv-msg-bubble \{[^}]*\}/)?.[0] ?? '';
    expect(bubbleRule).toContain('var(--space-5)');
    expect(bubbleRule).not.toMatch(/padding:[^;]*\b18px/);
  });
});

describe('UIPOL-2 布局均衡', () => {
  it('A3:任务台空态垂直居中,且 scoped 与 margin 同改', () => {
    // 只改全局 task.css 无效(specificity 不够);只居中不删 margin-top 会被下推 24~72px
    expect(taskEmptySource).toMatch(
      /\.sv-task-empty\.fill \{[^}]*justify-content: center;/,
    );
    expect(taskEmptySource).toMatch(/\.sv-task-start \{[^}]*margin: 0 auto;/);
    expect(taskEmptySource).not.toContain('margin: clamp(var(--space-6), 6vh');
    // 窄屏内容已接近满高,居中会顶掉首屏可读起点
    expect(taskEmptySource).toMatch(/justify-content: flex-start;[\s\S]*padding-bottom: var\(--space-4\)/);
  });

  it('A4:角色列表下方有引导条,且零角色时不重复(0 角色已有空态)', () => {
    expect(charListSource).toContain('sv-char-empty');
    // 引导条只在有角色且在未搜索时出现
    expect(charListSource).toContain('characters.length > 0 && !store.searchQuery.trim()');
    expect(charListSource).toMatch(/\.sv-char-empty \{[^}]*font-size: var\(--text-xs\);/);
  });

  it('B1:消息表格整宽 + 固定列宽 + 表头分底色', () => {
    // 修前 auto 布局:气泡 588px 里表仅 228px、列宽 112/57/58
    const tableRule = contentCss.match(/\.sv-msg-bubble\.sv-msg-md table \{[^}]*\}/)?.[0] ?? '';
    expect(tableRule).toContain('width: 100%;');
    expect(tableRule).toContain('table-layout: fixed;');
    // fixed 布局下长串(URL)会溢出,必须配 anywhere
    const cellRule = contentCss.match(/\.sv-msg-bubble\.sv-msg-md th,[\s\S]*?\}/)?.[0] ?? '';
    expect(cellRule).toContain('overflow-wrap: anywhere;');
    // 表头底色从 --sv-paper(与 body 同色)改为 --sv-pink-light
    expect(contentCss).toMatch(/\.sv-msg-bubble\.sv-msg-md th \{[^}]*background: var\(--sv-pink-light\);/);
  });
});
