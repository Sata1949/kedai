// @vitest-environment jsdom
// 附件管理弹窗测试(IMPHUB 步骤 D):四页签渲染、文件夹过滤、移动/删除/启停接线、
// 预设正则页签无文件夹树。虚拟文件夹纯函数在 utils/assetFolders.test.ts 全量覆盖,
// 本文件只管组件行为(页签切换、数据源切换、API 调用与参数)。
//
// 直接 import '../api' 是 ratchet「有测试」判据(barrel 不传递);组件经 '../api' 门面调用,
// vi.mock 按解析后的同一文件生效。
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
  get length() {
    return memStorage.size;
  },
});

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    listSkills: vi.fn().mockResolvedValue([]),
    updateSkill: vi.fn().mockResolvedValue({ ok: true, skill: {} }),
    deleteSkill: vi.fn().mockResolvedValue({ ok: true }),
    listPluginTools: vi.fn().mockResolvedValue({ tools: [], files: [] }),
    movePluginTool: vi.fn().mockResolvedValue({ ok: true, file: '', folder: '' }),
    deletePluginTool: vi.fn().mockResolvedValue({ ok: true, removed: '' }),
    reloadPluginTools: vi.fn().mockResolvedValue({ ok: true, loaded: 0, removed: 0 }),
    getAgentFlow: vi
      .fn()
      .mockResolvedValue({ library: { current_flow_id: null, flows: [] }, config: null }),
    getRegexScripts: vi.fn().mockResolvedValue([]),
    saveRegexScripts: vi.fn().mockResolvedValue({ ok: true, scripts: [], count: 0, skipped: 0 }),
  };
});

import * as api from '../api';
import { useAppStore } from '../store';
import AssetsModal from './AssetsModal.vue';

async function mountModal() {
  setActivePinia(createPinia());
  const store = useAppStore();
  const wrapper = mount(AssetsModal);
  await flushPromises();
  return { store, wrapper };
}

function skill(id: string, name: string, folder: string): api.SkillRecord {
  return {
    id,
    name,
    description: '',
    content: '',
    enabled: true,
    created_at: '',
    allowed_tools: [],
    run_as_subagent: false,
    model: '',
    folder,
  };
}

describe('AssetsModal(附件管理)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    memStorage.clear();
  });

  it('四页签按序渲染;默认技能页签带文件夹树', async () => {
    const { wrapper } = await mountModal();
    const tabs = wrapper.findAll('.sv-mode button').map((b) => b.text());
    expect(tabs).toEqual(['技能', '流程', '插件', '预设正则']);
    expect(wrapper.find('.asset-folders').exists()).toBe(true);
    expect(wrapper.text()).toContain('未分组');
  });

  it('技能列表按选中文件夹过滤;「移动到…」调 updateSkill 带 folder', async () => {
    vi.mocked(api.listSkills).mockResolvedValue([skill('s1', '写作', 'A'), skill('s2', '翻译', '')]);
    const { wrapper } = await mountModal();

    // 默认选中「未分组」:只显示 s2
    expect(wrapper.text()).toContain('翻译');
    expect(wrapper.text()).not.toContain('写作');

    // 切到 A 文件夹:只显示 s1
    const folderA = wrapper.findAll('.asset-folder').find((b) => b.text().includes('A'));
    expect(folderA, '文件夹树应出现 A').toBeTruthy();
    await folderA?.trigger('click');
    expect(wrapper.text()).toContain('写作');
    expect(wrapper.text()).not.toContain('翻译');

    // 移动到…未分组
    await wrapper.find('.asset-item select').setValue('__root__');
    await flushPromises();
    expect(api.updateSkill).toHaveBeenCalledWith('s1', { folder: '' });
  });

  it('技能删除:确认后调 deleteSkill', async () => {
    vi.mocked(api.listSkills).mockResolvedValue([skill('s1', '写作', '')]);
    vi.stubGlobal('confirm', vi.fn(() => true));
    const { wrapper } = await mountModal();

    const del = wrapper.findAll('.asset-item button').find((b) => b.text() === '删除');
    await del?.trigger('click');
    await flushPromises();
    expect(api.deleteSkill).toHaveBeenCalledWith('s1');
  });

  it('插件页签:按文件夹过滤,删除调 deletePluginTool', async () => {
    vi.mocked(api.listPluginTools).mockResolvedValue({
      tools: [{ name: 't1', description: '工具', origin: 'plugin', file: 'a.json', folder: '工具' }],
      files: ['a.json'],
    });
    vi.stubGlobal('confirm', vi.fn(() => true));
    const { wrapper } = await mountModal();

    // 切到插件页签(索引 2)
    await wrapper.findAll('.sv-mode button')[2].trigger('click');
    await flushPromises();
    const folder = wrapper.findAll('.asset-folder').find((b) => b.text().includes('工具'));
    expect(folder, '插件页签应出现「工具」文件夹').toBeTruthy();
    await folder?.trigger('click');
    expect(wrapper.text()).toContain('a.json');

    const del = wrapper.findAll('.asset-item button').find((b) => b.text() === '删除');
    await del?.trigger('click');
    await flushPromises();
    expect(api.deletePluginTool).toHaveBeenCalledWith('a.json');
  });

  it('预设正则页签:无文件夹树;停用调 saveRegexScripts', async () => {
    vi.mocked(api.getRegexScripts).mockResolvedValue([
      {
        id: 'r1',
        script_name: '去星号',
        find_regex: '\\*',
        replace_string: '',
        markdown_only: false,
        enabled: true,
      },
    ]);
    vi.mocked(api.saveRegexScripts).mockResolvedValue({
      scripts: [],
      count: 1,
      skipped: 0,
    });
    const { wrapper } = await mountModal();

    // 切到预设正则页签(索引 3)
    await wrapper.findAll('.sv-mode button')[3].trigger('click');
    await flushPromises();
    // 正则无 folder:不渲染文件夹树
    expect(wrapper.find('.asset-folders').exists()).toBe(false);
    expect(wrapper.text()).toContain('去星号');

    const btn = wrapper.findAll('.asset-item button').find((b) => b.text() === '停用');
    await btn?.trigger('click');
    await flushPromises();
    expect(api.saveRegexScripts).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(api.saveRegexScripts).mock.calls[0][0] as api.RegexScript[];
    expect(sent[0].enabled).toBe(false);
  });
});
