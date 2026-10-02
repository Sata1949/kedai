// UIP 批次布局/观感源码契约(2026-10-02)。jsdom 无布局引擎、vitest `css:false` 连样式
// 都不加载,尺寸/滚动类回归无法用组件测试断言;沿用 UIFIX 章同一口径,用源码文本守住
// 「最隐蔽、一改就悄悄回退」的几条规则。注意 `.css` 的 `?raw` 在本仓返回空串
// (见 ChatInput.test.ts 顶部注释;2026-10-02 再次实测确认),故 .css 一律走 node:fs 读取。
import { describe, expect, it } from 'vitest';
// 本仓库未安装 @types/node(前端源码不需要),测试里用 node: 内置模块的类型来自
// src/test-node-shim.d.ts 的最小声明(2026-10-02 UIP-10 起),不再逐文件写显式压错注释。
import { readFileSync } from 'node:fs';
import sidebarSource from './Sidebar.vue?raw';

/** 读取相对本测试文件的源码文本(readFileSync 直接接受 file: URL)。 */
const readText = (rel: string): string => readFileSync(new URL(rel, import.meta.url), 'utf8');

const taskCss = readText('../styles/task.css');
const panelsCss = readText('../styles/panels.css');
const mobileCss = readText('../styles/mobile.css');

describe('UIP 批次源码级布局契约', () => {
  it('UIP-1:行内控件不被「下达目标」区全宽规则吃掉(执行者行豁免在)', () => {
    // 全宽规则本身必须保留(纵向堆叠控件依赖它);UIP-10 起间距值走 --space-* 令牌
    expect(taskCss).toContain('.sv-task-new .sv-select { width: 100%; margin-top: var(--space-2); }');
    expect(taskCss).toContain('.sv-task-new .sv-btn { width: 100%; margin-top: var(--space-2-5); }');
    // 行内豁免:复位为行内布局,否则按钮 basis=整行把 select 压成只剩箭头的窄块
    expect(taskCss).toMatch(
      /\.sv-task-new \.sv-inp-row \.sv-btn\s*\{\s*width: auto;\s*margin-top: 0;\s*\}/,
    );
  });

  it('UIP-2:工作区 placeholder 精简且输入框保 170px 下限(窄栏折行不裁字)', () => {
    expect(sidebarSource).toContain('placeholder="工作区:项目目录绝对路径"');
    expect(sidebarSource).toContain('flex: 1 1 170px');
    expect(sidebarSource).toContain('min-width: 170px');
  });

  it('UIP-3:按钮禁用态显式压平(不再用 opacity 半透明退化成灰条)', () => {
    expect(panelsCss).toMatch(
      /\.sv-btn:disabled \{[^}]*opacity: 1;[^}]*background: var\(--sv-line\);[^}]*box-shadow: none;[^}]*\}/,
    );
    // hover 变体必须排除禁用态(否则禁用按钮悬停仍变粉)
    expect(panelsCss).toContain('.sv-btn.primary:hover:not(:disabled)');
    expect(panelsCss).toContain('.sv-btn.ghost:hover:not(:disabled)');
    expect(panelsCss).toContain('.sv-btn.danger:hover:not(:disabled)');
  });

  it('UIP-4:任务中部为单滚动区,历史列表让出内部滚动(重叠修复不退化)', () => {
    expect(sidebarSource).toContain('sv-task-scroll');
    expect(taskCss).toMatch(/\.sv-task-scroll \{\s*overflow-y: auto;\s*\}/);
    expect(taskCss).toMatch(
      /\.sv-task-scroll \.sv-task-list \{[^}]*flex: none;[^}]*overflow: visible;[^}]*\}/,
    );
    // 手机断点还原「抽屉整滚 + 列表内滚」,不得形成嵌套滚动
    expect(mobileCss).toContain('.sv-task-scroll { overflow-y: visible; }');
  });

  it('UIP-5:进度行不再引用不存在的变量与蓝色回退', () => {
    const taskBoardSource = readText('TaskBoard.vue');
    expect(taskBoardSource).not.toContain('--sv-text-dim');
    expect(taskBoardSource).not.toContain('4ea1ff');
  });
});
