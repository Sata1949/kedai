// @vitest-environment jsdom
// McpSection 组件测试(PLGM 3.2):状态徽章/失败原因/工具数/重连启停链路;保存文案。
// 直接 import '../../api/mcp' 是 ratchet「有测试」判据(barrel 不传递)。
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

vi.mock('../../api/mcp', () => ({
  listMcpServers: vi.fn(),
  restartMcpServer: vi.fn(),
  stopMcpServer: vi.fn(),
  startMcpServer: vi.fn(),
}));

import * as mcpApi from '../../api/mcp';
import { useAppStore } from '../../store';
import McpSection from './McpSection.vue';

const listMock = vi.mocked(mcpApi.listMcpServers);

function serverStatus(over: Partial<mcpApi.McpServerStatus> = {}): mcpApi.McpServerStatus {
  return {
    name: 'fs',
    enabled: true,
    state: 'running',
    tool_count: 2,
    tools: ['mcp_fs_a', 'mcp_fs_b'],
    last_error: null,
    ...over,
  };
}

async function mountSection() {
  setActivePinia(createPinia());
  const store = useAppStore();
  store.mcpEnabled = true;
  store.mcpServers = [{ name: 'fs', command: 'npx', args: [], enabled: true }];
  const wrapper = mount(McpSection);
  await Promise.resolve();
  await Promise.resolve();
  return { store, wrapper };
}

describe('McpSection(MCP 服务区,PLGM 3.2)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listMock.mockResolvedValue({ servers: [], mcp_enabled: true });
  });

  it('运行中:显示状态徽章与工具数,提供停止/重连', async () => {
    listMock.mockResolvedValue({ servers: [serverStatus()], mcp_enabled: true });
    const { wrapper } = await mountSection();
    await Promise.resolve();
    const text = wrapper.text();
    expect(text).toContain('运行中');
    expect(text).toContain('工具 2 个');
    expect(text).toContain('重连');
    expect(text).toContain('停止');
  });

  it('失败:显示失败徽章与 last_error,提供启动', async () => {
    listMock.mockResolvedValue({
      servers: [
        serverStatus({ state: 'failed', tool_count: 0, tools: [], last_error: '未找到命令 npx' }),
      ],
      mcp_enabled: true,
    });
    const { wrapper } = await mountSection();
    await Promise.resolve();
    const text = wrapper.text();
    expect(text).toContain('失败');
    expect(text).toContain('未找到命令 npx');
    expect(text).toContain('启动');
  });

  it('已禁用:总开关关时徽章显示已禁用', async () => {
    listMock.mockResolvedValue({
      servers: [serverStatus({ state: 'disabled', enabled: false })],
      mcp_enabled: false,
    });
    const { wrapper } = await mountSection();
    await Promise.resolve();
    expect(wrapper.text()).toContain('已禁用');
  });

  it('点「重连」调用重启 API 并刷新状态', async () => {
    listMock.mockResolvedValue({ servers: [serverStatus()], mcp_enabled: true });
    vi.mocked(mcpApi.restartMcpServer).mockResolvedValue({
      ok: true,
      name: 'fs',
      state: 'running',
      tool_count: 2,
    });
    const { wrapper } = await mountSection();
    await Promise.resolve();
    const btn = wrapper.findAll('button').find((b) => b.text() === '重连');
    expect(btn).toBeTruthy();
    await btn!.trigger('click');
    await Promise.resolve();
    await Promise.resolve();
    expect(mcpApi.restartMcpServer).toHaveBeenCalledWith('fs');
    expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it('保存按钮调用 saveSettings 且文案提示「重连」生效', async () => {
    const { store, wrapper } = await mountSection();
    await Promise.resolve();
    const spy = vi.spyOn(store, 'saveSettings').mockResolvedValue(undefined);
    const btn = wrapper.findAll('button').find((b) => b.text() === '保存 MCP 设置');
    expect(btn).toBeTruthy();
    await btn!.trigger('click');
    await Promise.resolve();
    await Promise.resolve();
    expect(spy).toHaveBeenCalled();
    expect(wrapper.text()).toContain('已保存,点该行「重连」即生效');
  });
});
