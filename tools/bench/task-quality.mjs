import { createHash } from 'node:crypto';
import { copyFile, cp, mkdir, readFile, readdir, realpath, stat, writeFile } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import { startBrowser } from './task-quality/browser.mjs';
import { caseDigest, cases, goalFor } from './task-quality/cases.mjs';
import { artifactsFor, gradeTask, localFile } from './task-quality/grade.mjs';
import { assertOutsideGit, readJson, writeJson } from './task-quality/host.mjs';
import { limits, provider, providerModelsForTask, runTask, startKedai, validatedConfig } from './task-quality/live.mjs';
import { blindIndex, compareRuns, schedule, summarizeReview } from './task-quality/results.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const stop = new AbortController();
const interrupt = () => stop.abort();
process.once('SIGINT', interrupt);
process.once('SIGTERM', interrupt);

async function outsideRepo(path) {
  return join(await assertOutsideGit(dirname(resolve(path))), basename(path));
}

async function graderDigest() {
  const hash = createHash('sha256').update(await readFile(fileURLToPath(import.meta.url)));
  for (const name of (await readdir(join(here, 'task-quality'))).sort()) {
    if (name.endsWith('.mjs') && !name.endsWith('.test.mjs')) {
      hash.update(name).update(await readFile(join(here, 'task-quality', name)));
    }
  }
  return hash.digest('hex');
}

const runDir = (root, { caseId, round, arm }) => join(root, 'private', caseId, String(round), arm);

async function createBlindBundle(root, rows) {
  const { labels, key } = blindIndex(rows.filter(({ verdict }) => verdict !== 'unavailable'));
  const bundle = join(root, 'review');
  await mkdir(bundle);
  const output = [];
  for (const label of labels) {
    const src = runDir(root, key[label.id]);
    const dest = join(bundle, label.id);
    const workspace = join(src, 'workspace');
    const files = [];
    await mkdir(dest);
    for (const name of await artifactsFor(label.caseId, workspace)) {
      const file = await localFile(workspace, name);
      if (!file || (await stat(file)).size > 256_000) continue;
      const path = join(dest, 'code', name);
      await mkdir(dirname(path), { recursive: true });
      await copyFile(file, path);
      files.push(`code/${name}`);
    }
    for (const shot of (await readdir(src)).filter((name) => name.endsWith('.png')).sort()) {
      const path = join(dest, 'screenshots', shot);
      await mkdir(dirname(path), { recursive: true });
      await copyFile(join(src, shot), path);
      files.push(`screenshots/${shot}`);
    }
    output.push({ ...label, files });
  }
  await writeJson(join(bundle, 'INDEX.json'), {
    rubric: {
      code: '0=无可审产物；1=脆弱/难维护；2=清晰且边界合理；3=结构、错误处理与扩展性出色。',
      visual: '0=不可用；1=可操作但可读性或层级弱；2=布局清晰、状态可读；3=视觉层级、移动布局与反馈出色。非图形样本或无截图填 null。',
    },
    prompts: Object.fromEntries(Object.entries(cases).map(([id, fixture]) => [id, goalFor(fixture)])),
    samples: output,
  });
  // 归属映射与盲审目录分离，先冻结评分再解盲。
  await writeJson(join(root, 'review-key.json'), key);
}

async function retainPairs(previous, root, plan, browser) {
  const old = await readJson(join(previous, 'plan.json'));
  for (const key of ['kind', 'provider', 'limits', 'authorization', 'node', 'browser', 'arms', 'fixtures', 'schedule']) {
    if (!isDeepStrictEqual(plan[key], old[key])) throw new Error(`续跑条件与原对照不一致：${key}`);
  }
  const oldRows = await readJson(join(previous, 'runs.json'));
  compareRuns(plan.schedule, oldRows); // 校验没有重复或非计划槽位。
  const retained = [];
  for (let i = 0; i < plan.schedule.length; i += 2) {
    const pair = plan.schedule.slice(i, i + 2);
    const candidates = [];
    for (const slot of pair) {
      const row = oldRows.find((entry) =>
        entry.caseId === slot.caseId && entry.round === slot.round && entry.arm === slot.arm);
      if (!row?.taskStatus) break;
      const src = runDir(previous, slot);
      const execution = await readJson(join(src, 'execution.json'));
      if (row.verdict === 'unavailable') {
        if (row.infrastructureIssue !== '未记录到指定真实模型调用' || execution.stopped !== '宿主墙钟到期') break;
        const log = await readFile(join(previous, `server-${slot.arm}`, 'process.log'), 'utf8');
        execution.model = providerModelsForTask(log, execution.id);
        execution.modelEvidence = 'provider-round-start';
      }
      if (execution.model.length === 0 || execution.model.some((actual) => actual !== provider.model)) break;
      candidates.push({ row, src, execution });
    }
    if (candidates.length !== pair.length) break;
    for (const { row, src, execution } of candidates) {
      const dest = runDir(root, row);
      await mkdir(dest, { recursive: true });
      await cp(join(src, 'workspace'), join(dest, 'workspace'), { recursive: true, force: false, errorOnExist: true });
      // 用当前同一评分器重验旧产物，不消耗额度，也不复用旧分数。
      const grade = await gradeTask(row.caseId, join(dest, 'workspace'), dest, browser);
      if (grade.verdict === 'unavailable') throw new Error('续跑前无法独立复验已有产物');
      const retainedRow = { ...row, infrastructureIssue: null, ...grade };
      await writeJson(join(dest, 'execution.json'), execution);
      await writeJson(join(dest, 'score.json'), retainedRow);
      retained.push(retainedRow);
    }
  }
  await writeJson(join(root, 'continuation.json'), {
    previous: await realpath(previous),
    previousPlanSha256: createHash('sha256').update(await readFile(join(previous, 'plan.json'))).digest('hex'),
    retainedPairs: retained.length / 2,
  });
  return retained;
}

async function launchLab(configFile, out, mode, previous) {
  const config = await validatedConfig(await readJson(configFile));
  const live = mode === 'run' || mode === 'resume';
  const rescore = mode === 'rescore';
  const key = live ? process.env.KEDAI_BENCH_KEY?.trim() : null;
  if (live && !key) throw new Error('真实评测需要外部环境变量 KEDAI_BENCH_KEY');
  const root = await outsideRepo(out);
  await mkdir(root);
  const planned = schedule(Object.keys(cases), config.rounds);
  const servers = {};
  let browser;
  try {
    browser = await startBrowser(config.browser, join(root, 'chromium-profile'));
    const probe = join(root, 'browser-preflight');
    await mkdir(probe);
    await writeFile(join(probe, 'index.html'), '<!doctype html><meta charset="utf-8"><p>ready</p>', { flag: 'wx' });
    await browser.withPage(probe, async (page) => {
      if (await page.evaluate('document.body.textContent') !== 'ready') throw new Error('独立浏览器验收预检失败');
    });
    if (!rescore) {
      for (const [arm, spec] of Object.entries(config.arms)) {
        servers[arm] = await startKedai(spec, config.browser, join(root, `server-${arm}`), key);
      }
    }
    const plan = {
      kind: mode === 'preflight' ? 'no-model-preflight' : 'real-model',
      provider: mode === 'preflight' ? null : provider, limits, execution: mode,
      authorization: 'workspace; deny all pending escalations',
      node: process.version, browser: browser.version, graderSha256: await graderDigest(),
      arms: Object.fromEntries(Object.entries(config.arms).map(([name, { revision, sha256 }]) => [name, { revision, sha256 }])),
      fixtures: Object.fromEntries(Object.entries(cases).map(([id, fixture]) => [id, caseDigest(fixture)])),
      schedule: planned,
    };
    await writeJson(join(root, 'plan.json'), plan);
    if (mode === 'preflight') {
      const fixture = { title: '传输冒烟', prompt: '只返回检查完毕，不需要写文件。', seed: {} };
      for (const [arm, server] of Object.entries(servers)) {
        const smoke = join(root, `mock-${arm}`);
        await mkdir(smoke);
        const result = await runTask(server, fixture, join(smoke, 'workspace'), smoke, stop.signal);
        if (result.infrastructureIssue) throw new Error(`隔离任务传输预检失败：${result.infrastructureIssue}`);
      }
      console.log('浏览器、实际预算和两端任务 API 通过预检（仅桩，不调用真实提供方）。');
      return;
    }

    const rows = previous ? await retainPairs(previous, root, plan, browser) : [];
    if (rescore && rows.length !== planned.length) throw new Error('离线重验只接受完整的已执行对照');
    if (previous) console.log(`已独立复验并保留 ${rows.length / 2} 对旧样本。`);
    let endReason = null;
    for (let i = rows.length; live && i < planned.length; i += 2) {
      if (stop.signal.aborted) { endReason = '用户中断'; break; }
      const pair = planned.slice(i, i + 2);
      console.log(`开始第 ${pair[0].round} 轮 ${pair[0].caseId}（${i + 1}–${i + 2}/${planned.length}）`);
      const attempts = await Promise.allSettled(pair.map(async (slot) => {
        const dest = runDir(root, slot);
        await mkdir(dest, { recursive: true });
        return runTask(servers[slot.arm], cases[slot.caseId], join(dest, 'workspace'), dest, stop.signal);
      }));
      for (const [j, slot] of pair.entries()) {
        const attempt = attempts[j];
        const dest = runDir(root, slot);
        const row = { ...slot, taskStatus: null, elapsedMs: null, usage: null, verdict: 'unavailable', checks: {} };
        if (attempt.status === 'rejected') {
          row.diagnostic = String(attempt.reason?.message ?? attempt.reason).replaceAll(key, '[密钥已隐藏]').slice(0, 600);
          endReason = '隔离任务或 API 不可用';
        } else {
          Object.assign(row, attempt.value);
          if (row.infrastructureIssue) {
            endReason = row.infrastructureIssue;
          } else {
            Object.assign(row, await gradeTask(slot.caseId, join(dest, 'workspace'), dest, browser));
            if (row.verdict === 'unavailable') endReason = '独立浏览器验收不可用';
          }
        }
        rows.push(row);
        await writeJson(join(dest, 'score.json'), row);
      }
      if (endReason) break;
    }
    await writeJson(join(root, 'runs.json'), rows);
    await writeJson(join(root, 'automatic.json'), { endReason, ...compareRuns(planned, rows) });
    if (rescore) {
      // 自动判据可以修正；已冻结的盲评素材和映射不随自动分变化。
      await cp(join(previous, 'review'), join(root, 'review'), { recursive: true, force: false, errorOnExist: true });
      await copyFile(join(previous, 'review-key.json'), join(root, 'review-key.json'));
    } else {
      await createBlindBundle(root, rows);
    }
    if (endReason || rows.length !== planned.length) throw new Error(`对照未跑完：${endReason ?? '中断'}（已记录 ${rows.length} 个槽位）`);
    console.log(rescore ? '离线重验完成；未调用模型，盲审素材原样保留。' : '对照执行完毕；自动评分和盲审样本均已落盘。');
  } finally {
    const cleanup = await Promise.allSettled([
      ...Object.values(servers).map((server) => server.close()), ...(browser ? [browser.close()] : []),
    ]);
    await writeJson(join(root, 'cleanup.json'), cleanup.map(({ status }) => status));
    if (cleanup.some(({ status }) => status !== 'fulfilled')) throw new Error('有隔离进程未完成清理，请查 cleanup.json');
  }
}

async function report(root, review) {
  root = await assertOutsideGit(root);
  const plan = await readJson(join(root, 'plan.json'));
  const runs = await readJson(join(root, 'runs.json'));
  const automatic = await readJson(join(root, 'automatic.json'));
  const key = await readJson(join(root, 'review-key.json'));
  const ratings = await readJson(review);
  const summary = {
    kind: plan.kind, provider: plan.provider, limits: plan.limits, arms: plan.arms, graderSha256: plan.graderSha256,
    automatic, blind: summarizeReview(key, runs, ratings),
    ratingsSha256: createHash('sha256').update(await readFile(review)).digest('hex'),
  };
  await writeJson(join(root, 'report.json'), summary);
  console.log(JSON.stringify({ pairs: automatic.pairs, arms: automatic.arms, blind: summary.blind }, null, 2));
}

const [command, ...args] = process.argv.slice(2);
try {
  if (['preflight', 'run'].includes(command) && args.length === 2) await launchLab(args[0], args[1], command);
  else if (['resume', 'rescore'].includes(command) && args.length === 3) await launchLab(args[0], args[2], command, args[1]);
  else if (command === 'report' && args.length === 2) await report(args[0], args[1]);
  else throw new Error('用法：task-quality.mjs preflight|run <配置> <新目录>；resume|rescore <配置> <旧目录> <新目录>；report <评测目录> <盲评分>');
} catch (error) {
  const key = process.env.KEDAI_BENCH_KEY?.trim();
  console.error(key ? error.message.replaceAll(key, '[密钥已隐藏]') : error.message);
  process.exitCode = 1;
}
