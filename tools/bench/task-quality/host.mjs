import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { lstat, readFile, realpath, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

// 不让真实模型凭据、代理变量或 NODE_OPTIONS 进入任务和受测产物的子进程。
const environment = /^(path|pathext|comspec|systemroot|systemdrive|windir|temp|tmp|tmpdir|home|userprofile|localappdata|appdata|lang)$/i;
export const hostEnv = Object.fromEntries(Object.entries(process.env).filter(([name]) => environment.test(name)));
export const readJson = async (file) => JSON.parse((await readFile(file, 'utf8')).replace(/^\uFEFF/, ''));
export const writeJson = (file, value) => writeFile(file, JSON.stringify(value, null, 2) + '\n', { flag: 'wx' });

export async function assertOutsideGit(directory) {
  // .git 可能是目录，也可能是链接 worktree 的指针文件。只比当前 checkout 会漏掉兄弟工作区。
  const root = await realpath(directory);
  for (let at = root; ; at = dirname(at)) {
    try {
      await lstat(join(at, '.git'));
      throw new Error('评测证据不能写入 Git 工作区');
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
    if (dirname(at) === at) break;
  }
  return root;
}

export async function until(check, ms, description) {
  const end = Date.now() + ms;
  do {
    const value = await check();
    if (value) return value;
    await delay(100);
  } while (Date.now() < end);
  throw new Error(`等待超时：${description}`);
}

export function launch(file, args, options) {
  const child = spawn(file, args, { env: hostEnv, windowsHide: true, ...options });
  const closed = new Promise((resolve) => child.once('close', (code, signal) => resolve({ code, signal })));
  const started = once(child, 'spawn');
  return {
    child, started, closed,
    async stop() {
      if (child.exitCode === null && child.signalCode === null) child.kill();
      const stopped = await Promise.race([closed, delay(3000, null, { ref: false })]);
      if (!stopped) throw new Error(`子进程停止超时：${file}`);
    },
  };
}
