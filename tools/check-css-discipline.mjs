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
 * 行数基线(2026-10-02,UIP-6 收编动效令牌后的实测值;只降不升)。键为相对 web/src 的路径。
 */
const LINE_BASELINE = {
  'style.css': 47,
  'styles/tokens.css': 134,
  'styles/base.css': 117,
  'styles/shell.css': 828,
  'styles/panels.css': 895,
  'styles/content.css': 939,
  'styles/task.css': 995,
  'styles/mobile.css': 434,
};

/**
 * !important 基线(2026-10-02 实测;去注释后计数;只降不升)。
 * 全局域仅 content.css 有 10 处 = 7 条深底正文色双保险(文件尾「置于文件末尾」段)
 * + 3 条 prefers-reduced-motion 强制降级;新增样式不得引入(用更具体选择器解决)。
 */
const IMPORTANT_BASELINE = { 'styles/content.css': 10 };
/** 组件 scoped(.vue)的 !important 合计基线:当前 0(一旦出现即 FAIL)。 */
const VUE_IMPORTANT_BASELINE = 0;

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

// ===== 输出 =====
console.log('========== Kedai 前端样式纪律(只降不升)==========');
if (VERBOSE) {
  console.log('  全局域行数:');
  for (const r of lineRows) console.log(`    ${r.lines.toString().padStart(5)} / ${r.base}  ${r.rel}`);
  console.log('  !important:');
  for (const r of importantRows) console.log(`    ${r.count.toString().padStart(5)} / ${r.base}  ${r.rel}`);
  console.log(`    ${vueImportant.toString().padStart(5)} / ${VUE_IMPORTANT_BASELINE}  组件 scoped(.vue 合计)`);
  console.log(`  裸时长:${bareHits.length} 处`);
} else {
  const totalLines = lineRows.reduce((a, r) => a + r.lines, 0);
  const totalBase = lineRows.reduce((a, r) => a + (r.base ?? 0), 0);
  const totalImportant = importantRows.reduce((a, r) => a + r.count, 0);
  const totalImportantBase = importantRows.reduce((a, r) => a + r.base, 0);
  console.log(`  全局域:${lineRows.length} 文件 ${totalLines} 行(基线 ${totalBase})`);
  console.log(`  !important:全局域 ${totalImportant}(基线 ${totalImportantBase});组件 scoped ${vueImportant}(基线 ${VUE_IMPORTANT_BASELINE})`);
  console.log(`  裸时长:${bareHits.length} 处(应 0)`);
}
for (const n of notes) console.log(`  [提示] ${n}`);
if (failures.length > 0) {
  for (const f of failures) console.log(`  [FAIL] ${f}`);
  for (const h of bareHits) console.log(`    ${h}`);
  console.log('[ FAIL ] 样式纪律超基线(见上;ratchet 只降不升)');
  process.exit(1);
}
console.log('[ OK ] 样式纪律门禁通过');
