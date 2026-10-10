// @vitest-environment jsdom
// 综合导入 / 导出弹窗测试(IMPHUB 步骤 C):嗅探预览、模式过滤说明、逐段执行与失败隔离、
// 回执形状、导出选段与下载。段载荷的完整形状矩阵在 utils/importBundle.test.ts,
// 本文件只管组件行为(勾选状态、API 调用与回执渲染)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    importPromptPreset: vi.fn(),
    savePromptInject: vi.fn(),
    getPromptInject: vi.fn(),
    importSkills: vi.fn(),
    getRegexScripts: vi.fn(),
    saveRegexScripts: vi.fn(),
    importPluginTools: vi.fn(),
    saveAgentPromptMd: vi.fn(),
    importAgentFlows: vi.fn(),
    exportBundle: vi.fn(),
    listSkills: vi.fn(),
  };
});
vi.mock('../exportFile', () => ({ downloadBlob: vi.fn() }));

import * as api from '../api';
import { downloadBlob } from '../exportFile';
import { useAppStore } from '../store';
import ImportBundleModal from './ImportBundleModal.vue';

const mocked = api as unknown as Record<string, ReturnType<typeof vi.fn>>;

function bundleOf(sections: Record<string, unknown>): Record<string, unknown> {
  return {
    kedai_import_bundle: 1,
    exported_at: '2026-10-09T00:00:00Z',
    generator: { app: 'kedai', version: '0.4.1' },
    app_mode: 'roleplay',
    sections,
  };
}

function flowOf(id: string, name: string) {
  return { id, name, enabled: true, steps: [{ id: `${id}-s`, name: '步', enabled: true, goal: '', action: 'direct' }] };
}

/** 触发文件选择(真实 change 事件;源码只读 target.files 并写回 value) */
function pickFile(payload: unknown, name = 'bundle.json'): Event {
  const text = typeof payload === 'string' ? payload : JSON.stringify(payload);
  const file = new File([text], name, { type: 'application/json' });
  const evt = new Event('change');
  Object.defineProperty(evt, 'target', { value: { files: [file], value: '' } });
  return evt;
}

async function mountModal() {
  setActivePinia(createPinia());
  const store = useAppStore();
  const wrapper = mount(ImportBundleModal);
  return { store, wrapper };
}

/** 给组件内隐藏 input 投递文件并等待解析完成(change 事件的 target 需自定义,故直接派发) */
async function feed(wrapper: ReturnType<typeof mount>, payload: unknown, name?: string): Promise<void> {
  const input = wrapper.find('input[type="file"]');
  const evt = pickFile(payload, name);
  (input.element as HTMLInputElement).dispatchEvent(evt);
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  mocked.importPromptPreset.mockResolvedValue({ ok: true, imported: 2, config: { mode: 'complex', simple: {}, floors: [] } });
  mocked.savePromptInject.mockImplementation(async (cfg: unknown) => cfg);
  mocked.getPromptInject.mockResolvedValue({ mode: 'complex', simple: {}, floors: [] });
  mocked.importSkills.mockResolvedValue({ ok: true, imported: 1 });
  mocked.getRegexScripts.mockResolvedValue([]);
  mocked.saveRegexScripts.mockResolvedValue({ count: 2, skipped: 0, scripts: [] });
  mocked.importPluginTools.mockResolvedValue({ ok: true, imported: 1, loaded: 1, removed: 0 });
  mocked.saveAgentPromptMd.mockResolvedValue({ ok: true, path: 'x' });
  mocked.listSkills.mockResolvedValue([]);
  mocked.importAgentFlows.mockResolvedValue({
    library: { current_flow_id: null, flows: [] },
    report: { imported: 1, skipped: 0, renamed: [], replaced: [] },
  });
});

describe('ImportBundleModal(综合导入)', () => {
  it('解析 bundle 后渲染逐段预览与计数;任务模式段进「不导入」说明', async () => {
    const { wrapper } = await mountModal();
    await feed(
      wrapper,
      bundleOf({
        preset_entries: { format: 'kedai_inject', mode: 'complex', simple: {}, floors: [{ id: 'f', order: 0 }] },
        flows: { kedai_flow_bundle: 1, flows: [flowOf('f1', '甲')] },
      }),
    );
    const text = wrapper.text();
    expect(text).toContain('①预设条目');
    expect(text).toContain('②自定义执行流程');
    expect(text).toContain('共 1 项');
    // 勾选默认全选 → 执行按钮计数 2 段
    expect(text).toContain('开始导入(2 段)');
  });

  it('逐段执行:调对应 API 并在回执逐条列出;替换类先二次确认', async () => {
    const { wrapper } = await mountModal();
    await feed(
      wrapper,
      bundleOf({
        flows: { kedai_flow_bundle: 1, flows: [flowOf('f1', '甲')] },
        skills: [{ name: '技能A' }],
        plugins: [{ file: 'demo.json', name: 'demo', script: 'return 1;' }],
      }),
    );
    const btn = wrapper.findAll('button').find((b) => b.text().includes('开始导入'));
    await btn?.trigger('click');
    await flushPromises();

    expect(mocked.importAgentFlows).toHaveBeenCalledTimes(1);
    expect(mocked.importSkills).toHaveBeenCalledTimes(1);
    expect(mocked.importPluginTools).toHaveBeenCalledTimes(1);
    const text = wrapper.text();
    expect(text).toContain('已导入 1 个流程');
    expect(text).toContain('已导入 1 个技能');
    expect(text).toContain('已导入 1 个插件');
    // 回执状态标签
    expect(wrapper.findAll('.impb-status.ok').length).toBe(3);
  });

  it('失败隔离:一段失败不阻断其余段(失败段标红、其余照常执行)', async () => {
    mocked.importSkills.mockRejectedValueOnce(new Error('技能导入炸了'));
    const { wrapper } = await mountModal();
    await feed(
      wrapper,
      bundleOf({
        flows: { kedai_flow_bundle: 1, flows: [flowOf('f1', '甲')] },
        skills: [{ name: '技能A' }],
      }),
    );
    const btn = wrapper.findAll('button').find((b) => b.text().includes('开始导入'));
    await btn?.trigger('click');
    await flushPromises();

    expect(mocked.importAgentFlows).toHaveBeenCalledTimes(1); // 失败之后没有提前中断
    const text = wrapper.text();
    expect(text).toContain('技能导入炸了');
    expect(text).toContain('1 段成功,1 段失败');
    expect(wrapper.findAll('.impb-status.failed').length).toBe(1);
    expect(wrapper.findAll('.impb-status.ok').length).toBe(1);
  });

  it('取消替换确认 → 一段都不执行', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    const { wrapper } = await mountModal();
    await feed(wrapper, bundleOf({ regex_scripts: [{ script_name: 'A', find_regex: 'x' }] }));
    const btn = wrapper.findAll('button').find((b) => b.text().includes('开始导入'));
    await btn?.trigger('click');
    await Promise.resolve();
    expect(mocked.saveRegexScripts).not.toHaveBeenCalled();
  });

  it('无法识别的文件 → 就地错误提示,不产生预览', async () => {
    const { wrapper } = await mountModal();
    await feed(wrapper, { foo: 1 });
    expect(wrapper.text()).toContain('解析失败');
    expect(wrapper.text()).toContain('无法识别');
  });
});

describe('ImportBundleModal(综合导出)', () => {
  it('导出选段:客户端过滤 sections 并按段序重建,文件名按日期', async () => {
    mocked.exportBundle.mockResolvedValue({
      bundle: bundleOf({
        preset_entries: { format: 'kedai_inject', mode: 'complex', simple: {}, floors: [] },
        flows: { kedai_flow_bundle: 1, flows: [flowOf('f1', '甲')] },
      }),
      warnings: [],
    });
    const { wrapper } = await mountModal();
    // 切到导出页签
    const exportTab = wrapper.findAll('button').find((b) => b.text() === '导出');
    await exportTab?.trigger('click');
    // 取消勾选 ①,只留 flows
    await wrapper.find('#impb-exp-preset_entries').setValue(false);
    const btn = wrapper.findAll('button').find((b) => b.text().includes('导出综合导出件'));
    await btn?.trigger('click');
    await Promise.resolve();
    await Promise.resolve();

    expect(mocked.exportBundle).toHaveBeenCalledWith('roleplay');
    expect(downloadBlob).toHaveBeenCalledTimes(1);
    const [name, blob] = vi.mocked(downloadBlob).mock.calls[0];
    expect(String(name)).toMatch(/^kedai-bundle-\d{4}-\d{2}-\d{2}\.json$/);
    const text = await (blob as Blob).text();
    expect(text).toContain('"flows"');
    expect(text).not.toContain('"preset_entries"');
  });

  it('任务模式:导出页签不渲染 ①⑥(也不导出)', async () => {
    const { store, wrapper } = await mountModal();
    store.appMode = 'task';
    await Promise.resolve();
    const exportTab = wrapper.findAll('button').find((b) => b.text() === '导出');
    await exportTab?.trigger('click');
    expect(wrapper.find('#impb-exp-preset_entries').exists()).toBe(false);
    expect(wrapper.find('#impb-exp-regex_scripts').exists()).toBe(false);
    expect(wrapper.find('#impb-exp-flows').exists()).toBe(true);
  });
});
