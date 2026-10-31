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
} = props.state;
</script>

<template>
  <div v-show="props.show" class="sv-field">
    <div class="sv-field-label"><span class="sv-supreme orange" /> 后端连接</div>
    <div class="sv-stack">
      <!-- UIVIS-3/UI-C1:改前加载态用裸文本「...」占位(首开设置约 1.5s),读起来像 bug。
           换成已有的 SkeletonBlock(UIP-13)。宽度按内容估:连接器名最长十余字符,
           140px 足够;模型计数那行文案短,96px。加载完成即换回真值。
           第二处只在 connInfo 已到、模型列表未到时才出骨架——两者都空时不并排两块骨架。 -->
      <div class="sv-inp-row">
        <span class="sv-inp-tag">CONN</span>
        <code v-if="connInfo" class="sv-code">{{ connInfo.connector }}</code>
        <SkeletonBlock v-else :lines="1" width="140px" />
      </div>
      <button class="sv-btn ghost sv-btn-fill" :disabled="connecting" @click="doConnect">
        {{ connecting ? '连接测试中...' : '测试连接' }}
      </button>
      <div v-if="models.length" class="sv-count">可用 {{ models.length }} 个模型</div>
      <SkeletonBlock v-else-if="connInfo" :lines="1" width="96px" />
    </div>
    <div v-if="modelFeedback" class="sv-feedback info">{{ modelFeedback }}</div>
    <div v-if="connFeedback" class="sv-feedback" :class="connFeedback.kind">{{ connFeedback.text }}</div>
  </div>
</template>
