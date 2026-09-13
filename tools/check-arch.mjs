#!/usr/bin/env node
/**
 * Kedai 架构护栏(Node 零依赖)。
 *
 * 前端两项检查:
 *   A. store 循环依赖 —— store 间顶层 import 构成的环。Pinia setup store 允许在
 *      action 内运行期 `useXStore()`,但顶层 import 成环会让初始化顺序变成隐式契约
 *      (谁先被 import 影响 setup 期行为),且无法单独替换任一 store。检出即 FAIL。
 *   B. 组件直改 store state —— 组件里 `store.field = ...` 绕过 action 直接赋值,
 *      使状态变更路径分散、无法在 action 里统一做副作用与校验。仅报「疑似绕过」
 *      (字段写了 action 才会被拦),存量无害赋值经 BASELINE 白名单豁免。
 *
 * 后端分层与冻结检查(2026-09-13 起,落实三结合 L1/L2 边界):
 *   C. `services/task_engine/**` 不得 import `task_service`(任务引擎依赖倒置)。
 *   D. L1(parsing/models/contracts)不得 `use crate::services::`(老层不被上层渗透)。
 *   E. `services/**` 生产代码不得出现 `.expect(`(连接池异常不得 panic 掉请求线程)。
 *   G. `api/**` 不得新增「裸 {error}」响应(错误形状 ratchet,数量不得超基线)。
 *   H. EJS 自研解释器 builtin 表条数不得增长(威胁模型 D5;冻结纪律的机器门禁)。
 *
 * 用法:
 *   node tools/check-arch.mjs
 *   node tools/check-arch.mjs --verbose   # 打印通过的检查项与白名单命中
 *
 * 新增白名单须写明理由:白名单条目代表「已确认无害、暂不修」,不是「检测不到」。
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SRC = join(ROOT, 'web', 'src');
const VERBOSE = process.argv.includes('--verbose');

/**
 * 已知且暂未偿还的 store 循环依赖(基线快照)。脚本对**新增**环 FAIL;
 * 基线内的环打印为待偿还债务,不阻塞。修完一对就删一行;全绿时应为空数组。
 *
 * 2026-09-12(M5 断环后):五个 store 原构成一个强连通分量
 * (character/chat/genSettings/uiPrefs/task 互相可达),已通过
 * sharedState.ts(共享状态外提)+ storeBridge.ts(动作回调注册)拆解为单向 DAG,
 * 故此处为空。新增环会立即被检出并判 FAIL——不要为了「过检查」往这里加条目,
 * 那等同于关掉护栏。
 */
const KNOWN_CYCLES = [];

const failures = [];
const notes = [];

/**
 * 弹窗 flag 白名单**单点派生**(批次 5.2):读取 web/src/modals.ts 的弹窗注册表,
 * 不再在此手抄 flag 名(此前含僵尸项 devToolsOpen,真名是 eventsOpen)。
 *
 * 解析契约:modals.ts 内每个弹窗必须是单独一行
 *   `modal('flag', '中文 label', () => import('./components/X.vue')),`
 * 解析失败(文件缺失/结构改写)时**显式报错并判 FAIL**——绝不静默返回空列表:
 * 白名单静默变空会让「组件直改弹窗开关」从合规变违规(或反之),护栏即失效。
 */
function readModalFlags() {
  const file = join(SRC, 'modals.ts');
  let src;
  try {
    src = readFileSync(file, 'utf8');
  } catch (e) {
    failures.push(`弹窗白名单派生失败:无法读取 web/src/modals.ts(${e.message})`);
    return [];
  }
  const start = src.indexOf('export const MODALS');
  const end = start === -1 ? -1 : src.indexOf('] as const;', start);
  if (start === -1 || end === -1) {
    failures.push(
      '弹窗白名单派生失败:web/src/modals.ts 的 `export const MODALS = [...] as const;` 结构未识别(改了结构请同步 check-arch.mjs 的解析契约)',
    );
    return [];
  }
  const body = src.slice(start, end);
  const entry = /^\s*modal\(\s*'([A-Za-z_$][\w$]*)'\s*,\s*'[^']*'\s*,\s*\(\)\s*=>\s*import\(/gm;
  const flags = [...body.matchAll(entry)].map((m) => m[1]);
  if (flags.length === 0) {
    failures.push(
      '弹窗白名单派生失败:web/src/modals.ts 的 MODALS 注册表解析出 0 条(解析契约已失效,请同步 check-arch.mjs)',
    );
    return [];
  }
  const dup = [...new Set(flags.filter((f, i) => flags.indexOf(f) !== i))];
  if (dup.length) {
    failures.push(`弹窗白名单派生失败:web/src/modals.ts 弹窗 flag 重复:${dup.join(', ')}`);
  }
  if (VERBOSE) {
    console.log(`  [B] 弹窗白名单由 web/src/modals.ts 派生:${flags.length} 项(${flags.join(', ')})`);
  }
  return flags;
}

/**
 * 组件直改 store state 的白名单:字段名 → 理由。
 * 这些字段是纯 UI 开关(弹窗显隐 / 抽屉开合 / 面板展开 / 加载错误提示),
 * 变更无副作用、无校验、无持久化需求,直改不构成风险;一旦为其补了 action
 * (如 renderHtml 已改走 setRenderHtml/toggleRenderHtml)应从白名单移出。
 * 注意:不在白名单的字段会被判为违规——这正是护栏生效的地方,新增 action
 * 后请同步把字段名从本集合删除。
 */
const UI_FLAG_WHITELIST = new Set([
  // 弹窗/面板显隐:由 web/src/modals.ts 弹窗注册表派生(新增弹窗本脚本零改动)
  ...readModalFlags(),
  // 抽屉/面板开合(sidebarOpen 无副作用;agentPanelOpen 的收合语义走 collapseAgentPanel)
  'sidebarOpen',
  'audioOpen',
  'callTraceOpen',
  'taskResultSummaryOpen',
  // 启动与加载状态提示
  'splashDone',
  'renderHintDismissed',
  'modalLoadError',
  'dataLoadError',
]);

// ---------- 工具 ----------

const isTest = (f) => /\.test\.ts$/.test(f);

/** 归一化为「文件名」键(仅比较同目录内 store 文件名)。 */
const baseName = (rel) => rel.split(sep).pop();

// ---------- A. store 循环依赖 ----------

function collectStores() {
  const dir = join(SRC, 'stores');
  const present = new Set(
    readdirSync(dir).filter((n) => n.endsWith('.ts') && !isTest(n)),
  );
  const out = new Map();
  for (const name of present) {
    const src = readFileSync(join(dir, name), 'utf8');
    // 只认顶层 import(行首 import);TS 项目里 specifier 通常省略 .ts 后缀
    const deps = [];
    for (const m of src.matchAll(/^import\s[^\n]*from\s+'\.\/([^']+)'/gm)) {
      const raw = baseName(m[1]);
      const target = raw.endsWith('.ts') ? raw : `${raw}.ts`;
      if (present.has(target) && target !== name) deps.push(target);
    }
    out.set(name, deps);
  }
  return out;
}

/** DFS 找有向环,返回规范化后的环列表(每环以字典序最小的节点开头去重)。 */
function findCycles(graph) {
  const cycles = new Map();
  const state = new Map(); // 0=未访问 1=在栈 2=完成
  const stack = [];
  const visit = (node) => {
    state.set(node, 1);
    stack.push(node);
    for (const next of graph.get(node) ?? []) {
      const s = state.get(next) ?? 0;
      if (s === 0) {
        visit(next);
      } else if (s === 1) {
        // 找到环:截取栈中 next..末尾
        const from = stack.indexOf(next);
        const ring = stack.slice(from).filter((x) => x !== 'storeFacade');
        if (ring.length > 1) {
          // 规范化:旋转到字典序最小开头,便于与基线比较
          let min = 0;
          for (let i = 1; i < ring.length; i++) if (ring[i] < ring[min]) min = i;
          const norm = ring.slice(min).concat(ring.slice(0, min));
          cycles.set(norm.join('>'), norm);
        }
      }
    }
    stack.pop();
    state.set(node, 2);
  };
  for (const node of graph.keys()) if ((state.get(node) ?? 0) === 0) visit(node);
  return [...cycles.values()];
}

/** 环与基线条目等价比较(节点集无序比较)。 */
function sameAsKnown(ring) {
  const key = [...ring].sort().join('|');
  return KNOWN_CYCLES.some((c) => [...c].sort().join('|') === key);
}

const stores = collectStores();
const cycles = findCycles(stores);
let newCycles = 0;
let knownCycles = 0;
for (const ring of cycles) {
  if (sameAsKnown(ring)) {
    knownCycles++;
    notes.push(`[债务] 已知循环依赖未偿还:${ring.join(' ↔ ')}(修完请从 KNOWN_CYCLES 删除)`);
  } else {
    newCycles++;
    failures.push(`新增 store 循环依赖:${ring.join(' → ')}(顶层 import 成环,须断环)`);
  }
}
if (VERBOSE) {
  console.log(`  [A] 扫描 ${stores.size} 个 store,发现 ${cycles.length} 个环(新增 ${newCycles},已知 ${knownCycles})`);
}

// ---------- B. 组件直改 store state ----------

/** 收集 .vue 文件(递归)。 */
function collectVue(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...collectVue(p));
    else if (name.endsWith('.vue')) out.push(p);
  }
  return out;
}

/** 找出组件内 store 绑定名:const X = useYStore() / const X = store(门面)。 */
function storeBindings(src) {
  const names = new Map();
  for (const m of src.matchAll(/(?:const|let)\s+([A-Za-z_$][\w$]*)\s*=\s*use[A-Z][\w]*Store\(\)/g)) {
    names.set(m[1], 'store');
  }
  // 门面 store.ts 的 useAppStore() 也匹配上面;补 `= store` 形态(别名导入)
  for (const m of src.matchAll(/(?:const|let)\s+([A-Za-z_$][\w$]*)\s*=\s*store\b/g)) {
    names.set(m[1], 'facade');
  }
  return names;
}

/** 找出 `<binding>.<field> = ` 形态的赋值(排除 == / === 与对象字面量键)。 */
function directWrites(src, bindings) {
  const hits = [];
  for (const [bind, kind] of bindings) {
    const re = new RegExp(`\\b${bind}\\.([A-Za-z_$][\\w$]*)\\s*(?:=[^=]|\\+=|-=|\\*=|\\|=)`, 'g');
    for (const m of src.matchAll(re)) {
      hits.push({ field: m[1], kind, index: m.index });
    }
  }
  return hits;
}

/** 取赋值处的行号(用于报错定位)。 */
const lineOf = (src, index) => src.slice(0, index).split('\n').length;

let writeHits = 0;
let whitelisted = 0;
for (const file of collectVue(SRC)) {
  const src = readFileSync(file, 'utf8');
  const bindings = storeBindings(src);
  if (!bindings.size) continue;
  for (const hit of directWrites(src, bindings)) {
    writeHits++;
    if (UI_FLAG_WHITELIST.has(hit.field)) {
      whitelisted++;
      if (VERBOSE) {
        console.log(`  [B] 白名单命中:${relative(ROOT, file)}:${lineOf(src, hit.index)} → .${hit.field}`);
      }
      continue;
    }
    failures.push(
      `组件直改 store state:${relative(ROOT, file)}:${lineOf(src, hit.index)} → .${hit.field} = ...(应走 action;若确为无害 UI 开关,加 UI_FLAG_WHITELIST 并写理由)`,
    );
  }
}
if (VERBOSE) {
  console.log(`  [B] 命中赋值 ${writeHits} 处(白名单 ${whitelisted},待修 ${writeHits - whitelisted})`);
}

// ---------- 汇总 ----------

// ========== 后端分层检查(C/D/E) ==========
//
// 分级策略:
//   C(任务引擎不得依赖 task_service)、D(L1 不得依赖 services)——
//     2026-09-13 已完成断环/清理,违规数归零,**永久硬门禁**(出现在这里即 FAIL)。
//   E(services 生产代码不得 .expect)、G(不得新增裸 {error})——
//     同批已清零 E;G 存量 117 处待逐个补 code(需逐处判定 HTTP 状态码),故走
//     ratchet(数量不得超基线)。二者违规即 FAIL。
//   H(EJS builtin 表条数)——
//     自研迷你 JS 引擎冻结纪律的机器门禁,同样是 ratchet(见该规则注释)。
const SERVER = join(ROOT, 'server-rs', 'src');

/** 递归收集指定后缀文件(相对 ROOT 的路径)。 */
function collect(dir, exts) {
  const out = [];
  const walk = (d) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      if (statSync(p).isDirectory()) walk(p);
      else if (exts.some((e) => name.endsWith(e))) out.push(p);
    }
  };
  if (statSync(dir, { throwIfNoEntry: false })?.isDirectory()) walk(dir);
  return out;
}

/** 生产代码行(剔除 #[cfg(test)] 之后的内容)的行号集合。 */
function productionLineCount(src) {
  const idx = src.search(/^#\[cfg\(test\)\]/m);
  return idx === -1 ? Infinity : src.slice(0, idx).split('\n').length;
}

const backendFailures = [];
const backendWarnings = [];

// --- 规则 C:task_engine 不得依赖 task_service(批次 B 断环) ---
{
  const dir = join(SERVER, 'services', 'task_engine');
  let hits = 0;
  for (const f of collect(dir, ['.rs'])) {
    const src = readFileSync(f, 'utf8');
    src.split('\n').forEach((line, i) => {
      if (/^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+crate::services::task_service/.test(line)) {
        hits++;
        backendFailures.push(
          `[C] 任务引擎反向依赖 task_service:${relative(ROOT, f)}:${i + 1}(应经 task_core::TaskBackend 接口)`,
        );
      }
    });
  }
  if (VERBOSE) console.log(`  [C] task_engine → task_service 违规 import:${hits} 处`);
}

// --- 规则 D:L1 不得依赖 services ---
{
  const ALLOW = new Set([join(SERVER, 'models', 'transport.rs')]); // 传输子域类型引用豁免
  let hits = 0;
  for (const top of ['parsing', 'models', 'contracts']) {
    for (const f of collect(join(SERVER, top), ['.rs'])) {
      if (ALLOW.has(f)) continue;
      const src = readFileSync(f, 'utf8');
      src.split('\n').forEach((line, i) => {
        if (/^\s*use\s+crate::services::/.test(line)) {
          hits++;
          backendFailures.push(
            `[D] L1 层(${top}/)反向依赖 services:${relative(ROOT, f)}:${i + 1}(L1 不得被上层渗透)`,
          );
        }
      });
    }
  }
  if (VERBOSE) console.log(`  [D] parsing/models/contracts → services 违规 import:${hits} 处`);
}

// --- 规则 E:services 生产代码不得 .expect( ---
{
  // 存量白名单:格式 "相对路径:行号"。批次 D 清零过程中逐条删除。
  const EXPECT_BASELINE = new Set();
  let hits = 0;
  for (const f of collect(join(SERVER, 'services'), ['.rs'])) {
    const src = readFileSync(f, 'utf8');
    const prodLines = productionLineCount(src);
    src.split('\n').forEach((line, i) => {
      const ln = i + 1;
      if (ln > prodLines) return; // 测试模块内豁免
      if (!/\.expect\(/.test(line)) return;
      // 注释行不算
      if (/^\s*\/\//.test(line)) return;
      const rel = relative(ROOT, f);
      if (EXPECT_BASELINE.has(`${rel}:${ln}`)) return;
      hits++;
      backendFailures.push(
        `[E] services 生产代码使用 .expect(:${rel}:${ln}(应返回 Result 或 tracing::error,不得 panic 请求线程)`,
      );
    });
  }
  if (VERBOSE) console.log(`  [E] services 生产 .expect( 违规:${hits} 处`);
}

// --- 规则 G:HTTP 错误响应不得新增「裸 {error}」形状(错误形状统一 ratchet) ---
//
// 背景:`api/errors.rs` 提供 `err_with_code`(body `{error, code}`),但历史上大量 handler
// 直接返回 `Json(json!({ "error": ... }))`——同一错误出现两种形状,前端无法统一解析。
// 存量 117 处**逐个**补齐 code 需为每处判定 HTTP 状态码(盲改会改 API 行为),属批次 D 的
// 跟踪项而非本轮范围;此处先加 ratchet 防新增(数量不得超基线,修好一批请下调基线)。
{
  const BASELINE_BARE_ERROR = 117; // 2026-09-13 实测;只降不升
  const apiDir = join(SERVER, 'api');
  let hits = 0;
  const samples = [];
  for (const f of collect(apiDir, ['.rs'])) {
    // errors.rs 自身是错误出口的定义处,豁免
    if (f.endsWith('errors.rs')) continue;
    const src = readFileSync(f, 'utf8');
    const prodLines = productionLineCount(src);
    src.split('\n').forEach((line, i) => {
      const ln = i + 1;
      if (ln > prodLines) return;
      if (/^\s*\/\//.test(line)) return;
      if (!/json!\(\s*\{\s*"error"/.test(line)) return;
      hits++;
      if (samples.length < 3) samples.push(`${relative(ROOT, f)}:${ln}`);
    });
  }
  if (hits > BASELINE_BARE_ERROR) {
    backendFailures.push(
      `[G] 裸 {error} 错误形状新增:${hits} 处 > 基线 ${BASELINE_BARE_ERROR}` +
        `(${samples.join(', ')});请改用 api/errors.rs 的 err_with_code 携带 code`,
    );
  }
  if (VERBOSE) {
    console.log(`  [G] 裸 {error} 响应:${hits} 处(基线 ${BASELINE_BARE_ERROR},只降不升)`);
  }
}

// --- 规则 H:EJS builtin 表冻结 ratchet(威胁模型 D5 / MAINTENANCE.md §0) ---
//
// 背景:`parsing/assistant/ejs/` 是自研迷你 JS 引擎(约 4000 行),纪律为「只接受安全
// 修复,不再扩展新能力」——任何新模板能力必须走 `scripts/runtime.rs` 的 rquickjs 沙箱。
// 此前该纪律仅靠文档约束(威胁模型 D5),此处把 `env.rs` 的 `builtin_global` 表**条数**
// 钉成 ratchet:超过基线即 FAIL。
//
// 计数口径:match 分支的字符串模式名(**别名各计一条**,别名同样是对模板暴露的新名字面)
// + JSON/Math/Object 等内联对象的成员名(成员也是新增能力面)。
//
// 基线纪律:**调低是唯一合法方向**。删除条目、收敛别名后请同步下调基线值;
// 上调等于关掉护栏,确属安全修复必须在变更说明里给出理由。
//
// 解析失败(文件缺失 / `fn builtin_global` 或 `match name` 或 `_ => None` 结构未识别 /
// 计出 0 条 / 出现无法分类的字面量)一律**显式 FAIL**,绝不静默跳过——静默返回 0 会让
// 门禁形同虚设(错误文案风格参照本文件 readModalFlags)。
{
  const EJS_ENV = join(SERVER, 'parsing', 'assistant', 'ejs', 'env.rs');
  const BASELINE_EJS_BUILTINS = 64; // 2026-09-13 实测;只降不升
  const rel = relative(ROOT, EJS_ENV);
  let src = null;
  try {
    src = readFileSync(EJS_ENV, 'utf8');
  } catch (e) {
    backendFailures.push(`[H] EJS builtin 表冻结检查失败:无法读取 ${rel}(${e.message})`);
  }
  if (src !== null) {
    const fnIdx = src.search(/fn\s+builtin_global\s*\(\s*name\s*:\s*&str\s*\)/);
    const matchIdx = fnIdx === -1 ? -1 : src.indexOf('match name', fnIdx);
    const endIdx = matchIdx === -1 ? -1 : src.indexOf('_ => None', matchIdx);
    if (fnIdx === -1 || matchIdx === -1 || endIdx === -1) {
      backendFailures.push(
        `[H] EJS builtin 表冻结检查失败:${rel} 的 \`fn builtin_global\` / \`match name\` / \`_ => None\` 结构未识别(重构了该函数请同步 check-arch.mjs 的解析契约)`,
      );
    } else {
      // 去注释后再数:注释里提到的函数名不算条目
      const body = src.slice(matchIdx, endIdx).replace(/\/\/[^\n]*/g, '');
      const names = [];
      const unknown = [];
      for (const m of body.matchAll(/"((?:[^"\\]|\\.)*)"/g)) {
        const after = body.slice(m.index + m[0].length);
        // 分支名(`"a" | "b" =>`,多别名各计一条)或内联对象成员名(`("name".into(), …)`)
        if (/^\s*(?:\|\s*"(?:[^"\\]|\\.)*"\s*)*=>/.test(after) || /^\s*\.into\s*\(/.test(after)) {
          names.push(m[1]);
        } else {
          unknown.push(m[1]);
        }
      }
      if (names.length === 0) {
        backendFailures.push(
          `[H] EJS builtin 表冻结检查失败:${rel} 的 builtin_global 解析出 0 条(解析契约已失效,请同步 check-arch.mjs)`,
        );
      } else if (unknown.length) {
        backendFailures.push(
          `[H] EJS builtin 表冻结检查失败:${rel} 的 builtin_global 出现无法分类的字面量:${unknown
            .map((s) => `"${s}"`)
            .join(', ')}(解析契约已失效,请同步 check-arch.mjs)`,
        );
      } else if (names.length > BASELINE_EJS_BUILTINS) {
        backendFailures.push(
          `[H] EJS builtin 表新增条目:${names.length} 条 > 基线 ${BASELINE_EJS_BUILTINS}(EJS 自研解释器已冻结:` +
            `只接受安全修复,新模板能力请走 scripts/runtime.rs 的 rquickjs 沙箱;确属安全修复请说明理由并**下调**基线)`,
        );
      }
      if (VERBOSE) {
        const mark = names.length < BASELINE_EJS_BUILTINS ? '(可下调基线)' : '';
        console.log(
          `  [H] EJS builtin 表:${names.length} 条(基线 ${BASELINE_EJS_BUILTINS},只降不升)${mark}`,
        );
      }
    }
  }
}

console.log('\n========== Kedai 后端分层与冻结护栏(C/D/E/G/H) ==========');
if (backendFailures.length) {
  console.log(`[FAIL] ${backendFailures.length} 处分层违规(全部规则均为硬门禁):`);
  for (const f of backendFailures.slice(0, 40)) console.log(`  - ${f}`);
  if (backendFailures.length > 40) console.log(`  ... 另有 ${backendFailures.length - 40} 处`);
  process.exit(1);
} else {
  console.log('[ OK ] 后端分层无违规');
}

console.log('\n========== Kedai 前端架构护栏 ==========');
console.log(`store 循环依赖:${cycles.length} 个(新增 ${newCycles})`);
console.log(`组件直改 state:${writeHits} 处(白名单 ${whitelisted})`);
if (notes.length) {
  console.log(`\n待偿还债务 ${notes.length} 条:`);
  for (const n of notes) console.log(`  - ${n}`);
}
if (failures.length) {
  console.log(`\n[FAIL] ${failures.length} 处架构违规:`);
  for (const f of failures) console.log(`  - ${f}`);
  process.exit(1);
}
console.log('\n[ OK ] 无新增架构违规');
