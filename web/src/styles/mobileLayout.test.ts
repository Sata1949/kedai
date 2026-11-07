// ANDUI 批移动端窄屏布局源码契约(2026-10-13)。jsdom 无布局引擎、vitest `css:false`
// 不加载样式,尺寸/遮挡类回归无法用组件测试断言;沿用 UIP 批同一口径
// (components/sidebarTaskLayout.test.ts),用源码文本守住「一改就悄悄回退」的几条移动端规则。
// 本轮实证:411×914 下任务列表塌成 12px / 「已关闭」按钮竖排 / 模态底部被底导航遮挡,
// 修复要求见 ANDROID-UI-FIX-PLAN.md(ANDUI-1~6)。注意 `.css` 的 `?raw` 在本仓返回空串,
// 故样式一律走 node:fs 读取。
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import sidebarSource from '../components/Sidebar.vue?raw';
import contractsSource from '../components/ContractsModal.vue?raw';
import appSource from '../App.vue?raw';
import chatMessageSource from '../components/ChatMessageItem.vue?raw';
import chatInputSource from '../components/ChatInput.vue?raw';
import chatWindowSource from '../components/ChatWindow.vue?raw';
import chatTopbarSource from '../components/ChatTopbar.vue?raw';
import taskBoardSource from '../components/TaskBoard.vue?raw';
import taskTranscriptSource from '../components/task/TaskTranscript.vue?raw';
import markdownBlockSource from '../components/task/MarkdownBlock.vue?raw';

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

  it('ANDUI-4:任务中部不再被压缩(抽屉整滚;UIP-4 两条规则保留防回退)', () => {
    // wrapper 的 grow-不缩钩子只在任务模式挂(Sidebar.vue);实测中部双重可缩时
    // 列表被压成 12px(3 条任务后两条全不可见)
    expect(sidebarSource).toContain("'sv-side-body-task': appMode === 'task'");
    expect(mobileCss).toMatch(/\.sv-side-body-task \{ flex: 1 0 auto; \}/);
    // UIP-4 既有规则必须保留:中部不另开内滚、列表让出内部滚动
    expect(mobileCss).toContain('.sv-task-scroll { overflow-y: visible; }');
    expect(mobileCss).toMatch(
      /\.sv-task-scroll \.sv-task-list \{\s*flex: 1;\s*min-height: 0;\s*overflow-y: auto;\s*\}/,
    );
  });

  it('ANDUI-5:会话/条目行窄屏竖排(标题不再被压成 0 宽、标签不再压按钮)', () => {
    expect(mobileCss).toMatch(
      /\.sv-modal-mask \.sv-wb-row \{\s*flex-direction: column;\s*align-items: stretch;\s*\}/,
    );
  });

  it('ANDUI-6:契约编辑双栏窄屏竖排(编辑器不再被 diff 固定宽挤成一列字)', () => {
    expect(contractsSource).toContain('sv-contract-split');
    expect(contractsSource).toMatch(
      /@media \(max-width: 767px\) \{[\s\S]*?\.sv-contract-split \{ flex-direction: column; \}[\s\S]*?\.sv-contract-diff \{ width: auto; max-height: 40%; \}[\s\S]*?\.sv-contract-editor \{ min-height: 180px; \}/,
    );
  });

  it('2026-11-06 抽屉可交互:遮罩必须在 .sv-body 内(否则被 z-index:1 层叠上下文困住抽屉)', () => {
    // 遮罩 z=51 若留在 .sv-body 外,会盖住整个 .sv-body(抽屉 z=52 在里层),
    // 表现为「抽屉展开后点哪儿都没反应」。断言 DOM 顺序:AgentPanel 之后、</div> 之前。
    expect(appSource).toMatch(
      /<AgentPanel \/>[\s\S]*?<Transition name="sv-fade">[\s\S]*?sv-drawer-backdrop[\s\S]*?<\/Transition>\s*<\/div>/,
    );
  });

  it('2026-11-06 任务内容不横向截断:看板 min-width:0 + 长 token 就地折行', () => {
    expect(taskBoardSource).toMatch(/\.sv-taskboard \{[^}]*?min-width: 0;/);
    expect(taskTranscriptSource).toMatch(
      /\.sv-transcript \.sv-task-step-name,\s*\.sv-transcript \.sv-task-step-goal,\s*\.sv-transcript \.sv-task-step-result \{ overflow-wrap: anywhere; \}/,
    );
    expect(markdownBlockSource).toMatch(/\.sv-task-result \{[^}]*?overflow-wrap: anywhere;/);
  });

  it('2026-11-06 移动端角色扮演观感:薄边框气泡 + 输入卡内嵌发送键', () => {
    // 规则放在组件 scoped 块(@media max-width:767px),不占全局域行数。
    expect(chatMessageSource).toMatch(
      /@media \(max-width: 767px\) \{[\s\S]*?\.sv-msg\.assistant \.sv-msg-bubble \{\s*border-width: var\(--bw-thin\);\s*border-top-width: 2px;\s*\}/,
    );
    expect(chatInputSource).toMatch(
      /@media \(max-width: 767px\) \{[\s\S]*?\.sv-inputbox \{ display: block; position: relative; \}/,
    );
    expect(chatInputSource).toMatch(
      /\.sv-btn-send \{[^}]*?position: absolute;[^}]*?right: 4px;[^}]*?bottom: 4px;/,
    );
    // 文本域必须块级:inline-block 的基线缝隙会让绝对定位的发送键低 5.73px
    expect(chatInputSource).toMatch(/\.sv-inputbox textarea \{[^}]*?display: block;/);
  });

  it('2026-11-06 触屏操作行不挤气泡:用户消息换行 + 按钮 44px 横向命中区', () => {
    // 实机(hover:none)下操作行常显;修复前它与气泡并列,411px 下把气泡压到 275px
    // 并让四个两字按钮各以 17.6×94px 竖排。窄屏必须允许换行并整行落到气泡下方。
    expect(chatMessageSource).toMatch(/\.sv-msg\.user \{ flex-wrap: wrap; \}/);
    expect(chatMessageSource).toMatch(
      /\.sv-msg\.user \.sv-msg-actions \{ flex: 1 1 100%; justify-content: flex-end; \}/,
    );
    expect(chatMessageSource).toMatch(
      /\.sv-msg \.sv-msg-action \{[^}]*?min-width: 44px;[^}]*?white-space: nowrap;/,
    );
  });

  it('2026-11-06 移动端页头与输入区压缩:页头两行,输入区控件统一 30px', () => {
    // 有消息时的页头在 411×914 实测 92px + 24px 外边距;窄屏收掉 kicker 并压到 15px 标题。
    expect(chatWindowSource).toMatch(/\.sv-chat-masthead-kicker \{ display: none; \}/);
    expect(chatWindowSource).toMatch(
      /\.sv-chat-masthead-title \{ margin: 0; font-size: var\(--text-md\); \}/,
    );
    // 初始两行空高改为窄屏一行;模型下拉的全局 height:40px 必须显式覆盖。
    expect(chatInputSource).toContain(':rows="narrowViewport ? 1 : 2"');
    expect(chatInputSource).toMatch(/\.sv-model-select \{[^}]*?height: 30px;[^}]*?min-height: 30px;/);
  });

  it('2026-11-06 移动端呼吸感:元信息一行、双列主动作、工具条去框、顶栏只留一个 ⋯', () => {
    // 空态不再用三栏表格与 2px 黑杠;两个主动作并排、低频提示收起。
    expect(chatWindowSource).toMatch(/\.sv-start-meta \{[^}]*?display: flex;/);
    expect(chatWindowSource).toMatch(
      /\.sv-start-actions \{ display: grid; grid-template-columns: 1fr 1fr;/,
    );
    expect(chatWindowSource).toMatch(/\.sv-start-hint \{ display: none; \}/);
    // 工具条:模式/授权/模型去框,只有选中态保留墨块/粉块。
    expect(chatInputSource).toMatch(
      /\.sv-mode button \{[^}]*?background: transparent;[^}]*?color: var\(--sv-ink-faint\);/,
    );
    expect(chatInputSource).toMatch(
      /\.sv-model-select \{[^}]*?border: 0;[^}]*?background-color: transparent;/,
    );
    // 顶栏:蓝方块收起、会话下拉与图标按钮去框。
    expect(chatTopbarSource).toMatch(
      /\.sv-topbar-identity \.sv-supreme\.blue \{ display: none; \}/,
    );
    expect(chatTopbarSource).toMatch(
      /\.sv-session-select \{[^}]*?border: 0;[^}]*?background-color: transparent;/,
    );
  });
});
