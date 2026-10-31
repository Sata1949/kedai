#!/usr/bin/env node
/**
 * check-css-discipline.mjs — 前端样式纪律护栏(纯 Node 零依赖)。
 *
 * 背景(UIP-8;承接 FRONTEND-REPORT §七第 10 条与 FE-12 口径):
 *   「style.css 纪律计数进门禁」原是报告遗留建议——凡是写在注释里的纪律都在漂移:
 *   注释自称「!important 存量 11 处」而实测 13 处。本脚本把三条纪律变成机器门禁:
 *
 *   1) 全局域行数 ratchet:web/src/style.css + web/src/styles/*.css 逐文件行数只降不升
 *      (D-5 纪律:新增组件样式一律 <style scoped> 或 Tailwind 工具类,全局域只应缩小);
 *   2) !important ratchet:全局域逐文件 + 组件 scoped(.vue 内)合计,只降不升;
 *   3) 裸时长归零:animation / animation-delay / transition / transition-duration 的
 *      时长必须走令牌(--dur-* / --stagger-step / --transition-*);0ms/0s 与
 *      reduced-motion 的 0.01ms 属归零语义,白名单放行。
 *   4) 间距裸值 ratchet(2026-10-02 UIP-10 新增):padding/margin/gap/row-gap/column-gap
 *      (含单后缀)声明里的非 0 裸 px 值计数只降不升——可令牌化值必须走 --space-*
 *      (var()/calc() 等括号内不计,无令牌值如 1/2/3/5/7/9/18px 等计入基线但不再新增)。
 *
 * 用法:
 *   node tools/check-css-discipline.mjs            # 超基线即 FAIL
 *   node tools/check-css-discipline.mjs --verbose  # 打印逐项计数
 *
 * ratchet 纪律:基线只能调**低**;确需上调(如批次有意扩张全局域)要在提交信息里给出
 * 理由并同步改本文件——与 check-bundle.mjs 的预算纪律同源。注释文本/注释行不计入计数
 * (CSS 块注释与 HTML 注释先剥离;含「!important」字样的说明文字不参与计数)。
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const WEB_SRC = join(ROOT, 'web', 'src');
const STYLES = join(WEB_SRC, 'styles');
const VERBOSE = process.argv.includes('--verbose');

/**
 * 行数基线(2026-10-02 UIP-10 收编后的实测值;只降不升)。
 * UIP-9 变动:content 939→941(无障碍 delay 归零 +2 行);task 995→996(状态点闪点 +1);
 * panels 895→894(tab 容器合并 −2、回执条入场 +1);tokens 134→133(删别名 −2、--ease-in-out +1)。
 * UIP-10 变动:style 47→49(截断 canon 2 行);panels 894→903(单行控件高度规则与 .sv-btn 档位 +9);
 * shell 828→832(控件高度档位 +2、角色名截断与附件 chip min-width +2);task 996→997(appmode 高度 +1);
 * tokens 133→144(半步档 6/10/14 令牌 3 + 控件高度档 2 + 黑底次级文字 1 + 注释 5)。
 * UIP-12 变动:panels 903→911(指针模态下 select 焦点收窄规则 8 行)。
 * UIP-14 变动:base 117→122(深底焦点反色规则 5 行——模态头/Agent 面板头为 ink 底,ink 描边不可见)。
 * UIP-15 变动:content 941→959(列表增删/重排过渡 sv-list 18 行——两处试点:记忆行/会话行)。
 * CU-1 变动:panels 911→941(电脑操作急停按钮 + 反馈行 30 行——Agent 面板头新增用户侧急停
 * 入口,与既有 .sv-agent-panel-close 同族样式;全局域行数上调仅为此一处新增控件)。
 * RPFLOW 变动:panels 941→971(草稿折叠区 30 行——AgentPanel 无 scoped 块,样式按纪律
 * 落全局域:草稿步产物默认隐藏,面板提供可展开的只读视图)。
 * RPFLOW 提交 3 变动:content 959→1008(酒馆式消息 HTML 子集渲染 49 行——details/summary
 * 折叠块 + .sv-status-block 状态块 + 分隔段 hr;消息正文经 v-html 注入,scoped 样式
 * 不作用于 v-html 内容,故必须落全局域)。
 * IMPHUB 步骤 E 变动(只降):content 1008→1007 / mobile 434→433——楼层「注入位置」下拉与
 * depth 输入删除后 `.floor-depth` 孤儿规则随之清除,`.floor-row` grid 由 7 轨收到 5 轨
 * (与现存子节点数一致,消掉尾部空轨的多余 gap)。
 * ANDUI 变动(2026-10-13):mobile 433→464(安卓窄屏布局修复批 +31 行,五处新增各带实测注释:
 * 模态 Sheet 底部让位底导航 + 左抽屉同款让位 + 行内按钮豁免(3 条) + 任务中部 grow-不缩钩子 +
 * 会话条目行竖排——全部发生在既有 <768px 断点内、无新域。理由:mobile.css 是唯一的移动端适配层
 * (UIP 批起的既定形态),窄屏专用规则无处可去;新增规则仍遵守「不改桌面、不加 !important、
 * 间距走 --space-*」三条既有纪律)。
 * UIQ-4 变动(2026-10-17):tokens 144→153(新增 --measure-prose/--measure-chat/
 * --measure-task-result 与焦点环两个令牌,供双主体终稿共用),task 997→1007
 * (侧栏模式切换 01/02 序号 + 任务历史激活态与搜索框聚焦态)。均为两主体版式服务的
 * 正常增长,不新增 !important、不引入裸时长;新增规则优先留在 .vue scoped。
 * C-2A 变动(2026-10-25):task 1007→987——`.sv-task-new` 基础视觉块随 Sidebar 拆分
 * 移入 components/sidebar/TaskComposer.vue 的 scoped 块(单一 CSS 所有者),全局域净减 20 行。
 * UIPOL 变动(2026-10-31,两批):
 *   content 997→1001(消息表格整宽:width/table-layout/overflow-wrap 三条声明 + 一行注释。
 *     `.sv-msg-bubble.sv-msg-md table` 只能落全局域——消息正文经 v-html 注入,scoped 不生效);
 *   shell 保持 827(气泡内边距 18px→--space-5 行数不变,其间距裸值 10→9 已同步下调)。
 *   两批均不新增 !important、不引入裸时长;新增的 calc(--space-4 + 48px) 属浮钮让位常量,
 *   走令牌表达式而非裸值。
 * UIVIS/UIPOL-3 变动(2026-10-31,一致性 + 排版治理九项;全部为**既有元素的对齐与状态收口**):
 *   content 1001→1022(用户气泡深底 markdown 配色 21 行:UI-A3 让用户消息也走 markdown 后,
 *     引用块/行内代码/链接/horizontal rule 原先是浅底配色,套在黑底气泡上对比塌陷。
 *     消息正文经 v-html 注入,scoped 不生效,只能落全局域——与上一条 justify 同源);
 *   mobile 464→469(平板档侧栏页脚改单列 5 行:UI-A5,220px 侧栏下两列网格每格约 92px,
 *     「导入角色卡」即使 nowrap 也溢出。规则只在 768–1099px 断点内);
 *   panels 971→1000(UI-B3 滑块行 flex→grid + 定宽数值框 20 行;UI-B5 数据行 flex→grid 9 行);
 *   shell 827→862(UI-B1 工具栏换行优先级 12 行;UI-B2 折叠态子元素定宽 8 行;
 *     UI-A5 连接状态灯 nowrap 8 行;UIPOL-3/B2 发送键禁用态 8 行——四处都是改既有规则,
 *     行数增长来自注释与新增声明,未新增选择器族);
 *   task 987→993(UI-A5 侧栏底部按钮 nowrap + 省略号 6 行)。
 *   九项均不新增 !important、不引入裸时长;新增间距值全部走 --space-* 或既有 88px/340px
 *   这类「控件规格常量」(与 UIPOL 的 calc(--space-4 + 48px) 同一判断口径)。
 */
const LINE_BASELINE = {
  'style.css': 49,
  'styles/tokens.css': 153,
  'styles/base.css': 122,
  'styles/shell.css': 862,
  'styles/panels.css': 1000,
  'styles/content.css': 1022,
  'styles/task.css': 993,
  'styles/mobile.css': 469,
};

/**
 * !important 基线(2026-10-02 UIP-9 实测;去注释后计数;只降不升)。
 * 全局域仅 content.css 有 10 处 = 5 条深底正文色双保险(文件尾「置于文件末尾」段)
 * + 5 条 prefers-reduced-motion 强制降级(UIP-9 补 delay 归零 +2);新增样式不得引入
 * (用更具体选择器解决)。
 */
const IMPORTANT_BASELINE = { 'styles/content.css': 10 };
/** 组件 scoped(.vue)的 !important 合计基线:当前 0(一旦出现即 FAIL)。 */
const VUE_IMPORTANT_BASELINE = 0;

/**
 * 间距裸值基线(2026-10-02 UIP-10 等值替换后的实测值;计数口径见 countBareSpacing;只降不升)。
 * 无令牌值(1/2/3/5/7/9/18px 等)计入基线,但不得再新增;可令牌化裸值一律走 var(--space-*)。
 */
const SPACING_BASELINE = {
  'style.css': 0,
  'styles/tokens.css': 0,
  'styles/base.css': 0,
  'styles/shell.css': 9, /* UIPOL-1/C2:气泡右内边距 18px → --space-5(20px),裸值 −1 */
  'styles/panels.css': 7,
  'styles/content.css': 24,
  'styles/task.css': 14,
  'styles/mobile.css': 4,
};
/** 组件(.vue:scoped 块 + 静态 style 属性)间距裸值合计基线(2026-10-02 收编后实测)。 */
const VUE_SPACING_BASELINE = 40;

/** 间距声明(属性含单后缀,如 margin-bottom / row-gap)。 */
const SPACING_DECL =
  /(^|[;{}\s])((?:padding|margin|gap|row-gap|column-gap)(?:-[a-z]+)?)\s*:\s*([^;{}]+)/g;

/** 去掉所有括号内容(var()/calc() 等内部不计),返回剩下文本。 */
function stripParens(value) {
  let prev = value;
  for (;;) {
    const next = prev.replace(/\([^()]*\)/g, '');
    if (next === prev) return next;
    prev = next;
  }
}

/** 计数:一段 CSS 文本内「间距声明里的非 0 裸 px 值」个数。 */
function countBareSpacing(cssText) {
  let count = 0;
  for (const m of stripComments(cssText).matchAll(SPACING_DECL)) {
    for (const v of stripParens(m[3]).matchAll(/-?\d+(?:\.\d+)?px\b/g)) {
      if (Number.parseFloat(v[0]) !== 0) count++;
    }
  }
  return count;
}

/** 计数:.vue 只扫 <style> 块与静态 style="..." 属性(不碰 <script> 内 JS 字符串)。 */
function countBareSpacingVue(source) {
  let count = 0;
  for (const m of source.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)) count += countBareSpacing(m[1]);
  for (const m of source.matchAll(/\sstyle="([^"]*)"/g)) count += countBareSpacing(m[1]);
  return count;
}

/** 裸时长检测(先剥注释;归零语义 0/0.01 白名单)。 */
const BARE_DURATION =
  /(?:animation(?:-delay)?|transition(?:-duration)?):[^;}]*?(\d+(?:\.\d+)?)(ms|s)\b/g;
const ZERO_DURATIONS = new Set([0, 0.01]);

/** 剥离块注释与 HTML 注释(只影响计数,不改文件)。 */
const stripComments = (text) =>
  text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/<!--[\s\S]*?-->/g, '');

/** 递归收集指定扩展名的文件。 */
function collect(dir, exts, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) collect(p, exts, out);
    else if (exts.some((e) => name.endsWith(e))) out.push(p);
  }
  return out;
}

const newlineCount = (text) => (text.match(/\n/g) ?? []).length;

/** 全局域文件(style.css 入口 + styles/*.css 七域)。 */
const globalFiles = [
  join(WEB_SRC, 'style.css'),
  ...readdirSync(STYLES)
    .filter((n) => n.endsWith('.css'))
    .map((n) => join(STYLES, n)),
];

const failures = [];
const notes = [];

// ===== 1) 行数 ratchet =====
const lineRows = [];
for (const file of globalFiles) {
  const rel = relative(WEB_SRC, file).replace(/\\/g, '/');
  const lines = newlineCount(readFileSync(file, 'utf8'));
  const base = LINE_BASELINE[rel];
  lineRows.push({ rel, lines, base });
  if (base === undefined) failures.push(`行数基线缺失: ${rel}(新增全局域文件需先在脚本里登记)`);
  else if (lines > base) failures.push(`行数超基线: ${rel} ${lines} > ${base}`);
  else if (lines < base) notes.push(`可下调行数基线: ${rel} ${base} → ${lines}`);
}

// ===== 2) !important ratchet =====
const importantRows = [];
for (const file of globalFiles) {
  const rel = relative(WEB_SRC, file).replace(/\\/g, '/');
  const count = (stripComments(readFileSync(file, 'utf8')).match(/!important/g) ?? []).length;
  const base = IMPORTANT_BASELINE[rel] ?? 0;
  importantRows.push({ rel, count, base });
  if (count > base) failures.push(`!important 超基线: ${rel} ${count} > ${base}`);
  else if (count < base) notes.push(`可下调 !important 基线: ${rel} ${base} → ${count}`);
}
let vueImportant = 0;
for (const file of collect(WEB_SRC, ['.vue'])) {
  vueImportant += (stripComments(readFileSync(file, 'utf8')).match(/!important/g) ?? []).length;
}
if (vueImportant > VUE_IMPORTANT_BASELINE)
  failures.push(`组件 scoped !important 超基线: ${vueImportant} > ${VUE_IMPORTANT_BASELINE}`);
else if (vueImportant < VUE_IMPORTANT_BASELINE)
  notes.push(`可下调组件 !important 基线: ${VUE_IMPORTANT_BASELINE} → ${vueImportant}`);

// ===== 3) 裸时长归零 =====
const bareHits = [];
for (const file of [...globalFiles, ...collect(WEB_SRC, ['.vue'])]) {
  const text = stripComments(readFileSync(file, 'utf8'));
  for (const m of text.matchAll(BARE_DURATION)) {
    const value = Number(m[1]);
    if (ZERO_DURATIONS.has(value)) continue;
    bareHits.push(`${relative(ROOT, file).replace(/\\/g, '/')}: ${m[0].replace(/\s+/g, ' ')}`);
  }
}
if (bareHits.length > 0) failures.push(`裸时长 ${bareHits.length} 处(应走 --dur-*/--stagger-step 令牌)`);

// ===== 4) 间距裸值 ratchet =====
const spacingRows = [];
for (const file of globalFiles) {
  const rel = relative(WEB_SRC, file).replace(/\\/g, '/');
  const count = countBareSpacing(readFileSync(file, 'utf8'));
  const base = SPACING_BASELINE[rel] ?? 0;
  spacingRows.push({ rel, count, base });
  if (count > base) failures.push(`间距裸值超基线: ${rel} ${count} > ${base}`);
  else if (count < base) notes.push(`可下调间距裸值基线: ${rel} ${base} → ${count}`);
}
let vueSpacing = 0;
for (const file of collect(WEB_SRC, ['.vue'])) vueSpacing += countBareSpacingVue(readFileSync(file, 'utf8'));
if (vueSpacing > VUE_SPACING_BASELINE)
  failures.push(`组件间距裸值超基线: ${vueSpacing} > ${VUE_SPACING_BASELINE}`);
else if (vueSpacing < VUE_SPACING_BASELINE)
  notes.push(`可下调组件间距裸值基线: ${VUE_SPACING_BASELINE} → ${vueSpacing}`);

// ===== 输出 =====
console.log('========== Kedai 前端样式纪律(只降不升)==========');
if (VERBOSE) {
  console.log('  全局域行数:');
  for (const r of lineRows) console.log(`    ${r.lines.toString().padStart(5)} / ${r.base}  ${r.rel}`);
  console.log('  !important:');
  for (const r of importantRows) console.log(`    ${r.count.toString().padStart(5)} / ${r.base}  ${r.rel}`);
  console.log(`    ${vueImportant.toString().padStart(5)} / ${VUE_IMPORTANT_BASELINE}  组件 scoped(.vue 合计)`);
  console.log(`  裸时长:${bareHits.length} 处`);
  console.log('  间距裸值:');
  for (const r of spacingRows) console.log(`    ${r.count.toString().padStart(5)} / ${r.base}  ${r.rel}`);
  console.log(`    ${vueSpacing.toString().padStart(5)} / ${VUE_SPACING_BASELINE}  组件(.vue 合计)`);
} else {
  const totalLines = lineRows.reduce((a, r) => a + r.lines, 0);
  const totalBase = lineRows.reduce((a, r) => a + (r.base ?? 0), 0);
  const totalImportant = importantRows.reduce((a, r) => a + r.count, 0);
  const totalImportantBase = importantRows.reduce((a, r) => a + r.base, 0);
  const totalSpacing = spacingRows.reduce((a, r) => a + r.count, 0);
  const totalSpacingBase = spacingRows.reduce((a, r) => a + r.base, 0);
  console.log(`  全局域:${lineRows.length} 文件 ${totalLines} 行(基线 ${totalBase})`);
  console.log(`  !important:全局域 ${totalImportant}(基线 ${totalImportantBase});组件 scoped ${vueImportant}(基线 ${VUE_IMPORTANT_BASELINE})`);
  console.log(`  裸时长:${bareHits.length} 处(应 0)`);
  console.log(`  间距裸值:全局域 ${totalSpacing}(基线 ${totalSpacingBase});组件 ${vueSpacing}(基线 ${VUE_SPACING_BASELINE})`);
}
for (const n of notes) console.log(`  [提示] ${n}`);
if (failures.length > 0) {
  for (const f of failures) console.log(`  [FAIL] ${f}`);
  for (const h of bareHits) console.log(`    ${h}`);
  console.log('[ FAIL ] 样式纪律超基线(见上;ratchet 只降不升)');
  process.exit(1);
}
console.log('[ OK ] 样式纪律门禁通过');
