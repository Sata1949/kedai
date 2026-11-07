<script setup lang="ts">
// 设置区:后端连接(连接器信息 + 测试连接 + 模型计数)。
// 与 ApiSettingsSection 共享壳传入的同一份 useApiSettings 状态。
import type { useApiSettings } from '../../composables/useApiSettings';
import SkeletonBlock from '../SkeletonBlock.vue';

const props = withDefaults(defineProps<{
  /** useApiSettings 的返回对象(壳共享实例) */
  state: ReturnType<typeof useApiSettings>;
  /** 是否显示(embedded 模式按 activeSection 切换;standalone 恒 true) */
  show?: boolean;
}>(), {
  show: true,
});

const {
  connInfo, models, connecting, connFeedback, modelFeedback, doConnect,
  loadingConnection, connectionLoadError, modelsLoading, loadApiSettings,
} = props.state;
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme orange" /> 后端连接</div>
    <div class="sv-stack">
      <!-- 占位只表示正在加载；空列表与失败都有明确终态，不用内容是否为空代替加载标志。 -->
      <div class="sv-inp-row">
        <span class="sv-inp-tag">CONN</span>
        <code v-if="connInfo" class="sv-code">{{ connInfo.connector }}</code>
        <SkeletonBlock v-else-if="loadingConnection" :lines="1" width="140px" />
        <span v-else class="sv-note">连接信息不可用</span>
      </div>
      <button class="sv-btn ghost sv-btn-fill" :disabled="connecting" @click="doConnect">
        {{ connecting ? '连接测试中...' : '测试连接' }}
      </button>
      <SkeletonBlock v-if="modelsLoading && connInfo" :lines="1" width="96px" />
      <div v-else-if="models.length" class="sv-count">可用 {{ models.length }} 个模型</div>
      <p v-else-if="!modelsLoading && !connectionLoadError" class="sv-note">暂无可用模型，可点击「从 API 加载模型列表」重试。</p>
    </div>
    <div v-if="connectionLoadError" class="sv-feedback err" role="alert">
      {{ connectionLoadError }}
      <button class="sv-btn ghost sv-btn-sm" :disabled="loadingConnection" @click="loadApiSettings">重试</button>
    </div>
    <div v-if="modelFeedback" class="sv-feedback info">{{ modelFeedback }}</div>
    <div v-if="connFeedback" class="sv-feedback" :class="connFeedback.kind">{{ connFeedback.text }}</div>
  </div>
</template>
