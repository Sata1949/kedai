<script setup lang="ts">
// 侧栏顶部:品牌 / 连接状态 / Token 统计 / 顶层模式切换（C-2A，2026-10-25）。
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';

const store = useAppStore();
const {
  sessionTotalTokens, globalTotalTokens,
  appMode, currentTaskUsage, globalTaskUsage,
} = storeToRefs(store);

const connColor = computed(() => store.connStatus);
const connLabel = computed(() => {
  switch (store.connStatus) {
    case 'ok': return '已连接';
    case 'fail': return '未连接';
    default: return '检测中';
  }
});

/** 任务模式统计块数据源:当前任务/全部任务累计 token(prompt + completion)。 */
const currentTaskTotalTokens = computed(() => {
  const u = currentTaskUsage.value;
  return u ? u.prompt_tokens + u.completion_tokens : 0;
});
const globalTaskTotalTokens = computed(() => {
  const u = globalTaskUsage.value;
  return u ? u.prompt_tokens + u.completion_tokens : 0;
});

</script>

<template>
  <!-- 顶部:品牌 + Token 状态区 -->
  <div class="sv-side-head">
    <div class="sv-brand">
      <img class="logo" src="/logo.png" alt="Kedai" draggable="false" />
      <h1>KEDAI</h1>
      <span class="sv-supreme sm" aria-hidden="true" />
      <span class="sv-conn sv-ml-auto">
        <span class="dot" :class="connColor" />
        {{ connLabel }}
      </span>
      <!-- 抽屉关闭键:仅移动端显示(桌面端由 CSS 隐藏) -->
      <button
        class="sv-sidebar-close"
        title="收起角色库"
        aria-label="收起角色库"
        @click="store.sidebarOpen = false"
      >✕</button>
    </div>

    <!-- 侧栏只放累计用量；上下文、本轮和命中率统一由聊天顶栏展示。 -->
    <div class="sv-tokens two">
      <template v-if="appMode === 'task'">
        <div class="sv-token-cell">
          <span class="sv-supreme pink" />
          <div>
            <b class="sv-tnum">{{ currentTaskTotalTokens.toLocaleString() }}</b>
            <span>当前任务累计</span>
          </div>
        </div>
        <div class="sv-token-cell">
          <span class="sv-supreme pink-deep" />
          <div>
            <b class="sv-tnum">{{ globalTaskTotalTokens.toLocaleString() }}</b>
            <span>全部任务累计</span>
          </div>
        </div>
      </template>
      <template v-else>
        <div class="sv-token-cell">
          <span class="sv-supreme pink" />
          <div>
            <b class="sv-tnum">{{ sessionTotalTokens.toLocaleString() }}</b>
            <span>本会话累计</span>
          </div>
        </div>
        <div class="sv-token-cell">
          <span class="sv-supreme pink-deep" />
          <div>
            <b class="sv-tnum">{{ globalTotalTokens.toLocaleString() }}</b>
            <span>全局累计</span>
          </div>
        </div>
      </template>
    </div>
  </div>

  <!-- 顶层模式切换:角色扮演 / 任务 -->
  <div class="sv-appmode">
    <button
      :class="{ active: appMode === 'roleplay' }"
      title="角色扮演模式:与角色进行沉浸式对话与文学创作"
      @click="store.setAppMode('roleplay')"
    >
      <span class="sv-appmode-idx sv-tnum">01</span>角色扮演
    </button>
    <button
      :class="{ active: appMode === 'task' }"
      title="任务模式:下达目标,自动拆解计划并派子智能体执行"
      @click="store.setAppMode('task')"
    >
      <span class="sv-appmode-idx sv-tnum">02</span>任务
    </button>
  </div>
</template>
