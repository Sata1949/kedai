import assert from 'node:assert/strict';
import { test } from 'node:test';
import { gradeGraphics, terrainSpec } from './graphics.mjs';
import { denyPending, provider, providerModelsForTask } from './live.mjs';
import { blindIndex, compareRuns, schedule, summarizeReview } from './results.mjs';

function pixelFixture(defect) {
  const palette = [0x588741, 0x689a50, 0x769f57, 0x467037];
  const stone = [0x919196, 0x727276, 0x616169, 0xa2a2ad];
  const paint = (front) => (x, y) => {
    if (x < 64 && y < 32) {
      if (defect === 'nearest' && x === 1 && y === 0) return 0x6b8e60;
      const colors = x < 32 || defect === 'diversity' ? palette : stone;
      return colors[(Math.floor(x / 4) + Math.floor(y / 4) * 3) % 4];
    }
    if (x >= 72 && x < 88 && y < 16) {
      return x < 80 || defect === 'alpha' ? 0x45b567 : 0x172235;
    }
    if (x >= 96 && x < 112 && y < 16) {
      return front || defect === 'cull' ? 0x4e9ceb : 0x172235;
    }
    if (x >= 120 && x < 136 && y < 16) {
      return x < 128 && defect !== 'ao' ? 0x555555 : 0x888888;
    }
    return 0x172235;
  };
  return { front: paint(true), back: paint(false) };
}

test('专项规则逐项识别真实像素缺陷，不借源码关键词判定', () => {
  const ideal = gradeGraphics(pixelFixture(), terrainSpec);
  assert.deepEqual(Object.values(ideal), [true, true, true, true, true]);
  for (const [defect, failed] of [
    ['alpha', 'alpha-cutout'], ['cull', 'backface-cull'], ['ao', 'ao-shading'],
    ['diversity', 'texture-diversity'], ['nearest', 'nearest-sampling'],
  ]) {
    const grade = gradeGraphics(pixelFixture(defect), terrainSpec);
    assert.deepEqual(Object.entries(grade).filter(([, ok]) => !ok).map(([id]) => id), [failed]);
  }
});

test('未要求的合法双面、无 AO 和线性过滤不扣分', () => {
  const spec = terrainSpec.filter(({ id }) => id === 'alpha-cutout');
  for (const defect of ['cull', 'ao', 'nearest', 'diversity']) {
    assert.deepEqual(gradeGraphics(pixelFixture(defect), spec), { 'alpha-cutout': true });
  }
  assert.deepEqual(gradeGraphics(pixelFixture('alpha'), []), {});
});

test('每类各两轮成对、交错先后顺序', () => {
  const runs = schedule(['ui', 'repo'], 2);
  assert.equal(runs.length, 8);
  assert.deepEqual(runs.map(({ arm }) => arm), [
    'baseline', 'candidate', 'candidate', 'baseline',
    'candidate', 'baseline', 'baseline', 'candidate',
  ]);
  for (let i = 0; i < runs.length; i += 2) {
    assert.equal(runs[i].caseId, runs[i + 1].caseId);
    assert.equal(runs[i].round, runs[i + 1].round);
  }
});

test('独立失败不被 done 覆盖，未验和半对不混入分母', () => {
  const planned = schedule(['ui', 'repo'], 1);
  const rows = [
    { ...planned[0], taskStatus: 'done', verdict: 'failed', checks: { a: true, b: false }, usage: null, elapsedMs: 3 },
    { ...planned[1], taskStatus: 'partial', verdict: 'passed', checks: { a: true, b: true }, usage: { input: 10, output: 2, reasoning: 1 }, elapsedMs: 5 },
    { ...planned[2], taskStatus: 'done', verdict: 'unavailable', checks: {}, usage: null, elapsedMs: 8 },
  ];
  const report = compareRuns(planned, rows);
  assert.deepEqual(report.pairs, { planned: 2, scored: 1, improved: 1, regressed: 0, tied: 0 });
  assert.deepEqual(report.arms.baseline.falseDone, { numerator: 1, denominator: 1 });
  assert.deepEqual(report.arms.baseline.coverage, { numerator: 1, denominator: 2 });
  assert.deepEqual(report.arms.candidate.success, { numerator: 1, denominator: 1 });
  assert.deepEqual(report.arms.candidate.usage, { measuredRuns: 1, input: 10, output: 2, reasoning: 1 });
  assert.equal(report.arms.candidate.unavailable, 1);
  assert.equal(report.arms.baseline.missing, 1);
  assert.throws(() => compareRuns(planned, [...rows, rows[0]]), /重复/);
});

test('盲审公开索引不包含版本、轮次、路径和通过状态', () => {
  const rows = [
    { caseId: 'ui', arm: 'baseline', round: 1, verdict: 'passed', taskStatus: 'done' },
    { caseId: 'ui', arm: 'candidate', round: 1, verdict: 'failed', taskStatus: 'partial' },
  ];
  let n = 0;
  const { labels, key } = blindIndex(rows, () => `opaque-${++n}`);
  assert.deepEqual(labels, [{ id: 'opaque-1', caseId: 'ui' }, { id: 'opaque-2', caseId: 'ui' }]);
  assert.deepEqual(key['opaque-1'], { caseId: 'ui', round: 1, arm: 'baseline' });
  assert.doesNotMatch(JSON.stringify(labels), /baseline|candidate|round|passed|failed|partial/);
});

test('盲评解盲按已评样本计数，拒绝重复及对非视觉题打视觉分', () => {
  const runs = [
    { caseId: 'api', arm: 'baseline', round: 1, verdict: 'passed' },
    { caseId: 'ui', arm: 'candidate', round: 1, verdict: 'passed' },
  ];
  let n = 0;
  const { key } = blindIndex(runs, () => `blind-${++n}`);
  const first = { id: 'blind-1', code: 2, visual: null, note: '路由职责清楚' };
  const good = summarizeReview(key, runs, [first]);
  assert.deepEqual(good.baseline.visual, { n: 0, mean: null });
  assert.deepEqual(good.baseline.code, { n: 1, mean: 2 });
  assert.equal(good.candidate.missing, 1);
  assert.throws(() => summarizeReview(key, runs, [first, first]), /无效或重复/);
  assert.throws(() => summarizeReview(key, runs, [{ ...first, visual: 2 }]), /无效或重复/);
});

test('待确认队列按调用身份显式拒绝，不等待任务墙钟也不批准', async () => {
  const calls = [];
  const api = async (...args) => {
    calls.push(args);
    return { pending: [{ name: 'bash', session_id: 'task:1', run_id: 'run', call_id: 'call' }] };
  };
  assert.equal(await denyPending(api, '1'), 1);
  assert.deepEqual(calls, [
    ['/api/tasks/1/tool-authorizations'],
    ['/api/agent/tool-permissions/resolve', 'POST', {
      tool: 'bash', session_id: 'task:1', run_id: 'run', call_id: 'call', scope: 'deny',
    }],
  ]);
});

test('超时请求日志只证明本题实际连接，不能借前后任务补证', () => {
  const started = (id) => ({ msg: 'http', method: 'POST', path: `/api/tasks/${id}/run`, status: 200 });
  const call = (model, host = 'api.commandcode.ai') => ({ msg: 'provider_round_start', provider_host: host, model });
  const log = [
    started('before'), call('wrong-before'), started('sample'),
    call('other-host', 'example.invalid'), call(provider.model), started('after'), call('wrong-after'),
  ].map((event) => JSON.stringify(event)).join('\n') + '\n';
  assert.deepEqual(providerModelsForTask(log, 'sample'), [provider.model]);
  assert.deepEqual(providerModelsForTask(log, 'missing'), []);
});
