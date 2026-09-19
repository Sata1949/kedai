// Agent 设置:系统提示词 / 搜索端点 / 变量状态注入位置 / 反思提示词的编辑与保存。
import { ref } from 'vue';
import { useAppStore } from '../store';
import { storeToRefs } from 'pinia';

export function useAgentSettings() {
  const store = useAppStore();
  const { agentSystemPrompt, searchEndpoint, mvuVarsPosition, reflectPrompt, taskPersonaFull, taskPromptInjectEnabled, mvuModel, mvuTemperatureInput } =
    storeToRefs(store);

  const agentSaving = ref(false);
  const agentMsg = ref('');

  /** 恢复内置默认提示词(清空 = 服务端用内置默认)并保存 */
  async function resetAgentPrompt(): Promise<void> {
    agentSystemPrompt.value = '';
    await saveAgentNow();
  }

  async function saveAgentNow(): Promise<void> {
    if (agentSaving.value) return;
    agentSaving.value = true;
    agentMsg.value = '';
    try {
      await store.saveSettings({
        agent_system_prompt: agentSystemPrompt.value,
        search_endpoint: searchEndpoint.value,
        mvu_vars_position: mvuVarsPosition.value,
        // HB-7:变量两步生成的独立模型/温度(空模型 = 与正文共用;空温度 = 内置 0.3;
        // 清除用哨兵值表达,与后端 PATCH 语义一致)
        mvu_model: mvuModel.value.trim(),
        mvu_temperature: mvuTemperatureInput.value.trim() === '' ? -1 : Number(mvuTemperatureInput.value),
        reflect_prompt: reflectPrompt.value,
        // 执行者人设精简/完整(R3a):随 Agent 设置保存链路持久化,仅任务模式生效
        task_persona_full: taskPersonaFull.value,
        // 任务模式提示词注入继承开关(2026-09-10 实跑修复):默认隔离,仅任务模式生效
        task_prompt_inject_enabled: taskPromptInjectEnabled.value,
      });
      agentMsg.value = 'Agent 设置已保存';
      setTimeout(() => (agentMsg.value = ''), 2500);
    } catch (e) {
      agentMsg.value = `保存失败:${(e as Error).message}`;
    } finally {
      agentSaving.value = false;
    }
  }

  return { agentSaving, agentMsg, resetAgentPrompt, saveAgentNow };
}
