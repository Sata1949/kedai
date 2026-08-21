<script setup lang="ts">
// 设置模态框(壳):embedded 模式裸内容(供 SettingsHub 内容区嵌入,按 activeSection 切换显示),
// standalone 模式遮罩 + 头部 + 内容 + 底部(App.vue 深链接/旧入口兜底)。
// 两个模式共享同一组 section 组件(src/components/settings/),业务逻辑在 src/composables/。
// 历史背景:原实现 embedded / standalone 两套模板复制粘贴且已漂移(standalone 缺失
// 预设导入导出、提示词预览、工具策略行与压缩参数),现已合并为单组共享 section。
import { useAppStore } from '../store';
import { useApiSettings } from '../composables/useApiSettings';
import { usePromptInject } from '../composables/usePromptInject';
import { useDataManager } from '../composables/useDataManager';
import ApiSettingsSection from './settings/ApiSettingsSection.vue';
import ConnectionSection from './settings/ConnectionSection.vue';
import GenParamsSection from './settings/GenParamsSection.vue';
import AgentSettingsSection from './settings/AgentSettingsSection.vue';
import AgentFlowSection from './settings/AgentFlowSection.vue';
import PromptInjectSection from './settings/PromptInjectSection.vue';
import PresetImportExportSection from './settings/PresetImportExportSection.vue';
import DataManagementSection from './settings/DataManagementSection.vue';
import UiSection from './settings/UiSection.vue';

const props = withDefaults(defineProps<{
  embedded?: boolean;
  activeSection?: string;
}>(), {
  embedded: false,
  activeSection: 'api',
});

const store = useAppStore();

// ===== 跨 section 共享的业务状态(在壳创建一次,经 prop 传入,避免重复实例化导致状态分叉) =====
// API 设置区 + 后端连接区共享
const apiSettings = useApiSettings();
// 提示词注入区 + 预设导入导出区共享
const promptInject = usePromptInject();
// 数据管理区 + 界面区共享
const dataManager = useDataManager();

const close = (): void => {
  store.settingsOpen = false;
};
</script>

<template>
  <!-- embedded 模式:仅渲染内容区,供 SettingsHub 嵌入(按 activeSection 切换显示) -->
  <div v-if="props.embedded" class="sv-settings-embedded">
    <ApiSettingsSection :state="apiSettings" :show="props.activeSection === 'api'" />
    <ConnectionSection :state="apiSettings" :show="props.activeSection === 'api'" />
    <GenParamsSection :show="props.activeSection === 'model'" />
    <AgentSettingsSection :show="props.activeSection === 'agent'" />
    <AgentFlowSection :show="props.activeSection === 'flow'" />
    <PromptInjectSection :state="promptInject" :show="props.activeSection === 'prompt'" />
    <PresetImportExportSection :state="promptInject" :show="props.activeSection === 'preset'" />
    <DataManagementSection :state="dataManager" :show="props.activeSection === 'data'" />
    <UiSection :state="dataManager" :show="props.activeSection === 'ui'" />
  </div>

  <!-- 独立模态框模式(遮罩 + 头部 + 全部设置区 + 底部) -->
  <div v-else class="sv-modal-mask" @click.self="close">
    <div class="sv-modal">
      <!-- 头部 -->
      <div class="sv-modal-head">
        <h2 class="flex items-center gap-2">
          <span class="sv-supreme pink" /> 设置
        </h2>
        <button class="sv-btn ghost sv-btn-square" @click="close">✕</button>
      </div>

      <div class="sv-modal-body">
        <ApiSettingsSection :state="apiSettings" />
        <ConnectionSection :state="apiSettings" />
        <GenParamsSection />
        <AgentSettingsSection />
        <AgentFlowSection />
        <PromptInjectSection :state="promptInject" />
        <PresetImportExportSection :state="promptInject" />
        <DataManagementSection :state="dataManager" />
        <UiSection :state="dataManager" />
      </div>

      <div class="sv-modal-foot">
        <button class="sv-btn primary" @click="close">完成</button>
      </div>
    </div>
  </div>
</template>
