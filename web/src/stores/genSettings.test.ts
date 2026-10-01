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
    authorization_mode: 'loose',
    // HB-1 成本护栏:单次生成 token 预算(0 = 关闭)与超限动作
    session_token_budget: 0,
    session_budget_action: 'warn',
    // HB-2 语义熔断:窗口/下限/去重上限
    loop_guard_semantic_window: 16,
    loop_guard_semantic_min_calls: 12,
    loop_guard_semantic_max_distinct: 2,
    // 提交 3:任务侧两道闸(步骤墙钟预算默认 1200 = 开;空闲看守默认 900)
    task_step_budget_secs: 1200,
    task_idle_timeout_secs: 900,
    // HB-7:变量两步生成的独立模型/温度(默认未配置)
    mvu_model: null,
    mvu_temperature: null,
    bypass_blacklist: [],
    tool_authorization_timeout_secs: 300,
    task_tool_policy: 'deny_dangerous',
    task_tool_allowlist: [],
    max_tool_rounds: 32,
    // 流程调用闸与节点默认上下文(A 批 A3/A4;三者都是任务侧设置)
    max_flow_call_depth: 2,
    max_flow_calls_per_task: 8,
    default_node_max_context: 0,
    tool_history_keep_rounds: 4,
    tool_history_budget_tokens: 16384,
    render_html: false,
    compaction_mode: 'off',
    compaction_threshold: 0.8,
    compaction_keep_recent: 4,
    compaction_snip_bytes: 8192,
    llm_request_log: false,
    memory_distill_enabled: false,
    memory_inject_limit: 8,
    memory_inject_char_budget: 2000,
    memory_max_entries: 200,
    embedding_enabled: false,
    embedding_base_url: '',
    embedding_api_key_masked: '',
    has_embedding_api_key: false,
    embedding_model: '',
    embedding_dim: 0,
    skill_progressive_disclosure: true,
    subagent_max_depth: 2,
    subagent_max_concurrency: 6,
    subagent_result_max_chars: 2000,
    undo_enabled: true,
    mcp_enabled: false,
    mcp_servers: [],
    exec_enabled: false,
    exec_allow_root: false,
    exec_allow_shizuku: false,
    exec_allow_sandbox: false,
    vision_screenshot_enabled: false,
    task_coding_bundle_enabled: false,
    task_default_connection_id: '',
    // 多套连接(批次 4):默认空列表,具体连接由用例覆盖
    connections: [],
    active_connection_id: null,
    ...overrides,
  };
}

/**
 * 抹掉若干字段(模拟旧服务端/异常响应缺字段)。
 * 用 delete 而不是 `undefined as unknown as T`:后者属类型逃逸,会推高
 * `tools/check-frontend-lint.mjs` 的 ratchet 计数(该门禁只降不升)。
 */
function withoutFields(base: RuntimeSettings, keys: Array<keyof RuntimeSettings>): RuntimeSettings {
  const out: Partial<RuntimeSettings> = { ...base };
  for (const k of keys) delete out[k];
  return out as RuntimeSettings;
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

describe('genSettings 流程调用闸与节点默认上下文(A 批 A3/A4)', () => {
  beforeEach(() => {
    memStorage.clear();
    setActivePinia(createPinia());
  });

  it('store 默认值与后端缺省一致(深度 2 / 每任务 8 / 不裁剪 0)', () => {
    const store = useGenSettingsStore();
    expect(store.maxFlowCallDepth).toBe(2);
    expect(store.maxFlowCallsPerTask).toBe(8);
    expect(store.defaultNodeMaxContext).toBe(0);
  });

  it('loadSettings 回填服务端值;缺字段兜底 2 / 8 / 0', async () => {
    const store = useGenSettingsStore();
    getSettingsMock.mockReset().mockResolvedValue(
      makeSettings({
        max_flow_call_depth: 4,
        max_flow_calls_per_task: 32,
        default_node_max_context: 8192,
      }),
    );
    await store.loadSettings();
    expect(store.maxFlowCallDepth).toBe(4);
    expect(store.maxFlowCallsPerTask).toBe(32);
    expect(store.defaultNodeMaxContext).toBe(8192);

    // 旧服务端/异常响应缺字段:回退默认值(不裁剪 = 0,即 A 批之前的行为)
    getSettingsMock.mockReset().mockResolvedValue(
      withoutFields(makeSettings(), [
        'max_flow_call_depth',
        'max_flow_calls_per_task',
        'default_node_max_context',
      ]),
    );
    await store.loadSettings();
    expect(store.maxFlowCallDepth).toBe(2);
    expect(store.maxFlowCallsPerTask).toBe(8);
    expect(store.defaultNodeMaxContext).toBe(0);
  });

  it('saveSettings 响应回填三个字段(与 loadSettings 同口径)', async () => {
    const store = useGenSettingsStore();
    saveSettingsMock.mockReset().mockResolvedValue({
      ok: true,
      settings: makeSettings({
        max_flow_call_depth: 5,
        max_flow_calls_per_task: 64,
        default_node_max_context: 4096,
      }),
    });
    await store.saveSettings({
      max_flow_call_depth: 5,
      max_flow_calls_per_task: 64,
      default_node_max_context: 4096,
    });
    expect(store.maxFlowCallDepth).toBe(5);
    expect(store.maxFlowCallsPerTask).toBe(64);
    expect(store.defaultNodeMaxContext).toBe(4096);
  });
});

describe('genSettings 任务侧两道闸(提交 3 · D3/D7)', () => {
  beforeEach(() => {
    memStorage.clear();
    setActivePinia(createPinia());
  });

  it('store 默认值与后端缺省一致(步骤预算 1200 / 空闲超时 900)', () => {
    const store = useGenSettingsStore();
    expect(store.taskStepBudgetSecs).toBe(1200);
    expect(store.taskIdleTimeoutSecs).toBe(900);
  });

  it('loadSettings 回填服务端值;缺字段兜底 1200 / 900', async () => {
    const store = useGenSettingsStore();
    getSettingsMock.mockReset().mockResolvedValue(
      makeSettings({
        task_step_budget_secs: 300,
        task_idle_timeout_secs: 601,
      }),
    );
    await store.loadSettings();
    expect(store.taskStepBudgetSecs).toBe(300);
    expect(store.taskIdleTimeoutSecs).toBe(601);

    // 旧服务端缺字段:回退默认(而非 0——0 是「关」,不能把缺字段当用户关掉了闸门)
    getSettingsMock.mockReset().mockResolvedValue(
      withoutFields(makeSettings(), ['task_step_budget_secs', 'task_idle_timeout_secs']),
    );
    await store.loadSettings();
    expect(store.taskStepBudgetSecs).toBe(1200);
    expect(store.taskIdleTimeoutSecs).toBe(900);
  });

  it('saveSettings 响应回填两个字段(与 loadSettings 同口径)', async () => {
    const store = useGenSettingsStore();
    saveSettingsMock.mockReset().mockResolvedValue({
      ok: true,
      settings: makeSettings({
        task_step_budget_secs: 0,
        task_idle_timeout_secs: 0,
      }),
    });
    await store.saveSettings({ task_step_budget_secs: 0, task_idle_timeout_secs: 0 });
    // 0 = 关,是合法用户选择:回填必须如实为 0(不得用 `?? 1200` 之类的兜底吃掉它)
    expect(store.taskStepBudgetSecs).toBe(0);
    expect(store.taskIdleTimeoutSecs).toBe(0);
  });
});

describe('genSettings 记忆槽预算与容量上限(B2/B3)', () => {
  beforeEach(() => {
    memStorage.clear();
    setActivePinia(createPinia());
  });

  it('loadSettings 回填服务端值;缺字段兜底 2000 / 200', async () => {
    const store = useGenSettingsStore();
    getSettingsMock.mockReset().mockResolvedValue(
      makeSettings({ memory_inject_char_budget: 4000, memory_max_entries: 500 }),
    );
    await store.loadSettings();
    expect(store.memoryInjectCharBudget).toBe(4000);
    expect(store.memoryMaxEntries).toBe(500);

    // 旧服务端/异常响应缺字段:回退默认值
    getSettingsMock.mockReset().mockResolvedValue(
      makeSettings({
        memory_inject_char_budget: undefined as unknown as number,
        memory_max_entries: undefined as unknown as number,
      }),
    );
    await store.loadSettings();
    expect(store.memoryInjectCharBudget).toBe(2000);
    expect(store.memoryMaxEntries).toBe(200);
  });

  it('saveSettings 响应回填两个字段(与 loadSettings 同口径)', async () => {
    const store = useGenSettingsStore();
    saveSettingsMock.mockReset().mockResolvedValue({
      ok: true,
      settings: makeSettings({ memory_inject_char_budget: 0, memory_max_entries: 0 }),
    });
    await store.saveSettings({ memory_inject_char_budget: 0, memory_max_entries: 0 });
    expect(store.memoryInjectCharBudget).toBe(0);
    expect(store.memoryMaxEntries).toBe(0);
  });
});

describe('genSettings 编码能力包开关(task_coding_bundle_enabled)', () => {
  beforeEach(() => {
    memStorage.clear();
    setActivePinia(createPinia());
  });

  it('loadSettings 回填服务端值;缺字段兜底 false(默认关)', async () => {
    const store = useGenSettingsStore();
    getSettingsMock.mockReset().mockResolvedValue(makeSettings({ task_coding_bundle_enabled: true }));
    await store.loadSettings();
    expect(store.taskCodingBundleEnabled).toBe(true);

    // 旧服务端/异常响应缺字段:不得污染 store,回退默认关
    // (抹字段用 withoutFields,避免 `as unknown as` 推高 check-frontend-lint 的 ratchet 基线)
    getSettingsMock.mockReset().mockResolvedValue(
      withoutFields(makeSettings(), ['task_coding_bundle_enabled']),
    );
    await store.loadSettings();
    expect(store.taskCodingBundleEnabled).toBe(false);
  });

  it('saveSettings 响应回填 taskCodingBundleEnabled(与 loadSettings 同口径)', async () => {
    const store = useGenSettingsStore();
    saveSettingsMock.mockReset().mockResolvedValue({
      ok: true,
      settings: makeSettings({ task_coding_bundle_enabled: true }),
    });
    await store.saveSettings({ task_coding_bundle_enabled: true });
    expect(saveSettingsMock).toHaveBeenCalledWith({ task_coding_bundle_enabled: true }, 'roleplay');
    expect(store.taskCodingBundleEnabled).toBe(true);
  });
});

describe('genSettings 授权模式三档(2026-09 授权改造)', () => {
  beforeEach(() => {
    memStorage.clear();
    setActivePinia(createPinia());
  });

  it('loadSettings 读取 authorization_mode 三档', async () => {
    const store = useGenSettingsStore();
    for (const mode of ['strict', 'loose', 'bypass'] as const) {
      getSettingsMock.mockReset().mockResolvedValue(makeSettings({ authorization_mode: mode }));
      await store.loadSettings();
      expect(store.authorizationMode).toBe(mode);
    }
  });

  it('旧配置只有 bypass_mode 时映射:true→bypass,false→strict', async () => {
    const store = useGenSettingsStore();
    // 模拟旧服务端:authorization_mode 缺失,仅 bypass_mode
    getSettingsMock.mockReset().mockResolvedValue(
      makeSettings({
        authorization_mode: undefined as unknown as 'loose',
        bypass_mode: true,
      }),
    );
    await store.loadSettings();
    expect(store.authorizationMode).toBe('bypass');

    getSettingsMock.mockReset().mockResolvedValue(
      makeSettings({
        authorization_mode: undefined as unknown as 'loose',
        bypass_mode: false,
      }),
    );
    await store.loadSettings();
    expect(store.authorizationMode).toBe('strict');
  });

  it('setAuthorizationMode 持久化 authorization_mode 并回填', async () => {
    const store = useGenSettingsStore();
    saveSettingsMock.mockReset().mockResolvedValue({
      ok: true,
      settings: makeSettings({ authorization_mode: 'strict' }),
    });
    await store.setAuthorizationMode('strict');
    expect(saveSettingsMock).toHaveBeenCalledWith({ authorization_mode: 'strict' }, 'roleplay');
    expect(store.authorizationMode).toBe('strict');
  });

  it('setAuthorizationMode 失败时回滚本地值', async () => {
    const store = useGenSettingsStore();
    store.authorizationMode = 'loose';
    saveSettingsMock.mockReset().mockRejectedValue(new Error('网络错误'));
    await store.setAuthorizationMode('bypass');
    expect(store.authorizationMode).toBe('loose');
  });

  it('loadSettings 回填始终需授权清单、超时与任务工具策略', async () => {
    const store = useGenSettingsStore();
    getSettingsMock.mockReset().mockResolvedValue(
      makeSettings({
        bypass_blacklist: ['write', 'replace'],
        tool_authorization_timeout_secs: 120,
        task_tool_policy: 'all',
        task_tool_allowlist: ['read'],
      }),
    );
    await store.loadSettings();
    expect(store.authorizationAlwaysRequired).toEqual(['write', 'replace']);
    expect(store.toolAuthorizationTimeoutSecs).toBe(120);
    expect(store.taskToolPolicy).toBe('all');
    expect(store.taskToolAllowlist).toEqual(['read']);
  });
});
