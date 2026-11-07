/**
 * 命令日志摘要：原文不动，只输出失败上下文及可回读的行号。
 * 用法：node tools/logdigest.mjs <文件|-> [--context N] [--max-windows N]
 *       [--max-bytes N] [--json]
 * 文本和 JSON（含元数据、转义、结尾换行）默认且至多 16000 UTF-8 字节。
 * 这是保守的输出体积上限，不把字符数冒充模型 token 数。
 * 摘要进程 exit=0 仅表示读取/摘要成功；原命令状态以日志末次独立 exit=N 为准。
 */
import fs from 'node:fs';
import path from 'node:path';
import { stripVTControlCharacters } from 'node:util';

const MAX_BYTES = 16000;
const MAX_LINE_BYTES = 1000;
const MAX_FAILURE_LOCATIONS = 32;
const USAGE = '用法: node tools/logdigest.mjs <日志文件|-> [--context N] [--max-windows N] [--max-bytes 4096..16000] [--json]';

const FAIL_PATTERNS = [
  /^\s*error(?:\[E\d+\])?:\s/,
  /\berror TS\d+:/,
  /\bpanicked at\b/,
  /^\s*failures:\s*$/,
  /^\s*FAILED\b/,
  /^test result: FAILED/i,
  /\bAssertionError\b/,
  /^\s*[✗×✖]\s/,
  /^\s*not ok\s+\d+\b/,
  /^\s*\[FAIL\]\s/,
  /FAIL\s+\S+\.test\./,
  /^\s*npm (?:ERR!|error)(?:\s|$)/,
];
const BENIGN_PATTERNS = [
  /^\s*test .+ \.\.\. ok\s*$/,
  /^\s*[✓√✔]\s/,
  /^\s*ok\s+\d+\b/,
  /^\s*not ok\s+\d+\b.*#\s*(?:SKIP|TODO)\b/i,
  /^\s*PASS\b/,
  /^\s+at\s+.*?:\d+:\d+\)?\s*$/,
  /^\s*(stderr|stdout)\s*\|/,
];

function explicitExit(line) {
  const m = /^\s*exit=(-?\d+)\s*$/.exec(line);
  return m && Number.isSafeInteger(Number(m[1])) ? Number(m[1]) : null;
}

function isFail(line) {
  if (BENIGN_PATTERNS.some((re) => re.test(line))) return false;
  const code = explicitExit(line);
  return (code !== null && code !== 0) || FAIL_PATTERNS.some((re) => re.test(line));
}

function parseArgs(argv) {
  const opts = { file: null, context: 20, maxWindows: 20, maxBytes: MAX_BYTES, json: false };
  const numeric = { '--context': ['context', 0, 10000], '--max-windows': ['maxWindows', 0, 1000], '--max-bytes': ['maxBytes', 4096, MAX_BYTES] };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (Object.hasOwn(numeric, arg)) {
      const [key, min, max] = numeric[arg];
      const value = argv[++i];
      if (!/^\d+$/.test(value ?? '') || Number(value) < min || Number(value) > max) {
        throw new Error(`参数 ${arg} 需要 ${min}..${max} 之间的整数`);
      }
      opts[key] = Number(value);
    } else if (arg === '--json') opts.json = true;
    else if (arg.startsWith('--')) throw new Error(`未知选项: ${arg}`);
    else if (opts.file !== null) throw new Error('参数只允许一个日志文件');
    else opts.file = arg;
  }
  if (!opts.file) throw new Error(USAGE);
  return opts;
}

function decode(buf) {
  let enc = 'utf8';
  let offset = 0;
  if (buf[0] === 0xff && buf[1] === 0xfe) { enc = 'utf16le'; offset = 2; }
  else if (buf[0] === 0xfe && buf[1] === 0xff) { enc = 'utf16be'; offset = 2; }
  else if (buf[0] === 0xef && buf[1] === 0xbb && buf[2] === 0xbf) offset = 3;
  else {
    // 编码猜测与 BOM 长度分开：无 BOM 时绝不能丢掉首字符。
    const sample = buf.subarray(0, 512);
    let zeros = 0;
    for (let i = 1; i < sample.length; i += 2) if (sample[i] === 0) zeros++;
    if (sample.length >= 4 && zeros > sample.length / 4) enc = 'utf16le';
  }
  let body = buf.subarray(offset);
  if (enc === 'utf16be') body = Buffer.from(body.subarray(0, body.length - body.length % 2)).swap16();
  return { text: body.toString(enc === 'utf16be' ? 'utf16le' : enc), enc };
}

function clipLine(text, line) {
  const buf = Buffer.from(text);
  if (buf.length <= MAX_LINE_BYTES) return { text, clipped: false };
  const suffix = ` … [第 ${line} 行过长，已截断；按原文件回读]`;
  let end = MAX_LINE_BYTES - Buffer.byteLength(suffix);
  // 不切断 UTF-8 多字节字符，包括代理对编码的 emoji。
  while ((buf[end] & 0xc0) === 0x80) end--;
  return { text: buf.subarray(0, end).toString('utf8') + suffix, clipped: true };
}

function mergeWindows(hits, context, length) {
  const windows = [];
  for (const i of hits) {
    const start = Math.max(0, i - context);
    const end = Math.min(length - 1, i + context);
    const last = windows.at(-1);
    if (last && start <= last.end + 1) last.end = Math.max(last.end, end);
    else windows.push({ start, end });
  }
  return windows;
}

function exitInfo(lines) {
  let code = null;
  let line = null;
  const hints = new Set();
  for (let i = 0; i < lines.length; i++) {
    const value = explicitExit(lines[i]);
    if (value !== null) { code = value; line = i + 1; }
    if (/^\s*(?:test result: FAILED|error: test failed)/i.test(lines[i])) hints.add('非 0（测试失败措辞；不推定具体退出码）');
    if (/^\s*npm (?:ERR!|error)(?:\s|$)/.test(lines[i])) hints.add('非 0（npm 失败措辞）');
    const m = /(?:\[FAIL\].*\bexit=|\bexited with code |\bfailed\b.*exit code )(-?\d+)\b/i.exec(lines[i]);
    if (m && hints.size < 6) hints.add(`${m[1]}（子命令线索，第 ${i + 1} 行；不代表最终状态）`);
  }
  return {
    exit_code: code,
    exit_code_line: line,
    command_status: code === null ? 'unknown' : code === 0 ? 'success' : 'failed',
    exit_hints: [...(code === null ? [] : [`${code}（末次显式 exit=，第 ${line} 行）`]), ...hints],
  };
}

function render(s) {
  const out = [
    '# 日志摘要',
    `路径: ${s.path}`,
    `规模: ${s.bytes} 字节 / ${s.lines} 行（编码 ${s.encoding}）`,
    `失败信号: ${s.fail_hits} 处，展开 ${s.windows} 个窗口（目标各 ±${s.context} 行）`,
    `原命令退出码: ${s.exit_code ?? '未知'}（摘要进程成功不代表原命令成功）`,
    `输出: ${s.output_bytes}/${s.output_budget_bytes} 字节；保留 ${s.kept_lines} 行 / ${s.kept_chars} 字符（正文减少 ${s.reduction_pct}%）`,
  ];
  if (s.not_worth_digest) out.push('小日志可直接回读；下列内容仍受总输出预算限制。');
  if (s.first_fail_line_pct !== null) out.push(`首个失败位于全文 ${s.first_fail_line_pct}% 处`);
  if (s.failure_lines.length) out.push(`失败行号: ${s.failure_lines.join(', ')}${s.failure_lines_not_listed ? `；另有 ${s.failure_lines_not_listed} 处未列出` : ''}`);
  if (s.fail_hits_not_shown) out.push(`未展示 ${s.fail_hits_not_shown} 处失败，首处第 ${s.first_omitted_fail_line} 行；请按原文件回读。`);
  if (s.windows_not_expanded || s.windows_partially_expanded) out.push(`窗口未展开 ${s.windows_not_expanded} 个，部分展开 ${s.windows_partially_expanded} 个。`);
  if (s.budget_truncated || s.truncated_lines) out.push(`预算截断: ${s.budget_truncated ? '是' : '否'}；长行截断 ${s.truncated_lines} 行，完整证据留在原文件。`);
  if (s.exit_hints.length) out.push('退出码线索:', ...s.exit_hints.map((h) => `  - ${h}`));
  if (s.body.length) out.push('', '## 失败上下文', ...s.body);
  else if (s.fallback) {
    out.push('', '## 未发现失败信号（不代表命令成功，仅头尾各至多 10 行）');
    if (s.fallback.head.length) out.push('--- 第 1 行起 ---', ...s.fallback.head);
    if (s.fallback.tail.length) out.push(`--- 第 ${s.fallback_tail_start} 行起 ---`, ...s.fallback.tail);
  }
  return out.join('\n');
}

function serialize(s, json) {
  // output_bytes 本身也占空间，迭代到位数稳定；结尾换行同样计入预算。
  for (;;) {
    const out = (json ? JSON.stringify(s, null, 1) : render(s)) + '\n';
    const size = Buffer.byteLength(out);
    if (size === s.output_bytes) return out;
    s.output_bytes = size;
  }
}

// 下方统一装配与限流：任何输出分支都不能绕过 serialize 后的字节检查。
function summarize(rawText, opts, meta) {
  // 检测和展示使用同一份去 ANSI 文本，行号仍对应原文件；绝不改写原文。
  const lines = rawText.split(/\r?\n/).map(stripVTControlCharacters);
  const hits = [];
  for (let i = 0; i < lines.length; i++) if (isFail(lines[i])) hits.push(i);
  const hitSet = new Set(hits);
  const allWindows = mergeWindows(hits, opts.context, lines.length);
  const selected = allWindows.slice(0, opts.maxWindows);
  const tailStart = Math.max(10, lines.length - 10);
  const candidateCount = hits.length
    ? selected.reduce((n, w) => n + w.end - w.start + 1, 0)
    : Math.min(10, lines.length) + Math.max(0, lines.length - tailStart);
  const exit = exitInfo(lines);

  function* candidates() {
    if (hits.length) {
      for (let window = 0; window < selected.length; window++) {
        const w = selected[window];
        for (let i = w.start; i <= w.end; i++) yield { i, window };
      }
    } else {
      for (let i = 0; i < Math.min(10, lines.length); i++) yield { i, zone: 'head' };
      for (let i = tailStart; i < lines.length; i++) yield { i, zone: 'tail' };
    }
  }

  function assemble(rows) {
    const shown = new Set(rows.map((r) => r.i));
    const firstMissing = hits.find((i) => !shown.has(i));
    const ranges = new Map();
    const body = [];
    let previous = -2;
    for (const row of rows) {
      if (!hits.length) continue;
      if (row.i > previous + 1) body.push(`--- 第 ${row.i + 1} 行起 ---`);
      body.push(row.text);
      previous = row.i;
      const range = ranges.get(row.window);
      if (range) range[1] = row.i + 1;
      else ranges.set(row.window, [row.i + 1, row.i + 1]);
    }
    const fallback = hits.length ? null : {
      head: rows.filter((r) => r.zone === 'head').map((r) => r.text),
      tail: rows.filter((r) => r.zone === 'tail').map((r) => r.text),
    };
    const keptChars = (fallback ? [...fallback.head, ...fallback.tail] : body).join('\n').length;
    return {
      path: meta.path,
      encoding: meta.enc,
      bytes: meta.bytes,
      lines: lines.length,
      context: opts.context,
      fail_hits: hits.length,
      fail_hits_not_shown: hits.length - rows.filter((r) => hitSet.has(r.i)).length,
      first_omitted_fail_line: firstMissing === undefined ? null : firstMissing + 1,
      failure_lines: hits.slice(0, MAX_FAILURE_LOCATIONS).map((i) => i + 1),
      failure_lines_not_listed: Math.max(0, hits.length - MAX_FAILURE_LOCATIONS),
      windows: ranges.size,
      windows_not_expanded: allWindows.length - ranges.size,
      windows_partially_expanded: [...ranges].filter(([idx, range]) => range[1] < selected[idx].end + 1).length,
      window_ranges: [...ranges.values()],
      kept_lines: rows.length,
      kept_chars: keptChars,
      reduction_pct: +(100 * (1 - keptChars / Math.max(1, rawText.length))).toFixed(1),
      first_fail_line_pct: hits.length ? +(100 * hits[0] / lines.length).toFixed(1) : null,
      ...exit,
      not_worth_digest: rawText.length < 8000,
      budget_truncated: rows.length < candidateCount,
      truncated_lines: rows.filter((r) => r.clipped).length,
      output_budget_bytes: opts.maxBytes,
      output_bytes: 0,
      body,
      fallback,
      fallback_tail_start: fallback?.tail.length ? tailStart + 1 : null,
    };
  }

  const rows = [];
  let output = serialize(assemble(rows), opts.json);
  if (Buffer.byteLength(output) > opts.maxBytes) throw new Error('日志路径或元数据超出摘要预算，请改用较短路径');
  for (const candidate of candidates()) {
    rows.push({ ...candidate, ...clipLine(lines[candidate.i], candidate.i + 1) });
    const next = serialize(assemble(rows), opts.json);
    if (Buffer.byteLength(next) > opts.maxBytes) break;
    output = next;
  }
  return output;
}

try {
  const opts = parseArgs(process.argv.slice(2));
  const absolute = opts.file === '-' ? '<stdin>' : path.resolve(opts.file);
  const buf = fs.readFileSync(opts.file === '-' ? 0 : absolute);
  const { text, enc } = decode(buf);
  process.stdout.write(summarize(text, opts, { path: absolute, enc, bytes: buf.length }));
} catch (error) {
  console.error(`日志摘要失败: ${error.message}`);
  process.exitCode = 2;
}
