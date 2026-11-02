#!/usr/bin/env node
// RTSPLIT 真实模型实测探针(2026-11-02)
//
// 目标:验证「运行时主提示词按模式拆两份」在**真实模型**下真的生效——
// 不是看代码,而是看模型收到的 system 里到底有哪一份。
//
// 判据(全部走真实 provider,不用 mock):
//   ① 任务模式写入 AGENTS_RUNTIME.task.md 后,任务执行者 system 含该文本;
//   ② 同一时刻角色扮演那份**不得**出现在任务 system 里(反向隔离);
//   ③ 两模式 prompt-preview 的 runtime 层各自命中,且响应回显 mode;
//   ④ GET/PUT agent-prompt 按 ?mode= 操作各自的文件。
//
// 用法:
//   . /d/kedai-bench-run/realmodel.env
//   BASE=http://127.0.0.1:3021 node tools/bench/rtsplit-probe.mjs
//
// 注意:凭据只在环境变量里,本文件不写任何 key。

const BASE = process.env.BASE || 'http://127.0.0.1:3021';
const OUT = process.env.OUT_DIR || 'D:/kedai-bench-run/rtsplit-evidence';

import { mkdirSync, writeFileSync } from 'node:fs';

mkdirSync(OUT, { recursive: true });

const log = [];
function note(line) {
  log.push(line);
  console.log(line);
}

/** bearer token:loopback 也要鉴权(见经验:真实 exe 的 API 探针要带 token) */
let TOKEN = process.env.KEDAI_TOKEN || '';
async function ensureToken() {
  if (TOKEN) return;
  const res = await fetch(`${BASE}/api/bootstrap`);
  const json = await res.json();
  TOKEN = json.token;
}

async function req(method, path, body) {
  await ensureToken();
  const headers = {};
  if (TOKEN) headers.authorization = `Bearer ${TOKEN}`;
  // 写请求一律带 JSON content-type:`/run`、`/approve` 等走 JsonBody 提取器,
  // 缺类型会 415(与前端 client.ts 的行为一致)。
  const payload = method === 'GET' ? undefined : JSON.stringify(body ?? {});
  if (payload !== undefined) headers['content-type'] = 'application/json';
  const res = await fetch(`${BASE}${path}`, { method, headers, body: payload });
  const text = await res.text();
  let json = null;
  try { json = JSON.parse(text); } catch { /* 非 JSON 原样留证 */ }
  return { status: res.status, json, text };
}

function layerContent(preview, source) {
  const layer = (preview?.layers || []).find((l) => l.source === source);
  return layer ? layer.content : null;
}

const findings = [];

async function main() {
  note(`BASE=${BASE}`);
  note(`时间=${new Date().toISOString()}`);

  // ===== 0. 连通性与 provider 确认 =====
  const info = await req('GET', '/api/settings/info');
  note(`[0] settings/info ${info.status}: connector=${info.json?.connector} model=${info.json?.model}`);
  if (info.status !== 200) throw new Error('服务未就绪');

  // ===== 1. 两个端点按模式操作各自文件 =====
  const rpBefore = await req('GET', '/api/settings/agent-prompt?mode=roleplay');
  const taskBefore = await req('GET', '/api/settings/agent-prompt?mode=task');
  note(`[1] GET roleplay -> ${rpBefore.status} mode=${rpBefore.json?.mode} path=${rpBefore.json?.path}`);
  note(`[1] GET task     -> ${taskBefore.status} mode=${taskBefore.json?.mode} path=${taskBefore.json?.path}`);
  findings.push({
    check: '两个端点路径不同且回显 mode',
    pass:
      rpBefore.status === 200 &&
      taskBefore.status === 200 &&
      rpBefore.json?.mode === 'roleplay' &&
      taskBefore.json?.mode === 'task' &&
      rpBefore.json?.path !== taskBefore.json?.path,
    detail: { rp: rpBefore.json?.path, task: taskBefore.json?.path },
  });

  // ===== 2. 写入两份不同的哨兵,验证互不覆盖 =====
  const RP = 'RTS-RP-真实模型哨兵:你是文学创作者。';
  const TK = 'RTS-TASK-真实模型哨兵:你是任务执行体。';
  const rpPut = await req('PUT', '/api/settings/agent-prompt?mode=roleplay', { content: RP });
  const taskPut = await req('PUT', '/api/settings/agent-prompt?mode=task', { content: TK });
  note(`[2] PUT roleplay -> ${rpPut.status} path=${rpPut.json?.path}`);
  note(`[2] PUT task     -> ${taskPut.status} path=${taskPut.json?.path}`);

  const rpAfter = await req('GET', '/api/settings/agent-prompt?mode=roleplay');
  const taskAfter = await req('GET', '/api/settings/agent-prompt?mode=task');
  findings.push({
    check: '写入后各读各的(互不覆盖)',
    pass: rpAfter.json?.content === RP && taskAfter.json?.content === TK,
    detail: { rp: rpAfter.json?.content, task: taskAfter.json?.content },
  });

  // ===== 3. 预览按模式取对应文件 =====
  const rpPrev = await req('GET', '/api/settings/prompt-preview?mode=roleplay');
  const taskPrev = await req('GET', '/api/settings/prompt-preview?mode=task');
  const rpLayer = layerContent(rpPrev.json, 'runtime_prompt');
  const taskLayer = layerContent(taskPrev.json, 'runtime_prompt');
  note(`[3] preview roleplay mode=${rpPrev.json?.mode} runtime层含RP=${String(rpLayer).includes('RTS-RP')}`);
  note(`[3] preview task     mode=${taskPrev.json?.mode} runtime层含TK=${String(taskLayer).includes('RTS-TASK')}`);
  findings.push({
    check: '预览 runtime 层按模式取对应文件且回显 mode',
    pass:
      rpPrev.json?.mode === 'roleplay' &&
      taskPrev.json?.mode === 'task' &&
      String(rpLayer).includes('RTS-RP') &&
      String(taskLayer).includes('RTS-TASK') &&
      !String(taskLayer).includes('RTS-RP'),
    detail: { rpLayerHead: String(rpLayer).slice(0, 80), taskLayerHead: String(taskLayer).slice(0, 80) },
  });

  // ===== 4. 真实模型:任务模式下真的把这层喂给模型 =====
  // 用真实 provider 跑一个最小任务,并让任务自己报告 system 里看到的哨兵。
  // 提示词要求模型原文复述哨兵,以证明它确实读到了。
  const goal =
    '只做一件事:查看你的系统提示词开头,把其中以 "RTS-TASK-" 开头的那一行原文复制出来作为回答。' +
    '如果你没看到任何以 RTS-TASK- 开头的行,就回答「未看到」。不要调用任何工具。';

  const created = await req('POST', '/api/tasks', { title: goal, task_mode: 'solo' });
  note(`[4] 创建任务 ${created.status} id=${created.json?.task?.id}`);
  if (created.status !== 201) throw new Error('任务创建失败');
  const taskId = created.json.task.id;

  const run = await req('POST', `/api/tasks/${taskId}/run`);
  note(`[4] run ${run.status}`);

  // 轮询到终态(真实模型,给足时间)
  let detail = null;
  const deadline = Date.now() + 300_000;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 3000));
    const r = await req('GET', `/api/tasks/${taskId}`);
    const status = r.json?.task?.status;
    if (['done', 'partial', 'error', 'ended'].includes(status)) {
      detail = r.json;
      note(`[4] 终态=${status}`);
      break;
    }
  }
  if (!detail) throw new Error('任务超时未终态');

  const subtaskText = (detail.subtasks || []).map((s) => s.result || '').join('\n');
  const taskResult = detail.task?.result || '';
  const sawTaskSentinel = /RTS-TASK-/.test(subtaskText + taskResult);
  const sawRpSentinel = /RTS-RP-/.test(subtaskText + taskResult);
  note(`[4] 模型输出含 RTS-TASK 哨兵=${sawTaskSentinel},含 RTS-RP 哨兵=${sawRpSentinel}`);

  findings.push({
    check: '真实模型:任务 system 里确实有任务那份运行时提示词',
    pass: sawTaskSentinel,
    detail: { subtaskHead: subtaskText.slice(0, 300), taskResultHead: taskResult.slice(0, 300) },
  });
  findings.push({
    check: '真实模型:任务 system 里绝不出现角色扮演那份',
    pass: !sawRpSentinel,
    detail: { sawRpSentinel },
  });

  // ===== 5. 角色扮演链同款验证(真实模型,最小 generate-raw) =====
  const raw = await req('POST', '/api/chat/generate-raw', {
    messages: [{ role: 'user', content: '只回答一个字:好' }],
    max_tokens: 256,
  });
  note(`[5] generate-raw ${raw.status}, usage=${JSON.stringify(raw.json?.usage || null)}`);
  findings.push({
    check: '角色扮演链真实模型连通(HTTP 200 且有 usage)',
    pass: raw.status === 200 && !!raw.json?.usage,
    detail: { status: raw.status, usage: raw.json?.usage },
  });

  // ===== 收尾:清空两份文件,避免污染 =====
  await req('PUT', '/api/settings/agent-prompt?mode=roleplay', { content: '' });
  await req('PUT', '/api/settings/agent-prompt?mode=task', { content: '' });
  note('[6] 已清空两份运行时提示词文件');

  const allPass = findings.every((f) => f.pass);
  note('');
  note('===== 判据汇总 =====');
  for (const f of findings) note(`${f.pass ? '[PASS]' : '[FAIL]'} ${f.check}`);
  note(`总计 ${findings.filter((f) => f.pass).length}/${findings.length}`);

  writeFileSync(
    `${OUT}/rtsplit-probe.json`,
    JSON.stringify({ base: BASE, at: new Date().toISOString(), findings, raw }, null, 2),
  );
  writeFileSync(`${OUT}/rtsplit-probe.log`, log.join('\n'));

  process.exit(allPass ? 0 : 1);
}

main().catch((e) => {
  note(`ERROR: ${e.message}`);
  writeFileSync(`${OUT}/rtsplit-probe.log`, log.join('\n'));
  process.exit(2);
});
