// 双模式附件实测：legacy（无工具）与 solo（有工具）。
// 验证注入文案按模式分叉，且模型不是幻觉。
const BASE = 'http://127.0.0.1:3035';

async function token() {
  const r = await fetch(`${BASE}/api/bootstrap`);
  return (await r.json()).token;
}
const H = (t) => ({ authorization: `Bearer ${t}` });

async function runCase(tok, mode, mark, fileName) {
  const mk = await (
    await fetch(`${BASE}/api/tasks`, {
      method: 'POST',
      headers: { ...H(tok), 'content-type': 'application/json' },
      body: JSON.stringify({
        title: '列出你收到的附件文件名；如果能读取内容，把文件里的暗号原样告诉我',
        task_mode: mode,
      }),
    })
  ).json();
  const id = mk.task.id;
  const form = new FormData();
  form.append('file', new Blob([Buffer.from(`${mark}\n这是 ${mode} 模式的附件。\n`)]), fileName);
  const up = await fetch(`${BASE}/api/tasks/${id}/attachments`, {
    method: 'POST',
    headers: H(tok),
    body: form,
  });
  await fetch(`${BASE}/api/tasks/${id}/run`, {
    method: 'POST',
    headers: { ...H(tok), 'content-type': 'application/json' },
    body: '{}',
  });
  for (let i = 0; i < 80; i++) {
    await new Promise((r) => setTimeout(r, 3000));
    const d = await (await fetch(`${BASE}/api/tasks/${id}`, { headers: H(tok) })).json();
    if (['done', 'partial', 'error', 'ended'].includes(d.task.status)) {
      const c = await (await fetch(`${BASE}/api/tasks/${id}/calls`, { headers: H(tok) })).json();
      const rows = c.calls || [];
      const byPhase = (p) => rows.filter((r) => r.phase === p).map((r) => r.prompt_summary || '').join('\n');
      return {
        mode,
        status: d.task.status,
        uploadStatus: up.status,
        attachmentPromptHasSection: {
          planner: byPhase('planner').includes('## 附件'),
          step: byPhase('step').includes('## 附件'),
          summarize: byPhase('summarize').includes('## 附件'),
        },
        promptSaysCannotRead: byPhase('step').includes('本模式无读取工具'),
        promptSaysFsRead: byPhase('step').includes('用 fs_read 读取'),
        resultMentionsFile: (d.task.result || '').includes(fileName.replace(/\.txt$/, '')),
        resultHasMark: (d.task.result || '').includes(mark),
        result: (d.task.result || '').slice(0, 420),
      };
    }
  }
  return { mode, status: 'timeout' };
}

const tok = await token();
for (const [mode, mark, f] of [
  ['legacy', 'LEGACY-MARK-9Q2Z', 'legacy说明.txt'],
  ['solo', 'SOLO-MARK-4W8K', 'solo说明.txt'],
]) {
  const out = await runCase(tok, mode, mark, f);
  console.log('\n===== ' + mode + ' =====');
  console.log(JSON.stringify(out, null, 2));
}
