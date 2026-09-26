// 自定义流程的画布布局(二维批次 3 前端)。
//
// 只做「步骤 → 坐标」的纯计算:层级取 agentFlowGraph.computeLevels(与后端图原语同口径),
// 层号定 y、层内下标定 x。用户拖拽过的节点把坐标写回步骤的 x/y,此后以用户坐标为准。
//
// 纪律:画布**不主动写坐标**。`autoLayout` 只产出显示位置,只有用户显式动作
// (拖拽节点 / 点「重新布局」→ `applyAutoLayout`)才把坐标落到步骤上,保证存量一维流程
// 的 JSON 不会因为「打开过画布」而改变(与二维批次 1 的逐字节兼容承诺一致)。

import type { AgentFlowStep } from '../api/types';
import { effectiveLevels } from './agentFlowGraph';

/**
 * 节点卡片尺寸与间距,仅用于自动布局。测试**不断言这些具体数值**(计划 §四 批次 3
 * 验收断言「测试不断言像素/布局坐标」),只断言节点之间的相对关系。
 */
export const NODE_W = 220;
export const NODE_H = 64;
export const GAP_X = 48;
export const GAP_Y = 56;

/** 画布坐标 */
export interface FlowPoint {
  x: number;
  y: number;
}

/** 步骤的持久化坐标(两个分量都是数字才算已布局) */
export function savedPosition(s: AgentFlowStep): FlowPoint | null {
  return typeof s.x === 'number' && typeof s.y === 'number' ? { x: s.x, y: s.y } : null;
}

/** 是否有任一节点带用户坐标(决定「重新布局」是否要二次确认) */
export function hasCustomPositions(steps: AgentFlowStep[]): boolean {
  return steps.some((s) => savedPosition(s) !== null);
}

/**
 * 自动布局(纵向分层):第 N 层的 y 随层号递增,同层节点按下标从左到右排列,
 * 并按最宽一层横向居中 → 存量一维流程进来就是一条自上而下的链(WF-10「不惊吓用户」)。
 *
 * 分层用 `effectiveLevels`(执行器视角):线性兼容流程按数组顺序串成链,所以每步独占
 * 一层;显式配了上游的二维流程才按真实依赖分层。存在环时 `effectiveLevels` 返回 null,
 * 此处**退化为单列**(每步独占一行)而不是抛错:画布仍要能渲染出问题流程,让用户看到
 * 是哪几个节点连成了环(环的权威拦截在保存期,由后端 400)。
 */
export function autoLayout(steps: AgentFlowStep[]): Record<string, FlowPoint> {
  const levels = effectiveLevels(steps);
  const lane = levels ?? steps.map((_, i) => i);
  const byLane = new Map<number, number[]>();
  lane.forEach((lv, i) => {
    const bucket = byLane.get(lv);
    if (bucket) bucket.push(i);
    else byLane.set(lv, [i]);
  });
  const widest = Math.max(1, ...[...byLane.values()].map((b) => b.length));
  const out: Record<string, FlowPoint> = {};
  for (const [lv, bucket] of byLane) {
    const offset = ((widest - bucket.length) * (NODE_W + GAP_X)) / 2;
    bucket.forEach((idx, k) => {
      out[steps[idx].id] = { x: offset + k * (NODE_W + GAP_X), y: lv * (NODE_H + GAP_Y) };
    });
  }
  return out;
}

/** 画布实际显示位置:有用户坐标的用用户坐标,其余走自动布局 */
export function displayPositions(steps: AgentFlowStep[]): Record<string, FlowPoint> {
  const auto = autoLayout(steps);
  const out: Record<string, FlowPoint> = {};
  for (const s of steps) {
    const saved = savedPosition(s);
    out[s.id] = saved ?? auto[s.id] ?? { x: 0, y: 0 };
  }
  return out;
}

/** 把自动布局写回步骤(「重新布局」按钮;会覆盖用户坐标) */
export function applyAutoLayout(steps: AgentFlowStep[]): void {
  const pos = autoLayout(steps);
  for (const s of steps) {
    const p = pos[s.id];
    if (!p) continue;
    s.x = p.x;
    s.y = p.y;
  }
}

/**
 * 写回单个节点的拖拽坐标(取整:坐标只用于展示,整数让落盘 JSON 更短更稳定)。
 * 找不到 id 时静默忽略(画布与草稿短暂不同步时不应抛错打断交互)。
 */
export function setNodePosition(steps: AgentFlowStep[], id: string, at: FlowPoint): void {
  const target = steps.find((s) => s.id === id);
  if (!target) return;
  target.x = Math.round(at.x);
  target.y = Math.round(at.y);
}
