import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

// node 环境无 localStorage,store 初始化即访问(appMode / 渲染记忆等),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

import { useAgentPromptEditor } from './useAgentPromptEditor';
import { useAppStore } from '../store';
import * as api from '../api';

// 只桩掉预览请求,其余 api 原样(store 门面引用众多,不能整体替身)
vi.spyOn(api, 'getPromptPreview').mockResolvedValue({ ok: true, note: '', layers: [], warnings: [] });

function previewOf(mode: 'roleplay' | 'task', source: string) {
  return {
    ok: true,
    mode,
    note: '',
    layers: [{ source, role: 'system', layer: 5, order: 0, content: source }],
    warnings: [] as string[],
  };
}

describe('useAgentPromptEditor', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(api.getPromptPreview).mockReset();
    vi.mocked(api.getPromptPreview).mockResolvedValue({ ok: true, note: '', layers: [], warnings: [] });
  });

  it('loadPromptPreview 把当前 appMode 传给预览接口', async () => {
    const store = useAppStore();
    store.appMode = 'task';
    const ed = useAgentPromptEditor();
    await ed.loadPromptPreview();
    expect(api.getPromptPreview).toHaveBeenCalledWith(undefined, undefined, 'task');
  });

  it('appMode 变更后再次加载传新模式(预览随模式切换的数据面)', async () => {
    const store = useAppStore();
    store.appMode = 'roleplay';
    const ed = useAgentPromptEditor();
    await ed.loadPromptPreview();
    expect(api.getPromptPreview).toHaveBeenLastCalledWith(undefined, undefined, 'roleplay');
    store.appMode = 'task';
    await ed.loadPromptPreview();
    expect(api.getPromptPreview).toHaveBeenLastCalledWith(undefined, undefined, 'task');
  });

  // ===== RTSPLIT-2:切模式瞬间的三类竞态 =====

  it('请求在飞时切模式:丢弃旧响应并尾随重拉(此前被 loading 早退静默吞掉)', async () => {
    const store = useAppStore();
    store.appMode = 'roleplay';
    const ed = useAgentPromptEditor();

    let releaseOld: (v: unknown) => void = () => {};
    vi.mocked(api.getPromptPreview)
      .mockImplementationOnce(() => new Promise((r) => { releaseOld = r as (v: unknown) => void; }))
      .mockResolvedValueOnce(previewOf('task', 'task_layer'));

    const first = ed.loadPromptPreview();
    // 在飞期间切模式并请求:旧实现的 `if (loading) return` 会在这里丢请求
    store.appMode = 'task';
    const second = ed.loadPromptPreview();

    // 旧请求此刻才回来,带的是 roleplay 的结果
    releaseOld(previewOf('roleplay', 'roleplay_layer'));
    await Promise.all([first, second]);

    // 最终必须落在任务模式那份上,且旧结果从未被写进 promptPreview
    expect(ed.promptPreview.value?.layers[0]?.source).toBe('task_layer');
    expect(ed.promptPreview.value?.mode).toBe('task');
    // 两次请求都真的发出去了(尾随重拉存在)
    expect(api.getPromptPreview).toHaveBeenCalledTimes(2);
    expect(api.getPromptPreview).toHaveBeenLastCalledWith(undefined, undefined, 'task');
  });

  it('服务端回显的 mode 与发起时不一致时不写回,并补拉一次', async () => {
    const store = useAppStore();
    store.appMode = 'task';
    const ed = useAgentPromptEditor();

    // 服务端错回了 roleplay(例如反代缓存/旧服务端),不能当成任务模式的结果展示
    vi.mocked(api.getPromptPreview)
      .mockResolvedValueOnce(previewOf('roleplay', 'wrong_mode_layer'))
      .mockResolvedValueOnce(previewOf('task', 'right_mode_layer'));

    await ed.loadPromptPreview();

    expect(ed.promptPreview.value?.layers[0]?.source).toBe('right_mode_layer');
    expect(api.getPromptPreview).toHaveBeenCalledTimes(2);
  });

  it('旧服务端不带 mode 字段时不因缺字段而丢弃结果(向后兼容)', async () => {
    const store = useAppStore();
    store.appMode = 'roleplay';
    const ed = useAgentPromptEditor();
    vi.mocked(api.getPromptPreview).mockResolvedValueOnce({
      ok: true, note: '', layers: [{ source: 'legacy', role: 'system', layer: 5, order: 0, content: 'x' }], warnings: [],
    });

    await ed.loadPromptPreview();

    expect(ed.promptPreview.value?.layers[0]?.source).toBe('legacy');
    expect(api.getPromptPreview).toHaveBeenCalledTimes(1);
  });

  // ===== RTSPLIT-1:运行时主提示词文件按模式加载 =====

  it('loadAgentPromptMd 按当前模式读对应文件,并记录内容属于哪个模式', async () => {
    const store = useAppStore();
    const load = vi.spyOn(api, 'getAgentPromptMd').mockResolvedValue({ path: 'p', content: '任务主提示词' });
    store.appMode = 'task';
    const ed = useAgentPromptEditor();

    await ed.loadAgentPromptMd();

    expect(load).toHaveBeenCalledWith('task');
    expect(ed.agentPromptMd.value).toBe('任务主提示词');
    expect(ed.agentPromptMode.value).toBe('task');
  });

  it('加载期间切走模式时丢弃旧模式的正文(否则文本框停在上一模式)', async () => {
    const store = useAppStore();
    let release: (v: { path: string; content: string }) => void = () => {};
    vi.spyOn(api, 'getAgentPromptMd').mockImplementationOnce(
      () => new Promise((r) => { release = r; }),
    );
    store.appMode = 'roleplay';
    const ed = useAgentPromptEditor();
    ed.agentPromptMd.value = '初始';

    const pending = ed.loadAgentPromptMd();
    store.appMode = 'task';
    release({ path: 'p', content: '角色扮演主提示词' });
    await pending;

    expect(ed.agentPromptMd.value).toBe('初始');
    expect(ed.agentPromptMode.value).toBeNull();
  });

  it('saveAgentPromptMd 按发起时模式写入,成功后刷新已加载快照', async () => {
    const store = useAppStore();
    const save = vi.spyOn(api, 'saveAgentPromptMd').mockResolvedValue({ ok: true, path: '任务文件', mode: 'task' });
    store.appMode = 'task';
    const ed = useAgentPromptEditor();
    ed.agentPromptMd.value = '新任务主提示词';

    await ed.saveAgentPromptMd();

    expect(save).toHaveBeenCalledWith('新任务主提示词', 'task');
    expect(ed.agentPromptLoaded.value).toBe('新任务主提示词');
    expect(ed.mdMsg.value?.kind).toBe('ok');
  });
});
