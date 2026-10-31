#!/usr/bin/env node
/**
 * check-text-punct.mjs — 模板文案标点纪律(纯 Node 零依赖)。
 *
 * 背景(UIVIS-3/UI-B4,2026-10-31 前端观感审计):
 *   同一句中文里混用半角与全角标点是最便宜也最显眼的「不专业」信号。审计实测
 *   模板文本节点里有 **327 处**「中文字符 + 半角逗号/分号/冒号/问号/叹号」,
 *   而对照组只有 12 处全角——不是刻意风格,是历史文案写作时没切输入法。
 *
 * 判据(严格限定，避免误报)：
 *   只检查 `.vue` 的 `<template>` 区间内、**标签之外**、**HTML 注释之外**的文本节点，
 *   且要求半角标点的**前一个字符是汉字**。命中即「中文句子里用了英文标点」。
 *
 * 刻意排除的区间(这些地方半角标点是正确写法)：
 *   - 标签属性内(title / placeholder / aria-label 等)：属性值是纯文本但不是「句子排版」,
 *     且键盘提示（「Enter 发送, Shift+Enter 换行」）本就按半角对齐键名;
 *   - `{{ … }}` 插值表达式内(代码,不是文案);
 *   - HTML 注释与 `<!-- -->` 说明;
 *   - `<script>` / `<style>`(本脚本只扫 template 区间，天然排除)。
 *
 * 用法:
 *   node tools/check-text-punct.mjs            # 有命中即 FAIL
 *   node tools/check-text-punct.mjs --verbose  # 打印逐条命中
 *
 * ratchet 纪律:基线为 **0**——这是可一次清零、且没有任何「合理豁免」的纪律
 * （半角标点在中文句子里永远是笔误）。若确实需要保留半角（例如引用了英文原文），
 * 用全角引号包裹整段英文即可，不要为本脚本开白名单。
 */

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SRC = join(ROOT, 'web', 'src');
const VERBOSE = process.argv.includes('--verbose');

/** 半角标点 → 对应全角（仅用于提示文案） */
const PUNCT = { ',': '，', ';': '；', ':': '：', '?': '？', '!': '！' };

/** 递归收集 .vue 文件 */
function collectVue(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) collectVue(p, out);
    else if (p.endsWith('.vue')) out.push(p);
  }
  return out;
}

/**
 * 取 `<template>` 顶层区间。
 * 用第一个 `<template>` 与最后一个 `</template>`：`<script setup>` 里若有 template 字面量
 * 不会在它之前，而嵌套 `<template v-if>` 的闭合会早于最后一个——包含进来无害（它们同样是
 * 模板文本节点），排除出去反而会漏。
 */
function templateBlock(src) {
  const start = src.indexOf('<template>');
  const end = src.lastIndexOf('</template>');
  if (start < 0 || end < 0) return null;
  return { text: src.slice(start + '<template>'.length, end), offset: start + '<template>'.length };
}

/** 把注释替换为等长空白（保留行号） */
function blankComments(text) {
  return text.replace(/<!--[\s\S]*?-->/g, (m) => m.replace(/[^\n]/g, ' '));
}

/** 扫描一个模板块，返回 [{line, ch, col, snippet}] */
function scanTemplate(templateText) {
  const text = blankComments(templateText);
  const hits = [];
  let inTag = false;
  let inMustache = false;
  let line = 1;
  let lineStart = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '\n') {
      line++;
      lineStart = i + 1;
      continue;
    }
    if (!inMustache) {
      if (c === '<') inTag = true;
      else if (c === '>') inTag = false;
    }
    // 插值边界：{{ … }} 内的内容是表达式，不是文案
    if (!inTag && c === '{' && text[i + 1] === '{') {
      inMustache = true;
      i++;
      continue;
    }
    if (inMustache) {
      if (c === '}' && text[i + 1] === '}') {
        inMustache = false;
        i++;
      }
      continue;
    }
    if (inTag) continue;
    if (!PUNCT[c]) continue;
    // 前一个字符是汉字 → 中文句子里的英文标点
    if (!/[\u4e00-\u9fa5]/.test(text[i - 1] ?? '')) continue;
    const lineEnd = text.indexOf('\n', i);
    const snippet = text.slice(lineStart, lineEnd < 0 ? undefined : lineEnd).trim().slice(0, 110);
    hits.push({ line, col: i - lineStart + 1, ch: c, snippet });
  }
  return hits;
}

const failures = [];
for (const file of collectVue(SRC)) {
  const src = readFileSync(file, 'utf8');
  const block = templateBlock(src);
  if (!block) continue;
  const hits = scanTemplate(block.text);
  if (hits.length) failures.push({ file: relative(ROOT, file).replace(/\\/g, '/'), hits });
}

/**
 * 补充面:`api/labels.ts` 这类**纯展示文案常量表**。
 *
 * 它们不出现在 `<template>` 里，但会经 `{{ TABLE[x] }}` 原样渲染成下拉选项文本
 * (UIVIS-3/UI-B4 实施期实测:模板清零后「模式：三段式（默认）」在 labels.ts 里仍是半角,
 * 界面上的下拉照样是半角——只扫模板会漏)。这里只登记**确认为界面文案**的表,
 * 不扫全部 .ts:错误消息、日志、注释里出现半角标点不是排版问题。
 */
const LABEL_TABLES = [
  {
    file: 'web/src/api/labels.ts',
    /** 表名(仅用于提示);实际按「字符串字面量 + 前字符为汉字」扫描整个文件 */
    name: 'api/labels.ts 展示标签',
  },
];

for (const t of LABEL_TABLES) {
  const abs = join(ROOT, t.file);
  let src;
  try {
    src = readFileSync(abs, 'utf8');
  } catch {
    continue; // 文件已删则跳过(本表是登记式,不因文件移动而误报)
  }
  // 去掉行注释与块注释，避免把说明文字当文案
  const code = src.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/(^|[^:])\/\/[^\n]*/g, (m, p1) => p1 + m.slice(p1.length).replace(/[^\n]/g, ' '));
  const hits = [];
  let line = 1;
  let lineStart = 0;
  for (let i = 0; i < code.length; i++) {
    const c = code[i];
    if (c === '\n') {
      line++;
      lineStart = i + 1;
      continue;
    }
    if (!PUNCT[c]) continue;
    if (!/[\u4e00-\u9fa5]/.test(code[i - 1] ?? '')) continue;
    const lineEnd = code.indexOf('\n', i);
    hits.push({
      line,
      col: i - lineStart + 1,
      ch: c,
      snippet: code.slice(lineStart, lineEnd < 0 ? undefined : lineEnd).trim().slice(0, 110),
    });
  }
  if (hits.length) failures.push({ file: t.file, hits, note: t.name });
}

if (!failures.length) {
  console.log('[ OK ] 模板文案标点纪律:中文句子中无半角逗号/分号/冒号/问号/叹号');
  process.exit(0);
}

const total = failures.reduce((n, f) => n + f.hits.length, 0);
console.log(`\n[FAIL] ${total} 处中文句子用了半角标点(${failures.length} 个文件):`);
for (const f of failures) {
  console.log(`  ${f.file} — ${f.hits.length} 处`);
  if (!VERBOSE) {
    for (const h of f.hits.slice(0, 3)) {
      console.log(`    L${h.line}:${h.col} \`${h.ch}\` → \`${PUNCT[h.ch]}\`  ${h.snippet}`);
    }
    if (f.hits.length > 3) console.log(`    …另 ${f.hits.length - 3} 处(用 --verbose 全列)`);
  } else {
    for (const h of f.hits) {
      console.log(`    L${h.line}:${h.col} \`${h.ch}\` → \`${PUNCT[h.ch]}\`  ${h.snippet}`);
    }
  }
}
console.log('\n  说明:中文句子里的 , ; : ? ! 一律用全角。属性(title/placeholder)与 {{ }} 表达式不在检查范围。');
process.exit(1);
