/**
 * tools/logdigest.test.mjs — logdigest 的行为守护
 *
 * 守护：失败识别与通过行去噪、窗口合并、显式退出码、编码首字符，
 * 以及文本/JSON/小日志/头尾回退共享的总输出预算和截断回读位置。
 */

import test, { after } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SCRIPT = path.join(ROOT, 'tools', 'logdigest.mjs');
const TEMP = fs.mkdtempSync(path.join(os.tmpdir(), 'logdigest-test-'));
let nextLog = 0;
after(() => fs.rmSync(TEMP, { recursive: true, force: true }));

function run(args, input) {
  return execFileSync(process.execPath, [SCRIPT, ...args], {
    encoding: 'utf8',
    input,
    maxBuffer: 64 * 1024 * 1024,
  });
}

function tmpLog(content) {
  const file = path.join(TEMP, `sample-${nextLog++}.log`);
  fs.writeFileSync(file, content, 'utf8');
  return file;
}

test('通过的测试行不被判为失败', () => {
  const content = [
    'running 4 tests',
    'test tools::x::tests::handles_failed_request ... ok',
    'test tools::y::tests::error_path_is_audited ... ok',
    'test result: ok. 4 passed; 0 failed; 0 ignored',
  ].join('\n');
  const out = run(['--json', tmpLog(content)]);
  const j = JSON.parse(out);
  assert.equal(j.fail_hits, 0, `不应命中任何失败信号，实际 ${j.fail_hits}`);
  assert.equal(j.windows, 0);
});

test('被测代码故意抛错时的堆栈不算失败', () => {
  // 真实场景：vitest 测「加载失败时置 error」这类用例，会打印 stderr 与完整堆栈，
  // 但整轮测试是绿的。按裸 `at file:line:col` 或 `ERR_*` 匹配会误报上百处。
  const content = [
    'stderr | src/composables/useTaskTrace.test.ts > loadTaskTrace > 加载失败时置 error',
    'Error: boom',
    '    at apiToken (D:/proj/web/src/api/client.ts:18:20)',
    '    at authorizedFetch (D:/proj/web/src/api/client.ts:46:49)',
    '    at ERR_INVALID_ARG (D:/proj/web/src/api/client.ts:60:12)',
    '',
    ' Test Files  187 passed (187)',
    '      Tests  2065 passed (2065)',
  ].join('\n');
  const j = JSON.parse(run(['--json', tmpLog(content)]));
  assert.equal(j.fail_hits, 0, `全绿测试的故意堆栈不应触发失败信号，实际 ${j.fail_hits}`);
});

test('失败证据完整保留：测试名、源码位置、结果行', () => {
  const padding = Array.from({ length: 400 }, (_, i) => `test filler::case_${i} ... ok`);
  const content = [
    'running 402 tests',
    ...padding,
    'failures:',
    '',
    '---- services::demo::tests::keeps_order stdout ----',
    "thread 'services::demo::tests::keeps_order' (123) panicked at src\\services\\demo.rs:301:9:",
    'assertion failed: left == right',
    '',
    'failures:',
    '    services::demo::tests::keeps_order',
    '',
    'test result: FAILED. 401 passed; 1 failed; 2 ignored; finished in 1.00s',
    'error: test failed, to rerun pass `--lib`',
  ].join('\n');

  const out = run(['--json', tmpLog(content)]);
  const j = JSON.parse(out);

  assert.ok(j.fail_hits > 0, '应命中失败信号');
  assert.ok(j.reduction_pct > 80, `应显著压缩，实际 ${j.reduction_pct}%`);

  const body = j.body.join('\n');
  assert.match(body, /services::demo::tests::keeps_order/, '测试名必须保留');
  assert.match(body, /src\\services\\demo\.rs:301:9/, '源码位置必须保留');
  assert.match(body, /test result: FAILED/, '结果行必须保留');
  assert.ok(j.exit_hints.length > 0, '应给出退出码线索');
});

test('首个失败位置以全文百分比给出', () => {
  const padding = Array.from({ length: 90 }, (_, i) => `line ${i}`);
  const content = [...padding, 'error[E0308]: mismatched types', ...Array.from({ length: 9 }, (_, i) => `tail ${i}`)].join('\n');
  const j = JSON.parse(run(['--json', tmpLog(content)]));
  assert.ok(j.first_fail_line_pct !== null);
  assert.ok(j.first_fail_line_pct >= 85, `首个失败应在后段，实际 ${j.first_fail_line_pct}%`);
});

test('小文件不走摘要路径', () => {
  const content = 'error: tiny failure\ncontext line\n';
  const j = JSON.parse(run(['--json', tmpLog(content)]));
  assert.equal(j.not_worth_digest, true, '小于 8000 字符时应标记为不值得摘要');
});

test('无失败信号的大日志给出头尾而不是整篇', () => {
  const lines = Array.from({ length: 500 }, (_, i) => `filler line ${i} with some content`);
  const j = JSON.parse(run(['--json', tmpLog(lines.join('\n'))]));
  assert.equal(j.fail_hits, 0);
  assert.ok(j.fallback, '应提供头尾回退');
  assert.ok(j.fallback.head.length <= 10);
  assert.ok(j.fallback.tail.length <= 10);
});

test('UTF-16LE 带 BOM 的日志能被正确读取', () => {
  const text = ['running 1 test', 'failures:', 'thread \'x::y\' panicked at src\\a.rs:1:1:', 'test result: FAILED. 0 passed; 1 failed'].join('\r\n');
  const file = tmpLog(Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(text, 'utf16le')]));

  const j = JSON.parse(run(['--json', file]));
  assert.equal(j.encoding, 'utf16le');
  assert.ok(j.fail_hits > 0, 'UTF-16 日志里的失败应被识别');
});

test('从 stdin 读取', () => {
  const content = ['a'.repeat(9000), 'panicked at src\\z.rs:9:9:', 'tail'].join('\n');
  const j = JSON.parse(run(['--json', '-'], content));
  assert.equal(j.path, '<stdin>');
  assert.ok(j.fail_hits > 0);
});

const padding = (n) => Array.from({ length: n }, (_, i) => `progress ${i} ${'normal '.repeat(12)}`);
const middleLog = (signal) => [...padding(100), signal, ...padding(100)].join('\n');

for (const [label, signal] of [
  ['TypeScript', 'src/demo.ts(1,7): error TS2322: Type number is not assignable to string.'],
  ['TypeScript 彩色格式', 'src/demo.ts:1:7 - error TS2322: 类型不匹配'],
  ['Node spec', '✖ deliberately failing test (1.2ms)'],
  ['TAP', '    not ok 2 - deliberately failing test'],
  ['本仓门禁', '[FAIL] web: vue-tsc (exit=2, 耗时 3.1s)'],
  ['新版 npm', 'npm error code ELIFECYCLE'],
  ['ANSI Rust', '\u001b[1m\u001b[91merror[E0308]\u001b[0m: mismatched types'],
]) {
  test(`识别日志中段的 ${label} 失败`, () => {
    const file = tmpLog(middleLog(signal));
    const j = JSON.parse(run([file, '--json']));
    assert.equal(j.fail_hits, 1);
    assert.ok(j.body.join('\n').includes(signal.replace(/\u001b\[[\d;]*m/g, '')));
    assert.doesNotMatch(run([file]), /未发现失败信号/);
  });
}

test('Node 测试运行器的实际失败输出可以被摘要', () => {
  const fixture = path.join(TEMP, 'deliberate-failure.cjs');
  fs.writeFileSync(fixture, "require('node:test')('真实运行器失败夹具', () => { throw new Error('synthetic failure'); });\n");
  // 子进程要作为独立运行器启动，不能继承父级的测试工作进程标记。
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  for (const reporter of ['tap', 'spec']) {
    const r = spawnSync(process.execPath, ['--test', `--test-reporter=${reporter}`, fixture], { encoding: 'utf8', env });
    assert.equal(r.status, 1);
    const j = JSON.parse(run(['--json', tmpLog(middleLog(r.stdout + r.stderr))]));
    assert.ok(j.fail_hits > 0, `${reporter} 应识别失败`);
  }
});

test('通过行即使带颜色、Node 勾号和失败关键词也不误报', () => {
  const content = ['\u001b[32mtest error_path ... ok\u001b[0m', '✔ npm error handling works (1ms)', 'ok 1 - AssertionError is handled', 'not ok 2 - skipped case # SKIP fixture'];
  const j = JSON.parse(run(['--json', tmpLog(content.join('\n'))]));
  assert.equal(j.fail_hits, 0);
});

test('重叠窗口必须合并并延长右边界，不能吞掉后一个错误', () => {
  const lines = padding(240);
  lines[99] = 'error: EARLY_UNIQUE_SIGNAL';
  lines[124] = 'error: LATER_UNIQUE_SIGNAL';
  const j = JSON.parse(run(['--json', '--max-windows', '1', tmpLog(lines.join('\n'))]));
  assert.equal(j.fail_hits, 2);
  assert.deepEqual(j.window_ranges, [[80, 145]]);
  assert.equal(j.windows_not_expanded, 0);
  assert.match(j.body.join('\n'), /LATER_UNIQUE_SIGNAL/);
});

test('窗口上限后继续扫描，给出未展开失败的行号与数量', () => {
  const lines = padding(240);
  lines[20] = 'error: first';
  lines[120] = 'error: second';
  lines[220] = 'error: third';
  const j = JSON.parse(run(['--json', '--context', '0', '--max-windows', '1', tmpLog(lines.join('\n'))]));
  assert.deepEqual(j.window_ranges, [[21, 21]]);
  assert.equal(j.windows_not_expanded, 2);
  assert.equal(j.fail_hits_not_shown, 2);
  assert.equal(j.first_omitted_fail_line, 121);
});

test('显式最终退出码优先于推测，摘要成功与命令失败分开', () => {
  const file = tmpLog('test result: FAILED. 0 passed; 1 failed\nexit=7\n');
  const r = spawnSync(process.execPath, [SCRIPT, file, '--json'], { encoding: 'utf8' });
  assert.equal(r.status, 0, '摘要工具本身成功');
  const j = JSON.parse(r.stdout);
  assert.equal(j.exit_code, 7);
  assert.equal(j.command_status, 'failed');
  assert.match(j.exit_hints[0], /7/);
  assert.doesNotMatch(j.exit_hints.join('\n'), /^1（cargo/m, '不能把 cargo 的实际退出码编成 1');
  assert.match(run([file]), /7/);
  const success = JSON.parse(run(['--json', tmpLog('exit=7\nretry succeeded\nexit=0\n')]));
  assert.equal(success.exit_code, 0);
  assert.equal(success.command_status, 'success');
  const unknown = JSON.parse(run(['--json', tmpLog('progress only\n')]));
  assert.equal(unknown.exit_code, null);
  assert.equal(unknown.command_status, 'unknown');
});

test('无其他诊断的非零退出码也不能报告未发现失败', () => {
  const file = tmpLog(middleLog('exit=2'));
  const j = JSON.parse(run(['--json', file]));
  assert.equal(j.fail_hits, 1);
  assert.equal(j.exit_code, 2);
  assert.doesNotMatch(run([file]), /未发现失败信号/);
});

for (const [label, encode, expected] of [
  ['UTF-16LE 无 BOM', (s) => Buffer.from(s, 'utf16le'), 'utf16le'],
  ['UTF-8 有 BOM', (s) => Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), Buffer.from(s)]), 'utf8'],
  ['UTF-16BE 有 BOM', (s) => Buffer.concat([Buffer.from([0xfe, 0xff]), Buffer.from(s, 'utf16le').swap16()]), 'utf16be'],
]) {
  test(`${label} 保留首个错误的第一个字符，文件与 stdin 一致`, () => {
    const data = encode('error: keep first character\r\n' + padding(100).join('\r\n'));
    for (const [args, input] of [[[tmpLog(data), '--json'], undefined], [['-', '--json'], data]]) {
      const j = JSON.parse(run(args, input));
      assert.equal(j.encoding, expected);
      assert.equal(j.fail_hits, 1);
      assert.ok(j.body.includes('error: keep first character'));
    }
  });
}

for (const [label, content] of [
  ['超长失败单行', 'error: ' + '诊断😀'.repeat(25000)],
  ['超长无失败单行', '正常😀'.repeat(25000)],
  ['多窗口与多字节', Array.from({ length: 5000 }, (_, i) => i % 55 === 0 ? `error: failure ${i}` : '上下文😀'.repeat(30)).join('\n')],
  ['JSON 转义膨胀', 'error: ' + '\u0001\\"'.repeat(40000)],
]) {
  test(`${label}：文本和 JSON 都服从包括元数据与换行的总预算`, () => {
    const file = tmpLog(content);
    for (const budget of [4096, 16000]) {
      for (const json of [false, true]) {
        const out = run([file, '--max-bytes', String(budget), ...(json ? ['--json'] : [])]);
        assert.ok(Buffer.byteLength(out) <= budget, `输出 ${Buffer.byteLength(out)} 字节，预算 ${budget}`);
        assert.doesNotMatch(out, /\uFFFD/, 'UTF-8 截断不能切坏字符');
        if (json) {
          const j = JSON.parse(out);
          assert.equal(j.output_bytes, Buffer.byteLength(out));
          assert.ok(j.budget_truncated || j.truncated_lines > 0);
          assert.ok(j.kept_chars > 0, '头尾回退也要计入实际保留量');
          assert.ok(j.reduction_pct < 100);
          assert.equal(j.path, path.resolve(file));
        } else {
          assert.ok(out.includes(path.resolve(file)));
          assert.match(out, /截断|预算|回读/);
        }
      }
    }
  });
}

test('预算截断后准确报告仍未显示的失败位置', () => {
  const lines = Array.from({ length: 600 }, (_, i) => `error: failure_${i} ${'长文本'.repeat(200)}`);
  const j = JSON.parse(run(['--json', '--max-bytes', '4096', tmpLog(lines.join('\n'))]));
  assert.equal(j.fail_hits, 600);
  assert.equal(j.budget_truncated, true);
  assert.ok(j.fail_hits_not_shown > 0);
  assert.ok(j.first_omitted_fail_line > 1);
  assert.ok(j.failure_lines_not_listed > 0);
});

test('小日志同样显示退出码、失败数量，回退内容计入压缩率', () => {
  const file = tmpLog('exit=9\n');
  assert.match(run([file]), /失败信号: 1/);
  assert.match(run([file]), /退出码.*9/);
  const j = JSON.parse(run(['--json', tmpLog(middleLog('normal progress'))]));
  const payload = [...j.fallback.head, ...j.fallback.tail].join('\n');
  assert.equal(j.kept_chars, payload.length);
  assert.ok(j.reduction_pct < 100);
});

test('参数校验拒绝负数、缺值、非整数和未知选项', () => {
  for (const args of [['--context', '-1'], ['--context', '1junk'], ['--context'], ['--max-windows', '-1'], ['--max-bytes', '0'], ['--max-bytes', '999999999'], ['--unknown']]) {
    const r = spawnSync(process.execPath, [SCRIPT, tmpLog('progress'), ...args], { encoding: 'utf8' });
    assert.equal(r.status, 2, JSON.stringify(args));
    assert.match(r.stderr, /参数|选项|用法/);
  }
});

test('不传预算选项也限流，零窗口仍保留失败定位', () => {
  const file = tmpLog('error: ' + '诊断'.repeat(100000));
  for (const args of [[], ['--json']]) assert.ok(Buffer.byteLength(run([file, ...args])) <= 16000);
  const j = JSON.parse(run([file, '--json', '--max-windows', '0']));
  assert.equal(j.windows, 0);
  assert.equal(j.fail_hits, 1);
  assert.equal(j.fail_hits_not_shown, 1);
  assert.equal(j.first_omitted_fail_line, 1);
});

test('AGENTS 的 PowerShell 示例实跑保留原生命令非零退出码', { skip: process.platform !== 'win32' }, () => {
  const doc = fs.readFileSync(path.join(ROOT, 'AGENTS.md'), 'utf8');
  const section = doc.split('## 长输出处理')[1]?.split('\n## ')[0];
  const example = /```powershell\s*\n([\s\S]*?)```/.exec(section ?? '')?.[1];
  assert.ok(example, '必须有可执行的 PowerShell 日志示例');
  const psQuote = (s) => `'${s.replaceAll("'", "''")}'`;
  const file = tmpLog('');
  const command = example
    .replace(/\$log = .*/, `$log = ${psQuote(file)}`)
    .replace('node tools/check-arch.mjs', `& ${psQuote(process.execPath)} -e 'process.stdout.write(String.fromCharCode(120));process.exit(7)'`)
    .replace('npm run log:digest -- $log', `& ${psQuote(process.execPath)} ${psQuote(SCRIPT)} $log --json`);
  const r = spawnSync('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', command + '\nexit $code'], { encoding: 'utf8' });
  assert.equal(r.status, 7, r.stderr);
  const j = JSON.parse(r.stdout);
  assert.equal(j.exit_code, 7);
  assert.equal(j.command_status, 'failed');
  assert.ok(j.body.some((line) => line === 'exit=7'));
});
