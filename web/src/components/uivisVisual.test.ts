// UIVIS 三批源码级契约(2026-10-31,接续 uipolVisual.test.ts)。
//
// 口径与前者一致:jsdom 无布局引擎、vitest `css:false` 不加载样式,几何类回归只能用源码文本
// 守住「一改就悄悄回退」。每条都把**修前的实测值**写进注释——审计报告的每一条都带
// 「修前 → 修后」数字(见 docs/plans/UI-AUDIT-2026-10-30.md),回退时才能对照判断。
//
// 覆盖 UIVIS-2(一致性:UI-A3 / UI-B1 / UI-B2 / UI-B3)
// 与 UIVIS-3(排版治理:UI-A5 / UI-B4 / UI-C1 / UI-C2 / UI-C3)以及 UIPOL-3(B2 / C1 / C3)。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { renderUserMarkdown } from '../markdown';

const readText = (rel: string): string => readFileSync(new URL(rel, import.meta.url), 'utf8');

const shellCss = readText('../styles/shell.css');
const panelsCss = readText('../styles/panels.css');
const mobileCss = readText('../styles/mobile.css');
const taskCss = readText('../styles/task.css');

describe('UIVIS-2 一致性', () => {
  it('UI-A3:用户消息走 markdown,但与助手路径有两处刻意差异(关 linkify、不剥协议块)', () => {
    // 修前:用户气泡是 `{{ }}` 纯文本,同一会话里用户写 `- a` 是字面量、助手写是 <ul><li>。
    // 安全基线不变:html 仍 false(原始标签一律转义),故用户路径不过 sanitizeHtml 白名单。
    const src = readText('../markdown.ts');
    const userBuilder = src.match(/function buildMdUser\(\)[\s\S]*?\n\}/)?.[0] ?? '';
    expect(userBuilder).toContain('html: false');
    expect(userBuilder).toContain('linkify: false');
    // 用户路径不得复用 stripProtocolBlocks(mvu 变量块是助手输出的约定)
    const userRenderer = src.match(/export function renderUserMarkdown\([\s\S]*?\n\}/)?.[0] ?? '';
    expect(userRenderer).not.toContain('stripProtocolBlocks');
    expect(userRenderer).not.toContain('stripHtmlComments');

    // 行为验证:markdown 语法生效
    expect(renderUserMarkdown('- 第一条\n- 第二条')).toContain('<li>第一条</li>');
    // 原始 HTML 被转义(不是真元素)
    const escaped = renderUserMarkdown('<img src=x onerror=alert(1)>');
    expect(escaped).not.toContain('<img');
    expect(escaped).toContain('&lt;img');
    // 关闭 linkify:裸 URL 不得被改写成 <a>
    expect(renderUserMarkdown('见 https://example.com/x')).not.toContain('<a ');

    // 模板侧:用户气泡挂 sv-msg-md 且走 v-html
    const item = readText('ChatMessageItem.vue');
    expect(item).toMatch(/sv-msg-bubble sv-msg-md"[\s\S]*?v-html="userHtml"/);
    // 缓存键必须与助手路径可分(scriptHash 位放固定标记)
    expect(item).toContain("scriptHash: 'user-md'");
  });

  it('UI-B1:输入区工具栏有明确换行优先级(只有模型选择器可让位)', () => {
    // 修前:Agent 面板展开后工具栏被拆成三行(模式+授权 / 模型独占 / 两个附件)
    expect(shellCss).toMatch(/\.sv-input-toolbar \.sv-mode \{ flex-shrink: 0; \}/);
    expect(shellCss).toMatch(
      /\.sv-input-toolbar \.sv-auth-mode,\s*\n\s*\.sv-input-toolbar \.sv-btn-attach \{ flex-shrink: 0; \}/,
    );
    // 模型选择器是唯一可让位项:flex:1 1 180px + min-width:0
    const chatInput = readText('ChatInput.vue');
    const modelRule = chatInput.match(/\.sv-model-select \{[\s\S]*?\n\}/)?.[0] ?? '';
    expect(modelRule).toContain('flex: 1 1 180px;');
    expect(modelRule).toContain('min-width: 0;');
    expect(modelRule).not.toContain('flex: none;');
  });

  it('UI-B2:折叠态 Agent 面板的子元素不被压缩排布(改前页签挤到 34px 竖折 4 行)', () => {
    // 修前:width:0 的同时子元素按 0 宽容器重排,「调用情况」实测 w=34.3 h=86.4
    expect(shellCss).toMatch(/\.sv-agent-panel:not\(\.open\) > \* \{ width: 340px; \}/);
    // 展开态宽度不变
    expect(shellCss).toMatch(/\.sv-agent-panel\.open \{\s*width: 340px;/);
  });

  it('UI-B3:滑块行 = grid 1fr + 88px 定宽,数字右对齐且等宽', () => {
    // 修前 flex + min-width:34px:三条滑块数值框宽度各异(0.30 / 1.00 / 32768)
    const rowRule = panelsCss.match(/\.sv-range-row \{[^}]*\}/)?.[0] ?? '';
    expect(rowRule).toContain('display: grid;');
    expect(rowRule).toContain('grid-template-columns: minmax(0, 1fr) 88px;');
    expect(rowRule).toContain('align-items: center;');
    // 数值框:等宽数字
    expect(panelsCss).toMatch(/\.sv-range-row output \{[\s\S]*?font-variant-numeric: tabular-nums;/);
    expect(panelsCss).toMatch(/\.sv-range-row \.sv-input\.inject-num \{[\s\S]*?text-align: right;/);
  });
});

describe('UIVIS-3 排版治理', () => {
  it('UI-A5:窄档中文不逐字折断(按钮/状态灯 nowrap + 页脚改单列)', () => {
    // 修前 768 档:「已连接」竖排三行、「退出 Kedai」两行
    expect(taskCss).toMatch(/\.sv-side-btn \{[\s\S]*?white-space: nowrap;/);
    const connRule = shellCss.match(/\.sv-conn \{[^}]*\}/)?.[0] ?? '';
    expect(connRule).toContain('white-space: nowrap;');
    expect(connRule).toContain('flex-shrink: 0;');
    // 220px 侧栏下两列网格每格约 92px,必须改单列
    const tablet = mobileCss.match(/@media \(max-width: 1099px\) and \(min-width: 768px\) \{[\s\S]*?\n\}/)?.[0] ?? '';
    expect(tablet).toContain('.sv-side-foot { grid-template-columns: 1fr; }');
  });

  it('UI-B4:门禁脚本存在且已接入 check-all(基线 0,防回流)', () => {
    const script = readText('../../../tools/check-text-punct.mjs');
    // 只扫模板文本节点:必须显式排除标签内、注释、{{ }} 表达式
    expect(script).toContain('inTag');
    expect(script).toContain('inMustache');
    expect(script).toContain('blankComments');
    // 展示标签常量表是补充面(模板清零后仍会渲染成下拉文案)
    expect(script).toContain('web/src/api/labels.ts');
    expect(readText('../../../tools/check-all.ps1')).toContain(
      "Invoke-Stage 'web: text-punct'          { node tools/check-text-punct.mjs }",
    );
  });

  it('UI-C1:CONN 加载态用 SkeletonBlock,不再裸 ...', () => {
    const src = readText('settings/ConnectionSection.vue');
    expect(src).toContain("import SkeletonBlock from '../SkeletonBlock.vue';");
    expect(src).not.toContain("?? '...'");
    // 有值时仍渲染真值,而不是两处同时出现
    expect(src).toMatch(/<code v-if="connInfo" class="sv-code">/);
    expect(src).toMatch(/<SkeletonBlock v-else :lines="1" width="140px" \/>/);
    expect(src).toMatch(/<SkeletonBlock v-else-if="connInfo" :lines="1" width="96px" \/>/);
  });

  it('UI-C2:主区懒加载挂 loadingComponent,不再有空白窗', () => {
    const wrapper = readText('../asyncMain.ts');
    expect(wrapper).toContain('loadingComponent: MainLoadingSkeleton');
    // 骨架本身存在且带无障碍语义
    const skeleton = readText('MainLoadingSkeleton.vue');
    expect(skeleton).toContain('role="status"');
    expect(skeleton).toContain('aria-label="正在加载"');
    // App.vue 两个主区组件都改走 lazyMain(直接 import 会让骨架永远不出现)
    const app = readText('../App.vue');
    expect(app).toMatch(/const ChatWindow = lazyMain\(\(\) => import\('\.\/components\/ChatWindow\.vue'\)\);/);
    expect(app).toMatch(/const TaskBoard = lazyMain\(\(\) => import\('\.\/components\/TaskBoard\.vue'\)\);/);
    expect(app).not.toContain("import ChatWindow from './components/ChatWindow.vue';");
  });

  it('UI-C2:lazyMain 产出真·异步组件(不是被改回静态 import 的假包装)', async () => {
    // 真 import(而非只读源码文本):defineAsyncComponent 的产物带 __asyncLoader,
    // 据此可区分「真懒加载」与「被改回静态 import 的假包装」。
    // 骨架的 DOM 结构断言在 MainLoadingSkeleton.test.ts(该文件才有 jsdom 环境)。
    const { lazyMain } = await import('../asyncMain');
    const produced = lazyMain(() => import('./MainLoadingSkeleton.vue'));
    // 通过索引访问而不做类型断言:__asyncLoader 是 Vue 内部私有标记,
    // 不在公开类型里,index signature 读法同样能表达「它存在且是函数」。
    const internal = produced as Record<string, unknown>;
    expect(typeof internal.__asyncLoader).toBe('function');
  });

  it('UI-C3:引导页步骤标签与正文之间有显式间距', () => {
    // 修前 .ob-lead { margin: 0 },间距完全来自父 gap(10px),与标题/正文行之间同档
    const ob = readText('OnboardingModal.vue');
    const leadRule = ob.match(/\.ob-lead \{[\s\S]*?\n\}/)?.[0] ?? '';
    expect(leadRule).toContain('margin: var(--space-2) 0 0;');
    expect(leadRule).not.toMatch(/margin: 0;/);
  });
});

describe('UIPOL-3 视觉权重', () => {
  it('B2:发送按钮禁用态轮廓可见(实线 ink-faint + 55%,不是浅粉底 1.03:1)', () => {
    const disabledRule = shellCss.match(/\.sv-btn-send:disabled \{[^}]*\}/)?.[0] ?? '';
    expect(disabledRule).toContain('background: transparent;');
    expect(disabledRule).toContain('border: var(--bw) solid var(--sv-ink-faint);');
    expect(disabledRule).toContain('opacity: 0.55;');
    // 虚线方案实测只有 1.61:1,不得回退
    expect(disabledRule).not.toContain('dashed');
    expect(disabledRule).not.toContain('background: var(--sv-pink-light);');
  });

  it('C1:导航序号回令牌档 11px(9px 不在字阶内,中文下接近不可读)', () => {
    const idxRule = panelsCss.match(/\.sv-hub-nav-idx \{[^}]*\}/)?.[0] ?? '';
    expect(idxRule).toContain('font-size: var(--text-2xs);');
    expect(idxRule).not.toContain('font-size: 9px;');
    // 令牌真值是 11px,防止有人把 --text-2xs 改小
    expect(readText('../styles/tokens.css')).toMatch(/--text-2xs: 11px;/);
  });

  it('C3:主次按钮权重 2:1(不再是 357px vs 357px 的兄弟关系)', () => {
    expect(panelsCss).toMatch(
      /\.sv-btn-row:has\(> \.sv-btn\.primary\) > \.sv-btn\.primary \{ flex: 2 1 0; \}/,
    );
    expect(panelsCss).toMatch(
      /\.sv-btn-row:has\(> \.sv-btn\.primary\) > \.sv-btn:not\(\.primary\) \{ flex: 1 1 0; \}/,
    );
  });

  it('UI-A4:设置页说明文案不再泄漏 Markdown 星号', () => {
    const src = readText('settings/ApiSettingsSection.vue');
    // 断言落在「实际渲染的 <p class="sv-note"> 段」上:该文件里有一处注释也引用了星号原文
    // (说明改法),整文件 toContain 会被自己的注释绊倒。
    const noteBlock = src.match(/<p class="sv-note">([\s\S]*?)<\/p>/)?.[1] ?? '';
    expect(noteBlock).toContain('<strong>默认连接</strong>');
    expect(noteBlock).not.toContain('**');
    // 同段半角标点一并转全角
    expect(noteBlock).not.toMatch(/<\/strong>;/);
    expect(noteBlock).toContain('；');
  });
});
