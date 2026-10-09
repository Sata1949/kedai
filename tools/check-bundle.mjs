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
 * 预算基线(2026-09-26 自定义流程 A+B 批后重测,「实测 + 10%」;此前为
 *   2026-09-20 二维批次 3 画布落地后的实测值)。
 * 首屏 288,000 → 317,030(2026-10-07,CU-1 安全前提批后重测):该批把「停止/恢复操作电脑」
 *   急停入口放进 Agent 面板头(首屏组件链)并新增 `api/computerUse.ts`(随 AgentPanel
 *   静态引入进 index chunk);首屏实测 288,209 gz,越过 288,000(超 209)。属功能正常增长
 *   (急停是安全入口,不拆懒加载——面板头按钮须随首屏可用),按协议上调到 288,209 + 10%
 *   ≈ 317,030。同批记录:全部资产实测 512,701 / 预算 557,300(余 44,599,未动)。
 * 全部资产 506,000 → 557,300(2026-10-05,文学能力包 LIT-1~3 批后重测):该批新增
 *   「文学能力包」懒加载设置分区(`LiteraryBundleSection`,自带 chunk 实测 1,627 gz)
 *   并在 index chunk 注册懒加载引用;全部资产实测 506,580 gz,越过 506,000(超 580)。
 *   属功能正常增长,按协议上调到 506,580 + 10% ≈ 557,300。
 *   同批记录:首屏实测 285,213 / 预算 288,000(余 2,787,已知偏紧项——本批未超、未动,
 *   下次功能增长由当批按同一协议上调)。
 * index.css 18,500 → 19,700(2026-10-02,UIP-6~8 观感批后重测):该批做动效令牌收口
 *   (tokens.css 扩档 + 12 处裸时长挂令牌),index.css 实测 17,753 → 17,906 gz;
 *   而 18,500 对现值只剩 594 B 余量,任何正常样式改动都会顶线。按协议上调到
 *   17,906 + 10% ≈ 19,700;同时对冲新增样式纪律门禁(`tools/check-css-discipline.mjs`:
 *   全局域逐文件行数/!important 只降不升 + 动画/过渡裸时长归零),预算放宽不放松结构约束。
 *   其余 chunk 与首屏/总量预算本批未动(首屏 283,140/288,000 余 4,860,属已知偏紧项,
 *   下次功能增长由当批按同一协议上调)。
 * 实测值(2026-09-26):首屏 275,060 / 全部资产 477,970 / index 107,834 / flow-vendor 71,603 /
 *   vendor 70,815 / content-rendering 44,786 / vue-vendor 34,069 / index css 17,184。
 *   → 与 2026-09-20 相比,**只有 index 涨了 10,354 gz**(97,480 → 107,834),其余七项持平。
 *   该涨幅全部来自 A+B 批的前端新增代码(任务改绑入口 + 连接选择组件、导入覆盖模式、
 *   节点级超时/重试控件、三个任务侧调用闸设置项;提交 `df62067` / `6d46144`),属**功能增长**,
 *   故按本文件下方的既有协议上调对应预算(该批提交信息里的验收只跑了
 *   `npm test` / `typecheck` / `check-contract` / `check-arch` / `check-frontend-lint`,
 *   **未跑 bundle budget**,故当时未被发现——只跑部分门禁不等于门禁绿)。
 * 历史基线(2026-09-20):首屏 264,388 / 全部资产 459,928 / index 97,480 / flow-vendor 71,594 /
 *   vendor 70,815 / content-rendering 44,786 / vue-vendor 34,067 / index css 16,868。
 * index 118,600 → 131,300(2026-10-09,RPFLOW 批后重测):该批前端新增草稿事件在 Agent 面板
 *   的折叠展示、聊天档位持久化、设置「剧情推演词条同步」两档开关;index 实测 119,378 gz,
 *   越过 118,600(超 778)。属功能正常增长,按协议上调到 119,378 + 10% ≈ 131,300。
 *   同批记录:首屏实测 290,508 / 预算 317,030(余 26,522)、全部资产实测 516,895 / 预算
 *   557,300(余 40,405),两项本批未动。
 *
 * flow-vendor 必须单列一条(不能走默认上限):二维批次 3 引入的流程画布库
 *   (`@vue-flow/*` + 传递依赖 d3-* / @vueuse/core / vue-demi)整体 71,594 gz。
 *   它由 `web/vite.config.ts` 的 manualChunks 单列成块,只被执行流程编辑区里的画布
 *   异步组件引用,**不在首屏预载里**(首屏 5 项为 vue-vendor/vendor/content-rendering
 *   与两个 css),所以不影响首屏;但它远超「未列出前缀」的默认上限 12,000,不登记就会
 *   被默认上限判红——这正是默认上限的作用(拦住「有重依赖落进某个小 chunk」)。
 *   全部资产预算同步上调:画布库计入总量是事实,不能用「反正不首屏加载」绕过总量兜底。
 */
const FIRST_PAINT_BUDGET_GZ = 317030;
const ALL_ASSETS_BUDGET_GZ = 557300;

/**
 * 单 chunk 预算:键为「去哈希后的文件名」(`index.js` / `index.css` / `vendor.js` …)。
 * 只列体积有实际意义的 chunk;其余走 DEFAULT_CHUNK_BUDGET_GZ。
 * 哈希会随内容变化,故**必须按去哈希名索引**,不能写死文件名。
 */
const CHUNK_BUDGETS_GZ = {
  'index.js': 131300,
  'flow-vendor.js': 79000,
  'vendor.js': 78000,
  'content-rendering.js': 49500,
  'vue-vendor.js': 37500,
  'index.css': 19700,
};

/**
 * 未列出前缀的单 chunk 上限。当前最大的未列出 chunk 是 AgentFlowSection(gz 11,651,
 * 2026-09-26 实测;A+B 批给它加了「覆盖同名流程」勾选与相应提示后由 8,721 涨上来),
 * 其余均 < 8,400。12,000 仍能完成它「拦住有重依赖落进某个小 chunk」的职责;但 AgentFlowSection
 * 距上限只剩约 3%,下次再动它请先跑本脚本——真要上调,按上方协议(实测 + 10%)并在此登记实测值。
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

/**
 * 去哈希:把产物名折回预算表里的键(`index-CQ9DXqUA.js` → `index.js`)。
 *
 * **不能只剥最后一段 `-<hash>`**:Vite 的 base64url 哈希本身可能含 `-`
 * (实测产物里有 `index-n-WGUpyo.js`),那时 `-[^-]+` 只吃得掉 `-WGUpyo`,
 * 折出 `index-n.js` —— 于是入口 chunk 落到默认上限 12,000 被误判超预算,
 * 而预算表里的 `index.js` 反倒被报成「产物中已不存在」。哈希是否含 `-` 取决于
 * 产物内容,故这是随机触发的假失败(2026-09-22 实测:同一份脚本,内容一变即复发)。
 *
 * 改为**按预算表的键做前缀匹配**:产物名 = `<键干>-<hash>.<ext>`,故 `index-` 开头
 * 即归 `index.js`。匹配不到时退回原来的剥尾规则(未登记 chunk 仍按默认上限判)。
 */
function dehash(name) {
  const ext = name.endsWith('.css') ? '.css' : '.js';
  for (const key of Object.keys(CHUNK_BUDGETS_GZ)) {
    if (!key.endsWith(ext)) continue; // 扩展名必须同类:index-*.css 不能折成 index.js
    const stem = key.slice(0, -ext.length);
    if (name === key || name.startsWith(`${stem}-`)) return key;
  }
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
