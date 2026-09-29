#!/usr/bin/env node
// check-doc-claims.mjs — 文档「可计数事实」机检:把由代码/门禁脚本派生的事实与文档字面逐条比对。
//
// 背景(2026-09-26 文档漂移收口批):一次全仓盘点实测出 20 余处计数漂移,它们的共同点是
// **真值都能从代码或脚本里数出来,却没有门禁去对**——count-tests.mjs 只管测试数字(且原先
// 只比对文件里的第一处),check-docs.mjs 只管六类文档的结构/索引/链接。于是:
//   表数 28/30/31、ensure_* 9/11/13、MAPPINGS 10/11/18/21、check-arch 规则字母 A~K/A–L、
//   BASELINE_BARE_ERROR 105/1、SseEvent|TaskEventKind「十/十一」、check-all「8 个 stage」…
// 同一事实在不同文档各写一个数,谁也不报错。
//
// 本脚本**只锁已校正过的这几类计数**,不追求解析全部 prose:每条断言 = 「派生器(从代码/脚本
// 数出真值)」+「文档断言(文件 + 正则)」。带历史层标记的命中不计失配(各文档既定体例:
// 「原文 + 变更标注」双层、变更史记录「当时数值」,不得为了整齐而统一)。
//
// 用法: node tools/check-doc-claims.mjs [--verbose] [--json]
// 退出: 任一断言失配 → exit 1;输出「文件:行 + 现值 + 期望 + 该改哪一侧」。
//
// 新增一条断言的成本:在这里加一个 claim(派生器 + 文件/正则),并把该文档的现行句统一到派生值。
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const VERBOSE = process.argv.includes('--verbose');
const JSON_OUT = process.argv.includes('--json');

/** 历史层标记:命中这些字样的行属于「原文/变更标注」的历史层,只 INFO 不计失配。 */
const HISTORY_MARKERS = [
  /2026-09-15/,
  /不是笔误/,
  /原文照录/,
  /当时数值/,
];

const readFile = (rel) => fs.readFileSync(path.join(ROOT, rel), 'utf8');

/** 按 section 切出待检区段(避免把带日期的证据正文当现行句)。返回切片与行号偏移。 */
function sliceText(text, section) {
  if (!section) return { text, lineOffset: 0 };
  const m = text.match(section.start);
  if (!m) return { text, lineOffset: 0 };
  let endIdx = text.length;
  if (section.end) {
    const em = text.slice(m.index).match(section.end);
    if (em) endIdx = m.index + em.index;
  }
  return { text: text.slice(m.index, endIdx), lineOffset: text.slice(0, m.index).split('\n').length - 1 };
}

function lineOf(text, index) {
  return text.slice(0, index).split('\n').length;
}

function isHistory(line) {
  return HISTORY_MARKERS.some((re) => re.test(line));
}

// ===== 派生器(从代码/脚本数真值) =====

/** schema.rs 里的 CREATE TABLE 张数(FTS5 虚拟表用 CREATE VIRTUAL TABLE,天然不匹配)。 */
function schemaTableCount() {
  const src = readFile('server-rs/src/models/db/schema.rs');
  return (src.match(/CREATE TABLE IF NOT EXISTS/g) || []).length;
}

/** migration/ddl.rs 里的 `pub fn ensure_*` 定义数。 */
function ensureDefCount() {
  const src = readFile('server-rs/src/migration/ddl.rs');
  return (src.match(/pub fn ensure_\w+/g) || []).length;
}

/** models/db/mod.rs 升级事务内实际调用的 ensure_* 个数(BEGIN IMMEDIATE … COMMIT 之间)。 */
function ensureInTransactionCount() {
  const src = readFile('server-rs/src/models/db/mod.rs');
  const begin = src.indexOf('BEGIN IMMEDIATE');
  const end = src.indexOf('COMMIT', begin);
  if (begin < 0 || end < 0) return -1;
  return (src.slice(begin, end).match(/crate::migration::ensure_\w+/g) || []).length;
}

/** tools/check-contract.mjs 的 MAPPINGS 条数。 */
function mappingsCount() {
  const src = readFile('tools/check-contract.mjs');
  return (src.match(/^\s*label:/gm) || []).length;
}

/** tools/check-arch.mjs 实际出现的规则字母全集。 */
function archRuleLetters() {
  const src = readFile('tools/check-arch.mjs');
  const set = new Set((src.match(/\[[A-Z]\]/g) || []).map((s) => s[1]));
  return [...set].sort();
}

/** check-arch.mjs 头注释(前 40 行)里逐条列出的规则字母。 */
function archRuleLettersInHeader() {
  const head = readFile('tools/check-arch.mjs').split('\n').slice(0, 40).join('\n');
  return (head.match(/^\s*\*\s+([A-Z])\./gm) || []).map((s) => s.replace(/[^A-Z]/g, ''));
}

/** check-arch.mjs 内某个 ratchet 基线的当前字面值。 */
function baselineValue(name) {
  const m = readFile('tools/check-arch.mjs').match(new RegExp(`const ${name} = (\\d+)`));
  return m ? Number(m[1]) : -1;
}

/** types.rs 里某个枚举的变体个数(枚举块内 4 空格缩进的变体行)。 */
function enumVariantCount(name) {
  const src = readFile('server-rs/src/models/types.rs');
  const start = src.indexOf(`pub enum ${name} {`);
  if (start < 0) return -1;
  const block = src.slice(start, src.indexOf('\n}', start));
  return (block.match(/^ {4}[A-Z][A-Za-z0-9_]*\s*[({,]/gm) || []).length;
}

/** check-all.ps1 的 Invoke-Stage 调用数(不含函数定义)。 */
function checkAllStageCount() {
  const src = readFile('tools/check-all.ps1');
  return (src.match(/Invoke-Stage '/g) || []).length;
}

/** check-all.ps1 的独立审计段数(两份 `===== cargo/npm audit… =====` 标题)。 */
function checkAllAuditSegments() {
  const src = readFile('tools/check-all.ps1');
  return (src.match(/Write-Host "`n===== (?:cargo|npm) audit/g) || []).length;
}

/** 仓库根 `rust-toolchain.toml` 的 channel —— 工具链口径的唯一真值源(经验 E60/E63)。 */
function toolchainChannel() {
  const m = readFile('rust-toolchain.toml').match(/^\s*channel\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}

/**
 * `.github/workflows` 下**全部** workflow 文件里的 `dtolnay/rust-toolchain` 版本取值集合
 * (去重 + 排序)。该 action 不读 `rust-toolchain.toml`(经验 E63),故每个 job 的版本
 * 都只能手写 —— 2026-09-27 新增 ci-linux.yml 后版本字面量从 1 处变 3 处,
 * 「只读 ci.yml 第一处」等于给新增档开天窗,故改为扫全集。
 * 两种写法都收:`@1.97.1`(字面 pin)与 `with: toolchain: 1.97.1`(输入式);
 * 写成 `@stable` / `@master` 而不给 `toolchain:` 输入时集合为空 → 断言失败(期望 [channel])。
 */
function workflowToolchainPins() {
  const dir = '.github/workflows';
  const files = fs.readdirSync(path.join(ROOT, dir)).filter((f) => /\.ya?ml$/.test(f));
  const found = new Set();
  for (const f of files) {
    const src = readFile(`${dir}/${f}`);
    for (const m of src.matchAll(/uses:\s*dtolnay\/rust-toolchain@(\S+)/g)) {
      if (/^[\d.]+$/.test(m[1])) found.add(m[1]);
    }
    for (const m of src.matchAll(/^\s*toolchain:\s*["']?([\w.-]+)["']?\s*$/gm)) found.add(m[1]);
  }
  return [...found].sort();
}

/** MAINTENANCE.md 工具链表行的版本(句式:`| Rust 工具链 | 1.97.1 stable | …`)。 */
function maintenanceToolchainVersion() {
  const m = readFile('MAINTENANCE.md').match(/^\|\s*Rust 工具链\s*\|\s*([\d.]+)/m);
  return m ? m[1] : null;
}

/** docs/契约.md 的 MAPPINGS 镜像表行数(「已登记的 N 组映射」与「守什么」之间的表格)。 */
function contractMappingTableRows() {
  const src = readFile('docs/契约.md');
  const start = src.indexOf('**已登记的');
  const end = src.indexOf('**守什么**');
  if (start < 0 || end < 0) return -1;
  return (src.slice(start, end).match(/^\| \d+ \|/gm) || []).length;
}

/** docs/契约.md 的 check-arch 规则表行字母集合(### 2. 小节内)。 */
function contractArchRuleTableLetters() {
  const src = readFile('docs/契约.md');
  const start = src.indexOf('### 2. `tools/check-arch.mjs`');
  const end = src.indexOf('### 3.', start);
  if (start < 0 || end < 0) return null;
  const set = new Set();
  for (const m of src.slice(start, end).matchAll(/^\|\s*\*{0,2}([A-Z])\*{0,2}\s*\|/gm)) set.add(m[1]);
  return [...set].sort();
}

// ===== 断言表 =====
// 每条: id / desc / derive(派生器) / checks(文档断言;re 捕获组 1 必须是数字或字母串)。
// 约定:同一条 check 里**所有**非历史层命中都必须等于派生值;requireMatch 表示该文件必须至少有一处现行命中。
const CLAIMS = [
  {
    id: 'db-tables',
    desc: '数据库表数',
    derive: schemaTableCount,
    checks: [
      { file: 'MAINTENANCE.md', re: /共 (\d+) 张表/g, requireMatch: true },
      { file: 'docs/契约-架构与数据.md', re: /共 (\d+) 张表/g, requireMatch: true },
    ],
  },
  {
    id: 'ensure-in-tx',
    desc: '升级事务内 ensure_* 调用数',
    derive: ensureInTransactionCount,
    checks: [
      { file: 'docs/契约-架构与数据.md', re: /建表批 \+ (\d+) 个 `ensure_\*`/g, requireMatch: true },
    ],
  },
  {
    id: 'ensure-defs',
    desc: 'migration/ddl.rs 的 ensure_* 定义数',
    derive: ensureDefCount,
    checks: [
      // 现行句写「定义 15 个」;历史层(8/9/11)由 HISTORY_MARKERS 放行。
      { file: 'docs/契约-架构与数据.md', re: /定义 (\d+) 个在 `migration\/ddl\.rs`/g, requireMatch: false },
    ],
  },
  {
    id: 'contract-mappings',
    desc: 'check-contract.mjs 已登记映射组数',
    derive: mappingsCount,
    checks: [
      { file: 'docs/契约.md', re: /已登记的 (\d+) 组映射/g, requireMatch: true },
      { file: 'docs/契约.md', re: /这 (\d+) 组类型\/枚举的/g, requireMatch: true },
      { file: 'docs/README.md', re: /\((\d+) 组 MAPPINGS/g, requireMatch: true },
      { file: 'MAINTENANCE.md', re: /\((\d+) 组 MAPPINGS/g, requireMatch: true },
      { file: 'docs/功能-变更史.md', re: /check-contract\.mjs`（(\d+) 组映射/g, requireMatch: true,
        // 变更史正文里的「N 组映射」是各批的当时证据(文件头「数字纪律」允许保留),只检文末权威索引段
        section: { start: /\*\*事实权威与数字溯源\*\*/ } },
    ],
    extra: [
      {
        desc: 'docs/契约.md 的映射镜像表行数与脚本条数一致',
        derive: contractMappingTableRows,
        file: 'docs/契约.md',
        useClaimValue: true,
      },
    ],
  },
  {
    id: 'arch-rule-letters',
    desc: 'check-arch.mjs 规则字母全集',
    derive: archRuleLetters,
    checks: [
      { file: 'docs/契约.md', re: /架构护栏（规则 ([A-Z])[–~]([A-Z])）/g, requireMatch: true },
    ],
    extra: [
      { desc: 'check-arch.mjs 头注释逐条列出全部规则字母(K 曾漏列)', derive: archRuleLettersInHeader, file: 'tools/check-arch.mjs', useClaimValue: true },
      {
        desc: 'docs/契约.md 的规则表行字母集合与脚本一致',
        derive: contractArchRuleTableLetters,
        file: 'docs/契约.md',
        useClaimValue: true,
        // F 是历史空缺:脚本无 [F] 标记,文档以「缺口登记」行如实说明,故不计入集合比对
        ignoreLetters: ['F'],
      },
    ],
  },
  {
    id: 'baseline-bare-error',
    desc: 'check-arch 规则 G 的 BASELINE_BARE_ERROR',
    derive: () => baselineValue('BASELINE_BARE_ERROR'),
    checks: [
      { file: 'docs/契约.md', re: /BASELINE_BARE_ERROR = (\d+)/g, requireMatch: true },
    ],
  },
  {
    id: 'baseline-raw-json-body',
    desc: 'check-arch 规则 K 的 BASELINE_RAW_JSON_BODY',
    derive: () => baselineValue('BASELINE_RAW_JSON_BODY'),
    checks: [
      { file: 'docs/契约.md', re: /BASELINE_RAW_JSON_BODY = (\d+)/g, requireMatch: true },
    ],
  },
  {
    id: 'sse-event-variants',
    desc: 'SseEvent 变体数',
    derive: () => enumVariantCount('SseEvent'),
    checks: [
      { file: 'docs/契约.md', re: /`SseEvent` 枚举线格式 \| [^|\n]*；(\d+) 个变体/g, requireMatch: true },
      { file: 'docs/契约.md', re: /即 `type` 字面值（共 (\d+) 个）/g, requireMatch: true },
    ],
  },
  {
    id: 'task-event-kind-values',
    desc: 'TaskEventKind 取值数',
    derive: () => enumVariantCount('TaskEventKind'),
    checks: [
      { file: 'docs/契约.md', re: /`TaskEventKind` 枚举线格式 \| snake_case；(\d+) 个取值/g, requireMatch: true },
      { file: 'docs/契约.md', re: /`TaskEventKind` 全部取值（(\d+) 个）/g, requireMatch: true },
    ],
  },
  {
    id: 'check-all-stages',
    desc: 'check-all.ps1 的阶段数与审计段数',
    derive: checkAllStageCount,
    checks: [
      { file: 'docs/契约-协议与配置.md', re: /(\d+) 个 `Invoke-Stage`/g, requireMatch: true },
    ],
    extra: [
      {
        desc: 'check-all.ps1 独立审计段数',
        derive: checkAllAuditSegments,
        file: 'docs/契约-协议与配置.md',
        re: /(\d+) 个独立审计段/,
        expected: 2,
      },
    ],
  },
  {
    // 版本号是**字符串**,走 extra 的直比路径(checks 的 gotNorm 分支只支持数字与字母区间,
    // 见执行段 `typeof gotNorm === 'number' ? … : Array.isArray(expected) && …`)。
    id: 'toolchain-version',
    desc: 'Rust 工具链版本全仓一致(rust-toolchain.toml ⇄ .github/workflows/*.yml 每一处 ⇄ MAINTENANCE.md)',
    derive: toolchainChannel,
    checks: [],
    extra: [
      {
        desc: '`.github/workflows` 下所有 `dtolnay/rust-toolchain` 版本(该 action 不支持读 toml,经验 E63,故每一处都必须手写同步)',
        derive: workflowToolchainPins,
        file: '.github/workflows/*.yml',
        useClaimValue: true,
        wrapExpected: true,
      },
      {
        desc: 'MAINTENANCE.md 工具链行的版本',
        derive: maintenanceToolchainVersion,
        file: 'MAINTENANCE.md',
        useClaimValue: true,
      },
    ],
  },
  {
    id: 'api-routes-covered',
    desc: 'HTTP 端点清单穷尽性（后端注册 ↔ 契约.md 收录）',
    // 本断言走 extra 的集合直比（checks 通道只能表达数字或字母区间）
    checks: [],
    derive: () => 0,
    extra: [
      {
        desc: 'docs/契约.md 未收录的后端 (method,path)（有意缺口见 ROUTE_BASELINE）',
        derive: uncoveredRoutes,
        file: 'docs/契约.md',
        expected: [],
      },
      {
        desc: 'docs/契约.md 收录但后端未注册的 (method,path)',
        derive: docOnlyRoutes,
        file: 'docs/契约.md',
        expected: [],
      },
      {
        desc: 'docs/契约.md 声明的本脚本项数与实际条数一致',
        derive: claimCountMismatch,
        file: 'docs/契约.md',
        expected: [],
      },
    ],
  },
];

// ===== 派生器：HTTP 路由穷尽性（2026-09-28 文档漂移收口批新增） =====
// 目的：把「代码注册了端点、契约.md 没收录」与「契约.md 收录了、代码没有」这两类漂移
// 变成硬红——它是本批最大的一类文档漂移（曾一次盘出 10 组缺录端点 + 3 处方法写错）。
// 手法与 check-arch 的 registeredEdges 同型：集合差集 + 内联基线（有意缺口）。

/** 有意不登记的 (method,path)：非 /api 路由已在下方过滤，此处仅放「确认为有意缺口」者。 */
const ROUTE_BASELINE = [];

/** 归一化路由字面：去查询、`:id`→`{id}`、去尾部通配与尾斜杠、去省略号。 */
function normRoute(p) {
  let s = p.split('?')[0].replace(/…/g, '');
  s = s.replace(/:([A-Za-z_][A-Za-z0-9_]*)/g, '{$1}');
  s = s.replace(/\*+$/, '');
  s = s.replace(/\/+$/, '');
  return s;
}

/** 该路径字面是否以通配结尾（`/api/skills*` 之类的集合写法）。 */
const isWildcard = (p) => /\*+\s*$/.test(p.split('?')[0]);

/** 扫出源码里全部 `.route(` 调用的括号内文本（圆括号深度配平，跨行安全）。 */
function routeCallBodies(src) {
  const res = [];
  const needle = '.route(';
  let i = 0;
  while ((i = src.indexOf(needle, i)) !== -1) {
    let j = i + needle.length;
    let depth = 1;
    while (j < src.length && depth > 0) {
      const c = src[j];
      if (c === '(') depth += 1;
      else if (c === ')') depth -= 1;
      j += 1;
    }
    res.push(src.slice(i + needle.length, j - 1));
    i = j;
  }
  return res;
}

/** 后端实际注册的 (METHOD /api/path) 集合。 */
function backendRoutes() {
  const apiDir = path.join(ROOT, 'server-rs/src/api');
  const files = [path.join(apiDir, 'mod.rs')];
  for (const f of fs.readdirSync(path.join(apiDir, 'routes'))) {
    if (f.endsWith('.rs')) files.push(path.join(apiDir, 'routes', f));
  }
  const out = new Set();
  for (const f of files) {
    for (const call of routeCallBodies(fs.readFileSync(f, 'utf8'))) {
      const pm = call.match(/"([^"]+)"/);
      if (!pm || !pm[1].startsWith('/api/')) continue;
      const route = normRoute(pm[1]);
      const methods = new Set(
        [...call.matchAll(/\b(get|post|put|delete|patch)\s*\(/g)].map((m) => m[1].toUpperCase()),
      );
      for (const m of methods) out.add(`${m} ${route}`);
    }
  }
  return out;
}

/** 标题行（形态 A）里「方法 → 路径」的配对：方法在前、路径随后；并列路径沿用前面的方法集。 */
function pairTitle(title) {
  const out = [];
  let pending = [];
  let lastMethods = [];
  const re = /`(\/api\/[^`]*)`|(?<![\w/])(GET|POST|PUT|DELETE|PATCH)(?![A-Za-z])/g;
  let m;
  while ((m = re.exec(title))) {
    if (m[1]) {
      const methods = pending.length ? pending : lastMethods;
      for (const x of methods) out.push({ method: x, route: m[1] });
      lastMethods = methods;
      pending = [];
    } else {
      pending.push(m[2]);
    }
  }
  return out;
}

/** docs/契约.md 里声明的端点集合：`exact` 为精确 (METHOD 路径)，`prefix` 为通配 (METHOD 前缀)。 */
function documentedRoutes() {
  const lines = fs.readFileSync(path.join(ROOT, 'docs/契约.md'), 'utf8').split(/\r?\n/);
  const exact = new Set();
  const prefix = new Set();
  const push = (method, rawRoute) => {
    const route = normRoute(rawRoute);
    if (!route.startsWith('/api/')) return;
    (isWildcard(rawRoute) ? prefix : exact).add(`${method} ${route}`);
  };
  for (const raw of lines) {
    const l = raw.trim();
    // 形态 A：小节标题 `#### 方法…`/api/…`…`
    if (l.startsWith('#### ')) {
      for (const { method, route } of pairTitle(l)) push(method, route);
      continue;
    }
    // 形态 B：表格 `| 方法 | /api/… |`（方法可写 `GET/PUT/DELETE`）
    const b = l.match(
      /^\|\s*((?:GET|POST|PUT|DELETE|PATCH)(?:\s*\/\s*(?:GET|POST|PUT|DELETE|PATCH))*)\s*\|\s*(\/api\/[^|\s]*)/,
    );
    if (b) {
      for (const m of b[1].split('/').map((s) => s.trim())) push(m, b[2]);
      continue;
    }
    // 形态 C：表格 `| `方法 /api/…` |`（方法与路径同格）
    const c = l.match(/^\|\s*`((?:GET|POST|PUT|DELETE|PATCH)\s+\/api\/[^`]*)`/);
    if (c) {
      const parts = c[1].split(/\s+/);
      push(parts[0].toUpperCase(), parts.slice(1).join(' '));
    }
  }
  return { exact, prefix };
}

/** 代码侧条目是否被文档收录（精确或命中通配前缀）。 */
function coveredByDoc(doc, pair) {
  if (doc.exact.has(pair)) return true;
  const [method, route] = pair.split(' ');
  for (const pre of doc.prefix) {
    const [pm, pr] = pre.split(' ');
    if (pm === method && (route === pr || route.startsWith(`${pr}/`))) return true;
  }
  return false;
}

/** 代码有、文档无（扣掉有意缺口基线）。 */
function uncoveredRoutes() {
  const doc = documentedRoutes();
  return [...backendRoutes()]
    .filter((pair) => !coveredByDoc(doc, pair) && !ROUTE_BASELINE.includes(pair))
    .sort();
}

/** 文档有、代码无（通配条目按「前缀下是否至少有一个同方法端点」判定）。 */
function docOnlyRoutes() {
  const be = backendRoutes();
  const out = [];
  for (const pair of documentedRoutes().exact) {
    if (!be.has(pair)) out.push(pair);
  }
  for (const pre of documentedRoutes().prefix) {
    const [method, route] = pre.split(' ');
    const hit = [...be].some((x) => {
      const [xm, xr] = x.split(' ');
      return xm === method && (xr === route || xr.startsWith(`${route}/`));
    });
    if (!hit) out.push(`(通配)${pre}`);
  }
  return out.sort();
}

/** 自指：契约.md 声明的「已登记 N 项」必须等于 CLAIMS 实际条数。 */
function claimCountMismatch() {
  const m = fs
    .readFileSync(path.join(ROOT, 'docs/契约.md'), 'utf8')
    .match(/已登记\s*(\d+)\s*项/);
  const declared = m ? Number(m[1]) : -1;
  return declared === CLAIMS.length
    ? []
    : [`docs/契约.md 声明 ${declared} 项 ≠ 脚本实际 ${CLAIMS.length} 项（改脚本后须同步该文）`];
}

// ===== 执行 =====
const failures = [];
const notes = [];

function expectedText(id, value) {
  if (Array.isArray(value)) return value.join('/');
  return String(value);
}

for (const claim of CLAIMS) {
  const expected = claim.derive();
  if (expected === -1 || expected === null) {
    failures.push({ claim: claim.id, where: '(派生器)', msg: '无法从代码/脚本派生出真值(源码结构变了?)' });
    continue;
  }
  let live = 0;
  for (const check of claim.checks) {
    const full = readFile(check.file);
    const { text, lineOffset } = sliceText(full, check.section);
    const matches = [...text.matchAll(check.re)];
    if (matches.length === 0) {
      if (check.requireMatch) {
        failures.push({
          claim: claim.id,
          where: check.file,
          msg: `未找到断言句式(期望现值 ${expectedText(claim.id, expected)});该句可能被改写或删除`,
        });
      }
      continue;
    }
    for (const m of matches) {
      const localLine = lineOf(text, m.index);
      const line = localLine + lineOffset;
      const raw = text.split('\n')[localLine - 1];
      const got = m[2] ? `${m[1]}–${m[2]}` : m[1];
      const gotNorm = /^\d+$/.test(got) ? Number(got) : got.split(/[–~]/);
      const expNorm = /^\d+$/.test(got) ? Number(got) : got.split(/[–~]/);
      const ok =
        typeof gotNorm === 'number'
          ? gotNorm === expected
          // 字母区间(A–L):首尾字母必须与派生集合的首尾一致
          : Array.isArray(expected) &&
            gotNorm.length === 2 &&
            gotNorm[0] === expected[0] &&
            gotNorm[1] === expected[expected.length - 1];
      if (ok) {
        live += 1;
        continue;
      }
      if (isHistory(raw)) {
        notes.push(`${claim.id}: ${check.file}:${line} 历史层保留「${got}」(现行值 ${expectedText(claim.id, expected)})`);
        continue;
      }
      failures.push({
        claim: claim.id,
        where: `${check.file}:${line}`,
        msg: `${claim.desc}:文档「${got}」≠ 现值「${expectedText(claim.id, expected)}」｜原文: ${raw.trim().slice(0, 120)}`,
      });
    }
    if (check.requireMatch && live === 0 && matches.length > 0) {
      failures.push({
        claim: claim.id,
        where: check.file,
        msg: `断言句式只有历史层命中,缺现行句(现值 ${expectedText(claim.id, expected)})`,
      });
    }
  }

  for (const extra of claim.extra || []) {
    let got = extra.derive();
    // useClaimValue 默认拿派生的**标量**直比;派生器返回集合、而语义是「集合里每个元素
    // 都必须等于该标量」时(如「全部 workflow 的版本」),用 wrapExpected 把期望包成单元素集合。
    let target = extra.useClaimValue ? expected : extra.expected;
    if (extra.wrapExpected) target = [target];
    if (Array.isArray(got) && Array.isArray(extra.ignoreLetters)) {
      got = got.filter((l) => !extra.ignoreLetters.includes(l));
    }
    const same = Array.isArray(got) && Array.isArray(target)
      ? got.length === target.length && got.every((v, i) => v === target[i])
      : got === target;
    if (!same) {
      failures.push({
        claim: claim.id,
        where: extra.file,
        msg: `${extra.desc}:实测「${expectedText(claim.id, got)}」≠ 期望「${expectedText(claim.id, target)}」`,
      });
    } else if (VERBOSE) {
      notes.push(`${claim.id}: ${extra.desc} ✓ ${expectedText(claim.id, got)}`);
    }
  }

  if (VERBOSE && live > 0) {
    notes.push(`${claim.id}: ${claim.desc} = ${expectedText(claim.id, expected)}（现行命中 ${live} 处）`);
  }
}

if (JSON_OUT) {
  console.log(JSON.stringify({ ok: failures.length === 0, failures, notes }, null, 2));
  process.exit(failures.length ? 1 : 0);
}

console.log('========== Kedai 文档口径检查(计数类硬口径) ==========');
for (const n of notes) console.log(`  [INFO] ${n}`);
if (failures.length) {
  console.error('\n[FAIL] 文档计数与代码/脚本派生值不一致(文档漂移):');
  for (const f of failures) console.error(`       - [${f.claim}] ${f.where}\n         ${f.msg}`);
  console.error('\n       处置:① 若是文档过期 → 改文档到派生值;② 若是代码/脚本改了 → 同步文档与');
  console.error('       tools/check-doc-claims.mjs 的断言句式;③ 历史层请保留原句并加日期化复核注。');
  process.exit(1);
}
console.log(`\n[ OK ] ${CLAIMS.length} 项计数口径与代码/脚本一致`);
