// UIP-9 动效词汇源码契约(2026-10-02)。jsdom 无布局/动画引擎、vitest `css:false` 不加载样式,
// 动效类回归只能靠源码文本守住「唯一 keyframes / 词汇收口 / reduced-motion 补齐」这几条
// 一改就悄悄回退的规则。`.css` 的 `?raw` 在本仓返回空串(见 sidebarTaskLayout.test.ts 顶部),
// 故 .css 一律走 node:fs 读取,`.vue` 用 `?raw`。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import settingsHubSource from '../components/SettingsHub.vue?raw';
import memoryPanelSource from '../components/MemoryPanel.vue?raw';
import sidebarSource from '../components/Sidebar.vue?raw';
import taskBoardSource from '../components/TaskBoard.vue?raw';
import appSource from '../App.vue?raw';

/** 读取相对本测试文件的源码文本(readFileSync 直接接受 file: URL)。 */
const readText = (rel: string): string => readFileSync(new URL(rel, import.meta.url), 'utf8');

const tokensCss = readText('tokens.css');
const baseCss = readText('base.css');
const shellCss = readText('shell.css');
const panelsCss = readText('panels.css');
const contentCss = readText('content.css');
const taskCss = readText('task.css');
const mobileCss = readText('mobile.css');

/** 行级过滤:含关键词但不含 var() 引用(用于「无裸值」类断言)。 */
const linesWithBare = (text: string, keyword: string): string[] =>
  text
    .split('\n')
    .filter((l) => l.includes(keyword) && !l.includes(`var(--${keyword})`));

describe('UIP-9 动效词汇源码契约', () => {
  it('UIP-9a:入场动画唯一来源是 content.css 的 @keyframes sv-list-in,旧三条已清除', () => {
    expect(contentCss).toContain('@keyframes sv-list-in');
    for (const css of [taskCss, shellCss, panelsCss, baseCss, mobileCss]) {
      expect(css).not.toContain('@keyframes sv-list-in');
      expect(css).not.toContain('@keyframes sv-msg-in');
      expect(css).not.toContain('@keyframes hub-sub-in');
      expect(css).not.toContain('@keyframes memory-row-in');
    }
    expect(settingsHubSource).not.toContain('@keyframes hub-sub-in');
    expect(memoryPanelSource).not.toContain('@keyframes memory-row-in');
  });

  it('UIP-9a:六类消费者都改挂唯一 sv-list-in(消息/回执条/面板 tab/任务列表/设置二级项/记忆行)', () => {
    expect(shellCss).toMatch(/\.sv-msg \{[^}]*animation: sv-list-in /);
    expect(panelsCss).toMatch(/\.sv-feedback \{[^}]*animation: sv-list-in /);
    expect(panelsCss).toMatch(
      /\.sv-agent-tab,\s*\n\.sv-calltrace-tab \{[^}]*animation: sv-list-in /,
    );
    expect(taskCss).toMatch(/\.sv-task-step \{ animation: sv-list-in /);
    expect(taskCss).toMatch(/\.sv-timeline-item \{ animation: sv-list-in /);
    expect(settingsHubSource).toContain('animation: sv-list-in var(--dur-normal) var(--ease-out) backwards;');
    expect(memoryPanelSource).toContain('animation: sv-list-in var(--dur-normal) var(--ease-out) backwards;');
  });

  it('UIP-9b:scale 脉冲共用 sv-pulse-scale(时间线/状态点闪点/空态呼吸),旧名 sv-tl-pulse 已无', () => {
    expect(taskCss).toContain('@keyframes sv-pulse-scale');
    expect(taskCss).not.toContain('sv-tl-pulse');
    expect(taskCss).toMatch(/\.sv-dot-flash \{ animation: sv-pulse-scale /);
    expect(shellCss).toMatch(/\.sv-empty-geo \.sq\.deep \{[^}]*animation: sv-pulse-scale /);
  });

  it('UIP-9b:状态点挂闪点类与 :key(状态变更时节点重建重播)', () => {
    expect(sidebarSource).toContain('sv-dot-flash');
    expect(sidebarSource).toContain(':key="t.status"');
    expect(taskBoardSource).toContain('sv-dot-flash');
    expect(taskBoardSource).toContain(':key="st.status"');
  });

  it('UIP-9c:--transition-* 兼容别名已删除且全仓无引用,--ease-in-out 已建', () => {
    expect(tokensCss).toContain('--ease-in-out: ease-in-out;');
    // 断言的是「定义已删」——注释里提到旧别名名不算引用
    expect(tokensCss).not.toMatch(/^\s*--transition-(fast|normal):/m);
    for (const text of [tokensCss, baseCss, shellCss, panelsCss, contentCss, taskCss, mobileCss, settingsHubSource, memoryPanelSource, appSource]) {
      expect(text).not.toContain('var(--transition-');
    }
    // 全局域不得再有裸 ease-in-out(tokens.css 的定义行除外)
    for (const [name, css] of [
      ['base.css', baseCss],
      ['shell.css', shellCss],
      ['panels.css', panelsCss],
      ['content.css', contentCss],
      ['task.css', taskCss],
      ['mobile.css', mobileCss],
    ] as const) {
      expect(linesWithBare(css, 'ease-in-out'), `${name} 残留裸 ease-in-out`).toEqual([]);
    }
  });

  it('UIP-9d:reduced-motion 兜底补 delay 归零(stagger 不再延迟出现)', () => {
    expect(contentCss).toMatch(
      /@media \(prefers-reduced-motion: reduce\) \{[\s\S]*animation-delay: 0\.01ms !important;[\s\S]*transition-delay: 0\.01ms !important;[\s\S]*\}/,
    );
  });

  it('UIP-9e:移动端抽屉遮罩经 sv-fade 过渡(与抽屉本体同步淡入)', () => {
    expect(appSource).toMatch(/<Transition name="sv-fade">[\s\S]*sv-drawer-backdrop/);
  });
});
