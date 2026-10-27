import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useAppStore } from '../store';
import { TASK_PARAM_TIERS } from '../stores/genSettings';
import { useDataManager, useGenerationParams } from './useDataManager';

// useDataManager / useGenerationParams 装配测试(批次 6.1b undo_enabled):
// 装载(getSettings → store.undoEnabled)与保存(保存 patch 含 undo_enabled)两条链路。
// mock 范式参照 stores/task.test.ts(../api 整体 mock + hoisted 句柄)。

// node 环境无 localStorage(子 store 初始化即访问),补内存桩
const memStorage = new Map<string, string>();
vi.stubGlobal('localStorage', {
  getItem: (k: string) => memStorage.get(k) ?? null,
  setItem: (k: string, v: string) => void memStorage.set(k, String(v)),
  removeItem: (k: string) => void memStorage.delete(k),
  clear: () => memStorage.clear(),
  key: (i: number) => [...memStorage.keys()][i] ?? null,
  get length() { return memStorage.size; },
});

/** mock 控制句柄 */
const h = vi.hoisted(() => ({
  /** getSettings 返回的设置体(装载链路) */
  settingsBody: {} as Record<string, unknown>,
  /** saveSettings 收到的 patch 序列(保存链路断言用) */
  savedPatches: [] as Array<Record<string, unknown>>,
  /** true 时 saveSettings 抛错(保存失败回滚场景) */
  saveFail: false,
}));

vi.mock('../api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('../api')>();
  return {
    ...orig,
    getSettings: vi.fn(async () => h.settingsBody),
    saveSettings: vi.fn(async (patch: Record<string, unknown>) => {
      h.savedPatches.push(patch);
      if (h.saveFail) throw new Error('服务端错误,请查看日志');
      return { settings: h.settingsBody };
    }),
  };
});

beforeEach(() => {
  memStorage.clear();
  setActivePinia(createPinia());
  h.settingsBody = {};
  h.savedPatches = [];
  h.saveFail = false;
});

/** 取最后一次保存的 patch(不存在即断言失败并回退空对象——避免非空断言,ratchet 只降不升) */
function lastSavedPatch(): Record<string, unknown> {
  const patch = h.savedPatches.at(-1);
  expect(patch).toBeDefined();
  return patch ?? {};
}

describe('批量保存按模式过滤(TM-SET-3)', () => {
  it('角色扮演模式:patch 含聊天专属项(最大上下文窗口/压缩/记忆)与全局项', async () => {
    const { saveParamsNow } = useGenerationParams();
    await saveParamsNow();
    const patch = lastSavedPatch();
    expect(patch.max_context_tokens, '角色扮演保留最大上下文窗口').toBeDefined();
    expect(patch.compaction_mode, '角色扮演保留压缩模式').toBeDefined();
    expect(patch.memory_inject_limit, '角色扮演保留记忆注入条数').toBeDefined();
    // 剧情推演词条同步两档(RPFLOW-2):deep/agent 流程专属,同随记忆组保留
    expect(patch.worldbook_sync_character_enabled, '角色扮演保留角色词条同步开关').toBeDefined();
    expect(patch.worldbook_sync_global_enabled, '角色扮演保留全局词条同步开关').toBeDefined();
    // 全局字段(直写扁平)两模式都携带
    expect(patch.subagent_max_depth).toBeDefined();
    expect(patch.undo_enabled).toBeDefined();
  });

  it('任务模式:patch 不含聊天专属项(隐藏项不写覆盖层),全局与任务项照常', async () => {
    const store = useAppStore();
    store.appMode = 'task';
    const { saveParamsNow } = useGenerationParams();
    await saveParamsNow();
    const patch = lastSavedPatch();
    expect(patch.max_context_tokens, '任务模式不得写最大上下文窗口').toBeUndefined();
    expect(patch.compaction_mode, '任务模式不得写压缩模式').toBeUndefined();
    expect(patch.compaction_snip_bytes).toBeUndefined();
    expect(patch.memory_inject_limit, '任务模式不得写记忆注入条数').toBeUndefined();
    expect(patch.memory_distill_enabled).toBeUndefined();
    expect(patch.worldbook_sync_character_enabled, '任务模式不得写词条同步开关').toBeUndefined();
    expect(patch.worldbook_sync_global_enabled, '任务模式不得写词条同步开关').toBeUndefined();
    // 全局字段与任务侧项保留
    expect(patch.subagent_max_depth).toBeDefined();
    expect(patch.undo_enabled).toBeDefined();
    expect(patch.default_temperature).toBeDefined();
    expect(patch.task_step_budget_secs).toBeDefined();
    // 任务级总预算(PRODCAP-2):装配漏一处即类型/UI 断裂,在此钉住
    expect(patch.task_total_budget_secs).toBeDefined();
    // 团队任务总预算(TMR3-4 / TPROF-1):同款装配钉住;默认 = 0(不限)
    expect(patch.task_team_budget_secs).toBe(0);
  });

  it('团队预算保存前归一:>1200 写 0(不限),1..599 写 600', async () => {
    const store = useAppStore();
    const { saveParamsNow } = useGenerationParams();

    store.taskTeamBudgetSecs = 1300;
    await saveParamsNow();
    expect(lastSavedPatch().task_team_budget_secs, '超过 1200 = 不限').toBe(0);

    store.taskTeamBudgetSecs = 300;
    await saveParamsNow();
    expect(lastSavedPatch().task_team_budget_secs, '低于 600 抬到下限').toBe(600);
  });
});

describe('任务参数档位应用(TPROF-1)', () => {
  it('任务模式:一次 patch 写入完整档位,响应回填后识别为极限档', async () => {
    const store = useAppStore();
    store.appMode = 'task';
    const extreme = TASK_PARAM_TIERS.extreme;
    h.settingsBody = {
      default_temperature: extreme.temperature,
      default_top_p: extreme.topP,
      default_max_tokens: extreme.maxTokens,
      max_flow_call_depth: extreme.maxFlowCallDepth,
      max_flow_calls_per_task: extreme.maxFlowCallsPerTask,
      default_node_max_context: extreme.defaultNodeMaxContext,
      tool_history_keep_rounds: extreme.toolHistoryKeepRounds,
      tool_history_budget_tokens: extreme.toolHistoryBudgetTokens,
      subagent_max_depth: extreme.subagentMaxDepth,
      subagent_max_concurrency: extreme.subagentMaxConcurrency,
      subagent_result_max_chars: extreme.subagentResultMaxChars,
      task_step_budget_secs: extreme.taskStepBudgetSecs,
      task_total_budget_secs: extreme.taskTotalBudgetSecs,
      task_team_budget_secs: extreme.taskTeamBudgetSecs,
      task_idle_timeout_secs: extreme.taskIdleTimeoutSecs,
    };
    const { applyTaskTier, currentTaskTier } = useGenerationParams();
    await applyTaskTier('extreme');
    const patch = lastSavedPatch();
    expect(patch.default_max_tokens).toBe(65536);
    expect(patch.max_flow_call_depth).toBe(5);
    expect(patch.max_flow_calls_per_task).toBe(64);
    expect(patch.tool_history_keep_rounds).toBe(16);
    expect(patch.tool_history_budget_tokens).toBe(131072);
    expect(patch.subagent_max_depth).toBe(4);
    expect(patch.subagent_max_concurrency).toBe(16);
    expect(patch.task_team_budget_secs).toBe(0);
    expect(currentTaskTier.value).toBe('extreme');
  });

  it('角色扮演模式拒绝应用档位且不发保存请求', async () => {
    const store = useAppStore();
    store.appMode = 'roleplay';
    const { applyTaskTier, taskRecMsg } = useGenerationParams();
    await applyTaskTier('task');
    expect(h.savedPatches).toHaveLength(0);
    expect(taskRecMsg.value).toContain('仅任务模式');
  });
});

describe('undo_enabled 装配(批次 6.1b)', () => {
  it('装载:loadSettings 回填 undo_enabled;保存参数 patch 携带 undo_enabled', async () => {
    const store = useAppStore();
    // 缺省字段回退默认 true(后端默认开)
    await store.loadSettings();
    expect(store.undoEnabled).toBe(true);

    // 服务端返回 false 时回填 false
    h.settingsBody = { undo_enabled: false };
    await store.loadSettings();
    expect(store.undoEnabled).toBe(false);

    // 生成参数保存:patch 携带当前 undo_enabled
    const { saveParamsNow } = useGenerationParams();
    await saveParamsNow();
    const last = h.savedPatches.at(-1);
    expect(last).toBeDefined();
    expect(last!.undo_enabled, '保存 patch 应携带当前 undo_enabled').toBe(false);
  });

  it('数据管理开关:saveUndoEnabled 立即保存;失败回滚本地开关', async () => {
    const store = useAppStore();
    const { undoEnabled, undoMsg, saveUndoEnabled } = useDataManager();

    // 关闭开关 → 保存 patch 只含 undo_enabled: false
    // (mock 响应体同步携带 undo_enabled: false:saveSettings 会用响应回填 store)
    h.settingsBody = { undo_enabled: false };
    undoEnabled.value = false;
    await saveUndoEnabled();
    expect(h.savedPatches.at(-1)).toEqual({ undo_enabled: false });
    expect(undoMsg.value).toContain('已关闭');
    expect(store.undoEnabled, '保存成功不回滚').toBe(false);

    // 保存失败 → 本地开关回滚,提示失败
    h.saveFail = true;
    undoEnabled.value = true;
    await saveUndoEnabled();
    expect(undoEnabled.value, '保存失败应回滚本地开关').toBe(false);
    expect(undoMsg.value).toContain('保存失败');
  });
});
