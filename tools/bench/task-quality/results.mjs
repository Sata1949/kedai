import { randomUUID } from 'node:crypto';

const arms = ['baseline', 'candidate'];
const idOf = ({ caseId, round, arm }) => `${caseId}/${round}/${arm}`;

export function schedule(caseIds, rounds) {
  return Array.from({ length: rounds }, (_, r) => caseIds.flatMap((caseId, i) => {
    // 轮内交替，下一轮同一题交换先后，减小时段和网关缓存先热后冷的偏差。
    const order = (r + i) % 2 === 0 ? arms : [...arms].reverse();
    return order.map((arm) => ({ caseId, round: r + 1, arm }));
  })).flat();
}

function summarize(planned, actual) {
  const scored = actual.filter(({ verdict }) => verdict !== 'unavailable');
  const completed = scored.filter(({ taskStatus }) => taskStatus === 'done');
  const checks = scored.flatMap((run) => Object.values(run.checks));
  const usage = actual.map((run) => run.usage).filter(Boolean);
  const measured = actual.map((run) => run.elapsedMs).filter(Number.isFinite);
  return {
    missing: planned.length - actual.length,
    unavailable: actual.length - scored.length,
    success: { numerator: scored.filter(({ verdict }) => verdict === 'passed').length, denominator: scored.length },
    coverage: { numerator: checks.filter(Boolean).length, denominator: checks.length },
    falseDone: { numerator: completed.filter(({ verdict }) => verdict === 'failed').length, denominator: completed.length },
    usage: {
      measuredRuns: usage.length,
      input: usage.reduce((sum, row) => sum + row.input, 0),
      output: usage.reduce((sum, row) => sum + row.output, 0),
      reasoning: usage.reduce((sum, row) => sum + row.reasoning, 0),
    },
    elapsedMs: measured.length ? { measuredRuns: measured.length, total: measured.reduce((a, b) => a + b, 0) } : null,
  };
}

export function compareRuns(planned, rows) {
  const slots = new Set(planned.map(idOf));
  const actual = new Map();
  for (const row of rows) {
    const id = idOf(row);
    if (!slots.has(id) || actual.has(id)) throw new Error(`重复或非计划运行：${id}`);
    actual.set(id, row);
  }
  const baseline = planned.filter(({ arm }) => arm === 'baseline');
  const pairs = { planned: baseline.length, scored: 0, improved: 0, regressed: 0, tied: 0 };
  for (const slot of baseline) {
    const base = actual.get(idOf(slot));
    const next = actual.get(idOf({ ...slot, arm: 'candidate' }));
    if (!base || !next || [base.verdict, next.verdict].includes('unavailable')) continue;
    pairs.scored++;
    if (base.verdict === next.verdict) pairs.tied++;
    else if (next.verdict === 'passed') pairs.improved++;
    else pairs.regressed++;
  }
  const group = (slots, runs) => Object.fromEntries(arms.map((arm) => [
    arm, summarize(slots.filter((row) => row.arm === arm), runs.filter((row) => row.arm === arm)),
  ]));
  return {
    pairs,
    arms: group(planned, rows),
    cases: Object.fromEntries([...new Set(planned.map(({ caseId }) => caseId))].map((caseId) => [
      caseId, group(planned.filter((row) => row.caseId === caseId), rows.filter((row) => row.caseId === caseId)),
    ])),
  };
}

export function blindIndex(rows, newId = randomUUID) {
  const key = {};
  const labels = rows.map(({ caseId, round, arm }) => {
    const id = newId();
    key[id] = { caseId, round, arm };
    return { id, caseId };
  });
  // UUID 排序与样本生成顺序无关；索引和审阅包不包含自动评分或运行归属。
  labels.sort((a, b) => a.id.localeCompare(b.id));
  return { labels, key };
}

export function summarizeReview(key, runs, ratings) {
  if (!Array.isArray(ratings)) throw new Error('盲评输入须为评分数组');
  const known = new Set(runs.filter(({ verdict }) => verdict !== 'unavailable').map(idOf));
  const scored = new Map();
  const validScore = (value) => Number.isInteger(value) && value >= 0 && value <= 3;
  for (const { id, code, visual, note } of ratings) {
    const label = key[id];
    if (!label || !known.has(idOf(label)) || scored.has(id) || !validScore(code)
      || (visual !== null && (!['ui', 'graphics'].includes(label.caseId) || !validScore(visual)))
      || typeof note !== 'string' || !note.trim() || note.length > 600) {
      throw new Error(`盲评分项无效或重复：${id}`);
    }
    scored.set(id, { ...label, code, visual });
  }
  const grades = [...scored.values()];
  const average = (values) => ({
    n: values.length, mean: values.length ? values.reduce((a, b) => a + b, 0) / values.length : null,
  });
  return Object.fromEntries(arms.map((arm) => {
    const rows = grades.filter((row) => row.arm === arm);
    return [arm, {
      missing: Object.values(key).filter((row) => row.arm === arm).length - rows.length,
      code: average(rows.map(({ code }) => code)),
      visual: average(rows.map(({ visual }) => visual).filter((value) => value !== null)),
    }];
  }));
}
