import { createHash } from 'node:crypto';
import { mkdir, open, readFile, readdir, realpath } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { goalFor, seedWorkspace } from './cases.mjs';
import { hostEnv, launch, until, writeJson } from './host.mjs';

export const provider = Object.freeze({
  baseUrl: 'https://api.commandcode.ai/provider/v1',
  model: 'deepseek/deepseek-v4.1-flash',
});
export const limits = Object.freeze({
  default_max_tokens: 4096,
  default_temperature: 0.2,
  max_context_tokens: 65536,
  tool_history_budget_tokens: 24576,
  task_step_budget_secs: 180,
  task_total_budget_secs: 180,
  session_token_budget: 220000,
  session_budget_action: 'stop',
});
const tools = ['fs_read', 'fs_glob', 'fs_grep', 'fs_write', 'fs_edit', 'fs_patch',
  'bash', 'exec_log_read', 'todo', 'view_image'];
const terminal = ['done', 'partial', 'error', 'ended'];
const providerFailure = /(?:\bHTTP\s*|\bstatus\s*[:=]?\s*|\()(?:401|402|403|429)\b|too many requests|insufficient (?:funds|quota|credits)|rate.?limit|quota exceeded|额度不足|余额不足/i;

const absolutePath = (value) => resolve(value).replace(/^[a-z]:/i, (prefix) => prefix.toUpperCase());
const sum = (counts) => Number.isSafeInteger(counts) && counts >= 0;

export async function validatedConfig(config) {
  if (![1, 2].includes(config.rounds)) throw new Error('rounds 只接受 1 或 2（最多 20 个任务）');
  const arms = {};
  for (const arm of ['baseline', 'candidate']) {
    const entry = config.arms?.[arm];
    if (!entry || !/^[a-f0-9]{40}$/.test(entry.revision) || !/^[a-f0-9]{64}$/.test(entry.sha256)
      || !Number.isInteger(entry.port) || entry.port < 1024 || entry.port > 65535) {
      throw new Error(`${arm} 需要 revision、sha256、binary、port`);
    }
    const binary = await realpath(entry.binary);
    const hash = createHash('sha256').update(await readFile(binary)).digest('hex');
    if (hash !== entry.sha256) throw new Error(`${arm} 二进制 SHA-256 不符`);
    arms[arm] = { ...entry, binary };
  }
  if (arms.baseline.port === arms.candidate.port || arms.baseline.revision === arms.candidate.revision
    || arms.baseline.sha256 === arms.candidate.sha256) throw new Error('两组需有不同版本、二进制和本地端口');
  return { rounds: config.rounds, arms, browser: await realpath(config.browser) };
}

async function observedDataDir(logs, port) {
  let dataDir = null;
  let ready = false;
  for (const file of await readdir(logs, { withFileTypes: true })) {
    if (!file.isFile()) continue;
    for (const line of (await readFile(join(logs, file.name), 'utf8')).split('\n')) {
      if (!line.includes('"data_dir"') && !line.includes('Kedai 已启动')) continue;
      const event = JSON.parse(line);
      const fields = event.fields ?? event;
      if (typeof fields.data_dir === 'string') dataDir = absolutePath(fields.data_dir);
      if (fields.url === `http://127.0.0.1:${port}/`) ready = true;
    }
  }
  // 监听成功的日志属于本进程，避免误把占用同一端口的实例当实验服务器写入。
  return ready ? dataDir : null;
}

export async function startKedai(spec, browser, isolation, key) {
  const data = join(isolation, 'data');
  const logs = join(isolation, 'logs');
  await mkdir(isolation);
  await mkdir(logs);
  const processLog = join(isolation, 'process.log');
  const pipe = await open(processLog, 'wx');
  const run = launch(spec.binary, [], {
    cwd: isolation,
    env: { ...hostEnv, DATA_DIR: data, LOG_DIR: logs, PORT: String(spec.port), HOST: '127.0.0.1',
      CONNECTOR: 'mock', LOG_LEVEL: 'info', RUST_LOG: 'info', KEDAI_HEADLESS_BROWSER: browser },
    stdio: ['ignore', pipe.fd, pipe.fd],
  });
  await pipe.close();
  let token;
  const api = async (route, method = 'GET', body) => {
    const response = await fetch(`http://127.0.0.1:${spec.port}${route}`, {
      method,
      headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(6000),
    });
    if (!response.ok) throw new Error(`${route}：HTTP ${response.status}`);
    return response.json();
  };
  try {
    await run.started;
    const observed = await until(() => {
      if (run.child.exitCode !== null) throw new Error(`隔离服务退出：${run.child.exitCode}`);
      return observedDataDir(logs, spec.port);
    }, 10_000, '隔离服务 data_dir 日志');
    if (observed !== absolutePath(data)) throw new Error('实际 DATA_DIR 与本次隔离目录不一致，拒绝写设置');
    token = await until(async () => {
      if (run.child.exitCode !== null) throw new Error(`隔离服务退出：${run.child.exitCode}`);
      try {
        const state = await api('/api/bootstrap');
        if (!state.token) throw new Error('隔离服务未返回访问令牌');
        return state.token;
      } catch (error) {
        if (error.cause?.code === 'ECONNREFUSED') return null;
        throw error;
      }
    }, 8000, '隔离服务启动');
    await writeJson(join(isolation, 'isolation.json'), { dataDir: observed, port: spec.port, revision: spec.revision });
    const connection = key
      ? { connector_type: 'openai-compatible', base_url: provider.baseUrl, api_key: key, model: provider.model, supports_vision: true }
      : { connector_type: 'mock', model: 'mock' };
    const settings = {
      ...limits,
      task_coding_bundle_enabled: true,
      exec_enabled: true, vision_screenshot_enabled: true,
      task_tool_policy: 'allowlist', task_tool_allowlist: tools,
      connections: [{ name: '任务质量实验连接', enabled: true, ...connection }],
    };
    await api('/api/settings', 'PUT', settings);
    await api('/api/settings?mode=task', 'PUT', settings);
    const actual = await api('/api/settings?mode=task');
    const selected = actual.connections.find((entry) => entry.name === settings.connections[0].name);
    if (!selected?.id || selected.model !== connection.model || selected.connector_type !== connection.connector_type
      || selected.enabled !== true) throw new Error('实际选中连接与实验配置不一致');
    for (const [name, value] of Object.entries(limits)) {
      if (actual[name] !== value) throw new Error(`实际任务预算 ${name} 与实验配置不一致`);
    }
    return { api, connectionId: selected.id, model: connection.model, processLog, close: () => run.stop() };
  } catch (error) {
    await run.stop();
    throw error;
  }
}

export async function denyPending(api, taskId) {
  const { pending } = await api(`/api/tasks/${taskId}/tool-authorizations`);
  for (const { name, session_id, run_id, call_id } of pending) {
    // 待确认队列不在任务详情里。两组都显式拒绝越界申请，让模型在原权限内继续，
    // 不以审批等待耗尽任务预算，也不替模型授予更大权限。
    await api('/api/agent/tool-permissions/resolve', 'POST', {
      tool: name, session_id, run_id, call_id, scope: 'deny',
    });
  }
  return pending.length;
}

export function providerModelsForTask(log, taskId) {
  const route = `/api/tasks/${taskId}/run`;
  const providerHost = new URL(provider.baseUrl).hostname;
  const models = new Set();
  let active = false;
  // 每个隔离实例同一时刻只跑一题。按 /run 的任务边界取请求事实，避免把前后题的日志串起来。
  for (const line of log.split('\n').slice(0, -1)) {
    if (!line.includes('"provider_round_start"') && !line.includes('/run"')) continue;
    const event = JSON.parse(line);
    if (event.msg === 'http' && event.method === 'POST' && /^\/api\/tasks\/[^/]+\/run$/.test(event.path)) {
      if (active) break;
      active = event.path === route && event.status === 200;
    }
    if (active && event.msg === 'provider_round_start' && event.provider_host === providerHost) models.add(event.model);
  }
  return [...models];
}

export async function runTask(server, fixture, workspace, evidence, signal) {
  await seedWorkspace(fixture, workspace);
  const { api, connectionId, model, processLog } = server;
  const { task } = await api('/api/tasks', 'POST', {
    title: goalFor(fixture), task_mode: 'solo', workspace,
    connection_id: connectionId, tool_authorization: 'workspace',
  });
  if (!task?.id || task.connection_id !== connectionId || task.task_mode !== 'solo'
    || task.tool_authorization !== 'workspace') throw new Error('任务创建回执与实验配置不一致');
  const id = task.id;
  const begin = Date.now();
  let detail;
  let stopped = null;
  let denied = 0;
  try {
    await api(`/api/tasks/${id}/run`, 'POST', {});
    do {
      detail = await api(`/api/tasks/${id}`);
      if (terminal.includes(detail.task.status)) break;
      if (!stopped && (signal.aborted || Date.now() - begin > (limits.task_total_budget_secs + 8) * 1000)) {
        stopped = signal.aborted ? '用户中断' : '宿主墙钟到期';
        await api(`/api/tasks/${id}/stop`, 'POST', {});
      }
      if (!stopped) denied += await denyPending(api, id);
      if (Date.now() - begin > (limits.task_total_budget_secs + 15) * 1000) throw new Error('停止后仍未得到任务终态');
      await delay(500);
    } while (true);
  } finally {
    if (!stopped && (!detail || !terminal.includes(detail.task.status))) {
      await api(`/api/tasks/${id}/stop`, 'POST', {});
    }
  }
  const elapsedMs = Date.now() - begin;
  const { calls } = await api(`/api/tasks/${id}/calls`);
  let models = [...new Set(calls.map((call) => call.model))];
  let modelEvidence = 'call-rows';
  if (models.length === 0 && stopped === '宿主墙钟到期' && model !== 'mock') {
    // 被宿主取消的旧版循环可能来不及聚合调用行；请求日志能证明选中模型，
    // 不能补造缺失的 token 用量。
    models = providerModelsForTask(await readFile(processLog, 'utf8'), id);
    modelEvidence = 'provider-round-start';
  }
  // 桩在不同版本可能标记为 mock 或 mock-demo；传输预检只要求确有调用，
  // 正式样本则必须使用指定的真实模型，不能把回退连接计入对照。
  const unexpectedModel = models.length === 0 || (model !== 'mock' && models.some((actual) => actual !== model));
  const issue = [detail.task.error, ...calls.filter(({ status }) => status !== 'ok').map(({ response_summary }) => response_summary)]
    .filter(Boolean).find((message) => providerFailure.test(message));
  const usage = detail.usage_total;
  const counts = [usage.prompt_tokens, usage.completion_tokens, usage.reasoning_tokens];
  // 无提供方统计时保留 null，不能用零费用或推理成本估计顶替实测。
  const tokens = counts.every(sum) && counts[0] + counts[1] > 0
    ? { input: counts[0], output: counts[1], reasoning: counts[2] } : null;
  await writeJson(join(evidence, 'execution.json'), {
    id, status: detail.task.status, stopped, elapsedMs, denied, model: models, modelEvidence,
    usage: tokens, taskError: detail.task.error?.slice(0, 600),
    verification: detail.delivery?.verification?.status ?? null,
  });
  return {
    taskStatus: detail.task.status, elapsedMs, denied, usage: tokens,
    infrastructureIssue: issue ? '服务商额度、限流或授权错误' : unexpectedModel ? '未记录到指定真实模型调用' : null,
  };
}
