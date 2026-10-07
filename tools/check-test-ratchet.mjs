#!/usr/bin/env node
/**
 * check-test-ratchet.mjs — 前端「零测试文件」ratchet 护栏(纯 Node 零依赖)。
 *
 * 背景(FE-11;依据 `plans/FRONTEND-REPORT.md` §6.2「零测试的核心模块」):
 *   仓库没有覆盖率度量(覆盖率工具链在本机网络上拉不到新依赖,属环境阻塞),
 *   退而求其次的把关是「零测试文件只降不升」:一个源码文件若没有任何 `*.test.ts`
 *   通过 import 引用它,它就是零测试文件;这类文件只允许来自**显式基线清单**,
 *   清单外出现即 FAIL(新增源码文件不带测试会被拦下)。
 *
 * 判定口径(比报告 §6.2 的 grep 口径更严):
 *   - 「有测试」= 任一 `*.test.ts` 的 import 说明符**解析后指向该文件**
 *     (`from '…'` / `import('…')` / `import '…'`;含 `?raw` 查询串)。
 *     仅「名字在测试里出现过」(注释、it 标题、同名字符串)不算——报告实测有 7 个
 *     文件正是被这种字面命中掩盖为「有测试」的(如仅被注释提到的 ChatTopbar.vue)。
 *   - **不传递门面(barrel)**:测试导入 `api/index.ts` 这类再导出门面时,只算门面
 *     本身被引用,不算它 `export … from` 的目标。理由是静态上分不清「门面被导入」
 *     与「门面下的模块真被调用」:按传递口径实测,`characterScriptSandbox.ts`(15 行
 *     纯再导出)会把 `sandbox/boot-script.ts`(907 行、报告点名的最大盲区)整块洗白。
 *     代价是新增 API 模块若只用门面写测试会被拦下——补一行直接 import 即可通过。
 *   - 源码域 = `web/src` 下 `*.ts` / `*.vue`,排除 `*.test.ts` 与 `*.d.ts`
 *     (类型声明无运行期行为,不纳入)。
 *
 * 用法:
 *   node tools/check-test-ratchet.mjs            # 超出基线即 FAIL
 *   node tools/check-test-ratchet.mjs --list     # 打印当前零测试文件清单(维护基线用)
 *   node tools/check-test-ratchet.mjs --verbose  # 打印基线与现状对照
 *
 * ratchet 纪律:基线只减不增。给基线内文件补了测试就删掉对应行(下次触碰时顺手
 * 或本批收口时);确需新增零测试文件(如纯常量表、barrel 再导出)要在提交信息里
 * 写明理由并把该文件加进基线。
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SRC = join(ROOT, 'web', 'src');
const LIST = process.argv.includes('--list');
const VERBOSE = process.argv.includes('--verbose');

/**
 * 零测试基线(2026-10-06 FE-11 建立,值为**本脚本自身统计**的实测清单)。
 * 只能删行(补了测试就从这里删),不得加行——加行等于关掉护栏。
 * 清单按路径排序,便于肉眼 diff。
 *
 * 结构特征(与 `plans/FRONTEND-REPORT.md` §6.2 的观察一致):弹窗组件与 settings
 * 分区是最大盲区;`.ts` 侧最大空洞是 `sandbox/boot-script.ts`(907 行)与
 * `sandbox/iframe-lifecycle.ts`(497 行)。`main.ts` 是入口文件(无行为可单测),
 * 属登记在案的长期项。
 */
const BASELINE = [
  'web/src/api/agent.ts',
  'web/src/api/audio.ts',
  'web/src/api/characters.ts',
  'web/src/api/contracts.ts',
  'web/src/api/health.ts',
  'web/src/api/labels.ts',
  'web/src/api/skills.ts',
  'web/src/api/slash.ts',
  'web/src/api/workspace.ts',
  'web/src/asyncModal.ts',
  'web/src/audioController.ts',
  'web/src/components/ChatTopbar.vue',
  'web/src/components/DevToolsModal.vue',
  'web/src/components/GreetingPickerModal.vue',
  'web/src/components/MacrosModal.vue',
  'web/src/components/MemoryModal.vue',
  'web/src/components/OptimizeModal.vue',
  'web/src/components/QuickRepliesModal.vue',
  'web/src/components/RenderPanelHost.vue',
  'web/src/components/SkillsModal.vue',
  'web/src/components/TaskExecutorsModal.vue',
  'web/src/components/settings/AboutSection.vue',
  'web/src/components/settings/AgentFlowEdge.vue',
  'web/src/components/settings/AgentFlowNodeCard.vue',
  'web/src/components/settings/ApiSettingsSection.vue',
  'web/src/components/settings/AuthorizationSection.vue',
  'web/src/components/settings/EmbeddingSection.vue',
  'web/src/components/settings/ExecAuthSection.vue',
  'web/src/components/settings/PromptInjectSection.vue',
  'web/src/composables/useEmbeddingSettings.ts',
  'web/src/composables/useExternalLinks.ts',
  'web/src/composables/useKeepAlive.ts',
  'web/src/main.ts',
  'web/src/mvu/mini-jquery.ts',
  'web/src/mvu/unwrap.ts',
  'web/src/platform.ts',
  'web/src/sandbox/boot-script.ts',
  'web/src/sandbox/iframe-lifecycle.ts',
  'web/src/stores/executor.ts',
  'web/src/utils/flowCandidates.ts',
  'web/src/utils/flowCanvasChunk.ts',
];

/** 相对 ROOT 的正斜杠路径(全脚本统一用它做键,避免 Windows 反斜杠差异)。 */
const rel = (p) => relative(ROOT, p).replace(/\\/g, '/');

function collect(dir, exts, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) collect(p, exts, out);
    else if (exts.some((e) => name.endsWith(e))) out.push(p);
  }
  return out;
}

const isSource = (name) => !name.endsWith('.test.ts') && !name.endsWith('.d.ts');
const allFiles = collect(SRC, ['.ts', '.vue']);
const sourceFiles = new Set(allFiles.filter((p) => isSource(p.split(/[\\/]/).pop())).map(rel));
const testFiles = allFiles.filter((p) => p.endsWith('.test.ts'));

/** 测试文件里的**相对** import 说明符(去掉 `?raw` 等查询串)。 */
function relativeSpecifiers(src) {
  const out = [];
  const re = /(?:from\s*|import\s*\(?\s*)['"]([^'"]+)['"]/g;
  let m;
  while ((m = re.exec(src)) !== null) {
    const spec = m[1].split('?')[0];
    if (spec.startsWith('.')) out.push(spec);
  }
  return out;
}

/** 说明符 → 源码文件(相对路径;行内可能省略扩展名或指向 index)。 */
const SUFFIXES = ['', '.ts', '.vue', '/index.ts', '/index.vue'];
function resolveSpecifier(testFile, spec) {
  const base = resolve(dirname(testFile), spec);
  for (const suffix of SUFFIXES) {
    const key = rel(base + suffix);
    if (sourceFiles.has(key)) return key;
  }
  return null;
}

const tested = new Set();
for (const t of testFiles) {
  const src = readFileSync(t, 'utf8');
  for (const spec of relativeSpecifiers(src)) {
    const hit = resolveSpecifier(t, spec);
    if (hit) tested.add(hit);
  }
}

const untested = [...sourceFiles].filter((f) => !tested.has(f)).sort();
const baseline = new Set(BASELINE);
const newlyUntested = untested.filter((f) => !baseline.has(f));
const nowTested = BASELINE.filter((f) => !untested.includes(f));

console.log('========== 前端零测试 ratchet(只降不升)==========');
console.log(`  源码文件 ${sourceFiles.size} 个(web/src 下 .ts/.vue,排除测试与 .d.ts)`);
console.log(`  零测试文件:${untested.length} 个(基线 ${BASELINE.length})`);

if (LIST) {
  for (const f of untested) console.log(`    ${f}`);
  process.exit(0);
}

if (VERBOSE) {
  console.log('  基线内仍零测试:');
  for (const f of BASELINE) if (untested.includes(f)) console.log(`    ${f}`);
}

if (newlyUntested.length > 0) {
  console.log(`\n[FAIL] ${newlyUntested.length} 个源码文件没有任何测试引用(不在基线内):`);
  for (const f of newlyUntested) console.log(`  - ${f}`);
  console.log(
    '\n  说明:新增源码文件请补一个最小测试(哪怕只断言一个纯函数);' +
      '确无行为可测(纯常量表/barrel)请在提交信息里写明理由并加入本脚本 BASELINE。',
  );
  process.exit(1);
}
if (nowTested.length > 0) {
  console.log(`\n[提示] 基线内 ${nowTested.length} 个文件已有测试,可从 BASELINE 删除:`);
  for (const f of nowTested) console.log(`  - ${f}`);
}
console.log('\n[ OK ] 未超出零测试基线');
