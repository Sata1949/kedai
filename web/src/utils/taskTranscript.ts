// 任务执行轨迹的三档显示纯函数(TMSHOW-2,2026-11-01)。
//
// 为什么单独成模块:三档渲染(TaskTranscript.vue)要在**不挂 DOM**的前提下被单测覆盖,
// 而它的核心只有三件事——排序去重、分类计数、按归属分组。把这三件事抽成纯函数后,
// 组件只负责把结果铺成模板,档位切换的正确性可以完全在 node 环境下钉住。
//
// 数据源是 TMSHOW-1 的 `task_trace`(经 `GET /api/tasks/{id}/trace` 与 SSE `kind=trace`
// 两条通道进来,两路会重叠,故**去重是必须的而不是保险**)。

import { phaseText, type StepNameAt } from './phaseLabel';
import type { TaskTrace } from '../api/types';
/**
 * 三档显示模式。
 *
 * - `legacy`:优化后的旧方式——计划步骤时间线(状态色带、长结果折叠),**不显示中间过程**;
 * - `full`:扁平全消息时间线——每条思考 / 工具调用 / 工具结果 / 步骤产出独立成条,按 `seq` 排序;
 * - `condensed`:按 `phase + step_index` 分组,每组一张卡,中间过程折叠其内,右下角显示分类计数。
 *
 * 档位的**持久化**分两级(TMSHOW-3):设置页写默认档(`uiPrefs.taskTranscriptMode`,
 * localStorage 全局键),任务详情顶栏写单任务覆盖(每任务一个键),未覆盖的任务跟随默认。
 * 两级合成与降级判定的唯一入口是 `resolveTranscriptMode`——两个消费组件必须同式。
 */
export type TranscriptMode = 'legacy' | 'full' | 'condensed';

/** 三档的展示顺序(分段控件的渲染顺序,也是文档表格的顺序) */
export const TRANSCRIPT_MODES: readonly TranscriptMode[] = ['legacy', 'full', 'condensed'];

/**
 * 分段控件的文案。
 *
 * `satisfies Record<TranscriptMode, string>` 让新增档位**编译失败**——
 * 漏一个文案就是一枚空按钮,那是运行时才看得见、测试常漏的缺陷。
 */
export const TRANSCRIPT_MODE_LABELS = {
  legacy: '步骤',
  full: '全消息',
  condensed: '压缩',
} satisfies Record<TranscriptMode, string>;

/** 档位中文全称(说明文案与无障碍标签用,与上面的短标签分开) */
export const TRANSCRIPT_MODE_TITLES = {
  legacy: '步骤视图:只看计划步骤,不显示中间过程',
  full: '全消息:思考 / 工具调用 / 工具结果逐条按执行顺序列出',
  condensed: '压缩:按阶段分组折叠,右下角显示分类计数',
} satisfies Record<TranscriptMode, string>;

/** 轨迹条目分类的中文标签(与后端 `TaskTraceKind` 一一对应) */
export const TRACE_KIND_LABELS = {
  reasoning: '思考',
  tool_call: '工具调用',
  tool_result: '工具结果',
  step_result: '步骤产出',
} satisfies Record<TaskTrace['kind'], string>;

/**
 * 用户**选择的**档位(单任务覆盖优先,未覆盖跟随默认)。
 *
 * 与 `resolveTranscriptMode` 的分工:本函数只回答「用户要哪一档」,不含降级。
 * 分段控件的高亮、设置页的回显都用它;实际渲染哪一档用 `resolveTranscriptMode`。
 * 两者分开正是为了守住「降级不改用户选择」这条不变量。
 */
export function requestedTranscriptMode(
  override: TranscriptMode | null | undefined,
  fallback: TranscriptMode,
): TranscriptMode {
  return override ?? fallback;
}

/**
 * 合成生效档位与降级判定(TMSHOW-3;`TaskTranscript` 与 `TaskExecutionPanel` 共用)。
 *
 * 输入三项:
 * - `override`:该任务的单任务覆盖(`null` = 未覆盖,跟随默认);
 * - `fallback`:设置页的默认档;
 * - `hasTrace`:当前任务是否有可展示的轨迹。
 *
 * 两条规则,顺序不可换:
 * 1. **未覆盖跟随默认**:`override` 为 null 时用 `fallback` 补位。
 * 2. **降级**:选了档 2/档 3 但轨迹为空(旧任务,或本次运行没有工具循环)时返回 `legacy`。
 *    不降级会让用户看到一片空白时间线,那会被读成「加载失败」。
 *
 * **降级不改用户选择**:本函数只算「实际渲染哪一档」,用户选的档位仍由调用方经
 * `requestedTranscriptMode` 原样显示(否则持久化的 full 在空轨迹任务上会被洗成 legacy,
 * 换个任务回来就丢了)。
 */
export function resolveTranscriptMode(
  override: TranscriptMode | null | undefined,
  fallback: TranscriptMode,
  hasTrace: boolean,
): TranscriptMode {
  const requested = requestedTranscriptMode(override, fallback);
  if (requested === 'legacy') return 'legacy';
  return hasTrace ? requested : 'legacy';
}

/** 分类计数(档 3 角标「思考 N · 工具 N · bash N」的数据源) */
export interface TraceCounts {
  /** 思考条目数(`kind === 'reasoning'` 的行数) */
  reasoning: number;
  /** 工具调用数(`kind === 'tool_call'` 的行数) */
  tool: number;
  /** 其中 bash 工具调用数(`kind === 'tool_call'` 且 `name === 'bash'`) */
  bash: number;
}

/**
 * bash 工具名。
 *
 * 后端唯一来源是 `server-rs/src/tools/bash.rs::TOOL_NAME`;这里写字面量是有意的:
 * 前端无法从 Rust 常量派生,而这道耦合由 `check-contract` 的同名工具清单间接看护。
 * 改名会同时改两侧并在这里留痕。
 */
const BASH_TOOL_NAME = 'bash';

/**
 * 排序 + 去重(档 2 / 档 3 的数据入口)。
 *
 * **为什么必须去重**:同一条轨迹会从两条通道到达前端——SSE `kind=trace` 增量帧与
 * `GET /trace` 补拉结果。断线重连、缺口补拉、切换任务后的重新拉取都会让两路重叠;
 * `seq` 是任务内单调且唯一的,**按 `seq` 去重就是唯一正确口径**(用 `id` 也行但没必要
 * 引第二把钥匙)。
 *
 * 排序按 `seq` 升序:轨迹表写入侧就是按 `seq` 递增落库的,前端重排只是防御脏数据。
 * 输入数组不被修改(调用方可能持有 store 里的同一份引用)。
 */
export function flattenTrace(rows: readonly TaskTrace[]): TaskTrace[] {
  const seen = new Set<number>();
  const out: TaskTrace[] = [];
  let ordered = true;
  let prev = Number.NEGATIVE_INFINITY;
  for (const row of rows) {
    if (seen.has(row.seq)) continue;
    seen.add(row.seq);
    // 写入侧与实时并入都按 seq 递增;输入已有序时跳过 O(n log n) 排序。
    if (row.seq < prev) ordered = false;
    prev = row.seq;
    out.push(row);
  }
  if (!ordered) out.sort((a, b) => a.seq - b.seq);
  return out;
}

/**
 * 分类计数(档 3 角标)。
 *
 * 口径**刻意不对称**:`tool_result` 不计入任何一项。一次工具调用的完整故事是
 * 「call + result」两行,若把 result 也算进「工具 N」,一次调用会显示成 N=2,
 * 与用户看到的调用次数不符。而 `bash N` 只数 `tool_call` 是为了回答
 * 「这一组里跑了几次命令行」——那正是档 3 里最需要一眼看清的信息。
 */
export function countTrace(rows: readonly TaskTrace[]): TraceCounts {
  let reasoning = 0;
  let tool = 0;
  let bash = 0;
  for (const row of rows) {
    if (row.kind === 'reasoning') {
      reasoning += 1;
    } else if (row.kind === 'tool_call') {
      tool += 1;
      if (row.name === BASH_TOOL_NAME) bash += 1;
    }
  }
  return { reasoning, tool, bash };
}

/** 档 3 的一组:同一 `phase + step_index` 归属下的全部轨迹 */
export interface TraceGroup {
  /** 分组键(`phase` 与 `step_index` 的稳定拼接;可直接用作 `v-for` 的 key) */
  key: string;
  phase: string;
  stepIndex: number | null;
  /** 组标题(经 `phaseText` 翻译,带步骤名后缀) */
  label: string;
  /** 组内轨迹(已按 `seq` 升序) */
  rows: TaskTrace[];
  /** 组内分类计数(挂在组标题右下角) */
  counts: TraceCounts;
}

/**
 * 按 `phase + step_index` 分组(档 3)。
 *
 * 分组键必须**同时**含这两段:同一个 `phase` 下的不同步骤(如 team 的主 Agent N 个子目标)
 * 各有独立上下文,只按 `phase` 分会把它们糊成一组,而同一个 `step_index`
 * 在不同 `phase` 里(如 `step` 与 `audit`)也完全是两件事。
 *
 * 组的顺序 = 组内首条轨迹的出现顺序(即首次 `seq` 的先后),而不是按 `phase` 字典序——
 * 执行顺序才是用户脑中的顺序。组内仍按 `seq` 升序。
 *
 * `stepName` 透传给 `phaseText` 用于给步骤类阶段补一个可读步骤名(取不到则不加)。
 */
export function groupTrace(rows: readonly TaskTrace[], stepName?: StepNameAt): TraceGroup[] {
  const ordered = flattenTrace(rows);
  const groups = new Map<string, TraceGroup>();
  for (const row of ordered) {
    const stepIndex = row.step_index;
    const key = `${row.phase}:${stepIndex ?? ''}`;
    let group = groups.get(key);
    if (!group) {
      group = {
        key,
        phase: row.phase,
        stepIndex,
        label: phaseText(row.phase, stepIndex, stepName),
        rows: [],
        counts: { reasoning: 0, tool: 0, bash: 0 },
      };
      groups.set(key, group);
    }
    group.rows.push(row);
  }
  for (const group of groups.values()) {
    group.counts = countTrace(group.rows);
  }
  return [...groups.values()];
}

/**
 * 档 3 角标的文案(「思考 3 · 工具 5 · bash 2」)。
 *
 * 空项不显示:一组里没有思考条目就不写「思考 0」——角标是为了让人一眼看出这组
 * **发生了什么**,而不是罗列全部类别的零值。三项全空时返回空串(调用方据此不渲染角标)。
 */
export function formatTraceCounts(counts: TraceCounts): string {
  const parts: string[] = [];
  if (counts.reasoning > 0) parts.push(`思考 ${counts.reasoning}`);
  if (counts.tool > 0) parts.push(`工具 ${counts.tool}`);
  if (counts.bash > 0) parts.push(`bash ${counts.bash}`);
  return parts.join(' · ');
}

/**
 * 轨迹正文的展示文本（UIFIX-8）。
 *
 * **问题面**：提交 1 之前落库的工具轨迹把「模型原始参数字符串」当成了字符串值再
 * pretty 序列化，于是存成 `"{\"command\": \"ls -la\"}"` —— 换行变成字面 `\n` 两字符、
 * 内层引号被判转义，前端 `<pre>` 里整段挤成一行。
 *
 * **为什么前端还要兜一层**：后端修的是**新写入**的行（`sink.rs::render_payload`），
 * 历史轨迹已经是坏数据，不会因为改代码而自愈。旧任务回看时间线时仍应可读。
 *
 * **只对工具类生效**：`reasoning` 是模型原样输出的纯文本，里面可能正常出现花括号；
 * 对它做 `JSON.parse` 会把「碰巧是合法 JSON 的思考」错误地重排，那是篡改内容而非修复。
 *
 * **两段式解析**：坏数据是「JSON 字符串字面量里装着 JSON 文本」，需要解析两次才能拿到
 * 结构化参数；第一次拿到的若是对象/数组则直接美化，若是字符串则再试一次。
 * 任何一步失败都原样返回——repair 绝不能把可读内容变成空或报错。
 */
export function formatTraceContent(kind: TaskTrace['kind'], content: string): string {
  if (kind !== 'tool_call' && kind !== 'tool_result') return content;
  const once = tryParse(content);
  if (once === undefined) return content;
  if (typeof once === 'string') {
    const twice = tryParse(once);
    // 二次解析不出结构化数据 = 它本来就是纯文本（bash stdout），保留原文（含其换行）
    return twice === undefined ? once : JSON.stringify(twice, null, 2);
  }
  return JSON.stringify(once, null, 2);
}

/** 解析失败与「解析出 undefined」不可混同：用哨兵区分「没有值」与「不是 JSON」 */
function tryParse(text: string): unknown {
  const trimmed = text.trim();
  // 只对像 JSON 的文本尝试解析：纯文本（bash 输出、中文结果）连试都不必试，
  // 也避免把 "123"、'true' 这类裸标量当成结构化数据重排
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[') && !trimmed.startsWith('"')) {
    return undefined;
  }
  try {
    return JSON.parse(trimmed) as unknown;
  } catch {
    return undefined;
  }
}
