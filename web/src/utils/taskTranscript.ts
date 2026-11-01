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
 * TMSHOW-2 只做渲染与临时切换(不持久化);默认档位与单任务覆盖记忆属 TMSHOW-3。
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
  for (const row of rows) {
    if (seen.has(row.seq)) continue;
    seen.add(row.seq);
    out.push(row);
  }
  out.sort((a, b) => a.seq - b.seq);
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
