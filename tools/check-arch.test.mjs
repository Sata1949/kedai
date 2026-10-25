// check-arch 检测逻辑的 fixture 自测(2026-10-24 C-2-0)。
//
// 为什么需要:2026-10-24 审计实测发现 check-arch 有两类假阴性——只认
// `store.x =` 漏掉 `v-model` / `.value =`,只认静态 `from` 漏掉动态
// `import()`;而门禁脚本自身没有测试,改坏了也只能靠跑真实仓库发现。
// 这里用最小源码 fixture 直接覆盖纯函数,不读仓库、不依赖 cwd。
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  collectFrontendModuleSpecs,
  directWrites,
  hasArchAllow,
  hasFileArchAllow,
  resolveFrontendSpecifier,
  storeBindings,
} from './lib/arch-scan.mjs';

const VUE_FIXTURE = `
const store = useTaskStore();
const { taskFlowId: flowId, draft } = storeToRefs(store);
flowId.value = 'flow-1';
draft.value = 1;
<select v-model.number="flowId" />
<select v-model="store.currentTaskId" />
`;

test('storeToRefs 解构(含别名)与 .value / v-model 直写都能被抓到', () => {
  const bindings = storeBindings(VUE_FIXTURE);
  assert.equal(bindings.get('store'), 'store', 'useXStore 绑定');
  assert.equal(bindings.get('flowId'), 'ref', 'storeToRefs 别名解构');
  assert.equal(bindings.get('draft'), 'ref', 'storeToRefs 同名解构');

  const syntaxes = directWrites(VUE_FIXTURE, bindings).map((h) => h.syntax);
  assert.ok(syntaxes.includes('flowId.value ='), `.value = 漏检:${syntaxes.join(', ')}`);
  assert.ok(syntaxes.includes('draft.value ='), `.value = 漏检:${syntaxes.join(', ')}`);
  assert.ok(
    syntaxes.includes('v-model.number="flowId"'),
    `ref v-model 漏检:${syntaxes.join(', ')}`,
  );
  assert.ok(
    syntaxes.includes('v-model="store.currentTaskId"'),
    `store 字段 v-model 漏检:${syntaxes.join(', ')}`,
  );
});

test('静态 from 与动态 import() 走同一采集入口,动态相对路径能解析出跨代模块', () => {
  const src = `
import Foo from './Foo.vue';
const Bar = () => import('../components/Bar.vue');
const Baz = () => import('@/components/Baz.vue');
`;
  assert.deepEqual(collectFrontendModuleSpecs(src), [
    './Foo.vue',
    '../components/Bar.vue',
    '@/components/Baz.vue',
  ]);

  assert.equal(
    resolveFrontendSpecifier('utils/flowCanvasChunk.ts', '../components/Bar.vue'),
    'components',
    'utils 动态 import 上层 components 必须解析为跨代边',
  );
  assert.equal(resolveFrontendSpecifier('components/settings/a.ts', './Bar.vue'), 'components');
  assert.equal(resolveFrontendSpecifier('components/settings/a.ts', '../AgentFlowCanvas.vue'), 'components');
  assert.equal(resolveFrontendSpecifier('utils/a.ts', './local.ts'), 'utils');
  assert.equal(resolveFrontendSpecifier('utils/a.ts', '../../components/X.vue'), null, '上跳路径不视作同层');
  assert.equal(resolveFrontendSpecifier('utils/a.ts', 'vue'), null, '包名不参与跨代采集');
});

test('arch-allow 只认当前行/上一行且必须写非空理由', () => {
  const sameLine = `a.value = 1; // arch-allow: 运行时同步`;
  assert.equal(hasArchAllow(sameLine, sameLine.indexOf('a.value')), true);

  const prevLine = `// arch-allow: 事件回调\na.value = 1;`;
  assert.equal(hasArchAllow(prevLine, prevLine.indexOf('a.value')), true);

  const twoLinesAway = `a.value = 1; // arch-allow: 理由\n\nb.value = 2;`;
  assert.equal(hasArchAllow(twoLinesAway, twoLinesAway.indexOf('b.value')), false);

  const emptyReason = `a.value = 1; // arch-allow:`;
  assert.equal(hasArchAllow(emptyReason, emptyReason.indexOf('a.value')), false);
});

test('arch-allow-file 只认文件头 20 行且必须写非空理由', () => {
  const header20 = `${Array.from({ length: 19 }, (_, i) => `// ${i}`).join('\n')}
// arch-allow-file: 整文件表单
const x = 1;`;
  assert.equal(hasFileArchAllow(header20), true, '第 20 行的文件级豁免应生效');

  const header21 = `${Array.from({ length: 20 }, (_, i) => `// ${i}`).join('\n')}
// arch-allow-file: 整文件表单
const x = 1;`;
  assert.equal(hasFileArchAllow(header21), false, '第 21 行的文件级豁免不应生效');

  assert.equal(hasFileArchAllow('// arch-allow-file:\nconst x = 1;'), false, '空理由不算');
});
