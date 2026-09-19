// @vitest-environment jsdom
// useAgentFlow 的二维依赖编辑接线测试(二维批次 1/2 前端)。
// 覆盖:新增步骤带二维字段缺省、删除步骤同步清理悬空上游引用、上游勾选/成果标注的
// 就地写入、并行上限归一化。纯图算法在 utils/agentFlowGraph.test.ts 覆盖,此处只验证
// composable 的接线与草稿写入(保存/校验仍由后端权威把关)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useAgentFlow } from './useAgentFlow';
import { resetApiTokenForTest } from '../api/client';
import type { AgentFlowConfig } from '../api/types';

/** 样例二维流程:左路 / 右路 两源节点 → 合并(显式成果节点) */
function sampleFlow(): AgentFlowConfig {
  return {
    id: 'f1',
    name: '测试菱形流程',
    description: null,
    enabled: true,
    steps: [
      { id: 'a', name: '左路', enabled: true, goal: 'g', action: 'direct', generates: true, inputs: [] },
      { id: 'b', name: '右路', enabled: true, goal: 'g', action: 'direct', generates: true, inputs: [] },
      {
        id: 'c',
        name: '合并',
        enabled: true,
        goal: 'g',
        action: 'direct',
        generates: true,
        inputs: ['a', 'b'],
        is_output: true,
      },
    ],
    max_parallel_nodes: 2,
  };
}

/** 第一跳 bootstrap 取 token,第二跳返回流程库 */
function mockFetchLibrary(): void {
  const spy = vi.spyOn(globalThis, 'fetch');
  spy.mockResolvedValueOnce(new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 }));
  spy.mockResolvedValueOnce(
    new Response(
      JSON.stringify({ ok: true, library: { current_flow_id: 'f1', flows: [sampleFlow()] }, config: null }),
      { status: 200 },
    ),
  );
}

async function loadedFlow() {
  mockFetchLibrary();
  const flow = useAgentFlow();
  await flow.loadFlowConfig();
  expect(flow.flowDraft.value?.steps.length).toBe(3);
  return flow;
}

describe('useAgentFlow 二维依赖编辑', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    resetApiTokenForTest();
    vi.restoreAllMocks();
  });

  it('新增步骤默认不设上游(一维语义)、未标注成果', async () => {
    const flow = await loadedFlow();
    flow.addStep();
    const added = flow.flowDraft.value?.steps.at(-1);
    expect(added?.inputs).toEqual([]);
    expect(added?.is_output).toBeNull();
  });

  it('删除步骤同步清理其它步骤对它的上游引用(悬空引用会被后端拒绝)', async () => {
    const flow = await loadedFlow();
    flow.removeStep('a');
    const steps = flow.flowDraft.value?.steps ?? [];
    expect(steps.map((s) => s.id)).toEqual(['b', 'c']);
    expect(steps[1].inputs).toEqual(['b']);
  });

  it('上游勾选就地写入 inputs;成果标注往返切换(null = 未标注)', async () => {
    const flow = await loadedFlow();
    const merge = flow.flowDraft.value?.steps[2] as AgentFlowConfig['steps'][number];
    flow.toggleStepInput(merge, 'b');
    expect(merge.inputs).toEqual(['a']);
    flow.toggleStepInput(merge, 'a');
    expect(merge.inputs).toEqual([]);
    flow.toggleStepInput(merge, 'a');
    expect(merge.inputs).toEqual(['a']);

    flow.toggleStepOutput(merge);
    expect(merge.is_output).toBeNull();
    flow.toggleStepOutput(merge);
    expect(merge.is_output).toBe(true);
  });

  it('并行上限:空 = 用后端默认,越界夹取到 1-8', async () => {
    const flow = await loadedFlow();
    flow.setFlowParallel('');
    expect(flow.flowDraft.value?.max_parallel_nodes).toBeNull();
    flow.setFlowParallel('0');
    expect(flow.flowDraft.value?.max_parallel_nodes).toBe(1);
    flow.setFlowParallel('99');
    expect(flow.flowDraft.value?.max_parallel_nodes).toBe(8);
    flow.setFlowParallel('3');
    expect(flow.flowDraft.value?.max_parallel_nodes).toBe(3);
  });
});
