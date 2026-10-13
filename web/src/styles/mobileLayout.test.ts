// ANDUI 批移动端窄屏布局源码契约(2026-10-13)。jsdom 无布局引擎、vitest `css:false`
// 不加载样式,尺寸/遮挡类回归无法用组件测试断言;沿用 UIP 批同一口径
// (components/sidebarTaskLayout.test.ts),用源码文本守住「一改就悄悄回退」的几条移动端规则。
// 本轮实证:411×914 下任务列表塌成 12px / 「已关闭」按钮竖排 / 模态底部被底导航遮挡,
// 修复要求见 ANDROID-UI-FIX-PLAN.md(ANDUI-1~6)。注意 `.css` 的 `?raw` 在本仓返回空串,
// 故样式一律走 node:fs 读取。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

/** 读取相对本测试文件的源码文本(readFileSync 直接接受 file: URL)。 */
const readText = (rel: string): string => readFileSync(new URL(rel, import.meta.url), 'utf8');

const mobileCss = readText('./mobile.css');

describe('ANDUI 批移动端源码级布局契约', () => {
  it('ANDUI-1:模态 Sheet 底部给底导航让位(底导航 z=53 恒在弹窗上,内容区必须避开)', () => {
    // Sheet 变体组(统一拉满)内的 padding-bottom 必须叠加底导航高,否则弹窗页脚
    // (聊天记录「完成」/契约编辑底部说明)落在导航之下不可点/被切。
    // 用 [^}]*? 限定在本块内匹配,防止正则溜到后续 .sv-agent-panel 等同值规则上。
    expect(mobileCss).toMatch(
      /\.sv-modal-mask \.sv-modal,[^}]*?height: 100dvh;[^}]*?padding-bottom: calc\(var\(--mobile-nav-h\) \+ var\(--safe-bottom\)\);/,
    );
  });

  it('ANDUI-2:左侧抽屉底部给底导航让位(「退出 Kedai」可达;与 .sv-agent-panel 先例对齐)', () => {
    expect(mobileCss).toMatch(
      /\.sv-sidebar,\s*\.sv-sidebar\.wide \{[^}]*?padding-bottom: calc\(var\(--mobile-nav-h\) \+ var\(--safe-bottom\)\);/,
    );
  });

  it('ANDUI-3:数据行/按钮行的按钮不被窄屏压缩(不再逐字竖排、不再折行)', () => {
    // MCP「已关闭」实测被压成 56×80(white-space:normal 逐字换行);照 .sv-inp-row
    // 的既有豁免先例给行内按钮 nowrap + 不收缩
    expect(mobileCss).toMatch(
      /\.sv-data-row > \.sv-btn:not\(\.sv-btn-fill\) \{\s*flex: 0 0 auto;\s*white-space: nowrap;\s*\}/,
    );
    expect(mobileCss).toMatch(/\.sv-btn-row \{ flex-wrap: wrap; \}/);
    expect(mobileCss).toMatch(/\.sv-btn-row > \.sv-btn \{ white-space: nowrap; \}/);
  });
});
