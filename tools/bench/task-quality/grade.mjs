import { execFile } from 'node:child_process';
import { lstat, readdir, realpath } from 'node:fs/promises';
import { isAbsolute, join, relative, sep } from 'node:path';
import { isDeepStrictEqual, promisify } from 'node:util';
import { BrowserUnavailable } from './browser.mjs';
import { cases, policies, unchanged } from './cases.mjs';
import { gradeGraphics, terrainSpec } from './graphics.mjs';
import { hostEnv, launch, until } from './host.mjs';

const exec = promisify(execFile);
const q = JSON.stringify;

export async function localFile(root, name) {
  const target = join(root, name);
  try {
    if (!(await lstat(target)).isFile()) return null;
    const resolved = await realpath(target);
    const rel = relative(await realpath(root), resolved);
    return rel !== '..' && !rel.startsWith('..' + sep) && !isAbsolute(rel) ? resolved : null;
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

async function sourceFiles(root, subdir = '') {
  const names = [];
  for (const item of await readdir(join(root, subdir), { withFileTypes: true })) {
    if (!subdir && item.name === '.git') continue;
    const name = subdir ? `${subdir}/${item.name}` : item.name;
    if (item.isDirectory()) names.push(...await sourceFiles(root, name));
    else names.push(name);
    if (names.length > 64) throw new Error('产物文件数量超过合成评测上限（64）');
  }
  return names;
}

export async function artifactsFor(caseId, root) {
  if (caseId === 'repo' || caseId === 'long') return cases[caseId].artifacts;
  return (await sourceFiles(root)).filter((name) => /\.(html|[cm]?js|css|json|md|svg)$/i.test(name) && !name.startsWith('.'));
}

const textOf = (page, selector) => page.evaluate(`document.querySelector(${q(selector)})?.textContent?.trim() ?? ''`);
const rows = (page, onlyVisible = false) => page.evaluate(`Array.from(document.querySelectorAll('#tasks [data-task]'))
  .filter(e => !${onlyVisible} || (e.getClientRects().length && getComputedStyle(e).visibility !== 'hidden'))
  .map(e => ({ text: e.textContent.trim(), done: e.dataset.done }))`);
const has = (items, name, done) => items.some((item) => item.text.includes(name) && item.done === String(done));

async function gradeUi(root, out, browser, checks) {
  checks.entry = !!await localFile(root, 'index.html');
  checks.frozen = await unchanged(cases.ui, root);
  if (!checks.entry) return;
  await browser.withPage(root, async (page) => {
    const initial = await rows(page);
    checks.initial = initial.length === 3 && has(initial, '门禁', false)
      && has(initial, '照明', false) && has(initial, '备件', true);
    const marker = '北窗';
    async function add(value) {
      if (!await page.click('#new-task')) return false;
      await page.evaluate("document.querySelector('#new-task').select()");
      await page.type(value);
      return page.click('#add-task');
    }
    if (await add('   ')) checks.blank = (await rows(page)).length === initial.length;
    if (await add(`  ${marker}  `)) checks.add = has(await rows(page), marker, false);
    const before = (await rows(page)).length;
    if (await add(marker)) checks.duplicate = (await rows(page)).length === before
      && (await textOf(page, '#status')).includes('重复');
    const toggleIndex = await page.evaluate(`Array.from(document.querySelectorAll('#tasks [data-toggle]'))
      .findIndex(e => e.closest('[data-task]')?.textContent.includes(${q(marker)}))`);
    if (toggleIndex >= 0 && await page.click('#tasks [data-toggle]', toggleIndex)) {
      checks.toggle = has(await rows(page), marker, true);
    }
    // 原生开关可把 input 视觉隐藏而由关联的 label 接收点击。仍走真实鼠标，
    // 不用脚本修改 checked，否则会把不可操作的控件误判为通过。
    const clickFilter = async () => await page.click('#filter-open')
      || page.click('label[for="filter-open"], label:not([for]):has(#filter-open)');
    if (await clickFilter()) {
      const open = await rows(page, true);
      checks.filter = open.length === 2 && has(open, '门禁', false) && has(open, '照明', false);
      await clickFilter();
    }
    await page.reload();
    checks.persist = has(await rows(page), marker, true);
    await page.screenshot(join(out, 'desktop.png'));
    await page.viewport(375, 720);
    checks.mobile = await page.evaluate(`(() => {
      const input = document.querySelector('#new-task');
      const add = document.querySelector('#add-task');
      if (!input || !add) return false;
      input.focus();
      const refs = (input.getAttribute('aria-labelledby') || '').split(/\\s+/);
      const name = input.getAttribute('aria-label') || refs.map(id => document.getElementById(id)?.textContent || '').join('')
        || [...input.labels].map(l => l.textContent).join('');
      return name.trim().length > 0 && add.textContent.trim().length > 0 && document.activeElement === input
        && input.getBoundingClientRect().width > 0 && add.getBoundingClientRect().width > 0
        && document.documentElement.scrollWidth <= innerWidth;
    })()`);
    await page.screenshot(join(out, 'mobile.png'));
    checks.assets = page.faults.length === 0;
  });
}

async function gradeTerrain(root, out, browser, checks) {
  checks.frozen = await unchanged(cases.graphics, root);
  if (!await localFile(root, 'index.html')) return;
  await browser.withPage(root, async (page) => {
    const capture = () => page.evaluate(`(() => {
      const canvas = document.querySelector('#scene');
      if (canvas?.width !== 160 || canvas?.height !== 64) return null;
      // 拷到验收者自建的 2D 画布，原产物用 Canvas2D 或 WebGL 都可被采样。
      const snapshot = document.createElement('canvas');
      snapshot.width = canvas.width; snapshot.height = canvas.height;
      const ctx = snapshot.getContext('2d');
      ctx.drawImage(canvas, 0, 0);
      const rgba = ctx.getImageData(0, 0, 160, 32).data;
      return Array.from({length: 160 * 32}, (_, i) =>
        rgba[i * 4 + 3] === 255 ? (rgba[i * 4] << 16) | (rgba[i * 4 + 1] << 8) | rgba[i * 4 + 2] : -1);
    })()`);
    await page.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    const front = await capture();
    checks.entry = front !== null && front.some((pixel) => pixel !== -1);
    checks['local-modules'] = await page.evaluate(`['app.mjs', 'tiles.mjs'].every(name =>
      performance.getEntriesByType('resource').some(r => new URL(r.name).pathname === '/' + name))`);
    const frontLabel = await textOf(page, '#face-label');
    await page.screenshot(join(out, 'front.png'));
    const clicked = await page.click('#reverse');
    await page.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    const back = await capture();
    checks.interaction = clicked && frontLabel === '正面' && await textOf(page, '#face-label') === '背面';
    if (front && back) {
      Object.assign(checks, gradeGraphics({ front: (x, y) => front[y * 160 + x], back: (x, y) => back[y * 160 + x] }, terrainSpec));
    }
    await page.screenshot(join(out, 'back.png'));
    checks.assets = page.faults.length === 0;
  });
}

// 独立验证进程只接收输入向量。判定、期望值与退出状态由宿主读取，不接受模型自测结果。
const oracle = `
import { pathToFileURL } from 'node:url';
const { file, method, vectors } = JSON.parse(process.argv[1]);
const fn = (await import(pathToFileURL(file)))[method];
if (typeof fn !== 'function') throw new Error('缺失导出：' + method);
const observations = [];
for (const args of vectors) {
  const before = JSON.stringify(args);
  try {
    observations.push({ value: await fn(...args), mutated: before !== JSON.stringify(args) });
  } catch {
    observations.push({ thrown: true, mutated: before !== JSON.stringify(args) });
  }
}
process.stdout.write('\\n' + JSON.stringify(observations) + '\\n');
`;
const permissions = (root) => ['--permission', `--allow-fs-read=${root}`];

async function observeCode(root, name, method, vectors) {
  const file = await localFile(root, name);
  if (!file) throw new Error(`缺少产物：${name}`);
  const { stdout } = await exec(process.execPath, [
    ...permissions(root), '--input-type=module', '-e', oracle, q({ file, method, vectors }),
  ], { env: hostEnv, cwd: root, windowsHide: true, timeout: 6000, maxBuffer: 128_000 });
  const observations = JSON.parse(stdout.trimEnd().split(/\r?\n/).at(-1));
  if (!Array.isArray(observations) || observations.length !== vectors.length) throw new Error('产物调用观察记录不完整');
  return observations;
}

async function onlyChanged(caseId, root, file) {
  const expected = Object.keys(cases[caseId].seed).sort();
  return await unchanged(cases[caseId], root, file)
    && isDeepStrictEqual((await sourceFiles(root)).sort(), expected);
}

async function gradeRepo(root, checks) {
  checks.frozen = await onlyChanged('repo', root, 'src/cart.mjs');
  const good = { lamp: 255, tea: 150, gift: 0 };
  const vectors = [
    [[{ sku: 'tea', quantity: 1 }, { sku: 'lamp', quantity: 2 }, { sku: 'tea', quantity: 3 }], good],
    ...[0, -1, 1.5, '2', null].map((quantity) => [[{ sku: 'tea', quantity }], good]),
    ...['unknown', 'toString', '__proto__'].map((sku) => [[{ sku, quantity: 1 }], good]),
    [[{ sku: 'gift', quantity: 2 }], good], [[], good],
  ];
  const result = await observeCode(root, 'src/cart.mjs', 'quote', vectors);
  checks['merge-order'] = isDeepStrictEqual(result[0].value, { total: 1110, lines: [
    { sku: 'tea', quantity: 4, subtotal: 600 }, { sku: 'lamp', quantity: 2, subtotal: 510 },
  ] });
  checks['reject-quantity'] = result.slice(1, 6).every(({ thrown }) => thrown === true);
  checks['unknown-sku'] = result.slice(6, 9).every(({ thrown }) => thrown === true);
  checks['zero-price'] = isDeepStrictEqual(result[9].value, { total: 0, lines: [{ sku: 'gift', quantity: 2, subtotal: 0 }] });
  checks.empty = isDeepStrictEqual(result[10].value, { total: 0, lines: [] });
  checks['immutable-input'] = result.every(({ mutated }) => mutated === false);
}

async function gradeLong(root, checks) {
  checks.frozen = await onlyChanged('long', root, 'rules.mjs');
  const valid = policies.flatMap(([region, base, batch, rate, cap, fee]) => [
    { group: 'all-regions', args: [region, 1], want: base + rate },
    { group: 'batch-boundaries', args: [region, batch], want: base + rate },
    { group: 'batch-boundaries', args: [region, batch + 1], want: Math.min(base + 2 * rate, cap) },
    { group: 'zero-usage', args: [region, 0, true], want: 0 },
    { group: 'express-cap', args: [region, 10_000, true], want: cap },
    { group: 'express-cap', args: [region, 1, true], want: Math.min(base + rate + fee, cap) },
  ]);
  const invalid = [['unknown', 1], ['ALDER', 1], ['alder', -1], ['alder', 10001],
    ['alder', 1.5], ['alder', '2'], ['alder', 1, 'yes'], ['alder', 1, null]];
  const result = await observeCode(root, 'rules.mjs', 'charge', [...valid.map(({ args }) => args), ...invalid]);
  for (const id of ['all-regions', 'batch-boundaries', 'zero-usage', 'express-cap']) {
    checks[id] = valid.every(({ group, want }, i) => group !== id || result[i].value === want);
  }
  checks['bad-input'] = result.slice(valid.length).every(({ thrown }) => thrown === true);
}

async function gradeApi(root, checks) {
  const file = await localFile(root, 'server.mjs');
  if (!file) return;
  const run = launch(process.execPath, [...permissions(root), file], {
    cwd: root, env: { ...hostEnv, PORT: '0' }, stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  run.child.stdout.on('data', (chunk) => { if (stdout.length < 8192) stdout += chunk.toString(); });
  run.child.stderr.on('data', (chunk) => { if (stderr.length < 8192) stderr += chunk.toString(); });
  try {
    await run.started;
    const port = await until(() => {
      if (run.child.exitCode !== null) throw new Error(`产物进程退出：${stderr.slice(0, 600)}`);
      const value = Number(stdout.match(/^READY (\d+)\s*$/m)?.[1]);
      return Number.isInteger(value) && value > 0 && value <= 65535 ? value : null;
    }, 4000, '产物 READY 端口');
    checks.listen = true;
    const request = async (route, body, key) => {
      const response = await fetch(`http://127.0.0.1:${port}${route}`, {
        method: body === undefined ? 'GET' : 'POST',
        body, headers: {
          'Content-Type': 'application/json', ...(key ? { 'Idempotency-Key': key } : {}),
        },
        signal: AbortSignal.timeout(2000),
      });
      const json = response.headers.get('content-type')?.toLowerCase().includes('application/json')
        ? await response.json() : null;
      return { status: response.status, json };
    };
    const route = '/v1/bookings';
    const payload = (room, start, end) => q({ room, start, end });
    const first = await request(route, payload('north', 10, 20), 'first');
    checks.create = first.status === 201 && ['string', 'number'].includes(typeof first.json?.id)
      && first.json.room === 'north' && first.json.start === 10 && first.json.end === 20;
    const invalid = await Promise.all(['{', 'null', payload(123, 1, 2), payload('', 1, 2), payload('north', -1, 2),
      payload('north', 1.5, 2), payload('north', 2, 2)].map((body) => request(route, body)));
    checks.validation = invalid.every(({ status, json }) => status === 400 && json !== null);
    const collision = await request(route, payload('north', 19, 22));
    checks.overlap = collision.status === 409 && collision.json !== null;
    const adjacent = await request(route, payload('north', 20, 25));
    checks.adjacent = adjacent.status === 201 && adjacent.json?.start === 20;
    const other = await request(route, payload('south', 15, 21));
    checks['other-room'] = other.status === 201 && other.json?.room === 'south';
    const replay = await request(route, payload('north', 10, 20), 'first');
    checks.idempotent = replay.status === 200 && first.json !== null && isDeepStrictEqual(replay.json, first.json);
    const reused = await request(route, payload('north', 28, 30), 'first');
    checks['key-conflict'] = reused.status === 409 && reused.json !== null;
    const all = await request(route);
    checks['ordered-list'] = all.status === 200 && new Set([first.json?.id, adjacent.json?.id, other.json?.id]).size === 3
      && isDeepStrictEqual(all.json, [first.json, adjacent.json, other.json]);
    const filtered = await request(route + '?room=north');
    checks.filter = filtered.status === 200 && isDeepStrictEqual(filtered.json, [first.json, adjacent.json]);
    const missing = await request('/not-found');
    checks['not-found'] = missing.status === 404 && missing.json !== null;
  } finally {
    await run.stop();
  }
}

export async function gradeTask(caseId, root, out, browser) {
  const checks = Object.fromEntries(cases[caseId].checks.map((id) => [id, false]));
  let diagnostic = null;
  try {
    switch (caseId) {
      case 'ui': await gradeUi(root, out, browser, checks); break;
      case 'graphics': await gradeTerrain(root, out, browser, checks); break;
      case 'repo': await gradeRepo(root, checks); break;
      case 'long': await gradeLong(root, checks); break;
      case 'api': await gradeApi(root, checks); break;
    }
  } catch (error) {
    diagnostic = (error.stderr || error.message).slice(0, 2000);
    if (error instanceof BrowserUnavailable) return { verdict: 'unavailable', checks: {}, diagnostic };
  }
  return {
    verdict: !diagnostic && Object.values(checks).every(Boolean) ? 'passed' : 'failed',
    checks, diagnostic,
  };
}
