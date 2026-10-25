#!/usr/bin/env node
// 设置字段对拍(C-2B):后端线格式 ⇄ 前端类型,双向集合比对。
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
// 解析口径与 `check-contract.mjs` 一致:只比字段名集合与「是否可选」不涉及类型。
// 输出:三组计数 + 差异明细;有差异即 exit 1。
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const RUST_SETTINGS = join(ROOT, 'server-rs', 'src', 'api', 'settings', 'mod.rs');
const RUST_PROJECTION = join(ROOT, 'server-rs', 'src', 'api', 'settings', 'projection.rs');
const TS_SETTINGS = join(ROOT, 'web', 'src', 'api', 'types', 'settings.ts');

const read = (path) => readFileSync(path, 'utf8').replace(/\r\n/g, '\n');

/** 取 `pub struct <name> { … }` 的字段名(按 4 空格缩进的 `pub name:` 行)。 */
function rustStructFields(text, name, file) {
  const marker = `pub struct ${name} {`;
  const start = text.indexOf(marker);
  if (start < 0) fail(`${file}: 找不到 ${marker}`);
  const rest = text.slice(start + marker.length);
  const end = rest.indexOf('\n}\n');
  if (end < 0) fail(`${file}: ${name} 结构体未闭合`);
  return [...rest.slice(0, end).matchAll(/^\s{4}pub\s+(\w+):/gm)].map((m) => m[1]);
}

/** 取 `(export) interface <name> { … }` 的字段名与可选性(按 2 空格缩进)。 */
function tsInterfaceFields(text, name, file) {
  const marker = new RegExp(`(?:export )?interface ${name} \\{`);
  const match = marker.exec(text);
  if (!match) fail(`${file}: 找不到 interface ${name}`);
  const rest = text.slice(match.index + match[0].length);
  const end = rest.indexOf('\n}');
  if (end < 0) fail(`${file}: ${name} 接口未闭合`);
  return [...rest.slice(0, end).matchAll(/^\s{2}(\w+)(\?)?:/gm)].map((m) => ({
    name: m[1],
    optional: Boolean(m[2]),
  }));
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

const rustApi = read(RUST_SETTINGS);
const rustProjection = read(RUST_PROJECTION);
const tsSettings = read(TS_SETTINGS);

// ---- 1. PUT:UpdateSettingsBody ⇄ 派生的 RuntimeSettingsPatch ----
const rustBody = rustStructFields(rustApi, 'UpdateSettingsBody', 'settings/mod.rs');
const tsRuntime = tsInterfaceFields(tsSettings, 'RuntimeSettings', 'settings.ts');
const tsRuntimeNames = tsRuntime.map((f) => f.name);
const overrides = tsInterfaceFields(
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

// ---- 2. GET:settings_json() 投影键 ⇄ RuntimeSettings ----
const projected = projectionKeys(rustProjection);
const getCount = diff('GET settings_json() ⇄ RuntimeSettings', projected, tsRuntime);

// ---- 3. 连接元素:ConnectionProfileInput ⇄ ConnectionProfilePatch ----
const rustConnection = rustStructFields(
  rustApi,
  'ConnectionProfileInput',
  'settings/mod.rs',
);
const tsConnection = tsInterfaceFields(
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

console.log(
  `[check-settings-parity] OK PUT ${putCount} 字段 / GET ${getCount} 字段 / 连接 ${connectionCount} 字段`,
);
console.log(
  `[check-settings-parity] 差异常量:GET-only ${getOnly.length} / PUT-only ${patchOnly.length} / 同名重定义 ${overrideFields.length}`,
);
