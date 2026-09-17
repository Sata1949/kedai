#!/usr/bin/env node
/**
 * check-bundle.mjs — 前端产物体积预算门禁(纯 Node 零依赖)。
 *
 * 为什么需要它:
 *   `check-all.ps1` 跑 `vite build` 只断言「构建成功」,不看产物大小。于是 chunk 划分被
 *   无意改坏(依赖挪动、manualChunks 调整、新引一个重库)时**构建依然全绿**,只有用户能
 *   感觉到首屏变慢。本脚本把「首屏体积」与「单 chunk 体积」变成可回归的硬断言。
 *
 * 与既有护栏同体例:纯 Node 零依赖 + 基线常量 + ratchet 只降不升(对齐
 *   `tools/check-frontend-lint.mjs` 的 BASELINE 写法与 `--verbose` 用法)。
 *
 * 检查项:
 *   1. `web/dist/index.html` 的 modulepreload **不得**含 `modal-*`
 *      —— 2026-09-17 P-8 的固化护栏:弹窗 chunk 一旦被首屏静态引用就会被预载,
 *      等于懒加载失效(修复前实测首屏白拉 188,635 B / gz 62,793 B)。
 *   2. 首屏集合(entry + 其预载的 js/css)的 gzip 总量 ≤ 预算。
 *   3. 每个 chunk 的 gzip 体积 ≤ 该前缀的预算(未列前缀走 DEFAULT 上限)。
 *   4. 全部资产 gzip 总量 ≤ 预算(整体膨胀兜底)。
 *
 * 命令行:
 *   node tools/check-bundle.mjs            # 超预算即 FAIL(exit 1)
 *   node tools/check-bundle.mjs --verbose  # 打印逐项实测值与预算
 *
 * fail-closed:`web/dist` 或 `index.html` 缺失即 FAIL。构建产物不在时**不得**静默通过
 *   —— 与仓库「门禁 fail-closed」纪律一致(先跑 `npm run build -w web`)。
 *
 * 口径说明(负向验证时实测确认):
 *   判定按 **gzip** 体积(loopback 场景浏览器按 gzip 传输,更贴近用户感知),`--verbose`
 *   同时打印 raw 供参考。已知局限:高度可压缩的增长(如用重复字节填充)在 gzip 下几乎不涨,
 *   可能逃过判定;真实代码/依赖增长不具该特征,故不为此改成按 raw 判定(会把正常的
 *   格式变化也判红)。
 *
 * 基线维护(ratchet 纪律):预算取「实测值 + 10% 余量」。功能正常增长导致超预算时,
 *   先量实测值确认增长有正当理由,再**上调到实测值 + 10%**;若增长无正当理由,修代码而不是
 *   调预算。**禁止**直接删除检查项或把预算调到明显失效的量级。
 */

import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const DIST = join(ROOT, 'web', 'dist');
const ASSETS = join(DIST, 'assets');
const VERBOSE = process.argv.includes('--verbose');

/**
 * 预算基线(2026-09-17 P-8/P-9 完成后实测 + 10% 余量,单位:gzip 字节)。
 * 实测值:首屏 261,293 / 全部资产 369,533 / index 95,096 / vendor 70,815 /
 * content-rendering 44,786 / vue-vendor 33,687 / index css 16,537。
 */
const FIRST_PAINT_BUDGET_GZ = 288000;
const ALL_ASSETS_BUDGET_GZ = 407000;

/**
 * 单 chunk 预算:键为「去哈希后的文件名」(`index.js` / `index.css` / `vendor.js` …)。
 * 只列体积有实际意义的 chunk;其余走 DEFAULT_CHUNK_BUDGET_GZ。
 * 哈希会随内容变化,故**必须按去哈希名索引**,不能写死文件名。
 */
const CHUNK_BUDGETS_GZ = {
  'index.js': 105000,
  'vendor.js': 78000,
  'content-rendering.js': 49500,
  'vue-vendor.js': 37500,
  'index.css': 18500,
};

/**
 * 未列出前缀的单 chunk 上限。当前最大的未列出 chunk 是 AgentSettingsSection(gz 8,721),
 * 12,000 既留有约 25% 正常增长余量,又能在「有重依赖落进某个小 chunk」时拦住。
 */
const DEFAULT_CHUNK_BUDGET_GZ = 12000;

/** 首屏预载里出现这些前缀即 FAIL(弹窗必须留在动态 import 的异步 chunk 里) */
const FORBIDDEN_PRELOAD_PREFIXES = ['modal-'];

const fail = [];
const warn = [];

if (!existsSync(ASSETS)) {
  console.error('[FAIL] 未找到 web/dist/assets —— 请先跑 `npm run build -w web`');
  console.error('       门禁为 fail-closed:产物缺失不等于通过。');
  process.exit(1);
}

/** 去哈希:去掉扩展名前的最后一段 `-<hash>`(`index-CQ9DXqUA.js` → `index.js`) */
function dehash(name) {
  return name.replace(/-[^-]+\.(js|css)$/, '.$1');
}

function gz(file) {
  return gzipSync(readFileSync(file)).length;
}

function raw(file) {
  return statSync(file).size;
}

// ===== 读取 index.html 的预载与入口 =====
const htmlPath = join(DIST, 'index.html');
if (!existsSync(htmlPath)) {
  console.error('[FAIL] 未找到 web/dist/index.html —— 请先跑 `npm run build -w web`');
  process.exit(1);
}
const html = readFileSync(htmlPath, 'utf8');

const preload = [...html.matchAll(/rel="(?:modulepreload|stylesheet)"[^>]*href="\/assets\/([^"]+)"/g)].map(
  (m) => m[1],
);
const entryMatch = html.match(/src="\/assets\/([^"]+)"/);
if (!entryMatch) {
  console.error('[FAIL] index.html 里找不到入口 script(src="/assets/…")——产物结构异常');
  process.exit(1);
}
const entry = entryMatch[1];

// ===== 检查 1:首屏不得预载弹窗 chunk =====
for (const p of preload) {
  const bad = FORBIDDEN_PRELOAD_PREFIXES.find((prefix) => p.startsWith(prefix));
  if (bad) {
    fail.push(
      `首屏预载了弹窗 chunk:${p}\n` +
        `        弹窗必须经动态 import() 懒加载(node_modules 之外的 manualChunks 归组是常见成因,\n` +
        `        详见 web/vite.config.ts 的 P-8 说明)。`,
    );
  }
}

// ===== 检查 2:首屏集合 gzip 总量 =====
const firstPaint = [...new Set([...preload, entry])];
let firstPaintGz = 0;
const missing = [];
for (const p of firstPaint) {
  const abs = join(ASSETS, p);
  if (!existsSync(abs)) {
    missing.push(p);
    continue;
  }
  firstPaintGz += gz(abs);
}
if (missing.length) {
  fail.push(`首屏引用了不存在的产物:${missing.join(', ')}(产物不完整)`);
}

// ===== 检查 3:逐 chunk 预算 =====
const files = readdirSync(ASSETS).filter((f) => /\.(js|css)$/.test(f));
const perChunk = [];
let allGz = 0;
for (const f of files) {
  const abs = join(ASSETS, f);
  const g = gz(abs);
  allGz += g;
  const key = dehash(f);
  const budget = CHUNK_BUDGETS_GZ[key] ?? DEFAULT_CHUNK_BUDGET_GZ;
  perChunk.push({ file: f, key, gz: g, raw: raw(abs), budget, listed: key in CHUNK_BUDGETS_GZ });
}

// ===== 检查 4:全部资产总量 =====
perChunk.sort((a, b) => b.gz - a.gz);

for (const c of perChunk) {
  if (c.gz > c.budget) {
    fail.push(
      `chunk 超预算:${c.file} → gzip ${c.gz} > 预算 ${c.budget}(超 ${c.gz - c.budget})` +
        (c.listed ? '' : `\n        该前缀未列入 CHUNK_BUDGETS_GZ,走默认上限 ${DEFAULT_CHUNK_BUDGET_GZ}`),
    );
  }
}

// 首屏总量与全部资产总量必须**同样入 fail 列表**:首屏涨了但每个 chunk 都在各自预算内
// (例如多个 chunk 各涨一点)是真实回归形态,只打印不判定等于门禁失效。
if (firstPaintGz > FIRST_PAINT_BUDGET_GZ) {
  fail.push(
    `首屏合计超预算 → gzip ${firstPaintGz} > 预算 ${FIRST_PAINT_BUDGET_GZ}` +
      `(超 ${firstPaintGz - FIRST_PAINT_BUDGET_GZ})`,
  );
}
if (allGz > ALL_ASSETS_BUDGET_GZ) {
  fail.push(
    `全部资产超预算 → gzip ${allGz} > 预算 ${ALL_ASSETS_BUDGET_GZ}(超 ${allGz - ALL_ASSETS_BUDGET_GZ})`,
  );
}

// 预算表里已不存在的条目:提示清理(不 FAIL——正常重构会合并/改名 chunk)
const seenKeys = new Set(perChunk.map((c) => c.key));
for (const key of Object.keys(CHUNK_BUDGETS_GZ)) {
  if (!seenKeys.has(key)) warn.push(`预算表条目 ${key} 在产物中已不存在,可删除`);
}

// ===== 报告 =====
console.log('========== Kedai 前端体积预算(bundle budget)==========');
console.log(`  入口:${entry}`);
console.log(
  `  首屏预载:${preload.length} 项(${preload.join(', ')})`,
);
const fpDelta = firstPaintGz - FIRST_PAINT_BUDGET_GZ;
console.log(
  `  首屏合计(entry + 预载, gzip):${firstPaintGz} / 预算 ${FIRST_PAINT_BUDGET_GZ}` +
    `[${fpDelta > 0 ? '超预算' : `余 ${-fpDelta}`}]`,
);
const allDelta = allGz - ALL_ASSETS_BUDGET_GZ;
console.log(
  `  全部资产(gzip):${allGz} / 预算 ${ALL_ASSETS_BUDGET_GZ}[${allDelta > 0 ? '超预算' : `余 ${-allDelta}`}]`,
);
console.log(`  chunk 数:${perChunk.length}`);

if (VERBOSE) {
  console.log('\n  -- 逐 chunk(gzip / raw / 预算) --');
  for (const c of perChunk) {
    const mark = c.gz > c.budget ? '[超预算]' : '';
    console.log(
      `  ${String(c.gz).padStart(7)} / ${String(c.raw).padStart(8)} / ${String(c.budget).padStart(7)}  ${c.file}` +
        `${c.listed ? '' : ' (默认上限)'}${mark}`,
    );
  }
}

for (const w of warn) console.log(`  [WARN] ${w}`);

if (fail.length) {
  console.log(`\n[FAIL] ${fail.length} 项体积超预算:`);
  for (const f of fail) console.log(`  - ${f}`);
  console.log(
    '\n  说明:先确认增长是否有正当理由(新功能/新依赖)。有则把预算上调到实测值 + 10%;' +
      '\n        无则应修代码(拆分 chunk、改回懒加载、去掉重复依赖),而不是调预算。',
  );
  process.exit(1);
}

console.log('\n[ OK ] 体积预算内,且首屏无弹窗 chunk 预载');
