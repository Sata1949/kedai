// @vitest-environment jsdom
// useAgentFlow 的二维依赖编辑接线测试(二维批次 1/2 前端)。
// 覆盖:新增步骤带二维字段缺省、删除步骤同步清理悬空上游引用、上游勾选/成果标注的
// 就地写入、并行上限归一化。纯图算法在 utils/agentFlowGraph.test.ts 覆盖,此处只验证
// composable 的接线与草稿写入(保存/校验仍由后端权威把关)。
// 二维批次 7a 追加:流程搬运(文件归一 / 一次 POST 导入 / 走端点的导出)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { importReportMessage, normalizeFlowFile, useAgentFlow } from './useAgentFlow';
import { downloadBlob } from '../exportFile';
import { resetApiTokenForTest } from '../api/client';
import type { AgentFlowConfig } from '../api/types';

// 导出落盘在 jsdom 里无处可去(URL.createObjectURL 未实现),换成本地捕获:
// 断言「导的是什么文件名、什么内容」才是本批的验收点。
vi.mock('../exportFile', () => ({ downloadBlob: vi.fn(), saveExportFile: vi.fn() }));
const downloadMock = downloadBlob as unknown as ReturnType<typeof vi.fn>;

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

// ==================== 流程搬运(二维批次 7a) ====================

/** 一份最小流程(带 steps 即算合法形状) */
function flowOf(id: string, name: string): AgentFlowConfig {
  return {
    id,
    name,
    description: null,
    enabled: true,
    steps: [{ id: 'n1', name: '节点', enabled: true, goal: 'g', action: 'direct', generates: true }],
  };
}

/**
 * 按 URL 匹配应答的 fetch 假实现(bootstrap 恒先应答 token);
 * 返回 { urls, bodies } 供断言「调了哪些端点、发了什么请求体」。
 */
function mockApi(routes: Array<{ url: string; body: unknown }>): {
  urls: string[];
  bodies: Array<{ url: string; body?: string }>;
} {
  const urls: string[] = [];
  const bodies: Array<{ url: string; body?: string }> = [];
  vi.spyOn(globalThis, 'fetch').mockImplementation(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      urls.push(url);
      bodies.push({ url, body: typeof init?.body === 'string' ? init.body : undefined });
      if (url.includes('/api/bootstrap')) {
        return new Response(JSON.stringify({ token: 'test-secret' }), { status: 200 });
      }
      const hit = routes.find((r) => url.includes(r.url));
      return hit
        ? new Response(JSON.stringify(hit.body), { status: 200 })
        : new Response('{}', { status: 404 });
    },
  );
  return { urls, bodies };
}

/** 构造一次「选择文件」事件(只需 files[0].text() 与 value) */
function filePickEvent(payload: unknown): Event {
  const file = { text: async () => JSON.stringify(payload) };
  return { target: { files: [file], value: '' } } as unknown as Event;
}

describe('流程搬运 7a:文件归一', () => {
  it('搬运包:取 flows 与 root_id(版本键校验通过)', () => {
    const out = normalizeFlowFile({
      kedai_flow_bundle: 1,
      root_id: 'f1',
      flows: [flowOf('f1', '主'), flowOf('f2', '子')],
    });
    expect(out.flows.map((f) => f.id)).toEqual(['f1', 'f2']);
    expect(out.rootId).toBe('f1');
  });

  it('库格式:整库导入(不再只取当前流程,否则静默丢流程),入口取 current_flow_id', () => {
    const out = normalizeFlowFile({
      current_flow_id: 'f2',
      flows: [flowOf('f1', 'A'), flowOf('f2', 'B')],
    });
    expect(out.flows.map((f) => f.name)).toEqual(['A', 'B']);
    expect(out.rootId).toBe('f2');
  });

  it('{config} 包装与单流程各取一份(旧导出文件必须继续可导)', () => {
    expect(normalizeFlowFile({ config: flowOf('', '包装') }).flows).toHaveLength(1);
    const single = normalizeFlowFile(flowOf('', '裸流程'));
    expect(single.flows[0].name).toBe('裸流程');
  });

  it('版本键不是 1 → 明确拒绝(不静默丢字段);无法识别 → 中文错误', () => {
    expect(() => normalizeFlowFile({ kedai_flow_bundle: 2, flows: [flowOf('f1', 'x')] })).toThrow(
      /升级 Kedai/,
    );
    expect(() => normalizeFlowFile({ flows: [{ name: '没有 steps' }] })).toThrow(/steps 数组/);
    expect(() => normalizeFlowFile({ note: '随便一个 JSON' })).toThrow(/无法识别/);
  });

  it('导入报告文案含导入 / 跳过 / 新 id 计数与点名', () => {
    expect(importReportMessage({ imported: 2, skipped: 0, renamed: [], replaced: [] })).toBe('已导入 2 个流程');
    const msg = importReportMessage({
      imported: 1,
      skipped: 3,
      renamed: [{ old_id: 'a', new_id: 'b', name: '调研' }],
      replaced: [],
    });
    expect(msg).toContain('跳过 3 个(内容已存在)');
    expect(msg).toContain('1 个因 id 冲突分配了新 id');
    expect(msg).toContain('「调研」');
  });
});

describe('流程搬运 7a:导入/导出接线', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    resetApiTokenForTest();
    vi.restoreAllMocks();
    downloadMock.mockClear();
  });

  it('导入:一次 POST 归一后的 flows + root_id,并按报告提示', async () => {
    const api = mockApi([
      {
        url: '/api/agent-flows/import',
        body: {
          ok: true,
          library: { current_flow_id: 'f1', flows: [flowOf('f1', '主')] },
          report: { imported: 1, skipped: 1, renamed: [{ old_id: 'f1', new_id: 'new', name: '主' }] },
        },
      },
      {
        url: '/api/agent-flows',
        body: { ok: true, library: { current_flow_id: 'f1', flows: [flowOf('f1', '主')] }, config: null },
      },
    ]);
    const flow = useAgentFlow();
    await flow.onFlowImport(
      filePickEvent({
        kedai_flow_bundle: 1,
        root_id: 'f1',
        flows: [flowOf('f1', '主'), flowOf('f2', '子')],
      }),
    );

    const post = api.bodies.find((b) => b.url.includes('/agent-flows/import'));
    if (!post) throw new Error('必须走搬运包导入端点(旧路径逐个 PUT 做不到原子)');
    const sent = JSON.parse(post.body ?? '{}') as { flows: AgentFlowConfig[]; root_id?: string };
    expect(sent.flows.map((f) => f.id)).toEqual(['f1', 'f2']);
    expect(sent.root_id).toBe('f1');
    expect(api.urls.filter((u) => u.includes('/agent-flows/import'))).toHaveLength(1);
    expect(flow.flowMsg.value).toContain('已导入 1 个流程');
    expect(flow.flowMsg.value).toContain('跳过 1 个');
    expect(flow.flowImporting.value).toBe(false);
  });

  it('导入失败:后端 400 文案进 flowMsg,不抛穿', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/api/bootstrap')) {
        return new Response(JSON.stringify({ token: 't' }), { status: 200 });
      }
      return new Response(
        JSON.stringify({ error: '流程「悬空」的步骤「挂载」引用的子流程不存在:x', code: 'VALIDATION' }),
        { status: 400 },
      );
    });
    const flow = useAgentFlow();
    await flow.onFlowImport(filePickEvent({ config: flowOf('', '悬空') }));
    expect(flow.flowMsg.value).toContain('导入失败');
    expect(flow.flowMsg.value).toContain('子流程不存在');
  });

  it('导出当前流程:走搬运包端点(带 id),文件名与提示含子流程数', async () => {
    const api = mockApi([
      {
        url: '/api/agent-flows/export',
        body: {
          ok: true,
          bundle: {
            kedai_flow_bundle: 1,
            root_id: 'f1',
            flows: [flowOf('f1', '主流程'), flowOf('f2', '子')],
          },
        },
      },
    ]);
    const flow = await loadedFlow();
    flow.flowId.value = 'f1';
    // 服务端那份的名字是权威(导出取的就是它),草稿名只在库未命中时兜底
    await flow.exportFlowNow();

    expect(api.urls.some((u) => u.includes('/agent-flows/export?id=f1'))).toBe(true);
    expect(downloadMock).toHaveBeenCalledTimes(1);
    const [fileName, blob] = downloadMock.mock.calls[0] as [string, Blob];
    expect(fileName).toBe('kedai-flow-测试菱形流程.json');
    const payload = JSON.parse(await blob.text()) as { flows: AgentFlowConfig[]; root_id: string };
    expect(payload.flows.map((f) => f.id)).toEqual(['f1', 'f2']);
    expect(payload.root_id).toBe('f1');
    expect(flow.flowMsg.value).toContain('1 个子流程');
  });

  it('导出单流程(无子流程)也给成功提示,不静默清空消息', async () => {
    mockApi([
      {
        url: '/api/agent-flows/export',
        body: {
          ok: true,
          bundle: { kedai_flow_bundle: 1, root_id: 'f1', flows: [flowOf('f1', '测试菱形流程')] },
        },
      },
    ]);
    const flow = await loadedFlow();
    flow.flowId.value = 'f1';
    await flow.exportFlowNow();
    expect(flow.flowMsg.value).toBe('已导出「测试菱形流程」');
  });

  it('有未保存修改:导出前先确认;取消则不发请求', async () => {
    const api = mockApi([
      {
        url: '/api/agent-flows/export',
        body: { ok: true, bundle: { kedai_flow_bundle: 1, root_id: 'f1', flows: [] } },
      },
    ]);
    const flow = await loadedFlow();
    flow.flowId.value = 'f1';
    // 真改一处草稿(加一个步骤)→ 与库中那份不同
    flow.addStep();
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);

    await flow.exportFlowNow();
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(confirm.mock.calls[0][0]).toContain('已保存');
    expect(api.urls.some((u) => u.includes('/agent-flows/export'))).toBe(false);
    expect(downloadMock).not.toHaveBeenCalled();

    // 确认继续 → 正常导出
    confirm.mockReturnValue(true);
    await flow.exportFlowNow();
    expect(api.urls.some((u) => u.includes('/agent-flows/export?id=f1'))).toBe(true);
    expect(downloadMock).toHaveBeenCalledTimes(1);
  });

  it('无草稿改动时不弹确认(避免噪音)', async () => {
    mockApi([
      {
        url: '/api/agent-flows/export',
        body: { ok: true, bundle: { kedai_flow_bundle: 1, root_id: 'f1', flows: [] } },
      },
    ]);
    const flow = await loadedFlow();
    flow.flowId.value = 'f1';
    const confirm = vi.spyOn(window, 'confirm');
    await flow.exportFlowNow();
    expect(confirm).not.toHaveBeenCalled();
    expect(downloadMock).toHaveBeenCalledTimes(1);
  });

  it('导出全部流程:不带 id 调端点,提示流程总数', async () => {
    const api = mockApi([
      {
        url: '/api/agent-flows/export',
        body: {
          ok: true,
          bundle: { kedai_flow_bundle: 1, root_id: 'f1', flows: [flowOf('f1', 'A'), flowOf('f2', 'B')] },
        },
      },
    ]);
    const flow = await loadedFlow();
    await flow.exportAllFlows();
    const call = api.urls.find((u) => u.includes('/agent-flows/export'));
    expect(call).toBe('/api/agent-flows/export');
    expect(flow.flowMsg.value).toContain('已导出全部 2 个流程');
  });
});
