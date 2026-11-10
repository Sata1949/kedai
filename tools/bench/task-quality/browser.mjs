import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { readFile, realpath, stat, writeFile } from 'node:fs/promises';
import { dirname, extname, isAbsolute, join, relative, sep } from 'node:path';
import { launch, until } from './host.mjs';

const local = '127.0.0.1';
const types = {
  '.html': 'text/html; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.png': 'image/png', '.svg': 'image/svg+xml', '.json': 'application/json',
};
const within = (root, file) => {
  const rel = relative(root, file);
  return rel !== '..' && !rel.startsWith('..' + sep) && !isAbsolute(rel);
};
export class BrowserUnavailable extends Error {}

async function serve(workspace) {
  const root = await realpath(workspace);
  const server = createServer(async (req, res) => {
    try {
      const request = new URL(req.url, `http://${local}`);
      if (request.pathname === '/favicon.ico') { res.writeHead(204).end(); return; }
      const name = request.pathname === '/' ? 'index.html' : decodeURIComponent(request.pathname.slice(1));
      const path = join(root, name);
      if (req.method !== 'GET' || !within(root, path)) { res.writeHead(403).end(); return; }
      const file = await realpath(path);
      const type = types[extname(file)];
      if (!within(root, file) || !type || !(await stat(file)).isFile()) { res.writeHead(404).end(); return; }
      // 只提供本次产物，不让示例的 CDN、表单或 fetch 成为对照中的额外变量。
      res.writeHead(200, {
        'Content-Type': type,
        'Content-Security-Policy': "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; form-action 'self'; base-uri 'none'",
        'Cache-Control': 'no-store',
      });
      createReadStream(file).on('error', () => res.destroy()).pipe(res);
    } catch (error) {
      res.writeHead(error.code === 'ENOENT' ? 404 : 400).end();
    }
  });
  await new Promise((resolve, reject) => server.once('error', reject).listen(0, local, resolve));
  return {
    url: `http://${local}:${server.address().port}/`,
    close: () => new Promise((resolve) => {
      server.close(resolve);
      server.closeAllConnections();
    }),
  };
}

async function cdp(url, observe) {
  const ws = new WebSocket(url);
  const pending = new Map();
  let seq = 0;
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.close(); reject(new BrowserUnavailable('CDP 握手超时')); }, 4000);
    ws.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
    ws.addEventListener('error', () => { clearTimeout(timer); reject(new BrowserUnavailable('CDP 连接失败')); }, { once: true });
  });
  ws.addEventListener('message', (message) => {
    const data = JSON.parse(message.data);
    if (!data.id) { observe(data); return; }
    const reply = pending.get(data.id);
    if (reply) reply(data);
  });
  function command(method, params = {}) {
    return new Promise((resolve, reject) => {
      if (ws.readyState !== WebSocket.OPEN) { reject(new BrowserUnavailable('CDP 已断开')); return; }
      const id = ++seq;
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP 超时：${method}`)); }, 6000);
      pending.set(id, ({ result, error }) => {
        clearTimeout(timer);
        pending.delete(id);
        if (error) reject(new Error(`${method}：${error.message}`));
        else resolve(result);
      });
      ws.send(JSON.stringify({ id, method, params }));
    });
  }
  return { command, close: () => ws.close() };
}

export async function startBrowser(executable, profile) {
  const run = launch(executable, [
    '--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
    '--disable-sync', '--disable-extensions', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank',
  ], { cwd: dirname(profile), stdio: 'ignore' });
  try {
    await run.started;
    const port = await until(async () => {
      try { return Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split(/\r?\n/)[0]); }
      catch (error) { if (error.code === 'ENOENT') return null; throw error; }
    }, 10_000, '隔离 Chromium DevTools');
    const endpoint = `http://${local}:${port}`;
    const json = async (path, init) => {
      try {
        const response = await fetch(endpoint + path, { ...init, signal: AbortSignal.timeout(4000) });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        return await response.json();
      } catch (error) {
        throw new BrowserUnavailable(`浏览器控制接口不可用：${error.message}`);
      }
    };
    const browser = await json('/json/version');
    const control = await cdp(browser.webSocketDebuggerUrl, () => {});
    return {
      version: browser.Browser,
      async withPage(workspace, exercise) {
        const origin = await serve(workspace);
        let tab;
        let client;
        try {
          tab = await json('/json/new?about:blank', { method: 'PUT' });
          const faults = [];
          client = await cdp(tab.webSocketDebuggerUrl, ({ method, params }) => {
            if (method === 'Runtime.exceptionThrown') faults.push('页面运行异常');
            if (method === 'Network.loadingFailed' && !params.canceled) faults.push('资源加载失败');
            if (method === 'Network.responseReceived' && params.response.status >= 400) {
              faults.push(`资源 HTTP ${params.response.status}`);
            }
          });
          const send = client.command;
          for (const domain of ['Page', 'Runtime', 'Network']) await send(`${domain}.enable`);
          const evaluate = async (expression) => {
            const res = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
            if (res.exceptionDetails) throw new Error(`浏览器验收表达式异常：${res.exceptionDetails.text}`);
            return res.result.value;
          };
          const viewport = (width, height) => send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
          await viewport(1024, 768);
          const nav = await send('Page.navigate', { url: origin.url });
          if (nav.errorText) throw new Error(`页面导航失败：${nav.errorText}`);
          await until(() => evaluate('document.readyState === "complete"'), 6000, '加载产物页面');
          const click = async (selector, index = 0) => {
            const point = await evaluate(`(() => {
              const el = document.querySelectorAll(${JSON.stringify(selector)})[${index}];
              if (!el) return null;
              const r = el.getBoundingClientRect();
              const x = r.left + r.width / 2, y = r.top + r.height / 2;
              return el.contains(document.elementFromPoint(x, y)) ? {x, y} : null;
            })()`);
            if (!point) return false;
            for (const type of ['mousePressed', 'mouseReleased']) {
              await send('Input.dispatchMouseEvent', { ...point, type, button: 'left', clickCount: 1 });
            }
            return true;
          };
          const screenshot = async (file) => {
            const { data } = await send('Page.captureScreenshot', { format: 'png' });
            await writeFile(file, Buffer.from(data, 'base64'), { flag: 'wx' });
          };
          const reload = async () => {
            const previous = await evaluate('performance.timeOrigin');
            await send('Page.reload', { ignoreCache: true });
            await until(async () => {
              try { return await evaluate(`performance.timeOrigin !== ${previous} && document.readyState === "complete"`); }
              catch (error) { if (/context|navigat/i.test(error.message)) return false; throw error; }
            }, 6000, '刷新产物页面');
          };
          return await exercise({
            evaluate, click, viewport, screenshot, reload, faults,
            type: (text) => send('Input.insertText', { text }),
          });
        } finally {
          client?.close();
          try {
            if (tab) await fetch(`${endpoint}/json/close/${tab.id}`, { signal: AbortSignal.timeout(3000) });
          } finally {
            await origin.close();
          }
        }
      },
      async close() {
        try { await control.command('Browser.close'); }
        finally { control.close(); await run.stop(); }
      },
    };
  } catch (error) {
    await run.stop();
    throw error;
  }
}
