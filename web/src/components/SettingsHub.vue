<script setup lang="ts">
// 综合设置弹窗:左侧分类导航 + 右侧内容区(嵌入 SettingsModal 全部功能)
// 整合原左侧栏 9 个工具按钮的功能入口
import { ref, computed, watch } from 'vue';
import { useAppStore } from '../store';
import SettingsModal from './SettingsModal.vue';

const store = useAppStore();

const close = (): void => {
  store.settingsOpen = false;
};

/** 导航分类(图标用 .sv-supreme 几何色块,与至上主义黑白风格一致) */
const allSections = [
  { key: 'api', label: 'API 连接', dot: 'red' },
  { key: 'model', label: '模型与生成', dot: 'orange' },
  { key: 'agent', label: 'Agent 设置', dot: 'blue' },
  { key: 'prompt', label: '提示词注入', dot: 'orange' },
  { key: 'flow', label: '执行流程', dot: 'orange' },
  { key: 'preset', label: '预设导入', dot: 'yellow' },
  { key: 'data', label: '数据管理', dot: 'blue' },
  { key: 'ui', label: '界面', dot: 'yellow' },
] as const;

type SectionKey = (typeof allSections)[number]['key'];

/** 角色扮演专属分区:任务模式下隐藏 */
const ROLEPLAY_ONLY: ReadonlySet<string> = new Set(['prompt', 'flow', 'preset']);

/** 按顶层模式过滤:任务模式隐藏角色扮演专属分区(提示词注入/执行流程/预设导入) */
const sections = computed(() =>
  store.appMode === 'task' ? allSections.filter((s) => !ROLEPLAY_ONLY.has(s.key)) : allSections,
);

const activeSection = ref<SectionKey>('api');

// 切换模式后若当前激活分区已被隐藏,回退到第一个可见分区
watch(
  () => store.appMode,
  () => {
    const keys = sections.value.map((s) => s.key);
    if (!keys.includes(activeSection.value)) {
      activeSection.value = keys[0] ?? 'api';
    }
  },
);

/** 快速操作(原工具条功能;统一黑色方块区分层级) */
const quickActions = [
  { key: 'upload', label: '上传角色卡', dot: '', action: () => document.querySelector<HTMLInputElement>('.sv-sidebar input[type=file]')?.click() },
  { key: 'records', label: '聊天记录', dot: '', action: () => { store.chatRecordsOpen = true; close(); } },
  { key: 'worldbooks', label: '世界书', dot: '', action: () => { store.worldBooksOpen = true; close(); } },
  { key: 'contracts', label: '契约编辑', dot: '', action: () => { store.contractsOpen = true; close(); } },
  { key: 'plugins', label: '插件', dot: '', action: () => { store.pluginsOpen = true; close(); } },
  { key: 'skills', label: '技能库', dot: '', action: () => { store.skillsOpen = true; close(); } },
  { key: 'prompts', label: '提示词管理', dot: '', action: () => { store.promptsOpen = true; close(); } },
  { key: 'scripts', label: '脚本管理', dot: '', action: () => { store.scriptsOpen = true; close(); } },
  { key: 'quickReplies', label: '快速回复', dot: '', action: () => { store.quickRepliesOpen = true; close(); } },
  { key: 'macros', label: '宏调试', dot: '', action: () => { store.macrosOpen = true; close(); } },
  { key: 'events', label: '事件监控', dot: '', action: () => { store.eventsOpen = true; close(); } },
  { key: 'optimize', label: '优化面板', dot: '', action: () => { store.optimizeOpen = true; close(); } },
  { key: 'clear', label: '清空会话', dot: '', action: () => { if (confirm('确定清空当前会话?')) void store.clearCurrentChat(); close(); } },
];

/** 当前分类标题 */
const activeLabel = computed(() => sections.value.find((s) => s.key === activeSection.value)?.label ?? '');
</script>

<template>
  <div class="sv-modal-mask" @click.self="close">
    <div class="sv-modal sv-hub-modal">
      <div class="sv-modal-head">
        <h2 class="flex items-center gap-2">
          <span class="sv-supreme pink" /> 综合设置
        </h2>
        <button class="sv-btn ghost sv-btn-square" @click="close">✕</button>
      </div>

      <div class="sv-hub-body">
        <!-- 左侧导航 -->
        <div class="sv-hub-nav">
          <button
            v-for="(s, idx) in sections"
            :key="s.key"
            class="sv-hub-nav-item"
            :class="{ active: activeSection === s.key }"
            @click="activeSection = s.key"
          >
            <span class="sv-hub-nav-idx">{{ String(idx + 1).padStart(2, '0') }}</span>
            <span class="sv-supreme" :class="s.dot" style="width: 10px; height: 10px; flex: none" />
            <span>{{ s.label }}</span>
          </button>

          <!-- 快速操作分隔(构成主义双细线) -->
          <div class="sv-separator tight">
            <div style="font-size: var(--text-2xs); color: var(--sv-ink-faint); letter-spacing: 0.1em; padding: 0 12px 6px">快速操作</div>
            <button
              v-for="a in quickActions"
              :key="a.key"
              class="sv-hub-nav-item"
              style="font-size: var(--text-sm)"
              @click="a.action"
            >
              <span class="sv-supreme" :class="a.dot" style="width: 10px; height: 10px; flex: none" />
              <span>{{ a.label }}</span>
            </button>
          </div>
        </div>

        <!-- 右侧内容区:嵌入 SettingsModal(完整设置功能) -->
        <div class="sv-hub-content">
          <h3 class="sv-hub-content-title">{{ activeLabel }}</h3>

          <!-- 嵌入原 SettingsModal 全部内容 -->
          <SettingsModal embedded :active-section="activeSection" />
        </div>
      </div>
    </div>
  </div>
</template>
