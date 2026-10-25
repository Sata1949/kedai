#!/usr/bin/env node
/**
 * check-file-size.mjs — 超千行文件 ratchet(纯 Node 零依赖)。
 *
 * 背景(2026-10-25 C-2E;依据 `plans/CODEBASE-DEBT-AUDIT-2026-10-24.md` C-2E-2):
 *   审计实测超千行非测试源文件 30 个(2026-09-28 台账记 15 个,约 +100%),C-2 的
 *   「≤6」目标已被现实甩开。直接承诺清零不现实,先做「只降不升」:基线内文件行数
 *   不得增加;基线外新文件一旦越过阈值即 FAIL。先止增,再分批拆。
 *
 * 口径(与审计附录一致):
 *   - 源码域 = `server-rs/src/**\/*.rs` + `web/src/**\/*.ts|vue`;
 *   - 排除测试与测试辅助:`tests` 目录、`tests.rs`、`*_tests.rs`、`tests_*.rs`、
 *     `*.test.ts`、`*.spec.ts`、`*.d.ts`(测试代码不承担生产结构债);
 *   - 行数按文件逻辑行计(与 `wc -l` 同口径,末尾换行不额外计一行)。
 *
 * 用法:
 *   node tools/check-file-size.mjs            # 校验,超基线即 exit 1
 *   node tools/check-file-size.mjs --verbose  # 打印全部超阈值文件与基线对照
 *   node tools/check-file-size.mjs --update   # 重写基线(拆分后下调;默认只降不升)
 *   node tools/check-file-size.mjs --json     # 机器可读输出
 *
 * ratchet 纪律:基线只降不升。拆分/搬移使文件降到阈值以下时,下次触碰顺手跑
 * `--update` 把它移出基线;确需让某个文件变大,必须在提交正文说明「为什么现在不能拆」,
 * 并显式用 `--update --force` 重写——不要让门禁在无人知晓的情况下被放宽。
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BASELINE_PATH = path.join(ROOT, 'tools', 'file-size-baseline.json');
const VERBOSE = process.argv.includes('--verbose');
const JSON_OUT = process.argv.includes('--json');
const UPDATE = process.argv.includes('--update');
const FORCE = process.argv.includes('--force');

const SCAN_DIRS = [
  { dir: 'server-rs/src', exts: ['.rs'] },
  { dir: 'web/src', exts: ['.ts', '.vue'] },
];

/** 测试与测试辅助文件不纳入生产结构债口径。 */
function isTestFile(rel) {
  const base = path.basename(rel);
  return (
    rel.split('/').includes('tests') ||
    base === 'tests.rs' ||
    base.endsWith('_tests.rs') ||
    base.startsWith('tests_') ||
    base.endsWith('.test.ts') ||
    base.endsWith('.spec.ts') ||
    base.endsWith('.d.ts')
  );
}

/** 递归收集源码文件(返回相对仓库根、正斜杠路径)。 */
function collectFiles() {
  const out = [];
  const walk = (absDir, exts) => {
    if (!fs.existsSync(absDir)) return;
    for (const e of fs.readdirSync(absDir, { withFileTypes: true })) {
      const abs = path.join(absDir, e.name);
      if (e.isDirectory()) {
        walk(abs, exts);
        continue;
      }
      if (!exts.some((ext) => e.name.endsWith(ext))) continue;
      const rel = path.relative(ROOT, abs).split(path.sep).join('/');
      if (isTestFile(rel)) continue;
      out.push(rel);
    }
  };
  for (const { dir, exts } of SCAN_DIRS) walk(path.join(ROOT, dir), exts);
  return out.sort();
}

/** 逻辑行数(与 `wc -l` 同口径:末尾换行不额外计一行)。 */
function lineCount(src) {
  if (!src) return 0;
  const nl = (src.match(/\n/g) || []).length;
  return src.endsWith('\n') ? nl : nl + 1;
}

const baseline = JSON.parse(fs.readFileSync(BASELINE_PATH, 'utf8'));
const threshold = baseline.threshold;
const baselineFiles = baseline.files;
const current = new Map();
for (const rel of collectFiles()) {
  const src = fs.readFileSync(path.join(ROOT, rel), 'utf8');
  current.set(rel, lineCount(src));
}

if (UPDATE) {
  const next = {};
  const changed = [];
  for (const [rel, n] of current) {
    if (n <= threshold) continue;
    const old = baselineFiles[rel];
    if (old != null) {
      // 默认只允许下调;增长必须显式 --force,防止「顺手更新基线」绕过 ratchet。
      if (n > old && !FORCE) {
        console.error(`[FAIL] ${rel}: ${old} → ${n} 行,增长需显式 --force 并在提交正文说明`);
        process.exit(1);
      }
      if (n !== old) changed.push(`${rel}: ${old} → ${n}`);
      next[rel] = n;
    } else {
      if (!FORCE) {
        console.error(`[FAIL] ${rel}: ${n} 行是新超阈值文件,需先拆分;确需登记请用 --force`);
        process.exit(1);
      }
      changed.push(`${rel}: (新增) ${n}`);
      next[rel] = n;
    }
  }
  for (const rel of Object.keys(baselineFiles)) {
    if (!current.has(rel)) changed.push(`${rel}: (已删除/改名为测试)`);
  }
  const ordered = {};
  for (const rel of Object.keys(next).sort()) ordered[rel] = next[rel];
  fs.writeFileSync(
    BASELINE_PATH,
    `${JSON.stringify({ ...baseline, updated: '2026-10-25', files: ordered }, null, 2)}\n`
  );
  console.log(`基线已更新:${Object.keys(ordered).length} 个文件`);
  for (const c of changed) console.log(`  - ${c}`);
  process.exit(0);
}

const failures = [];
const notes = [];
for (const [rel, max] of Object.entries(baselineFiles)) {
  const n = current.get(rel);
  if (n == null) {
    notes.push(`${rel}: 基线条目已不存在(可跑 --update 移除)`);
    continue;
  }
  if (n > max) failures.push(`${rel}: ${n} 行 > 基线 ${max}(增加 ${n - max})`);
}
for (const [rel, n] of current) {
  if (n > threshold && !(rel in baselineFiles)) {
    failures.push(`${rel}: ${n} 行 > 阈值 ${threshold},且不在基线(新超千行文件)`);
  }
}
const oversized = [...current.entries()].filter(([, n]) => n > threshold).sort((a, b) => b[1] - a[1]);
const dropped = [...current.entries()].filter(([rel, n]) => n <= threshold && rel in baselineFiles);

if (JSON_OUT) {
  console.log(
    JSON.stringify(
      { ok: failures.length === 0, threshold, oversized, failures, notes, dropped: dropped.map(([r]) => r) },
      null,
      2
    )
  );
  process.exit(failures.length ? 1 : 0);
}

if (VERBOSE) {
  console.log(`超阈值文件(${oversized.length} 个,阈值 ${threshold}):`);
  for (const [rel, n] of oversized) {
    const max = baselineFiles[rel];
    console.log(`  ${String(n).padStart(5)}  ${max == null ? '(未登记)' : `基线 ${max}`}  ${rel}`);
  }
}
for (const n of notes) console.log(`[INFO] ${n}`);
for (const rel of dropped.map(([r]) => r)) console.log(`[INFO] ${rel} 已降到阈值以下,可移出基线`);

if (failures.length) {
  console.log(`\n[FAIL] 文件大小 ratchet 未通过(${failures.length} 项):`);
  for (const f of failures) console.log(`  - ${f}`);
  console.log('\n规则:超千行文件只降不升;新文件越过阈值即 FAIL。');
  console.log('拆分后跑 `node tools/check-file-size.mjs --update` 下调基线;');
  console.log('确需增长时用 `--update --force` 并在提交正文说明为什么现在不能拆。');
  process.exit(1);
}
console.log(
  `\n[ OK ] 文件大小 ratchet:超千行文件 ${oversized.length} 个,` +
    `基线 ${Object.keys(baselineFiles).length} 个,无增长`
);
