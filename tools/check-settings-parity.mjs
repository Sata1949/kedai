#!/usr/bin/env node
// 设置字段对拍(C-2B 建;C-2F-5 增类型类别与取值域):后端线格式 ⇄ 前端类型。
//
// 守三组:
//   1. `UpdateSettingsBody`(PUT 请求体)⇄ 前端 `RuntimeSettingsPatch` 的派生集合;
//   2. `settings_json()` 的 GET 投影键 ⇄ 前端 `RuntimeSettings`;
//   3. `ConnectionProfileInput` ⇄ 前端 `ConnectionProfilePatch`(连接元素)。
//
// 为什么不能只靠 `check-contract.mjs`:
//   - `RuntimeSettingsPatch` C-2B 起是 `Partial<Omit<RuntimeSettings, …>> & Overrides`
//     的派生类型,没有可直接解析的字段块;本脚本按同一派生规则从 `RuntimeSettings`
//     与三个显式差异常量重算集合,再与 Rust 请求体对拍(逻辑变更即红)。
//   - GET 响应由 `settings_json()` 手工拼 JSON,不是直接序列化 `RuntimeSettings`;
//     只比 Rust 结构体会漏掉「结构体加了字段但投影忘加」的假阴性。
//
// 三个层次的判据(2026-10-26 C-2F-5 起):
//   ① 字段名集合 + 是否可选(原有);
//   ② 类型类别:bool / number / string / string[] / enum / object / unknown。跨侧按
//      「可安全互换」判:`String` ⇄ 字面量联合(enums)属同一类、`Option<T>` ⇄
//      `T | null` 同值、未知类型(Value/unknown)一律放行。后端命名类型按全仓
//      `pub enum` / `#[serde(transparent)] struct X(pub String)` 扫描归类。
//   ③ 取值域:从 `api/settings/patch/*.rs` 的 `PatchError::new("<field>", "…必须
//      在 A..=B/仅支持 x / y…")` 派生受限字段,与下方 `VALUE_RULES` 显式登记表
//      逐条比对——**新增受限字段不登记即 FAIL**(不默认放过)。枚举字段若前端已
//      收窄为字面量联合则逐值对拍;仍是 string 的记 WARN(前端未收窄,后端侧仍守护)。
//
// 输出:三组计数 + 类型/取值域摘要 + 差异明细;有差异即 exit 1。
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const RUST_SETTINGS = join(ROOT, 'server-rs', 'src', 'api', 'settings', 'mod.rs');
const RUST_PROJECTION = join(ROOT, 'server-rs', 'src', 'api', 'settings', 'projection.rs');
const RUST_RUNTIME = join(ROOT, 'server-rs', 'src', 'services', 'settings_service', 'mod.rs');
const RUST_PATCH_DIR = join(ROOT, 'server-rs', 'src', 'api', 'settings', 'patch');
const RUST_SRC = join(ROOT, 'server-rs', 'src');
const TS_SETTINGS = join(ROOT, 'web', 'src', 'api', 'types', 'settings.ts');

const read = (path) => readFileSync(path, 'utf8').replace(/\r\n/g, '\n');

/** 取 `pub struct <name> { … }` 的字段名与类型(按 4 空格缩进的 `pub name: Type,`)。 */
function rustStructFieldList(text, name, file) {
  const marker = `pub struct ${name} {`;
  const start = text.indexOf(marker);
  if (start < 0) fail(`${file}: 找不到 ${marker}`);
  const rest = text.slice(start + marker.length);
  const end = rest.indexOf('\n}\n');
  if (end < 0) fail(`${file}: ${name} 结构体未闭合`);
  return [...rest.slice(0, end).matchAll(/^\s{4}pub\s+(\w+):\s*([^,\n]+),/gm)].map((m) => ({
    name: m[1],
    type: m[2].trim(),
  }));
}

function rustStructFields(text, name, file) {
  return rustStructFieldList(text, name, file).map((f) => f.name);
}

/** 取 `(export) interface <name> { … }` 的字段名 / 可选性 / 类型(按 2 空格缩进)。 */
function tsInterfaceFieldList(text, name, file) {
  const marker = new RegExp(`(?:export )?interface ${name} \\{`);
  const match = marker.exec(text);
  if (!match) fail(`${file}: 找不到 interface ${name}`);
  const rest = text.slice(match.index + match[0].length);
  const end = rest.indexOf('\n}');
  if (end < 0) fail(`${file}: ${name} 接口未闭合`);
  return [...rest.slice(0, end).matchAll(/^\s{2}(\w+)(\?)?:\s*([^;]+);/gm)].map((m) => ({
    name: m[1],
    optional: Boolean(m[2]),
    type: m[3].replace(/\s+/g, ' ').trim(),
  }));
}

function tsInterfaceFields(text, name, file) {
  return tsInterfaceFieldList(text, name, file);
}

/** 取 `(export) const <NAME> = [ … ] as const;` 的字符串元素。 */
function tsStringArray(text, name) {
  const re = new RegExp(`(?:export )?const ${name} = \\[([\\s\\S]*?)\\] as const;`);
  const match = text.match(re);
  if (!match) fail(`web/src/api/types/settings.ts: 找不到 ${name}`);
  return [...match[1].matchAll(/'([^']+)'/g)].map((m) => m[1]);
}

/** 取 `settings_json()` 的顶层键:8 空格缩进的 json! 键 + `dst.insert("…")`。 */
function projectionKeys(text) {
  const marker = 'fn settings_json(s: &RuntimeSettings) -> Value {';
  const start = text.indexOf(marker);
  if (start < 0) fail('projection.rs: 找不到 settings_json()');
  const rest = text.slice(start + marker.length);
  const end = rest.indexOf('\n}\n');
  if (end < 0) fail('projection.rs: settings_json() 未闭合');
  const body = rest.slice(0, end);
  const literalKeys = [...body.matchAll(/^ {8}"(\w+)":/gm)].map((m) => m[1]);
  const insertedKeys = [...body.matchAll(/dst\.insert\(\s*(?:\n\s*)?"(\w+)"/g)].map(
    (m) => m[1],
  );
  return [...literalKeys, ...insertedKeys];
}

function fail(message) {
  console.error(`[check-settings-parity] FAIL ${message}`);
  process.exit(1);
}

function diff(label, rustFields, tsFields, { checkOptional = false } = {}) {
  const rustSet = new Set(rustFields);
  const tsSet = new Set(tsFields.map((f) => f.name));
  const onlyRust = [...rustSet].filter((name) => !tsSet.has(name)).sort();
  const onlyTs = [...tsSet].filter((name) => !rustSet.has(name)).sort();
  if (onlyRust.length || onlyTs.length) {
    console.error(`[check-settings-parity] FAIL ${label} 字段集合不一致`);
    if (onlyRust.length) console.error(`  仅后端有: ${onlyRust.join(', ')}`);
    if (onlyTs.length) console.error(`  仅前端有: ${onlyTs.join(', ')}`);
    process.exit(1);
  }
  if (checkOptional) {
    // PUT 体两侧都必须是可选字段:必填字段会让「缺省不覆盖」语义失效。
    const required = tsFields.filter((f) => !f.optional).map((f) => f.name);
    if (required.length) {
      fail(`${label} 前端字段必须全部可选,以下为必填: ${required.join(', ')}`);
    }
  }
  return rustFields.length;
}

// ===== 类型类别 =====

/** 扫全仓 Rust 源码,收集「命名枚举」与「serde transparent 字符串新类型」。 */
function collectRustNamedKinds() {
  const enums = new Set();
  const transparentStrings = new Set();
  const walk = (dir) => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const abs = join(dir, e.name);
      if (e.isDirectory()) {
        walk(abs);
        continue;
      }
      if (!e.name.endsWith('.rs')) continue;
      const src = read(abs);
      for (const m of src.matchAll(/^pub enum (\w+)/gm)) enums.add(m[1]);
      for (const m of src.matchAll(
        /#\[serde\(transparent\)\]\s*\n\s*pub struct (\w+)\(pub String\);/g,
      )) {
        transparentStrings.add(m[1]);
      }
    }
  };
  walk(RUST_SRC);
  return { enums, transparentStrings };
}

const RUST_NAMED = collectRustNamedKinds();
const NUMERIC_RUST = /^(?:i8|i16|i32|i64|isize|u8|u16|u32|u64|usize|f32|f64)$/;

function stripRustOption(type) {
  const m = /^Option<([\s\S]+)>$/.exec(type.trim());
  return m ? m[1].trim() : type.trim();
}

function rustCategory(type) {
  const t = stripRustOption(type).replace(/\s+/g, ' ');
  if (t === 'bool') return 'bool';
  if (NUMERIC_RUST.test(t)) return 'number';
  if (t === 'String' || t === '&str' || t === 'str' || t === 'char') return 'string';
  const vec = /^Vec<([\s\S]+)>$/.exec(t);
  if (vec) {
    const inner = vec[1].trim();
    return inner === 'String' || inner === '&str' ? 'string[]' : 'object[]';
  }
  if (t === 'Value' || t === 'serde_json::Value') return 'unknown';
  if (RUST_NAMED.transparentStrings.has(t)) return 'string';
  if (RUST_NAMED.enums.has(t)) return 'enum';
  return 'object';
}

const tsSourceCache = new Map();
function readTs(abs) {
  const key = abs.replace(/\\/g, '/');
  if (!tsSourceCache.has(key)) tsSourceCache.set(key, read(key));
  return tsSourceCache.get(key);
}

/** 解析相对 import 到仓库内 TS 文件(无扩展名时补 .ts / index.ts);失败返回 null。 */
function resolveTsImport(fromFile, spec) {
  if (!spec.startsWith('.')) return null;
  const base = join(dirname(fromFile), spec);
  for (const candidate of [base, `${base}.ts`, join(base, 'index.ts')]) {
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate;
  }
  return null;
}

/** 解析 TS 命名类型别名(含跨文件 import type),返回 {type, text, file} 或 null。 */
function resolveTsNamed(text, file, name, seen = new Set()) {
  const key = `${file}::${name}`;
  if (seen.has(key)) return null;
  seen.add(key);
  const local = new RegExp(`(?:export )?type ${name} = ([^;]+);`).exec(text);
  if (local) return { type: local[1].trim(), text, file };
  for (const m of text.matchAll(/import\s+type\s*\{([^}]*)\}\s*from\s*'([^']+)'/g)) {
    const names = m[1]
      .split(',')
      .map((s) => s.trim().replace(/\s+as\s+.*$/, ''))
      .filter(Boolean);
    if (!names.includes(name)) continue;
    const target = resolveTsImport(file, m[2]);
    if (!target) continue;
    const nextText = readTs(target);
    const resolved = resolveTsNamed(nextText, target, name, seen);
    if (resolved) return resolved;
  }
  return null;
}

function tsCategory(raw, text, file, seen = new Set()) {
  let parts = raw
    .replace(/\s+/g, ' ')
    .trim()
    .split('|')
    .map((s) => s.trim())
    .filter((p) => p !== 'null' && p !== 'undefined');
  const t = parts.join(' | ');
  if (t === 'boolean') return { cat: 'bool' };
  if (t === 'number') return { cat: 'number' };
  if (t === 'string') return { cat: 'string' };
  if (t === 'string[]') return { cat: 'string[]' };
  if (/^'.*'$/.test(t) || (parts.length > 1 && parts.every((p) => /^'[^']*'$/.test(p)))) {
    return { cat: 'enum', values: parts.map((p) => p.replace(/^'|'$/g, '')) };
  }
  if (t.endsWith('[]')) return { cat: 'object[]' };
  if (/^(unknown|any)$/.test(t)) return { cat: 'unknown' };
  if (/^(Record<|\{|Array<|\(\[)/.test(t)) return { cat: 'object' };
  if (/^[A-Z]\w*$/.test(t)) {
    const resolved = resolveTsNamed(text, file, t, seen);
    if (resolved) return tsCategory(resolved.type, resolved.text, resolved.file, seen);
    return { cat: 'object' };
  }
  return { cat: 'unknown' };
}

/** 跨侧类型类别是否可安全互换(字面量联合是 string 的子集,未知类型放行)。 */
function compatibleCategory(rust, ts) {
  if (rust === ts) return true;
  if (rust === 'unknown' || ts === 'unknown') return true;
  if (rust === 'string' && ts === 'enum') return true;
  if (rust === 'enum' && ts === 'string') return true;
  return false;
}

function diffTypes(label, rustFields, tsFields, tsText) {
  const tsMap = new Map(tsFields.map((f) => [f.name, f]));
  const mismatches = [];
  for (const rf of rustFields) {
    const tf = tsMap.get(rf.name);
    if (!tf) continue; // 字段集合差异已由上一步报错
    const rc = rustCategory(rf.type);
    const tc = tsCategory(tf.type, tsText, TS_SETTINGS);
    if (!compatibleCategory(rc, tc.cat)) {
      mismatches.push(`${rf.name}: 后端 ${rf.type}(=${rc}) ⇄ 前端 ${tf.type}(=${tc.cat})`);
    }
  }
  if (mismatches.length) {
    fail(`${label} 类型类别不一致:\n  ${mismatches.join('\n  ')}`);
  }
  return rustFields.length;
}

// ===== 取值域 =====
//
// 登记表只收「后端 patch 显式校验过」的字段:range = 数值区间(allowZero/allowNegative
// 表示区间外另放行的哨兵值),enum = 字面量集合,free = 有校验但无取值域(长度/引用存在性)。
// 新增受限字段若不在此表,deriveValueRules() 会发现并 FAIL——必须显式登记。
const VALUE_RULES = {
  authorization_mode: { kind: 'enum', values: ['strict', 'loose', 'bypass'] },
  compaction_keep_recent: { kind: 'range', min: 2, max: 200 },
  compaction_mode: { kind: 'enum', values: ['off', 'manual', 'auto'] },
  compaction_snip_bytes: { kind: 'range', min: 0, max: 1048576 },
  compaction_threshold: { kind: 'range', min: 0.5, max: 0.95 },
  default_max_tokens: { kind: 'range', min: 1, max: 131072 },
  default_node_max_context: { kind: 'range', min: 256, max: 1048576, allowZero: true },
  default_temperature: { kind: 'range', min: 0, max: 2 },
  default_top_p: { kind: 'range', min: 0, max: 1 },
  embedding_dim: { kind: 'range', min: 16, max: 8192, allowZero: true },
  literary_recommend_preset: { kind: 'enum', values: ['medium', 'long'], emptyOk: true },
  literary_style_preset: {
    kind: 'enum',
    values: ['plain', 'classical', 'lightnovel', 'hardboiled'],
    emptyOk: true,
  },
  loop_guard_semantic_max_distinct: { kind: 'range', min: 1, max: 8 },
  loop_guard_semantic_min_calls: { kind: 'range', min: 4, max: 64, allowZero: true },
  loop_guard_semantic_window: { kind: 'range', min: 4, max: 64 },
  max_context_tokens: { kind: 'range', min: 65536, max: 1048576 },
  max_flow_call_depth: { kind: 'range', min: 1, max: 5 },
  max_flow_calls_per_task: { kind: 'range', min: 1, max: 64 },
  max_tool_rounds: { kind: 'range', min: 1, max: 200 },
  memory_inject_char_budget: { kind: 'range', min: 0, max: 20000 },
  memory_inject_limit: { kind: 'range', min: 0, max: 50 },
  memory_max_entries: { kind: 'range', min: 0, max: 10000 },
  mvu_model: { kind: 'free' },
  mvu_temperature: { kind: 'range', min: 0, max: 2, allowNegative: true },
  mvu_vars_position: { kind: 'enum', values: ['system', 'user_tail'] },
  preset_tail_role: { kind: 'enum', values: ['user', 'assistant'] },
  reflect_advice_role: { kind: 'enum', values: ['user', 'assistant'] },
  session_budget_action: { kind: 'enum', values: ['warn', 'stop'] },
  session_token_budget: { kind: 'range', min: 1024, max: 1000000000, allowZero: true },
  subagent_max_concurrency: { kind: 'range', min: 1, max: 16 },
  subagent_max_depth: { kind: 'range', min: 1, max: 4 },
  subagent_result_max_chars: { kind: 'range', min: 500, max: 8000 },
  task_default_connection_id: { kind: 'free' },
  task_step_budget_secs: { kind: 'range', min: 1, max: 86400, allowZero: true },
  // 2026-10-26 团队预算口径:600..=1200,超过 1200 归一为 0(不限)。
  task_team_budget_secs: { kind: 'range', min: 600, max: 1200, allowZero: true },
  task_tool_policy: { kind: 'enum', values: ['all', 'deny_dangerous', 'allowlist'] },
  task_total_budget_secs: { kind: 'range', min: 1, max: 86400, allowZero: true },
  tool_authorization_timeout_secs: { kind: 'range', min: 30, max: 1800 },
  tool_history_budget_tokens: { kind: 'range', min: 1024, max: 1048576, allowZero: true },
  tool_history_keep_rounds: { kind: 'range', min: 1, max: 32 },
};

/** 从 patch/*.rs 的 PatchError 文案派生规则(与 VALUE_RULES 同构)。 */
function deriveValueRules() {
  const byField = new Map();
  for (const file of readdirSync(RUST_PATCH_DIR)) {
    if (!file.endsWith('.rs') || file === 'mod.rs' || file === 'tests.rs') continue;
    const src = read(join(RUST_PATCH_DIR, file));
    for (const m of src.matchAll(/PatchError::new\(\s*"([a-z_0-9]+)",\s*"([^"]*)"/g)) {
      const field = m[1];
      const message = m[2].replace(new RegExp(`^${field}\\s*`), '').trim();
      if (!byField.has(field)) byField.set(field, []);
      byField.get(field).push(message);
    }
  }
  const out = new Map();
  for (const [field, messages] of byField) out.set(field, classifyValueRule(messages));
  return out;
}

function classifyValueRule(messages) {
  for (const message of messages) {
    const only = /^仅支持\s+(.+?)(?:\s*\(|$)/.exec(message);
    if (only) {
      return {
        kind: 'enum',
        values: only[1].split(/\s*\/\s*/).map((s) => s.trim()).filter(Boolean),
        emptyOk: /空\s*=/.test(message),
      };
    }
    const either = /^须为\s+(\w+)\s+或\s+(\w+)/.exec(message);
    if (either) return { kind: 'enum', values: [either[1], either[2]], emptyOk: false };
  }
  for (const message of messages) {
    const range = /(\d+(?:\.\d+)?)\s*\.\.=\s*(\d+(?:\.\d+)?)/.exec(message);
    if (range) {
      return {
        kind: 'range',
        min: Number(range[1]),
        max: Number(range[2]),
        allowZero: /(须为|必须为)\s*0\s*\(/.test(message),
        allowNegative: /负值/.test(message),
      };
    }
  }
  return { kind: 'free' };
}

function normalizeRule(rule) {
  if (rule.kind === 'enum') {
    return { kind: 'enum', values: [...rule.values].sort(), emptyOk: Boolean(rule.emptyOk) };
  }
  if (rule.kind === 'range') {
    return {
      kind: 'range',
      min: rule.min,
      max: rule.max,
      allowZero: Boolean(rule.allowZero),
      allowNegative: Boolean(rule.allowNegative),
    };
  }
  return { kind: 'free' };
}

function ruleKey(rule) {
  const r = normalizeRule(rule);
  if (r.kind === 'enum') return `enum:${r.values.join('|')}:emptyOk=${r.emptyOk}`;
  if (r.kind === 'range') {
    return `range:${r.min}..=${r.max}:zero=${r.allowZero}:neg=${r.allowNegative}`;
  }
  return 'free';
}

/** 派生 ⇄ 登记表逐条对拍;新增受限字段未登记、登记与代码不一致、登记失效均 FAIL。 */
function checkValueRules() {
  const derived = deriveValueRules();
  const issues = [];
  for (const [field, rule] of derived) {
    const registered = VALUE_RULES[field];
    if (!registered) {
      issues.push(`${field}: 新增受限字段未登记(派生 ${JSON.stringify(normalizeRule(rule))})`);
      continue;
    }
    if (ruleKey(rule) !== ruleKey(registered)) {
      issues.push(
        `${field}: 登记 ${JSON.stringify(normalizeRule(registered))} ≠ 代码派生 ` +
          JSON.stringify(normalizeRule(rule)),
      );
    }
  }
  for (const field of Object.keys(VALUE_RULES)) {
    if (!derived.has(field)) issues.push(`${field}: 登记已失效(patch/*.rs 无对应校验)`);
  }
  if (issues.length) fail(`取值域登记与 patch/*.rs 不一致:\n  ${issues.join('\n  ')}`);
  return {
    total: derived.size,
    counts: {
      enum: [...derived.values()].filter((r) => r.kind === 'enum').length,
      range: [...derived.values()].filter((r) => r.kind === 'range').length,
      free: [...derived.values()].filter((r) => r.kind === 'free').length,
    },
  };
}

/** 取值域的前端侧:枚举联合逐值对拍、数值区间要求前端是 number。 */
function checkTsValueDomains(tsFields) {
  const tsMap = new Map(tsFields.map((f) => [f.name, f]));
  const unNarrowed = [];
  for (const [field, rule] of Object.entries(VALUE_RULES)) {
    const tf = tsMap.get(field);
    if (!tf) continue;
    const tc = tsCategory(tf.type, tsSettings, TS_SETTINGS);
    if (rule.kind === 'enum') {
      if (tc.cat === 'enum') {
        const missing = rule.values.filter((v) => !tc.values.includes(v));
        const extra = tc.values.filter((v) => !rule.values.includes(v));
        if (missing.length || extra.length) {
          fail(
            `${field} 枚举取值不一致:后端 [${rule.values.join(', ')}] ⇄ 前端 ` +
              `[${tc.values.join(', ')}]` +
              (missing.length ? `;前端缺 [${missing.join(', ')}]` : '') +
              (extra.length ? `;前端多 [${extra.join(', ')}]` : ''),
          );
        }
      } else if (tc.cat === 'string') {
        unNarrowed.push(field);
      } else {
        fail(`${field} 是枚举字段,前端类型 ${tf.type}(=${tc.cat}) 既非字面量联合也非 string`);
      }
    } else if (rule.kind === 'range' && tc.cat !== 'number') {
      fail(`${field} 是数值区间字段,前端类型 ${tf.type}(=${tc.cat}) 不是 number`);
    }
  }
  return unNarrowed;
}

// ===== 执行 =====

const rustApi = read(RUST_SETTINGS);
const rustProjection = read(RUST_PROJECTION);
const rustRuntime = read(RUST_RUNTIME);
const tsSettings = read(TS_SETTINGS);

// ---- 1. PUT:UpdateSettingsBody ⇄ 派生的 RuntimeSettingsPatch ----
const rustBodyList = rustStructFieldList(rustApi, 'UpdateSettingsBody', 'settings/mod.rs');
const rustBody = rustBodyList.map((f) => f.name);
const tsRuntime = tsInterfaceFieldList(tsSettings, 'RuntimeSettings', 'settings.ts');
const tsRuntimeNames = tsRuntime.map((f) => f.name);
const overrides = tsInterfaceFieldList(
  tsSettings,
  'RuntimeSettingsPatchOverrides',
  'settings.ts',
);
const getOnly = tsStringArray(tsSettings, 'SETTINGS_GET_ONLY_FIELDS');
const patchOnly = tsStringArray(tsSettings, 'SETTINGS_PATCH_ONLY_FIELDS');
const overrideFields = tsStringArray(tsSettings, 'SETTINGS_PATCH_OVERRIDE_FIELDS');

for (const name of [...getOnly, ...overrideFields]) {
  if (!tsRuntimeNames.includes(name)) {
    fail(`差异常量引用了 RuntimeSettings 不存在的字段: ${name}`);
  }
}
for (const name of patchOnly) {
  if (tsRuntimeNames.includes(name)) {
    fail(`SETTINGS_PATCH_ONLY_FIELDS 不应出现在 RuntimeSettings 中: ${name}`);
  }
}
const expectedOverrideNames = new Set([...patchOnly, ...overrideFields]);
const declaredOverrideNames = overrides.map((f) => f.name);
if (
  expectedOverrideNames.size !== declaredOverrideNames.length ||
  declaredOverrideNames.some((name) => !expectedOverrideNames.has(name))
) {
  fail(
    `RuntimeSettingsPatchOverrides 与差异常量不一致:` +
      ` 类型=[${declaredOverrideNames.join(', ')}]` +
      ` 期望=[${[...expectedOverrideNames].join(', ')}]`,
  );
}
const excluded = new Set([...getOnly, ...overrideFields]);
const derivedPatch = [
  ...tsRuntimeNames.filter((name) => !excluded.has(name)),
  ...patchOnly,
  ...overrideFields,
];
const putCount = diff(
  'PUT UpdateSettingsBody ⇄ RuntimeSettingsPatch',
  rustBody,
  derivedPatch.map((name) => ({ name, optional: true })),
  { checkOptional: true },
);
const tsRuntimeMap = new Map(tsRuntime.map((f) => [f.name, f]));
const tsOverrideMap = new Map(overrides.map((f) => [f.name, f]));
const tsPatchTypes = derivedPatch.map((name) => ({
  name,
  optional: true,
  type: (tsOverrideMap.get(name) ?? tsRuntimeMap.get(name) ?? { type: 'unknown' }).type,
}));
const putTypeCount = diffTypes(
  'PUT UpdateSettingsBody',
  rustBodyList,
  tsPatchTypes,
  tsSettings,
);

// ---- 2. GET:settings_json() 投影键 ⇄ RuntimeSettings ----
const projected = projectionKeys(rustProjection);
const getCount = diff('GET settings_json() ⇄ RuntimeSettings', projected, tsRuntime);
const getTypeCount = diffTypes(
  'GET RuntimeSettings',
  rustStructFieldList(rustRuntime, 'RuntimeSettings', 'settings_service/mod.rs'),
  tsRuntime,
  tsSettings,
);

// ---- 3. 连接元素:ConnectionProfileInput ⇄ ConnectionProfilePatch ----
const rustConnectionList = rustStructFieldList(
  rustApi,
  'ConnectionProfileInput',
  'settings/mod.rs',
);
const rustConnection = rustConnectionList.map((f) => f.name);
const tsConnection = tsInterfaceFieldList(
  tsSettings,
  'ConnectionProfilePatch',
  'settings.ts',
);
const connectionCount = diff(
  'ConnectionProfileInput ⇄ ConnectionProfilePatch',
  rustConnection,
  tsConnection,
  { checkOptional: true },
);
const connectionTypeCount = diffTypes(
  'ConnectionProfileInput',
  rustConnectionList,
  tsConnection,
  tsSettings,
);

// ---- 4. 取值域 ----
const valueRules = checkValueRules();
const unNarrowed = checkTsValueDomains(tsRuntime);

console.log(
  `[check-settings-parity] OK PUT ${putCount} 字段 / GET ${getCount} 字段 / 连接 ${connectionCount} 字段`,
);
console.log(
  `[check-settings-parity] OK 类型类别: PUT ${putTypeCount} / GET ${getTypeCount} / 连接 ${connectionTypeCount} 字段全一致`,
);
console.log(
  `[check-settings-parity] OK 取值域: 登记 ${valueRules.total} 字段` +
    `(enum ${valueRules.counts.enum} / range ${valueRules.counts.range} / free ${valueRules.counts.free})` +
    `,与 patch/*.rs 派生一致`,
);
console.log(
  `[check-settings-parity] 差异常量:GET-only ${getOnly.length} / PUT-only ${patchOnly.length} / 同名重定义 ${overrideFields.length}`,
);
if (unNarrowed.length) {
  console.log(
    `[check-settings-parity] WARN 前端仍是 string 的枚举字段(值域仅后端侧守护): ${unNarrowed.join(', ')}`,
  );
}
