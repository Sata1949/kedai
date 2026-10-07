// @vitest-environment jsdom
// PluginsModal 组件测试(PLGM 1.4 补测):列表按来源文件分组、空态文案、重载反馈含注销计数、删除链路。
// 直接 import '../api/plugins' 是 ratchet「有测试」判据(barrel 不传递);组件经 '../api' 门面调用,
// vi.mock 按解析后的同一文件生效(门面是 export * 再导出)。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
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

vi.mock('../api/plugins', () => ({
  listPluginTools: vi.fn(),
  reloadPluginTools: vi.fn(),
  uploadPluginTool: vi.fn(),
  deletePluginTool: vi.fn(),
}));

import * as pluginApi from '../api/plugins';
import { useAppStore } from '../store';
import PluginsModal from './PluginsModal.vue';

const listMock = vi.mocked(pluginApi.listPluginTools);
const reloadMock = vi.mocked(pluginApi.reloadPluginTools);

async function mountModal() {
  setActivePinia(createPinia());
  const store = useAppStore();
  const wrapper = mount(PluginsModal);
  await Promise.resolve();
  await Promise.resolve();
  return { store, wrapper };
}

describe('PluginsModal(插件管理弹窗)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listMock.mockResolvedValue({ tools: [], files: [] });
  });

  it('已注册工具按来源文件分组,组头按文件名排序', async () => {
    listMock.mockResolvedValue({
      tools: [
        { name: 'b_tool', description: 'B 工具', origin: 'plugin', file: 'b.json' },
        { name: 'a_tool', description: 'A 工具', origin: 'plugin', file: 'a.json' },
        { name: 'a_tool2', description: 'A2 工具', origin: 'plugin', file: 'a.json' },
      ],
      files: ['a.json', 'b.json'],
    });
    const { wrapper } = await mountModal();
    await Promise.resolve();
    const text = wrapper.text();
    expect(text).toContain('a_tool');
    expect(text).toContain('a_tool2');
    expect(text).toContain('b_tool');
    // 组头 a.json 在前;b_tool 落在 b.json 组头之后(归属正确),a_tool 落在 a.json 之后
    expect(text.indexOf('a.json')).toBeLessThan(text.indexOf('b.json'));
    expect(text.indexOf('a_tool')).toBeGreaterThan(text.indexOf('a.json'));
    expect(text.indexOf('b_tool')).toBeGreaterThan(text.indexOf('b.json'));
  });

  it('空态文案说明内置与 MCP 工具不在列表,并指引示例目录', async () => {
    const { wrapper } = await mountModal();
    await Promise.resolve();
    const text = wrapper.text();
    expect(text).toContain('内置工具与 MCP 工具不在此列表');
    expect(text).toContain('examples/plugins/tools/');
  });

  it('重载反馈展示 loaded 与 removed 计数', async () => {
    reloadMock.mockResolvedValue({ ok: true, loaded: 2, removed: 1 });
    const { wrapper } = await mountModal();
    await Promise.resolve();
    const btn = wrapper.findAll('button').find((b) => b.text().includes('重载插件'));
    expect(btn).toBeTruthy();
    await btn!.trigger('click');
    await Promise.resolve();
    await Promise.resolve();
    expect(reloadMock).toHaveBeenCalledTimes(1);
    expect(wrapper.text()).toContain('已重载 2 个,注销 1 个');
  });

  it('删除文件走 API 并刷新列表', async () => {
    listMock.mockResolvedValue({ tools: [], files: ['x.json'] });
    vi.mocked(pluginApi.deletePluginTool).mockResolvedValue({ ok: true, removed: 'x.json' });
    vi.stubGlobal('confirm', vi.fn(() => true));
    const { wrapper } = await mountModal();
    await Promise.resolve();
    const btn = wrapper.findAll('button').find((b) => b.text() === '删除');
    expect(btn).toBeTruthy();
    await btn!.trigger('click');
    await Promise.resolve();
    await Promise.resolve();
    expect(pluginApi.deletePluginTool).toHaveBeenCalledWith('x.json');
    expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2); // 初次装载 + 删除后刷新
  });
});
