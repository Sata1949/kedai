#!/usr/bin/env node
/**
 * Kedai 前端架构护栏(Node 零依赖)。
 *
 * 两项检查:
 *   A. store 循环依赖 —— store 间顶层 import 构成的环。Pinia setup store 允许在
 *      action 内运行期 `useXStore()`,但顶层 import 成环会让初始化顺序变成隐式契约
 *      (谁先被 import 影响 setup 期行为),且无法单独替换任一 store。检出即 FAIL。
 *   B. 组件直改 store state —— 组件里 `store.field = ...` 绕过 action 直接赋值,
 *      使状态变更路径分散、无法在 action 里统一做副作用与校验。仅报「疑似绕过」
 *      (字段写了 action 才会被拦),存量无害赋值经 BASELINE 白名单豁免。
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

/**
 * 组件直改 store state 的白名单:字段名 → 理由。
 * 这些字段是纯 UI 开关(弹窗显隐 / 抽屉开合 / 面板展开 / 加载错误提示),
 * 变更无副作用、无校验、无持久化需求,直改不构成风险;一旦为其补了 action
 * (如 renderHtml 已改走 setRenderHtml/toggleRenderHtml)应从白名单移出。
 * 注意:不在白名单的字段会被判为违规——这正是护栏生效的地方,新增 action
 * 后请同步把字段名从本集合删除。
 */
const UI_FLAG_WHITELIST = new Set([
  // 弹窗/面板显隐
  'settingsOpen',
  'promptsOpen',
  'worldBooksOpen',
  'quickRepliesOpen',
  'pluginsOpen',
  'skillsOpen',
  'contractsOpen',
  'scriptsOpen',
  'macrosOpen',
  'eventsOpen',
  'optimizeOpen',
  'memoryOpen',
  'repoIndexOpen',
  'chatRecordsOpen',
  'devToolsOpen',
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

const failures = [];
const notes = [];

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

console.log('========== Kedai 前端架构护栏 ==========');
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
