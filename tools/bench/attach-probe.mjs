// 真实模型实测：任务附件（ATTACH-1）端到端。
//
// 验证三件事：
//   1. 附件真的落盘且详情能读到（数据层）；
//   2. 目标文本里出现「## 附件」段（注入层）——从 task_llm_calls.prompt_summary 读；
//   3. 模型**确实读了**该文件的内容（不是幻觉）——给它一个只有文件里才有的暗号。
//
// 用法：node tools/bench/attach-probe.mjs <baseUrl> <apiKey> <model>
import { readFileSync } from 'node:fs';

const [, , BASE = 'http://127.0.0.1:3034', KEY, MODEL] = process.argv;
if (!KEY) {
  console.error('缺少 apiKey');
  process.exit(2);
}

const log = (...a) => console.log('[probe]', ...a);

async function auth() {
  const r = await fetch(`${BASE}/api/bootstrap`, { headers: { 'X-Kedai-Boot': '1' } });
  const j = await r.json();
  return j.token;
}

function api(token) {
  const h = { authorization: `Bearer ${token}` };
  return {
    get: async (p) => {
      const r = await fetch(`${BASE}${p}`, { headers: h });
      return { status: r.status, body: await r.json().catch(() => null) };
    },
    postJson: async (p, b) => {
      const r = await fetch(`${BASE}${p}`, {
        method: 'POST',
        headers: { ...h, 'content-type': 'application/json' },
        body: JSON.stringify(b),
      });
      return { status: r.status, body: await r.json().catch(() => null) };
    },
    putJson: async (p, b) => {
      const r = await fetch(`${BASE}${p}`, {
        method: 'PUT',
        headers: { ...h, 'content-type': 'application/json' },
        body: JSON.stringify(b),
      });
      return { status: r.status, body: await r.json().catch(() => null) };
    },
    upload: async (p, files) => {
      const form = new FormData();
      for (const f of files) form.append('file', new Blob([f.bytes]), f.name);
      const r = await fetch(`${BASE}${p}`, { method: 'POST', headers: h, body: form });
      return { status: r.status, body: await r.json().catch(() => null) };
    },
  };
}

const token = await auth();
const apiClient = api(token);

// 1) 配置连接（隔离实例默认无 key）
log('配置连接…');
const cfg = await apiClient.putJson('/api/settings', {
  openai_base_url: process.env.KEDAI_TEST_BASE_URL || 'https://api.commandcode.ai/provider/v1',
  openai_api_key: KEY,
  model: MODEL || 'deepseek/deepseek-v4.1-flash',
  supports_vision: false,
});
log('settings', cfg.status, cfg.body?.ok ?? cfg.body?.error ?? '');

// 2) 建任务
const MARK = 'ATTACH-E2E-MARK-7F3A'; // 只出现在附件正文里
const title = '读一下随附的附件文件,把里面那串暗号原样告诉我';
const created = await apiClient.postJson('/api/tasks', { title, task_mode: 'solo' });
if (created.status !== 201) {
  console.error('建任务失败', created.status, created.body);
  process.exit(1);
}
const id = created.body.task.id;
log('task', id);

// 3) 上传 .txt（含暗号）+ .png
const up = await apiClient.upload(`/api/tasks/${id}/attachments`, [
  { name: '暗号.txt', bytes: `这是一份测试附件。\n暗号是:${MARK}\n请把它原样复述。\n` },
  { name: 'shot.png', bytes: Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]) },
]);
log('upload', up.status, JSON.stringify(up.body?.attachments?.map((a) => ({ name: a.name, rel: a.rel_path, size: a.size }))));
if (up.status !== 201) {
  console.error('上传失败', up.body);
  process.exit(1);
}

// 4) 详情核对
const detail = await apiClient.get(`/api/tasks/${id}`);
log('detail.attachments', detail.body?.attachments?.length);

// 5) 起跑并等待终态
const run = await apiClient.postJson(`/api/tasks/${id}/run`, {});
log('run', run.status);
const deadline = Date.now() + 300_000;
let final = null;
while (Date.now() < deadline) {
  await new Promise((r) => setTimeout(r, 3000));
  const d = await apiClient.get(`/api/tasks/${id}`);
  const st = d.body?.task?.status;
  if (['done', 'partial', 'error', 'ended'].includes(st)) {
    final = d.body;
    break;
  }
}
if (!final) {
  console.error('超时未到终态');
  process.exit(1);
}
log('terminal', final.task.status, 'error=', (final.task.error || '').slice(0, 160));
log('result 前 400 字:', (final.task.result || '').slice(0, 400));

// 6) 注入证据：从调用追踪读模型实际收到的 prompt
const calls = await apiClient.get(`/api/tasks/${id}/calls`);
const prompts = (calls.body?.calls ?? []).map((c) => c.prompt_summary || '').join('\n');
const hasSection = prompts.includes('## 附件');
const hasPath = prompts.includes('.kedai/attachments/') || prompts.includes('attachments/');
// 7) 「模型确实读了」的证据：结果里出现暗号
const resultText = `${final.task.result || ''}\n${(final.messages ?? []).map((m) => m.content || '').join('\n')}`;
const sawMark = resultText.includes(MARK);

const verdict = {
  taskStatus: final.task.status,
  attachmentsInDetail: (detail.body?.attachments ?? []).length,
  goalHasAttachmentSection: hasSection,
  goalHasRelPath: hasPath,
  modelSawMarker: sawMark,
  pass: hasSection && hasPath && sawMark,
};
log('VERDICT', JSON.stringify(verdict, null, 2));
process.exit(verdict.pass ? 0 : 1);
