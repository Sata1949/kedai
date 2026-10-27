// 数据管理:聊天导入/导出/清空 + 界面项(Agent 面板开关、安全 HTML 渲染、脚本授权)。
import { computed, ref } from 'vue';
import { useAppStore } from '../store';
import { storeToRefs } from 'pinia';
import * as api from '../api';
import {
  buildTaskParamTierPatch,
  identifyTaskParamTier,
  normalizeTaskTeamBudgetSecs,
  TASK_PARAM_TIER_LABELS,
  type TaskParamTier,
  type TaskParamTierValues,
} from '../stores/genSettings';

export function useDataManager() {
  const store = useAppStore();
  const { currentSessionId, undoEnabled } = storeToRefs(store);

  const importInput = ref<HTMLInputElement | null>(null);
  const importError = ref('');
  const clearMsg = ref('');
  /** 导出成功提示(成功 3 秒后自动清除) */
  const exportMsg = ref('');
  /** 回退快照(undo)开关保存反馈(成功/失败,3 秒后自动清除) */
  const undoMsg = ref('');

  /** 切换回退快照(undo)开关:立即保存到服务端设置(批次 6.1b) */
  async function saveUndoEnabled(): Promise<void> {
    undoMsg.value = '';
    try {
      await store.saveSettings({ undo_enabled: undoEnabled.value });
      undoMsg.value = undoEnabled.value ? '已开启回退快照' : '已关闭回退快照';
    } catch (e) {
      // 保存失败:回滚本地开关,避免 UI 与服务端状态分叉
      undoEnabled.value = !undoEnabled.value;
      undoMsg.value = `保存失败:${(e as Error).message}`;
    } finally {
      setTimeout(() => (undoMsg.value = ''), 3000);
    }
  }

  function characterLabel(id: string): string {
    const character = store.characters.find((item) => item.id === id);
    return character?.chara_name ?? character?.name ?? `角色 ${id.slice(0, 8)}`;
  }

  function confirmRevokeScriptAuthorization(characterId: string): void {
    if (confirm(`撤销“${characterLabel(characterId)}”的角色卡 JavaScript 授权？\n\n安全 HTML 渲染不受影响。`)) {
      store.revokeCharacterScripts(characterId);
    }
  }

  async function onImportFile(e: Event): Promise<void> {
    const input = e.target as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    importError.value = '';
    try {
      const messages = JSON.parse(await file.text());
      if (!Array.isArray(messages)) throw new Error('文件须为 SillyTavern 消息数组');
      const sid = currentSessionId.value;
      if (!sid) {
        importError.value = '请先选择一个角色会话';
        return;
      }
      await api.importChat(sid, messages);
      await store.loadHistory(sid);
      alert(`导入成功,共 ${messages.length} 条消息`);
    } catch (err) {
      importError.value = (err as Error).message;
    }
  }

  async function clearAllData(): Promise<void> {
    if (!currentSessionId.value) return;
    if (!confirm('确定清空当前会话全部消息?(角色定义保留)')) return;
    try {
      await store.clearCurrentChat();
      clearMsg.value = '已清空当前会话';
      setTimeout(() => (clearMsg.value = ''), 2500);
    } catch (err) {
      importError.value = (err as Error).message;
    }
  }

  async function exportChat(): Promise<void> {
    if (!currentSessionId.value) {
      importError.value = '请先选择一个角色会话再导出';
      return;
    }
    importError.value = '';
    try {
      const fileName = await store.exportCurrentChat();
      if (fileName === null) return; // 用户在保存对话框中取消
      exportMsg.value = `已导出:${fileName}`;
      setTimeout(() => (exportMsg.value = ''), 3000);
    } catch (e) {
      importError.value = `导出失败:${(e as Error).message}`;
    }
  }

  return {
    importInput, importError, exportMsg, clearMsg, characterLabel, confirmRevokeScriptAuthorization,
    onImportFile, clearAllData, exportChat,
    undoEnabled, undoMsg, saveUndoEnabled,
  };
}

export function useGenerationParams() {
  const store = useAppStore();
  const {
    temperature, topP, maxTokens, maxContextTokens, maxToolRounds,
    maxFlowCallDepth, maxFlowCallsPerTask, defaultNodeMaxContext,
    toolHistoryKeepRounds, toolHistoryBudgetTokens,
    sessionTokenBudget, sessionBudgetAction,
    loopGuardSemanticWindow, loopGuardSemanticMinCalls, loopGuardSemanticMaxDistinct,
    taskStepBudgetSecs, taskTotalBudgetSecs, taskTeamBudgetSecs, taskIdleTimeoutSecs,
    compactionMode, compactionThreshold, compactionKeepRecent, compactionSnipBytes,
    memoryDistillEnabled, memoryInjectLimit, memoryInjectCharBudget, memoryMaxEntries,
    worldbookSyncCharacterEnabled, worldbookSyncGlobalEnabled,
    subagentMaxDepth, subagentMaxConcurrency, subagentResultMaxChars,
    undoEnabled,
  } = storeToRefs(store);

  const tempLabel = computed(() => `${Math.round(temperature.value * 100)}%`);
  const topPLabel = computed(() => `${Math.round(topP.value * 100)}%`);
  const ctxLabel = computed(() =>
    maxContextTokens.value >= 1000 ? `${(maxContextTokens.value / 1000).toFixed(1)}k` : String(maxContextTokens.value),
  );

  const saveParams = ref(false);
  const paramsMsg = ref('');

  async function saveParamsNow(): Promise<void> {
    saveParams.value = true;
    paramsMsg.value = '';
    try {
      // 模式过滤(TM-SET-3):最大上下文窗口/压缩/记忆为聊天专属(任务侧零消费),
      // 任务模式下已隐藏——批量保存不得把它们写进任务覆盖层(否则是永不生效的死写)。
      // 子代理三参数与回退快照为全局字段(直写扁平),两模式都保留在 patch 里。
      const isTask = store.appMode === 'task';
      await store.saveSettings({
        default_temperature: temperature.value,
        default_top_p: topP.value,
        default_max_tokens: maxTokens.value,
        max_tool_rounds: maxToolRounds.value,
        // 流程调用闸与节点默认上下文(A 批 A3/A4;三者都是任务侧设置)
        max_flow_call_depth: maxFlowCallDepth.value,
        max_flow_calls_per_task: maxFlowCallsPerTask.value,
        default_node_max_context: defaultNodeMaxContext.value,
        tool_history_keep_rounds: toolHistoryKeepRounds.value,
        tool_history_budget_tokens: toolHistoryBudgetTokens.value,
        session_token_budget: sessionTokenBudget.value,
        session_budget_action: sessionBudgetAction.value,
        loop_guard_semantic_window: loopGuardSemanticWindow.value,
        loop_guard_semantic_min_calls: loopGuardSemanticMinCalls.value,
        loop_guard_semantic_max_distinct: loopGuardSemanticMaxDistinct.value,
        task_step_budget_secs: taskStepBudgetSecs.value,
        task_total_budget_secs: taskTotalBudgetSecs.value,
        task_team_budget_secs: normalizeTaskTeamBudgetSecs(taskTeamBudgetSecs.value),
        task_idle_timeout_secs: taskIdleTimeoutSecs.value,
        subagent_max_depth: subagentMaxDepth.value,
        subagent_max_concurrency: subagentMaxConcurrency.value,
        subagent_result_max_chars: subagentResultMaxChars.value,
        undo_enabled: undoEnabled.value,
        ...(isTask
          ? {}
          : {
              max_context_tokens: maxContextTokens.value,
              compaction_mode: compactionMode.value,
              compaction_threshold: compactionThreshold.value,
              compaction_keep_recent: compactionKeepRecent.value,
              compaction_snip_bytes: compactionSnipBytes.value,
              memory_distill_enabled: memoryDistillEnabled.value,
              memory_inject_limit: memoryInjectLimit.value,
              memory_inject_char_budget: memoryInjectCharBudget.value,
              memory_max_entries: memoryMaxEntries.value,
              // 剧情推演词条同步两档(RPFLOW-2):deep/agent 流程专属(聊天侧),同随记忆组过滤
              worldbook_sync_character_enabled: worldbookSyncCharacterEnabled.value,
              worldbook_sync_global_enabled: worldbookSyncGlobalEnabled.value,
            }),
      });
      paramsMsg.value = '已保存为默认生成参数';
      setTimeout(() => (paramsMsg.value = ''), 2500);
    } catch (e) {
      paramsMsg.value = `保存失败:${(e as Error).message}`;
    } finally {
      saveParams.value = false;
    }
  }

  /** 当前任务参数(字段与档位定义一一对应;团队预算先按服务端语义归一) */
  const taskTierValues = computed<TaskParamTierValues>(() => ({
    temperature: temperature.value,
    topP: topP.value,
    maxTokens: maxTokens.value,
    maxFlowCallDepth: maxFlowCallDepth.value,
    maxFlowCallsPerTask: maxFlowCallsPerTask.value,
    defaultNodeMaxContext: defaultNodeMaxContext.value,
    toolHistoryKeepRounds: toolHistoryKeepRounds.value,
    toolHistoryBudgetTokens: toolHistoryBudgetTokens.value,
    subagentMaxDepth: subagentMaxDepth.value,
    subagentMaxConcurrency: subagentMaxConcurrency.value,
    subagentResultMaxChars: subagentResultMaxChars.value,
    taskStepBudgetSecs: taskStepBudgetSecs.value,
    taskTotalBudgetSecs: taskTotalBudgetSecs.value,
    taskTeamBudgetSecs: normalizeTaskTeamBudgetSecs(taskTeamBudgetSecs.value),
    taskIdleTimeoutSecs: taskIdleTimeoutSecs.value,
  }));

  /** 当前识别到的档位;手动改过任一项即 custom(不猜、不自动写回) */
  const currentTaskTier = computed(() => identifyTaskParamTier(taskTierValues.value));

  /** 应用任务参数档位(TPROF-1):一次整批写 task 覆盖层 + 全局扁平字段。
   *  走 `queueSettingsSave` 与自动保存串行,避免响应交错回填旧值。 */
  const taskRecSaving = ref(false);
  const taskRecMsg = ref('');
  async function applyTaskTier(tier: TaskParamTier): Promise<void> {
    if (store.appMode !== 'task') {
      taskRecMsg.value = '仅任务模式可应用任务参数档位';
      return;
    }
    taskRecSaving.value = true;
    taskRecMsg.value = '';
    try {
      await store.queueSettingsSave(buildTaskParamTierPatch(tier));
      taskRecMsg.value = `已应用${TASK_PARAM_TIER_LABELS[tier]}`;
      setTimeout(() => (taskRecMsg.value = ''), 3000);
    } catch (e) {
      taskRecMsg.value = `应用失败:${(e as Error).message}`;
    } finally {
      taskRecSaving.value = false;
    }
  }

  return {
    tempLabel, topPLabel, ctxLabel, saveParams, paramsMsg, saveParamsNow,
    taskRecSaving, taskRecMsg, currentTaskTier, applyTaskTier,
  };
}
