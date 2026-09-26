#!/usr/bin/env node
// count-tests.mjs — 自动统计后端/前端测试数量,防止文档里的测试数字持续漂移。
//
// 背景:MAINTENANCE.md / docs 中长期记录「后端 923(740 单测+183 集成)、前端 697(71 文件)」,
// 而实际已增长到后端 953、前端 732——数字靠手抄必然过期,故改为脚本统计。
//
// 用法:
//   node tools/count-tests.mjs            # 打印统计(人读)
//   node tools/count-tests.mjs --json     # 输出 JSON(供 check-all 比对)
//   node tools/count-tests.mjs --check    # 与 MAINTENANCE.md 中记录的数字比对,不一致则 exit 1
//
// 统计口径:
//   后端单元测试 = src/**/*.rs 中 #[test] / #[tokio::test] 出现次数
//   后端集成测试 = tests/**/*.rs 中同上(文件数另计)
//   前端测试文件 = web/src/**/*.test.ts 数量
//   前端用例     = 上述文件内 it( / test( 调用次数(排除 it.each/test.each 之外的误配)
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function walk(dir, filter, out = []) {
  if (!fs.existsSync(dir)) return out;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, filter, out);
    else if (filter(p)) out.push(p);
  }
  return out;
}

function countMatches(file, re) {
  const src = fs.readFileSync(file, 'utf8');
  return (src.match(re) || []).length;
}

// ===== 后端 =====
const rsFilter = (p) => p.endsWith('.rs');
const srcFiles = walk(path.join(ROOT, 'server-rs', 'src'), rsFilter);
const testFiles = walk(path.join(ROOT, 'server-rs', 'tests'), rsFilter);
const RUST_TEST_RE = /#\[(?:tokio::)?test(?:\]|\()/g;

const backendUnit = srcFiles.reduce((n, f) => n + countMatches(f, RUST_TEST_RE), 0);
const backendIntegration = testFiles.reduce((n, f) => n + countMatches(f, RUST_TEST_RE), 0);

// ===== 前端 =====
const tsFiles = walk(path.join(ROOT, 'web', 'src'), (p) => p.endsWith('.test.ts'));
// 用例:行首(可缩进) it( / test( ;排除 .each / .skip 等链式误配
const TS_CASE_RE = /^\s*(?:it|test)\s*\(/gm;
const frontendCases = tsFiles.reduce((n, f) => n + countMatches(f, TS_CASE_RE), 0);

const result = {
  backendUnit,
  backendIntegration,
  backendTotal: backendUnit + backendIntegration,
  backendIntegrationFiles: testFiles.length,
  frontendFiles: tsFiles.length,
  frontendCases,
};

if (process.argv.includes('--json')) {
  console.log(JSON.stringify(result, null, 2));
  process.exit(0);
}

const summary = [
  `后端单元测试 : ${backendUnit}`,
  `后端集成测试 : ${backendIntegration}(${testFiles.length} 个文件)`,
  `后端合计     : ${result.backendTotal}`,
  `前端测试文件 : ${tsFiles.length}`,
  `前端用例     : ${frontendCases}`,
].join('\n');

if (process.argv.includes('--check')) {
  // 2026-09-26 扩展(文档漂移收口批):
  // ① 原先只比对 MAINTENANCE.md 的**第一处**匹配,导致同文件 §11 的旧数字(单测 1071 /
  //    集成「34 个文件, 306 个」)长期不受守护、与 §2 的权威行自相矛盾。
  // ② §11 用**另一种句式**复写同一事实(「单元测试(…**1083 个**,以 `tools/count-tests.mjs` 为准)」、
  //    「API 集成测试(`tests/` 38 个文件,**324 个**,…)」、「(Vitest,**1188 个 / 113 文件**,…)」),
  //    故改为**逐行扫描**:两套句式、两份文档(MAINTENANCE.md 为真值源,根 README.md 允许复写
  //    但必须写对)全部比对,并补上一直漏掉的「集成文件数」。
  const problems = [];
  const hits = { backend: 0, backendFiles: 0, frontend: 0, frontendFiles: 0 };
  const cmp = (where, label, got, expected) => {
    if (got === undefined) return;
    if (Number(got) !== expected) problems.push(`${where} ${label}:文档 ${got} ≠ 实际 ${expected}`);
  };

  for (const rel of ['MAINTENANCE.md', 'README.md']) {
    const p = path.join(ROOT, rel);
    if (!fs.existsSync(p)) continue;
    const lines = fs.readFileSync(p, 'utf8').split('\n');

    lines.forEach((line, i) => {
      const where = `${rel}:${i + 1}`;
      // 句式 A:「N 个测试(M 单测 + K 集成[, F 个集成文件])」
      for (const m of line.matchAll(
        /(\d+)\s*个测试\s*\((\d+)\s*单测\s*\+\s*(\d+)\s*集成(?:,\s*(\d+)\s*个集成文件)?/g,
      )) {
        hits.backend += 1;
        if (m[4] !== undefined) hits.backendFiles += 1;
        cmp(where, '后端合计', m[1], result.backendTotal);
        cmp(where, '后端单测', m[2], backendUnit);
        cmp(where, '后端集成', m[3], backendIntegration);
        cmp(where, '集成文件数', m[4], testFiles.length);
      }
      // 句式 B:同一事实的复写(§11 三行)
      if (line.includes('单元测试(')) {
        const m = line.match(/\*\*(\d+) 个\*\*/);
        if (m) {
          hits.backend += 1;
          cmp(where, '后端单测(复写)', m[1], backendUnit);
        }
      }
      if (line.includes('API 集成测试(')) {
        const mf = line.match(/(\d+) 个文件/);
        const mc = line.match(/\*\*(\d+) 个\*\*/);
        if (mf) {
          hits.backendFiles += 1;
          cmp(where, '集成文件数(复写)', mf[1], testFiles.length);
        }
        if (mc) {
          hits.backend += 1;
          cmp(where, '后端集成(复写)', mc[1], backendIntegration);
        }
      }
      if (line.includes('npm test -w web`(Vitest')) {
        const m = line.match(/\*\*(\d+) 个 \/ (\d+) 文件\*\*/);
        if (m) {
          hits.frontend += 1;
          hits.frontendFiles += 1;
          cmp(where, '前端用例(复写)', m[1], frontendCases);
          cmp(where, '前端文件(复写)', m[2], tsFiles.length);
        }
      }
      // 句式 C:「N 个(M 文件)」
      for (const m of line.matchAll(/(\d+)\s*个\s*\((\d+)\s*文件/g)) {
        hits.frontend += 1;
        hits.frontendFiles += 1;
        cmp(where, '前端用例', m[1], frontendCases);
        cmp(where, '前端文件', m[2], tsFiles.length);
      }
    });
  }

  if (hits.backend === 0) problems.push('未找到后端测试数句式(「N 个测试(M 单测 + K 集成)」等),无法比对');
  if (hits.frontend === 0) problems.push('未找到前端用例数句式(「N 个(M 文件)」等),无法比对');

  if (problems.length) {
    console.error('[WARN] 测试数字与文档记录不一致(文档漂移):');
    problems.forEach((p) => console.error('       - ' + p));
    console.error('       请更新文档数字(唯一真值源:MAINTENANCE.md),或在文档中改为引用而不复写。');
    process.exit(1);
  }
  console.log(summary);
  console.log('\n[OK] 与 MAINTENANCE.md / README.md 记录一致');
  process.exit(0);
}

console.log(summary);
