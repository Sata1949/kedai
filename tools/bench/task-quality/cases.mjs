import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { terrainSpec } from './graphics.mjs';
import { hostEnv } from './host.mjs';

export const policies = [
  ['alder', 5, 2, 17, 110, 9], ['briar', 7, 5, 21, 105, 15], ['cedar', 3, 4, 19, 121, 11],
  ['dune', 6, 7, 23, 99, 14], ['elm', 9, 3, 25, 132, 16], ['fen', 5, 6, 16, 94, 10],
  ['glen', 11, 8, 24, 144, 13], ['heath', 2, 9, 15, 108, 17], ['iris', 4, 6, 27, 155, 8],
  ['juniper', 7, 5, 20, 123, 6], ['knoll', 9, 8, 12, 117, 20], ['larch', 3, 7, 22, 133, 7],
  ['mesa', 8, 3, 28, 145, 18], ['north', 5, 4, 14, 103, 21], ['oak', 2, 6, 30, 157, 12],
  ['pine', 6, 9, 26, 149, 19],
];

const html = `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>像素地形巡检台</title><link rel="stylesheet" href="./style.css"><main><h1>像素地形巡检台</h1><button id="reverse">翻转观察面</button><output id="face-label">正面</output><canvas id="scene" width="160" height="64" aria-label="草地、岩石与材质校准图"></canvas></main><script type="module" src="./app.mjs"></script></html>\n`;
const terrainManifest = JSON.stringify({
  entry: 'index.html',
  actions: [
    { action: 'assert_text', selector: '#face-label', contains: '正面' },
    { action: 'click', selector: '#reverse' },
    { action: 'assert_text', selector: '#face-label', contains: '背面' },
  ],
}, null, 2) + '\n';
const uiManifest = JSON.stringify({
  entry: 'index.html',
  actions: [{ action: 'assert_visible', selector: '#new-task' }, { action: 'assert_visible', selector: '#tasks' }],
}, null, 2) + '\n';

const publicBasketTest = `import test from 'node:test';
import assert from 'node:assert/strict';
import { quote } from '../src/cart.mjs';
test('同一商品合并', () => {
  assert.deepEqual(quote([{ sku: 'tea', quantity: 1 }, { sku: 'tea', quantity: 2 }], { tea: 150 }),
    { total: 450, lines: [{ sku: 'tea', quantity: 3, subtotal: 450 }] });
});
`;

function longSeeds() {
  const seeds = {
    'rules.mjs': 'export function charge() { return 0; }\n',
    'SPEC.md': [
      '# 计费规则（本文件优先于历史记录）',
      'charge(region, units, express=false) 返回整数分。region 必须是已登记的小写代号；units 必须是 0..10000 的整数；express 必须是布尔值。非法值抛错。',
      'units=0 恒返回 0；其他情况：基础费 + 向上取整(units/每批单位) × 批费，express=true 时加急费，最后应用封顶。每个地区使用 policy/ 下的现行参数。',
      '旧合同信息只作背景。不要修改本文、政策文档或已有回归；无需外部依赖。',
    ].join('\n') + '\n',
    'check.mjs': `import assert from 'node:assert/strict';
import { charge } from './rules.mjs';
for (let i = 0; i < 850; i++) console.log('准备计费样例，阶段号 %d；读取费率和账单输入', i);
let failure;
try {
  assert.equal(charge('alder', 2), 22);
  assert.equal(charge('pine', 18, true), 77);
  assert.equal(charge('mesa', 0, true), 0);
} catch (error) {
  failure = error;
  console.error('ERROR RATE_CONTRACT: ' + error.message);
}
for (let i = 0; i < 850; i++) console.log('整理计费样例，阶段号 %d；检查账单汇总', i);
if (failure) process.exitCode = 1;
`,
  };
  for (const [i, [name, base, batch, rate, cap, express]] of policies.entries()) {
    seeds[`policy/${name}.md`] = [
      `# ${name} 区域合同`,
      '以下为有效版本；同文件历史报价不能混入实现。',
      `基础费 ${base} 分；每批 ${batch} 单位；批费 ${rate} 分；封顶 ${cap} 分；加急费 ${express} 分。`,
      `例：普通 1 单位收 ${base + rate} 分；加急 1 单位收 ${base + rate + express} 分；0 单位永远免收。`,
      '历史客户答疑（已作废）：',
      ...Array.from({ length: 18 }, (_, j) =>
        `旧报表 ${2022 + j % 3}-${String(j % 12 + 1).padStart(2, '0')} / 单据 ${name}-${i * 18 + j}：按旧阶梯合同记账，仅留审计；新单使用上面的有效版本。`),
    ].join('\n') + '\n';
  }
  return seeds;
}

const common = '只在本任务工作区内修改文件，不用外部网络、依赖或系统数据。用 fs_* 核对文件；执行只用工作区内的相对路径脚本，不用内联解释器或绝对路径。若调用被拒绝，换已授权的方式继续。按实际运行结果报告，勿修改已有验收。';
export const cases = Object.freeze({
  ui: {
    title: '巡检清单交互',
    prompt: [
      '做一个无需构建的离线巡检清单，入口 index.html，初始三项“门禁”“照明”“备件”，前两项待办、备件已完成。',
      '用户可新增非空项目（去掉首尾空白、拒绝重名并在 #status 显示“重复”），可切换完成、仅看待办；刷新后项目和完成状态保留。新增后要有明显反馈。',
      '固定挂点：#new-task 输入框，#add-task 添加按钮，#filter-open 仅看待办开关，#tasks 列表；每项用 [data-task] 包住标题及 [data-toggle] 按钮，完成时 [data-task] 的 data-done="true"，未完成时为 "false"。',
      '输入和按钮要有可读的标签、可见焦点。375px 宽不应横向溢出，桌面和手机都应清楚展示状态。',
    ].join('\n'),
    seed: { '.kedai-verify.json': uiManifest },
    artifacts: ['index.html', 'app.mjs', 'app.js', 'style.css', 'styles.css'],
    checks: ['entry', 'initial', 'add', 'blank', 'duplicate', 'toggle', 'filter', 'persist', 'mobile', 'assets', 'frozen'],
  },
  repo: {
    title: '现有购物篮仓库修复',
    prompt: [
      '修复已有仓库 src/cart.mjs 的 quote(items, catalog)。只改这个文件，保留其公开接口和现有测试。',
      'items 是 {sku, quantity} 的数组，catalog 是 SKU 到非负整数分的自有属性映射。按首次出现顺序合并重复 SKU；返回 {total, lines:[{sku, quantity, subtotal}]}。',
      'quantity 必须是正整数；未知 SKU（包括原型属性）或无效数量抛错；合法零价 SKU 可用，空购物篮返回 total=0 / lines=[]。不能修改调用方传入的数据。',
      '运行已有 node --test，保留真实检查结果。',
    ].join('\n'),
    git: true,
    seed: {
      'README.md': '# 购物篮报价\n\n金额单位是分。运行 `node --test`。\n',
      'src/money.mjs': 'export const subtotal = (price, quantity) => price * quantity;\n',
      'src/cart.mjs': `import { subtotal } from './money.mjs';
export function quote(items, catalog) {
  const lines = items.map(({ sku, quantity }) => ({ sku, quantity, subtotal: subtotal(catalog[sku], quantity) }));
  return { total: lines.reduce((sum, row) => sum + row.subtotal, 0), lines };
}
`,
      'test/cart.test.mjs': publicBasketTest,
    },
    artifacts: ['src/cart.mjs'],
    checks: ['merge-order', 'reject-quantity', 'unknown-sku', 'zero-price', 'empty', 'immutable-input', 'frozen'],
  },
  graphics: {
    title: '多文件像素地形与显式材质规格',
    prompt: [
      '完成离线像素地形巡检台，保留现有 index.html 与 .kedai-verify.json；补 app.mjs、tiles.mjs、style.css。app.mjs 必须实际加载 tiles.mjs。没有框架或特定纹理算法要求。',
      '在现有 160×64 的 #scene 画布中铺底色 #172235。草、石两个 8×8 纹理分别画在 (0,0) 与 (32,0)，每个纹素映射到均匀的 4×4 像素块，不模糊。每块至少 4 色，同位置至少 32/64 个纹素不同。',
      '叶片遮罩区 x=72..87 / y=0..15：左半叶色 #45b567，右半完全裁掉、露出底色。是否用透明混合或 alphaTest 由你决定，只验最终像素。',
      '面剔除校准区 x=96..111 / y=0..15：正面 #4e9ceb，背面露出底色。点击 #reverse 切换、#face-label 显示“正面”或“背面”；两块纹理保持不变。',
      'AO 校准区 x=120..135 / y=0..15：同一灰色材质，右半无遮蔽（亮度至少 100），左半遮蔽（亮度至少 32），二者亮度相差至少 24。其余区域可自由美化，让校准项看起来有意义。',
    ].join('\n'),
    seed: { 'index.html': html, '.kedai-verify.json': terrainManifest },
    artifacts: ['index.html', 'app.mjs', 'tiles.mjs', 'style.css'],
    checks: ['entry', 'local-modules', 'interaction', ...terrainSpec.map(({ id }) => id), 'assets', 'frozen'],
  },
  api: {
    title: '预约 API 独立请求验收',
    prompt: [
      '用 Node.js 内置模块实现 server.mjs，进程监听 127.0.0.1，端口取环境 PORT（允许 0）；监听后向 stdout 输出一行 READY <实际端口>。不用外部依赖。',
      'POST /v1/bookings 接收 JSON {room,start,end}：room 非空字符串，start/end 非负整数且 start < end。创建返回 201 和带 id、room、start、end 的 JSON 对象；坏 JSON 或无效字段 400。',
      '同房间区间重叠返回 409（端点相接不冲突，不同房间互不干扰）。可带 Idempotency-Key：相同键和相同内容重放返回 200 与同一条记录，不新增；同键不同内容 409。',
      'GET /v1/bookings 返回创建顺序的 JSON 数组；可用 ?room= 只看指定房间。未知路径 404。所有响应均为 JSON。不需要数据库。',
    ].join('\n'),
    seed: {},
    artifacts: ['server.mjs'],
    checks: ['listen', 'create', 'validation', 'overlap', 'adjacent', 'other-room', 'idempotent', 'key-conflict', 'ordered-list', 'filter', 'not-found'],
  },
  long: {
    title: '长链路区域计费修复',
    prompt: [
      '修复 rules.mjs 导出的 charge(region, units, express=false)。先理解 SPEC.md 与 policy/ 下 16 份区域合同，区分现行参数和历史报价。',
      '只修改 rules.mjs。用相同的 node check.mjs 在修改前后回归；若日志很长，先找失败线索，不因输出截断把失败认成通过。',
      '需要覆盖全部地区、零用量、加急后的封顶和非法输入。交付时说明已经真实运行的检查。',
    ].join('\n'),
    seed: longSeeds(),
    artifacts: ['rules.mjs'],
    checks: ['all-regions', 'batch-boundaries', 'zero-usage', 'express-cap', 'bad-input', 'frozen'],
  },
});

export const goalFor = (fixture) => `${fixture.title}\n\n${fixture.prompt}\n\n${common}`;
export const caseDigest = (fixture) => createHash('sha256')
  .update(JSON.stringify([goalFor(fixture), fixture.seed, fixture.checks])).digest('hex');

export async function seedWorkspace(fixture, root) {
  await mkdir(root);
  for (const [file, value] of Object.entries(fixture.seed)) {
    const dest = join(root, file);
    await mkdir(dirname(dest), { recursive: true });
    await writeFile(dest, value, { flag: 'wx' });
  }
  if (fixture.git) {
    const git = (...args) => execFileSync('git', [
      '-C', root, '-c', 'init.templateDir=', '-c', 'core.hooksPath=', '-c', 'user.name=Task Quality Fixture',
      '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgSign=false', ...args,
    ], { env: hostEnv, windowsHide: true, stdio: 'pipe', timeout: 5000 });
    git('init', '-q', '--initial-branch=main');
    git('add', '--', ...Object.keys(fixture.seed));
    git('commit', '-qm', 'test: 购物篮缺陷夹具');
  }
}

export async function unchanged(fixture, root, except) {
  for (const [file, original] of Object.entries(fixture.seed)) {
    if (file === except) continue;
    try {
      if (await readFile(join(root, file), 'utf8') !== original) return false;
    } catch (error) {
      if (error.code === 'ENOENT') return false;
      throw error;
    }
  }
  return true;
}
