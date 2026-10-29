<script setup lang="ts">
// 设置内容壳:裸内容区(供 SettingsHub 内容区嵌入,按 activeSection 切换显示)。
// IMPHUB 步骤 E:standalone 模态分支已删(生产无入口,原为旧深链接兜底),只保留 embedded。
// 分区组件(src/components/settings/)共享同一壳,业务逻辑在 src/composables/。
// 历史背景:原实现 embedded / standalone 两套模板复制粘贴且已漂移(standalone 缺失
// 预设导入导出、提示词预览、工具策略行与压缩参数),现已合并为单组共享 section。
// IMPHUB 步骤 C:「预设导入 / 导出」区整区删除,能力并入「综合导入 / 导出」弹窗。
// 分区懒加载(M2-C5):默认首 tab(API 连接)两个分区同步导入(打开即见);其余分区
// 经 lazyModal 拆 chunk,首次切到对应 tab 才挂载加载,挂载后常驻不丢编辑态;
// 加载失败语义同弹窗(自动重试一次 → 全局错误条手动重试,重开综合设置触发重新加载)。
import { ref, watch } from 'vue';
import { lazyModal } from '../asyncModal';
import { useApiSettings } from '../composables/useApiSettings';
import { useConnectionProfiles } from '../composables/useConnectionProfiles';
import { usePromptInject } from '../composables/usePromptInject';
import { useDataManager } from '../composables/useDataManager';
import ApiSettingsSection from './settings/ApiSettingsSection.vue';
import ConnectionSection from './settings/ConnectionSection.vue';

const GenParamsSection = lazyModal(() => import('./settings/GenParamsSection.vue'), '设置区:模型与生成', 'settingsOpen');
// 多套连接配置(批次 4):与「API 连接」区同属连接域,但管理的是连接列表
const ConnectionProfilesSection = lazyModal(() => import('./settings/ConnectionProfilesSection.vue'), '设置区:连接配置', 'settingsOpen');
const AgentSettingsSection = lazyModal(() => import('./settings/AgentSettingsSection.vue'), '设置区:Agent 设置', 'settingsOpen');
const AgentFlowSection = lazyModal(() => import('./settings/AgentFlowSection.vue'), '设置区:执行流程', 'settingsOpen');
// 授权与命令执行(2026-09-17 独立分区):工具三档授权 + 命令执行/设备档位
const ExecAuthSection = lazyModal(() => import('./settings/ExecAuthSection.vue'), '设置区:授权与命令执行', 'settingsOpen');
// 编码能力包(默认关):任务模式执行者默认系统提示词模板开关
const CodingBundleSection = lazyModal(() => import('./settings/CodingBundleSection.vue'), '设置区:编码能力包', 'settingsOpen');
// 文学能力包(默认关;LIT-1):角色扮演侧 / 任务侧各一个开关
const LiteraryBundleSection = lazyModal(() => import('./settings/LiteraryBundleSection.vue'), '设置区:文学能力包', 'settingsOpen');
// 视觉与截图(默认关;视觉能力包):截图工具总开关(隐私提示见分区内)
const VisionScreenshotSection = lazyModal(() => import('./settings/VisionScreenshotSection.vue'), '设置区:视觉与截图', 'settingsOpen');
const PromptInjectSection = lazyModal(() => import('./settings/PromptInjectSection.vue'), '设置区:提示词注入', 'settingsOpen');
// 用户设定(USERPERSONA-1):角色扮演专属的全局用户角色配置
const UserPersonaSection = lazyModal(() => import('./settings/UserPersonaSection.vue'), '设置区:用户设定', 'settingsOpen');
const DataManagementSection = lazyModal(() => import('./settings/DataManagementSection.vue'), '设置区:数据管理', 'settingsOpen');
const McpSection = lazyModal(() => import('./settings/McpSection.vue'), '设置区:MCP 服务', 'settingsOpen');
const UiSection = lazyModal(() => import('./settings/UiSection.vue'), '设置区:界面', 'settingsOpen');
const EmbeddingSection = lazyModal(() => import('./settings/EmbeddingSection.vue'), '设置区:向量化模型', 'settingsOpen');
const AboutSection = lazyModal(() => import('./settings/AboutSection.vue'), '设置区:关于', 'settingsOpen');

const props = withDefaults(defineProps<{
  activeSection?: string;
}>(), {
  activeSection: 'api',
});

// ===== 跨 section 共享的业务状态(在壳创建一次,经 prop 传入,避免重复实例化导致状态分叉) =====
// API 设置区 + 后端连接区共享
const apiSettings = useApiSettings();
// 连接配置区(多套连接)独占一份列表状态
const connectionProfiles = useConnectionProfiles();
// 提示词注入区 + 预设导入导出区共享
const promptInject = usePromptInject();
// 数据管理区 + 界面区共享
const dataManager = useDataManager();

/**
 * 懒加载门禁:记录已访问分区,首次切到才挂载对应异步分区(触发 chunk 加载);
 * 挂载后常驻(:show 切显隐),切 tab 不丢分区本地编辑态。
 */
const visitedSections = ref<ReadonlySet<string>>(new Set([props.activeSection]));
watch(
  () => props.activeSection,
  (s) => {
    if (!visitedSections.value.has(s)) {
      visitedSections.value = new Set(visitedSections.value).add(s);
    }
  },
);

</script>

<template>
  <!-- 仅渲染内容区,供 SettingsHub 嵌入(按 activeSection 切换显示;
       懒加载分区首次切到才挂载,挂载后常驻由 :show 控制显隐) -->
  <div class="sv-settings-embedded">
    <ApiSettingsSection :state="apiSettings" :show="props.activeSection === 'api'" />
    <ConnectionSection :state="apiSettings" :show="props.activeSection === 'api'" />
    <ConnectionProfilesSection
      v-if="visitedSections.has('connections')"
      :state="connectionProfiles"
      :show="props.activeSection === 'connections'"
    />
    <GenParamsSection v-if="visitedSections.has('model')" :show="props.activeSection === 'model'" />
    <AgentSettingsSection v-if="visitedSections.has('agent')" :show="props.activeSection === 'agent'" />
    <AgentFlowSection v-if="visitedSections.has('flow')" :show="props.activeSection === 'flow'" />
    <ExecAuthSection v-if="visitedSections.has('exec')" :show="props.activeSection === 'exec'" />
    <CodingBundleSection v-if="visitedSections.has('coding')" :show="props.activeSection === 'coding'" />
    <LiteraryBundleSection v-if="visitedSections.has('literary')" :show="props.activeSection === 'literary'" />
    <VisionScreenshotSection v-if="visitedSections.has('vision')" :show="props.activeSection === 'vision'" />
    <PromptInjectSection v-if="visitedSections.has('prompt')" :state="promptInject" :show="props.activeSection === 'prompt'" />
    <UserPersonaSection v-if="visitedSections.has('userPersona')" :show="props.activeSection === 'userPersona'" />
    <DataManagementSection v-if="visitedSections.has('data')" :state="dataManager" :show="props.activeSection === 'data'" />
    <McpSection v-if="visitedSections.has('mcp')" :show="props.activeSection === 'mcp'" />
    <EmbeddingSection v-if="visitedSections.has('embedding')" :show="props.activeSection === 'embedding'" />
    <UiSection v-if="visitedSections.has('ui')" :state="dataManager" :show="props.activeSection === 'ui'" />
    <AboutSection v-if="visitedSections.has('about')" :show="props.activeSection === 'about'" />
  </div>

</template>
