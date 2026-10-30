import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useUiPrefsStore } from './uiPrefs';

// uiPrefs:Agent 面板「自动展开一次,收起后不再弹」语义。
// 回归点:autoOpenAgentPanel 在用户主动收起后必须失效,复位后恢复。

// node 环境无 localStorage(store 初始化即访问),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

beforeEach(() => {
  memStorage.clear();
  setActivePinia(createPinia());
});

describe('uiPrefs Agent 面板开合语义', () => {
  it('默认收起,autoOpenAgentPanel 在未抑制时展开', () => {
    const store = useUiPrefsStore();
    expect(store.agentPanelOpen).toBe(false);
    store.autoOpenAgentPanel();
    expect(store.agentPanelOpen).toBe(true);
  });

  it('用户收起后,自动展开失效(收起后不再弹)', () => {
    const store = useUiPrefsStore();
    store.autoOpenAgentPanel();
    expect(store.agentPanelOpen).toBe(true);

    store.collapseAgentPanel();
    expect(store.agentPanelOpen).toBe(false);
    expect(store.agentPanelAutoSuppressed).toBe(true);

    // 后续每次生成开始的自动展开都被抑制
    store.autoOpenAgentPanel();
    expect(store.agentPanelOpen).toBe(false);
  });

  it('用户显式展开会清除抑制,下一次生成仍可自动展开', () => {
    const store = useUiPrefsStore();
    store.collapseAgentPanel();
    expect(store.agentPanelAutoSuppressed).toBe(true);

    store.openAgentPanel();
    expect(store.agentPanelOpen).toBe(true);
    expect(store.agentPanelAutoSuppressed).toBe(false);

    store.collapseAgentPanel();
    store.openAgentPanel(); // 再次显式展开
    store.collapseAgentPanel();
    store.resetAgentPanelAutoSuppress(); // 新会话复位
    store.autoOpenAgentPanel();
    expect(store.agentPanelOpen).toBe(true);
  });

  it('toggleAgentPanel 依据当前态开合,收起时记抑制', () => {
    const store = useUiPrefsStore();
    store.toggleAgentPanel();
    expect(store.agentPanelOpen).toBe(true);
    expect(store.agentPanelAutoSuppressed).toBe(false);

    store.toggleAgentPanel();
    expect(store.agentPanelOpen).toBe(false);
    expect(store.agentPanelAutoSuppressed).toBe(true);
  });

  it('overlayOpen 汇总模态/抽屉:任一打开即暂停环境层', () => {
    const store = useUiPrefsStore();
    expect(store.overlayOpen).toBe(false);

    store.settingsOpen = true;
    expect(store.overlayOpen).toBe(true);
    store.settingsOpen = false;
    expect(store.overlayOpen).toBe(false);

    store.sidebarOpen = true;
    expect(store.overlayOpen).toBe(true);
    store.sidebarOpen = false;

    store.openAgentPanel();
    expect(store.overlayOpen).toBe(true);
    store.collapseAgentPanel();
    expect(store.overlayOpen).toBe(false);
  });
});

describe('uiPrefs 双库分叉提示(DB-7)', () => {
  it('检测到另一套库时置位,并保留当前目录与另一套路径', () => {
    const store = useUiPrefsStore();
    expect(store.splitDataDirs).toBeNull();
    store.setSplitDataDirs({
      data_dir: 'C:\\a',
      others: [{ path: 'D:\\b', evidence: 'settings.json + 数据库' }],
    });
    expect(store.splitDataDirs).not.toBeNull();
    expect(store.splitDataDirs?.dataDir).toBe('C:\\a');
    expect(store.splitDataDirs?.others[0].path).toBe('D:\\b');
  });

  it('others 为空时保持 null(无分叉不弹提示)', () => {
    const store = useUiPrefsStore();
    store.setSplitDataDirs({ data_dir: 'C:\\a', others: [] });
    expect(store.splitDataDirs).toBeNull();
  });

  it('关闭后置 null(本会话不再显示)', () => {
    const store = useUiPrefsStore();
    store.setSplitDataDirs({ data_dir: 'C:\\a', others: [{ path: 'D:\\b', evidence: 'e' }] });
    store.dismissSplitDataDirs();
    expect(store.splitDataDirs).toBeNull();
  });
});
