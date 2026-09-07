// genSettings 模式分流测试:loadSettings/saveSettings 按 useTaskStore().appMode
// 向 api 层传 mode 参数(task / roleplay 两套提示词在服务端独立存储),
// 且 appMode 切换后再次调用参数跟随切换。mock 方式:vi.mock 替换 api 模块的
// getSettings/saveSettings(保留其余导出),断言调用入参。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useGenSettingsStore } from './genSettings';
import { useTaskStore } from './task';
import * as api from '../api';
import type { RuntimeSettings } from '../api';

// node 环境无 localStorage(task store setup 即读 appMode 持久化),补内存桩(同 task.test.ts)
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
    getSettings: vi.fn(),
    saveSettings: vi.fn(),
  };
});

const getSettingsMock = vi.mocked(api.getSettings);
const saveSettingsMock = vi.mocked(api.saveSettings);

/** 构造最小可用的 RuntimeSettings 响应(仅关心本测试断言字段,其余取默认) */
function makeSettings(overrides: Partial<RuntimeSettings> = {}): RuntimeSettings {
  return {
    openai_base_url: '',
    api_key_masked: '',
    has_api_key: false,
    model: 'test-model',
    default_temperature: 0.8,
    default_top_p: 0.9,
    default_max_tokens: 1024,
    max_context_tokens: 65536,
    agent_system_prompt: '',
    search_endpoint: '',
    mvu_vars_position: 'system',
    reflect_prompt: '',
    preset_tail_prompt: '',
    preset_tail_role: 'user',
    reflect_advice_prompt: '',
    reflect_advice_role: 'user',
    bypass_mode: false,
    bypass_blacklist: [],
    max_tool_rounds: 32,
    render_html: false,
    compaction_mode: 'off',
    compaction_threshold: 0.8,
    compaction_keep_recent: 4,
    compaction_snip_bytes: 8192,
    llm_request_log: false,
    memory_distill_enabled: false,
    memory_inject_limit: 8,
    skill_progressive_disclosure: true,
    subagent_max_depth: 2,
    subagent_max_concurrency: 6,
    subagent_result_max_chars: 2000,
    undo_enabled: true,
    mcp_enabled: false,
    mcp_servers: [],
    task_persona_full: false,
    ...overrides,
  };
}

describe('genSettings 模式分流:loadSettings/saveSettings 按 appMode 传 mode 参数', () => {
  beforeEach(() => {
    memStorage.clear();
    setActivePinia(createPinia());
    getSettingsMock.mockReset().mockResolvedValue(makeSettings());
    saveSettingsMock.mockReset().mockResolvedValue({ ok: true, settings: makeSettings() });
  });

  it('appMode=task 时 loadSettings 与 saveSettings 均带 mode=task', async () => {
    const task = useTaskStore();
    // 直接写 ref,避开 setAppMode 的任务列表加载等副作用(本测试只关注 mode 参数)
    task.appMode = 'task';
    const store = useGenSettingsStore();

    await store.loadSettings();
    expect(getSettingsMock).toHaveBeenCalledWith('task');

    await store.saveSettings({ agent_system_prompt: '任务专用词' });
    expect(saveSettingsMock).toHaveBeenCalledWith({ agent_system_prompt: '任务专用词' }, 'task');
  });

  it("appMode=roleplay 时 loadSettings 与 saveSettings 均带 mode=roleplay", async () => {
    const task = useTaskStore();
    task.appMode = 'roleplay';
    const store = useGenSettingsStore();

    await store.loadSettings();
    expect(getSettingsMock).toHaveBeenCalledWith('roleplay');

    await store.saveSettings({ agent_system_prompt: '人设词' });
    expect(saveSettingsMock).toHaveBeenCalledWith({ agent_system_prompt: '人设词' }, 'roleplay');
  });

  it('切换 appMode 后再次 loadSettings,mode 参数跟随切换', async () => {
    const task = useTaskStore();
    const store = useGenSettingsStore();

    task.appMode = 'task';
    await store.loadSettings();
    expect(getSettingsMock).toHaveBeenLastCalledWith('task');

    task.appMode = 'roleplay';
    await store.loadSettings();
    expect(getSettingsMock).toHaveBeenLastCalledWith('roleplay');
    expect(getSettingsMock).toHaveBeenCalledTimes(2);

    task.appMode = 'task';
    await store.loadSettings();
    expect(getSettingsMock).toHaveBeenLastCalledWith('task');
  });
});

describe('genSettings 执行者人设开关(R3a task_persona_full)', () => {
  beforeEach(() => {
    memStorage.clear();
    setActivePinia(createPinia());
  });

  it('loadSettings 回填服务端值;缺字段(null/undefined)兜底 false(精简)', async () => {
    const store = useGenSettingsStore();
    getSettingsMock.mockReset().mockResolvedValue(makeSettings({ task_persona_full: true }));
    await store.loadSettings();
    expect(store.taskPersonaFull).toBe(true);

    // 旧服务端/异常响应缺字段:不得污染 store,回退精简
    getSettingsMock.mockReset().mockResolvedValue(
      makeSettings({ task_persona_full: undefined as unknown as boolean }),
    );
    await store.loadSettings();
    expect(store.taskPersonaFull).toBe(false);
  });

  it('saveSettings 响应回填 taskPersonaFull(与 loadSettings 同口径)', async () => {
    const store = useGenSettingsStore();
    saveSettingsMock.mockReset().mockResolvedValue({
      ok: true,
      settings: makeSettings({ task_persona_full: true }),
    });
    await store.saveSettings({ task_persona_full: true });
    expect(saveSettingsMock).toHaveBeenCalledWith({ task_persona_full: true }, 'roleplay');
    expect(store.taskPersonaFull).toBe(true);
  });
});
