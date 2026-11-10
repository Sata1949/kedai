import assert from 'node:assert/strict';
import { mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { test } from 'node:test';
import { startBrowser } from './browser.mjs';
import { cases, policies, seedWorkspace } from './cases.mjs';
import { gradeTask } from './grade.mjs';
import { assertOutsideGit } from './host.mjs';

async function fixture(t, caseId) {
  const parent = await realpath(tmpdir());
  const prefix = 'kedai-quality-test-';
  const root = await mkdtemp(join(parent, prefix));
  t.after(async () => {
    const verified = await realpath(root);
    if (dirname(verified) !== parent || !basename(verified).startsWith(prefix)) {
      throw new Error('拒绝清理非本次测试的目录');
    }
    await rm(verified, { recursive: true });
  });
  const ws = join(root, 'work');
  await seedWorkspace(cases[caseId], ws);
  return {
    root, ws,
    write: (file, text) => writeFile(join(ws, file), text),
    async grade(name, browser) {
      const out = join(root, name);
      await mkdir(out);
      return gradeTask(caseId, ws, out, browser);
    },
  };
}

test('已有仓库：缺陷源码失败；真实导出通过；修改原验收仍被拒绝', async (t) => {
  const f = await fixture(t, 'repo');
  await assert.rejects(assertOutsideGit(join(f.ws, 'src')), /不能写入 Git 工作区/);
  const bad = await f.grade('bad');
  assert.equal(bad.verdict, 'failed');
  assert.equal(bad.checks['merge-order'], false);
  assert.equal(bad.checks['unknown-sku'], false);
  assert.equal(bad.checks.frozen, true);
  await f.write('src/cart.mjs', `import { subtotal } from './money.mjs';
export function quote(items, catalog) {
  const totals = new Map();
  for (const { sku, quantity } of items) {
    if (!Object.hasOwn(catalog, sku) || !Number.isInteger(quantity) || quantity <= 0) throw Error('无效输入');
    totals.set(sku, (totals.get(sku) ?? 0) + quantity);
  }
  const lines = [...totals].map(([sku, quantity]) => ({ sku, quantity, subtotal: subtotal(catalog[sku], quantity) }));
  return { total: lines.reduce((sum, row) => sum + row.subtotal, 0), lines };
}
`);
  const good = await f.grade('good');
  assert.equal(good.verdict, 'passed', JSON.stringify(good));
  await f.write('test/cart.test.mjs', '// 被改写的测试\n');
  const tampered = await f.grade('tampered');
  assert.equal(tampered.checks.frozen, false);
  assert.equal(tampered.verdict, 'failed');
});

test('长任务：按全部政策逐一验证，最后一个地区出错也会失败', async (t) => {
  const f = await fixture(t, 'long');
  const implementation = (rows) => `const rates = new Map(${JSON.stringify(rows.map(([id, ...v]) => [id, v]))});
export function charge(region, units, express = false) {
  if (!rates.has(region) || !Number.isInteger(units) || units < 0 || units > 10000 || typeof express !== 'boolean') {
    throw Error('无效请求');
  }
  const [base, batch, rate, cap, urgent] = rates.get(region);
  return units ? Math.min(cap, base + Math.ceil(units / batch) * rate + (express ? urgent : 0)) : 0;
}
`;
  await f.write('rules.mjs', implementation(policies.slice(0, -1)));
  assert.equal((await f.grade('missing')).checks['all-regions'], false);
  await f.write('rules.mjs', implementation(policies));
  const good = await f.grade('complete');
  assert.equal(good.verdict, 'passed', JSON.stringify(good));
});

test('API 验收向产物端口发真实请求，可识别错误的幂等重放状态', async (t) => {
  const f = await fixture(t, 'api');
  const server = (replayStatus) => `import { createServer } from 'node:http';
const records = [], keys = new Map(), route = '/v1/bookings';
const server = createServer(async (req, res) => {
  const send = (status, value) => res.writeHead(status, {'content-type':'application/json'}).end(JSON.stringify(value));
  const url = new URL(req.url, 'http://localhost');
  if (url.pathname !== route) return send(404, {});
  if (req.method === 'GET') return send(200, records.filter(row => !url.searchParams.has('room') || row.room === url.searchParams.get('room')));
  let body = '', request;
  for await (const chunk of req) body += chunk;
  try { request = JSON.parse(body); } catch { return send(400, {}); }
  const { room, start, end } = request ?? {};
  if (typeof room !== 'string' || !room || !Number.isInteger(start) || !Number.isInteger(end) || start < 0 || start >= end) return send(400, {});
  const key = req.headers['idempotency-key'];
  const old = keys.get(key);
  if (old) return old.room === room && old.start === start && old.end === end ? send(${replayStatus}, old) : send(409, {});
  if (records.some(row => row.room === room && row.start < end && start < row.end)) return send(409, {});
  const created = { id: records.length + 1, room, start, end };
  records.push(created);
  if (key) keys.set(key, created);
  return send(201, created);
});
server.listen(Number(process.env.PORT), '127.0.0.1', () => console.log('READY ' + server.address().port));
`;
  await f.write('server.mjs', server(201));
  const bad = await f.grade('bad');
  assert.equal(bad.checks.listen, true, JSON.stringify(bad));
  assert.equal(bad.checks.idempotent, false);
  assert.equal(bad.verdict, 'failed');
  await f.write('server.mjs', server(200));
  const good = await f.grade('good');
  assert.equal(good.verdict, 'passed', JSON.stringify(good));
});

const executable = process.env.KEDAI_HEADLESS_BROWSER;
const browserOptions = { skip: !executable && '设 KEDAI_HEADLESS_BROWSER 后执行真实浏览器回归' };

test('多文件浏览器采样检测运行帧中重复的纹理', browserOptions, async (t) => {
  const f = await fixture(t, 'graphics');
  const browser = await startBrowser(executable, join(f.root, 'profile'));
  try {
    await f.write('style.css', 'canvas{display:block;width:640px;image-rendering:pixelated}');
    const grass = ['#588741', '#689a50', '#769f57', '#467037'];
    const stone = ['#919196', '#727276', '#616169', '#a2a2ad'];
    const tiles = (rock) => `export const palettes = ${JSON.stringify([grass, rock])};`;
    await f.write('app.mjs', `import { palettes } from './tiles.mjs';
const ctx = document.querySelector('canvas').getContext('2d');
const label = document.querySelector('#face-label');
let front = true;
function draw() {
  const paint = (color, x, y, w, h) => { ctx.fillStyle = color; ctx.fillRect(x, y, w, h); };
  paint('#172235', 0, 0, 160, 64);
  for (let tile = 0; tile < 2; tile++) for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) {
    paint(palettes[tile][(x + y * 3) % 4], tile * 32 + x * 4, y * 4, 4, 4);
  }
  paint('#45b567', 72, 0, 8, 16);
  if (front) paint('#4e9ceb', 96, 0, 16, 16);
  paint('#555555', 120, 0, 8, 16); paint('#888888', 128, 0, 8, 16);
  label.textContent = front ? '正面' : '背面';
}
document.querySelector('#reverse').onclick = () => { front = !front; draw(); };
draw();
`);
    await f.write('tiles.mjs', tiles(grass));
    const bad = await f.grade('bad', browser);
    assert.deepEqual(Object.entries(bad.checks).filter(([, ok]) => !ok).map(([id]) => id), ['texture-diversity'], JSON.stringify(bad));
    await f.write('tiles.mjs', tiles(stone));
    const good = await f.grade('good', browser);
    assert.equal(good.verdict, 'passed', JSON.stringify(good));
  } finally {
    await browser.close();
  }
});

test('UI 验收走真实鼠标、输入、刷新，不把可见静态壳当交付', browserOptions, async (t) => {
  const f = await fixture(t, 'ui');
  const browser = await startBrowser(executable, join(f.root, 'profile'));
  try {
    const shell = `<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
    <label for=new-task>项目</label><input id=new-task><button id=add-task>添加</button>
    <label for=filter-open><input id=filter-open type=checkbox style="position:absolute;width:1px;height:1px;clip-path:inset(50%)">仅看待办</label>
    <ul id=tasks></ul><output id=status></output>`;
    await f.write('index.html', shell);
    const bad = await f.grade('bad', browser);
    assert.equal(bad.checks.entry, true);
    assert.equal(bad.checks.add, false);
    await f.write('index.html', shell + `<script>
const input=document.querySelector('#new-task'), list=document.querySelector('#tasks'), status=document.querySelector('#status');
const items=JSON.parse(localStorage.getItem('tasks') || '[["门禁",false],["照明",false],["备件",true]]');
let only=false;
function render(){
  list.replaceChildren();
  for(const row of items){
    if(only && row[1]) continue;
    const li=document.createElement('li'), toggle=document.createElement('button');
    li.dataset.task=''; li.dataset.done=row[1]; li.textContent=row[0]+' ';
    toggle.dataset.toggle=''; toggle.textContent='切换完成';
    toggle.onclick=()=>{row[1]=!row[1]; save();};
    li.append(toggle); list.append(li);
  }
}
function save(){localStorage.setItem('tasks',JSON.stringify(items));render();}
document.querySelector('#add-task').onclick=()=>{
  const name=input.value.trim();
  if(!name) return;
  if(items.some(item=>item[0]===name)){status.textContent='重复';return;}
  items.push([name,false]); input.value=''; status.textContent='已添加'; save();
};
document.querySelector('#filter-open').onchange=(event)=>{only=event.target.checked;render();};
render();
</script>`);
    const good = await f.grade('good', browser);
    assert.equal(good.verdict, 'passed', JSON.stringify(good));
  } finally {
    await browser.close();
  }
});
