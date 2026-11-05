/**
 * Kedai 文档一致性门禁(tools/check-docs.mjs)
 *
 * 背景:2026-09-16 把 68 份散落 md 整合为「契约/功能/计划/展望/经验/遗留」六类体系,
 * 并归档到 docs/archive/2026-09-16-consolidation/。整合的成果若无守护,会缓慢退化回
 * 「根下又散落一堆文档」「索引与实际不一致」「链接指向已归档文件」的状态。
 * 本脚本把该约定变成硬门禁(此前仓库没有任何 docs/ 的机器校验)。
 *
 * 用法:
 *   node tools/check-docs.mjs            # 校验,失败 exit 1
 *   node tools/check-docs.mjs --verbose  # 额外打印通过项明细
 *   node tools/check-docs.mjs --json     # 机器可读输出
 *
 * 规则:
 *   D1 活文档均存在（六类 + 裁定分册）
 *   D2 docs/ 根下只允许白名单(活文档 + README.md + fixtures/ + archive/ + plans/ + 已登记子目录)
 *   D3 docs/README.md 的活文档表格与实际文件**双向一致**(列了必须存在;存在必须列了)
 *   D4 活文档与子目录正文分卷内的相对 markdown 链接目标必须存在(锚点尽量校验)
 *   D5 归档映射表覆盖归档目录下全部文件
 *   D6 (WARN) 计划/遗留条目应带 ID(P- / L / T / D / 批次 / 项)
 *   D7 仓库根只允许 README.md / AGENTS.md / MAINTENANCE.md 三份约定文档
 *   D8 门面索引完整性(子目录存在时:门面必须链接全部子文件,且不得链接不存在者)
 *
 * 失败只影响本脚本自身结论;check-all.ps1 侧用统一 Invoke-Stage 聚合,
 * **不要**另建失败桶(见 MAINTENANCE.md §0「修复纪律」:曾因前端分支写进后端失败桶
 * 导致报告段被静默吞掉)。
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DOCS = path.join(ROOT, 'docs');
const VERBOSE = process.argv.includes('--verbose');
const JSON_OUT = process.argv.includes('--json');

// 活文档(2026-09-16 六类体系 + 2026-11-04 裁定分册)
const CLASSIFIED = [
  'docs/契约.md',
  'docs/契约-协议与配置.md',
  'docs/契约-架构与数据.md',
  'docs/功能.md',
  'docs/功能-变更史.md',
  'docs/计划.md',
  'docs/裁定.md',
  'docs/展望.md',
  'docs/经验.md',
  'docs/遗留.md',
];

// 拆分后的正文子卷目录(门面 + 子目录方案;X-2 起允许存在)
const DOC_SUBDIRS = ['docs/功能-变更史', 'docs/计划', 'docs/经验', 'docs/功能'];

// docs/ 根下允许出现的条目(D2 白名单)
const ROOT_ALLOW = new Set([...CLASSIFIED, 'docs/README.md']);
const ROOT_ALLOW_DIRS = new Set(['docs/fixtures', 'docs/archive', 'docs/plans', ...DOC_SUBDIRS]);

// 仓库根允许出现的 md(D7 白名单;其余过程稿一律进 docs/plans/ 或 docs/archive/)
const REPO_ROOT_MD_ALLOW = new Set(['README.md', 'AGENTS.md', 'MAINTENANCE.md']);

const ARCHIVE_DIR = 'docs/archive/2026-09-16-consolidation';

/** 检查条目:level 为 'FAIL' | 'WARN' */
const findings = [];
const fail = (rule, msg) => findings.push({ level: 'FAIL', rule, msg });
const warn = (rule, msg) => findings.push({ level: 'WARN', rule, msg });
const okLog = [];
const ok = (rule, msg) => okLog.push(`[ OK ] ${rule} ${msg}`);

const read = (p) => fs.readFileSync(path.join(ROOT, p), 'utf8');
const exists = (p) => fs.existsSync(path.join(ROOT, p));
const rel = (abs) => path.relative(ROOT, abs).split(path.sep).join('/');

// ---------- D1 活文档存在(六类 + 裁定分册) ----------
for (const f of CLASSIFIED) {
  if (!exists(f)) fail('D1', `活文档缺失:${f}`);
}
if (CLASSIFIED.every(exists)) ok('D1', `活文档齐备(${CLASSIFIED.length} 份：六类 + 裁定分册)`);

// ---------- D2 docs/ 根下只允许白名单 ----------
if (!fs.existsSync(DOCS)) {
  fail('D2', 'docs/ 目录不存在');
} else {
  const stray = [];
  for (const e of fs.readdirSync(DOCS, { withFileTypes: true })) {
    const p = `docs/${e.name}`;
    if (e.isDirectory()) {
      if (!ROOT_ALLOW_DIRS.has(p)) stray.push(p + '/');
      continue;
    }
    if (!ROOT_ALLOW.has(p)) stray.push(p);
  }
  if (stray.length) {
    fail(
      'D2',
      `docs/ 根下出现未登记条目(应归档、或先在 docs/README.md 登记分类):\n` +
        stray.map((s) => `       - ${s}`).join('\n')
    );
  } else {
    ok('D2', 'docs/ 根下无未登记条目');
  }
}

// ---------- D7 仓库根只允许三份约定 md ----------
// 背景(2026-10-25 C-2E):根目录曾长期滞留 P0-HANDOFF / 更新说明稿等过程稿。
// docs/README.md 自述「仓库根只留 README/AGENTS/MAINTENANCE」,但 D2 只守 docs/ 根;
// 本规则把该叙述变成硬门禁:过程稿应进 docs/plans/(调研/报告)或 docs/archive/(历史)。
{
  const strayRootMd = fs
    .readdirSync(ROOT, { withFileTypes: true })
    .filter((e) => e.isFile() && e.name.endsWith('.md') && !REPO_ROOT_MD_ALLOW.has(e.name))
    .map((e) => e.name);
  if (strayRootMd.length) {
    fail(
      'D7',
      `仓库根出现未约定的 md(过程稿请移入 docs/plans/ 或 docs/archive/):\n` +
        strayRootMd.map((s) => `       - ${s}`).join('\n')
    );
  } else {
    ok('D7', '仓库根只有 README.md / AGENTS.md / MAINTENANCE.md');
  }
}

// ---------- D3 docs/README.md 索引与实际双向一致 ----------
const README = 'docs/README.md';
if (!exists(README)) {
  fail('D3', `索引文件缺失:${README}`);
} else {
  const src = read(README);
  const linked = new Set();
  // 收集索引表里指向 docs/*.md 的链接(排除 archive/、fixtures/)
  const linkRe = /\]\((?!https?:)([^)#]+\.md)(?:#[^)]*)?\)/g;
  let m;
  while ((m = linkRe.exec(src))) {
    const t = m[1].replace(/^(\.\.\/)+/, '').replace(/^\.\//, '');
    if (t.startsWith('archive/') || t.startsWith('fixtures/')) continue;
    // 根文档(AGENTS/README/MAINTENANCE)用 ../ 形式指向仓库根,不算 docs 体系
    if (!src.slice(Math.max(0, m.index - 60), m.index).includes('../')) {
      // docs 相对解析(而非 basename):照 basename 会把 `plans/X.md` 塌缩成 `docs/X.md`
      // 并误判为 citingGhost。对全部单段目标(活文档都在 `docs/` 根下)等价。
      linked.add('docs/' + path.posix.normalize(t));
    }
  }
  const missingInIndex = CLASSIFIED.filter((f) => !linked.has(f));
  const citingGhost = [...linked].filter((f) => !exists(f));
  if (missingInIndex.length) {
    fail('D3', `新增活文档未登记进 ${README}:\n` + missingInIndex.map((f) => `       - ${f}`).join('\n'));
  }
  if (citingGhost.length) {
    fail('D3', `${README} 引用了不存在的文档:\n` + citingGhost.map((f) => `       - ${f}`).join('\n'));
  }
  if (!missingInIndex.length && !citingGhost.length) {
    ok('D3', `${README} 索引与 docs/ 实际文件一致(${linked.size} 条活文档链接)`);
  }
}

// ---------- D4 活文档与子目录正文分卷的相对链接目标存在 ----------
{
  // 子卷不登记 CLASSIFIED(它们不是新的一类活文档),但仍须受链接校验保护。
  const subFiles = [];
  for (const d of DOC_SUBDIRS) {
    const abs = path.join(ROOT, d);
    if (!fs.existsSync(abs)) continue;
    for (const e of fs.readdirSync(abs, { withFileTypes: true })) {
      if (e.isFile() && e.name.endsWith('.md')) subFiles.push(`${d}/${e.name}`);
    }
  }
  const files = [...CLASSIFIED, README, ...subFiles].filter(exists);
  let checked = 0;
  const broken = [];
  for (const f of files) {
    const src = read(f);
    const re = /\]\((?!https?:|mailto:)([^)#]+?)(#[^)]*)?\)/g;
    let m;
    while ((m = re.exec(src))) {
      let target = m[1].trim();
      if (!target) continue;
      if (/\.(png|jpg|jpeg|gif|svg|json|rs|ts|vue|ps1|mjs)$/i.test(target) && !target.endsWith('.md')) {
        // 非 markdown 目标:仅当是相对路径时才校验
      }
      const abs = path.resolve(path.dirname(path.join(ROOT, f)), target);
      checked++;
      if (!fs.existsSync(abs)) broken.push(`${f} -> ${target}`);
    }
  }
  if (broken.length) {
    fail('D4', `活文档内链接目标不存在(${broken.length} 条):\n` + broken.map((b) => `       - ${b}`).join('\n'));
  } else {
    ok('D4', `活文档内相对链接全部有效(${checked} 条)`);
  }
}

// ---------- D5 归档映射表覆盖归档目录全部文件 ----------
if (!exists(ARCHIVE_DIR)) {
  warn('D5', `归档目录不存在,跳过:${ARCHIVE_DIR}(若尚未执行归档属预期)`);
} else {
  const indexPath = `${ARCHIVE_DIR}/README.md`;
  if (!exists(indexPath)) {
    fail('D5', `归档映射表缺失:${indexPath}`);
  } else {
    const idx = read(indexPath);
    const files = fs
      .readdirSync(path.join(ROOT, ARCHIVE_DIR), { withFileTypes: true })
      .filter((e) => e.isFile() && e.name.endsWith('.md') && e.name !== 'README.md')
      .map((e) => e.name);
    const uncovered = files.filter((n) => !idx.includes(n));
    const ghostRefs = [];
    const re = /\[`([^`]+\.md)`\]\(([^)]+)\)/g;
    let m;
    while ((m = re.exec(idx))) {
      const encoded = m[2];
      const abs = path.join(ROOT, ARCHIVE_DIR, decodeURIComponent(encoded));
      if (!fs.existsSync(abs)) ghostRefs.push(m[1]);
    }
    if (uncovered.length) {
      fail(
        'D5',
        `归档目录下有 ${uncovered.length} 份文件未登记进映射表:\n` +
          uncovered.map((n) => `       - ${n}`).join('\n')
      );
    }
    if (ghostRefs.length) {
      fail('D5', `映射表引用了不存在的归档文件:\n` + ghostRefs.map((n) => `       - ${n}`).join('\n'));
    }
    if (!uncovered.length && !ghostRefs.length) {
      ok('D5', `归档映射表覆盖全部 ${files.length} 份归档文档`);
    }
  }
}

// ---------- D8 门面索引完整性（门面 ↔ 子目录双向） ----------
{
  let dirCount = 0;
  let fileCount = 0;
  const problems = [];
  for (const dir of DOC_SUBDIRS) {
    const absDir = path.join(ROOT, dir);
    if (!fs.existsSync(absDir)) continue;
    dirCount += 1;
    const facade = `${dir}.md`;
    if (!exists(facade)) {
      problems.push(`子目录 ${dir}/ 存在但门面 ${facade} 不存在`);
      continue;
    }
    const files = fs
      .readdirSync(absDir, { withFileTypes: true })
      .filter((e) => e.isFile() && e.name.endsWith('.md'))
      .map((e) => e.name)
      .sort();
    fileCount += files.length;
    const src = read(facade);
    const linked = new Set();
    const ghosts = new Set();
    const re = /\]\((?!https?:|mailto:)([^)#]+?)(?:#[^)]*)?\)/g;
    let m;
    while ((m = re.exec(src))) {
      const target = m[1].trim();
      if (!target) continue;
      const abs = path.resolve(path.dirname(path.join(ROOT, facade)), target);
      const relToDir = path.relative(absDir, abs);
      if (!relToDir || relToDir.startsWith('..') || path.isAbsolute(relToDir)) continue;
      if (!relToDir.endsWith('.md') || relToDir.includes(path.sep)) continue;
      linked.add(relToDir);
      if (!files.includes(relToDir)) ghosts.add(relToDir);
    }
    const missing = files.filter((n) => !linked.has(n));
    if (missing.length) problems.push(`${facade} 未链接子文件:${missing.join(', ')}`);
    if (ghosts.size) problems.push(`${facade} 链接了不存在的子文件:${[...ghosts].join(', ')}`);
  }
  if (problems.length) {
    fail('D8', `门面索引不完整:\n` + problems.map((p) => `       - ${p}`).join('\n'));
  } else if (dirCount === 0) {
    ok('D8', '尚无子目录（拆分未开始）');
  } else {
    ok('D8', `门面索引覆盖全部 ${fileCount} 个子文件（${dirCount} 个目录）`);
  }
}

// ---------- D6 (WARN) 条目 ID 存在性 ----------
// 拆分后条目正文移入 docs/<门面>/ 子卷:D6 必须跟着扫描子卷,否则门面只剩索引,
// 「编号是全仓引用锚点」这条检查会在拆分后静默失效(与 D4/D8 扩范围的同一理由)。
{
  const idRules = [
    { file: 'docs/遗留.md', re: /^#{2,3}\s+.*\b(L\d{1,2}|T\d|D\d)\b/m, hint: 'L*/T*/D*' },
    { file: 'docs/计划.md', dir: 'docs/计划', re: /^#{2,3}\s+.*(P-\d+|批次\s*\d|[A-Z]\.\d)/m, hint: 'P-*/批次 N/X.N' },
  ];
  for (const { file, dir, re, hint } of idRules) {
    if (!exists(file)) continue;
    const body = [read(file)];
    if (dir && exists(dir)) {
      for (const e of fs.readdirSync(path.join(ROOT, dir), { withFileTypes: true })) {
        if (e.isFile() && e.name.endsWith('.md')) body.push(read(`${dir}/${e.name}`));
      }
    }
    const h2 = (body[0].match(/^##\s+/gm) || []).length;
    if (h2 === 0) warn('D6', `${file} 无二级章节,疑似结构被破坏`);
    if (!body.some((src) => re.test(src))) {
      warn('D6', `${file} 未见条目 ID 形态(${hint}),编号是全仓引用锚点,请勿改成无语义标题`);
    }
  }
  ok('D6', '条目 ID 形态检查完成(仅 WARN 级)');
}

// ---------- 输出 ----------
const fails = findings.filter((f) => f.level === 'FAIL');
const warns = findings.filter((f) => f.level === 'WARN');

if (JSON_OUT) {
  console.log(JSON.stringify({ ok: fails.length === 0, findings }, null, 2));
} else {
  if (VERBOSE) {
    console.log('\n---------- 通过项 ----------');
    okLog.forEach((l) => console.log(l));
  }
  if (warns.length) {
    console.log('\n[WARN] 文档检查警告(不阻断):');
    warns.forEach((w) => console.log(`  - [${w.rule}] ${w.msg}`));
  }
  if (fails.length) {
    console.log('\n[FAIL] 文档一致性检查未通过:');
    fails.forEach((f) => console.log(`  - [${f.rule}] ${f.msg}`));
    console.log('\n活文档体系约定见 docs/README.md。');
    process.exit(1);
  }
  console.log('\n[ OK ] 文档体系一致(六类 + 裁定分册;docs/ 结构 / 索引 / 链接 / 归档映射 / 门面索引)');
}

process.exit(fails.length ? 1 : 0);
