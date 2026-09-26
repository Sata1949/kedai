// 画布数据层测试(二维批次 3 前端)。
//
// 覆盖画布的**数据与交互逻辑**:节点/连线产出、连线合法性拦截(自环/重复/成环)、
// 拖拽坐标写回、重新布局。渲染层(像素、缩放、minimap)不在断言范围内
// ——与计划 §四 批次 3 的「测试不断言像素/布局坐标」口径一致,也与画布库解耦的分层一致。
import { describe, expect, it } from 'vitest';
import { ref } from 'vue';
import type { AgentFlowConfig, AgentFlowStep } from '../api/types';
import { useFlowCanvas } from './useFlowCanvas';

/** 步骤工厂(与 agentFlowGraph.test.ts 同形) */
function step(
  id: string,
  inputs: string[] = [],
  overrides: Partial<AgentFlowStep> = {},
): AgentFlowStep {
  return {
    id,
    name: id,
    enabled: true,
    goal: `${id} 目标`,
    action: 'direct',
    generates: true,
    inputs,
    ...overrides,
  };
}

/** 把步骤数组装进响应式草稿(组件里草稿是 ref,故测试也走 ref 才能验证响应性) */
function draftOf(steps: AgentFlowStep[]) {
  const draft = ref<AgentFlowConfig>({ enabled: true, steps });
  return { draft, canvas: useFlowCanvas(() => draft.value.steps) };
}

/** 菱形:a、b 源节点 → c 合并 → d 收口 */
function diamond(): AgentFlowStep[] {
  return [step('a'), step('b'), step('c', ['a', 'b']), step('d', ['c'])];
}

describe('useFlowCanvas 节点与连线', () => {
  it('线性流程(存量一维)补出隐式串联链:节点逐层下降、连线首尾相接', () => {
    const { canvas } = draftOf([step('a'), step('b'), step('c')]);
    // 节点层级按执行器视角递增(源节点第 1 层)
    expect(canvas.nodes.value.map((n) => n.data.level)).toEqual([1, 2, 3]);
    // y 随层级下降 → 画布上是一条纵向链,与 WF-10「存量流程自动纵向布局」一致
    const ys = canvas.nodes.value.map((n) => n.position.y);
    expect(ys[1]).toBeGreaterThan(ys[0]);
    expect(ys[2]).toBeGreaterThan(ys[1]);
    // 隐式边全部画出来,否则用户看到的图与实际执行顺序不符
    expect(canvas.edges.value.map((e) => `${e.source}->${e.target}`)).toEqual([
      'a->b',
      'b->c',
    ]);
  });

  it('二维流程按显式上游画边,同层节点层级相同', () => {
    const { canvas } = draftOf(diamond());
    expect(canvas.edges.value.map((e) => `${e.source}->${e.target}`)).toEqual([
      'a->c',
      'b->c',
      'c->d',
    ]);
    const level = new Map(canvas.nodes.value.map((n) => [n.id, n.data.level]));
    expect(level.get('a')).toBe(1);
    expect(level.get('b')).toBe(1);
    expect(level.get('c')).toBe(2);
    expect(level.get('d')).toBe(3);
  });

  it('悬空上游(保存会被拒)不画边,避免画到不存在的节点上', () => {
    const { canvas } = draftOf([step('a'), step('b', ['ghost'])]);
    expect(canvas.edges.value).toEqual([]);
  });

  it('节点坐标:用户拖拽过的用拖拽坐标,其余走自动布局', () => {
    const steps = diamond();
    steps[3].x = 500;
    steps[3].y = 600;
    const { canvas } = draftOf(steps);
    const d = canvas.nodes.value.find((n) => n.id === 'd');
    expect(d?.position).toEqual({ x: 500, y: 600 });
    // 拖拽过的节点是 d(末尾),判为成果节点
    expect(d?.data.isOutput).toBe(true);
  });

  it('成果节点按 id 标注(重名步骤不会互相串位)', () => {
    const steps = [step('a', [], { name: '同名' }), step('b', [], { name: '同名' })];
    steps[0].is_output = true;
    const { canvas } = draftOf(steps);
    expect(canvas.nodes.value.map((n) => n.data.isOutput)).toEqual([true, false]);
  });

  it('成环流程不抛错,层级为 null(画布仍要能显示问题流程)', () => {
    const { canvas } = draftOf([step('a', ['b']), step('b', ['a'])]);
    expect(canvas.nodes.value.map((n) => n.data.level)).toEqual([null, null]);
    expect(canvas.nodes.value).toHaveLength(2);
  });

  it('边带画布组件类型与「是否隐式」标记(隐式链没有可断开的 inputs 记录)', () => {
    const linear = draftOf([step('a'), step('b')]);
    expect(
      linear.canvas.edges.value.every((e) => e.type === 'flowEdge' && e.data.implicit),
    ).toBe(true);

    const twoDim = draftOf(diamond());
    expect(
      twoDim.canvas.edges.value.every((e) => e.type === 'flowEdge' && !e.data.implicit),
    ).toBe(true);
  });
});

describe('useFlowCanvas 连线拦截', () => {
  it('合法连线写入下游的上游并给中文提示', () => {
    const { draft, canvas } = draftOf([step('a'), step('b', ['a']), step('c', ['a'])]);
    expect(canvas.connect('b', 'c')).toBe(true);
    expect(draft.value.steps[2].inputs).toEqual(['a', 'b']);
    expect(canvas.canvasMsg.value).toContain('已连接');
  });

  it('自环被拒(后端亦会拒)', () => {
    const { canvas } = draftOf([step('a'), step('b', ['a'])]);
    expect(canvas.connect('a', 'a')).toBe(false);
    expect(canvas.canvasMsg.value).toContain('自环');
  });

  it('重复上游被拒且不重复写入', () => {
    const { draft, canvas } = draftOf([step('a'), step('b', ['a'])]);
    expect(canvas.connect('a', 'b')).toBe(false);
    expect(draft.value.steps[1].inputs).toEqual(['a']);
    expect(canvas.canvasMsg.value).toContain('已经有');
  });

  it('成环被拒:把下游接成上游', () => {
    // a → c → d;再把 d 接成 a 的上游即闭合成环
    const { draft, canvas } = draftOf(diamond());
    expect(canvas.connect('d', 'a')).toBe(false);
    expect(draft.value.steps[0].inputs).toEqual([]);
    expect(canvas.canvasMsg.value).toContain('环');
  });

  it('一维流程连上第一条边时明确提示语义变化(不再是「上一步」)', () => {
    const { canvas } = draftOf([step('a'), step('b'), step('c')]);
    expect(canvas.connect('a', 'c')).toBe(true);
    expect(canvas.canvasMsg.value).toContain('二维流程');
    expect(canvas.canvasMsg.value).toContain('源节点');
  });

  it('未知节点 id 静默忽略(画布与草稿短暂不同步时不打断交互)', () => {
    const { canvas } = draftOf(diamond());
    expect(canvas.connect('ghost', 'a')).toBe(false);
    expect(canvas.connect('a', 'ghost')).toBe(false);
  });

  it('断开连接移除上游;不存在该上游时保持原样', () => {
    const { draft, canvas } = draftOf(diamond());
    canvas.disconnect('a', 'c');
    expect(draft.value.steps[2].inputs).toEqual(['b']);
    canvas.disconnect('a', 'c');
    expect(draft.value.steps[2].inputs).toEqual(['b']);
  });

  it('连到停用上游:允许连但当场说清「保存会被拒」(与列表视图同口径)', () => {
    const { draft, canvas } = draftOf([step('a', [], { enabled: false }), step('b')]);
    expect(canvas.connect('a', 'b')).toBe(true);
    expect(draft.value.steps[1].inputs).toEqual(['a']);
    expect(canvas.canvasMsg.value).toContain('已停用');
    expect(canvas.canvasMsg.value).toContain('保存会被后端拒绝');
  });

  it('断开连线提示需保存;断掉最后一条上游时说明该步骤只用任务目标', () => {
    const { draft, canvas } = draftOf(diamond());
    canvas.disconnect('a', 'c');
    expect(canvas.canvasMsg.value).toContain('点「保存执行流程」后生效');
    canvas.disconnect('b', 'c');
    expect(draft.value.steps[2].inputs).toEqual([]);
    expect(canvas.canvasMsg.value).toContain('将只用任务目标');
  });

  it('线性隐式边不可断开:提示改串联关系要先显式设置上游', () => {
    const { draft, canvas } = draftOf([step('a'), step('b')]);
    canvas.disconnect('a', 'b');
    expect(draft.value.steps[1].inputs).toEqual([]);
    expect(canvas.canvasMsg.value).toContain('隐式串联');
  });
});

describe('useFlowCanvas 坐标与 Inspector', () => {
  it('拖拽结束写回坐标并提示需保存', () => {
    const { draft, canvas } = draftOf(diamond());
    canvas.onNodeDragStop('b', { x: 12.4, y: 88.6 });
    expect(draft.value.steps[1].x).toBe(12);
    expect(draft.value.steps[1].y).toBe(89);
    expect(canvas.canvasMsg.value).toContain('保存');
  });

  it('hasLayout 在用户拖拽后为真(供「重新布局」二次确认)', () => {
    const { canvas } = draftOf(diamond());
    expect(canvas.hasLayout.value).toBe(false);
    canvas.onNodeDragStop('b', { x: 1, y: 1 });
    expect(canvas.hasLayout.value).toBe(true);
  });

  it('重新布局把坐标写回全部步骤', () => {
    const { draft, canvas } = draftOf(diamond());
    canvas.relayout();
    expect(draft.value.steps.every((s) => typeof s.x === 'number' && typeof s.y === 'number')).toBe(
      true,
    );
    expect(canvas.canvasMsg.value).toContain('重新布局');
  });

  it('Inspector 开关记录当前节点 id', () => {
    const { canvas } = draftOf(diamond());
    expect(canvas.inspectingId.value).toBeNull();
    canvas.inspect('c');
    expect(canvas.inspectingId.value).toBe('c');
    canvas.inspect(null);
    expect(canvas.inspectingId.value).toBeNull();
  });
});
