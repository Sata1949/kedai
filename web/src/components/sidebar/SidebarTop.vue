<script setup lang="ts">
// 侧栏顶部:品牌 / 连接状态 / Token 统计 / 顶层模式切换（C-2A，2026-10-25）。
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import { useAppStore } from '../../store';

const store = useAppStore();
const {
  contextTokens, lastUsage, sessionTotalTokens, globalTotalTokens, cacheZeroStreak,
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

/** 缓存命中率:连续两次为 0 则不显示 */
const hitRate = computed<number | null>(() => {
  if (cacheZeroStreak.value >= 2) return null;
  const u = lastUsage.value;
  if (!u || !u.prompt_tokens) return null;
  const hit = u.prompt_cache_hit_tokens ?? 0;
  return Math.min(100, Math.round((hit / u.prompt_tokens) * 100));
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

    <!-- Token 统计区:角色扮演 = 会话累计/全局累计/上下文/缓存命中率;
         任务模式 = 当前任务累计/全部任务累计(无上下文与命中率数据源,两格隐藏) -->
    <div class="sv-tokens" :class="{ two: appMode === 'task' }">
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
        <div class="sv-token-cell">
          <span class="sv-supreme blue" />
          <div>
            <b class="sv-tnum">{{ contextTokens.toLocaleString() }}</b>
            <span>上下文 TOKEN</span>
          </div>
        </div>
        <div v-if="hitRate !== null" class="sv-token-cell">
          <span class="sv-supreme green" />
          <div>
            <b class="sv-tnum">{{ hitRate }}%</b>
            <span>缓存命中率</span>
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
